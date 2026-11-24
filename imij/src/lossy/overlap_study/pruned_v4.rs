//! Study-only seven-output variant of lossy/recon_v4.rs; production is unchanged.
//! AVX-512 dequantization + inverse DCT of a block row, two horizontally
//! adjacent blocks per zmm (block bx in lanes 0..8, bx + 1 in lanes 8..16).
//!
//! Bit-identical to [`super::simd`]'s recon kernel and to
//! [`super::dct::idct8x8`]: the same IEEE operations in the same order, lane
//! for lane; transposes only move data. A flat block (no AC) paired with a
//! non-flat one goes through the transform: with only the DC nonzero, every
//! AAN output is exactly the DC value (x + 0, x - 0, 0 * c), so the result
//! equals the flat fill.

use super::dct::{AAN_C1, AAN_C2, AAN_C3, AAN_R2};
use archmage::prelude::*;

/// Indices for the cross-128 step of an 8x8 transpose done independently
/// in each 256-bit half: out_lo takes 128-bit chunks (a.q0, b.q0, a.q2,
/// b.q2), out_hi (a.q1, b.q1, a.q3, b.q3).
static PERM_LO: [u32; 16] = [0, 1, 2, 3, 16, 17, 18, 19, 8, 9, 10, 11, 24, 25, 26, 27];
static PERM_HI: [u32; 16] = [4, 5, 6, 7, 20, 21, 22, 23, 12, 13, 14, 15, 28, 29, 30, 31];

/// Transposes the two 8x8 matrices held in the 256-bit halves of `r`.
#[rite(import_intrinsics)]
fn transpose2(_t: X64V4Token, r: &mut [__m512; 8], plo: __m512i, phi: __m512i) {
    let t0 = _mm512_unpacklo_ps(r[0], r[1]);
    let t1 = _mm512_unpackhi_ps(r[0], r[1]);
    let t2 = _mm512_unpacklo_ps(r[2], r[3]);
    let t3 = _mm512_unpackhi_ps(r[2], r[3]);
    let t4 = _mm512_unpacklo_ps(r[4], r[5]);
    let t5 = _mm512_unpackhi_ps(r[4], r[5]);
    let t6 = _mm512_unpacklo_ps(r[6], r[7]);
    let t7 = _mm512_unpackhi_ps(r[6], r[7]);
    let u0 = _mm512_shuffle_ps::<0x44>(t0, t2);
    let u1 = _mm512_shuffle_ps::<0xEE>(t0, t2);
    let u2 = _mm512_shuffle_ps::<0x44>(t1, t3);
    let u3 = _mm512_shuffle_ps::<0xEE>(t1, t3);
    let u4 = _mm512_shuffle_ps::<0x44>(t4, t6);
    let u5 = _mm512_shuffle_ps::<0xEE>(t4, t6);
    let u6 = _mm512_shuffle_ps::<0x44>(t5, t7);
    let u7 = _mm512_shuffle_ps::<0xEE>(t5, t7);
    r[0] = _mm512_permutex2var_ps(u0, plo, u4);
    r[1] = _mm512_permutex2var_ps(u1, plo, u5);
    r[2] = _mm512_permutex2var_ps(u2, plo, u6);
    r[3] = _mm512_permutex2var_ps(u3, plo, u7);
    r[4] = _mm512_permutex2var_ps(u0, phi, u4);
    r[5] = _mm512_permutex2var_ps(u1, phi, u5);
    r[6] = _mm512_permutex2var_ps(u2, phi, u6);
    r[7] = _mm512_permutex2var_ps(u3, phi, u7);
}

/// One 8-point AAN pass over v[0..8], lane-wise (as `dct::idct8`).
#[rite(import_intrinsics)]
fn pass<const FMA: bool>(_t: X64V4Token, v: &mut [__m512; 8]) {
    let r2 = _mm512_set1_ps(AAN_R2);
    let c1 = _mm512_set1_ps(AAN_C1);
    let c2 = _mm512_set1_ps(AAN_C2);
    let c3 = _mm512_set1_ps(AAN_C3);
    let tmp10 = _mm512_add_ps(v[0], v[4]);
    let tmp11 = _mm512_sub_ps(v[0], v[4]);
    let tmp13 = _mm512_add_ps(v[2], v[6]);
    let tmp12 = if FMA {
        _mm512_fmsub_ps(_mm512_sub_ps(v[2], v[6]), r2, tmp13)
    } else {
        _mm512_sub_ps(_mm512_mul_ps(_mm512_sub_ps(v[2], v[6]), r2), tmp13)
    };
    let e0 = _mm512_add_ps(tmp10, tmp13);
    let e3 = _mm512_sub_ps(tmp10, tmp13);
    let e1 = _mm512_add_ps(tmp11, tmp12);
    let e2 = _mm512_sub_ps(tmp11, tmp12);
    let z13 = _mm512_add_ps(v[5], v[3]);
    let z10 = _mm512_sub_ps(v[5], v[3]);
    let z11 = _mm512_add_ps(v[1], v[7]);
    let z12 = _mm512_sub_ps(v[1], v[7]);
    let t7 = _mm512_add_ps(z11, z13);
    let t11 = _mm512_mul_ps(_mm512_sub_ps(z11, z13), r2);
    let z5 = _mm512_mul_ps(_mm512_add_ps(z10, z12), c1);
    let t10 = if FMA {
        _mm512_fnmadd_ps(z12, c2, z5)
    } else {
        _mm512_sub_ps(z5, _mm512_mul_ps(z12, c2))
    };
    let t12 = if FMA {
        _mm512_fnmadd_ps(z10, c3, z5)
    } else {
        _mm512_sub_ps(z5, _mm512_mul_ps(z10, c3))
    };
    let t6 = _mm512_sub_ps(t12, t7);
    let t5 = _mm512_sub_ps(t11, t6);
    let t4 = _mm512_sub_ps(t10, t5);
    v[0] = _mm512_add_ps(e0, t7);
    v[7] = _mm512_set1_ps(0.0);
    v[1] = _mm512_add_ps(e1, t6);
    v[6] = _mm512_sub_ps(e1, t6);
    v[2] = _mm512_add_ps(e2, t5);
    v[5] = _mm512_sub_ps(e2, t5);
    v[3] = _mm512_add_ps(e3, t4);
    v[4] = _mm512_sub_ps(e3, t4);
}

/// Like `simd::recon_block_row`: dequantizes and inverse-transforms blocks
/// `bx0..bx1` of block row `r` of a stripe into `planes` (3 channels x 8
/// rows, row stride `pw`, block `bx` at column `(bx - bx0) * 8`), zeroing
/// the coefficients it reads.
#[arcane(import_intrinsics)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn recon_block_row<const FMA: bool>(
    t: X64V4Token,
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
) {
    let plo = _mm512_loadu_si512(&PERM_LO);
    let (m1, p1) = (_mm512_set1_epi32(-1), _mm512_set1_epi32(1));
    let phi = _mm512_loadu_si512(&PERM_HI);
    let zero_blk = [0i32; 64];
    let (mut da, mut db) = ([0.0f32; 64], [0.0f32; 64]);
    for bx in (bx0..bx1).step_by(2) {
        let pair = bx + 1 < bx1;
        for ch in 0..3 {
            let ba = r * bw + bx;
            let ea = eobs[ba * 3 + ch];
            let eb = if pair { eobs[(ba + 1) * 3 + ch] } else { 0 };
            let base = ch * 8 * pw + (bx - bx0) * 8;
            let (d0, d1) = if aq.is_empty() {
                (&dq[ch], &dq[ch])
            } else {
                (
                    super::aq_dequant_in(&dq[ch], aq.get(bx).copied(), &mut da),
                    super::aq_dequant_in(&dq[ch], aq.get(bx + 1).copied(), &mut db),
                )
            };
            if ea == 0 && eb == 0 {
                // Both flat (or a flat single): fill.
                for (k, d) in [d0, d1].into_iter().enumerate().take(1 + pair as usize) {
                    let qi = ((ba + k) * 3 + ch) * 64;
                    let v = q[qi] as f32 * d[0];
                    q[qi] = 0;
                    for y in 0..7 {
                        planes[base + k * 8 + y * pw..][..8].fill(v);
                    }
                }
                continue;
            }
            let qa = ((ba * 3) + ch) * 64;
            let qb = (((ba + 1) * 3) + ch) * 64;
            let mut v: [__m512; 8] = core::array::from_fn(|y| {
                let ia: &[i32; 8] = q[qa + y * 8..qa + y * 8 + 8].try_into().unwrap();
                let ib: &[i32; 8] = if pair {
                    q[qb + y * 8..qb + y * 8 + 8].try_into().unwrap()
                } else {
                    zero_blk[..8].try_into().unwrap()
                };
                let i = _mm512_inserti64x4::<1>(
                    _mm512_castsi256_si512(_mm256_loadu_si256(ia)),
                    _mm256_loadu_si256(ib),
                );
                let fa: &[f32; 8] = d0[y * 8..y * 8 + 8].try_into().unwrap();
                let fb: &[f32; 8] = d1[y * 8..y * 8 + 8].try_into().unwrap();
                let d = _mm512_insertf32x8::<1>(
                    _mm512_castps256_ps512(_mm256_loadu_ps(fa)),
                    _mm256_loadu_ps(fb),
                );
                let x = _mm512_cvtepi32_ps(i);
                let x = match bias {
                    // Lossy 0.4 bias: q - sign(q) * B, B by |q| = 1 or >= 2.
                    Some(bt) => {
                        let row = |t: &[f32; 64]| -> __m512 {
                            let h: &[f32; 8] = t[y * 8..y * 8 + 8].try_into().unwrap();
                            let h = _mm256_loadu_ps(h);
                            _mm512_insertf32x8::<1>(_mm512_castps256_ps512(h), h)
                        };
                        let sg = _mm512_min_epi32(_mm512_max_epi32(i, m1), p1);
                        let one = _mm512_cmpeq_epi32_mask(i, sg);
                        let b = _mm512_mask_blend_ps(one, row(&bt[ch][1]), row(&bt[ch][0]));
                        _mm512_sub_ps(x, _mm512_mul_ps(_mm512_cvtepi32_ps(sg), b))
                    }
                    None => x,
                };
                _mm512_mul_ps(x, d)
            });
            q[qa..qa + 64].fill(0);
            if pair {
                q[qb..qb + 64].fill(0);
            }
            pass::<FMA>(t, &mut v);
            transpose2(t, &mut v, plo, phi);
            pass::<FMA>(t, &mut v);
            transpose2(t, &mut v, plo, phi);
            for (y, row) in v.iter().enumerate().take(7) {
                if pair {
                    let o: &mut [f32; 16] =
                        (&mut planes[base + y * pw..][..16]).try_into().unwrap();
                    _mm512_storeu_ps(o, *row);
                } else {
                    let o: &mut [f32; 8] = (&mut planes[base + y * pw..][..8]).try_into().unwrap();
                    _mm256_storeu_ps(o, _mm512_castps512_ps256(*row));
                }
            }
        }
    }
}
