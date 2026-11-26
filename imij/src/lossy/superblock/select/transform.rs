//! Superblock DCT bases, forward spectra, SIMD candidate quantizers, and
//! 8x8/4x4 reconstruction error metrics.

use super::Group2;
use crate::lossy::superblock::*;
use archmage::prelude::*;

/// cos(m pi / 32) for m = 0..=16 (f64).
const COS32: [f64; 17] = [
    1.0,
    0.995_184_726_672_196_9,
    0.980_785_280_403_230_4,
    0.956_940_335_732_208_8,
    0.923_879_532_511_286_7,
    0.881_921_264_348_355,
    0.831_469_612_302_545_2,
    0.773_010_453_362_737,
    core::f64::consts::FRAC_1_SQRT_2,
    0.634_393_284_163_645_5,
    0.555_570_233_019_602_3,
    0.471_396_736_825_997_8,
    0.382_683_432_365_089_84,
    0.290_284_677_254_462_33,
    0.195_090_322_016_128_33,
    0.098_017_140_329_560_77,
    0.0,
];

/// cos(m pi / 32) for any m.
fn cos32(m: usize) -> f64 {
    let m = m % 64;
    let m = if m > 32 { 64 - m } else { m };
    if m <= 16 { COS32[m] } else { -COS32[32 - m] }
}

/// The orthonormal DCT-II basis of length 16, C[k][n], rounded to f32.
fn dct16_matrix() -> [[f32; 16]; 16] {
    core::array::from_fn(|k| {
        let ck = if k == 0 {
            0.25
        } else {
            core::f64::consts::FRAC_1_SQRT_2 * 0.5
        };
        core::array::from_fn(|n| (ck * cos32(k * (2 * n + 1))) as f32)
    })
}

/// The forward DCT bases (encoder): the transposed bases for the row
/// passes, as f32x8 rows, and the bases for the column passes.
pub(super) struct Bases {
    /// `mt16[i][h][j] = C16[8 h + j][i]`.
    mt16: [[[f32; 8]; 2]; 16],
    /// `mt8[i][j] = C8[j][i]`.
    mt8: [[f32; 8]; 8],
    m16: [[f32; 16]; 16],
    pub(super) m8: [[f32; 8]; 8],
}

/// The candidate spectra of one plane of a 2x2 group, `v * 16 + u`: the
/// 16x16 block, the 16x8 blocks by parent row and the 8x16 blocks by parent
/// column.
pub(in crate::lossy) struct Spectra {
    s16: [f32; 256],
    s16x8: [[f32; 256]; 2],
    s8x16: [[f32; 256]; 2],
}

impl Spectra {
    pub(in crate::lossy) fn get(&self, s: Shape, py0: usize, px0: usize) -> &[f32; 256] {
        match s {
            Shape::S16x16 => &self.s16,
            Shape::S16x8 => &self.s16x8[py0],
            Shape::S8x16 => &self.s8x16[px0],
        }
    }
}

pub(in crate::lossy) fn new_spectra() -> [Spectra; NCH] {
    core::array::from_fn(|_| Spectra {
        s16: [0.0; 256],
        s16x8: [[0.0; 256]; 2],
        s8x16: [[0.0; 256]; 2],
    })
}

/// Every candidate spectrum of one plane of a group from its 16 x 16 pixels
/// `x` (rows of the group's two block rows, columns of its two block
/// columns; absent parents' pixels are zero and their candidates unused):
/// row passes (16-point over each row, 8-point over each half row) shared
/// by the shapes, then the column passes. Orthonormal, f32 (encoder only).
#[magetypes(define(f32x8), v3, neon, wasm128, scalar)]
fn spectra_impl(token: Token, b: &Bases, x: &[[f32; 16]; 16], out: &mut Spectra) {
    let z = f32x8::zero_t(token);
    let mut h16 = [[z; 2]; 16];
    let mut h8 = [[z; 2]; 16];
    let mt8: [f32x8; 8] = core::array::from_fn(|i| f32x8::load_t(token, &b.mt8[i]));
    for y in 0..16 {
        let (mut a0, mut a1) = (z, z);
        for i in 0..16 {
            let xv = f32x8::splat_t(token, x[y][i]);
            a0 += xv * f32x8::load_t(token, &b.mt16[i][0]);
            a1 += xv * f32x8::load_t(token, &b.mt16[i][1]);
        }
        h16[y] = [a0, a1];
        for (half, hv) in h8[y].iter_mut().enumerate() {
            let mut a = z;
            for (i, m) in mt8.iter().enumerate() {
                a += f32x8::splat_t(token, x[y][half * 8 + i]) * *m;
            }
            *hv = a;
        }
    }
    let o16: &mut [[f32; 8]] = out.s16.as_chunks_mut::<8>().0;
    for v in 0..16 {
        let (mut a0, mut a1) = (z, z);
        for (j, hj) in h16.iter().enumerate() {
            let m = f32x8::splat_t(token, b.m16[v][j]);
            a0 += m * hj[0];
            a1 += m * hj[1];
        }
        a0.store(&mut o16[v * 2]);
        a1.store(&mut o16[v * 2 + 1]);
    }
    for (px, sp) in out.s8x16.iter_mut().enumerate() {
        let o: &mut [[f32; 8]] = sp.as_chunks_mut::<8>().0;
        for v in 0..16 {
            let mut a = z;
            for (j, hj) in h8.iter().enumerate() {
                a += f32x8::splat_t(token, b.m16[v][j]) * hj[px];
            }
            a.store(&mut o[v * 2]);
        }
    }
    for (py, sp) in out.s16x8.iter_mut().enumerate() {
        let o: &mut [[f32; 8]] = sp.as_chunks_mut::<8>().0;
        for v in 0..8 {
            let (mut a0, mut a1) = (z, z);
            for j in 0..8 {
                let m = f32x8::splat_t(token, b.m8[v][j]);
                a0 += m * h16[py * 8 + j][0];
                a1 += m * h16[py * 8 + j][1];
            }
            a0.store(&mut o[v * 2]);
            a1.store(&mut o[v * 2 + 1]);
        }
    }
}

/// Quantizes one plane of a candidate spectrum `c` over `rows` rows of
/// `octets` column octets: `q = trunc(min(|c| inv / aq + round, 2^22 - 1))`
/// with c's sign into `q` (spectrum layout), and the error `(|c| - q step
/// aq) esc` with c's sign into `err`. `inv` and `esc` are zero in the LLF
/// corner, which so quantizes to zero with no error. Returns the squared
/// error in quantizer steps.
#[magetypes(define(f32x8, i32x8), v3, neon, wasm128, scalar)]
#[allow(clippy::too_many_arguments)]
pub(super) fn quant_impl(
    token: Token,
    (rows, octets): (usize, usize),
    c: &[f32; 256],
    (inv, st, esc): (&[f32; 256], &[f32; 256], &[f32; 256]),
    (aq, round): (f32, f32),
    q: &mut [i32; 256],
    err: &mut [f32; 256],
) -> f32 {
    let z = f32x8::zero_t(token);
    let iz = i32x8::zero_t(token);
    let (inv_aq, aqv) = (f32x8::splat_t(token, 1.0 / aq), f32x8::splat_t(token, aq));
    let freq_zb = round < 0.0;
    let (rv, cap) = (
        f32x8::splat_t(token, round.abs()),
        f32x8::splat_t(token, 4_194_303.0),
    );
    let (cc, ic, sc, ec) = (
        c.as_chunks::<8>().0,
        inv.as_chunks::<8>().0,
        st.as_chunks::<8>().0,
        esc.as_chunks::<8>().0,
    );
    let qo = q.as_chunks_mut::<8>().0;
    let eo = err.as_chunks_mut::<8>().0;
    let mut d = z;
    for v in 0..rows {
        for j in 0..octets {
            let k = v * 2 + j;
            let row_rv = if freq_zb {
                let arr: [f32; 8] = core::array::from_fn(|u| {
                    let col = j * 8 + u;
                    let f = ((v + col) as f32 / 30.0).clamp(0.0, 1.0);
                    0.45 - 0.14 * f
                });
                f32x8::from_array_t(token, arr)
            } else {
                rv
            };
            let cv = f32x8::load_t(token, &cc[k]);
            let mag = cv.abs();
            let a = mag * (f32x8::load_t(token, &ic[k]) * inv_aq);
            let qa = (a + row_rv).min(cap).to_i32();
            let qf = f32x8::from_i32x8_t(token, qa);
            let e = a - qf;
            d += e * e;
            let er =
                (mag - qf * (f32x8::load_t(token, &sc[k]) * aqv)) * f32x8::load_t(token, &ec[k]);
            let neg = cv.simd_lt(z);
            f32x8::blend(neg, z - er, er).store(&mut eo[k]);
            i32x8::blend(neg.bitcast_to_i32(), iz - qa, qa).store(&mut qo[k]);
        }
    }
    d.reduce_add()
}

/// The AC terms of a unit `q` of coefficients `c` in one pass: [`dtilde`]
/// (reciprocal steps `inv`, AQ `1 / aq`) and [`energy`] (steps `st`, AQ
/// `aq`), with their lane sums.
#[magetypes(define(f32x8, i32x8), v3, neon, wasm128, scalar)]
pub(super) fn terms_impl(
    token: Token,
    c: &[f32; 64],
    q: &[i32; 64],
    (inv, st): (&[f32; 64], &[f32; 64]),
    aq: f32,
) -> (f64, f64) {
    let iz = i32x8::zero_t(token);
    let (inv_aq, aqv) = (f32x8::splat_t(token, 1.0 / aq), f32x8::splat_t(token, aq));
    let (cc, qc, ic, sc) = (
        c.as_chunks::<8>().0,
        q.as_chunks::<8>().0,
        inv.as_chunks::<8>().0,
        st.as_chunks::<8>().0,
    );
    let mut ac = [1.0f32; 8];
    ac[0] = 0.0;
    let (mut d, mut e) = (f32x8::zero_t(token), f32x8::zero_t(token));
    for r in 0..8 {
        let mag = f32x8::load_t(token, &cc[r]).abs();
        let qv = i32x8::load_t(token, &qc[r]);
        let qf = f32x8::from_i32x8_t(token, i32x8::blend(qv.simd_lt(iz), iz - qv, qv));
        let ed = mag * (f32x8::load_t(token, &ic[r]) * inv_aq) - qf;
        let ee = mag - qf * (f32x8::load_t(token, &sc[r]) * aqv);
        let (mut dd, mut de) = (ed * ed, ee * ee);
        if r == 0 {
            // No DC term (the scalar sums add 0 for it).
            let m = f32x8::load_t(token, &ac);
            dd *= m;
            de *= m;
        }
        d += dd;
        e += de;
    }
    let sum = |v: f32x8| v.to_array().iter().map(|&a| a as f64).sum::<f64>();
    (sum(d), sum(e))
}

/// The 4x4 candidate of coefficients `cf` in one pass: its AC units as
/// [`layout::quantize`] computes them into `q4` (the DC slot is left for the
/// caller), and their terms as [`terms_impl`].
#[magetypes(define(f32x8, i32x8), v3, neon, wasm128, scalar)]
pub(super) fn cand4_impl(
    token: Token,
    cf: &[f32; 64],
    (inv, st): (&[f32; 64], &[f32; 64]),
    (aq, round): (f32, f32),
    q4: &mut [i32; 64],
) -> (f64, f64) {
    let z = f32x8::zero_t(token);
    let iz = i32x8::zero_t(token);
    let (inv_aq, aqv) = (f32x8::splat_t(token, 1.0 / aq), f32x8::splat_t(token, aq));
    let freq_zb = round < 0.0;
    let (rv, cap) = (
        f32x8::splat_t(token, round.abs()),
        f32x8::splat_t(token, 4_194_303.0),
    );
    let (cc, ic, sc) = (
        cf.as_chunks::<8>().0,
        inv.as_chunks::<8>().0,
        st.as_chunks::<8>().0,
    );
    let qo = q4.as_chunks_mut::<8>().0;
    let mut ac = [1.0f32; 8];
    ac[0] = 0.0;
    let (mut d, mut e) = (z, z);
    for r in 0..8 {
        let row_rv = if freq_zb {
            let arr: [f32; 8] = core::array::from_fn(|u| {
                let idx = r * 8 + u;
                if idx == 1 || idx == 8 || idx == 9 {
                    0.48 // Haar 2x2 sub-DC detail slots: preserve sharp 4x4 step edges
                } else {
                    let f = ((r + u) as f32 / 14.0).clamp(0.0, 1.0);
                    0.45 - 0.13 * f
                }
            });
            f32x8::from_array_t(token, arr)
        } else {
            rv
        };
        let cv = f32x8::load_t(token, &cc[r]);
        let mag = cv.abs();
        let s = f32x8::load_t(token, &sc[r]) * aqv;
        let qa = (mag / s + row_rv).min(cap).to_i32();
        i32x8::blend(cv.simd_lt(z).bitcast_to_i32(), iz - qa, qa).store(&mut qo[r]);
        let qf = f32x8::from_i32x8_t(token, qa);
        let ed = mag * (f32x8::load_t(token, &ic[r]) * inv_aq) - qf;
        let ee = mag - qf * s;
        let (mut dd, mut de) = (ed * ed, ee * ee);
        if r == 0 {
            let m = f32x8::load_t(token, &ac);
            dd *= m;
            de *= m;
        }
        d += dd;
        e += de;
    }
    let sum = |v: f32x8| v.to_array().iter().map(|&a| a as f64).sum::<f64>();
    (sum(d), sum(e))
}

/// Encoder tables of one image.
pub(in crate::lossy) struct Enc {
    pub(super) b: Bases,
    /// Per shape: for unit slot `p * 64 + z`, the spectrum index whose
    /// quantized value it carries (0, which quantizes to zero, for the DC
    /// slots).
    pub(in crate::lossy) gather: [[u8; 256]; 3],
    /// Per plane, per shape: the step of coefficient `v * 16 + u` and its
    /// reciprocal (0 in the LLF corner).
    pub(in crate::lossy) steps: [[[f32; 256]; 3]; NCH],
    pub(super) inv: [[[f32; 256]; 3]; NCH],
    /// Per shape: the kernel's input scale of every coefficient (0 in the
    /// LLF corner).
    pub(super) sc: [[f32; 256]; 3],
    /// Per plane: the 8x8 steps and their reciprocals.
    pub(super) steps8: [[f32; 64]; NCH],
    pub(super) inv8: [[f32; 64]; NCH],
    /// Per plane: the 4x4 parents' steps ([`small::steps4`]) and their
    /// reciprocals.
    pub(super) steps4: [[f32; 64]; NCH],
    pub(super) inv4: [[f32; 64]; NCH],
    /// Per plane: the decoder's 8x8 and 4x4 dequantization factors (the
    /// inner-edge term reconstructs both candidates).
    pub(super) dq8: [[f32; 64]; NCH],
    pub(super) dq4: [[f32; 64]; NCH],
    /// Per plane: the smallest 4x4 AC step.
    pub(super) min4: [f32; NCH],
}

impl Enc {
    pub(in crate::lossy) fn new(steps8: &[[f32; 64]; NCH]) -> Self {
        let m8 = dct::dct_matrix();
        let m16 = dct16_matrix();
        let steps: [[[f32; 256]; 3]; NCH] = core::array::from_fn(|c| {
            Shape::ALL.map(|s| {
                let mut t = [1.0f32; 256];
                for v in 0..s.h() {
                    for u in 0..s.w() {
                        t[v * 16 + u] = step(&steps8[c], s, v, u);
                    }
                }
                t
            })
        });
        Self {
            b: Bases {
                mt16: core::array::from_fn(|i| {
                    core::array::from_fn(|h| core::array::from_fn(|j| m16[8 * h + j][i]))
                }),
                mt8: core::array::from_fn(|i| core::array::from_fn(|j| m8[j][i])),
                m16,
                m8,
            },
            gather: Shape::ALL.map(|s| {
                let mut t = [0u8; 256];
                for vv in 0..s.h() {
                    for u in 0..s.w() {
                        if vv < s.ph() && u < s.pw() {
                            continue;
                        }
                        let (p, z) = slot(s, vv, u);
                        t[p * 64 + z] = (vv * 16 + u) as u8;
                    }
                }
                t
            }),
            inv: core::array::from_fn(|c| {
                Shape::ALL.map(|s| {
                    let mut t = steps[c][s as usize].map(|v| 1.0 / v);
                    for v in 0..s.ph() {
                        for u in 0..s.pw() {
                            t[v * 16 + u] = 0.0;
                        }
                    }
                    t
                })
            }),
            steps,
            sc: Shape::ALL.map(|s| {
                let mut t = [0.0f32; 256];
                for v in 0..s.h() {
                    for u in 0..s.w() {
                        if v >= s.ph() || u >= s.pw() {
                            t[v * 16 + u] = input_scale(s, v, u);
                        }
                    }
                }
                t
            }),
            steps8: *steps8,
            inv8: steps8.map(|t| t.map(|v| 1.0 / v)),
            steps4: steps8.map(|t| small::steps4(&t)),
            inv4: steps8.map(|t| small::steps4(&t).map(|v| 1.0 / v)),
            dq8: steps8.map(|t| dct::dequant_factors(&t)),
            dq4: steps8.map(|t| small::dequant4(&t)),
            min4: steps8.map(|t| {
                small::steps4(&t)[1..]
                    .iter()
                    .fold(f32::INFINITY, |m, &v| m.min(v))
            }),
        }
    }

    /// The candidate spectra of every plane of group `g` (pixels from the
    /// encoder's planes, `pw` wide).
    pub(super) fn spectra(
        &self,
        planes: &[Vec<f32>; 2],
        pw: usize,
        g: &Group2,
        out: &mut [Spectra; NCH],
    ) {
        self.spectra_at(planes, pw, g.rows, g.cols, g.bx, out);
    }

    /// The candidate spectra of every plane of a `(rows x cols)` parent group
    /// at global block column `bx` (pixels from `planes`, `pw` wide).
    pub(in crate::lossy) fn spectra_at(
        &self,
        planes: &[Vec<f32>; 2],
        pw: usize,
        rows: usize,
        cols: usize,
        bx: usize,
        out: &mut [Spectra; NCH],
    ) {
        for (ch, sp) in out.iter_mut().enumerate() {
            let mut x = [[0.0f32; 16]; 16];
            let n = 8 * cols;
            for (py, pl) in planes.iter().enumerate().take(rows) {
                for y in 0..8 {
                    let row = &pl[ch * 8 * pw + y * pw + bx * 8..][..n];
                    x[py * 8 + y][..n].copy_from_slice(row);
                }
            }
            incant!(spectra_impl(&self.b, &x, sp), [v3, neon, wasm128, scalar]);
        }
    }
}

/// A merged candidate: its units, parent DCs and cost.
#[derive(Clone)]
pub(super) struct MCand {
    pub(super) shape: Shape,
    /// J (distortion plus rate in bits).
    pub(super) j: f64,
    pub(super) units: [[[i32; 64]; 4]; NCH],
    pub(super) dcs: [[[i32; 2]; 2]; NCH],
    pub(super) aq: u8,
}

/// One worker's selector buffers, reused from group to group: every plane's
/// candidate spectra, the candidates (at most five per group) and the
/// error spectra of the candidate being priced.
pub(super) struct Work {
    pub(super) sp: [Spectra; NCH],
    pub(super) cands: [MCand; 5],
    pub(super) err: [[f32; 256]; NCH],
}

impl Work {
    pub(super) fn new() -> alloc::boxed::Box<Self> {
        let cand = MCand {
            shape: Shape::S16x16,
            j: 0.0,
            units: [[[0; 64]; 4]; NCH],
            dcs: [[[0; 2]; 2]; NCH],
            aq: 64,
        };
        alloc::boxed::Box::new(Self {
            sp: core::array::from_fn(|_| Spectra {
                s16: [0.0; 256],
                s16x8: [[0.0; 256]; 2],
                s8x16: [[0.0; 256]; 2],
            }),
            cands: core::array::from_fn(|_| cand.clone()),
            err: [[0.0; 256]; NCH],
        })
    }
}

/// The decoder's reconstruction of one plane from its units (`four`: 4x4
/// planes, else 8x8), AQ multiplier `aq` on the AC.
pub(super) fn recon(q: &[i32; 64], dq: &[f32; 64], aq: f32, four: bool) -> [f32; 64] {
    let mut x: [f32; 64] =
        core::array::from_fn(|i| q[i] as f32 * if i == 0 { dq[0] } else { dq[i] * aq });
    if four {
        small::inverse_ref(&mut x);
    } else {
        dct::idct8x8(&mut x);
    }
    x
}

/// The inner-edge measure of a reconstruction `r` of source `s` (raster
/// 8x8): the squared change of the error across the middle column pair
/// (3 | 4) of every row and the middle row pair of every column, where a
/// 4x4 plane's tiles meet.
pub(super) fn inner_edge(s: &[f32; 64], r: &[f32; 64]) -> f64 {
    let e = |i: usize| (r[i] - s[i]) as f64;
    let mut acc = 0.0;
    for k in 0..8 {
        let h = e(k * 8 + 4) - e(k * 8 + 3);
        let v = e(4 * 8 + k) - e(3 * 8 + k);
        acc += h * h + v * v;
    }
    acc
}
