//! 8x8 DCT.
//!
//! The inverse is normative: the scaled float IDCT of Arai, Agui and
//! Nakajima (as in libjpeg's `jidctflt`), columns first, then rows, each
//! operation a single f32 operation in the order written (no fused
//! multiply-add). Its input scaling is folded into the dequantization
//! factors (see [`dequant_factors`]). The forward transform is encoder-only.

/// AAN scale factors: 1, then cos(k*pi/16) * sqrt(2).
pub(crate) const AAN: [f32; 8] = [
    1.0,
    1.387_039_8,
    1.306_563,
    1.175_875_6,
    1.0,
    0.785_694_96,
    0.541_196_1,
    0.275_899_38,
];

pub(crate) const AAN_R2: f32 = core::f32::consts::SQRT_2;
pub(crate) const AAN_C1: f32 = 1.847_759;
pub(crate) const AAN_C2: f32 = 1.082_392_2;
pub(crate) const AAN_C3: f32 = 2.613_126;
use AAN_C1 as C1;
use AAN_C2 as C2;
use AAN_C3 as C3;
use AAN_R2 as R2;

/// Dequantization factors for raster position i = v * 8 + u:
/// step * (AAN[u] * AAN[v] * 0.125).
pub(crate) fn dequant_factors(steps: &[f32; 64]) -> [f32; 64] {
    let mut d = [0.0f32; 64];
    for (i, v) in d.iter_mut().enumerate() {
        let s = AAN[i % 8] * AAN[i / 8] * 0.125;
        *v = steps[i] * s;
    }
    d
}

/// One 8-point inverse pass over `x[o], x[o + st], ..., x[o + 7 st]`.
#[inline(always)]
fn idct8(x: &mut [f32; 64], o: usize, st: usize) {
    let i = |k: usize| o + k * st;
    // Even part.
    let tmp10 = x[i(0)] + x[i(4)];
    let tmp11 = x[i(0)] - x[i(4)];
    let tmp13 = x[i(2)] + x[i(6)];
    let tmp12 = (x[i(2)] - x[i(6)]) * R2 - tmp13;
    let e0 = tmp10 + tmp13;
    let e3 = tmp10 - tmp13;
    let e1 = tmp11 + tmp12;
    let e2 = tmp11 - tmp12;
    // Odd part.
    let z13 = x[i(5)] + x[i(3)];
    let z10 = x[i(5)] - x[i(3)];
    let z11 = x[i(1)] + x[i(7)];
    let z12 = x[i(1)] - x[i(7)];
    let t7 = z11 + z13;
    let t11 = (z11 - z13) * R2;
    let z5 = (z10 + z12) * C1;
    let t10 = z5 - z12 * C2;
    let t12 = z5 - z10 * C3;
    let t6 = t12 - t7;
    let t5 = t11 - t6;
    let t4 = t10 - t5;
    x[i(0)] = e0 + t7;
    x[i(7)] = e0 - t7;
    x[i(1)] = e1 + t6;
    x[i(6)] = e1 - t6;
    x[i(2)] = e2 + t5;
    x[i(5)] = e2 - t5;
    x[i(3)] = e3 + t4;
    x[i(4)] = e3 - t4;
}

/// Normative 2D inverse of a dequantized block (raster order, already
/// multiplied by [`dequant_factors`]): columns, then rows.
pub(crate) fn idct8x8(x: &mut [f32; 64]) {
    for u in 0..8 {
        idct8(x, u, 8);
    }
    for v in 0..8 {
        idct8(x, v * 8, 1);
    }
}

/// Orthonormal DCT-II basis, C[k][n].
#[cfg(feature = "encode")]
pub(crate) fn dct_matrix() -> [[f32; 8]; 8] {
    // cos(k * (2n + 1) * pi / 16) for the 32 distinct angles, from the
    // AAN constants: cos(m * pi / 16) = AAN[m] / sqrt(2) for m = 1..7.
    let cos16 = |m: usize| -> f64 {
        let m = m % 32;
        let (m, sign) = if m > 16 { (32 - m, 1.0) } else { (m, 1.0) };
        let (m, sign) = if m > 8 { (16 - m, -sign) } else { (m, sign) };
        let c = if m == 0 {
            1.0
        } else if m == 8 {
            0.0
        } else {
            AAN[m] as f64 / core::f64::consts::SQRT_2
        };
        sign * c
    };
    let mut c = [[0.0f32; 8]; 8];
    for (k, row) in c.iter_mut().enumerate() {
        let ck = if k == 0 {
            core::f64::consts::FRAC_1_SQRT_2 * 0.5
        } else {
            0.5
        };
        for (n, v) in row.iter_mut().enumerate() {
            *v = (ck * cos16(k * (2 * n + 1))) as f32;
        }
    }
    c
}

/// Orthonormal forward 2D DCT of a raster block (reference for tests).
#[cfg(all(test, feature = "encode"))]
pub(crate) fn fdct8x8(c: &[[f32; 8]; 8], x: &[f32; 64]) -> [f32; 64] {
    let mut t = [0.0f32; 64];
    for y in 0..8 {
        for k in 0..8 {
            let mut s = 0.0;
            for n in 0..8 {
                s += c[k][n] * x[y * 8 + n];
            }
            t[y * 8 + k] = s;
        }
    }
    let mut o = [0.0f32; 64];
    for u in 0..8 {
        for k in 0..8 {
            let mut s = 0.0;
            for n in 0..8 {
                s += c[k][n] * t[n * 8 + u];
            }
            o[k * 8 + u] = s;
        }
    }
    o
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    #[test]
    fn aan_idct_inverts_orthonormal_dct() {
        let c = dct_matrix();
        let mut seed = 7u32;
        for _ in 0..200 {
            let mut x = [0.0f32; 64];
            for v in x.iter_mut() {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                *v = (seed >> 24) as f32 - 128.0;
            }
            let coef = fdct8x8(&c, &x);
            let ones = [1.0f32; 64];
            let dq = dequant_factors(&ones);
            let mut y = [0.0f32; 64];
            for i in 0..64 {
                y[i] = coef[i] * dq[i];
            }
            idct8x8(&mut y);
            for i in 0..64 {
                assert!((y[i] - x[i]).abs() < 1e-3, "{i}: {} vs {}", y[i], x[i]);
            }
        }
    }

    #[test]
    fn dct_matrix_is_orthonormal() {
        let c = dct_matrix();
        for a in 0..8 {
            for b in 0..8 {
                let d: f32 = (0..8).map(|n| c[a][n] * c[b][n]).sum();
                let e = if a == b { 1.0 } else { 0.0 };
                assert!((d - e).abs() < 1e-6, "{a} {b} {d}");
            }
        }
    }
}
