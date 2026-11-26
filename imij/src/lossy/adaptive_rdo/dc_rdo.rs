//! Causal 3-state row-Viterbi 2D-MED DC rate-distortion optimizer.
//!
//! Encoder-only, zero wire changes, zero decode overhead. Operates
//! independently per nested DC stripe (`dc_rows` block rows) so whole-image
//! and bounded-memory streaming encodes produce bit-identical DC coefficients.

use alloc::vec::Vec;

use super::fallible::{try_vec, try_vec_cap};
use crate::Error;
use crate::lossy::{NCH, RowPlanes, StripeCoefs};
use crate::mathx::log2;

/// Production parameters for per-DC-stripe 2D-MED DC RDO.
#[derive(Clone, Copy, Debug)]
pub(in crate::lossy) struct DcRdoConfig {
    /// Base distortion weight multiplier (`1.0` matches flat-block AC trellis `64 * lambda(0) = 16.0`).
    pub lambda: f32,
    /// 1-row south vertical MED rate lookahead weight (`0.0` = off).
    pub south_w: f32,
}

impl DcRdoConfig {
    /// Frozen production parameters validated on the `dev` split:
    /// `lambda = 1.5` (`w_base = 24.0`), `south_w = 0.5`, constant-block lock,
    /// and UI/palette stripe skip.
    pub(in crate::lossy) const PRODUCTION: Self = Self {
        lambda: 1.5,
        south_w: 0.5,
    };
}

#[inline]
fn med(w: i32, n: i32, nw: i32) -> i32 {
    let mn = w.min(n);
    let mx = w.max(n);
    if nw >= mx {
        mn
    } else if nw <= mn {
        mx
    } else {
        w + n - nw
    }
}

#[inline]
fn zigzag(r: i32) -> u32 {
    if r >= 0 {
        (r as u32).saturating_mul(2)
    } else {
        ((-r) as u32).saturating_mul(2).saturating_sub(1)
    }
}

#[inline]
fn ctx_bin(sum: u32) -> usize {
    match sum {
        0 => 0,
        1..=2 => 1,
        3..=7 => 2,
        _ => 3,
    }
}

const SYM_BINS: usize = 17;

struct DcRateModel {
    bits: [[f64; SYM_BINS]; 4],
    zero_run_bits: f64,
}

impl DcRateModel {
    fn fit_stripe(q0: &[i32], bw: usize, nr: usize, gbw: usize) -> Result<Self, Error> {
        let mut hist = [[0.5f64; SYM_BINS]; 4];
        let mut tot = [0.5 * SYM_BINS as f64; 4];
        let mut zr_yes = 1.0f64;
        let mut zr_no = 1.0f64;
        let mut prev_u = try_vec(bw, 0u32)?;
        let mut cur_u = try_vec(bw, 0u32)?;
        let gw = if gbw == 0 { bw } else { gbw };
        for by in 0..nr {
            let has_n = by > 0;
            for bx in 0..bw {
                let col_left = bx % gw == 0;
                let idx = by * bw + bx;
                let q = q0[idx];
                let pred = match (!col_left, has_n) {
                    (true, true) => med(q0[idx - 1], q0[idx - bw], q0[idx - bw - 1]),
                    (true, false) => q0[idx - 1],
                    (false, true) => q0[idx - bw],
                    (false, false) => 0,
                };
                let u = zigzag(q.wrapping_sub(pred));
                let uw = if !col_left { cur_u[bx - 1] } else { 0 };
                let un = if has_n { prev_u[bx] } else { 0 };
                let c = ctx_bin(uw.saturating_add(un));
                let b = (u as usize).min(SYM_BINS - 1);
                hist[c][b] += 1.0;
                tot[c] += 1.0;
                if !col_left && uw == 0 {
                    if u == 0 {
                        zr_yes += 1.0;
                    } else {
                        zr_no += 1.0;
                    }
                }
                cur_u[bx] = u;
            }
            core::mem::swap(&mut prev_u, &mut cur_u);
        }
        let mut bits = [[0.0f64; SYM_BINS]; 4];
        for c in 0..4 {
            for b in 0..SYM_BINS {
                bits[c][b] = -log2((hist[c][b] / tot[c]).clamp(1e-6, 1.0));
            }
        }
        let p_cont = (zr_yes / (zr_yes + zr_no)).clamp(0.05, 0.99);
        let zero_run_bits = (-log2(p_cont)).min(bits[0][0]);
        Ok(Self {
            bits,
            zero_run_bits,
        })
    }

    #[inline]
    fn cost(&self, ctx: usize, u: u32, prev_zero: bool) -> f64 {
        if u == 0 && prev_zero {
            return self.zero_run_bits;
        }
        if (u as usize) < SYM_BINS - 1 {
            self.bits[ctx][u as usize]
        } else {
            self.bits[ctx][SYM_BINS - 1] + 2.0 * log2((u - (SYM_BINS as u32 - 2)) as f64 + 1.0)
        }
    }
}

/// Returns `true` if the DC stripe is screen/UI/palette-dominated (either at
/// least 25% of 8x8 blocks are exact constant color across all 3 planes, or
/// the stripe contains at most 24 distinct `(Y, X, B)` quantized DC triples).
fn stripe_is_palette_like(q0: &[&[i32]; NCH], ac_norm: &[&[f32]; NCH]) -> bool {
    let n = q0[0].len();
    let mut flat = 0usize;
    for i in 0..n {
        if ac_norm[0][i] <= 1e-3 && ac_norm[1][i] <= 1e-3 && ac_norm[2][i] <= 1e-3 {
            flat += 1;
        }
    }
    if flat.saturating_mul(4) >= n {
        return true;
    }
    const SLOTS: usize = 64;
    const MASK: usize = SLOTS - 1;
    let mut keys = [[0i32; NCH]; SLOTS];
    let mut used = [false; SLOTS];
    let mut distinct = 0usize;
    for i in 0..n {
        let k = [q0[0][i], q0[1][i], q0[2][i]];
        let mut h = (k[0] as u32 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (k[1] as u32 as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9)
            ^ (k[2] as u32 as u64).wrapping_mul(0x94D0_49BB_1331_11EB);
        h ^= h >> 30;
        let mut slot = (h as usize) & MASK;
        loop {
            if !used[slot] {
                used[slot] = true;
                keys[slot] = k;
                distinct += 1;
                if distinct > 24 {
                    return false;
                }
                break;
            }
            if keys[slot] == k {
                break;
            }
            slot = (slot + 1) & MASK;
        }
    }
    true
}

/// Optimizes one DC stripe (`nr` block rows × `bw` block columns) in place.
/// Pure per-DC-stripe function shared identically by whole-image and bounded
/// streaming encoders.
#[allow(clippy::too_many_arguments)]
pub(in crate::lossy) fn optimize_dc_stripe(
    q_ch: &mut [Vec<i32>; NCH],
    raw_c: &[&[f32]; NCH],
    ac_norm: &[&[f32]; NCH],
    base_lock: &[bool],
    ext_lock: &[bool],
    lambda_slice: &[f32],
    bw: usize,
    nr: usize,
    gbw: usize,
    cfg: DcRdoConfig,
    stop: &dyn how_far::Stop,
) -> Result<(), Error> {
    let n = bw.checked_mul(nr).ok_or(Error::TooLarge)?;
    if n == 0 || cfg.lambda <= 0.0 {
        return Ok(());
    }
    if stripe_is_palette_like(&[&q_ch[0], &q_ch[1], &q_ch[2]], ac_norm) {
        return Ok(());
    }

    let gw = if gbw == 0 { bw } else { gbw };
    let w_base = 16.0 * (cfg.lambda as f64);
    let south_w = cfg.south_w as f64;
    let mut orig = try_vec(n, 0i32)?;
    let mut prev_u = try_vec(bw, 0u32)?;
    let mut cur_u = try_vec(bw, 0u32)?;
    let mut back = try_vec(bw.checked_mul(3).ok_or(Error::TooLarge)?, 1u8)?;

    for ch in 0..NCH {
        stop.check().map_err(|_| Error::Cancelled)?;
        orig.copy_from_slice(&q_ch[ch]);
        let model = DcRateModel::fit_stripe(&orig, bw, nr, gbw)?;
        let q_out = &mut q_ch[ch];
        let rc = raw_c[ch];
        let acn = ac_norm[ch];
        prev_u.fill(0);

        for by in 0..nr {
            stop.check().map_err(|_| Error::Cancelled)?;
            let has_n = by > 0;
            let has_s = by + 1 < nr;
            let row_off = by * bw;
            let mut dp = [0.0f64; 3];
            let mut dp_u = [0u32; 3];

            for bx in 0..bw {
                let gi = row_off + bx;
                let col_left = bx % gw == 0;
                let seg_end = bx + 1 == bw || (bx + 1) % gw == 0;
                let q0 = orig[gi];
                let c = rc[gi] as f64;
                let locked = base_lock[gi]
                    || acn[gi] <= 1e-3
                    || (ch == 0 && ext_lock[gi])
                    || !c.is_finite()
                    || q0.abs() >= 32766;
                let lmul = lambda_slice.get(gi).copied().unwrap_or(1.0).clamp(0.5, 3.0) as f64;
                let w_blk = w_base * lmul;

                let q_n = if has_n { q_out[gi - bw] } else { 0 };
                let u_n = if has_n { prev_u[bx] } else { 0 };
                let q_nw = if has_n && !col_left {
                    q_out[gi - bw - 1]
                } else {
                    0
                };

                let q_s = if has_s { orig[gi + bw] } else { 0 };
                let q_sw = if has_s && !col_left {
                    orig[gi + bw - 1]
                } else {
                    0
                };
                let q_se = if has_s && !seg_end {
                    orig[gi + bw + 1]
                } else {
                    0
                };
                let q_e = if has_s && !seg_end { orig[gi + 1] } else { 0 };
                let south_cost = |q: i32, q_w: i32| -> f64 {
                    if south_w <= 0.0 || !has_s {
                        return 0.0;
                    }
                    let pred_s = if col_left { q } else { med(q_sw, q, q_w) };
                    let u_s = zigzag(q_s.wrapping_sub(pred_s));
                    let mut sc_cost = model.cost(1, u_s, false);
                    if !seg_end {
                        let pred_se = med(q_s, q_e, q);
                        let u_se = zigzag(q_se.wrapping_sub(pred_se));
                        sc_cost += model.cost(1, u_se, u_s == 0);
                    }
                    south_w * sc_cost
                };

                let mut ndp = [f64::INFINITY; 3];
                let mut ndp_u = [0u32; 3];
                for s in 0..3usize {
                    if locked && s != 1 {
                        continue;
                    }
                    let q = q0 + (s as i32 - 1);
                    let err = q as f64 - c;
                    if s != 1 && err.abs() > 1.05 {
                        continue;
                    }
                    let d_node = w_blk * err * err;
                    if col_left {
                        let pred = if has_n { q_n } else { 0 };
                        let u = zigzag(q.wrapping_sub(pred));
                        let rate = model.cost(ctx_bin(u_n), u, false);
                        let sc_delta = if s == 1 {
                            0.0
                        } else {
                            south_cost(q, 0) - south_cost(q0, 0)
                        };
                        ndp[s] = d_node + rate + sc_delta;
                        ndp_u[s] = u;
                        back[bx * 3 + s] = 1;
                    } else {
                        let q0_w = orig[gi - 1];
                        let mut best_c = f64::INFINITY;
                        let mut best_ps = 1u8;
                        let mut best_u = 0u32;
                        for ps in 0..3usize {
                            if !dp[ps].is_finite() {
                                continue;
                            }
                            let q_w = q0_w + (ps as i32 - 1);
                            let pred = if has_n { med(q_w, q_n, q_nw) } else { q_w };
                            let u = zigzag(q.wrapping_sub(pred));
                            let u_w = dp_u[ps];
                            let rate = model.cost(ctx_bin(u_w.saturating_add(u_n)), u, u_w == 0);
                            let sc_delta = south_cost(q, q_w) - south_cost(q0, q0_w);
                            let cost = dp[ps] + d_node + rate + sc_delta;
                            if cost < best_c {
                                best_c = cost;
                                best_ps = ps as u8;
                                best_u = u;
                            }
                        }
                        ndp[s] = best_c;
                        ndp_u[s] = best_u;
                        back[bx * 3 + s] = best_ps;
                    }
                }
                dp = ndp;
                dp_u = ndp_u;

                if seg_end {
                    let seg_start = (bx / gw) * gw;
                    let mut best_s = 1usize;
                    let mut best_c = dp[1];
                    for s in [0usize, 2] {
                        if dp[s] < best_c {
                            best_c = dp[s];
                            best_s = s;
                        }
                    }
                    let mut s = best_s;
                    for x in (seg_start..=bx).rev() {
                        let idx = row_off + x;
                        q_out[idx] = orig[idx] + (s as i32 - 1);
                        s = back[x * 3 + s] as usize;
                    }
                    for x in seg_start..=bx {
                        let idx = row_off + x;
                        let pred = match (x > seg_start, has_n) {
                            (true, true) => {
                                med(q_out[idx - 1], q_out[idx - bw], q_out[idx - bw - 1])
                            }
                            (true, false) => q_out[idx - 1],
                            (false, true) => q_out[idx - bw],
                            (false, false) => 0,
                        };
                        cur_u[x] = zigzag(q_out[idx].wrapping_sub(pred));
                    }
                }
            }
            core::mem::swap(&mut prev_u, &mut cur_u);
        }
    }
    Ok(())
}

/// Computes continuous DC (`raw_dc / dc_step`), Parseval AC energy norm, and
/// extreme-block locks for one block row into pre-allocated slices of length `bw`.
#[allow(clippy::too_many_arguments)]
fn extract_row_dc_stats(
    by: usize,
    rows: RowPlanes<'_>,
    pw: usize,
    bw: usize,
    inv_dc: &[f64; NCH],
    planes: &mut [f32],
    extremes: &mut Vec<(u64, bool)>,
    rc_out: [&mut [f32]; NCH],
    acn_out: [&mut [f32]; NCH],
    ext_out: &mut [bool],
) {
    extremes.clear();
    rows(by, planes, extremes);
    for bx in 0..bw {
        ext_out[bx] = extremes
            .get(bx)
            .is_some_and(|&(white, black)| white == u64::MAX || black);
        for ch in 0..NCH {
            let base = ch * 8 * pw + bx * 8;
            let mut sum = 0.0f64;
            let mut sum_sq = 0.0f64;
            for y in 0..8 {
                let row = &planes[base + y * pw..][..8];
                for &v in row {
                    let vf = v as f64;
                    sum += vf;
                    sum_sq += vf * vf;
                }
            }
            rc_out[ch][bx] = (sum * 0.125 * inv_dc[ch]) as f32;
            acn_out[ch][bx] = (sum_sq - sum * sum * (1.0 / 64.0)).max(0.0) as f32;
        }
    }
}

/// Reserves exact capacity `needed` without geometric doubling if `v` has
/// smaller capacity.
fn reserve_exact_cap<T>(v: &mut Vec<T>, needed: usize) -> Result<(), Error> {
    if v.capacity() < needed {
        v.try_reserve_exact(needed - v.len())
            .map_err(|_| Error::TooLarge)?;
    }
    Ok(())
}

/// Streaming single-DC-stripe accumulator for bounded-memory lossy passes.
///
/// Accumulates bands belonging to one DC stripe (`<= dc_rows * bw` blocks),
/// runs the exact same [`optimize_dc_stripe`] pass as the whole-image encoder,
/// and packs the resulting DC image rows for `v1` measurement or emission.
pub(in crate::lossy) struct DcStripeRdoAcc {
    q: [Vec<i32>; NCH],
    raw_c: [Vec<f32>; NCH],
    ac_norm: [Vec<f32>; NCH],
    base_lock: Vec<bool>,
    ext_lock: Vec<bool>,
    lambda: Vec<f32>,
    aq_code: Vec<i32>,
    rows_acc: usize,
    planes: Vec<f32>,
    extremes: Vec<(u64, bool)>,
}

impl DcStripeRdoAcc {
    pub(in crate::lossy) fn new() -> Self {
        Self {
            q: [Vec::new(), Vec::new(), Vec::new()],
            raw_c: [Vec::new(), Vec::new(), Vec::new()],
            ac_norm: [Vec::new(), Vec::new(), Vec::new()],
            base_lock: Vec::new(),
            ext_lock: Vec::new(),
            lambda: Vec::new(),
            aq_code: Vec::new(),
            rows_acc: 0,
            planes: Vec::new(),
            extremes: Vec::new(),
        }
    }

    /// Pre-reserves exact capacity for one DC stripe (`max_rows * bw` blocks
    /// plus 1 block-row of scratch planes and extremes) so streaming passes
    /// never grow geometrically or retain slack above `plan_working_bytes`.
    pub(in crate::lossy) fn reserve_stripe(
        &mut self,
        max_rows: usize,
        bw: usize,
        pw: usize,
    ) -> Result<(), Error> {
        let cap = max_rows.checked_mul(bw).ok_or(Error::TooLarge)?;
        for ch in 0..NCH {
            reserve_exact_cap(&mut self.q[ch], cap)?;
            reserve_exact_cap(&mut self.raw_c[ch], cap)?;
            reserve_exact_cap(&mut self.ac_norm[ch], cap)?;
        }
        reserve_exact_cap(&mut self.base_lock, cap)?;
        reserve_exact_cap(&mut self.ext_lock, cap)?;
        reserve_exact_cap(&mut self.lambda, cap)?;
        reserve_exact_cap(&mut self.aq_code, cap)?;
        let plane_len = NCH
            .checked_mul(8)
            .and_then(|v| v.checked_mul(pw))
            .ok_or(Error::TooLarge)?;
        if self.planes.len() < plane_len {
            reserve_exact_cap(&mut self.planes, plane_len)?;
            self.planes.resize(plane_len, 0.0);
        }
        reserve_exact_cap(&mut self.extremes, bw)?;
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn block_capacity(&self) -> usize {
        self.base_lock.capacity()
    }

    pub(in crate::lossy) fn clear(&mut self) {
        for ch in 0..NCH {
            self.q[ch].clear();
            self.raw_c[ch].clear();
            self.ac_norm[ch].clear();
        }
        self.base_lock.clear();
        self.ext_lock.clear();
        self.lambda.clear();
        self.aq_code.clear();
        self.rows_acc = 0;
    }

    /// Appends one work band `[by0, by1)` to the active DC stripe accumulator.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::lossy) fn push_band(
        &mut self,
        stripes: &[StripeCoefs],
        rows: RowPlanes<'_>,
        pw: usize,
        bw: usize,
        by0: usize,
        by1: usize,
        steps: &[[f32; 64]; NCH],
        band_open_lambda: &[f32],
        has_exact: bool,
        skip_on: bool,
        stop: &dyn how_far::Stop,
    ) -> Result<(), Error> {
        let nr = by1 - by0;
        let add = nr.checked_mul(bw).ok_or(Error::TooLarge)?;
        let old_len = self.base_lock.len();
        let new_len = old_len.checked_add(add).ok_or(Error::TooLarge)?;
        for ch in 0..NCH {
            reserve_exact_cap(&mut self.q[ch], new_len)?;
            self.q[ch].resize(new_len, 0);
            reserve_exact_cap(&mut self.raw_c[ch], new_len)?;
            self.raw_c[ch].resize(new_len, 0.0);
            reserve_exact_cap(&mut self.ac_norm[ch], new_len)?;
            self.ac_norm[ch].resize(new_len, 0.0);
        }
        reserve_exact_cap(&mut self.base_lock, new_len)?;
        self.base_lock.resize(new_len, false);
        reserve_exact_cap(&mut self.ext_lock, new_len)?;
        self.ext_lock.resize(new_len, false);
        reserve_exact_cap(&mut self.lambda, new_len)?;
        if band_open_lambda.len() >= add {
            self.lambda.extend_from_slice(&band_open_lambda[..add]);
        } else {
            self.lambda.resize(new_len, 1.0);
        }
        reserve_exact_cap(&mut self.aq_code, new_len)?;
        self.aq_code.resize(new_len, 0);

        for sc in stripes {
            for r in 0..sc.nby {
                for bx in 0..sc.bw {
                    let b = r * sc.bw + bx;
                    let idx = old_len + (sc.by0 + r - by0) * bw + bx;
                    let merged = sc.modes.get(b).is_some_and(|&m| m != 0);
                    let bypassed = if has_exact {
                        sc.aq.get(b) == Some(&1)
                    } else if skip_on {
                        sc.aq.get(b) == Some(&0)
                    } else {
                        false
                    };
                    self.base_lock[idx] = merged || bypassed;
                    if !sc.aq.is_empty() {
                        self.aq_code[idx] = sc.aq[b] as i32;
                    }
                    for ch in 0..NCH {
                        self.q[ch][idx] = sc.q[(b * NCH + ch) * 64];
                    }
                }
            }
        }

        let plane_len = NCH
            .checked_mul(8)
            .and_then(|v| v.checked_mul(pw))
            .ok_or(Error::TooLarge)?;
        if self.planes.len() < plane_len {
            self.planes
                .try_reserve_exact(plane_len - self.planes.len())
                .map_err(|_| Error::TooLarge)?;
            self.planes.resize(plane_len, 0.0);
        }
        if self.extremes.capacity() < bw {
            self.extremes
                .try_reserve_exact(bw - self.extremes.len())
                .map_err(|_| Error::TooLarge)?;
        }
        let inv_dc: [f64; NCH] = core::array::from_fn(|ch| 1.0 / (steps[ch][0] as f64));
        let [rc0, rc1, rc2] = &mut self.raw_c;
        let [an0, an1, an2] = &mut self.ac_norm;
        for r in 0..nr {
            stop.check().map_err(|_| Error::Cancelled)?;
            let off = old_len + r * bw;
            extract_row_dc_stats(
                by0 + r,
                rows,
                pw,
                bw,
                &inv_dc,
                &mut self.planes,
                &mut self.extremes,
                [
                    &mut rc0[off..off + bw],
                    &mut rc1[off..off + bw],
                    &mut rc2[off..off + bw],
                ],
                [
                    &mut an0[off..off + bw],
                    &mut an1[off..off + bw],
                    &mut an2[off..off + bw],
                ],
                &mut self.ext_lock[off..off + bw],
            );
        }
        self.rows_acc += nr;
        Ok(())
    }

    /// Runs [`optimize_dc_stripe`] on the accumulated DC stripe and returns
    /// whether every resulting DC value fits in signed 16-bit (`-32768..=32767`).
    pub(in crate::lossy) fn finish_stripe(
        &mut self,
        bw: usize,
        gbw: usize,
        stop: &dyn how_far::Stop,
    ) -> Result<bool, Error> {
        optimize_dc_stripe(
            &mut self.q,
            &[&self.raw_c[0], &self.raw_c[1], &self.raw_c[2]],
            &[&self.ac_norm[0], &self.ac_norm[1], &self.ac_norm[2]],
            &self.base_lock,
            &self.ext_lock,
            &self.lambda,
            bw,
            self.rows_acc,
            gbw,
            DcRdoConfig::PRODUCTION,
            stop,
        )?;
        Ok((0..NCH).all(|ch| self.q[ch].iter().all(|&v| (-32768..=32767).contains(&v))))
    }

    /// Packs the finalized DC stripe into `v1` nested image layout (`U16` with
    /// `+32768` bias or native-endian `I32`, 3 or 4 channels).
    pub(in crate::lossy) fn pack_dc(
        &self,
        bw: usize,
        fits16: bool,
        codes: bool,
        out: &mut Vec<u8>,
    ) -> Result<(), Error> {
        let dcs = if fits16 { 2 } else { 4 };
        let dch = if codes { 4 } else { NCH };
        let n = self.rows_acc * bw;
        let bytes = n
            .checked_mul(dch)
            .and_then(|v| v.checked_mul(dcs))
            .ok_or(Error::TooLarge)?;
        out.clear();
        if out.capacity() < bytes {
            out.try_reserve_exact(bytes - out.len())
                .map_err(|_| Error::TooLarge)?;
        }
        out.resize(bytes, 0);
        for i in 0..n {
            let o0 = i * dch * dcs;
            for ch in 0..NCH {
                let v = self.q[ch][i];
                let o = o0 + ch * dcs;
                if fits16 {
                    out[o..o + 2].copy_from_slice(&((v + 32768) as u16).to_ne_bytes());
                } else {
                    out[o..o + 4].copy_from_slice(&v.to_ne_bytes());
                }
            }
            if codes {
                let v = self.aq_code[i];
                let o = o0 + 3 * dcs;
                if fits16 {
                    out[o..o + 2].copy_from_slice(&(v as u16).to_ne_bytes());
                } else {
                    out[o..o + 4].copy_from_slice(&v.to_ne_bytes());
                }
            }
        }
        Ok(())
    }
}

/// Optimizes quantized DC coefficients in place across `stripes`, processing
/// each nested DC stripe independently in parallel (thread-count invariant and
/// bit-identical to bounded streaming).
#[allow(clippy::too_many_arguments)]
pub(in crate::lossy) fn optimize_dc(
    stripes: &mut [StripeCoefs],
    rows: RowPlanes<'_>,
    pw: usize,
    bw: usize,
    bh: usize,
    dc_rows: usize,
    gbw: usize,
    steps: &[[f32; 64]; NCH],
    lambda_map: &[f32],
    has_exact: bool,
    skip_on: bool,
    cfg: DcRdoConfig,
    threads: usize,
    stop: &dyn how_far::Stop,
) -> Result<(), Error> {
    stop.check().map_err(|_| Error::Cancelled)?;
    let nb = bw.checked_mul(bh).ok_or(Error::TooLarge)?;
    if nb == 0 || cfg.lambda <= 0.0 {
        return Ok(());
    }
    let inv_dc: [f64; NCH] = core::array::from_fn(|ch| 1.0 / (steps[ch][0] as f64));
    let plane_len = NCH
        .checked_mul(8)
        .and_then(|v| v.checked_mul(pw))
        .ok_or(Error::TooLarge)?;
    let dcr = if dc_rows == 0 { bh } else { dc_rows };
    let n_dc_stripes = bh.div_ceil(dcr);

    let mut q_all = [try_vec(nb, 0i32)?, try_vec(nb, 0i32)?, try_vec(nb, 0i32)?];
    let mut base_lock = try_vec(nb, false)?;
    for sc in stripes.iter() {
        for r in 0..sc.nby {
            for bx in 0..sc.bw {
                let b = r * sc.bw + bx;
                let gi = (sc.by0 + r) * bw + bx;
                let merged = sc.modes.get(b).is_some_and(|&m| m != 0);
                let bypassed = if has_exact {
                    sc.aq.get(b) == Some(&1)
                } else if skip_on {
                    sc.aq.get(b) == Some(&0)
                } else {
                    false
                };
                base_lock[gi] = merged || bypassed;
                for ch in 0..NCH {
                    q_all[ch][gi] = sc.q[(b * NCH + ch) * 64];
                }
            }
        }
    }

    let mut ds_indices = try_vec_cap(n_dc_stripes)?;
    ds_indices.extend(0..n_dc_stripes);
    let optimized: Vec<Result<(usize, usize, [Vec<i32>; NCH]), Error>> =
        crate::par::par_map(ds_indices, threads, |_, dsi| {
            let r0 = dsi * dcr;
            let r1 = (r0 + dcr).min(bh);
            let nr = r1 - r0;
            let n = nr.checked_mul(bw).ok_or(Error::TooLarge)?;
            let mut rc = [
                try_vec(n, 0.0f32)?,
                try_vec(n, 0.0f32)?,
                try_vec(n, 0.0f32)?,
            ];
            let mut acn = [
                try_vec(n, 0.0f32)?,
                try_vec(n, 0.0f32)?,
                try_vec(n, 0.0f32)?,
            ];
            let mut ext_lock = try_vec(n, false)?;
            let mut planes = try_vec(plane_len, 0.0f32)?;
            let mut extremes = try_vec_cap(bw)?;
            {
                let [rc0, rc1, rc2] = &mut rc;
                let [an0, an1, an2] = &mut acn;
                for r in 0..nr {
                    stop.check().map_err(|_| Error::Cancelled)?;
                    let off = r * bw;
                    extract_row_dc_stats(
                        r0 + r,
                        rows,
                        pw,
                        bw,
                        &inv_dc,
                        &mut planes,
                        &mut extremes,
                        [
                            &mut rc0[off..off + bw],
                            &mut rc1[off..off + bw],
                            &mut rc2[off..off + bw],
                        ],
                        [
                            &mut an0[off..off + bw],
                            &mut an1[off..off + bw],
                            &mut an2[off..off + bw],
                        ],
                        &mut ext_lock[off..off + bw],
                    );
                }
            }
            let off0 = r0 * bw;
            let off1 = r1 * bw;
            let mut q_ds = [try_vec(n, 0i32)?, try_vec(n, 0i32)?, try_vec(n, 0i32)?];
            for ch in 0..NCH {
                q_ds[ch].copy_from_slice(&q_all[ch][off0..off1]);
            }
            let lam_slice = if lambda_map.len() >= off1 {
                &lambda_map[off0..off1]
            } else {
                &[]
            };
            optimize_dc_stripe(
                &mut q_ds,
                &[&rc[0], &rc[1], &rc[2]],
                &[&acn[0], &acn[1], &acn[2]],
                &base_lock[off0..off1],
                &ext_lock,
                lam_slice,
                bw,
                nr,
                gbw,
                cfg,
                stop,
            )?;
            Ok((r0, nr, q_ds))
        });

    for res in optimized {
        let (r0, nr, q_ds) = res?;
        for ch in 0..NCH {
            q_all[ch][r0 * bw..(r0 + nr) * bw].copy_from_slice(&q_ds[ch]);
        }
    }

    for sc in stripes.iter_mut() {
        for r in 0..sc.nby {
            for bx in 0..sc.bw {
                let b = r * sc.bw + bx;
                let gi = (sc.by0 + r) * bw + bx;
                for ch in 0..NCH {
                    let v = q_all[ch][gi];
                    sc.q[(b * NCH + ch) * 64] = v;
                    let m = &mut sc.nz[b * NCH + ch];
                    *m = (*m & !1) | u64::from(v != 0);
                }
            }
        }
    }
    Ok(())
}
