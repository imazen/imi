//! Frozen entropy coder pricing (`Coder`, `Refs`, `unit_bits`, `ring_unit`)
//! shared by classic and adaptive RDO partition selection.

use crate::lossy::superblock::*;
use crate::lossy::trellis::{HufPrices, Prices};

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
        let parts = crate::par::par_fold(
            stripes,
            cfg.threads,
            || TokenHistogram::new(split),
            |hist, sc| hist.add_stripe(sc, group_bw),
        );
        let mut total = TokenHistogram::new(split);
        for part in parts {
            total.merge(part);
        }
        total.selector_coder(cfg, effort, split)
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
pub(super) fn eob_of_mask(m: u64) -> u32 {
    if m == 0 { 0 } else { 63 - m.leading_zeros() }
}

/// The zigzag positions 1..=63 of `q` that are nonzero, bit k for position
/// k: each raster row's nonzero byte through [`ZZ_BITS`].
#[inline]
pub(super) fn nonzero(q: &[i32; 64]) -> u64 {
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
pub(super) struct Refs {
    pub(super) eob: u32,
    pub(super) count: u32,
}

impl Refs {
    /// Chroma references of a block whose luma unit is `luma`.
    pub(super) fn of_luma(luma: &[i32; 64]) -> Self {
        Self::of_mask(nonzero(luma))
    }

    /// [`Self::of_luma`] from the luma unit's [`nonzero`] mask.
    pub(super) fn of_mask(m: u64) -> Self {
        let eob = eob_of_mask(m);
        Self {
            eob,
            count: eob.div_ceil(4),
        }
    }
}

/// Bits of one unit of plane `ch` under the frozen coder (its DC slot is
/// not coded here).
pub(super) fn unit_bits(coder: &Coder, ch: usize, q: &[i32; 64], r: Refs) -> f64 {
    unit_bits_m(coder, ch, q, nonzero(q), r)
}

/// [`unit_bits`] given the unit's [`nonzero`] mask `m`.
pub(super) fn unit_bits_m(coder: &Coder, ch: usize, q: &[i32; 64], m: u64, r: Refs) -> f64 {
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
pub(super) fn dtilde(c: &[f32], q: &[i32; 64], inv: &[f32; 64], inv_aq: f32) -> f64 {
    lanes8(|i| {
        let e = c[i].abs() * (inv[i] * inv_aq) - q[i].unsigned_abs() as f32;
        e * e
    })
}

/// Absolute squared error of the AC coefficients of a unit; the reference
/// of [`terms_impl`].
#[cfg(all(test, feature = "encode"))]
pub(super) fn energy(c: &[f32], q: &[i32; 64], steps: &[f32; 64], aq: f32) -> f64 {
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
pub(super) fn ring_unit(first_step: f32, wt: f32, energy: f64) -> f64 {
    let d = first_step as f64;
    wt as f64 * energy / (d * d)
}
