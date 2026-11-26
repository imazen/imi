//! The encoder configuration: one small `Clone + Send + Sync` value that
//! chooses the coding, the effort and what the decoder will see, used
//! directly by the native entry points and as the zencodec
//! [`EncoderConfig`](zencodec::EncoderConfig) of this crate.
//!
//! Expert knobs for the two coders live behind `Option<Box<_>>` in
//! [`LosslessTuning`] and [`LossyTuning`]; an unset box means "derive from
//! effort and decode speed", and the configuration itself stays small
//! enough to live on the stack and copy freely (see `tests::size_budget`).
use crate::lossy::LossyConfig;
use crate::quick::QuickConfig;
use crate::{
    DecodeSpeed, DecoderMemoryLimit, Error, HashAlgorithm, ImageInfo, ImageRef, LossyQuality,
    Metadata, QuickBitSmarts,
};
use alloc::boxed::Box;
use alloc::vec::Vec;
use zencodec::encode::{Fidelity, LossyTarget};
use zenpixels::PixelSlice;

/// The effort an untouched configuration encodes at.
pub const DEFAULT_EFFORT: f32 = 7.5;

/// Which coder runs, and so what the file holds.
///
/// `Lossless` and `Quick` reproduce every stored sample bit; `Quick` spends
/// almost no analysis (independent raw or pixel-packet units) and is the
/// choice for archives and for streaming encode of very large images.
/// `Lossy` aims at a [`LossyQuality`] target.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum Coder {
    /// The context-modelled lossless coder. The default.
    Lossless,
    /// The low-analysis lossless coder; see [`QuickBitSmarts`].
    Quick(QuickBitSmarts),
    /// The transform coder, aiming at the given quality.
    Lossy(LossyQuality),
}

impl Coder {
    /// `true` for both lossless coders.
    pub const fn is_lossless(self) -> bool {
        !matches!(self, Self::Lossy(_))
    }
}

/// Row-group streaming encode: the image is coded in horizontal groups of
/// `rows` pixel rows, each split into `columns` pixel columns, with the
/// encoder's working memory kept under `max_working_bytes`. Produces the
/// same container as a whole-image encode; a decoder can start on the first
/// group before the last one is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowGroups {
    /// Pixel columns per group; `u32::MAX` for the full width.
    pub columns: u32,
    /// Pixel rows per group.
    pub rows: u32,
    /// Encoder working-memory budget per group, in bytes.
    pub max_working_bytes: usize,
}

impl RowGroups {
    pub const fn new(columns: u32, rows: u32, max_working_bytes: usize) -> Self {
        Self {
            columns,
            rows,
            max_working_bytes,
        }
    }
}

/// Encoder settings. `Default` is exact lossless at effort 7.5, the setting
/// every size and speed claim in this repository's benchmarks is made at.
///
/// ```
/// use imij::{ImiEncoderConfig, LossyQuality, DecodeSpeed};
/// let archival = ImiEncoderConfig::lossless();
/// let web = ImiEncoderConfig::lossy(LossyQuality::Ssim2(80.0))
///     .with_decode_speed(DecodeSpeed::Fast);
/// let bulk = ImiEncoderConfig::quick().with_threads(8);
/// # let _ = (archival, web, bulk);
/// ```
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ImiEncoderConfig {
    coder: Coder,
    effort: f32,
    decode_speed: Option<DecodeSpeed>,
    threads: Option<usize>,
    checksum: bool,
    hash: HashAlgorithm,
    unit_ordinals: bool,
    column_groups: Option<u32>,
    decoder_memory_limit: Option<DecoderMemoryLimit>,
    row_groups: Option<RowGroups>,
    prefer_streaming: Option<bool>,
    lossless: Option<Box<LosslessTuning>>,
    lossy: Option<Box<LossyOptions>>,
}

#[derive(Clone, Debug)]
struct LossyOptions {
    tuning: LossyTuning,
    ac_scale: f32,
    local_ans_contexts: bool,
    coding_defaults: bool,
    model_references: bool,
    dc_coverage: Option<Vec<Vec<u32>>>,
}
impl Default for LossyOptions {
    fn default() -> Self {
        Self {
            tuning: LossyTuning::default(),
            ac_scale: 1.0,
            local_ans_contexts: false,
            coding_defaults: false,
            model_references: false,
            dc_coverage: None,
        }
    }
}

impl Default for ImiEncoderConfig {
    fn default() -> Self {
        Self {
            coder: Coder::Lossless,
            effort: DEFAULT_EFFORT,
            decode_speed: None,
            threads: None,
            checksum: false,
            hash: HashAlgorithm::DEFAULT,
            unit_ordinals: false,
            column_groups: None,
            decoder_memory_limit: None,
            row_groups: None,
            prefer_streaming: None,
            lossless: None,
            lossy: None,
        }
    }
}

impl ImiEncoderConfig {
    /// Exact lossless coding (the default).
    pub fn lossless() -> Self {
        Self::default()
    }

    /// Lossy coding aiming at `quality`: a bare `f32` is the quantizer scale
    /// ([`LossyQuality::Scale`]); [`LossyQuality::Ssim2`] and
    /// [`LossyQuality::Butteraugli`] are calibrated metric targets.
    pub fn lossy(quality: impl Into<LossyQuality>) -> Self {
        Self::default().with_quality(quality)
    }

    /// Restricted writer preset for [`crate::v4::CodingProfile::Fixed8x8Photo`].
    /// Supply an RGB8 still. Keep ordinary 8×8 transforms and turn off
    /// exact overlays and activity AQ in the DC raster. Uniform AC scale
    /// and either entropy coder remain available. Changing transform or
    /// coding controls can take the output outside the profile.
    pub fn lossy_fixed_8x8_photo(quality: impl Into<LossyQuality>) -> Self {
        Self::lossy(quality).tune_lossy(|tuning| {
            tuning.channel_layouts = [crate::LossyChannelLayout::default(); 3];
            tuning.exact_blocks = Some(0.0);
            tuning.adaptive_partitions = false;
            tuning.small_transforms = false;
            tuning.aq_gamma = 0.0;
        })
    }

    /// The low-analysis lossless coder without bit analysis.
    pub fn quick() -> Self {
        Self::default().with_coder(Coder::Quick(QuickBitSmarts::Off))
    }

    /// Sets the coder directly.
    pub fn with_coder(mut self, coder: Coder) -> Self {
        self.coder = coder;
        self
    }

    /// Switches to lossy coding at `quality` (see [`Self::lossy`]).
    pub fn with_quality(mut self, quality: impl Into<LossyQuality>) -> Self {
        self.coder = Coder::Lossy(quality.into());
        self
    }

    /// Switches to the quick lossless coder with the given bit analysis.
    pub fn with_quick(mut self, bits: QuickBitSmarts) -> Self {
        self.coder = Coder::Quick(bits);
        self
    }

    /// Encoder work budget, `0..=10` (validated when encoding; default `7.5`).
    ///
    /// Effort never changes lossless samples and is ignored by the quick coder.
    /// For lossy XYB encoding:
    /// - `< 9.0` (including default `7.5`): fixed quantization (for fixed-8x8
    ///   encodes without effort-priced adaptive partitions, decoded samples are
    ///   identical across efforts `< 9.0`); `>= 7.5` searches entropy table
    ///   counts by coded size.
    /// - `9.0`: enables 1-pass AC trellis, open-loop surrounding-block lambda
    ///   AQ, and 2D-MED DC RDO; [`DecodeSpeed::Compact`] also enables full
    ///   adaptive superblock + 4x4 partitions with 9-context partition-map DP.
    /// - `9.5`: fast mid-tier Pareto rung for adaptive partitions (2-pass
    ///   merged trellis + two-pass empirical pilot + closed-loop seam AQ,
    ///   without per-candidate `trellis_select`).
    /// - `10.0`: 2-pass AC trellis + closed-loop seam AQ (plus per-candidate
    ///   `trellis_select` for adaptive partitions) for maximum compression.
    pub fn with_effort(mut self, effort: f32) -> Self {
        self.effort = effort;
        self
    }

    /// Decode-complexity tier of the lossy stream (smallest file to fastest
    /// decode). `None` (default) lets the encoder pick per image (fixed 8x8
    /// XYB blocks).
    ///
    /// - [`DecodeSpeed::Compact`]: smallest files — classic superblocks below
    ///   effort `9.0` and full adaptive superblock + 4x4 partitions from
    ///   effort `9.0` onward (8-bit images with longer side `>= 128 px`).
    /// - [`DecodeSpeed::Balanced`] / [`DecodeSpeed::Fast`]: fixed 8x8 XYB
    ///   blocks trading a small amount of file size for faster entropy decode.
    /// - [`DecodeSpeed::Fastest`]: skips XYB in favor of YCoCg for maximum
    ///   decode speed.
    ///
    /// Setting a tier fixes the color coding (`Fastest` codes YCoCg, the
    /// others XYB) regardless of [`LossyTuning::color`]. The lossless coders
    /// have one decode path and ignore it.
    pub fn with_decode_speed(mut self, speed: impl Into<Option<DecodeSpeed>>) -> Self {
        self.decode_speed = speed.into();
        self
    }

    /// Maximum encoder workers. Unset, a zencodec job takes the count from
    /// its [`ResourceLimits`](zencodec::ResourceLimits) and the native entry
    /// points encode serially.
    pub fn with_threads(mut self, threads: usize) -> Self {
        self.threads = Some(threads.max(1));
        self
    }

    /// Adds the encoded-file checksum (off by default): a `cSUM` table of
    /// piece digests under [`Self::hash_algorithm`].
    pub fn with_checksum(mut self, enabled: bool) -> Self {
        self.checksum = enabled;
        self
    }

    /// The digest algorithm of every digest this configuration writes: the
    /// file checksum's pieces and frame reference-raster digests. Readers
    /// verify every algorithm they know; default [`HashAlgorithm::DEFAULT`].
    pub fn with_hash_algorithm(mut self, algorithm: HashAlgorithm) -> Self {
        self.hash = algorithm;
        self
    }

    /// Request the optional unit-ordinal record in explicit legacy encodes
    /// (off by default). V4 always identifies groups and does not serialize
    /// the legacy record or capability bit.
    pub fn with_unit_ordinals(mut self, enabled: bool) -> Self {
        self.unit_ordinals = enabled;
        self
    }

    /// Column groups: independent column ranges of this many pixels within
    /// each stripe. `None` (default) = automatic, `Some(0)` = never.
    pub fn with_column_groups(mut self, columns: impl Into<Option<u32>>) -> Self {
        self.column_groups = columns.into();
        self
    }

    /// Shapes the stream so a decoder stays within `limit` (a conservative
    /// encoder estimate; see [`DecoderMemoryLimit`]). Never changes
    /// quantization, color or lossless alpha.
    pub fn with_decoder_memory_limit(
        mut self,
        limit: impl Into<Option<DecoderMemoryLimit>>,
    ) -> Self {
        self.decoder_memory_limit = limit.into();
        self
    }

    /// Encodes in row groups (streaming output and bounded working memory);
    /// see [`RowGroups`]. `None` encodes the whole image at once.
    pub fn with_row_groups(mut self, groups: impl Into<Option<RowGroups>>) -> Self {
        self.row_groups = groups.into();
        self
    }

    /// Expert settings for the lossless coder (unset = derived from effort).
    pub fn with_lossless_tuning(mut self, tuning: LosslessTuning) -> Self {
        self.lossless = Some(Box::new(tuning));
        self
    }

    /// Expert settings for the lossy coder (unset = derived from effort and
    /// decode speed).
    pub fn with_lossy_tuning(mut self, tuning: LossyTuning) -> Self {
        self.lossy.get_or_insert_with(Default::default).tuning = tuning;
        self
    }

    /// Opt into independent ANS AC-unit models (Lossy 0.5). Default off.
    /// Requires explicit RunLevel coding without automatic decode-tier selection.
    /// Existing stripe/column geometry sets each context's extent; 8 block rows
    /// gives 64 pixel rows. Every unit may retain the shared model when smaller.
    /// This affects lossy coding; lossless and quick coding ignore the setting.
    pub fn with_local_ans_contexts(mut self, enabled: bool) -> Self {
        self.lossy
            .get_or_insert_with(Default::default)
            .local_ans_contexts = enabled;
        self
    }

    /// Re-express existing lossy transform choices through scoped defaults.
    /// Default off; this changes representation, not quantization or pixels.
    pub fn with_coding_defaults(mut self, enabled: bool) -> Self {
        self.lossy
            .get_or_insert_with(Default::default)
            .coding_defaults = enabled;
        self
    }

    /// Opt into typed independent DC/AC model references. The whole-image
    /// encoder may train shared AC candidates; DC retains its root model.
    /// This is an offline search, not causal one-pass model training.
    pub fn with_model_references(mut self, enabled: bool) -> Self {
        self.lossy
            .get_or_insert_with(Default::default)
            .model_references = enabled;
        self
    }

    /// Consecutive AC-stripe counts covered by each DC rectangle. One list
    /// shares boundaries across columns; otherwise supply one per column group.
    /// Counts must be positive and each list must cover the whole image.
    pub fn with_dc_coverage(mut self, counts: Vec<Vec<u32>>) -> Self {
        self.lossy.get_or_insert_with(Default::default).dc_coverage = Some(counts);
        self
    }

    /// Whether independent ANS AC-unit models are enabled (default false).
    pub fn local_ans_contexts(&self) -> bool {
        self.lossy
            .as_ref()
            .is_some_and(|options| options.local_ans_contexts)
    }

    /// Uniform AC step multiplier, leaving DC steps unchanged. Supported values
    /// are 0.25, 0.5, 1, 2 and 4. Invalid or unrepresentable combinations fail
    /// when encoding. Default 1 preserves the existing bytes. This image-level
    /// control uses the existing channel/DC header fields, never an AQ map.
    /// It applies to every group; lossless/quick coding ignores it.
    pub fn with_ac_scale(mut self, scale: f32) -> Self {
        self.lossy.get_or_insert_with(Default::default).ac_scale = scale;
        self
    }

    /// The uniform AC step multiplier (default 1).
    pub fn ac_scale(&self) -> f32 {
        self.lossy.as_ref().map_or(1.0, |options| options.ac_scale)
    }

    /// Adjusts the lossless coder's expert settings in place, starting from
    /// the defaults when none are set:
    /// `cfg.tune_lossless(|t| t.palette = false)`.
    pub fn tune_lossless(mut self, f: impl FnOnce(&mut LosslessTuning)) -> Self {
        f(self.lossless.get_or_insert_with(Default::default));
        self
    }

    /// Adjusts the lossy coder's expert settings in place, starting from the
    /// defaults when none are set: `cfg.tune_lossy(|t| t.color = LossyColor::YCoCg)`.
    pub fn tune_lossy(mut self, f: impl FnOnce(&mut LossyTuning)) -> Self {
        f(&mut self.lossy.get_or_insert_with(Default::default).tuning);
        self
    }

    /// The quantizer scale a lossy coding resolves to for a `width` x
    /// `height` image coded in `color` (a calibrated target depends on both);
    /// `None` for a lossless coder.
    pub fn scale_for(&self, color: crate::LossyColor, width: u32, height: u32) -> Option<f32> {
        match self.coder {
            Coder::Lossy(q) => Some(q.to_scale(color, width, height)),
            _ => None,
        }
    }

    /// The decoder working memory this configuration's file of `info` needs,
    /// a conservative encoder estimate (see [`DecoderMemoryLimit`]).
    pub fn estimated_decoder_memory(&self, info: &ImageInfo) -> Result<u64, Error> {
        match self.lower() {
            Lowered::Lossless(c) => c.estimated_decoder_memory(info),
            Lowered::Lossy(c) => c.estimated_decoder_memory(info),
            Lowered::Quick(_) => Err(Error::Unsupported(
                "decoder memory estimate for quick units",
            )),
        }
    }

    // -- accessors ---------------------------------------------------------

    pub const fn coder(&self) -> Coder {
        self.coder
    }
    pub const fn effort(&self) -> f32 {
        self.effort
    }
    pub const fn decode_speed(&self) -> Option<DecodeSpeed> {
        self.decode_speed
    }
    pub const fn threads(&self) -> Option<usize> {
        self.threads
    }
    pub const fn checksum(&self) -> bool {
        self.checksum
    }
    pub const fn hash_algorithm(&self) -> HashAlgorithm {
        self.hash
    }
    /// The file checksum's algorithm when a checksum is written.
    pub const fn checksum_algorithm(&self) -> Option<HashAlgorithm> {
        if self.checksum { Some(self.hash) } else { None }
    }
    pub const fn unit_ordinals(&self) -> bool {
        self.unit_ordinals
    }
    pub const fn column_groups(&self) -> Option<u32> {
        self.column_groups
    }
    pub const fn decoder_memory_limit(&self) -> Option<DecoderMemoryLimit> {
        self.decoder_memory_limit
    }
    pub const fn row_groups(&self) -> Option<RowGroups> {
        self.row_groups
    }
    pub fn lossless_tuning(&self) -> Option<&LosslessTuning> {
        self.lossless.as_deref()
    }
    pub fn lossy_tuning(&self) -> Option<&LossyTuning> {
        self.lossy.as_deref().map(|options| &options.tuning)
    }

    /// `true` when decoding reproduces every stored sample bit: a lossless
    /// coding without a near-lossless error bound.
    pub fn is_exact(&self) -> bool {
        self.coder.is_lossless()
            && self
                .lossless
                .as_deref()
                .is_none_or(|t| t.max_error == [0; 4])
    }

    /// zencodec's streaming preference, resolved into [`RowGroups`] at
    /// `job()` time when the coding allows it.
    pub(crate) const fn prefer_streaming(&self) -> Option<bool> {
        self.prefer_streaming
    }
    pub(crate) fn set_prefer_streaming(&mut self, prefer: Option<bool>) {
        self.prefer_streaming = prefer;
    }

    // -- lowering to the coders' own settings -----------------------------

    /// The lossless coder's settings for this configuration.
    pub(crate) fn lossless_config(&self) -> crate::v1::EncodeConfig {
        let mut cfg = match &self.lossless {
            Some(t) => t.lowered(),
            None => crate::v1::EncodeConfig::default(),
        };
        cfg.effort = self.effort;
        cfg.threads = self.threads.unwrap_or(1);
        cfg.checksum = self.checksum;
        cfg.hash = self.hash;
        cfg.unit_ordinals = self.unit_ordinals;
        cfg.column_groups = self.column_groups;
        cfg.decoder_memory_limit = self.decoder_memory_limit;
        cfg
    }

    /// The lossy coder's settings for this configuration. The quality is
    /// the `Lossy` target, or the default scale for a lossless coding.
    pub(crate) fn lossy_config(&self) -> LossyConfig {
        let mut cfg = match &self.lossy {
            Some(t) => {
                let mut cfg = t.tuning.lowered();
                cfg.ac_scale = t.ac_scale;
                cfg.local_ans_contexts = t.local_ans_contexts;
                cfg.coding_defaults = t.coding_defaults;
                cfg.model_references = t.model_references;
                cfg.dc_coverage = t.dc_coverage.clone();
                cfg
            }
            None => LossyConfig::default(),
        };
        if let Coder::Lossy(quality) = self.coder {
            cfg = cfg.with_quality(quality);
        }
        cfg.effort = self.effort;
        cfg.threads = self.threads.unwrap_or(1);
        if let Some(speed) = self.decode_speed {
            // The tier owns the color: YCoCg skips the XYB transform for the
            // fastest decode, every other tier codes XYB.
            cfg.decode_speed = Some(speed);
            cfg.color = if speed == DecodeSpeed::Fastest {
                crate::LossyColor::YCoCg
            } else {
                crate::LossyColor::Xyb
            };
        }
        cfg.checksum = self.checksum;
        cfg.hash = self.hash;
        cfg.unit_ordinals = self.unit_ordinals;
        cfg.column_groups = self.column_groups;
        cfg.decoder_memory_limit = self.decoder_memory_limit;
        cfg
    }

    /// The quick coder's settings for this configuration.
    pub(crate) fn quick_config(&self) -> QuickConfig {
        let mut cfg = QuickConfig::default();
        if let Coder::Quick(bits) = self.coder {
            cfg.bit_smarts = bits;
        }
        if let Some(t) = &self.lossless
            && let Some(rows) = t.stripe_rows
        {
            cfg.stripe_rows = rows;
        }
        cfg.column_groups = self.column_groups.unwrap_or(0);
        cfg.threads = self.threads.unwrap_or(1);
        cfg.checksum = self.checksum;
        cfg.hash = self.hash;
        cfg
    }

    // -- encoding ---------------------------------------------------------

    /// Encodes one image to a complete IMI file.
    ///
    /// The pixel description (layout, sample type, alpha, CICP) comes from
    /// `pixels`; `meta` carries everything else. An ICC profile in the
    /// pixels' color context is embedded when `meta` has none. For
    /// cancellation, progress, resource limits and streaming output use the
    /// zencodec job: `config.job()`.
    pub fn encode(&self, pixels: &PixelSlice<'_>, meta: &Metadata) -> Result<Vec<u8>, Error> {
        let (info, meta) = crate::codec::native_description(pixels, meta)?;
        self.encode_samples(
            ImageRef::new(pixels.as_strided_bytes(), pixels.stride(), &info),
            &meta,
        )
    }

    /// [`Self::encode`] plus preview layers (for example a 50% downscale
    /// made by the caller), each stored as a complete stream before the
    /// main image so a reader can show it from a prefix of the file.
    /// Previews are coded with the lossless coder.
    pub fn encode_with_previews(
        &self,
        pixels: &PixelSlice<'_>,
        meta: &Metadata,
        previews: &[PixelSlice<'_>],
    ) -> Result<Vec<u8>, Error> {
        let (info, meta) = crate::codec::native_description(pixels, meta)?;
        let infos = previews
            .iter()
            .map(ImageInfo::from_pixels)
            .collect::<Result<Vec<_>, _>>()?;
        let views: Vec<ImageRef<'_>> = previews
            .iter()
            .zip(&infos)
            .map(|(p, i)| ImageRef::new(p.as_strided_bytes(), p.stride(), i))
            .collect();
        self.encode_samples_with_previews(
            ImageRef::new(pixels.as_strided_bytes(), pixels.stride(), &info),
            &meta,
            &views,
        )
    }

    /// [`Self::encode`] for raw samples described by an [`ImageInfo`].
    pub fn encode_samples(&self, image: ImageRef<'_>, meta: &Metadata) -> Result<Vec<u8>, Error> {
        self.encode_samples_with_progress(image, meta, &how_far::NoPulse)
    }

    /// Encode an exact V4 image with checked per-channel float guarantees.
    /// Guarantees are verified against the native sample bits before encoding.
    /// Pass masks in stored channel order, with zero masks for unused slots.
    pub fn encode_samples_with_float_guarantees(
        &self,
        image: ImageRef<'_>,
        meta: &Metadata,
        guarantees: [crate::FloatGuarantees; 4],
    ) -> Result<Vec<u8>, Error> {
        use crate::float_guarantees as domain;
        if guarantees == domain::EMPTY {
            return self.encode_samples(image, meta);
        }
        if !self.is_exact() {
            return Err(Error::Unsupported(
                "float guarantees require exact encoding",
            ));
        }
        domain::check_declaration(image.info, guarantees)?;
        domain::verify_source(image, guarantees)?;
        let file = self
            .clone()
            .with_checksum(false)
            .encode_samples(image, meta)?;
        let file = domain::write_header(file, image.info, guarantees)?;
        crate::checksum::finish_file(file, self.checksum_algorithm())
    }

    /// [`Self::encode_samples`] reporting progress through `pulse`.
    pub fn encode_samples_with_progress(
        &self,
        image: ImageRef<'_>,
        meta: &Metadata,
        pulse: &dyn how_far::Pulse,
    ) -> Result<Vec<u8>, Error> {
        if let Some(groups) = self.row_groups {
            return crate::stream_encode::encode_row_groups(
                image.pixels,
                image.stride,
                image.info,
                meta,
                self,
                groups,
                pulse,
            );
        }
        let file = self.encode_native_samples_with_progress(image, meta, pulse)?;
        pulse.check().map_err(|_| Error::Cancelled)?;
        let file = crate::v4::migrate::finish_encoded_image(file)?;
        pulse.check().map_err(|_| Error::Cancelled)?;
        Ok(file)
    }

    /// Encode the retained V3 grammar for explicit compatibility or migration
    /// fixtures. Ordinary encodes emit V4. This preserves the native encoder's
    /// previous framing and entropy selection, including legacy unit indexes.
    pub fn encode_legacy_samples(
        &self,
        image: ImageRef<'_>,
        meta: &Metadata,
    ) -> Result<Vec<u8>, Error> {
        self.encode_native_samples_with_progress(image, meta, &how_far::NoPulse)
    }
    /// Pixel-slice form of [`Self::encode_legacy_samples`].
    pub fn encode_legacy(
        &self,
        pixels: &PixelSlice<'_>,
        meta: &Metadata,
    ) -> Result<Vec<u8>, Error> {
        let (info, meta) = crate::codec::native_description(pixels, meta)?;
        self.encode_legacy_samples(
            ImageRef::new(pixels.as_strided_bytes(), pixels.stride(), &info),
            &meta,
        )
    }
    fn encode_native_samples_with_progress(
        &self,
        image: ImageRef<'_>,
        meta: &Metadata,
        pulse: &dyn how_far::Pulse,
    ) -> Result<Vec<u8>, Error> {
        let ImageRef {
            pixels,
            stride,
            info,
        } = image;
        if let Some(groups) = self.row_groups {
            crate::stream_encode::encode_legacy_row_groups(
                pixels, stride, info, meta, self, groups, pulse,
            )
        } else {
            self.encode_whole(pixels, stride, info, meta, pulse)
        }
    }

    /// [`Self::encode_with_previews`] for raw samples.
    pub fn encode_samples_with_previews(
        &self,
        image: ImageRef<'_>,
        meta: &Metadata,
        previews: &[ImageRef<'_>],
    ) -> Result<Vec<u8>, Error> {
        if previews.is_empty() {
            return self.encode_samples(image, meta);
        }
        if !matches!(self.coder, Coder::Lossless) || self.row_groups.is_some() {
            return Err(Error::Unsupported(
                "previews require whole-image lossless coding",
            ));
        }
        crate::v1::encode_with_previews(
            image.pixels,
            image.stride,
            image.info,
            meta,
            &self.lossless_config(),
            previews,
        )
        .and_then(crate::v4::migrate::finish_encoded)
    }

    /// The whole-image encode of the selected coder (row groups ignored).
    pub(crate) fn encode_whole(
        &self,
        pixels: &[u8],
        stride: usize,
        info: &ImageInfo,
        meta: &Metadata,
        pulse: &dyn how_far::Pulse,
    ) -> Result<Vec<u8>, Error> {
        match self.lower() {
            Lowered::Lossless(c) => {
                crate::v1::encode_with_progress(pixels, stride, info, meta, &c, pulse)
            }
            Lowered::Quick(c) => {
                crate::quick::encode_with_progress(pixels, stride, info, meta, &c, pulse)
            }
            Lowered::Lossy(c) => {
                crate::lossy::encode_lossy_with_progress(pixels, stride, info, meta, &c, pulse)
            }
        }
    }

    /// The selected coder with its complete settings.
    pub(crate) fn lower(&self) -> Lowered {
        match self.coder {
            Coder::Lossless => Lowered::Lossless(self.lossless_config()),
            Coder::Quick(_) => Lowered::Quick(self.quick_config()),
            Coder::Lossy(_) => Lowered::Lossy(self.lossy_config()),
        }
    }

    /// zencodec's view of the target: `Lossless` for an exact coding, the
    /// lossy target where zencodec has a name for it, `None` for a
    /// near-lossless bound or a target zencodec cannot express (the
    /// butteraugli 3-norm).
    pub fn fidelity(&self) -> Option<Fidelity> {
        match self.coder {
            Coder::Lossless | Coder::Quick(_) => self.is_exact().then_some(Fidelity::Lossless),
            Coder::Lossy(LossyQuality::Ssim2(q)) => {
                Some(Fidelity::Lossy(LossyTarget::ApproxSsim2(q)))
            }
            Coder::Lossy(LossyQuality::Butteraugli {
                distance,
                norm: crate::ButteraugliNorm::Max,
            }) => Some(Fidelity::Lossy(LossyTarget::ApproxButteraugli(distance))),
            Coder::Lossy(LossyQuality::Scale(q)) => {
                Some(Fidelity::Lossy(LossyTarget::CodecSpecificQuality(q)))
            }
            Coder::Lossy(_) => None,
        }
    }
}

/// A coder and its complete settings, as the kernels read them. Lab builds
/// make the lossy variant large; the value lives for one encode call.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "lab", allow(clippy::large_enum_variant))]
pub(crate) enum Lowered {
    Quick(QuickConfig),
    Lossless(crate::v1::EncodeConfig),
    Lossy(LossyConfig),
}

/// Expert settings of the lossless coder. Every field has the default the
/// coder would derive on its own; set only what you mean to override.
///
/// Fields are added over time: write literals as
/// `LosslessTuning { palette: false, ..Default::default() }`, or use
/// [`ImiEncoderConfig::tune_lossless`].
#[derive(Clone, Debug, PartialEq)]
pub struct LosslessTuning {
    /// Rows per stripe; `None` = automatic.
    pub stripe_rows: Option<u32>,
    /// rANS lanes per stripe; `None` = automatic.
    pub lanes: Option<u32>,
    /// Golomb parameter for samples of 8 bits or fewer.
    pub k_low: u32,
    /// Golomb parameter for wider samples.
    pub k_high: u32,
    /// Context-model mixing parameter.
    pub m: u32,
    /// Minimum run length for run coding.
    pub run_min: u32,
    /// Context count cap.
    pub max_ctx: usize,
    /// Cross-channel context modelling.
    pub ctx_cross: bool,
    /// Allowed color decorrelation modes, one bit each: bits 0..=3 are the
    /// fixed residual modes (none, R-G/B-G, R-G/B-R, R-G/B-avg(R,G)) and bit 4
    /// (`0x10`) is per-lane adaptive pixel color transform (8-bit RGB/RGBA).
    pub decor_mask: u8,
    /// Allowed spatial predictors, one bit each.
    pub predictor_mask: u8,
    /// Try palette coding for images with few distinct colors.
    pub palette: bool,
    /// Near-lossless bound: the largest absolute error allowed per channel
    /// (memory order). All zero (default) is exact; anything else is lossy
    /// and reported so by [`ImiEncoderConfig::is_exact`].
    pub max_error: [u32; 4],
    /// Shift float samples to their significant bits before coding.
    pub float_shift: bool,
    /// Try value-table coding for sparse sample alphabets.
    pub value_tables: bool,
    #[cfg(feature = "lab")]
    #[doc(hidden)]
    pub guide: Option<alloc::sync::Arc<Vec<Vec<u16>>>>,
    #[cfg(feature = "lab")]
    #[doc(hidden)]
    /// Lab bank identity; zero disables selection. No policy bits are packed here.
    pub seed_bank: u64,
    #[cfg(feature = "lab")]
    #[doc(hidden)]
    /// Compare explicit candidates before choosing the smallest complete file.
    pub seed_exact: bool,
}

impl Default for LosslessTuning {
    fn default() -> Self {
        let d = crate::v1::EncodeConfig::default();
        Self {
            stripe_rows: d.stripe_rows,
            lanes: d.lanes,
            k_low: d.k_low,
            k_high: d.k_high,
            m: d.m,
            run_min: d.run_min,
            max_ctx: d.max_ctx,
            ctx_cross: d.ctx_cross,
            decor_mask: d.decor_mask,
            predictor_mask: d.predictor_mask,
            palette: d.palette,
            max_error: d.max_error,
            float_shift: d.float_shift,
            value_tables: d.value_tables,
            #[cfg(feature = "lab")]
            guide: d.guide,
            #[cfg(feature = "lab")]
            seed_bank: d.seed_bank,
            #[cfg(feature = "lab")]
            seed_exact: d.seed_exact,
        }
    }
}

impl LosslessTuning {
    /// One error bound for every channel; see [`Self::max_error`].
    pub fn with_max_error(mut self, max_error: u32) -> Self {
        self.max_error = [max_error; 4];
        self
    }
    pub fn with_channel_max_errors(mut self, max_error: [u32; 4]) -> Self {
        self.max_error = max_error;
        self
    }
    pub fn with_stripe_rows(mut self, rows: Option<u32>) -> Self {
        self.stripe_rows = rows;
        self
    }
    pub fn with_lanes(mut self, lanes: Option<u32>) -> Self {
        self.lanes = lanes;
        self
    }
    pub fn with_palette(mut self, enabled: bool) -> Self {
        self.palette = enabled;
        self
    }
    pub fn with_predictor_mask(mut self, mask: u8) -> Self {
        self.predictor_mask = mask;
        self
    }
    pub fn with_decor_mask(mut self, mask: u8) -> Self {
        self.decor_mask = mask;
        self
    }

    pub(crate) fn lowered(&self) -> crate::v1::EncodeConfig {
        let t = self.clone();
        crate::v1::EncodeConfig {
            stripe_rows: t.stripe_rows,
            lanes: t.lanes,
            k_low: t.k_low,
            k_high: t.k_high,
            m: t.m,
            run_min: t.run_min,
            max_ctx: t.max_ctx,
            ctx_cross: t.ctx_cross,
            decor_mask: t.decor_mask,
            predictor_mask: t.predictor_mask,
            palette: t.palette,
            max_error: t.max_error,
            float_shift: t.float_shift,
            value_tables: t.value_tables,
            #[cfg(feature = "lab")]
            guide: t.guide,
            #[cfg(feature = "lab")]
            seed_bank: t.seed_bank,
            #[cfg(feature = "lab")]
            seed_exact: t.seed_exact,
            ..crate::v1::EncodeConfig::default()
        }
    }
}

/// Expert settings of the lossy coder. Every field has the default the
/// coder would derive from effort and decode speed on its own.
///
/// Fields are added over time: write literals as
/// `LossyTuning { color: LossyColor::YCoCg, ..Default::default() }`, or use
/// [`ImiEncoderConfig::tune_lossy`].
#[derive(Clone, Debug, PartialEq)]
pub struct LossyTuning {
    /// Color coding: XYB (default) or YCoCg.
    pub color: crate::LossyColor,
    /// Per-color-plane transform and sampling.
    pub channel_layouts: [crate::LossyChannelLayout; 3],
    /// Compact rANS restart states where smaller (default on).
    pub compact_streams: bool,
    /// Stripe height in block rows; 0 (default) = automatic per decode speed.
    pub stripe_block_rows: u32,
    /// AC entropy coding (ignored when a decode speed is set).
    pub entropy: crate::LossyEntropy,
    /// Exact-block ratio; `None` = the decode-speed tier decides (`Compact`
    /// selects exact blocks automatically — at effort `>= 9.0`, only for
    /// quality scale `>= 60`; set `Some(ratio)` to force or `Some(0.0)` to
    /// disable).
    pub exact_blocks: Option<f32>,
    /// Explicit 4x4 sub-block transforms for `Compact` U8 stills below effort
    /// `9.0` (rejected at effort `>= 9.0`, where [`DecodeSpeed::Compact`] and
    /// [`Self::adaptive_partitions`] enable 4x4 splits automatically).
    pub small_transforms: bool,
    /// Frequency-dependent dead-zone quantization bias (`0.46` at low AC
    /// frequencies tapering to `0.33` at high AC frequencies).
    pub freq_zero_bias: bool,
    /// Joint rate-distortion selection of `16x16`/`16x8`/`8x16` superblocks
    /// and `4x4` transforms with 9-context partition-map DP across all efforts
    /// and entropy modes (`RunLevel` and `SplitHuf`). [`DecodeSpeed::Compact`]
    /// enables the same adaptive RDO ladder automatically at effort `>= 9.0`.
    pub adaptive_partitions: bool,
    /// Explicit color transform; `None` = automatic. Needs `color ==
    /// LossyColor::YCoCg`; [`Self::with_color_transform`] sets that.
    pub color_transform: Option<crate::lossy::ColorTransform>,
    #[doc(hidden)]
    pub round: f32,
    #[doc(hidden)]
    pub max_ac_tables: usize,
    #[doc(hidden)]
    pub channel_mul: Option<[f32; 3]>,
    #[doc(hidden)]
    pub aq_gamma: f32,
    #[doc(hidden)]
    pub aq_r0: f32,
    #[doc(hidden)]
    pub matrix: u8,
    #[cfg(feature = "lab")]
    #[doc(hidden)]
    pub lab: Box<LossyLab>,
}

/// Lab-only lossy switches, boxed so a `lab` build does not change the
/// size of [`LossyTuning`] by more than one pointer.
#[cfg(feature = "lab")]
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct LossyLab {
    pub lf_pred: u8,
    pub rdoq_lambda: f32,
    pub rdoq_policy: u8,
    pub trellis: Option<crate::LossyTrellis>,
    pub aq_map: Option<alloc::sync::Arc<[u8]>>,
    pub aq_field: Option<crate::LossyAqField>,
    /// Lab bank identity; zero disables selection. No policy bits are packed here.
    pub seed_bank: u64,
    #[cfg(feature = "lab")]
    #[doc(hidden)]
    /// Compare explicit candidates before choosing the smallest complete file.
    pub seed_exact: bool,
    pub switches: crate::LossyLabSwitches,
}

impl Default for LossyTuning {
    fn default() -> Self {
        let d = LossyConfig::default();
        Self {
            color: d.color,
            channel_layouts: d.channel_layouts,
            compact_streams: d.compact_streams,
            stripe_block_rows: d.stripe_block_rows,
            entropy: d.entropy,
            exact_blocks: d.exact_blocks,
            small_transforms: d.small_transforms,
            freq_zero_bias: d.freq_zero_bias,
            adaptive_partitions: d.adaptive_partitions,
            color_transform: d.color_transform,
            round: d.round,
            max_ac_tables: d.max_ac_tables,
            channel_mul: d.channel_mul,
            aq_gamma: d.aq_gamma,
            aq_r0: d.aq_r0,
            matrix: d.matrix,
            #[cfg(feature = "lab")]
            lab: Box::new(LossyLab {
                lf_pred: d.lf_pred,
                rdoq_lambda: d.rdoq_lambda,
                rdoq_policy: d.rdoq_policy,
                trellis: d.trellis,
                aq_map: d.aq_map,
                aq_field: d.aq_field,
                seed_bank: d.seed_bank,
                #[cfg(feature = "lab")]
                seed_exact: d.seed_exact,
                switches: d.lab,
            }),
        }
    }
}

impl LossyTuning {
    pub fn with_color(mut self, color: crate::LossyColor) -> Self {
        self.color = color;
        self
    }
    pub fn with_channel_layouts(mut self, layouts: [crate::LossyChannelLayout; 3]) -> Self {
        self.channel_layouts = layouts;
        self
    }
    pub fn with_compact_streams(mut self, enabled: bool) -> Self {
        self.compact_streams = enabled;
        self
    }
    pub fn with_stripe_block_rows(mut self, rows: u32) -> Self {
        self.stripe_block_rows = rows;
        self
    }
    pub fn with_entropy(mut self, entropy: crate::LossyEntropy) -> Self {
        self.entropy = entropy;
        self
    }
    pub fn with_exact_blocks(mut self, ratio: Option<f32>) -> Self {
        self.exact_blocks = ratio;
        self
    }
    pub fn with_small_transforms(mut self, enabled: bool) -> Self {
        self.small_transforms = enabled;
        self
    }
    pub fn with_freq_zero_bias(mut self, enabled: bool) -> Self {
        self.freq_zero_bias = enabled;
        self
    }
    pub fn with_adaptive_partitions(mut self, enabled: bool) -> Self {
        self.adaptive_partitions = enabled;
        self
    }
    /// An explicit color transform needs YCoCg coding: setting one switches
    /// `color` to `YCoCg`, and `Identity` gives every channel unit weight
    /// unless `channel_mul` is already set.
    pub fn with_color_transform(mut self, transform: Option<crate::lossy::ColorTransform>) -> Self {
        self.color_transform = transform;
        if transform.is_some() {
            self.color = crate::LossyColor::YCoCg;
        }
        if transform == Some(crate::lossy::ColorTransform::Identity) && self.channel_mul.is_none() {
            self.channel_mul = Some([1.0, 1.0, 1.0]);
        }
        self
    }

    pub(crate) fn lowered(&self) -> LossyConfig {
        let t = self.clone();
        LossyConfig {
            color: t.color,
            channel_layouts: t.channel_layouts,
            compact_streams: t.compact_streams,
            stripe_block_rows: t.stripe_block_rows,
            entropy: t.entropy,
            exact_blocks: t.exact_blocks,
            small_transforms: t.small_transforms,
            freq_zero_bias: t.freq_zero_bias,
            adaptive_partitions: t.adaptive_partitions,
            color_transform: t.color_transform,
            round: t.round,
            max_ac_tables: t.max_ac_tables,
            channel_mul: t.channel_mul,
            aq_gamma: t.aq_gamma,
            aq_r0: t.aq_r0,
            matrix: t.matrix,
            #[cfg(feature = "lab")]
            lf_pred: t.lab.lf_pred,
            #[cfg(feature = "lab")]
            rdoq_lambda: t.lab.rdoq_lambda,
            #[cfg(feature = "lab")]
            rdoq_policy: t.lab.rdoq_policy,
            #[cfg(feature = "lab")]
            trellis: t.lab.trellis,
            #[cfg(feature = "lab")]
            aq_map: t.lab.aq_map,
            #[cfg(feature = "lab")]
            aq_field: t.lab.aq_field,
            #[cfg(feature = "lab")]
            seed_bank: t.lab.seed_bank,
            #[cfg(feature = "lab")]
            seed_exact: t.lab.seed_exact,
            #[cfg(feature = "lab")]
            lab: t.lab.switches,
            ..LossyConfig::default()
        }
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    /// The configuration is a stack value; boxed tuning keeps it that way in
    /// every feature combination.
    #[test]
    fn size_budget() {
        assert!(
            core::mem::size_of::<ImiEncoderConfig>() <= 128,
            "ImiEncoderConfig is {} bytes",
            core::mem::size_of::<ImiEncoderConfig>()
        );
        assert!(core::mem::size_of::<Coder>() <= 16);
    }

    /// Lowering an untouched configuration gives the coders' own defaults,
    /// so the byte gate's default encodes are unchanged.
    #[test]
    fn lowering_defaults_are_the_coders_defaults() {
        let cfg = ImiEncoderConfig::default();
        assert_eq!(cfg.lossless_config(), crate::v1::EncodeConfig::default());
        assert_eq!(cfg.lossy_config(), LossyConfig::default());
        assert_eq!(cfg.quick_config(), QuickConfig::default());
        let tuned = ImiEncoderConfig::default()
            .with_lossless_tuning(LosslessTuning::default())
            .with_lossy_tuning(LossyTuning::default());
        assert_eq!(tuned.lossless_config(), crate::v1::EncodeConfig::default());
        assert_eq!(tuned.lossy_config(), LossyConfig::default());
    }

    #[test]
    fn quality_switches_to_lossy() {
        let cfg = ImiEncoderConfig::default().with_quality(80.0);
        assert!(matches!(cfg.coder(), Coder::Lossy(LossyQuality::Scale(q)) if q == 80.0));
        assert!(!cfg.is_exact());
        assert!(cfg.with_coder(Coder::Lossless).is_exact());
        let near = ImiEncoderConfig::lossless()
            .with_lossless_tuning(LosslessTuning::default().with_max_error(2));
        assert!(!near.is_exact());
    }
}
