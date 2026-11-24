//! zencodec configuration and jobs for encoding and decoding.
//!
//! Pixels are passed through with their native stride and sample bits; this
//! adapter performs no color conversion or alpha ablation. Gray/GA/RGB/RGBA
//! U8/U16/F16/F32, native Oklab/OklabAlpha F32, CMYK U8 and all four shared
//! alpha modes are supported. Other layouts
//! return `Unsupported` instead of being relabelled.
//!
//! Default executors use the existing whole-image encoders. Cancellation is checked
//! within lossless pixel/unit and lossy transform/unit pipelines. Long refinement
//! kernels and finalizers check at boundaries. Adaptive lossless stills admit a
//! conservative owned-capacity plan before analysis. Lossy and animation jobs
//! refuse working-memory caps. For
//! bounded, caller-driven output use `crate::stream_encode::GroupEncodeSession`.
//! `ImiEncoderConfig::with_row_groups` exposes this fixed lossless backend
//! through zencodec row push/pull while retaining the encoded output required
//! by the trait.

#[cfg(feature = "std")]
#[cfg(feature = "encode")]
mod bounded;
mod decode;
#[cfg(feature = "encode")]
mod encode;
#[cfg(feature = "encode")]
mod groups;
#[cfg(feature = "encode")]
mod memory;
#[cfg(all(feature = "std", feature = "encode"))]
mod retained;
#[cfg(feature = "encode")]
mod rows;
mod timing;
#[cfg(feature = "std")]
#[cfg(feature = "encode")]
pub use bounded::Scratch;
pub use decode::{
    AnimationDecoder, BufferedPullDecoder, DecodeConfig, DecodeError, DecodeRequest,
    DecodedFrameMetadata, DecodedMetadata, ImageDecoder, RowDecoder,
};
#[cfg(feature = "std")]
pub use decode::{FeedDecoder, FeedStatus};
#[cfg(feature = "encode")]
pub use encode::AnimationEncoder;
#[cfg(feature = "encode")]
pub use groups::GroupEncoder;

#[cfg(feature = "encode")]
use crate::Metadata;
use crate::{Channels, Error, ImageInfo, SampleType};
#[cfg(feature = "encode")]
use crate::{Coder, ImiEncoderConfig, LossyQuality, RowGroups};
use zencodec::ImageFormatDefinition;
#[cfg(feature = "encode")]
use zencodec::encode::{
    EncodeCapabilities, EncodeJob, EncodeOutput, Encoder, EncoderConfig, Fidelity, LossyTarget,
};
#[cfg(feature = "encode")]
use zencodec::enough::Stop;
#[cfg(feature = "encode")]
use zencodec::{ImageFormat, ResourceLimits, StopToken};
use zenpixels::{AlphaMode, ChannelLayout, ChannelType, PixelDescriptor, PixelFormat, PixelSlice};

/// IMI registration for zencodec's downstream format extension point.
/// The identifier stays `imij`, the crate's name; `image/x-imij` remains an
/// accepted MIME type.
pub static FORMAT: ImageFormatDefinition = ImageFormatDefinition::new(
    "imij",
    None,
    "IMI",
    "imi",
    &["imi", "imij"],
    "image/x-imi",
    &["image/x-imi", "image/x-imij"],
    true,
    true,
    true,
    true,
    8,
    |data| data.starts_with(b"IMIJ\r\n\x87\n"),
);

const BASELINE_DESCRIPTORS: usize = 13;
const DESCRIPTORS: &[PixelDescriptor] = &[
    PixelDescriptor::GRAY8_SRGB,
    PixelDescriptor::GRAYA8_SRGB,
    PixelDescriptor::RGB8_SRGB,
    PixelDescriptor::RGBA8_SRGB,
    PixelDescriptor::GRAY16_SRGB,
    PixelDescriptor::GRAYA16_SRGB,
    PixelDescriptor::RGB16_SRGB,
    PixelDescriptor::RGBA16_SRGB,
    PixelDescriptor::from_pixel_format(PixelFormat::GrayF16),
    PixelDescriptor::from_pixel_format(PixelFormat::GrayAF16),
    PixelDescriptor::from_pixel_format(PixelFormat::RgbF16),
    PixelDescriptor::from_pixel_format(PixelFormat::RgbaF16),
    PixelDescriptor::CMYK8.with_primaries(zenpixels::ColorPrimaries::Unknown),
    PixelDescriptor::GRAYF32_LINEAR,
    PixelDescriptor::GRAYAF32_LINEAR,
    PixelDescriptor::RGBF32_LINEAR,
    PixelDescriptor::RGBAF32_LINEAR,
    PixelDescriptor::OKLABF32,
    PixelDescriptor::OKLABAF32,
];
#[cfg(feature = "encode")]
const ENCODE_CAPS: EncodeCapabilities = EncodeCapabilities::new()
    .with_animation(true)
    .with_push_rows(true)
    // Row groups (`group_rows`, `quick_rows*`) stream; see `ImageEncoder::begin`.
    .with_streaming_output(true)
    .with_encode_from(true)
    .with_icc(true)
    .with_exif(true)
    .with_xmp(true)
    .with_cicp(true)
    .with_lossy(true)
    .with_lossless(true)
    .with_native_gray(true)
    .with_native_16bit(true)
    .with_native_f32(true)
    .with_native_alpha(true)
    .with_enforces_max_pixels(true)
    .with_effort_range(0, 10)
    .with_quality_range(0.0, 100.0);

/// The working allowance `with_prefer_streaming` gives the row groups it
/// selects.
#[cfg(feature = "encode")]
const PREFERRED_GROUP_WORKING_BYTES: usize = 64 << 20;

/// Generic quality maps to IMI's calibrated SSIMULACRA2 target and switches
/// the coder to lossy; the codec-specific target is IMI's quantizer scale. A
/// zensim target has no IMI calibration: it keeps the configuration's lossy
/// target (the default scale when the coder was lossless), visible through
/// `resolved_target_fidelity`, as zencodec's best-effort API requires.
#[cfg(feature = "encode")]
impl EncoderConfig for ImiEncoderConfig {
    type Error = Error;
    type Job = EncodeRequest;

    fn format() -> ImageFormat {
        ImageFormat::Custom(&FORMAT)
    }
    fn supported_descriptors() -> &'static [PixelDescriptor] {
        DESCRIPTORS
    }
    fn capabilities() -> &'static EncodeCapabilities {
        &ENCODE_CAPS
    }
    fn with_generic_quality(self, quality: f32) -> Self {
        self.with_quality(LossyQuality::Ssim2(quality))
    }
    fn with_generic_effort(self, effort: i32) -> Self {
        self.with_effort(effort as f32)
    }
    fn with_lossless(self, lossless: bool) -> Self {
        if lossless {
            let tuning = self
                .lossless_tuning()
                .filter(|t| t.max_error != [0; 4])
                .map(|t| t.clone().with_channel_max_errors([0; 4]));
            let cfg = self.with_coder(Coder::Lossless);
            match tuning {
                Some(t) => cfg.with_lossless_tuning(t),
                None => cfg,
            }
        } else if self.coder().is_lossless() {
            self.with_quality(LossyQuality::Scale(80.0))
        } else {
            self
        }
    }
    fn with_fidelity(self, fidelity: Fidelity) -> Self {
        match fidelity {
            Fidelity::Lossless => EncoderConfig::with_lossless(self, true),
            Fidelity::Lossy(LossyTarget::ApproxSsim2(q)) => {
                self.with_quality(LossyQuality::Ssim2(q))
            }
            Fidelity::Lossy(LossyTarget::ApproxButteraugli(distance)) => {
                self.with_quality(LossyQuality::Butteraugli {
                    distance,
                    norm: crate::ButteraugliNorm::Max,
                })
            }
            Fidelity::Lossy(LossyTarget::CodecSpecificQuality(q)) => {
                self.with_quality(LossyQuality::Scale(q))
            }
            _ => EncoderConfig::with_lossless(self, false),
        }
    }
    /// With `true`, an exact lossless configuration at the default effort
    /// and without row groups of its own encodes whole-width, one-row
    /// groups whose output streams after `begin`. Applied when the job is
    /// created. Row groups refuse effort tuning and lossy targets, which keep
    /// the whole-image encoder.
    fn with_prefer_streaming(mut self, prefer: bool) -> Self {
        self.set_prefer_streaming(Some(prefer));
        self
    }
    fn prefers_streaming(&self) -> Option<bool> {
        self.prefer_streaming()
    }
    fn generic_quality(&self) -> Option<f32> {
        match self.coder() {
            Coder::Lossy(LossyQuality::Ssim2(q)) => Some(q),
            _ => None,
        }
    }
    fn generic_effort(&self) -> Option<i32> {
        Some(self.effort() as i32)
    }
    fn is_lossless(&self) -> Option<bool> {
        Some(self.is_exact())
    }
    fn resolved_target_fidelity(&self) -> Option<Fidelity> {
        self.fidelity()
    }
    fn job(mut self) -> EncodeRequest {
        if self.prefer_streaming() == Some(true)
            && self.row_groups().is_none()
            && self.coder() == Coder::Lossless
            && self.is_exact()
            && self.effort() == crate::config::DEFAULT_EFFORT
        {
            self = self.with_row_groups(Some(RowGroups::new(
                u32::MAX,
                1,
                PREFERRED_GROUP_WORKING_BYTES,
            )));
        }
        EncodeRequest {
            config: self,
            #[cfg(feature = "std")]
            scratch: None,
            #[cfg(feature = "std")]
            prepared: std::sync::Mutex::new(None),
            stop: None,
            progress: None,
            limits: ResourceLimits::none(),
            metadata: zencodec::Metadata::none(),
            color_origin: None,
            policy: zencodec::encode::EncodePolicy::none(),
            canvas: None,
            loop_count: None,
            input_descriptor: None,
        }
    }
}

/// Per-operation limits, stop token and retained metadata.
///
/// With `std` and [`Self::with_scratch`], [`EncodeJob::plan_encode`] retains the
/// actual bounded native preparation. Identical declarations reuse it; changed
/// declarations refuse without replacing it. Setters invalidate preparation.
/// Shared metadata is copied fallibly into native payload storage once per
/// accepted plan; neither repeated planning nor startup clones those payloads.
/// `begin_declared` requires this same job's plan; first `pull_rows` materializes
/// it. Output is buffered until finish. This route refuses working-memory caps
/// until its retained state is qualified; ordinary bounded encoding is unchanged.
#[cfg(feature = "encode")]
pub struct EncodeRequest {
    config: ImiEncoderConfig,
    /// Bounded lossy encoding ([`Self::with_scratch`]): where its scratch lives.
    #[cfg(feature = "std")]
    scratch: Option<Scratch>,
    #[cfg(feature = "std")]
    prepared: std::sync::Mutex<Option<alloc::boxed::Box<retained::Prepared>>>,
    stop: Option<StopToken>,
    progress: Option<how_far::SharedPulse>,
    limits: ResourceLimits,
    metadata: zencodec::Metadata,
    color_origin: Option<zenpixels::ColorOrigin>,
    policy: zencodec::encode::EncodePolicy,
    canvas: Option<(u32, u32)>,
    loop_count: Option<u32>,
    input_descriptor: Option<PixelDescriptor>,
}

#[cfg(feature = "encode")]
impl EncodeJob for EncodeRequest {
    type Error = Error;
    type Enc = ImageEncoder;
    type AnimationFrameEnc = AnimationEncoder;
    fn with_stop(mut self, stop: StopToken) -> Self {
        self.invalidate_preparation();
        self.stop = Some(stop);
        self
    }
    /// Progress and cancellation, as [`EncodeRequest::with_progress`].
    fn with_pulse(mut self, pulse: how_far::SharedPulse) -> Self {
        self.invalidate_preparation();
        self.progress = Some(pulse);
        self
    }
    fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.invalidate_preparation();
        self.limits = limits;
        self
    }
    fn with_metadata(mut self, metadata: zencodec::Metadata) -> Self {
        self.invalidate_preparation();
        self.metadata = metadata;
        self
    }
    fn with_policy(mut self, policy: zencodec::encode::EncodePolicy) -> Self {
        self.invalidate_preparation();
        self.policy = policy;
        self
    }
    fn with_canvas_size(mut self, width: u32, height: u32) -> Self {
        self.invalidate_preparation();
        self.canvas = Some((width, height));
        self
    }
    fn with_loop_count(mut self, count: Option<u32>) -> Self {
        self.invalidate_preparation();
        self.loop_count = count;
        self
    }
    #[cfg(feature = "std")]
    fn plan_encode(
        &self,
        declaration: &zencodec::encode::EncodeDeclaration<'_>,
    ) -> Result<zencodec::encode::EncodePlan, zencodec::encode::BoxedError> {
        self.plan_retained(declaration)
            .map_err(|e| alloc::boxed::Box::new(e) as zencodec::encode::BoxedError)
    }

    fn encoder(self) -> Result<ImageEncoder, Error> {
        self.validate()?;
        Ok(ImageEncoder {
            job: self,
            rows: None,
            #[cfg(feature = "std")]
            spool: None,
            #[cfg(feature = "std")]
            retained: None,
            failed: None,
        })
    }
    fn animation_frame_encoder(mut self) -> Result<AnimationEncoder, Error> {
        self.invalidate_preparation();
        AnimationEncoder::new(self)
    }
}

#[cfg(feature = "encode")]
impl EncodeRequest {
    fn invalidate_preparation(&mut self) {
        #[cfg(feature = "std")]
        {
            *self.prepared.get_mut().unwrap_or_else(|e| e.into_inner()) = None;
        }
    }

    /// Lossy encoding with working memory bounded by one band of about 8
    /// block rows at full width, the job form of
    /// `crate::stream_encode::encode_lossy_bounded`. Decoded pixels equal
    /// the whole-image encoder's for the same configuration; the file is
    /// slightly larger (explicit AC tables, nested images coded in bands).
    /// AQ and explicit quantization tables are refused with `Unsupported`,
    /// as is a lossless coder.
    ///
    /// The encoder reads its source two or more times. `encode` reads the
    /// caller's pixels in place. Pushed rows (`push_rows`, then `finish`) and
    /// pulled rows (`encode_from`) are spooled to `scratch` first, since
    /// neither can be read twice; output starts at `finish`. The unit index,
    /// spooled records and checksum digests also live in `scratch`. The
    /// encoded file is returned whole, like every `EncodeOutput`; for output
    /// larger than memory, use the native function with a file sink.
    ///
    /// `max_memory_bytes` admits a bound on the encoder's working heap, plus
    /// heap scratch with [`Scratch::Memory`].
    #[cfg(feature = "std")]
    pub fn with_scratch(mut self, scratch: Scratch) -> Self {
        self.invalidate_preparation();
        self.scratch = Some(scratch);
        self
    }

    /// The row-group settings, or the refusal `name` when the configuration
    /// has none.
    pub(crate) fn group_config(
        &self,
        name: &'static str,
    ) -> Result<crate::stream_encode::GroupEncodeConfig, Error> {
        let groups = self.config.row_groups().ok_or(Error::Unsupported(name))?;
        crate::stream_encode::GroupEncodeConfig::from_config(&self.config, groups)
    }

    #[cfg(feature = "std")]
    pub(crate) fn is_bounded(&self) -> bool {
        self.scratch.is_some()
    }
    #[cfg(not(feature = "std"))]
    pub(crate) fn is_bounded(&self) -> bool {
        false
    }

    /// Retain historical color independently of current pixel color/ICC.
    /// This is per-job metadata; `EncodePolicy::embed_icc = Some(false)` drops
    /// the complete historical record without affecting active color checks.
    pub fn with_color_origin(mut self, origin: zenpixels::ColorOrigin) -> Self {
        self.invalidate_preparation();
        self.color_origin = Some(origin);
        self
    }

    /// Describe the rows supplied by `Encoder::encode_from`.
    ///
    /// The common pull callback has no descriptor argument. Supply the canvas
    /// through `EncodeJob::with_canvas_size` and color carriers through job
    /// metadata. Push input may establish its descriptor instead.
    pub fn with_input_descriptor(mut self, descriptor: PixelDescriptor) -> Self {
        self.invalidate_preparation();
        self.input_descriptor = Some(descriptor);
        self
    }

    /// Observe encoding through how-far phases and completed work.
    /// Whole-image jobs report analysis/coding phases. Group-row jobs advance
    /// the supplied leaf by one after all groups in each row have been emitted
    /// into the output buffer; callers may set its total to the canvas height.
    /// The caller retains and completes the root;
    /// this job owns only a shared view. Existing `with_stop` remains additive.
    /// Animation jobs advance the supplied leaf once per completed native
    /// frame, including unchanged frames; callers may set its total to the
    /// frame count. Lossy stills report transform rows, block refinement and
    /// coefficient coding. Refinement/table search and finalization currently
    /// check cancellation at stage boundaries, not inside every kernel.
    ///
    /// ```
    /// use how_far::{ResultExt, Pulse};
    /// use how_far_along::{Phase, PulseTree, Total, Unstoppable};
    /// use imij::ImiEncoderConfig;
    /// use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
    /// use zenpixels::{PixelDescriptor, PixelSlice};
    /// let data = [127; 3 * 16 * 16];
    /// let pixels = PixelSlice::new(&data, 16, 16, 48, PixelDescriptor::RGB8_SRGB)?;
    /// let tree = PulseTree::new(Phase::new("encode", Total::Unknown), Unstoppable);
    /// let observer = tree.observer();
    /// let job = ImiEncoderConfig::default().job().with_progress(tree.share()?);
    /// let result = job.encoder().and_then(|encoder| encoder.encode(pixels));
    /// let output = result.finish_phase(tree)?;
    /// assert_eq!(observer.snapshot().fraction(), Some(1.0));
    /// assert!(!output.data().is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn with_progress(mut self, pulse: how_far::SharedPulse) -> Self {
        self.invalidate_preparation();
        self.progress = Some(pulse);
        self
    }

    fn validate(&self) -> Result<(), Error> {
        self.check_stop()?;
        let bounded = self.is_bounded();
        if bounded && self.config.coder().is_lossless() {
            return Err(Error::Unsupported("bounded lossy encoding is lossy"));
        }
        if self.limits.max_memory_bytes.is_some() && self.config.row_groups().is_none() && !bounded
        {
            match self.config.coder() {
                Coder::Lossy(_) => {
                    return Err(Error::Unsupported("lossy encoder working-memory limit"));
                }
                Coder::Lossless => memory::supported(&self.config.lossless_config())?,
                Coder::Quick(_) => {}
            }
        }
        if let Some(groups) = self.config.row_groups() {
            if groups.columns == 0 {
                return Err(Error::InvalidInput("zero row group width"));
            }
            if !self.config.coder().is_lossless()
                || self.config.effort() != crate::config::DEFAULT_EFFORT
            {
                return Err(Error::Unsupported(
                    "lossy fidelity or effort tuning in group row encoder",
                ));
            }
        }
        if self
            .policy
            .color
            .is_some_and(|p| p != zencodec::ColorEmitPolicy::Verbatim)
        {
            return Err(Error::Unsupported("color-carrier rewriting in IMI adapter"));
        }
        Ok(())
    }
    /// Explicit configuration threads win; otherwise the job's threading
    /// policy decides.
    fn configure_threads(&mut self) {
        if self.config.threads().is_some() {
            return;
        }
        let threads = if self.limits.threading.is_parallel() {
            #[cfg(feature = "rayon")]
            {
                rayon::current_num_threads()
            }
            #[cfg(not(feature = "rayon"))]
            {
                1
            }
        } else {
            1
        };
        self.config = self.config.clone().with_threads(threads);
    }
    fn check_stop(&self) -> Result<(), Error> {
        if self.stop.as_ref().is_some_and(|s| s.check().is_err())
            || self.progress.as_ref().is_some_and(|p| p.check().is_err())
        {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// zencodec operations by [`name`](zencodec::UnsupportedOperation::name),
/// for [`Error::source`](core::error::Error::source): refusals carry the name
/// as `Error::Unsupported` and stay typed for zencodec callers.
static OPERATIONS: [zencodec::UnsupportedOperation; 16] = {
    use zencodec::UnsupportedOperation as Op;
    [
        Op::RowLevelEncode,
        Op::PullEncode,
        Op::AnimationEncode,
        Op::AnimationTiming,
        Op::DecodePolicy,
        Op::EncodePolicy,
        Op::DecodeInto,
        Op::RowLevelDecode,
        Op::AnimationDecode,
        Op::PixelFormat,
        Op::MultiImageDecode,
        Op::GainMapEncode,
        Op::Inventory,
        Op::IncrementalDecode,
        Op::SeekableDecode,
        Op::EncodePlanning,
    ]
};

/// The zencodec operation whose `name()` is `name`.
pub(crate) fn operation(name: &str) -> Option<&'static zencodec::UnsupportedOperation> {
    OPERATIONS.iter().find(|op| op.name() == name)
}

impl zencodec::CategorizedError for Error {
    fn codec_name(&self) -> Option<&'static str> {
        Some("imij")
    }
    fn category(&self) -> zencodec::ErrorCategory {
        use zencodec::{
            CodecIoKind, ErrorCategory, ImageError, InvalidKind, RequestError, ResourceError,
            UnsupportedImageKind, enough::StopReason,
        };
        match self {
            Error::NotImij => {
                ErrorCategory::Image(ImageError::Unsupported(UnsupportedImageKind::Type))
            }
            Error::Corrupt(msg) if msg.starts_with("truncated") => {
                ErrorCategory::Image(ImageError::UnexpectedEof)
            }
            Error::Corrupt(_) => ErrorCategory::Image(ImageError::Malformed),
            Error::Unsupported(name) => {
                if let Some(&op) = operation(name) {
                    ErrorCategory::Request(RequestError::Unsupported(op))
                } else {
                    ErrorCategory::Image(ImageError::Unsupported(UnsupportedImageKind::Feature))
                }
            }
            Error::TooLarge => ErrorCategory::Resource(ResourceError::OutOfMemory),
            Error::InvalidInput(_) => {
                ErrorCategory::Request(RequestError::Invalid(InvalidKind::Parameters))
            }
            Error::Read => ErrorCategory::Io(CodecIoKind::opaque()),
            Error::Cancelled => ErrorCategory::Stopped(StopReason::Cancelled),
        }
    }
}

/// Whole-image executor; no hidden row buffer or I/O is introduced.
#[cfg(feature = "encode")]
pub struct ImageEncoder {
    job: EncodeRequest,
    rows: Option<rows::RowEncoder>,
    /// Bounded lossy: pushed rows spooled until `finish`.
    #[cfg(feature = "std")]
    spool: Option<bounded::RowSpool>,
    #[cfg(feature = "std")]
    retained: Option<alloc::boxed::Box<retained::Active>>,
    failed: Option<Error>,
}

#[cfg(feature = "encode")]
impl Encoder for ImageEncoder {
    type Error = Error;
    fn reject(op: zencodec::UnsupportedOperation) -> Error {
        Error::Unsupported(op.name())
    }
    #[cfg(feature = "std")]
    fn begin_declared(
        &mut self,
        declaration: &zencodec::encode::EncodeDeclaration<'_>,
    ) -> Result<zencodec::encode::EncodePlan, Error> {
        self.begin_retained(declaration)
    }
    #[cfg(feature = "std")]
    fn pull_rows(
        &mut self,
        rows: &mut dyn zencodec::encode::EncodeRows,
        pulse: Option<&dyn zencodec::Pulse>,
    ) -> Result<zencodec::encode::PullProgress, Error> {
        self.pull_retained(rows, pulse)
    }
    fn preferred_strip_height(&self) -> u32 {
        1
    }
    /// Row groups (`group_rows`, `quick_rows*`) stream: the header is
    /// pending at once and each row's groups as soon as the row is pushed
    /// (`OutputTiming::streaming(0)`). Colour comes from the job's metadata
    /// and colour origin; rows must not carry a different pixel colour
    /// context. The whole-image encoders analyse every pixel before writing
    /// and refuse with `UnsupportedOperation::RowLevelEncode`.
    fn begin(
        &mut self,
        width: u32,
        height: u32,
        descriptor: PixelDescriptor,
    ) -> Result<zencodec::encode::OutputTiming, Error> {
        #[cfg(feature = "std")]
        if self.retained.is_some() {
            return Err(Error::InvalidInput("declared pull already started"));
        }
        self.begin_inner(width, height, descriptor)
    }
    fn pending_output(&self) -> &[u8] {
        #[cfg(feature = "std")]
        if let Some(active) = self.retained.as_ref() {
            return active.pending_output();
        }
        self.rows.as_ref().map_or(&[], |r| r.pending())
    }
    fn consume_output(&mut self, n: usize) {
        #[cfg(feature = "std")]
        if let Some(active) = self.retained.as_mut() {
            active.consume_output(n);
            return;
        }
        if let Some(rows) = self.rows.as_mut() {
            rows.consume(n);
        }
    }
    fn take_output(&mut self, into: &mut alloc::vec::Vec<u8>) {
        #[cfg(feature = "std")]
        if let Some(active) = self.retained.as_mut() {
            active.take_output(into);
            return;
        }
        if let Some(rows) = self.rows.as_mut() {
            rows.take(into);
        }
    }
    fn push_rows(&mut self, pixels: PixelSlice<'_>) -> Result<(), Error> {
        #[cfg(feature = "std")]
        if self.retained.take().is_some() {
            let error = Error::InvalidInput("push after declared pull");
            self.failed = Some(error.clone());
            return Err(error);
        }
        self.job.invalidate_preparation();
        let result = self.push_rows_inner(pixels, 0);
        if let Err(error) = &result {
            self.failed = Some(error.clone());
        }
        result
    }
    fn finish(mut self) -> Result<EncodeOutput, Error> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        self.job.check_stop()?;
        #[cfg(feature = "std")]
        if let Some(active) = self.retained.take() {
            return (*active).finish();
        }
        #[cfg(feature = "std")]
        if self.job.is_bounded() {
            let spool = self.spool.take().ok_or(Error::InvalidInput("no rows"))?;
            let data = spool.finish(&self.job)?;
            self.job.check_stop()?;
            return Ok(EncodeOutput::new(data, ImageFormat::Custom(&FORMAT)));
        }
        if self.job.config.row_groups().is_none() {
            return Err(Error::Unsupported("row_level_encode"));
        }
        let data = self
            .rows
            .take()
            .ok_or(Error::InvalidInput("no rows"))?
            .finish()?;
        self.job.check_stop()?;
        Ok(EncodeOutput::new(data, ImageFormat::Custom(&FORMAT)))
    }
    fn encode_from(
        mut self,
        source: &mut dyn FnMut(u32, zenpixels::PixelSliceMut<'_>) -> usize,
    ) -> Result<EncodeOutput, Error> {
        #[cfg(feature = "std")]
        if self.retained.is_some() {
            return Err(Error::InvalidInput("legacy pull after declared pull"));
        }
        self.job.invalidate_preparation();
        #[cfg(feature = "std")]
        if self.job.is_bounded() {
            return rows::pull_bounded(self, source);
        }
        rows::pull(self, source)
    }
    fn encode(mut self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, Error> {
        #[cfg(feature = "std")]
        if self.retained.is_some() {
            return Err(Error::InvalidInput("one-shot after declared pull"));
        }
        self.job.invalidate_preparation();
        if self.rows.is_some() || self.failed.is_some() {
            return Err(self
                .failed
                .unwrap_or(Error::InvalidInput("row encoding already started")));
        }
        if self.job.config.row_groups().is_some() {
            if self
                .job
                .canvas
                .is_some_and(|c| c != (pixels.width(), pixels.rows()))
            {
                return Err(Error::InvalidInput("row canvas mismatch"));
            }
            self.job.canvas = Some((pixels.width(), pixels.rows()));
            self.push_rows(pixels)?;
            return self.finish();
        }
        let job = &mut self.job;
        job.check_stop()?;
        job.limits
            .check_dimensions(pixels.width(), pixels.rows())
            .map_err(|_| Error::TooLarge)?;
        job.limits
            .check_total_pixels(u64::from(pixels.width()) * u64::from(pixels.rows()))
            .map_err(|_| Error::TooLarge)?;
        job.limits.check_frames(1).map_err(|_| Error::TooLarge)?;
        job.limits
            .check_input_size(pixels.as_strided_bytes().len() as u64)
            .map_err(|_| Error::TooLarge)?;
        job.configure_threads();
        #[cfg(feature = "std")]
        if job.is_bounded() {
            let data = job.encode_bounded_slice(&pixels)?;
            job.check_stop()?;
            return Ok(EncodeOutput::new(data, ImageFormat::Custom(&FORMAT)));
        }
        if let Some(limit) = job.limits.max_memory_bytes {
            let (channels, sample) = encode_storage(pixels.descriptor())?;
            let shape = ImageInfo::new(pixels.width(), pixels.rows(), channels, sample);
            let context_icc = pixels.color_context().and_then(|c| c.icc.as_ref());
            let metadata_bytes = [
                job.metadata.icc_profile.as_deref(),
                job.metadata.exif.as_deref(),
                job.metadata.xmp.as_deref(),
                context_icc.map(AsRef::as_ref),
                job.color_origin.as_ref().and_then(|o| o.icc.as_deref()),
            ]
            .iter()
            .flatten()
            .map(|v| v.len() as u128)
            .sum();
            let planned = match job.config.coder() {
                Coder::Quick(_) => {
                    crate::quick::working_bytes(&shape, &job.config.quick_config(), metadata_bytes)?
                }
                _ => u128::from(memory::lossless_bytes(
                    &shape,
                    &job.config.lossless_config(),
                    metadata_bytes,
                )?),
            };
            if planned > u128::from(limit) {
                return Err(Error::TooLarge);
            }
        }
        let (info, meta) = encode_description(
            &pixels,
            &job.metadata,
            &job.policy,
            job.color_origin.as_ref(),
        )?;
        let pulse = job
            .progress
            .as_ref()
            .map_or(&how_far::NoPulse as &dyn how_far::Pulse, |p| p.as_pulse());
        let pulse = how_far::WithStop::borrowed(pulse, &job.stop);
        let data = job.config.encode_samples_with_progress(
            crate::ImageRef::new(pixels.as_strided_bytes(), pixels.stride(), &info),
            &meta,
            &pulse,
        )?;
        job.check_stop()?;
        job.limits
            .check_output_size(data.len() as u64)
            .map_err(|_| Error::TooLarge)?;
        Ok(EncodeOutput::new(data, ImageFormat::Custom(&FORMAT)))
    }
}

#[cfg(feature = "encode")]
fn encode_description(
    pixels: &PixelSlice<'_>,
    meta: &zencodec::Metadata,
    policy: &zencodec::encode::EncodePolicy,
    origin: Option<&zenpixels::ColorOrigin>,
) -> Result<(ImageInfo, Metadata), Error> {
    encode_description_for(
        pixels.descriptor(),
        (pixels.width(), pixels.rows()),
        pixels.color_context().map(AsRef::as_ref),
        meta,
        policy,
        origin,
    )
}

fn encode_storage(desc: PixelDescriptor) -> Result<(Channels, SampleType), Error> {
    let channels = match desc.layout() {
        ChannelLayout::Gray => Channels::Gray,
        ChannelLayout::GrayAlpha => Channels::GrayAlpha,
        ChannelLayout::Rgb => Channels::Rgb,
        ChannelLayout::Rgba => Channels::Rgba,
        ChannelLayout::Oklab => Channels::Oklab,
        ChannelLayout::OklabA => Channels::OklabAlpha,
        ChannelLayout::Cmyk => Channels::Cmyk,
        _ => return Err(Error::Unsupported("pixel channel layout in IMI adapter")),
    };
    let sample = match desc.channel_type() {
        ChannelType::U8 => SampleType::U8,
        ChannelType::U16 => SampleType::U16,
        ChannelType::F16 => SampleType::F16,
        ChannelType::F32 => SampleType::F32,
        _ => return Err(Error::Unsupported("pixel sample type in IMI adapter")),
    };
    Ok((channels, sample))
}

#[cfg(feature = "encode")]
fn copy_metadata(bytes: &[u8]) -> Result<alloc::vec::Vec<u8>, Error> {
    let mut out = alloc::vec::Vec::new();
    out.try_reserve_exact(bytes.len())
        .map_err(|_| Error::TooLarge)?;
    out.extend_from_slice(bytes);
    Ok(out)
}

/// The IMI description of pixels: layout, sample type, alpha and the CICP
/// labels (from `meta_cicp`, the color context, or the descriptor).
pub(crate) fn image_info_for(
    desc: PixelDescriptor,
    dimensions: (u32, u32),
    context: Option<&zenpixels::ColorContext>,
    meta_cicp: Option<zencodec::Cicp>,
) -> Result<ImageInfo, Error> {
    let (channels, sample) = encode_storage(desc)?;
    let alpha_mode = match (channels.has_alpha(), desc.alpha()) {
        (false, None) => AlphaMode::Straight,
        (
            true,
            Some(
                mode @ (AlphaMode::Straight
                | AlphaMode::Premultiplied
                | AlphaMode::Opaque
                | AlphaMode::Undefined),
            ),
        ) => mode,
        _ => {
            return Err(Error::Unsupported(
                "pixel alpha interpretation in IMI adapter",
            ));
        }
    };
    if channels.is_native_model()
        && (!matches!(
            desc.primaries,
            zenpixels::ColorPrimaries::Unknown | zenpixels::ColorPrimaries::Bt709
        ) || desc.transfer != zenpixels::TransferFunction::Unknown)
    {
        return Err(Error::InvalidInput(
            "RGB color labels on native pixel model",
        ));
    }
    let context_cicp = context.and_then(|c| c.cicp);
    if meta_cicp.zip(context_cicp).is_some_and(|(a, b)| a != b) {
        return Err(Error::InvalidInput("metadata and pixel CICP disagree"));
    }
    let described = if channels.is_native_model() {
        let c = channels.default_cicp();
        Some(zencodec::Cicp::new(
            c.color_primaries,
            c.transfer_characteristics,
            0,
            true,
        ))
    } else {
        zencodec::Cicp::from_descriptor(&desc)
    };
    let cicp = meta_cicp.or(context_cicp).or(described).unwrap_or_else(|| {
        zencodec::Cicp::new(
            desc.primaries.to_cicp().unwrap_or(2),
            desc.transfer.to_cicp().unwrap_or(2),
            0,
            matches!(desc.signal_range, zenpixels::SignalRange::Full),
        )
    });
    // CMYK's generic zenpixels descriptor historically defaults to Bt709;
    // that is not RGB colorimetry for ink components. The model and ICC remain
    // separate from these CICP labels. Never apply an RGB transfer to Oklab.
    if channels.is_native_model() && Some(cicp) != described {
        return Err(Error::InvalidInput(
            "CICP does not describe native pixel model",
        ));
    }
    if cicp.matrix_coefficients != 0
        || (!channels.is_native_model()
            && desc
                .primaries
                .to_cicp()
                .is_some_and(|v| v != cicp.color_primaries))
        || desc
            .transfer
            .to_cicp()
            .is_some_and(|v| v != cicp.transfer_characteristics)
        || cicp.full_range != matches!(desc.signal_range, zenpixels::SignalRange::Full)
    {
        return Err(Error::InvalidInput(
            "CICP does not describe supplied pixels",
        ));
    }
    let mut info = ImageInfo::new(dimensions.0, dimensions.1, channels, sample);
    info.alpha_mode = alpha_mode;
    info.cicp = crate::Cicp {
        color_primaries: cicp.color_primaries,
        transfer_characteristics: cicp.transfer_characteristics,
        matrix_coefficients: 0,
        full_range: cicp.full_range,
    };
    Ok(info)
}

/// The native entry points' description: `info` from the pixels and `meta`
/// as given, with the color context's ICC profile when `meta` has none.
#[cfg(feature = "encode")]
pub(crate) fn native_description(
    pixels: &PixelSlice<'_>,
    meta: &Metadata,
) -> Result<(ImageInfo, Metadata), Error> {
    let info = ImageInfo::from_pixels(pixels)?;
    let context_icc = pixels.color_context().and_then(|c| c.icc.as_ref());
    let mut meta = meta.clone();
    match (&meta.icc, context_icc) {
        (Some(a), Some(b)) if a.as_slice() != b.as_ref() => {
            return Err(Error::InvalidInput("metadata and pixel ICC disagree"));
        }
        (None, Some(b)) => meta.icc = Some(copy_metadata(b)?),
        _ => {}
    }
    Ok((info, meta))
}

impl ImageInfo {
    /// The IMI description of a [`PixelSlice`]: its layout, sample type,
    /// alpha mode and CICP (from the color context or the descriptor).
    /// Fails for layouts and sample types IMI does not store.
    pub fn from_pixels(pixels: &PixelSlice<'_>) -> Result<Self, Error> {
        image_info_for(
            pixels.descriptor(),
            (pixels.width(), pixels.rows()),
            pixels.color_context().map(AsRef::as_ref),
            None,
        )
    }
}

#[cfg(feature = "encode")]
fn encode_description_for(
    desc: PixelDescriptor,
    dimensions: (u32, u32),
    context: Option<&zenpixels::ColorContext>,
    meta: &zencodec::Metadata,
    policy: &zencodec::encode::EncodePolicy,
    origin: Option<&zenpixels::ColorOrigin>,
) -> Result<(ImageInfo, Metadata), Error> {
    let info = image_info_for(desc, dimensions, context, meta.cicp)?;
    let context_white = context.and_then(|c| c.diffuse_white);
    if meta
        .diffuse_white
        .zip(context_white)
        .is_some_and(|(a, b)| a != b)
    {
        return Err(Error::InvalidInput(
            "metadata and pixel diffuse white disagree",
        ));
    }
    let context_icc = context.and_then(|c| c.icc.as_ref());
    if meta
        .icc_profile
        .as_ref()
        .zip(context_icc)
        .is_some_and(|(a, b)| a != b)
    {
        return Err(Error::InvalidInput("metadata and pixel ICC disagree"));
    }
    let icc = meta.icc_profile.as_ref().or(context_icc);
    if policy.embed_icc == Some(false) && icc.is_some() {
        return Err(Error::Unsupported(
            "dropping authoritative ICC without color conversion",
        ));
    }
    let mut output = Metadata {
        diffuse_white: meta.diffuse_white.or(context_white),
        icc: icc.map(|v| copy_metadata(v)).transpose()?,
        exif: meta
            .exif
            .as_ref()
            .filter(|_| policy.embed_exif != Some(false))
            .map(|v| copy_metadata(v))
            .transpose()?,
        xmp: meta
            .xmp
            .as_ref()
            .filter(|_| policy.embed_xmp != Some(false))
            .map(|v| copy_metadata(v))
            .transpose()?,
        orientation: meta.orientation as u8,
        ..Metadata::default()
    };
    if policy.embed_icc != Some(false) {
        output.set_color_origin(origin)?;
    }
    if let Some(cll) = meta.content_light_level {
        let mut bytes = [0; 8];
        bytes[..4]
            .copy_from_slice(&(u32::from(cll.max_content_light_level) * 10_000).to_be_bytes());
        bytes[4..].copy_from_slice(
            &(u32::from(cll.max_frame_average_light_level) * 10_000).to_be_bytes(),
        );
        output.content_light_level = Some(bytes);
    }
    if let Some(m) = meta.mastering_display {
        let mut bytes = [0; 24];
        for (i, value) in m
            .primaries_xy
            .into_iter()
            .flatten()
            .chain(m.white_point_xy)
            .enumerate()
        {
            let scaled = scaled_metadata(value, 50_000.0, u16::MAX as u64)? as u16;
            bytes[i * 2..i * 2 + 2].copy_from_slice(&scaled.to_be_bytes());
        }
        bytes[16..20].copy_from_slice(
            &(scaled_metadata(m.max_luminance, 10_000.0, u32::MAX as u64)? as u32).to_be_bytes(),
        );
        bytes[20..24].copy_from_slice(
            &(scaled_metadata(m.min_luminance, 10_000.0, u32::MAX as u64)? as u32).to_be_bytes(),
        );
        output.mastering_display = Some(bytes);
    }
    Ok((info, output))
}

#[cfg(feature = "encode")]
fn scaled_metadata(value: f32, scale: f64, max: u64) -> Result<u64, Error> {
    let value = f64::from(value) * scale + 0.5;
    if !value.is_finite() || value < 0.5 || value >= (max as f64 + 1.0) {
        return Err(Error::InvalidInput("HDR metadata out of range"));
    }
    Ok(value as u64)
}
