//! 9-context V4 partition-map statistics, 4x4 split-flag statistics, and the
//! exact 8-state row-strip Viterbi dynamic program.

use alloc::vec::Vec;

use super::fallible::try_vec;
use crate::Error;
use crate::lossy::{NCH, StripeCoefs, superblock};
use crate::mathx::log2;

/// 9-context partition map symbol bit table: `bits[ctx][sym]`,
/// matching `superblock::map`'s context `3 * class(left) + class(up)`
/// (`class`: 0 => 0, 1 => 1, 2..=7 => 2) and its 1-bit `any` + 3-bit tree coder.
#[derive(Clone, Copy, Debug)]
pub(in crate::lossy) struct MapBitTable {
    pub bits: [[f64; 8]; 9],
}

#[inline]
pub(in crate::lossy) fn sym_class(sym: u8) -> usize {
    match sym {
        0 => 0,
        1 => 1,
        _ => 2,
    }
}

/// Associative exact integer histograms for the 9-context V4 partition map
/// (`any` bit + 3-level binary tree). Shared between parallel whole-image
/// folding and bounded streaming passes for 0-bit identical tables.
#[derive(Clone, Copy, Debug, Default)]
pub(in crate::lossy) struct MapBitStats {
    any: [[u64; 2]; 9],
    tree: [[[u64; 2]; 8]; 9],
}

impl MapBitStats {
    pub(in crate::lossy) fn add_stripe(
        &mut self,
        sc: &StripeCoefs,
        group_bw: usize,
    ) -> Result<(), Error> {
        if sc.modes.is_empty() {
            return Ok(());
        }
        let (bw, nby) = (sc.bw, sc.nby);
        let gw = if group_bw == 0 { bw } else { group_bw };
        let max_gcols = gw.min(bw).div_ceil(2);
        let mut prev_row = try_vec(max_gcols, 0u8)?;
        let mut cur_row = try_vec(max_gcols, 0u8)?;
        for gs in (0..bw).step_by(gw) {
            let ge = (gs + gw).min(bw);
            let cols = ge - gs;
            let gcols = cols.div_ceil(2);
            let grows = nby.div_ceil(2);
            prev_row[..gcols].fill(0);
            for gr in 0..grows {
                let r = gr * 2;
                let dy = r + 1 < nby;
                let mut left = 0u8;
                for gc in 0..gcols {
                    let x = gs + gc * 2;
                    let dx = x + 1 < ge;
                    if !dx && !dy {
                        cur_row[gc] = 0;
                        left = 0;
                        continue;
                    }
                    let roles = [
                        sc.modes[r * bw + x],
                        if dx { sc.modes[r * bw + x + 1] } else { 0 },
                        if dy { sc.modes[(r + 1) * bw + x] } else { 0 },
                        if dx && dy {
                            sc.modes[(r + 1) * bw + x + 1]
                        } else {
                            0
                        },
                    ];
                    let sym = superblock::symbol_of(roles).unwrap_or(0);
                    let up = if gr > 0 { prev_row[gc] } else { 0 };
                    let ctx = sym_class(left) * 3 + sym_class(up);
                    if sym == 0 {
                        self.any[ctx][0] += 1;
                    } else {
                        self.any[ctx][1] += 1;
                        let mut node = 1usize;
                        for shift in (0..3).rev() {
                            let bit = ((sym as usize - 1) >> shift) & 1;
                            self.tree[ctx][node][bit] += 1;
                            node = node * 2 + bit;
                        }
                    }
                    cur_row[gc] = sym;
                    left = sym;
                }
                core::mem::swap(&mut prev_row, &mut cur_row);
            }
        }
        Ok(())
    }

    pub(in crate::lossy) fn merge(&mut self, other: &Self) {
        for ctx in 0..9 {
            self.any[ctx][0] += other.any[ctx][0];
            self.any[ctx][1] += other.any[ctx][1];
            for node in 1..8 {
                self.tree[ctx][node][0] += other.tree[ctx][node][0];
                self.tree[ctx][node][1] += other.tree[ctx][node][1];
            }
        }
    }

    pub(in crate::lossy) fn into_table(self) -> MapBitTable {
        let prior = MapBitTable::prior();
        // Seed with weak Laplace priors anchored on `prior` so unobserved
        // contexts stay well-behaved; add exact integer counts once per bin.
        let mut any = [[0.0f64; 2]; 9];
        let mut tree = [[[0.0f64; 2]; 8]; 9];
        for ctx in 0..9 {
            let p0 = crate::lossy::transfer::pow(2.0, -prior.bits[ctx][0]);
            any[ctx][0] = 2.0 * p0 + self.any[ctx][0] as f64;
            any[ctx][1] = 2.0 * (1.0 - p0) + self.any[ctx][1] as f64;
            let mut ps = [0.0f64; 8];
            for s in 1..8 {
                ps[s] = crate::lossy::transfer::pow(2.0, -prior.bits[ctx][s]);
            }
            for s in 1..8usize {
                let w = 2.0 * ps[s];
                let mut node = 1usize;
                for shift in (0..3).rev() {
                    let bit = ((s - 1) >> shift) & 1;
                    tree[ctx][node][bit] += w;
                    node = node * 2 + bit;
                }
            }
            for node in 1..8 {
                tree[ctx][node][0] += self.tree[ctx][node][0] as f64;
                tree[ctx][node][1] += self.tree[ctx][node][1] as f64;
            }
        }

        let mut bits = [[0.0; 8]; 9];
        for ctx in 0..9 {
            let tot = any[ctx][0] + any[ctx][1];
            let b0 = -log2((any[ctx][0] / tot).clamp(1e-6, 1.0 - 1e-6));
            let bm = -log2((any[ctx][1] / tot).clamp(1e-6, 1.0 - 1e-6));
            bits[ctx][0] = b0;
            for s in 1..8usize {
                let mut cost = bm;
                let mut node = 1usize;
                for shift in (0..3).rev() {
                    let bit = ((s - 1) >> shift) & 1;
                    let t = tree[ctx][node][0] + tree[ctx][node][1];
                    let p = if t > 0.0 {
                        (tree[ctx][node][bit] / t).clamp(1e-6, 1.0 - 1e-6)
                    } else {
                        0.5
                    };
                    cost += -log2(p);
                    node = node * 2 + bit;
                }
                bits[ctx][s] = cost;
            }
        }
        MapBitTable { bits }
    }
}

impl MapBitTable {
    /// Flat fallback matching the historical selector constants (`0.25` / `3.0`).
    pub(in crate::lossy) fn flat() -> Self {
        let mut bits = [[3.0; 8]; 9];
        for row in &mut bits {
            row[0] = 0.25;
        }
        Self { bits }
    }

    /// Calibrated 9-context prior matching the adaptive range coder's `any` bit
    /// and 3-level binary tree before image-specific map statistics exist.
    pub(in crate::lossy) fn prior() -> Self {
        // Prior probability of merging and of symbol 1 (16x16) given merge,
        // indexed by ctx = 3 * class(left) + class(up).
        const P_MERGE: [f64; 9] = [0.14, 0.52, 0.42, 0.52, 0.78, 0.65, 0.42, 0.65, 0.62];
        const P_S1_GIVEN_M: [f64; 9] = [0.40, 0.70, 0.30, 0.70, 0.84, 0.55, 0.30, 0.55, 0.28];
        let mut bits = [[0.0; 8]; 9];
        for ctx in 0..9 {
            let pm = P_MERGE[ctx];
            let ps1 = P_S1_GIVEN_M[ctx];
            let p_other = (1.0 - ps1) / 6.0;
            bits[ctx][0] = -log2(1.0 - pm);
            bits[ctx][1] = -log2(pm * ps1);
            for s in 2..8 {
                bits[ctx][s] = -log2(pm * p_other);
            }
        }
        Self { bits }
    }

    /// Empirical 9-context symbol bit table learned from a pilot pass's
    /// partition roles across `threads` workers (thread-count invariant).
    #[allow(dead_code)]
    pub(in crate::lossy) fn from_stripes(
        stripes: &[StripeCoefs],
        group_bw: usize,
        threads: usize,
    ) -> Result<Self, Error> {
        let parts = crate::par::par_fold(
            stripes,
            threads,
            || Ok::<MapBitStats, Error>(MapBitStats::default()),
            |acc, sc| {
                if let Ok(stats) = acc
                    && let Err(e) = stats.add_stripe(sc, group_bw)
                {
                    *acc = Err(e);
                }
            },
        );
        let mut total = MapBitStats::default();
        for p in parts {
            total.merge(&p?);
        }
        Ok(total.into_table())
    }
}

/// Associative exact integer histograms for empirical 4x4 split flags.
#[derive(Clone, Copy, Debug, Default)]
pub(in crate::lossy) struct FlagStats {
    n: [[[u64; 2]; 2]; NCH],
}

impl FlagStats {
    pub(in crate::lossy) fn add_stripe(&mut self, sc: &StripeCoefs) {
        if sc.splits.is_empty() {
            return;
        }
        for b in 0..sc.nby * sc.bw {
            if sc.modes.get(b).is_some_and(|&m| m != 0) || sc.aq.get(b) == Some(&0) {
                continue;
            }
            let split = sc.splits[b];
            let luma = (split & 1) as usize;
            for ch in 0..NCH {
                if sc.nz[b * NCH + ch] & !1 == 0 {
                    continue;
                }
                let l = if ch == 0 { 0 } else { luma };
                self.n[ch][l][(split >> ch & 1) as usize] += 1;
            }
        }
    }

    pub(in crate::lossy) fn merge(&mut self, other: &Self) {
        for ch in 0..NCH {
            for l in 0..2 {
                self.n[ch][l][0] += other.n[ch][l][0];
                self.n[ch][l][1] += other.n[ch][l][1];
            }
        }
    }

    pub(in crate::lossy) fn into_flags(self) -> LearnedFlags {
        let mut n = [[[0.0; 2]; 2]; NCH];
        for ch in 0..NCH {
            for l in 0..2 {
                n[ch][l][0] = 1.0 + self.n[ch][l][0] as f64;
                n[ch][l][1] = 0.25 + self.n[ch][l][1] as f64;
            }
        }
        LearnedFlags { n }
    }
}

/// Empirical 4x4 split-flag counts learned from a pilot pass's `splits` and `nz`.
#[derive(Clone, Copy, Debug)]
pub(in crate::lossy) struct LearnedFlags {
    pub n: [[[f64; 2]; 2]; NCH],
}

impl LearnedFlags {
    #[allow(dead_code)]
    pub(in crate::lossy) fn from_stripes(stripes: &[StripeCoefs], threads: usize) -> Self {
        let parts = crate::par::par_fold(stripes, threads, FlagStats::default, |acc, sc| {
            acc.add_stripe(sc);
        });
        let mut total = FlagStats::default();
        for p in parts {
            total.merge(&p);
        }
        total.into_flags()
    }
}

/// Exact 8-state Viterbi DP across a row strip of `G` aligned 2x2 groups.
///
/// Minimizes:
/// `sum_{gc=0..G} (unary[gc][s_gc] + map_bits[ctx(gc)][s_gc])`
/// `+ sum_{gc=1..G} horiz_cost(gc, s_{gc-1}, s_gc)`
/// where `ctx(gc) = 3 * sym_class(left) + up_class[gc]`, with `left = 0` at `gc == 0`.
///
/// Ties strictly prefer the lower predecessor state, and at the final column
/// strictly prefer the lower symbol index (deterministic across all targets).
pub(in crate::lossy) fn solve_row_strip_dp<F>(
    unary: &[[f64; 8]],
    up_class: &[u8],
    map_bits: &MapBitTable,
    horiz_cost: F,
) -> Result<(f64, Vec<u8>), Error>
where
    F: Fn(usize, usize, usize) -> f64,
{
    let g = unary.len();
    debug_assert_eq!(up_class.len(), g);
    if g == 0 {
        return Ok((0.0, Vec::new()));
    }
    let mut prev = [f64::INFINITY; 8];
    let mut back = try_vec(g, [0u8; 8])?;
    let ctx0 = up_class[0] as usize;
    for s in 0..8 {
        if unary[0][s].is_finite() {
            prev[s] = unary[0][s] + map_bits.bits[ctx0][s];
        }
    }
    for gc in 1..g {
        let mut cur = [f64::INFINITY; 8];
        let upc = up_class[gc] as usize;
        for s in 0..8 {
            let u = unary[gc][s];
            if !u.is_finite() {
                continue;
            }
            let mut best = f64::INFINITY;
            let mut best_p = 0u8;
            for sp in 0..8 {
                let p = prev[sp];
                if !p.is_finite() {
                    continue;
                }
                let ctx = sym_class(sp as u8) * 3 + upc;
                let cost = p + map_bits.bits[ctx][s] + horiz_cost(gc, sp, s) + u;
                if cost < best {
                    best = cost;
                    best_p = sp as u8;
                }
            }
            cur[s] = best;
            back[gc][s] = best_p;
        }
        prev = cur;
    }
    let mut best_s = 0usize;
    let mut best_cost = f64::INFINITY;
    for (s, &c) in prev.iter().enumerate() {
        if c < best_cost {
            best_cost = c;
            best_s = s;
        }
    }
    let mut path = try_vec(g, 0u8)?;
    let mut cur_s = best_s as u8;
    for gc in (0..g).rev() {
        path[gc] = cur_s;
        cur_s = back[gc][cur_s as usize];
    }
    Ok((best_cost, path))
}
