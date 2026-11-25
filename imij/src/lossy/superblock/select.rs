//! Encoder: the partition of every aligned 2x2 parent group, priced with
//! the file's own coder (benchmarks/superblocks_2026-10-09, `select = 1`).
//!
//! Per group every partition symbol is priced as J = D + R, after the plain
//! 8x8 quantization (pass 1) has fixed the entropy tables the prices come
//! from:
//!
//! * **Rate R** (bits): every unit of the candidate under those tables:
//!   for run-level files the EOB token in its lane context, run-level
//!   tokens in their (plane, position, positions-left) contexts and the raw
//!   bits; for split-Huffman files the group count's code length, one width
//!   code per coded group, magnitudes at the group's width and one sign bit
//!   per nonzero. Plus the partition symbol (3 bits for a merge, 0.25 for
//!   none) and 0.5 bit per merged parent for the nested DC image's growth.
//! * **Distortion D:** squared error of every coded coefficient in units of
//!   its own step (all steps are physical-frequency steps, so the unit is
//!   the same in every transform size), weighted per plane with zenjpeg's
//!   trellis lambda for the group's mean AC energy, so a bit is worth what
//!   the trellis would pay for it; plus a ringing term: the pixel-domain
//!   error energy of each parent, weighted by how flat the parent's source
//!   is, in squared first AC steps.
//! * Kept, exact, all-white and all-black parents are never merged; AQ codes
//!   of a merge unify to the finest of its parents.
//!
//! * **4x4 planes** (small transforms, SPEC 2.11): a parent outside a merge
//!   codes each plane with the cheaper of its 8x8 unit and its four 4x4
//!   DCTs (the lab's `split4`), at the same weights and prices, plus the
//!   split flags' estimated bits.
//!
//! Lab variants that did not qualify (trellis-aware pricing, more passes)
//! are not ported; the study's README records them.

#[cfg(not(feature = "std"))]
use num_traits::Float as _;

use super::*;
use crate::lossy::trellis::{HufPrices, LossyTrellis, Prices, lane_neighbors, support};
use archmage::prelude::*;

/// Flatness reference of the ringing weight, in squared first AC steps of
/// mean AC energy: a parent whose mean squared AC coefficient equals it has
/// weight 1/2.
const FLAT_A0: f32 = 1.0;
/// Weight of the ringing term.
const RING: f64 = 1.0;
/// Nested DC image growth per merged parent (bits, all planes): the
/// 2026-10-08 study measured 88-137 kbit over 28 images with about 230k
/// parents merged.
const DC_KAPPA: f64 = 0.5;
/// Partition symbol costs before any map statistics exist (bits).
const SYM_NONE_BITS: f64 = 0.25;
const SYM_MERGE_BITS: f64 = 3.0;

/// Token prices of the file's entropy coder under frozen tables.
pub(in crate::lossy) enum Coder {
    RunLevel(alloc::boxed::Box<Prices>),
    SplitHuf(alloc::boxed::Box<HufPrices>),
}

impl Coder {
    /// Prices from run-level token counts `hist` (`nraw * ALPHA`,
    /// context-major) or split-Huffman frequencies `freqs` of the units: what
    /// [`Self::of`] derives from the units themselves.
    pub(in crate::lossy) fn of_counts(
        hist: &[u64],
        freqs: &crate::lossy::trellis::Freqs,
        cfg: &LossyConfig,
        effort: crate::effort::Effort,
        split: bool,
    ) -> Self {
        if split {
            return Coder::SplitHuf(alloc::boxed::Box::new(HufPrices::from_freqs(
                freqs,
                effort,
                table_k_mode(cfg),
            )));
        }
        #[cfg(feature = "lab")]
        let cfg = &{
            let mut c = cfg.clone();
            c.seed_bank = crate::seeds::DEFAULT_BANK;
            c
        };
        let hist: Vec<Vec<u64>> = hist
            .as_chunks::<{ coef::ALPHA }>()
            .0
            .iter()
            .map(|row| row.to_vec())
            .collect();
        let (tables, explicit) = coef::Tables::build_seeded(
            &hist,
            4,
            cfg.max_ac_tables,
            LossyEntropy::RunLevel,
            effort,
            seed_bank(cfg),
            table_k_mode(cfg),
        );
        Coder::RunLevel(alloc::boxed::Box::new(Prices::new(
            &explicit.unwrap_or(tables),
        )))
    }

    /// Prices tallied directly from full-width `stripes` without cloning the
    /// whole image's coefficient buffer (`group_bw == 0` reads in place;
    /// `group_bw > 0` splits at most one stripe per active worker).
    pub(in crate::lossy) fn of_stripes(
        stripes: &[StripeCoefs],
        group_bw: usize,
        cfg: &LossyConfig,
        effort: crate::effort::Effort,
        split: bool,
    ) -> Self {
        if split {
            let parts = crate::par::par_fold(
                stripes,
                cfg.threads,
                crate::lossy::trellis::freqs_new,
                |freqs, sc| {
                    let mut tally = |u: &StripeCoefs| {
                        let (counts, widths) = stripe_counts_widths(u, u.bw);
                        let syms = huf::stripe_syms(&counts, &widths, u.nby * u.bw);
                        let [luma, chroma, wid] = freqs;
                        for (h, symbols) in
                            [(luma, &syms.luma), (chroma, &syms.chroma), (wid, &syms.wid)]
                        {
                            for &(ctx, sym) in symbols {
                                h[ctx as usize][sym as usize] += 1;
                            }
                        }
                    };
                    if group_bw == 0 {
                        tally(sc);
                    } else {
                        for u in split_units(sc.clone(), group_bw) {
                            tally(&u);
                        }
                    }
                },
            );
            let mut total = crate::lossy::trellis::freqs_new();
            for p in parts {
                for (dst, src) in total.iter_mut().zip(p) {
                    for (drow, srow) in dst.iter_mut().zip(src) {
                        for (d, s) in drow.iter_mut().zip(srow) {
                            *d += s;
                        }
                    }
                }
            }
            Self::of_counts(&[], &total, cfg, effort, true)
        } else {
            let n = LossyEntropy::RunLevel.nraw() * coef::ALPHA;
            let parts = crate::par::par_fold(
                stripes,
                cfg.threads,
                || vec![0u64; n],
                |hist, sc| {
                    let mut tally = |u: &StripeCoefs| {
                        stripe_tokens(u, u.bw, Group::ALL, |_, ctx, tok, _, _| {
                            hist[ctx * coef::ALPHA + tok as usize] += 1;
                        });
                    };
                    if group_bw == 0 {
                        tally(sc);
                    } else {
                        for u in split_units(sc.clone(), group_bw) {
                            tally(&u);
                        }
                    }
                },
            );
            let mut total = vec![0u64; n];
            for p in parts {
                for (d, s) in total.iter_mut().zip(p) {
                    *d += s;
                }
            }
            Self::of_counts(
                &total,
                &crate::lossy::trellis::freqs_new(),
                cfg,
                effort,
                false,
            )
        }
    }

    /// Prices from `units`' own statistics: split-Huffman when the file is
    /// coded with it, else run-level.
    #[allow(dead_code)]
    pub(in crate::lossy) fn of(
        units: &[StripeCoefs],
        cfg: &LossyConfig,
        effort: crate::effort::Effort,
        split: bool,
    ) -> Self {
        if split {
            Coder::SplitHuf(alloc::boxed::Box::new(HufPrices::new(
                units,
                cfg.threads,
                effort,
                table_k_mode(cfg),
            )))
        } else {
            let (tables, explicit) = ans_tables_for_decisions(units, cfg, effort);
            Coder::RunLevel(alloc::boxed::Box::new(Prices::new(
                &explicit.unwrap_or(tables),
            )))
        }
    }
}

/// Position of the last nonzero coefficient in zigzag order (0 = none), from
/// the unit's [`nonzero`] mask.
fn eob_of_mask(m: u64) -> u32 {
    if m == 0 { 0 } else { 63 - m.leading_zeros() }
}

/// The zigzag positions 1..=63 of `q` that are nonzero, bit k for position
/// k: each raster row's nonzero byte through [`ZZ_BITS`].
#[inline]
fn nonzero(q: &[i32; 64]) -> u64 {
    let mut m = 0u64;
    for (r, row) in q.as_chunks::<8>().0.iter().enumerate() {
        let mut byte = 0usize;
        for (j, &v) in row.iter().enumerate() {
            byte |= ((v != 0) as usize) << j;
        }
        m |= ZZ_BITS[r][byte];
    }
    m & !1
}

/// For raster row `r` and a byte `b` of its nonzero columns (bit j: column
/// j), the zigzag bits of those positions.
static ZZ_BITS: [[u64; 256]; 8] = {
    let mut zz = [0u8; 64];
    let mut k = 0;
    while k < 64 {
        zz[ZIGZAG[k] as usize] = k as u8;
        k += 1;
    }
    let mut t = [[0u64; 256]; 8];
    let mut r = 0;
    while r < 8 {
        let mut b = 0;
        while b < 256 {
            let mut j = 0;
            while j < 8 {
                if b >> j & 1 != 0 {
                    t[r][b] |= 1 << zz[r * 8 + j];
                }
                j += 1;
            }
            b += 1;
        }
        r += 1;
    }
    t
};

/// Context references of a unit: the run-level EOB reference (the lane's
/// previous luma EOB for luma, the block's luma EOB for chroma) and the
/// split-Huffman count reference (the previous block's luma count for luma,
/// the block's luma count for chroma).
#[derive(Clone, Copy)]
struct Refs {
    eob: u32,
    count: u32,
}

impl Refs {
    /// Chroma references of a block whose luma unit is `luma`.
    fn of_luma(luma: &[i32; 64]) -> Self {
        Self::of_mask(nonzero(luma))
    }

    /// [`Self::of_luma`] from the luma unit's [`nonzero`] mask.
    fn of_mask(m: u64) -> Self {
        let eob = eob_of_mask(m);
        Self {
            eob,
            count: eob.div_ceil(4),
        }
    }
}

/// Bits of one unit of plane `ch` under the frozen coder (its DC slot is
/// not coded here).
fn unit_bits(coder: &Coder, ch: usize, q: &[i32; 64], r: Refs) -> f64 {
    unit_bits_m(coder, ch, q, nonzero(q), r)
}

/// [`unit_bits`] given the unit's [`nonzero`] mask `m`.
fn unit_bits_m(coder: &Coder, ch: usize, q: &[i32; 64], m: u64, r: Refs) -> f64 {
    match coder {
        Coder::RunLevel(p) => unit_bits_rl(p, ch, q, m, r.eob),
        Coder::SplitHuf(h) => unit_bits_sh(h, ch, q, m, r.count),
    }
}

/// Run-level: the EOB token in its lane context, run-level tokens in their
/// (plane, position, positions-left) contexts, raw sign and remainder bits.
fn unit_bits_rl(prices: &Prices, ch: usize, q: &[i32; 64], mut m: u64, ref_eob: u32) -> f64 {
    let eob = if m == 0 { 0 } else { 63 - m.leading_zeros() };
    let mut bits = prices.token(coef::eob_ctx(ch, ref_eob), eob);
    let mut start = 1u32;
    while m != 0 {
        let k = m.trailing_zeros();
        m &= m - 1;
        let v = q[ZIGZAG[k as usize] as usize];
        let mut run = k - start;
        while run > coef::MAX_RUN {
            bits += prices.token(coef::ac_ctx(ch, start, eob), coef::ZRL);
            start += 4;
            run -= 4;
        }
        let (t, _, n) = rl_token(run, v);
        bits += prices.token(coef::ac_ctx(ch, start, eob), t) + n as f64;
        start = k + 1;
    }
    bits
}

/// Split-Huffman: the group count (luma: context of the previous block's
/// luma count; chroma: plane and the block's luma count), one width per
/// coded group, then each group's magnitudes at its width and one sign bit
/// per nonzero coefficient.
fn unit_bits_sh(h: &HufPrices, ch: usize, q: &[i32; 64], m: u64, ref_count: u32) -> f64 {
    let n = eob_of_mask(m).div_ceil(4) as usize;
    let mut bits = if ch == 0 {
        h.luma[huf::class(ref_count)][n]
    } else {
        h.chroma[(ch - 1) * huf::CLASSES + huf::class(ref_count)][n]
    };
    let l = if ch == 0 { n as u32 } else { ref_count };
    for gi in 0..n {
        let len = coef::pk_len(gi);
        let (mut m, mut nz) = (0u32, 0u32);
        for j in 0..len {
            let v = q[ZIGZAG[1 + 4 * gi + j] as usize].unsigned_abs();
            m = m.max(v);
            nz += (v != 0) as u32;
        }
        let w = 32 - m.leading_zeros();
        bits += h.wid[huf::wid_ctx(ch, gi, l)][w as usize] + (len as u32 * w + nz) as f64;
    }
    bits
}

/// Squared error in quantizer steps of the AC coefficients of a unit
/// (`inv`: reciprocal steps; `inv_aq`: reciprocal AQ multiplier); the
/// reference of [`terms_impl`].
#[cfg(all(test, feature = "encode"))]
fn dtilde(c: &[f32], q: &[i32; 64], inv: &[f32; 64], inv_aq: f32) -> f64 {
    lanes8(|i| {
        let e = c[i].abs() * (inv[i] * inv_aq) - q[i].unsigned_abs() as f32;
        e * e
    })
}

/// Absolute squared error of the AC coefficients of a unit; the reference
/// of [`terms_impl`].
#[cfg(all(test, feature = "encode"))]
fn energy(c: &[f32], q: &[i32; 64], steps: &[f32; 64], aq: f32) -> f64 {
    lanes8(|i| {
        let e = c[i].abs() - q[i].unsigned_abs() as f32 * (steps[i] * aq);
        e * e
    })
}

/// The sum of `f(i)` over the AC positions 1..64, in eight lane sums.
#[cfg(all(test, feature = "encode"))]
fn lanes8(f: impl Fn(usize) -> f32) -> f64 {
    let mut acc = [0.0f32; 8];
    for r in 0..8 {
        for (x, a) in acc.iter_mut().enumerate() {
            let i = r * 8 + x;
            *a += if i == 0 { 0.0 } else { f(i) };
        }
    }
    acc.iter().map(|&a| a as f64).sum()
}

/// Ringing term of one unit in squared first AC steps: the unit's error
/// energy times its flatness weight.
fn ring_unit(first_step: f32, wt: f32, energy: f64) -> f64 {
    let d = first_step as f64;
    wt as f64 * energy / (d * d)
}

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
struct Bases {
    /// `mt16[i][h][j] = C16[8 h + j][i]`.
    mt16: [[[f32; 8]; 2]; 16],
    /// `mt8[i][j] = C8[j][i]`.
    mt8: [[f32; 8]; 8],
    m16: [[f32; 16]; 16],
    m8: [[f32; 8]; 8],
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
fn quant_impl(
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
fn terms_impl(
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
fn cand4_impl(
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
    b: Bases,
    /// Per shape: for unit slot `p * 64 + z`, the spectrum index whose
    /// quantized value it carries (0, which quantizes to zero, for the DC
    /// slots).
    pub(in crate::lossy) gather: [[u8; 256]; 3],
    /// Per plane, per shape: the step of coefficient `v * 16 + u` and its
    /// reciprocal (0 in the LLF corner).
    pub(in crate::lossy) steps: [[[f32; 256]; 3]; NCH],
    inv: [[[f32; 256]; 3]; NCH],
    /// Per shape: the kernel's input scale of every coefficient (0 in the
    /// LLF corner).
    sc: [[f32; 256]; 3],
    /// Per plane: the 8x8 steps and their reciprocals.
    steps8: [[f32; 64]; NCH],
    inv8: [[f32; 64]; NCH],
    /// Per plane: the 4x4 parents' steps ([`small::steps4`]) and their
    /// reciprocals.
    steps4: [[f32; 64]; NCH],
    inv4: [[f32; 64]; NCH],
    /// Per plane: the decoder's 8x8 and 4x4 dequantization factors (the
    /// inner-edge term reconstructs both candidates).
    dq8: [[f32; 64]; NCH],
    dq4: [[f32; 64]; NCH],
    /// Per plane: the smallest 4x4 AC step.
    min4: [f32; NCH],
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
    fn spectra(&self, planes: &[Vec<f32>; 2], pw: usize, g: &Group2, out: &mut [Spectra; NCH]) {
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
struct MCand {
    shape: Shape,
    /// J (distortion plus rate in bits).
    j: f64,
    units: [[[i32; 64]; 4]; NCH],
    dcs: [[[i32; 2]; 2]; NCH],
    aq: u8,
}

/// One worker's selector buffers, reused from group to group: every plane's
/// candidate spectra, the candidates (at most five per group) and the
/// error spectra of the candidate being priced.
struct Work {
    sp: [Spectra; NCH],
    cands: [MCand; 5],
    err: [[f32; 256]; NCH],
}

impl Work {
    fn new() -> alloc::boxed::Box<Self> {
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

/// What the joint selector may choose.
#[derive(Clone, Copy)]
pub(in crate::lossy) struct Opts {
    /// Superblock merges, with the shapes of bit `1 << s` (lab).
    pub merges: bool,
    /// The AC trellis runs on this encode (it changes pixels with effort).
    pub trellis: bool,
    pub shapes: u8,
    /// 4x4 planes (small transforms): `Some(None)` priced per parent and
    /// plane, `Some(Some(m))` (lab) the planes of `m` in every eligible
    /// parent, `None` never.
    pub small: Option<Option<u8>>,
    /// Price split-Huffman coding instead of run-level.
    pub split_huf: bool,
    /// The 4x4 forward transform ([`small::forward_cfg`]).
    pub forward: fn(&[f32; 64]) -> [f32; 64],
    /// The split choice's distortion weight, relative to the merge
    /// decision's (lab `small_lambda`; 1 = the same).
    pub kappa: f32,
    /// Weight of the inner-edge term of the split choice (lab
    /// `small_edge`; 0 = none): the reconstruction error's jump across the
    /// block's middle row and column, in squared first AC steps.
    pub edge: f32,
    /// Centralized lossy effort plan governing adaptive RDO mechanisms.
    pub rdo: crate::effort::LossyEffortPlan,
}

/// Chooses the partitions of every stripe (`stripes`: pass 1's coefficients,
/// codes 0 for parents that may not merge or split) and rewrites the merged
/// parents' units, DCs, nonzero masks and AQ codes and the 4x4 planes'
/// units; `modes` gets each parent's merge role, `splits` (with
/// `opts.small`) its 4x4 plane bits. `rows` is the encoder's row pipeline
/// (planes `pw` wide); `group_bw` the column group width in blocks (0 =
/// none).
#[allow(clippy::too_many_arguments, dead_code)]
pub(in crate::lossy) fn apply(
    stripes: Vec<StripeCoefs>,
    rows: RowPlanes,
    pw: usize,
    group_bw: usize,
    steps: &[[f32; 64]; NCH],
    round: f32,
    cfg: &LossyConfig,
    effort: crate::effort::Effort,
    opts: Opts,
    stop: &dyn how_far::Stop,
) -> Result<Vec<StripeCoefs>, crate::Error> {
    apply_with_lambda(
        stripes,
        rows,
        pw,
        group_bw,
        steps,
        round,
        cfg,
        effort,
        &[],
        opts,
        stop,
    )
}

/// [`apply`] with an open-loop per-block trellis-lambda multiplier map.
#[allow(clippy::too_many_arguments)]
pub(in crate::lossy) fn apply_with_lambda(
    stripes: Vec<StripeCoefs>,
    rows: RowPlanes,
    pw: usize,
    group_bw: usize,
    steps: &[[f32; 64]; NCH],
    round: f32,
    cfg: &LossyConfig,
    effort: crate::effort::Effort,
    lambda_map: &[f32],
    opts: Opts,
    stop: &dyn how_far::Stop,
) -> Result<Vec<StripeCoefs>, crate::Error> {
    stop.check().map_err(|_| crate::Error::Cancelled)?;
    let coder = Coder::of_stripes(
        &stripes,
        group_bw,
        cfg,
        price_effort(effort, opts),
        opts.split_huf,
    );
    apply_with_learned(
        stripes, rows, pw, group_bw, steps, round, cfg, coder, None, None, lambda_map, opts, stop,
    )
}

/// [`apply`] using caller-supplied frozen token prices and optional empirical
/// partition-map / 4x4-flag tables from a pilot pass.
#[allow(clippy::too_many_arguments)]
pub(in crate::lossy) fn apply_with_learned(
    stripes: Vec<StripeCoefs>,
    rows: RowPlanes,
    pw: usize,
    group_bw: usize,
    steps: &[[f32; 64]; NCH],
    round: f32,
    cfg: &LossyConfig,
    coder: Coder,
    learned_map: Option<crate::lossy::adaptive_rdo::MapBitTable>,
    learned_flags: Option<crate::lossy::adaptive_rdo::LearnedFlags>,
    lambda_map: &[f32],
    opts: Opts,
    stop: &dyn how_far::Stop,
) -> Result<Vec<StripeCoefs>, crate::Error> {
    stop.check().map_err(|_| crate::Error::Cancelled)?;
    let sel = Selector::new_learned(
        coder,
        pw,
        group_bw,
        steps,
        round,
        opts,
        learned_map,
        learned_flags,
    );
    crate::par::par_map(stripes, cfg.threads, |_, sc: StripeCoefs| {
        sel.stripe_with_lambda(sc, rows, lambda_map, 0, stop)
    })
    .into_iter()
    .collect()
}

/// Without merges and without the trellis, the 4x4 choice prices with the
/// tables effort 7.5 builds whatever the encode's effort, so its decisions,
/// and the pixels, are the same at every effort (`LossyConfig::effort`).
/// Merges (Compact) and trellis encodes price with the encode's own effort.
pub(in crate::lossy) fn price_effort(
    effort: crate::effort::Effort,
    opts: Opts,
) -> crate::effort::Effort {
    if opts.merges || opts.trellis {
        effort
    } else {
        crate::effort::Effort::new(crate::effort::TABLE_COUNT_SEARCH).expect("valid effort")
    }
}

/// [`apply`]'s choice one stripe at a time, under prices settled first (the
/// bounded encoder measures them in a pass of its own).
pub(in crate::lossy) struct Selector {
    coder: Coder,
    enc: Enc,
    lam: LossyTrellis,
    pw: usize,
    group_bw: usize,
    round: f32,
    opts: Opts,
    learned_map: Option<crate::lossy::adaptive_rdo::MapBitTable>,
    learned_flags: Option<crate::lossy::adaptive_rdo::LearnedFlags>,
}

impl Selector {
    #[allow(clippy::too_many_arguments)]
    pub(in crate::lossy) fn new_learned(
        coder: Coder,
        pw: usize,
        group_bw: usize,
        steps: &[[f32; 64]; NCH],
        round: f32,
        opts: Opts,
        learned_map: Option<crate::lossy::adaptive_rdo::MapBitTable>,
        learned_flags: Option<crate::lossy::adaptive_rdo::LearnedFlags>,
    ) -> Self {
        Self {
            coder,
            enc: Enc::new(steps),
            lam: LossyTrellis::default(),
            pw,
            group_bw,
            round,
            opts,
            learned_map,
            learned_flags,
        }
    }

    /// The selector under prices from the units' run-level token counts
    /// `hist` (`nraw * ALPHA`, context-major) or split-Huffman frequencies
    /// `freqs` (pass-1 coefficients, codes 0 for parents that may not merge
    /// or split), as [`apply`] prices them from the units.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::lossy) fn from_counts(
        hist: &[u64],
        freqs: &crate::lossy::trellis::Freqs,
        pw: usize,
        group_bw: usize,
        steps: &[[f32; 64]; NCH],
        round: f32,
        cfg: &LossyConfig,
        effort: crate::effort::Effort,
        opts: Opts,
    ) -> Self {
        Self::from_counts_learned(
            hist, freqs, pw, group_bw, steps, round, cfg, effort, opts, None, None,
        )
    }

    /// [`Self::from_counts`] with empirical partition-map and 4x4-flag tables
    /// learned from a streaming or parallel pilot pass.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::lossy) fn from_counts_learned(
        hist: &[u64],
        freqs: &crate::lossy::trellis::Freqs,
        pw: usize,
        group_bw: usize,
        steps: &[[f32; 64]; NCH],
        round: f32,
        cfg: &LossyConfig,
        effort: crate::effort::Effort,
        opts: Opts,
        learned_map: Option<crate::lossy::adaptive_rdo::MapBitTable>,
        learned_flags: Option<crate::lossy::adaptive_rdo::LearnedFlags>,
    ) -> Self {
        let coder = Coder::of_counts(hist, freqs, cfg, price_effort(effort, opts), opts.split_huf);
        Self::new_learned(
            coder,
            pw,
            group_bw,
            steps,
            round,
            opts,
            learned_map,
            learned_flags,
        )
    }

    /// One stripe's choice with cooperative cancellation polling and an
    /// optional per-block trellis-lambda multiplier map starting at block row
    /// `lambda_by0` (`0` for whole-image maps, band `by0` for bounded passes).
    pub(in crate::lossy) fn stripe_with_lambda(
        &self,
        mut sc: StripeCoefs,
        rows: RowPlanes,
        lambda_map: &[f32],
        lambda_by0: usize,
        stop: &dyn how_far::Stop,
    ) -> Result<StripeCoefs, crate::Error> {
        stop.check().map_err(|_| crate::Error::Cancelled)?;
        let rdo = self.opts.rdo;
        if rdo.trellis_select
            || rdo.map_dp
            || self.learned_map.is_some()
            || self.learned_flags.is_some()
            || !lambda_map.is_empty()
        {
            return self.stripe_rdo(sc, rows, lambda_map, lambda_by0, stop);
        }
        let (pw, group_bw, round, opts) = (self.pw, self.group_bw, self.round, self.opts);
        let (coder, enc, lam) = (&self.coder, &self.enc, &self.lam);
        let cm = enc.b.m8;
        let (bw, nby) = (sc.bw, sc.nby);
        sc.modes = crate::lossy::adaptive_rdo::try_vec(nby * bw, 0u8)?;
        if opts.small.is_some() {
            sc.splits = crate::lossy::adaptive_rdo::try_vec(nby * bw, 0u8)?;
        }
        let plane_len = NCH
            .checked_mul(8)
            .and_then(|v| v.checked_mul(pw))
            .ok_or(crate::Error::TooLarge)?;
        let coef_len = NCH
            .checked_mul(bw)
            .and_then(|v| v.checked_mul(64))
            .ok_or(crate::Error::TooLarge)?;
        let mut planes = [
            crate::lossy::adaptive_rdo::try_vec(plane_len, 0.0f32)?,
            crate::lossy::adaptive_rdo::try_vec(plane_len, 0.0f32)?,
        ];
        let mut ext: [Vec<(u64, bool)>; 2] = [
            crate::lossy::adaptive_rdo::try_vec_cap(bw)?,
            crate::lossy::adaptive_rdo::try_vec_cap(bw)?,
        ];
        let mut coefs = [
            crate::lossy::adaptive_rdo::try_vec(coef_len, 0.0f32)?,
            crate::lossy::adaptive_rdo::try_vec(coef_len, 0.0f32)?,
        ];
        let mut work = Work::new();
        let mut flags = FlagCost::new();
        let mut written = Vec::new();
        for r in (0..nby).step_by(2) {
            stop.check().map_err(|_| crate::Error::Cancelled)?;
            let rr = if r + 1 < nby { 2 } else { 1 };
            for k in 0..rr {
                ext[k].clear();
                rows(sc.by0 + r + k, &mut planes[k], &mut ext[k]);
                for ch in 0..NCH {
                    enc_simd::fdct_row(
                        &planes[k][ch * 8 * pw..][..8 * pw],
                        pw,
                        bw,
                        &cm,
                        &mut coefs[k][ch * bw * 64..][..bw * 64],
                    );
                }
            }
            let mut bx = 0;
            while bx < bw {
                // Groups are aligned within each column group.
                let gs = bx.checked_div(group_bw).map_or(0, |g| g * group_bw);
                let ge = if group_bw == 0 {
                    bw
                } else {
                    (gs + group_bw).min(bw)
                };
                let cols = if bx + 1 < ge { 2 } else { 1 };
                if rr * cols > 1 || opts.small.is_some() {
                    let g = Group2 {
                        r,
                        rows: rr,
                        bx,
                        cols,
                    };
                    decide(
                        &mut sc,
                        enc,
                        &planes,
                        &ext,
                        &coefs,
                        pw,
                        &g,
                        round,
                        coder,
                        lam,
                        opts,
                        &mut work,
                        &mut flags,
                        &mut written,
                    )?;
                }
                bx += cols;
            }
        }
        if opts.small.is_some_and(|f| f.is_none()) {
            prune_units(&mut sc, group_bw, &written);
        }
        Ok(sc)
    }

    /// Surrounding-block adaptive RDO stripe selector: trellis-priced solo and
    /// merged candidates, exact 8-state row-strip Viterbi DP over the 9-context
    /// partition-map model, exact unit-level V4 partition-map merge pruning,
    /// empirical pilot-pass map/flag tables, and unified open-loop lambda AQ.
    fn stripe_rdo(
        &self,
        mut sc: StripeCoefs,
        rows: RowPlanes,
        lambda_map: &[f32],
        lambda_by0: usize,
        stop: &dyn how_far::Stop,
    ) -> Result<StripeCoefs, crate::Error> {
        use crate::lossy::adaptive_rdo::{
            MapBitTable, solve_row_strip_dp, sym_class, try_push, try_vec, try_vec_cap,
        };
        let (pw, group_bw, round, opts) = (self.pw, self.group_bw, self.round, self.opts);
        let rdo = opts.rdo;
        let (coder, enc, lam) = (&self.coder, &self.enc, &self.lam);
        let cm = enc.b.m8;
        let (bw, nby) = (sc.bw, sc.nby);
        let snb = nby.checked_mul(bw).ok_or(crate::Error::TooLarge)?;
        sc.modes = try_vec(snb, 0u8)?;
        if opts.small.is_some() {
            sc.splits = try_vec(snb, 0u8)?;
        }
        let map_bits = if let Some(m) = self.learned_map {
            m
        } else if rdo.map_dp {
            MapBitTable::prior()
        } else {
            MapBitTable::flat()
        };
        let mut flags = match self.learned_flags {
            Some(lf) => FlagCost { n: lf.n },
            None => FlagCost::new(),
        };
        let plane_len = NCH
            .checked_mul(8)
            .and_then(|v| v.checked_mul(pw))
            .ok_or(crate::Error::TooLarge)?;
        let coef_len = NCH
            .checked_mul(bw)
            .and_then(|v| v.checked_mul(64))
            .ok_or(crate::Error::TooLarge)?;
        let mut planes = [try_vec(plane_len, 0.0f32)?, try_vec(plane_len, 0.0f32)?];
        let mut ext: [Vec<(u64, bool)>; 2] = [try_vec_cap(bw)?, try_vec_cap(bw)?];
        let mut coefs = [try_vec(coef_len, 0.0f32)?, try_vec(coef_len, 0.0f32)?];
        let mut work = Work::new();
        let mut tcands: alloc::boxed::Box<crate::lossy::trellis::Cands> =
            alloc::boxed::Box::default();
        let mut written = Vec::new();

        let gw = if group_bw == 0 { bw } else { group_bw };
        let n_cg = bw.div_ceil(gw);
        let mut prev_sym_cg: Vec<Vec<u8>> = try_vec_cap(n_cg)?;
        for ci in 0..n_cg {
            let gs = ci * gw;
            let ge = (gs + gw).min(bw);
            prev_sym_cg.push(try_vec((ge - gs).div_ceil(2), 0u8)?);
        }

        // Unit-level merge pruning: record exact partition symbols, pre-merge
        // solo backups, and merge gain per column group so marginal merges that
        // do not pay for the actual V4 partition map bytes are reverted cleanly.
        struct MergedBackup {
            b: usize,
            q8: [[i32; 64]; NCH],
            nz8: [u64; NCH],
            aq8: Option<u8>,
            ok: bool,
            solo: Solo,
        }
        let grows = nby.div_ceil(2);
        let mut cg_syms: Vec<Vec<u8>> = try_vec_cap(n_cg)?;
        for ci in 0..n_cg {
            let gs = ci * gw;
            let ge = (gs + gw).min(bw);
            let sym_len = grows
                .checked_mul((ge - gs).div_ceil(2))
                .ok_or(crate::Error::TooLarge)?;
            cg_syms.push(try_vec(sym_len, 0u8)?);
        }
        let mut cg_merge_gain: Vec<f64> = try_vec(n_cg, 0.0)?;
        let mut cg_backups: Vec<Vec<MergedBackup>> = try_vec_cap(n_cg)?;
        for _ in 0..n_cg {
            cg_backups.push(Vec::new());
        }
        let mut has_prev_row = false;

        let prev_refs = |sc: &StripeCoefs, b: usize| -> Refs {
            Refs {
                eob: lane_neighbors(b, bw, nby)
                    .0
                    .map_or(0, |p| support(sc.nz[p * NCH] & !1)),
                count: if b > 0 {
                    support(sc.nz[(b - 1) * NCH] & !1).div_ceil(4)
                } else {
                    0
                },
            }
        };
        let ac_of = |sc: &StripeCoefs, b: usize| {
            (0..NCH).fold(0u8, |m, ch| {
                let u = &sc.q[(b * NCH + ch) * 64..][..64];
                m | (u[1..].iter().any(|&v| v != 0) as u8) << ch
            })
        };

        struct GroupEval {
            bx: usize,
            cols: usize,
            ok: [[bool; 2]; 2],
            solo: [[Solo; 2]; 2],
            cands: [MCand; 5],
            cand_ix: [[Option<usize>; 4]; 3],
        }

        let dummy_cand = MCand {
            shape: Shape::S16x16,
            j: 0.0,
            units: [[[0; 64]; 4]; NCH],
            dcs: [[[0; 2]; 2]; NCH],
            aq: 64,
        };
        let dummy_solo = Solo {
            j: 0.0,
            split: 0,
            q4: [[0; 64]; NCH],
            gain: [0.0; NCH],
        };

        for r in (0..nby).step_by(2) {
            stop.check().map_err(|_| crate::Error::Cancelled)?;
            let rr = if r + 1 < nby { 2 } else { 1 };
            for k in 0..rr {
                ext[k].clear();
                rows(sc.by0 + r + k, &mut planes[k], &mut ext[k]);
                for ch in 0..NCH {
                    enc_simd::fdct_row(
                        &planes[k][ch * 8 * pw..][..8 * pw],
                        pw,
                        bw,
                        &cm,
                        &mut coefs[k][ch * bw * 64..][..bw * 64],
                    );
                }
            }

            for ci in 0..n_cg {
                let gs = ci * gw;
                let ge = (gs + gw).min(bw);
                let ng = (ge - gs).div_ceil(2);
                let mut evals: Vec<GroupEval> = try_vec_cap(ng)?;
                let mut unary: Vec<[f64; 8]> = try_vec_cap(ng)?;
                let mut up_class: Vec<u8> = try_vec_cap(ng)?;

                for gc in 0..ng {
                    stop.check().map_err(|_| crate::Error::Cancelled)?;
                    let bx = gs + gc * 2;
                    let cols = (ge - bx).min(2);
                    let g = Group2 {
                        r,
                        rows: rr,
                        bx,
                        cols,
                    };
                    let b_of = |py: usize, px: usize| (g.r + py) * bw + g.bx + px;
                    let code = |b: usize| sc.aq.get(b).copied().unwrap_or(64);
                    let mut ok = [[false; 2]; 2];
                    for py in 0..g.rows {
                        for px in 0..g.cols {
                            let extreme = ext[py]
                                .get(g.bx + px)
                                .is_some_and(|&(white, black)| white == u64::MAX || black);
                            ok[py][px] = code(b_of(py, px)) != 0 && !extreme;
                        }
                    }
                    let all_ok = (0..g.rows).all(|py| (0..g.cols).all(|px| ok[py][px]));
                    let merges = opts.merges && all_ok && g.rows * g.cols > 1;

                    let mut cand = [[0u8; 2]; 2];
                    if opts.small.is_some() {
                        for py in 0..g.rows {
                            for px in 0..g.cols {
                                if !ok[py][px] {
                                    continue;
                                }
                                let aq = code(b_of(py, px)) as f32 / 64.0;
                                for ch in 0..NCH {
                                    let c = &coefs[py][(ch * bw + g.bx + px) * 64..][..64];
                                    let ac_energy: f32 = c[1..].iter().map(|&v| v * v).sum();
                                    let thr = enc.min4[ch] * aq * (1.0 - round.abs());
                                    cand[py][px] |= ((ac_energy >= thr * thr) as u8) << ch;
                                }
                            }
                        }
                    }

                    let mut w = [0.0f64; NCH];
                    let mut group_norm = [0.0f64; NCH];
                    let mut wt = [[[0.0f32; 2]; 2]; NCH];
                    let mut lmul_max = 0.0f64;
                    if !lambda_map.is_empty() {
                        for py in 0..g.rows {
                            let rel_by = (sc.by0 + g.r + py).saturating_sub(lambda_by0);
                            for px in 0..g.cols {
                                let gi = rel_by * bw + g.bx + px;
                                let m = lambda_map.get(gi).copied().unwrap_or(1.0) as f64;
                                if m > lmul_max {
                                    lmul_max = m;
                                }
                            }
                        }
                    }
                    let lmul_g = if lmul_max > 0.0 { lmul_max } else { 1.0 };
                    let n_par = (g.rows * g.cols) as f64;
                    for ch in 0..NCH {
                        let mut sum = 0.0f64;
                        for py in 0..g.rows {
                            for px in 0..g.cols {
                                let c = &coefs[py][(ch * bw + g.bx + px) * 64..][..64];
                                let e: f64 = c[1..].iter().map(|&v| (v as f64) * (v as f64)).sum();
                                sum += e;
                                let aq = code(b_of(py, px)) as f32 / 64.0;
                                let d = enc.steps8[ch][1] * aq;
                                let ms = (e / 63.0) as f32 / (d * d);
                                wt[ch][py][px] = FLAT_A0 / (FLAT_A0 + ms);
                            }
                        }
                        group_norm[ch] = sum / n_par;
                        w[ch] = 64.0 * lam.lambda(group_norm[ch] * 64.0 / 63.0) * lmul_g;
                    }

                    let mut solo = [[dummy_solo; 2]; 2];
                    for py in 0..g.rows {
                        for px in 0..g.cols {
                            let small = opts.small.filter(|_| ok[py][px]);
                            solo[py][px] = solo_parent_rdo(
                                &sc,
                                enc,
                                &planes,
                                &coefs,
                                pw,
                                &g,
                                (py, px),
                                &w,
                                &group_norm,
                                lmul_g,
                                &wt,
                                round,
                                coder,
                                lam,
                                small.map(|f| (f, cand[py][px])),
                                &opts,
                                &flags,
                                &prev_refs,
                                &mut tcands,
                            );
                        }
                    }

                    let Work { sp, cands, err } = &mut *work;
                    let mut cand_ix = [[None::<usize>; 4]; 3];
                    if merges {
                        enc.spectra(&planes, pw, &g, sp);
                        let mut n = 0;
                        for blocks in BLOCKS.iter().skip(1) {
                            for &(s, py0, px0) in *blocks {
                                if py0 + s.ph() > g.rows
                                    || px0 + s.pw() > g.cols
                                    || opts.shapes >> s as u8 & 1 == 0
                                {
                                    continue;
                                }
                                if cand_ix[s as usize][py0 * 2 + px0].is_none() {
                                    cand_ix[s as usize][py0 * 2 + px0] = Some(n);
                                    let c = &mut cands[n];
                                    let pr = |b: usize| prev_refs(&sc, b);
                                    merged_rdo(
                                        &sc,
                                        enc,
                                        sp,
                                        &g,
                                        (s, py0, px0),
                                        &w,
                                        &group_norm,
                                        lmul_g,
                                        &wt,
                                        round,
                                        coder,
                                        lam,
                                        &opts,
                                        &pr,
                                        &mut tcands,
                                        c,
                                        err,
                                    );
                                    n += 1;
                                }
                            }
                        }
                    }

                    let mut u_row = [f64::INFINITY; 8];
                    let has = [true, g.cols == 2, g.rows == 2, g.cols == 2 && g.rows == 2];

                    for (sym, roles) in PARTITIONS.iter().enumerate() {
                        if sym != 0 && !merges {
                            continue;
                        }
                        if (0..4).any(|i| roles[i] != 0 && !has[i]) {
                            continue;
                        }
                        let mut j = 0.0f64;
                        let mut covered = [false; 4];
                        let mut allowed = true;
                        for &(s, py0, px0) in BLOCKS[sym] {
                            let Some(ci) = cand_ix[s as usize][py0 * 2 + px0] else {
                                allowed = false;
                                break;
                            };
                            j += cands[ci].j + DC_KAPPA * (s.ph() * s.pw()) as f64;
                            for py in 0..s.ph() {
                                for px in 0..s.pw() {
                                    covered[(py0 + py) * 2 + px0 + px] = true;
                                }
                            }
                        }
                        if !allowed {
                            continue;
                        }
                        for py in 0..g.rows {
                            for px in 0..g.cols {
                                if !covered[py * 2 + px] {
                                    j += solo[py][px].j;
                                }
                            }
                        }
                        u_row[sym] = j;
                    }

                    unary.push(u_row);
                    up_class.push(if has_prev_row {
                        sym_class(prev_sym_cg[ci][gc]) as u8
                    } else {
                        0
                    });
                    let mut saved_cands: [MCand; 5] = core::array::from_fn(|_| dummy_cand.clone());
                    if merges {
                        saved_cands.clone_from_slice(cands);
                    }
                    evals.push(GroupEval {
                        bx,
                        cols,
                        ok,
                        solo,
                        cands: saved_cands,
                        cand_ix,
                    });
                }

                // Exact 8-state row-strip Viterbi DP across the column group's row.
                // Exclude a trailing 1x1 corner (odd unit rows AND odd unit cols),
                // which `superblock::map_groups` omits and emits zero map bits for.
                let trailing_1x1 = rr == 1 && !(ge - gs).is_multiple_of(2);
                let dp_ng = if trailing_1x1 { ng - 1 } else { ng };
                let mut path = if dp_ng > 0 {
                    solve_row_strip_dp(
                        &unary[..dp_ng],
                        &up_class[..dp_ng],
                        &map_bits,
                        |_, _, _| 0.0,
                    )?
                    .1
                } else {
                    Vec::new()
                };
                if trailing_1x1 {
                    try_push(&mut path, 0)?;
                }

                // Commit winning partition symbols to `sc`.
                let gr = r / 2;
                let gcols_cg = (ge - gs).div_ceil(2);
                for gc in 0..ng {
                    let sym = path[gc] as usize;
                    let ev = &evals[gc];
                    let sym_out = if rr == 1 && ev.cols == 1 {
                        0
                    } else {
                        sym as u8
                    };
                    cg_syms[ci][gr * gcols_cg + gc] = sym_out;
                    if sym != 0 {
                        cg_merge_gain[ci] += unary[gc][0] - unary[gc][sym];
                    }
                    let mut covered = [false; 4];
                    for &(s, py0, px0) in BLOCKS[sym] {
                        for py in 0..s.ph() {
                            for px in 0..s.pw() {
                                let b = (r + py0 + py) * bw + ev.bx + px0 + px;
                                let q8: [[i32; 64]; NCH] = core::array::from_fn(|ch| {
                                    sc.q[(b * NCH + ch) * 64..][..64].try_into().unwrap()
                                });
                                let nz8: [u64; NCH] =
                                    core::array::from_fn(|ch| sc.nz[b * NCH + ch]);
                                let aq8 = sc.aq.get(b).copied();
                                try_push(
                                    &mut cg_backups[ci],
                                    MergedBackup {
                                        b,
                                        q8,
                                        nz8,
                                        aq8,
                                        ok: ev.ok[py0 + py][px0 + px],
                                        solo: ev.solo[py0 + py][px0 + px],
                                    },
                                )?;
                            }
                        }
                        let ci_c = ev.cand_ix[s as usize][py0 * 2 + px0].expect("candidate");
                        write_merged(&mut sc, s, r + py0, ev.bx + px0, &ev.cands[ci_c]);
                        for py in 0..s.ph() {
                            for px in 0..s.pw() {
                                covered[(py0 + py) * 2 + px0 + px] = true;
                            }
                        }
                    }
                    if opts.small.is_some() {
                        for py in 0..rr {
                            for px in 0..ev.cols {
                                if covered[py * 2 + px] || !ev.ok[py][px] {
                                    continue;
                                }
                                let so = &ev.solo[py][px];
                                let b = (r + py) * bw + ev.bx + px;
                                for ch in 0..NCH {
                                    if so.split >> ch & 1 != 0 {
                                        try_push(
                                            &mut written,
                                            Written {
                                                b,
                                                ch,
                                                q8: sc.q[(b * NCH + ch) * 64..][..64]
                                                    .try_into()
                                                    .unwrap(),
                                                nz8: sc.nz[b * NCH + ch],
                                                gain: so.gain[ch],
                                            },
                                        )?;
                                    }
                                }
                                write_split(&mut sc, b, so);
                                if self.learned_flags.is_none() {
                                    flags.add(so.split, ac_of(&sc, b));
                                }
                            }
                        }
                    }
                    prev_sym_cg[ci][gc] = sym_out;
                }
            }
            has_prev_row = true;
        }
        // Unit-level merge pruning: revert column groups whose merges do not
        // repay the exact V4 range-coded partition map overhead.
        if opts.merges {
            for ci in 0..n_cg {
                if cg_backups[ci].is_empty() {
                    continue;
                }
                let gs = ci * gw;
                let ge = (gs + gw).min(bw);
                let syms = &cg_syms[ci];
                let map_bits = super::map::encode_v4_partitions(nby, ge - gs, |i| syms[i])
                    .map_or(f64::INFINITY, |m| (m.len() as f64 + 2.0) * 8.0);
                if cg_merge_gain[ci] < map_bits {
                    for bk in &cg_backups[ci] {
                        sc.modes[bk.b] = 0;
                        if let Some(a) = bk.aq8 {
                            sc.aq[bk.b] = a;
                        }
                        for ch in 0..NCH {
                            sc.q[(bk.b * NCH + ch) * 64..][..64].copy_from_slice(&bk.q8[ch]);
                            sc.nz[bk.b * NCH + ch] = bk.nz8[ch];
                        }
                        if opts.small.is_some() && bk.ok {
                            for ch in 0..NCH {
                                if bk.solo.split >> ch & 1 != 0 {
                                    try_push(
                                        &mut written,
                                        Written {
                                            b: bk.b,
                                            ch,
                                            q8: bk.q8[ch],
                                            nz8: bk.nz8[ch],
                                            gain: bk.solo.gain[ch],
                                        },
                                    )?;
                                }
                            }
                            write_split(&mut sc, bk.b, &bk.solo);
                        }
                    }
                }
            }
        }
        if opts.small.is_some_and(|f| f.is_none()) {
            prune_units(&mut sc, group_bw, &written);
        }
        stop.check().map_err(|_| crate::Error::Cancelled)?;
        Ok(sc)
    }
}

/// Trellis-quantize a single 64-slot candidate AC unit under `coder`.
#[allow(clippy::too_many_arguments)]
fn trellis_unit(
    coder: &Coder,
    src: &[f32; 64],
    steps_aq: &[f32; 64],
    ch: usize,
    r: Refs,
    lam: &LossyTrellis,
    norm_override: Option<f64>,
    lambda_mul: f64,
    cands: &mut crate::lossy::trellis::Cands,
    q: &mut [i32; 64],
    nz: &mut u64,
) {
    match coder {
        Coder::RunLevel(p) => {
            crate::lossy::trellis::trellis_unit_rl(
                src,
                steps_aq,
                ch,
                r.eob,
                p,
                lam,
                norm_override,
                lambda_mul,
                cands,
                q,
                nz,
            );
        }
        Coder::SplitHuf(h) => {
            crate::lossy::trellis::trellis_unit_sh(
                src,
                steps_aq,
                ch,
                r.count,
                h,
                lam,
                norm_override,
                lambda_mul,
                q,
                nz,
            );
        }
    }
}

/// Surrounding-block adaptive RDO version of [`solo_parent`]: supports
/// trellis-priced solo 8x8/4x4 candidates under unified group lambda.
#[allow(clippy::too_many_arguments)]
fn solo_parent_rdo(
    sc: &StripeCoefs,
    enc: &Enc,
    planes: &[Vec<f32>; 2],
    coefs: &[Vec<f32>; 2],
    pw: usize,
    g: &Group2,
    (py, px): (usize, usize),
    w: &[f64; NCH],
    group_norm: &[f64; NCH],
    lambda_mul: f64,
    wt: &[[[f32; 2]; 2]; NCH],
    round: f32,
    coder: &Coder,
    lam: &LossyTrellis,
    small: Option<(Option<u8>, u8)>,
    opts: &Opts,
    flags: &FlagCost,
    prev_refs: &dyn Fn(&StripeCoefs, usize) -> Refs,
    tcands: &mut crate::lossy::trellis::Cands,
) -> Solo {
    let tr_on = opts.rdo.trellis_select && opts.trellis;
    let bw = sc.bw;
    let b = (g.r + py) * bw + g.bx + px;
    let aq = sc.aq.get(b).copied().unwrap_or(64) as f32 / 64.0;
    let r0 = prev_refs(sc, b);

    let mut q8: [[i32; 64]; NCH] =
        core::array::from_fn(|ch| sc.q[(b * NCH + ch) * 64..][..64].try_into().unwrap());
    let mut m8 = q8.map(|q| nonzero(&q));
    if tr_on && sc.aq.get(b) != Some(&0) {
        let mut lref = r0;
        for ch in 0..NCH {
            let c: &[f32; 64] = coefs[py][(ch * bw + g.bx + px) * 64..][..64]
                .try_into()
                .unwrap();
            let steps_aq: [f32; 64] = core::array::from_fn(|i| {
                if i == 0 {
                    enc.steps8[ch][0]
                } else {
                    enc.steps8[ch][i] * aq
                }
            });
            let mut nz = m8[ch] | u64::from(q8[ch][0] != 0);
            let rf = if ch == 0 { r0 } else { lref };
            trellis_unit(
                coder,
                c,
                &steps_aq,
                ch,
                rf,
                lam,
                Some(group_norm[ch]),
                lambda_mul,
                tcands,
                &mut q8[ch],
                &mut nz,
            );
            m8[ch] = nz & !1;
            if ch == 0 {
                lref = Refs::of_mask(m8[0]);
            }
        }
    }

    let (mut d8, mut x8) = ([0.0f64; NCH], [0.0f64; NCH]);
    let mut q4 = [[0i32; 64]; NCH];
    let (mut m4, mut d4, mut x4) = ([0u64; NCH], [0.0f64; NCH], [0.0f64; NCH]);
    let kx = (opts.kappa - 1.0) as f64;
    let mut lref4 = Refs::of_mask(m8[0]);

    for ch in 0..NCH {
        let c: &[f32; 64] = coefs[py][(ch * bw + g.bx + px) * 64..][..64]
            .try_into()
            .unwrap();
        let st = &enc.steps8[ch];
        let first = st[1] * aq;
        let (d, en) = incant!(
            terms_impl(c, &q8[ch], (&enc.inv8[ch], st), aq),
            [v3, neon, wasm128, scalar]
        );
        let ring8 = RING * ring_unit(first, wt[ch][py][px], en);
        (d8[ch], x8[ch]) = (w[ch] * (d + ring8), w[ch] * kx * (d + ring8));

        if small.is_some_and(|(_, cand)| cand >> ch & 1 != 0) {
            let mut pix = [0.0f32; 64];
            for (y, row) in pix.as_chunks_mut::<8>().0.iter_mut().enumerate() {
                row.copy_from_slice(&planes[py][ch * 8 * pw + y * pw + (g.bx + px) * 8..][..8]);
            }
            let cf = (opts.forward)(&pix);
            let (mut d_c4, mut en_c4) = incant!(
                cand4_impl(
                    &cf,
                    (&enc.inv4[ch], &enc.steps4[ch]),
                    (aq, round),
                    &mut q4[ch]
                ),
                [v3, neon, wasm128, scalar]
            );
            q4[ch][0] = q8[ch][0];
            m4[ch] = nonzero(&q4[ch]);
            if tr_on && sc.aq.get(b) != Some(&0) {
                let steps4_aq: [f32; 64] = core::array::from_fn(|i| {
                    if i == 0 {
                        enc.steps4[ch][0]
                    } else {
                        enc.steps4[ch][i] * aq
                    }
                });
                let mut nz = m4[ch] | u64::from(q4[ch][0] != 0);
                let rf = if ch == 0 { r0 } else { lref4 };
                trellis_unit(
                    coder,
                    &cf,
                    &steps4_aq,
                    ch,
                    rf,
                    lam,
                    Some(group_norm[ch]),
                    lambda_mul,
                    tcands,
                    &mut q4[ch],
                    &mut nz,
                );
                m4[ch] = nz & !1;
                if ch == 0 && m4[0] != 0 {
                    lref4 = Refs::of_mask(m4[0]);
                }
                let (dt, et) = incant!(
                    terms_impl(&cf, &q4[ch], (&enc.inv4[ch], &enc.steps4[ch]), aq),
                    [v3, neon, wasm128, scalar]
                );
                d_c4 = dt;
                en_c4 = et;
            }
            let ring4 = RING * ring_unit(first, wt[ch][py][px], en_c4);
            (d4[ch], x4[ch]) = (w[ch] * (d_c4 + ring4), w[ch] * kx * (d_c4 + ring4));
            if opts.edge > 0.0 {
                let first64 = first as f64;
                let k = w[ch] * (opts.kappa * opts.edge) as f64 / (first64 * first64);
                x8[ch] += k * inner_edge(&pix, &recon(&q8[ch], &enc.dq8[ch], aq, false));
                x4[ch] += k * inner_edge(&pix, &recon(&q4[ch], &enc.dq4[ch], aq, true));
            }
        }
    }

    let unit = |ch: usize, s: usize| -> (&[i32; 64], u64) {
        if s == 0 {
            (&q8[ch], m8[ch])
        } else {
            (&q4[ch], m4[ch])
        }
    };
    let lref = [Refs::of_mask(m8[0]), Refs::of_mask(m4[0])];
    let mut lmemo = [None::<f64>; 2];
    let mut lbits = |s: usize| {
        *lmemo[s].get_or_insert_with(|| {
            let (q, m) = unit(0, s);
            unit_bits_m(coder, 0, q, m, r0)
        })
    };
    let mut cmemo = [[[None::<f64>; 2]; 2]; NCH];
    let mut cbits = |ch: usize, ls: usize, s: usize| {
        *cmemo[ch][ls][s].get_or_insert_with(|| {
            let (q, m) = unit(ch, s);
            unit_bits_m(coder, ch, q, m, lref[ls])
        })
    };
    let Some((force, _)) = small else {
        let mut j = 0.0;
        for ch in 0..NCH {
            j += d8[ch] + if ch == 0 { lbits(0) } else { cbits(ch, 0, 0) };
        }
        return Solo {
            j,
            split: 0,
            q4: [[0; 64]; NCH],
            gain: [0.0; NCH],
        };
    };
    let ac4 = m4.map(|m| m != 0);
    let mut best = (f64::MAX, 0u8, 0.0f64);
    for lsplit in 0..2u8 {
        if force.is_some_and(|m| (m & 1) != lsplit) || (lsplit == 1 && !ac4[0]) {
            continue;
        }
        let ls = lsplit as usize;
        let (lc, lx, lm) = if ls == 0 {
            (d8[0], x8[0], m8[0])
        } else {
            (d4[0], x4[0], m4[0])
        };
        let fl = if lm != 0 { flags.bits(0, 0, ls) } else { 0.0 };
        let mut j = lc + lbits(ls) + fl;
        let mut x = lx;
        let mut sp = lsplit;
        for ch in 1..NCH {
            let mut bj = (f64::MAX, 0u8, 0.0f64);
            for csplit in 0..2u8 {
                if force.is_some_and(|m| (m >> ch & 1) != csplit) || (csplit == 1 && !ac4[ch]) {
                    continue;
                }
                let cs = csplit as usize;
                let (cc, cx, cm) = if cs == 0 {
                    (d8[ch], x8[ch], m8[ch])
                } else {
                    (d4[ch], x4[ch], m4[ch])
                };
                let fl = if cm != 0 { flags.bits(ch, ls, cs) } else { 0.0 };
                let jj = cc + cbits(ch, ls, cs) + fl;
                if jj + cx < bj.0 + bj.2 {
                    bj = (jj, csplit, cx);
                }
            }
            if bj.0 == f64::MAX {
                bj = (d8[ch] + cbits(ch, ls, 0), 0, 0.0);
            }
            j += bj.0;
            x += bj.2;
            sp |= bj.1 << ch;
        }
        if j + x < best.0 + best.2 {
            best = (j, sp, x);
        }
    }
    if best.0 == f64::MAX {
        let mut j = 0.0;
        for ch in 0..NCH {
            j += d8[ch] + if ch == 0 { lbits(0) } else { cbits(ch, 0, 0) };
        }
        best = (j, 0, 0.0);
    }
    let mut out = [[0i32; 64]; NCH];
    let mut gain = [0.0f64; NCH];
    let ls = (best.1 & 1) as usize;
    for ch in 0..NCH {
        if best.1 >> ch & 1 != 0 {
            out[ch] = q4[ch];
            let (b8, b4) = if ch == 0 {
                (lbits(0), lbits(1))
            } else {
                (cbits(ch, ls, 0), cbits(ch, ls, 1))
            };
            gain[ch] = (d8[ch] + x8[ch] + b8) - (d4[ch] + x4[ch] + b4);
        }
    }
    Solo {
        j: best.0,
        split: best.1,
        q4: out,
        gain,
    }
}

/// Surrounding-block adaptive RDO version of [`merged`]: supports
/// trellis-priced merged superblock units under unified group lambda.
#[allow(clippy::too_many_arguments)]
fn merged_rdo(
    sc: &StripeCoefs,
    enc: &Enc,
    sp: &[Spectra; NCH],
    g: &Group2,
    (s, py0, px0): (Shape, usize, usize),
    w: &[f64; NCH],
    group_norm: &[f64; NCH],
    lambda_mul: f64,
    wt: &[[[f32; 2]; 2]; NCH],
    round: f32,
    coder: &Coder,
    lam: &LossyTrellis,
    opts: &Opts,
    prev_refs: &dyn Fn(usize) -> Refs,
    tcands: &mut crate::lossy::trellis::Cands,
    out: &mut MCand,
    spec_err: &mut [[f32; 256]; NCH],
) {
    let tr_on = opts.rdo.trellis_select && opts.trellis;
    let bw = sc.bw;
    let b_of = |py: usize, px: usize| (g.r + py0 + py) * bw + g.bx + px0 + px;
    let aqc = (0..s.ph())
        .flat_map(|py| (0..s.pw()).map(move |px| (py, px)))
        .map(|(py, px)| sc.aq.get(b_of(py, px)).copied().unwrap_or(64))
        .min()
        .unwrap();
    let aq = aqc as f32 / 64.0;
    out.shape = s;
    out.j = 0.0;
    out.aq = aqc;
    let si = s as usize;
    let gather = &enc.gather[si];
    let n_p = s.ph() * s.pw();

    if tr_on {
        let mut lrefs = [Refs { eob: 0, count: 0 }; 4];
        for ch in 0..NCH {
            let c = sp[ch].get(s, py0, px0);
            let dd = llf_to_dcs(s, [[c[0], c[1]], [c[16], c[17]]]);
            for py in 0..s.ph() {
                for px in 0..s.pw() {
                    let v = dd[py][px] / enc.steps8[ch][0];
                    let r = crate::mathx::round_pos(v.abs() as f64).min(4_194_303.0) as i32;
                    out.dcs[ch][py][px] = if v < 0.0 { -r } else { r };
                }
            }
            let st = &enc.steps[ch][si];
            let inv = &enc.inv[ch][si];
            let sc_k = &enc.sc[si];
            let mut qs = [0i32; 256];
            for p in 0..n_p {
                let mut usrc = [0.0f32; 64];
                let mut usteps = [1.0f32; 64];
                for z in 1..64 {
                    let idx = gather[p * 64 + z] as usize;
                    usrc[z] = c[idx];
                    usteps[z] = st[idx] * aq;
                }
                let b = b_of(p / s.pw(), p % s.pw());
                let rf = if ch == 0 { prev_refs(b) } else { lrefs[p] };
                out.units[ch][p] = [0i32; 64];
                let mut nz = 0u64;
                trellis_unit(
                    coder,
                    &usrc,
                    &usteps,
                    ch,
                    rf,
                    lam,
                    Some(group_norm[ch]),
                    lambda_mul,
                    tcands,
                    &mut out.units[ch][p],
                    &mut nz,
                );
                if ch == 0 {
                    lrefs[p] = Refs::of_mask(nz & !1);
                }
                for z in 1..64 {
                    qs[gather[p * 64 + z] as usize] = out.units[ch][p][z];
                }
            }
            spec_err[ch].fill(0.0);
            let inv_aq = 1.0 / aq;
            let mut d = 0.0f64;
            for v in 0..s.h() {
                for u in 0..s.w() {
                    if v < s.ph() && u < s.pw() {
                        continue;
                    }
                    let idx = v * 16 + u;
                    let cv = c[idx];
                    let qv = qs[idx] as f32;
                    let e_step = cv * inv[idx] * inv_aq - qv;
                    d += (e_step as f64) * (e_step as f64);
                    spec_err[ch][idx] = (cv - qv * st[idx] * aq) * sc_k[idx];
                }
            }
            out.j += w[ch] * d;
        }
    } else {
        let mut qs = [0i32; 256];
        for ch in 0..NCH {
            let c = sp[ch].get(s, py0, px0);
            let tabs = (&enc.inv[ch][si], &enc.steps[ch][si], &enc.sc[si]);
            let d = incant!(
                quant_impl(
                    (s.h(), s.w() / 8),
                    c,
                    tabs,
                    (aq, round),
                    &mut qs,
                    &mut spec_err[ch]
                ),
                [v3, neon, wasm128, scalar]
            );
            for (p, unit) in out.units[ch].iter_mut().enumerate().take(n_p) {
                for (z, v) in unit.iter_mut().enumerate() {
                    *v = qs[gather[p * 64 + z] as usize];
                }
            }
            out.j += w[ch] * d as f64;
            let dd = llf_to_dcs(s, [[c[0], c[1]], [c[16], c[17]]]);
            for py in 0..s.ph() {
                for px in 0..s.pw() {
                    let v = dd[py][px] / enc.steps8[ch][0];
                    let r = crate::mathx::round_pos(v.abs() as f64).min(4_194_303.0) as i32;
                    out.dcs[ch][py][px] = if v < 0.0 { -r } else { r };
                }
            }
        }
    }

    for (ch, px_err) in spec_err.iter_mut().enumerate() {
        kernel::inverse_full(s, px_err);
        let first = enc.steps8[ch][1] * aq;
        for py in 0..s.ph() {
            for px in 0..s.pw() {
                let mut acc = [0.0f32; 8];
                for y in 0..8 {
                    let row = &px_err[(py * 8 + y) * 16 + px * 8..][..8];
                    for (a, &v) in acc.iter_mut().zip(row) {
                        *a += v * v;
                    }
                }
                let e: f64 = acc.iter().map(|&a| a as f64).sum();
                out.j += w[ch] * RING * ring_unit(first, wt[ch][py0 + py][px0 + px], e);
            }
        }
    }

    for p in 0..n_p {
        let b = b_of(p / s.pw(), p % s.pw());
        out.j += unit_bits(coder, 0, &out.units[0][p], prev_refs(b));
        let lref = Refs::of_luma(&out.units[0][p]);
        for ch in 1..NCH {
            out.j += unit_bits(coder, ch, &out.units[ch][p], lref);
        }
    }
}

/// Estimated bits of the split flags (small transforms): adaptive counts
/// per (plane, the parent's luma flag) over a stripe, the coder's contexts
/// without the neighbour terms.
struct FlagCost {
    n: [[[f64; 2]; 2]; NCH],
}

impl FlagCost {
    fn new() -> Self {
        // A split starts at about 2.3 bits, no split at 0.3.
        Self {
            n: [[[1.0, 0.25]; 2]; NCH],
        }
    }

    fn bits(&self, ch: usize, luma: usize, bit: usize) -> f64 {
        let n = self.n[ch][luma];
        ((n[0] + n[1]) / n[bit]).log2()
    }

    /// Counts a parent's flags: `split` its 4x4 planes, `ac` the planes
    /// whose coding has AC (the planes with a flag).
    fn add(&mut self, split: u8, ac: u8) {
        let luma = (split & 1) as usize;
        for ch in 0..NCH {
            if ac >> ch & 1 == 0 {
                continue;
            }
            let l = if ch == 0 { 0 } else { luma };
            self.n[ch][l][(split >> ch & 1) as usize] += 1.0;
        }
    }
}

/// An aligned group of parents: top-left (`r`, `bx`) of a stripe, `rows` x
/// `cols` present.
struct Group2 {
    r: usize,
    rows: usize,
    bx: usize,
    cols: usize,
}

/// A parent coded alone: its cost J and, with small transforms, its 4x4
/// plane bits and those planes' units.
#[derive(Clone, Copy)]
struct Solo {
    j: f64,
    split: u8,
    q4: [[i32; 64]; NCH],
    /// Per 4x4 plane: J of its 8x8 coding minus J of its 4x4 coding, flags
    /// excluded (the unit pruning's gain).
    gain: [f64; NCH],
}

/// A 4x4 plane written by the selector, for the unit pruning: its parent,
/// plane, the 8x8 unit and nonzero mask it replaced, and its gain.
struct Written {
    b: usize,
    ch: usize,
    q8: [i32; 64],
    nz8: u64,
    gain: f64,
}

/// Reverts the 4x4 planes of every unit of stripe `sc` (column groups of
/// `group_bw` blocks; 0 = one) whose summed gain does not pay for the
/// unit's split-flag section: the planes with AC among its unmerged,
/// unbypassed parents, each `N H(K / N)` bits for K splits of N flags, plus
/// half a bit per log2 N of learning per plane that splits and one header
/// bit.
fn prune_units(sc: &mut StripeCoefs, group_bw: usize, written: &[Written]) {
    if written.is_empty() {
        return;
    }
    let (bw, nby) = (sc.bw, sc.nby);
    let gw = if group_bw == 0 { bw } else { group_bw };
    for gs in (0..bw).step_by(gw) {
        let ge = (gs + gw).min(bw);
        let in_unit = |b: usize| (gs..ge).contains(&(b % bw));
        let gain: f64 = written
            .iter()
            .filter(|w| in_unit(w.b))
            .map(|w| w.gain)
            .sum();
        if gain == 0.0 && !written.iter().any(|w| in_unit(w.b)) {
            continue;
        }
        let (mut n, mut k) = ([0u32; NCH], [0u32; NCH]);
        for r in 0..nby {
            for x in gs..ge {
                let b = r * bw + x;
                if sc.modes.get(b).is_some_and(|&m| m != 0) || sc.aq.get(b) == Some(&0) {
                    continue;
                }
                for ch in 0..NCH {
                    if sc.nz[b * NCH + ch] & !1 != 0 {
                        n[ch] += 1;
                        k[ch] += (sc.splits[b] >> ch & 1) as u32;
                    }
                }
            }
        }
        let mut cost = 1.0;
        for ch in 0..NCH {
            if k[ch] == 0 || n[ch] == 0 {
                continue;
            }
            let (nn, kk) = (n[ch] as f64, k[ch] as f64);
            let p = kk / nn;
            let h = if p >= 1.0 {
                0.0
            } else {
                -(p * p.log2() + (1.0 - p) * (1.0 - p).log2())
            };
            cost += nn * h + 0.5 * nn.log2();
        }
        if gain >= cost {
            continue;
        }
        for w in written.iter().filter(|w| in_unit(w.b)) {
            sc.q[(w.b * NCH + w.ch) * 64..][..64].copy_from_slice(&w.q8);
            sc.nz[w.b * NCH + w.ch] = w.nz8;
            sc.splits[w.b] &= !(1 << w.ch);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn decide(
    sc: &mut StripeCoefs,
    enc: &Enc,
    planes: &[Vec<f32>; 2],
    ext: &[Vec<(u64, bool)>; 2],
    coefs: &[Vec<f32>; 2],
    pw: usize,
    g: &Group2,
    round: f32,
    coder: &Coder,
    lam: &LossyTrellis,
    opts: Opts,
    work: &mut Work,
    flags: &mut FlagCost,
    written: &mut Vec<Written>,
) -> Result<(), crate::Error> {
    let (bw, nby) = (sc.bw, sc.nby);
    let b_of = |py: usize, px: usize| (g.r + py) * bw + g.bx + px;
    // Parents that may merge or split: not kept or exact (code 0 here),
    // not an all-white / all-black block (their DCs round toward the
    // clamp).
    let code = |b: usize| sc.aq.get(b).copied().unwrap_or(64);
    let mut ok = [[false; 2]; 2];
    for py in 0..g.rows {
        for px in 0..g.cols {
            let extreme = ext[py]
                .get(g.bx + px)
                .is_some_and(|&(white, black)| white == u64::MAX || black);
            ok[py][px] = code(b_of(py, px)) != 0 && !extreme;
        }
    }
    let all_ok = (0..g.rows).all(|py| (0..g.cols).all(|px| ok[py][px]));
    let merges = opts.merges && all_ok && g.rows * g.cols > 1;
    if !merges && opts.small.is_none() {
        return Ok(());
    }
    // 4x4 candidates per parent, bit `1 << ch`: a plane whose AC energy is
    // below the smallest 4x4 AC threshold quantizes every 4x4 AC slot to
    // zero (each |c| is at most the energy's root), so its 4x4 coding has
    // no AC and is no candidate.
    let mut cand = [[0u8; 2]; 2];
    if opts.small.is_some() {
        for py in 0..g.rows {
            for px in 0..g.cols {
                if !ok[py][px] {
                    continue;
                }
                let aq = code(b_of(py, px)) as f32 / 64.0;
                for ch in 0..NCH {
                    let c = &coefs[py][(ch * bw + g.bx + px) * 64..][..64];
                    let ac_energy: f32 = c[1..].iter().map(|&v| v * v).sum();
                    let thr = enc.min4[ch] * aq * (1.0 - round.abs());
                    cand[py][px] |= ((ac_energy >= thr * thr) as u8) << ch;
                }
            }
        }
    }
    let ac_of = |sc: &StripeCoefs, b: usize| {
        (0..NCH).fold(0u8, |m, ch| {
            let u = &sc.q[(b * NCH + ch) * 64..][..64];
            m | (u[1..].iter().any(|&v| v != 0) as u8) << ch
        })
    };
    if !merges && cand.iter().flatten().all(|&m| m == 0) {
        // Nothing to choose: every parent keeps pass 1's 8x8 coding.
        for py in 0..g.rows {
            for px in 0..g.cols {
                if ok[py][px] {
                    flags.add(0, ac_of(sc, b_of(py, px)));
                }
            }
        }
        return Ok(());
    }
    // Per plane: zenjpeg's trellis lambda on the group's mean AC energy
    // (libjpeg units), and each parent's flatness weight.
    let mut w = [0.0f64; NCH];
    let mut wt = [[[0.0f32; 2]; 2]; NCH];
    for ch in 0..NCH {
        let mut sum = 0.0f64;
        for py in 0..g.rows {
            for px in 0..g.cols {
                let c = &coefs[py][(ch * bw + g.bx + px) * 64..][..64];
                let e: f64 = c[1..].iter().map(|&v| (v as f64) * (v as f64)).sum();
                sum += e;
                let aq = code(b_of(py, px)) as f32 / 64.0;
                let d = enc.steps8[ch][1] * aq;
                let ms = (e / 63.0) as f32 / (d * d);
                wt[ch][py][px] = FLAT_A0 / (FLAT_A0 + ms);
            }
        }
        let n = (g.rows * g.cols) as f64;
        w[ch] = 64.0 * lam.lambda(sum / n * 64.0 / 63.0);
    }
    // Context references of a luma unit: the lane's previous luma EOB
    // (run-level) and the previous block's luma count (split-Huffman).
    let prev_refs = |sc: &StripeCoefs, b: usize| -> Refs {
        Refs {
            eob: lane_neighbors(b, bw, nby)
                .0
                .map_or(0, |p| support(sc.nz[p * NCH] & !1)),
            count: if b > 0 {
                support(sc.nz[(b - 1) * NCH] & !1).div_ceil(4)
            } else {
                0
            },
        }
    };
    // Parents coded alone: as pass 1 quantized them, or per plane the
    // cheaper of that and its 4x4 coding.
    let mut solo = [[Solo {
        j: 0.0,
        split: 0,
        q4: [[0; 64]; NCH],
        gain: [0.0; NCH],
    }; 2]; 2];
    for py in 0..g.rows {
        for px in 0..g.cols {
            // Without merges only the split choice matters (not the cost).
            if !merges && cand[py][px] == 0 {
                continue;
            }
            let small = opts.small.filter(|_| ok[py][px]);
            solo[py][px] = solo_parent(
                sc,
                enc,
                planes,
                coefs,
                pw,
                g,
                (py, px),
                &w,
                &wt,
                round,
                coder,
                small.map(|f| (f, cand[py][px])),
                &opts,
                flags,
                &prev_refs,
            );
        }
    }
    let mut best = (f64::MAX, 0usize);
    let Work { sp, cands, err } = work;
    let mut cand_ix = [[None::<usize>; 4]; 3];
    if merges {
        // Merged candidates, from every plane's candidate spectra.
        enc.spectra(planes, pw, g, sp);
        let mut n = 0;
        for blocks in BLOCKS.iter().skip(1) {
            for &(s, py0, px0) in *blocks {
                if py0 + s.ph() > g.rows || px0 + s.pw() > g.cols || opts.shapes >> s as u8 & 1 == 0
                {
                    continue;
                }
                if cand_ix[s as usize][py0 * 2 + px0].is_none() {
                    cand_ix[s as usize][py0 * 2 + px0] = Some(n);
                    let c = &mut cands[n];
                    let pr = |b: usize| prev_refs(sc, b);
                    merged(
                        sc,
                        enc,
                        sp,
                        g,
                        (s, py0, px0),
                        &w,
                        &wt,
                        round,
                        coder,
                        &pr,
                        c,
                        err,
                    );
                    n += 1;
                }
            }
        }
        // The cheapest partition (ties keep the earlier symbol, no merge
        // first), with the symbol costs of a map without statistics.
        let has = [true, g.cols == 2, g.rows == 2, g.cols == 2 && g.rows == 2];
        for (sym, roles) in PARTITIONS.iter().enumerate() {
            if (0..4).any(|i| roles[i] != 0 && !has[i]) {
                continue;
            }
            let mut j = if sym == 0 {
                SYM_NONE_BITS
            } else {
                SYM_MERGE_BITS
            };
            let mut covered = [false; 4];
            let mut allowed = true;
            for &(s, py0, px0) in BLOCKS[sym] {
                let Some(ci) = cand_ix[s as usize][py0 * 2 + px0] else {
                    allowed = false;
                    break;
                };
                j += cands[ci].j + DC_KAPPA * (s.ph() * s.pw()) as f64;
                for py in 0..s.ph() {
                    for px in 0..s.pw() {
                        covered[(py0 + py) * 2 + px0 + px] = true;
                    }
                }
            }
            if !allowed {
                continue;
            }
            for py in 0..g.rows {
                for px in 0..g.cols {
                    if !covered[py * 2 + px] {
                        j += solo[py][px].j;
                    }
                }
            }
            if j < best.0 {
                best = (j, sym);
            }
        }
    } else {
        best.1 = 0;
    }
    let mut covered = [false; 4];
    for &(s, py0, px0) in BLOCKS[best.1] {
        let ci = cand_ix[s as usize][py0 * 2 + px0].expect("candidate");
        write_merged(sc, s, g.r + py0, g.bx + px0, &cands[ci]);
        for py in 0..s.ph() {
            for px in 0..s.pw() {
                covered[(py0 + py) * 2 + px0 + px] = true;
            }
        }
    }
    if opts.small.is_some() {
        for py in 0..g.rows {
            for px in 0..g.cols {
                if covered[py * 2 + px] || !ok[py][px] {
                    continue;
                }
                let so = &solo[py][px];
                let b = b_of(py, px);
                for ch in 0..NCH {
                    if so.split >> ch & 1 != 0 {
                        crate::lossy::adaptive_rdo::try_push(
                            written,
                            Written {
                                b,
                                ch,
                                q8: sc.q[(b * NCH + ch) * 64..][..64].try_into().unwrap(),
                                nz8: sc.nz[b * NCH + ch],
                                gain: so.gain[ch],
                            },
                        )?;
                    }
                }
                write_split(sc, b, so);
                flags.add(so.split, ac_of(sc, b));
            }
        }
    }
    Ok(())
}

/// The cost of parent (`py`, `px`) of group `g` coded alone and, with
/// `small` (small transforms: `(force, cand)`, `force = Some(m)` forcing the
/// planes of `m`, `cand` the planes with a 4x4 candidate), its cheaper
/// coding per plane: luma first against the lane's previous luma
/// unit, each chroma plane against the luma option it follows; luma's
/// option is the cheaper including the chroma planes it makes cheaper or
/// dearer.
#[allow(clippy::too_many_arguments)]
fn solo_parent(
    sc: &StripeCoefs,
    enc: &Enc,
    planes: &[Vec<f32>; 2],
    coefs: &[Vec<f32>; 2],
    pw: usize,
    g: &Group2,
    (py, px): (usize, usize),
    w: &[f64; NCH],
    wt: &[[[f32; 2]; 2]; NCH],
    round: f32,
    coder: &Coder,
    small: Option<(Option<u8>, u8)>,
    opts: &Opts,
    flags: &FlagCost,
    prev_refs: &dyn Fn(&StripeCoefs, usize) -> Refs,
) -> Solo {
    let bw = sc.bw;
    let b = (g.r + py) * bw + g.bx + px;
    let aq = sc.aq.get(b).copied().unwrap_or(64) as f32 / 64.0;
    let r0 = prev_refs(sc, b);
    // Per plane, 8x8 and 4x4: the units, their nonzero masks, the
    // distortion term and the split choice's extra term (the distortion
    // again times kappa - 1 plus the inner-edge term, both weighted).
    let q8: [&[i32; 64]; NCH] =
        core::array::from_fn(|ch| sc.q[(b * NCH + ch) * 64..][..64].try_into().unwrap());
    let m8 = q8.map(nonzero);
    let (mut d8, mut x8) = ([0.0f64; NCH], [0.0f64; NCH]);
    let mut q4 = [[0i32; 64]; NCH];
    let (mut m4, mut d4, mut x4) = ([0u64; NCH], [0.0f64; NCH], [0.0f64; NCH]);
    let kx = (opts.kappa - 1.0) as f64;
    for ch in 0..NCH {
        let c: &[f32; 64] = coefs[py][(ch * bw + g.bx + px) * 64..][..64]
            .try_into()
            .unwrap();
        let st = &enc.steps8[ch];
        let (d, en) = incant!(
            terms_impl(c, q8[ch], (&enc.inv8[ch], st), aq),
            [v3, neon, wasm128, scalar]
        );
        let ring = RING * ring_unit(st[1] * aq, wt[ch][py][px], en);
        (d8[ch], x8[ch]) = (w[ch] * (d + ring), w[ch] * kx * (d + ring));
        if small.is_some_and(|(_, cand)| cand >> ch & 1 != 0) {
            let mut pix = [0.0f32; 64];
            for (y, row) in pix.as_chunks_mut::<8>().0.iter_mut().enumerate() {
                row.copy_from_slice(&planes[py][ch * 8 * pw + y * pw + (g.bx + px) * 8..][..8]);
            }
            let cf = (opts.forward)(&pix);
            let (d, en) = incant!(
                cand4_impl(
                    &cf,
                    (&enc.inv4[ch], &enc.steps4[ch]),
                    (aq, round),
                    &mut q4[ch]
                ),
                [v3, neon, wasm128, scalar]
            );
            // The DC is the parent's: pass 1's value (the DC image does not
            // depend on the split).
            q4[ch][0] = q8[ch][0];
            m4[ch] = nonzero(&q4[ch]);
            let ring = RING * ring_unit(st[1] * aq, wt[ch][py][px], en);
            (d4[ch], x4[ch]) = (w[ch] * (d + ring), w[ch] * kx * (d + ring));
            if opts.edge > 0.0 {
                let first = (st[1] * aq) as f64;
                let k = w[ch] * (opts.kappa * opts.edge) as f64 / (first * first);
                x8[ch] += k * inner_edge(&pix, &recon(q8[ch], &enc.dq8[ch], aq, false));
                x4[ch] += k * inner_edge(&pix, &recon(&q4[ch], &enc.dq4[ch], aq, true));
            }
        }
    }
    // Plane `ch`'s units coded 8x8 (`s` 0) or 4x4 (1), and their bits:
    // luma against the lane's previous luma unit, chroma against luma
    // option `ls` (memoized; every option prices the same units).
    let unit = |ch: usize, s: usize| -> (&[i32; 64], u64) {
        if s == 0 {
            (q8[ch], m8[ch])
        } else {
            (&q4[ch], m4[ch])
        }
    };
    let lref = [Refs::of_mask(m8[0]), Refs::of_mask(m4[0])];
    let mut lmemo = [None::<f64>; 2];
    let mut lbits = |s: usize| {
        *lmemo[s].get_or_insert_with(|| {
            let (q, m) = unit(0, s);
            unit_bits_m(coder, 0, q, m, r0)
        })
    };
    let mut cmemo = [[[None::<f64>; 2]; 2]; NCH];
    let mut cbits = |ch: usize, ls: usize, s: usize| {
        *cmemo[ch][ls][s].get_or_insert_with(|| {
            let (q, m) = unit(ch, s);
            unit_bits_m(coder, ch, q, m, lref[ls])
        })
    };
    let Some((force, _)) = small else {
        // 8x8 only.
        let mut j = 0.0;
        for ch in 0..NCH {
            j += d8[ch] + if ch == 0 { lbits(0) } else { cbits(ch, 0, 0) };
        }
        return Solo {
            j,
            split: 0,
            q4: [[0; 64]; NCH],
            gain: [0.0; NCH],
        };
    };
    // The choice minimizes J plus the extra terms; the J it reports (for
    // the merge decision) has none of them.
    // A plane carries a split flag only when its coding has AC (SPEC
    // 2.11.1); a 4x4 coding without AC is the 8x8 flat block, not a split.
    let ac4 = m4.map(|m| m != 0);
    let mut best = (f64::MAX, 0u8, 0.0f64);
    for lsplit in 0..2u8 {
        if force.is_some_and(|m| (m & 1) != lsplit) || (lsplit == 1 && !ac4[0]) {
            continue;
        }
        let ls = lsplit as usize;
        let (lc, lx, lm) = if ls == 0 {
            (d8[0], x8[0], m8[0])
        } else {
            (d4[0], x4[0], m4[0])
        };
        let fl = if lm != 0 { flags.bits(0, 0, ls) } else { 0.0 };
        let mut j = lc + lbits(ls) + fl;
        let mut x = lx;
        let mut sp = lsplit;
        for ch in 1..NCH {
            let mut bj = (f64::MAX, 0u8, 0.0f64);
            for csplit in 0..2u8 {
                if force.is_some_and(|m| (m >> ch & 1) != csplit) || (csplit == 1 && !ac4[ch]) {
                    continue;
                }
                let cs = csplit as usize;
                let (cc, cx, cm) = if cs == 0 {
                    (d8[ch], x8[ch], m8[ch])
                } else {
                    (d4[ch], x4[ch], m4[ch])
                };
                let fl = if cm != 0 { flags.bits(ch, ls, cs) } else { 0.0 };
                let jj = cc + cbits(ch, ls, cs) + fl;
                if jj + cx < bj.0 + bj.2 {
                    bj = (jj, csplit, cx);
                }
            }
            if bj.0 == f64::MAX {
                // Forced 4x4 without AC: the 8x8 coding.
                bj = (d8[ch] + cbits(ch, ls, 0), 0, 0.0);
            }
            j += bj.0;
            x += bj.2;
            sp |= bj.1 << ch;
        }
        if j + x < best.0 + best.2 {
            best = (j, sp, x);
        }
    }
    if best.0 == f64::MAX {
        // Forced luma 4x4 without AC: the 8x8 codings.
        let mut j = 0.0;
        for ch in 0..NCH {
            j += d8[ch] + if ch == 0 { lbits(0) } else { cbits(ch, 0, 0) };
        }
        best = (j, 0, 0.0);
    }
    // Gains of the chosen 4x4 planes (flags excluded): luma against the
    // lane's previous luma, chroma against the chosen luma unit.
    let mut out = [[0i32; 64]; NCH];
    let mut gain = [0.0f64; NCH];
    let ls = (best.1 & 1) as usize;
    for ch in 0..NCH {
        if best.1 >> ch & 1 != 0 {
            out[ch] = q4[ch];
            let (b8, b4) = if ch == 0 {
                (lbits(0), lbits(1))
            } else {
                (cbits(ch, ls, 0), cbits(ch, ls, 1))
            };
            gain[ch] = (d8[ch] + x8[ch] + b8) - (d4[ch] + x4[ch] + b4);
        }
    }
    Solo {
        j: best.0,
        split: best.1,
        q4: out,
        gain,
    }
}

/// The decoder's reconstruction of one plane from its units (`four`: 4x4
/// planes, else 8x8), AQ multiplier `aq` on the AC.
fn recon(q: &[i32; 64], dq: &[f32; 64], aq: f32, four: bool) -> [f32; 64] {
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
fn inner_edge(s: &[f32; 64], r: &[f32; 64]) -> f64 {
    let e = |i: usize| (r[i] - s[i]) as f64;
    let mut acc = 0.0;
    for k in 0..8 {
        let h = e(k * 8 + 4) - e(k * 8 + 3);
        let v = e(4 * 8 + k) - e(3 * 8 + k);
        acc += h * h + v * v;
    }
    acc
}

/// Writes a parent's chosen 4x4 planes into its units.
fn write_split(sc: &mut StripeCoefs, b: usize, so: &Solo) {
    sc.splits[b] = so.split;
    for ch in 0..NCH {
        if so.split >> ch & 1 == 0 {
            continue;
        }
        let unit = &mut sc.q[(b * NCH + ch) * 64..][..64];
        unit.copy_from_slice(&so.q4[ch]);
        sc.nz[b * NCH + ch] = ZIGZAG
            .iter()
            .enumerate()
            .fold(0, |m, (k, &i)| m | ((unit[i as usize] != 0) as u64) << k);
    }
}

/// Quantizes and prices the merged block of shape `s` at parent (`py0`,
/// `px0`) of group `g`.
#[allow(clippy::too_many_arguments)]
fn merged(
    sc: &StripeCoefs,
    enc: &Enc,
    sp: &[Spectra; NCH],
    g: &Group2,
    (s, py0, px0): (Shape, usize, usize),
    w: &[f64; NCH],
    wt: &[[[f32; 2]; 2]; NCH],
    round: f32,
    coder: &Coder,
    prev_refs: &dyn Fn(usize) -> Refs,
    out: &mut MCand,
    spec_err: &mut [[f32; 256]; NCH],
) {
    let bw = sc.bw;
    let b_of = |py: usize, px: usize| (g.r + py0 + py) * bw + g.bx + px0 + px;
    // AQ: the finest code among the parents.
    let aqc = (0..s.ph())
        .flat_map(|py| (0..s.pw()).map(move |px| (py, px)))
        .map(|(py, px)| sc.aq.get(b_of(py, px)).copied().unwrap_or(64))
        .min()
        .unwrap();
    let aq = aqc as f32 / 64.0;
    // Every unit slot but the DCs is written below; the DCs after it.
    out.shape = s;
    out.j = 0.0;
    out.aq = aqc;
    let mut qs = [0i32; 256];
    for ch in 0..NCH {
        let c = sp[ch].get(s, py0, px0);
        let si = s as usize;
        let tabs = (&enc.inv[ch][si], &enc.steps[ch][si], &enc.sc[si]);
        // The error goes through the kernel, so it is pre-scaled.
        let d = incant!(
            quant_impl(
                (s.h(), s.w() / 8),
                c,
                tabs,
                (aq, round),
                &mut qs,
                &mut spec_err[ch]
            ),
            [v3, neon, wasm128, scalar]
        );
        let gather = &enc.gather[si];
        for (p, unit) in out.units[ch].iter_mut().enumerate().take(s.ph() * s.pw()) {
            for (z, v) in unit.iter_mut().enumerate() {
                *v = qs[gather[p * 64 + z] as usize];
            }
        }
        out.j += w[ch] * d as f64;
        let dd = llf_to_dcs(s, [[c[0], c[1]], [c[16], c[17]]]);
        for py in 0..s.ph() {
            for px in 0..s.pw() {
                // Nearest, half away from zero, within the coefficient cap.
                let v = dd[py][px] / enc.steps8[ch][0];
                let r = crate::mathx::round_pos(v.abs() as f64).min(4_194_303.0) as i32;
                out.dcs[ch][py][px] = if v < 0.0 { -r } else { r };
            }
        }
    }
    // Ringing: the error spectra through the inverse transform, energy per
    // parent weighted by its flatness.
    for (ch, px_err) in spec_err.iter_mut().enumerate() {
        kernel::inverse_full(s, px_err);
        for py in 0..s.ph() {
            for px in 0..s.pw() {
                let mut acc = [0.0f32; 8];
                for y in 0..8 {
                    let row = &px_err[(py * 8 + y) * 16 + px * 8..][..8];
                    for (a, &v) in acc.iter_mut().zip(row) {
                        *a += v * v;
                    }
                }
                let e: f64 = acc.iter().map(|&a| a as f64).sum();
                let first = enc.steps8[ch][1] * aq;
                out.j += w[ch] * RING * ring_unit(first, wt[ch][py0 + py][px0 + px], e);
            }
        }
    }
    // Rate: every unit under the frozen tables; luma units against their
    // predecessors, chroma units against their parent's luma.
    for p in 0..s.ph() * s.pw() {
        let b = b_of(p / s.pw(), p % s.pw());
        out.j += unit_bits(coder, 0, &out.units[0][p], prev_refs(b));
        let lref = Refs::of_luma(&out.units[0][p]);
        for ch in 1..NCH {
            out.j += unit_bits(coder, ch, &out.units[ch][p], lref);
        }
    }
}

/// Writes a chosen merged block into its parents' units (AQ codes unified).
fn write_merged(sc: &mut StripeCoefs, s: Shape, r: usize, bx: usize, c: &MCand) {
    debug_assert!(c.shape == s);
    for py in 0..s.ph() {
        for px in 0..s.pw() {
            let b = (r + py) * sc.bw + bx + px;
            sc.modes[b] = s.role();
            if !sc.aq.is_empty() {
                sc.aq[b] = c.aq;
            }
            let p = py * s.pw() + px;
            for ch in 0..NCH {
                let unit = &mut sc.q[(b * NCH + ch) * 64..][..64];
                unit.copy_from_slice(&c.units[ch][p]);
                unit[0] = c.dcs[ch][py][px];
                sc.nz[b * NCH + ch] = ZIGZAG
                    .iter()
                    .enumerate()
                    .fold(0, |m, (k, &i)| m | ((unit[i as usize] != 0) as u64) << k);
            }
        }
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    fn xorshift(seed: &mut u32) -> u32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        *seed
    }

    /// The table-driven nonzero mask equals the zigzag gather.
    #[test]
    fn nonzero_mask_matches_the_gather() {
        let mut seed = 0x9b05_688cu32;
        for trial in 0..5000 {
            let density = 1 + trial % 9;
            let q: [i32; 64] = core::array::from_fn(|_| {
                let v = xorshift(&mut seed);
                if v % 9 < density as u32 {
                    (v >> 8) as i32 % 7 - 3
                } else {
                    0
                }
            });
            let mut want = 0u64;
            for (k, &z) in ZIGZAG.iter().enumerate().skip(1) {
                want |= ((q[z as usize] != 0) as u64) << k;
            }
            assert_eq!(nonzero(&q), want, "trial {trial}");
        }
    }

    /// The fused term and 4x4 candidate kernels equal the scalar
    /// references (`dtilde`, `energy`, `layout::quantize`) bit for bit, on
    /// the dispatched tier and the scalar instantiation.
    #[test]
    fn term_kernels_match_the_references() {
        let mut seed = 0x510e_527fu32;
        for trial in 0..2000 {
            let scale = [1.0f32, 30.0, 400.0][trial % 3];
            let c: [f32; 64] = core::array::from_fn(|_| {
                (xorshift(&mut seed) % 20001) as f32 / 10000.0 * scale - scale
            });
            let st: [f32; 64] = core::array::from_fn(|i| {
                0.5 + (xorshift(&mut seed) % 400) as f32 / 10.0 + i as f32
            });
            let inv = st.map(|v| 1.0 / v);
            let aq = (16 + xorshift(&mut seed) % 160) as f32 / 64.0;
            let round = [0.5f32, 0.38, 0.2][trial / 3 % 3];
            let mut want_q = layout::quantize(&c, &st, aq, round, 0.5);
            want_q[0] = 0;
            let want = (
                dtilde(&c, &want_q, &inv, 1.0 / aq),
                energy(&c, &want_q, &st, aq),
            );
            for tier in 0..2 {
                let mut q4 = [0i32; 64];
                let got = if tier == 0 {
                    incant!(
                        cand4_impl(&c, (&inv, &st), (aq, round), &mut q4),
                        [v3, neon, wasm128, scalar]
                    )
                } else {
                    cand4_impl_scalar(ScalarToken, &c, (&inv, &st), (aq, round), &mut q4)
                };
                q4[0] = 0;
                assert_eq!(q4, want_q, "trial {trial} tier {tier}: units");
                assert_eq!(
                    got.0.to_bits(),
                    want.0.to_bits(),
                    "trial {trial} tier {tier}: dtilde"
                );
                assert_eq!(
                    got.1.to_bits(),
                    want.1.to_bits(),
                    "trial {trial} tier {tier}: energy"
                );
                // The terms of given units (pass 1's 8x8 units).
                let got = if tier == 0 {
                    incant!(
                        terms_impl(&c, &want_q, (&inv, &st), aq),
                        [v3, neon, wasm128, scalar]
                    )
                } else {
                    terms_impl_scalar(ScalarToken, &c, &want_q, (&inv, &st), aq)
                };
                assert_eq!(
                    got.0.to_bits(),
                    want.0.to_bits(),
                    "trial {trial} tier {tier}: terms d"
                );
                assert_eq!(
                    got.1.to_bits(),
                    want.1.to_bits(),
                    "trial {trial} tier {tier}: terms e"
                );
            }
        }
    }
}
