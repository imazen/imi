//! Base quantization matrices and header quantization parameters (`QParams`).

use super::{ColorTransform, LossyColor, MATRIX_EXPLICIT, ReconBias, transfer};
#[cfg(feature = "encode")]
use crate::Error;

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

/// Header quantization parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::lossy) struct QParams {
    pub(in crate::lossy) scale: f32,
    pub(in crate::lossy) floor: f32,
    pub(in crate::lossy) mul: [f32; 3],
    /// Base matrix id (SPEC 2.8): 0..=2, the presets up to
    /// [`MATRIX_LAST_PRESET`] and [`MATRIX_EXPLICIT`] (Lossy 0.4).
    pub(in crate::lossy) matrix: u8,
    /// Per-plane DC (position 0) step multiplier in 1/64 units, 1..=255.
    pub(in crate::lossy) dc: [u8; 3],
    /// Lossy 0.4: the base tables of matrix [`MATRIX_EXPLICIT`], raster order.
    pub(in crate::lossy) explicit: Option<[[f32; 64]; 3]>,
    /// Lossy 0.4: the reconstruction bias (`None` = reconstruct at q x step).
    pub(in crate::lossy) bias: Option<ReconBias>,
    /// Lossy 0.4: a signalled linear color transform (`None` = the color's
    /// default transform: XYB or the YCoCg lifting).
    pub(in crate::lossy) transform: Option<ColorTransform>,
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
    pub(in crate::lossy) fn from_quality(q: f32, color: LossyColor, matrix: u8) -> QParams {
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
    pub(in crate::lossy) fn base(&self, c: usize) -> [f32; 64] {
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
    pub(in crate::lossy) fn base_steps(&self) -> [[f32; 64]; 3] {
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
    pub(in crate::lossy) fn steps(&self) -> [[f32; 64]; 3] {
        let mut s = self.base_steps();
        for (c, sc) in s.iter_mut().enumerate() {
            sc[0] = (sc[0] * (self.dc[c] as f32 / 64.0)).max(1.0 / 64.0);
        }
        s
    }

    #[cfg(feature = "encode")]
    pub(in crate::lossy) fn apply_ac_scale(&mut self, scale: f32) -> Result<(), Error> {
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

    pub(in crate::lossy) fn valid(&self) -> bool {
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
