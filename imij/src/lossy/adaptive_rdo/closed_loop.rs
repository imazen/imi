//! Closed-loop perceptual + unfiltered-seam residual feedback for trellis
//! lambda modulation (whole-image parallel update and bounded streaming model).

use alloc::vec::Vec;

use super::fallible::{try_vec, try_vec_cap};
use crate::Error;
use crate::lossy::{NCH, RowPlanes, StripeCoefs, dct, small, superblock};

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
                        small::inverse_ref(&mut buf);
                    } else {
                        dct::idct8x8(&mut buf);
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
pub(in crate::lossy) struct ClosedLoopCtx {
    dq8: [[f32; 64]; NCH],
    dq4: [[f32; 64]; NCH],
    sb_tables: superblock::Tables,
    inv_s1_sq: [f64; NCH],
    inv_seam_tau: [f64; NCH],
    y_step1: f32,
}

const CLOSED_LOOP_CH_W: [f64; NCH] = [1.0, 0.5, 0.5];

impl ClosedLoopCtx {
    pub(in crate::lossy) fn new(steps: &[[f32; 64]; NCH]) -> Self {
        Self {
            dq8: steps.map(|t| dct::dequant_factors(&t)),
            dq4: steps.map(|t| small::dequant4(&t)),
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
pub(in crate::lossy) struct StripeClosedLoop {
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
pub(in crate::lossy) fn closed_loop_stripe_errors(
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
pub(in crate::lossy) fn update_lambda_map_closed_loop(
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
pub(in crate::lossy) const CLOSED_LOOP_EXACT_MAX_BLOCKS: usize = 262_144;

const LOG_Q_MIN: f64 = -1.386_294_361_119_890_6; // ln(0.25)
const LOG_Q_MAX: f64 = 6.2;
const LOG_Q_SCALE: f64 = 253.0 / (LOG_Q_MAX - LOG_Q_MIN);

pub(super) enum ClosedLoopStore {
    /// Exact `f64` per block (`NaN` marks `skip_blk`).
    Exact(Vec<f64>),
    /// Compact `u8` log-error per block (`0` = skip, `1` = inactive `e <= 1e-4`,
    /// `2..=255` = uniformly quantized `ln(e + 0.25)` over `[ln(0.25), 6.2]`).
    Compact(Vec<u8>),
}

/// Streaming closed-loop perceptual & unfiltered-seam residual collector for
/// `bounded.rs` (fed during pass `10 + 0`, keeping only a 1-row stripe halo in
/// working RAM).
pub(in crate::lossy) struct ClosedLoopLambdaStream {
    ctx: ClosedLoopCtx,
    image_bw: usize,
    bh: usize,
    feedback_weight: f32,
    pub(super) store: ClosedLoopStore,
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
    pub(in crate::lossy) fn new(
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

    pub(super) fn new_with_exact_max(
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

    pub(in crate::lossy) fn ctx(&self) -> &ClosedLoopCtx {
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
    pub(in crate::lossy) fn push_stripe(&mut self, mut st: StripeClosedLoop) -> Result<(), Error> {
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

    pub(in crate::lossy) fn finish(mut self) -> ClosedLoopLambdaModel {
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
pub(in crate::lossy) struct ClosedLoopLambdaModel {
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
    pub(in crate::lossy) fn apply_in_place(
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
    pub(in crate::lossy) fn fill_band(
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
