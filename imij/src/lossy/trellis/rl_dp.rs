//! Run-level rANS trellis Viterbi dynamic program and unified per-block
//! dispatcher across run-level and split-Huffman rate models.

use super::huf_dp::*;
use super::rates::{Powers, Prices, Rates};
use super::{Lab, LossyTrellis, for_channel};
use crate::lossy::*;
use archmage::prelude::*;
use coef::PK_GROUPS;

/// The most any token can cost under [`Prices`] (one slot of M).
const TOKEN_MAX_BITS: f64 = LOG_M as f64;
const MAX_MAGNITUDE: u32 = (1 << 22) - 1;

pub(in crate::lossy) fn support(ac: u64) -> u32 {
    if ac == 0 { 0 } else { 63 - ac.leading_zeros() }
}

/// The lane-order neighbours of block `b` in a unit `bw` blocks wide and
/// `nby` rows tall: the block whose luma EOB is `b`'s EOB context, and the
/// block that takes `b`'s luma EOB as its context (`stripe_tokens` order).
pub(in crate::lossy) fn lane_neighbors(
    b: usize,
    bw: usize,
    nby: usize,
) -> (Option<usize>, Option<usize>) {
    let (r, x) = (b / bw, b % bw);
    let lane = x % NL;
    let previous = if x >= NL {
        Some(b - NL)
    } else if r != 0 {
        Some((r - 1) * bw + lane + (bw - 1 - lane) / NL * NL)
    } else {
        None
    };
    let next = if x + NL < bw {
        Some(b + NL)
    } else if r + 1 < nby {
        Some((r + 1) * bw + lane)
    } else {
        None
    };
    (previous, next)
}

/// Run-level magnitude class of `a >= 1` (the low five bits of its symbol).
#[inline]
pub(super) fn class(a: u32) -> u32 {
    if a <= 4 {
        a - 1
    } else {
        4 + (32 - (a - 5).leading_zeros())
    }
}

/// The largest magnitude of class `c`.
#[inline]
pub(super) fn class_max(c: u32) -> u32 {
    match c {
        0..=4 => c + 1,
        _ => 4 + (1 << (c - 4)),
    }
}

/// Raw bits (sign and remainder) of a magnitude-`a` coefficient.
#[inline]
pub(super) fn raw_bits(a: u32) -> u32 {
    coef::rl_extra_bits(class(a))
}

/// zenjpeg's `TrellisSpeedMode` limits: (lookback positions, candidates).
pub(super) fn limits(nonzero: usize, thorough: bool) -> (usize, usize) {
    if thorough || nonzero <= 48 {
        (63, 32)
    } else if nonzero <= 55 {
        (16, 4)
    } else {
        (8, 3)
    }
}

/// Candidate slots per position index, padded to whole `f64x4`s. A position
/// offers at most `1 + class(MAX_MAGNITUDE)` = 28, so a full-width scan
/// always covers every candidate.
const CAND_PAD: usize = 32;

/// Inverse of `ZIGZAG`: `IZIG[ZIGZAG[k]] == k` (raster index → zigzag
/// index), for raster-ordered apply loops.
const IZIG: [u8; 64] = {
    let mut iz = [0u8; 64];
    let mut k = 0;
    while k < 64 {
        iz[ZIGZAG[k] as usize] = k as u8;
        k += 1;
    }
    iz
};

/// Candidates per position index, one array per field for the vectorized
/// `step` eval: (magnitude, symbol class, raw bits + distortion). `extra`
/// lanes past `ncand[i]` are `INFINITY`, so a full-width scan can't pick
/// them.
pub(in crate::lossy) struct Cands {
    mag: [[u32; CAND_PAD]; 64],
    cls: [[u32; CAND_PAD]; 64],
    extra: [[f64; CAND_PAD]; 64],
}

impl Default for Cands {
    fn default() -> Self {
        Self {
            mag: [[0; CAND_PAD]; 64],
            cls: [[0; CAND_PAD]; 64],
            extra: [[0.0; CAND_PAD]; 64],
        }
    }
}

/// The per-channel setup of `channel_impl`/`huf_channel`:
/// `a[k]`/`qn[k]`/`norm` — `|c| / s`, `floor(x + 0.5)` and the clamp are
/// per-lane exact ops computed in raster order, then permuted into zigzag
/// order by scalar moves; `norm`'s sum stays in scalar zigzag order (FP
/// addition order is observable).
#[inline]
#[magetypes(define(f64x4), v3, neon, wasm128, scalar)]
pub(super) fn setup(
    token: Token,
    src: &[f32],
    steps: &[f32; 64],
    a: &mut [f64; 64],
    qn: &mut [u32; 64],
) -> f64 {
    let mut av = [0f64; 64];
    let mut qv = [0u32; 64];
    let mut csq = [0f64; 64];
    let half = f64x4::splat_t(token, 0.5);
    let cap = f64x4::splat_t(token, MAX_MAGNITUDE as f64);
    for z in (0..64).step_by(4) {
        let c = f64x4::from_array_t(
            token,
            [
                f64::from(src[z]),
                f64::from(src[z + 1]),
                f64::from(src[z + 2]),
                f64::from(src[z + 3]),
            ],
        );
        let s = f64x4::from_array_t(
            token,
            [
                f64::from(steps[z]),
                f64::from(steps[z + 1]),
                f64::from(steps[z + 2]),
                f64::from(steps[z + 3]),
            ],
        );
        (c * c).store(csq[z..].first_chunk_mut::<4>().unwrap());
        let v = c.abs() / s;
        v.store(av[z..].first_chunk_mut::<4>().unwrap());
        // `as u32` truncates like `cvttpd`: the value is capped and
        // nonnegative, so the lane conversion is exact.
        for (l, &x) in (v + half).floor().min(cap).to_array().iter().enumerate() {
            qv[z + l] = x as u32;
        }
    }
    let mut norm = 0f64;
    for k in 1..64 {
        let z = ZIGZAG[k] as usize;
        norm += csq[z];
        a[k] = av[z];
        qn[k] = qv[z];
    }
    norm
}

/// The `best` → `q`/`nz` apply: same values as the scalar loop — raster
/// order, so `q`'s stores and `best`'s reads are contiguous after the
/// `IZIG` permute; `(m ^ sign) - sign` negates exactly where `src < 0.0`
/// (a real compare: `-0.0` is not `< 0`) and the mask OR's `1 << k` for
/// the lanes with `m != 0` — integer ops, bitwise identical to the
/// scalar loop.
#[magetypes(define(f32x8, i32x8), v3, neon, wasm128, scalar)]
fn apply(token: Token, best: &[u32; 64], src: &[f32], q: &mut [i32]) -> u64 {
    // `best` permuted to raster order so every vector op is contiguous.
    let mut bestr = [0i32; 64];
    for (z, &k) in IZIG.iter().enumerate() {
        bestr[z] = best[k as usize] as i32;
    }
    let mut mask = 0u64;
    let zero_f = f32x8::zero_t(token);
    let zero_i = i32x8::zero_t(token);
    for z0 in (0..64).step_by(8) {
        let m = i32x8::load_t(token, bestr[z0..].first_chunk::<8>().unwrap());
        let lt = f32x8::load_t(token, src[z0..].first_chunk::<8>().unwrap())
            .simd_lt(zero_f)
            .bitcast_i32x8();
        ((m ^ lt) - lt).store(q[z0..].first_chunk_mut::<8>().unwrap());
        // The lanes' support bits: `1 << k` at the lane's zigzag index.
        let mut bits = m.simd_ne(zero_i).bitmask();
        while bits != 0 {
            let l = bits.trailing_zeros() as usize;
            mask |= 1u64 << IZIG[z0 + l];
            bits &= bits - 1;
        }
    }
    mask
}

/// One predecessor's candidate costs for `step`: `Some((min, first
/// argmin))` of `r + row[run * 32 + cls] + extra` over `ci < ncand` —
/// `Some` only when the min beats `bc` (the strict `<` keeps the first
/// such `(m, ci)` in scan order). `rv`/`built` cache `row`'s gathered
/// entries for the four run states of the current position, so a
/// predecessor lands on an already-built row (`f64x4` lanes of the same
/// expression; `min` is order-free on these finite values and the argmin
/// lane is the first one equal to the min).
#[magetypes(define(f64x4), v3, neon, wasm128, scalar)]
#[allow(clippy::too_many_arguments)]
fn step_eval(
    token: Token,
    r: f64,
    row: &[f64; coef::ALPHA],
    run: usize,
    i: usize,
    cands: &Cands,
    ncand: usize,
    rv: &mut [f64; 4 * CAND_PAD],
    built: &mut [bool; 4],
    bc: f64,
) -> Option<(f64, u8)> {
    // Lanes actually needed: `ncand` rounded up to a vector; the gather
    // and the pad fill cost one pass over that, once per (position, run)
    // rather than per predecessor.
    let n4 = ncand.next_multiple_of(4);
    if !built[run] {
        let base = run * CAND_PAD;
        for ci in 0..ncand {
            rv[base + ci] = row[base + cands.cls[i][ci] as usize];
        }
        rv[base + ncand..base + n4].fill(f64::INFINITY);
        built[run] = true;
    }
    let rv = &rv[run * CAND_PAD..][..CAND_PAD];
    if ncand < 8 {
        // Few candidates: the sequential scan beats the lane setup.
        let ex = &cands.extra[i][..ncand];
        let mut best = (f64::INFINITY, 0u8);
        for (ci, (&v, &e)) in rv[..ncand].iter().zip(ex.iter()).enumerate() {
            let c = r + v + e;
            if c < best.0 {
                best = (c, ci as u8);
            }
        }
        return (best.0 < bc).then_some(best);
    }
    let extra = &cands.extra[i];
    let ru = f64x4::splat_t(token, r);
    let mut mv = f64x4::splat_t(token, f64::INFINITY);
    let mut vs = [f64x4::zero_t(token); 8];
    for (k, v) in vs.iter_mut().enumerate().take(n4 / 4) {
        *v = (ru + f64x4::load_t(token, rv[k * 4..].first_chunk::<4>().unwrap()))
            + f64x4::load_t(token, extra[k * 4..].first_chunk::<4>().unwrap());
        mv = mv.min(*v);
    }
    let m = mv.reduce_min();
    if m >= bc {
        return None;
    }
    for (k, v) in vs.iter().enumerate().take(n4 / 4) {
        for (l, &x) in v.to_array().iter().enumerate() {
            if x == m {
                return Some((m, (k * 4 + l) as u8));
            }
        }
    }
    None // unreachable: `m` is one of the lanes
}

/// Trellis-quantizes the AC of one block channel. `src`: canonical FDCT
/// coefficients (raster), `steps`: the channel's steps (raster), `q`/`nz`:
/// the block channel's coefficients (raster) and zigzag support mask, whose
/// DC is kept. `ref_eob`: the EOB context reference; `lookahead[class]`:
/// extra bits other tokens cost when this channel's EOB has that class.
#[cfg(all(test, feature = "encode"))]
#[allow(clippy::too_many_arguments)]
pub(super) fn channel(
    src: &[f32],
    steps: &[f32; 64],
    ch: usize,
    ref_eob: u32,
    lookahead: &[f64; coef::EOB_CLASSES],
    prices: &Prices,
    set: &LossyTrellis,
    pw: Powers,
    cands: &mut Cands,
    q: &mut [i32],
    nz: &mut u64,
) {
    incant!(
        channel_impl::<true, true>(
            src, steps, ch, ref_eob, lookahead, prices, set, pw, None, cands, q, nz
        ),
        [v3, neon, wasm128, scalar]
    )
}

/// [`channel`]; `REUSE` shares the Viterbi prefix whose token contexts are
/// all 31 or more positions before the EOB (the last remaining-positions
/// class, the same for every EOB that far away) across candidate EOBs;
/// `PRUNE` stops the predecessor scan once the zeroed run alone costs the
/// best path found. Both are exact (tests compare all four variants).
#[cfg(all(test, feature = "encode"))]
#[magetypes(v3, neon, wasm128, scalar)]
#[allow(clippy::too_many_arguments, clippy::needless_range_loop)]
pub(super) fn channel_impl<const REUSE: bool, const PRUNE: bool>(
    token: Token,
    src: &[f32],
    steps: &[f32; 64],
    ch: usize,
    ref_eob: u32,
    lookahead: &[f64; coef::EOB_CLASSES],
    prices: &Prices,
    set: &LossyTrellis,
    pw: Powers,
    bias: Option<&[[f32; 64]; 2]>,
    cands: &mut Cands,
    q: &mut [i32],
    nz: &mut u64,
) {
    incant!(
        channel_impl_norm::<REUSE, PRUNE>(
            src, steps, ch, ref_eob, lookahead, prices, set, pw, None, 1.0, bias, cands, q, nz
        ) with token,
        [v3, neon, wasm128, scalar]
    )
}

#[magetypes(v3, neon, wasm128, scalar)]
#[allow(clippy::too_many_arguments, clippy::needless_range_loop)]
fn channel_impl_norm<const REUSE: bool, const PRUNE: bool>(
    token: Token,
    src: &[f32],
    steps: &[f32; 64],
    ch: usize,
    ref_eob: u32,
    lookahead: &[f64; coef::EOB_CLASSES],
    prices: &Prices,
    set: &LossyTrellis,
    pw: Powers,
    norm_override: Option<f64>,
    lambda_mul: f64,
    bias: Option<&[[f32; 64]; 2]>,
    cands: &mut Cands,
    q: &mut [i32],
    nz: &mut u64,
) {
    // Zigzag magnitudes in steps, nearest quantized values, block norm.
    let mut a = [0f64; 64];
    let mut qn = [0u32; 64];
    let norm = incant!(setup(src, steps, &mut a, &mut qn) with token, [v3, neon, wasm128, scalar]);
    let norm = norm_override.unwrap_or(norm);
    // Integer DCT norm scaling.
    let w = 64.0 * set.lambda_p(norm * 64.0 / 63.0, pw) * lambda_mul;
    // zd[k]: distortion of zeroing positions 1..=k.
    let mut zd = [0f64; 64];
    for k in 1..64 {
        zd[k] = zd[k - 1] + w * a[k] * a[k];
    }
    let mut pos = [0u8; 64];
    let mut n = 0;
    for k in 1..64 {
        if qn[k] != 0 {
            pos[n] = k as u8;
            n += 1;
        }
    }
    let (lookback, max_cands) = limits(n, set.thorough);
    // Candidates per position index: the nearest value first, then larger
    // class maxima first. `least[i]`: the smallest raw bits + distortion
    // among them (a lower bound on coding position i with any token).
    let mut ncand = [0usize; 64];
    let mut least = [0f64; 64];
    for i in 0..n {
        let k = pos[i] as usize;
        let top = qn[k];
        let dist = |m: u32| {
            let b = match bias {
                Some(t) => t[(m >= 2) as usize][ZIGZAG[k] as usize] as f64,
                None => 0.0,
            };
            let e = m as f64 - b - a[k];
            w * e * e
        };
        let top_cost = raw_bits(top) as f64 + dist(top);
        cands.mag[i][0] = top;
        cands.cls[i][0] = class(top);
        cands.extra[i][0] = top_cost;
        let mut least_i = top_cost;
        let mut len = 1;
        for c in (0..class(top)).rev() {
            if len == max_cands {
                break;
            }
            let m = class_max(c);
            let cost = raw_bits(m) as f64 + dist(m);
            // It cannot pay for itself: `top`'s token costs at most one
            // slot, this one's at least nothing.
            if cost - top_cost > TOKEN_MAX_BITS {
                continue;
            }
            cands.mag[i][len] = m;
            cands.cls[i][len] = c;
            cands.extra[i][len] = cost;
            least_i = least_i.min(cost);
            len += 1;
        }
        ncand[i] = len;
        // `step_eval` reads `extra[i]` only through `ncand`'s next vector
        // multiple, so the INFINITY padding stops there.
        cands.extra[i][len..len.next_multiple_of(4)].fill(f64::INFINITY);
        least[i] = least_i;
    }

    let eob_cost = |e: u32| prices.token(eob_ctx(ch, ref_eob), e) + lookahead[coef::eob_class(e)];
    // Best path ending with a nonzero at position index i, given each start
    // position's table `tk`: (cost, predecessor index or u8::MAX, candidate).
    let step = |i: usize,
                tk: &[u8; 64],
                cost: &[f64; 64],
                rv: &mut [f64; 4 * CAND_PAD]|
     -> (f64, u8, u8) {
        let p = pos[i] as usize;
        let mut bc = f64::INFINITY;
        let (mut bj, mut bk) = (u8::MAX, 0u8);
        // Which of the four run rows `rv` holds this call's entries for
        // (`step_eval`'s lane cache; reset per position).
        let mut built = [false; 4];
        // zenjpeg: predecessors within `lookback` positions; the block's
        // start only when p itself is that close.
        let first = p.saturating_sub(lookback);
        let mut m = i;
        loop {
            // Predecessors from the nearest back: the zeroed run between j
            // and p only grows, and every other term is at least 0, so once
            // it alone reaches the best cost no earlier j can win (exact:
            // acceptance needs a strictly smaller cost).
            let jj = if m == 0 { 0 } else { pos[m - 1] as usize };
            if PRUNE && (zd[p - 1] - zd[jj]) + least[i] >= bc {
                break;
            }
            let (j, prev) = if m == 0 {
                if first != 0 {
                    break;
                }
                (0usize, 0f64)
            } else {
                let j = pos[m - 1] as usize;
                if j < first {
                    break;
                }
                (j, cost[m - 1])
            };
            if prev.is_finite() {
                let rb = prev + (zd[p - 1] - zd[j]);
                // A predecessor's eval costs `r` plus `row + extra` terms
                // that are all nonnegative, and `r` itself is at least
                // `rb` (the ZRL bits are nonnegative), so
                // `rb + least[i]` (rounding slack allowed) already
                // lower-bounds it: skip when it cannot beat `bc`, before
                // spending the ZRL adds.
                if rb + least[i] < bc * (1.0 + 1e-14) {
                    let mut start = j + 1;
                    let mut run = p - j - 1;
                    let mut r = rb;
                    while run > coef::MAX_RUN as usize {
                        r += prices.bits[tk[start] as usize][coef::ZRL as usize];
                        start += 4;
                        run -= 4;
                    }
                    if r + least[i] < bc * (1.0 + 1e-14)
                        && let Some((c, ci)) = incant!(
                            step_eval(
                                r,
                                &prices.bits[tk[start] as usize],
                                run,
                                i,
                                cands,
                                ncand[i],
                                rv,
                                &mut built,
                                bc,
                            ) with token,
                            [v3, neon, wasm128, scalar]
                        )
                    {
                        bc = c;
                        bj = if m == 0 { u8::MAX } else { (m - 1) as u8 };
                        bk = ci;
                    }
                }
            }
            if m == 0 {
                break;
            }
            m -= 1;
        }
        (bc, bj, bk)
    };
    // `step_eval`'s gathered run rows, rebuilt per position.
    let mut rv = [0.0f64; 4 * CAND_PAD];
    // Positions 31 or more before the EOB: one shared context class.
    const FAR: usize = 31;
    // `ac_ctx(ch, k, e)` ends in `rem_class(e - k)`, which saturates at
    // `e - k >= FAR`: those `tk` entries are the same for every `e` —
    // and for the far prefix's `e = k + FAR` reference, so that table is
    // exactly the saturated init. The EOB scan visits strictly
    // decreasing `e`s: overwrite only the `[e + 1 - FAR, e]` tail per
    // EOB — stale tail entries from a larger `e` sit at positions `> e`,
    // which `step` never reads (its starts are all `<= p <= e`).
    let mut tk = prices.tk0[ch];
    let (mut far_cost, mut far_back, mut far_pick) = ([0f64; 64], [0u8; 64], [0u8; 64]);
    let mut far_n = 0;
    if REUSE && n > 0 && pos[n - 1] as usize > FAR {
        while far_n < n && pos[far_n] as usize + FAR <= pos[n - 1] as usize {
            (far_cost[far_n], far_back[far_n], far_pick[far_n]) =
                step(far_n, &tk, &far_cost, &mut rv);
            far_n += 1;
        }
    }
    // EOB 0: every AC coefficient zero.
    let mut best_total = eob_cost(0) + zd[63];
    let mut best = [0u32; 64];
    let mut cost = [0f64; 64];
    let mut back = [0u8; 64];
    let mut pick = [0u8; 64];
    // Candidate EOBs from the last position down: the zeroed tail only
    // grows, so once it alone costs as much as the best total, stop.
    // The shared prefix shrinks as `e` descends: keep `i0` between
    // iterations and decrement (total work is O(far_n), not O(far_n × EOBs)).
    let mut i0 = far_n;
    // The `e` of the previous processed candidate (`continue`d ones leave
    // the table untouched): every tail entry written then is still right
    // unless `e - k` crossed a `rem_class` boundary since.
    let mut pe = usize::MAX;
    for ie in (0..n).rev() {
        let e = pos[ie] as usize;
        let tail = zd[63] - zd[e];
        if tail >= best_total {
            break;
        }
        // `cost[ie]` is a sum of nonnegative terms, so when the EOB
        // token plus the zeroed tail already match the best, this EOB
        // cannot strictly improve — skip its step chain entirely.
        // (`continue`, not `break`: `eob_cost` is not monotone in e.)
        if eob_cost(e as u32) + tail >= best_total {
            continue;
        }
        let lo = (e + 1).saturating_sub(FAR).max(1);
        if pe == usize::MAX {
            for k in lo..=e {
                tk[k] = prices.map[ac_ctx(ch, k as u32, e as u32)];
            }
        } else {
            let lo_p = (pe + 1).saturating_sub(FAR).max(1);
            // Positions entering the tail still hold the saturated init.
            for k in lo..lo_p.min(e + 1) {
                tk[k] = prices.map[ac_ctx(ch, k as u32, e as u32)];
            }
            // A `rem_class(x)` boundary sits at x in {1,3,7,15,31}: a
            // kept tail entry needs its new context only when `e - k`
            // fell across one (k in `(e - b, pe - b]`).
            for b in [1usize, 3, 7, 15, 31] {
                for k in (e + 1).saturating_sub(b).max(lo_p)..=pe.saturating_sub(b).min(e) {
                    tk[k] = prices.map[ac_ctx(ch, k as u32, e as u32)];
                }
            }
        }
        pe = e;
        // Shared prefix: positions with p + FAR <= e (every start s <= p
        // then has e - s >= FAR).
        while i0 > 0 && pos[i0 - 1] as usize + FAR > e {
            i0 -= 1;
        }
        cost[..i0].copy_from_slice(&far_cost[..i0]);
        back[..i0].copy_from_slice(&far_back[..i0]);
        pick[..i0].copy_from_slice(&far_pick[..i0]);
        for i in i0..=ie {
            (cost[i], back[i], pick[i]) = step(i, &tk, &cost, &mut rv);
        }
        let total = eob_cost(e as u32) + cost[ie] + tail;
        if total < best_total {
            best_total = total;
            best = [0; 64];
            let mut i = ie as u8;
            while i != u8::MAX {
                let ii = i as usize;
                best[pos[ii] as usize] = cands.mag[ii][pick[ii] as usize];
                i = back[ii];
            }
        }
    }
    let mut mask = *nz & 1;
    // `apply` writes all of `q` (its first lane makes `q[0]` 0); the
    // scalar loop leaves the DC lane alone.
    let dc = q[0];
    mask |= incant!(apply(&best, src, q) with token, [v3, neon, wasm128, scalar]);
    q[0] = dc;
    *nz = mask;
}

/// Trellis-quantize one standalone 64-slot AC unit under RunLevel prices
/// (for candidate pricing in partition selection). Preserves `q[0]` and `*nz & 1`.
#[allow(clippy::too_many_arguments)]
pub(in crate::lossy) fn trellis_unit_rl(
    src: &[f32; 64],
    steps: &[f32; 64],
    ch: usize,
    ref_eob: u32,
    prices: &Prices,
    set: &LossyTrellis,
    norm_override: Option<f64>,
    lambda_mul: f64,
    cands: &mut Cands,
    q: &mut [i32; 64],
    nz: &mut u64,
) {
    let pw = set.powers();
    let la = [0.0f64; coef::EOB_CLASSES];
    incant!(
        channel_impl_norm::<true, true>(
            src,
            steps,
            ch,
            ref_eob,
            &la,
            prices,
            set,
            pw,
            norm_override,
            lambda_mul,
            None,
            cands,
            q,
            nz,
        ),
        [v3, neon, wasm128, scalar]
    );
}

/// One block of a unit: luma, then chroma. `src(ch)`: the block's canonical
/// FDCT coefficients of channel `ch` (raster). `pw`: `set.powers()` (the
/// pow inputs are constant per settings). A block code (`sc.aq`) of 0
/// keeps the block as coded (inter frames); other codes scale its AC steps
/// by `a / 64`, as the quantizer does.
#[cfg(all(test, feature = "encode"))]
#[magetypes(v3, neon, wasm128, scalar)]
#[allow(unused_variables, clippy::too_many_arguments)]
pub(super) fn block<'a>(
    token: Token,
    sc: &mut StripeCoefs,
    b: usize,
    src: impl Fn(usize) -> &'a [f32],
    steps: &[[f32; 64]; 3],
    rates: &Rates,
    set: &LossyTrellis,
    lab: &Lab,
    pw: Powers,
    bias: Option<&[[[f32; 64]; 2]; 3]>,
    cands: &mut Cands,
    cspw: Option<&[(LossyTrellis, Powers); NCH]>,
) -> Option<[(u32, [u8; PK_GROUPS]); NCH]> {
    incant!(
        block_norm(sc, b, src, steps, rates, set, lab, pw, None, 1.0, bias, cands, cspw) with token,
        [v3, neon, wasm128, scalar]
    )
}

#[magetypes(v3, neon, wasm128, scalar)]
#[allow(unused_variables, clippy::too_many_arguments)]
pub(super) fn block_norm<'a>(
    token: Token,
    sc: &mut StripeCoefs,
    b: usize,
    src: impl Fn(usize) -> &'a [f32],
    steps: &[[f32; 64]; 3],
    rates: &Rates,
    set: &LossyTrellis,
    lab: &Lab,
    pw: Powers,
    norm_override: Option<&[f64; NCH]>,
    lambda_mul: f64,
    bias: Option<&[[[f32; 64]; 2]; 3]>,
    cands: &mut Cands,
    // Per-channel `for_channel`/`powers`, precomputed when `set` is
    // block-invariant (no AQ coupling).
    cspw: Option<&[(LossyTrellis, Powers); NCH]>,
) -> Option<[(u32, [u8; PK_GROUPS]); NCH]> {
    let scaled: [[f32; 64]; 3];
    let steps = match sc.aq.get(b) {
        Some(0) => return None,
        Some(&a) if a != 64 => {
            let m = a as f32 * (1.0 / 64.0);
            scaled = core::array::from_fn(|c| {
                core::array::from_fn(|i| if i == 0 { steps[c][0] } else { steps[c][i] * m })
            });
            &scaled
        }
        _ => steps,
    };
    // LAB: lambda times m^k for an AQ multiplier m (k = 2 weighs the
    // distortion in the unscaled table's steps, as flat weights do).
    // `pw` belongs to `set`; an adjusted settings value carries its own.
    #[cfg(feature = "lab")]
    let adjusted;
    #[cfg(feature = "lab")]
    let (set, pw) = {
        let k = lab.aq_exp;
        let m = sc.aq.get(b).map_or(1.0, |&a| a as f32 * (1.0 / 64.0));
        if k != 0.0 && m != 1.0 {
            let log2m = crate::lossy::transfer::ln(m as f64) / core::f64::consts::LN_2;
            adjusted = LossyTrellis {
                lambda_log_scale1: set.lambda_log_scale1 + (k as f64 * log2m) as f32,
                ..*set
            };
            (&adjusted, adjusted.powers())
        } else {
            (set, pw)
        }
    };
    let prices = match rates {
        Rates::RunLevel(p) => p,
        Rates::Huf(hp) => {
            let mut hw = [(0, [0; PK_GROUPS]); NCH];
            for ch in 0..NCH {
                let (cs, pw) = match cspw {
                    Some(c) => (c[ch].0, c[ch].1),
                    None => {
                        let cs = for_channel(set, ch, lab);
                        // A lab channel shift means new powers; else reuse `set`'s.
                        let pw = if cs == *set { pw } else { cs.powers() };
                        (cs, pw)
                    }
                };
                hw[ch] = incant!(
                    huf_block_channel_norm(
                        sc,
                        b,
                        ch,
                        src(ch),
                        &steps[ch],
                        hp,
                        &cs,
                        pw,
                        norm_override.map(|n| n[ch]),
                        lambda_mul,
                        bias.map(|t| &t[ch])
                    ) with token,
                    [v3, neon, wasm128, scalar]
                );
            }
            return Some(hw);
        }
    };
    let bw = sc.bw;
    let (previous, next) = lane_neighbors(b, bw, sc.nby);
    let reference = previous.map_or(0, |p| support(sc.nz[p * NCH] & !1));
    let chroma = [
        support(sc.nz[b * NCH + 1] & !1),
        support(sc.nz[b * NCH + 2] & !1),
    ];
    let following = next.map(|n| support(sc.nz[n * NCH] & !1));
    // The luma EOB's class is the context of both chroma EOB tokens and
    // of the lane's next luma EOB token.
    let lookahead: [f64; coef::EOB_CLASSES] = core::array::from_fn(|cls| {
        // Any EOB of this class has the same contexts.
        let e = if cls == 0 { 0 } else { 1u32 << (cls - 1) };
        let mut bits =
            prices.token(eob_ctx(1, e), chroma[0]) + prices.token(eob_ctx(2, e), chroma[1]);
        if let Some(f) = following {
            bits += prices.token(eob_ctx(0, e), f);
        }
        bits
    });
    for ch in 0..NCH {
        let bc = b * NCH + ch;
        let (reference, la) = if ch == 0 {
            (reference, lookahead)
        } else {
            (support(sc.nz[b * NCH] & !1), [0.0; coef::EOB_CLASSES])
        };
        let cs = for_channel(set, ch, lab);
        let pw = if cs == *set { pw } else { cs.powers() };
        incant!(
            channel_impl_norm::<true, true>(
                src(ch),
                &steps[ch],
                ch,
                reference,
                &la,
                prices,
                &cs,
                pw,
                norm_override.map(|n| n[ch]),
                lambda_mul,
                bias.map(|t| &t[ch]),
                cands,
                &mut sc.q[bc * 64..][..64],
                &mut sc.nz[bc],
            ) with token,
            [v3, neon, wasm128, scalar]
        );
    }
    None
}

/// Dispatches `block_norm` across SIMD tiers for one 8x8/4x4/merged block.
#[allow(clippy::too_many_arguments)]
pub(super) fn dispatch_block_norm<'a>(
    sc: &mut StripeCoefs,
    b: usize,
    src: impl Fn(usize) -> &'a [f32],
    steps: &[[f32; 64]; 3],
    rates: &Rates,
    set: &LossyTrellis,
    lab: &Lab,
    pw: Powers,
    norm_override: Option<&[f64; NCH]>,
    lambda_mul: f64,
    bias: Option<&[[[f32; 64]; 2]; 3]>,
    cands: &mut Cands,
    cspw: Option<&[(LossyTrellis, Powers); NCH]>,
) -> Option<[(u32, [u8; PK_GROUPS]); NCH]> {
    incant!(
        block_norm(
            sc,
            b,
            src,
            steps,
            rates,
            set,
            lab,
            pw,
            norm_override,
            lambda_mul,
            bias,
            cands,
            cspw
        ),
        [v3, neon, wasm128, scalar]
    )
}
