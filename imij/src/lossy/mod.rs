//! Lossy mode: 8x8 DCT blocks in independent stripe/column-group units.
//! Design and measurements: `docs/LOSSY.md`.
//!
//! The `DATA` payload (SPEC 2.8) starts with the stripe/group geometry, the
//! parameter presence mask and quantizer scale with the parameters that
//! differ from their defaults (seed byte, entropy mode, matrix, DC steps,
//! color scales, floor, multipliers), the AC entropy tables, the head record
//! list (unit table hint, exact blocks, AQ, keep blocks, 32-bit DC, Lossy
//! 0.4 records, channel layouts) and the nested lossless DC, alpha and
//! exact heads. Unit records (SPEC 2.7: `id, len, payload`) follow in
//! stripe-major, column-group order: a DC record when a new nested DC stripe
//! begins, alpha, AC, then exact. The internal color is the header's
//! `coding`. See `parse_lossy_head_checked`, `parse_lossy` and docs/LOSSY.md.
//!
//! DC uses U16 or I32 as required; alpha preserves the original samples.
//! AC supports run-level rANS and split-Huffman; packed rANS and split rANS
//! are retired (SPEC 2.8 reserves wire values 1 and 2; see
//! [`coef::LossyEntropy`]).
//! Units are independent given their shared models
//! and corresponding DC/alpha data. Coded 8x8 blocks extend to the
//! right/bottom edge; output is clipped to the header's exact dimensions.
//!
//! U32 color is full-range RGB code data, with independently declared 1..32
//! significant bits. YCoCg uses normalized 0..255 units; XYB uses the declared
//! sRGB/PQ/HLG transfer to linear light. Conversion never allocates a 2^32
//! table or truncates through U16. The existing f32 transforms/DCT and bounded
//! inverse-transfer interpolation are lossy even at maximum quality: U32
//! storage is not a promise of 32-bit color precision. Alpha remains lossless.

#[cfg(feature = "encode")]
pub(crate) mod adaptive_rdo;
#[cfg(all(feature = "lab", feature = "std"))]
pub(crate) mod block_lab;
#[cfg(feature = "encode")]
pub(crate) mod bounded;
#[cfg(feature = "lab")]
pub mod chroma_lab;
mod coding_defaults;
pub(crate) mod coef;
pub(crate) mod color;
#[cfg(target_arch = "x86_64")]
mod color_v3;
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
mod color_v4x;
mod compact;
pub(crate) mod dc_coverage;
pub(crate) mod dct;
pub(crate) mod dering;
#[cfg(feature = "encode")]
pub(crate) mod enc_simd;
#[cfg(target_arch = "x86_64")]
mod entropy_avx2;
#[cfg(target_arch = "x86_64")]
mod entropy_avx512;
#[cfg(target_arch = "aarch64")]
mod entropy_neon;
pub(crate) mod exact;
mod local_ans;
#[cfg(feature = "encode")]
mod model_candidates;
mod model_refs;
mod reconstruction;
pub(crate) mod rows;
mod xyb_power;
use reconstruction::Reconstruction;
pub(crate) mod v4;
#[cfg(feature = "std")]
#[cfg(feature = "encode")]
pub(crate) use bounded::bounded_working_bytes;
pub(crate) use rows::{LossyStream, OwnedRecords};
mod superblock;
#[cfg(feature = "std")]
pub(crate) use rows::read_lossy;
pub(crate) use superblock::map::{decode_v4_partitions, joint as joint_settings};
// Callers are gated on wasm32 (the portable kernels' target); `test` also
// compiles it on other arches so its hostile-input sweeps exercise the
// generic scalar tier, and the items are dead there outside tests.
#[cfg(any(target_arch = "wasm32", all(test, feature = "encode")))]
#[allow(dead_code)]
mod entropy_portable;
mod envelope;
pub(crate) mod huf;
#[cfg(feature = "native-jpeg")]
pub(crate) mod jpeg;
mod layout;
mod small;
#[cfg(feature = "lab")]
pub(crate) mod small_lab;
mod subblock;
pub use layout::{ChannelSampling, LossyChannelLayout, LossyTransform};
#[cfg(feature = "lab")]
mod aq_field_lab;
#[cfg(target_arch = "x86_64")]
mod kernel_choice;
#[cfg(all(feature = "lab", feature = "std"))]
pub(crate) mod lanes_lab;
mod quality_scale;
mod quality_scale_tables;
mod rawbits;
#[cfg(feature = "lab")]
mod rdoq_lab;
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
mod recon_v4;
#[cfg(feature = "encode")]
pub(crate) mod trellis;
#[cfg(all(test, feature = "encode"))]
#[cfg(feature = "encode")]
mod trellis_base;
#[cfg(feature = "lab")]
pub use trellis::LossyTrellis;
mod simd;
#[cfg(target_arch = "x86_64")]
mod split_avx2;
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
mod split_avx512;
mod srgb_table;
mod transfer;
pub use quality_scale::{ButteraugliNorm, LossyQuality};

#[cfg(all(not(feature = "std"), feature = "encode"))]
use num_traits::Float as _;

#[cfg(feature = "encode")]
use alloc::vec;
use alloc::vec::Vec;
#[cfg(feature = "encode")]
use how_far::{Complete, PhaseSpec, ProgressExt, Pulse, Report, Stages, Total};

#[cfg(feature = "encode")]
use crate::Metadata;
use crate::bits::BitReader;
#[cfg(feature = "encode")]
use crate::bits::BitWriter;
#[cfg(feature = "encode")]
use crate::put_varint;
#[cfg(feature = "encode")]
use crate::rans::{EncodeTables, Sym, encode_lanes_tables};
use crate::rans::{LOG_M, M, NL, RANS_L, Words};
use crate::{ByteReader, Channels, Error, ImageInfo, SampleType};
pub use coef::LossyEntropy;
#[cfg(feature = "encode")]
use coef::rl_token;
use coef::{NCH, Tables, ZIGZAG, ac_ctx, eob_ctx, rl_value};
pub use color::LossyColor;

/// Lossy head record ids (SPEC 2.8); id 10 is the unit table hint.
/// Exact blocks (feature `exact_blocks`): block code 1 = exact block.
const H_EXACT: u64 = crate::registry::HEAD_EXACT;
/// AQ: the DC image's fourth channel holds AC step multipliers a/64.
const H_AQ: u64 = crate::registry::HEAD_AQ;
/// Keep blocks (inter frames): block code 0 keeps the reference block.
const H_KEEP: u64 = crate::registry::HEAD_KEEP;
/// The DC image's samples are i32 instead of u16.
const H_DC32: u64 = crate::registry::HEAD_DC32;
/// Lossy 0.4 explicit quantization tables (feature `explicit_tables`).
const H_EXPLICIT: u64 = crate::registry::HEAD_EXPLICIT;
/// Lossy 0.4 reconstruction bias (feature `recon_bias`).
const H_BIAS: u64 = crate::registry::HEAD_BIAS;
/// Lossy 0.4 color transform (feature `color_transform`).
const H_TRANSFORM: u64 = crate::registry::HEAD_TRANSFORM;
/// Per-plane channel layouts (transform size, sampling).
const H_LAYOUTS: u64 = crate::registry::HEAD_LAYOUTS;

pub(crate) use envelope::MATRIX_EXPLICIT;
#[cfg(feature = "encode")]
pub(crate) use envelope::MATRIX_LAST_PRESET;

const JPEG_LUMA: [u16; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56,
    14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113,
    92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
];
const JPEG_CHROMA: [u16; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99,
    47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
];

/// jpegli's XYB base quantization matrices (libjxl jpegli, via zenjpeg's
/// `BASE_QUANT_MATRIX_XYB`), raster order, in imij channel order: Y, X, B - Y.
#[rustfmt::skip]
#[allow(clippy::excessive_precision)]
const JPEGLI_XYB: [[f32; 64]; 3] = [
    [
        1.626_200_1, 3.219_924_2, 3.490_378, 3.914_835_9, 4.833_721_2, 4.910_884_4, 5.313_712, 6.167_679_3,
        3.219_924_2, 3.454_789_9, 3.603_683, 4.265_283_6, 4.836_838_7, 4.822_622_3, 5.612_051_5, 6.343_147_3,
        3.490_378, 3.603_683, 3.904_456, 4.337_439_5, 4.843_509_7, 5.405_798, 5.606_636, 6.107_513_4,
        3.914_835_9, 4.265_283_6, 4.337_439_5, 4.606_483_5, 5.175_147_5, 5.401_392_5, 6.039_981, 6.782_523,
        4.833_721_2, 4.836_838_7, 4.843_509_7, 5.175_147_5, 5.374_805, 6.141_083_7, 7.652_930_7, 7.523_521_4,
        4.910_884_4, 4.822_622_3, 5.405_798, 5.401_392_5, 6.141_083_7, 6.343_147_3, 7.108_305, 7.600_83,
        5.313_712, 5.612_051_5, 5.606_636, 6.039_981, 7.652_930_7, 7.108_305, 7.094_315_5, 7.047_836_3,
        6.167_679_3, 6.343_147_3, 6.107_513_4, 6.782_523, 7.523_521_4, 7.600_83, 7.047_836_3, 6.918_614_4,
    ],
    [
        7.562_993_5, 19.824_781, 22.572_495, 20.670_67, 22.686_459, 23.569_628, 25.812_908, 36.330_757,
        19.824_781, 21.550_318, 19.937_223, 20.542_421, 21.864_55, 23.904_139, 28.284_407, 32.660_976,
        22.572_495, 19.937_223, 21.901_726, 19.122_345, 21.751_581, 24.672_47, 25.424_965, 32.665_382,
        20.670_67, 20.542_421, 19.122_345, 20.161_022, 25.371_969, 25.966_89, 30.980_495, 31.340_601,
        22.686_459, 21.864_55, 21.751_581, 25.371_969, 26.243_185, 40.599_22, 43.262_463, 63.301_094,
        23.569_628, 23.904_139, 24.672_47, 25.966_89, 40.599_22, 48.302_677, 34.096_436, 61.985_214,
        25.812_908, 28.284_407, 25.424_965, 30.980_495, 43.262_463, 34.096_436, 34.493_744, 66.970_276,
        36.330_757, 32.660_976, 32.665_382, 31.340_601, 63.301_094, 61.985_214, 66.970_276, 39.965_27,
    ],
    [
        3.303_847_3, 10.068_926, 12.278_522, 14.604_117, 16.210_732, 19.231_453, 28.012_955, 55.668_29,
        10.068_926, 11.408_502, 11.387_135, 15.493_417, 16.536_493, 14.915_342, 26.374_872, 40.861_443,
        12.278_522, 11.387_135, 17.088_688, 13.950_035, 16.000_322, 28.566_063, 26.212_42, 30.126_013,
        14.604_117, 15.493_417, 13.950_035, 21.123_503, 26.157_978, 25.557_922, 40.685_936, 33.805_634,
        16.210_732, 16.536_493, 16.000_322, 26.157_978, 26.804_283, 26.158_772, 35.734_398, 43.685_703,
        19.231_453, 14.915_342, 28.566_063, 25.557_922, 26.158_772, 34.541_813, 41.319_794, 48.786_766,
        28.012_955, 26.374_872, 26.212_42, 40.685_936, 35.734_398, 41.319_794, 47.632_946, 55.349_846,
        55.668_29, 40.861_443, 30.126_013, 33.805_634, 43.685_703, 48.786_766, 55.349_846, 63.606_56,
    ],
];

/// jpegli scaled-XYB plane scales (Y, X, B - Y), for converting
/// [`JPEGLI_XYB`] steps into imij plane units.
const JPEGLI_XYB_SCALE: [f32; 3] = [1.183_000_1, 22.995_789, 1.502_141_3];

/// N. Robidoux's quantization table, ImageMagick's and mozjpeg's default
/// (mozjpeg `quant_table` 3; `mozjpeg-rs` `src/consts.rs`, its chroma table is
/// the same), raster order.
#[rustfmt::skip]
const ROBIDOUX: [u16; 64] = [
    16, 16, 16, 18, 25, 37, 56, 85,
    16, 17, 20, 27, 34, 40, 53, 75,
    16, 20, 24, 31, 43, 62, 91, 135,
    18, 27, 31, 40, 53, 74, 106, 156,
    25, 34, 43, 53, 69, 94, 131, 189,
    37, 40, 62, 74, 94, 124, 169, 238,
    56, 53, 91, 106, 131, 169, 226, 311,
    85, 75, 135, 156, 189, 238, 311, 418,
];

/// jpegli's YCbCr base quantization matrices (libjxl jpegli, via zenjpeg's
/// `BASE_QUANT_MATRIX_YCBCR`), raster order: Y, Cb, Cr.
#[rustfmt::skip]
#[allow(clippy::excessive_precision)]
const JPEGLI_YCBCR: [[f32; 64]; 3] = [
    [
        1.239_740_9, 1.722_711_5, 2.921_216_7, 2.812_737_4, 3.339_819_7, 3.463_603_8, 3.840_915_2, 3.869_56,
        1.722_711_5, 2.092_889_4, 2.845_676, 2.704_506_8, 3.440_767_4, 3.166_232_4, 4.025_208_7, 4.035_324_5,
        2.921_216_7, 2.845_676, 2.958_740_4, 3.386_295, 3.619_523_8, 3.904_628, 3.757_835_8, 4.237_447_5,
        2.812_737_4, 2.704_506_8, 3.386_295, 3.380_058_8, 4.167_986_7, 4.805_510_6, 4.784_259, 4.605_934,
        3.339_819_7, 3.440_767_4, 3.619_523_8, 4.167_986_7, 4.579_851_3, 4.923_237, 5.574_107, 5.485_333_4,
        3.463_603_8, 3.166_232_4, 3.904_628, 4.805_510_6, 4.923_237, 5.439_36, 5.093_895_7, 6.087_225_4,
        3.840_915_2, 4.025_208_7, 3.757_835_8, 4.784_259, 5.574_107, 5.093_895_7, 5.438_461, 5.403_736,
        3.869_56, 4.035_324_5, 4.237_447_5, 4.605_934, 5.485_333_4, 6.087_225_4, 5.403_736, 4.377_871,
    ],
    [
        2.823_619_8, 6.495_639_4, 9.310_489, 10.647_479, 11.074_191, 17.146_39, 18.463_982, 29.087_002,
        6.495_639_4, 8.890_104, 8.976_895_8, 13.666_27, 16.547_072, 16.638_714, 26.778_397, 21.330_343,
        9.310_489, 8.976_895_8, 11.087_377, 18.205_482, 19.752_482, 23.985_66, 102.645_74, 24.450_989,
        10.647_479, 13.666_27, 18.205_482, 18.628_012, 16.042_51, 25.049_183, 25.017_14, 35.797_89,
        11.074_191, 16.547_072, 19.752_482, 16.042_51, 19.373_483, 14.677_53, 19.946_96, 51.094_112,
        17.146_39, 16.638_714, 23.985_66, 25.049_183, 14.677_53, 31.320_412, 46.357_234, 67.481_11,
        18.463_982, 26.778_397, 102.645_74, 25.017_14, 19.946_96, 46.357_234, 61.315_765, 88.346_65,
        29.087_002, 21.330_343, 24.450_989, 35.797_89, 51.094_112, 67.481_11, 88.346_65, 112.160_99,
    ],
    [
        2.921_725_5, 4.497_681, 7.356_344_5, 6.583_891_5, 8.535_608_7, 8.799_434_4, 9.188_341_5, 9.482_7,
        4.497_681, 6.309_548_9, 7.024_609, 7.156_445_3, 8.049_059_2, 7.012_429, 6.711_923_2, 8.380_308,
        7.356_344_5, 7.024_609, 6.892_101_2, 6.882_82, 8.782_226, 6.877_475, 7.885_817_6, 8.679_09,
        6.583_891_5, 7.156_445_3, 6.882_82, 7.003_073, 7.722_346_5, 7.955_425_7, 7.473_411, 8.362_933,
        8.535_608_7, 8.049_059_2, 8.782_226, 7.722_346_5, 6.778_005_9, 9.484_922_7, 9.043_702_7, 8.053_178_2,
        8.799_434_4, 7.012_429, 6.877_475, 7.955_425_7, 9.484_922_7, 8.607_606_5, 9.922_697_4, 64.251_35,
        9.188_341_5, 6.711_923_2, 7.885_817_6, 7.473_411, 9.043_702_7, 9.922_697_4, 63.184_937, 83.352_94,
        9.482_7, 8.380_308, 8.679_09, 8.362_933, 8.053_178_2, 64.251_35, 83.352_94, 114.892_02,
    ],
];

/// jpegli's YCbCr global scale (1.7396601, zenjpeg `GLOBAL_SCALE_YCBCR`)
/// divided by imij's scale at quality 90 (0.2), rounded to f32: matrix 4 at
/// scale s is jpegli's table at distance 5 s below distance 1.5.
const JPEGLI_YCBCR_FACTOR: f32 = 8.698_3;

/// jpegli's per-frequency exponents (zenjpeg `src/quant/mod.rs`, raster
/// order) and its distance threshold: the nonlinear distance law of matrix 5.
const JPEGLI_FREQUENCY_EXPONENT: [f32; 64] = [
    1.00, 0.51, 0.67, 0.74, 1.00, 1.00, 1.00, 1.00, 0.51, 0.66, 0.69, 0.87, 1.00, 1.00, 1.00, 1.00,
    0.67, 0.69, 0.84, 0.83, 0.96, 1.00, 1.00, 1.00, 0.74, 0.87, 0.83, 1.00, 1.00, 0.91, 0.91, 1.00,
    1.00, 1.00, 0.96, 1.00, 1.00, 1.00, 1.00, 1.00, 1.00, 1.00, 1.00, 0.91, 1.00, 1.00, 1.00, 1.00,
    1.00, 1.00, 1.00, 0.91, 1.00, 1.00, 1.00, 1.00, 1.00, 1.00, 1.00, 1.00, 1.00, 1.00, 1.00, 1.00,
];

/// jpegli's nonlinear distance-to-scale at raster index `i` (zenjpeg
/// `distance_to_scale`), computed with the deterministic `transfer` ln/exp
/// so every decoder tier and platform agrees: linear below 1.5, then the
/// per-frequency exponents.
pub(crate) fn jpegli_distance_to_scale(distance: f32, i: usize) -> f32 {
    const DIST_THRESHOLD: f64 = 1.5;
    let d = distance as f64;
    if d < DIST_THRESHOLD {
        return distance;
    }
    let e = JPEGLI_FREQUENCY_EXPONENT[i] as f64;
    (0.5 * d).max(transfer::pow(DIST_THRESHOLD, 1.0 - e) * transfer::pow(d, e)) as f32
}

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
struct BiasSums {
    sum: [[[f64; 2]; 2]; 3],
    count: [[[u64; 2]; 2]; 3],
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
    fn add_row(
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
    fn bias(&self, k_split: u8) -> ReconBias {
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
    /// the decode-speed tier decides (`Compact`: 1.5; other tiers and no
    /// tier: off). `Some(k > 0)`: on with ratio k on any tier; `Some(0.0)`:
    /// off. Images without a qualifying block encode as without the feature.
    pub exact_blocks: Option<f32>,
    /// Allow the rate-priced selector to code color planes as four 4x4 DCTs
    /// (feature `small_transforms`, Compact 0.3). Off by default. Initial
    /// production support requires an explicitly requested Compact tier,
    /// U8 intra images (including intra animation frames and pages), default
    /// channel layouts, effort below 9 and no AC trellis. Reference-predicted
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

/// Entropy-work counts of the quantized image: [run-level tokens, coded
/// groups (split-Huffman), block-channels with any coded group].
#[cfg(feature = "encode")]
fn side_stats(stripes: &[StripeCoefs], bw: usize, threads: usize) -> [u64; 3] {
    let per: Vec<[u64; 3]> =
        crate::par::par_map(stripes.iter().collect(), threads, |_, sc: &StripeCoefs| {
            let mut n = 0u64;
            stripe_tokens(sc, bw, Group::ALL, |_, _, _, _, _| n += 1);
            let (mut groups, mut coded) = (0u64, 0u64);
            for &m in &sc.nz {
                let ac = m & !1;
                if ac != 0 {
                    groups += (63 - ac.leading_zeros() as u64).div_ceil(4);
                    coded += 1;
                }
            }
            [n, groups, coded]
        });
    per.iter()
        .fold([0; 3], |a, b| [a[0] + b[0], a[1] + b[1], a[2] + b[2]])
}

/// [`side_stats`] totals when the token count is already known: column-
/// group splitting preserves the per-block `nz` multiset, so groups and
/// coded totals are a linear `nz` scan with no coefficient walk.
#[cfg(feature = "encode")]
fn side_stats_lanes(stripes: &[StripeCoefs], tokens: u64) -> [u64; 3] {
    let (mut groups, mut coded) = (0u64, 0u64);
    for sc in stripes {
        for &m in &sc.nz {
            let ac = m & !1;
            if ac != 0 {
                groups += (63 - ac.leading_zeros() as u64).div_ceil(4);
                coded += 1;
            }
        }
    }
    [tokens, groups, coded]
}

/// Images up to this many pixels code both ways under a tier and keep
/// split-Huffman when it is smaller.
#[cfg(feature = "encode")]
const SMALL_IMAGE: usize = 1 << 16;

/// Predicted decode time of split-Huffman over run-level from the
/// encoder's counts `[tokens, groups, coded block-channels]`: per-mode
/// linear models fitted to 1-thread decode times on Zen 4 (7900X), 344
/// image / quality pairs from 64x64 crops to 35 MP, photos and graphics,
/// q10-q95; held-out median ratio error 0.035-0.10 (docs/LOSSY.md E17,
/// imij-cmp/fit_cost.py).
#[cfg(feature = "encode")]
fn predicted_huf_ratio(px: f64, st: &[u64; 3]) -> f64 {
    let [tok, grp, bc] = st.map(|v| v as f64);
    let rl = 36_200.0 + 1.624 * px + 4.183 * tok;
    let sh = 26_200.0 + 1.749 * px + 0.842 * grp + 41.416 * bc;
    sh / rl
}

/// Largest predicted time ratio at which a tier takes split-Huffman.
#[cfg(feature = "encode")]
fn huf_max_ratio(speed: crate::DecodeSpeed) -> Option<f64> {
    use crate::DecodeSpeed as D;
    match speed {
        D::Compact => None,
        D::Balanced => Some(0.85),
        D::Fast | D::Fastest => Some(0.93),
    }
}

/// Pixel formats the lossy mode accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputKind {
    U8,
    U16,
    U32,
    F16,
    F32,
}

impl InputKind {
    /// Sample kind of a supported layout (gray, gray+alpha, RGB, RGBA).
    fn of(info: &ImageInfo) -> Option<InputKind> {
        if info.channels.is_native_model() {
            return None;
        }
        match info.sample {
            SampleType::U8 => Some(InputKind::U8),
            SampleType::U16 => Some(InputKind::U16),
            SampleType::U32 => Some(InputKind::U32),
            SampleType::F16 => Some(InputKind::F16),
            SampleType::F32 => Some(InputKind::F32),
            _ => None,
        }
    }
}

/// Color samples of a pixel: gray replicates its one channel.
fn color_index(ch: Channels) -> [usize; 3] {
    if matches!(ch, Channels::Gray | Channels::GrayAlpha) {
        [0, 0, 0]
    } else {
        [0, 1, 2]
    }
}

/// Non-finite input samples are coded as 0.
#[inline]
#[cfg(feature = "encode")]
fn finite(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

/// Deinterleave one source row and extend its last pixel to the coded width.
/// Typed sample iteration avoids repeated byte-offset arithmetic and slice
/// bounds checks in the per-component loop.
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "encode")]
fn fill_color_row<const BPS: usize>(
    row: &[u8],
    planes: &mut [f32],
    pw: usize,
    y: usize,
    w: usize,
    channels: usize,
    indices: [usize; 3],
    read: impl Fn(usize, &[u8; BPS]) -> f32,
) {
    let samples = row.as_chunks::<BPS>().0;
    for (c, channel) in indices.into_iter().enumerate() {
        let dst = &mut planes[(c * 8 + y) * pw..][..pw];
        for (sample, value) in samples
            .iter()
            .skip(channel)
            .step_by(channels)
            .zip(&mut dst[..w])
        {
            *value = read(c, sample);
        }
        let last = dst[w - 1];
        dst[w..].fill(last);
    }
}

/// The encoder's row pipeline (`fill_block_row` in the encoder):
/// `rows(by, planes, extremes)` writes block row `by`'s color-converted (and
/// deringed) planes for every column of the image, `NCH * 8 * pw` samples,
/// plane-major, and the row's white/black blocks into `extremes` (integer
/// inputs when needed; otherwise left untouched). Shared with the passes that
/// recompute rows (the trellis, lab RDOQ).
#[cfg(feature = "encode")]
type RowPlanes<'a> = &'a (dyn Fn(usize, &mut [f32], &mut Vec<(u64, bool)>) + Sync);

/// Per block of one block row: the mask of white samples (every color
/// channel at its maximum code, raster order) and whether every sample has
/// every color channel at code 0. Columns and rows past the image edge
/// repeat the edge, as the planes do.
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "encode")]
fn block_extremes(
    g: &Geom,
    pixels: &[u8],
    stride: usize,
    y0: usize,
    nci: usize,
    ci: [usize; 3],
    kind: InputKind,
    white_code: &[u32; 3],
    out: &mut Vec<(u64, bool)>,
) {
    let bps = match kind {
        InputKind::U8 => 1,
        _ => 2,
    };
    let code = |row: &[u8], x: usize, c: usize| -> u32 {
        let at = (x * nci + ci[c]) * bps;
        if bps == 1 {
            row[at] as u32
        } else {
            u16::from_ne_bytes([row[at], row[at + 1]]) as u32
        }
    };
    out.clear();
    for bx in 0..g.bw {
        let mut white = 0u64;
        let mut black = true;
        for y in 0..8 {
            let row = &pixels[(y0 + y).min(g.h - 1) * stride..];
            for x in 0..8 {
                let sx = (bx * 8 + x).min(g.w - 1);
                let c = [code(row, sx, 0), code(row, sx, 1), code(row, sx, 2)];
                if c == *white_code {
                    white |= 1 << (y * 8 + x);
                }
                black &= c == [0, 0, 0];
            }
        }
        out.push((white, black));
    }
}

/// Deringing for one block row (dering.rs): reshapes each block's white
/// samples in plane 0.
#[cfg(feature = "encode")]
fn dering_row(plane0: &mut [f32], pw: usize, extremes: &[(u64, bool)], cap: f32) {
    for (bx, &(white, _)) in extremes.iter().enumerate() {
        if white == 0 {
            continue;
        }
        let i = white.trailing_zeros() as usize;
        let top = plane0[(i / 8) * pw + bx * 8 + i % 8];
        // 31/255 of the white level, as mozjpeg's 31 of 255 codes.
        let cap = cap.min(top.abs() * (31.0 / 255.0));
        dering::dering_block(plane0, pw, bx * 8, white, top, cap);
    }
}

/// DC quantization for one channel of a block row, replacing the shared
/// quantizer's dead-zone rounding (which pulls every block mean toward zero:
/// flat white came back as 253-254 in YCoCg) with rounding to nearest. With
/// `extremes` (luma, default layout), a block whose source is entirely at
/// the maximum code rounds its DC up and one entirely at code 0 rounds it
/// down, so the decoder's clamp returns exact white and black.
#[cfg(feature = "encode")]
fn dc_row(
    coefs: &[f32],
    bw: usize,
    ch: usize,
    inv_step: f32,
    extremes: Option<&[(u64, bool)]>,
    q: &mut [i32],
    nz: &mut [u64],
) {
    const CAP: f32 = ((1 << 22) - 1) as f32;
    for bx in 0..bw {
        let c = coefs[bx * 64] * inv_step;
        let v = match extremes.map(|e| e[bx]) {
            Some((u64::MAX, _)) => c.ceil(),
            Some((_, true)) => c.floor(),
            _ => c.round(),
        };
        let v = v.clamp(-CAP, CAP) as i32;
        q[(bx * NCH + ch) * 64] = v;
        let m = &mut nz[bx * NCH + ch];
        *m = (*m & !1) | u64::from(v != 0);
    }
}

/// Lab: encoder override of the DC step multipliers (1/64 units, luma then
/// chroma); 0 keeps the defaults. Written to the file, so it decodes anywhere.
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) static DC_STEP_OVERRIDE: [core::sync::atomic::AtomicU8; 2] = [
    core::sync::atomic::AtomicU8::new(0),
    core::sync::atomic::AtomicU8::new(0),
];

/// Lab A/B switch for nearest and clamp-aware DC rounding (on by default).
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) static DC_NEAREST: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(true);

#[cfg(feature = "encode")]
pub(crate) fn dc_nearest(cfg: &LossyConfig) -> bool {
    #[cfg(feature = "lab")]
    {
        cfg.lab
            .dc_nearest
            .unwrap_or_else(|| DC_NEAREST.load(core::sync::atomic::Ordering::Relaxed))
    }
    #[cfg(not(feature = "lab"))]
    {
        let _ = cfg;
        true
    }
}

/// Lab: the DC step override of an encode ([`LossyLabSwitches::dc_step`]).
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
fn lab_dc_step(cfg: &LossyConfig) -> [u8; 2] {
    cfg.lab.dc_step.unwrap_or_else(|| {
        DC_STEP_OVERRIDE
            .each_ref()
            .map(|s| s.load(core::sync::atomic::Ordering::Relaxed))
    })
}

/// Header quantization parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
struct QParams {
    scale: f32,
    floor: f32,
    mul: [f32; 3],
    /// Base matrix id (SPEC 2.8): 0..=2, the presets up to
    /// [`MATRIX_LAST_PRESET`] and [`MATRIX_EXPLICIT`] (Lossy 0.4).
    matrix: u8,
    /// Per-plane DC (position 0) step multiplier in 1/64 units, 1..=255.
    dc: [u8; 3],
    /// Lossy 0.4: the base tables of matrix [`MATRIX_EXPLICIT`], raster order.
    explicit: Option<[[f32; 64]; 3]>,
    /// Lossy 0.4: the reconstruction bias (`None` = reconstruct at q x step).
    bias: Option<ReconBias>,
    /// Lossy 0.4: a signalled linear color transform (`None` = the color's
    /// default transform: XYB or the YCoCg lifting).
    transform: Option<ColorTransform>,
}

/// Default DC step multipliers (1/64 units): XYB x0.75 on all planes;
/// YCoCg x0.25 luma, x0.5 chroma (2026-10-06: the base DC steps shifted
/// flat colours visibly at low quality and cost bytes at equal quality;
/// XYB chroma x0.5 then cost photos at CVVDP and Butteraugli max, and x0.75
/// still keeps the shifts away; benchmarks/dc_step_2026-10-06). Files may
/// override them.
pub(crate) const fn default_dc_step(color: LossyColor) -> [u8; 3] {
    match color {
        LossyColor::Xyb => [48, 48, 48],
        LossyColor::YCoCg => [16, 32, 32],
    }
}

#[cfg(feature = "encode")]
pub(crate) fn valid_ac_scale(scale: f32) -> bool {
    matches!(scale, 0.25 | 0.5 | 1.0 | 2.0 | 4.0)
}

impl QParams {
    fn from_quality(q: f32, color: LossyColor, matrix: u8) -> QParams {
        let q = q.clamp(1.0, 116.0);
        let (scale, floor) = if q < 50.0 {
            (50.0 / q, 1.0)
        } else if q <= 100.0 {
            ((200.0 - 2.0 * q) / 100.0, 1.0)
        } else {
            (0.0, exp2_neg_quarter(q - 100.0))
        };
        let mul = match (color, matrix) {
            // jpegli's own channel balance.
            (LossyColor::Xyb, 1) => [1.0, 1.0, 1.0],
            // B - Y slightly coarser: neutral on SSIMULACRA2, -1.5% at equal
            // zensim (benchmarks/lossy_qmul_2026-10-02.txt).
            (LossyColor::Xyb, _) => [1.0, 1.0, 1.4],
            (LossyColor::YCoCg, _) => [1.0, 2.0, 2.0],
        };
        QParams {
            scale,
            floor,
            mul,
            matrix,
            dc: default_dc_step(color),
            explicit: None,
            bias: None,
            transform: None,
        }
    }

    /// Base table of channel c (Y, X, B - Y order for XYB):
    /// 0: JPEG Annex K (luma for channel 0, chroma otherwise).
    /// 1: jpegli XYB, converted to imij plane units with jpegli's channel
    ///    balance; normalized so the Y table's first AC entry is Annex K's 11.
    /// 2: jpegli XYB shapes; each channel's first AC entry set to Annex K's
    ///    (11 luma, 18 chroma).
    /// 3 (Lossy 0.4): N. Robidoux's table (ImageMagick's and mozjpeg's
    ///    default) for every channel, with Annex K's DC entry.
    /// 4 (Lossy 0.4): jpegli's YCbCr tables (Y, Cb, Cr for channels 0, 1, 2)
    ///    times [`JPEGLI_YCBCR_FACTOR`], so at scale 0.2 (quality 90) they are
    ///    zenjpeg's default (distance 1.0) tables; Annex K's DC entry.
    /// 5 (Lossy 0.4): the same tables reshaped by jpegli's nonlinear distance
    ///    law at distance `5 x scale` (matrix 4 is its linear part), divided
    ///    by that distance; the table follows the file's own scale, so it
    ///    costs no head-record bytes where the explicit id 63 would. Annex
    ///    K's DC entry.
    /// [`MATRIX_EXPLICIT`] (Lossy 0.4): the file's explicit tables.
    fn base(&self, c: usize) -> [f32; 64] {
        let ak = |c: usize| -> [f32; 64] {
            let t = if c == 0 { &JPEG_LUMA } else { &JPEG_CHROMA };
            t.map(|v| v as f32)
        };
        let with_ak_dc = |mut t: [f32; 64]| {
            t[0] = ak(c)[0];
            t
        };
        match self.matrix {
            1 => {
                let plane = [300.0f32, 2400.0, 300.0];
                let unit = |c: usize| plane[c] / JPEGLI_XYB_SCALE[c];
                let f = 11.0 / JPEGLI_XYB[0][1] * unit(c) / unit(0);
                JPEGLI_XYB[c].map(|v| v * f)
            }
            2 => {
                let f = ak(c)[1] / JPEGLI_XYB[c][1];
                JPEGLI_XYB[c].map(|v| v * f)
            }
            3 => with_ak_dc(ROBIDOUX.map(|v| v as f32)),
            4 => with_ak_dc(JPEGLI_YCBCR[c].map(|v| v * JPEGLI_YCBCR_FACTOR)),
            5 => {
                let d = (5.0 * self.scale).max(0.1);
                let mut t = with_ak_dc(JPEGLI_YCBCR[c].map(|v| v * JPEGLI_YCBCR_FACTOR));
                for (i, v) in t.iter_mut().enumerate().skip(1) {
                    *v *= jpegli_distance_to_scale(d, i) / d;
                }
                t
            }
            #[cfg(feature = "lab")]
            6 => {
                // LAB (tablemath study): matrix 5 with each chroma plane's
                // base shape corrected to the measured YCoCg coefficient
                // spectra. The factor is `(u + v)^beta`, normalized at
                // `u + v = 1` (the first AC entry the channel multipliers
                // are relative to); `beta` is -0.0940 for Co and +0.1774
                // for Cg, the least-squares fits of
                // `sqrt(V_YCoCg / V_YCbCr)` over the TRAIN 1024 px
                // renditions of benchmarks/tablemath_2026-10-10/stats.py.
                // The step then tracks the square root of the measured
                // coefficient energy, the rate-distortion optimum under a
                // shared chroma CSF. A study probe that did not qualify
                // (VALIDATE rule miss; benchmarks/tablemath_2026-10-10):
                // lab-only and never chosen automatically.
                let d = (5.0 * self.scale).max(0.1);
                let mut t = with_ak_dc(JPEGLI_YCBCR[c].map(|v| v * JPEGLI_YCBCR_FACTOR));
                for (i, v) in t.iter_mut().enumerate().skip(1) {
                    *v *= jpegli_distance_to_scale(d, i) / d;
                }
                if c > 0 {
                    let beta = if c == 1 { -0.0940 } else { 0.1774 };
                    for (i, v) in t.iter_mut().enumerate().skip(1) {
                        let r = ((i / 8) + (i % 8)).max(1) as f64;
                        *v *= transfer::pow(r, beta) as f32;
                    }
                }
                t
            }
            #[cfg(feature = "lab")]
            7..=9 => {
                // LAB (tablemath study): the matrix-5 law with one or both
                // shape families swapped for Annex K's. 7 = Annex K luma,
                // 8 = Annex K chroma, 9 = Annex K for both. The chosen
                // shape is anchored at its first AC entry, so only the
                // frequency shape changes and the channel multipliers keep
                // their meaning. Study probes that did not qualify; lab-only
                // and never chosen automatically.
                let d = (5.0 * self.scale).max(0.1);
                let ak_shape = if c == 0 {
                    self.matrix != 8
                } else {
                    self.matrix != 7
                };
                let chosen: [f32; 64] = if ak_shape {
                    let a = ak(c);
                    a.map(|v| v / a[1])
                } else {
                    JPEGLI_YCBCR[c].map(|v| v / JPEGLI_YCBCR[c][1])
                };
                let mut t = with_ak_dc(JPEGLI_YCBCR[c].map(|v| v * JPEGLI_YCBCR_FACTOR));
                for i in 1..64 {
                    t[i] = JPEGLI_YCBCR[c][1]
                        * JPEGLI_YCBCR_FACTOR
                        * chosen[i]
                        * jpegli_distance_to_scale(d, i)
                        / d;
                }
                t
            }
            MATRIX_EXPLICIT => self.explicit.map_or([1.0; 64], |t| t[c]),
            _ => ak(c),
        }
    }

    /// Step table before the DC multipliers, [channel][raster position].
    fn base_steps(&self) -> [[f32; 64]; 3] {
        let mut s = [[0.0f32; 64]; 3];
        for (c, sc) in s.iter_mut().enumerate() {
            let base = self.base(c);
            for i in 0..64 {
                sc[i] = (base[i] * self.scale).max(self.floor) * self.mul[c];
            }
        }
        s
    }

    /// Normative step table, [channel][raster position]: the base table
    /// with the DC step multiplied by `dc[c] / 64`, floored at 1/64.
    fn steps(&self) -> [[f32; 64]; 3] {
        let mut s = self.base_steps();
        for (c, sc) in s.iter_mut().enumerate() {
            sc[0] = (sc[0] * (self.dc[c] as f32 / 64.0)).max(1.0 / 64.0);
        }
        s
    }

    #[cfg(feature = "encode")]
    fn apply_ac_scale(&mut self, scale: f32) -> Result<(), Error> {
        if !valid_ac_scale(scale) {
            return Err(Error::InvalidInput("AC scale must be 0.25, 0.5, 1, 2 or 4"));
        }
        if scale == 1.0 {
            return Ok(());
        }
        let old = self.steps();
        for c in 0..3 {
            let dc = f32::from(self.dc[c]) / scale;
            if !(1.0..=255.0).contains(&dc) || dc != (dc as u8) as f32 {
                return Err(Error::InvalidInput(
                    "AC scale cannot preserve the DC multiplier",
                ));
            }
            self.mul[c] *= scale;
            self.dc[c] = dc as u8;
        }
        let scaled = self.steps();
        if scaled.map(|plane| plane[0].to_bits()) != old.map(|plane| plane[0].to_bits()) {
            return Err(Error::InvalidInput("AC scale changes DC rounding"));
        }
        for c in 0..3 {
            for i in 1..64 {
                if scaled[c][i].to_bits() != (old[c][i] * scale).to_bits() {
                    return Err(Error::InvalidInput("AC scale changes step rounding"));
                }
            }
        }
        Ok(())
    }

    fn valid(&self) -> bool {
        let ok = |v: f32| v.is_finite() && v > 0.0 && v < 65536.0;
        self.scale.is_finite()
            && (0.0..65536.0).contains(&self.scale)
            && ok(self.floor)
            && self.floor >= 1.0 / 64.0
            && self.mul.iter().all(|&m| ok(m))
            && self.dc.iter().all(|&d| d != 0)
            // Bounded steps keep every reconstructed value finite.
            // (the DC multipliers only shrink steps).
            && self.base_steps().iter().flatten().all(|&s| s <= 65536.0)
    }
}

/// 2^(-x/4) for 0 <= x <= 16 (encoder parameter mapping only).
fn exp2_neg_quarter(x: f32) -> f32 {
    let steps = (x * 4.0) as u32; // 1/16 resolution
    let mut v = 1.0f32;
    let r = 0.957_603_3f32; // 2^(-1/16)
    for _ in 0..steps {
        v *= r;
    }
    v
}

#[derive(Clone, Copy)]
pub(crate) struct Geom {
    w: usize,
    h: usize,
    bw: usize,
    bh: usize,
    sb: usize,
}

impl Geom {
    fn new(w: u32, h: u32, sb: u32) -> Geom {
        Geom {
            w: w as usize,
            h: h as usize,
            bw: (w as usize).div_ceil(8),
            bh: (h as usize).div_ceil(8),
            sb: sb as usize,
        }
    }
    fn stripes(&self) -> usize {
        self.bh.div_ceil(self.sb)
    }
    /// Block rows [by0, by1) of stripe s.
    fn rows(&self, s: usize) -> (usize, usize) {
        let by0 = s * self.sb;
        (by0, (by0 + self.sb).min(self.bh))
    }
    fn active_lanes(&self) -> usize {
        self.bw.min(NL)
    }
}

/// Quantized coefficients of one stripe: `[(block * 3 + ch) * 64 + raster]`,
/// block = (by - by0) * bw + bx.
#[derive(Clone)]
#[cfg(feature = "encode")]
struct StripeCoefs {
    by0: usize,
    nby: usize,
    /// Blocks per row (the image's, or a column group's).
    bw: usize,
    q: Vec<i32>,
    /// Per (block, ch): bit k set when zigzag coefficient k is nonzero.
    nz: Vec<u64>,
    /// Per block: AQ code a (step multiplier a / 64); empty without AQ.
    aq: Vec<u8>,
    /// One three-bit transform selector per parent, empty for fixed layouts;
    /// with superblocks one merge role per parent.
    modes: Vec<u8>,
    /// Small transforms: per parent the bits of its 4x4 planes (bit c =
    /// plane c); empty without them.
    splits: Vec<u8>,
}

/// Channels coded together in one set of lane streams: channels
/// `ch0..ch0 + nch` of each block, laid out `[(block * nch + c) * 64]`. The
/// first channel's EOB / count context uses the lane's previous value of
/// that channel; the others use this block's first-channel value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Group {
    pub ch0: usize,
    pub nch: usize,
}

impl Group {
    /// All three channels in one group (4:4:4).
    pub(crate) const ALL: Group = Group { ch0: 0, nch: NCH };
}

/// The DC image carries the AQ channel: the activity rule (`aq_gamma`) or,
/// in lab builds, a caller-supplied map.
#[cfg(feature = "encode")]
fn aq_on_cfg(cfg: &LossyConfig) -> bool {
    cfg.aq_gamma != 0.0 || aq_map(cfg).is_some() || aq_field(cfg).is_some()
}

/// Smallest longer side the `Compact` tier merges blocks on: merges lost at
/// 64 px and gained from 128 px up on imazen-26 VALIDATE
/// (benchmarks/superblocks_2026-10-09).
#[cfg(feature = "encode")]
const SUPERBLOCK_MIN_SIDE: u32 = 128;

/// Whether the encoder chooses superblock merges (superblock.rs). `eligible`:
/// the format allows them (default layouts, no keep blocks) and no trellis
/// runs (at efforts 9 and 10 the trellis took most of the gain, and merged
/// units are not trellis-quantized). The production rule: the `Compact`
/// tier, 8-bit samples, the longer side at least
/// [`SUPERBLOCK_MIN_SIDE`]. The lab switch overrides the rule, not
/// `eligible`.
#[cfg(feature = "encode")]
fn superblocks_on(cfg: &LossyConfig, info: &ImageInfo, eligible: bool) -> bool {
    if !eligible {
        return false;
    }
    #[cfg(feature = "lab")]
    if let Some(on) = cfg.lab.superblocks {
        return on;
    }
    (cfg.adaptive_partitions || cfg.decode_speed == Some(crate::DecodeSpeed::Compact))
        && info.sample == SampleType::U8
        && info.width.max(info.height) >= SUPERBLOCK_MIN_SIDE
}

/// The small-transform selector's inner-edge weight: 0 (0.5, 1 and 4 cost
/// at CVVDP 6-8 and gained nothing at the top on the same set).
#[cfg(feature = "encode")]
const SMALL_EDGE: f32 = 0.0;

/// Whether the encoder may code planes as four 4x4 DCTs (small transforms,
/// `small.rs`). `eligible`: the format and the encoder's other tools allow
/// them (default layouts, no keep blocks). Production encoding enables them
/// for explicit `small_transforms`, `adaptive_partitions` on U8, and
/// `DecodeSpeed::Compact` at trellis efforts (`>= 9.0`) on U8 images with
/// longer side `>= SUPERBLOCK_MIN_SIDE`. Lab overrides retain the study
/// configurations (`benchmarks/smallxf_2026-10-11/README.md`).
#[cfg(feature = "encode")]
fn small_on(
    cfg: &LossyConfig,
    info: &ImageInfo,
    effort: crate::effort::Effort,
    eligible: bool,
) -> bool {
    if !eligible {
        return false;
    }
    #[cfg(feature = "lab")]
    if let Some(on) = cfg.lab.small.as_ref().and_then(|s| s.on) {
        return on;
    }
    cfg.small_transforms
        || (cfg.adaptive_partitions && info.sample == SampleType::U8)
        || (cfg.decode_speed == Some(crate::DecodeSpeed::Compact)
            && effort.trellis_passes() >= 1
            && info.sample == SampleType::U8
            && info.width.max(info.height) >= SUPERBLOCK_MIN_SIDE)
}

/// LAB: a lossy file's entropy-decoded coefficients (quantized DCs from the
/// nested DC image in slot 0) as (block width, block height, coefficients
/// at `(block * 3 + channel) * 64 + natural index`, AC record bytes, DC
/// record bytes), for coder comparisons on identical symbols. One column
/// group, default layouts.
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) fn quantized_lab(data: &[u8]) -> Result<(usize, usize, Vec<i32>, usize, usize), Error> {
    if crate::parse::header::header_version(data)? == 4 {
        return v4::inspect_quantized(data);
    }
    let p = parse_lossy(data)?;
    if p.ngroups() != 1 || p.layouts != [LossyChannelLayout::default(); 3] {
        return Err(Error::Unsupported(
            "lab coefficient export: one column group and default layouts",
        ));
    }
    let g = &p.g;
    let mut out = vec![0i32; g.bw * g.bh * NCH * 64];
    for s in 0..g.stripes() {
        let (a, b) = g.rows(s);
        let (q, _, _, _) = decode_stripe_raw(&p, s, Group::ALL, false, Vec::new(), Vec::new())?;
        let n = (b - a) * g.bw * NCH * 64;
        out[a * g.bw * NCH * 64..][..n].copy_from_slice(&q[..n]);
    }
    let ac: usize = p.stripes.iter().map(|r| r.len()).sum();
    let dc_bytes: usize =
        p.dc.as_ref()
            .map_or(0, |d| d.stripes.iter().map(|r| r.len()).sum());
    let (aq, exact, bw, bh) = (p.aq, p.exact.is_some(), g.bw, g.bh);
    let (dinfo, px, _) = dc_image_lab(data)?;
    let (mut dv, mut scales) = (Vec::new(), Vec::new());
    dc_values(dinfo.sample, aq, exact, &px, &mut dv, &mut scales)?;
    for (blk, &d) in out.as_chunks_mut::<64>().0.iter_mut().zip(&dv) {
        blk[0] = d;
    }
    Ok((bw, bh, out, ac, dc_bytes))
}

/// LAB: per lossy unit (record order), per parent, the planes with AC (bit
/// c: a nonzero coefficient in slots 1..63): the planes that carry split
/// flags in a joint map (SPEC 2.11.1), for tests that forge maps.
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) fn unit_ac_bits_lab(data: &[u8]) -> Result<Vec<Vec<u8>>, Error> {
    let p = parse_lossy(data)?;
    let ng = p.ngroups();
    let mut out = Vec::new();
    for (u, &rec) in p.stripes.iter().enumerate() {
        let (s, gi) = (u / ng, u % ng);
        let (_, gg) = p.group(gi);
        let (a, b) = gg.rows(s);
        let (q, _, _, _) =
            decode_stripe_raw_bytes(&p, &gg, s, rec, Group::ALL, false, Vec::new(), Vec::new())?;
        out.push(superblock::ac_bits(&q, (b - a) * gg.bw));
    }
    Ok(out)
}

/// LAB: [`quantized_lab`] for files with partition maps too (the maps are
/// skipped; one column group).
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) fn quantized_lab_any(data: &[u8]) -> Result<(usize, usize, Vec<i32>), Error> {
    let p = parse_lossy(data)?;
    let g = &p.g;
    let mut out = vec![0i32; g.bw * g.bh * NCH * 64];
    for (u, &bytes) in p.stripes.iter().enumerate() {
        let s = u / p.ngroups();
        let (x, gp) = p.group(u % p.ngroups());
        let (a, b) = gp.rows(s);
        let (q, _, _, _) =
            decode_stripe_raw_bytes(&p, &gp, s, bytes, Group::ALL, false, Vec::new(), Vec::new())?;
        let n = gp.bw * NCH * 64;
        for row in 0..b - a {
            let dst = ((a + row) * g.bw + x / 8) * NCH * 64;
            out[dst..dst + n].copy_from_slice(&q[row * n..][..n]);
        }
    }
    Ok((g.bw, g.bh, out))
}

/// LAB: superblock statistics of a lossy file: [parents, parents of 16x16,
/// 16x8 and 8x16 blocks, partition map bytes with their length varints].
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) fn superblock_stats_lab(data: &[u8]) -> Result<[u64; 5], Error> {
    let p = parse_lossy(data)?;
    let mut out = [0u64; 5];
    out[0] = (p.g.bw * p.g.bh) as u64;
    if p.sb.is_none() {
        return Ok(out);
    }
    let ng = p.ngroups();
    let mut roles = Vec::new();
    for (u, &rec) in p.stripes.iter().enumerate() {
        let (s, gi) = (u / ng, u % ng);
        let (_, gg) = p.group(gi);
        let (a, b) = gg.rows(s);
        let (_, rest) = superblock::read_map(rec, b - a, gg.bw, &mut roles)?;
        out[4] += (rec.len() - rest.len()) as u64;
        for &r in &roles {
            if (1..=3).contains(&r) {
                out[r as usize] += 1;
            }
        }
    }
    Ok(out)
}

/// LAB: small-transform statistics of a lossy file: [parents, merged
/// parents, 4x4 planes of plane 0, 1 and 2, partition map bytes with their
/// length varints, AC record bytes] (column groups, exact and kept blocks
/// included).
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) fn small_stats_lab(data: &[u8]) -> Result<[u64; 7], Error> {
    let p = parse_lossy(data)?;
    let mut out = [0u64; 7];
    out[0] = (p.g.bw * p.g.bh) as u64;
    out[6] = p.stripes.iter().map(|r| r.len() as u64).sum();
    if p.map == MapKind::None {
        return Ok(out);
    }
    let ng = p.ngroups();
    // Block multipliers (exact and kept blocks are bypassed).
    let scales: Vec<f32> = if p.aq {
        let (dinfo, px, _) = dc_image_lab(data)?;
        let (mut dv, mut sc) = (Vec::new(), Vec::new());
        dc_values(dinfo.sample, p.aq, p.exact.is_some(), &px, &mut dv, &mut sc)?;
        sc
    } else {
        Vec::new()
    };
    let skip = code0_skips(&p);
    let (mut roles, mut splits) = (Vec::new(), Vec::new());
    for (u, &rec) in p.stripes.iter().enumerate() {
        let (s, gi) = (u / ng, u % ng);
        let (x0, gg) = p.group(gi);
        let (a, b) = gg.rows(s);
        let rest = if p.map == MapKind::Joint {
            let (q, eobs, _, _) = decode_stripe_raw_bytes(
                &p,
                &gg,
                s,
                rec,
                Group::ALL,
                false,
                Vec::new(),
                Vec::new(),
            )?;
            let aq: Vec<f32> = if scales.is_empty() {
                Vec::new()
            } else {
                (a..b)
                    .flat_map(|by| {
                        let row = by * p.g.bw + x0 / 8;
                        scales[row..row + gg.bw].iter().copied()
                    })
                    .collect()
            };
            superblock::read_joint_map(
                rec,
                (b - a, gg.bw),
                (&aq, skip),
                (&q, &eobs),
                &mut roles,
                &mut splits,
            )?
            .2
        } else {
            splits.clear();
            superblock::read_map(rec, b - a, gg.bw, &mut roles)?.1
        };
        out[5] += (rec.len() - rest.len()) as u64;
        out[1] += roles.iter().filter(|&&r| r != 0).count() as u64;
        for &f in &splits {
            for c in 0..3 {
                out[2 + c] += (f >> c & 1) as u64;
            }
        }
    }
    Ok(out)
}

/// LAB: the nested DC image of a lossy file (its info, packed samples and
/// stripe height), for side-information studies and coefficient-parity tests.
#[cfg(any(test, feature = "lab"))]
#[cfg(feature = "encode")]
pub(crate) fn dc_image_lab(data: &[u8]) -> Result<(ImageInfo, Vec<u8>, u32), Error> {
    #[cfg(feature = "lab")]
    if crate::parse::header::header_version(data)? == 4 {
        return v4::inspect_dc_image(data);
    }
    let mut p = parse_lossy(data)?;
    let dc =
        p.dc.take()
            .ok_or(Error::Corrupt("lossy file without DC image"))?;
    let (dinfo, hs) = (dc.info.clone(), dc.hs as u32);
    let rb = dinfo.row_bytes();
    let mut out = vec![0u8; rb * dinfo.height as usize];
    if dc.unit_rectangles.is_some() || p.model_refs.is_some() {
        let ng = dc.ngroups();
        for (u, &bytes) in dc.stripes.iter().enumerate() {
            let (a, b, gi) = dc.unit_rectangles.as_ref().map_or_else(
                || {
                    let a = u / ng * dc.hs;
                    (a, (a + dc.hs).min(dc.info.height as usize), u % ng)
                },
                |v| v[u],
            );
            let (x, w, _) = dc.group(gi);
            let n = w * dc.bpp();
            let (model, body) = if let Some(models) = &p.model_refs {
                models.dc(bytes, &dc)?
            } else {
                (None, bytes)
            };
            let model = model.as_deref().unwrap_or(&dc);
            let om = crate::v1::OutMap::new(&model.info, &model.layout);
            let mut rows = crate::v1::UnitRows::new_with(model, u, body)?;
            for y in a..b {
                rows.next_row(model, &om, &mut out[y * rb + x * dc.bpp()..][..n])?;
            }
        }
        return Ok((dinfo, out, hs));
    }
    let mut stream = crate::v1::LosslessStream::from_parsed(dc);
    for row in out.chunks_mut(rb) {
        if !stream.next_row(row)? {
            return Err(Error::Corrupt("short DC image"));
        }
    }
    Ok((dinfo, out, hs))
}

#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
fn aq_map(cfg: &LossyConfig) -> Option<&[u8]> {
    cfg.aq_map.as_deref()
}

#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
fn aq_field(cfg: &LossyConfig) -> Option<LossyAqField> {
    cfg.aq_field
}

#[cfg(not(feature = "lab"))]
#[cfg(feature = "encode")]
fn aq_field(_: &LossyConfig) -> Option<()> {
    None
}

#[cfg(not(feature = "lab"))]
#[cfg(feature = "encode")]
fn aq_map(_: &LossyConfig) -> Option<&[u8]> {
    None
}

/// LAB: replaces low-frequency AC coefficients by their difference from a
/// prediction made from neighbouring blocks' (dequantized) DCs, in the
/// quantized domain, so a decoder could add it back exactly. Ratios from
/// linear / quadratic / bilinear ramps through the orthonormal DCT.
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
fn lf_pred_lab(
    stripes: &mut [StripeCoefs],
    g: &Geom,
    steps: &[[f32; 64]; 3],
    mode: u8,
    dc_rows: usize,
) {
    let cm = dct::dct_matrix();
    let fdct = |f: &dyn Fn(f32, f32) -> f32, ox: f32, oy: f32| -> [f32; 64] {
        let mut out = [0.0f32; 64];
        for v in 0..8 {
            for u in 0..8 {
                let mut s = 0.0;
                for y in 0..8 {
                    for x in 0..8 {
                        s += cm[v][y] * cm[u][x] * f(ox + x as f32, oy + y as f32);
                    }
                }
                out[v * 8 + u] = s;
            }
        }
        out
    };
    let ramp = |x: f32, _: f32| x;
    let a01 = fdct(&ramp, 0.0, 0.0)[1] / (fdct(&ramp, 8.0, 0.0)[0] - fdct(&ramp, -8.0, 0.0)[0]);
    let quad = |x: f32, _: f32| (x - 3.5) * (x - 3.5);
    let b02 = fdct(&quad, 0.0, 0.0)[2]
        / (fdct(&quad, -8.0, 0.0)[0] + fdct(&quad, 8.0, 0.0)[0] - 2.0 * fdct(&quad, 0.0, 0.0)[0]);
    let bil = |x: f32, y: f32| (x - 3.5) * (y - 3.5);
    let c11 = fdct(&bil, 8.0, 8.0)[0] - fdct(&bil, -8.0, 8.0)[0] - fdct(&bil, 8.0, -8.0)[0]
        + fdct(&bil, -8.0, -8.0)[0];
    let g11 = fdct(&bil, 0.0, 0.0)[9] / c11;
    let (bw, bh) = (g.bw, g.bh);
    // Dequantized DC of every block and channel.
    let mut dc = vec![0.0f32; bw * bh * NCH];
    for sc in stripes.iter() {
        for b in 0..sc.nby * bw {
            for ch in 0..NCH {
                dc[(sc.by0 * bw + b) * NCH + ch] = sc.q[(b * NCH + ch) * 64] as f32 * steps[ch][0];
            }
        }
    }
    let local = mode & 4 != 0;
    let up_ok = |by: usize| by > 0 && !(local && by.is_multiple_of(dc_rows));
    let down_ok = |by: usize| by + 1 < bh && !(local && (by + 1).is_multiple_of(dc_rows));
    let zz_of = |k: usize| ZIGZAG.iter().position(|&z| z as usize == k).unwrap();
    let ks: &[usize] = if mode & 2 != 0 {
        &[1, 8, 2, 16, 9]
    } else {
        &[1, 8]
    };
    let gate = mode & 8 != 0;
    // Both one-sided slopes agree within 50% of the larger one.
    let smooth = |a: f32, c: f32, b: f32| {
        let (s1, s2) = (c - a, b - c);
        !gate || (s1 - s2).abs() <= 0.5 * s1.abs().max(s2.abs())
    };
    for sc in stripes.iter_mut() {
        for r in 0..sc.nby {
            let by = sc.by0 + r;
            for bx in 0..bw {
                let b = r * bw + bx;
                for ch in 0..NCH {
                    let at = |y: usize, x: usize| dc[(y * bw + x) * NCH + ch];
                    let c = at(by, bx);
                    let l = if bx > 0 { at(by, bx - 1) } else { c };
                    let rr = if bx + 1 < bw { at(by, bx + 1) } else { c };
                    let u = if up_ok(by) { at(by - 1, bx) } else { c };
                    let d = if down_ok(by) { at(by + 1, bx) } else { c };
                    let diag = up_ok(by) && down_ok(by) && bx > 0 && bx + 1 < bw;
                    for &k in ks {
                        let hs = smooth(l, c, rr);
                        let vs = smooth(u, c, d);
                        let p = match k {
                            1 if hs => a01 * (rr - l),
                            8 if vs => a01 * (d - u),
                            2 if hs => b02 * (l + rr - 2.0 * c),
                            16 if vs => b02 * (u + d - 2.0 * c),
                            1 | 8 | 2 | 16 => 0.0,
                            _ if diag && hs && vs => {
                                g11 * (at(by + 1, bx + 1) - at(by + 1, bx - 1) - at(by - 1, bx + 1)
                                    + at(by - 1, bx - 1))
                            }
                            _ => 0.0,
                        };
                        let pq = (p / steps[ch][k]).round() as i32;
                        let i = (b * NCH + ch) * 64 + k;
                        sc.q[i] -= pq;
                        let z = zz_of(k);
                        let m = &mut sc.nz[b * NCH + ch];
                        if sc.q[i] != 0 {
                            *m |= 1 << z;
                        } else {
                            *m &= !(1 << z);
                        }
                    }
                }
            }
        }
    }
}

/// Default lossy column group width in pixels; images wider than this get
/// groups. Zen 4, 12 threads: 43534x900 screenshots 2.3x, 32768x2048
/// photos +68%, 1 thread unchanged, +0.4..+1.0% bytes (4096-wide groups:
/// 2.6x / +70% for +0.9..+2.0% bytes and -2..-4% at 1 thread; each unit's
/// 16 lane states cost 64 bytes). benchmarks/lossy_column_groups_zen4_2026-10-03.md.
#[cfg(feature = "encode")]
const AUTO_GROUP_COLS: usize = 8192;

/// DC stripe height in block rows for lossy stripes `sb` block rows tall in
/// a `width` x `height` image: a multiple of the lossy stripe height (a DC
/// stripe covers whole lossy stripes), at least about 8 block rows, and
/// taller where the tier allows: `Balanced` (and no tier) keeps at least 32
/// DC stripes per image, `Compact` at least 8 (16 with a decoder memory
/// budget), as the largest power-of-two pixel height <= height / N, and one
/// for small images (`compact_one_dc_stripe`); `Fast`/`Fastest` stay at 8
/// block rows. Each DC boundary costs bytes, but a DC stripe is decoded
/// serially, so many-thread decode speed tracks the number of DC stripes
/// (benchmarks/lossy_stripe_decode_2026-10-07, lossy_dc_schedule_2026-10-07,
/// lossy_dc_limits_2026-10-07).
/// LAB: `set_lossy_dc_stripes` overrides the lossy stripes per DC stripe.
#[cfg(feature = "encode")]
pub(crate) fn dc_stripe_rows(cfg: &LossyConfig, sb: u32, width: u32, height: u32) -> usize {
    let sb = sb.max(1) as usize;
    #[cfg(feature = "lab")]
    match cfg
        .lab
        .dc_stripes
        .unwrap_or_else(|| DC_STRIPES_LAB.load(core::sync::atomic::Ordering::Relaxed))
    {
        0 => {}
        k => return (sb * k as usize).min(u32::MAX as usize),
    }
    let base = sb * (8 / sb).max(1);
    use crate::DecodeSpeed as D;
    let min_stripes = match cfg.decode_speed.unwrap_or(D::Balanced) {
        D::Compact if compact_one_dc_stripe(cfg, width, height) => {
            // Whole lossy stripes covering the image.
            return base.max((height as usize).div_ceil(8).div_ceil(sb) * sb);
        }
        // Without a memory budget at least 8 (benchmarks/lossy_dc_limits_2026-10-07).
        D::Compact if cfg.decoder_memory_limit.is_none() => 8,
        D::Compact => 16,
        D::Balanced => 32,
        D::Fast | D::Fastest => return base,
    };
    let target_px = (height as usize / min_stripes).max(1);
    // Largest power of two <= target, in block rows, whole lossy stripes.
    let rows = (1usize << target_px.ilog2()) / 8 / sb * sb;
    base.max(rows)
}

/// `Compact` codes images of up to `COMPACT_ONE_DC_PIXELS` with one DC
/// stripe. Each DC boundary costs bytes: one stripe instead of 2-4 saves
/// 3.2-4.3% at q5, 1.0-1.2% at q30 and 0.3-0.4% at q90 (128-256 tall).
/// One-thread decodes (`decode`), streaming and the first row stay within a
/// few percent up to 2048x2048, but with more threads one DC stripe is a
/// serial chain: at 256x256 up to -37% at 4-8 threads on M4 and N1 (at most
/// +37 / +156 us) and -20% on Zen 4; at 512x512 up to -50% (+152 / +465 us)
/// (benchmarks/lossy_dc_schedule_2026-10-07). A decoder memory budget keeps
/// the 16-stripe rule: one DC stripe raises the planner's estimate (2 x 64
/// bytes per block of a DC stripe per worker) far more than the decoder's
/// use.
#[cfg(feature = "encode")]
fn compact_one_dc_stripe(cfg: &LossyConfig, width: u32, height: u32) -> bool {
    cfg.decoder_memory_limit.is_none()
        && u64::from(width) * u64::from(height) <= COMPACT_ONE_DC_PIXELS
}

/// Largest image `Compact` codes with one DC stripe (256 x 256).
#[cfg(feature = "encode")]
const COMPACT_ONE_DC_PIXELS: u64 = 65_536;

/// LAB: lossy stripes per DC stripe (0 = automatic).
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) static DC_STRIPES_LAB: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// LAB (coefgap study): the decode-cost proxy of EOB block-bucket contexts
/// (see `eob_bucket_proxy`).
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) static EOB_BKT_PROXY: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// LAB: the proxy's mode (see `eob_bucket_proxy`), 1 by default.
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) static EOB_BKT_PROXY_MODE: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(1);

/// LAB: an offset added to every proxy bucket (0; 1 only to check that the
/// proxy reaches the kernels: decodes then go wrong).
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) static EOB_BKT_PROBE: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// LAB (coefgap study): the decode work of an EOB context split by a
/// per-block bucket (a candidate format change, not implemented): per stripe
/// the luma DC gradient |DC - left DC| + |DC - above DC| of every block,
/// quantized by three thresholds into an EOB context offset (bucket x 24),
/// which the run-level kernels load at each block start and add to the EOB
/// context. The entropy decode has no DC values yet, so this proxy runs the
/// same arithmetic on a zero buffer and scales the offsets by a runtime zero:
/// the coefficients and pixels stay exact. Not modelled: moving the stripe's
/// DC rows ahead of its entropy decode in multi-threaded schedules.
#[cfg(feature = "lab")]
fn eob_bucket_proxy(nb: usize, bw: usize) -> Option<alloc::vec::Vec<u32>> {
    use core::sync::atomic::Ordering::Relaxed;
    // Modes: 0 off, 1 bucket pass and kernel loads, 2 the pass only (the
    // kernels get no buckets), 3 the kernel loads only (a zero array).
    let mode = EOB_BKT_PROXY_MODE.load(Relaxed);
    if !EOB_BKT_PROXY.load(Relaxed) || mode == 0 {
        return None;
    }
    let mut out = alloc::vec![0u32; nb + NL];
    if mode != 3 {
        let dc = alloc::vec![0i32; nb];
        let zero = core::hint::black_box(0u32);
        let probe = EOB_BKT_PROBE.load(Relaxed);
        let th = [4i32, 12, 32];
        let bucket = |g: i32| -> u32 {
            ((g > th[0]) as u32 + (g > th[1]) as u32 + (g > th[2]) as u32) * 24 * zero + probe
        };
        for (r, (row, o)) in dc.chunks(bw).zip(out[..nb].chunks_mut(bw)).enumerate() {
            // Left neighbours in the row; the row above inside the stripe.
            o[0] = bucket(if r > 0 {
                (row[0] - dc[(r - 1) * bw]).abs()
            } else {
                0
            });
            if r > 0 {
                let up = &dc[(r - 1) * bw..r * bw];
                for (((v, &d), &l), &u) in o[1..].iter_mut().zip(&row[1..]).zip(row).zip(&up[1..]) {
                    *v = bucket((d - l).abs() + (d - u).abs());
                }
            } else {
                for ((v, &d), &l) in o[1..].iter_mut().zip(&row[1..]).zip(row) {
                    *v = bucket((d - l).abs());
                }
            }
        }
        if mode == 2 {
            core::hint::black_box(&out);
            return None;
        }
    }
    Some(out)
}

// Called only from the x86 two-stripe decode path; other targets never use it.
#[cfg(not(feature = "lab"))]
#[allow(dead_code)]
fn eob_bucket_proxy(_: usize, _: usize) -> Option<alloc::vec::Vec<u32>> {
    None
}

/// LAB (coefgap study): an override of how the lossy encoder picks its
/// table counts ([`TABLE_K_DEFAULT`]: the production rule,
/// `coef::k_search_share`): 0 the effort's budgets, 1 the budgets' clusters
/// merged down to the count that codes smallest (`coef::merge_by_cost`), 2 the
/// counts whose seeded tables code smallest (`coef::KSearch`; split-Huffman:
/// `huf::HSearch`), 3 mode 2 only where the tables weigh
/// (`coef::K_SEARCH_MIN_SHARE`; the production rule from effort 7.5).
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) static TABLE_K_SEARCH: core::sync::atomic::AtomicU8 =
    core::sync::atomic::AtomicU8::new(TABLE_K_DEFAULT);

/// LAB: [`TABLE_K_SEARCH`] without an override.
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) const TABLE_K_DEFAULT: u8 = u8::MAX;

/// LAB (coefgap study): lanes of the nested DC image (0 = the lossless
/// encoder's default, `default_lanes`).
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) static DC_LANES_LAB: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// The lab table-count override of an encode
/// ([`LossyLabSwitches::table_k_mode`]; `u8::MAX`, none: the production rule,
/// always without `lab`).
#[cfg(feature = "encode")]
pub(crate) fn table_k_mode(cfg: &LossyConfig) -> u8 {
    #[cfg(feature = "lab")]
    {
        cfg.lab
            .table_k_mode
            .unwrap_or_else(|| TABLE_K_SEARCH.load(core::sync::atomic::Ordering::Relaxed))
    }
    #[cfg(not(feature = "lab"))]
    {
        let _ = cfg;
        u8::MAX
    }
}

/// LAB: the base tables of preset `id` (0..=[`MATRIX_LAST_PRESET`], raster
/// order, as the decoder reads them): imij-cmp's explicit-table catalog
/// builds its variants (swapped chroma and the like) from these.
#[cfg(feature = "lab")]
#[doc(hidden)]
#[cfg(feature = "encode")]
pub fn lab_base_tables(id: u8) -> Option<[[f32; 64]; 3]> {
    (id <= MATRIX_LAST_PRESET).then(|| {
        let qp = QParams::from_quality(90.0, LossyColor::YCoCg, id);
        [0, 1, 2].map(|c| qp.base(c))
    })
}

/// The nested DC image's lane count (`None` = the lossless default).
#[cfg(feature = "encode")]
fn dc_lanes(cfg: &LossyConfig) -> Option<u32> {
    #[cfg(feature = "lab")]
    {
        let n = cfg
            .lab
            .dc_lanes
            .unwrap_or_else(|| DC_LANES_LAB.load(core::sync::atomic::Ordering::Relaxed));
        if n > 0 {
            return Some(n);
        }
    }
    let _ = cfg;
    None
}

/// Lossy column group width for an image `w` wide (0 = none; else a
/// multiple of 8 below `w`).
#[cfg(feature = "encode")]
pub(crate) fn lossy_group_cols(cfg: &LossyConfig, w: usize) -> usize {
    let g = match cfg.column_groups {
        Some(g) => g as usize & !7,
        None if w > AUTO_GROUP_COLS => AUTO_GROUP_COLS,
        None => 0,
    };
    if g == 0 || g >= w { 0 } else { g }
}

/// Splits a stripe's coefficients into column-group units of `gb` blocks.
#[cfg(feature = "encode")]
fn split_units(sc: StripeCoefs, gb: usize) -> Vec<StripeCoefs> {
    let bw = sc.bw;
    (0..bw)
        .step_by(gb)
        .map(|bx0| {
            let gbw = gb.min(bw - bx0);
            let n = sc.nby * gbw;
            let (mut q, mut nz, mut aq) = (
                Vec::with_capacity(n * NCH * 64),
                Vec::with_capacity(n * NCH),
                Vec::new(),
            );
            let mut modes = Vec::new();
            let mut splits = Vec::new();
            for r in 0..sc.nby {
                let b0 = r * bw + bx0;
                q.extend_from_slice(&sc.q[b0 * NCH * 64..(b0 + gbw) * NCH * 64]);
                nz.extend_from_slice(&sc.nz[b0 * NCH..(b0 + gbw) * NCH]);
                if !sc.modes.is_empty() {
                    modes.extend_from_slice(&sc.modes[b0..b0 + gbw]);
                }
                if !sc.splits.is_empty() {
                    splits.extend_from_slice(&sc.splits[b0..b0 + gbw]);
                }
                if !sc.aq.is_empty() {
                    aq.extend_from_slice(&sc.aq[b0..b0 + gbw]);
                }
            }
            StripeCoefs {
                by0: sc.by0,
                nby: sc.nby,
                bw: gbw,
                q,
                nz,
                aq,
                modes,
                splits,
            }
        })
        .collect()
}

/// Visits the tokens of every lane of a stripe in lane order (encoder):
/// `emit(lane, ctx, token, extra bit count, extra bits)`.
#[cfg(feature = "encode")]
fn stripe_tokens(
    sc: &StripeCoefs,
    bw: usize,
    grp: Group,
    mut emit: impl FnMut(usize, usize, u32, u32, u32),
) {
    let nch = grp.nch;
    for l in 0..NL {
        let mut prev_luma = 0u32;
        for r in 0..sc.nby {
            for bx in (l..bw).step_by(NL) {
                let b = r * bw + bx;
                let mut luma = 0u32;
                for c in 0..nch {
                    let ch = grp.ch0 + c;
                    let q: &[i32; 64] = sc.q[(b * nch + c) * 64..][..64].try_into().unwrap();
                    let ac = sc.nz[b * nch + c] & !1;
                    let eob = if ac == 0 { 0 } else { 63 - ac.leading_zeros() };
                    emit(
                        l,
                        eob_ctx(ch, if c == 0 { prev_luma } else { luma }),
                        eob,
                        0,
                        0,
                    );
                    if c == 0 {
                        luma = eob;
                        prev_luma = eob;
                    }
                    // Run-level tokens; `start` is the decoder's next position.
                    let mut start = 1u32;
                    let mut bits = ac;
                    while bits != 0 {
                        let k = bits.trailing_zeros();
                        bits &= bits - 1;
                        let v = q[ZIGZAG[k as usize] as usize];
                        let mut run = k - start;
                        while run > coef::MAX_RUN {
                            emit(l, ac_ctx(ch, start, eob), coef::ZRL, 0, 0);
                            start += 4;
                            run -= 4;
                        }
                        let (t, e, n) = rl_token(run, v);
                        emit(l, ac_ctx(ch, start, eob), t, n, e);
                        start = k + 1;
                    }
                }
            }
        }
    }
}

/// The split-Huffman mode's plain bit streams of a stripe: magnitudes
/// (`len * w` bits per group, coefficient j at bit `j * w`) and signs (one
/// bit per nonzero, 1 = negative), blocks in raster order, channels,
/// groups.
#[cfg(feature = "encode")]
fn stripe_raw_split(
    sc: &StripeCoefs,
    bw: usize,
    counts: &[u8],
    widths: &[u8],
) -> (Vec<u8>, Vec<u8>) {
    debug_assert_eq!(counts.len(), sc.nby * bw * NCH);
    debug_assert_eq!(widths.len(), counts.len() * coef::PK_GROUPS);
    let (mut mag, mut sgn) = (rawbits::RawWriter::default(), rawbits::RawWriter::default());
    for b in 0..sc.nby * bw {
        for c in 0..NCH {
            let bc = b * NCH + c;
            let q: &[i32; 64] = sc.q[bc * 64..][..64].try_into().unwrap();
            let n = counts[bc] as usize;
            for gi in 0..n {
                let len = coef::pk_len(gi);
                let vals: [i32; 4] = core::array::from_fn(|j| {
                    if j < len {
                        q[ZIGZAG[1 + 4 * gi + j] as usize]
                    } else {
                        0
                    }
                });
                let w = u32::from(widths[bc * coef::PK_GROUPS + gi]);
                for &v in &vals[..len] {
                    mag.put(v.unsigned_abs(), w);
                }
                for &v in &vals[..len] {
                    if v != 0 {
                        sgn.put((v < 0) as u32, 1);
                    }
                }
            }
        }
    }
    (mag.finish(), sgn.finish())
}

/// Split-mode group counts `[bc]` and widths `[bc * 16 + gi]` of a stripe.
#[cfg(feature = "encode")]
fn stripe_counts_widths(sc: &StripeCoefs, bw: usize) -> (Vec<u8>, Vec<u8>) {
    let nbc = sc.nby * bw * NCH;
    let mut counts = vec![0u8; nbc];
    let mut widths = vec![0u8; nbc * coef::PK_GROUPS];
    for bc in 0..nbc {
        let q: &[i32; 64] = sc.q[bc * 64..][..64].try_into().unwrap();
        let ac = sc.nz[bc] & !1;
        let eob = if ac == 0 { 0 } else { 63 - ac.leading_zeros() };
        let n = eob.div_ceil(4) as usize;
        counts[bc] = n as u8;
        for gi in 0..n {
            let len = coef::pk_len(gi);
            let m = (0..len)
                .map(|j| q[ZIGZAG[1 + 4 * gi + j] as usize].unsigned_abs())
                .max()
                .unwrap();
            widths[bc * coef::PK_GROUPS + gi] = (32 - m.leading_zeros()) as u8;
        }
    }
    (counts, widths)
}

#[cfg(feature = "encode")]
fn stripe_symbols(sc: &StripeCoefs, bw: usize, lanes: &mut [Vec<Sym>; NL]) {
    let mut steps = [0u32; NL];
    for l in lanes.iter_mut() {
        l.clear();
        l.reserve(sc.q.len() / 64 / NL * 8);
    }
    stripe_tokens(sc, bw, Group::ALL, |l, ctx, tok, nb, extra| {
        lanes[l].push(Sym {
            step: steps[l],
            ctx: ctx as u16,
            tok: tok as u8,
            nb: nb as u8,
            extra,
        });
        steps[l] += 1;
    });
    #[cfg(all(feature = "lab", feature = "std"))]
    prof::tokens(&steps);
}

/// Build image-wide ANS tables with bounded, worker-local histogram storage.
#[cfg(feature = "encode")]
fn ans_tables(
    stripes: &[StripeCoefs],
    cfg: &LossyConfig,
    effort: crate::effort::Effort,
) -> (Tables, Option<Tables>) {
    ans_tables_with_bank(stripes, cfg, effort, seed_bank(cfg))
}

/// Use the usual table-count selection for coefficient and partition prices.
/// Choosing a different file bank is an entropy-writing decision.
#[cfg(feature = "encode")]
fn ans_tables_for_decisions(
    stripes: &[StripeCoefs],
    cfg: &LossyConfig,
    effort: crate::effort::Effort,
) -> (Tables, Option<Tables>) {
    ans_tables_with_bank(
        stripes,
        cfg,
        effort,
        crate::seeds::bank(crate::seeds::DEFAULT_BANK),
    )
}

#[cfg(feature = "encode")]
fn ans_tables_with_bank(
    stripes: &[StripeCoefs],
    cfg: &LossyConfig,
    effort: crate::effort::Effort,
    bank: Option<&'static crate::seeds::Bank>,
) -> (Tables, Option<Tables>) {
    // u64 is needed because each worker can accumulate many stripes.
    let hists = crate::par::par_fold(
        stripes,
        cfg.threads,
        || vec![0u64; LossyEntropy::RunLevel.nraw() * coef::ALPHA],
        |hist, sc| {
            stripe_tokens(sc, sc.bw, Group::ALL, |_, ctx, tok, _, _| {
                hist[ctx * coef::ALPHA + tok as usize] += 1;
            });
        },
    );
    let mut hists = hists.into_iter();
    let mut hist = hists.next().unwrap();
    for h in hists {
        for (sum, count) in hist.iter_mut().zip(h) {
            *sum += count;
        }
    }
    let hist: Vec<Vec<u64>> = hist
        .as_chunks::<{ coef::ALPHA }>()
        .0
        .iter()
        .map(|row| row.to_vec())
        .collect();
    Tables::build_seeded(
        &hist,
        4,
        cfg.max_ac_tables,
        LossyEntropy::RunLevel,
        effort,
        bank,
        table_k_mode(cfg),
    )
}

/// Retain legacy packed-bank robustness/resource fixtures while the default
/// writer uses the general varint field. Entropy payloads are unchanged.
#[cfg(test)]
pub(crate) fn legacy_seed_fixture(file: &[u8]) -> Vec<u8> {
    let (info, color, body, declared) = crate::v1::lossy_body(file).unwrap().unwrap();
    let header = crate::v1::header(file).unwrap();
    let (_, meta, _) = crate::v1::parse_header(file).unwrap();
    let mut r = ByteReader::new(body);
    r.u8().unwrap();
    r.varint().unwrap();
    let prefix = r.pos;
    let mut e =
        envelope::read(&mut r, color, InputKind::of(&info).unwrap(), declared, true).unwrap();
    let rest = r.pos;
    if e.seed.is_some() {
        assert!(e.seed.bank <= 15);
        e.seed.legacy = true;
    }
    let mut data = body[..prefix].to_vec();
    envelope::write(&mut data, color, InputKind::of(&info).unwrap(), &e);
    data.extend_from_slice(&body[rest..]);
    let mut features = declared.without(crate::parse::header::Feature::SeedBankIds);
    if e.seed.is_some() {
        features = features.with(crate::parse::header::Feature::SeedBank1);
    }
    let mut out = Vec::new();
    crate::container::write_head_features(
        &mut out,
        &info,
        &meta,
        crate::Coding::Lossy,
        Some(color),
        header.decode_speed,
        features,
    )
    .unwrap();
    crate::container::put_data(&mut out, &data);
    out
}

/// Lab: entropy mode and the independent bank/scope selection.
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) fn lab_entropy_seed(
    file: &[u8],
) -> Result<(LossyEntropy, crate::seeds::Selection), Error> {
    if crate::parse::header::header_version(file)? == 4 {
        return v4::inspect_models(file).map(|(coder, seed, _)| (coder, seed));
    }
    let (info, color, body, declared) =
        crate::v1::lossy_body(file)?.ok_or(Error::InvalidInput("not a lossy file"))?;
    let mut r = ByteReader::new(body);
    r.u8()?; // stripe rows
    r.varint()?; // column groups
    let kind = InputKind::of(&info).ok_or(Error::Corrupt("pixel format"))?;
    let e = envelope::read(&mut r, color, kind, declared, true)?;
    Ok((e.entropy, e.seed))
}

/// Lab: the entropy mode and AC entropy-table payload bytes of a lossy
/// file: the length-prefixed table section between the parameter envelope
/// and the head records / nested heads (what `Tables::write` /
/// `huf::HufTables::write` produced). Reads the same grammar as
/// [`lab_entropy_seed`].
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) fn lab_ac_table_bytes(file: &[u8]) -> Result<(LossyEntropy, u64), Error> {
    if crate::parse::header::header_version(file)? == 4 {
        return v4::inspect_models(file).map(|(coder, _, bytes)| (coder, bytes));
    }
    let (info, color, body, declared) =
        crate::v1::lossy_body(file)?.ok_or(Error::InvalidInput("not a lossy file"))?;
    let mut r = ByteReader::new(body);
    r.u8()?; // stripe rows
    r.varint()?; // column groups
    let kind = InputKind::of(&info).ok_or(Error::Corrupt("pixel format"))?;
    let e = envelope::read(&mut r, color, kind, declared, true)?;
    Ok((e.entropy, r.varint()?))
}

/// The seed bank `cfg` encodes with: bank 1 (lab: `seed_bank`, 0 = none).
#[cfg(feature = "encode")]
fn seed_bank(cfg: &LossyConfig) -> Option<&'static crate::seeds::Bank> {
    #[cfg(feature = "lab")]
    let id = cfg.seed_bank;
    #[cfg(not(feature = "lab"))]
    let id = {
        let _ = cfg;
        crate::seeds::DEFAULT_BANK
    };
    crate::seeds::bank(id)
}

/// Seeded blobs are also coded explicitly when the lab `seed_exact` policy is on.
#[cfg(feature = "encode")]
fn seed_exact(cfg: &LossyConfig) -> bool {
    #[cfg(feature = "lab")]
    return cfg.seed_exact;
    #[cfg(not(feature = "lab"))]
    {
        let _ = cfg;
        true
    }
}

/// Table-independent stripe work, shared by seeded and explicit trials.
#[cfg(feature = "encode")]
struct HuffmanStripe {
    symbols: huf::Syms,
    magnitudes: Vec<u8>,
    signs: Vec<u8>,
}

#[cfg(feature = "encode")]
fn huffman_tables(
    syms: &[huf::Syms],
    threads: usize,
    effort: crate::effort::Effort,
    bank: Option<&'static crate::seeds::Bank>,
    k_mode: u8,
) -> (huf::HufTables, Option<huf::HufTables>) {
    let hists = crate::par::par_fold(
        syms,
        threads,
        || {
            [
                vec![vec![0u64; huf::CNT_ALPHA]; huf::LUMA_CTX],
                vec![vec![0u64; huf::CNT_ALPHA]; huf::CHROMA_CTX],
                vec![vec![0u64; huf::WID_ALPHA]; huf::WID_CTX],
            ]
        },
        |hist, syms| {
            for (h, symbols) in hist.iter_mut().zip([&syms.luma, &syms.chroma, &syms.wid]) {
                for &(ctx, sym) in symbols {
                    h[ctx as usize][sym as usize] += 1;
                }
            }
        },
    );
    let mut hists = hists.into_iter();
    let mut hist = hists.next().unwrap();
    for h in hists {
        for (dst, src) in hist.iter_mut().zip(h) {
            for (a, b) in dst.iter_mut().zip(src) {
                for (sum, count) in a.iter_mut().zip(b) {
                    *sum += count;
                }
            }
        }
    }
    huf::HufTables::build(&hist[0], &hist[1], &hist[2], effort, bank, k_mode)
}

/// Split-Huffman stores three side-information sections, then raw magnitudes
/// and signs. Canonical codes are shared by all stripe workers.
#[cfg(feature = "encode")]
fn encode_huffman_stripes(
    stripes: &[StripeCoefs],
    cfg: &LossyConfig,
    effort: crate::effort::Effort,
    pulse: &dyn Pulse,
) -> Result<(Vec<u8>, Vec<Vec<u8>>, Option<Coded>), Error> {
    if cfg.model_references {
        return model_candidates::encode_native(
            stripes,
            cfg,
            effort,
            LossyEntropy::SplitHuf,
            pulse,
        );
    }
    if cfg.catalog_lab.is_some() {
        return model_candidates::encode(stripes, cfg, effort, LossyEntropy::SplitHuf, pulse);
    }
    let pulse = pulse.live();
    use how_far::Stop;
    pulse.check().map_err(|_| Error::Cancelled)?;
    // Enumerate each stripe once: the same section symbols feed both the
    // table histograms and the final coding.
    let syms: Vec<HuffmanStripe> = crate::par::par_map(
        stripes.iter().collect(),
        cfg.threads,
        |_, sc| -> Result<_, Error> {
            pulse.check().map_err(|_| Error::Cancelled)?;
            let (counts, widths) = stripe_counts_widths(sc, sc.bw);
            let symbols = huf::stripe_syms(&counts, &widths, sc.nby * sc.bw);
            let (magnitudes, signs) = stripe_raw_split(sc, sc.bw, &counts, &widths);
            Ok(HuffmanStripe {
                symbols,
                magnitudes,
                signs,
            })
        },
    )
    .into_iter()
    .collect::<Result<Vec<_>, Error>>()?;
    let (syms, raw): (Vec<_>, Vec<_>) = syms
        .into_iter()
        .map(|stripe| (stripe.symbols, (stripe.magnitudes, stripe.signs)))
        .unzip();
    let (tables, explicit) = huffman_tables(
        &syms,
        cfg.threads,
        effort,
        seed_bank(cfg),
        table_k_mode(cfg),
    );
    let code = |tables: &huf::HufTables| -> Result<(Vec<u8>, Vec<Vec<u8>>), Error> {
        pulse.check().map_err(|_| Error::Cancelled)?;
        let encoder = tables.encoder();
        let stripes = crate::par::par_map(
            syms.iter().zip(raw.iter()).collect(),
            cfg.threads,
            |_, (symbols, (magnitudes, signs))| -> Result<_, Error> {
                pulse.check().map_err(|_| Error::Cancelled)?;
                let mut out = Vec::new();
                for (symbols, map) in [
                    (&symbols.luma, &tables.luma_map[..]),
                    (&symbols.chroma, &tables.chroma_map[..]),
                    (&symbols.wid, &tables.wid_map[..]),
                ] {
                    let mut section = Vec::new();
                    encoder.encode_section(symbols, map, &mut section);
                    put_varint(&mut out, section.len() as u64);
                    out.extend_from_slice(&section);
                }
                put_varint(&mut out, magnitudes.len() as u64);
                out.extend_from_slice(magnitudes);
                out.extend_from_slice(signs);
                pulse.advance(1);
                pulse.check().map_err(|_| Error::Cancelled)?;
                Ok(out)
            },
        )
        .into_iter()
        .collect::<Result<Vec<_>, Error>>()?;
        let mut header = BitWriter::new();
        tables.write(&mut header);
        Ok((header.finish(), stripes))
    };
    let (tb, records) = code(&tables)?;
    let explicit = explicit.filter(|_| seed_exact(cfg));
    Ok((tb, records, explicit.map(|t| code(&t)).transpose()?))
}

/// Run-level lane coding. Returns `(header, stripe records, token count)`:
/// the count is the run-level token total [`side_stats`] needs, free from
/// the lane pass.
#[cfg(feature = "encode")]
fn encode_ans_stripes(
    stripes: &[StripeCoefs],
    cfg: &LossyConfig,
    effort: crate::effort::Effort,
    pulse: &dyn Pulse,
) -> Result<(Vec<u8>, Vec<Vec<u8>>, u64, Option<Coded>), Error> {
    if cfg.model_references || cfg.catalog_lab.is_some() {
        let (tb, records, explicit) = if cfg.model_references {
            model_candidates::encode_native(stripes, cfg, effort, LossyEntropy::RunLevel, pulse)?
        } else {
            model_candidates::encode(stripes, cfg, effort, LossyEntropy::RunLevel, pulse)?
        };
        let tokens = if cfg.decode_speed.is_some() {
            stripes
                .iter()
                .map(|sc| {
                    let mut lanes = core::array::from_fn(|_| Vec::new());
                    stripe_symbols(sc, sc.bw, &mut lanes);
                    lanes.iter().map(|l| l.len() as u64).sum::<u64>()
                })
                .sum()
        } else {
            0
        };
        return Ok((tb, records, tokens, explicit));
    }
    let pulse = pulse.live();
    use how_far::Stop;
    pulse.check().map_err(|_| Error::Cancelled)?;
    // The baseline structure verbatim — the table histogram counts tokens
    // directly and each stripe's lane list exists only inside its encode
    // worker. On the tier path the run-level token count rides back with
    // the records; the default path adds nothing.
    let want = cfg.decode_speed.is_some();
    let (tables, explicit) = ans_tables(stripes, cfg, effort);
    #[cfg(all(feature = "lab", feature = "std"))]
    block_lab::record_block_bits(stripes, &tables);
    let code = |tables: &Tables| -> Result<(Vec<u8>, Vec<Vec<u8>>, u64), Error> {
        pulse.check().map_err(|_| Error::Cancelled)?;
        let encoder = EncodeTables::new(&tables.freq, &tables.ctx_map);
        let coded = crate::par::par_map(
            stripes.iter().collect(),
            cfg.threads,
            |_, sc| -> Result<_, Error> {
                pulse.check().map_err(|_| Error::Cancelled)?;
                let mut lanes: [Vec<Sym>; NL] = core::array::from_fn(|_| Vec::new());
                stripe_symbols(sc, sc.bw, &mut lanes);
                let mut out = Vec::new();
                encode_lanes_tables(&encoder, &lanes, sc.bw.min(NL), &mut out);
                if cfg.local_ans_contexts {
                    out = local_ans::encode(&lanes, sc.bw.min(NL), cfg, effort, &out)?;
                }
                let tokens = if want {
                    lanes.iter().map(|l| l.len() as u64).sum()
                } else {
                    0
                };
                pulse.advance(1);
                pulse.check().map_err(|_| Error::Cancelled)?;
                Ok((out, tokens))
            },
        )
        .into_iter()
        .collect::<Result<Vec<_>, Error>>()?;
        let tokens = coded.iter().map(|(_, t)| *t).sum();
        let stripes = coded.into_iter().map(|(out, _)| out).collect();
        let mut header = BitWriter::new();
        tables.write(&mut header);
        Ok((header.finish(), stripes, tokens))
    };
    let (tb, records, tokens) = code(&tables)?;
    let explicit = explicit
        .filter(|_| seed_exact(cfg))
        .map(|t| {
            let (tb, records, _) = code(&t)?;
            Ok::<_, Error>((tb, records))
        })
        .transpose()?;
    Ok((tb, records, tokens, explicit))
}

/// A coded table blob and its records (the explicit-table variant of a
/// seeded coding).
#[cfg(feature = "encode")]
type Coded = (Vec<u8>, Vec<Vec<u8>>);

/// Test helper: the lossy coder without progress.
#[cfg(all(test, feature = "encode"))]
#[cfg(feature = "encode")]
pub(crate) fn encode_lossy(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
) -> Result<Vec<u8>, Error> {
    encode_lossy_with_progress(pixels, stride, info, meta, cfg, &how_far::NoPulse)
}

#[cfg(feature = "encode")]
pub(crate) fn encode_lossy_with_progress(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    pulse: &dyn Pulse,
) -> Result<Vec<u8>, Error> {
    pulse.check().map_err(|_| Error::Cancelled)?;
    Stages::new(
        pulse,
        &[
            PhaseSpec::new("lossy payload", 1, Total::Unknown),
            PhaseSpec::new("unit index", 1, Total::Unknown),
            PhaseSpec::new("locator", 1, Total::Unknown),
            PhaseSpec::new("file checksum", 1, Total::Unknown),
        ],
    )
    .complete_with(|stages| {
        let out = stages
            .run(|p| encode_lossy_frame_with_progress(pixels, stride, info, meta, cfg, None, p))?;
        let out = stages.run(|p| {
            p.check().map_err(|_| Error::Cancelled)?;
            let out = crate::container::finish_unit_table(out)?;
            p.check().map_err(|_| Error::Cancelled)?;
            Ok(out)
        })?;
        let out = stages.run(|p| {
            p.check().map_err(|_| Error::Cancelled)?;
            let out = crate::container::locate(out)?;
            p.check().map_err(|_| Error::Cancelled)?;
            Ok(out)
        })?;
        if !cfg.checksum {
            stages.skip();
            pulse.check().map_err(|_| Error::Cancelled)?;
            return Ok(out);
        }
        stages.run(|p| {
            p.check().map_err(|_| Error::Cancelled)?;
            let out = crate::checksum::finish_file(out, Some(cfg.hash))?;
            p.check().map_err(|_| Error::Cancelled)?;
            Ok(out)
        })
    })
}

/// [`encode_lossy`] of an inter frame (or any nested image: never a checksum): `previous` (pixels, stride; the
/// image's size and format) is the previous frame's source under the frame.
/// Blocks whose pixels equal it are coded "keep": the decoder's canvas
/// already holds them as coded before (RGB layouts; ignored otherwise).
#[cfg(feature = "encode")]
pub(crate) fn encode_lossy_frame(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    previous: Option<(&[u8], usize)>,
) -> Result<Vec<u8>, Error> {
    encode_lossy_frame_with_progress(pixels, stride, info, meta, cfg, previous, &how_far::NoPulse)
}

#[allow(clippy::too_many_arguments)]
#[cfg(feature = "encode")]
pub(crate) fn encode_lossy_frame_with_progress(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    previous: Option<(&[u8], usize)>,
    pulse: &dyn Pulse,
) -> Result<Vec<u8>, Error> {
    pulse.check().map_err(|_| Error::Cancelled)?;
    if info.channels.is_native_model() {
        return Err(Error::Unsupported("pixel model requires lossless coding"));
    }
    // LAB: this encode's switches, fixed now for every thread it uses.
    #[cfg(feature = "lab")]
    let pinned_lab;
    #[cfg(feature = "lab")]
    let cfg = {
        let mut c = cfg.clone();
        c.lab = c.lab.pinned();
        pinned_lab = c;
        &pinned_lab
    };
    local_ans::validate(cfg)?;
    #[cfg(feature = "lab")]
    if cfg.coding_defaults && cfg.lab.small.as_ref().is_some_and(|s| s.int) {
        return Err(Error::InvalidInput(
            "coding defaults use native float transforms",
        ));
    }
    #[cfg(feature = "lab")]
    crate::seeds::validate_id(cfg.seed_bank)?;
    info.validate_precision()?;
    let effort = crate::effort::Effort::new(cfg.effort)?;
    let planned;
    let cfg = if cfg.decoder_memory_limit.is_some() {
        planned = crate::decoder_memory::plan_lossy(info, cfg)?.0;
        &planned
    } else {
        cfg
    };
    if !cfg.quality.is_finite()
        || !cfg.round.is_finite()
        || !cfg.aq_gamma.is_finite()
        || !cfg.aq_r0.is_finite()
        || (cfg.aq_gamma != 0.0 && cfg.aq_r0 <= 0.0)
    {
        return Err(Error::InvalidInput("invalid lossy numeric parameter"));
    }
    #[cfg(feature = "lab")]
    {
        if !cfg.rdoq_lambda.is_finite() || !(0.0..=4.0).contains(&cfg.rdoq_lambda) {
            return Err(Error::InvalidInput("invalid lab RDOQ lambda"));
        }
        if cfg.rdoq_policy > 4 {
            return Err(Error::InvalidInput("invalid lab RDOQ policy"));
        }
        if cfg.rdoq_policy != 0
            && cfg.rdoq_lambda != 0.0
            && (info.channels != Channels::Rgb
                || info.cicp != crate::Cicp::SRGB
                || cfg.color != LossyColor::Xyb)
        {
            return Err(Error::Unsupported(
                "bounded lab RDOQ requires sRGB RGB8 XYB",
            ));
        }
        if cfg.rdoq_lambda != 0.0
            && (cfg.entropy != LossyEntropy::RunLevel
                || cfg.decode_speed.is_some()
                || aq_on_cfg(cfg)
                || cfg.lf_pred != 0
                || previous.is_some()
                || info.sample != SampleType::U8
                || info.has_custom_precision())
        {
            return Err(Error::Unsupported(
                "lab RDOQ requires run-level U8 stills without AQ, tiers or LF prediction",
            ));
        }
        if let Some(t) = &cfg.trellis
            && !t.valid()
        {
            return Err(Error::InvalidInput("invalid lab trellis settings"));
        }
        if cfg.lab.recon_bias.is_some() && cfg.lab.recon_bias_centroid.is_some() {
            return Err(Error::InvalidInput(
                "fixed and centroid reconstruction bias exclude each other",
            ));
        }
        if let Some(k) = cfg.lab.recon_bias_centroid
            && !(k == 64 || (2..=63).contains(&k))
        {
            return Err(Error::InvalidInput("invalid reconstruction bias band"));
        }
    }
    // Centralized lossy effort policy (effort.rs).
    let plan = effort.lossy_plan(adaptive_mode(cfg));
    // The AC trellis: (settings, set by the lab override). The effort's
    // trellis is narrowed to run-level XYB coding after pass 1. Subblock
    // layouts and the lab RDOQ / LF prediction modes encode without it.
    let trellis = {
        #[cfg(feature = "lab")]
        let set = match cfg.trellis {
            // min_quality is compared with the resolved quantizer scale below
            // (a quality target leaves `cfg.quality` as it was).
            Some(t) => (t.passes > 0).then_some((t, true)),
            None => trellis::for_effort_plan(plan).map(|t| (t, false)),
        };
        #[cfg(not(feature = "lab"))]
        let set = trellis::for_effort_plan(plan).map(|t| (t, false));
        #[cfg(feature = "lab")]
        let set = set.filter(|_| cfg.rdoq_lambda == 0.0 && cfg.lf_pred == 0);
        set.filter(|_| cfg.channel_layouts == [LossyChannelLayout::default(); 3])
    };
    if cfg.small_transforms
        && (cfg.decode_speed != Some(crate::DecodeSpeed::Compact)
            || info.sample != SampleType::U8
            || previous.is_some()
            || cfg.channel_layouts != [LossyChannelLayout::default(); 3]
            || cfg.effort >= 9.0
            || trellis.is_some())
    {
        return Err(Error::Unsupported(
            "small transforms require Compact U8 stills with default layouts, effort below 9 and no trellis",
        ));
    }
    #[cfg(feature = "lab")]
    if cfg.small_transforms && (cfg.rdoq_lambda != 0.0 || cfg.lf_pred != 0) {
        return Err(Error::Unsupported(
            "production small transforms exclude lab RDOQ and LF prediction",
        ));
    }
    let layouts = cfg.channel_layouts;
    let adaptive_mask = layout::adaptive_mask(&layouts);
    #[cfg(feature = "lab")]
    if layouts != [LossyChannelLayout::default(); 3] && (cfg.lf_pred != 0 || cfg.rdoq_lambda != 0.0)
    {
        return Err(Error::Unsupported(
            "subblocks with lab LF prediction or RDOQ",
        ));
    }
    #[cfg(feature = "lab")]
    if cfg.lab.recon_bias_centroid.is_some() && layouts != [LossyChannelLayout::default(); 3] {
        return Err(Error::Unsupported(
            "the centroid reconstruction bias needs full 8x8 layouts",
        ));
    }
    let previous = previous.filter(|_| info.channels == Channels::Rgb);
    let skip_on = previous.is_some();
    let kind = InputKind::of(info).ok_or(Error::Unsupported(
        "lossy mode supports u8, u16, u32, f16 or f32 samples",
    ))?;
    if kind == InputKind::U32 && !cfg!(feature = "extended") {
        return Err(Error::Unsupported("32-bit samples (extended tier)"));
    }
    let nci = info.channels.count();
    let bps = info.sample.bytes();
    let ci = color_index(info.channels);
    // 16-bit input is coded as YCoCg on the code values (any transfer:
    // sRGB, PQ, HLG), in units of 1/257 so quality maps like 8-bit. Float
    // input is linear light and coded as XYB.
    // 16-bit XYB needs a known transfer (sRGB, PQ, HLG) to reach linear light.
    let xfer16 = if kind == InputKind::U16 && cfg.color == LossyColor::Xyb {
        transfer::Transfer::from_cicp(info.cicp.transfer_characteristics)
    } else {
        None
    };
    if precision::custom_color(info)
        && cfg.color == LossyColor::Xyb
        && transfer::Transfer::from_cicp(info.cicp.transfer_characteristics).is_none()
    {
        return Err(Error::Unsupported(
            "unknown transfer for significant-bit XYB",
        ));
    }
    let color = match kind {
        InputKind::U8 => cfg.color,
        InputKind::U32 => cfg.color,
        InputKind::U16 if xfer16.is_some() => LossyColor::Xyb,
        InputKind::U16 => LossyColor::YCoCg,
        InputKind::F16 | InputKind::F32 => LossyColor::Xyb,
    };
    let wide = kind == InputKind::U16 && color == LossyColor::YCoCg;
    // Code value -> linear light for 16-bit XYB.
    let custom_color = precision::custom_color(info);
    let lut16: Vec<f32> = match xfer16.filter(|_| !custom_color) {
        Some(t) => (0..65536)
            .map(|c| t.decode(c as f64 / 65535.0) as f32)
            .collect(),
        None => Vec::new(),
    };
    if info.width == 0 || info.height == 0 {
        return Err(Error::InvalidInput("empty image"));
    }
    let extent = info
        .checked_stride_len(stride)
        .ok_or(Error::InvalidInput("invalid image layout"))?;
    if pixels.len() < extent {
        return Err(Error::InvalidInput("pixel buffer too small"));
    }
    precision::validate_pixels(info, pixels, stride)?;
    let input_precision = if custom_color || kind == InputKind::U32 {
        Some(precision::Input::new(info, color)?)
    } else {
        None
    };
    let rb = info.row_bytes();
    let sb = cfg.stripe_rows_for(info.width);
    let g = Geom::new(info.width, info.height, sb);
    let map = aq_map(cfg);
    if let Some(m) = map
        && (cfg.aq_gamma != 0.0 || m.len() != g.bw * g.bh || m.contains(&0))
    {
        return Err(Error::InvalidInput(
            "AQ map needs one nonzero code per block and no aq_gamma",
        ));
    }
    #[cfg(feature = "lab")]
    if let Some(f) = cfg.aq_field
        && (cfg.aq_gamma != 0.0
            || map.is_some()
            || !(f.strength.is_finite() && (0.0..=4.0).contains(&f.strength))
            || f.levels > 16
            || !(1..=8).contains(&f.cell))
    {
        return Err(Error::InvalidInput("invalid lab AQ field settings"));
    }
    let quantizer_scale = cfg.scale_for(color, info.width, info.height);
    if !quantizer_scale.is_finite() {
        return Err(Error::InvalidInput("invalid lossy numeric parameter"));
    }
    // jpegli's XYB shapes beat Annex K on SSIMULACRA2, zensim and both
    // butteraugli norms (benchmarks/lossy_matrix_2026-10-02.txt). The
    // automatic choice is matrix 5 for YCoCg (maintainer decision
    // 2026-10-10: jpegli's distance-dependent tables) and matrix 2 for XYB;
    // the Lossy 0.4 presets 3/4/5 and the explicit id 63 are selectable
    // (lab builds add the tablemath study's probes 6-9, which did not
    // qualify and are never automatic).
    let matrix = match cfg.matrix {
        m @ 0..=MATRIX_LAST_PRESET => m,
        MATRIX_EXPLICIT => MATRIX_EXPLICIT,
        _ if color == LossyColor::Xyb => 2,
        _ => {
            // Matrix 5's coarsest chroma steps exceed the format's 65536
            // bound at quality at or below 1.52 (scale >= 32.8) with the
            // YCoCg multiplier 2; fall back to Annex K there.
            if QParams::from_quality(quantizer_scale, color, 5).valid() {
                5
            } else {
                0
            }
        }
    };
    // Both effort-selected and explicit lab trellis gates use the resolved scale.
    let trellis = trellis.filter(|(t, _)| quantizer_scale >= t.min_quality);
    let mut qp = QParams::from_quality(quantizer_scale, color, matrix);
    if cfg.color_transform == Some(ColorTransform::Identity) {
        qp.mul = [1.0, 1.0, 1.0];
    }
    if let Some(m) = cfg.channel_mul {
        qp.mul = m;
    }
    qp.transform = cfg.color_transform;
    #[cfg(feature = "lab")]
    {
        if let Some(t) = &cfg.lab.explicit_matrix {
            qp.explicit = Some(envelope::round_trip_explicit(**t)?);
            qp.matrix = MATRIX_EXPLICIT;
        }
        qp.bias = cfg.lab.recon_bias.filter(|b| !b.is_zero());
        if qp.transform.is_none() {
            qp.transform = cfg.lab.color_transform;
        }
    }
    if let Some(t) = qp.transform {
        // The signalled transform is defined for the U8 YCoCg family;
        // XYB is its own transform and other sample types keep their
        // paths.
        if color != LossyColor::YCoCg || kind != InputKind::U8 || input_precision.is_some() {
            return Err(Error::Unsupported(
                "the signalled color transform needs U8 YCoCg input",
            ));
        }
        if let ColorTransform::Explicit(m) = t
            && (!ColorTransform::explicit_valid(&m) || t.forward().is_none())
        {
            return Err(Error::InvalidInput("invalid explicit color transform"));
        }
    }
    #[cfg(feature = "lab")]
    for (i, v) in lab_dc_step(cfg).into_iter().enumerate() {
        if v != 0 {
            for c in if i == 0 { 0..1 } else { 1..3 } {
                qp.dc[c] = v;
            }
        }
    }
    qp.apply_ac_scale(cfg.ac_scale)?;
    if !qp.valid() {
        return Err(Error::InvalidInput("invalid quantization parameters"));
    }
    let steps = qp.steps();
    // Lossy 0.4: the encoder's forward of a signalled color transform
    // (`coded = A * rgb + B`), applied with the plane scaling by
    // `enc_simd::color_rows`.
    let transform_forward = qp.transform.and_then(|t| t.forward());
    let scale = if wide {
        [1.0 / 257.0; 3]
    } else {
        color.default_scales()
    };
    let layout_steps: [[[_; 64]; 2]; 3] = core::array::from_fn(|c| {
        [false, true].map(|split| {
            let l = layouts[c].layout(split);
            if l == subblock::Layout::FULL8 {
                steps[c]
            } else {
                l.steps(&steps[c])
            }
        })
    });
    let cm = dct::dct_matrix();
    let base_round = cfg.round.clamp(0.0, 0.5);
    let round = if cfg.freq_zero_bias {
        -base_round.max(0.40)
    } else {
        base_round
    };

    // Pass 1: transform + quantize every stripe, one block row at a time:
    // fill planar rows (linear light for XYB, code values for YCoCg), color
    // convert, then DCT + quantize.
    let inv_steps: [[f32; 64]; 3] = steps.map(|st| st.map(|v| 1.0 / v));
    let xyb = color == LossyColor::Xyb;
    let u8_lin: [f32; 256] = core::array::from_fn(|i| color::SRGB8_TO_LINEAR[i] as f32);
    let pw = g.bw * 8;
    // Overshoot deringing on the luma plane (dering.rs): integer outputs
    // only (the decoder clamps them), full 8x8 luma layout. `dering_white`
    // holds each color channel's maximum code; the cap is mozjpeg's: twice
    // the DC step (our DC is 8x the block mean, the scale of a JPEG
    // quantizer), and at most 31/255 of the plane's white level.
    let full8_luma = layouts[0] == LossyChannelLayout::default();
    let dering_on = dering::enabled(cfg) && full8_luma;
    let dc_fix = dc_nearest(cfg);
    // Maximum code per color channel, for integer inputs (the decoder
    // clamps those): white and black blocks for deringing and DC rounding.
    let white_code: Option<[u32; 3]> =
        (matches!(kind, InputKind::U8 | InputKind::U16) && full8_luma && (dering_on || dc_fix))
            .then(|| core::array::from_fn(|c| (1u32 << info.channel_bits(ci[c]).unwrap()) - 1));
    let dering_cap = 2.0 * steps[0][0];
    // LAB: jpegli's AQ strengths for the trellis's AQ coupling (U8 input).
    #[cfg(not(feature = "lab"))]
    let trellis_aq: Vec<f32> = Vec::new();
    // LAB: jpegli's field from the luma plane (U8 input), for the trellis's
    // AQ coupling strengths and the lab AQ field map.
    #[cfg(feature = "lab")]
    let luma_rows = |by: usize, out: &mut [f32]| {
        let mut planes = vec![0.0f32; NCH * 8 * pw];
        for y in 0..8 {
            let row = &pixels[(by * 8 + y).min(g.h - 1) * stride..][..rb];
            if xyb {
                fill_color_row::<1>(row, &mut planes, pw, y, g.w, nci, ci, |_, b| {
                    u8_lin[b[0] as usize]
                });
            } else {
                fill_color_row::<1>(row, &mut planes, pw, y, g.w, nci, ci, |_, b| b[0] as f32);
            }
        }
        let (p0, rest) = planes.split_at_mut(8 * pw);
        let (p1, p2) = rest.split_at_mut(8 * pw);
        match transform_forward.as_ref() {
            Some(t) => enc_simd::color_rows_transform(
                xyb,
                scale,
                &color::OPSIN_F32,
                color::OPSIN_BIAS,
                color::CBRT_BIAS,
                Some(t),
                p0,
                p1,
                p2,
            ),
            None => enc_simd::color_rows(
                xyb,
                scale,
                &color::OPSIN_F32,
                color::OPSIN_BIAS,
                color::CBRT_BIAS,
                p0,
                p1,
                p2,
            ),
        }
        out.copy_from_slice(&planes[..8 * pw]);
    };
    #[cfg(feature = "lab")]
    let coupling = matches!(&trellis, Some((t, _)) if t.aq_coupling != 0.0);
    #[cfg(feature = "lab")]
    if (coupling || cfg.aq_field.is_some()) && (kind != InputKind::U8 || input_precision.is_some())
    {
        return Err(Error::Unsupported(
            "lab trellis AQ coupling and AQ field need U8 input",
        ));
    }
    #[cfg(feature = "lab")]
    let trellis_aq: Vec<f32> = if coupling {
        trellis::aq_strengths(g.bw, g.bh, xyb, steps[0][1], cfg.threads, luma_rows)
    } else {
        Vec::new()
    };
    #[cfg(feature = "lab")]
    let field_codes: Vec<u8> = match cfg.aq_field {
        Some(f) => {
            let units = aq_field_lab::luma_units(xyb);
            let qf = aq_field_lab::field(
                g.bw,
                g.bh,
                steps[0][1] * units,
                f.fade,
                cfg.threads,
                |by, out| {
                    luma_rows(by, out);
                    for v in out.iter_mut() {
                        *v *= units;
                    }
                },
            );
            aq_field_lab::step_codes(
                &qf,
                g.bw,
                g.bh,
                f.strength,
                f.levels,
                f.cell as usize,
                f.relative,
            )
        }
        None => Vec::new(),
    };
    #[cfg(feature = "lab")]
    let map = if field_codes.is_empty() {
        map
    } else {
        Some(&field_codes[..])
    };
    // Planar color for the 8 pixel rows of block row `by` (linear light for
    // XYB, code values for YCoCg; rows past the bottom repeat the last one),
    // color converted, with overshoot deringing on the luma plane;
    // `extremes` gets the row's white/black blocks (integer inputs). Shared
    // by the quantization pass and the trellis, which recomputes rows.
    let fill_block_row = |by: usize, planes: &mut [f32], extremes: &mut Vec<(u64, bool)>| {
        for y in 0..8 {
            let sy = (by * 8 + y).min(g.h - 1);
            let row = &pixels[sy * stride..][..rb];
            match kind {
                InputKind::U8 if input_precision.is_some() => {
                    let input = input_precision.as_ref().unwrap();
                    fill_color_row::<1>(row, planes, pw, y, g.w, nci, ci, |c, b| {
                        input.read(c, b[0] as u32)
                    });
                }
                InputKind::U16 if input_precision.is_some() => {
                    let input = input_precision.as_ref().unwrap();
                    fill_color_row::<2>(row, planes, pw, y, g.w, nci, ci, |c, b| {
                        input.read(c, u16::from_ne_bytes(*b) as u32)
                    });
                }
                InputKind::U8 if xyb => {
                    fill_color_row::<1>(row, planes, pw, y, g.w, nci, ci, |_, b| {
                        u8_lin[b[0] as usize]
                    })
                }
                InputKind::U8 => {
                    fill_color_row::<1>(row, planes, pw, y, g.w, nci, ci, |_, b| b[0] as f32)
                }
                InputKind::U16 if !lut16.is_empty() => {
                    fill_color_row::<2>(row, planes, pw, y, g.w, nci, ci, |_, b| {
                        lut16[u16::from_ne_bytes(*b) as usize]
                    })
                }
                InputKind::U16 => fill_color_row::<2>(row, planes, pw, y, g.w, nci, ci, |_, b| {
                    u16::from_ne_bytes(*b) as f32
                }),
                InputKind::U32 => {
                    let input = input_precision.as_ref().unwrap();
                    fill_color_row::<4>(row, planes, pw, y, g.w, nci, ci, |c, b| {
                        input.read(c, u32::from_ne_bytes(*b))
                    })
                }
                InputKind::F16 => fill_color_row::<2>(row, planes, pw, y, g.w, nci, ci, |_, b| {
                    finite(f32::from_bits(crate::mapping::f16_to_f32_bits(
                        u16::from_ne_bytes(*b),
                    )))
                }),
                InputKind::F32 => fill_color_row::<4>(row, planes, pw, y, g.w, nci, ci, |_, b| {
                    finite(f32::from_ne_bytes(*b))
                }),
            }
        }
        let (p0, rest) = planes.split_at_mut(8 * pw);
        let (p1, p2) = rest.split_at_mut(8 * pw);
        match transform_forward.as_ref() {
            Some(t) => enc_simd::color_rows_transform(
                xyb,
                scale,
                &color::OPSIN_F32,
                color::OPSIN_BIAS,
                color::CBRT_BIAS,
                Some(t),
                p0,
                p1,
                p2,
            ),
            None => enc_simd::color_rows(
                xyb,
                scale,
                &color::OPSIN_F32,
                color::OPSIN_BIAS,
                color::CBRT_BIAS,
                p0,
                p1,
                p2,
            ),
        }
        if let Some(white_code) = white_code {
            block_extremes(
                &g,
                pixels,
                stride,
                by * 8,
                nci,
                ci,
                kind,
                &white_code,
                extremes,
            );
            if dering_on {
                dering_row(p0, pw, extremes, dering_cap);
            }
        }
    };
    // LAB: per-image centroid reconstruction bias (Lossy 0.4). The stats
    // ride along the pass-1 quantization, one accumulator per stripe.
    #[cfg(feature = "lab")]
    let centroid_k = cfg.lab.recon_bias_centroid;
    #[cfg(not(feature = "lab"))]
    let centroid_k: Option<u8> = None;
    Stages::new(
        pulse,
        &[
            PhaseSpec::new("transform rows", 1, Total::Unknown).units("block rows"),
            PhaseSpec::new("block refinement", 1, Total::Unknown),
            PhaseSpec::new("coefficient coding", 1, Total::Unknown).units("encoded units"),
        ],
    )
    .complete_with(|stages| {
        let stripe_parts: Vec<(StripeCoefs, BiasSums)> = stages.run(|pulse| {
            let pulse = pulse.live();
            use how_far::Stop;
            crate::par::par_map(
                (0..g.stripes()).collect(),
                cfg.threads,
                |_, s| -> Result<_, Error> {
                    pulse.check().map_err(|_| Error::Cancelled)?;
                    let (by0, by1) = g.rows(s);
                    let nby = by1 - by0;
                    let mut q = vec![0i32; nby * g.bw * NCH * 64];
                    let mut nz = vec![0u64; nby * g.bw * NCH];
                    let mut modes = vec![0u8; if adaptive_mask != 0 { nby * g.bw } else { 0 }];
                    let mut planes = vec![0.0f32; NCH * 8 * pw];
                    let mut extremes: Vec<(u64, bool)> = Vec::with_capacity(g.bw);
                    let mut coefs = vec![0.0f32; g.bw * 64];
                    let mut bias_stats = BiasSums::default();
                    let activity_on = cfg.aq_gamma != 0.0;
                    let aq_on = activity_on || map.is_some();
                    // Block codes (DC image channel 3): AQ multiplier a / 64, or 0 =
                    // keep the reference block (inter frames).
                    let codes_on = aq_on || skip_on;
                    let mut aq_codes = vec![64u8; if codes_on { nby * g.bw } else { 0 }];
                    let mut aq_row = vec![1.0f32; if aq_on { g.bw } else { 0 }];
                    for r in 0..nby {
                        pulse.check().map_err(|_| Error::Cancelled)?;
                        if let Some(m) = map {
                            let codes = &m[(by0 + r) * g.bw..][..g.bw];
                            aq_codes[r * g.bw..][..g.bw].copy_from_slice(codes);
                            for (s, &a) in aq_row.iter_mut().zip(codes) {
                                *s = a as f32 * (1.0 / 64.0);
                            }
                        }
                        fill_block_row(by0 + r, &mut planes, &mut extremes);
                        let qr = &mut q[r * g.bw * NCH * 64..][..g.bw * NCH * 64];
                        let nr = &mut nz[r * g.bw * NCH..][..g.bw * NCH];
                        for ch in 0..NCH {
                            if layouts[ch].layout(false) == subblock::Layout::FULL8
                                || (activity_on && ch == 0)
                            {
                                enc_simd::fdct_row(
                                    &planes[ch * 8 * pw..][..8 * pw],
                                    pw,
                                    g.bw,
                                    &cm,
                                    &mut coefs,
                                );
                            }
                            if activity_on && ch == 0 {
                                // Activity: mean |AC| of luma relative to the first AC step.
                                let r0 = cfg.aq_r0 * steps[0][1];
                                for bx in 0..g.bw {
                                    let c = &coefs[bx * 64..][..64];
                                    let act = c[1..].iter().map(|v| v.abs()).sum::<f32>() / 63.0;
                                    let m = transfer::pow(
                                        (act / r0).max(1e-6) as f64,
                                        cfg.aq_gamma as f64,
                                    )
                                    .clamp(0.5, 2.0);
                                    let a = (m * 64.0 + 0.5) as u8; // m in [0.5, 2]: round half up
                                    aq_codes[r * g.bw + bx] = a;
                                    aq_row[bx] = a as f32 * (1.0 / 64.0);
                                }
                            }
                            if layouts[ch] == LossyChannelLayout::default() {
                                enc_simd::quant_row(
                                    &coefs,
                                    g.bw,
                                    ch,
                                    &inv_steps[ch],
                                    &aq_row,
                                    round,
                                    qr,
                                    nr,
                                );
                                if let Some(k) = centroid_k {
                                    bias_stats.add_row(
                                        &coefs,
                                        qr,
                                        g.bw,
                                        ch,
                                        &inv_steps[ch],
                                        &aq_row,
                                        k,
                                    );
                                }
                                if dc_fix {
                                    let ext =
                                        (ch == 0 && white_code.is_some()).then_some(&extremes[..]);
                                    dc_row(&coefs, g.bw, ch, inv_steps[ch][0], ext, qr, nr);
                                }
                            } else {
                                let mr = if modes.is_empty() {
                                    &mut [][..]
                                } else {
                                    &mut modes[r * g.bw..(r + 1) * g.bw]
                                };
                                layout::encode_row(
                                    &planes[ch * 8 * pw..][..8 * pw],
                                    pw,
                                    g.bw,
                                    ch,
                                    layouts[ch],
                                    &layout_steps[ch],
                                    steps[ch][1],
                                    &coefs,
                                    &aq_row,
                                    round,
                                    if dc_fix { 0.5 } else { round },
                                    qr,
                                    nr,
                                    mr,
                                );
                            }
                        }
                        // Inter frames: blocks whose pixels equal the previous frame's
                        // keep the decoder's canvas (coded "keep"; the DC stays for a
                        // smooth DC image, the AC goes).
                        if let Some((pp, ps)) = previous {
                            let y0 = (by0 + r) * 8;
                            let y1 = (y0 + 8).min(g.h);
                            for bx in 0..g.bw {
                                let (x0, x1) =
                                    (bx * 8 * nci * bps, ((bx * 8 + 8).min(g.w)) * nci * bps);
                                let same = (y0..y1).all(|y| {
                                    pixels[y * stride + x0..y * stride + x1]
                                        == pp[y * ps + x0..y * ps + x1]
                                });
                                if same {
                                    for ch in 0..NCH {
                                        let o = (bx * NCH + ch) * 64;
                                        qr[o + 1..o + 64].fill(0);
                                        nr[bx * NCH + ch] = (qr[o] != 0) as u64;
                                    }
                                    aq_codes[r * g.bw + bx] = 0;
                                }
                            }
                        }
                        pulse.advance(1);
                        pulse.check().map_err(|_| Error::Cancelled)?;
                    }
                    Ok((
                        StripeCoefs {
                            by0,
                            nby,
                            bw: g.bw,
                            q,
                            nz,
                            aq: aq_codes,
                            modes,
                            splits: Vec::new(),
                        },
                        bias_stats,
                    ))
                },
            )
            .into_iter()
            .collect::<Result<Vec<_>, Error>>()
        })?;
        let (stripes, exact, sb_on, open_lambda_map) = stages.run(|pulse| {
            pulse.check().map_err(|_| Error::Cancelled)?;
            let (stripes, bias_stats): (Vec<StripeCoefs>, Vec<BiasSums>) =
                stripe_parts.into_iter().unzip();
            #[cfg(feature = "lab")]
            if let Some(k) = centroid_k {
                let b = bias_stats.iter().fold(BiasSums::default(), |mut a, s| {
                    for c in 0..3 {
                        for band in 0..2 {
                            for g in 0..2 {
                                a.sum[c][band][g] += s.sum[c][band][g];
                                a.count[c][band][g] += s.count[c][band][g];
                            }
                        }
                    }
                    a
                });
                let b = b.bias(k);
                qp.bias = (!b.is_zero()).then_some(b);
            }
            #[cfg(not(feature = "lab"))]
            {
                let _ = bias_stats;
            }

            #[cfg(feature = "lab")]
            let stripes = {
                let mut stripes = stripes;
                #[cfg(feature = "std")]
                block_lab::zero_masked_ac(&mut stripes);
                if cfg.lf_pred != 0 && !aq_on_cfg(cfg) {
                    lf_pred_lab(
                        &mut stripes,
                        &g,
                        &steps,
                        cfg.lf_pred,
                        (sb * (8 / sb).max(1)) as usize,
                    );
                }
                stripes
            };
            // Exact blocks (exact.rs): blocks whose estimated lossless bits are at
            // most the policy's ratio of their run-level AC bits (within its
            // decode-cost budget) drop their AC and are coded exactly in a nested
            // lossless image. Without a qualifying block nothing changes.
            let (stripes, exact) = {
                let mut stripes = stripes;
                let eligible =
                    !aq_on_cfg(cfg) && !skip_on && layouts == [LossyChannelLayout::default(); 3];
                let exact = match exact::policy(cfg, info, eligible) {
                    Some(pol) => {
                        let bits = exact::estimate_bits(&stripes, cfg.threads);
                        let mask = exact::select_mask(
                            pixels,
                            stride,
                            nci * bps,
                            g.w,
                            g.h,
                            &bits,
                            &pol,
                            cfg.threads,
                        );
                        if mask.contains(&true) {
                            exact::mark(&mut stripes, &mask);
                            Some(exact::encode(
                                pixels,
                                stride,
                                nci * bps,
                                &g,
                                lossy_group_cols(cfg, g.w),
                                &mask,
                                pol.stripe_rows,
                                cfg.effort,
                                cfg.threads,
                            )?)
                        } else {
                            None
                        }
                    }
                    None => None,
                };
                (stripes, exact)
            };
            pulse.check().map_err(|_| Error::Cancelled)?;
            // Superblocks: merge aligned 8x8 parents into 16x16, 16x8 and 8x16 DCTs
            // where the file's own coder prices it cheaper (before the DC image,
            // which carries the merged blocks' lowest coefficients). Exact parents
            // take block code 0 for the selection (never merged) and get their
            // codes back after it.
            let sb_eligible = (trellis.is_none() || adaptive_enabled(cfg))
                && !skip_on
                && layouts == [LossyChannelLayout::default(); 3]
                && (cfg.decode_speed.is_some()
                    || matches!(cfg.entropy, LossyEntropy::RunLevel | LossyEntropy::SplitHuf));
            #[cfg(feature = "lab")]
            let sb_eligible = sb_eligible && cfg.rdoq_lambda == 0.0 && cfg.lf_pred == 0;
            let sb_on = superblocks_on(cfg, info, sb_eligible);
            // Small transforms: as superblocks, but the trellis runs on 4x4 planes
            // too (it skips merged parents).
            let small_eligible = !skip_on
                && layouts == [LossyChannelLayout::default(); 3]
                && (cfg.decode_speed.is_some()
                    || matches!(cfg.entropy, LossyEntropy::RunLevel | LossyEntropy::SplitHuf));
            #[cfg(feature = "lab")]
            let small_eligible = small_eligible && cfg.rdoq_lambda == 0.0 && cfg.lf_pred == 0;
            let small_on = small_on(cfg, info, effort, small_eligible);
            let trellis_active = trellis.as_ref().is_some_and(|&(_, forced)| forced || xyb);
            let open_lambda_map: Vec<f32> =
                if plan.lambda_aq_open != 0.0 && (sb_on || small_on || trellis_active) {
                    adaptive_rdo::build_open_loop_lambda_map(
                        &fill_block_row,
                        pw,
                        g.bw,
                        g.bh,
                        &steps,
                        plan.lambda_aq_open,
                        cfg.threads,
                        pulse,
                    )?
                } else {
                    Vec::new()
                };
            let stripes = if sb_on || small_on {
                let mut stripes = stripes;
                if exact.is_some() {
                    for sc in &mut stripes {
                        for code in &mut sc.aq {
                            *code = if *code == 0 { 64 } else { 0 };
                        }
                    }
                }
                let split = cfg.entropy == LossyEntropy::SplitHuf
                    || matches!(
                        cfg.decode_speed,
                        Some(crate::DecodeSpeed::Fast | crate::DecodeSpeed::Fastest)
                    );
                #[cfg(feature = "lab")]
                let (shapes, force) = (
                    cfg.lab.superblock_shapes.unwrap_or(7),
                    cfg.lab.small.as_ref().and_then(|s| s.split),
                );
                #[cfg(not(feature = "lab"))]
                let (shapes, force) = (7, None);
                let sel_opts = superblock::select::Opts {
                    merges: sb_on,
                    // The trellis runs on XYB files (or where the lab forces
                    // it); it is narrowed to them after this selection.
                    trellis: trellis_active,
                    shapes,
                    small: small_on.then_some(force),
                    split_huf: split,
                    forward: {
                        #[cfg(feature = "lab")]
                        let f: fn(&[f32; 64]) -> [f32; 64] =
                            if cfg.lab.small.as_ref().is_some_and(|s| s.int) {
                                small_lab::forward_int
                            } else {
                                small::forward
                            };
                        #[cfg(not(feature = "lab"))]
                        let f: fn(&[f32; 64]) -> [f32; 64] = small::forward;
                        f
                    },
                    kappa: {
                        let base_k = plan.small_kappa;
                        #[cfg(feature = "lab")]
                        let k = cfg
                            .lab
                            .small
                            .as_ref()
                            .and_then(|s| s.lambda)
                            .unwrap_or(base_k);
                        #[cfg(not(feature = "lab"))]
                        let k = base_k;
                        k
                    },
                    edge: {
                        #[cfg(feature = "lab")]
                        let e = cfg
                            .lab
                            .small
                            .as_ref()
                            .and_then(|s| s.edge)
                            .unwrap_or(SMALL_EDGE);
                        #[cfg(not(feature = "lab"))]
                        let e = SMALL_EDGE;
                        e
                    },
                    rdo: plan,
                };
                let gbw = lossy_group_cols(cfg, g.w) / 8;
                let mut stripes = if plan.two_pass_pilot {
                    // Two-pass empirical pilot without cloning the whole image's
                    // `StripeCoefs`: keep clean pass-1 `stripes` untouched, run
                    // pilot selection one stripe at a time per worker to gather
                    // fixed-size empirical map/flag/token histograms (~25 KiB),
                    // then run final selection directly in place on `stripes`.
                    let mut pilot_opts = sel_opts;
                    pilot_opts.rdo.trellis_select = false;
                    let pilot_coder = superblock::select::Coder::of_stripes(
                        &stripes,
                        gbw,
                        cfg,
                        superblock::select::price_effort(effort, pilot_opts),
                        split,
                    );
                    let pilot_sel = superblock::select::Selector::new_learned(
                        pilot_coder,
                        pw,
                        gbw,
                        &steps,
                        round,
                        pilot_opts,
                        None,
                        None,
                    );
                    let rl_len = LossyEntropy::RunLevel.nraw() * coef::ALPHA;
                    let pilot_huf_model = split && trellis::huf_rate(cfg);
                    let want_pilot_huf = if trellis_active {
                        pilot_huf_model
                    } else {
                        split
                    };

                    struct PilotA {
                        map: adaptive_rdo::MapBitStats,
                        flags: adaptive_rdo::FlagStats,
                        rl: Vec<u64>,
                        freqs: trellis::Freqs,
                    }
                    let parts_a = crate::par::par_fold(
                        &stripes,
                        cfg.threads,
                        || {
                            Ok::<PilotA, Error>(PilotA {
                                map: adaptive_rdo::MapBitStats::default(),
                                flags: adaptive_rdo::FlagStats::default(),
                                rl: if want_pilot_huf {
                                    Vec::new()
                                } else {
                                    vec![0u64; rl_len]
                                },
                                freqs: if want_pilot_huf {
                                    trellis::freqs_new()
                                } else {
                                    [Vec::new(), Vec::new(), Vec::new()]
                                },
                            })
                        },
                        |acc, sc| {
                            let Ok(st) = acc else { return };
                            let selected = match pilot_sel.stripe_with_lambda(
                                sc.clone(),
                                &fill_block_row,
                                &open_lambda_map,
                                0,
                                pulse,
                            ) {
                                Ok(s) => s,
                                Err(e) => {
                                    *acc = Err(e);
                                    return;
                                }
                            };
                            if let Err(e) = st.map.add_stripe(&selected, gbw) {
                                *acc = Err(e);
                                return;
                            }
                            if small_on {
                                st.flags.add_stripe(&selected);
                            }
                            let mut tally = |u: &StripeCoefs| {
                                if want_pilot_huf {
                                    let (counts, widths) = stripe_counts_widths(u, u.bw);
                                    let syms = huf::stripe_syms(&counts, &widths, u.nby * u.bw);
                                    let [luma, chroma, wid] = &mut st.freqs;
                                    for (h, symbols) in [
                                        (luma, &syms.luma),
                                        (chroma, &syms.chroma),
                                        (wid, &syms.wid),
                                    ] {
                                        for &(ctx, sym) in symbols {
                                            h[ctx as usize][sym as usize] += 1;
                                        }
                                    }
                                } else {
                                    stripe_tokens(u, u.bw, Group::ALL, |_, ctx, tok, _, _| {
                                        st.rl[ctx * coef::ALPHA + tok as usize] += 1;
                                    });
                                }
                            };
                            if gbw == 0 {
                                tally(&selected);
                            } else {
                                for u in split_units(selected, gbw) {
                                    tally(&u);
                                }
                            }
                        },
                    );
                    let mut map_stats = adaptive_rdo::MapBitStats::default();
                    let mut flag_stats = adaptive_rdo::FlagStats::default();
                    let mut pilot_rl = if want_pilot_huf {
                        Vec::new()
                    } else {
                        vec![0u64; rl_len]
                    };
                    let mut pilot_freqs = if want_pilot_huf {
                        trellis::freqs_new()
                    } else {
                        [Vec::new(), Vec::new(), Vec::new()]
                    };
                    for p in parts_a {
                        let p = p?;
                        map_stats.merge(&p.map);
                        flag_stats.merge(&p.flags);
                        if want_pilot_huf {
                            for (dst, src) in pilot_freqs.iter_mut().zip(p.freqs) {
                                for (drow, srow) in dst.iter_mut().zip(src) {
                                    for (d, s) in drow.iter_mut().zip(srow) {
                                        *d += s;
                                    }
                                }
                            }
                        } else {
                            for (d, s) in pilot_rl.iter_mut().zip(p.rl) {
                                *d += s;
                            }
                        }
                    }
                    let learned_map = map_stats.into_table();
                    let learned_flags = small_on.then(|| flag_stats.into_flags());

                    let learned_coder = if let Some((mut t_set, forced)) = trellis
                        && (forced || xyb)
                    {
                        t_set.passes = 1;
                        let pilot_bias = qp.bias.filter(|b| !b.is_zero()).map(|b| b.tables());
                        let pilot_pass = if pilot_huf_model {
                            trellis::Pass::split_huf(&pilot_freqs, &t_set, cfg, effort)
                        } else {
                            trellis::Pass::run_level(&pilot_rl, &t_set, cfg, effort)
                        };
                        let parts_b = crate::par::par_fold(
                            &stripes,
                            cfg.threads,
                            || {
                                Ok::<(Vec<u64>, trellis::Freqs), Error>((
                                    if split {
                                        Vec::new()
                                    } else {
                                        vec![0u64; rl_len]
                                    },
                                    if split {
                                        trellis::freqs_new()
                                    } else {
                                        [Vec::new(), Vec::new(), Vec::new()]
                                    },
                                ))
                            },
                            |acc, sc| {
                                let Ok((rl_acc, fr_acc)) = acc else { return };
                                let selected = match pilot_sel.stripe_with_lambda(
                                    sc.clone(),
                                    &fill_block_row,
                                    &open_lambda_map,
                                    0,
                                    pulse,
                                ) {
                                    Ok(s) => s,
                                    Err(e) => {
                                        *acc = Err(e);
                                        return;
                                    }
                                };
                                let mut units = if gbw == 0 {
                                    alloc::vec![selected]
                                } else {
                                    split_units(selected, gbw)
                                };
                                if let Err(e) = trellis::rewrite_stripe_lambda(
                                    &pilot_pass,
                                    &mut units,
                                    &fill_block_row,
                                    pw,
                                    g.bw,
                                    gbw,
                                    &steps,
                                    cfg,
                                    &t_set,
                                    &trellis_aq,
                                    &open_lambda_map,
                                    0,
                                    plan.merged_trellis,
                                    pilot_bias.as_ref(),
                                    0,
                                    None,
                                    pulse,
                                ) {
                                    *acc = Err(e);
                                    return;
                                }
                                for u in &units {
                                    if split {
                                        let (counts, widths) = stripe_counts_widths(u, u.bw);
                                        let syms = huf::stripe_syms(&counts, &widths, u.nby * u.bw);
                                        let [luma, chroma, wid] = fr_acc;
                                        for (h, symbols) in [
                                            (luma, &syms.luma),
                                            (chroma, &syms.chroma),
                                            (wid, &syms.wid),
                                        ] {
                                            for &(ctx, sym) in symbols {
                                                h[ctx as usize][sym as usize] += 1;
                                            }
                                        }
                                    } else {
                                        stripe_tokens(u, u.bw, Group::ALL, |_, ctx, tok, _, _| {
                                            rl_acc[ctx * coef::ALPHA + tok as usize] += 1;
                                        });
                                    }
                                }
                            },
                        );
                        let mut learned_rl = if split {
                            Vec::new()
                        } else {
                            vec![0u64; rl_len]
                        };
                        let mut learned_freqs = if split {
                            trellis::freqs_new()
                        } else {
                            [Vec::new(), Vec::new(), Vec::new()]
                        };
                        for p in parts_b {
                            let (rl_p, fr_p) = p?;
                            if split {
                                for (dst, src) in learned_freqs.iter_mut().zip(fr_p) {
                                    for (drow, srow) in dst.iter_mut().zip(src) {
                                        for (d, s) in drow.iter_mut().zip(srow) {
                                            *d += s;
                                        }
                                    }
                                }
                            } else {
                                for (d, s) in learned_rl.iter_mut().zip(rl_p) {
                                    *d += s;
                                }
                            }
                        }
                        superblock::select::Coder::of_counts(
                            &learned_rl,
                            &learned_freqs,
                            cfg,
                            effort,
                            split,
                        )
                    } else {
                        superblock::select::Coder::of_counts(
                            &pilot_rl,
                            &pilot_freqs,
                            cfg,
                            effort,
                            split,
                        )
                    };
                    superblock::select::apply_with_learned(
                        stripes,
                        &fill_block_row,
                        pw,
                        gbw,
                        &steps,
                        round,
                        cfg,
                        learned_coder,
                        Some(learned_map),
                        learned_flags,
                        &open_lambda_map,
                        sel_opts,
                        pulse,
                    )?
                } else {
                    superblock::select::apply_with_lambda(
                        stripes,
                        &fill_block_row,
                        pw,
                        gbw,
                        &steps,
                        round,
                        cfg,
                        effort,
                        &open_lambda_map,
                        sel_opts,
                        pulse,
                    )?
                };
                if exact.is_some() {
                    for sc in &mut stripes {
                        for code in &mut sc.aq {
                            *code = u8::from(*code == 0);
                        }
                    }
                }
                stripes
            } else {
                stripes
            };
            pulse.check().map_err(|_| Error::Cancelled)?;
            Ok((stripes, exact, sb_on, open_lambda_map))
        })?;
        stages.run(|pulse| {
            finish_lossy_frame(
                info,
                meta,
                cfg,
                effort,
                Some((pixels, stride)),
                Some(&fill_block_row),
                &g,
                sb,
                color,
                kind,
                scale,
                qp,
                steps,
                layouts,
                skip_on,
                exact,
                sb_on,
                trellis,
                &trellis_aq,
                &open_lambda_map,
                dc_fix.then_some(white_code.is_some()),
                stripes,
                pulse,
            )
        })
    })
}

/// Everything after the stripes' quantized coefficients exist: the nested
/// DC image, the per-unit AC coding, the seed-table choice and the file
/// assembly. Shared by the pixel path (`encode_lossy_frame`, which passes
/// `pixels` and `rows`) and the lab coefficient path (`encode_lossy_coefs`,
/// both `None`): `rows` recomputes block-row planes for the stages that
/// still need pixels (the AC trellis, lab RDOQ), all of which a
/// coefficient-path `LossyConfig` keeps off.
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "encode")]
fn finish_lossy_frame(
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    effort: crate::effort::Effort,
    pixels: Option<(&[u8], usize)>,
    rows: Option<RowPlanes<'_>>,
    g: &Geom,
    sb: u32,
    color: LossyColor,
    kind: InputKind,
    scale: [f32; 3],
    qp: QParams,
    steps: [[f32; 64]; 3],
    layouts: [LossyChannelLayout; 3],
    skip_on: bool,
    exact: Option<Coded>,
    sb_on: bool,
    trellis: Option<(trellis::LossyTrellis, bool)>,
    trellis_aq: &[f32],
    open_lambda_map: &[f32],
    rdoq_dc: Option<bool>,
    stripes: Vec<StripeCoefs>,
    pulse: &dyn Pulse,
) -> Result<Vec<u8>, Error> {
    pulse.check().map_err(|_| Error::Cancelled)?;
    local_ans::validate(cfg)?;
    #[cfg(feature = "lab")]
    crate::seeds::validate_id(cfg.seed_bank)?;
    let stop = pulse.live();
    let nested = how_far::WithStop::borrowed(&how_far::NoPulse, &stop);
    let nci = info.channels.count();
    let bps = info.sample.bytes();
    let rb = info.row_bytes();
    let pw = g.bw * 8;
    let xyb = color == LossyColor::Xyb;
    let trellis_bias = qp.bias.filter(|b| !b.is_zero()).map(|b| b.tables());
    let adaptive_mask = layout::adaptive_mask(&layouts);
    #[cfg(not(feature = "lab"))]
    let _ = rdoq_dc;
    // DC image: U16 (value + 32768) when every DC fits, else I32.
    let fits16 = stripes.iter().all(|sc| {
        sc.q.as_chunks::<64>()
            .0
            .iter()
            .all(|b| (-32768..=32767).contains(&b[0]))
    });
    let dcs = if fits16 { 2 } else { 4 };
    let aq_on = aq_on_cfg(cfg);
    let dch = if aq_on || skip_on || exact.is_some() {
        4
    } else {
        NCH
    };
    let mut dc = vec![0u8; g.bw * g.bh * dch * dcs];
    for sc in &stripes {
        for r in 0..sc.nby {
            pulse.check().map_err(|_| Error::Cancelled)?;
            for bx in 0..g.bw {
                let b = r * g.bw + bx;
                let o0 = (((sc.by0 + r) * g.bw + bx) * dch) * dcs;
                let put = |dc: &mut Vec<u8>, c: usize, v: i32| {
                    let o = o0 + c * dcs;
                    if fits16 {
                        dc[o..o + 2].copy_from_slice(&(v as u16).to_ne_bytes());
                    } else {
                        dc[o..o + 4].copy_from_slice(&v.to_ne_bytes());
                    }
                };
                for c in 0..NCH {
                    let v = sc.q[(b * NCH + c) * 64];
                    put(&mut dc, c, if fits16 { v + 32768 } else { v });
                }
                if dch == 4 {
                    put(&mut dc, 3, sc.aq[b] as i32);
                }
            }
        }
    }
    let dc_info = ImageInfo::new(
        g.bw as u32,
        g.bh as u32,
        if dch == 4 {
            Channels::Rgba
        } else {
            Channels::Rgb
        },
        if fits16 {
            SampleType::U16
        } else {
            SampleType::I32
        },
    );
    let mut dc_rows = dc_stripe_rows(cfg, sb, info.width, info.height);
    // Column groups: the DC image's groups cover the same blocks.
    let gcols = lossy_group_cols(cfg, g.w);
    let ng = if gcols == 0 { 1 } else { g.w.div_ceil(gcols) };
    let coverage_head = cfg
        .dc_coverage
        .as_ref()
        .map(|lists| dc_coverage::Coverage::write(lists));
    let coverage = coverage_head
        .as_ref()
        .map(|head| {
            dc_coverage::Coverage::read(
                head,
                g.stripes(),
                ng,
                g.sb,
                g.bh,
                &crate::parse::limits::Unrestricted,
            )
        })
        .transpose()?;
    if let Some(c) = &coverage {
        dc_rows = c
            .rectangles
            .iter()
            .map(|&(a, b, _)| b - a)
            .max()
            .unwrap()
            .div_ceil(g.sb)
            * g.sb;
    }
    let dc_cfg = crate::EncodeConfig {
        effort: cfg.effort,
        stripe_rows: Some(dc_rows as u32),
        lanes: dc_lanes(cfg),
        threads: cfg.threads,
        column_groups: Some((gcols / 8) as u32),
        // Nested images stay within Lossless (and Extended) 0.1.
        float_shift: false,
        ..Default::default()
    };
    let (dc_head, dc_stripes, dc_explicit) =
        crate::seeds::with_role(crate::seeds::Role::Dc, || {
            let seeds = seed_bank(cfg).map(|b| (b, seed_exact(cfg)));
            if let Some(c) = &coverage {
                return crate::v1::encode_dc_rectangles(
                    &dc,
                    g.bw * dch * dcs,
                    &dc_info,
                    &dc_cfg,
                    seeds,
                    &c.rectangles,
                    &nested,
                );
            }
            crate::v1::encode_parts_seeded_with_progress(
                &dc,
                g.bw * dch * dcs,
                &dc_info,
                &dc_cfg,
                seeds,
                &nested,
            )
        })?;
    // Features the nested heads use (a 32-bit DC image's offset maps): the
    // file declares them (SPEC 2.8).
    let mut nested_features = crate::v1::head_features(&dc_info, &dc_head);
    if let Some(e) = &dc_explicit {
        nested_features = nested_features.union(crate::v1::head_features(&dc_info, &e.0));
    }

    // Alpha: a nested lossless gray image with the same sample type, one
    // stripe per lossy stripe.
    let alpha = if info.channels.has_alpha() {
        let mut a = Vec::with_capacity(g.w * g.h * bps);
        for y in 0..g.h {
            pulse.check().map_err(|_| Error::Cancelled)?;
            let (pixels, stride) = pixels.expect("alpha channel requires pixel input");
            let row = &pixels[y * stride..][..rb];
            for x in 0..g.w {
                let o = (x * nci + nci - 1) * bps;
                a.extend_from_slice(&row[o..o + bps]);
            }
        }
        let ai = ImageInfo::new(info.width, info.height, Channels::Gray, info.sample)
            .with_significant_bits(&[info.channel_bits(info.channels.count() - 1).unwrap()])?;
        let acfg = crate::EncodeConfig {
            effort: cfg.effort,
            stripe_rows: Some(sb * 8),
            threads: cfg.threads,
            column_groups: Some(gcols as u32),
            // Nested images stay within Lossless (and Extended) 0.1.
            float_shift: false,
            ..Default::default()
        };
        let coded = crate::seeds::with_role(crate::seeds::Role::Alpha, || {
            let seeds = seed_bank(cfg).map(|b| (b, seed_exact(cfg)));
            crate::v1::encode_parts_seeded_with_progress(&a, g.w * bps, &ai, &acfg, seeds, &nested)
        })?;
        nested_features = nested_features.union(crate::v1::head_features(&ai, &coded.0));
        if let Some(e) = &coded.2 {
            nested_features = nested_features.union(crate::v1::head_features(&ai, &e.0));
        }
        Some(coded)
    } else {
        None
    };

    pulse.check().map_err(|_| Error::Cancelled)?;
    // Pass 2: histograms -> tables.
    // Entropy mode: the tier's choice from the run-level token density, or
    // the configured mode. Small-tier images fold the token count into the
    // run-level coding below (both modes are coded anyway); larger tiers
    // need the counts before choosing, and lab profiling always wants them.
    let small = g.w * g.h <= SMALL_IMAGE;
    let want_stats =
        (cfg.decode_speed.is_some() && !small) || cfg!(all(feature = "lab", feature = "std"));
    let mut stats = want_stats.then(|| side_stats(&stripes, g.bw, cfg.threads));
    // The effort's trellis runs where the AC is coded run-level or
    // split-Huffman in XYB, priced with the coding's own rate model (in
    // YCoCg it lost 18-20% Butteraugli max; benchmarks/trellis_effort_2026-10-08,
    // aq_trellis_2026-10-09). Under a tier the cost model's choice from these
    // pass-1 counts (the tier decision below reuses them) picks the rate
    // model (`tier_huf`: an image sent to split-Huffman); small images, which
    // also try split-Huffman and keep the smaller file, code their
    // split-Huffman candidate from the split-Huffman trellis of the pass-1
    // coefficients. The lab override runs wherever it is set (`forced`);
    // lab `set_trellis_huf_rate(false)` keeps split-Huffman without it.
    let (trellis, forced, tier_huf) = match trellis {
        Some((t, true)) => (Some(t), true, false),
        Some((t, false)) if xyb => match cfg.decode_speed {
            None => (
                (cfg.entropy == LossyEntropy::RunLevel
                    || (cfg.entropy == LossyEntropy::SplitHuf && trellis::huf_rate(cfg)))
                .then_some(t),
                false,
                false,
            ),
            Some(speed) => {
                let st = *stats.get_or_insert_with(|| side_stats(&stripes, g.bw, cfg.threads));
                let ratio = predicted_huf_ratio((g.w * g.h) as f64, &st);
                if huf_max_ratio(speed).is_some_and(|m| ratio <= m) {
                    (trellis::huf_rate(cfg).then_some(t), false, true)
                } else {
                    (Some(t), false, false)
                }
            }
        },
        _ => (None, false, false),
    };
    let separate_tier_coefficients =
        trellis.is_some() && !forced && !tier_huf && small && cfg.decode_speed.is_some();
    // A small automatic-tier encode can compare two different reconstructions.
    // For a lab request with a different bank, preserve the mode the normal
    // complete-file comparison would select, including its explicit fallback.
    #[cfg(feature = "lab")]
    let reference_entropy =
        if separate_tier_coefficients && cfg.seed_bank != crate::seeds::DEFAULT_BANK {
            let mut reference = cfg.clone();
            reference.seed_bank = crate::seeds::DEFAULT_BANK;
            let file = finish_lossy_frame(
                info,
                meta,
                &reference,
                effort,
                pixels,
                rows,
                g,
                sb,
                color,
                kind,
                scale,
                qp,
                steps,
                layouts,
                skip_on,
                exact.clone(),
                sb_on,
                trellis.map(|t| (t, false)),
                trellis_aq,
                open_lambda_map,
                rdoq_dc,
                stripes.clone(),
                &nested,
            )?;
            Some(crate::seeds::lossy_entropy_seed(&file)?.0)
        } else {
            None
        };
    // Coding units: (stripe, column group), stripe-major.
    let stripes: Vec<StripeCoefs> = if ng == 1 {
        stripes
    } else {
        stripes
            .into_iter()
            .flat_map(|sc| split_units(sc, gcols / 8))
            .collect()
    };
    #[cfg(feature = "lab")]
    let stripes = if cfg.rdoq_lambda != 0.0
        && (cfg.rdoq_policy == 0 || cfg.scale_for(color, info.width, info.height) <= 80.0)
    {
        rdoq_lab::optimize(
            stripes,
            rows.expect("the coefficient path runs no pixel stages"),
            pw,
            g.bw,
            cfg,
            &steps,
            gcols,
            effort,
            rdoq_dc,
            pulse,
        )?
    } else {
        stripes
    };
    pulse.check().map_err(|_| Error::Cancelled)?;
    // Small tier images keep the pass-1 coefficients for their split-Huffman
    // coding when the effort's trellis runs (at most 64K pixels).
    let pass1 = separate_tier_coefficients.then(|| stripes.clone());
    // The AC trellis re-decides the AC coefficients from each stripe's
    // recomputed transform.
    // Exact masks use 0 = lossy, 1 = exact; trellis block codes use
    // 64 = ordinary quantization, 0 = keep. Translate only for the search,
    // then restore the wire codes used by the already encoded DC image.
    let mut stripes = stripes;
    if trellis.is_some() && exact.is_some() {
        for unit in &mut stripes {
            for code in &mut unit.aq {
                *code = if *code == 0 { 64 } else { 0 };
            }
        }
    }
    let mut stripes = match &trellis {
        Some(t) => trellis::optimize(
            stripes,
            rows.expect("the coefficient path runs no pixel stages"),
            pw,
            g.bw,
            gcols / 8,
            &steps,
            cfg,
            effort,
            t,
            trellis_aq,
            open_lambda_map,
            trellis_bias.as_ref(),
            tier_huf
                || (cfg.decode_speed.is_none()
                    && cfg.entropy == LossyEntropy::SplitHuf
                    && trellis::huf_rate(cfg)),
            pulse,
        )?,
        None => stripes,
    };
    // Small tier images' split-Huffman candidate: the split-Huffman rate
    // model's trellis of the pass-1 coefficients.
    let pass1 = match (pass1, &trellis) {
        (Some(mut p), Some(t)) if trellis::huf_rate(cfg) => {
            if exact.is_some() {
                for unit in &mut p {
                    for code in &mut unit.aq {
                        *code = if *code == 0 { 64 } else { 0 };
                    }
                }
            }
            let mut p = trellis::optimize(
                p,
                rows.expect("the coefficient path runs no pixel stages"),
                pw,
                g.bw,
                gcols / 8,
                &steps,
                cfg,
                effort,
                t,
                trellis_aq,
                open_lambda_map,
                trellis_bias.as_ref(),
                true,
                pulse,
            )?;
            if exact.is_some() {
                for unit in &mut p {
                    for code in &mut unit.aq {
                        *code = u8::from(*code == 0);
                    }
                }
            }
            Some(p)
        }
        (p, _) => p,
    };
    if trellis.is_some() && exact.is_some() {
        for unit in &mut stripes {
            for code in &mut unit.aq {
                *code = u8::from(*code == 0);
            }
        }
    }
    // Lab counters of the coded coefficients (`lab::lossy_side_stats`).
    #[cfg(all(feature = "lab", feature = "std"))]
    if trellis.is_some() {
        prof::side_stats(&trellis::unit_stats(&stripes, cfg.threads));
    } else if let Some(st) = &stats {
        prof::side_stats(st);
    }
    pulse.check().map_err(|_| Error::Cancelled)?;
    // Table headers and independently coded stripe records in the selected
    // mode; with seed tables, also the same blob with explicit tables.
    struct AcCoding {
        entropy: LossyEntropy,
        tb: Vec<u8>,
        records: Vec<Vec<u8>>,
        explicit: Option<Coded>,
    }
    impl AcCoding {
        /// The coding without seed tables.
        fn explicit(&self) -> (&[u8], &[Vec<u8>]) {
            match &self.explicit {
                Some((t, r)) => (t, r),
                None => (&self.tb, &self.records),
            }
        }
    }
    let code_units = |units: &[StripeCoefs], entropy| -> Result<AcCoding, Error> {
        let (tb, records, explicit) = if entropy == LossyEntropy::SplitHuf {
            encode_huffman_stripes(units, cfg, effort, pulse)?
        } else {
            let (tb, sb, _, explicit) = encode_ans_stripes(units, cfg, effort, pulse)?;
            (tb, sb, explicit)
        };
        Ok(AcCoding {
            entropy,
            tb,
            records,
            explicit,
        })
    };
    let code = |entropy| code_units(&stripes, entropy);
    // The tier's choice: split-Huffman when the cost model predicts it
    // decodes within the tier's ratio of run-level's time; small images
    // try both and also take split-Huffman when it is smaller (its tables
    // cost less than the rANS tables). With seed tables the size compares
    // each mode's smaller coding; the mode the encoder picks without seeds
    // is kept too, for the final comparison below.
    let (codings, pick, pick_explicit) = match cfg.decode_speed {
        None => (alloc::vec![code(cfg.entropy)?], 0, 0),
        Some(speed) => {
            let size = |(t, r): (&[u8], &[Vec<u8>])| {
                t.len() + r.iter().map(|s| s.len() + 3).sum::<usize>()
            };
            let smaller = |c: &AcCoding| size((&c.tb, &c.records)).min(size(c.explicit()));
            if small {
                // The run-level coding's token count stands in for the
                // side-stats coefficient walk: splitting column groups
                // preserves per-block emits.
                let (rl, n) = {
                    let (tb, records, n, explicit) =
                        encode_ans_stripes(&stripes, cfg, effort, pulse)?;
                    let rl = AcCoding {
                        entropy: LossyEntropy::RunLevel,
                        tb,
                        records,
                        explicit,
                    };
                    (rl, n)
                };
                let st = stats.unwrap_or_else(|| side_stats_lanes(&stripes, n));
                let ratio = predicted_huf_ratio((g.w * g.h) as f64, &st);
                let fast = huf_max_ratio(speed).is_some_and(|m| ratio <= m);
                // Split-Huffman never carries the run-level trellis's
                // coefficients (with the effort's trellis: the split-Huffman
                // rate model's, from pass 1).
                let sh = code_units(
                    pass1.as_ref().map_or(&stripes[..], |p| &p[..]),
                    LossyEntropy::SplitHuf,
                )?;
                let pick = (fast || smaller(&sh) < smaller(&rl)) as usize;
                let pick_explicit = (fast || size(sh.explicit()) < size(rl.explicit())) as usize;
                #[cfg(feature = "lab")]
                let (pick, pick_explicit) =
                    reference_entropy.map_or((pick, pick_explicit), |entropy| {
                        let pick = usize::from(entropy == LossyEntropy::SplitHuf as u8);
                        (pick, pick)
                    });
                (alloc::vec![rl, sh], pick, pick_explicit)
            } else {
                let st = stats.as_ref().unwrap();
                let ratio = predicted_huf_ratio((g.w * g.h) as f64, st);
                let fast = huf_max_ratio(speed).is_some_and(|m| ratio <= m);
                let mode = if fast {
                    LossyEntropy::SplitHuf
                } else {
                    LossyEntropy::RunLevel
                };
                (alloc::vec![code(mode)?], 0, 0)
            }
        }
    };

    // Bytes of the AC records as written, and whether compact restart states
    // pay for themselves (they are chosen when smaller).
    // Superblocks and small transforms: each unit's partition map. A file
    // whose units split some plane takes the joint maps (head record 23),
    // else one that merges takes record 21's maps, else no map (the plain
    // lossy file).
    let any_split = stripes.iter().any(|sc| sc.splits.iter().any(|&v| v != 0));
    let any_merge = sb_on && stripes.iter().any(|sc| sc.modes.iter().any(|&v| v != 0));
    let map_kind = if any_split {
        MapKind::Joint
    } else if any_merge {
        MapKind::Merges
    } else {
        MapKind::None
    };
    let default_maps = if cfg.coding_defaults {
        if layouts != [LossyChannelLayout::default(); 3] {
            return Err(Error::InvalidInput("coding defaults with channel layouts"));
        }
        Some(coding_defaults::encode(&stripes)?)
    } else {
        None
    };
    let sb_maps: Option<Vec<Vec<u8>>> = if let Some((_, maps)) = &default_maps {
        Some(maps.clone())
    } else {
        match map_kind {
            MapKind::None => None,
            MapKind::Merges => Some(
                stripes
                    .iter()
                    .map(|sc| superblock::map::encode(&sc.modes, sc.nby, sc.bw))
                    .collect(),
            ),
            MapKind::Joint => Some(
                stripes
                    .iter()
                    .map(|sc| {
                        // Bypassed parents (exact blocks: wire code 1; kept:
                        // code 0) carry no flags.
                        let bypass = |b: usize| {
                            let c = sc.aq.get(b).copied();
                            (exact.is_some() && c == Some(1)) || (skip_on && c == Some(0))
                        };
                        let n = sc.nby * sc.bw;
                        let modes = if sc.modes.is_empty() {
                            alloc::vec![0u8; n]
                        } else {
                            sc.modes.clone()
                        };
                        // Flags only where the final coefficients have AC (a
                        // plane the trellis emptied decodes as the 8x8 block).
                        let ac = superblock::ac_bits(&sc.q, n);
                        let splits: Vec<u8> = (0..n)
                            .map(|b| sc.splits.get(b).copied().unwrap_or(0) & ac[b])
                            .collect();
                        superblock::map::encode_joint(
                            &modes,
                            &splits,
                            &ac,
                            (sc.nby, sc.bw),
                            &bypass,
                        )
                    })
                    .collect(),
            ),
        }
    };
    let ac_records_len =
        |entropy: LossyEntropy, stripe_bytes: &[Vec<u8>]| -> Result<(u128, bool), Error> {
            let record_len =
                |n: usize| n as u128 + (64 - (n as u64).leading_zeros()).max(1).div_ceil(7) as u128;
            let (mut raw, mut packed) = (0u128, 0u128);
            #[cfg(all(feature = "lab", test))]
            let catalog = cfg
                .catalog_lab
                .is_some_and(model_candidates::Settings::uses_envelope);
            #[cfg(not(all(feature = "lab", test)))]
            let catalog = false;
            let compactable = cfg.compact_streams
                && entropy == LossyEntropy::RunLevel
                && !cfg.local_ans_contexts
                && !cfg.model_references
                && !catalog;
            for (u, (bytes, sc)) in stripe_bytes.iter().zip(&stripes).enumerate() {
                let map_len = if let Some(maps) = &sb_maps {
                    crate::varint_len(maps[u].len() as u64) + maps[u].len()
                } else if adaptive_mask == 0 || sc.modes.is_empty() {
                    // No mode map is written (with superblocks on and nothing
                    // merged, `modes` holds the selector's all-zero roles).
                    0
                } else if sc.modes.iter().all(|m| *m == sc.modes[0]) {
                    2
                } else {
                    1 + (sc.modes.len() * 3).div_ceil(8)
                };
                raw += record_len(map_len + bytes.len());
                if compactable {
                    packed += record_len(map_len + compact::packed_len(bytes, sc.bw.min(NL))?);
                }
            }
            Ok(if compactable && packed < raw {
                (packed, true)
            } else {
                (raw, false)
            })
        };
    let assemble = |entropy: LossyEntropy,
                    tb: &[u8],
                    stripe_bytes: &[Vec<u8>],
                    dc_head: &[u8],
                    dc_stripes: &[Vec<u8>],
                    alpha: Option<(&[u8], &[Vec<u8>])>,
                    seed_selection: crate::seeds::Selection|
     -> Result<Vec<u8>, Error> {
        use crate::parse::records::{
            H_UNIT_ORDINALS, R_AC, R_ALPHA, R_DC, R_EXACT, main_framed_len, put_main_record,
            put_record,
        };
        let compact_states = ac_records_len(entropy, stripe_bytes)?.1;
        let mut model_writer = if cfg.model_references {
            Some(model_refs::Writer::new(
                tb,
                entropy,
                if seed_selection.scope & crate::seeds::SCOPE_AC != 0 {
                    seed_selection.bank
                } else {
                    0
                },
            )?)
        } else {
            None
        };
        let tb = model_writer.as_ref().map_or(tb, |w| w.root);
        // Unit records (SPEC 2.7), in decode order: per lossy stripe s, the
        // DC stripe s / k when s is its first lossy stripe (k = DC stripe
        // height / sb), the alpha stripe s, lossy stripe s, then its exact
        // record (exact blocks only); with column groups, per stripe and
        // group in order. The unit table hint lists their spans.
        let k = dc_rows / sb as usize;
        let ordinals = cfg.unit_ordinals;
        let mut body = Vec::new();
        let mut spans = Vec::new();
        let ordinal_counter = core::cell::Cell::new(0u64);
        let mut rec = |id: u64, b: &[u8]| {
            let prefixed;
            let b = if cfg.model_references && id == R_DC {
                let mut bytes = vec![0, 0];
                bytes.extend_from_slice(b);
                prefixed = bytes;
                &prefixed[..]
            } else {
                b
            };
            let ord = ordinals.then_some(ordinal_counter.get());
            ordinal_counter.set(ordinal_counter.get() + 1);
            spans.push(main_framed_len(id, ord, b.len()));
            put_main_record(&mut body, id, ord, b);
        };
        for (u, st) in stripe_bytes.iter().enumerate() {
            let (si, gi) = (u / ng, u % ng);
            if let Some(c) = &coverage {
                let span = c.span(si, gi);
                if span.start == si {
                    rec(R_DC, &dc_stripes[span.unit]);
                }
            } else if si % k == 0 {
                rec(R_DC, &dc_stripes[si / k * ng + gi]);
            }
            if let Some((_, a)) = &alpha {
                rec(R_ALPHA, &a[u]);
            }
            let native;
            let st = if let Some(writer) = &mut model_writer {
                native = writer.ac(st, ordinal_counter.get())?;
                &native[..]
            } else {
                st
            };
            if adaptive_mask != 0 || compact_states || sb_maps.is_some() {
                let mut record = Vec::new();
                if let Some(maps) = &sb_maps {
                    put_varint(&mut record, maps[u].len() as u64);
                    record.extend_from_slice(&maps[u]);
                } else if adaptive_mask != 0 {
                    layout::write_modes(&mut record, &stripes[u].modes);
                }
                if compact_states {
                    record.extend_from_slice(&compact::pack(st, stripes[u].bw.min(NL))?);
                } else {
                    record.extend_from_slice(st);
                }
                rec(R_AC, &record);
            } else {
                rec(R_AC, st);
            }
            if let Some((_, er)) = &exact {
                rec(R_EXACT, &er[u]);
            }
        }
        // Head records in increasing id order (SPEC 2.8). The unit table
        // can be a hint (10) or a reservation (12), on either side of DC32 (11).
        let mut records = Vec::new();
        let hint_form = matches!(crate::v1::unit_table_form(), crate::v1::UnitTableForm::Hint);
        for (on, id) in [(exact.is_some(), H_EXACT), (aq_on, H_AQ), (skip_on, H_KEEP)] {
            if on {
                put_record(&mut records, id, &[]);
            }
        }
        if hint_form {
            crate::v1::put_unit_table(&mut records, stripe_bytes.len(), &spans);
        }
        if !fits16 {
            put_record(&mut records, H_DC32, &[]);
        }
        if !hint_form {
            crate::v1::put_unit_table(&mut records, stripe_bytes.len(), &spans);
        }
        if qp.matrix == MATRIX_EXPLICIT {
            let mut rec = Vec::new();
            envelope::write_explicit(&mut rec, qp.explicit.as_ref().expect("explicit tables"));
            put_record(&mut records, H_EXPLICIT, &rec);
        }
        if let Some(b) = &qp.bias {
            let mut rec = Vec::new();
            envelope::write_bias(&mut rec, b);
            put_record(&mut records, H_BIAS, &rec);
        }
        if let Some(t) = qp.transform {
            let mut rec = Vec::new();
            envelope::write_transform(&mut rec, t);
            put_record(&mut records, H_TRANSFORM, &rec);
        }
        match if cfg.coding_defaults {
            MapKind::None
        } else {
            map_kind
        } {
            MapKind::None => {}
            MapKind::Merges => put_record(&mut records, superblock::H_SUPERBLOCKS, &[]),
            MapKind::Joint => {
                #[cfg(feature = "lab")]
                let payload: &[u8] = if cfg.lab.small.as_ref().is_some_and(|s| s.int) {
                    &[1]
                } else {
                    &[]
                };
                #[cfg(not(feature = "lab"))]
                let payload: &[u8] = &[];
                put_record(&mut records, small::H_PARTITIONS, payload)
            }
        }
        if layouts != [LossyChannelLayout::default(); 3] {
            put_record(&mut records, H_LAYOUTS, &layouts.map(|l| l.byte()));
        }
        if ordinals {
            put_record(&mut records, H_UNIT_ORDINALS, &[]);
        }
        if cfg.local_ans_contexts {
            put_record(&mut records, local_ans::HEAD, &[]);
        }
        if let Some((head, _)) = &default_maps {
            put_record(&mut records, coding_defaults::HEAD, head);
        }
        if cfg.model_references {
            put_record(&mut records, model_refs::HEAD, &[1, 0]);
        }
        if let Some(head) = &coverage_head {
            put_record(&mut records, dc_coverage::HEAD, head);
        }
        #[cfg(all(feature = "lab", test))]
        let tb = if !cfg.model_references
            && cfg
                .catalog_lab
                .is_some_and(model_candidates::Settings::uses_envelope)
        {
            let mut bundle = ByteReader::new(tb);
            let n = bundle.var_usize()?;
            let tables = bundle.bytes(n)?;
            put_record(
                &mut records,
                model_candidates::HEAD,
                bundle.bytes(bundle.remaining())?,
            );
            tables
        } else {
            tb
        };
        let mut data = Vec::new();
        data.push(sb as u8);
        put_varint(&mut data, gcols as u64);
        envelope::write(
            &mut data,
            color,
            kind,
            &envelope::Params {
                scale,
                qp,
                entropy,
                compact: compact_states,
                seed: seed_selection,
                records: !records.is_empty(),
            },
        );
        put_varint(&mut data, tb.len() as u64);
        data.extend_from_slice(tb);
        if !records.is_empty() {
            put_varint(&mut data, records.len() as u64);
            data.extend_from_slice(&records);
        }
        // The nested heads, positional (SPEC 2.8): DC, alpha (with alpha),
        // exact blocks (with head record 5).
        put_varint(&mut data, dc_head.len() as u64);
        data.extend_from_slice(dc_head);
        if let Some((ah, _)) = &alpha {
            put_varint(&mut data, ah.len() as u64);
            data.extend_from_slice(ah);
        }
        if let Some((eh, _)) = &exact {
            put_varint(&mut data, eh.len() as u64);
            data.extend_from_slice(eh);
        }
        data.extend_from_slice(&body);
        let mut out = Vec::with_capacity(data.len() + 64);
        let mut c = cfg.clone();
        c.color = color;
        // New seed IDs declare `seed_bank_ids`, exact blocks `exact_blocks`,
        // an i32 DC image `samples_32` (implied by 32-bit sample types: the
        // header writer drops it there), channel layouts `channel_layouts`,
        // with the nested heads' features, in the header's feature mask
        // (the comparisons below count its bytes).
        use crate::parse::header::Feature;
        let mut features = nested_features;
        if seed_selection.is_some() {
            features = features.with(Feature::SeedBankIds);
        }
        if exact.is_some() {
            features = features.with(Feature::ExactBlocks);
        }
        if !fits16 {
            features = features.with(Feature::Samples32);
        }
        if layouts != [LossyChannelLayout::default(); 3] {
            features = features.with(Feature::ChannelLayouts);
        }
        if (3..=MATRIX_LAST_PRESET).contains(&qp.matrix) {
            features = features.with(Feature::DequantPresets);
        }
        if qp.matrix == MATRIX_EXPLICIT {
            features = features.with(Feature::ExplicitTables);
        }
        if qp.bias.is_some() {
            features = features.with(Feature::ReconBias);
        }
        if qp.transform.is_some() {
            features = features.with(Feature::ColorTransform);
        }
        if any_merge {
            features = features.with(Feature::Superblocks);
        }
        if cfg.model_references {
            features = features.with(Feature::ModelReferences);
        }
        if cfg.dc_coverage.is_some() {
            features = features.with(Feature::DcCoverage);
        }
        if cfg.coding_defaults {
            features = features.with(Feature::CodingDefaults);
        }
        if map_kind == MapKind::Joint {
            features = features.with(Feature::SmallTransforms);
        }
        if ordinals {
            features = features.with(Feature::UnitOrdinals);
        }
        if cfg.local_ans_contexts {
            features = features.with(Feature::LocalAnsContexts);
        }
        crate::container::write_head_features(
            &mut out,
            info,
            meta,
            crate::Coding::Lossy,
            Some(color),
            c.decode_speed(),
            features,
        )?;
        crate::container::put_data(&mut out, &data);
        Ok(out)
    };
    // Each table blob takes its smaller coding (the parts' bytes add up),
    // then the result is compared once with the all-explicit file: the file
    // this encoder writes without seed tables, byte for byte, which wins
    // ties. Both are complete files, so the seeded file's feature mask (the
    // mask field, or nothing when exact blocks already wrote it) counts. So
    // seed tables never make a file larger. Both candidates carry the same
    // exact blocks.
    type Part<'p> = (&'p [u8], &'p [Vec<u8>]);
    fn varint_len(n: usize) -> u128 {
        (64 - (n as u64).leading_zeros()).max(1).div_ceil(7) as u128
    }
    fn nested_len((head, recs): Part<'_>) -> u128 {
        varint_len(head.len())
            + head.len() as u128
            + recs
                .iter()
                .map(|r| varint_len(r.len()) + r.len() as u128)
                .sum::<u128>()
    }
    /// The smaller of a nested head's seeded coding `main` and its explicit
    /// one, and whether it is the seeded one.
    fn choose<'p>(main: Part<'p>, explicit: &'p Option<Coded>) -> (Part<'p>, bool) {
        match explicit {
            Some(e) if nested_len(main) < nested_len((&e.0, &e.1)) => (main, true),
            Some(e) => ((&e.0[..], &e.1[..]), false),
            None => (main, false),
        }
    }
    let ac_len = |c: &AcCoding, (tb, recs): Part<'_>| -> Result<u128, Error> {
        Ok(varint_len(tb.len()) + tb.len() as u128 + ac_records_len(c.entropy, recs)?.0)
    };
    let ac = &codings[pick];
    let (ac_part, ac_seeded) = match &ac.explicit {
        Some(e) if ac_len(ac, (&ac.tb, &ac.records))? < ac_len(ac, (&e.0, &e.1))? => {
            ((&ac.tb[..], &ac.records[..]), true)
        }
        _ => (ac.explicit(), false),
    };
    let (dc_part, dc_seeded) = choose((&dc_head, &dc_stripes), &dc_explicit);
    let al = alpha.as_ref().map(|(h, r, e)| choose((h, r), e));
    let scope = ((ac_seeded as u8) * crate::seeds::SCOPE_AC)
        | ((dc_seeded as u8) * crate::seeds::SCOPE_DC)
        | ((al.is_some_and(|a| a.1) as u8) * crate::seeds::SCOPE_ALPHA);
    let explicit_file = |c: &AcCoding| {
        let (dh, dr) = dc_explicit
            .as_ref()
            .map_or((&dc_head[..], &dc_stripes[..]), |e| (&e.0[..], &e.1[..]));
        let al = alpha.as_ref().map(|(h, r, e)| {
            e.as_ref()
                .map_or((&h[..], &r[..]), |e| (&e.0[..], &e.1[..]))
        });
        let (t, r) = c.explicit();
        assemble(c.entropy, t, r, dh, dr, al, crate::seeds::Selection::NONE)
    };
    if scope == 0 && pick == pick_explicit {
        #[cfg(all(feature = "lab", feature = "std"))]
        prof::ac_tables(
            codings[pick_explicit].entropy as u8,
            codings[pick_explicit].explicit().0.len() as u64,
        );
        let out = explicit_file(&codings[pick_explicit])?;
        pulse.check().map_err(|_| Error::Cancelled)?;
        return Ok(out);
    }
    let bank_id = seed_bank(cfg).map_or(0, |b| b.id);
    let seed_selection = if scope == 0 {
        crate::seeds::Selection::NONE
    } else {
        crate::seeds::Selection::new(bank_id, scope)
    };
    let file = assemble(
        ac.entropy,
        ac_part.0,
        ac_part.1,
        dc_part.0,
        dc_part.1,
        al.map(|a| a.0),
        seed_selection,
    )?;
    let without = explicit_file(&codings[pick_explicit])?;
    #[cfg(all(feature = "lab", feature = "std"))]
    {
        let (mode, tb) = if file.len() < without.len() {
            (ac.entropy, ac_part.0)
        } else {
            (
                codings[pick_explicit].entropy,
                codings[pick_explicit].explicit().0,
            )
        };
        prof::ac_tables(mode as u8, tb.len() as u64);
    }
    pulse.check().map_err(|_| Error::Cancelled)?;
    Ok(if file.len() < without.len() {
        file
    } else {
        without
    })
}

/// LAB: assemble a lossy file from already-quantized coefficients — the
/// encoder's back half ([`finish_lossy_frame`]) with coefficients in place
/// of its forward DCT and quantizer output, for coefficient-import studies
/// (`imij::lab::encode_lossy_quantized`).
///
/// `q` covers the image's whole `ceil(w/8) x ceil(h/8)` block grid in
/// [`StripeCoefs`] layout: `q[(block * 3 + ch) * 64 + raster]` holds the
/// quantized coefficients with the quantized DC at index 0 (the nested DC
/// image is rebuilt from it). Each (block, channel)'s zigzag nonzero mask
/// is derived from `q` itself.
///
/// Every coefficient-reading or coefficient-modifying stage must be off or
/// it would silently not run; this is checked, not assumed: no AQ
/// (`aq_gamma`, `aq_map`, `aq_field`), subblock layouts, lab trellis
/// (`trellis` passes), RDOQ, LF prediction, exact blocks, forced
/// superblocks or a decoder-memory plan. The effort's trellis is never run
/// (it would re-decide AC); `effort` still drives the entropy-table
/// search. `color` must be YCoCg: the coefficients are code-value domain,
/// which is also what a signalled `lab.color_transform` is defined on.
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) fn encode_lossy_coefs(
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    q: &[i32],
) -> Result<Vec<u8>, Error> {
    let pinned_lab;
    let cfg = {
        let mut c = cfg.clone();
        c.lab = c.lab.pinned();
        pinned_lab = c;
        &pinned_lab
    };
    info.validate_precision()?;
    if info.sample != SampleType::U8
        || !matches!(info.channels, Channels::Rgb | Channels::Gray)
        || info.has_custom_precision()
    {
        return Err(Error::Unsupported(
            "coefficient input supports U8 RGB or Gray image infos",
        ));
    }
    let color = LossyColor::YCoCg;
    let effort = crate::effort::Effort::new(cfg.effort)?;
    // The coefficient path has no pixels: every stage that recomputes or
    // edits coefficients must be off, or the file would not hold `q`.
    if aq_on_cfg(cfg)
        || cfg.channel_layouts != [LossyChannelLayout::default(); 3]
        || cfg.exact_blocks.is_some_and(|k| k > 0.0)
        || cfg.decoder_memory_limit.is_some()
        || cfg.lf_pred != 0
        || cfg.rdoq_lambda != 0.0
        || cfg.trellis.is_some_and(|t| t.passes > 0)
        || cfg.lab.superblocks == Some(true)
        || cfg.lab.recon_bias_centroid.is_some()
    {
        return Err(Error::Unsupported(
            "coefficient input needs AQ, subblock layouts, trellis, RDOQ, LF prediction, exact blocks, superblocks, centroid bias and decoder-memory plans off",
        ));
    }
    if info.width == 0 || info.height == 0 || info.checked_buffer_len().is_none() {
        return Err(Error::InvalidInput("coefficient image geometry"));
    }
    let count = (info.width as usize)
        .div_ceil(8)
        .checked_mul((info.height as usize).div_ceil(8))
        .and_then(|n| n.checked_mul(NCH * 64))
        .ok_or(Error::InvalidInput("coefficient grid is not representable"))?;
    if q.len() != count {
        return Err(Error::InvalidInput(
            "coefficient planes must cover the image's block grid",
        ));
    }
    if q.as_chunks::<64>()
        .0
        .iter()
        .any(|block| block[1..].iter().any(|v| v.unsigned_abs() >= 1 << 22))
    {
        return Err(Error::InvalidInput(
            "AC coefficient magnitude must be below 2^22",
        ));
    }
    let sb = cfg.stripe_rows_for(info.width);
    let g = Geom::new(info.width, info.height, sb);
    let quantizer_scale = cfg.scale_for(color, info.width, info.height);
    if !quantizer_scale.is_finite() {
        return Err(Error::InvalidInput("invalid lossy numeric parameter"));
    }
    let matrix = match cfg.matrix {
        m @ 0..=MATRIX_LAST_PRESET => m,
        MATRIX_EXPLICIT => MATRIX_EXPLICIT,
        _ => 0,
    };
    let mut qp = QParams::from_quality(quantizer_scale, color, matrix);
    if let Some(m) = cfg.channel_mul {
        qp.mul = m;
    }
    if let Some(t) = &cfg.lab.explicit_matrix {
        qp.explicit = Some(envelope::round_trip_explicit(**t)?);
        qp.matrix = MATRIX_EXPLICIT;
    }
    qp.bias = cfg.lab.recon_bias.filter(|b| !b.is_zero());
    qp.transform = cfg.lab.color_transform;
    if let Some(t) = qp.transform
        && let ColorTransform::Explicit(m) = t
        && (!ColorTransform::explicit_valid(&m) || t.forward().is_none())
    {
        return Err(Error::InvalidInput("invalid explicit color transform"));
    }
    for (i, v) in lab_dc_step(cfg).into_iter().enumerate() {
        if v != 0 {
            for c in if i == 0 { 0..1 } else { 1..3 } {
                qp.dc[c] = v;
            }
        }
    }
    qp.apply_ac_scale(cfg.ac_scale)?;
    if !qp.valid() {
        return Err(Error::InvalidInput("invalid quantization parameters"));
    }
    finish_quantized_coefficients(info, meta, cfg, effort, &g, sb, qp, q)
}

/// Entropy/DC assembly shared by native coefficient import and the study
/// hook. It never transforms pixels or changes the supplied coefficients.
#[cfg(all(feature = "encode", any(feature = "lab", feature = "native-jpeg")))]
fn finish_quantized_coefficients(
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    effort: crate::effort::Effort,
    g: &Geom,
    sb: u32,
    qp: QParams,
    q: &[i32],
) -> Result<Vec<u8>, Error> {
    let color = LossyColor::YCoCg;
    let kind = InputKind::U8;
    let steps = qp.steps();
    // The zigzag nonzero masks the entropy coders emit, from `q` (bit k:
    // zigzag coefficient k nonzero — `quant_row`'s contract).
    let nz: Vec<u64> = q
        .as_chunks::<64>()
        .0
        .iter()
        .map(|b| {
            coef::ZIGZAG
                .iter()
                .enumerate()
                .fold(0u64, |m, (k, &i)| m | u64::from(b[i as usize] != 0) << k)
        })
        .collect();
    let stripes: Vec<StripeCoefs> = (0..g.stripes())
        .map(|s| {
            let (by0, by1) = g.rows(s);
            StripeCoefs {
                by0,
                nby: by1 - by0,
                bw: g.bw,
                q: q[by0 * g.bw * NCH * 64..by1 * g.bw * NCH * 64].to_vec(),
                nz: nz[by0 * g.bw * NCH..by1 * g.bw * NCH].to_vec(),
                aq: Vec::new(),
                modes: Vec::new(),
                splits: Vec::new(),
            }
        })
        .collect();
    finish_lossy_frame(
        info,
        meta,
        cfg,
        effort,
        None,
        None,
        g,
        sb,
        color,
        kind,
        color.default_scales(),
        qp,
        steps,
        cfg.channel_layouts,
        false,
        None,
        false,
        None,
        &[],
        &[],
        None,
        stripes,
        &how_far::NoPulse,
    )
}

/// Native component coefficient carrier, independent of source JPEG scans.
/// Coded dimensions include the source's retained partial/MCU blocks.
#[cfg(feature = "encode")]
#[cfg(feature = "native-jpeg")]
pub(crate) fn encode_native_component(
    width: u32,
    height: u32,
    table: &[u16; 64],
    coefficients: &[i16],
    entropy: LossyEntropy,
    effort: f32,
) -> Result<Vec<u8>, Error> {
    let info = ImageInfo::new(width, height, Channels::Gray, SampleType::U8);
    info.validate_precision()?;
    if width == 0
        || height == 0
        || !width.is_multiple_of(8)
        || !height.is_multiple_of(8)
        || table.contains(&0)
    {
        return Err(Error::InvalidInput(
            "native component coded grid or quantizer",
        ));
    }
    let cfg = LossyConfig {
        color: LossyColor::YCoCg,
        quality: 50.0,
        entropy,
        effort,
        channel_mul: Some([1.0; 3]),
        column_groups: Some(0),
        exact_blocks: Some(0.0),
        ..LossyConfig::default()
    };
    let sb = cfg.stripe_rows_for(width);
    let g = Geom::new(width, height, sb);
    let count =
        g.bw.checked_mul(g.bh)
            .and_then(|n| n.checked_mul(64))
            .ok_or(Error::TooLarge)?;
    if count != coefficients.len() {
        return Err(Error::InvalidInput("native component coefficient count"));
    }
    let mut q = Vec::new();
    resize_fallible(&mut q, count.checked_mul(NCH).ok_or(Error::TooLarge)?, 0i32)?;
    for (src, dst) in coefficients
        .as_chunks::<64>()
        .0
        .iter()
        .zip(q.as_chunks_mut::<{ NCH * 64 }>().0.iter_mut())
    {
        for (a, &b) in dst[..64].iter_mut().zip(src) {
            *a = i32::from(b);
        }
    }
    let mut qp = QParams::from_quality(50.0, LossyColor::YCoCg, MATRIX_EXPLICIT);
    qp.mul = [1.0; 3];
    qp.dc = [64; 3];
    qp.explicit = Some([table.map(f32::from); 3]);
    if !qp.valid() {
        return Err(Error::InvalidInput("native quantization table"));
    }
    let file = finish_quantized_coefficients(
        &info,
        &Metadata::default(),
        &cfg,
        crate::effort::Effort::new(effort)?,
        &g,
        sb,
        qp,
        &q,
    )?;
    let file = crate::container::finish_unit_table(file)?;
    crate::container::locate(file)
}

/// A strict coefficient carrier reader. It shares production stripe/DC
/// decoding and refuses any declaration that would edit native spectra.
#[cfg(feature = "native-jpeg")]
pub(crate) fn decode_native_component(
    file: &[u8],
    width: u32,
    height: u32,
    table: &[u16; 64],
) -> Result<Vec<i16>, Error> {
    decode_native_component_checked(
        file,
        width,
        height,
        table,
        &crate::parse::limits::Unrestricted,
    )
}

#[cfg(feature = "native-jpeg")]
pub(crate) fn parse_native_component_checked<'a>(
    file: &'a [u8],
    width: u32,
    height: u32,
    table: &[u16; 64],
    admission: &impl crate::parse::limits::Admission,
) -> Result<LossyParsed<'a>, Error> {
    let mut raw = ByteReader::new(file);
    let hp = crate::parse::header::read_header_sized(&mut raw)?;
    crate::parse::chunks::walk_to_data(&mut raw, |_, _| {})?;
    if crate::parse::payload::native_selected(hp.header.lossy_color, hp.declared, &file[raw.pos..])?
    {
        return Err(Error::Corrupt("nested native component envelope"));
    }
    let f = crate::v1::parse_file(file, admission, crate::checksum::Verify::Now(1))?;
    if f.header.info.width != width
        || f.header.info.height != height
        || f.header.info.channels != Channels::Gray
        || f.header.info.sample != SampleType::U8
        || f.header.info.has_custom_precision()
        || f.header.lossy_color != Some(LossyColor::YCoCg)
        || width == 0
        || height == 0
        || !width.is_multiple_of(8)
        || !height.is_multiple_of(8)
        || table.contains(&0)
    {
        return Err(Error::Corrupt("native component carrier header"));
    }
    let p = parse_lossy_checked(
        f.header.info.clone(),
        f.header.version,
        LossyColor::YCoCg,
        f.declared,
        f.data,
        &crate::parse::limits::Checked(admission),
    )?;
    let expected = dct::dequant_factors(&table.map(f32::from));
    if !p.native_quantizer
        || p.ngroups() != 1
        || p.aq
        || p.skip
        || p.exact.is_some()
        || p.sb.is_some()
        || p.alpha.is_some()
        || p.bias.is_some()
        || p.transform.is_some()
        || p.layouts != [LossyChannelLayout::default(); 3]
        || p.inv_scale != [1.0; 3]
        || p.dq != [expected; 3]
    {
        return Err(Error::Corrupt(
            "native component carrier changes coefficients",
        ));
    }
    Ok(p)
}

#[cfg(feature = "native-jpeg")]
#[cfg(feature = "native-jpeg")]
pub(crate) fn decode_native_component_checked(
    file: &[u8],
    width: u32,
    height: u32,
    table: &[u16; 64],
    admission: &impl crate::parse::limits::Admission,
) -> Result<Vec<i16>, Error> {
    let mut p = parse_native_component_checked(file, width, height, table, admission)?;
    let dc =
        p.dc.take()
            .ok_or(Error::Corrupt("native component DC image"))?;
    let dinfo = dc.info.clone();
    let dc_stride = dinfo.row_bytes();
    let mut dc_rows = Vec::new();
    resize_fallible(
        &mut dc_rows,
        dc_stride.checked_mul(p.g.sb).ok_or(Error::TooLarge)?,
        0u8,
    )?;
    let mut stream = crate::v1::LosslessStream::from_parsed(dc);
    let mut out = Vec::new();
    let count =
        p.g.bw
            .checked_mul(p.g.bh)
            .and_then(|n| n.checked_mul(64))
            .ok_or(Error::TooLarge)?;
    out.try_reserve_exact(count).map_err(|_| Error::TooLarge)?;
    let (mut q, mut eobs, mut values, mut scales) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for stripe in 0..p.g.stripes() {
        crate::parse::limits::Admission::check(admission)?;
        let (a, b) = p.g.rows(stripe);
        let rows = b - a;
        for y in 0..rows {
            admission.check()?;
            if !stream.next_row(&mut dc_rows[y * dc_stride..(y + 1) * dc_stride])? {
                return Err(Error::Corrupt("short native component DC image"));
            }
        }
        dc_values(
            dinfo.sample,
            false,
            false,
            &dc_rows[..rows * dc_stride],
            &mut values,
            &mut scales,
        )?;
        let raw = decode_stripe_raw_bytes(
            &p,
            &p.g,
            stripe,
            p.stripes[stripe],
            Group::ALL,
            true,
            core::mem::take(&mut q),
            core::mem::take(&mut eobs),
        )?;
        finish_coefs(&p, &p.g, stripe, &values, raw, &mut q, &mut eobs)?;
        for block in q[..rows * p.g.bw * NCH * 64].as_chunks::<{ NCH * 64 }>().0 {
            if block[64..].iter().any(|&v| v != 0) {
                return Err(Error::Corrupt("nonzero unused native component channel"));
            }
            for &v in &block[..64] {
                out.push(i16::try_from(v).map_err(|_| Error::Corrupt("native coefficient range"))?);
            }
        }
        q.fill(0);
        values.clear();
        scales.clear();
    }
    let mut final_row = Vec::new();
    resize_fallible(&mut final_row, dc_stride, 0u8)?;
    if stream.next_row(&mut final_row)? {
        return Err(Error::Corrupt("long native component DC image"));
    }
    Ok(out)
}

#[cfg(feature = "lab")]
#[cfg(feature = "native-jpeg")]
pub(crate) fn jpeg_idct_lab(q: &[i16; 64], table: &[u16; 64]) -> [f32; 64] {
    jpeg::idct(q, table)
}

#[cfg(feature = "lab")]
pub(crate) fn linear_rgb_lab(matrix: &[[f32; 4]; 3], values: [f32; 3]) -> [u8; 3] {
    color::inverse_linear_px(matrix, [1.0; 3], values)
}

// ---- decoding ---------------------------------------------------------------

pub(crate) struct LossyParsed<'a> {
    pub info: ImageInfo,
    /// Bytes of the `DATA` payload.
    payload_len: usize,
    g: Geom,
    color: LossyColor,
    inv_scale: [f32; 3],
    native_quantizer: bool,
    dq: [[f32; 64]; 3],
    /// Lossy 0.4 reconstruction bias per channel, raster order: B for
    /// |q| = 1 and for |q| >= 2 ([`ReconBias::tables`]); `None` without one.
    bias: Option<alloc::boxed::Box<[[[f32; 64]; 2]; 3]>>,
    /// Lossy 0.4 signalled linear color transform (its decode matrix,
    /// `rgb_j = M_j0*c0 + M_j1*c1 + M_j2*c2 + O_j`); `None` = the color's
    /// default (XYB, or the YCoCg lifting, including the YCoCg preset id).
    transform: Option<alloc::boxed::Box<[[f32; 4]; 3]>>,
    layouts: [LossyChannelLayout; 3],
    layout_steps: [[[f32; 64]; 2]; 3],
    adaptive_mask: u8,
    compact_states: bool,
    local_ans_contexts: bool,
    coding_defaults: Option<coding_defaults::Defaults>,
    model_refs: Option<model_refs::Models>,
    uniform_dc_k: usize,
    pub(crate) dc_coverage: Option<alloc::sync::Arc<dc_coverage::Coverage>>,
    #[cfg(all(feature = "lab", test))]
    catalog_lab: Option<model_candidates::Catalog>,
    tables: Tables,
    /// SplitHuf: Huffman tables, their decode tables, width-table rows.
    huf: Option<(huf::HufTables, huf::Dec, huf::WidRows)>,
    /// rANS decode tables (absent in modes without rANS lanes).
    rans: Option<RansTables>,
    /// Nested lossless DC image (its stripes from the records); taken by a
    /// streaming decoder.
    dc: Option<crate::v1::Parsed<'a>>,
    /// Per-block AQ step multipliers present (4-channel DC image).
    aq: bool,
    /// 16-bit XYB output: transfer table and linear clamp.
    xfer16: Option<(Vec<f32>, f32)>,
    /// Nested lossless alpha image (empty without alpha).
    alpha: Option<crate::v1::Parsed<'a>>,
    /// Column group width in pixels (0 = none).
    gcols: usize,
    /// Block code 0 keeps the reference block (inter frames only).
    skip: bool,
    /// The unit table hint (head record 10, or a filled reservation, SPEC
    /// 2.7): the bytes of each main record's span, when the head carries one.
    pub(crate) hint: Option<Vec<u64>>,
    /// The unit table reservation (head record 12), if any.
    pub(crate) reserve: Option<crate::parse::records::Reserve>,
    /// Whether main unit records carry 0-based ordinals (head record 37, SPEC 2.7).
    pub(crate) unit_ordinals: bool,
    /// Records of the units (stripe, column group), stripe-major.
    stripes: Vec<&'a [u8]>,
    /// Exact blocks (block code 1): the nested block-list image and each
    /// unit's list range.
    exact: Option<exact::Exact<'a>>,
    /// The merged blocks' dequantization tables (superblocks: head record
    /// 21, or record 23 in a file that declares `superblocks`).
    sb: Option<alloc::boxed::Box<superblock::Tables>>,
    /// Which partition map starts every unit record.
    map: MapKind,
    /// Small transforms (head record 23): the 4x4 planes' dequantization
    /// factors ([`small::dequant4`]).
    small: Option<alloc::boxed::Box<[[f32; 64]; NCH]>>,
    /// LAB: the 4x4 planes use the integer candidate (`small_lab.rs`).
    small_int: bool,
}

/// The partition map at the start of every lossy unit record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MapKind {
    /// No map (neither record 21 nor 23).
    None,
    /// Superblock merges (record 21, SPEC 2.9.2).
    Merges,
    /// Merges and 4x4 split flags (record 23, SPEC 2.11.2).
    Joint,
}

/// Decode tables of the rANS lanes.
struct RansTables {
    dec: alloc::boxed::Box<[u32; coef::MAX_TABLES << LOG_M]>,
    #[cfg_attr(
        not(any(
            target_arch = "x86_64",
            target_arch = "aarch64",
            target_arch = "wasm32"
        )),
        allow(dead_code)
    )]
    cmap: alloc::boxed::Box<[u8; coef::NRAW_PAD]>,
    /// `cmap` widened to u32 (hardware-gather table).
    #[cfg(target_arch = "x86_64")]
    cmap32: alloc::boxed::Box<[u32; coef::NRAW_PAD]>,
}

impl RansTables {
    fn new(tables: &Tables) -> Self {
        let cmap = tables.padded_map();
        Self {
            dec: tables.dec_tables(),
            #[cfg(target_arch = "x86_64")]
            cmap32: cmap
                .iter()
                .map(|&v| u32::from(v))
                .collect::<Vec<_>>()
                .into_boxed_slice()
                .try_into()
                .unwrap(),
            cmap,
        }
    }
}

/// Planning allowances live beside the structures/constants they account for.
pub(crate) fn decoder_table_bytes() -> u128 {
    // Fixed 64-bit-sized allowance keeps encoder plans identical on wasm32.
    const HEADER_ALLOWANCE: usize = 16 * 1024;
    const _: () = assert!(core::mem::size_of::<LossyParsed<'static>>() <= HEADER_ALLOWANCE);
    let rans = core::mem::size_of::<[u32; coef::MAX_TABLES << LOG_M]>() + coef::NRAW_PAD * 5;
    let huffman = (huf::MAX_CNT_TABLES + huf::MAX_WID_TABLES) * (1 << huf::LOG) * 2;
    let models = coef::MAX_TABLES * (2 * coef::ALPHA * 4 + 64) + 2 * coef::NRAW_PAD;
    (rans.max(huffman) + models + 2 * HEADER_ALLOWANCE + 32 * 1024) as u128
}

pub(crate) fn decoder_recon_bytes() -> u128 {
    // Vec growth, plane padding, linear color row and run descriptors. The
    // public decoder uses RECON_CHUNK; lab-only overrides are outside the plan.
    (2 * ((RECON_CHUNK * 8 + 16) * 8 * NCH * 4 + RECON_CHUNK * 8 * 3 * 4)) as u128
}

impl LossyParsed<'_> {
    #[cfg(feature = "v3")]
    pub(crate) fn has_model_references(&self) -> bool {
        self.model_refs.is_some()
    }

    #[cfg(feature = "v3")]
    pub(crate) fn model_storage_bytes(&self) -> u128 {
        self.model_refs
            .as_ref()
            .map_or(0, model_refs::Models::storage_bytes)
    }

    #[cfg(any(feature = "v3", test))]
    pub(crate) fn model_prefix<'b>(
        &self,
        u: usize,
        role: u64,
        bytes: &'b [u8],
    ) -> Result<&'b [u8], Error> {
        if role == 0 {
            return Ok(bytes);
        }
        let (_, g) = self.group(u % self.ngroups());
        Ok(self.record(&g, u / self.ngroups(), bytes)?.1)
    }

    /// 0: no leading map; 1: length-delimited map; 2: legacy packed layouts.
    #[cfg(feature = "v3")]
    pub(crate) fn model_map_prefix(&self, u: usize) -> (u8, usize) {
        if self.coding_defaults.is_some() || self.map != MapKind::None {
            return (1, 0);
        }
        let (_, g) = self.group(u % self.ngroups());
        let (a, b) = g.rows(u / self.ngroups());
        (if self.adaptive_mask == 0 { 0 } else { 2 }, (b - a) * g.bw)
    }

    #[cfg(feature = "v3")]
    pub(crate) fn referenced_model(
        &self,
        ordinal: u64,
        role: u64,
        bytes: &[u8],
    ) -> Result<Option<u64>, Error> {
        self.model_refs
            .as_ref()
            .map_or(Ok(None), |m| m.dependency(ordinal, role, bytes))
    }

    #[cfg(feature = "v3")]
    pub(crate) fn admit_model_prefix(
        &mut self,
        ordinal: u64,
        role: u64,
        bytes: &[u8],
        admission: &impl crate::parse::limits::Admission,
    ) -> Result<(), Error> {
        if let Some(models) = &mut self.model_refs {
            models.admit(
                ordinal,
                role,
                bytes,
                self.dc.as_ref().ok_or(Error::Corrupt("missing DC head"))?,
                admission,
            )?;
        }
        Ok(())
    }

    #[cfg(feature = "v3")]
    pub(crate) fn check_first_use_prefix(&self, role: u64, bytes: &[u8]) -> Result<(), Error> {
        let mut r = ByteReader::new(bytes);
        let (selector, _) = model_refs::prefix(&mut r, role, self.tables.mode)?;
        if !matches!(selector, model_refs::Selector::Define(_)) || r.remaining() != 0 {
            return Err(Error::Corrupt(
                "reference target is not a first-use definition",
            ));
        }
        Ok(())
    }

    #[cfg(feature = "v3")]
    pub(crate) fn model_uses_ans(&self) -> bool {
        self.tables.mode == LossyEntropy::RunLevel
    }

    #[cfg(all(feature = "lab", feature = "std"))]
    pub(crate) fn component_requires_reference(&self) -> bool {
        self.skip
    }

    /// Move the owned head after the caller has retained its record spans.
    /// Borrowed unit records are intentionally discarded.
    pub(crate) fn without_records(self) -> LossyParsed<'static> {
        LossyParsed {
            info: self.info,
            payload_len: self.payload_len,
            g: self.g,
            color: self.color,
            inv_scale: self.inv_scale,
            native_quantizer: self.native_quantizer,
            dq: self.dq,
            bias: self.bias,
            transform: self.transform,
            layouts: self.layouts,
            layout_steps: self.layout_steps,
            adaptive_mask: self.adaptive_mask,
            compact_states: self.compact_states,
            local_ans_contexts: self.local_ans_contexts,
            coding_defaults: self.coding_defaults,
            model_refs: self.model_refs,
            dc_coverage: self.dc_coverage,
            uniform_dc_k: self.uniform_dc_k,
            #[cfg(all(feature = "lab", test))]
            catalog_lab: self.catalog_lab,
            tables: self.tables,
            huf: self.huf,
            rans: self.rans,
            dc: self.dc.map(crate::v1::Parsed::without_records),
            aq: self.aq,
            xfer16: self.xfer16,
            alpha: self.alpha.map(crate::v1::Parsed::without_records),
            gcols: self.gcols,
            skip: self.skip,
            hint: self.hint,
            reserve: self.reserve,
            unit_ordinals: self.unit_ordinals,
            stripes: Vec::new(),
            exact: self.exact.map(exact::Exact::without_records),
            sb: self.sb,
            map: self.map,
            small: self.small,
            small_int: self.small_int,
        }
    }

    pub(crate) fn needs_reference(&self) -> bool {
        self.skip
    }

    /// A unit record's selector map and the rest of its bytes. With
    /// superblocks or small transforms the record starts with its partition
    /// map instead, which this skips (layouts are the defaults then:
    /// [`recon_unit`] reads it where the unit is reconstructed).
    fn record<'a>(
        &self,
        g: &Geom,
        s: usize,
        bytes: &'a [u8],
    ) -> Result<(layout::ModeMap<'a>, &'a [u8]), Error> {
        if self.coding_defaults.is_some() {
            return Ok((
                layout::ModeMap::Constant(0),
                coding_defaults::Defaults::split(bytes)?.1,
            ));
        }
        if self.map != MapKind::None {
            return Ok((layout::ModeMap::Constant(0), superblock::skip_map(bytes)?));
        }
        let (a, b) = g.rows(s);
        layout::read_modes(bytes, (b - a) * g.bw, self.adaptive_mask)
    }
    // The rANS modes always carry their tables (`parse_lossy`).
    #[cfg(any(
        all(target_arch = "x86_64", feature = "avx512"),
        all(feature = "lab", feature = "std")
    ))]
    fn dec(&self) -> &[u32; coef::MAX_TABLES << LOG_M] {
        &self.rans.as_ref().expect("rANS tables").dec
    }
    #[cfg(any(
        all(target_arch = "x86_64", feature = "avx512"),
        all(feature = "lab", feature = "std")
    ))]
    fn cmap(&self) -> &[u8; coef::NRAW_PAD] {
        &self.rans.as_ref().expect("rANS tables").cmap
    }
    #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
    fn cmap32(&self) -> &[u32; coef::NRAW_PAD] {
        &self.rans.as_ref().expect("rANS tables").cmap32
    }
}

/// Tests and lab tools: the payload of a lossy file, parsed under its
/// header's color and every feature this decoder knows.
#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
pub(crate) fn parse_lossy(file: &[u8]) -> Result<LossyParsed<'_>, Error> {
    let (info, color, body, _) =
        crate::v1::lossy_body(file)?.ok_or(Error::InvalidInput("not a lossy file"))?;
    let version = crate::parse::header::read_header(&mut ByteReader::new(file))?.version;
    parse_lossy_as(
        info,
        version,
        color,
        crate::parse::header::Features::ALL,
        body,
    )
}

/// Parses a lossy `DATA` payload of a file whose header has internal color
/// `color` and declares the features `lossy` (seed tables need
/// `seed_bank_1`, exact blocks `exact_blocks`).
pub(crate) fn parse_lossy_as<'a>(
    info: ImageInfo,
    version: u8,
    color: LossyColor,
    lossy: crate::parse::header::Features,
    body: &'a [u8],
) -> Result<LossyParsed<'a>, Error> {
    parse_lossy_checked(
        info,
        version,
        color,
        lossy,
        body,
        &crate::parse::limits::Unrestricted,
    )
}

pub(crate) fn parse_lossy_checked<'a>(
    info: ImageInfo,
    version: u8,
    color: LossyColor,
    lossy: crate::parse::header::Features,
    body: &'a [u8],
    admission: &impl crate::parse::limits::Admission,
) -> Result<LossyParsed<'a>, Error> {
    let parsed = parse_lossy_prefix_checked(info, version, color, lossy, body, admission)?;
    if parsed.payload_len != body.len() {
        return Err(Error::Corrupt("trailing DATA bytes"));
    }
    Ok(parsed)
}

fn parse_lossy_prefix_checked<'a>(
    info: ImageInfo,
    version: u8,
    color: LossyColor,
    lossy: crate::parse::header::Features,
    body: &'a [u8],
    admission: &impl crate::parse::limits::Admission,
) -> Result<LossyParsed<'a>, Error> {
    if version == 4 {
        return Err(Error::Unsupported(
            "V4 requires its declared payload driver",
        ));
    }
    let mut r = ByteReader::new(body);
    let mut p = parse_lossy_head_checked(info, color, lossy, &mut r, admission)?;
    // Records in decode order (see `encode_lossy`, SPEC 2.7), checked
    // against the hint.
    use crate::parse::records::{R_AC, R_ALPHA, R_DC, R_EXACT};
    let ns = p.g.stripes();
    let ng = p.ngroups();
    let map = p.unit_map();
    let units = ns.checked_mul(ng).ok_or(Error::TooLarge)?;
    if units > r.remaining() / 2 {
        return Err(Error::Corrupt("lossy unit count exceeds remaining records"));
    }
    p.stripes
        .try_reserve_exact(units)
        .map_err(|_| Error::TooLarge)?;
    let mut records = crate::parse::records::Split::with_ordinals(
        &mut r,
        version,
        p.hint.take(),
        units,
        p.unit_ordinals,
    )?;
    let mut ordinal = 0u64;
    for u in 0..ns * ng {
        admission.check()?;
        let si = u / ng;
        if map.starts_dc_at(si, u % ng) {
            let bytes = records.next(R_DC)?;
            if let Some(models) = &mut p.model_refs {
                models.admit(ordinal, 0, bytes, p.dc.as_ref().unwrap(), admission)?;
            }
            p.dc.as_mut().unwrap().stripes.push(bytes);
            ordinal += 1;
        }
        if let Some(a) = &mut p.alpha {
            a.stripes.push(records.next(R_ALPHA)?);
            ordinal += 1;
        }
        let bytes = records.next(R_AC)?;
        let (_, gg) = p.group(u % ng);
        let model_bytes = p.record(&gg, si, bytes)?.1;
        if let Some(models) = &mut p.model_refs {
            models.admit(ordinal, 1, model_bytes, p.dc.as_ref().unwrap(), admission)?;
        }
        p.stripes.push(bytes);
        ordinal += 1;
        if let Some(e) = &mut p.exact {
            e.push_record(records.next(R_EXACT)?)?;
            ordinal += 1;
        }
    }
    records.finish()?;
    if let Some(e) = &p.exact {
        e.finish()?;
    }
    p.payload_len = r.pos;
    Ok(p)
}

impl LossyParsed<'_> {
    /// Lossy stripes (units per column group).
    pub(crate) fn stripe_count(&self) -> usize {
        self.g.stripes()
    }

    /// Where each unit's records sit (SPEC 2.7).
    pub(crate) fn unit_map(&self) -> crate::parse::records::UnitMap {
        crate::parse::records::UnitMap::lossy(
            self.ngroups(),
            self.dc_k(),
            self.alpha.is_some(),
            self.exact.is_some(),
        )
        .with_coverage(self.dc_coverage.clone())
    }

    /// The payload codes some blocks exactly (lossy head record 5).
    #[cfg(feature = "v3")]
    pub(crate) fn has_exact_blocks(&self) -> bool {
        self.exact.is_some()
    }

    /// The exact blocks' head: every unit's list entries and every nested
    /// stripe's place (region decoding).
    #[cfg(feature = "v3")]
    pub(crate) fn exact(&self) -> Option<&exact::Exact<'_>> {
        self.exact.as_ref()
    }

    /// Lossy stripes per DC stripe.
    pub(crate) fn dc_k(&self) -> usize {
        self.uniform_dc_k
    }

    pub(crate) fn dc_span(&self, s: usize, g: usize) -> dc_coverage::Span {
        self.dc_coverage.as_ref().map_or_else(
            || {
                let k = self.dc_k();
                dc_coverage::Span {
                    start: s / k * k,
                    end: ((s / k + 1) * k).min(self.g.stripes()),
                    unit: s / k * self.ngroups() + g,
                }
            },
            |c| c.span(s, g),
        )
    }

    /// Column groups per stripe.
    pub(crate) fn ngroups(&self) -> usize {
        if self.gcols == 0 {
            1
        } else {
            self.g.w.div_ceil(self.gcols)
        }
    }

    /// First pixel column and geometry of column group `gi` (a group
    /// decodes like an image as wide as the group).
    fn group(&self, gi: usize) -> (usize, Geom) {
        if self.gcols == 0 {
            return (
                0,
                Geom::new(self.g.w as u32, self.g.h as u32, self.g.sb as u32),
            );
        }
        let x0 = gi * self.gcols;
        let gw = self.gcols.min(self.g.w - x0);
        (x0, Geom::new(gw as u32, self.g.h as u32, self.g.sb as u32))
    }
}

/// Unit geometry of a lossy payload, for region decoding (`crate::region`).
#[cfg(any(feature = "v3", feature = "lab"))]
pub(crate) struct RegionGeometry {
    /// Pixel rows per lossy stripe.
    pub rows: usize,
    pub stripes: usize,
    pub groups: usize,
    /// Column group width in pixels (0 = none).
    pub gcols: usize,
    /// Lossy stripes per DC stripe.
    pub k: usize,
    pub alpha: bool,
}

impl<'a> LossyParsed<'a> {
    #[cfg(any(feature = "v3", feature = "lab"))]
    pub(crate) fn region_geometry(&self) -> RegionGeometry {
        RegionGeometry {
            rows: self.g.sb * 8,
            stripes: self.g.stripes(),
            groups: self.ngroups(),
            gcols: self.gcols,
            k: self.dc_k(),
            alpha: self.alpha.is_some(),
        }
    }
}

/// Region decoding (`crate::region`): the working memory one work item of
/// column groups `g0..g1` holds while it decodes (the widest group counts):
/// one stripe's coefficients ([`crate::decoder_memory::lossy_stripe_bytes`],
/// the estimate the decode planner charges), reconstruction scratch, its
/// staged RGB and alpha rows, its DC rows and values, the prediction
/// state of the nested DC and alpha units it decodes, and with exact blocks
/// a decoded nested stripe and its decoder's state
/// ([`exact::Exact::item_bytes`]).
#[cfg(feature = "v3")]
pub(crate) fn region_unit_bytes(p: &LossyParsed<'_>, g0: usize, g1: usize) -> Result<u64, Error> {
    let dcp = p.dc.as_ref().ok_or(Error::Corrupt("no DC image"))?;
    let (sb, rows) = (p.g.sb as u128, (p.g.sb * 8) as u128);
    let bps = p.info.sample.bytes() as u128;
    let mut most = 0u128;
    for g in g0..g1 {
        let gw = p.group(g).1.w as u128;
        let mut unit = crate::decoder_memory::lossy_stripe_bytes(gw, sb)?
            + decoder_recon_bytes()
            + gw * rows * 4 * bps
            + gw.div_ceil(8) * sb * (dcp.bpp() as u128 + 16)
            + u128::from(crate::v1::region_unit_bytes(dcp, g, g + 1)?);
        if let Some(a) = &p.alpha {
            unit += u128::from(crate::v1::region_unit_bytes(a, g, g + 1)?);
        }
        if let Some(e) = &p.exact {
            unit += e.item_bytes(gw.div_ceil(8) * sb)?;
        }
        most = most.max(unit);
    }
    if p.has_model_references() {
        most += 2 * decoder_table_bytes() + crate::decoder_memory::lossless_tables(4, 32, true);
    }
    u64::try_from(most).map_err(|_| Error::TooLarge)
}

/// Region decoding (`crate::region`): decodes lossy stripes `s0..s1` of
/// column groups `g0..g1` into `out` (rows of `stride` bytes; row 0 is
/// stripe `s0`'s first pixel row, column 0 is group `g0`'s first column).
/// `dc[(d - s0 / k) * gn + g - g0]` holds the DC record of DC stripe `d`,
/// group `g` (gn = `g1 - g0`); `units[(s - s0) * gn + g - g0]` the alpha
/// (empty without alpha) and lossy records of unit (`s`, `g`); `stripes`
/// the encoded nested exact-block stripes those units need, by index. Each work
/// item decodes the region's stripes of one DC stripe and group, decoding
/// that DC stripe's rows from its top once.
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "v3")]
pub(crate) fn decode_lossy_region(
    p: &LossyParsed<'_>,
    (s0, s1, g0, g1): (usize, usize, usize, usize),
    dc: &[(usize, &[u8])],
    units: &[(&[u8], &[u8])],
    stripes: &[(usize, &[u8])],
    out: &mut [u8],
    stride: usize,
    threads: usize,
    obs: &crate::region::RegionObs<'_>,
) -> Result<(), Error> {
    // crate::region refuses keep blocks at open.
    debug_assert!(!p.skip);
    let rows = p.g.sb * 8;
    let (y0, y1) = (s0 * rows, (s1 * rows).min(p.g.h));
    let ng = p.ngroups();
    let gn = g1 - g0;
    let pxb = p.info.row_bytes() / p.g.w;
    let box_w: usize = (g0..g1).map(|g| p.group(g).1.w).sum();
    if stride < box_w * pxb || out.len() < (y1 - y0 - 1) * stride + box_w * pxb {
        return Err(Error::InvalidInput("region buffer too small"));
    }
    let dcp = p.dc.as_ref().ok_or(Error::Corrupt("no DC image"))?;
    let alpha_img = p
        .alpha
        .as_ref()
        .map(|ap| (ap, crate::v1::OutMap::new(&ap.info, &ap.layout)));
    // Exact blocks: the nested image's output map; each work item decodes
    // the stripes its units read, once.
    let exact_om = p
        .exact
        .as_ref()
        .map(|e| crate::v1::OutMap::new(&e.img.info, &e.img.layout));
    let no_check = || Ok(());
    let rgb = p.info.channels == Channels::Rgb;
    let bps = p.info.sample.bytes();
    let mut unit_rows: Vec<Vec<&mut [u8]>> = (0..units.len()).map(|_| Vec::new()).collect();
    for (i, row) in out.chunks_mut(stride).take(y1 - y0).enumerate() {
        let s = (y0 + i) / rows;
        let mut rest = &mut row[..box_w * pxb];
        for gi in g0..g1 {
            let (a, b) = rest.split_at_mut(p.group(gi).1.w * pxb);
            unit_rows[(s - s0) * gn + gi - g0].push(a);
            rest = b;
        }
    }
    // Work items: (group, the region's stripes of one DC stripe).
    let mut items: Vec<(usize, Vec<(usize, Vec<&mut [u8]>)>)> = Vec::new();
    for gi in g0..g1 {
        for s in s0..s1 {
            let r = core::mem::take(&mut unit_rows[(s - s0) * gn + gi - g0]);
            match items.last_mut() {
                Some((g, v)) if *g == gi && p.dc_span(v[0].0, gi).unit == p.dc_span(s, gi).unit => {
                    v.push((s, r))
                }
                _ => items.push((gi, alloc::vec![(s, r)])),
            }
        }
    }
    let decode_item = |_, (gi, group): (usize, Vec<(usize, Vec<&mut [u8]>)>)| {
        let mut sc = Scratch::default();
        let mut exact_st = exact::ItemState::default();
        let gg = p.group(gi).1;
        let span = p.dc_span(group[0].0, gi);
        let dr = gg.bw * dcp.bpp();
        let ordinal = p.unit_map().first(span.start * ng + gi);
        let at = dc
            .binary_search_by_key(&ordinal, |&(id, _)| id)
            .map_err(|_| Error::Corrupt("missing region DC dependency"))?;
        let (model, body) = if let Some(models) = &p.model_refs {
            models.dc(dc[at].1, dcp)?
        } else {
            (None, dc[at].1)
        };
        let dcp = model.as_deref().unwrap_or(dcp);
        let dc_om = crate::v1::OutMap::new(&dcp.info, &dcp.layout);
        let mut dc_rows = crate::v1::UnitRows::new_with(dcp, span.unit, body)?;
        let mut next = 0;
        for (s, mut rws) in group {
            obs.begin(s, gi)?;
            let nb = p.g.sb.min(p.g.bh - s * p.g.sb);
            let first = (s - span.start) * p.g.sb;
            resize_fallible(&mut sc.dc_px, nb * dr, 0)?;
            // Rows above the stripe (a region starting inside a DC stripe)
            // are decoded into row 0 and overwritten.
            while next < first + nb {
                let r = next.saturating_sub(first);
                dc_rows.next_row(dcp, &dc_om, &mut sc.dc_px[r * dr..][..dr])?;
                next += 1;
            }
            let (mut dcv, mut aqv) = (core::mem::take(&mut sc.dc), core::mem::take(&mut sc.aq));
            dcv.clear();
            aqv.clear();
            dc_values(
                dcp.info.sample,
                p.aq,
                p.exact.is_some(),
                &sc.dc_px[..nb * dr],
                &mut dcv,
                &mut aqv,
            )?;
            let (alpha_bytes, ac) = units[(s - s0) * gn + gi - g0];
            let q = core::mem::take(&mut sc.q);
            let eobs = core::mem::take(&mut sc.eobs);
            let raw = decode_stripe_raw_bytes(p, &gg, s, ac, Group::ALL, true, q, eobs)?;
            finish_coefs(p, &gg, s, &dcv, raw, &mut sc.q, &mut sc.eobs)?;
            let Scratch {
                q,
                eobs,
                planes,
                lin,
                alpha,
                tmp,
                roles,
                splits,
                ..
            } = &mut sc;
            let ws = (planes, lin, roles, splits);
            let exact =
                |rows: &mut [&mut [u8]], st: &mut exact::ItemState| match (&p.exact, &exact_om) {
                    (Some(e), Some(om)) => {
                        let nested = exact::Nested::Bytes { om, stripes };
                        let range = e.range(s * ng + gi)?;
                        exact::apply_unit(e, nested, st, range, s, &gg, &aqv, rows, &no_check)
                    }
                    _ => Ok(()),
                };
            if rgb {
                recon_unit::<false>(p, &gg, s, ac, &aqv, &mut rws, true, q, eobs, ws)?;
                exact(rws.as_mut_slice(), &mut exact_st)?;
            } else {
                let n = rws.len();
                let crb = gg.w * 3 * bps;
                let arb = gg.w * bps;
                resize_fallible(tmp, n * crb, 0)?;
                let mut trows: Vec<&mut [u8]> = tmp.chunks_mut(crb).collect();
                recon_unit::<false>(p, &gg, s, ac, &aqv, &mut trows, true, q, eobs, ws)?;
                exact(trows.as_mut_slice(), &mut exact_st)?;
                let a = match &alpha_img {
                    Some((ap, om)) => {
                        resize_fallible(alpha, n * arb, 0)?;
                        let mut st = crate::v1::UnitRows::new_with(ap, s * ng + gi, alpha_bytes)?;
                        for r in 0..n {
                            st.next_row(ap, om, &mut alpha[r * arb..][..arb])?;
                        }
                        Some(&alpha[..n * arb])
                    }
                    None => None,
                };
                pack_rows(&p.info, gg.w, tmp, a, &mut rws);
            }
            (sc.dc, sc.aq) = (dcv, aqv);
            obs.end(s, gi, &Ok(()));
        }
        Ok::<(), Error>(())
    };
    for r in crate::par::par_map(items, threads, decode_item) {
        r?;
    }
    Ok(())
}

/// The head of a lossy payload (everything before the records): no
/// stripes yet, nested images with their heads only. `color` is the
/// header's internal color (SPEC 2.8: the payload does not repeat it);
/// `admission` charges the parsed geometry and models (`Unrestricted`
/// compiles away); `declared` is the header's feature mask.
pub(crate) fn parse_lossy_head_checked<'a, I: crate::parse::payload::HeadInput>(
    info: ImageInfo,
    color: LossyColor,
    declared: crate::parse::header::Features,
    r: &mut I,
    admission: &impl crate::parse::limits::Admission,
) -> Result<LossyParsed<'a>, Error> {
    crate::parse::payload::ready(parse_lossy_head_async(
        info,
        color,
        declared,
        &mut crate::parse::payload::ReadyInput(r),
        admission,
    ))
}

pub(crate) async fn parse_lossy_head_async<'a, I: crate::parse::payload::AsyncHeadInput>(
    info: ImageInfo,
    color: LossyColor,
    declared: crate::parse::header::Features,
    r: &mut I,
    admission: &impl crate::parse::limits::Admission,
) -> Result<LossyParsed<'a>, Error> {
    #[cfg(all(feature = "lab", feature = "std"))]
    use crate::trace::describe as d;
    use crate::trace::{tf, tfd};
    admission.model()?;
    let kind =
        InputKind::of(&info).ok_or(Error::Corrupt("lossy stream with unsupported pixel format"))?;
    if kind == InputKind::U32 && !cfg!(feature = "extended") && admission.decodes() {
        return Err(Error::Unsupported("32-bit samples (extended tier)"));
    }
    let xfer = transfer::Transfer::from_cicp(info.cicp.transfer_characteristics);
    let ok = match kind {
        InputKind::U8 => true,
        InputKind::U16 => color == LossyColor::YCoCg || xfer.is_some(),
        InputKind::U32 => color == LossyColor::YCoCg || xfer.is_some(),
        InputKind::F16 | InputKind::F32 => color == LossyColor::Xyb,
    };
    if !ok
        || ((info.has_custom_precision() || kind == InputKind::U32)
            && (!info.cicp.full_range || info.cicp.matrix_coefficients != 0))
        || (precision::custom_color(&info) && color == LossyColor::Xyb && xfer.is_none())
    {
        return Err(Error::Corrupt(
            "lossy color does not match the sample type or range",
        ));
    }
    let sb = tfd!(
        "stripe height",
        r,
        r.u8().await?,
        |v: &u8| d::stripe_blocks(*v)
    );
    if sb == 0 {
        return Err(Error::Corrupt("bad stripe height"));
    }
    let gcols = tfd!("column group width", r, r.varint().await?, |v: &u64| {
        if *v == 0 {
            "0 = none: one group spans the width".into()
        } else {
            alloc::format!("{v} pixels per column group")
        }
    });
    if gcols != 0 && (gcols % 8 != 0 || gcols >= info.width as u64) {
        return Err(Error::Corrupt("bad column group width"));
    }
    let gcols = gcols as usize;
    let params = tfd!(
        "parameters",
        r,
        envelope::read_async(r, color, kind, declared, admission.decodes()).await?,
        |e: &envelope::Params| {
            alloc::format!(
                "plane scales {:?}; quant scale {}, floor {}, plane multipliers {:?}, base matrix {}, \
             DC step x{:?}/64; entropy {:?}; compact restart states {}; seed selection {:?}",
                e.scale,
                e.qp.scale,
                e.qp.floor,
                e.qp.mul,
                e.qp.matrix,
                e.qp.dc,
                e.entropy,
                e.compact,
                e.seed
            )
        }
    );
    let envelope::Params {
        scale,
        mut qp,
        entropy,
        compact: compact_states,
        seed: seed_selection,
        records,
    } = params;
    if !scale.iter().all(|s| s.is_finite() && *s > 1e-6 && *s < 1e9) {
        return Err(Error::Corrupt("bad color scale"));
    }
    if !qp.valid() {
        return Err(Error::Corrupt("bad quantization parameters"));
    }
    // 16-bit XYB: normative linear -> code table for the image's transfer.
    let xfer16 = match (kind, color, xfer) {
        (InputKind::U16, LossyColor::Xyb, Some(t)) => Some((t.table(), t.linear_max_f32())),
        (InputKind::U32, LossyColor::Xyb, Some(t)) => Some((t.table(), t.linear_max_f32())),
        (InputKind::U8, LossyColor::Xyb, Some(t)) if precision::custom_color(&info) => {
            Some((t.table(), t.linear_max_f32()))
        }
        _ => None,
    };
    let tl = tf!("AC table bytes", r, r.var_usize().await?);
    let tb = crate::trace::tfba!("AC entropy tables (bit-packed)", r, tl);
    let scoped = |bit: u8| seed_selection.scoped(bit);
    let mut table_bits = BitReader::new(tb);
    let (tables, huf) = if entropy == LossyEntropy::SplitHuf {
        let ht = huf::HufTables::read(&mut table_bits, scoped(crate::seeds::SCOPE_AC))?;
        let d = ht.dec();
        let rows = huf::WidRows::new(&ht);
        (Tables::placeholder(entropy), Some((ht, d, rows)))
    } else {
        (
            Tables::read(&mut table_bits, entropy, scoped(crate::seeds::SCOPE_AC))?,
            None,
        )
    };
    // Head records (SPEC 2.8): the unit table hint, the options of the
    // DC image (exact blocks, AQ, keep blocks, 32-bit samples), the Lossy
    // 0.4 dequantization records and the channel layouts.
    let mut hint_rec: Option<Vec<u8>> = None;
    // The reservation (head record 12): its bytes, kept like the hint's,
    // and where its payload sits.
    let mut reserve_rec: Option<(Vec<u8>, u64)> = None;
    let (mut exact_on, mut aq_on, mut skip, mut dc32) = (false, false, false, false);
    let mut sb_on = false;
    let mut parts_on = false;
    let mut unit_ordinals = false;
    let mut local_ans_contexts = false;
    let mut coding_defaults = None;
    let mut dc_coverage_rec = None;
    let mut model_refs_rec = None;
    #[cfg(all(feature = "lab", test))]
    let mut catalog_lab = None;
    #[cfg(feature = "lab")]
    let mut small_int = false;
    #[cfg(not(feature = "lab"))]
    let small_int = false;
    let mut layouts = [LossyChannelLayout::default(); 3];
    let mut layouts_rec = false;
    if records {
        use crate::parse::header::Feature;
        let n = tf!("head record bytes", r, r.var_usize().await?);
        let list_at = r.pos();
        let list = r.bytes(n).await?;
        #[cfg(all(feature = "lab", feature = "std"))]
        crate::trace::head_records(list, true);
        crate::parse::records::head_records_at(list, |id, rec, at| {
            // Options with no payload.
            let flag = |on: &mut bool| {
                if !rec.is_empty() {
                    return Err(Error::Corrupt("lossy head record length"));
                }
                *on = true;
                Ok(())
            };
            match id {
                #[cfg(all(feature = "lab", test))]
                model_candidates::HEAD => {
                    if local_ans_contexts || compact_states {
                        return Err(Error::Corrupt("conflicting lab model declarations"));
                    }
                    catalog_lab = Some(model_candidates::Catalog::read(
                        rec,
                        entropy,
                        scoped(crate::seeds::SCOPE_AC),
                        admission,
                    )?);
                    Ok(())
                }
                model_refs::HEAD => {
                    if !declared.allows(Feature::ModelReferences) {
                        return Err(Error::Corrupt("model references without feature"));
                    }
                    if local_ans_contexts || compact_states {
                        return Err(Error::Corrupt("conflicting model declarations"));
                    }
                    admission.hint(rec.len())?;
                    admission.wire_extra_bytes(rec.len() as u128)?;
                    let mut saved = Vec::new();
                    saved
                        .try_reserve_exact(rec.len())
                        .map_err(|_| Error::TooLarge)?;
                    saved.extend_from_slice(rec);
                    model_refs_rec = Some(saved);
                    Ok(())
                }
                dc_coverage::HEAD => {
                    if !declared.allows(Feature::DcCoverage) {
                        return Err(Error::Corrupt("DC coverage without feature"));
                    }
                    admission.hint(rec.len())?;
                    admission.wire_extra_bytes(rec.len() as u128)?;
                    let mut saved = Vec::new();
                    saved
                        .try_reserve_exact(rec.len())
                        .map_err(|_| Error::TooLarge)?;
                    saved.extend_from_slice(rec);
                    dc_coverage_rec = Some(saved);
                    Ok(())
                }
                coding_defaults::HEAD => {
                    if !declared.allows(Feature::CodingDefaults) {
                        return Err(Error::Corrupt("coding defaults without feature"));
                    }
                    coding_defaults = Some(coding_defaults::Defaults::read(rec, admission)?);
                    Ok(())
                }
                local_ans::HEAD => {
                    if !declared.allows(Feature::LocalAnsContexts) {
                        return Err(Error::Corrupt("local ANS context without its feature"));
                    }
                    if entropy != LossyEntropy::RunLevel || compact_states {
                        return Err(Error::Corrupt(
                            "local ANS needs RunLevel and per-unit restart states",
                        ));
                    }
                    flag(&mut local_ans_contexts)
                }
                crate::parse::records::H_UNIT_TABLE => {
                    // Its entry count needs the DC image's stripes: kept
                    // (charged) and decoded after the nested heads.
                    admission.hint(rec.len())?;
                    let mut b = Vec::new();
                    b.try_reserve_exact(rec.len())
                        .map_err(|_| Error::TooLarge)?;
                    b.extend_from_slice(rec);
                    hint_rec = Some(b);
                    Ok(())
                }
                crate::parse::records::H_UNIT_RESERVE => {
                    if hint_rec.is_some() {
                        return Err(Error::Corrupt("unit table twice"));
                    }
                    admission.hint(rec.len())?;
                    let mut b = Vec::new();
                    b.try_reserve_exact(rec.len())
                        .map_err(|_| Error::TooLarge)?;
                    b.extend_from_slice(rec);
                    reserve_rec = Some((b, list_at + at as u64));
                    Ok(())
                }
                H_EXACT if !declared.allows(Feature::ExactBlocks) => Err(Error::Corrupt(
                    "exact blocks without the feature exact_blocks",
                )),
                H_EXACT => flag(&mut exact_on),
                H_AQ => flag(&mut aq_on),
                H_KEEP => flag(&mut skip),
                // Gated syntax (SPEC 0.1): the i32 DC image needs
                // `samples_32`, declared or implied by a 32-bit sample type.
                H_DC32 if !declared.allows(Feature::Samples32) => {
                    Err(Error::Corrupt("32-bit DC without the feature samples_32"))
                }
                H_DC32 => flag(&mut dc32),
                H_EXPLICIT | H_BIAS | H_TRANSFORM => {
                    let f = match id {
                        H_EXPLICIT => Feature::ExplicitTables,
                        H_BIAS => Feature::ReconBias,
                        _ => Feature::ColorTransform,
                    };
                    // Gated syntax: a record in a file without its
                    // feature is Corrupt (a decoder without the feature
                    // already failed at the header). A structure parse
                    // (writers) skips the payload; a decoding parse reads
                    // it.
                    if !declared.allows(f) {
                        return Err(Error::Corrupt("lossy head record without its feature"));
                    }
                    if !admission.decodes() {
                        return Ok(());
                    }
                    let mut rr = ByteReader::new(rec);
                    match id {
                        H_EXPLICIT => {
                            if qp.matrix != MATRIX_EXPLICIT || qp.explicit.is_some() {
                                return Err(Error::Corrupt("explicit tables without matrix 63"));
                            }
                            qp.explicit = Some(envelope::read_explicit(&mut rr)?);
                        }
                        H_BIAS => {
                            if qp.bias.is_some() {
                                return Err(Error::Corrupt("repeated reconstruction bias"));
                            }
                            qp.bias = Some(envelope::read_bias(&mut rr)?);
                        }
                        _ => {
                            if qp.transform.is_some() {
                                return Err(Error::Corrupt("repeated color transform"));
                            }
                            qp.transform = Some(envelope::read_transform(&mut rr)?);
                        }
                    }
                    if rr.remaining() != 0 {
                        return Err(Error::Corrupt("lossy head record length"));
                    }
                    Ok(())
                }
                H_LAYOUTS if !declared.allows(Feature::ChannelLayouts) => Err(Error::Corrupt(
                    "channel layouts without the feature channel_layouts",
                )),
                H_LAYOUTS => {
                    if rec.len() != 3 {
                        return Err(Error::Corrupt("lossy head record length"));
                    }
                    for (l, &b) in layouts.iter_mut().zip(rec) {
                        *l = LossyChannelLayout::from_byte(b)?;
                    }
                    layouts_rec = true;
                    Ok(())
                }
                superblock::H_SUPERBLOCKS if !declared.allows(Feature::Superblocks) => Err(
                    Error::Corrupt("superblocks without the feature superblocks"),
                ),
                superblock::H_SUPERBLOCKS => flag(&mut sb_on),
                small::H_PARTITIONS if !declared.allows(Feature::SmallTransforms) => Err(
                    Error::Corrupt("small transforms without the feature small_transforms"),
                ),
                // LAB: a one-byte payload 1 selects the integer candidate
                // inverse (`small_lab.rs`); production decoders take only
                // the empty record.
                #[cfg(feature = "lab")]
                small::H_PARTITIONS if rec == [1] => {
                    small_int = true;
                    parts_on = true;
                    Ok(())
                }
                small::H_PARTITIONS => flag(&mut parts_on),
                crate::parse::records::H_UNIT_ORDINALS
                    if !declared.allows(Feature::UnitOrdinals) =>
                {
                    Err(Error::Corrupt(
                        "unit ordinals without the feature unit_ordinals",
                    ))
                }
                crate::parse::records::H_UNIT_ORDINALS => flag(&mut unit_ordinals),
                _ if id & 1 == 0 => Ok(()),
                _ => Err(Error::Unsupported("unknown lossy head record")),
            }
        })?;
    }
    // Lossy 0.4: matrix 63 carries its tables in record 13, and only the
    // YCoCg code-value family may carry a color transform (U8 samples only).
    if qp.matrix == MATRIX_EXPLICIT && qp.explicit.is_none() {
        return Err(Error::Corrupt("explicit tables without their record"));
    }
    if qp.transform.is_some() && color != LossyColor::YCoCg {
        return Err(Error::Corrupt("color transform outside the YCoCg family"));
    }
    if qp.transform.is_some() && (kind != InputKind::U8 || info.has_custom_precision()) {
        return Err(Error::Corrupt("color transform needs U8 samples"));
    }
    // The records carry values (explicit tables) the first check could not
    // see; the step bound must hold for the tables actually used.
    if !qp.valid() {
        return Err(Error::Corrupt("bad quantization parameters"));
    }
    admission.lossy(&info, sb, gcols, color, &layouts)?;
    if local_ans_contexts {
        let groups = if gcols == 0 {
            1
        } else {
            u128::from(info.width).div_ceil(gcols as u128)
        };
        let units = u128::from(info.height).div_ceil(8).div_ceil(u128::from(sb)) * groups;
        admission.local_ans(units)?;
    }
    let steps = qp.steps();
    let dq = [0, 1, 2].map(|c| dct::dequant_factors(&steps[c]));
    let g = Geom::new(info.width, info.height, sb as u32);
    if skip && info.channels != Channels::Rgb {
        return Err(Error::Corrupt("keep blocks outside RGB"));
    }
    // Merged blocks need the default 8x8 layouts and still frames: record 21
    // with record 9 or with record 27 is `Corrupt`, whatever the layout bytes
    // (SPEC 2.9). (Entropy coding is 0 or 3 here: 1 and 2 are retired,
    // `Unsupported` above.)
    if sb_on && (skip || layouts_rec) {
        return Err(Error::Corrupt(
            "superblocks with keep blocks or channel layouts",
        ));
    }
    // The joint map (record 23) replaces record 21's and excludes the lab
    // channel layouts (SPEC 2.11.1); keep blocks are bypassed parents.
    if parts_on && (sb_on || layouts_rec) {
        return Err(Error::Corrupt(
            "small transforms with superblocks or channel layouts",
        ));
    }
    if coding_defaults.is_some() && (sb_on || parts_on || layouts_rec) {
        return Err(Error::Corrupt("conflicting coding default maps"));
    }
    // A fourth DC-image channel carries per-block codes: AQ step
    // multipliers a / 64, and with keep blocks, 0 = keep the reference
    // block (inter frames); with exact blocks (no AQ, no keep), 1 = exact
    // block, else 0.
    let aq = aq_on || skip || exact_on;
    let dc_info = ImageInfo::new(
        g.bw as u32,
        g.bh as u32,
        if aq { Channels::Rgba } else { Channels::Rgb },
        if dc32 {
            SampleType::I32
        } else {
            SampleType::U16
        },
    );
    // Nested images group the same columns (the DC image in blocks).
    let head = async |r: &mut I,
                      info: ImageInfo,
                      want_gcols: usize,
                      scope: u8|
           -> Result<crate::v1::Parsed<'a>, Error> {
        crate::trace::trace_enter_addr("head", "2", "nested lossless image head", r.mark());
        let n = tf!("head bytes", r, r.var_usize().await?);
        let mut hr = ByteReader::new(r.bytes(n).await?);
        // Nested heads never carry a unit table of their own (SPEC 2.8).
        let p = crate::v1::parse_head_nested(info, &mut hr, admission, scoped(scope), declared)?;
        if hr.remaining() != 0 {
            return Err(Error::Corrupt("nested image head has trailing bytes"));
        }
        crate::trace::exit_addr(r.mark());
        if p.gcols != want_gcols {
            return Err(Error::Corrupt("nested image column groups"));
        }
        Ok(p)
    };
    #[cfg(all(feature = "lab", feature = "std"))]
    if crate::trace::on() {
        let pl: [&str; 3] = match color {
            LossyColor::Xyb => ["Y", "X", "B-Y"],
            LossyColor::YCoCg => ["Y", "Co", "Cg"],
        };
        let off = if dc32 { "" } else { " +32768" };
        let mut n: Vec<alloc::string::String> =
            pl.iter().map(|p| alloc::format!("{p} DC{off}")).collect();
        n.push("AQ step a/64".into());
        crate::trace::set_names(Some(n));
    }
    if seed_selection.scope & crate::seeds::SCOPE_ALPHA != 0 && !info.channels.has_alpha() {
        return Err(Error::Corrupt("seeded alpha tables without alpha"));
    }
    let mut dc = head(r, dc_info, gcols / 8, crate::seeds::SCOPE_DC).await?;
    let dc_coverage = if let Some(rec) = dc_coverage_rec.as_deref() {
        let ng = if gcols == 0 { 1 } else { g.w.div_ceil(gcols) };
        let cover = dc_coverage::Coverage::read(rec, g.stripes(), ng, g.sb, g.bh, admission)?;
        if cover.rectangles.iter().any(|&(a, b, _)| b - a > dc.hs) {
            return Err(Error::Corrupt("DC coverage exceeds admitted rows"));
        }
        dc.unit_rectangles = Some(alloc::sync::Arc::clone(&cover.rectangles));
        Some(alloc::sync::Arc::new(cover))
    } else {
        None
    };
    #[cfg(all(feature = "lab", feature = "std"))]
    crate::trace::set_names(Some(alloc::vec!["alpha".into()]));
    admission.dc(&dc)?;
    if dc.hs % g.sb != 0 {
        return Err(Error::Corrupt(
            "DC stripes do not cover whole lossy stripes",
        ));
    }
    let alpha = if info.channels.has_alpha() {
        let ai = ImageInfo::new(info.width, info.height, Channels::Gray, info.sample)
            .with_significant_bits(&[info.channel_bits(info.channels.count() - 1).unwrap()])?;
        let a = head(r, ai, gcols, crate::seeds::SCOPE_ALPHA).await?;
        if a.hs != g.sb * 8 {
            return Err(Error::Corrupt("alpha stripes do not match"));
        }
        Some(a)
    } else {
        None
    };
    // Exact blocks: varint(len) + [varint B (exact blocks per packed row),
    // varint exact blocks, the nested RGB8 image's lossless head].
    let exact = if exact_on {
        if !matches!(info.channels, Channels::Rgb | Channels::Rgba)
            || info.sample != SampleType::U8
            || aq_on
            || skip
        {
            return Err(Error::Corrupt(
                "exact blocks need RGB(A)8 without AQ or keep blocks",
            ));
        }
        let n = r.var_usize().await?;
        let mut er = ByteReader::new(r.bytes(n).await?);
        // Units in decode order (stripe-major, then column group) and their
        // blocks.
        let gb = if gcols == 0 { g.bw } else { gcols / 8 };
        let ng = g.bw.div_ceil(gb);
        let units = g.stripes().saturating_mul(ng);
        let unit_blocks = |u: usize| {
            let (by0, by1) = g.rows(u / ng);
            let gi = u % ng;
            (by1 - by0) * (gb.min(g.bw - gi * gb))
        };
        Some(exact::Exact::parse_head(
            &mut er,
            g.bw,
            units,
            unit_blocks,
            admission,
        )?)
    } else {
        None
    };
    #[cfg(all(feature = "lab", feature = "std"))]
    crate::trace::set_names(None);
    let rans = huf.is_none().then(|| RansTables::new(&tables));
    #[cfg(all(feature = "lab", feature = "std"))]
    if crate::trace::on() {
        let planes = match color {
            LossyColor::Xyb => "Y, X, B - Y",
            LossyColor::YCoCg => "Y, Co, Cg",
        };
        crate::trace::note(
            "internal color",
            alloc::format!(
                "{color:?} planes {planes}: the {:?} {:?} input is converted to three f32 planes \
                 before the transform; the decoder converts back to {:?} {:?} on output{}",
                info.channels,
                info.sample,
                info.channels,
                info.sample,
                if xfer16.is_some() {
                    " (through the transfer table for 16-bit XYB)"
                } else {
                    ""
                }
            ),
        );
        crate::trace::note(
            "coefficients",
            alloc::format!(
                "per plane {:?}; quantized to i32 (|q| <= 2^22 - 1); stripes of {} pixel rows; \
                 entropy {entropy:?}",
                layouts.map(|l| alloc::format!("{:?} {:?}", l.transform(), l.sampling())),
                8 * sb as u32
            ),
        );
        crate::trace::note(
            "DC image",
            alloc::format!(
                "nested lossless image, {}x{} blocks, {} channels of {:?}: per-block DC of each plane{}",
                g.bw,
                g.bh,
                if aq { 4 } else { 3 },
                dc.info.sample,
                if aq {
                    ", plus the AQ step multiplier a/64"
                } else {
                    ""
                }
            ),
        );
        if let Some(a) = &alpha {
            crate::trace::note(
                "alpha",
                alloc::format!(
                    "nested lossless image, full resolution, {:?}: alpha is exact, never transformed",
                    a.info.sample
                ),
            );
        }
    }
    let model_refs = model_refs_rec
        .as_deref()
        .map(|rec| {
            model_refs::Models::read(
                rec,
                &dc,
                entropy,
                declared,
                g.stripes()
                    .checked_mul(if gcols == 0 { 1 } else { g.w.div_ceil(gcols) })
                    .ok_or(Error::TooLarge)?,
                admission,
            )
        })
        .transpose()?;
    let default_maps = coding_defaults.is_some();
    let mut p = LossyParsed {
        info,
        g,
        color,
        inv_scale: scale.map(|s| 1.0 / s),
        native_quantizer: qp.matrix == MATRIX_EXPLICIT
            && qp.scale == 1.0
            && qp.floor == 1.0
            && qp.mul == [1.0; 3]
            && qp.dc == [64; 3]
            && qp.bias.is_none()
            && qp.transform.is_none()
            && qp.explicit.is_some_and(|tables| {
                tables
                    .iter()
                    .flatten()
                    .all(|&v| (1.0..=65535.0).contains(&v) && v == v as u16 as f32)
            }),
        dq,
        bias: qp
            .bias
            .filter(|b| !b.is_zero())
            .map(|b| alloc::boxed::Box::new(b.tables())),
        transform: qp
            .transform
            .and_then(|t| t.decode())
            .map(alloc::boxed::Box::new),
        layouts,
        layout_steps: core::array::from_fn(|c| {
            [false, true].map(|split| {
                let l = layouts[c].layout(split);
                if l == subblock::Layout::FULL8 {
                    steps[c]
                } else {
                    l.steps(&steps[c])
                }
            })
        }),
        adaptive_mask: layout::adaptive_mask(&layouts),
        compact_states,
        local_ans_contexts,
        coding_defaults,
        model_refs,
        dc_coverage,
        uniform_dc_k: dc.hs / g.sb,
        #[cfg(all(feature = "lab", test))]
        catalog_lab,
        tables,
        huf,
        rans,
        dc: Some(dc),
        aq,
        xfer16,
        alpha,
        gcols,
        skip,
        hint: None,
        reserve: None,
        unit_ordinals,
        stripes: Vec::new(),
        payload_len: 0,
        exact,
        // Record 23 merges only in files that declare `superblocks`.
        sb: (sb_on
            || ((parts_on || default_maps)
                && declared.allows(crate::parse::header::Feature::Superblocks)))
        .then(|| alloc::boxed::Box::new(superblock::Tables::new(&steps))),
        map: if parts_on {
            MapKind::Joint
        } else if sb_on {
            MapKind::Merges
        } else {
            MapKind::None
        },
        small: (parts_on
            || (default_maps && declared.allows(crate::parse::header::Feature::SmallTransforms)))
        .then(|| {
            alloc::boxed::Box::new(steps.map(|t| {
                #[cfg(feature = "lab")]
                if small_int {
                    return small_lab::dequant4_int(&t);
                }
                small::dequant4(&t)
            }))
        }),
        small_int,
    };
    // One hint entry per main record.
    let records = |p: &LossyParsed<'_>| {
        p.unit_map()
            .count(p.g.stripes())
            .map_err(|_| Error::Corrupt("unit table length"))
    };
    if let Some(b) = hint_rec {
        let n = records(&p)?;
        p.hint = Some(crate::parse::records::hint_entries(&b, n)?);
    } else if let Some((b, at)) = reserve_rec {
        let n = records(&p)?;
        let (state, filled) = crate::parse::records::reserve_entries(&b, n)?;
        p.hint = filled;
        p.reserve = Some(crate::parse::records::Reserve {
            state,
            at,
            len: b.len(),
        });
    }
    Ok(p)
}

/// LAB trace: labels the records of group `gi` in stripe `s` (DC when a DC
/// stripe starts, then alpha, then AC) with their image regions.
#[cfg(all(feature = "lab", feature = "std"))]
#[allow(clippy::too_many_arguments)]
fn trace_lossy_group(
    g: &Geom,
    s: usize,
    k: usize,
    gi: usize,
    x0: usize,
    gw: usize,
    alpha: bool,
    color: LossyColor,
    aq: bool,
) {
    if !crate::trace::on() {
        return;
    }
    let planes = match color {
        LossyColor::Xyb => "Y, X, B-Y",
        LossyColor::YCoCg => "Y, Co, Cg",
    };
    let (by0, by1) = g.rows(s);
    let y0 = (by0 * 8) as u64;
    let rows = ((by1 * 8).min(g.h) - by0 * 8) as u64;
    let gw = gw.min(g.w - x0) as u64;
    let x0 = x0 as u64;
    if s.is_multiple_of(k) {
        let last = (s + k).min(g.stripes()) - 1;
        let y1 = (g.rows(last).1 * 8).min(g.h) as u64;
        crate::trace::tnext!(
            alloc::format!(
                "DC stripe {} group {gi} ({planes} DC{}, one value per 8x8 block)",
                s / k,
                if aq { ", AQ step" } else { "" }
            ),
            Some([x0, y0, gw, y1 - y0])
        );
    }
    if alpha {
        crate::trace::tnext!(
            alloc::format!("alpha stripe {s} group {gi} (alpha, exact)"),
            Some([x0, y0, gw, rows])
        );
    }
    crate::trace::tnext!(
        alloc::format!("AC stripe {s} group {gi} (planes {planes}: DCT coefficients)"),
        Some([x0, y0, gw, rows])
    );
}

/// Bytes of a lossy `DATA` payload.
pub(crate) fn payload_len_checked(
    info: &ImageInfo,
    version: u8,
    color: LossyColor,
    lossy: crate::parse::header::Features,
    body: &[u8],
    admission: &impl crate::parse::limits::Admission,
) -> Result<usize, Error> {
    Ok(
        parse_lossy_prefix_checked(info.clone(), version, color, lossy, body, admission)?
            .payload_len,
    )
}

/// Quantized DCs from DC-image sample bytes (U16 or I32).
/// Quantized DCs (and, for a 4-channel DC image, the per-block AQ step
/// multipliers `a / 64`, a clamped to 0..=255) from DC-image sample bytes.
/// 16-bit DC samples (value + 32768) -> quantized DC (no clamp needed).
#[archmage::autoversion]
fn dc_u16(px: &[u8], out: &mut [i32]) {
    for (o, c) in out.iter_mut().zip(px.as_chunks::<2>().0) {
        *o = u16::from_ne_bytes(*c) as i32 - 32768;
    }
}

fn resize_fallible<T: Clone>(v: &mut Vec<T>, n: usize, value: T) -> Result<(), Error> {
    v.try_reserve(n.saturating_sub(v.len()))
        .map_err(|_| Error::TooLarge)?;
    v.resize(n, value);
    Ok(())
}

/// DC values of nested DC rows and, with a fourth (block code) channel,
/// per-block scales: AQ multipliers code / 64 (0 also keeps the reference
/// block), or with `exact` 1.0 for code 0 and 0.0 (not reconstructed) for an
/// exact block (code 1); any other exact code is Corrupt.
fn dc_values(
    sample: SampleType,
    aq: bool,
    exact: bool,
    px: &[u8],
    out: &mut Vec<i32>,
    scales: &mut Vec<f32>,
) -> Result<(), Error> {
    if sample == SampleType::U16 && !aq {
        let start = out.len();
        resize_fallible(
            out,
            start.checked_add(px.len() / 2).ok_or(Error::TooLarge)?,
            0,
        )?;
        dc_u16(px, &mut out[start..]);
        return Ok(());
    }
    let lim = 1 << 22;
    let mut raw = Vec::new();
    raw.try_reserve_exact(px.len() / sample.bytes())
        .map_err(|_| Error::TooLarge)?;
    match sample {
        SampleType::U16 => raw.extend(
            px.as_chunks::<2>()
                .0
                .iter()
                .map(|&c| u16::from_ne_bytes(c) as i32),
        ),
        _ => raw.extend(px.as_chunks::<4>().0.iter().map(|&c| i32::from_ne_bytes(c))),
    }
    let off = if sample == SampleType::U16 { 32768 } else { 0 };
    let n = if aq { 4 } else { 3 };
    let blocks = raw.len() / n;
    out.try_reserve(blocks.checked_mul(3).ok_or(Error::TooLarge)?)
        .map_err(|_| Error::TooLarge)?;
    if aq {
        scales.try_reserve(blocks).map_err(|_| Error::TooLarge)?;
    }
    for b in raw.chunks_exact(n) {
        out.extend(b[..3].iter().map(|&v| (v - off).clamp(-lim, lim)));
        if exact {
            scales.push(match b[3] {
                0 => 1.0,
                1 => 0.0,
                _ => return Err(Error::Corrupt("exact block code")),
            });
        } else if aq {
            scales.push(b[3].clamp(0, 255) as f32 * (1.0 / 64.0));
        }
    }
    Ok(())
}

/// Split-Huffman: magnitudes and signs of every coded group, blocks in
/// order.
/// Each group's magnitudes come from one 64-bit window (len * w <= 57; wider
/// groups take a per-coefficient path) and its signs from one window of the
/// sign stream, the i-th nonzero taking bit i.
#[inline(never)]
fn split_unpack(
    mag: &[u8],
    sgn: &[u8],
    nblocks: usize,
    counts: &[u8],
    wid: &[u8],
    q: &mut [i32],
    simd: bool,
) -> Result<(), Error> {
    let nbc = nblocks * NCH;
    let (mut om, mut os) = (0u64, 0u64);
    #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
    let done = if simd
        && !crate::v1::force_scalar()
        && !crate::v1::force_avx2()
        && let Some(t) = <archmage::X64V4xToken as archmage::SimdToken>::summon()
    {
        (om, os) = split_avx512::unpack(t, mag, sgn, nbc, counts, wid, q);
        true
    } else {
        false
    };
    #[cfg(not(all(target_arch = "x86_64", feature = "avx512")))]
    let done = {
        let _ = simd;
        false
    };
    #[cfg(target_arch = "x86_64")]
    let done = done
        || (simd
            && !crate::v1::force_scalar()
            && match <archmage::X64V3Token as archmage::SimdToken>::summon() {
                Some(t) => {
                    (om, os) = split_avx2::unpack(t, mag, sgn, nbc, counts, wid, q);
                    true
                }
                None => false,
            });
    if !done {
        for (bc, qb) in q[..nbc * 64].as_chunks_mut::<64>().0.iter_mut().enumerate() {
            let n = (counts[bc] as usize).min(coef::PK_GROUPS);
            let ws: &[u8; coef::PK_GROUPS] = wid[bc * coef::PK_GROUPS..][..coef::PK_GROUPS]
                .try_into()
                .unwrap();
            (om, os) = split_unpack_bc(mag, sgn, n, ws, qb, om, os);
        }
    }
    if om > mag.len() as u64 * 8 || os > sgn.len() as u64 * 8 {
        return Err(Error::Corrupt("split bit stream overrun"));
    }
    Ok(())
}

/// One block-channel's `n` groups from bit offsets (`om`, `os`); returns
/// the offsets after it.
#[inline(always)]
pub(crate) fn split_unpack_bc(
    mag: &[u8],
    sgn: &[u8],
    n: usize,
    ws: &[u8; coef::PK_GROUPS],
    qb: &mut [i32; 64],
    mut om: u64,
    mut os: u64,
) -> (u64, u64) {
    for gi in 0..n {
        (om, os) = split_group(mag, sgn, gi, ws[gi] as u32, qb, om, os);
    }
    (om, os)
}

/// Group `gi` (width `w`) of a block-channel from bit offsets (`om`, `os`);
/// returns the offsets after it.
#[inline(always)]
pub(crate) fn split_group(
    mag: &[u8],
    sgn: &[u8],
    gi: usize,
    w: u32,
    qb: &mut [i32; 64],
    mut om: u64,
    mut os: u64,
) -> (u64, u64) {
    // Group 15 has three coefficients.
    let len = 4 - (gi == coef::PK_GROUPS - 1) as u32;
    let field = len * w;
    let mask = (1u64 << w) - 1;
    let m: [u32; 4] = if field <= 57 {
        let f = rawbits::get(mag, om, field);
        core::array::from_fn(|j| ((f >> (j as u32 * w)) & mask) as u32)
    } else {
        core::array::from_fn(|j| {
            if (j as u32) < len {
                rawbits::get(mag, om + (j as u32 * w) as u64, w) as u32
            } else {
                0
            }
        })
    };
    om += field as u64;
    let nzm = m.map(|v| (v != 0) as u32);
    let nz = nzm[0] + nzm[1] + nzm[2] + nzm[3];
    let s = rawbits::get(sgn, os, nz) as u32;
    os += nz as u64;
    let mut rank = 0;
    for j in 0..4 {
        let neg = 0u32.wrapping_sub((s >> rank) & nzm[j]);
        rank += nzm[j];
        let v = ((m[j] ^ neg).wrapping_sub(neg)) as i32;
        if (j as u32) < len {
            qb[ZIGZAG[1 + 4 * gi + j] as usize & 63] = v;
        }
    }
    (om, os)
}

#[derive(Clone, Copy, Default)]
struct Cursor {
    /// Block-row offset within the stripe and block column.
    r: u32,
    bx: u32,
    ch: u8,
    /// Next AC index (0 = expecting the EOB token).
    k: u8,
    eob: u8,
    /// Luma EOB of the current block / previous luma EOB of the lane.
    luma: u8,
    done: bool,
}

/// Scalar reference: entropy-decodes stripe `s` into quantized
/// coefficients `[(block * 3 + ch) * 64 + raster]` (DC from `dc`) plus the
/// EOB of each (block, ch). `q` has 64 and `eobs` 1 spare slots at the end
/// (write targets for inactive SIMD lanes).
#[allow(clippy::too_many_arguments)]
fn decode_stripe_coefs(
    p: &LossyParsed<'_>,
    g: &Geom,
    s: usize,
    u: usize,
    dc: &[i32],
    simd: bool,
    sc: &mut Scratch,
) -> Result<(), Error> {
    let q = core::mem::take(&mut sc.q);
    let eobs = core::mem::take(&mut sc.eobs);
    let r = decode_stripe_raw_bytes(p, g, s, p.stripes[u], Group::ALL, simd, q, eobs)?;
    finish_coefs(p, g, s, dc, r, &mut sc.q, &mut sc.eobs)
}

/// After the entropy decode of stripe `s`: clears the spare slots, checks
/// for a stream overrun and fills in the DCs.
fn finish_coefs(
    p: &LossyParsed<'_>,
    g: &Geom,
    s: usize,
    dc: &[i32],
    (mut q, eobs, wp, len): (Vec<i32>, Vec<u8>, usize, usize),
    sq: &mut Vec<i32>,
    seobs: &mut Vec<u8>,
) -> Result<(), Error> {
    let n = q.len();
    // The spare slots are never read; clear them for the next stripe.
    q[n - 64..].fill(0);
    *sq = q;
    *seobs = eobs;
    if wp > len {
        sq.fill(0);
        return Err(Error::Corrupt("lane stream overrun"));
    }
    let _ = p;
    // Empty `dc`: shared DC rows the caller fills in after their wait.
    if !dc.is_empty() {
        let (by0, by1) = g.rows(s);
        fill_dc(sq, dc, by1 - by0, g.bw);
    }
    Ok(())
}

/// Entropy-decodes stripes `s[0]` and `s[1]` into `sc.q`/`sc.eobs` and
/// `sc.q2`/`sc.eobs2`: through the two-stripe AVX-512 kernel (two
/// independent dependency chains per loop), else one stripe after the
/// other. Same coefficients either way.
#[allow(clippy::too_many_arguments)]
fn decode_stripe_coefs2(
    p: &LossyParsed<'_>,
    g: &Geom,
    s: [usize; 2],
    u: [usize; 2],
    dc: [&[i32]; 2],
    simd: bool,
    sc: &mut Scratch,
) -> Result<(), Error> {
    #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
    if simd
        && p.layouts == [LossyChannelLayout::default(); 3]
        && p.tables.mode == LossyEntropy::RunLevel
        && !crate::v1::force_scalar()
        && !crate::v1::force_avx2()
        && let Some(t) = <archmage::X64V4Token as archmage::SimdToken>::summon()
        && kernel_choice::choice().rl != kernel_choice::Rl::Avx2
    {
        let hw = kernel_choice::choice().rl == kernel_choice::Rl::Avx512Hw;
        let bw = g.bw;
        let active = g.active_lanes();
        let nby = s.map(|s| {
            let (a, b) = g.rows(s);
            b - a
        });
        // The records' bytes after their partition maps (superblocks,
        // small transforms).
        let body = |u: usize| {
            p.record(g, u / p.ngroups(), p.stripes[u])
                .map(|(_, body)| body)
        };
        let unit0 = local_ans::unit(p, body(u[0])?)?;
        let unit1 = local_ans::unit(p, body(u[1])?)?;
        let model0 = unit0
            .model()
            .and_then(|m| m.rans.as_ref())
            .or(p.rans.as_ref())
            .expect("ANS model");
        let model1 = unit1
            .model()
            .and_then(|m| m.rans.as_ref())
            .or(p.rans.as_ref())
            .expect("ANS model");
        fn maps(model: &RansTables) -> entropy_avx512::Maps<'_> {
            entropy_avx512::Maps {
                cmap: &model.cmap,
                cmap32: &model.cmap32,
                dec: &model.dec,
            }
        }
        let m0 = if unit0.model().is_none() {
            entropy_avx512::Maps {
                cmap: p.cmap(),
                cmap32: p.cmap32(),
                dec: p.dec(),
            }
        } else {
            maps(model0)
        };
        let m1 = if unit1.model().is_none() {
            entropy_avx512::Maps {
                cmap: p.cmap(),
                cmap32: p.cmap32(),
                dec: p.dec(),
            }
        } else {
            maps(model1)
        };
        let w0 = unit0.words(active)?;
        let w1 = unit1.words(active)?;
        let mut q = [core::mem::take(&mut sc.q), core::mem::take(&mut sc.q2)];
        let mut e = [
            core::mem::take(&mut sc.eobs),
            core::mem::take(&mut sc.eobs2),
        ];
        for k in 0..2 {
            // Arrive all zero (any length); size to the stripe plus spares.
            q[k].resize(nby[k] * bw * NCH * 64 + 64, 0);
            e[k].clear();
            e[k].resize(nby[k] * bw * NCH + 1, 0);
        }
        let [q0, q1] = &mut q;
        let [e0, e1] = &mut e;
        let bk = [
            eob_bucket_proxy(nby[0] * bw, bw),
            eob_bucket_proxy(nby[1] * bw, bw),
        ];
        let bkt = match &bk {
            [Some(a), Some(b)] => Some([&a[..], &b[..]]),
            _ => None,
        };
        let wp = if core::ptr::eq(model0, model1) {
            entropy_avx512::decode_stripes2(
                t,
                hw,
                [&w0.0, &w1.0],
                [w0.1, w1.1],
                &m0,
                bw,
                nby,
                active,
                0,
                NCH as u32,
                [&mut q0[..], &mut q1[..]],
                [&mut e0[..], &mut e1[..]],
                bkt,
            )
        } else {
            entropy_avx512::decode_stripes2_models(
                t,
                hw,
                [&w0.0, &w1.0],
                [w0.1, w1.1],
                [&m0, &m1],
                bw,
                nby,
                active,
                0,
                NCH as u32,
                [&mut q0[..], &mut q1[..]],
                [&mut e0[..], &mut e1[..]],
                bkt,
            )
        };
        let [q0, q1] = q;
        let [e0, e1] = e;
        let r0 = finish_coefs(
            p,
            g,
            s[0],
            dc[0],
            (q0, e0, wp[0], w0.0.len),
            &mut sc.q,
            &mut sc.eobs,
        );
        let r1 = finish_coefs(
            p,
            g,
            s[1],
            dc[1],
            (q1, e1, wp[1], w1.0.len),
            &mut sc.q2,
            &mut sc.eobs2,
        );
        return r0.and(r1);
    }
    decode_stripe_coefs(p, g, s[0], u[0], dc[0], simd, sc)?;
    core::mem::swap(&mut sc.q, &mut sc.q2);
    core::mem::swap(&mut sc.eobs, &mut sc.eobs2);
    let r = decode_stripe_coefs(p, g, s[1], u[1], dc[1], simd, sc);
    core::mem::swap(&mut sc.q, &mut sc.q2);
    core::mem::swap(&mut sc.eobs, &mut sc.eobs2);
    r
}

/// Per-thread stripe buffers. Invariant between stripes: `q` is all zero
/// (reconstruction zeroes each block after reading it).
#[derive(Default)]
pub(crate) struct Scratch {
    q: Vec<i32>,
    eobs: Vec<u8>,
    /// Second stripe of a pair (two-stripe entropy decode).
    q2: Vec<i32>,
    eobs2: Vec<u8>,
    dc2: Vec<i32>,
    aq2: Vec<f32>,
    /// Color planes of one block row (fully overwritten per row).
    planes: Vec<f32>,
    /// One row of linear output (float / 16-bit XYB paths).
    lin: Vec<f32>,
    /// Nested DC / alpha stripe pixels and the stripe's DC values.
    dc_px: Vec<u8>,
    dc: Vec<i32>,
    aq: Vec<f32>,
    alpha: Vec<u8>,
    /// RGB rows of a stripe for gray / alpha layouts.
    tmp: Vec<u8>,
    /// Superblocks: a unit's merge roles.
    roles: Vec<u8>,
    /// Small transforms: a unit's 4x4 plane bits per block.
    splits: Vec<u8>,
}

/// Entropy decode of stripe `s` without the DC or the overrun check:
/// (coefficients, EOBs, words consumed, words available).
#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
#[allow(clippy::type_complexity)]
fn decode_stripe_raw(
    p: &LossyParsed<'_>,
    s: usize,
    grp: Group,
    simd: bool,
    q: Vec<i32>,
    eobs: Vec<u8>,
) -> Result<(Vec<i32>, Vec<u8>, usize, usize), Error> {
    decode_stripe_raw_bytes(p, &p.g, s, p.stripes[s], grp, simd, q, eobs)
}

#[allow(clippy::type_complexity)]
fn decode_stripe_raw_bytes(
    p: &LossyParsed<'_>,
    g: &Geom,
    s: usize,
    bytes: &[u8],
    grp: Group,
    simd: bool,
    q: Vec<i32>,
    eobs: Vec<u8>,
) -> Result<(Vec<i32>, Vec<u8>, usize, usize), Error> {
    let (modes, bytes) = p.record(g, s, bytes)?;
    let result = decode_stripe_raw_inner(p, g, s, bytes, grp, simd, q, eobs)?;
    // Sampled transforms use a strict compact prefix. Reject hidden values in
    // discarded slots instead of permitting multiple unchecked representations.
    if p.layouts
        .iter()
        .any(|l| l.sampling() != ChannelSampling::Full)
    {
        let (a, b) = g.rows(s);
        for block in 0..(b - a) * g.bw {
            for ch in 0..grp.nch {
                let channel = grp.ch0 + ch;
                let l = p.layouts[channel].layout(modes.get(block) & (1 << channel) != 0);
                for &i in &ZIGZAG[l.samples()..] {
                    if result.0[(block * grp.nch + ch) * 64 + i as usize] != 0 {
                        return Err(Error::Corrupt("nonzero unused sampled coefficient"));
                    }
                }
            }
        }
    }
    Ok(result)
}

#[allow(clippy::type_complexity)]
fn decode_stripe_raw_inner(
    p: &LossyParsed<'_>,
    g: &Geom,
    s: usize,
    bytes: &[u8],
    grp: Group,
    simd: bool,
    q: Vec<i32>,
    eobs: Vec<u8>,
) -> Result<(Vec<i32>, Vec<u8>, usize, usize), Error> {
    let unit = local_ans::unit(p, bytes)?;
    decode_coefficient_unit(
        g,
        s,
        &unit,
        AcModelView {
            tables: &p.tables,
            rans: p.rans.as_ref(),
            huf: p.huf.as_ref(),
        },
        grp,
        simd,
        q,
        eobs,
    )
}

/// Borrowed probability tables, independent of the surrounding wire grammar.
struct AcModelView<'a> {
    tables: &'a Tables,
    rans: Option<&'a RansTables>,
    huf: Option<&'a (huf::HufTables, huf::Dec, huf::WidRows)>,
}

#[allow(clippy::type_complexity)]
fn decode_coefficient_unit(
    g: &Geom,
    s: usize,
    unit: &local_ans::Unit<'_>,
    root: AcModelView<'_>,
    grp: Group,
    simd: bool,
    mut q: Vec<i32>,
    mut eobs: Vec<u8>,
) -> Result<(Vec<i32>, Vec<u8>, usize, usize), Error> {
    let (by0, by1) = g.rows(s);
    let nby = by1 - by0;
    let bw = g.bw;
    // `q` arrives all zero (any length); size it to this stripe plus the
    // 64 spare slots.
    let nch = grp.nch;
    let (ch0, nchu) = (grp.ch0 as u32, nch as u32);
    q.resize(nby * bw * nch * 64 + 64, 0);
    eobs.clear();
    eobs.resize(nby * bw * nch + 1, 0);
    let active = g.active_lanes();
    let model = unit.model();
    let chosen_huf = model.map_or(root.huf, |m| m.huf.as_ref());
    let chosen_tables = model.map_or(root.tables, |m| &m.tables);
    let chosen_rans = model.map_or(root.rans, |m| m.rans.as_ref());
    let bytes = unit.body;
    if let Some((ht, d, rows)) = chosen_huf {
        let _ = (ch0, nchu, active);
        let t = prof::start();
        let mut r = ByteReader::new(bytes);
        let mut sec = [&[][..]; 3];
        for v in sec.iter_mut() {
            let n = r.var_usize()?;
            *v = r.bytes(n)?;
        }
        let n = r.var_usize()?;
        let mag = r.bytes(n)?;
        let sgn = r.bytes(r.remaining())?;
        let nb = nby * bw;
        let mut wid = Vec::new();
        resize_fallible(&mut wid, nb * NCH * coef::PK_GROUPS + coef::PK_GROUPS, 0)?;
        let mut hs = huf::Scratch::default();
        huf::decode_side(ht, d, rows, sec, nb, &mut eobs, &mut wid, &mut hs)?;
        t.add(4);
        let t = prof::start();
        split_unpack(mag, sgn, nb, &eobs, &wid, &mut q, simd)?;
        t.add(5);
        return Ok((q, eobs, 0, 0));
    }
    let tables = chosen_tables;
    let rans = chosen_rans.expect("ANS tables");
    let (words, mut x) = unit.words(active)?;
    #[cfg(target_arch = "x86_64")]
    if simd && !crate::v1::force_scalar() {
        use archmage::SimdToken;
        if !crate::v1::force_avx2()
            && let Some(t) = archmage::X64V4Token::summon()
            && kernel_choice::choice().rl != kernel_choice::Rl::Avx2
        {
            let m = entropy_avx512::Maps {
                cmap: &rans.cmap,
                cmap32: &rans.cmap32,
                dec: &rans.dec,
            };
            let hw = kernel_choice::choice().rl == kernel_choice::Rl::Avx512Hw;
            let bk = eob_bucket_proxy(nby * bw, bw);
            let wp = entropy_avx512::decode_stripe(
                t,
                hw,
                &words,
                x,
                &m,
                bw,
                nby,
                active,
                ch0,
                nchu,
                &mut q,
                &mut eobs,
                bk.as_deref(),
            );
            return Ok((q, eobs, wp, words.len));
        }
        if let Some(t) = archmage::X64V3Token::summon() {
            let wp = entropy_avx2::decode_stripe(
                t, &words, x, &rans.cmap, &rans.dec, bw, nby, active, ch0, nchu, &mut q, &mut eobs,
            );
            return Ok((q, eobs, wp, words.len));
        }
    }
    #[cfg(target_arch = "aarch64")]
    if simd && !crate::v1::force_scalar() {
        use archmage::SimdToken;
        if let Some(t) = archmage::NeonToken::summon() {
            let bk = eob_bucket_proxy(nby * bw, bw);
            let wp = entropy_neon::decode_stripe(
                t,
                &words,
                x,
                &rans.cmap,
                &rans.dec,
                bw,
                nby,
                active,
                ch0,
                nchu,
                &mut q,
                &mut eobs,
                bk.as_deref(),
            );
            return Ok((q, eobs, wp, words.len));
        }
    }
    #[cfg(target_arch = "wasm32")]
    if simd && !crate::v1::force_scalar() {
        let wp = entropy_portable::decode_stripe(
            &words, x, &rans.cmap, &rans.dec, bw, nby, active, ch0, nchu, &mut q, &mut eobs,
        );
        return Ok((q, eobs, wp, words.len));
    }
    let _ = simd;
    let mut wp = 0usize;
    let mut cur = [Cursor::default(); NL];
    for (l, c) in cur.iter_mut().enumerate() {
        c.bx = l as u32;
        c.done = l >= active || nby == 0;
    }
    let renorm = |x: &mut u32, wp: &mut usize| {
        if *x < RANS_L {
            *x = (*x << 16) | words.word(*wp) as u32;
            *wp += 1;
        }
    };
    let mut sym = [0u32; NL];
    let mut nbits = [0u32; NL];
    let mut live = [false; NL];
    loop {
        let mut any = false;
        // Token phase.
        for l in 0..NL {
            live[l] = !cur[l].done;
            if !live[l] {
                continue;
            }
            any = true;
            let c = cur[l];
            let ch = c.ch as usize;
            let raw = if c.k == 0 {
                eob_ctx(grp.ch0 + ch, c.luma as u32)
            } else {
                ac_ctx(grp.ch0 + ch, c.k as u32, c.eob as u32)
            };
            let t = tables.ctx_map[raw] as usize;
            let xs = x[l];
            let e = rans.dec[(t << LOG_M) | (xs & (M - 1)) as usize];
            let mut nx = ((e & 1023) + 1) * (xs >> LOG_M) + ((e >> 10) & 1023);
            renorm(&mut nx, &mut wp);
            x[l] = nx;
            sym[l] = (e >> 20) & 127;
            nbits[l] = e >> 27;
        }
        if !any {
            break;
        }
        // Extra bits: low part (<= 16), then high part.
        let mut extra = [0u32; NL];
        for l in 0..NL {
            if live[l] && nbits[l] > 0 {
                let n = nbits[l].min(16);
                extra[l] = x[l] & crate::mask_of(n);
                x[l] >>= n;
                renorm(&mut x[l], &mut wp);
            }
        }
        for l in 0..NL {
            if live[l] && nbits[l] > 16 {
                let n = nbits[l] - 16;
                extra[l] |= (x[l] & crate::mask_of(n)) << 16;
                x[l] >>= n;
                renorm(&mut x[l], &mut wp);
            }
        }
        // Apply.
        for l in 0..NL {
            if !live[l] {
                continue;
            }
            let c = &mut cur[l];
            let b = c.r as usize * bw + c.bx as usize;
            let ch = c.ch as usize;
            let mut next_channel = false;
            if c.k == 0 {
                c.eob = sym[l] as u8;
                eobs[b * nch + ch] = c.eob;
                if ch == 0 {
                    c.luma = c.eob;
                }
                if c.eob == 0 {
                    next_channel = true;
                } else {
                    c.k = 1;
                }
            } else if sym[l] == coef::ZRL {
                let k = c.k as u32 + 4;
                if k > c.eob as u32 {
                    next_channel = true;
                } else {
                    c.k = k as u8;
                }
            } else {
                let pos = c.k as u32 + (sym[l] >> 5);
                let e = c.eob as u32;
                if pos <= e {
                    q[(b * nch + ch) * 64 + ZIGZAG[pos as usize] as usize] =
                        rl_value(sym[l] & 31, extra[l]);
                }
                if pos >= e {
                    next_channel = true;
                } else {
                    c.k = (pos + 1) as u8;
                }
            }
            if next_channel {
                c.k = 0;
                c.ch += 1;
                if c.ch as usize == nch {
                    c.ch = 0;
                    c.bx += NL as u32;
                    if c.bx as usize >= bw {
                        c.bx = l as u32;
                        c.r += 1;
                        if c.r as usize == nby {
                            c.done = true;
                        }
                    }
                }
            }
        }
        // Past the end of the stream the decode is already invalid.
        if wp > words.len {
            break;
        }
    }
    Ok((q, eobs, wp, words.len))
}

/// [`aq_dequant`] without the copy when the block has no multiplier or
/// multiplier 1 (identical factors: x * 1.0 == x): `dq` itself, else the
/// scaled factors in `buf`.
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[inline(always)]
pub(crate) fn aq_dequant_in<'a>(
    dq: &'a [f32; 64],
    s: Option<f32>,
    buf: &'a mut [f32; 64],
) -> &'a [f32; 64] {
    match s {
        Some(s) if s != 1.0 => {
            *buf = aq_dequant(dq, Some(s));
            buf
        }
        _ => dq,
    }
}

/// Normative AQ dequantization factors: AC factors times the block's
/// multiplier, the DC factor unchanged.
pub(crate) fn aq_dequant(dq: &[f32; 64], s: Option<f32>) -> [f32; 64] {
    let mut d = *dq;
    if let Some(s) = s {
        for v in &mut d[1..] {
            *v *= s;
        }
    }
    d
}

/// Copies the stripe's DCs (`dc`: the stripe's rows of the DC image) into
/// `q`.
fn fill_dc(q: &mut [i32], dc: &[i32], nby: usize, bw: usize) {
    for r in 0..nby {
        for bx in 0..bw {
            for ch in 0..NCH {
                q[((r * bw + bx) * NCH + ch) * 64] = dc[(r * bw + bx) * NCH + ch];
            }
        }
    }
}

/// Reconstructs stripe `s` from its record `bytes` into `out` (its first row is the stripe's first
/// pixel row), with the SIMD kernels or the scalar reference. `dc` holds
/// the stripe's rows of the DC image.
#[allow(clippy::too_many_arguments)]
fn decode_stripe_bytes_mode<const BOUNDED: bool>(
    p: &LossyParsed<'_>,
    g: &Geom,
    s: usize,
    bytes: &[u8],
    dc: &[i32],
    aq: &[f32],
    out: &mut [u8],
    stride: usize,
    simd: bool,
    sc: &mut Scratch,
) -> Result<(), Error> {
    let t = prof::start();
    let q = core::mem::take(&mut sc.q);
    let eobs = core::mem::take(&mut sc.eobs);
    let r = decode_stripe_raw_bytes(p, g, s, bytes, Group::ALL, simd, q, eobs)?;
    finish_coefs(p, g, s, dc, r, &mut sc.q, &mut sc.eobs)?;
    t.add(1);
    let Scratch {
        q,
        eobs,
        planes,
        lin,
        roles,
        splits,
        ..
    } = sc;
    let (by0, by1) = g.rows(s);
    let n = (by1 * 8).min(g.h) - by0 * 8;
    let mut rows: Vec<&mut [u8]> = Vec::new();
    rows.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
    rows.extend(out.chunks_mut(stride).take(n));
    let ws = (planes, lin, roles, splits);
    recon_unit::<BOUNDED>(p, g, s, bytes, aq, &mut rows, simd, q, eobs, ws)
}

/// Reconstructs stripe `s` (geometry `g`) from its unit record `bytes` and
/// decoded coefficients into `out`: the 8x8 path, with superblocks the
/// merged traversal when the unit's partition map merges anything, with
/// small transforms the 4x4 planes its map splits. A corrupt map or merge
/// clears `q` (the scratch invariant) before it returns the error.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn recon_unit<const BOUNDED: bool>(
    p: &LossyParsed<'_>,
    g: &Geom,
    s: usize,
    bytes: &[u8],
    aq: &[f32],
    out: &mut [&mut [u8]],
    simd: bool,
    q: &mut [i32],
    eobs: &[u8],
    (planes, lin, roles, splits): (&mut Vec<f32>, &mut Vec<f32>, &mut Vec<u8>, &mut Vec<u8>),
) -> Result<(), Error> {
    let render = p.reconstruction();
    let (a, b) = g.rows(s);
    if let Some(defaults) = &p.coding_defaults {
        let mut scales = Vec::new();
        let (merged, split) =
            defaults.maps(p, bytes, b - a, g.bw, aq, roles, splits, &mut scales)?;
        let small = simd::Small {
            split: if split { splits } else { &[] },
            dq: p.small.as_deref().unwrap_or(&p.dq),
            int: false,
        };
        if merged {
            superblock::recon_stripe::<BOUNDED>(
                &render,
                p.sb.as_deref().expect("declared merges"),
                g,
                s,
                &scales,
                roles,
                out,
                simd,
                q,
                eobs,
                planes,
                lin,
                small,
            );
        } else {
            recon_stripe_mode::<BOUNDED>(
                &render,
                g,
                s,
                &scales,
                layout::ModeMap::Constant(0),
                out,
                simd,
                q,
                eobs,
                planes,
                lin,
                small,
            );
        }
        return Ok(());
    }
    match p.map {
        MapKind::None => {}
        MapKind::Merges => {
            let t = p.sb.as_deref().expect("superblock tables");
            let merged = superblock::read_map(bytes, b - a, g.bw, roles)
                .and_then(|(m, _)| {
                    if m {
                        superblock::validate(roles, aq, g.bw, code0_skips(p))?;
                    }
                    Ok(m)
                })
                .inspect_err(|_| q.fill(0))?;
            if merged {
                superblock::recon_stripe::<BOUNDED>(
                    &render,
                    t,
                    g,
                    s,
                    aq,
                    roles,
                    out,
                    simd,
                    q,
                    eobs,
                    planes,
                    lin,
                    simd::Small::NONE,
                );
                return Ok(());
            }
        }
        MapKind::Joint => {
            let skip = code0_skips(p);
            let (merged, split) = superblock::read_joint_map(
                bytes,
                (b - a, g.bw),
                (aq, skip),
                (q, eobs),
                roles,
                splits,
            )
            .and_then(|(m, sp, _)| {
                if m {
                    // Merges in a joint map are gated syntax of
                    // `superblocks` (SPEC 2.11.1).
                    if p.sb.is_none() {
                        return Err(Error::Corrupt(
                            "partition map merges without the feature superblocks",
                        ));
                    }
                    superblock::validate(roles, aq, g.bw, skip)?;
                }
                Ok((m, sp))
            })
            .inspect_err(|_| q.fill(0))?;
            let small = simd::Small {
                split: if split { &splits[..] } else { &[] },
                dq: p.small.as_deref().expect("small-transform tables"),
                int: p.small_int,
            };
            if merged {
                let t = p.sb.as_deref().expect("superblock tables");
                superblock::recon_stripe::<BOUNDED>(
                    &render, t, g, s, aq, roles, out, simd, q, eobs, planes, lin, small,
                );
                return Ok(());
            }
            if split {
                recon_stripe_mode::<BOUNDED>(
                    &render,
                    g,
                    s,
                    aq,
                    layout::ModeMap::Constant(0),
                    out,
                    simd,
                    q,
                    eobs,
                    planes,
                    lin,
                    small,
                );
                return Ok(());
            }
        }
    }
    let modes = p.record(g, s, bytes)?.0;
    recon_stripe_mode::<BOUNDED>(
        &render,
        g,
        s,
        aq,
        modes,
        out,
        simd,
        q,
        eobs,
        planes,
        lin,
        simd::Small::NONE,
    );
    Ok(())
}

/// Reconstructs stripe `s` from its decoded coefficients (`q`, zeroed as
/// read when `p.clear_q` is true, and `eobs`) into `out`.
#[allow(clippy::too_many_arguments)]
fn recon_stripe_mode<const BOUNDED: bool>(
    p: &Reconstruction<'_>,
    g: &Geom,
    s: usize,
    aq: &[f32],
    modes: layout::ModeMap<'_>,
    out: &mut [&mut [u8]],
    simd: bool,
    q: &mut [i32],
    eobs: &[u8],
    planes: &mut Vec<f32>,
    lin: &mut Vec<f32>,
    small: simd::Small<'_>,
) {
    let (by0, by1) = g.rows(s);
    // Each block row is reconstructed and color converted in chunks of
    // `cw` blocks, so the planes chunk written by the IDCT is still in L1
    // when the color kernel reads it.
    let cw = if BOUNDED {
        RECON_CHUNK
    } else {
        recon_chunk_blocks()
    }
    .min(g.bw);
    let pw = cw * 8;
    // Plane row stride: padded by 64 bytes so the 8 rows a block writes do
    // not share their low 12 address bits (4K aliasing) for widths whose
    // rows are a multiple of 4 KiB.
    let ps = pw + 16;
    planes.resize(NCH * 8 * ps, 0.0);
    let float_out =
        matches!(p.info.sample, SampleType::F16 | SampleType::F32) || p.xfer16.is_some();
    lin.resize(if float_out { pw * 3 } else { 0 }, 0.0);
    let kind = InputKind::of(p.info).unwrap();
    let wide = kind == InputKind::U16;
    let xyb16 = p.xfer16.as_ref();
    let bps = p.info.sample.bytes();
    for r in 0..by1 - by0 {
        // Per-block AQ step multipliers of this block row (empty: none).
        let aqr = if aq.is_empty() {
            &[][..]
        } else {
            &aq[r * g.bw..(r + 1) * g.bw]
        };
        let y0 = (by0 + r) * 8;
        let rows = 8.min(g.h - y0);
        let skip = p.code0_skips;
        for (bx0, bx1) in coded_runs(skip, aqr, g.bw, cw) {
            let t = prof::start();
            if skip && p.clear_q {
                // Kept blocks: clear their coefficients (left all zero for
                // the next stripe).
                let end = runs_end(bx1, aqr, g.bw);
                for bx in bx1..end {
                    let b = r * g.bw + bx;
                    q[b * NCH * 64..(b + 1) * NCH * 64].fill(0);
                }
            }
            // Pixel columns x0..x0 + n of this chunk.
            let x0 = bx0 * 8;
            let n = (bx1 * 8).min(g.w) - x0;
            let rb = n * 3 * bps;
            let Err(t) = flat_run(
                p,
                g,
                by0,
                r,
                (bx0, bx1),
                q,
                eobs,
                out,
                lin,
                (kind, wide, xyb16, simd),
                t,
            ) else {
                continue;
            };
            if simd && p.layouts == [LossyChannelLayout::default(); 3] && small.split.is_empty() {
                simd::recon_block_row(
                    q,
                    eobs,
                    r,
                    g.bw,
                    bx0,
                    bx1,
                    p.dq,
                    aqr,
                    p.bias.as_deref(),
                    planes,
                    ps,
                    simd::Small::NONE,
                    p.clear_q,
                );
            } else if p.layouts == [LossyChannelLayout::default(); 3] {
                // 4x4 planes (small transforms), or the scalar route of 8x8
                // blocks: the reference.
                superblock::recon8(
                    p,
                    q,
                    eobs,
                    r,
                    g.bw,
                    (bx0, bx1),
                    aqr,
                    planes,
                    0,
                    ps,
                    simd,
                    small,
                );
            } else {
                for bx in bx0..bx1 {
                    let b = r * g.bw + bx;
                    for ch in 0..NCH {
                        let mut blk = [0.0f32; 64];
                        let qq = &mut q[(b * NCH + ch) * 64..][..64];
                        let l = p.layouts[ch].layout(modes.get(b) & (1 << ch) != 0);
                        if l == subblock::Layout::FULL8 {
                            let dqs = aq_dequant(&p.dq[ch], aqr.get(bx).copied());
                            match &p.bias {
                                // Lossy 0.4: (q - sign(q) * B) * step.
                                Some(bt) => {
                                    for i in 0..64 {
                                        let s = qq[i].clamp(-1, 1);
                                        let b = bt[ch][(qq[i] != s) as usize][i];
                                        blk[i] = (qq[i] as f32 - s as f32 * b) * dqs[i];
                                    }
                                }
                                None => {
                                    for i in 0..64 {
                                        blk[i] = qq[i] as f32 * dqs[i];
                                    }
                                }
                            }
                            dct::idct8x8(&mut blk);
                        } else {
                            let split = (modes.get(b) & (1 << ch) != 0) as usize;
                            let steps = &p.layout_steps[ch][split];
                            let aq = aqr.get(bx).copied().unwrap_or(1.0);
                            match &p.bias {
                                // Lossy 0.4: (q - sign(q) * B) * step.
                                Some(bt) => {
                                    for i in 0..64 {
                                        let s = qq[i].clamp(-1, 1);
                                        let b = bt[ch][(qq[i] != s) as usize][i];
                                        blk[i] = (qq[i] as f32 - s as f32 * b)
                                            * (steps[i] * if i == 0 { 1.0 } else { aq });
                                    }
                                }
                                None => {
                                    for i in 0..64 {
                                        blk[i] = qq[i] as f32
                                            * (steps[i] * if i == 0 { 1.0 } else { aq });
                                    }
                                }
                            }
                            blk = l.inverse(&blk);
                        }
                        qq.fill(0);
                        for y in 0..8 {
                            planes[(ch * 8 + y) * ps + (bx - bx0) * 8..][..8]
                                .copy_from_slice(&blk[y * 8..y * 8 + 8]);
                        }
                    }
                }
            }
            t.add(2);
            let t = prof::start();
            for y in 0..rows {
                let row = &mut out[y0 - by0 * 8 + y][x0 * 3 * bps..][..rb];
                let pl = |c: usize| &planes[(c * 8 + y) * ps..][..pw];
                color_chunk_row(p, kind, wide, xyb16, simd, pl(0), pl(1), pl(2), lin, row, n);
            }
            t.add(3);
        }
    }
}

/// The flat fast path of the reconstruction of run `bx0..bx1` of block row
/// `r` (stripe-local; the stripe starts at block row `by0`). When every
/// visible block of the run has `eobs == 0` in all three channels, each
/// channel plane is a per-block constant `q[DC] * dq[0]` (AQ never scales
/// DC), so every output pixel of an 8px group, in every row, is identical:
/// compute one pixel per group, expand its bytes into the run's first output
/// row, then copy it to the other rows; recon and the planes roundtrip are
/// skipped entirely. Only a fully flat run earns this: partial masks
/// fragment the wide color calls into narrow tails, which costs more than
/// the saved math (+1..3% Ir measured). The scan exits on the first
/// non-flat block. Chunks wider than 64 blocks take the ordinary path, as
/// in the qualified timing (2026-10-06). Returns the stage timer when the
/// run is not flat.
#[inline(always)]
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn flat_run(
    p: &Reconstruction<'_>,
    g: &Geom,
    by0: usize,
    r: usize,
    (bx0, bx1): (usize, usize),
    q: &mut [i32],
    eobs: &[u8],
    out: &mut [&mut [u8]],
    lin: &mut [f32],
    (kind, wide, xyb16, simd): (InputKind, bool, Option<&(Vec<f32>, f32)>, bool),
    t: prof::T,
) -> Result<(), prof::T> {
    let y0 = (by0 + r) * 8;
    let rows = 8.min(g.h - y0);
    let bps = p.info.sample.bytes();
    // Pixel columns x0..x0 + n of this chunk.
    let x0 = bx0 * 8;
    let n = (bx1 * 8).min(g.w) - x0;
    let rb = n * 3 * bps;
    let bpp = 3 * bps;
    let ng = n.div_ceil(8);
    let flat = p.layouts == [LossyChannelLayout::default(); 3]
        && ng <= 64
        && (bx0..bx0 + ng).all(|bx| eobs[(r * g.bw + bx) * NCH..][..NCH].iter().all(|&v| v == 0));
    if !flat {
        return Err(t);
    }
    // Expand each group's pixel into the first output row of the block
    // row; remaining rows copy it verbatim.
    let mut px = [0u8; 12];
    let r0 = y0 - by0 * 8;
    for i in 0..ng {
        let b = r * g.bw + bx0 + i;
        // Plane slices padded to a full vector: the kernels' tail path loads
        // a whole 8-wide chunk for `n < 8`.
        let (x, y, z) = (
            [q[b * NCH * 64] as f32 * p.dq[0][0]; 8],
            [q[(b * NCH + 1) * 64] as f32 * p.dq[1][0]; 8],
            [q[(b * NCH + 2) * 64] as f32 * p.dq[2][0]; 8],
        );
        color_chunk_row(
            p,
            kind,
            wide,
            xyb16,
            simd,
            &x,
            &y,
            &z,
            lin,
            &mut px[..bpp],
            1,
        );
        for j in 0..(n - i * 8).min(8) {
            out[r0][x0 * bpp + (i * 8 + j) * bpp..][..bpp].copy_from_slice(&px[..bpp]);
        }
    }
    if p.clear_q {
        // Keep the coefficient-zero invariant: recon clears each DC it
        // consumed.
        for bx in bx0..bx1 {
            let b = r * g.bw + bx;
            for ch in 0..NCH {
                q[(b * NCH + ch) * 64] = 0;
            }
        }
    }
    t.add(2);
    let t = prof::start();
    for y in 1..rows {
        let (pre, post) = out.split_at_mut(r0 + y);
        post[0][x0 * bpp..][..rb].copy_from_slice(&pre[r0][x0 * bpp..][..rb]);
    }
    t.add(3);
    Ok(())
}

/// Runs [a, b) of at most `cw` coded blocks of a block row: chunks of `cw`
/// blocks, or with `skip` the maximal runs of blocks whose code is not 0
/// (split at `cw`). The first run's leading kept blocks and each run's
/// trailing ones are cleared by the caller ([`runs_end`]).
/// Block scale 0 skips reconstruction: keep blocks (code 0, inter frames)
/// or exact blocks (code 1, written after the stripe).
fn code0_skips(p: &LossyParsed<'_>) -> bool {
    p.skip || p.exact.is_some()
}

fn coded_runs(skip: bool, aqr: &[f32], bw: usize, cw: usize) -> Vec<(usize, usize)> {
    let mut v = Vec::new();
    if !skip {
        for a in (0..bw).step_by(cw) {
            v.push((a, (a + cw).min(bw)));
        }
        return v;
    }
    let mut bx = 0;
    while bx < bw {
        if aqr[bx] == 0.0 {
            bx += 1;
            continue;
        }
        let a = bx;
        while bx < bw && aqr[bx] != 0.0 && bx - a < cw {
            bx += 1;
        }
        v.push((a, bx));
    }
    if v.is_empty() || v[0].0 > 0 {
        // Leading kept blocks: an empty run at 0 clears them.
        v.insert(0, (0, 0));
    }
    v
}

/// End of the kept blocks after a run ending at `b`.
fn runs_end(b: usize, aqr: &[f32], bw: usize) -> usize {
    let mut e = b;
    while e < bw && aqr[e] == 0.0 {
        e += 1;
    }
    e
}

/// Lossy stripes per work item when one DC stripe spans `k > 1` of them:
/// all `k` (the DC stripe is decoded once) unless that leaves fewer than
/// about three items per thread, e.g. short wide images whose whole height
/// is one DC stripe; then the groups are split into smaller items, each
/// decoding its DC stripe again (a DC stripe is ~1/64 of its lossy stripes'
/// pixels). Zen 4, 12 threads: 32768x64 2.9-3.8x, 1440x900 and 11520x900
/// +16-17%, tall images unchanged (benchmarks/lossy_dc_split_zen4_2026-10-03.md).
fn split_dc_groups(nstripes: usize, k: usize, threads: usize) -> usize {
    if k <= 1 || threads <= 1 {
        return k.max(1);
    }
    #[cfg(any(all(test, feature = "encode"), feature = "lab"))]
    match DC_SPLIT_LAB.load(core::sync::atomic::Ordering::Relaxed) {
        0 => {}
        n => return n,
    }
    if nstripes.div_ceil(k) >= 2 * threads {
        return k;
    }
    // Target items: ~3 per thread when there are enough stripes, else two
    // stripes per item (pairs share the DC decode and use the two-stripe
    // kernel), else single stripes. Each DC group is divided evenly.
    let target = if nstripes >= 6 * threads {
        nstripes / (3 * threads)
    } else if nstripes >= 2 * threads {
        2
    } else {
        1
    }
    .clamp(1, k);
    let per = k.div_ceil(k.div_ceil(target));
    if per >= 2 {
        (per + (per & 1)).min(k)
    } else {
        1
    }
}

#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
static DC_SPLIT_LAB: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// DC values and AQ scales of one lossy stripe's rows of a nested DC stripe.
#[cfg(feature = "std")]
type DcChunk = (Vec<i32>, Vec<f32>);

/// One lossy stripe's place in a shared DC stripe.
#[cfg(feature = "std")]
enum DcSlot {
    Pending,
    Ready(DcChunk),
    Taken,
}

/// A nested DC unit (DC stripe, column group) shared by the work items of its
/// lossy stripes: decoded once, top to bottom, by the first worker that needs
/// it; each lossy stripe's rows are published as soon as they are decoded and
/// taken (moved out) by the one item that decodes that stripe.
#[cfg(feature = "std")]
struct DcUnit {
    state: std::sync::Mutex<DcUnitState>,
    ready: std::sync::Condvar,
}

#[cfg(feature = "std")]
struct DcUnitState {
    claimed: bool,
    slots: Vec<DcSlot>,
    failed: Option<Error>,
}

/// Every DC unit of one threaded decode. Without sharing, a DC stripe split
/// over several work items (`split_dc_groups`) is decoded again by each
/// item, which made many-thread decodes of tall DC stripes up to 6x slower
/// (benchmarks/lossy_stripe_decode_2026-10-07). Shared, the items no longer
/// follow DC groups and the DC decode overlaps the lossy stripes below it.
#[cfg(feature = "std")]
struct SharedDc {
    units: Vec<DcUnit>,
}

#[cfg(feature = "std")]
impl SharedDc {
    /// `units` DC units of up to `k` lossy stripes each (`k` capped at the
    /// image's lossy stripes).
    fn new(units: usize, k: usize) -> Result<Self, Error> {
        let mut v = Vec::new();
        v.try_reserve_exact(units).map_err(|_| Error::TooLarge)?;
        for _ in 0..units {
            let mut slots = Vec::new();
            slots.try_reserve_exact(k).map_err(|_| Error::TooLarge)?;
            slots.extend((0..k).map(|_| DcSlot::Pending));
            v.push(DcUnit {
                state: std::sync::Mutex::new(DcUnitState {
                    claimed: false,
                    slots,
                    failed: None,
                }),
                ready: std::sync::Condvar::new(),
            });
        }
        Ok(Self { units: v })
    }

    /// Lossy stripe `j` of unit `u`: taken when published, decoded here
    /// (`produce` decodes and publishes the whole unit) when no worker has
    /// started the unit, else waited for. A failed decode returns its error
    /// to every waiting item.
    fn take(
        &self,
        u: usize,
        j: usize,
        produce: impl FnOnce(&Self, usize) -> Result<(), Error>,
    ) -> Result<DcChunk, Error> {
        let unit = &self.units[u];
        let mut produce = Some(produce);
        let mut st = unit.state.lock().unwrap();
        loop {
            match core::mem::replace(&mut st.slots[j], DcSlot::Taken) {
                DcSlot::Ready(chunk) => return Ok(chunk),
                DcSlot::Taken => return Err(Error::Corrupt("DC stripe rows taken twice")),
                DcSlot::Pending => st.slots[j] = DcSlot::Pending,
            }
            if let Some(e) = &st.failed {
                return Err(e.clone());
            }
            if produce.is_none() {
                // Decoded here, yet this stripe's rows were not published.
                return Err(Error::Corrupt("DC stripe rows missing"));
            }
            if !st.claimed {
                st.claimed = true;
                drop(st);
                if let Some(f) = produce.take() {
                    self.run(u, f)?;
                }
                st = unit.state.lock().unwrap();
                continue;
            }
            st = unit.ready.wait(st).unwrap();
        }
    }

    /// Decodes unit `u` unless a worker has started it.
    fn produce_unit(
        &self,
        u: usize,
        produce: impl FnOnce(&Self, usize) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let mut st = self.units[u].state.lock().unwrap();
        if st.claimed {
            return Ok(());
        }
        st.claimed = true;
        drop(st);
        self.run(u, produce)
    }

    /// Runs the claimed unit's `produce`, recording its error (or panic) as
    /// the unit's failure so that no waiting item stays blocked.
    fn run(
        &self,
        u: usize,
        produce: impl FnOnce(&Self, usize) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let mut guard = FailUnit(
            &self.units[u],
            Some(Error::Corrupt("DC stripe decode panicked")),
        );
        let r = produce(self, u);
        guard.1 = r.as_ref().err().cloned();
        r
    }

    /// Publishes lossy stripe `j`'s rows of unit `u` and wakes its waiter.
    fn publish(&self, u: usize, j: usize, chunk: DcChunk) {
        let unit = &self.units[u];
        unit.state.lock().unwrap().slots[j] = DcSlot::Ready(chunk);
        unit.ready.notify_all();
    }
}

/// Records an armed error as the unit's failure on drop and wakes waiters.
#[cfg(feature = "std")]
struct FailUnit<'a>(&'a DcUnit, Option<Error>);

#[cfg(feature = "std")]
impl Drop for FailUnit<'_> {
    fn drop(&mut self) {
        if let Some(e) = self.1.take() {
            let mut st = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
            st.failed = Some(e);
            drop(st);
            self.0.ready.notify_all();
        }
    }
}

/// How unbounded std decodes schedule DC stripes that span several lossy
/// stripes: 0 = each work item decodes its DC stripe (the bounded and no_std
/// path), 1 = shared DC units, decoded by the first item that needs one,
/// 2 = shared DC units decoded by their own work items (`Item::Dc`), with
/// the lossy items entropy decoding before they wait for DC rows.
#[cfg(feature = "std")]
const SHARED_DC: u8 = 2;

#[cfg(feature = "std")]
fn shared_dc_mode() -> u8 {
    #[cfg(any(all(test, feature = "encode"), feature = "lab"))]
    return SHARED_DC_LAB.load(core::sync::atomic::Ordering::Relaxed);
    #[cfg(not(any(all(test, feature = "encode"), feature = "lab")))]
    SHARED_DC
}

#[cfg(all(feature = "std", any(all(test, feature = "encode"), feature = "lab")))]
static SHARED_DC_LAB: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(SHARED_DC);

// Decodes on this thread that shared DC units (tests check coverage).
#[cfg(all(feature = "std", all(test, feature = "encode")))]
std::thread_local! {
    pub(crate) static SHARED_DC_USED: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
}

/// LAB: DC scheduling mode (see `SHARED_DC`).
#[cfg(all(feature = "std", any(all(test, feature = "encode"), feature = "lab")))]
pub(crate) fn set_shared_dc(mode: u8) {
    SHARED_DC_LAB.store(mode, core::sync::atomic::Ordering::Relaxed);
}

/// LAB: lossy stripes per work item within a DC stripe (0 = automatic).
#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
pub(crate) fn set_dc_split(n: usize) {
    DC_SPLIT_LAB.store(n, core::sync::atomic::Ordering::Relaxed);
}

/// Single-stripe work items kept at the tail, per thread (pairs before
/// them): the pairs' interleaved entropy decode is faster per stripe, the
/// small tail items keep threads finishing together.
const TAIL_PER_THREAD: usize = 2;

#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
static PAIRS_LAB: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// LAB: stripe pairing (0 automatic, 1 always, 2 never).
#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
pub(crate) fn set_pairs(v: u8) {
    PAIRS_LAB.store(v, core::sync::atomic::Ordering::Relaxed);
}

/// Number of leading stripes decoded in pairs (rounded down to even).
fn paired_stripes(nstripes: usize, threads: usize) -> usize {
    #[cfg(any(all(test, feature = "encode"), feature = "lab"))]
    match PAIRS_LAB.load(core::sync::atomic::Ordering::Relaxed) {
        1 => return nstripes & !1,
        2 => return 0,
        _ => {}
    }
    let tail = if threads > 1 {
        TAIL_PER_THREAD * threads
    } else {
        0
    };
    nstripes.saturating_sub(tail) & !1
}

#[cfg(all(feature = "std", any(all(test, feature = "encode"), feature = "lab")))]
static KEEP_SCRATCH: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
#[cfg(all(feature = "std", any(all(test, feature = "encode"), feature = "lab")))]
static KEPT: std::sync::Mutex<Vec<Scratch>> = std::sync::Mutex::new(Vec::new());

/// LAB: keep per-thread scratch buffers across decodes.
#[cfg(all(feature = "std", feature = "lab"))]
pub(crate) fn set_keep_scratch(on: bool) {
    KEEP_SCRATCH.store(on, core::sync::atomic::Ordering::Relaxed);
    if !on {
        KEPT.lock().unwrap().clear();
    }
}

/// Default reconstruction chunk width in blocks (see `decode_stripe`).
const RECON_CHUNK: usize = 32;

#[cfg(feature = "lab")]
pub(crate) static IDCT_FMA: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
static RECON_CHUNK_LAB: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Serialize test override writers while permitting ordinary parallel readers.
/// The guard restores the default even when an assertion unwinds.
#[cfg(all(all(test, feature = "encode"), feature = "lab", feature = "std"))]
pub(crate) fn scoped_recon_chunk_override() -> impl Drop {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    struct Restore {
        _guard: std::sync::MutexGuard<'static, ()>,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            set_recon_chunk(0);
        }
    }
    let guard = LOCK.lock().unwrap();
    set_recon_chunk(0);
    Restore { _guard: guard }
}

/// LAB: overrides the reconstruction chunk width in blocks (0 = default;
/// `usize::MAX` = whole block rows, the unfused order).
#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
pub(crate) fn set_recon_chunk(n: usize) {
    RECON_CHUNK_LAB.store(n, core::sync::atomic::Ordering::Relaxed);
}

fn recon_chunk_blocks() -> usize {
    #[cfg(any(all(test, feature = "encode"), feature = "lab"))]
    {
        let n = RECON_CHUNK_LAB.load(core::sync::atomic::Ordering::Relaxed);
        if n != 0 {
            return n;
        }
    }
    RECON_CHUNK
}

/// Color-converts `n` pixels of one plane row (`p0..p2`) into `row`
/// (RGB of the output sample type).
#[allow(clippy::too_many_arguments)]
fn color_chunk_row(
    p: &Reconstruction<'_>,
    kind: InputKind,
    wide: bool,
    xyb16: Option<&(alloc::vec::Vec<f32>, f32)>,
    simd: bool,
    p0: &[f32],
    p1: &[f32],
    p2: &[f32],
    lin: &mut [f32],
    row: &mut [u8],
    n: usize,
) {
    let pl = [p0, p1, p2];
    if kind == InputKind::U32 {
        precision::output_row(p.info, p.color, p.inv_scale, pl, xyb16, row, n);
        return;
    }
    if precision::custom_color(p.info) {
        if simd {
            let ranges = color_index(p.info.channels)
                .map(|c| crate::mask_of(p.info.channel_bits(c).unwrap() as u32) as f32);
            if let Some((table, lmax)) = xyb16 {
                let lin = &mut lin[..n * 3];
                simd::xyb_row_linear(p0, p1, p2, p.inv_scale, lin, n);
                simd::encode_range_row(lin, table, *lmax, row, ranges, p.info.sample.bytes());
            } else {
                simd::ycocg_range_row(
                    p0,
                    p1,
                    p2,
                    p.inv_scale,
                    row,
                    n,
                    ranges,
                    crate::mask_of(p.info.sample.depth()) as f32,
                    p.info.sample.bytes(),
                );
            }
        } else {
            precision::output_row(p.info, p.color, p.inv_scale, pl, xyb16, row, n);
        }
        return;
    }
    if p.xyb_power {
        match kind {
            InputKind::U8 => xyb_power::row_u8(simd, p0, p1, p2, p.inv_scale, row, n),
            InputKind::U16 => xyb_power::row_u16(simd, p0, p1, p2, p.inv_scale, row, n),
            _ => unreachable!("V4 XYB power transfer only admits U8 and U16"),
        }
        return;
    }
    if matches!(kind, InputKind::F16 | InputKind::F32) || xyb16.is_some() {
        // Linear-light output straight from the XYB inverse.
        let lin = &mut lin[..n * 3];
        if simd {
            simd::xyb_row_linear(p0, p1, p2, p.inv_scale, lin, n);
        } else {
            for x in 0..n {
                let v = [pl[0][x], pl[1][x], pl[2][x]];
                lin[x * 3..x * 3 + 3].copy_from_slice(&color::inverse_xyb_linear(p.inv_scale, v));
            }
        }
        if let Some((table, lmax)) = xyb16 {
            if simd {
                simd::encode16_row(lin, table, *lmax, row);
            } else {
                for (o, v) in row.as_chunks_mut::<2>().0.iter_mut().zip(lin.iter()) {
                    *o = transfer::encode16(table, *lmax, *v).to_ne_bytes();
                }
            }
        } else if kind == InputKind::F32 {
            for (o, v) in row.as_chunks_mut::<4>().0.iter_mut().zip(lin.iter()) {
                *o = v.to_ne_bytes();
            }
        } else {
            for (o, v) in row.as_chunks_mut::<2>().0.iter_mut().zip(lin.iter()) {
                *o = color::f32_to_f16_rne(*v).to_ne_bytes();
            }
        }
        return;
    }
    if let Some(m) = &p.transform {
        // Lossy 0.4 signalled linear color transform (U8 only; parse rejects
        // other sample types and precision declarations).
        if simd {
            simd::linear_row(p0, p1, p2, p.inv_scale, m, row, n);
        } else {
            for x in 0..n {
                let v = [pl[0][x], pl[1][x], pl[2][x]];
                let o = color::inverse_linear_px(m, p.inv_scale, v);
                row[x * 3..x * 3 + 3].copy_from_slice(&o);
            }
        }
        return;
    }
    match (simd, wide) {
        (true, false) => simd::color_row(p.color, p0, p1, p2, p.inv_scale, row, n),
        (true, true) => simd::ycocg_row16(p0, p1, p2, p.inv_scale, row, n),
        (false, false) => {
            for x in 0..n {
                let v = [pl[0][x], pl[1][x], pl[2][x]];
                let o = color::inverse_px(p.color, p.inv_scale, v);
                row[x * 3..x * 3 + 3].copy_from_slice(&o);
            }
        }
        (false, true) => {
            for x in 0..n {
                let v = [pl[0][x], pl[1][x], pl[2][x]];
                let o = color::inverse_ycocg16(p.inv_scale, v);
                for c in 0..3 {
                    row[x * 6 + 2 * c..x * 6 + 2 * c + 2].copy_from_slice(&o[c].to_ne_bytes());
                }
            }
        }
    }
}

/// Packs `rows` color rows (RGB of the sample type, tightly packed) and
/// optional alpha rows into the image's channel layout. Gray takes the
/// green sample (gray input decodes to R = G = B up to rounding).
fn pack_rows(info: &ImageInfo, w: usize, rgb: &[u8], alpha: Option<&[u8]>, out: &mut [&mut [u8]]) {
    match info.sample.bytes() {
        1 => pack_rows_typed::<1>(info.channels, w, rgb, alpha, out),
        2 => pack_rows_typed::<2>(info.channels, w, rgb, alpha, out),
        4 => pack_rows_typed::<4>(info.channels, w, rgb, alpha, out),
        _ => unreachable!("unsupported sample size"),
    }
}

fn pack_rows_typed<const BPS: usize>(
    channels: Channels,
    w: usize,
    rgb: &[u8],
    alpha: Option<&[u8]>,
    out: &mut [&mut [u8]],
) {
    let nco = channels.count();
    let gray = matches!(channels, Channels::Gray | Channels::GrayAlpha);
    // Callers supply decoded alpha exactly when the image layout includes it.
    debug_assert_eq!(channels.has_alpha(), alpha.is_some());
    for (y, row) in out.iter_mut().enumerate() {
        let src = rgb[y * w * 3 * BPS..][..w * 3 * BPS]
            .as_chunks::<BPS>()
            .0
            .as_chunks::<3>()
            .0;
        let dst = row[..w * nco * BPS].as_chunks_mut::<BPS>().0;
        let arow = alpha.map(|a| a[y * w * BPS..][..w * BPS].as_chunks::<BPS>().0);
        // Samples stay byte arrays, preserving native endian and float payloads.
        // Fixed-size assignments expose the same vectorizable loops at all BPS.
        match (gray, arow) {
            (true, None) => {
                for (d, s) in dst.iter_mut().zip(src) {
                    *d = s[1];
                }
            }
            (true, Some(a)) => {
                for ((d, s), &a) in dst.as_chunks_mut::<2>().0.iter_mut().zip(src).zip(a) {
                    *d = [s[1], a];
                }
            }
            (false, Some(a)) => {
                for ((d, s), &a) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src).zip(a) {
                    *d = [s[0], s[1], s[2], a];
                }
            }
            (false, None) => dst.as_chunks_mut::<3>().0.copy_from_slice(src),
        }
    }
}

#[cfg(all(test, feature = "encode"))]
mod pack_tests;

#[cfg(all(test, feature = "std", feature = "encode"))]
mod bundle_tests;

/// Decode stage timers for imij-bench (`lab` + `std`); no-ops otherwise.
/// Stages: 0 DC image, 1 entropy, 2 IDCT, 3 color.
#[cfg(all(feature = "lab", feature = "std"))]
pub(crate) mod prof {
    use core::sync::atomic::{AtomicU64, Ordering::Relaxed};
    pub(crate) static NS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
    /// Stage timing on (default); A/B timing turns it off.
    pub(crate) static ON: core::sync::atomic::AtomicBool =
        core::sync::atomic::AtomicBool::new(true);
    pub(crate) struct T(Option<std::time::Instant>);
    pub(crate) fn start() -> T {
        T(ON.load(Relaxed).then(std::time::Instant::now))
    }
    impl T {
        pub(crate) fn add(self, i: usize) {
            if let Some(t) = self.0 {
                NS[i].fetch_add(t.elapsed().as_nanos() as u64, Relaxed);
            }
        }
    }
    pub(crate) fn take() -> [u64; 6] {
        core::array::from_fn(|i| NS[i].swap(0, Relaxed))
    }
    /// Encoder token counts: tokens, lockstep slots (16 x longest lane per
    /// stripe).
    pub(crate) static TOK: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
    pub(crate) fn tokens(lens: &[u32]) {
        TOK[0].fetch_add(lens.iter().map(|&v| v as u64).sum(), Relaxed);
        TOK[1].fetch_add(
            *lens.iter().max().unwrap_or(&0) as u64 * lens.len() as u64,
            Relaxed,
        );
    }
    static SIDE: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
    pub(crate) fn side_stats(s: &[u64; 3]) {
        for (a, &v) in SIDE.iter().zip(s) {
            a.store(v, Relaxed);
        }
    }
    pub(crate) fn take_side_stats() -> [u64; 3] {
        core::array::from_fn(|i| SIDE[i].load(Relaxed))
    }
    // The entropy-table section `encode_lossy_frame` wrote to its file:
    // `(entropy mode byte, table payload bytes)`, `(u8::MAX, 0)` before any
    // encode on this thread (split-bound seed-table study, 2026-10-10).
    // Thread-local: a harness that encodes on worker threads reads it on the
    // thread that ran the encode.
    std::thread_local! {
        static AC_TABLES: core::cell::Cell<(u64, u64)> =
            const { core::cell::Cell::new((u8::MAX as u64, 0)) };
    }
    pub(crate) fn ac_tables(mode: u8, bytes: u64) {
        AC_TABLES.with(|c| c.set((mode as u64, bytes)));
    }
    pub(crate) fn take_ac_tables() -> [u64; 2] {
        AC_TABLES.with(|c| {
            let v = c.get();
            c.set((u8::MAX as u64, 0));
            [v.0, v.1]
        })
    }
    pub(crate) fn take_tokens() -> [u64; 2] {
        core::array::from_fn(|i| TOK[i].swap(0, Relaxed))
    }
}
#[cfg(not(all(feature = "lab", feature = "std")))]
pub(crate) mod prof {
    pub(crate) struct T;
    #[inline(always)]
    pub(crate) fn start() -> T {
        T
    }
    impl T {
        #[inline(always)]
        pub(crate) fn add(self, _: usize) {}
    }
}

/// Forces the AVX-512 entropy kernel's lookup style (`None`: automatic).
#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
pub(crate) fn force_gather(on: Option<bool>) {
    let v = match on {
        None => 0,
        Some(true) => 1,
        Some(false) => 2,
    };
    entropy_avx512::FORCE.store(v, core::sync::atomic::Ordering::Relaxed);
}

/// The entropy kernel chosen for this CPU (None without AVX-512): the
/// run-level kernel's lookup style.
#[cfg(all(target_arch = "x86_64", feature = "lab"))]
pub(crate) fn kernel_choice_str() -> Option<alloc::string::String> {
    use archmage::SimdToken;
    archmage::X64V4Token::summon()
        .map(|_| alloc::format!("run-level {:?}", kernel_choice::choice().rl))
}

/// Decodes a lossy stream into `out` (rows of `stride` bytes).
/// Decodes a frame; `inter`: `out` holds the previous
/// frame, and blocks coded "keep" stay as they are.
pub(crate) fn decode_lossy_frame(
    info: ImageInfo,
    version: u8,
    color: LossyColor,
    lossy: crate::parse::header::Features,
    body: &[u8],
    out: &mut [u8],
    stride: usize,
    threads: usize,
    inter: bool,
) -> Result<ImageInfo, Error> {
    decode_lossy_impl(
        info,
        version,
        color,
        lossy,
        body,
        out,
        stride,
        threads,
        &crate::Limits::DEFAULT,
        true,
        inter,
    )
}

/// `simd = false` runs the scalar reference (tests compare the two).
#[allow(clippy::too_many_arguments)]
fn decode_lossy_impl(
    info: ImageInfo,
    version: u8,
    color: LossyColor,
    lossy: crate::parse::header::Features,
    body: &[u8],
    out: &mut [u8],
    stride: usize,
    threads: usize,
    limits: &crate::Limits,
    simd: bool,
    inter: bool,
) -> Result<ImageInfo, Error> {
    let p = parse_lossy_as(info, version, color, lossy, body)?;
    decode_lossy_parsed(p, out, stride, threads, limits, simd, inter)
}

/// Decode validated structure. Allocating callers parse before reserving the
/// output, then reuse that state rather than rebuilding nested tables.
pub(crate) fn decode_lossy_parsed(
    p: LossyParsed<'_>,
    out: &mut [u8],
    stride: usize,
    threads: usize,
    limits: &crate::Limits,
    simd: bool,
    inter: bool,
) -> Result<ImageInfo, Error> {
    decode_lossy_parsed_checked::<false>(p, out, stride, threads, limits, simd, inter, &|| Ok(()))
}

/// A work item of a frame decode: the lossy stripes (index, output rows) of
/// column group `.0`, or one shared DC unit (`SHARED_DC` mode 2).
enum Item<'a> {
    Stripes(usize, Vec<(usize, Vec<&'a mut [u8]>)>),
    #[cfg(feature = "std")]
    Dc(usize),
}

pub(crate) fn decode_lossy_parsed_checked<const BOUNDED: bool>(
    p: LossyParsed<'_>,
    out: &mut [u8],
    stride: usize,
    threads: usize,
    limits: &crate::Limits,
    simd: bool,
    inter: bool,
    check: &(impl Fn() -> Result<(), Error> + Sync),
) -> Result<ImageInfo, Error> {
    decode_lossy_parsed_observed::<BOUNDED>(
        p, out, stride, threads, limits, simd, inter, check, None,
    )
}

/// [`decode_lossy_frame`] reporting each lossy unit (stripe, column group) to
/// a live `pulse` ([`crate::progress`]).
#[cfg(all(feature = "std", feature = "lab"))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_lossy_frame_observed(
    info: ImageInfo,
    version: u8,
    color: LossyColor,
    lossy: crate::parse::header::Features,
    body: &[u8],
    out: &mut [u8],
    stride: usize,
    threads: usize,
    inter: bool,
    pulse: &dyn how_far::Pulse,
) -> Result<ImageInfo, Error> {
    let p = parse_lossy_as(info, version, color, lossy, body)?;
    decode_lossy_parsed_observed::<false>(
        p,
        out,
        stride,
        threads,
        &crate::Limits::DEFAULT,
        true,
        inter,
        &|| Ok(()),
        Some(pulse),
    )
}

/// [`decode_lossy_parsed_checked`] reporting each lossy unit to a live
/// `pulse`: both stripes of a decoded pair start and finish together.
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_lossy_parsed_observed<const BOUNDED: bool>(
    p: LossyParsed<'_>,
    out: &mut [u8],
    stride: usize,
    threads: usize,
    limits: &crate::Limits,
    simd: bool,
    inter: bool,
    check: &(impl Fn() -> Result<(), Error> + Sync),
    pulse: Option<&dyn how_far::Pulse>,
) -> Result<ImageInfo, Error> {
    check()?;
    if p.skip && !inter {
        return Err(Error::Corrupt(
            "keep-reference blocks outside an inter frame",
        ));
    }
    let extent = p
        .info
        .checked_stride_len(stride)
        .ok_or(Error::InvalidInput("invalid output layout"))?;
    if out.len() < extent {
        return Err(Error::InvalidInput("output buffer too small"));
    }
    let rb = p.info.row_bytes();
    let rows = p.g.sb * 8;
    let nstripes = p.g.stripes();
    // Nested images (validated by `parse_lossy`): the DC image's stripes
    // span k lossy stripes, the alpha image is striped like the lossy one.
    let t = prof::start();
    let dcp = p.dc.as_ref().unwrap();
    let dc_om = crate::v1::OutMap::new(&dcp.info, &dcp.layout);
    let k = dcp.hs / p.g.sb;
    let alpha_img = p
        .alpha
        .as_ref()
        .map(|ap| (ap, crate::v1::OutMap::new(&ap.info, &ap.layout)));
    // Exact blocks: the nested image's output map and, with threads, its
    // decoded stripes shared by the work items.
    let exact_om = p
        .exact
        .as_ref()
        .map(|e| crate::v1::OutMap::new(&e.img.info, &e.img.layout));
    #[cfg(feature = "std")]
    let exact_shared = p.exact.as_ref().map(exact::Shared::new).transpose()?;
    #[cfg(not(feature = "std"))]
    let exact_shared: Option<exact::Shared> = None;
    let _ = limits;
    t.add(0);
    let rgb = p.info.channels == Channels::Rgb;
    let bps = p.info.sample.bytes();
    let pxb = rb / p.g.w;
    // Row slices of every unit (stripe, column group).
    let ng = p.ngroups();
    let mut unit_rows: Vec<Vec<&mut [u8]>> = (0..nstripes * ng).map(|_| Vec::new()).collect();
    for (y, row) in out[..extent].chunks_mut(stride).enumerate() {
        let mut rest = &mut row[..rb];
        for gi in 0..ng {
            let gw = p.group(gi).1.w;
            let (a, b) = rest.split_at_mut(gw * pxb);
            unit_rows[(y / rows) * ng + gi].push(a);
            rest = b;
        }
    }
    // Work items per column group. One DC stripe per lossy stripe (k = 1):
    // pairs of consecutive stripes whose entropy decodes run interleaved
    // (`decode_stripe_coefs2`), then single stripes at the tail so the
    // threads finish together. k > 1: the lossy stripes of one DC stripe
    // (decoded once per item), taken in pairs, unless there are too few DC
    // stripes per thread and `split_dc_groups` would split them over
    // several items, each decoding the DC stripe again. Unbounded std
    // decodes on more than one worker then share DC units instead: the
    // k = 1 items, each stripe taking its DC rows from the shared unit,
    // and (mode 2) each unit decoded by its own item queued just before the
    // unit's first stripe. Without a split, sharing gains nothing and cost
    // up to 2.5% (benchmarks/lossy_dc_schedule_2026-10-07).
    // Threads per column group; one on hosts without threads, where split
    // DC stripes would only be decoded again in turn.
    let workers = crate::par::worker_count(threads, nstripes * ng);
    // Reports each unit to a live pulse; otherwise one branch per pair.
    let observed = pulse.and_then(|pulse| {
        let (h, rows_per) = (p.info.height as usize, p.g.sb * 8);
        let shapes = (0..nstripes * ng).map(|u| {
            let (si, gi) = (u / ng, u % ng);
            let (x, gg) = p.group(gi);
            let y = si * rows_per;
            crate::progress::UnitShape {
                stripe: si,
                group: gi,
                groups: ng,
                y,
                rows: rows_per.min(h - y),
                x,
                cols: gg.w,
            }
        });
        crate::progress::UnitProgress::plan(pulse, shapes, workers)
    });
    let observed = observed.as_ref();
    let tg = if workers > 1 { threads.div_ceil(ng) } else { 1 };
    #[cfg(feature = "std")]
    let mode = if p.model_refs.is_none()
        && p.dc_coverage.is_none()
        && !BOUNDED
        && k > 1
        && workers > 1
        && split_dc_groups(nstripes, k, tg) < k
    {
        shared_dc_mode()
    } else {
        0
    };
    #[cfg(feature = "std")]
    let shared = if mode != 0 {
        #[cfg(all(test, feature = "encode"))]
        SHARED_DC_USED.with(|n| n.set(n.get() + 1));
        // A DC stripe may declare more rows than the image has: slots only
        // for the lossy stripes that exist.
        Some(SharedDc::new(nstripes.div_ceil(k) * ng, k.min(nstripes))?)
    } else {
        None
    };
    #[cfg(feature = "std")]
    let (k_items, late) = (if mode != 0 { 1 } else { k }, mode == 2);
    #[cfg(not(feature = "std"))]
    let k_items = k;
    let paired = paired_stripes(nstripes, tg);
    let per = split_dc_groups(nstripes, k_items, tg);
    let mut items: Vec<Item<'_>> = Vec::new();
    for gi in 0..ng {
        let first = items.len();
        for s in 0..nstripes {
            let r = core::mem::take(&mut unit_rows[s * ng + gi]);
            #[cfg(feature = "std")]
            if late && s % k == 0 {
                items.push(Item::Dc(s / k * ng + gi));
            }
            // The last item belongs to this group.
            let own = items.len() > first;
            if p.dc_coverage.is_some() {
                match items.last_mut() {
                    Some(Item::Stripes(_, group))
                        if own && p.dc_span(group[0].0, gi).unit == p.dc_span(s, gi).unit =>
                    {
                        group.push((s, r))
                    }
                    _ => items.push(Item::Stripes(gi, alloc::vec![(s, r)])),
                }
                continue;
            }
            match items.last_mut() {
                Some(Item::Stripes(_, g)) if own && k_items == 1 && s < paired && g.len() == 1 => {
                    g.push((s, r))
                }
                Some(Item::Stripes(_, g)) if own && k_items > 1 && s % k != 0 && g.len() < per => {
                    g.push((s, r))
                }
                _ => items.push(Item::Stripes(gi, alloc::vec![(s, r)])),
            }
        }
    }
    #[cfg(feature = "std")]
    let pool = std::sync::Mutex::new(Vec::<Scratch>::new());
    // LAB: scratch kept across decodes (page-fault experiment).
    #[cfg(all(feature = "std", any(all(test, feature = "encode"), feature = "lab")))]
    let pool = if !BOUNDED && KEEP_SCRATCH.load(core::sync::atomic::Ordering::Relaxed) {
        &KEPT
    } else {
        &pool
    };
    // Decodes shared DC unit `u` (DC stripe `u / ng`, column group `u % ng`)
    // top to bottom, publishing each lossy stripe's DC values as soon as its
    // rows are decoded so that items below can start before the unit ends.
    #[cfg(feature = "std")]
    let produce = |sh: &SharedDc, u: usize, px: &mut Vec<u8>| -> Result<(), Error> {
        let (d, gi) = (u / ng, u % ng);
        let dr = p.group(gi).1.bw * dcp.bpp();
        let mut rows = crate::v1::UnitRows::new(dcp, u)?;
        for s in d * k..(d * k + k).min(nstripes) {
            let nb = p.g.sb.min(p.g.bh - s * p.g.sb);
            resize_fallible(px, nb * dr, 0)?;
            for r in 0..nb {
                check()?;
                rows.next_row(dcp, &dc_om, &mut px[r * dr..][..dr])?;
            }
            let (mut dc, mut aq) = (Vec::new(), Vec::new());
            dc_values(
                dcp.info.sample,
                p.aq,
                p.exact.is_some(),
                &px[..nb * dr],
                &mut dc,
                &mut aq,
            )?;
            sh.publish(u, s - d * k, (dc, aq));
        }
        Ok(())
    };
    let decode_item = |_, item: Item<'_>| {
        check()?;
        let mut exact_st = exact::ItemState::default();
        #[cfg(feature = "std")]
        let mut sc = pool.lock().unwrap().pop().unwrap_or_default();
        #[cfg(feature = "std")]
        let (gi, mut group) = match item {
            Item::Stripes(gi, group) => (gi, group),
            Item::Dc(u) => {
                let r = match &shared {
                    Some(sh) => sh.produce_unit(u, |sh, u| produce(sh, u, &mut sc.dc_px)),
                    None => Ok(()),
                };
                pool.lock().unwrap().push(sc);
                return r;
            }
        };
        #[cfg(not(feature = "std"))]
        let Item::Stripes(gi, mut group) = item;
        #[cfg(not(feature = "std"))]
        let mut sc = Scratch::default();
        let mut dcs = [
            (core::mem::take(&mut sc.dc), core::mem::take(&mut sc.aq)),
            (core::mem::take(&mut sc.dc2), core::mem::take(&mut sc.aq2)),
        ];
        // Per-item DC: the DC stripe being decoded (index, row decoder, next
        // row); each lossy stripe decodes just its own rows when it needs
        // them, so no whole DC stripe is ever buffered.
        let mut cursor: Option<(
            usize,
            crate::v1::UnitRows<'_>,
            usize,
            Option<(
                alloc::sync::Arc<crate::v1::Parsed<'static>>,
                crate::v1::OutMap,
            )>,
        )> = None;
        let gg = p.group(gi).1;
        let r = (|| {
            let bw = gg.bw;
            for pair in group.chunks_mut(2) {
                check()?;
                if let Some(observed) = observed {
                    for (j, (s, _)) in pair.iter().enumerate() {
                        if let Err(e) = observed.begin(s * ng + gi) {
                            // A stop at a unit's checkpoint fails the pair before
                            // decoding; the units begun so far record it.
                            for (s, _) in &pair[..=j] {
                                observed.end(s * ng + gi, &Err(e.clone()));
                            }
                            return Err(e);
                        }
                    }
                }
                // DC values of each stripe of the pair into `dcs[j]`.
                let t = prof::start();
                for (j, (s, _)) in pair.iter().enumerate() {
                    #[cfg(feature = "std")]
                    if let Some(sh) = &shared {
                        // Late: taken after the entropy decode below.
                        if !late {
                            let u = *s / k * ng + gi;
                            dcs[j] = sh.take(u, *s % k, |sh, u| produce(sh, u, &mut sc.dc_px))?;
                        }
                        continue;
                    }
                    let span = p.dc_span(*s, gi);
                    let d = span.unit;
                    if cursor.as_ref().is_none_or(|c| c.0 != d) {
                        let (model, body) = if let Some(models) = &p.model_refs {
                            models.dc(dcp.stripes[d], dcp)?
                        } else {
                            (None, dcp.stripes[d])
                        };
                        let selected = model.as_deref().unwrap_or(dcp);
                        let rows = crate::v1::UnitRows::new_with(selected, d, body)?;
                        let alternate = model.map(|p| {
                            let om = crate::v1::OutMap::new(&p.info, &p.layout);
                            (p, om)
                        });
                        cursor = Some((d, rows, 0, alternate));
                    }
                    let Some((_, rows, next, alternate)) = cursor.as_mut() else {
                        unreachable!()
                    };
                    let dr = bw * dcp.bpp();
                    let nb = p.g.sb.min(p.g.bh - *s * p.g.sb);
                    let first = (*s - span.start) * p.g.sb;
                    if *next > first {
                        return Err(Error::Corrupt("DC rows out of order"));
                    }
                    resize_fallible(&mut sc.dc_px, nb * dr, 0)?;
                    // An item that starts inside a DC stripe (only when the
                    // lab switch splits DC stripes without sharing) decodes
                    // the rows above its first stripe and drops them.
                    while *next < first + nb {
                        check()?;
                        let r = next.saturating_sub(first);
                        let (model, om) = alternate
                            .as_ref()
                            .map_or((dcp, &dc_om), |(p, om)| (p.as_ref(), om));
                        rows.next_row(model, om, &mut sc.dc_px[r * dr..][..dr])?;
                        *next += 1;
                    }
                    let (dc, aq) = &mut dcs[j];
                    dc.clear();
                    aq.clear();
                    dc_values(
                        dcp.info.sample,
                        p.aq,
                        p.exact.is_some(),
                        &sc.dc_px[..nb * dr],
                        dc,
                        aq,
                    )?;
                }
                t.add(0);
                let t = prof::start();
                #[cfg(feature = "std")]
                let skip = late && shared.is_some();
                #[cfg(not(feature = "std"))]
                let skip = false;
                // Empty: the DCs are filled in after the entropy decode.
                let dc_of = |j: usize| {
                    if skip { &[][..] } else { &dcs[j].0[..] }
                };
                if pair.len() == 2 {
                    let s = [pair[0].0, pair[1].0];
                    let u = s.map(|s| s * ng + gi);
                    decode_stripe_coefs2(&p, &gg, s, u, [dc_of(0), dc_of(1)], simd, &mut sc)?;
                } else {
                    let s = pair[0].0;
                    decode_stripe_coefs(&p, &gg, s, s * ng + gi, dc_of(0), simd, &mut sc)?;
                }
                t.add(1);
                // Late DC: wait for the shared unit's rows only now, so the
                // entropy decode above overlaps the unit's decode.
                #[cfg(feature = "std")]
                if let Some(sh) = &shared
                    && late
                {
                    let t = prof::start();
                    for (j, (s, _)) in pair.iter().enumerate() {
                        let u = *s / k * ng + gi;
                        dcs[j] = sh.take(u, *s % k, |sh, u| produce(sh, u, &mut sc.dc_px))?;
                        let (by0, by1) = gg.rows(*s);
                        let q = if j == 0 { &mut sc.q } else { &mut sc.q2 };
                        fill_dc(q, &dcs[j].0, by1 - by0, bw);
                    }
                    t.add(0);
                }
                let Scratch {
                    q,
                    eobs,
                    q2,
                    eobs2,
                    planes,
                    lin,
                    alpha,
                    tmp,
                    roles,
                    splits,
                    ..
                } = &mut sc;
                for (j, (s, rws)) in pair.iter_mut().enumerate() {
                    let s = *s;
                    let (q, eobs) = if j == 0 {
                        (&mut *q, &*eobs)
                    } else {
                        (&mut *q2, &*eobs2)
                    };
                    let aq = &dcs[j].1[..];
                    let rec = p.stripes[s * ng + gi];
                    let ws = (&mut *planes, &mut *lin, &mut *roles, &mut *splits);
                    if rgb {
                        recon_unit::<BOUNDED>(&p, &gg, s, rec, aq, rws, simd, q, eobs, ws)?;
                        if let (Some(e), Some(om)) = (&p.exact, &exact_om) {
                            let shared = exact_shared.as_ref();
                            exact::apply_unit(
                                e,
                                exact::Nested::Decode { om, shared },
                                &mut exact_st,
                                e.range(s * ng + gi)?,
                                s,
                                &gg,
                                aq,
                                rws.as_mut_slice(),
                                check,
                            )?;
                        }
                    } else {
                        let n = rws.len();
                        let crb = gg.w * 3 * bps;
                        let arb = gg.w * bps;
                        resize_fallible(tmp, n.checked_mul(crb).ok_or(Error::TooLarge)?, 0)?;
                        let mut trows: Vec<&mut [u8]> = tmp.chunks_mut(crb).collect();
                        recon_unit::<BOUNDED>(&p, &gg, s, rec, aq, &mut trows, simd, q, eobs, ws)?;
                        if let (Some(e), Some(om)) = (&p.exact, &exact_om) {
                            let shared = exact_shared.as_ref();
                            exact::apply_unit(
                                e,
                                exact::Nested::Decode { om, shared },
                                &mut exact_st,
                                e.range(s * ng + gi)?,
                                s,
                                &gg,
                                aq,
                                trows.as_mut_slice(),
                                check,
                            )?;
                        }
                        let a = match &alpha_img {
                            Some((ap, om)) => {
                                let mut a = core::mem::take(alpha);
                                resize_fallible(
                                    &mut a,
                                    n.checked_mul(arb).ok_or(Error::TooLarge)?,
                                    0,
                                )?;
                                crate::v1::decode_unit_into_checked(
                                    ap,
                                    s * ng + gi,
                                    om,
                                    &mut a,
                                    arb,
                                    check,
                                )?;
                                Some(a)
                            }
                            None => None,
                        };
                        pack_rows(&p.info, gg.w, tmp, a.as_deref(), rws);
                        if let Some(a) = a {
                            *alpha = a;
                        }
                    }
                }
                if let Some(observed) = observed {
                    for (s, _) in pair.iter() {
                        observed.end(s * ng + gi, &Ok(()));
                    }
                }
            }
            Ok(())
        })();
        let [(dc, aq), (dc2, aq2)] = dcs;
        (sc.dc, sc.aq, sc.dc2, sc.aq2) = (dc, aq, dc2, aq2);
        #[cfg(feature = "std")]
        pool.lock().unwrap().push(sc);
        r
    };
    if BOUNDED {
        for (index, item) in items.into_iter().enumerate() {
            decode_item(index, item)?;
        }
    } else {
        for result in crate::par::par_map(items, threads, decode_item) {
            result?;
        }
    }
    Ok(p.info)
}

#[cfg(all(test, feature = "encode"))]
pub(crate) mod tests;

#[cfg(all(test, feature = "lab", feature = "std", not(target_family = "wasm")))]
mod overlap_study;

#[cfg(all(test, feature = "encode"))]
mod color_precision_tests;

#[cfg(all(test, feature = "encode"))]
mod hdr_precision_lab_tests;

mod precision;

/// Shared transfer lookup allocation, separate from per-worker row scratch.
pub(crate) fn decoder_transfer_table_bytes(info: &ImageInfo, color: LossyColor) -> u128 {
    let uses_table = color == LossyColor::Xyb
        && transfer::Transfer::from_cicp(info.cicp.transfer_characteristics).is_some()
        && (matches!(info.sample, SampleType::U16 | SampleType::U32)
            || (info.sample == SampleType::U8 && precision::custom_color(info)));
    if uses_table {
        (transfer::TABLE_N as u128 + 1) * core::mem::size_of::<f32>() as u128
    } else {
        0
    }
}
