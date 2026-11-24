use crate::{Error, ImageInfo, Metadata, SampleType};
use alloc::{borrow::Cow, sync::Arc, vec::Vec};
use zencodec::decode::{
    Decode, DecodeCapabilities, DecodeJob, DecodeOutput, DecodeRowSink, DecoderConfig, OutputInfo,
    StreamingDecode,
};
use zencodec::enough::Stop;
use zencodec::{ImageFormat, ResourceLimits, StopToken};
use zenpixels::{PixelBuffer, PixelDescriptor};
mod animation;
#[cfg(feature = "std")]
mod feed;
mod rows;
pub use animation::{AnimationDecoder, DecodedFrameMetadata};
#[cfg(feature = "std")]
pub use feed::{FeedDecoder, FeedStatus};
pub use rows::RowDecoder;

/// Codec failures and original caller-owned sink errors.
#[derive(Debug)]
#[non_exhaustive]
pub enum DecodeError {
    Codec(Error),
    Sink(zencodec::decode::SinkError),
}
impl From<Error> for DecodeError {
    fn from(error: Error) -> Self {
        Self::Codec(error)
    }
}
impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Codec(e) => e.fmt(f),
            Self::Sink(e) => e.fmt(f),
        }
    }
}
impl core::error::Error for DecodeError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        Some(match self {
            Self::Codec(e) => e,
            Self::Sink(e) => e.as_ref(),
        })
    }
}
impl zencodec::CategorizedError for DecodeError {
    fn codec_name(&self) -> Option<&'static str> {
        Some("imij")
    }
    fn category(&self) -> zencodec::ErrorCategory {
        match self {
            Self::Codec(e) => e.category(),
            Self::Sink(_) => zencodec::ErrorCategory::Io(zencodec::CodecIoKind::opaque()),
        }
    }
}

/// Native information retained in `DecodeOutput::extras::<DecodedMetadata>()`.
/// This preserves significant bits and fractional/wide HDR light levels that
/// the common zencodec metadata types cannot express exactly. Metadata policy
/// exclusions apply here too.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct DecodedMetadata {
    pub image: ImageInfo,
    pub metadata: Metadata,
}

/// Reusable primary-image and animation settings, with native admission budgets.
/// Executors use the serial cancellable session and preserve stored orientation.
#[derive(Clone, Default)]
pub struct DecodeConfig {
    admission: crate::decode_resources::DecodePolicy,
}
impl DecodeConfig {
    /// Override codec work, metadata, unit and working-capacity budgets.
    /// Job limits are additionally enforced; neither setting relaxes the other.
    pub fn with_admission(mut self, policy: crate::decode_resources::DecodePolicy) -> Self {
        self.admission = policy;
        self
    }
}
static FORMATS: [ImageFormat; 1] = [ImageFormat::Custom(&super::FORMAT)];
const CAPS: DecodeCapabilities = DecodeCapabilities::new()
    .with_animation(true)
    .with_streaming(true)
    .with_icc(true)
    .with_exif(true)
    .with_xmp(true)
    .with_cicp(true)
    .with_stop(true)
    .with_native_gray(true)
    .with_native_16bit(true)
    .with_native_f32(cfg!(feature = "extended"))
    .with_native_alpha(true)
    .with_enforces_max_pixels(true)
    .with_enforces_max_input_bytes(true)
    .with_enforces_max_memory(true)
    .with_threads_supported_range(1, 1);
impl DecoderConfig for DecodeConfig {
    type Error = DecodeError;
    type Job<'a> = DecodeRequest;
    fn formats() -> &'static [ImageFormat] {
        &FORMATS
    }
    fn supported_descriptors() -> &'static [PixelDescriptor] {
        if cfg!(feature = "extended") {
            super::DESCRIPTORS
        } else {
            // F32 formats and Oklab occupy the tail of the shared list.
            super::DESCRIPTORS.split_at(super::BASELINE_DESCRIPTORS).0
        }
    }
    fn capabilities() -> &'static DecodeCapabilities {
        &CAPS
    }
    fn job<'a>(self) -> Self::Job<'a> {
        DecodeRequest {
            config: self,
            stop: None,
            pulse: None,
            limits: ResourceLimits::none(),
            policy: zencodec::decode::DecodePolicy::none(),
        }
    }
}

/// Per-operation decode, animation and metadata inspection.
/// Input is a complete borrowed or owned encoded slice. Push output retains one
/// row plus codec working buffers, never a hidden whole-image pixel buffer.
pub struct DecodeRequest {
    config: DecodeConfig,
    stop: Option<StopToken>,
    /// Progress for a primary-image decode: one child per lossless unit
    /// when the pulse is live. Cancellation goes through `stop`.
    pulse: Option<how_far::SharedPulse>,
    limits: ResourceLimits,
    policy: zencodec::decode::DecodePolicy,
}
struct Description {
    native: ImageInfo,
    metadata: Metadata,
    info: zencodec::ImageInfo,
    descriptor: PixelDescriptor,
    retained: u64,
}
impl Description {
    fn output(&self) -> OutputInfo {
        OutputInfo::full_decode(self.native.width, self.native.height, self.descriptor)
    }
}
impl DecodeRequest {
    /// Decode the review-only native component candidate from bounded caller-fed input.
    /// Native declarations and typed plane rows remain provisional until explicit
    /// EOF and final integrity validation. The ordinary packed feed API is unchanged.
    #[cfg(all(feature = "lab", feature = "std"))]
    pub fn component_feed_decoder(
        self,
        input_capacity: usize,
    ) -> Result<crate::components::ComponentFeedDecoder, DecodeError> {
        self.check()?;
        let mut policy = self.config.admission;
        if let Some(max) = self.limits.max_memory_bytes {
            policy = policy.cap_working(max);
        }
        let budget = crate::decode_resources::feed::Budget::new(policy, self.stop, self.limits)?;
        crate::components::ComponentFeedDecoder::start(
            policy,
            budget,
            input_capacity,
            self.policy.resolve_icc(true),
        )
        .map_err(Into::into)
    }
    fn check(&self) -> Result<(), Error> {
        if self.stop.as_ref().is_some_and(|s| s.check().is_err()) {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
    fn header(&self, data: &[u8]) -> Result<crate::Header, Error> {
        self.check()?;
        self.limits
            .check_input_size(data.len() as u64)
            .map_err(|_| Error::TooLarge)?;
        let parsed = crate::parse::header::read_header_sized(&mut crate::ByteReader::new(data))?;
        self.config
            .admission
            .check_probe(&parsed.header.info, parsed.len, data.len())?;
        let h = parsed.header;
        self.limits
            .check_dimensions(h.info.width, h.info.height)
            .map_err(|_| Error::TooLarge)?;
        self.limits
            .check_total_pixels(u64::from(h.info.width) * u64::from(h.info.height))
            .map_err(|_| Error::TooLarge)?;
        self.limits.check_frames(1).map_err(|_| Error::TooLarge)?;
        Ok(h)
    }
    fn session(
        &self,
        reserved: u64,
    ) -> Result<crate::decode_resources::DecodeSession<impl Fn() -> bool + Sync + '_>, Error> {
        let mut policy = self.config.admission;
        if let Some(cap) = self.limits.max_memory_bytes {
            policy = policy.cap_working(cap.checked_sub(reserved).ok_or(Error::TooLarge)?);
        }
        Ok(crate::decode_resources::DecodeSession::with_cancel(
            policy,
            || self.check().is_err(),
        ))
    }
    fn describe(&self, data: &[u8], preferred: &[PixelDescriptor]) -> Result<Description, Error> {
        self.describe_reserved(data, preferred, 0)
    }
    fn describe_reserved(
        &self,
        data: &[u8],
        preferred: &[PixelDescriptor],
        reserved: u64,
    ) -> Result<Description, Error> {
        self.header(data)?;
        let mut session = self.session(reserved)?;
        let crate::decode_resources::Inspection {
            info: native,
            metadata,
            kind,
            count,
            loop_count,
        } = session.inspect(data, |tag| {
            Ok(match &tag {
                b"iCCP" if !self.policy.resolve_icc(true) => {
                    return Err(Error::Unsupported(
                        "dropping authoritative ICC without color conversion",
                    ));
                }
                b"cORg" => self.policy.resolve_icc(true),
                b"eXIf" => self.policy.resolve_exif(true),
                b"xMPp" => self.policy.resolve_xmp(true),
                _ => true,
            })
        })?;
        self.limits
            .check_frames(u32::try_from(count).map_err(|_| Error::TooLarge)?)
            .map_err(|_| Error::TooLarge)?;
        // Native metadata and common Arc carriers may coexist. Reserve two
        // payload copies plus descriptor/list overhead already charged by the
        // container planner. This is a conservative capacity plan, not RSS.
        let retained = session
            .usage()
            .metadata_bytes
            .checked_mul(2)
            .and_then(|v| v.checked_add(session.usage().peak_planned_working_bytes))
            .ok_or(Error::TooLarge)?;
        self.limits
            .check_memory(retained.checked_add(reserved).ok_or(Error::TooLarge)?)
            .map_err(|_| Error::TooLarge)?;
        let (mut info, descriptor) = common_info(&native, &metadata)?;
        info.sequence = match kind {
            crate::FrameKind::Still => zencodec::ImageSequence::Single,
            crate::FrameKind::Animation => zencodec::ImageSequence::Animation {
                frame_count: Some(u32::try_from(count).map_err(|_| Error::TooLarge)?),
                loop_count: Some(loop_count),
                random_access: false,
            },
            _ => zencodec::ImageSequence::Multi {
                image_count: Some(u32::try_from(count).map_err(|_| Error::TooLarge)?),
                random_access: false,
            },
        };
        // Preferences are optional: preserve the exact native descriptor when
        // no offered descriptor matches; no conversion or relabelling occurs.
        let descriptor = preferred
            .iter()
            .copied()
            .find(|d| *d == descriptor)
            .unwrap_or(descriptor);
        self.check()?;
        Ok(Description {
            native,
            metadata,
            info,
            descriptor,
            retained,
        })
    }
}
impl<'a> DecodeJob<'a> for DecodeRequest {
    type Error = DecodeError;
    type Dec = ImageDecoder<'a>;
    type StreamDec = BufferedPullDecoder;
    type AnimationFrameDec = AnimationDecoder;
    fn with_stop(mut self, stop: StopToken) -> Self {
        self.stop = Some(stop);
        self
    }
    /// Cancellation and progress: a primary-image decode reports each unit
    /// (lossless or lossy stripe or column group, native JPEG band) as a
    /// child of the pulse; push and row decoders advance it by one per
    /// delivered row, animation decoders by one per decoded frame.
    fn with_pulse(mut self, pulse: how_far::SharedPulse) -> Self {
        self.stop = Some(StopToken::new(pulse.clone()));
        self.pulse = Some(pulse);
        self
    }
    fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = limits;
        self
    }
    fn with_policy(mut self, policy: zencodec::decode::DecodePolicy) -> Self {
        self.policy = policy;
        self
    }
    fn probe(&self, data: &[u8]) -> Result<zencodec::ImageInfo, DecodeError> {
        let header = self.header(data)?;
        // Header-only probe deliberately does not guess ICC authority, frame
        // count, or ancillary HDR fields. probe_full supplies those fields.
        let mut info = zencodec::ImageInfo::new(
            header.info.width,
            header.info.height,
            ImageFormat::Custom(&super::FORMAT),
        )
        .with_alpha(header.info.channels.has_alpha() && header.info.alpha_mode.has_alpha())
        .with_bit_depth(header.info.sample.depth() as u8)
        .with_channel_count(header.info.channels.count() as u8)
        .with_orientation(zencodec::Orientation::from_exif(header.orientation).unwrap_or_default());
        info.warnings
            .push("header-only probe; color authority and sequence require probe_full".into());
        Ok(info)
    }
    fn probe_full(&self, data: &[u8]) -> Result<zencodec::ImageInfo, DecodeError> {
        Ok(self.describe(data, &[])?.info)
    }
    fn output_info(&self, data: &[u8]) -> Result<OutputInfo, DecodeError> {
        Ok(self.describe(data, &[])?.output())
    }
    fn decoder(
        self,
        data: Cow<'a, [u8]>,
        preferred: &[PixelDescriptor],
    ) -> Result<ImageDecoder<'a>, DecodeError> {
        let description = self.describe(&data, preferred)?;
        Ok(ImageDecoder {
            job: self,
            data,
            description,
        })
    }
    fn push_decoder(
        self,
        data: Cow<'a, [u8]>,
        sink: &mut dyn DecodeRowSink,
        preferred: &[PixelDescriptor],
    ) -> Result<OutputInfo, DecodeError> {
        let description = self.describe(&data, preferred)?;
        let native = &description.native;
        let row_bytes = (native.width as usize)
            .checked_mul(native.bytes_per_pixel())
            .ok_or(Error::TooLarge)?;
        let reserved = description
            .retained
            .checked_add(row_bytes as u64)
            .ok_or(Error::TooLarge)?;
        let mut session = self.session(reserved)?;
        let mut row = Vec::new();
        row.try_reserve_exact(row_bytes)
            .map_err(|_| Error::TooLarge)?;
        row.resize(row_bytes, 0);
        let descriptor = description.descriptor;
        self.check()?;
        let mut sink_error = None;
        // One completed row on the job's pulse per row the sink receives.
        let pulse = self.pulse.as_ref().map(|p| p.as_pulse());
        let decoded = session.decode_rows(&data, &mut row, |_, y, bytes| {
            if y == 0 {
                if let Err(e) = sink.begin(native.width, native.height, descriptor) {
                    sink_error = Some(e);
                    return Err(Error::Read);
                }
                self.check()?;
            }
            let mut out = match sink.provide_next_buffer(y, 1, native.width, descriptor) {
                Ok(out) => out,
                Err(e) => {
                    sink_error = Some(e);
                    return Err(Error::Read);
                }
            };
            if out.width() != native.width || out.rows() < 1 || out.descriptor() != descriptor {
                return Err(Error::InvalidInput("row sink output description"));
            }
            self.check()?;
            out.row_mut(0).copy_from_slice(bytes);
            if let Some(pulse) = pulse {
                pulse.advance(1);
            }
            Ok(())
        });
        if let Some(e) = sink_error {
            return Err(DecodeError::Sink(e));
        }
        decoded?;
        self.check()?;
        sink.finish().map_err(DecodeError::Sink)?;
        self.check()?;
        Ok(description.output())
    }
    fn streaming_decoder(
        self,
        data: Cow<'a, [u8]>,
        preferred: &[PixelDescriptor],
    ) -> Result<Self::StreamDec, DecodeError> {
        // StreamingDecode permits a complete-image batch. Reuse the admitted,
        // cancellable owned decode, preserving all output memory accounting.
        let stop = self.stop.clone();
        let output = self.decoder(data, preferred)?.decode()?;
        Ok(BufferedPullDecoder {
            output,
            stop,
            yielded: false,
        })
    }
    fn animation_frame_decoder(
        self,
        data: Cow<'a, [u8]>,
        preferred: &[PixelDescriptor],
    ) -> Result<Self::AnimationFrameDec, DecodeError> {
        AnimationDecoder::new(self, data, preferred)
    }
}

/// A zencodec pull executor that owns one complete decoded primary image.
/// Construction decodes under the job's limits and cancellation policy, then
/// releases compressed input. `next_batch` yields the full image once at y=0.
/// For bounded output buffering use [`DecodeRequest::push_decoder`] instead.
/// This does not accept incremental compressed input or iterate animation frames.
pub struct BufferedPullDecoder {
    output: DecodeOutput,
    stop: Option<StopToken>,
    yielded: bool,
}
impl BufferedPullDecoder {
    /// Full decoded output, including exact native metadata extras. This remains
    /// available after the batch has been consumed; pixel storage is not copied.
    pub fn output(&self) -> &DecodeOutput {
        &self.output
    }
}
impl StreamingDecode for BufferedPullDecoder {
    type Error = DecodeError;
    fn next_batch(&mut self) -> Result<Option<(u32, zenpixels::PixelSlice<'_>)>, DecodeError> {
        if self.stop.as_ref().is_some_and(|s| s.check().is_err()) {
            return Err(Error::Cancelled.into());
        }
        if self.yielded {
            return Ok(None);
        }
        self.yielded = true;
        Ok(Some((0, self.output.pixels())))
    }
    fn info(&self) -> &zencodec::ImageInfo {
        self.output.info()
    }
}

/// Owned or borrowed compressed input and a primary-image decode job.
pub struct ImageDecoder<'a> {
    job: DecodeRequest,
    data: Cow<'a, [u8]>,
    description: Description,
}
impl Decode for ImageDecoder<'_> {
    type Error = DecodeError;
    fn decode(self) -> Result<DecodeOutput, DecodeError> {
        let d = self.description;
        let bytes = d
            .descriptor
            .aligned_stride(d.native.width)
            .checked_mul(d.native.height as usize)
            .and_then(|n| n.checked_add(d.descriptor.min_alignment() - 1))
            .ok_or(Error::TooLarge)?;
        let reserved = d
            .retained
            .checked_add(bytes as u64)
            .ok_or(Error::TooLarge)?;
        let mut session = self.job.session(reserved)?;
        self.job.check()?;
        let pixels = PixelBuffer::try_new(d.native.width, d.native.height, d.descriptor)
            .map_err(|_| Error::TooLarge)?;
        let mut pixels =
            pixels.with_color_context(Arc::new(d.info.source_color.to_color_context()));
        let stride = pixels.stride();
        let mut slice = pixels.as_slice_mut();
        let target = slice.as_strided_bytes_mut();
        match &self.job.pulse {
            Some(pulse) => {
                session.decode_into_observed(&self.data, target, stride, pulse.as_pulse())?
            }
            None => session.decode_into(&self.data, target, stride)?,
        };
        self.job.check()?;
        Ok(
            DecodeOutput::new(pixels, d.info).with_extras(DecodedMetadata {
                image: d.native,
                metadata: d.metadata,
            }),
        )
    }
}

fn common_info(
    native: &ImageInfo,
    metadata: &Metadata,
) -> Result<(zencodec::ImageInfo, PixelDescriptor), Error> {
    let format = match (native.channels, native.sample) {
        (crate::Channels::Oklab, SampleType::F32) => zenpixels::PixelFormat::OklabF32,
        (crate::Channels::OklabAlpha, SampleType::F32) => zenpixels::PixelFormat::OklabaF32,
        (crate::Channels::Cmyk, SampleType::U8) => zenpixels::PixelFormat::Cmyk8,
        (channels, _) if channels.is_native_model() => {
            return Err(Error::Unsupported("zencodec native model sample type"));
        }
        _ => {
            let index = match native.sample {
                SampleType::U8 => 0,
                SampleType::U16 => 4,
                SampleType::F32 => super::BASELINE_DESCRIPTORS,
                SampleType::F16 => 8,
                _ => return Err(Error::Unsupported("zencodec signed/wide pixel storage")),
            } + native.channels.count()
                - 1;
            super::DESCRIPTORS[index].format
        }
    };
    let cicp = zencodec::Cicp::new(
        native.cicp.color_primaries,
        native.cicp.transfer_characteristics,
        native.cicp.matrix_coefficients,
        native.cicp.full_range,
    );
    if cicp.matrix_coefficients != 0 {
        return Err(Error::Unsupported(
            "non-identity pixel matrix in zencodec adapter",
        ));
    }
    if native.channels.is_native_model() && native.cicp != native.channels.default_cicp() {
        return Err(Error::Unsupported(
            "CICP does not describe native pixel model",
        ));
    }
    let mut descriptor = if metadata.icc.is_some() || native.channels == crate::Channels::Cmyk {
        PixelDescriptor::from_pixel_format(format)
            .with_primaries(zenpixels::ColorPrimaries::Unknown)
    } else {
        // `Cicp::to_descriptor`'s mapping, spelled out (that helper is deprecated
        // from zenpixels 0.2.17): unmapped codes stay `Unknown` here and keep
        // their raw values in the image's source CICP.
        PixelDescriptor::from_pixel_format(format)
            .with_transfer(cicp.transfer_function_enum())
            .with_primaries(cicp.color_primaries_enum())
    };
    descriptor.signal_range = if native.cicp.full_range {
        zenpixels::SignalRange::Full
    } else {
        zenpixels::SignalRange::Narrow
    };
    if native.channels.has_alpha() {
        descriptor.alpha = Some(native.alpha_mode);
    }
    let mut color = zencodec::SourceColor::default()
        .with_cicp(cicp)
        .with_bit_depth(native.sample.depth() as u8)
        .with_channel_count(native.channels.count() as u8)
        .with_color_authority(if metadata.icc.is_some() {
            zencodec::ColorAuthority::Icc
        } else {
            zencodec::ColorAuthority::Cicp
        });
    if let Some(ref icc) = metadata.icc {
        color.icc_profile = Some(Arc::from(icc.as_slice()));
    }
    let mut info = zencodec::ImageInfo::new(
        native.width,
        native.height,
        ImageFormat::Custom(&super::FORMAT),
    )
    .with_alpha(native.channels.has_alpha() && native.alpha_mode.has_alpha())
    .with_orientation(zencodec::Orientation::from_exif(metadata.orientation).unwrap_or_default());
    if let Some(cll) = metadata.content_light_level {
        let cll = u32::from_be_bytes(cll[..4].try_into().unwrap());
        let fall = u32::from_be_bytes(
            metadata.content_light_level.unwrap()[4..]
                .try_into()
                .unwrap(),
        );
        if cll % 10_000 == 0
            && fall % 10_000 == 0
            && cll / 10_000 <= u16::MAX as u32
            && fall / 10_000 <= u16::MAX as u32
        {
            color.content_light_level = Some(zencodec::ContentLightLevel {
                max_content_light_level: (cll / 10_000) as u16,
                max_frame_average_light_level: (fall / 10_000) as u16,
            });
        } else {
            info.warnings
                .push("fractional/wide content light level retained in DecodedMetadata".into());
        }
    }
    if let Some(m) = metadata.mastering_display {
        let xy = |i| f32::from(u16::from_be_bytes([m[i], m[i + 1]])) / 50_000.0;
        color.mastering_display = Some(zencodec::MasteringDisplay::new(
            [[xy(0), xy(2)], [xy(4), xy(6)], [xy(8), xy(10)]],
            [xy(12), xy(14)],
            (u32::from_be_bytes(m[16..20].try_into().unwrap()) as f32) / 10_000.0,
            (u32::from_be_bytes(m[20..24].try_into().unwrap()) as f32) / 10_000.0,
        ));
    }
    // The wire retains absence. The shared view supplies the HDR reference-white
    // convention only when CICP is authoritative; ICC may describe another space.
    color.diffuse_white = metadata.diffuse_white.or_else(|| {
        (metadata.icc.is_none() && matches!(native.cicp.transfer_characteristics, 16 | 18))
            .then_some(zenpixels::hdr::DiffuseWhite::BT2408)
    });
    info.source_color = color;
    if let Some(ref exif) = metadata.exif {
        info.embedded_metadata.exif = Some(Arc::from(exif.as_slice()));
    }
    if let Some(ref xmp) = metadata.xmp {
        info.embedded_metadata.xmp = Some(Arc::from(xmp.as_slice()));
    }
    Ok((info, descriptor))
}
