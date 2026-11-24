//! Portable SIMD (magetypes f32x8) kernels for lossy reconstruction.
//!
//! Each kernel performs exactly the operations of the scalar reference
//! ([`super::dct::idct8x8`], [`super::color::inverse_px`]) in the same
//! order, lane-wise, so output is bit-identical on every tier. Only
//! operations with identical semantics on every backend are used: IEEE
//! add/sub/mul, min/max-based clamps on finite values, round-to-nearest-even
//! through the +2^23 adder, and truncating conversion of in-range values.

use super::color::{
    CBRT_BIAS, INV_OPSIN, LossyColor, OPSIN_BIAS, SRGB8_LOG_BASE, SRGB8_LOG_N, SRGB8_LOG8K,
};
use super::dct::{AAN_C1, AAN_C2, AAN_C3, AAN_R2};
use archmage::prelude::*;

/// Dequantizes and inverse-transforms blocks `bx0..bx1` of block row `r` of
/// a stripe into `planes` (3 channels x 8 rows x `pw`; block `bx` at column
/// `(bx - bx0) * 8`). `q` holds the stripe's
/// coefficients `[(block * 3 + ch) * 64 + raster]`, `eobs` the EOB of each
/// (block, ch); blocks with no AC coefficients take a flat fill.
#[magetypes(define(f32x8, i32x8), v3, neon, wasm128, scalar)]
fn recon_block_row_impl<const FMA: bool, const SPLIT: bool>(
    token: Token,
    q: &mut [i32],
    eobs: &[u8],
    r: usize,
    bw: usize,
    bx0: usize,
    bx1: usize,
    dq: &[[f32; 64]; 3],
    aq: &[f32],
    bias: Option<&[[[f32; 64]; 2]; 3]>,
    planes: &mut [f32],
    pw: usize,
    small: Small<'_>,
    clear_q: bool,
) {
    let r2 = f32x8::splat_t(token, AAN_R2);
    let (m1, p1) = (i32x8::splat_t(token, -1), i32x8::splat_t(token, 1));
    let c1 = f32x8::splat_t(token, AAN_C1);
    let c2 = f32x8::splat_t(token, AAN_C2);
    let c3 = f32x8::splat_t(token, AAN_C3);
    let one = f32x8::splat_t(token, 1.0);
    let idx = f32x8::from_array_t(token, [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
    let m0 = idx.simd_lt(f32x8::splat_t(token, 0.5));
    // One 8-point pass over v[0..8] (lane-wise), identical to dct::idct8.
    let pass = |v: &mut [f32x8; 8]| {
        let tmp10 = v[0] + v[4];
        let tmp11 = v[0] - v[4];
        let tmp13 = v[2] + v[6];
        let tmp12 = if FMA {
            (v[2] - v[6]).mul_sub_portable(r2, tmp13)
        } else {
            (v[2] - v[6]) * r2 - tmp13
        };
        let e0 = tmp10 + tmp13;
        let e3 = tmp10 - tmp13;
        let e1 = tmp11 + tmp12;
        let e2 = tmp11 - tmp12;
        let z13 = v[5] + v[3];
        let z10 = v[5] - v[3];
        let z11 = v[1] + v[7];
        let z12 = v[1] - v[7];
        let t7 = z11 + z13;
        let t11 = (z11 - z13) * r2;
        let z5 = (z10 + z12) * c1;
        let t10 = if FMA {
            z12.mul_add_portable(-c2, z5)
        } else {
            z5 - z12 * c2
        };
        let t12 = if FMA {
            z10.mul_add_portable(-c3, z5)
        } else {
            z5 - z10 * c3
        };
        let t6 = t12 - t7;
        let t5 = t11 - t6;
        let t4 = t10 - t5;
        v[0] = e0 + t7;
        v[7] = e0 - t7;
        v[1] = e1 + t6;
        v[6] = e1 - t6;
        v[2] = e2 + t5;
        v[5] = e2 - t5;
        v[3] = e3 + t4;
        v[4] = e3 - t4;
    };
    // Rows without 4x4 planes (SPLIT false) compile without their code.
    let k4 = super::small::Masks::new(token);
    for bx in bx0..bx1 {
        let b = r * bw + bx;
        let split = if SPLIT {
            small.split.get(b).copied().unwrap_or(0)
        } else {
            0
        };
        let aq_s = aq.get(bx).copied();
        let has_aq = matches!(aq_s, Some(s) if s != 1.0);
        let a = aq_s.unwrap_or(1.0);
        let aqv = f32x8::splat_t(token, a);
        let aqv0 = f32x8::blend(m0, one, aqv);
        for ch in 0..3 {
            let qb = &mut q[(b * 3 + ch) * 64..][..64];
            let base = ch * 8 * pw + (bx - bx0) * 8;
            if SPLIT && split >> ch & 1 != 0 {
                // A 4x4 plane (SPEC 2.11): no reconstruction bias. Without
                // AC its pixels equal the 8x8 flat value (slot 0's factor is
                // the 8x8 DC factor), so the flat fill below serves both.
                let dq4 = &small.dq[ch];
                if eobs[b * 3 + ch] == 0 {
                    let v = qb[0] as f32 * dq4[0];
                    if clear_q {
                        qb[0] = 0;
                    }
                    for y in 0..8 {
                        planes[base + y * pw..][..8].fill(v);
                    }
                    continue;
                }
                let v: [f32x8; 8] = core::array::from_fn(|y| {
                    let qi: &[i32; 8] = qb[y * 8..y * 8 + 8].try_into().unwrap();
                    let dd: &[f32; 8] = dq4[y * 8..y * 8 + 8].try_into().unwrap();
                    let dv = if has_aq {
                        f32x8::load_t(token, dd) * if y == 0 { aqv0 } else { aqv }
                    } else {
                        f32x8::load_t(token, dd)
                    };
                    f32x8::from_i32x8_t(token, i32x8::load_t(token, qi)) * dv
                });
                let haar = if has_aq {
                    super::small::haar_ref(
                        qb[0] as f32 * dq4[0],
                        qb[1] as f32 * (dq4[1] * a),
                        qb[8] as f32 * (dq4[8] * a),
                        qb[9] as f32 * (dq4[9] * a),
                    )
                } else {
                    super::small::haar_of(qb, dq4)
                };
                if clear_q {
                    qb.fill(0);
                }
                let o = super::small::block4(token, &k4, v, haar);
                for (y, row) in o.iter().enumerate() {
                    let out: &mut [f32; 8] =
                        (&mut planes[base + y * pw..][..8]).try_into().unwrap();
                    row.store(out);
                }
                continue;
            }
            let dqc = &dq[ch];
            if eobs[b * 3 + ch] == 0 {
                let v = qb[0] as f32 * dqc[0];
                if clear_q {
                    qb[0] = 0;
                }
                for y in 0..8 {
                    planes[base + y * pw..][..8].fill(v);
                }
                continue;
            }
            // Rows of the coefficient block: v[row][col] = coef(v=row, u=col).
            let mut v: [f32x8; 8] = if let Some(bt) = bias {
                // Lossy 0.4 bias: (q - sign(q) * B) * d, B by |q| = 1 or >= 2.
                let bt = &bt[ch];
                core::array::from_fn(|y| {
                    let qi: &[i32; 8] = qb[y * 8..y * 8 + 8].try_into().unwrap();
                    let dd: &[f32; 8] = dqc[y * 8..y * 8 + 8].try_into().unwrap();
                    let dv = if has_aq {
                        f32x8::load_t(token, dd) * if y == 0 { aqv0 } else { aqv }
                    } else {
                        f32x8::load_t(token, dd)
                    };
                    let b1: &[f32; 8] = bt[0][y * 8..y * 8 + 8].try_into().unwrap();
                    let b2: &[f32; 8] = bt[1][y * 8..y * 8 + 8].try_into().unwrap();
                    let qv = i32x8::load_t(token, qi);
                    let sg = qv.clamp(m1, p1);
                    let b = f32x8::blend(
                        qv.simd_eq(sg).bitcast_to_f32(),
                        f32x8::load_t(token, b1),
                        f32x8::load_t(token, b2),
                    );
                    let t = f32x8::from_i32x8_t(token, qv) - f32x8::from_i32x8_t(token, sg) * b;
                    t * dv
                })
            } else {
                core::array::from_fn(|y| {
                    let qi: &[i32; 8] = qb[y * 8..y * 8 + 8].try_into().unwrap();
                    let dd: &[f32; 8] = dqc[y * 8..y * 8 + 8].try_into().unwrap();
                    let dv = if has_aq {
                        f32x8::load_t(token, dd) * if y == 0 { aqv0 } else { aqv }
                    } else {
                        f32x8::load_t(token, dd)
                    };
                    f32x8::from_i32x8_t(token, i32x8::load_t(token, qi)) * dv
                })
            };
            // Keep the scratch invariant: coefficients read are zeroed.
            if clear_q {
                qb.fill(0);
            }
            // Columns: one pass across the row vectors.
            pass(&mut v);
            // Rows: transpose, pass, transpose back.
            f32x8::transpose_8x8(&mut v);
            pass(&mut v);
            f32x8::transpose_8x8(&mut v);
            for (y, row) in v.iter().enumerate() {
                let o: &mut [f32; 8] = (&mut planes[base + y * pw..][..8]).try_into().unwrap();
                row.store(o);
            }
        }
    }
}

/// The 4x4 planes of a block row (SPEC 2.11): `split[b]` has bit `ch` when
/// plane `ch` of block `b` (the stripe's block index, as `eobs`) is coded as
/// four 4x4 transforms (empty: none), `dq` their dequantization factors
/// ([`super::small::dequant4`]).
#[derive(Clone, Copy)]
pub(crate) struct Small<'a> {
    pub split: &'a [u8],
    pub dq: &'a [[f32; 64]; 3],
    /// LAB: the integer candidate's inverse (`small_lab.rs`, scalar route).
    pub int: bool,
}

impl Small<'_> {
    /// No 4x4 planes.
    pub(crate) const NONE: Small<'static> = Small {
        split: &[],
        dq: &[[0.0; 64]; 3],
        int: false,
    };
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn recon_block_row(
    q: &mut [i32],
    eobs: &[u8],
    r: usize,
    bw: usize,
    bx0: usize,
    bx1: usize,
    dq: &[[f32; 64]; 3],
    aq: &[f32],
    bias: Option<&[[[f32; 64]; 2]; 3]>,
    planes: &mut [f32],
    pw: usize,
    small: Small<'_>,
    clear_q: bool,
) {
    // `IMIJ_SCALAR` must reach every decoder path: the callers in `mod` only
    // gate the entropy kernels on `force_scalar`, so the wrappers here do
    // their own check to keep the scalar reference a real oracle.
    if !small.split.is_empty() {
        return recon_block_row_split(
            q, eobs, r, bw, bx0, bx1, dq, aq, bias, planes, pw, small, clear_q,
        );
    }
    if crate::v1::force_scalar() {
        #[cfg(feature = "lab")]
        if super::IDCT_FMA.load(core::sync::atomic::Ordering::Relaxed) {
            return recon_block_row_impl_scalar::<true, false>(
                ScalarToken,
                q,
                eobs,
                r,
                bw,
                bx0,
                bx1,
                dq,
                aq,
                bias,
                planes,
                pw,
                small,
                clear_q,
            );
        }
        return recon_block_row_impl_scalar::<false, false>(
            ScalarToken,
            q,
            eobs,
            r,
            bw,
            bx0,
            bx1,
            dq,
            aq,
            bias,
            planes,
            pw,
            small,
            clear_q,
        );
    }
    #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
    if !crate::v1::force_avx2()
        && let Some(t) = archmage::X64V4Token::summon()
    {
        #[cfg(feature = "lab")]
        if super::IDCT_FMA.load(core::sync::atomic::Ordering::Relaxed) {
            return super::recon_v4::recon_block_row::<true>(
                t, q, eobs, r, bw, bx0, bx1, dq, aq, bias, planes, pw, clear_q,
            );
        }
        return super::recon_v4::recon_block_row::<false>(
            t, q, eobs, r, bw, bx0, bx1, dq, aq, bias, planes, pw, clear_q,
        );
    }
    #[cfg(feature = "lab")]
    if super::IDCT_FMA.load(core::sync::atomic::Ordering::Relaxed) {
        return incant!(
            recon_block_row_impl::<true, false>(
                q, eobs, r, bw, bx0, bx1, dq, aq, bias, planes, pw, small, clear_q
            ),
            [v3, neon, wasm128, scalar]
        );
    }
    incant!(
        recon_block_row_impl::<false, false>(
            q, eobs, r, bw, bx0, bx1, dq, aq, bias, planes, pw, small, clear_q
        ),
        [v3, neon, wasm128, scalar]
    )
}

/// [`recon_block_row`] of a block row with 4x4 planes (`small.split` not
/// empty): the same tiers, the 8x8 planes as there (the lab FMA switch
/// covers 8x8 rows only).
#[allow(clippy::too_many_arguments)]
fn recon_block_row_split(
    q: &mut [i32],
    eobs: &[u8],
    r: usize,
    bw: usize,
    bx0: usize,
    bx1: usize,
    dq: &[[f32; 64]; 3],
    aq: &[f32],
    bias: Option<&[[[f32; 64]; 2]; 3]>,
    planes: &mut [f32],
    pw: usize,
    small: Small<'_>,
    clear_q: bool,
) {
    if crate::v1::force_scalar() {
        return recon_block_row_impl_scalar::<false, true>(
            ScalarToken,
            q,
            eobs,
            r,
            bw,
            bx0,
            bx1,
            dq,
            aq,
            bias,
            planes,
            pw,
            small,
            clear_q,
        );
    }
    // AVX-512 hosts take the 8-lane kernel too: on Zen 4 its 4x4 planes
    // cost 15.2 ns against 17.8 ns for two-block AVX-512 registers, 8x8
    // planes 17.1 against 16.6 (kernel probe, benchmarks/smallxf_2026-10-11).
    incant!(
        recon_block_row_impl::<false, true>(
            q, eobs, r, bw, bx0, bx1, dq, aq, bias, planes, pw, small, clear_q
        ),
        [v3, neon, wasm128, scalar]
    )
}

/// Tests: [`recon_block_row`] on tier `tier` (1: the f32x8 kernel on the
/// best x86 / ARM / wasm tier without AVX-512; 2: the scalar instantiation).
#[cfg(all(test, feature = "encode"))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn recon_block_row_tier(
    tier: u8,
    q: &mut [i32],
    eobs: &[u8],
    r: usize,
    bw: usize,
    bx0: usize,
    bx1: usize,
    dq: &[[f32; 64]; 3],
    aq: &[f32],
    bias: Option<&[[[f32; 64]; 2]; 3]>,
    planes: &mut [f32],
    pw: usize,
    small: Small<'_>,
) {
    if tier == 2 {
        return recon_block_row_impl_scalar::<false, true>(
            ScalarToken,
            q,
            eobs,
            r,
            bw,
            bx0,
            bx1,
            dq,
            aq,
            bias,
            planes,
            pw,
            small,
            true,
        );
    }
    incant!(
        recon_block_row_impl::<false, true>(
            q, eobs, r, bw, bx0, bx1, dq, aq, bias, planes, pw, small, true
        ),
        [v3, neon, wasm128, scalar]
    )
}

/// Color-converts one row of 8-aligned XYB planes into `w` RGB8 pixels.
#[magetypes(define(f32x8, i32x8), v3, neon, wasm128, scalar)]
fn xyb_row_impl(
    token: Token,
    p0: &[f32],
    p1: &[f32],
    p2: &[f32],
    inv: [f32; 3],
    out: &mut [u8],
    w: usize,
) {
    let lut = &SRGB8_LOG8K;
    let i0 = f32x8::splat_t(token, inv[0]);
    let i1 = f32x8::splat_t(token, inv[1]);
    let i2 = f32x8::splat_t(token, inv[2]);
    let zero = f32x8::zero_t(token);
    let one = f32x8::splat_t(token, 1.0);
    let bias = f32x8::splat_t(token, CBRT_BIAS);
    let lo = f32x8::splat_t(token, -64.0);
    let hi = f32x8::splat_t(token, 64.0);
    let ob = f32x8::splat_t(token, OPSIN_BIAS);
    let base = i32x8::splat_t(token, SRGB8_LOG_BASE);
    let imin = i32x8::splat_t(token, 0);
    let imax = i32x8::splat_t(token, SRGB8_LOG_N - 1);
    let m: [[f32x8; 3]; 3] =
        core::array::from_fn(|r| core::array::from_fn(|c| f32x8::splat_t(token, INV_OPSIN[r][c])));
    let conv = |a: &[f32; 8], bb: &[f32; 8], c: &[f32; 8]| -> [[i32; 8]; 3] {
        let y = f32x8::load_t(token, a) * i0;
        let xx = f32x8::load_t(token, bb) * i1;
        let b = f32x8::load_t(token, c) * i2 + y;
        let l =
            incant!(crate::wasm_relaxed::clamp_f32x8(token, y + xx + bias, lo, hi) without token);
        let mm =
            incant!(crate::wasm_relaxed::clamp_f32x8(token, y - xx + bias, lo, hi) without token);
        let s = incant!(crate::wasm_relaxed::clamp_f32x8(token, b + bias, lo, hi) without token);
        let l3 = l * l * l - ob;
        let m3 = mm * mm * mm - ob;
        let s3 = s * s * s - ob;
        core::array::from_fn(|ch| {
            let lin = m[ch][0] * l3 + m[ch][1] * m3 + m[ch][2] * s3;
            let bits =
                incant!(crate::wasm_relaxed::clamp_f32x8(token, lin, zero, one) without token)
                    .bitcast_to_i32();
            (bits.shr_arithmetic_const::<14>() - base)
                .clamp(imin, imax)
                .to_array()
        })
    };
    let px = |idx: &[[i32; 8]; 3], i: usize| -> [u8; 3] {
        [
            lut[idx[0][i] as usize & 8191],
            lut[idx[1][i] as usize & 8191],
            lut[idx[2][i] as usize & 8191],
        ]
    };
    rows8(p0, p1, p2, out, w, |a, b, c| conv(a, b, c), px);
}

/// Color-converts one row of 8-aligned YCoCg planes into `w` RGB8 pixels.
#[magetypes(define(f32x8, i32x8), v3, neon, wasm128, scalar)]
fn ycocg_row_impl(
    token: Token,
    p0: &[f32],
    p1: &[f32],
    p2: &[f32],
    inv: [f32; 3],
    out: &mut [u8],
    w: usize,
) {
    let i0 = f32x8::splat_t(token, inv[0]);
    let i1 = f32x8::splat_t(token, inv[1]);
    let i2 = f32x8::splat_t(token, inv[2]);
    let zero = f32x8::zero_t(token);
    let h = f32x8::splat_t(token, 0.5);
    let max = f32x8::splat_t(token, 255.0);
    let big = f32x8::splat_t(token, 8_388_608.0);
    let q = |v: f32x8| {
        let c = incant!(crate::wasm_relaxed::clamp_f32x8(token, v, zero, max) without token);
        incant!(crate::wasm_relaxed::to_i32x8(token, (c + big) - big) without token).to_array()
    };
    let conv = |a: &[f32; 8], bb: &[f32; 8], c: &[f32; 8]| -> [[i32; 8]; 3] {
        let y = f32x8::load_t(token, a) * i0;
        let co = f32x8::load_t(token, bb) * i1;
        let cg = f32x8::load_t(token, c) * i2;
        let t = y - cg * h;
        let g = cg + t;
        let b = t - co * h;
        let r = b + co;
        [q(r), q(g), q(b)]
    };
    let px =
        |v: &[[i32; 8]; 3], i: usize| -> [u8; 3] { [v[0][i] as u8, v[1][i] as u8, v[2][i] as u8] };
    rows8(p0, p1, p2, out, w, conv, px);
}

/// Signalled linear color transform (Lossy 0.4): coded planes -> RGB8,
/// `rgb_j = M_j0*c0 + M_j1*c1 + M_j2*c2 + O_j` in the scalar order.
#[magetypes(define(f32x8, i32x8), v3, neon, wasm128, scalar)]
fn linear_row_impl(
    token: Token,
    p0: &[f32],
    p1: &[f32],
    p2: &[f32],
    inv: [f32; 3],
    m: &[[f32; 4]; 3],
    out: &mut [u8],
    w: usize,
) {
    let i0 = f32x8::splat_t(token, inv[0]);
    let i1 = f32x8::splat_t(token, inv[1]);
    let i2 = f32x8::splat_t(token, inv[2]);
    let zero = f32x8::zero_t(token);
    let max = f32x8::splat_t(token, 255.0);
    let big = f32x8::splat_t(token, 8_388_608.0);
    let mm: [[f32x8; 4]; 3] =
        core::array::from_fn(|j| core::array::from_fn(|k| f32x8::splat_t(token, m[j][k])));
    let q = |v: f32x8| ((v.clamp(zero, max) + big) - big).to_i32x8().to_array();
    let conv = |a: &[f32; 8], b: &[f32; 8], c: &[f32; 8]| -> [[i32; 8]; 3] {
        let c0 = f32x8::load_t(token, a) * i0;
        let c1 = f32x8::load_t(token, b) * i1;
        let c2 = f32x8::load_t(token, c) * i2;
        core::array::from_fn(|j| q(mm[j][0] * c0 + mm[j][1] * c1 + mm[j][2] * c2 + mm[j][3]))
    };
    let px =
        |v: &[[i32; 8]; 3], i: usize| -> [u8; 3] { [v[0][i] as u8, v[1][i] as u8, v[2][i] as u8] };
    rows8(p0, p1, p2, out, w, conv, px);
}

pub(crate) fn linear_row(
    p0: &[f32],
    p1: &[f32],
    p2: &[f32],
    inv: [f32; 3],
    m: &[[f32; 4]; 3],
    out: &mut [u8],
    w: usize,
) {
    if crate::v1::force_scalar() {
        return linear_row_impl_scalar(ScalarToken, p0, p1, p2, inv, m, out, w);
    }
    incant!(
        linear_row_impl(p0, p1, p2, inv, m, out, w),
        [v3, neon, wasm128, scalar]
    )
}

/// Runs `conv` over 8-pixel groups of three plane rows and writes `w`
/// RGB8 pixels; `px(v, i)` gives pixel i of a converted group.
#[inline(always)]
fn rows8<V>(
    p0: &[f32],
    p1: &[f32],
    p2: &[f32],
    out: &mut [u8],
    w: usize,
    mut conv: impl FnMut(&[f32; 8], &[f32; 8], &[f32; 8]) -> V,
    px: impl Fn(&V, usize) -> [u8; 3],
) {
    let full = w / 8;
    let (head, tail) = out[..w * 3].split_at_mut(full * 24);
    let mut it = p0
        .as_chunks::<8>()
        .0
        .iter()
        .zip(p1.as_chunks::<8>().0)
        .zip(p2.as_chunks::<8>().0)
        .map(|((a, b), c)| (a, b, c));
    for o in head.as_chunks_mut::<24>().0 {
        let (a, b, c) = it.next().unwrap();
        let v = conv(a, b, c);
        for i in 0..8 {
            let p = px(&v, i);
            o[i * 3] = p[0];
            o[i * 3 + 1] = p[1];
            o[i * 3 + 2] = p[2];
        }
    }
    if !tail.is_empty() {
        let (a, b, c) = it.next().unwrap();
        let v = conv(a, b, c);
        for (i, o) in tail.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            *o = px(&v, i);
        }
    }
}

/// XYB planes -> `w` linear RGB pixels (f32, interleaved).
#[magetypes(define(f32x8), v3, neon, wasm128, scalar)]
fn xyb_row_linear_impl(
    token: Token,
    p0: &[f32],
    p1: &[f32],
    p2: &[f32],
    inv: [f32; 3],
    out: &mut [f32],
    w: usize,
) {
    let i0 = f32x8::splat_t(token, inv[0]);
    let i1 = f32x8::splat_t(token, inv[1]);
    let i2 = f32x8::splat_t(token, inv[2]);
    let bias = f32x8::splat_t(token, CBRT_BIAS);
    let lo = f32x8::splat_t(token, -64.0);
    let hi = f32x8::splat_t(token, 64.0);
    let ob = f32x8::splat_t(token, OPSIN_BIAS);
    let m: [[f32x8; 3]; 3] =
        core::array::from_fn(|r| core::array::from_fn(|c| f32x8::splat_t(token, INV_OPSIN[r][c])));
    let out = &mut out[..w * 3];
    let mut x = 0;
    for ((a, b), c) in p0
        .as_chunks::<8>()
        .0
        .iter()
        .zip(p1.as_chunks::<8>().0)
        .zip(p2.as_chunks::<8>().0)
    {
        if x >= w {
            break;
        }
        let y = f32x8::load_t(token, a) * i0;
        let xx = f32x8::load_t(token, b) * i1;
        let bb = f32x8::load_t(token, c) * i2 + y;
        let l =
            incant!(crate::wasm_relaxed::clamp_f32x8(token, y + xx + bias, lo, hi) without token);
        let mm =
            incant!(crate::wasm_relaxed::clamp_f32x8(token, y - xx + bias, lo, hi) without token);
        let s = incant!(crate::wasm_relaxed::clamp_f32x8(token, bb + bias, lo, hi) without token);
        let l3 = l * l * l - ob;
        let m3 = mm * mm * mm - ob;
        let s3 = s * s * s - ob;
        let v: [[f32; 8]; 3] =
            core::array::from_fn(|ch| (m[ch][0] * l3 + m[ch][1] * m3 + m[ch][2] * s3).to_array());
        for i in 0..(w - x).min(8) {
            for ch in 0..3 {
                out[(x + i) * 3 + ch] = v[ch][i];
            }
        }
        x += 8;
    }
}

pub(crate) fn xyb_row_linear(
    p0: &[f32],
    p1: &[f32],
    p2: &[f32],
    inv: [f32; 3],
    out: &mut [f32],
    w: usize,
) {
    if crate::v1::force_scalar() {
        return xyb_row_linear_impl_scalar(ScalarToken, p0, p1, p2, inv, out, w);
    }
    incant!(
        xyb_row_linear_impl(p0, p1, p2, inv, out, w),
        [v3, neon, wasm128, scalar]
    )
}

/// Linear values -> native-endian 16-bit codes through a transfer table;
/// bit-identical to [`super::transfer::encode16`] per sample.
#[magetypes(define(f32x8, i32x8), v3, neon, wasm128, scalar)]
fn encode16_row_impl<const CUSTOM: bool>(
    token: Token,
    lin: &[f32],
    table: &[f32],
    lmax: f32,
    out: &mut [u8],
    ranges: [f32; 3],
    bps: usize,
) {
    use super::transfer::{TABLE_BASE, TABLE_N};
    let zero = f32x8::zero_t(token);
    let lm = f32x8::splat_t(token, lmax);
    let base = i32x8::splat_t(token, TABLE_BASE);
    let imin = i32x8::splat_t(token, 0);
    let imax = i32x8::splat_t(token, TABLE_N - 1);
    let low = i32x8::splat_t(token, 0xFFFF);
    let inv = f32x8::splat_t(token, 1.0 / 65536.0);
    let max = f32x8::splat_t(token, 65535.0);
    let big = f32x8::splat_t(token, 8_388_608.0);
    let range: [f32x8; 3] = core::array::from_fn(|phase| {
        f32x8::from_array_t(token, core::array::from_fn(|j| ranges[(phase + j) % 3]))
    });
    let factors: [f32x8; 3] = core::array::from_fn(|phase| {
        f32x8::from_array_t(
            token,
            core::array::from_fn(|j| ranges[(phase + j) % 3] / 65535.0),
        )
    });
    let n = lin.len().min(out.len() / bps);
    let (chunks, _) = lin[..n].as_chunks::<8>();
    for (i, c) in chunks.iter().enumerate() {
        let b = incant!(crate::wasm_relaxed::clamp_f32x8(token, f32x8::load_t(token, c), zero, lm) without token)
            .bitcast_to_i32();
        let idx = (b.shr_arithmetic_const::<16>() - base)
            .clamp(imin, imax)
            .to_array();
        let frac = (b & low).to_f32x8() * inv;
        let t0: [f32; 8] = core::array::from_fn(|j| table[idx[j] as usize]);
        let t1: [f32; 8] = core::array::from_fn(|j| table[idx[j] as usize + 1]);
        let (t0, t1) = (
            f32x8::from_array_t(token, t0),
            f32x8::from_array_t(token, t1),
        );
        let v = t0 + (t1 - t0) * frac;
        let (v, upper) = if CUSTOM {
            (v * factors[(i * 8) % 3], range[(i * 8) % 3])
        } else {
            (v, max)
        };
        let c = incant!(crate::wasm_relaxed::clamp_f32x8(token, v, zero, upper) without token);
        let q =
            incant!(crate::wasm_relaxed::to_i32x8(token, (c + big) - big) without token).to_array();
        if CUSTOM && bps == 1 {
            let o: &mut [u8; 8] = (&mut out[i * 8..i * 8 + 8]).try_into().unwrap();
            for j in 0..8 {
                o[j] = q[j] as u8;
            }
        } else {
            // Keep one slice check per vector, including the historical
            // full16 path. Individual dynamic subslices inhibit elimination.
            let o: &mut [u8; 16] = (&mut out[i * 16..i * 16 + 16]).try_into().unwrap();
            for j in 0..8 {
                o[j * 2..j * 2 + 2].copy_from_slice(&(q[j] as u16).to_ne_bytes());
            }
        }
    }
    for k in chunks.len() * 8..n {
        let v = if CUSTOM {
            super::transfer::encode_code(table, lmax, lin[k], ranges[k % 3])
        } else {
            super::transfer::encode16(table, lmax, lin[k])
        };
        if CUSTOM && bps == 1 {
            out[k] = v as u8;
        } else {
            out[k * bps..k * bps + 2].copy_from_slice(&v.to_ne_bytes());
        }
    }
}

pub(crate) fn encode16_row(lin: &[f32], table: &[f32], lmax: f32, out: &mut [u8]) {
    if crate::v1::force_scalar() {
        return encode16_row_impl_scalar::<false>(
            ScalarToken,
            lin,
            table,
            lmax,
            out,
            [65535.0; 3],
            2,
        );
    }
    incant!(
        encode16_row_impl::<false>(lin, table, lmax, out, [65535.0; 3], 2),
        [v3, neon, wasm128, scalar]
    )
}

pub(crate) fn encode_range_row(
    lin: &[f32],
    table: &[f32],
    lmax: f32,
    out: &mut [u8],
    ranges: [f32; 3],
    bps: usize,
) {
    if crate::v1::force_scalar() {
        return encode16_row_impl_scalar::<true>(ScalarToken, lin, table, lmax, out, ranges, bps);
    }
    incant!(
        encode16_row_impl::<true>(lin, table, lmax, out, ranges, bps),
        [v3, neon, wasm128, scalar]
    )
}

/// YCoCg planes -> `w` native-endian 16-bit RGB pixels.
#[magetypes(define(f32x8, i32x8), v3, neon, wasm128, scalar)]
fn ycocg_row16_impl<const CUSTOM: bool>(
    token: Token,
    p0: &[f32],
    p1: &[f32],
    p2: &[f32],
    inv: [f32; 3],
    out: &mut [u8],
    w: usize,
    ranges: [f32; 3],
    full: f32,
    bps: usize,
) {
    let i0 = f32x8::splat_t(token, inv[0]);
    let i1 = f32x8::splat_t(token, inv[1]);
    let i2 = f32x8::splat_t(token, inv[2]);
    let zero = f32x8::zero_t(token);
    let h = f32x8::splat_t(token, 0.5);
    let max = f32x8::splat_t(token, 65535.0);
    let big = f32x8::splat_t(token, 8_388_608.0);
    let upper = ranges.map(|r| f32x8::splat_t(token, r));
    let factors = ranges.map(|r| f32x8::splat_t(token, r / full));
    let q = |v: f32x8, ch: usize| {
        let (v, hi) = if CUSTOM {
            (v * factors[ch], upper[ch])
        } else {
            (v, max)
        };
        let c = incant!(crate::wasm_relaxed::clamp_f32x8(token, v, zero, hi) without token);
        incant!(crate::wasm_relaxed::to_i32x8(token, (c + big) - big) without token).to_array()
    };
    let mut x = 0;
    let (g0, g1, g2) = (
        p0.as_chunks::<8>().0,
        p1.as_chunks::<8>().0,
        p2.as_chunks::<8>().0,
    );
    let out = &mut out[..w * 3 * bps];
    for ((a, b), c) in g0.iter().zip(g1).zip(g2) {
        if x >= w {
            break;
        }
        let y = f32x8::load_t(token, a) * i0;
        let co = f32x8::load_t(token, b) * i1;
        let cg = f32x8::load_t(token, c) * i2;
        let t = y - cg * h;
        let g = cg + t;
        let bl = t - co * h;
        let r = bl + co;
        let v = [q(r, 0), q(g, 1), q(bl, 2)];
        for i in 0..(w - x).min(8) {
            for ch in 0..3 {
                let o = ((x + i) * 3 + ch) * bps;
                if CUSTOM && bps == 1 {
                    out[o] = v[ch][i] as u8;
                } else {
                    out[o..o + 2].copy_from_slice(&(v[ch][i] as u16).to_ne_bytes());
                }
            }
        }
        x += 8;
    }
}

pub(crate) fn ycocg_row16(
    p0: &[f32],
    p1: &[f32],
    p2: &[f32],
    inv: [f32; 3],
    out: &mut [u8],
    w: usize,
) {
    if crate::v1::force_scalar() {
        return ycocg_row16_impl_scalar::<false>(
            ScalarToken,
            p0,
            p1,
            p2,
            inv,
            out,
            w,
            [65535.0; 3],
            65535.0,
            2,
        );
    }
    incant!(
        ycocg_row16_impl::<false>(p0, p1, p2, inv, out, w, [65535.0; 3], 65535.0, 2),
        [v3, neon, wasm128, scalar]
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn ycocg_range_row(
    p0: &[f32],
    p1: &[f32],
    p2: &[f32],
    inv: [f32; 3],
    out: &mut [u8],
    w: usize,
    ranges: [f32; 3],
    full: f32,
    bps: usize,
) {
    if crate::v1::force_scalar() {
        return ycocg_row16_impl_scalar::<true>(
            ScalarToken,
            p0,
            p1,
            p2,
            inv,
            out,
            w,
            ranges,
            full,
            bps,
        );
    }
    incant!(
        ycocg_row16_impl::<true>(p0, p1, p2, inv, out, w, ranges, full, bps),
        [v3, neon, wasm128, scalar]
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn color_row(
    color: LossyColor,
    p0: &[f32],
    p1: &[f32],
    p2: &[f32],
    inv: [f32; 3],
    out: &mut [u8],
    w: usize,
) {
    if crate::v1::force_scalar() {
        return match color {
            LossyColor::Xyb => xyb_row_impl_scalar(ScalarToken, p0, p1, p2, inv, out, w),
            LossyColor::YCoCg => ycocg_row_impl_scalar(ScalarToken, p0, p1, p2, inv, out, w),
        };
    }
    match color {
        LossyColor::Xyb => {
            #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
            if !crate::v1::force_avx2()
                && let Some(t) = archmage::X64V4xToken::summon()
            {
                let d = super::color_v4x::xyb_row::<false>(t, p0, p1, p2, inv, out, w);
                if d < w {
                    let (q0, q1, q2) = (&p0[d..], &p1[d..], &p2[d..]);
                    let o = &mut out[d * 3..];
                    incant!(
                        xyb_row_impl(q0, q1, q2, inv, o, w - d),
                        [v3, neon, wasm128, scalar]
                    );
                }
                return;
            }
            incant!(
                xyb_row_impl(p0, p1, p2, inv, out, w),
                [v3, neon, wasm128, scalar]
            )
        }
        LossyColor::YCoCg => {
            #[cfg(target_arch = "x86_64")]
            if let Some(t) = archmage::X64V3Token::summon() {
                let d = super::color_v3::ycocg_row(t, p0, p1, p2, inv, out, w);
                if d < w {
                    let (q0, q1, q2) = (&p0[d..], &p1[d..], &p2[d..]);
                    let o = &mut out[d * 3..];
                    incant!(
                        ycocg_row_impl(q0, q1, q2, inv, o, w - d),
                        [v3, neon, wasm128, scalar]
                    );
                }
                return;
            }
            incant!(
                ycocg_row_impl(p0, p1, p2, inv, out, w),
                [v3, neon, wasm128, scalar]
            )
        }
    }
}

// Bit-identity of the relaxed wasm128 arms: each kernel is run with the same
// adversarial finite plane values through the relaxed build and the scalar
// reference and must produce identical output bytes. The value set covers
// every clamp boundary, both zero signs, subnormals, huge finite values and
// the cube/transfer domain edges, i.e. the full input range the kernels can
// see from a decode (lossy reconstruction cannot produce NaN/inf, see
// `wasm_relaxed.rs`).
#[cfg(all(test, target_arch = "x86_64"))]
#[test]
fn ycocg_rgb8_avx2_matches_scalar_for_strided_rows_and_tails() {
    if archmage::X64V3Token::summon().is_none() {
        return; // The AVX2 path is unavailable on this CPU.
    }
    for width in [
        1usize, 7, 8, 15, 16, 17, 31, 32, 33, 255, 256, 257, 511, 512, 513,
    ] {
        let plane_stride = width.div_ceil(8) * 8 + 8;
        let stride = width * 3 + 13;
        let samples = |seed: usize| -> Vec<f32> {
            (0..3 * plane_stride)
                .map(|i| ((i * seed + i / 11 * 7) % 2331) as f32 * 0.25 - 230.0)
                .collect()
        };
        let (a, b, c) = (samples(79), samples(241), samples(877));
        for inv in [[1.0; 3], [0.70710677, 1.25, 0.625]] {
            let mut avx2 = alloc::vec![0xa7; stride * 3];
            let mut reference = avx2.clone();
            for row in 0..3 {
                let at = row * plane_stride;
                let (p0, p1, p2) = (
                    &a[at..][..plane_stride],
                    &b[at..][..plane_stride],
                    &c[at..][..plane_stride],
                );
                let at = row * stride;
                color_row(
                    LossyColor::YCoCg,
                    p0,
                    p1,
                    p2,
                    inv,
                    &mut avx2[at..at + stride],
                    width,
                );
                ycocg_row_impl_scalar(
                    ScalarToken,
                    p0,
                    p1,
                    p2,
                    inv,
                    &mut reference[at..at + stride],
                    width,
                );
            }
            assert_eq!(avx2, reference, "width {width}, inverse scales {inv:?}");
        }
    }
}

#[cfg(all(
    all(test, feature = "encode"),
    target_arch = "wasm32",
    target_feature = "relaxed-simd"
))]
mod relaxed_wasm_tests {
    use super::*;
    use archmage::{ScalarToken, SimdToken, Wasm128Token};

    fn t() -> Wasm128Token {
        Wasm128Token::summon().expect("wasm128 compile-time feature")
    }

    // Values every plane lane cycles through; finite, spanning the clamps,
    // the rounding trick, both zero signs and subnormals.
    const VALUES: [f32; 16] = [
        0.0,
        -0.0,
        f32::from_bits(1),
        -f32::from_bits(1),
        63.999_996,
        -63.999_996,
        64.0,
        -64.0,
        65_535.0,
        -65_535.0,
        255.0,
        -255.0,
        1.0,
        -1.0,
        1.0e30,
        -1.0e30,
    ];

    fn plane(mut seed: u64, w: usize) -> Vec<f32> {
        (0..w)
            .map(|_| {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let pick = (seed >> 60) as usize % (VALUES.len() + 1);
                if pick == VALUES.len() {
                    let bits = (seed as u32) & 0x7F7F_FFFF | 0x0080_0000;
                    f32::from_bits(bits)
                } else {
                    VALUES[pick]
                }
            })
            .collect()
    }

    // Plane rows are padded to full 8-lane chunks, like the decoder's planes
    // (the kernels read the tail group as one more full chunk).
    fn rows(w: usize) -> Vec<(Vec<f32>, Vec<f32>, Vec<f32>)> {
        let pw = w.div_ceil(8) * 8;
        (0..24)
            .map(|i| {
                (
                    plane(3 * i + 1, pw),
                    plane(3 * i + 2, pw),
                    plane(3 * i + 3, pw),
                )
            })
            .collect()
    }

    #[test]
    fn xyb_rgb8_relaxed_matches_scalar() {
        let t = t();
        for w in [8, 10, 16, 23] {
            for (p0, p1, p2) in rows(w) {
                let (mut a, mut b) = (vec![0u8; w * 3], vec![0u8; w * 3]);
                xyb_row_impl_wasm128(t, &p0, &p1, &p2, [1.0, 1.0, 1.0], &mut a, w);
                xyb_row_impl_scalar(ScalarToken, &p0, &p1, &p2, [1.0, 1.0, 1.0], &mut b, w);
                assert_eq!(a, b, "w {w}");
            }
        }
    }

    #[test]
    fn xyb_linear_relaxed_matches_scalar() {
        let t = t();
        for w in [8, 10, 16, 23] {
            for (p0, p1, p2) in rows(w) {
                let (mut a, mut b) = (vec![0.0f32; w * 3], vec![0.0f32; w * 3]);
                xyb_row_linear_impl_wasm128(t, &p0, &p1, &p2, [1.0, 1.0, 1.0], &mut a, w);
                xyb_row_linear_impl_scalar(ScalarToken, &p0, &p1, &p2, [1.0, 1.0, 1.0], &mut b, w);
                // f32 sample bits, not values: -0 and NaN payloads included.
                for (x, y) in a.iter().zip(&b) {
                    assert_eq!(x.to_bits(), y.to_bits(), "w {w}");
                }
            }
        }
    }

    #[test]
    fn ycocg_rgb8_relaxed_matches_scalar() {
        let t = t();
        for w in [8, 10, 16, 23] {
            for (p0, p1, p2) in rows(w) {
                let (mut a, mut b) = (vec![0u8; w * 3], vec![0u8; w * 3]);
                ycocg_row_impl_wasm128(t, &p0, &p1, &p2, [1.0, 1.0, 1.0], &mut a, w);
                ycocg_row_impl_scalar(ScalarToken, &p0, &p1, &p2, [1.0, 1.0, 1.0], &mut b, w);
                assert_eq!(a, b, "w {w}");
            }
        }
    }

    #[test]
    fn transfer16_relaxed_matches_scalar() {
        use crate::lossy::transfer::{TABLE_BASE, TABLE_N};
        let t = t();
        // A finite monotone table over the kernel's index range.
        let table: Vec<f32> = (0..=TABLE_N).map(|i| i as f32 * 0.25).collect();
        let lmax = f32::from_bits((TABLE_BASE as u32 + TABLE_N as u32) << 16);
        for w in [8, 16, 23] {
            for (lin, _, _) in rows(w) {
                let (mut a, mut b) = (vec![0u8; w * 2 + 8], vec![0u8; w * 2 + 8]);
                encode16_row_impl_wasm128::<false>(t, &lin, &table, lmax, &mut a, [65535.0; 3], 2);
                encode16_row_impl_scalar::<false>(
                    ScalarToken,
                    &lin,
                    &table,
                    lmax,
                    &mut b,
                    [65535.0; 3],
                    2,
                );
                assert_eq!(a, b, "w {w}");
            }
        }
    }

    #[test]
    fn ycocg16_relaxed_matches_scalar() {
        let t = t();
        for w in [8, 10, 16, 23] {
            for (p0, p1, p2) in rows(w) {
                let (mut a, mut b) = (vec![0u8; w * 3 * 2], vec![0u8; w * 3 * 2]);
                ycocg_row16_impl_wasm128::<false>(
                    t,
                    &p0,
                    &p1,
                    &p2,
                    [1.0, 1.0, 1.0],
                    &mut a,
                    w,
                    [65535.0; 3],
                    65535.0,
                    2,
                );
                ycocg_row16_impl_scalar::<false>(
                    ScalarToken,
                    &p0,
                    &p1,
                    &p2,
                    [1.0, 1.0, 1.0],
                    &mut b,
                    w,
                    [65535.0; 3],
                    65535.0,
                    2,
                );
                assert_eq!(a, b, "w {w}");
            }
        }
    }
}
