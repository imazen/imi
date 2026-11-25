//! Surrounding-block adaptive RDO, exact 8-state row-strip partition-map DP,
//! empirical pilot-pass table learning, and zero-decode-overhead perceptual
//! trellis-lambda AQ (open-loop + closed-loop unfiltered-seam residual
//! feedback) for the lossy encoder.
//!
//! All mechanisms operate strictly within the existing V4 bitstream format:
//! zero wire changes, zero decoder changes, fallible allocations returning
//! `Error::TooLarge`, deterministic scalar f64/f32 arithmetic across CPU
//! targets, and bit-identical output across encoder thread counts.

use alloc::vec::Vec;

use super::superblock;
use super::{NCH, RowPlanes, StripeCoefs};
use crate::Error;
use crate::mathx::log2;

/// Fallible zero/value-initialized allocation returning [`Error::TooLarge`] on
/// reservation failure instead of aborting the process.
pub(super) fn try_vec<T: Clone>(len: usize, val: T) -> Result<Vec<T>, Error> {
    let mut v = Vec::new();
    v.try_reserve_exact(len).map_err(|_| Error::TooLarge)?;
    v.resize(len, val);
    Ok(v)
}

/// Fallible empty `Vec` with exact reserved capacity.
pub(super) fn try_vec_cap<T>(cap: usize) -> Result<Vec<T>, Error> {
    let mut v = Vec::new();
    v.try_reserve_exact(cap).map_err(|_| Error::TooLarge)?;
    Ok(v)
}

/// Fallible `Vec::push` returning [`Error::TooLarge`] on allocation failure.
pub(super) fn try_push<T>(v: &mut Vec<T>, val: T) -> Result<(), Error> {
    if v.len() == v.capacity() {
        v.try_reserve(1).map_err(|_| Error::TooLarge)?;
    }
    v.push(val);
    Ok(())
}

/// 9-context partition map symbol bit table: `bits[ctx][sym]`,
/// matching `superblock::map`'s context `3 * class(left) + class(up)`
/// (`class`: 0 => 0, 1 => 1, 2..=7 => 2) and its 1-bit `any` + 3-bit tree coder.
#[derive(Clone, Copy, Debug)]
pub(super) struct MapBitTable {
    pub bits: [[f64; 8]; 9],
}

#[inline]
pub(super) fn sym_class(sym: u8) -> usize {
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
pub(super) struct MapBitStats {
    any: [[u64; 2]; 9],
    tree: [[[u64; 2]; 8]; 9],
}

impl MapBitStats {
    pub(super) fn add_stripe(&mut self, sc: &StripeCoefs, group_bw: usize) -> Result<(), Error> {
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

    pub(super) fn merge(&mut self, other: &Self) {
        for ctx in 0..9 {
            self.any[ctx][0] += other.any[ctx][0];
            self.any[ctx][1] += other.any[ctx][1];
            for node in 1..8 {
                self.tree[ctx][node][0] += other.tree[ctx][node][0];
                self.tree[ctx][node][1] += other.tree[ctx][node][1];
            }
        }
    }

    pub(super) fn into_table(self) -> MapBitTable {
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
    pub(super) fn flat() -> Self {
        let mut bits = [[3.0; 8]; 9];
        for row in &mut bits {
            row[0] = 0.25;
        }
        Self { bits }
    }

    /// Calibrated 9-context prior matching the adaptive range coder's `any` bit
    /// and 3-level binary tree before image-specific map statistics exist.
    pub(super) fn prior() -> Self {
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
    pub(super) fn from_stripes(
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
pub(super) struct FlagStats {
    n: [[[u64; 2]; 2]; NCH],
}

impl FlagStats {
    pub(super) fn add_stripe(&mut self, sc: &StripeCoefs) {
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

    pub(super) fn merge(&mut self, other: &Self) {
        for ch in 0..NCH {
            for l in 0..2 {
                self.n[ch][l][0] += other.n[ch][l][0];
                self.n[ch][l][1] += other.n[ch][l][1];
            }
        }
    }

    pub(super) fn into_flags(self) -> LearnedFlags {
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
pub(super) struct LearnedFlags {
    pub n: [[[f64; 2]; 2]; NCH],
}

impl LearnedFlags {
    #[allow(dead_code)]
    pub(super) fn from_stripes(stripes: &[StripeCoefs], threads: usize) -> Self {
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
pub(super) fn solve_row_strip_dp<F>(
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

/// Computes per-pixel visibility weights `vis[y][x]` in `(0, 1]` for an
/// `8 x 8` luma patch using a 3x3-eroded local gradient estimate normalized by
/// `mask_tau * step1`.
///
/// Flat pixels (including flat pixels adjacent to a sharp edge) receive weight
/// near `1.0` so ringing there is strongly weighted; dense textured pixels
/// receive small weight.
fn block_visibility(orig: &[[f32; 8]; 8], step1: f32, mask_tau: f32) -> [[f32; 8]; 8] {
    let mut grad = [[0.0f32; 8]; 8];
    for y in 0..8usize {
        let ym = y.saturating_sub(1);
        let yp = (y + 1).min(7);
        for x in 0..8usize {
            let xm = x.saturating_sub(1);
            let xp = (x + 1).min(7);
            let v = orig[y][x];
            grad[y][x] = (v - orig[y][xm])
                .abs()
                .max((v - orig[y][xp]).abs())
                .max((v - orig[ym][x]).abs())
                .max((v - orig[yp][x]).abs());
        }
    }
    let inv_tau = 1.0 / (mask_tau * step1).max(1e-4);
    let mut vis = [[0.0f32; 8]; 8];
    for y in 0..8usize {
        let ym = y.saturating_sub(1);
        let yp = (y + 1).min(7);
        for x in 0..8usize {
            let xm = x.saturating_sub(1);
            let xp = (x + 1).min(7);
            let mut mn = grad[y][x];
            for yy in ym..=yp {
                for xx in xm..=xp {
                    if grad[yy][xx] < mn {
                        mn = grad[yy][xx];
                    }
                }
            }
            let act = 0.5 * (grad[y][x] + mn);
            let r = act * inv_tau;
            vis[y][x] = 1.0 / (1.0 + r * r);
        }
    }
    vis
}

/// Smoothness-gated seam error across one pixel boundary pair `(a_orig, b_orig)`
/// with reconstruction errors `(e_a, e_b)`, in squared `step1` units.
#[inline]
fn seam_pair_cost(
    a_orig: f32,
    b_orig: f32,
    e_a: f32,
    e_b: f32,
    inv_seam_tau_s1: f64,
    inv_s1_sq: f64,
) -> f64 {
    let d_src = (b_orig - a_orig) as f64;
    let r = d_src * inv_seam_tau_s1;
    let gate = 1.0 / (1.0 + r * r);
    let de = (e_b - e_a) as f64;
    gate * (de * de) * inv_s1_sq
}

/// Coarse-step damping of open-loop trellis-lambda AQ strength (`q5..q25`).
#[inline]
fn open_loop_effective_strength(steps: &[[f32; 64]; NCH], strength: f32) -> f64 {
    let s1_y = steps[0][1].max(1e-4);
    let coarse_damp = if s1_y > 16.0 {
        (1.0 - 0.55 * ((s1_y - 16.0) / 48.0).clamp(0.0, 1.0)) as f64
    } else {
        1.0
    };
    strength as f64 * coarse_damp
}

/// Computes per-block `(min_q, mean_q, sat_smooth)` features for one 8-pixel
/// block row from color-converted `planes` (`NCH * 8 * pw`, plane-major).
pub(super) fn open_loop_row_features(
    planes: &[f32],
    pw: usize,
    bw: usize,
    steps: &[[f32; 64]; NCH],
    min_q: &mut [f32],
    mean_q: &mut [f32],
    sat_smooth: &mut [f32],
) {
    let s1_y = steps[0][1].max(1e-4);
    let s1_x = steps[1][1].max(1e-4);
    let s1_b = steps[2][1].max(1e-4);
    for bx in 0..bw {
        let base_y = bx * 8;
        let mut q_act = [0.0f32; 4];
        for qy in 0..2 {
            for qx in 0..2 {
                let mut sum = 0.0f32;
                for dy in 0..4 {
                    let y = qy * 4 + dy;
                    let y1 = (y + 1).min(7);
                    for dx in 0..4 {
                        let x = qx * 4 + dx;
                        let x1 = (x + 1).min(7);
                        let v = planes[y * pw + base_y + x];
                        let vx = planes[y * pw + base_y + x1];
                        let vy = planes[y1 * pw + base_y + x];
                        sum += (v - vx).abs() + (v - vy).abs();
                    }
                }
                q_act[qy * 2 + qx] = sum * (1.0 / 32.0) / s1_y;
            }
        }
        let mn = q_act[0].min(q_act[1]).min(q_act[2]).min(q_act[3]);
        let av = 0.25 * (q_act[0] + q_act[1] + q_act[2] + q_act[3]);
        min_q[bx] = mn;
        mean_q[bx] = av;

        // Chroma saturation & smoothness: protect smooth saturated regions
        // against chroma shifts.
        let base_x = 8 * pw + bx * 8;
        let base_b = 16 * pw + bx * 8;
        let (mut mag_c, mut grad_c) = (0.0f32, 0.0f32);
        for y in 0..8 {
            let y1 = (y + 1).min(7);
            for x in 0..8 {
                let x1 = (x + 1).min(7);
                let cx = planes[base_x + y * pw + x];
                let cb = planes[base_b + y * pw + x];
                mag_c += (cx / s1_x).abs() + (cb / s1_b).abs();
                grad_c += ((cx - planes[base_x + y * pw + x1]).abs()
                    + (cx - planes[base_x + y1 * pw + x]).abs())
                    / s1_x
                    + ((cb - planes[base_b + y * pw + x1]).abs()
                        + (cb - planes[base_b + y1 * pw + x]).abs())
                        / s1_b;
            }
        }
        let avg_mag = mag_c * (1.0 / 128.0);
        let avg_grad = grad_c * (1.0 / 256.0);
        let smooth = 1.0 / (1.0 + avg_grad * avg_grad);
        sat_smooth[bx] = (avg_mag / (avg_mag + 8.0)) * smooth;
    }
}

/// Computes raw open-loop perceptual scores for one block row using the 3-row
/// `mean_q` neighborhood `(above, cur_mean, below)`. Returns
/// `(row_active_sum, row_active_n, row_all_sum)`.
pub(super) fn open_loop_row_raw(
    above: Option<&[f32]>,
    cur_mean: &[f32],
    below: Option<&[f32]>,
    cur_min: &[f32],
    cur_sat: &[f32],
    out_raw: &mut [f64],
) -> (f64, usize, f64) {
    let bw = cur_mean.len();
    let mut row_active_sum = 0.0f64;
    let mut row_active_n = 0usize;
    let mut row_all_sum = 0.0f64;
    for bx in 0..bw {
        let x0 = bx.saturating_sub(1);
        let x1 = (bx + 1).min(bw - 1);
        let mut s_min = f32::INFINITY;
        let mut s_sum = 0.0f32;
        let mut s_cnt = 0.0f32;
        for row_opt in [above, Some(cur_mean), below] {
            let Some(row) = row_opt else { continue };
            for &v in &row[x0..=x1] {
                if v < s_min {
                    s_min = v;
                }
                s_sum += v;
                s_cnt += 1.0;
            }
        }
        let s_avg = s_sum / s_cnt;
        let act = 0.45 * cur_min[bx] + 0.30 * s_min + 0.25 * s_avg;
        let score = -crate::lossy::transfer::ln((act as f64) + 0.25) + 0.35 * (cur_sat[bx] as f64);
        out_raw[bx] = score;
        row_all_sum += score;
        if cur_mean[bx] > 0.05 {
            row_active_sum += score;
            row_active_n += 1;
        }
    }
    (row_active_sum, row_active_n, row_all_sum)
}

/// Open-loop zero-decode-overhead surrounding-block perceptual trellis-lambda
/// multiplier map (`bw * bh` f32s, centered over active blocks around 1.0).
///
/// Uses fixed 16-block-row bands parallelized across `threads` with strict
/// ascending row-order scalar reduction of `mean_raw`, guaranteeing bit-exact
/// invariance across thread counts and 0-bit parity with [`OpenLoopLambdaStream`].
#[allow(clippy::too_many_arguments)]
pub(super) fn build_open_loop_lambda_map(
    rows: RowPlanes,
    pw: usize,
    bw: usize,
    bh: usize,
    steps: &[[f32; 64]; NCH],
    strength: f32,
    threads: usize,
    stop: &dyn how_far::Stop,
) -> Result<Vec<f32>, Error> {
    stop.check().map_err(|_| Error::Cancelled)?;
    let nb = bw.checked_mul(bh).ok_or(Error::TooLarge)?;
    if nb == 0 || strength == 0.0 {
        return try_vec(nb, 1.0f32);
    }
    const BAND_ROWS: usize = 16;
    let n_bands = bh.div_ceil(BAND_ROWS);
    let plane_len = NCH
        .checked_mul(8)
        .and_then(|v| v.checked_mul(pw))
        .ok_or(Error::TooLarge)?;

    // Pass 1: per-row local 4x4-quadrant activity & chroma saturation/smoothness.
    type FeatBand = (Vec<f32>, Vec<f32>, Vec<f32>);
    let mut band_indices = try_vec_cap(n_bands)?;
    band_indices.extend(0..n_bands);
    let feat_bands: Vec<Result<FeatBand, Error>> =
        crate::par::par_map(band_indices, threads, |_, bi| {
            let r0 = bi * BAND_ROWS;
            let r1 = (r0 + BAND_ROWS).min(bh);
            let nr = r1 - r0;
            let n = nr.checked_mul(bw).ok_or(Error::TooLarge)?;
            let mut min_b = try_vec(n, 0.0f32)?;
            let mut mean_b = try_vec(n, 0.0f32)?;
            let mut sat_b = try_vec(n, 0.0f32)?;
            let mut planes = try_vec(plane_len, 0.0f32)?;
            let mut extremes = try_vec_cap(bw)?;
            for r in 0..nr {
                stop.check().map_err(|_| Error::Cancelled)?;
                extremes.clear();
                rows(r0 + r, &mut planes, &mut extremes);
                let off = r * bw;
                open_loop_row_features(
                    &planes,
                    pw,
                    bw,
                    steps,
                    &mut min_b[off..off + bw],
                    &mut mean_b[off..off + bw],
                    &mut sat_b[off..off + bw],
                );
            }
            Ok((min_b, mean_b, sat_b))
        });

    let mut min_q = try_vec_cap(nb)?;
    let mut mean_q = try_vec_cap(nb)?;
    let mut sat_smooth = try_vec_cap(nb)?;
    for band in feat_bands {
        let (min_b, mean_b, sat_b) = band?;
        min_q.extend_from_slice(&min_b);
        mean_q.extend_from_slice(&mean_b);
        sat_smooth.extend_from_slice(&sat_b);
    }

    // Pass 2: 3x3 surrounding-block raw scores per band + per-row scalar totals.
    type RawBand = (Vec<f64>, Vec<(f64, usize, f64)>);
    let mut band_indices = try_vec_cap(n_bands)?;
    band_indices.extend(0..n_bands);
    let raw_bands: Vec<Result<RawBand, Error>> =
        crate::par::par_map(band_indices, threads, |_, bi| {
            let r0 = bi * BAND_ROWS;
            let r1 = (r0 + BAND_ROWS).min(bh);
            let nr = r1 - r0;
            let n = nr.checked_mul(bw).ok_or(Error::TooLarge)?;
            let mut raw_b = try_vec(n, 0.0f64)?;
            let mut row_totals = try_vec_cap(nr)?;
            for r in 0..nr {
                stop.check().map_err(|_| Error::Cancelled)?;
                let by = r0 + r;
                let above = (by > 0).then(|| &mean_q[(by - 1) * bw..by * bw]);
                let cur_mean = &mean_q[by * bw..(by + 1) * bw];
                let below = (by + 1 < bh).then(|| &mean_q[(by + 1) * bw..(by + 2) * bw]);
                let cur_min = &min_q[by * bw..(by + 1) * bw];
                let cur_sat = &sat_smooth[by * bw..(by + 1) * bw];
                let totals = open_loop_row_raw(
                    above,
                    cur_mean,
                    below,
                    cur_min,
                    cur_sat,
                    &mut raw_b[r * bw..(r + 1) * bw],
                );
                row_totals.push(totals);
            }
            Ok((raw_b, row_totals))
        });

    let mut raw_chunks: Vec<Vec<f64>> = try_vec_cap(n_bands)?;
    let mut sum_raw = 0.0f64;
    let mut active_n = 0usize;
    let mut all_sum = 0.0f64;
    for band in raw_bands {
        let (raw_b, row_totals) = band?;
        for (rs, rn, ra) in row_totals {
            sum_raw += rs;
            active_n += rn;
            all_sum += ra;
        }
        raw_chunks.push(raw_b);
    }
    let mean_raw = if active_n > 0 {
        sum_raw / active_n as f64
    } else {
        all_sum / nb as f64
    };
    let st = open_loop_effective_strength(steps, strength);

    // Pass 3: exponentiate per band in parallel and assemble.
    let out_bands: Vec<Result<Vec<f32>, Error>> =
        crate::par::par_map(raw_chunks, threads, |_, raw_b| {
            let mut out_b = try_vec(raw_b.len(), 1.0f32)?;
            for (i, &rv) in raw_b.iter().enumerate() {
                if (i & 1023) == 0 {
                    stop.check().map_err(|_| Error::Cancelled)?;
                }
                let log_m = (st * (rv - mean_raw)).clamp(-0.85, 0.85);
                out_b[i] = crate::lossy::transfer::exp(log_m) as f32;
            }
            Ok(out_b)
        });
    let mut out = try_vec_cap(nb)?;
    for ob in out_bands {
        out.extend_from_slice(&ob?);
    }
    Ok(out)
}

/// Streaming 3-row sliding-window builder for [`OpenLoopLambdaModel`], used by
/// the bounded-memory encoder to compute the exact same global `mean_raw` as
/// [`build_open_loop_lambda_map`] in `O(width)` working RAM plus a tiny
/// per-band first-row boundary cache (`n_bands * bw * 4 B`).
pub(super) struct OpenLoopLambdaStream {
    bw: usize,
    bh: usize,
    work: usize,
    st: f64,
    next_by: usize,
    prev_mean: Vec<f32>,
    cur_min: Vec<f32>,
    cur_mean: Vec<f32>,
    cur_sat: Vec<f32>,
    next_min: Vec<f32>,
    next_mean: Vec<f32>,
    next_sat: Vec<f32>,
    raw_scratch: Vec<f64>,
    has_prev: bool,
    has_cur: bool,
    sum_raw: f64,
    active_n: usize,
    all_sum: f64,
    /// `mean_q` of the first block row of every work band (`band_idx * work`),
    /// enabling exact 3x3 bottom-halo recomputation on subsequent streaming
    /// passes without requesting lookahead rows from `PassIo`.
    band_first_mean: Vec<f32>,
}

impl OpenLoopLambdaStream {
    pub(super) fn new(
        bw: usize,
        bh: usize,
        work: usize,
        steps: &[[f32; 64]; NCH],
        strength: f32,
    ) -> Result<Self, Error> {
        let n_bands = bh.div_ceil(work.max(1));
        let cache_len = n_bands.checked_mul(bw).ok_or(Error::TooLarge)?;
        Ok(Self {
            bw,
            bh,
            work: work.max(1),
            st: open_loop_effective_strength(steps, strength),
            next_by: 0,
            prev_mean: try_vec(bw, 0.0f32)?,
            cur_min: try_vec(bw, 0.0f32)?,
            cur_mean: try_vec(bw, 0.0f32)?,
            cur_sat: try_vec(bw, 0.0f32)?,
            next_min: try_vec(bw, 0.0f32)?,
            next_mean: try_vec(bw, 0.0f32)?,
            next_sat: try_vec(bw, 0.0f32)?,
            raw_scratch: try_vec(bw, 0.0f64)?,
            has_prev: false,
            has_cur: false,
            sum_raw: 0.0,
            active_n: 0,
            all_sum: 0.0,
            band_first_mean: try_vec_cap(cache_len)?,
        })
    }

    /// Feeds block row `by` (strictly ascending `0..bh`) from its color planes.
    pub(super) fn push_row(
        &mut self,
        by: usize,
        planes: &[f32],
        pw: usize,
        steps: &[[f32; 64]; NCH],
    ) {
        debug_assert_eq!(by, self.next_by);
        self.next_by += 1;
        open_loop_row_features(
            planes,
            pw,
            self.bw,
            steps,
            &mut self.next_min,
            &mut self.next_mean,
            &mut self.next_sat,
        );
        if by.is_multiple_of(self.work) {
            self.band_first_mean.extend_from_slice(&self.next_mean);
        }
        if self.has_cur {
            let above = self.has_prev.then_some(&self.prev_mean[..]);
            let (rs, rn, ra) = open_loop_row_raw(
                above,
                &self.cur_mean,
                Some(&self.next_mean),
                &self.cur_min,
                &self.cur_sat,
                &mut self.raw_scratch,
            );
            self.sum_raw += rs;
            self.active_n += rn;
            self.all_sum += ra;
            core::mem::swap(&mut self.prev_mean, &mut self.cur_mean);
            self.has_prev = true;
        }
        core::mem::swap(&mut self.cur_min, &mut self.next_min);
        core::mem::swap(&mut self.cur_mean, &mut self.next_mean);
        core::mem::swap(&mut self.cur_sat, &mut self.next_sat);
        self.has_cur = true;
    }

    pub(super) fn finish(mut self) -> Result<OpenLoopLambdaModel, Error> {
        if self.has_cur {
            let above = self.has_prev.then_some(&self.prev_mean[..]);
            let (rs, rn, ra) = open_loop_row_raw(
                above,
                &self.cur_mean,
                None,
                &self.cur_min,
                &self.cur_sat,
                &mut self.raw_scratch,
            );
            self.sum_raw += rs;
            self.active_n += rn;
            self.all_sum += ra;
        }
        let nb = self.bw.checked_mul(self.bh).ok_or(Error::TooLarge)?;
        let mean_raw = if self.active_n > 0 {
            self.sum_raw / self.active_n as f64
        } else if nb > 0 {
            self.all_sum / nb as f64
        } else {
            0.0
        };
        Ok(OpenLoopLambdaModel {
            bw: self.bw,
            bh: self.bh,
            work: self.work,
            st: self.st,
            mean_raw,
            band_first_mean: self.band_first_mean,
        })
    }
}

/// Compact settled open-loop AQ model retained across streaming passes in
/// `bounded.rs` (`n_bands * bw * 4 B` = 512 KiB at 67 MPx, 8 MiB at 1 GPx).
pub(super) struct OpenLoopLambdaModel {
    bw: usize,
    bh: usize,
    work: usize,
    st: f64,
    mean_raw: f64,
    band_first_mean: Vec<f32>,
}

/// Per-pass streaming cursor over [`OpenLoopLambdaModel`] that computes exact
/// per-band `open_lambda_map` (`(by1 - by0) * bw` f32s) sequentially top-to-bottom.
pub(super) struct OpenLoopBandCursor {
    prev_last_mean: Vec<f32>,
    has_prev: bool,
    min_q: Vec<f32>,
    mean_q: Vec<f32>,
    sat_smooth: Vec<f32>,
    planes: Vec<f32>,
    extremes: Vec<(u64, bool)>,
    raw: Vec<f64>,
}

impl OpenLoopLambdaModel {
    pub(super) fn cursor(&self, pw: usize) -> Result<OpenLoopBandCursor, Error> {
        let band_blocks = self.work.checked_mul(self.bw).ok_or(Error::TooLarge)?;
        let plane_len = NCH
            .checked_mul(8)
            .and_then(|v| v.checked_mul(pw))
            .ok_or(Error::TooLarge)?;
        Ok(OpenLoopBandCursor {
            prev_last_mean: try_vec(self.bw, 0.0f32)?,
            has_prev: false,
            min_q: try_vec(band_blocks, 0.0f32)?,
            mean_q: try_vec(band_blocks, 0.0f32)?,
            sat_smooth: try_vec(band_blocks, 0.0f32)?,
            planes: try_vec(plane_len, 0.0f32)?,
            extremes: try_vec_cap(self.bw)?,
            raw: try_vec(self.bw, 0.0f64)?,
        })
    }

    /// Recomputes the exact open-loop lambda multipliers for work band
    /// `[by0, by1)` in strictly ascending band order into `out`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn fill_band(
        &self,
        cursor: &mut OpenLoopBandCursor,
        by0: usize,
        by1: usize,
        rows: RowPlanes,
        pw: usize,
        steps: &[[f32; 64]; NCH],
        out: &mut Vec<f32>,
    ) -> Result<(), Error> {
        let nr = by1 - by0;
        let bw = self.bw;
        let n = nr.checked_mul(bw).ok_or(Error::TooLarge)?;
        out.clear();
        out.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
        out.resize(n, 1.0f32);
        if nr == 0 || bw == 0 {
            return Ok(());
        }
        debug_assert!(by0.is_multiple_of(self.work));
        if by0 == 0 {
            cursor.has_prev = false;
        }
        for r in 0..nr {
            cursor.extremes.clear();
            rows(by0 + r, &mut cursor.planes, &mut cursor.extremes);
            let off = r * bw;
            open_loop_row_features(
                &cursor.planes,
                pw,
                bw,
                steps,
                &mut cursor.min_q[off..off + bw],
                &mut cursor.mean_q[off..off + bw],
                &mut cursor.sat_smooth[off..off + bw],
            );
        }
        let next_band_idx = by0 / self.work + 1;
        let next_first = (by1 < self.bh)
            .then(|| &self.band_first_mean[next_band_idx * bw..(next_band_idx + 1) * bw]);
        for r in 0..nr {
            let above = if r > 0 {
                Some(&cursor.mean_q[(r - 1) * bw..r * bw])
            } else if cursor.has_prev {
                Some(&cursor.prev_last_mean[..])
            } else {
                None
            };
            let cur_mean = &cursor.mean_q[r * bw..(r + 1) * bw];
            let below = if r + 1 < nr {
                Some(&cursor.mean_q[(r + 1) * bw..(r + 2) * bw])
            } else {
                next_first
            };
            open_loop_row_raw(
                above,
                cur_mean,
                below,
                &cursor.min_q[r * bw..(r + 1) * bw],
                &cursor.sat_smooth[r * bw..(r + 1) * bw],
                &mut cursor.raw,
            );
            let dst = &mut out[r * bw..(r + 1) * bw];
            for (d, &rv) in dst.iter_mut().zip(&cursor.raw) {
                let log_m = (self.st * (rv - self.mean_raw)).clamp(-0.85, 0.85);
                *d = crate::lossy::transfer::exp(log_m) as f32;
            }
        }
        cursor
            .prev_last_mean
            .copy_from_slice(&cursor.mean_q[(nr - 1) * bw..nr * bw]);
        cursor.has_prev = true;
        Ok(())
    }
}

/// Reconstructs one stripe's 2-block-row band into `out` (`[ch][y * pw + x]`)
/// for all 8x8, 4x4, and merged 16x16/16x8/8x16 blocks, matching the normative
/// decoder transforms.
#[allow(clippy::too_many_arguments)]
fn reconstruct_two_rows(
    stripe_units: &[&StripeCoefs],
    r0: usize,
    rr: usize,
    image_bw: usize,
    group_bw: usize,
    dq8: &[[f32; 64]; NCH],
    dq4: &[[f32; 64]; NCH],
    sb_tables: &superblock::Tables,
    out: &mut [Vec<f32>; NCH],
    done: &mut Vec<bool>,
) -> Result<(), Error> {
    let pw = image_bw * 8;
    for ch in 0..NCH {
        out[ch].fill(0.0);
    }
    for (g, sc) in stripe_units.iter().enumerate() {
        let bx0 = g * group_bw;
        let sbw = sc.bw;
        let need = rr.checked_mul(sbw).ok_or(Error::TooLarge)?;
        if done.len() < need {
            done.try_reserve_exact(need - done.len())
                .map_err(|_| Error::TooLarge)?;
        }
        done.clear();
        done.resize(need, false);
        if !sc.modes.is_empty() {
            for gx in (0..sbw).step_by(2) {
                let cols = (sbw - gx).min(2);
                let mut any_m = false;
                for dy in 0..rr {
                    for dx in 0..cols {
                        if sc.modes[(r0 + dy) * sbw + gx + dx] != 0 {
                            any_m = true;
                        }
                    }
                }
                if !any_m {
                    continue;
                }
                let roles = [
                    sc.modes[r0 * sbw + gx],
                    if cols == 2 {
                        sc.modes[r0 * sbw + gx + 1]
                    } else {
                        0
                    },
                    if rr == 2 {
                        sc.modes[(r0 + 1) * sbw + gx]
                    } else {
                        0
                    },
                    if rr == 2 && cols == 2 {
                        sc.modes[(r0 + 1) * sbw + gx + 1]
                    } else {
                        0
                    },
                ];
                let Some(sym) = superblock::symbol_of(roles) else {
                    continue;
                };
                for &(s, py0, px0) in superblock::BLOCKS[sym as usize] {
                    let b0 = (r0 + py0) * sbw + gx + px0;
                    let aq = sc.aq.get(b0).copied().unwrap_or(64) as f32 * (1.0 / 64.0);
                    let si = s as usize;
                    for ch in 0..NCH {
                        let mut spec = [0.0f32; 256];
                        let mut dcs = [[0.0f32; 2]; 2];
                        for py in 0..s.ph() {
                            for px in 0..s.pw() {
                                let b = (r0 + py0 + py) * sbw + gx + px0 + px;
                                let u = &sc.q[(b * NCH + ch) * 64..][..64];
                                dcs[py][px] = u[0] as f32 * sb_tables.dc_step[ch];
                                let p = py * s.pw() + px;
                                for z in 1..64 {
                                    let qv = u[z];
                                    if qv == 0 {
                                        continue;
                                    }
                                    let (vv, uu) = if p == 0 && (z == 1 || z == 8 || z == 9) {
                                        let qy = z / 8;
                                        let qx = z % 8;
                                        if qy < s.ph() && qx < s.pw() {
                                            (qy * 8, qx * 8)
                                        } else {
                                            (qy, qx)
                                        }
                                    } else {
                                        ((p / s.pw()) * 8 + z / 8, (p % s.pw()) * 8 + z % 8)
                                    };
                                    let idx = vv * 16 + uu;
                                    spec[idx] = qv as f32 * sb_tables.dq[ch][si][idx] * aq;
                                }
                            }
                        }
                        let llf = superblock::dcs_to_llf(s, dcs);
                        for v in 0..s.ph() {
                            for u in 0..s.pw() {
                                spec[v * 16 + u] = llf[v][u] * sb_tables.sc[si][v * 16 + u];
                            }
                        }
                        superblock::kernel::inverse_full(s, &mut spec);
                        for y in 0..s.h() {
                            let dst = (py0 * 8 + y) * pw + (bx0 + gx + px0) * 8;
                            out[ch][dst..dst + s.w()].copy_from_slice(&spec[y * 16..][..s.w()]);
                        }
                    }
                    for py in 0..s.ph() {
                        for px in 0..s.pw() {
                            done[(py0 + py) * sbw + gx + px0 + px] = true;
                        }
                    }
                }
            }
        }
        for dy in 0..rr {
            for x in 0..sbw {
                if done[dy * sbw + x] {
                    continue;
                }
                let b = (r0 + dy) * sbw + x;
                let aq_code = sc.aq.get(b).copied().unwrap_or(64);
                if aq_code == 0 {
                    // Exact or inter-keep block: skip reconstruction (its AC is
                    // zeroed on exact blocks or inherited from a prior frame);
                    // seam feedback excludes `skip_blk` boundaries.
                    continue;
                }
                let aq = aq_code as f32 * (1.0 / 64.0);
                let split = sc.splits.get(b).copied().unwrap_or(0);
                for ch in 0..NCH {
                    let u = &sc.q[(b * NCH + ch) * 64..][..64];
                    let four = (split >> ch & 1) != 0;
                    let dq = if four { &dq4[ch] } else { &dq8[ch] };
                    let mut buf: [f32; 64] = core::array::from_fn(|i| {
                        u[i] as f32 * if i == 0 { dq[0] } else { dq[i] * aq }
                    });
                    if four {
                        super::small::inverse_ref(&mut buf);
                    } else {
                        super::dct::idct8x8(&mut buf);
                    }
                    for y in 0..8 {
                        let dst = (dy * 8 + y) * pw + (bx0 + x) * 8;
                        out[ch][dst..dst + 8].copy_from_slice(&buf[y * 8..][..8]);
                    }
                }
            }
        }
    }
    Ok(())
}

/// Precomputed constants shared across closed-loop stripe workers and streaming
/// bounded feedback.
pub(super) struct ClosedLoopCtx {
    dq8: [[f32; 64]; NCH],
    dq4: [[f32; 64]; NCH],
    sb_tables: superblock::Tables,
    inv_s1_sq: [f64; NCH],
    inv_seam_tau: [f64; NCH],
    y_step1: f32,
}

const CLOSED_LOOP_CH_W: [f64; NCH] = [1.0, 0.5, 0.5];

impl ClosedLoopCtx {
    pub(super) fn new(steps: &[[f32; 64]; NCH]) -> Self {
        Self {
            dq8: steps.map(|t| super::dct::dequant_factors(&t)),
            dq4: steps.map(|t| super::small::dequant4(&t)),
            sb_tables: superblock::Tables::new(steps),
            inv_s1_sq: core::array::from_fn(|ch| {
                1.0 / ((steps[ch][1] as f64) * (steps[ch][1] as f64)).max(1e-12)
            }),
            inv_seam_tau: core::array::from_fn(|ch| 1.0 / ((1.5 * steps[ch][1]) as f64).max(1e-6)),
            y_step1: steps[0][1],
        }
    }
}

/// Per-stripe closed-loop perceptual + intra-stripe seam errors plus 1-pixel
/// top/bottom boundary halos for deterministic cross-stripe vertical seam fusion.
pub(super) struct StripeClosedLoop {
    pub by0: usize,
    pub nby: usize,
    pub blk_err: Vec<f64>,
    pub skip_blk: Vec<bool>,
    pub top_orig: [Vec<f32>; NCH],
    pub top_err: [Vec<f32>; NCH],
    pub bot_orig: [Vec<f32>; NCH],
    pub bot_err: [Vec<f32>; NCH],
}

/// Computes interior contrast-masked error + horizontal seams + intra-stripe
/// vertical seams for one stripe (`stripe_units`: its column groups in order).
/// Completely independent of other stripes and thread count.
#[allow(clippy::too_many_arguments)]
pub(super) fn closed_loop_stripe_errors(
    ctx: &ClosedLoopCtx,
    stripe_units: &[StripeCoefs],
    rows: RowPlanes,
    pw: usize,
    image_bw: usize,
    group_bw: usize,
    stop: &dyn how_far::Stop,
) -> Result<StripeClosedLoop, Error> {
    stop.check().map_err(|_| Error::Cancelled)?;
    let mut refs = try_vec_cap(stripe_units.len())?;
    refs.extend(stripe_units.iter());
    let (by0, nby) = (refs[0].by0, refs[0].nby);
    let snb = nby.checked_mul(image_bw).ok_or(Error::TooLarge)?;
    let mut blk_err = try_vec(snb, 0.0f64)?;
    let mut skip_blk = try_vec(snb, false)?;
    for (g, sc) in refs.iter().enumerate() {
        if sc.aq.is_empty() {
            continue;
        }
        let bx0 = g * group_bw;
        for r in 0..sc.nby {
            for x in 0..sc.bw {
                if sc.aq[r * sc.bw + x] == 0 {
                    skip_blk[r * image_bw + bx0 + x] = true;
                }
            }
        }
    }

    let plane_len = NCH
        .checked_mul(8)
        .and_then(|v| v.checked_mul(pw))
        .ok_or(Error::TooLarge)?;
    let rec_len = 16usize.checked_mul(pw).ok_or(Error::TooLarge)?;
    let mut planes2 = [try_vec(plane_len, 0.0f32)?, try_vec(plane_len, 0.0f32)?];
    let mut extremes = try_vec_cap(image_bw)?;
    let mut rec2: [Vec<f32>; NCH] = [
        try_vec(rec_len, 0.0f32)?,
        try_vec(rec_len, 0.0f32)?,
        try_vec(rec_len, 0.0f32)?,
    ];
    let mut top_orig: [Vec<f32>; NCH] = [
        try_vec(pw, 0.0f32)?,
        try_vec(pw, 0.0f32)?,
        try_vec(pw, 0.0f32)?,
    ];
    let mut top_err: [Vec<f32>; NCH] = [
        try_vec(pw, 0.0f32)?,
        try_vec(pw, 0.0f32)?,
        try_vec(pw, 0.0f32)?,
    ];
    let mut prev_bot_orig: [Vec<f32>; NCH] = [
        try_vec(pw, 0.0f32)?,
        try_vec(pw, 0.0f32)?,
        try_vec(pw, 0.0f32)?,
    ];
    let mut prev_bot_err: [Vec<f32>; NCH] = [
        try_vec(pw, 0.0f32)?,
        try_vec(pw, 0.0f32)?,
        try_vec(pw, 0.0f32)?,
    ];
    let mut done = Vec::new();

    for r0 in (0..nby).step_by(2) {
        stop.check().map_err(|_| Error::Cancelled)?;
        let rr = (nby - r0).min(2);
        for dy in 0..rr {
            extremes.clear();
            rows(by0 + r0 + dy, &mut planes2[dy], &mut extremes);
        }
        reconstruct_two_rows(
            &refs,
            r0,
            rr,
            image_bw,
            group_bw,
            &ctx.dq8,
            &ctx.dq4,
            &ctx.sb_tables,
            &mut rec2,
            &mut done,
        )?;
        for dy in 0..rr {
            let r = r0 + dy;
            if r == 0 {
                for ch in 0..NCH {
                    let p_off = ch * 8 * pw;
                    for x in 0..pw {
                        let o = planes2[0][p_off + x];
                        top_orig[ch][x] = o;
                        top_err[ch][x] = o - rec2[ch][x];
                    }
                }
            }
            for bx in 0..image_bw {
                let li = r * image_bw + bx;
                if skip_blk[li] {
                    continue;
                }
                let mut err_acc = 0.0f64;
                let mut orig_y = [[0.0f32; 8]; 8];
                for y in 0..8 {
                    orig_y[y].copy_from_slice(&planes2[dy][y * pw + bx * 8..][..8]);
                }
                let vis_y = block_visibility(&orig_y, ctx.y_step1, 2.0);
                for ch in 0..NCH {
                    let inv_s2 = ctx.inv_s1_sq[ch];
                    let cw = CLOSED_LOOP_CH_W[ch];
                    for y in 0..8 {
                        let p_off = ch * 8 * pw + y * pw + bx * 8;
                        let r_off = (dy * 8 + y) * pw + bx * 8;
                        for x in 0..8 {
                            let e = (planes2[dy][p_off + x] - rec2[ch][r_off + x]) as f64;
                            let e2 = (e * e * inv_s2).min(64.0);
                            err_acc += cw * vis_y[y][x] as f64 * (e2 + 0.15 * e2 * e2);
                        }
                    }
                    let inv_tau = ctx.inv_seam_tau[ch];
                    if bx > 0 && !skip_blk[li - 1] {
                        let mut hs = 0.0f64;
                        for y in 0..8 {
                            let p_off = ch * 8 * pw + y * pw + bx * 8;
                            let r_off = (dy * 8 + y) * pw + bx * 8;
                            let oa = planes2[dy][p_off - 1];
                            let ob = planes2[dy][p_off];
                            let ea = oa - rec2[ch][r_off - 1];
                            let eb = ob - rec2[ch][r_off];
                            hs += seam_pair_cost(oa, ob, ea, eb, inv_tau, inv_s2);
                        }
                        let term = cw * 1.5 * hs;
                        err_acc += term;
                        blk_err[li - 1] += term;
                    }
                    if r > 0 && !skip_blk[li - image_bw] {
                        let mut vs = 0.0f64;
                        let p_off = ch * 8 * pw + bx * 8;
                        let r_off = (dy * 8) * pw + bx * 8;
                        for x in 0..8 {
                            let oa = prev_bot_orig[ch][bx * 8 + x];
                            let ob = planes2[dy][p_off + x];
                            let ea = prev_bot_err[ch][bx * 8 + x];
                            let eb = ob - rec2[ch][r_off + x];
                            vs += seam_pair_cost(oa, ob, ea, eb, inv_tau, inv_s2);
                        }
                        let term = cw * 1.5 * vs;
                        err_acc += term;
                        blk_err[li - image_bw] += term;
                    }
                }
                blk_err[li] += err_acc;
            }
            for ch in 0..NCH {
                let p_off = ch * 8 * pw + 7 * pw;
                let r_off = (dy * 8 + 7) * pw;
                for x in 0..pw {
                    let o = planes2[dy][p_off + x];
                    prev_bot_orig[ch][x] = o;
                    prev_bot_err[ch][x] = o - rec2[ch][r_off + x];
                }
            }
        }
    }

    Ok(StripeClosedLoop {
        by0,
        nby,
        blk_err,
        skip_blk,
        top_orig,
        top_err,
        bot_orig: prev_bot_orig,
        bot_err: prev_bot_err,
    })
}

/// Fuses the cross-stripe vertical boundary seam between the previous stripe's
/// bottom block row and the current stripe's top block row in strict channel
/// order, identical in both parallel whole-image and bounded streaming paths.
#[allow(clippy::too_many_arguments)]
fn add_cross_stripe_seam(
    ctx: &ClosedLoopCtx,
    image_bw: usize,
    prev_bot_orig: &[Vec<f32>; NCH],
    prev_bot_err: &[Vec<f32>; NCH],
    prev_skip: &[bool],
    prev_err: &mut [f64],
    cur_top_orig: &[Vec<f32>; NCH],
    cur_top_err: &[Vec<f32>; NCH],
    cur_skip: &[bool],
    cur_err: &mut [f64],
) {
    for bx in 0..image_bw {
        if prev_skip[bx] || cur_skip[bx] {
            continue;
        }
        let mut add = 0.0f64;
        for ch in 0..NCH {
            let inv_s2 = ctx.inv_s1_sq[ch];
            let inv_tau = ctx.inv_seam_tau[ch];
            let cw = CLOSED_LOOP_CH_W[ch];
            let mut vs = 0.0f64;
            for x in 0..8 {
                let px = bx * 8 + x;
                vs += seam_pair_cost(
                    prev_bot_orig[ch][px],
                    cur_top_orig[ch][px],
                    prev_bot_err[ch][px],
                    cur_top_err[ch][px],
                    inv_tau,
                    inv_s2,
                );
            }
            add += cw * 1.5 * vs;
        }
        prev_err[bx] += add;
        cur_err[bx] += add;
    }
}

/// Closed-loop perceptual & unfiltered-seam residual feedback for
/// trellis-lambda AQ, parallelized per stripe across `threads` with
/// deterministic cross-stripe vertical seam fusion and strict ascending
/// reduction of `ref_log`.
#[allow(clippy::too_many_arguments)]
pub(super) fn update_lambda_map_closed_loop(
    lambda_map: &mut [f32],
    units: &[StripeCoefs],
    rows: RowPlanes,
    pw: usize,
    image_bw: usize,
    bh: usize,
    group_bw: usize,
    steps: &[[f32; 64]; NCH],
    feedback_weight: f32,
    threads: usize,
    stop: &dyn how_far::Stop,
) -> Result<(), Error> {
    stop.check().map_err(|_| Error::Cancelled)?;
    let nb = image_bw.checked_mul(bh).ok_or(Error::TooLarge)?;
    if nb == 0 || feedback_weight == 0.0 || lambda_map.len() < nb {
        return Ok(());
    }
    let groups = if group_bw == 0 {
        1
    } else {
        image_bw.div_ceil(group_bw)
    };
    let n_stripes = units.len() / groups;
    let mut stream = ClosedLoopLambdaStream::new(image_bw, bh, steps, feedback_weight)?;

    let mut stripe_indices = try_vec_cap(n_stripes)?;
    stripe_indices.extend(0..n_stripes);
    let per_stripe: Vec<Result<StripeClosedLoop, Error>> = {
        let ctx = stream.ctx();
        crate::par::par_map(stripe_indices, threads, |_, s| {
            let slice = &units[s * groups..(s + 1) * groups];
            closed_loop_stripe_errors(ctx, slice, rows, pw, image_bw, group_bw, stop)
        })
    };

    for res in per_stripe {
        stop.check().map_err(|_| Error::Cancelled)?;
        stream.push_stripe(res?)?;
    }

    let model = stream.finish();
    model.apply_in_place(&mut lambda_map[..nb], stop)
}

/// Block count threshold shared by BOTH whole-image
/// [`update_lambda_map_closed_loop`] and streaming [`ClosedLoopLambdaStream`]:
/// images at or below `262_144` blocks (`<= 16 MPx`, `<= 2 MiB`) retain exact
/// `f64` block errors, while larger images store quantized `u8` log-error
/// (`1 B/block` = 1 MiB at 67 MPx, 15.6 MiB at 1 GPx). Because both paths share
/// this exact collector and model, whole-image and bounded encodes produce
/// bit-identical closed-loop multipliers at every image size.
pub(super) const CLOSED_LOOP_EXACT_MAX_BLOCKS: usize = 262_144;

const LOG_Q_MIN: f64 = -1.386_294_361_119_890_6; // ln(0.25)
const LOG_Q_MAX: f64 = 6.2;
const LOG_Q_SCALE: f64 = 253.0 / (LOG_Q_MAX - LOG_Q_MIN);

enum ClosedLoopStore {
    /// Exact `f64` per block (`NaN` marks `skip_blk`).
    Exact(Vec<f64>),
    /// Compact `u8` log-error per block (`0` = skip, `1` = inactive `e <= 1e-4`,
    /// `2..=255` = uniformly quantized `ln(e + 0.25)` over `[ln(0.25), 6.2]`).
    Compact(Vec<u8>),
}

/// Streaming closed-loop perceptual & unfiltered-seam residual collector for
/// `bounded.rs` (fed during pass `10 + 0`, keeping only a 1-row stripe halo in
/// working RAM).
pub(super) struct ClosedLoopLambdaStream {
    ctx: ClosedLoopCtx,
    image_bw: usize,
    bh: usize,
    feedback_weight: f32,
    store: ClosedLoopStore,
    sum_log: f64,
    cnt: usize,
    /// Pending bottom block row of the previous stripe (waiting for the next
    /// stripe's top row to add its cross-stripe vertical seam before flushing).
    pending_row: Option<PendingClosedRow>,
}

struct PendingClosedRow {
    by: usize,
    blk_err: Vec<f64>,
    skip_blk: Vec<bool>,
    bot_orig: [Vec<f32>; NCH],
    bot_err: [Vec<f32>; NCH],
}

impl ClosedLoopLambdaStream {
    pub(super) fn new(
        image_bw: usize,
        bh: usize,
        steps: &[[f32; 64]; NCH],
        feedback_weight: f32,
    ) -> Result<Self, Error> {
        Self::new_with_exact_max(
            image_bw,
            bh,
            steps,
            feedback_weight,
            CLOSED_LOOP_EXACT_MAX_BLOCKS,
        )
    }

    fn new_with_exact_max(
        image_bw: usize,
        bh: usize,
        steps: &[[f32; 64]; NCH],
        feedback_weight: f32,
        exact_max_blocks: usize,
    ) -> Result<Self, Error> {
        let nb = image_bw.checked_mul(bh).ok_or(Error::TooLarge)?;
        let store = if nb <= exact_max_blocks {
            ClosedLoopStore::Exact(try_vec(nb, 0.0f64)?)
        } else {
            ClosedLoopStore::Compact(try_vec(nb, 0u8)?)
        };
        Ok(Self {
            ctx: ClosedLoopCtx::new(steps),
            image_bw,
            bh,
            feedback_weight,
            store,
            sum_log: 0.0,
            cnt: 0,
            pending_row: None,
        })
    }

    pub(super) fn ctx(&self) -> &ClosedLoopCtx {
        &self.ctx
    }

    fn flush_row(&mut self, by: usize, errs: &[f64], skips: &[bool]) {
        let base = by * self.image_bw;
        for bx in 0..self.image_bw {
            let e = errs[bx];
            let sk = skips[bx];
            if !sk && e > 1e-4 {
                self.sum_log += crate::lossy::transfer::ln(e + 0.25);
                self.cnt += 1;
            }
            match &mut self.store {
                ClosedLoopStore::Exact(v) => {
                    v[base + bx] = if sk { f64::NAN } else { e };
                }
                ClosedLoopStore::Compact(v) => {
                    v[base + bx] = if sk {
                        0
                    } else if e <= 1e-4 {
                        1
                    } else {
                        let le = crate::lossy::transfer::ln(e + 0.25).clamp(LOG_Q_MIN, LOG_Q_MAX);
                        let q = ((le - LOG_Q_MIN) * LOG_Q_SCALE + 0.5) as usize;
                        (2 + q.min(253)) as u8
                    };
                }
            }
        }
    }

    /// Consumes one completed stripe (strictly ascending `by0`) from pass `10 + 0`.
    pub(super) fn push_stripe(&mut self, mut st: StripeClosedLoop) -> Result<(), Error> {
        let bw = self.image_bw;
        if let Some(mut prev) = self.pending_row.take() {
            if st.by0 == prev.by + 1 {
                add_cross_stripe_seam(
                    &self.ctx,
                    bw,
                    &prev.bot_orig,
                    &prev.bot_err,
                    &prev.skip_blk,
                    &mut prev.blk_err,
                    &st.top_orig,
                    &st.top_err,
                    &st.skip_blk[..bw],
                    &mut st.blk_err[..bw],
                );
            }
            self.flush_row(prev.by, &prev.blk_err, &prev.skip_blk);
        }
        // Flush all interior rows `0..st.nby - 1` immediately; hold only the
        // bottom row until the next stripe supplies its vertical seam.
        for r in 0..st.nby.saturating_sub(1) {
            let off = r * bw;
            self.flush_row(
                st.by0 + r,
                &st.blk_err[off..off + bw],
                &st.skip_blk[off..off + bw],
            );
        }
        if st.nby > 0 {
            let last = st.nby - 1;
            let off = last * bw;
            let mut last_err = try_vec_cap(bw)?;
            last_err.extend_from_slice(&st.blk_err[off..off + bw]);
            let mut last_skip = try_vec_cap(bw)?;
            last_skip.extend_from_slice(&st.skip_blk[off..off + bw]);
            self.pending_row = Some(PendingClosedRow {
                by: st.by0 + last,
                blk_err: last_err,
                skip_blk: last_skip,
                bot_orig: st.bot_orig,
                bot_err: st.bot_err,
            });
        }
        Ok(())
    }

    pub(super) fn finish(mut self) -> ClosedLoopLambdaModel {
        if let Some(prev) = self.pending_row.take() {
            self.flush_row(prev.by, &prev.blk_err, &prev.skip_blk);
        }
        let ref_log = if self.cnt > 0 {
            Some(self.sum_log / self.cnt as f64)
        } else {
            None
        };
        ClosedLoopLambdaModel {
            image_bw: self.image_bw,
            bh: self.bh,
            feedback_weight: self.feedback_weight as f64,
            ref_log,
            store: self.store,
        }
    }
}

/// Compact settled closed-loop AQ model retained for pass `10 + 1` and `pass2`
/// in `bounded.rs`.
pub(super) struct ClosedLoopLambdaModel {
    image_bw: usize,
    bh: usize,
    feedback_weight: f64,
    ref_log: Option<f64>,
    store: ClosedLoopStore,
}

impl ClosedLoopLambdaModel {
    #[inline]
    fn update_block(&self, idx: usize, base_m: f64, ref_log: f64) -> f32 {
        match &self.store {
            ClosedLoopStore::Exact(v) => {
                let e = v[idx];
                if e.is_nan() {
                    base_m as f32
                } else {
                    let delta = (crate::lossy::transfer::ln(e + 0.25) - ref_log).clamp(-0.75, 1.15);
                    (base_m * crate::lossy::transfer::exp(self.feedback_weight * delta))
                        .clamp(0.30, 3.5) as f32
                }
            }
            ClosedLoopStore::Compact(v) => match v[idx] {
                0 => base_m as f32,
                1 => {
                    let delta = (LOG_Q_MIN - ref_log).clamp(-0.75, 1.15);
                    (base_m * crate::lossy::transfer::exp(self.feedback_weight * delta))
                        .clamp(0.30, 3.5) as f32
                }
                code => {
                    let le = LOG_Q_MIN + (code - 2) as f64 / LOG_Q_SCALE;
                    let delta = (le - ref_log).clamp(-0.75, 1.15);
                    (base_m * crate::lossy::transfer::exp(self.feedback_weight * delta))
                        .clamp(0.30, 3.5) as f32
                }
            },
        }
    }

    /// Applies the closed-loop multiplier update in place over the whole image.
    pub(super) fn apply_in_place(
        &self,
        lambda_map: &mut [f32],
        stop: &dyn how_far::Stop,
    ) -> Result<(), Error> {
        let Some(ref_log) = self.ref_log else {
            return Ok(());
        };
        let nb = self.image_bw.checked_mul(self.bh).ok_or(Error::TooLarge)?;
        let n = nb.min(lambda_map.len());
        for (i, slot) in lambda_map[..n].iter_mut().enumerate() {
            if (i & 1023) == 0 {
                stop.check().map_err(|_| Error::Cancelled)?;
            }
            *slot = self.update_block(i, *slot as f64, ref_log);
        }
        Ok(())
    }

    /// Applies the closed-loop multiplier update to `open_band` (for block rows
    /// `[by0, by1)`) into `out`, bit-exact with [`Self::apply_in_place`].
    pub(super) fn fill_band(
        &self,
        open_band: &[f32],
        by0: usize,
        by1: usize,
        out: &mut Vec<f32>,
    ) -> Result<(), Error> {
        let nr = by1.min(self.bh).saturating_sub(by0);
        let n = nr.checked_mul(self.image_bw).ok_or(Error::TooLarge)?;
        out.clear();
        out.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
        let Some(ref_log) = self.ref_log else {
            if open_band.len() >= n {
                out.extend_from_slice(&open_band[..n]);
            } else {
                out.resize(n, 1.0f32);
            }
            return Ok(());
        };
        let base = by0 * self.image_bw;
        for i in 0..n {
            let base_m = open_band.get(i).copied().unwrap_or(1.0) as f64;
            out.push(self.update_block(base + i, base_m, ref_log));
        }
        Ok(())
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::lossy::{LossyConfig, LossyEntropy, encode_lossy};
    use crate::{Channels, ImageInfo, Metadata, SampleType};
    use alloc::vec;

    fn lcg(seed: &mut u64) -> f64 {
        *seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((*seed >> 11) as f64) * (1.0 / ((1u64 << 53) as f64))
    }

    /// Verifies that the exact 8-state row-strip Viterbi DP equals exhaustive
    /// `8^G` brute-force search to `< 1e-12` on random costs, contexts, masks,
    /// and pairwise transition tables.
    #[test]
    fn row_strip_dp_matches_brute_force() {
        let mut seed = 0x2026_1125_cafe_babeu64;
        for g in 1..=4usize {
            for trial in 0..25 {
                let mut unary = vec![[f64::INFINITY; 8]; g];
                let mut up_class = vec![0u8; g];
                for gc in 0..g {
                    up_class[gc] = (lcg(&mut seed) * 3.0) as u8 % 3;
                    unary[gc][0] = lcg(&mut seed) * 50.0;
                    for s in 1..8 {
                        if lcg(&mut seed) < 0.75 {
                            unary[gc][s] = lcg(&mut seed) * 50.0;
                        }
                    }
                }
                let mut map_bits = MapBitTable::prior();
                for ctx in 0..9 {
                    for s in 0..8 {
                        map_bits.bits[ctx][s] = 0.2 + lcg(&mut seed) * 5.0;
                    }
                }
                let mut horiz = vec![[[0.0f64; 8]; 8]; g];
                for gc in 1..g {
                    for sp in 0..8 {
                        for s in 0..8 {
                            horiz[gc][sp][s] = lcg(&mut seed) * 10.0;
                        }
                    }
                }

                let (dp_cost, dp_path) =
                    solve_row_strip_dp(&unary, &up_class, &map_bits, |gc, sp, s| horiz[gc][sp][s])
                        .unwrap();

                let total_seqs = 1usize << (3 * g);
                let mut bf_cost = f64::INFINITY;
                let mut bf_path = vec![0u8; g];
                for mut code in 0..total_seqs {
                    let mut seq = [0u8; 4];
                    let mut ok = true;
                    for gc in 0..g {
                        let s = (code & 7) as u8;
                        code >>= 3;
                        if !unary[gc][s as usize].is_finite() {
                            ok = false;
                            break;
                        }
                        seq[gc] = s;
                    }
                    if !ok {
                        continue;
                    }
                    let mut c = unary[0][seq[0] as usize]
                        + map_bits.bits[up_class[0] as usize][seq[0] as usize];
                    for gc in 1..g {
                        let sp = seq[gc - 1] as usize;
                        let s = seq[gc] as usize;
                        let ctx = sym_class(sp as u8) * 3 + up_class[gc] as usize;
                        c += map_bits.bits[ctx][s] + horiz[gc][sp][s] + unary[gc][s];
                    }
                    if c < bf_cost {
                        bf_cost = c;
                        bf_path.copy_from_slice(&seq[..g]);
                    }
                }

                assert!(
                    (dp_cost - bf_cost).abs() < 1e-12,
                    "g={g} trial={trial}: dp={dp_cost} bf={bf_cost} dp_path={dp_path:?} bf_path={bf_path:?}"
                );
            }
        }
    }

    #[test]
    fn map_bit_table_prior_probabilities_sum_to_one() {
        let t = MapBitTable::prior();
        for ctx in 0..9 {
            let sum: f64 = (0..8)
                .map(|s| crate::lossy::transfer::pow(2.0, -t.bits[ctx][s]))
                .sum();
            assert!((sum - 1.0).abs() < 1e-6, "ctx={ctx} sum={sum}");
        }
    }

    /// Verifies that the open-loop surrounding-block lambda map assigns higher
    /// protection to smooth regions and flat-adjacent edges than to dense
    /// chaotic texture, is bit-identical across 1T/4T/8T, AND matches
    /// [`OpenLoopLambdaStream`] bit-for-bit across work-band sizes.
    #[test]
    fn open_loop_lambda_map_protects_smooth_and_matches_stream_and_threads() {
        let (bw, bh) = (12usize, 37usize);
        let pw = bw * 8;
        let steps = [[4.0f32; 64]; NCH];
        let rows = |by: usize, planes: &mut [f32], _ext: &mut Vec<(u64, bool)>| {
            planes.fill(0.0);
            for y in 0..8 {
                let gy = by * 8 + y;
                for x in 0..pw {
                    let bx = x / 8;
                    let v = if bx < 4 {
                        (x as f32) * 0.25 + (gy as f32) * 0.15
                    } else if bx < 8 {
                        if x % 8 < 4 { 0.0 } else { 40.0 }
                    } else if ((x + gy) & 1) == 0 {
                        35.0
                    } else {
                        -35.0
                    };
                    planes[y * pw + x] = v;
                }
            }
        };
        let m1 = build_open_loop_lambda_map(&rows, pw, bw, bh, &steps, 0.30, 1, &how_far::NoPulse)
            .unwrap();
        let m4 = build_open_loop_lambda_map(&rows, pw, bw, bh, &steps, 0.30, 4, &how_far::NoPulse)
            .unwrap();
        let m8 = build_open_loop_lambda_map(&rows, pw, bw, bh, &steps, 0.30, 8, &how_far::NoPulse)
            .unwrap();
        assert_eq!(m1, m4, "open-loop lambda map 1T vs 4T mismatch");
        assert_eq!(m1, m8, "open-loop lambda map 1T vs 8T mismatch");

        let lo = crate::lossy::transfer::exp(-0.85) as f32 - 1e-5;
        let hi = crate::lossy::transfer::exp(0.85) as f32 + 1e-5;
        for &v in &m1 {
            assert!(v.is_finite() && v >= lo && v <= hi, "out of bounds: {v}");
        }
        let smooth_interior = m1[2 * bw + 1];
        let edge_interior = m1[2 * bw + 5];
        let texture_interior = m1[2 * bw + 10];
        assert!(smooth_interior > texture_interior);
        assert!(edge_interior > texture_interior);

        // Verify streaming OpenLoopLambdaStream == whole-image build_open_loop_lambda_map bit-for-bit.
        for work in [1usize, 3, 8, 16] {
            let mut stream = OpenLoopLambdaStream::new(bw, bh, work, &steps, 0.30).unwrap();
            let mut planes = vec![0.0f32; NCH * 8 * pw];
            let mut ext = Vec::new();
            for by in 0..bh {
                ext.clear();
                rows(by, &mut planes, &mut ext);
                stream.push_row(by, &planes, pw, &steps);
            }
            let model = stream.finish().unwrap();
            let mut cursor = model.cursor(pw).unwrap();
            let mut streamed = Vec::with_capacity(bw * bh);
            let mut band_out = Vec::new();
            for by0 in (0..bh).step_by(work) {
                let by1 = (by0 + work).min(bh);
                model
                    .fill_band(&mut cursor, by0, by1, &rows, pw, &steps, &mut band_out)
                    .unwrap();
                streamed.extend_from_slice(&band_out);
            }
            assert_eq!(
                streamed, m1,
                "OpenLoopLambdaStream diverged from whole-image at work={work}"
            );
        }
    }

    #[test]
    fn overflow_dimensions_return_too_large() {
        let rows = |_: usize, _: &mut [f32], _: &mut Vec<(u64, bool)>| {};
        let res = build_open_loop_lambda_map(
            &rows,
            8,
            usize::MAX / 2,
            4,
            &[[4.0; 64]; NCH],
            0.30,
            1,
            &how_far::NoPulse,
        );
        assert!(matches!(res, Err(Error::TooLarge)));
    }

    /// Encodes a synthetic multi-region image (smooth gradient, sharp text/edge,
    /// dense texture, saturated color patch, and exact-eligible flat UI strip)
    /// with `adaptive_partitions` across efforts 7.5, 9.0, 9.5, 10.0, both entropy
    /// coders, column groups, and exact blocks, verifying roundtrip decode and
    /// 0-bit cross-ISA (SIMD vs scalar) and multi-thread parity.
    #[test]
    fn adaptive_partitions_roundtrip_exact_and_cross_isa_parity() {
        let (w, h) = (160u32, 128u32);
        let mut px = vec![0u8; (w * h * 3) as usize];
        let mut seed = 0x1234_5678_9abc_def0u64;
        for y in 0..h {
            for x in 0..w {
                let (r, g, b) = if y < 32 {
                    (
                        ((x * 255) / w) as u8,
                        ((y * 255) / 32) as u8,
                        140u8.saturating_sub((x / 2) as u8),
                    )
                } else if y < 64 {
                    let stroke = ((x % 16) < 2 && (y % 12) < 9) || (y % 12 == 0 && (x % 16) < 7);
                    if stroke {
                        (18, 22, 28)
                    } else {
                        (246, 246, 248)
                    }
                } else if y < 96 {
                    let n = (lcg(&mut seed) * 180.0) as u8;
                    (
                        n.saturating_add(30),
                        (n ^ 0x55).saturating_add(20),
                        220u8.saturating_sub(n / 2),
                    )
                } else if x < 80 {
                    (230, 35 + ((x / 4) as u8), 45)
                } else {
                    (30, 185, 225u8.saturating_sub(((x - 80) / 3) as u8))
                };
                let o = ((y * w + x) * 3) as usize;
                px[o] = r;
                px[o + 1] = g;
                px[o + 2] = b;
            }
        }
        let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);

        for ent in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
            for effort in [7.5f32, 9.0, 9.5, 10.0] {
                for q in [20.0f32, 75.0] {
                    let mut cfg = LossyConfig::new(q)
                        .with_effort(effort)
                        .with_entropy(ent)
                        .with_adaptive_partitions(true);
                    cfg.column_groups = Some(64);
                    if q > 50.0 {
                        cfg.exact_blocks = Some(0.5);
                    }
                    let encoded =
                        encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), &cfg)
                            .unwrap();
                    let (dec_simd, dec_scalar) = crate::lossy::tests::both(&encoded);
                    let (dec_simd, dec_scalar) = (dec_simd.unwrap(), dec_scalar.unwrap());
                    assert_eq!(dec_simd.len(), px.len());
                    assert_eq!(
                        dec_simd, dec_scalar,
                        "SIMD vs scalar decode mismatch on {ent:?} e{effort} q{q}"
                    );
                    #[cfg(feature = "std")]
                    {
                        let mut dec_mt = vec![0u8; dec_simd.len()];
                        crate::decode_into_threads(&encoded, &mut dec_mt, info.row_bytes(), 4)
                            .unwrap();
                        assert_eq!(
                            dec_mt, dec_simd,
                            "4T vs 1T decode mismatch on {ent:?} e{effort} q{q}"
                        );
                    }
                }
            }
        }
    }

    fn synthetic_multi_region(w: u32, h: u32) -> Vec<u8> {
        let mut px = vec![0u8; (w * h * 3) as usize];
        let mut seed = 0x51a7_9921_c0de_f00du64;
        for y in 0..h {
            for x in 0..w {
                let (r, g, b) = if y < h / 4 {
                    (
                        ((x * 255) / w.max(1)) as u8,
                        ((y * 255) / (h / 4).max(1)) as u8,
                        150u8.saturating_sub((x / 2) as u8),
                    )
                } else if y < h / 2 {
                    let stroke = ((x % 14) < 2 && (y % 11) < 8) || (y % 11 == 0 && (x % 14) < 6);
                    if stroke {
                        (16, 20, 26)
                    } else {
                        (244, 245, 247)
                    }
                } else if y < (3 * h) / 4 {
                    let n = (lcg(&mut seed) * 190.0) as u8;
                    (
                        n.saturating_add(25),
                        (n ^ 0x33).saturating_add(15),
                        225u8.saturating_sub(n / 2),
                    )
                } else {
                    (220, 40 + ((x / 3) as u8), 60 + ((y / 4) as u8))
                };
                let o = ((y * w + x) * 3) as usize;
                px[o] = r;
                px[o + 1] = g;
                px[o + 2] = b;
            }
        }
        px
    }

    /// Verifies bit-exact encoder determinism across 1T/4T/8T encoder thread
    /// counts, SIMD dispatch permutations (`for_each_token_permutation`), odd
    /// block-grid dimensions (exercising the trailing 1x1 corner omitted by V4
    /// partition maps), odd column-group widths, and ungrouped encodes.
    #[test]
    fn adaptive_partitions_encoder_determinism_and_odd_geometry() {
        use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

        // 133x117 => 17x15 blocks (odd bw & odd bh => trailing 1x1 corner).
        // 133x109 => 17x14 blocks (odd bw & even bh).
        for &(w, h) in &[(133u32, 117u32), (133u32, 109u32)] {
            let px = synthetic_multi_region(w, h);
            let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
            for col_groups in [None, Some(56u32), Some(64u32)] {
                for ent in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                    for effort in [7.5f32, 9.0, 9.5, 10.0] {
                        let mut cfg1 = LossyConfig::new(60.0)
                            .with_effort(effort)
                            .with_entropy(ent)
                            .with_adaptive_partitions(true)
                            .with_threads(1);
                        cfg1.column_groups = col_groups;
                        let bytes1 =
                            encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), &cfg1)
                                .unwrap();

                        #[cfg(feature = "std")]
                        for t in [4usize, 8] {
                            let mut cfgt = cfg1.clone().with_threads(t);
                            cfgt.column_groups = col_groups;
                            let bytest = encode_lossy(
                                &px,
                                info.row_bytes(),
                                &info,
                                &Metadata::default(),
                                &cfgt,
                            )
                            .unwrap();
                            assert_eq!(
                                bytes1, bytest,
                                "encoder output must be thread-count invariant (1T vs {t}T) on {w}x{h} cg={col_groups:?} {ent:?} e{effort}"
                            );
                        }

                        let (dec_simd, dec_scalar) = crate::lossy::tests::both(&bytes1);
                        assert_eq!(
                            dec_simd.unwrap(),
                            dec_scalar.unwrap(),
                            "decode parity mismatch on {w}x{h} cg={col_groups:?} {ent:?} e{effort}"
                        );
                    }
                }
            }
        }

        // Cross-ISA encoder determinism across every SIMD token permutation on
        // the odd-by-odd grid with an odd column-group width (gbw = 7).
        let (w, h) = (133u32, 117u32);
        let px = synthetic_multi_region(w, h);
        let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
        let cases: Vec<(LossyEntropy, f32, LossyConfig, Vec<u8>)> = [
            (LossyEntropy::RunLevel, 7.5),
            (LossyEntropy::RunLevel, 9.0),
            (LossyEntropy::RunLevel, 9.5),
            (LossyEntropy::RunLevel, 10.0),
            (LossyEntropy::SplitHuf, 10.0),
        ]
        .into_iter()
        .map(|(ent, effort)| {
            let mut cfg = LossyConfig::new(55.0)
                .with_effort(effort)
                .with_entropy(ent)
                .with_adaptive_partitions(true)
                .with_threads(1);
            cfg.column_groups = Some(56);
            let reference =
                encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), &cfg).unwrap();
            (ent, effort, cfg, reference)
        })
        .collect();

        let report = for_each_token_permutation(CompileTimePolicy::Warn, |perm| {
            for (ent, effort, cfg, reference) in &cases {
                let got =
                    encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), cfg).unwrap();
                assert_eq!(
                    &got, reference,
                    "encoded bytes diverged under dispatch permutation {perm} ({ent:?} e{effort})"
                );
            }
        });
        assert!(report.permutations_run >= 1);
    }

    /// Verifies cooperative cancellation interrupts open-loop lambda map
    /// construction, adaptive layout selection, and closed-loop feedback
    /// promptly instead of waiting for full-image passes to finish.
    #[test]
    fn adaptive_rdo_observes_cancellation() {
        use core::sync::atomic::{AtomicUsize, Ordering};

        struct StopAfterRows {
            rows: AtomicUsize,
            limit: usize,
        }
        impl how_far::Stop for StopAfterRows {
            fn check(&self) -> Result<(), how_far::StopReason> {
                if self.rows.load(Ordering::Relaxed) >= self.limit {
                    Err(how_far::StopReason::Cancelled)
                } else {
                    Ok(())
                }
            }
        }

        // 1. Open-loop lambda map stops after 2 rows of a 16-row grid.
        let stop = StopAfterRows {
            rows: AtomicUsize::new(0),
            limit: 2,
        };
        let (bw, bh) = (16usize, 16usize);
        let rows = |_: usize, planes: &mut [f32], _: &mut Vec<(u64, bool)>| {
            stop.rows.fetch_add(1, Ordering::Relaxed);
            planes.fill(0.0);
        };
        let res =
            build_open_loop_lambda_map(&rows, bw * 8, bw, bh, &[[4.0; 64]; NCH], 0.30, 1, &stop);
        assert!(matches!(res, Err(Error::Cancelled)));
        assert_eq!(
            stop.rows.load(Ordering::Relaxed),
            2,
            "open-loop lambda map must stop promptly at cancellation row"
        );

        // 2. End-to-end adaptive_partitions e10 encode observes cancellation mid-flight.
        struct StopAfterChecks {
            checks: AtomicUsize,
            limit: usize,
        }
        impl how_far::Stop for StopAfterChecks {
            fn check(&self) -> Result<(), how_far::StopReason> {
                if self.checks.fetch_add(1, Ordering::Relaxed) >= self.limit {
                    Err(how_far::StopReason::Cancelled)
                } else {
                    Ok(())
                }
            }
        }
        let (w, h) = (160u32, 128u32);
        let px = synthetic_multi_region(w, h);
        let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
        let cfg = LossyConfig::new(60.0)
            .with_effort(10.0)
            .with_adaptive_partitions(true)
            .with_threads(1);
        for limit in [2usize, 25, 60] {
            let stop = StopAfterChecks {
                checks: AtomicUsize::new(0),
                limit,
            };
            let pulse = how_far::WithStop::borrowed(&how_far::NoPulse, &stop);
            let out = crate::lossy::encode_lossy_with_progress(
                &px,
                info.row_bytes(),
                &info,
                &Metadata::default(),
                &cfg,
                &pulse,
            );
            assert!(
                matches!(out, Err(Error::Cancelled)),
                "expected Error::Cancelled at limit={limit}, got {out:?}"
            );
        }
    }

    /// Verifies that `ClosedLoopLambdaModel::apply_in_place` (whole-image path)
    /// and band-by-band `fill_band` (bounded streaming path) produce bit-exact
    /// multipliers under both `Exact` (`<= CLOSED_LOOP_EXACT_MAX_BLOCKS`) and
    /// `Compact` (`> CLOSED_LOOP_EXACT_MAX_BLOCKS`) stores, and that `Compact`
    /// stays closely aligned with `Exact`.
    #[test]
    fn closed_loop_stream_matches_whole_image_exact_and_compact() {
        let bw = 12usize;
        let bh = 10usize;
        let nb = bw * bh;
        let steps = [[1.0f32; 64]; NCH];
        let mut seed = 0x5EED_C105_ED10_0901u64;

        let make_stripe = |by0: usize, nby: usize, seed: &mut u64| -> StripeClosedLoop {
            let n = nby * bw;
            let mut blk_err = vec![0.0f64; n];
            let mut skip_blk = vec![false; n];
            for i in 0..n {
                let u = lcg(seed);
                if i % 17 == 0 {
                    skip_blk[i] = true;
                } else if i % 13 == 0 {
                    blk_err[i] = 0.0;
                } else {
                    blk_err[i] = u * u * 25.0;
                }
            }
            let row_px = bw * 8;
            let fill_row = |seed: &mut u64| -> [Vec<f32>; NCH] {
                core::array::from_fn(|_| (0..row_px).map(|_| (lcg(seed) - 0.5) as f32).collect())
            };
            StripeClosedLoop {
                by0,
                nby,
                blk_err,
                skip_blk,
                top_orig: fill_row(seed),
                top_err: fill_row(seed),
                bot_orig: fill_row(seed),
                bot_err: fill_row(seed),
            }
        };

        let stripes = vec![
            make_stripe(0, 4, &mut seed),
            make_stripe(4, 4, &mut seed),
            make_stripe(8, 2, &mut seed),
        ];

        let open_base: Vec<f32> = (0..nb)
            .map(|i| 0.75 + 0.5 * ((i % 11) as f32 / 10.0))
            .collect();

        let run_mode = |exact_max: usize| -> (Vec<f32>, Vec<f32>) {
            let mut stream =
                ClosedLoopLambdaStream::new_with_exact_max(bw, bh, &steps, 0.38, exact_max)
                    .unwrap();
            for st in &stripes {
                stream
                    .push_stripe(StripeClosedLoop {
                        by0: st.by0,
                        nby: st.nby,
                        blk_err: st.blk_err.clone(),
                        skip_blk: st.skip_blk.clone(),
                        top_orig: st.top_orig.clone(),
                        top_err: st.top_err.clone(),
                        bot_orig: st.bot_orig.clone(),
                        bot_err: st.bot_err.clone(),
                    })
                    .unwrap();
            }
            let model = stream.finish();
            let mut whole = open_base.clone();
            model.apply_in_place(&mut whole, &how_far::NoPulse).unwrap();

            let mut banded = Vec::with_capacity(nb);
            let mut scratch = Vec::new();
            for by0 in (0..bh).step_by(3) {
                let by1 = (by0 + 3).min(bh);
                model
                    .fill_band(&open_base[by0 * bw..by1 * bw], by0, by1, &mut scratch)
                    .unwrap();
                banded.extend_from_slice(&scratch);
            }
            (whole, banded)
        };

        let (exact_whole, exact_banded) = run_mode(usize::MAX);
        let (compact_whole, compact_banded) = run_mode(0);

        for i in 0..nb {
            assert_eq!(
                exact_whole[i].to_bits(),
                exact_banded[i].to_bits(),
                "exact whole vs banded mismatch at block {i}"
            );
            assert_eq!(
                compact_whole[i].to_bits(),
                compact_banded[i].to_bits(),
                "compact whole vs banded mismatch at block {i}"
            );
            let diff = (exact_whole[i] - compact_whole[i]).abs();
            assert!(
                diff < 0.05,
                "compact vs exact diverged at block {i}: exact={} compact={} diff={diff}",
                exact_whole[i],
                compact_whole[i]
            );
        }

        // Verify the production threshold switches cleanly to Compact above
        // `CLOSED_LOOP_EXACT_MAX_BLOCKS` without overflowing.
        let big_bw = 513usize;
        let big_bh = 512usize; // 262,656 > 262,144
        assert!(big_bw * big_bh > CLOSED_LOOP_EXACT_MAX_BLOCKS);
        let big = ClosedLoopLambdaStream::new(big_bw, big_bh, &steps, 0.38).unwrap();
        assert!(matches!(big.store, ClosedLoopStore::Compact(_)));
    }
}
