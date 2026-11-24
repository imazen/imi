//! Exact rendered-sample export through the typed zenpng encoder.
use crate::TranscodeError;
use enough::{Stop, Unstoppable};
use imij::ImiEncoderConfig;
use imij::codec::{DecodeConfig, DecodedFrameMetadata, DecodedMetadata};
use imij::{Error, ImageInfo, Metadata, SampleType};
use std::borrow::Cow;
use zencodec::decode::{AnimationFrameDecoder, Decode, DecodeJob, DecoderConfig};
use zencodec::encode::{AnimationFrameEncoder, EncodeJob, Encoder, EncoderConfig};
use zencodec::{ColorEmitPolicy, MetadataPolicy, ResourceLimits, StopToken};
use zenpixels::PixelSlice;

/// Reusable PNG compression settings for exact U8/U16 image export.
/// Samples are never quantized, color-converted, unpremultiplied or rescaled.
#[derive(Clone)]
pub struct ExportConfig {
    compression: zenpng::Compression,
    decode: DecodeConfig,
}
impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            compression: zenpng::Compression::Fast,
            decode: DecodeConfig::default(),
        }
    }
}
impl ExportConfig {
    pub fn with_compression(mut self, compression: zenpng::Compression) -> Self {
        self.compression = compression;
        self
    }
    /// Native cumulative work/metadata admission remains active during decoding.
    pub fn with_decode(mut self, decode: DecodeConfig) -> Self {
        self.decode = decode;
        self
    }
    pub fn job(self) -> ExportJob {
        ExportJob {
            config: self,
            limits: ResourceLimits::default(),
            stop: None,
        }
    }
}

/// Per-export limits and cancellation. Complete input and complete PNG output;
/// animation encoding retains all composited frames in zenpng.
pub struct ExportJob {
    config: ExportConfig,
    limits: ResourceLimits,
    stop: Option<StopToken>,
}
impl ExportJob {
    pub fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = limits;
        self
    }
    pub fn with_stop(mut self, stop: StopToken) -> Self {
        self.stop = Some(stop);
        self
    }
    /// Borrow an IMI still or animation. Pages require a separate export per page.
    /// A combined memory cap is refused: neither complete-output buffering nor
    /// the composed codecs currently supplies a combined working-memory bound.
    pub fn exporter(self, data: &[u8]) -> Result<Exporter<'_>, TranscodeError> {
        self.check()?;
        self.limits.check_input_size(data.len() as u64)?;
        if self.limits.max_memory_bytes.is_some() {
            return Err(Error::Unsupported("combined IMIJ/PNG working-memory cap").into());
        }
        Ok(Exporter { job: self, data })
    }
    fn check(&self) -> Result<(), TranscodeError> {
        self.stop
            .as_ref()
            .map_or(&Unstoppable as &dyn Stop, |s| s)
            .check()
            .map_err(TranscodeError::Stopped)
    }
    fn decode_job(&self) -> imij::codec::DecodeRequest {
        let mut job = self.config.decode.clone().job().with_limits(self.limits);
        if let Some(stop) = &self.stop {
            job = job.with_stop(stop.clone());
        }
        job
    }
    fn encode_job(
        &self,
        image: &ImageInfo,
        meta: &Metadata,
    ) -> Result<zenpng::PngEncodeJob, TranscodeError> {
        validate(image, meta)?;
        let mut metadata = zencodec::Metadata::none();
        if let Some(icc) = &meta.icc {
            metadata = metadata.with_icc(icc.as_slice());
        }
        if let Some(xmp) = &meta.xmp {
            metadata = metadata.with_xmp(xmp.as_slice());
        }
        let orientation = zencodec::Orientation::from_exif(meta.orientation.max(1))
            .ok_or(Error::Unsupported("PNG orientation"))?;
        if let Some(exif) = &meta.exif {
            // Preserve opaque EXIF bytes when its orientation agrees. Rewriting
            // arbitrary camera offsets requires an explicit metadata edit policy.
            let parsed = zencodec::exif::Exif::parse(exif)
                .ok_or(Error::Unsupported("malformed EXIF in PNG export"))?;
            if parsed.orientation().unwrap_or_default() != orientation {
                return Err(Error::Unsupported("IMIJ/EXIF orientation disagreement").into());
            }
            metadata = metadata.with_exif(exif.as_slice());
        } else if orientation != zencodec::Orientation::Identity {
            let mut exif = zencodec::exif::Exif::new(zencodec::exif::TextEncoding::Ascii);
            exif.set_orientation(orientation);
            metadata = metadata.with_exif(exif.to_bytes());
        }
        metadata.orientation = orientation;
        let cicp = (meta.icc.is_none()).then_some(zencodec::Cicp::new(
            image.cicp.color_primaries,
            image.cicp.transfer_characteristics,
            image.cicp.matrix_coefficients,
            image.cicp.full_range,
        ));
        let clli = meta.content_light_level.map(|v| {
            [
                u32::from_be_bytes(v[..4].try_into().unwrap()),
                u32::from_be_bytes(v[4..].try_into().unwrap()),
            ]
        });
        // Re-emit preserved unknown ancillary chunks on their original side
        // of IDAT (PNG-3 copy rules): the `idat` marker flips the position,
        // IMI-internal records (`cORg`) never leave, and unsafe-to-copy
        // names are dropped because the encode changes critical image data.
        let mut side = zenpng::AncillaryPosition::BeforeIdat;
        let mut unknowns = Vec::new();
        for c in &meta.chunks {
            if c.tag == crate::PNG_IDAT_BOUNDARY {
                side = zenpng::AncillaryPosition::AfterIdat;
            } else if c.tag != *b"cORg" && c.tag[3] & 0x20 != 0 {
                unknowns.push(zenpng::OwnedAncillaryChunk {
                    tag: c.tag,
                    data: c.data.clone(),
                    position: side,
                });
            }
        }
        let config = zenpng::PngEncoderConfig::new()
            .with_compression(self.config.compression)
            .with_downcast(zenpng::DowncastFlags::none())
            .with_preserve_transparent_rgb(true)
            .with_cicp(cicp)
            .with_content_light_level_raw(clli)
            .with_mastering_display_raw(meta.mastering_display)
            .with_unknown_chunks(unknowns);
        let mut job = config
            .job()
            .with_metadata_policy(metadata, MetadataPolicy::PreserveExact)
            .with_policy(
                zencodec::encode::EncodePolicy::none()
                    .with_color(ColorEmitPolicy::Verbatim),
            )
            .with_limits(self.limits);
        if let Some(stop) = &self.stop {
            job = job.with_stop(stop.clone());
        }
        Ok(job)
    }
}

/// Consumed exact export operation. No filesystem or network operations.
pub struct Exporter<'a> {
    job: ExportJob,
    data: &'a [u8],
}
impl Exporter<'_> {
    /// Return PNG or APNG bytes. APNG preserves composited canvases, frame count,
    /// rational delays and loops; per-frame metadata changes cannot be represented
    /// and are refused. A duration outside APNG's exact u16/u16 range is refused.
    pub fn export(self) -> Result<Vec<u8>, TranscodeError> {
        self.job.check()?;
        let info = self.job.decode_job().probe_full(self.data)?;
        let output = if info.sequence.is_animation() {
            let mut decoder = self
                .job
                .decode_job()
                .animation_frame_decoder(Cow::Borrowed(self.data), &[])?;
            let first = decoder
                .render_next_frame_owned(None)?
                .ok_or(Error::Corrupt("animation without frames"))?;
            let metadata = first
                .extras::<DecodedFrameMetadata>()
                .ok_or(Error::Corrupt("missing native frame metadata"))?
                .clone();
            let mut encoder = self
                .job
                .encode_job(&metadata.image, &metadata.metadata)?
                .with_loop_count(match info.sequence {
                    zencodec::ImageSequence::Animation { loop_count, .. } => loop_count,
                    _ => None,
                })
                .animation_frame_encoder()
                .map_err(png_error)?;
            encoder
                .push_frame_timed(detached(first.pixels())?, first.duration(), None)
                .map_err(png_error)?;
            while let Some(frame) = decoder.render_next_frame_owned(None)? {
                self.job.check()?;
                let current = frame
                    .extras::<DecodedFrameMetadata>()
                    .ok_or(Error::Corrupt("missing native frame metadata"))?;
                if current.image != metadata.image || current.metadata != metadata.metadata {
                    return Err(
                        Error::Unsupported("APNG per-frame image or metadata changes").into(),
                    );
                }
                encoder
                    .push_frame_timed(detached(frame.pixels())?, frame.duration(), None)
                    .map_err(png_error)?;
            }
            encoder.finish(None).map_err(png_error)?
        } else {
            if !matches!(info.sequence, zencodec::ImageSequence::Single) {
                return Err(Error::Unsupported("PNG export of page sequences").into());
            }
            let decoded = self
                .job
                .decode_job()
                .decoder(Cow::Borrowed(self.data), &[])?
                .decode()?;
            let metadata = decoded
                .extras::<DecodedMetadata>()
                .ok_or(Error::Corrupt("missing native image metadata"))?;
            self.job
                .encode_job(&metadata.image, &metadata.metadata)?
                .encoder()
                .map_err(png_error)?
                .encode(detached(decoded.pixels())?)
                .map_err(png_error)?
        };
        self.job.check()?;
        self.job
            .limits
            .check_output_size(output.data().len() as u64)?;
        Ok(output.into_vec())
    }
}

fn png_error(error: impl std::error::Error + Send + Sync + 'static) -> TranscodeError {
    TranscodeError::Png(Box::new(error))
}

// Metadata is resolved once above. Do not let an IMI ICC fallback CICP from
// the pixel context re-enter the PNG encoder and override the authoritative ICC.
fn detached(pixels: PixelSlice<'_>) -> Result<PixelSlice<'_>, TranscodeError> {
    PixelSlice::new(
        pixels.as_strided_bytes(),
        pixels.width(),
        pixels.rows(),
        pixels.stride(),
        pixels.descriptor(),
    )
    .map_err(|_| Error::InvalidInput("PNG export pixel layout").into())
}

fn validate(image: &ImageInfo, meta: &Metadata) -> Result<(), TranscodeError> {
    if !matches!(
        image.channels,
        imij::Channels::Gray
            | imij::Channels::GrayAlpha
            | imij::Channels::Rgb
            | imij::Channels::Rgba
    ) {
        return Err(Error::Unsupported("PNG cannot store native CMYK or Oklab").into());
    }
    if !matches!(image.sample, SampleType::U8 | SampleType::U16)
        || (image.alpha_mode != imij::AlphaMode::Straight)
        || (0..image.channels.count())
            .any(|c| image.channel_bits(c) != Some((image.sample.bytes() * 8) as u8))
    {
        return Err(
            Error::Unsupported("PNG export requires full-range straight U8/U16 samples").into(),
        );
    }
    if image.cicp.matrix_coefficients != 0 || !image.cicp.full_range {
        return Err(Error::Unsupported("PNG CICP matrix/range").into());
    }
    if meta.mastering_display.is_some() && meta.icc.is_some() {
        return Err(Error::Unsupported("PNG mastering display requires authoritative CICP").into());
    }
    if meta.diffuse_white.is_some() {
        return Err(Error::Unsupported("explicit diffuse white has no PNG carrier").into());
    }
    if meta.gain_map.is_some()
        || meta.chunks.iter().any(|c| {
            // The `idat` boundary marker and `cORg` provenance are
            // IMI-internal; unsafe-to-copy ancillary names are dropped
            // across the re-encode. Any other chunk must be a legal
            // ancillary name the PNG encoder can emit.
            !(c.tag == crate::PNG_IDAT_BOUNDARY
                || c.tag == *b"cORg"
                || (c.tag[0].is_ascii_lowercase() && c.tag[3] & 0x20 == 0)
                || (c.tag[0].is_ascii_lowercase()
                    && c.tag[2].is_ascii_uppercase()
                    && c.tag.iter().all(u8::is_ascii_alphabetic)))
        })
    {
        return Err(Error::Unsupported("native metadata has no PNG carrier").into());
    }
    // Historical origin is not active color. Validate it, but never re-embed
    // superseded carriers into PNG, where they could change precedence.
    meta.color_origin()?;
    if meta
        .xmp
        .as_ref()
        .is_some_and(|bytes| std::str::from_utf8(bytes).is_err())
    {
        return Err(Error::Unsupported("PNG XMP requires UTF-8").into());
    }
    Ok(())
}
