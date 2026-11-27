//! Open-loop perceptual trellis-lambda adaptive quantization (whole-image
//! parallel builder and bounded-memory streaming model/cursor).

use alloc::vec::Vec;

use super::fallible::{try_vec, try_vec_cap};
use crate::Error;
use crate::lossy::{NCH, RowPlanes};

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
/// block row from color-converted `planes` (`NCH * 8 * pw`, plane-major),
/// observing cancellation inside wide rows.
pub(in crate::lossy) fn open_loop_row_features(
    planes: &[f32],
    pw: usize,
    bw: usize,
    steps: &[[f32; 64]; NCH],
    min_q: &mut [f32],
    mean_q: &mut [f32],
    sat_smooth: &mut [f32],
    stop: &dyn how_far::Stop,
) -> Result<(), Error> {
    let s1_y = steps[0][1].max(1e-4);
    let s1_x = steps[1][1].max(1e-4);
    let s1_b = steps[2][1].max(1e-4);
    for bx in 0..bw {
        if bx & 3 == 0 {
            stop.check().map_err(|_| Error::Cancelled)?;
        }
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
    Ok(())
}

/// Computes raw open-loop perceptual scores for one block row using the 3-row
/// `mean_q` neighborhood `(above, cur_mean, below)`. Returns
/// `(row_active_sum, row_active_n, row_all_sum)`.
pub(in crate::lossy) fn open_loop_row_raw(
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
pub(in crate::lossy) fn build_open_loop_lambda_map(
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
                    stop,
                )?;
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
pub(in crate::lossy) struct OpenLoopLambdaStream {
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
    pub(in crate::lossy) fn new(
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
    pub(in crate::lossy) fn push_row(
        &mut self,
        by: usize,
        planes: &[f32],
        pw: usize,
        steps: &[[f32; 64]; NCH],
    ) {
        debug_assert_eq!(by, self.next_by);
        self.next_by += 1;
        let _ = open_loop_row_features(
            planes,
            pw,
            self.bw,
            steps,
            &mut self.next_min,
            &mut self.next_mean,
            &mut self.next_sat,
            &how_far::NoPulse,
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

    pub(in crate::lossy) fn finish(mut self) -> Result<OpenLoopLambdaModel, Error> {
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
pub(in crate::lossy) struct OpenLoopLambdaModel {
    bw: usize,
    bh: usize,
    work: usize,
    st: f64,
    mean_raw: f64,
    band_first_mean: Vec<f32>,
}

/// Per-pass streaming cursor over [`OpenLoopLambdaModel`] that computes exact
/// per-band `open_lambda_map` (`(by1 - by0) * bw` f32s) sequentially top-to-bottom.
pub(in crate::lossy) struct OpenLoopBandCursor {
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
    pub(in crate::lossy) fn cursor(&self, pw: usize) -> Result<OpenLoopBandCursor, Error> {
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
    pub(in crate::lossy) fn fill_band(
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
            let _ = open_loop_row_features(
                &cursor.planes,
                pw,
                bw,
                steps,
                &mut cursor.min_q[off..off + bw],
                &mut cursor.mean_q[off..off + bw],
                &mut cursor.sat_smooth[off..off + bw],
                &how_far::NoPulse,
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
