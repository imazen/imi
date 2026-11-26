//! Split-Huffman trellis dynamic program: per-group width and magnitude
//! selection under frozen [`HufPrices`].

use super::LossyTrellis;
use super::rates::{HufPrices, Powers, WID_PAD};
use super::rl_dp::*;
use crate::lossy::*;
use archmage::prelude::*;
use coef::PK_GROUPS;

/// Group count of an AC support mask (`nz & !1`), as split-Huffman codes
/// it.
#[inline]
pub(super) fn group_count(ac: u64) -> u32 {
    support(ac).div_ceil(4)
}

/// Split-Huffman widths of a block channel's coded groups. The width is
/// the max magnitude's bit length, which is also the OR's top bit — one
/// reduction instead of a max chain.
pub(super) fn group_widths(q: &[i32], n: u32) -> [u8; PK_GROUPS] {
    let mut w = [0u8; PK_GROUPS];
    for (gi, wi) in w.iter_mut().enumerate().take(n as usize) {
        let mut m = 0u32;
        for j in 0..coef::pk_len(gi) {
            m |= q[ZIGZAG[1 + 4 * gi + j] as usize].unsigned_abs();
        }
        *wi = (32 - m.leading_zeros()) as u8;
    }
    w
}

/// `huf_channel`'s `za`/`nf` fill: `za[k] = lw * a[k] * a[k]` and
/// `nf[k] = near[k] as f64`, four lanes at a time — the same per-lane
/// expression, so each lane holds a bitwise-identical value.
/// A `#[magetypes]` body: ops must live inside a generated tier so the
/// backend calls compile under that tier's target features (a plain
/// generic fn would compile baseline and call every op out-of-line).
#[magetypes(define(f64x4), v3, neon, wasm128, scalar)]
fn za_nf(
    token: Token,
    lw: f64,
    a: &[f64; 64],
    near: &[u32; 64],
    za: &mut [f64; 64],
    nf: &mut [f64; 64],
) {
    let lwv = f64x4::splat_t(token, lw);
    for k in (1..61).step_by(4) {
        let av = f64x4::load_t(token, a[k..].first_chunk::<4>().unwrap());
        (lwv * av * av).store(za[k..].first_chunk_mut::<4>().unwrap());
        f64x4::from_array_t(
            token,
            [
                near[k] as f64,
                near[k + 1] as f64,
                near[k + 2] as f64,
                near[k + 3] as f64,
            ],
        )
        .store(nf[k..].first_chunk_mut::<4>().unwrap());
    }
    for k in 61..64 {
        za[k] = lw * a[k] * a[k];
        nf[k] = near[k] as f64;
    }
}

/// `lo[w] = 2^(w-1)`, `cap[w] = 2^w - 1` (width 0 patched to width 1's
/// values, as `at` does): the width bounds `row`/`vals` need per
/// lane, tabulated once instead of rebuilt from scalar shifts.
static LO: [f64; WID_PAD] = {
    let mut t = [0f64; WID_PAD];
    let mut w = 0;
    while w < WID_PAD {
        let wc = if w == 0 { 1 } else { w };
        t[w] = (1u64 << (wc - 1)) as f64;
        w += 1;
    }
    t
};
static CAP: [f64; WID_PAD] = {
    let mut t = [0f64; WID_PAD];
    let mut w = 0;
    while w < WID_PAD {
        let wc = if w == 0 { 1 } else { w };
        t[w] = ((1u64 << wc) - 1) as f64;
        w += 1;
    }
    t
};

/// `huf_channel`'s per-group cost-row build: widths `0..4*NV` of group
/// `gi`, computed with the same per-lane expression and j-order
/// accumulation `at` runs, but hoisting each lane's `nf`/`a`/`za`
/// broadcasts across all width batches instead of once per batch. Lane
/// `w = 0` and lanes past `ends` are patched by the caller exactly as the
/// scalar row fill did.
#[magetypes(define(f64x4), v3, neon, wasm128, scalar)]
fn width_row<const NV: usize>(
    token: Token,
    lw: f64,
    nf: &[f64; 64],
    a: &[f64; 64],
    za: &[f64; 64],
    bias: Option<&[[f32; 64]; 2]>,
    gi: usize,
) -> [f64; WID_PAD] {
    let lwv = f64x4::splat_t(token, lw);
    let one = f64x4::splat_t(token, 1.0);
    let two = f64x4::splat_t(token, 2.0);
    let len = coef::pk_len(gi);
    static WV: [f64; WID_PAD] = {
        let mut t = [0f64; WID_PAD];
        let mut w = 0;
        while w < WID_PAD {
            t[w] = w as f64;
            w += 1;
        }
        t
    };
    let lenv = f64x4::splat_t(token, len as f64);
    let (mut acc, mut lov, mut capv, mut reached, mut lift) = (
        [f64x4::zero_t(token); NV],
        [f64x4::zero_t(token); NV],
        [f64x4::zero_t(token); NV],
        [f64x4::zero_t(token); NV],
        [f64x4::splat_t(token, f64::INFINITY); NV],
    );
    for k in 0..NV {
        lov[k] = f64x4::load_t(token, LO[4 * k..].first_chunk::<4>().unwrap());
        capv[k] = f64x4::load_t(token, CAP[4 * k..].first_chunk::<4>().unwrap());
        let wv = f64x4::load_t(token, WV[4 * k..].first_chunk::<4>().unwrap());
        acc[k] = lenv * wv;
    }
    for j in 0..len {
        let k = 1 + 4 * gi + j;
        let nv = f64x4::splat_t(token, nf[k]);
        let av = f64x4::splat_t(token, a[k]);
        let z = f64x4::splat_t(token, za[k]);
        let b1 = f64x4::splat_t(token, bias.map_or(0.0, |t| t[0][ZIGZAG[k] as usize] as f64));
        let b2 = f64x4::splat_t(token, bias.map_or(0.0, |t| t[1][ZIGZAG[k] as usize] as f64));
        for i in 0..NV {
            // m = n.clamp(1, cap); nzc = lw * (m - B(m) - a)^2 + 1;
            // free = min(nzc, z); B(m) = b1 for |m| = 1, b2 above.
            let m = nv.max(one).min(capv[i]);
            let bv = f64x4::blend(m.simd_ge(two), b2, b1);
            let dm = m - bv - av;
            let nzc = lwv * dm * dm + one;
            let free = nzc.min(z);
            acc[i] += free;
            // reached |= (nzc < z) & (m >= lo)
            reached[i] |= nzc.simd_lt(z) & m.simd_ge(lov[i]);
            let high = nv.max(lov[i]).min(capv[i]);
            let bvh = f64x4::blend(high.simd_ge(two), b2, b1);
            let dh = high - bvh - av;
            let extra = (lwv * dh * dh + one) - free;
            lift[i] = lift[i].min(extra);
        }
    }
    // cost += reached ? 0 : lift — blend picks per lane on the mask.
    let mut out = [f64::INFINITY; WID_PAD];
    for i in 0..NV {
        (acc[i] + f64x4::blend(reached[i], f64x4::zero_t(token), lift[i]))
            .store(out[4 * i..].first_chunk_mut::<4>().unwrap());
    }
    out
}

/// `huf_channel`'s width scan: `min_w cost[w] + row[w]` over `from..end`
/// with the scalar's first-argmin tie-break. `cost` and `row` are
/// [`WID_PAD`]-lane (INFINITY past `end` / `WID_ALPHA`), so six whole
/// vectors cover every candidate width and narrow groups simply touch
/// fewer of them; only lane 0 may need masking (`from <= 1`).
#[magetypes(define(f64x4), v3, neon, wasm128, scalar)]
fn wid_argmin(
    token: Token,
    from: usize,
    end: usize,
    cost: &[f64; WID_PAD],
    row: &[f64; WID_PAD],
) -> (f64, usize) {
    debug_assert!(from <= 1);
    // Narrow groups end inside the first vectors; the INFINITY padding
    // can't win, so the scan stops at `end`'s last vector.
    let nv = if end <= 4 {
        1
    } else if end <= 8 {
        2
    } else if end <= 16 {
        4
    } else {
        6
    };
    let mut vs = [f64x4::zero_t(token); 6];
    for (k, v) in vs.iter_mut().enumerate().take(nv) {
        *v = f64x4::load_t(token, cost[k * 4..].first_chunk::<4>().unwrap())
            + f64x4::load_t(token, row[k * 4..].first_chunk::<4>().unwrap());
    }
    if from == 1 {
        let mut a0 = vs[0].to_array();
        a0[0] = f64::INFINITY;
        vs[0] = f64x4::from_array_t(token, a0);
    }
    // `min` is order-free on these finite costs (never NaN, never -0.0:
    // distortion and bit prices are sums of nonnegatives).
    let mut mv = vs[0];
    for v in &vs[1..nv] {
        mv = mv.min(*v);
    }
    let m = mv.reduce_min();
    // First lane equal to the min — the scalar scan's strict `<` result.
    for (k, v) in vs.iter().enumerate().take(nv) {
        for (l, &x) in v.to_array().iter().enumerate() {
            if x == m {
                return (m, k * 4 + l);
            }
        }
    }
    (m, 0) // unreachable: `m` is one of the lanes
}

/// `at(gi, w).1` for the apply pass: the group's four magnitudes at width
/// `w >= 1` in one vector, with the scalar's lift of the cheapest lane to
/// `2^(w-1)` when nothing reaches it. Lanes past `pk_len` stay zero.
#[magetypes(define(f64x4), v3, neon, wasm128, scalar)]
fn vals(
    token: Token,
    lw: f64,
    nf: &[f64; 64],
    a: &[f64; 64],
    za: &[f64; 64],
    bias: Option<&[[f32; 64]; 2]>,
    gi: usize,
    w: usize,
) -> [u32; 4] {
    if w == 0 {
        return [0; 4];
    }
    let len = coef::pk_len(gi);
    let k0 = 1 + 4 * gi;
    let lwv = f64x4::splat_t(token, lw);
    let one = f64x4::splat_t(token, 1.0);
    let two = f64x4::splat_t(token, 2.0);
    let lo = f64x4::splat_t(token, LO[w]);
    let cap = f64x4::splat_t(token, CAP[w]);
    let lenf = f64x4::splat_t(token, len as f64);
    // Full groups take one load; the short tail group copies its lanes.
    let load = |s: &[f64; 64]| {
        if len == 4 {
            f64x4::load_t(token, s[k0..].first_chunk::<4>().unwrap())
        } else {
            let mut u = [0f64; 4];
            u[..len].copy_from_slice(&s[k0..k0 + len]);
            f64x4::from_array_t(token, u)
        }
    };
    let nv = load(nf);
    let av = load(a);
    let z = load(za);
    let live = f64x4::from_array_t(token, [0.0, 1.0, 2.0, 3.0]).simd_lt(lenf);
    // m = n.clamp(1, cap); nzc = lw * (m - B(m) - a)^2 + 1;
    // vals = nzc < z ? m : 0
    let load_bias = |band: usize| {
        let mut u = [0f64; 4];
        if let Some(t) = bias {
            for (dst, &zz) in u[..len].iter_mut().zip(&ZIGZAG[k0..k0 + len]) {
                *dst = t[band][zz as usize] as f64;
            }
        }
        f64x4::from_array_t(token, u)
    };
    let b1v = load_bias(0);
    let b2v = load_bias(1);
    let m = nv.max(one).min(cap);
    let bv = f64x4::blend(m.simd_ge(two), b2v, b1v);
    let dm = m - bv - av;
    let nzc = lwv * dm * dm + one;
    let keep = nzc.simd_lt(z) & live;
    let free = nzc.min(z);
    let vals_v = m & keep;
    // reached |= keep & (m >= lo); high = n.clamp(lo, cap)
    let reached = keep & m.simd_ge(lo);
    let high = nv.max(lo).min(cap);
    let bvh = f64x4::blend(high.simd_ge(two), b2v, b1v);
    let dh = high - bvh - av;
    let extra = f64x4::blend(
        live,
        (lwv * dh * dh + one) - free,
        f64x4::splat_t(token, f64::INFINITY),
    );
    let mut out = vals_v.to_array();
    if reached.to_array().iter().all(|x| x.to_bits() == 0) {
        // Lift the first argmin lane of `extra`, same as the scalar's
        // strict scan (compare-mask lanes are all-zero when false).
        let mn = extra.reduce_min();
        let j = extra.to_array().iter().position(|&x| x == mn).unwrap_or(0);
        out[j] = high.to_array()[j];
    }
    out.map(|x| x as u32)
}

/// Split-Huffman trellis of one block channel: the group count and, per
/// coded group, the width and its coefficients, minimizing distortion plus
/// bits under frozen code lengths. At width `w >= 1` a group costs its
/// width token, `len * w` magnitude bits and a sign bit per nonzero value,
/// and its largest magnitude must have bit length `w` (the writer derives
/// the width from the values): each coefficient takes zero or its nearest
/// value within `2^w - 1`, and when none reaches `2^(w-1)` the cheapest one
/// is raised to it. Widths up to one above the nearest values' width are
/// searched; the last coded group has `w >= 1` (so the count is the
/// writer's). `count_bits[n]`: the count token and the tokens whose
/// context it sets; `ctx(gi, n)`: group gi's width context under count n
/// and the saturated distance that context encodes.
/// Distortion, lambda and the AQ step scaling are the run-level trellis's.
/// Returns the count and each coded group's width (the values
/// `stripe_counts_widths` reads back from the written `q`/`nz`).
#[magetypes(v3, neon, wasm128, scalar)]
#[allow(clippy::too_many_arguments)]
fn huf_channel_norm(
    token: Token,
    src: &[f32],
    steps: &[f32; 64],
    set: &LossyTrellis,
    pw: Powers,
    norm_override: Option<f64>,
    lambda_mul: f64,
    count_bits: &[f64; PK_GROUPS + 1],
    wid: &[[f64; WID_PAD]; huf::WID_CTX],
    ctx: impl Fn(usize, u32) -> (usize, usize),
    bias: Option<&[[f32; 64]; 2]>,
    q: &mut [i32],
    nz: &mut u64,
) -> (u32, [u8; PK_GROUPS]) {
    let mut a = [0f64; 64];
    let mut near = [0u32; 64];
    let norm =
        incant!(setup(src, steps, &mut a, &mut near) with token, [v3, neon, wasm128, scalar]);
    let norm = norm_override.unwrap_or(norm);
    let lw = 64.0 * set.lambda_p(norm * 64.0 / 63.0, pw) * lambda_mul;
    // `q`'s AC lanes are zero wherever `nz`'s AC bits are: every
    // `StripeCoefs` writer keeps them paired (the quantizer masks come
    // from `q`, and the lab LF prediction keeps them in step). The
    // fast path and the conditional clear below both rely on it.
    debug_assert!(
        *nz & !1 != 0 || (1..64).all(|k| q[ZIGZAG[k] as usize] == 0),
        "nz has no AC bits but q still holds AC coefficients"
    );
    if *nz & !1 == 0 && zero_wins(&near, count_bits) {
        return zero_out();
    }
    // `za[k] = d(0)` (`lw * a * a`, the scalar's `lw * (0 - a) * (0 - a)`)
    // and `nf[k]` share the per-width evaluations below.
    let mut za = [0f64; 64];
    let mut nf = [0f64; 64];
    incant!(za_nf(lw, &a, &near, &mut za, &mut nf) with token, [v3, neon, wasm128, scalar]);
    // Same `za` lanes in the same order the `lw * a * a` map produced.
    let mut zero = [0f64; PK_GROUPS];
    for (gi, z) in zero.iter_mut().enumerate() {
        *z = za[1 + 4 * gi..1 + 4 * gi + coef::pk_len(gi)].iter().sum();
    }
    // Per group, built when the count search first reaches it: the costs of
    // widths 0..ends[gi] (0 = not built yet); lanes past that are INFINITY
    // so the V3 scan can't pick them.
    let mut cost = [[f64::INFINITY; WID_PAD]; PK_GROUPS];
    let mut ends = [0usize; PK_GROUPS];
    // The cheapest width of group gi, memoized by (width context, last): a
    // group sees at most five contexts (luma: min(n - gi, 5)), chroma one.
    // The last coded group searches from width 1. `s` equals `key % 12`
    // (keys are 12 * base + 2 * dist + last, dist <= 5) — a perfect hash,
    // so one probe suffices; a collision would only recompute.
    // The entry packs key << 5 | width; a hit recomputes `cost + row` —
    // the same operands, bitwise the same value the scan stored.
    const NO_KEY: u16 = u16::MAX;
    let mut memo = [[NO_KEY; 12]; PK_GROUPS];
    let mut choose = |gi: usize, n: u32| -> (f64, usize) {
        let from = usize::from(gi + 1 == n as usize);
        let (c, d) = ctx(gi, n);
        let key = c * 2 + from;
        let s = 2 * d + from;
        let row = &wid[c];
        let e = memo[gi][s];
        if e != NO_KEY && (e >> 5) as usize == key {
            let w = (e & 31) as usize;
            return (cost[gi][w] + row[w], w);
        }
        if ends[gi] == 0 {
            // The max magnitude's bit length is the OR's top bit.
            let mut top = 0u32;
            for j in 0..coef::pk_len(gi) {
                top |= near[1 + 4 * gi + j];
            }
            let wn = (32 - top.leading_zeros()) as usize;
            ends[gi] = (wn + 2).min(huf::WID_ALPHA);
            if top == 0 {
                // All-zero group: only widths 0 and 1 are in range, and
                // width 1's cost has a closed form — every lane has
                // a < 0.5, so m = 1 always loses to 0 (free = za) and the
                // group's lift is the min of `d(1) + 1 - za` per lane.
                // The adds run in `at`'s lane order, bit for bit.
                let len = coef::pk_len(gi);
                let mut c = len as f64;
                let mut lift = f64::INFINITY;
                for j in 0..len {
                    let k = 1 + 4 * gi + j;
                    c += za[k];
                    let b = bias.map_or(0.0, |t| t[0][ZIGZAG[k] as usize] as f64);
                    let e = 1.0 - b - a[k];
                    let extra = lw * e * e + 1.0 - za[k];
                    if extra < lift {
                        lift = extra;
                    }
                }
                cost[gi][1] = c + lift;
                cost[gi][0] = zero[gi];
            } else {
                // The cost-row build computes the widths `0..4*NV` with
                // `at`'s per-lane expression; lane 0 (w = 0) and lanes past
                // `ends[gi]` are masked afterward exactly as the scalar row
                // fill left them.
                cost[gi] = match ends[gi] {
                    e if e <= 4 => {
                        incant!(width_row::<1>(lw, &nf, &a, &za, bias, gi) with token, [v3, neon, wasm128, scalar])
                    }
                    e if e <= 8 => {
                        incant!(width_row::<2>(lw, &nf, &a, &za, bias, gi) with token, [v3, neon, wasm128, scalar])
                    }
                    e if e <= 16 => {
                        incant!(width_row::<4>(lw, &nf, &a, &za, bias, gi) with token, [v3, neon, wasm128, scalar])
                    }
                    _ => {
                        incant!(width_row::<6>(lw, &nf, &a, &za, bias, gi) with token, [v3, neon, wasm128, scalar])
                    }
                };
                cost[gi][0] = zero[gi];
                // Whole-batch writes can leave lanes past `ends[gi]`
                // holding real costs; re-mask them for the width scan.
                cost[gi][ends[gi]..].fill(f64::INFINITY);
            }
        }
        let mut best = (f64::INFINITY, 0);
        if ends[gi] == 2 {
            // Two live lanes: the `from..2` scan unrolled.
            let v = cost[gi][from] + row[from];
            if v < best.0 {
                best = (v, from);
            }
            if from == 0 {
                let v = cost[gi][1] + row[1];
                if v < best.0 {
                    best = (v, 1);
                }
            }
        } else {
            best = incant!(
                wid_argmin(from, ends[gi], &cost[gi], row) with token,
                [v3, neon, wasm128, scalar]
            );
        }
        memo[gi][s] = ((key << 5) | best.1) as u16;
        best
    };
    // Every count: its tokens, the coded groups, the zeroed tail.
    // `choose(gi, n)` for `n - gi >= 5` always resolves to the group's
    // distance-5 context with `from = 0` — one memo entry — so that value
    // is cached per group and added directly. The accumulation adds the
    // same values in the same order, bit for bit.
    let mut c5 = [(0f64, 0usize); PK_GROUPS];
    let mut c5ok = [false; PK_GROUPS];
    // `choose(gi, n) >= lb[gi]` for every n: width 0 costs `zero[gi]`, any
    // `w >= 1` already carries `len * w >= len` in `cost`. The suffix sums
    // bound the work a group loop still owes, so a count can quit before
    // evaluating a `choose` it cannot repay.
    //
    // What keeps a real winner safe is a margin of at least one bit, not
    // the slack constant: every coded group's `choose` also pays a width
    // code of at least 1 bit (`HufPrices` lengths are >= 1 or `huf::LOG`),
    // so a pruned count trails the best by >= 1 bit per remaining group,
    // while `count_bits` is integer-valued. The margin dominates the
    // rounding the `lbs`/`tail` subtractions accrue (a few ulps of the
    // totals) for every total under about 2^51 — and with a normalized
    // lambda (s2 > 0, every effort-selected setting) `sum lw*a^2` stays
    // under ~2^45 even at the smallest steps. Not proven: totals past
    // 2^52 via lab s2 = 0 with huge-magnitude input or a caller's tiny
    // `channel_mul`; a million directed cases and a 3,024-encode grid
    // found no counterexample there. The `1e-6` slack guards the bound's
    // own construction below those magnitudes.
    let mut lb = [0f64; PK_GROUPS];
    for (gi, l) in lb.iter_mut().enumerate() {
        *l = zero[gi].min(coef::pk_len(gi) as f64);
    }
    let mut lbs = [0f64; PK_GROUPS + 1];
    for gi in (0..PK_GROUPS).rev() {
        lbs[gi] = lbs[gi + 1] + lb[gi];
    }
    let mut wsel = [0u8; PK_GROUPS];
    let mut ws = [0u8; PK_GROUPS];
    let mut tail: f64 = zero.iter().sum();
    let mut best = (count_bits[0] + tail, 0u32);
    for n in 1..=PK_GROUPS as u32 {
        tail -= zero[n as usize - 1];
        // `choose` results are nonnegative, so when the count token plus
        // the zeroed tail already match the best, this count cannot
        // strictly improve — skip its group loop entirely. (`continue`,
        // not `break`: `count_bits` is not monotone in n.) The group's
        // lower bounds sharpen the same check: owed work counts too.
        let mut total = count_bits[n as usize] + tail;
        if total + lbs[0] - lbs[n as usize] - 1e-6 >= best.0 {
            continue;
        }
        let mut complete = true;
        for gi in 0..n as usize {
            if total + lbs[gi] - lbs[n as usize] - 1e-6 >= best.0 {
                complete = false;
                break;
            }
            let (v, w) = if n as usize - gi >= 5 {
                if !c5ok[gi] {
                    c5[gi] = choose(gi, n);
                    c5ok[gi] = true;
                }
                c5[gi]
            } else {
                choose(gi, n)
            };
            total += v;
            wsel[gi] = w as u8;
        }
        if complete && total < best.0 {
            best = (total, n);
            ws = wsel;
        }
    }
    let n = best.1;
    let mut mask = *nz & 1;
    // `ZIGZAG[1..]` is a permutation of `1..64`: this is a plain fill,
    // needed only to clear coefficients a previous pass left coded.
    if *nz & !1 != 0 {
        q[1..].fill(0);
    }
    // The applied widths: `at` lifts a lane to `2^(w-1)` per coded group,
    // so `group_widths` on the result scans back to exactly `w` — the
    // freq emitter takes these instead of rescanning `q`.
    for gi in 0..n as usize {
        let w = ws[gi] as usize;
        let vals =
            incant!(vals(lw, &nf, &a, &za, bias, gi, w) with token, [v3, neon, wasm128, scalar]);
        for (j, &m) in vals.iter().enumerate().take(coef::pk_len(gi)) {
            let k = 1 + 4 * gi + j;
            let z = ZIGZAG[k] as usize;
            q[z] = if src[z] < 0.0 { -(m as i32) } else { m as i32 };
            mask |= ((m != 0) as u64) << k;
        }
    }
    *nz = mask;
    (n, ws)
}

/// All-zero `near`: every group's widths are 0 and 1, so
/// `choose(gi, n) >= zero[gi]` (width 0 adds `row[0]`, width 1 adds
/// `len + lift + row[1]`, both positive), and every count's total is at
/// least `count_bits[n] + sum(zero)`. `count_bits` is integer-valued
/// (bit lengths), so a strict minimum leads every rival by at least
/// 1 bit — a margin that dwarfs the few ulps the running-tail
/// subtraction can shave; the `1e-6` term is only a guard, and `a < 0.5`
/// keeps the totals tiny anyway. When `count_bits[0]` is strictly the
/// cheapest token, n = 0 wins the search and nothing is written.
fn zero_wins(near: &[u32; 64], count_bits: &[f64; PK_GROUPS + 1]) -> bool {
    near[1..].iter().all(|&v| v == 0) && count_bits[1..].iter().all(|&c| count_bits[0] + 1e-6 <= c)
}

/// [`zero_wins`]'s return, out of line so the fast path stays dense.
#[cold]
#[inline(never)]
fn zero_out() -> (u32, [u8; PK_GROUPS]) {
    (0, [0; PK_GROUPS])
}

/// [`block`]'s split-Huffman search for channel `ch` of block `b`: luma
/// prices its count with the next luma count of its slice and this block's
/// chroma counts and widths (their previous-pass values); chroma prices
/// under the block's current luma count.
#[cfg(all(test, feature = "encode"))]
#[magetypes(v3, neon, wasm128, scalar)]
#[allow(unused_variables, clippy::too_many_arguments)]
pub(super) fn huf_block_channel(
    token: Token,
    sc: &mut StripeCoefs,
    b: usize,
    ch: usize,
    src: &[f32],
    steps: &[f32; 64],
    hp: &HufPrices,
    set: &LossyTrellis,
    pw: Powers,
    bias: Option<&[[f32; 64]; 2]>,
) -> (u32, [u8; PK_GROUPS]) {
    incant!(
        huf_block_channel_norm(sc, b, ch, src, steps, hp, set, pw, None, 1.0, bias) with token,
        [v3, neon, wasm128, scalar]
    )
}

#[magetypes(v3, neon, wasm128, scalar)]
#[allow(unused_variables, clippy::too_many_arguments)]
pub(super) fn huf_block_channel_norm(
    token: Token,
    sc: &mut StripeCoefs,
    b: usize,
    ch: usize,
    src: &[f32],
    steps: &[f32; 64],
    hp: &HufPrices,
    set: &LossyTrellis,
    pw: Powers,
    norm_override: Option<f64>,
    lambda_mul: f64,
    bias: Option<&[[f32; 64]; 2]>,
) -> (u32, [u8; PK_GROUPS]) {
    let lum = |sc: &StripeCoefs, b: usize| group_count(sc.nz[b * NCH] & !1);
    let bc = b * NCH + ch;
    if ch == 0 {
        let nb = sc.nby * sc.bw;
        let slice = huf::slice_len(nb);
        let prev = if b.is_multiple_of(slice) {
            0
        } else {
            lum(sc, b - 1)
        };
        let next = (b + 1 < nb && !(b + 1).is_multiple_of(slice)).then(|| lum(sc, b + 1));
        let chroma: [(u32, [u8; PK_GROUPS]); 2] = core::array::from_fn(|i| {
            let c = b * NCH + 1 + i;
            let n = group_count(sc.nz[c] & !1);
            (n, group_widths(&sc.q[c * 64..][..64], n))
        });
        // `wid_ctx(1 + i, gi, n)` freezes at `n - gi >= 5` (distance 5), so
        // that row's term is constant across `n`: cache it per group and
        // add it in the same slot the table read would have produced.
        let mut tv = [[0f64; PK_GROUPS]; 2];
        let mut tok = [[false; PK_GROUPS]; 2];
        // `wid_ctx(1 + i, gi, n) = wbase[i][gi] + min(n - gi, 5)` — the
        // `(ch * 6 + band) * 6` part is per-(channel, group) constant.
        let wbase: [[usize; PK_GROUPS]; 2] = core::array::from_fn(|i| {
            core::array::from_fn(|gi| ((1 + i) * 6 + coef::PK_BAND[gi] as usize) * 6)
        });
        let count_bits: [f64; PK_GROUPS + 1] = core::array::from_fn(|n| {
            let n = n as u32;
            let cl = huf::class(n);
            let mut bits = hp.luma[huf::class(prev)][n as usize];
            if let Some(f) = next {
                bits += hp.luma[cl][f as usize];
            }
            for (i, (cn, cw)) in chroma.iter().enumerate() {
                bits += hp.chroma[i * huf::CLASSES + cl][*cn as usize];
                for (gi, &w) in cw.iter().enumerate().take(*cn as usize) {
                    bits += if gi + 5 <= n as usize {
                        if !tok[i][gi] {
                            tv[i][gi] = hp.wid[wbase[i][gi] + 5][w as usize];
                            tok[i][gi] = true;
                        }
                        tv[i][gi]
                    } else {
                        hp.wid[wbase[i][gi] + (n as usize).saturating_sub(gi)][w as usize]
                    };
                }
            }
            bits
        });
        incant!(
            huf_channel_norm(
                src,
                steps,
                set,
                pw,
                norm_override,
                lambda_mul,
                &count_bits,
                &hp.wid,
                |gi, n| {
                    let d = n.saturating_sub(gi as u32).min(5) as usize;
                    (coef::PK_BAND[gi] as usize * 6 + d, d)
                },
                bias,
                &mut sc.q[bc * 64..][..64],
                &mut sc.nz[bc],
            ) with token,
            [v3, neon, wasm128, scalar]
        )
    } else {
        let l = lum(sc, b);
        let count_bits = hp.chroma[(ch - 1) * huf::CLASSES + huf::class(l)];
        incant!(
            huf_channel_norm(
                src,
                steps,
                set,
                pw,
                norm_override,
                lambda_mul,
                &count_bits,
                &hp.wid,
                |gi, _| {
                    let d = l.saturating_sub(gi as u32).min(5) as usize;
                    ((ch * 6 + coef::PK_BAND[gi] as usize) * 6 + d, d)
                },
                bias,
                &mut sc.q[bc * 64..][..64],
                &mut sc.nz[bc],
            ) with token,
            [v3, neon, wasm128, scalar]
        )
    }
}

/// Trellis-quantize one standalone 64-slot AC unit under SplitHuf prices
/// (for candidate pricing in partition selection). Preserves `q[0]` and `*nz & 1`.
#[allow(clippy::too_many_arguments)]
pub(in crate::lossy) fn trellis_unit_sh(
    src: &[f32; 64],
    steps: &[f32; 64],
    ch: usize,
    ref_count: u32,
    hp: &HufPrices,
    set: &LossyTrellis,
    norm_override: Option<f64>,
    lambda_mul: f64,
    q: &mut [i32; 64],
    nz: &mut u64,
) {
    let pw = set.powers();
    q[1..].fill(0);
    *nz &= 1;
    if ch == 0 {
        let count_bits = hp.luma[huf::class(ref_count)];
        incant!(
            huf_channel_norm(
                src,
                steps,
                set,
                pw,
                norm_override,
                lambda_mul,
                &count_bits,
                &hp.wid,
                |gi, n| {
                    let d = n.saturating_sub(gi as u32).min(5) as usize;
                    (coef::PK_BAND[gi] as usize * 6 + d, d)
                },
                None,
                q,
                nz,
            ),
            [v3, neon, wasm128, scalar]
        );
    } else {
        let count_bits = hp.chroma[(ch - 1) * huf::CLASSES + huf::class(ref_count)];
        incant!(
            huf_channel_norm(
                src,
                steps,
                set,
                pw,
                norm_override,
                lambda_mul,
                &count_bits,
                &hp.wid,
                |gi, _| {
                    let d = ref_count.saturating_sub(gi as u32).min(5) as usize;
                    ((ch * 6 + coef::PK_BAND[gi] as usize) * 6 + d, d)
                },
                None,
                q,
                nz,
            ),
            [v3, neon, wasm128, scalar]
        );
    }
}
