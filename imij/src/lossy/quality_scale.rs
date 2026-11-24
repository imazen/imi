//! Lossy quality settings with named units: perceptual-metric targets
//! (SSIMULACRA2, Butteraugli), compatibility scales (jpegli-style quality,
//! JPEG XL distance) and IMI's own quantizer scale.
//!
//! Every setting except [`LossyQuality::Scale`] maps to the quantizer scale
//! through a monotone table per color mode and image-size range, fitted on
//! medians over a training split; the tables, their corpus and held-out error
//! live in `quality_scale_tables.rs` (`benchmarks/quality_scale_2026-10-07/`).

use super::LossyColor;
use super::quality_scale_tables as tables;

/// A lossy quality setting for `ImiEncoderConfig::with_quality`.
/// Each variant names its unit.
///
/// The metric targets ([`Self::Ssim2`], [`Self::Butteraugli`]) are the
/// primary settings; [`Self::JpegQuality`] and [`Self::JxlDistance`] exist
/// for comparison with other encoders; [`Self::Scale`] is the quantizer scale
/// the lossy coder has always used.
///
/// Targets are **medians, not guarantees**: a target maps to the scale whose
/// median score over a calibration corpus equals it, so individual images land
/// above or below. The per-image spread (p10/p90 on held-out images) is
/// documented in `benchmarks/quality_scale_2026-10-07/README.md`.
/// Out-of-range values clamp to the calibrated range.
///
/// The mapping depends on the color mode the encoder uses and on the image's
/// longest side (four ranges: up to 128, 384 and 768 pixels, and above). The
/// tables were fitted on 8-bit sRGB images before production effort selected
/// the AC trellis. The historical error bands do not establish accuracy for
/// XYB effort 9 and above: there the trellis lands coarser at the same
/// setting (on a VALIDATE diagnostic, SSIMULACRA2 1.3–5.3 points lower in
/// median for targets 30–70, with fewer bytes;
/// `benchmarks/quality_scale_2026-10-07/effort_validate.md`). Other sample
/// types also use these tables without separate calibration.
///
/// The enum is `#[non_exhaustive]`: further metric targets (zensim-next) and a
/// 0–100 scale whose 100 means lossless can be added as new variants without
/// changing the meaning of these.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum LossyQuality {
    /// Target SSIMULACRA2 score (at most 100, higher is better), of the
    /// 8-bit sRGB decoded output against the source. Calibrated from about
    /// -52 to 95; a target an image size cannot reach in median clamps to
    /// the scale's end (1 or 100).
    Ssim2(f32),
    /// Target Butteraugli distance (lower is better; Butteraugli 0.9 with
    /// default parameters on 8-bit sRGB). The norm is part of the unit.
    /// Calibrated from about 0.3 to 31 (max) and 0.11 to 13 (3-norm), with
    /// the same clamping as [`Self::Ssim2`].
    Butteraugli {
        /// The distance in the chosen norm.
        distance: f32,
        /// Which aggregate of the per-pixel distance map is targeted.
        norm: ButteraugliNorm,
    },
    /// libjpeg/jpegli-style quality, 1–100 (compatibility): the scale whose
    /// median SSIMULACRA2 equals jpegli's at this quality (zenjpeg
    /// `ApproxJpegli`, 4:2:0, progressive, adaptive quantization, no trellis).
    JpegQuality(f32),
    /// JPEG XL distance, 0.5–25, lower is better (compatibility): the scale
    /// whose median Butteraugli 3-norm equals jxl-encoder's (effort 7) at this
    /// distance.
    JxlDistance(f32),
    /// IMI's quantizer scale (unitless, libjpeg-style; see `docs/LOSSY.md`):
    /// steps scale by `50 / q` below 50 and `(200 - 2q) / 100` from 50 to 100,
    /// and keep shrinking above 100 up to 116. q100 is not lossless. Passes
    /// through unchanged; a bare `f32` converts to it.
    Scale(f32),
}

/// The aggregate of Butteraugli's per-pixel distance map that a
/// [`LossyQuality::Butteraugli`] target refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ButteraugliNorm {
    /// The maximum over the image (Butteraugli's headline score).
    Max,
    /// The 3-norm (the "p-norm" JPEG XL's distance tracks).
    P3,
}

impl From<f32> for LossyQuality {
    /// A bare number is the quantizer scale, as `ImiEncoderConfig::lossy` takes it.
    fn from(scale: f32) -> Self {
        Self::Scale(scale)
    }
}

/// A monotone piecewise-linear map from a target value to the scale. `x` is
/// strictly increasing; `scale` is strictly monotone (rising for quality-like
/// units, falling for distances), so the table also inverts.
pub(crate) struct Table {
    pub(crate) x: &'static [f32],
    pub(crate) scale: &'static [f32],
}

impl Table {
    fn forward(&self, x: f32) -> f32 {
        interpolate(self.x, self.scale, x)
    }
    fn inverse(&self, scale: f32) -> f32 {
        interpolate(self.scale, self.x, scale)
    }
}

/// Piecewise-linear interpolation over the monotone (rising or falling) axis
/// `xs`, clamped to its range. NaN stays NaN, so encoding rejects it.
fn interpolate(xs: &[f32], ys: &[f32], x: f32) -> f32 {
    if x.is_nan() {
        return f32::NAN;
    }
    let n = xs.len();
    let rising = xs[0] <= xs[n - 1];
    let x = if rising {
        x.clamp(xs[0], xs[n - 1])
    } else {
        x.clamp(xs[n - 1], xs[0])
    };
    let mut i = 0;
    while i + 2 < n && if rising { xs[i + 1] < x } else { xs[i + 1] > x } {
        i += 1;
    }
    let (x0, x1) = (xs[i], xs[i + 1]);
    let t = if x0 == x1 { 0.0 } else { (x - x0) / (x1 - x0) };
    ys[i] + t * (ys[i + 1] - ys[i])
}

/// Longest-side range of the calibration buckets: up to 128, 384, 768, above.
fn bucket(width: u32, height: u32) -> usize {
    match width.max(height) {
        0..=128 => 0,
        129..=384 => 1,
        385..=768 => 2,
        _ => 3,
    }
}

#[derive(Clone, Copy)]
enum Unit {
    Ssim2,
    Butteraugli(ButteraugliNorm),
    JpegQuality,
    JxlDistance,
}

fn table(unit: Unit, color: LossyColor, width: u32, height: u32) -> &'static Table {
    use ButteraugliNorm::{Max, P3};
    use LossyColor::{Xyb, YCoCg};
    let set = match (unit, color) {
        (Unit::Ssim2, Xyb) => &tables::SSIM2_XYB,
        (Unit::Ssim2, YCoCg) => &tables::SSIM2_YCOCG,
        (Unit::Butteraugli(Max), Xyb) => &tables::BUTTERAUGLI_MAX_XYB,
        (Unit::Butteraugli(Max), YCoCg) => &tables::BUTTERAUGLI_MAX_YCOCG,
        (Unit::Butteraugli(P3), Xyb) => &tables::BUTTERAUGLI_P3_XYB,
        (Unit::Butteraugli(P3), YCoCg) => &tables::BUTTERAUGLI_P3_YCOCG,
        (Unit::JpegQuality, Xyb) => &tables::JPEG_QUALITY_XYB,
        (Unit::JpegQuality, YCoCg) => &tables::JPEG_QUALITY_YCOCG,
        (Unit::JxlDistance, Xyb) => &tables::JXL_DISTANCE_XYB,
        (Unit::JxlDistance, YCoCg) => &tables::JXL_DISTANCE_YCOCG,
    };
    set[bucket(width, height)]
}

impl LossyQuality {
    /// The quantizer scale this setting encodes a `width` x `height` image
    /// with in `color` (the color mode the encoder actually uses).
    pub fn to_scale(self, color: LossyColor, width: u32, height: u32) -> f32 {
        let t = |u| table(u, color, width, height);
        match self {
            Self::Scale(s) => s,
            Self::Ssim2(s) => t(Unit::Ssim2).forward(s),
            Self::Butteraugli { distance, norm } => t(Unit::Butteraugli(norm)).forward(distance),
            Self::JpegQuality(q) => t(Unit::JpegQuality).forward(q),
            Self::JxlDistance(d) => t(Unit::JxlDistance).forward(d),
        }
    }

    fn approx(self, unit: Unit, color: LossyColor, width: u32, height: u32) -> f32 {
        table(unit, color, width, height).inverse(self.to_scale(color, width, height))
    }

    /// The median SSIMULACRA2 the calibration corpus reached at this setting
    /// for images of this size, clamped to the calibrated range.
    pub fn approx_ssim2(self, color: LossyColor, width: u32, height: u32) -> f32 {
        self.approx(Unit::Ssim2, color, width, height)
    }

    /// The median Butteraugli distance (in `norm`) the calibration corpus
    /// reached at this setting for images of this size, clamped to the
    /// calibrated range.
    pub fn approx_butteraugli(
        self,
        norm: ButteraugliNorm,
        color: LossyColor,
        width: u32,
        height: u32,
    ) -> f32 {
        self.approx(Unit::Butteraugli(norm), color, width, height)
    }

    /// The approximately equivalent jpegli quality (see [`Self::JpegQuality`]).
    pub fn approx_jpeg_quality(self, color: LossyColor, width: u32, height: u32) -> f32 {
        self.approx(Unit::JpegQuality, color, width, height)
    }

    /// The approximately equivalent JPEG XL distance (see [`Self::JxlDistance`]).
    pub fn approx_jxl_distance(self, color: LossyColor, width: u32, height: u32) -> f32 {
        self.approx(Unit::JxlDistance, color, width, height)
    }
}
