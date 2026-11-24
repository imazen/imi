//! Encoder kernels (not normative): color conversion of planar rows and
//! the forward DCT + quantization of a block row, on magetypes f32x8.

use super::coef::ZIGZAG;
use archmage::prelude::*;

/// For raster byte b (row b of a block) and its 8-bit nonzero pattern, the
/// bits of the zigzag-ordered nonzero mask.
static ZZ_MASK: [[u64; 256]; 8] = {
    let mut inv = [0u8; 64];
    let mut k = 0;
    while k < 64 {
        inv[ZIGZAG[k] as usize] = k as u8;
        k += 1;
    }
    let mut t = [[0u64; 256]; 8];
    let mut row = 0;
    while row < 8 {
        let mut v = 0;
        while v < 256 {
            let mut m = 0u64;
            let mut col = 0;
            while col < 8 {
                if v & (1 << col) != 0 {
                    m |= 1u64 << inv[row * 8 + col];
                }
                col += 1;
            }
            t[row][v] = m;
            v += 1;
        }
        row += 1;
    }
    t
};

/// Forward DCT of every block of one channel's block row. `plane`: 8 rows
/// of `pw` samples; block `bx` writes 64 raster coefficients at
/// `coefs[bx * 64..]`.
#[magetypes(define(f32x8), v3, neon, wasm128, scalar)]
#[cfg(feature = "lab")]
fn fdct_dense_impl(
    token: Token,
    plane: &[f32],
    pw: usize,
    bw: usize,
    cm: &[[f32; 8]; 8],
    coefs: &mut [f32],
) {
    let c: [[f32x8; 8]; 8] =
        core::array::from_fn(|k| core::array::from_fn(|n| f32x8::splat_t(token, cm[k][n])));
    for bx in 0..bw {
        let v: [f32x8; 8] = core::array::from_fn(|y| {
            f32x8::load_t(token, plane[y * pw + bx * 8..][..8].try_into().unwrap())
        });
        // Vertical: t[k] = sum_n C[k][n] v[n] (lanes: x).
        let mut t: [f32x8; 8] = core::array::from_fn(|k| {
            let mut s = c[k][0] * v[0];
            for n in 1..8 {
                s = c[k][n].mul_add_portable(v[n], s);
            }
            s
        });
        f32x8::transpose_8x8(&mut t);
        // Horizontal: o[u] = sum_x C[u][x] t[x] (lanes: k).
        let mut o: [f32x8; 8] = core::array::from_fn(|u| {
            let mut s = c[u][0] * t[0];
            for x in 1..8 {
                s = c[u][x].mul_add_portable(t[x], s);
            }
            s
        });
        f32x8::transpose_8x8(&mut o);
        for (k, row) in o.iter().enumerate() {
            row.store((&mut coefs[bx * 64 + k * 8..][..8]).try_into().unwrap());
        }
    }
}

/// AAN produces coefficients scaled by 8 * AAN[u] * AAN[v]. Keep this
/// normalization separate from the inverse's dequantization factors.
const FDCT_NORMALIZE: [f32; 64] = {
    let mut factors = [0.0; 64];
    let mut i = 0;
    while i < 64 {
        factors[i] = 0.125 / (super::dct::AAN[i % 8] * super::dct::AAN[i / 8]);
        i += 1;
    }
    factors
};

/// Factored forward transform; FMA is permitted here because the encoder
/// does not define the reconstructed pixels. The inverse remains independent.
#[magetypes(define(f32x8), v3, neon, wasm128, scalar)]
fn fdct_aan_impl<const FMA: bool>(
    token: Token,
    plane: &[f32],
    pw: usize,
    bw: usize,
    coefs: &mut [f32],
) {
    let c4 = f32x8::splat_t(token, core::f32::consts::FRAC_1_SQRT_2);
    let c6 = f32x8::splat_t(token, 0.382_683_43);
    let c2mc6 = f32x8::splat_t(token, 0.541_196_1);
    let c2pc6 = f32x8::splat_t(token, 1.306_563);
    let norm: [f32x8; 8] = core::array::from_fn(|y| {
        f32x8::load_t(token, FDCT_NORMALIZE[y * 8..][..8].try_into().unwrap())
    });
    let pass = |v: &mut [f32x8; 8]| {
        let t0 = v[0] + v[7];
        let t7 = v[0] - v[7];
        let t1 = v[1] + v[6];
        let t6 = v[1] - v[6];
        let t2 = v[2] + v[5];
        let t5 = v[2] - v[5];
        let t3 = v[3] + v[4];
        let t4 = v[3] - v[4];
        let e0 = t0 + t3;
        let e3 = t0 - t3;
        let e1 = t1 + t2;
        let e2 = t1 - t2;
        v[0] = e0 + e1;
        v[4] = e0 - e1;
        let z1 = (e2 + e3) * c4;
        v[2] = e3 + z1;
        v[6] = e3 - z1;
        let o0 = t4 + t5;
        let o1 = t5 + t6;
        let o2 = t6 + t7;
        let z5 = (o0 - o2) * c6;
        let z2 = if FMA {
            o0.mul_add_portable(c2mc6, z5)
        } else {
            o0 * c2mc6 + z5
        };
        let z4 = if FMA {
            o2.mul_add_portable(c2pc6, z5)
        } else {
            o2 * c2pc6 + z5
        };
        let z3 = o1 * c4;
        let z11 = t7 + z3;
        let z13 = t7 - z3;
        v[5] = z13 + z2;
        v[3] = z13 - z2;
        v[1] = z11 + z4;
        v[7] = z11 - z4;
    };
    for bx in 0..bw {
        let mut v: [f32x8; 8] = core::array::from_fn(|y| {
            f32x8::load_t(token, plane[y * pw + bx * 8..][..8].try_into().unwrap())
        });
        pass(&mut v);
        f32x8::transpose_8x8(&mut v);
        pass(&mut v);
        f32x8::transpose_8x8(&mut v);
        for y in 0..8 {
            (v[y] * norm[y]).store((&mut coefs[bx * 64 + y * 8..][..8]).try_into().unwrap());
        }
    }
}

#[cfg(feature = "lab")]
pub(crate) static FDCT_MODE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

pub(crate) fn fdct_row(
    plane: &[f32],
    pw: usize,
    bw: usize,
    _cm: &[[f32; 8]; 8],
    coefs: &mut [f32],
) {
    #[cfg(feature = "lab")]
    match FDCT_MODE.load(core::sync::atomic::Ordering::Relaxed) {
        1 => {
            return incant!(
                fdct_dense_impl(plane, pw, bw, _cm, coefs),
                [v3, neon, wasm128, scalar]
            );
        }
        2 => {
            return incant!(
                fdct_aan_impl::<false>(plane, pw, bw, coefs),
                [v3, neon, wasm128, scalar]
            );
        }
        _ => {}
    }
    incant!(
        fdct_aan_impl::<true>(plane, pw, bw, coefs),
        [v3, neon, wasm128, scalar]
    )
}

/// Quantizes a block row of one channel: block `bx` divides its AC steps
/// by nothing extra, or multiplies them by `aq[bx]` when `aq` is given.
/// Writes raster coefficients at `out[(bx * 3 + ch) * 64..]` and the zigzag
/// nonzero mask at `nz[bx * 3 + ch]`.
#[magetypes(define(f32x8, i32x8), v3, neon, wasm128, scalar)]
#[allow(clippy::too_many_arguments)]
fn quant_row_impl(
    token: Token,
    coefs: &[f32],
    bw: usize,
    ch: usize,
    inv_step: &[f32; 64],
    aq: &[f32],
    round: f32,
    out: &mut [i32],
    nz: &mut [u64],
) {
    let freq_zb = round < 0.0;
    let rnd = f32x8::splat_t(token, round.abs());
    // Clamp before conversion: a wide HDR coefficient can exceed i32 even
    // though the encoded magnitude is limited to 22 bits. SIMD conversion
    // of an out-of-range float is not a saturating conversion on every tier.
    // The clamp is an ordered compare and blend, not `min`: a NaN magnitude
    // (0 x inf from a degenerate step, or an overflowed transform) must give
    // the cap on every tier, as x86 `minps` and the scalar `f32::min` do;
    // NEON `fmin` and wasm `f32x4.min` return the NaN, which converted to 0.
    let cap = f32x8::splat_t(token, ((1 << 22) - 1) as f32);
    let zero = f32x8::zero_t(token);
    let izero = i32x8::splat_t(token, 0);
    // Rows without AQ entries take the original per-block loop, so the
    // default (no-AQ) encode path compiles to the same code as before.
    if aq.is_empty() {
        for bx in 0..bw {
            let mut is = *inv_step;
            if let Some(&s) = aq.get(bx) {
                let r = 1.0 / s;
                for v in &mut is[1..] {
                    *v *= r;
                }
            }
            let dst = &mut out[(bx * 3 + ch) * 64..][..64];
            let mut mask = 0u64;
            for k in 0..8 {
                let row = f32x8::load_t(token, coefs[bx * 64 + k * 8..][..8].try_into().unwrap());
                let isk: &[f32; 8] = is[k * 8..k * 8 + 8].try_into().unwrap();
                let row_rnd = if freq_zb {
                    let arr: [f32; 8] = core::array::from_fn(|u| {
                        let f = ((k + u) as f32 / 14.0).clamp(0.0, 1.0);
                        0.46 - 0.13 * f
                    });
                    f32x8::from_array_t(token, arr)
                } else {
                    rnd
                };
                let v = row.abs() * f32x8::load_t(token, isk) + row_rnd;
                let a = f32x8::blend(v.simd_lt(cap), v, cap).to_i32x8();
                let q = i32x8::blend(row.simd_lt(zero).bitcast_to_i32(), izero - a, a);
                q.store((&mut dst[k * 8..k * 8 + 8]).try_into().unwrap());
                let bits = q.simd_ne(izero).bitmask() as usize & 0xFF;
                mask |= ZZ_MASK[k][bits];
            }
            nz[bx * 3 + ch] = mask;
        }
        return;
    }
    // AQ rows: the eight step vectors are loaded once and scaled per block in
    // registers instead of copying and scaling a 64-entry step table per block
    // (measured 2.8-3.8% faster AQ encodes, 2026-10-06). Lane 0 keeps the
    // unscaled DC step. Each scaled element is the same single f32 multiply
    // as the scalar `*=`, so `out` and `nz` are bit-identical.
    let lane0 = f32x8::from_array_t(
        token,
        [f32::from_bits(u32::MAX), 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    );
    let base: [f32x8; 8] =
        core::array::from_fn(|k| f32x8::load_t(token, inv_step[k * 8..][..8].try_into().unwrap()));
    for bx in 0..bw {
        let is: [f32x8; 8] = match aq.get(bx) {
            Some(&s) => {
                let rv = f32x8::splat_t(token, 1.0 / s);
                let mut v: [f32x8; 8] = core::array::from_fn(|k| base[k] * rv);
                v[0] = f32x8::blend(lane0, f32x8::splat_t(token, inv_step[0]), v[0]);
                v
            }
            None => base,
        };
        let dst = &mut out[(bx * 3 + ch) * 64..][..64];
        let mut mask = 0u64;
        for k in 0..8 {
            let row = f32x8::load_t(token, coefs[bx * 64 + k * 8..][..8].try_into().unwrap());
            let row_rnd = if freq_zb {
                let arr: [f32; 8] = core::array::from_fn(|u| {
                    let f = ((k + u) as f32 / 14.0).clamp(0.0, 1.0);
                    0.46 - 0.13 * f
                });
                f32x8::from_array_t(token, arr)
            } else {
                rnd
            };
            let v = row.abs() * is[k] + row_rnd;
            let a = f32x8::blend(v.simd_lt(cap), v, cap).to_i32x8();
            let q = i32x8::blend(row.simd_lt(zero).bitcast_to_i32(), izero - a, a);
            q.store((&mut dst[k * 8..k * 8 + 8]).try_into().unwrap());
            let bits = q.simd_ne(izero).bitmask() as usize & 0xFF;
            mask |= ZZ_MASK[k][bits];
        }
        nz[bx * 3 + ch] = mask;
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn quant_row(
    coefs: &[f32],
    bw: usize,
    ch: usize,
    inv_step: &[f32; 64],
    aq: &[f32],
    round: f32,
    out: &mut [i32],
    nz: &mut [u64],
) {
    incant!(
        quant_row_impl(coefs, bw, ch, inv_step, aq, round, out, nz),
        [v3, neon, wasm128, scalar]
    )
}

/// Planar rows -> coded planes, in place. `xyb`: inputs are linear light,
/// outputs scaled XYB (B - Y in the third plane); else YCoCg of the inputs
/// times `scale`.
#[magetypes(define(f32x8), v3, neon, wasm128, scalar)]
fn color_rows_impl(
    token: Token,
    xyb: bool,
    scale: [f32; 3],
    opsin: &[[f32; 3]; 3],
    bias: f32,
    cbrt_bias: f32,
    transform: Option<&[[f32; 4]; 3]>,
    p0: &mut [f32],
    p1: &mut [f32],
    p2: &mut [f32],
) {
    let s0 = f32x8::splat_t(token, scale[0]);
    let s1 = f32x8::splat_t(token, scale[1]);
    let s2 = f32x8::splat_t(token, scale[2]);
    let half = f32x8::splat_t(token, 0.5);
    let zero = f32x8::zero_t(token);
    let ob = f32x8::splat_t(token, bias);
    let cb = f32x8::splat_t(token, cbrt_bias);
    let m: [[f32x8; 3]; 3] =
        core::array::from_fn(|r| core::array::from_fn(|c| f32x8::splat_t(token, opsin[r][c])));
    // Lossy 0.4 signalled transform: `coded_j = (A_j0*r + A_j1*g + A_j2*b
    // + B_j) * scale_j`, the exact scalar order (no FMA).
    let tm: Option<[[f32x8; 4]; 3]> = transform.map(|t| {
        core::array::from_fn(|r| core::array::from_fn(|c| f32x8::splat_t(token, t[r][c])))
    });
    let n = p0.len().min(p1.len()).min(p2.len()) / 8;
    for i in 0..n {
        let r = i * 8..i * 8 + 8;
        let a = f32x8::load_t(token, p0[r.clone()].try_into().unwrap());
        let b = f32x8::load_t(token, p1[r.clone()].try_into().unwrap());
        let c = f32x8::load_t(token, p2[r.clone()].try_into().unwrap());
        let (o0, o1, o2) = if let Some(tm) = &tm {
            let t: [f32x8; 3] =
                core::array::from_fn(|j| tm[j][0] * a + tm[j][1] * b + tm[j][2] * c + tm[j][3]);
            (t[0] * s0, t[1] * s1, t[2] * s2)
        } else if xyb {
            let lms: [f32x8; 3] = core::array::from_fn(|k| {
                let v = m[k][0] * a + m[k][1] * b + m[k][2] * c + ob;
                v.max(zero).cbrt_midp() - cb
            });
            let x = (lms[0] - lms[1]) * half;
            let y = (lms[0] + lms[1]) * half;
            (y * s0, x * s1, (lms[2] - y) * s2)
        } else {
            let co = a - c;
            let t = c + co * half;
            let cg = b - t;
            ((t + cg * half) * s0, co * s1, cg * s2)
        };
        o0.store((&mut p0[r.clone()]).try_into().unwrap());
        o1.store((&mut p1[r.clone()]).try_into().unwrap());
        o2.store((&mut p2[r]).try_into().unwrap());
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn color_rows(
    xyb: bool,
    scale: [f32; 3],
    opsin: &[[f32; 3]; 3],
    bias: f32,
    cbrt_bias: f32,
    p0: &mut [f32],
    p1: &mut [f32],
    p2: &mut [f32],
) {
    color_rows_transform(xyb, scale, opsin, bias, cbrt_bias, None, p0, p1, p2)
}

/// [`color_rows`] with a signalled linear color transform (Lossy 0.4): the
/// forward `[A | B]` of the file's decode matrix, applied to the code-value
/// inputs in the scalar order before the plane scaling.
#[allow(clippy::too_many_arguments)]
pub(crate) fn color_rows_transform(
    xyb: bool,
    scale: [f32; 3],
    opsin: &[[f32; 3]; 3],
    bias: f32,
    cbrt_bias: f32,
    transform: Option<&[[f32; 4]; 3]>,
    p0: &mut [f32],
    p1: &mut [f32],
    p2: &mut [f32],
) {
    incant!(
        color_rows_impl(xyb, scale, opsin, bias, cbrt_bias, transform, p0, p1, p2),
        [v3, neon, wasm128, scalar]
    )
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    /// Scalar model of `quant_row` semantics: per-element f32 ops only
    /// (the SIMD kernel is elementwise, so results must be bit-equal).
    fn ref_quant_row(
        coefs: &[f32],
        bw: usize,
        ch: usize,
        inv_step: &[f32; 64],
        aq: &[f32],
        round: f32,
        out: &mut [i32],
        nz: &mut [u64],
    ) {
        let mut zz_inv = [0usize; 64];
        for (k, &z) in ZIGZAG.iter().enumerate() {
            zz_inv[z as usize] = k;
        }
        for bx in 0..bw {
            let mut is = *inv_step;
            if let Some(&s) = aq.get(bx) {
                let r = 1.0 / s;
                for v in &mut is[1..] {
                    *v *= r;
                }
            }
            let dst = &mut out[(bx * 3 + ch) * 64..][..64];
            let mut mask = 0u64;
            for k in 0..64 {
                let x = coefs[bx * 64 + k];
                let a = (x.abs() * is[k] + round).min(((1 << 22) - 1) as f32) as i32;
                let q = if x < 0.0 { -a } else { a };
                dst[k] = q;
                if q != 0 {
                    mask |= 1u64 << zz_inv[k];
                }
            }
            nz[bx * 3 + ch] = mask;
        }
    }

    #[test]
    fn quant_row_aq_and_unscaled_paths_match_scalar_model() {
        // Non-uniform steps; the DC step differs sharply from AC steps so a
        // mis-scaled DC lane would be caught by exact comparison.
        let inv_step: [f32; 64] =
            core::array::from_fn(|i| 1.0 / ([0.031, 0.5, 0.9, 1.7][i % 4] + i as f32 * 0.013));
        let mut seed = 7u32;
        let mut rnd_f = move || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / 8388608.0 - 1.0
        };
        // bw=3 but aq covers 2 blocks: block 2 exercises the absent-entry path.
        let bw = 3;
        let cases: &[&[f32]] = &[
            &[],                   // AQ disabled (empty slice)
            &[1.0, 1.0],           // r == 1.0: scaling must be identity
            &[0.5, 2.0],           // AQ clamp bounds (a/64 for a in 32..=128)
            &[1.375, 0.6015625],   // non-power-of-2 r: per-element rounding
            &[f32::INFINITY, 0.0], // degenerate scales: r = 0 / inf
        ];
        for (ci, aq) in cases.iter().enumerate() {
            let coefs: Vec<f32> = (0..bw * 64)
                .map(|i| {
                    let m = [
                        0.0,
                        -0.0,
                        0.49,
                        0.5,
                        0.51, // rounding boundary at round = 0.5
                        1.0,
                        -3.75,
                        255.0,
                        65535.0,
                        1e6,
                        -4_194_304.0, // cap edge
                        f32::MAX,
                        f32::INFINITY,
                        f32::NEG_INFINITY,
                    ][(i + ci * 7) % 14];
                    if i % 11 == 0 {
                        rnd_f() * 300.0 // dense signed coverage
                    } else {
                        m * if i % 2 == 0 { 1.0 } else { -1.0 }
                    }
                })
                .collect();
            let mut out = [0i32; 3 * 3 * 64];
            let mut nz = [0u64; 9];
            let mut ref_out = [0i32; 3 * 3 * 64];
            let mut ref_nz = [0u64; 9];
            for ch in [0usize, 1, 2] {
                quant_row(&coefs, bw, ch, &inv_step, aq, 0.5, &mut out, &mut nz);
                ref_quant_row(
                    &coefs,
                    bw,
                    ch,
                    &inv_step,
                    aq,
                    0.5,
                    &mut ref_out,
                    &mut ref_nz,
                );
            }
            assert_eq!(out, ref_out, "aq case {ci}");
            assert_eq!(nz, ref_nz, "aq case {ci}");
        }
        // The unit-scale case must be bit-identical to no-AQ everywhere.
        let coefs: Vec<f32> = (0..bw * 64).map(|_| rnd_f() * 1e5).collect();
        let mut o0 = [0i32; 3 * 3 * 64];
        let mut n0 = [0u64; 9];
        let mut o1 = [0i32; 3 * 3 * 64];
        let mut n1 = [0u64; 9];
        quant_row(&coefs, bw, 0, &inv_step, &[], 0.5, &mut o0, &mut n0);
        quant_row(&coefs, bw, 0, &inv_step, &[1.0; 3], 0.5, &mut o1, &mut n1);
        assert_eq!(o0, o1);
        assert_eq!(n0, n1);
    }

    /// Every compiled tier of `quant_row_impl` (not just the dispatched one)
    /// against the scalar model, on saturating, infinite and NaN magnitudes:
    /// NaN from 0 x inf (degenerate AQ scales, zero or infinite steps) and
    /// NaN or infinite coefficients. NEON and wasm128 once gave 0 where the
    /// model and x86 give the cap.
    #[test]
    fn quant_row_every_tier_matches_scalar_model_on_nan_and_saturation() {
        let specials = [
            0.0,
            -0.0,
            0.5,
            1.0,
            4_194_302.5,
            4_194_303.0,
            4_194_304.0,
            1e30,
            f32::MAX,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NAN,
            -f32::NAN,
        ];
        let bw = 4;
        let coefs: Vec<f32> = (0..bw * 64)
            .map(|i| specials[i % specials.len()] * if i / 13 % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        let steps: [[f32; 64]; 3] = [
            core::array::from_fn(|i| 1.0 / (0.5 + i as f32 * 0.25)),
            // Zero and infinite inverse steps: 0 x inf and inf x 0 lanes.
            core::array::from_fn(|i| [0.0, f32::INFINITY, 1.0, 1e-30][i % 4]),
            [1.0; 64],
        ];
        let aqs: [&[f32]; 4] = [
            &[],
            &[1.0, 0.5, 2.0],
            &[f32::INFINITY, 0.0, 1.5, 0.75],
            &[0.0; 4],
        ];
        let mut tiers = 0;
        for (si, inv) in steps.iter().enumerate() {
            for (ai, aq) in aqs.iter().enumerate() {
                let mut want = vec![0i32; bw * 3 * 64];
                let mut want_nz = vec![0u64; bw * 3];
                ref_quant_row(&coefs, bw, 1, inv, aq, 0.5, &mut want, &mut want_nz);
                let check = |tier: &str, out: &[i32], nz: &[u64]| {
                    assert_eq!(out, &want[..], "{tier} steps {si} aq {ai}");
                    assert_eq!(nz, &want_nz[..], "{tier} steps {si} aq {ai}");
                };
                let mut out = vec![0i32; bw * 3 * 64];
                let mut nz = vec![0u64; bw * 3];
                quant_row_impl_scalar(
                    archmage::ScalarToken,
                    &coefs,
                    bw,
                    1,
                    inv,
                    aq,
                    0.5,
                    &mut out,
                    &mut nz,
                );
                check("scalar", &out, &nz);
                tiers = 1;
                #[cfg(target_arch = "x86_64")]
                if let Some(t) = <archmage::X64V3Token as archmage::SimdToken>::summon() {
                    quant_row_impl_v3(t, &coefs, bw, 1, inv, aq, 0.5, &mut out, &mut nz);
                    check("v3", &out, &nz);
                    tiers = 2;
                }
                #[cfg(target_arch = "aarch64")]
                if let Some(t) = <archmage::NeonToken as archmage::SimdToken>::summon() {
                    quant_row_impl_neon(t, &coefs, bw, 1, inv, aq, 0.5, &mut out, &mut nz);
                    check("neon", &out, &nz);
                    tiers = 2;
                }
                #[cfg(target_arch = "wasm32")]
                if let Some(t) = <archmage::Wasm128Token as archmage::SimdToken>::summon() {
                    quant_row_impl_wasm128(t, &coefs, bw, 1, inv, aq, 0.5, &mut out, &mut nz);
                    check("wasm128", &out, &nz);
                    tiers = 2;
                }
            }
        }
        // NEON is baseline on aarch64; x86-64 runs v3 where the CPU has it.
        if cfg!(target_arch = "aarch64") {
            assert_eq!(tiers, 2, "NEON tier not run");
        }
        let _ = tiers;
    }

    /// `color_rows` on every compiled tier against the scalar tier, on
    /// finite inputs large enough to overflow the opsin mix (and tiny,
    /// negative and zero ones): each output bit-equal, or NaN on both, so
    /// what reaches `quant_row` is the same on every tier.
    #[test]
    fn color_rows_every_tier_agrees_on_extreme_input() {
        let vals = [
            0.0f32,
            -0.0,
            1e-30,
            -1e-30,
            0.18,
            1.0,
            -1.0,
            100.0,
            1e10,
            1e30,
            -1e30,
            3e38,
            -3e38,
            f32::MAX,
            f32::MIN,
            f32::MIN_POSITIVE,
        ];
        let n = 64;
        let plane =
            |o: usize| -> Vec<f32> { (0..n).map(|i| vals[(i * 7 + o) % vals.len()]).collect() };
        let opsin = super::super::color::OPSIN_F32;
        let same = |a: &[f32], b: &[f32]| {
            a.iter()
                .zip(b)
                .all(|(x, y)| x.to_bits() == y.to_bits() || (x.is_nan() && y.is_nan()))
        };
        for xyb in [true, false] {
            let run_scalar = || {
                let (mut a, mut b, mut c) = (plane(0), plane(5), plane(11));
                color_rows_impl_scalar(
                    archmage::ScalarToken,
                    xyb,
                    [1.0, 2.0, 0.5],
                    &opsin,
                    super::super::color::OPSIN_BIAS,
                    super::super::color::CBRT_BIAS,
                    None,
                    &mut a,
                    &mut b,
                    &mut c,
                );
                (a, b, c)
            };
            let want = run_scalar();
            // The extremes overflow: some outputs are not finite.
            assert!(
                want.0
                    .iter()
                    .chain(&want.1)
                    .chain(&want.2)
                    .any(|v| !v.is_finite()),
                "xyb {xyb}: no overflow exercised"
            );
            #[allow(unused_mut)]
            let mut got: Vec<(&str, (Vec<f32>, Vec<f32>, Vec<f32>))> = Vec::new();
            #[cfg(target_arch = "x86_64")]
            if let Some(t) = <archmage::X64V3Token as archmage::SimdToken>::summon() {
                let (mut a, mut b, mut c) = (plane(0), plane(5), plane(11));
                color_rows_impl_v3(
                    t,
                    xyb,
                    [1.0, 2.0, 0.5],
                    &opsin,
                    super::super::color::OPSIN_BIAS,
                    super::super::color::CBRT_BIAS,
                    None,
                    &mut a,
                    &mut b,
                    &mut c,
                );
                got.push(("v3", (a, b, c)));
            }
            #[cfg(target_arch = "aarch64")]
            if let Some(t) = <archmage::NeonToken as archmage::SimdToken>::summon() {
                let (mut a, mut b, mut c) = (plane(0), plane(5), plane(11));
                color_rows_impl_neon(
                    t,
                    xyb,
                    [1.0, 2.0, 0.5],
                    &opsin,
                    super::super::color::OPSIN_BIAS,
                    super::super::color::CBRT_BIAS,
                    None,
                    &mut a,
                    &mut b,
                    &mut c,
                );
                got.push(("neon", (a, b, c)));
            }
            #[cfg(target_arch = "wasm32")]
            if let Some(t) = <archmage::Wasm128Token as archmage::SimdToken>::summon() {
                let (mut a, mut b, mut c) = (plane(0), plane(5), plane(11));
                color_rows_impl_wasm128(
                    t,
                    xyb,
                    [1.0, 2.0, 0.5],
                    &opsin,
                    super::super::color::OPSIN_BIAS,
                    super::super::color::CBRT_BIAS,
                    None,
                    &mut a,
                    &mut b,
                    &mut c,
                );
                got.push(("wasm128", (a, b, c)));
            }
            for (tier, (a, b, c)) in &got {
                assert!(same(a, &want.0), "{tier} xyb {xyb} plane 0");
                assert!(same(b, &want.1), "{tier} xyb {xyb} plane 1");
                assert!(same(c, &want.2), "{tier} xyb {xyb} plane 2");
            }
        }
    }

    /// Scalar model of `color_rows`: the XYB path reuses the crate's own
    /// scalar reference (`forward_xyb_linear`); the non-XYB path replicates

    #[test]
    fn quantization_caps_before_integer_conversion() {
        let magnitudes = [
            0.0,
            0.25,
            1.0,
            1e6,
            4194304.0,
            2147483648.0,
            f32::MAX,
            f32::INFINITY,
        ];
        let coefs: [f32; 64] =
            core::array::from_fn(|i| magnitudes[i % 8] * if i / 8 % 2 == 0 { 1.0 } else { -1.0 });
        let mut out = [0; 192];
        let mut nz = [0; 3];
        quant_row(&coefs, 1, 0, &[1.0; 64], &[], 0.5, &mut out, &mut nz);
        for (i, &v) in coefs.iter().enumerate() {
            let magnitude = (v.abs() as f64 + 0.5).min(4194303.0) as i32;
            assert_eq!(out[i], if v < 0.0 { -magnitude } else { magnitude });
        }
    }

    #[test]
    fn factored_fdct_matches_f64_definition() {
        // Independent DCT-II definition, including impulses at every input
        // position, random signs, DC and a wide dynamic range. This catches
        // normalization, transpose, sign and cancellation mistakes.
        let basis: [[f64; 8]; 8] = core::array::from_fn(|k| {
            core::array::from_fn(|n| {
                let scale = if k == 0 { 1.0 / 8.0f64.sqrt() } else { 0.5 };
                scale * (core::f64::consts::PI * k as f64 * (2 * n + 1) as f64 / 16.0).cos()
            })
        });
        let mut seed = 31u32;
        for case in 0..260 {
            let amplitude = [1.0, 255.0, 65535.0, 1e12][case % 4];
            let x: [f32; 64] = core::array::from_fn(|i| {
                if case < 64 {
                    if i == case { amplitude } else { 0.0 }
                } else if case == 64 {
                    amplitude
                } else {
                    seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    ((seed >> 8) as f32 / 8388608.0 - 1.0) * amplitude
                }
            });
            let expected: [f64; 64] = core::array::from_fn(|i| {
                let (v, u) = (i / 8, i % 8);
                (0..64)
                    .map(|n| x[n] as f64 * basis[v][n / 8] * basis[u][n % 8])
                    .sum()
            });
            let mut fused = [0.0; 64];
            let mut unfused = [0.0; 64];
            incant!(
                fdct_aan_impl::<true>(&x, 8, 1, &mut fused),
                [v3, neon, wasm128, scalar]
            );
            incant!(
                fdct_aan_impl::<false>(&x, 8, 1, &mut unfused),
                [v3, neon, wasm128, scalar]
            );
            for got in [fused, unfused] {
                for i in 0..64 {
                    assert!(
                        (got[i] as f64 - expected[i]).abs() <= amplitude as f64 * 3e-6,
                        "case {case} coefficient {i}: {} vs {}",
                        got[i],
                        expected[i]
                    );
                }
            }
        }
    }
}
