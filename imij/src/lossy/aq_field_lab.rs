//! LAB: jpegli's adaptive quantization field, for zenjpeg's AQ-to-lambda
//! trellis coupling (`trellis.rs`).
//!
//! A port of jpegli's `adaptive_quantization.cc` (libjxl `lib/jpegli`), read
//! from zenjpeg's port (`zenjpeg/src/quant/aq/mod.rs`), first written for the
//! 2026-10-06 AQ study (archived patch, benchmarks/aq_jpegli_2026-10-06; its
//! parity test against zenjpeg's field agreed to 1.8e-6 relative once
//! zenjpeg's fast log2/exp2 were modelled). From the luma plane in jpegli's
//! sample units (0..255) it computes a per-block quant field `qf`: a
//! 4x4-downsampled masking estimate ("pre-erosion"), a weighted minimum over
//! each 3x3 neighbourhood summed to 8x8 blocks ("fuzzy erosion"), then
//! per-block masking, high-frequency and gamma modulations. A larger `qf`
//! asks for finer quantization. As quantization gets coarse (the luma
//! table's first AC step from 9 to 65 in 8-bit units) jpegli fades the field
//! to the constant 0.48 * 0.841.
//!
//! Borders replicate the edge pixel, as jpegli's padded buffers do. Scalar
//! f32 without fused multiply-add: the same field on every target.

use alloc::vec;
use alloc::vec::Vec;

use magetypes::nostd_math::sqrtf;

use super::transfer::{exp, ln};

/// jpegli's sample units per IMI luma-plane unit. XYB: IMI's luma plane
/// is 300 Y, jpegli's 255 x 1.183 Y (its scaled XYB times the JPEG sample
/// range). YCoCg: Y is already in 8-bit code units, like JPEG's Y.
pub(crate) fn luma_units(xyb: bool) -> f32 {
    if xyb {
        1.183_000_1 * 255.0 / 300.0
    } else {
        1.0
    }
}

const INPUT_SCALING: f32 = 1.0 / 255.0;

// RatioOfDerivativesOfCubicRootToSimpleGamma.
const LOG2: f32 = core::f32::consts::LN_2;
const SG_MUL: f32 = 226.048_04;
const SG_MUL2: f32 = 1.0 / 73.377_13;
const SG_RET_MUL: f32 = SG_MUL2 * 18.658_093 * LOG2;
const SG_VOFFSET: f32 = 7.146_724_7;
const EPSILON: f32 = 1e-2;
const NUM_OFFSET: f32 = EPSILON / INPUT_SCALING / INPUT_SCALING;
const NUM_MUL: f32 = SG_RET_MUL * 3.0 * SG_MUL;
const V_OFFSET: f32 = (SG_VOFFSET * LOG2 + EPSILON) / INPUT_SCALING;
const DEN_MUL: f32 = LOG2 * SG_MUL * INPUT_SCALING * INPUT_SCALING;

fn ratio_of_derivatives<const INVERT: bool>(v: f32) -> f32 {
    let v = v.max(0.0);
    let v2 = v * v;
    let num = NUM_MUL * v2 + NUM_OFFSET;
    let den = DEN_MUL * v * v2 + V_OFFSET;
    if INVERT { num / den } else { den / num }
}

fn masking_sqrt(v: f32, mul: f32) -> f32 {
    0.25 * sqrtf(v * mul + 28.0)
}

fn compute_mask(v: f32) -> f32 {
    const BASE: f32 = -0.741_749_93;
    const MUL4: f32 = 3.235_325_8;
    const MUL2: f32 = 12.906_028;
    const OFFSET2: f32 = 305.040_36;
    const MUL3: f32 = 5.022_031_3;
    const OFFSET3: f32 = 2.192_574;
    const OFFSET4: f32 = 0.25 * OFFSET3;
    const MUL0: f32 = 0.747_604_2;
    let v1 = (v * MUL0).max(1e-3);
    let v2 = 1.0 / (v1 + OFFSET2);
    let v3 = 1.0 / (v1 * v1 + OFFSET3);
    let v4 = 1.0 / (v1 * v1 + OFFSET4);
    BASE + (MUL4 * v4 + (MUL2 * v2 + MUL3 * v3))
}

/// jpegli's per-image field transform constants for a luma table whose
/// first AC step is `y_quant_01` (8-bit units): `qf = e^x * mul + add`.
fn dampen(y_quant_01: f32, on: bool) -> (f32, f32) {
    const AC_QUANT: f32 = 0.841;
    let base_level = 0.48 * AC_QUANT;
    let d = if on && y_quant_01 >= 9.0 {
        (1.0 - (y_quant_01 - 9.0) / (65.0 - 9.0)).max(0.0)
    } else {
        1.0
    };
    (AC_QUANT * d, (1.0 - d) * base_level)
}

/// The quant field, one value per block, raster order. `luma(by, out)`
/// writes the 8 luma rows of block row `by` (`bw * 8` samples each, the
/// image edge already repeated into the padding) in jpegli sample units.
pub(crate) fn field<F>(
    bw: usize,
    bh: usize,
    y_quant_01: f32,
    dampened: bool,
    threads: usize,
    luma: F,
) -> Vec<f32>
where
    F: Fn(usize, &mut [f32]) + Sync,
{
    // Work items of 8 block rows, each with one block row of context above
    // and below (its luma is converted twice).
    field_chunked(bw, bh, y_quant_01, dampened, threads, 8, luma)
}

fn field_chunked<F>(
    bw: usize,
    bh: usize,
    y_quant_01: f32,
    dampened: bool,
    threads: usize,
    chunk: usize,
    luma: F,
) -> Vec<f32>
where
    F: Fn(usize, &mut [f32]) + Sync,
{
    let pw = bw * 8;
    let (mul, add) = dampen(y_quant_01, dampened);
    let sqrt_mul = sqrtf(211.507_6 * 1e8);
    let chunks: Vec<usize> = (0..bh.div_ceil(chunk)).collect();
    let rows = crate::par::par_map(chunks, threads, |_, c| {
        let (b0, b1) = (c * chunk, ((c + 1) * chunk).min(bh));
        // Luma rows [8 lo, 8 hi): one block row of context above, one below.
        let (lo, hi) = (b0.saturating_sub(1), (b1 + 1).min(bh));
        let mut lum = vec![0.0f32; (hi - lo) * 8 * pw];
        for by in lo..hi {
            luma(by, &mut lum[(by - lo) * 8 * pw..][..8 * pw]);
        }
        let h = bh * 8;
        let row = |y: isize| -> &[f32] {
            let y = y.clamp(0, h as isize - 1) as usize;
            &lum[(y - lo * 8) * pw..][..pw]
        };
        // Pre-erosion rows [p0, p1] (4x4 cells), clamped to the image.
        let (ph, qw) = (bh * 2, bw * 2);
        let p0 = (2 * b0).saturating_sub(1);
        let p1 = (2 * b1).min(ph - 1);
        let mut pre = vec![0.0f32; (p1 + 1 - p0) * qw];
        let mut acc = vec![0.0f32; pw];
        for pr in p0..=p1 {
            for dy in 0..4 {
                let y = (pr * 4 + dy) as isize;
                let (r, rt, rb) = (row(y), row(y - 1), row(y + 1));
                for x in 0..pw {
                    let (xl, xr) = (x.saturating_sub(1), (x + 1).min(pw - 1));
                    let v = r[x];
                    let base = 0.25 * ((r[xr] + r[xl]) + (rb[x] + rt[x]));
                    let g = ratio_of_derivatives::<false>(v + 0.019 / INPUT_SCALING);
                    let d = g * (v - base);
                    let m = masking_sqrt((d * d).min(0.2), sqrt_mul);
                    acc[x] = if dy == 0 { m } else { acc[x] + m };
                }
            }
            let out = &mut pre[(pr - p0) * qw..][..qw];
            for (o, a) in out.iter_mut().zip(acc.as_chunks::<4>().0) {
                *o = (a[0] + a[1] + a[2] + a[3]) * 0.25;
            }
        }
        let pre_at = |x: isize, y: isize| -> f32 {
            let y = y.clamp(0, ph as isize - 1) as usize;
            let x = x.clamp(0, qw as isize - 1) as usize;
            pre[(y - p0) * qw + x]
        };
        // Fuzzy erosion: weighted four smallest of each 3x3 neighbourhood.
        let erode = |x: usize, y: usize| -> f32 {
            let (x, y) = (x as isize, y as isize);
            let mut m = [f32::INFINITY; 4];
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let mut v = pre_at(x + dx, y + dy);
                    for slot in m.iter_mut() {
                        if v < *slot {
                            core::mem::swap(&mut v, slot);
                        }
                    }
                }
            }
            0.125 * m[0] + (0.075 * m[1] + (0.06 * m[2] + 0.05 * m[3]))
        };
        let mut out = vec![0.0f32; (b1 - b0) * bw];
        for by in b0..b1 {
            let rows8: [&[f32]; 8] = core::array::from_fn(|dy| row((by * 8 + dy) as isize));
            for bx in 0..bw {
                let (x, y) = (2 * bx, 2 * by);
                let fuzzy =
                    ((erode(x, y + 1) + erode(x + 1, y + 1)) + erode(x, y)) + erode(x + 1, y);
                let mut v = compute_mask(fuzzy);
                // High-frequency modulation: absolute differences to the right
                // and below, inside the block.
                let mut sum = 0.0f32;
                let x0 = bx * 8;
                for dy in 0..8 {
                    let r = &rows8[dy][x0..x0 + 8];
                    for dx in 0..8 {
                        if dx < 7 {
                            sum += (r[dx] - r[dx + 1]).abs();
                        }
                        if dy < 7 {
                            sum += (r[dx] - rows8[dy + 1][x0 + dx]).abs();
                        }
                    }
                }
                v += sum * (-2.005_219_3 * INPUT_SCALING / 112.0);
                // Gamma modulation: mean inverted derivative ratio, as a log.
                let mut ratio = 0.0f32;
                for r in rows8 {
                    for &p in &r[x0..x0 + 8] {
                        ratio += ratio_of_derivatives::<true>(p + 0.16 / INPUT_SCALING);
                    }
                }
                ratio *= INPUT_SCALING / 64.0;
                v += -0.155_268_78 * ln(ratio as f64) as f32;
                out[(by - b0) * bw + bx] = exp(v as f64) as f32 * mul + add;
            }
        }
        out
    });
    rows.concat()
}

/// jpegli's per-block AQ strength (zenjpeg `AQStrengthMap`):
/// `max(0, 0.6 / qf - 1)`, 0 where the field asks for no extra dead zone.
pub(crate) fn strengths(qf: &[f32]) -> Vec<f32> {
    qf.iter().map(|&q| (0.6 / q - 1.0).max(0.0)).collect()
}

/// AC step codes (`a`, multiplier a / 64) from the field `qf` of a
/// `bw` x `bh` block grid: multiplier `(r / qf)^strength` with `r` the
/// field's geometric mean (`relative`; only the spatial distribution moves)
/// or jpegli's neutral 0.6, averaged in the log domain over `cell` x `cell`
/// blocks (cells aligned to the grid's origin), snapped to `levels` steps
/// per octave (0 = the a / 64 grid) and clamped to [1/4, 4].
pub(crate) fn step_codes(
    qf: &[f32],
    bw: usize,
    bh: usize,
    strength: f32,
    levels: u8,
    cell: usize,
    relative: bool,
) -> Vec<u8> {
    let cell = cell.max(1);
    let lnq = |q: f32| ln(q.max(1e-6) as f64);
    let log_ref = if relative {
        qf.iter().map(|&q| lnq(q)).sum::<f64>() / qf.len().max(1) as f64
    } else {
        ln(0.6)
    };
    let (cw, ch) = (bw.div_ceil(cell), bh.div_ceil(cell));
    let mut cells = vec![0f64; cw * ch];
    let mut counts = vec![0u32; cw * ch];
    for by in 0..bh {
        for bx in 0..bw {
            let c = (by / cell) * cw + bx / cell;
            cells[c] += strength as f64 * (log_ref - lnq(qf[by * bw + bx]));
            counts[c] += 1;
        }
    }
    let ln2 = core::f64::consts::LN_2;
    // Round half away from zero without std.
    let round = |x: f64| (if x < 0.0 { x - 0.5 } else { x + 0.5 }) as i64 as f64;
    let codes: Vec<u8> = cells
        .iter()
        .zip(&counts)
        .map(|(&s, &n)| {
            let mut log2m = s / n as f64 / ln2;
            if levels > 0 {
                log2m = round(log2m * levels as f64) / levels as f64;
            }
            let m = exp(log2m.clamp(-2.0, 2.0) * ln2);
            round(m * 64.0).clamp(16.0, 255.0) as u8
        })
        .collect();
    (0..bw * bh)
        .map(|i| codes[(i / bw / cell) * cw + (i % bw) / cell])
        .collect()
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    fn run(
        w: usize,
        h: usize,
        px: impl Fn(usize, usize) -> f32 + Sync,
        yq: f32,
        chunk: usize,
    ) -> Vec<f32> {
        let (bw, bh) = (w.div_ceil(8), h.div_ceil(8));
        field_chunked(bw, bh, yq, true, 3, chunk, |by, out| {
            for dy in 0..8 {
                let y = (by * 8 + dy).min(h - 1);
                for x in 0..bw * 8 {
                    out[dy * bw * 8 + x] = px(x.min(w - 1), y);
                }
            }
        })
    }

    #[test]
    fn flat_input_gives_a_flat_field_and_chunk_borders_add_nothing() {
        let f = run(72, 200, |_, _| 128.0, 3.0, 8);
        assert!(f.iter().all(|&v| v == f[0] && v.is_finite() && v > 0.0));
        // One work item for the whole image has no internal borders: every
        // chunking must reproduce it bit for bit.
        let tex = |x: usize, y: usize| ((x * 37 + y * 101) % 97) as f32 * 2.5 + (y % 13) as f32;
        let whole = run(100, 301, tex, 3.0, 64);
        for chunk in [1, 2, 3, 8] {
            assert_eq!(run(100, 301, tex, 3.0, chunk), whole, "chunk {chunk}");
        }
        // Busy texture asks for coarser quantization than flat gray.
        let mean = whole.iter().sum::<f32>() / whole.len() as f32;
        assert!(mean < f[0], "{mean} vs flat {}", f[0]);
    }

    #[test]
    fn coarse_quantization_fades_the_field_to_jpeglis_constant() {
        let tex = |x: usize, y: usize| ((x * 31 + y * 17) % 50) as f32 * 4.0;
        let f = run(64, 64, tex, 65.0, 8);
        assert!(f.iter().all(|&v| (v - 0.48 * 0.841).abs() < 1e-6));
        let half = run(64, 64, tex, 37.0, 8);
        let full = run(64, 64, tex, 9.0, 8);
        for (h, f) in half.iter().zip(&full) {
            assert!((h - (0.5 * f + 0.5 * 0.48 * 0.841)).abs() < 1e-5);
        }
    }

    #[test]
    fn strengths_follow_jpeglis_formula() {
        assert_eq!(strengths(&[0.3, 0.6, 1.2]), vec![1.0, 0.0, 0.0]);
    }

    /// AQ step codes from a field: a constant field is all ×1 (relative, or
    /// absolute at jpegli's 0.6), cells share one code, levels snap.
    #[test]
    fn step_codes_from_a_field() {
        let (bw, bh) = (9usize, 5usize);
        assert!(
            step_codes(&vec![0.37; bw * bh], bw, bh, 1.0, 0, 1, true)
                .iter()
                .all(|&a| a == 64)
        );
        assert!(
            step_codes(&vec![0.6; bw * bh], bw, bh, 2.0, 4, 2, false)
                .iter()
                .all(|&a| a == 64)
        );
        let qf: Vec<f32> = (0..bw * bh).map(|i| 0.2 + 0.05 * (i % 7) as f32).collect();
        let c = step_codes(&qf, bw, bh, 1.0, 0, 3, true);
        for i in 0..bw * bh {
            let tl = (i / bw / 3 * 3) * bw + (i % bw) / 3 * 3;
            assert_eq!(c[i], c[tl]);
        }
        // Larger field values (finer quantization asked) get smaller codes.
        let c1 = step_codes(&qf, bw, bh, 1.0, 0, 1, true);
        assert!(c1[0] > c1[6], "{} {}", c1[0], c1[6]);
        for &a in &step_codes(&qf, bw, bh, 1.0, 2, 1, true) {
            assert!([45u8, 64, 91, 128, 32].contains(&a), "{a}");
        }
    }
}
