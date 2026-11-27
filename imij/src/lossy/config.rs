//! Lossy encoder configuration, reconstruction bias, color transform, and lab
//! experiment switches.

#[cfg(feature = "encode")]
use alloc::vec::Vec;

#[cfg(feature = "lab")]
use super::LossyTrellis;
#[cfg(feature = "lab")]
use super::{
    DC_LANES_LAB, DC_NEAREST, DC_STEP_OVERRIDE, DC_STRIPES_LAB, TABLE_K_DEFAULT, TABLE_K_SEARCH,
    dering, trellis,
};
#[cfg(feature = "encode")]
use super::{
    LossyChannelLayout, LossyColor, LossyEntropy, LossyQuality, NCH, lossy_group_cols,
    model_candidates,
};
use super::{coef, color};
#[cfg(feature = "encode")]
use crate::{Error, ImageInfo};

/// Lossy 0.4 signalled linear color transform (the parameter envelope's
/// transform byte): the decoder applies `m[j] = [M_j0, M_j1, M_j2, O_j]` to
/// the unscaled coded planes, `rgb_j = M_j0*c0 + M_j1*c1 + M_j2*c2 + O_j`
/// (code-value domain, no FMA). [`Self::YCoCg`] is today's lifting, so its
/// pixels are unchanged; `XYB` files never carry one.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColorTransform {
    /// Identity RGB (id 1).
    Identity,
    /// YCbCr BT.601 (id 2).
    Bt601,
    /// YCbCr BT.709 (id 3).
    Bt709,
    /// YCoCg (id 4): the same pixels as today's lifting, explicitly
    /// signalled.
    YCoCg,
    /// An explicit decode matrix `[M | O]` (id 63), finite and bounded.
    Explicit([[f32; 4]; 3]),
}

impl ColorTransform {
    /// The envelope's transform byte.
    #[cfg(feature = "encode")]
    pub(crate) fn id(self) -> u8 {
        match self {
            Self::Identity => 1,
            Self::Bt601 => 2,
            Self::Bt709 => 3,
            Self::YCoCg => 4,
            Self::Explicit(_) => 63,
        }
    }

    /// The normative decode matrix, or `None` for the YCoCg lifting.
    pub(crate) fn decode(self) -> Option<[[f32; 4]; 3]> {
        match self {
            Self::Identity => Some(color::TRANSFORM_IDENTITY),
            Self::Bt601 => Some(color::TRANSFORM_BT601),
            Self::Bt709 => Some(color::TRANSFORM_BT709),
            Self::YCoCg => None,
            Self::Explicit(m) => Some(m),
        }
    }

    /// The encoder's forward `[A | B]` (`coded = A * rgb + B`), or `None`
    /// for the YCoCg lifting. `A` is the algebraic inverse of the decode
    /// matrix, computed in f64 from the normative f32 values.
    #[cfg(feature = "encode")]
    pub(crate) fn forward(self) -> Option<[[f32; 4]; 3]> {
        match self {
            Self::YCoCg => None,
            Self::Identity => Some(color::TRANSFORM_IDENTITY),
            t => color::forward_linear(&t.decode()?),
        }
    }

    /// Whether an explicit matrix is finite and bounded.
    pub(crate) fn explicit_valid(m: &[[f32; 4]; 3]) -> bool {
        m.iter()
            .flatten()
            .all(|v| v.is_finite() && v.abs() <= 65536.0)
    }
}

/// Reconstruction bias (Lossy 0.4): a nonzero AC coefficient q of channel c
/// at zigzag index z reconstructs at `(q - sign(q) * B) x step` instead of
/// `q x step`, with `B = b[c][z >= k][|q| >= 2] / 256` (positive B moves
/// toward zero). DC and zero coefficients are unchanged.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReconBias {
    /// First zigzag index of the high band, 2..=63; 64 = one band.
    pub k: u8,
    /// [channel][band][|q| = 1, |q| >= 2], in 1/256 of a step.
    pub b: [[[i8; 2]; 2]; 3],
}

impl ReconBias {
    /// Every bias zero: reconstruction as without a bias.
    pub(crate) fn is_zero(&self) -> bool {
        self.b.iter().flatten().flatten().all(|&v| v == 0)
    }

    /// Per channel, raster order: the bias B for |q| = 1 and for |q| >= 2
    /// (0 at DC). Exact in f32.
    pub(crate) fn tables(&self) -> [[[f32; 64]; 2]; 3] {
        let mut t = [[[0.0f32; 64]; 2]; 3];
        for (c, tc) in t.iter_mut().enumerate() {
            for z in 1..64 {
                let band = (z >= self.k as usize) as usize;
                let i = coef::ZIGZAG[z] as usize;
                for m in 0..2 {
                    tc[m][i] = self.b[c][band][m] as f32 * (1.0 / 256.0);
                }
            }
        }
        t
    }
}

/// LAB: the encoder's running sums for the per-image centroid bias: per
/// channel, band and |q| group (1, >= 2) the total of `|q| - |c| / step` and
/// the count.
#[derive(Clone, Copy, Debug, Default)]
#[cfg(feature = "encode")]
pub(super) struct BiasSums {
    pub(super) sum: [[[f64; 2]; 2]; 3],
    pub(super) count: [[[u64; 2]; 2]; 3],
}

#[cfg(feature = "encode")]
impl BiasSums {
    /// Adds one block row's quantized coefficients of channel `ch`.
    /// `coefs`: the row's transform coefficients (`bw * 64`, raster);
    /// `q`: the stripe's quantized coefficients (`(block * 3 + ch) * 64`);
    /// `inv_step`: the channel's inverse steps before AQ; `aq`: the row's
    /// per-block AC multipliers (empty = none); `k_split`: the bias band
    /// split (64 = one band).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn add_row(
        &mut self,
        coefs: &[f32],
        q: &[i32],
        bw: usize,
        ch: usize,
        inv_step: &[f32; 64],
        aq: &[f32],
        k_split: u8,
    ) {
        for bx in 0..bw {
            let aqm = aq.get(bx).copied().unwrap_or(1.0) as f64;
            let c = &coefs[bx * 64..][..64];
            let qr = &q[(bx * NCH + ch) * 64..][..64];
            for kz in 1..64 {
                let z = coef::ZIGZAG[kz] as usize;
                let qv = qr[z];
                if qv == 0 {
                    continue;
                }
                let a = (c[z] as f64).abs() * (inv_step[z] as f64) / aqm;
                let band = (kz >= k_split as usize) as usize;
                let g = (qv.unsigned_abs() >= 2) as usize;
                self.sum[ch][band][g] += qv.unsigned_abs() as f64 - a;
                self.count[ch][band][g] += 1;
            }
        }
    }

    /// The per-image centroid bias in 1/256 steps: the mean of
    /// `|q| - |c| / step` per channel, band and |q| group, rounded and
    /// clamped to i8; cells without coefficients stay 0.
    #[cfg(feature = "lab")]
    pub(super) fn bias(&self, k_split: u8) -> ReconBias {
        let mut b = [[[0i8; 2]; 2]; 3];
        for (c, bc) in b.iter_mut().enumerate() {
            for band in 0..2 {
                for g in 0..2 {
                    let n = self.count[c][band][g];
                    if n > 0 {
                        bc[band][g] = ((self.sum[c][band][g] / n as f64) * 256.0)
                            .round()
                            .clamp(-128.0, 127.0) as i8;
                    }
                }
            }
        }
        ReconBias { k: k_split, b }
    }
}

/// LAB: an AQ map from jpegli's adaptive quantization field
/// (`LossyConfig::aq_field`): per block the AC step multiplier
/// `(r / qf)^strength`, `r` the field's geometric mean (`relative`) or
/// jpegli's neutral 0.6, averaged over `cell` x `cell` blocks, snapped to
/// `levels` per octave (0 = the a/64 grid), within [1/4, 4].
#[cfg(feature = "lab")]
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LossyAqField {
    pub strength: f32,
    pub levels: u8,
    pub cell: u8,
    pub relative: bool,
    /// jpegli's fade of the field toward a constant at coarse steps.
    pub fade: bool,
}

/// LAB: encoder switches one encode carries ([`LossyConfig::lab`]). A `None`
/// field takes the process-global value of its `crate::lab::set_*` function,
/// read once when the encode starts; `Some` applies to this encode only, on
/// every thread it uses. Harnesses that encode different configurations on
/// several threads at once (imij-cmp `ladder-batch`) set every field, so no
/// encode can see another's switch (benchmarks/raceaudit_2026-10-09).
#[cfg(feature = "lab")]
#[doc(hidden)]
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct LossyLabSwitches {
    /// DC step multipliers in 1/64 units, luma then chroma (0 keeps the
    /// default; `lab::set_lossy_dc_step`).
    pub dc_step: Option<[u8; 2]>,
    /// Overshoot deringing (`lab::set_lossy_dering`; default on).
    pub dering: Option<bool>,
    /// Nearest, clamp-aware DC rounding (`lab::set_lossy_dc_nearest`;
    /// default on).
    pub dc_nearest: Option<bool>,
    /// Lossy stripes per DC stripe (0 = automatic;
    /// `lab::set_lossy_dc_stripes`).
    pub dc_stripes: Option<u32>,
    /// Lanes of the nested DC image (0 = the lossless default, at most 16;
    /// `lab::set_lossy_dc_lanes`).
    pub dc_lanes: Option<u32>,
    /// The trellis's lambda factor m^k per AQ multiplier m (0 = off;
    /// `lab::set_trellis_aq_lambda_exp`).
    pub trellis_aq_lambda_exp: Option<f32>,
    /// Per-channel offsets of the trellis's lambda log scale 1
    /// (`lab::set_trellis_channel_shift`).
    pub trellis_channel_shift: Option<[f32; 3]>,
    /// The effort's trellis on XYB split-Huffman files
    /// (`lab::set_trellis_huf_rate`; default on).
    pub trellis_huf_rate: Option<bool>,
    /// How the encoder picks its table counts, 0..=3
    /// (`lab::set_table_k_mode`): 0 the effort's budgets, 1 merged down by
    /// estimate, 2 searched for every file, 3 searched where the tables weigh;
    /// [`Self::TABLE_K_PRODUCTION`] no override (the production rule, the
    /// default). Values 4..=254 act as 3.
    pub table_k_mode: Option<u8>,
    /// Base tables of matrix [`MATRIX_EXPLICIT`] (raster order), rounded
    /// through the head record's coding before use so encoder and decoder
    /// quantize with identical steps. `None` (default): off; `LossyConfig::
    /// matrix` selects the id. Boxed: the tables are 768 bytes.
    pub explicit_matrix: Option<alloc::boxed::Box<[[f32; 64]; 3]>>,
    /// Fixed reconstruction bias ([`ReconBias`]); `None` (default): off.
    /// Excludes [`Self::recon_bias_centroid`].
    pub recon_bias: Option<ReconBias>,
    /// Per-image reconstruction bias: `Some(k)` computes, per channel and
    /// band (split at zigzag index `k`, 64 = one band), the mean of
    /// `|q| - |c| / step` over the image's nonzero coefficients with
    /// `|q| = 1` and with `|q| >= 2`, in 1/256 of a step. `None` (default):
    /// off. Excludes [`Self::recon_bias`].
    pub recon_bias_centroid: Option<u8>,
    /// Signalled linear color transform (feature `color_transform`, YCoCg
    /// code-value input only): replaces the YCoCg lifting with the given
    /// transform. `None` (default): off. The YCoCg variant writes the
    /// explicit id but decodes through the same lifting, so its pixels are
    /// today's exactly.
    pub color_transform: Option<ColorTransform>,
    /// Superblocks (`superblock.rs`): `Some(true)` merges on any tier and
    /// image size wherever the format allows (default layouts, no keep
    /// blocks, no trellis), `Some(false)` never; `None` follows the
    /// production rule ([`superblocks_on`]).
    pub superblocks: Option<bool>,
    /// The merged shapes the superblock selector may choose: bit 0 16x16,
    /// bit 1 16x8, bit 2 8x16 (`None`: all).
    pub superblock_shapes: Option<u8>,
    /// Small transforms (`small.rs`, SPEC 2.11): the study's switches
    /// ([`SmallLab`]), boxed; `None`: the production rule throughout.
    pub small: Option<alloc::boxed::Box<SmallLab>>,
}

/// Adaptive-partition mode for this lossy configuration.
#[cfg(feature = "encode")]
pub(crate) fn adaptive_mode(cfg: &LossyConfig) -> crate::effort::AdaptiveMode {
    if cfg.adaptive_partitions {
        crate::effort::AdaptiveMode::Explicit
    } else if cfg.decode_speed == Some(crate::DecodeSpeed::Compact) {
        crate::effort::AdaptiveMode::Compact
    } else {
        crate::effort::AdaptiveMode::Off
    }
}

/// Whether adaptive partitions and surrounding-block adaptive RDO are enabled
/// for this lossy configuration: either explicit `adaptive_partitions` or the
/// `DecodeSpeed::Compact` tier (consistent across all efforts).
#[cfg(feature = "encode")]
pub(crate) fn adaptive_enabled(cfg: &LossyConfig) -> bool {
    adaptive_mode(cfg) != crate::effort::AdaptiveMode::Off
}

/// LAB switches of small transforms (4x4 planes, SPEC 2.11).
#[cfg(feature = "lab")]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SmallLab {
    /// `Some(true)` lets the selector code planes as four 4x4 DCTs wherever
    /// the format allows, `Some(false)` never; `None` follows the
    /// production rule (`small_on`).
    pub on: Option<bool>,
    /// With small transforms on: code the planes of this mask (bit c =
    /// plane c) as 4x4 in every eligible parent instead of pricing them
    /// (fixed-layout and worst-case arms). `None`: the selector decides.
    pub split: Option<u8>,
    /// The 4x4 planes use the integer candidate inverse (`small_lab.rs`,
    /// the normative-arithmetic study); its files carry a lab payload in
    /// head record 23 that production decoders reject.
    pub int: bool,
    /// The selector's distortion weight for the 8x8 / 4x4 choice relative
    /// to the merge decision's (`None`: the production value).
    pub lambda: Option<f32>,
    /// The selector's inner-edge weight for the 8x8 / 4x4 choice (`None`:
    /// the production value).
    pub edge: Option<f32>,
}

#[cfg(feature = "lab")]
impl SmallLab {
    /// Small transforms on, every other switch at its production value.
    pub fn on() -> Self {
        Self {
            on: Some(true),
            ..Self::default()
        }
    }
}

#[cfg(feature = "lab")]
impl LossyLabSwitches {
    /// [`Self::table_k_mode`] without an override: the encoder's production
    /// rule (mode 3 from effort 7.5 for run-level and split-Huffman).
    pub const TABLE_K_PRODUCTION: u8 = TABLE_K_DEFAULT;

    /// Every switch `Some` at its default, the encoder's production behaviour
    /// (the table-count search from effort 7.5, the split-Huffman trellis at
    /// efforts 9 and 10), whatever the process-global switches hold: the
    /// starting point of a harness that encodes several configurations at once.
    pub fn defaults() -> Self {
        Self {
            dc_step: Some([0, 0]),
            dering: Some(true),
            dc_nearest: Some(true),
            dc_stripes: Some(0),
            dc_lanes: Some(0),
            trellis_aq_lambda_exp: Some(0.0),
            trellis_channel_shift: Some([0.0; 3]),
            trellis_huf_rate: Some(true),
            table_k_mode: Some(Self::TABLE_K_PRODUCTION),
            // No process-global fallback: `None` is the production value.
            explicit_matrix: None,
            recon_bias: None,
            recon_bias_centroid: None,
            color_transform: None,
            superblocks: None,
            superblock_shapes: None,
            small: None,
        }
    }

    /// Every switch at the value an encode starting now uses: the `Some`
    /// fields, the process-global switches for the others.
    pub(crate) fn pinned(self) -> Self {
        use core::sync::atomic::Ordering::Relaxed;
        let shift = trellis::CH_SHIFT
            .each_ref()
            .map(|s| f32::from_bits(s.load(Relaxed)));
        Self {
            dc_step: Some(
                self.dc_step
                    .unwrap_or_else(|| DC_STEP_OVERRIDE.each_ref().map(|s| s.load(Relaxed))),
            ),
            dering: Some(self.dering.unwrap_or_else(|| dering::ENABLED.load(Relaxed))),
            dc_nearest: Some(self.dc_nearest.unwrap_or_else(|| DC_NEAREST.load(Relaxed))),
            dc_stripes: Some(
                self.dc_stripes
                    .unwrap_or_else(|| DC_STRIPES_LAB.load(Relaxed)),
            ),
            dc_lanes: Some(
                self.dc_lanes
                    .unwrap_or_else(|| DC_LANES_LAB.load(Relaxed))
                    .min(16),
            ),
            trellis_aq_lambda_exp: Some(
                self.trellis_aq_lambda_exp
                    .unwrap_or_else(|| f32::from_bits(trellis::AQ_LAMBDA_EXP.load(Relaxed))),
            ),
            trellis_channel_shift: Some(self.trellis_channel_shift.unwrap_or(shift)),
            trellis_huf_rate: Some(
                self.trellis_huf_rate
                    .unwrap_or_else(|| trellis::HUF_RATE.load(Relaxed)),
            ),
            table_k_mode: Some(
                match self
                    .table_k_mode
                    .unwrap_or_else(|| TABLE_K_SEARCH.load(Relaxed))
                {
                    TABLE_K_DEFAULT => TABLE_K_DEFAULT,
                    m => m.min(3),
                },
            ),
            explicit_matrix: self.explicit_matrix,
            recon_bias: self.recon_bias,
            recon_bias_centroid: self.recon_bias_centroid,
            color_transform: self.color_transform,
            superblocks: self.superblocks,
            superblock_shapes: self.superblock_shapes,
            small: self.small,
        }
    }
}

/// Lossy encoder settings.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
#[cfg(feature = "encode")]
pub(crate) struct LossyConfig {
    /// Quantizer scale (unitless, libjpeg-style; [`LossyQuality::Scale`]):
    /// steps scale by `50 / q` below 50 and `(200 - 2q) / 100` from 50 to
    /// 100 with a floor of one 8-bit unit; above 100 the floor keeps
    /// shrinking (104 halves it, 108 quarters it, max 116). q100 is not
    /// lossless. [`Self::quality_target`] overrides it when set.
    pub quality: f32,
    /// A quality setting in another unit ([`LossyQuality`]: a SSIMULACRA2 or
    /// Butteraugli target, or a jpegli quality / JPEG XL distance
    /// equivalent), resolved to a scale at encode time against the image
    /// size and the color mode the encoder uses. `None` encodes
    /// [`Self::quality`]. Set by [`Self::with_quality`].
    pub quality_target: Option<LossyQuality>,
    pub color: LossyColor,
    /// Per-color-plane transform and sampling, in Y/X/B−Y or Y/Co/Cg order.
    pub channel_layouts: [LossyChannelLayout; 3],
    /// Compact rANS restart states where they are smaller (run-level
    /// entropy), enabled by default. Disable to always store full
    /// restart states.
    pub compact_streams: bool,
    pub local_ans_contexts: bool,
    pub coding_defaults: bool,
    pub model_references: bool,
    pub dc_coverage: Option<Vec<Vec<u32>>>,
    pub catalog_lab: Option<model_candidates::Settings>,
    /// Stripe height in 8-pixel block rows (independently decodable unit),
    /// 1..=255; 0 (default) = automatic: the tallest stripe whose area
    /// (column-group width x rows) fits the decode-speed tier's budget,
    /// from 1 block row up to 8 (4 for `Fast`/`Fastest`). Taller stripes
    /// cost fewer bytes per stripe boundary; shorter ones keep streaming
    /// memory and latency low and give threads more work items
    /// (benchmarks/lossy_stripe_decode_2026-10-06). See [`Self::stripe_rows_for`].
    pub stripe_block_rows: u32,
    /// Encoder threads.
    pub threads: usize,
    /// Encoder search effort, finite 0..=10, including fractional values.
    /// Controls entropy table counts, clustering passes, lossless DC/alpha
    /// search and, from effort 9, the AC trellis (zenjpeg's rate-distortion
    /// quantizer, priced with the file's own entropy tables; one pass at 9,
    /// two at 10) on XYB files coded run-level. Default 7.5 balances encode
    /// cost and size.
    ///
    /// Lossy effort may change pixels. Below 9, decoded pixels are identical
    /// at a fixed quality and color configuration; from 9 the trellis codes
    /// fewer or smaller AC coefficients: about 3-5% smaller files at equal
    /// perceptual quality for 2-8x the encode time. Alpha stays lossless at
    /// every effort, and lossless encodes ([`crate::EncodeConfig::effort`])
    /// never change samples. See docs/ENCODER_EFFORT.md.
    pub effort: f32,
    /// Quantization rounding offset (0.5 = round to nearest).
    #[doc(hidden)]
    pub round: f32,
    /// Maximum AC tables (decode speed vs size).
    #[doc(hidden)]
    pub max_ac_tables: usize,
    /// Per-channel step multipliers overriding the color's defaults
    /// (tuning experiments).
    #[doc(hidden)]
    pub channel_mul: Option<[f32; 3]>,
    /// Encoder-only reparameterization of existing channel/DC fields.
    pub ac_scale: f32,
    /// Adaptive quantization strength (0 = off): per-block AC step
    /// multiplier (activity / (aq_r0 * luma step))^aq_gamma, in [0.5, 2].
    #[doc(hidden)]
    pub aq_gamma: f32,
    #[doc(hidden)]
    pub aq_r0: f32,
    /// AC entropy coding (ignored when `decode_speed` is set).
    pub entropy: LossyEntropy,
    /// Decode-complexity tier ([`LossyConfig::with_decode_speed`]); picks
    /// the entropy mode per image.
    pub decode_speed: Option<crate::DecodeSpeed>,
    /// Base quantization matrix id (see `QParams::base`); 255 (default) =
    /// automatic: jpegli XYB shapes (2) for XYB, distance-dependent matrix 5
    /// for YCoCg; YCoCg falls back to Annex K (0) if matrix 5 exceeds bounds.
    #[doc(hidden)]
    pub matrix: u8,
    /// Column groups: stripes split into independent column ranges of this
    /// many pixels (rounded down to a multiple of 8), each with its own
    /// lanes and records. `None` = automatic, `Some(0)` = never.
    pub column_groups: Option<u32>,
    /// Optional decoder working-memory target; see [`crate::DecoderMemoryLimit`].
    pub decoder_memory_limit: Option<crate::DecoderMemoryLimit>,
    /// Write a file checksum (`cSUM` chunk before `DATA`, SPEC 1.4): XXH3-128
    /// of every 64 KiB to 1 MiB piece of the file. Off by default. Nested
    /// images (previews, frames, plane tiles) never get one: the outer
    /// file's covers them.
    pub checksum: bool,
    /// The checksum's digest algorithm.
    pub hash: crate::HashAlgorithm,
    /// Prefix every main unit record in `DATA` with its 0-based ordinal
    /// (head record 37, feature `unit_ordinals`, SPEC 2.7), so indexed
    /// region readers verify records locally without reading every record
    /// head at open. Off by default.
    pub unit_ordinals: bool,
    /// Exact blocks (8-bit RGB and RGBA only): 8x8 blocks whose estimated
    /// lossless cost is at most this many times their AC bits are coded
    /// losslessly instead (docs/LOSSY.md "Exact blocks"). `None` (default):
    /// the decode-speed tier decides (`Compact`: 1.5, gated to quality scale
    /// `>= 60` at effort `>= 9.0` to avoid 4-channel DC overhead at low/mid
    /// quality; other tiers and no tier: off). `Some(k > 0)`: on with ratio k
    /// on any tier and quality; `Some(0.0)`: off. Images without a qualifying
    /// block encode as without the feature.
    pub exact_blocks: Option<f32>,
    /// Allow the rate-priced selector to code color planes as four 4x4 DCTs
    /// (feature `small_transforms`, Compact 0.3). Off by default. Initial
    /// production support requires an explicitly requested Compact tier,
    /// U8 intra images (including intra animation frames and pages), default
    /// channel layouts, effort below 9 and no AC trellis (at effort `>= 9.0`,
    /// `DecodeSpeed::Compact` and [`Self::adaptive_partitions`] enable 4x4
    /// splits automatically without setting this flag). Reference-predicted
    /// inputs and other configurations return [`crate::Error::Unsupported`]. Selection may
    /// change decoded pixels at fixed quality and emits the feature only
    /// when a plane is selected. Exact blocks and alpha remain lossless.
    pub small_transforms: bool,
    /// Use frequency-dependent zero-bias dead-zone quantization (`0.46` at low
    /// AC frequencies tapering to `0.33` at high AC frequencies, and `0.48` on
    /// the 3 Haar `2x2` sub-DC detail slots of `4x4` small transforms).
    pub freq_zero_bias: bool,
    /// Enable joint rate-distortion selection of `16x16`/`16x8`/`8x16`
    /// superblock merges (`SPEC §2.9`) and `4x4` small transforms (`SPEC §2.11`)
    /// across all efforts (`0..=10`) and entropy modes (`RunLevel` and `SplitHuf`).
    pub adaptive_partitions: bool,
    /// Optional Lossy 0.4 linear color transform (`SPEC §2.8` Record 17,
    /// `Feature::ColorTransform`). Setting `Some(ColorTransform::Identity)` with
    /// `LossyColor::YCoCg` codes the raw channels directly without XYB or YCoCg
    /// cross-channel mixing.
    pub color_transform: Option<ColorTransform>,
    /// LAB: low-frequency AC prediction from neighbouring DCs (bit 0:
    /// AC01/AC10, bit 1: also AC02/AC20/AC11, bit 2: neighbours limited to
    /// the DC stripe, bit 3: only where the two one-sided DC slopes agree
    /// within 50% (smooth ramps, not edges)). Encoder-only experiment: the
    /// output is not decodable.
    #[cfg(feature = "lab")]
    #[doc(hidden)]
    pub lf_pred: u8,
    /// LAB: AC-only run-level zero/tail search, separate from effort. Zero
    /// disables it byte-exactly; finite 0..=4 prices bits in quantizer units.
    /// Initial experiment supports U8 still images without AQ or tiers only.
    #[cfg(feature = "lab")]
    #[doc(hidden)]
    pub rdoq_lambda: f32,
    /// LAB: 0 preserves classic zero/tail search. 1/2 use a bounded zero-only
    /// queue without/with spatial weights; 3/4 add magnitude neighbours.
    /// Positive lambda in modes 1..=4 requires sRGB RGB8 XYB full-8 stills;
    /// quality above 80 bypasses the search after validating these restrictions.
    #[cfg(feature = "lab")]
    #[doc(hidden)]
    pub rdoq_policy: u8,
    /// LAB: overrides the trellis settings effort selects (`trellis.rs`);
    /// `passes: 0` turns the trellis off. `None` (default) follows effort.
    #[cfg(feature = "lab")]
    #[doc(hidden)]
    pub trellis: Option<LossyTrellis>,
    /// LAB: caller-supplied adaptive quantization: one code `a` (1..=255,
    /// AC step multiplier a/64) per 8x8 block, row-major over the
    /// ceil(width/8) x ceil(height/8) block grid, written to the DC image's
    /// AQ channel. Excludes `aq_gamma`. `None` (default) leaves the AQ
    /// channel off.
    #[cfg(feature = "lab")]
    #[doc(hidden)]
    pub aq_map: Option<alloc::sync::Arc<[u8]>>,
    /// LAB: AQ step codes from jpegli's adaptive quantization field
    /// (`aq_field_lab.rs`; U8 input), written to the AQ channel like
    /// `aq_map`. Excludes `aq_gamma` and `aq_map`. `None` (default): off.
    #[cfg(feature = "lab")]
    #[doc(hidden)]
    pub aq_field: Option<LossyAqField>,
    /// LAB: full-width seed bank ID (default 1; 0 disables seed tables).
    /// `seed_exact` controls explicit candidate comparison independently.
    #[cfg(feature = "lab")]
    #[doc(hidden)]
    pub seed_bank: u64,
    #[cfg(feature = "lab")]
    #[doc(hidden)]
    pub seed_exact: bool,
    /// LAB: encoder switches carried by this encode ([`LossyLabSwitches`];
    /// default: every switch from its process-global value).
    #[cfg(feature = "lab")]
    #[doc(hidden)]
    pub lab: LossyLabSwitches,
}

#[cfg(feature = "encode")]
impl Default for LossyConfig {
    fn default() -> Self {
        Self {
            quality: 90.0,
            quality_target: None,
            color: LossyColor::Xyb,
            channel_layouts: [LossyChannelLayout::default(); 3],
            compact_streams: true,
            local_ans_contexts: false,
            coding_defaults: false,
            model_references: false,
            dc_coverage: None,
            catalog_lab: None,
            stripe_block_rows: 0,
            threads: 1,
            effort: 7.5,
            round: 0.4,
            max_ac_tables: 16,
            channel_mul: None,
            ac_scale: 1.0,
            matrix: 255,
            entropy: LossyEntropy::RunLevel,
            decode_speed: None,
            aq_gamma: 0.0,
            aq_r0: 1.0,
            column_groups: None,
            decoder_memory_limit: None,
            checksum: false,
            hash: crate::HashAlgorithm::DEFAULT,
            unit_ordinals: false,
            exact_blocks: None,
            small_transforms: false,
            freq_zero_bias: false,
            adaptive_partitions: false,
            color_transform: None,
            #[cfg(feature = "lab")]
            lf_pred: 0,
            #[cfg(feature = "lab")]
            rdoq_lambda: 0.0,
            #[cfg(feature = "lab")]
            rdoq_policy: 0,
            #[cfg(feature = "lab")]
            trellis: None,
            #[cfg(feature = "lab")]
            aq_map: None,
            #[cfg(feature = "lab")]
            aq_field: None,
            #[cfg(feature = "lab")]
            seed_bank: crate::seeds::DEFAULT_BANK,
            #[cfg(feature = "lab")]
            seed_exact: true,
            #[cfg(feature = "lab")]
            lab: LossyLabSwitches::default(),
        }
    }
}

#[cfg(feature = "encode")]
impl LossyConfig {
    /// Select transforms and sampling independently for each coded color plane.
    /// Alpha retains its original samples. Written as lossy head record 27,
    /// which needs the feature `channel_layouts` (the file declares it).
    #[cfg(all(test, feature = "encode"))]
    pub fn with_channel_layouts(mut self, layouts: [LossyChannelLayout; 3]) -> Self {
        self.channel_layouts = layouts;
        self
    }

    /// Compact repeated rANS restart states without changing pixels, when
    /// that is smaller. Run-level rANS supports state compaction.
    #[cfg(all(test, feature = "encode"))]
    pub fn with_compact_streams(mut self, enabled: bool) -> Self {
        self.compact_streams = enabled;
        self
    }

    /// Constrains estimated decoder buffers by reducing the requested stripe
    /// height and, if necessary, column-group width. Set a taller stripe height
    /// first to trade memory for fewer stripe records. Quantization is unchanged.
    /// An impossible budget fails before transforms or entropy search.
    #[cfg(all(test, feature = "encode"))]
    pub fn with_decoder_memory_limit(mut self, limit: crate::DecoderMemoryLimit) -> Self {
        self.decoder_memory_limit = Some(limit);
        self
    }

    /// Sets [`Self::checksum`].
    #[cfg(all(test, feature = "encode"))]
    pub fn with_checksum(mut self, enabled: bool) -> Self {
        self.checksum = enabled;
        self
    }

    /// Estimated bytes after applying the memory policy for this image. Without
    /// a budget, estimates the current layout for a single decoder worker.
    /// Reserves worst-case nested DC/alpha maps, paired stripes and table arrays.
    pub fn estimated_decoder_memory(&self, info: &ImageInfo) -> Result<u64, Error> {
        crate::decoder_memory::plan_lossy(info, self).map(|(_, bytes)| bytes)
    }

    #[cfg(all(test, feature = "encode"))]
    pub fn new(quality: f32) -> Self {
        Self {
            quality,
            ..Self::default()
        }
    }
    #[cfg(all(test, feature = "encode"))]
    pub fn with_color(mut self, color: LossyColor) -> Self {
        self.color = color;
        self
    }
    /// Sets the quality: a bare number or [`LossyQuality::Scale`] is the
    /// quantizer scale, exactly as [`Self::new`] takes it; the calibrated
    /// variants resolve when encoding, against the image size and the color
    /// mode actually used (so the order of `with_quality` and `with_color`
    /// does not matter).
    pub fn with_quality(mut self, quality: impl Into<LossyQuality>) -> Self {
        match quality.into() {
            LossyQuality::Scale(scale) => {
                self.quality = scale;
                self.quality_target = None;
            }
            target => self.quality_target = Some(target),
        }
        self
    }
    /// The quantizer scale this configuration encodes a `width` x `height`
    /// image with in `color`.
    pub fn scale_for(&self, color: LossyColor, width: u32, height: u32) -> f32 {
        match self.quality_target {
            Some(target) => target.to_scale(color, width, height),
            None => self.quality,
        }
    }
    #[cfg(all(test, feature = "encode"))]
    pub fn with_threads(mut self, threads: usize) -> Self {
        self.threads = threads;
        self
    }
    /// Sets search effort (validated when encoding). See [`Self::effort`].
    #[cfg(all(test, feature = "encode"))]
    pub fn with_effort(mut self, effort: f32) -> Self {
        self.effort = effort;
        self
    }
    #[cfg(all(test, feature = "encode"))]
    pub fn with_decode_speed(mut self, speed: crate::DecodeSpeed) -> Self {
        self.decode_speed = Some(speed);
        self.color = if speed == crate::DecodeSpeed::Fastest {
            LossyColor::YCoCg
        } else {
            LossyColor::Xyb
        };
        self
    }
    #[cfg(all(test, feature = "encode"))]
    pub fn with_freq_zero_bias(mut self, enabled: bool) -> Self {
        self.freq_zero_bias = enabled;
        self
    }
    #[cfg(all(test, feature = "encode"))]
    pub fn with_color_transform(mut self, transform: Option<ColorTransform>) -> Self {
        self.color_transform = transform;
        if transform.is_some() {
            self.color = LossyColor::YCoCg;
        }
        if transform == Some(ColorTransform::Identity) && self.channel_mul.is_none() {
            self.channel_mul = Some([1.0, 1.0, 1.0]);
        }
        self
    }
    #[cfg(all(test, feature = "encode"))]
    pub fn with_adaptive_partitions(mut self, enabled: bool) -> Self {
        self.adaptive_partitions = enabled;
        self
    }
    #[cfg(all(test, feature = "encode"))]
    pub fn with_stripe_block_rows(mut self, rows: u32) -> Self {
        self.stripe_block_rows = rows;
        self
    }

    /// Stripe height in block rows this configuration encodes an image
    /// `width` pixels wide with: the explicit [`Self::stripe_block_rows`],
    /// else the automatic choice. Automatic stripes cap the stripe area
    /// (column-group width x pixel rows) per decode-speed tier: 128K pixels
    /// for `Compact`, 64K for `Balanced` (also used when no tier is set),
    /// 32K for `Fast` and `Fastest`; heights run from 8 to 64 pixel rows
    /// (32 for `Fast`/`Fastest`). Balanced gives 64-row stripes up to 1024
    /// wide, 32 at 2048, 16 at 4096 and 8 at 8192.
    pub fn stripe_rows_for(&self, width: u32) -> u32 {
        if self.stripe_block_rows != 0 {
            return self.stripe_block_rows.min(255);
        }
        use crate::DecodeSpeed as D;
        let (area, max_rows) = match self.decode_speed.unwrap_or(D::Balanced) {
            D::Compact => (131_072, 64),
            D::Balanced => (65_536, 64),
            D::Fast | D::Fastest => (32_768, 32),
        };
        let g = lossy_group_cols(self, width as usize);
        let span = if g == 0 { width as usize } else { g }.max(1);
        ((area / span).min(max_rows) / 8).max(1) as u32
    }
    #[cfg(all(test, feature = "encode"))]
    pub fn with_entropy(mut self, entropy: LossyEntropy) -> Self {
        self.entropy = entropy;
        self
    }

    /// The tier recorded in the header: the requested one, else the tier
    /// the entropy mode and color belong to.
    pub fn decode_speed(&self) -> crate::DecodeSpeed {
        use crate::DecodeSpeed as D;
        if let Some(d) = self.decode_speed {
            return d;
        }
        match (self.entropy, self.color) {
            (LossyEntropy::RunLevel, _) => D::Compact,
            (LossyEntropy::SplitHuf, LossyColor::Xyb) => D::Fast,
            (LossyEntropy::SplitHuf, LossyColor::YCoCg) => D::Fastest,
        }
    }
}
