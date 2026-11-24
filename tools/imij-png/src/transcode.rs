//! Rendered-sample PNG/APNG import through zenpng's public decode APIs.
use enough::{Stop, Unstoppable};
use imij::{Channels, FrameConfig, FrameDuration, FrameEncoder, ImageInfo, SampleType};
use zencodec::{LimitExceeded, ResourceLimits, StopToken};
use zenpixels::{ChannelLayout, ChannelType, PixelSlice};

/// A PNG decode, IMI encode, input-policy or cancellation failure.
#[derive(Debug)]
#[non_exhaustive]
pub enum TranscodeError {
    Png(Box<dyn std::error::Error + Send + Sync>),
    Imij(imij::Error),
    Decode(imij::codec::DecodeError),
    Limit(LimitExceeded),
    Stopped(enough::StopReason),
}
impl core::fmt::Display for TranscodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Png(e) => e.fmt(f),
            Self::Imij(e) => e.fmt(f),
            Self::Decode(e) => e.fmt(f),
            Self::Limit(e) => e.fmt(f),
            Self::Stopped(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for TranscodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(match self {
            Self::Png(e) => e.as_ref(),
            Self::Imij(e) => e,
            Self::Decode(e) => e,
            Self::Limit(e) => e,
            Self::Stopped(_) => return None,
        })
    }
}
impl From<imij::Error> for TranscodeError {
    fn from(e: imij::Error) -> Self {
        Self::Imij(e)
    }
}

impl From<LimitExceeded> for TranscodeError {
    fn from(e: LimitExceeded) -> Self {
        Self::Limit(e)
    }
}

/// Reusable native PNG policy and exact lossless IMI settings.
/// Per-operation resource limits and cancellation belong to [`TranscodeJob`].
/// PNG decoding and native encoding retain complete image/frame buffers;
/// this is not the bounded-memory group input adapter.
#[derive(Clone, Default)]
pub struct TranscodeConfig {
    png: zenpng::PngDecodeConfig,
    imij: imij::ImiEncoderConfig,
}
impl TranscodeConfig {
    /// Consume reusable settings to create one operation, as in zencodec.
    /// Clone the config first to retain it for subsequent jobs.
    pub fn job(self) -> TranscodeJob {
        TranscodeJob {
            config: self,
            limits: ResourceLimits::default(),
            stop: None,
        }
    }
    pub fn with_png(mut self, png: zenpng::PngDecodeConfig) -> Self {
        self.png = png;
        self
    }
    /// Scientific error bounds must remain zero for rendered-sample preservation.
    pub fn with_imij(mut self, imij: imij::ImiEncoderConfig) -> Self {
        self.imij = imij;
        self
    }
}

/// Per-operation settings using zencodec's shared limits and owned stop token.
/// This composes a decoder and encoder; zencodec has no transcode trait.
pub struct TranscodeJob {
    config: TranscodeConfig,
    limits: ResourceLimits,
    stop: Option<StopToken>,
}
impl TranscodeJob {
    pub fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = limits;
        self
    }
    pub fn with_stop(mut self, stop: StopToken) -> Self {
        self.stop = Some(stop);
        self
    }
    /// Borrow complete encoded input. This does not perform file or network I/O.
    /// Incremental compressed input is not supported by this executor.
    pub fn transcoder(self, data: &[u8]) -> Result<Transcoder<'_>, TranscodeError> {
        let stop = self.stop.as_ref().map_or(&Unstoppable as &dyn Stop, |s| s);
        check_stop(stop)?;
        self.limits.check_input_size(data.len() as u64)?;
        if self.limits.max_memory_bytes.is_some() {
            return Err(imij::Error::Unsupported("combined PNG/IMIJ working-memory cap").into());
        }
        Ok(Transcoder { job: self, data })
    }
}

/// Borrowed-input executor, consumed when transcoding completes.
pub struct Transcoder<'a> {
    job: TranscodeJob,
    data: &'a [u8],
}
impl Transcoder<'_> {
    pub fn transcode(mut self) -> Result<TranscodeOutput, TranscodeError> {
        if !self.job.limits.threading.is_parallel() {
            self.job.config.png.max_threads = 1;
            self.job.config.imij.threads = 1;
        }
        // Unknown-chunk preservation is the transcoder's semantics, not a
        // caller knob: collection is opt-in at the zenpng layer because it
        // admits up to ~one input-size of verbatim payload copies.
        self.job.config.png.collect_unknown_chunks = true;
        let stop = self
            .job
            .stop
            .as_ref()
            .map_or(&Unstoppable as &dyn Stop, |s| s);
        transcode_impl(self.data, &self.job.config, stop, &self.job.limits)
    }
}

/// Converted images plus source metadata that has no native wire mapping yet.
#[derive(Debug)]
#[non_exhaustive]
pub struct TranscodeOutput {
    /// A still IMI or the fully composited IMI animation with exact timing.
    pub image: Vec<u8>,
    /// For APNG, the default IDAT image encoded separately as a still IMI.
    /// Keep this alongside `image`: it may be a distinct non-animation poster.
    /// It is retained even when it also supplies the first animation frame.
    pub fallback: Option<Vec<u8>>,
    /// Original parsed metadata, including text, physical size and source sBIT.
    /// These fields are not all installed in the IMI codestream. Retain this
    /// record when application metadata or source provenance is required.
    pub source: zenpng::PngInfo,
    pub warnings: Vec<zenpng::PngWarning>,
}

/// Transcode stored sample codes and composited animation canvases exactly.
/// Palette identity, original frame rectangles and compressed chunk boundaries
/// are intentionally not retained. Color/HDR/orientation are installed in both
/// codestreams. No CMS transform, alpha ablation or low-bit truncation occurs.
/// Stop is checked by zenpng internally and around each IMI image/frame encode;
/// the native encoder's internal search does not yet have stop checkpoints.
pub fn transcode(
    data: &[u8],
    config: &TranscodeConfig,
    stop: &dyn Stop,
) -> Result<TranscodeOutput, TranscodeError> {
    transcode_impl(data, config, stop, &ResourceLimits::default())
}

fn transcode_impl(
    data: &[u8],
    config: &TranscodeConfig,
    stop: &dyn Stop,
    limits: &ResourceLimits,
) -> Result<TranscodeOutput, TranscodeError> {
    check_stop(stop)?;
    limits.check_input_size(data.len() as u64)?;
    // Use zenpng's own probe; no second PNG parser or metadata interpretation.
    let head = zenpng::probe(data).map_err(|e| TranscodeError::Png(Box::new(e)))?;
    limits.check_dimensions(head.width, head.height)?;
    let count = head
        .sequence
        .count()
        .ok_or(imij::Error::Unsupported("unknown PNG frame count"))?;
    limits.check_frames(count)?;
    let total = u64::from(head.width)
        .checked_mul(u64::from(head.height))
        .and_then(|n| n.checked_mul(u64::from(count)))
        .ok_or(imij::Error::TooLarge)?;
    limits.check_total_pixels(total)?;
    check_stop(stop)?;
    drop(head);
    if config.imij.max_error != [0; 4] {
        return Err(
            imij::Error::InvalidInput("PNG transcode requires exact lossless settings").into(),
        );
    }
    let decoded =
        zenpng::decode(data, &config.png, stop).map_err(|e| TranscodeError::Png(Box::new(e)))?;
    let (cicp, mut meta) = crate::color_metadata(&decoded.info)?;
    // Preserve unknown safe-to-copy ancillary chunks verbatim (PNG-3 copy
    // rules): zenpng's decoder already walked the file and reports only
    // chunks it does not interpret, so nothing semantically handled is
    // duplicated. The first/only `IDAT` boundary rides as an `idat`
    // `Metadata::chunks` marker; unsafe-to-copy unknowns are dropped across
    // this critical-data re-encode.
    let mut marked_after_idat = false;
    for chunk in &decoded.info.unknown_chunks {
        if !chunk.is_safe_to_copy() {
            continue;
        }
        if chunk.position == zenpng::AncillaryPosition::AfterIdat && !marked_after_idat {
            meta.chunks.push(imij::Chunk {
                tag: crate::PNG_IDAT_BOUNDARY,
                data: Vec::new(),
            });
            marked_after_idat = true;
        }
        meta.chunks.push(imij::Chunk {
            tag: chunk.tag,
            data: chunk.data.clone(),
        });
    }
    validate_sbit(&decoded.info)?;
    if let Some(ref bytes) = meta.exif {
        let exif = zencodec::exif::Exif::parse(bytes).ok_or(imij::Error::Unsupported(
            "malformed PNG EXIF cannot establish orientation",
        ))?;
        meta.orientation = exif.orientation().unwrap_or_default().to_exif();
    }
    let pixels = decoded.pixels.as_slice();
    let info = description(&pixels, cicp)?;
    check_stop(stop)?;
    let still = config.imij.encode_samples(
        imij::ImageRef::new(pixels.as_strided_bytes(), pixels.stride(), &info),
        &meta,
    )?;
    check_stop(stop)?;
    limits.check_output_size(still.len() as u64)?;
    if !decoded.info.sequence.is_animation() {
        return Ok(TranscodeOutput {
            image: still,
            fallback: None,
            source: decoded.info,
            warnings: decoded.warnings,
        });
    }
    // The public zenpng API supplies composited full canvases, including
    // SOURCE/OVER and all disposal operations, with rational frame delays.
    drop(decoded.pixels);
    let animation = zenpng::decode_apng(data, &config.png, stop)
        .map_err(|e| TranscodeError::Png(Box::new(e)))?;
    let first = animation
        .frames
        .first()
        .ok_or(imij::Error::Corrupt("APNG has no animation frames"))?;
    let canvas = description(&first.pixels.as_slice(), cicp)?;
    let mut frames = FrameConfig::new(config.imij.clone());
    frames.loop_count = animation.num_plays;
    let mut encoder = FrameEncoder::animation(canvas.clone(), meta, frames);
    let mut duration_ms = 0u64;
    for frame in animation.frames {
        check_stop(stop)?;
        let pixels = frame.pixels.as_slice();
        if description(&pixels, cicp)? != canvas {
            return Err(imij::Error::Corrupt("APNG canvas format changes").into());
        }
        let duration = FrameDuration {
            num: u64::from(frame.frame_info.delay_num),
            den: u32::from(if frame.frame_info.delay_den == 0 {
                100
            } else {
                frame.frame_info.delay_den
            }),
        };
        // Conservatively round each frame up to milliseconds for admission;
        // the emitted frame delay remains the exact original rational.
        duration_ms = duration_ms
            .checked_add((duration.num * 1000).div_ceil(u64::from(duration.den)))
            .ok_or(imij::Error::TooLarge)?;
        limits.check_animation_ms(duration_ms)?;
        encoder.push_frame(pixels.as_strided_bytes(), pixels.stride(), duration)?;
        check_stop(stop)?;
    }
    let image = encoder.finish()?;
    limits.check_output_size(
        (image.len() as u64)
            .checked_add(still.len() as u64)
            .ok_or(imij::Error::TooLarge)?,
    )?;
    check_stop(stop)?;
    let mut warnings = decoded.warnings;
    warnings.extend(animation.warnings);
    Ok(TranscodeOutput {
        image,
        fallback: Some(still),
        source: decoded.info,
        warnings,
    })
}

fn check_stop(stop: &dyn Stop) -> Result<(), TranscodeError> {
    stop.check().map_err(TranscodeError::Stopped)
}
fn description(pixels: &PixelSlice<'_>, cicp: imij::Cicp) -> Result<ImageInfo, imij::Error> {
    let descriptor = pixels.descriptor();
    let sample = match descriptor.channel_type() {
        ChannelType::U8 => SampleType::U8,
        ChannelType::U16 => SampleType::U16,
        _ => return Err(imij::Error::Unsupported("PNG decoder sample type")),
    };
    let channels = match descriptor.layout() {
        ChannelLayout::Gray => Channels::Gray,
        ChannelLayout::GrayAlpha => Channels::GrayAlpha,
        ChannelLayout::Rgb => Channels::Rgb,
        ChannelLayout::Rgba => Channels::Rgba,
        _ => return Err(imij::Error::Unsupported("PNG decoder channel layout")),
    };
    let mut info = ImageInfo::new(pixels.width(), pixels.rows(), channels, sample);
    info.cicp = cicp;
    // PNG sBIT describes source precision in expanded storage codes. It is
    // not IMI's right-aligned code grid. Keep full reconstructed storage bits
    // here, including new intermediate values created by APNG composition.
    Ok(info)
}
fn validate_sbit(info: &zenpng::PngInfo) -> Result<(), imij::Error> {
    use zenpng::SignificantBits::*;
    let Some(bits) = info.significant_bits else {
        return Ok(());
    };
    let (values, count) = match (info.color_type, bits) {
        (0, Gray(g)) => ([g, 0, 0, 0], 1),
        (2 | 3, Rgb(r, g, b)) => ([r, g, b, 0], 3),
        (4, GrayAlpha(g, a)) => ([g, a, 0, 0], 2),
        (6, Rgba(r, g, b, a)) => ([r, g, b, a], 4),
        _ => return Err(imij::Error::Corrupt("PNG sBIT channel declaration")),
    };
    let maximum = if info.color_type == 3 {
        8
    } else {
        info.bit_depth
    };
    if values[..count].iter().any(|&n| n == 0 || n > maximum) {
        return Err(imij::Error::Corrupt("PNG sBIT precision"));
    }
    Ok(())
}

impl From<imij::codec::DecodeError> for TranscodeError {
    fn from(error: imij::codec::DecodeError) -> Self {
        Self::Decode(error)
    }
}
