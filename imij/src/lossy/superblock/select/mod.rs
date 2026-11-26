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

mod classic;
mod coder;
mod rdo;
#[cfg(all(test, feature = "encode"))]
mod tests;
mod transform;

pub(in crate::lossy) use coder::Coder;
pub(in crate::lossy) use transform::{Enc, Spectra, new_spectra};

use super::*;
use crate::lossy::trellis::{LossyTrellis, lane_neighbors, support};
use coder::{Refs, ring_unit, unit_bits};
use transform::MCand;

/// Flatness reference of the ringing weight, in squared first AC steps of
/// mean AC energy: a parent whose mean squared AC coefficient equals it has
/// weight 1/2.
pub(super) const FLAT_A0: f32 = 1.0;
/// Weight of the ringing term.
pub(super) const RING: f64 = 1.0;
/// Nested DC image growth per merged parent (bits, all planes): the
/// 2026-10-08 study measured 88-137 kbit over 28 images with about 230k
/// parents merged.
pub(super) const DC_KAPPA: f64 = 0.5;
/// Partition symbol costs before any map statistics exist (bits).
pub(super) const SYM_NONE_BITS: f64 = 0.25;
pub(super) const SYM_MERGE_BITS: f64 = 3.0;

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
    pub(super) coder: Coder,
    pub(super) enc: Enc,
    pub(super) lam: LossyTrellis,
    pub(super) pw: usize,
    pub(super) group_bw: usize,
    pub(super) round: f32,
    pub(super) opts: Opts,
    pub(super) learned_map: Option<crate::lossy::adaptive_rdo::MapBitTable>,
    pub(super) learned_flags: Option<crate::lossy::adaptive_rdo::LearnedFlags>,
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
        sc: StripeCoefs,
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
        self.stripe_classic(sc, rows, stop)
    }
}

/// Reusable 2-block-row planes, extremes, and 8x8 FDCT coefficient buffers
/// shared between classic and adaptive RDO stripe selection.
pub(super) struct RowBuffers {
    pub(super) planes: [Vec<f32>; 2],
    pub(super) ext: [Vec<(u64, bool)>; 2],
    pub(super) coefs: [Vec<f32>; 2],
}

impl RowBuffers {
    pub(super) fn new(pw: usize, bw: usize) -> Result<Self, crate::Error> {
        use crate::lossy::adaptive_rdo::{try_vec, try_vec_cap};
        let plane_len = NCH
            .checked_mul(8)
            .and_then(|v| v.checked_mul(pw))
            .ok_or(crate::Error::TooLarge)?;
        let coef_len = NCH
            .checked_mul(bw)
            .and_then(|v| v.checked_mul(64))
            .ok_or(crate::Error::TooLarge)?;
        Ok(Self {
            planes: [try_vec(plane_len, 0.0f32)?, try_vec(plane_len, 0.0f32)?],
            ext: [try_vec_cap(bw)?, try_vec_cap(bw)?],
            coefs: [try_vec(coef_len, 0.0f32)?, try_vec(coef_len, 0.0f32)?],
        })
    }

    pub(super) fn load_pair(
        &mut self,
        rows: RowPlanes<'_>,
        by0: usize,
        rr: usize,
        pw: usize,
        bw: usize,
        cm: &[[f32; 8]; 8],
    ) {
        for k in 0..rr {
            self.ext[k].clear();
            rows(by0 + k, &mut self.planes[k], &mut self.ext[k]);
            for ch in 0..NCH {
                enc_simd::fdct_row(
                    &self.planes[k][ch * 8 * pw..][..8 * pw],
                    pw,
                    bw,
                    cm,
                    &mut self.coefs[k][ch * bw * 64..][..bw * 64],
                );
            }
        }
    }
}

/// Context references of luma unit `b`: the lane's previous luma EOB
/// (run-level) and the previous block's luma group count (split-Huffman).
fn prev_refs(sc: &StripeCoefs, b: usize) -> Refs {
    Refs {
        eob: lane_neighbors(b, sc.bw, sc.nby)
            .0
            .map_or(0, |p| support(sc.nz[p * NCH] & !1)),
        count: if b > 0 {
            support(sc.nz[(b - 1) * NCH] & !1).div_ceil(4)
        } else {
            0
        },
    }
}

/// Bitmask of channels in block `b` that have at least one nonzero AC value.
pub(super) fn ac_of(sc: &StripeCoefs, b: usize) -> u8 {
    (0..NCH).fold(0u8, |m, ch| {
        let u = &sc.q[(b * NCH + ch) * 64..][..64];
        m | (u[1..].iter().any(|&v| v != 0) as u8) << ch
    })
}

/// Adds pixel-domain ringing distortion (inverse superblock transform of the
/// error spectrum weighted by parent flatness) and coded AC bit cost across
/// all constituent parent units of merged candidate `out`.
#[allow(clippy::too_many_arguments)]
fn score_merged_ringing_and_rate(
    enc: &Enc,
    s: Shape,
    (py0, px0): (usize, usize),
    aq: f32,
    w: &[f64; NCH],
    wt: &[[[f32; 2]; 2]; NCH],
    coder: &Coder,
    b_of: &dyn Fn(usize, usize) -> usize,
    prev_refs_at: &dyn Fn(usize) -> Refs,
    out: &mut MCand,
    spec_err: &mut [[f32; 256]; NCH],
) {
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
    for p in 0..s.ph() * s.pw() {
        let b = b_of(p / s.pw(), p % s.pw());
        out.j += unit_bits(coder, 0, &out.units[0][p], prev_refs_at(b));
        let lref = Refs::of_luma(&out.units[0][p]);
        for ch in 1..NCH {
            out.j += unit_bits(coder, ch, &out.units[ch][p], lref);
        }
    }
}

/// Estimated bits of the split flags (small transforms): adaptive counts
/// per (plane, the parent's luma flag) over a stripe, the coder's contexts
/// without the neighbour terms.
pub(super) struct FlagCost {
    pub(super) n: [[[f64; 2]; 2]; NCH],
}

impl FlagCost {
    pub(super) fn new() -> Self {
        // A split starts at about 2.3 bits, no split at 0.3.
        Self {
            n: [[[1.0, 0.25]; 2]; NCH],
        }
    }

    pub(super) fn bits(&self, ch: usize, luma: usize, bit: usize) -> f64 {
        let n = self.n[ch][luma];
        ((n[0] + n[1]) / n[bit]).log2()
    }

    /// Counts a parent's flags: `split` its 4x4 planes, `ac` the planes
    /// whose coding has AC (the planes with a flag).
    pub(super) fn add(&mut self, split: u8, ac: u8) {
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
pub(super) struct Group2 {
    pub(super) r: usize,
    pub(super) rows: usize,
    pub(super) bx: usize,
    pub(super) cols: usize,
}

/// A parent coded alone: its cost J and, with small transforms, its 4x4
/// plane bits and those planes' units.
#[derive(Clone, Copy)]
pub(super) struct Solo {
    pub(super) j: f64,
    pub(super) split: u8,
    pub(super) q4: [[i32; 64]; NCH],
    /// Per 4x4 plane: J of its 8x8 coding minus J of its 4x4 coding, flags
    /// excluded (the unit pruning's gain).
    pub(super) gain: [f64; NCH],
}

/// A 4x4 plane written by the selector, for the unit pruning: its parent,
/// plane, the 8x8 unit and nonzero mask it replaced, and its gain.
pub(super) struct Written {
    pub(super) b: usize,
    pub(super) ch: usize,
    pub(super) q8: [i32; 64],
    pub(super) nz8: u64,
    pub(super) gain: f64,
}

/// Reverts the 4x4 planes of every unit of stripe `sc` (column groups of
/// `group_bw` blocks; 0 = one) whose summed gain does not pay for the
/// unit's split-flag section: the planes with AC among its unmerged,
/// unbypassed parents, each `N H(K / N)` bits for K splits of N flags, plus
/// half a bit per log2 N of learning per plane that splits and one header
/// bit.
pub(super) fn prune_units(sc: &mut StripeCoefs, group_bw: usize, written: &[Written]) {
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

/// Writes a parent's chosen 4x4 planes into its units.
pub(super) fn write_split(sc: &mut StripeCoefs, b: usize, so: &Solo) {
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
