//! Encoder-only, bounded AC zeroing for run-level syntax. No decoder changes.
//! One frozen-table sweep; see docs/EVAL_RDOQ_2026-10-04.md.
#[cfg(not(feature = "std"))]
use num_traits::Float as _;

use super::*;

struct Prices {
    map: Vec<u8>,
    bits: Vec<[f64; coef::ALPHA]>,
}

impl Prices {
    fn new(tables: &Tables) -> Self {
        Self {
            map: tables.ctx_map.clone(),
            bits: tables
                .freq
                .iter()
                .map(|f| {
                    core::array::from_fn(|s| {
                        // An absent symbol has no price in the provisional table. Use
                        // one slot as a conservative surrogate; final tables rebuild.
                        crate::mathx::log2(M as f64 / f[s].max(1) as f64)
                    })
                })
                .collect(),
        }
    }
    fn token(&self, ctx: usize, symbol: u32) -> f64 {
        self.bits[self.map[ctx] as usize][symbol as usize]
    }
    fn channel(&self, q: &[i32], nz: u64, ch: usize, reference: u32) -> f64 {
        let mut ac = nz & !1;
        let eob = support(ac);
        let mut rate = self.token(eob_ctx(ch, reference), eob);
        let mut start = 1;
        while ac != 0 {
            let k = ac.trailing_zeros();
            ac &= ac - 1;
            let mut run = k - start;
            while run > coef::MAX_RUN {
                rate += self.token(ac_ctx(ch, start, eob), coef::ZRL);
                start += 4;
                run -= 4;
            }
            let (symbol, _, extra) = rl_token(run, q[ZIGZAG[k as usize] as usize]);
            rate += self.token(ac_ctx(ch, start, eob), symbol) + extra as f64;
            start = k + 1;
        }
        rate
    }
    // Price all three channels plus the following lane-local luma EOB.
    // Changing luma's support affects chroma here and the next luma context.
    fn affected(&self, sc: &StripeCoefs, b: usize) -> f64 {
        let (previous, next) = lane_neighbors(b, sc.bw, sc.nby);
        let reference = previous.map_or(0, |p| support(sc.nz[p * NCH] & !1));
        let luma = support(sc.nz[b * NCH] & !1);
        let mut rate = 0.0;
        for ch in 0..NCH {
            rate += self.channel(
                &sc.q[(b * NCH + ch) * 64..][..64],
                sc.nz[b * NCH + ch],
                ch,
                if ch == 0 { reference } else { luma },
            );
        }
        if let Some(n) = next {
            rate += self.token(eob_ctx(0, luma), support(sc.nz[n * NCH] & !1));
        }
        rate
    }
}

fn support(ac: u64) -> u32 {
    if ac == 0 { 0 } else { 63 - ac.leading_zeros() }
}

fn lane_neighbors(b: usize, bw: usize, nby: usize) -> (Option<usize>, Option<usize>) {
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

// The current block is always a candidate. Strict improvement preserves it
// on ties. DC is never touched. At most 63 tail + 63 single-zero trials/ch.
fn zero_channel(
    sc: &mut StripeCoefs,
    b: usize,
    ch: usize,
    target: &[f32],
    steps: &[f32; 64],
    lambda: f64,
    prices: &Prices,
) {
    let bc = b * NCH + ch;
    let o = bc * 64;
    let original: [i32; 64] = sc.q[o..o + 64].try_into().unwrap();
    let original_nz = sc.nz[bc];
    let base_rate = prices.affected(sc, b);
    let mut best_delta = 0.0;
    let mut best_q = original;
    let mut best_nz = original_nz;
    let mut delta_d = 0.0;
    // Explicit contiguous-tail trials include whole-AC-zero. Original source
    // coefficients are canonical FDCT values, not rounded reconstructions.
    for k in (1..64).rev() {
        let i = ZIGZAG[k] as usize;
        let q = original[i] as f64;
        if q == 0.0 {
            continue;
        }
        let t = target[i] as f64 / steps[i] as f64;
        delta_d += q * (2.0 * t - q);
        sc.q[o + i] = 0;
        sc.nz[bc] &= !(1u64 << k);
        let delta = delta_d + lambda * (prices.affected(sc, b) - base_rate);
        if delta < best_delta {
            best_delta = delta;
            best_q.copy_from_slice(&sc.q[o..o + 64]);
            best_nz = sc.nz[bc];
        }
    }
    sc.q[o..o + 64].copy_from_slice(&best_q);
    sc.nz[bc] = best_nz;
    // One reverse coordinate sweep, with the winning tail as its starting
    // candidate. No convergence loop or data-dependent pass count.
    for k in (1..64).rev() {
        let i = ZIGZAG[k] as usize;
        let q = sc.q[o + i];
        if q == 0 {
            continue;
        }
        let base_rate = prices.affected(sc, b);
        let nz = sc.nz[bc];
        sc.q[o + i] = 0;
        sc.nz[bc] &= !(1u64 << k);
        let t = target[i] as f64 / steps[i] as f64;
        let delta = q as f64 * (2.0 * t - q as f64) + lambda * (prices.affected(sc, b) - base_rate);
        if delta >= 0.0 || !delta.is_finite() {
            sc.q[o + i] = q;
            sc.nz[bc] = nz;
        }
    }
}

const MAX_MAGNITUDE: i32 = (1 << 22) - 1;
const QUEUE_CAP: usize = 4;

// A changed luma support only changes chroma EOBs and next-luma EOB,
// in addition to that channel's token sequence. Other AC walks are cached.
struct ChangeCache {
    ch: usize,
    reference: u32,
    channel: f64,
    luma: u32,
    chroma: [u32; 2],
    next: Option<u32>,
}
impl Prices {
    fn change_cache(&self, sc: &StripeCoefs, b: usize, ch: usize) -> ChangeCache {
        let (previous, next) = lane_neighbors(b, sc.bw, sc.nby);
        let luma = support(sc.nz[b * NCH] & !1);
        let reference = if ch == 0 {
            previous.map_or(0, |p| support(sc.nz[p * NCH] & !1))
        } else {
            luma
        };
        let bc = b * NCH + ch;
        ChangeCache {
            ch,
            reference,
            channel: self.channel(&sc.q[bc * 64..][..64], sc.nz[bc], ch, reference),
            luma,
            chroma: [
                support(sc.nz[b * NCH + 1] & !1),
                support(sc.nz[b * NCH + 2] & !1),
            ],
            next: next.map(|n| support(sc.nz[n * NCH] & !1)),
        }
    }
    fn changed_delta(&self, cache: &ChangeCache, q: &[i32; 64], nz: u64) -> f64 {
        let mut delta = self.channel(q, nz, cache.ch, cache.reference) - cache.channel;
        if cache.ch == 0 {
            let luma = support(nz & !1);
            for (c, &eob) in cache.chroma.iter().enumerate() {
                delta += self.token(eob_ctx(c + 1, luma), eob)
                    - self.token(eob_ctx(c + 1, cache.luma), eob);
            }
            if let Some(eob) = cache.next {
                delta +=
                    self.token(eob_ctx(0, luma), eob) - self.token(eob_ctx(0, cache.luma), eob);
            }
        }
        delta
    }
}
fn magnitude_neighbors(q: i32, target: f32) -> [i32; 2] {
    let magnitude = q.unsigned_abs().min(MAX_MAGNITUDE as u32) as i32;
    let sign = if target < 0.0 { -1 } else { 1 };
    [
        sign * (magnitude - 1).max(0),
        sign * (magnitude + 1).min(MAX_MAGNITUDE),
    ]
}
fn queue(
    original: &[i32; 64],
    nz: u64,
    target: &[f32],
    steps: &[f32; 64],
) -> ([usize; QUEUE_CAP], usize) {
    let mut out = [0; QUEUE_CAP];
    let mut len = 0;
    let mut ac = nz & !1;
    for _ in 0..2 {
        if ac == 0 {
            break;
        }
        let k = (63 - ac.leading_zeros()) as usize;
        out[len] = k;
        len += 1;
        ac &= !(1u64 << k);
    }
    let mut boundary = [(f64::INFINITY, 0usize); 2];
    let better = |a: (f64, usize), b: (f64, usize)| a.0 < b.0 || (a.0 == b.0 && a.1 > b.1);
    for k in 1..64 {
        if out[..len].contains(&k) {
            continue;
        }
        let i = ZIGZAG[k] as usize;
        let t = target[i] as f64 / steps[i] as f64;
        let initial = (t - original[i] as f64).powi(2);
        let score = magnitude_neighbors(original[i], target[i])
            .into_iter()
            .filter(|&q| q != original[i])
            .map(|q| (t - q as f64).powi(2) - initial)
            .fold(f64::INFINITY, f64::min);
        let rank = (score, k);
        if better(rank, boundary[0]) {
            boundary[1] = boundary[0];
            boundary[0] = rank;
        } else if better(rank, boundary[1]) {
            boundary[1] = rank;
        }
    }
    for (_, k) in boundary {
        if k != 0 {
            out[len] = k;
            len += 1;
        }
    }
    out[..len].sort_unstable_by(|a, b| b.cmp(a));
    (out, len)
}
fn spatial_weight<const STEER: bool>(
    target: &[f32],
    original: &[i32; 64],
    steps: &[f32; 64],
) -> f64 {
    if !STEER {
        return 1.0;
    }
    let mut error = 0.0;
    let mut contrast = 0.0;
    for i in 0..64 {
        let c = target[i] as f64;
        let e = c - original[i] as f64 * steps[i] as f64;
        error += e * e;
        if i != 0 {
            contrast += c * c;
        }
    }
    let epsilon = (steps[1] as f64 / 8.0).powi(2);
    1.0 + (error / 64.0 / (contrast / 64.0 + epsilon)).clamp(0.0, 1.0)
}
// Returns queue length/trials for bounded-search oracles. Optimizer ignores
// these counters; disabled/classic paths never enter this routine.
#[allow(clippy::too_many_arguments)]
fn round_channel<const STEER: bool, const NEIGHBOR: bool>(
    sc: &mut StripeCoefs,
    b: usize,
    ch: usize,
    target: &[f32],
    steps: &[f32; 64],
    lambda: f64,
    prices: &Prices,
) -> (usize, usize, f64) {
    let bc = b * NCH + ch;
    let offset = bc * 64;
    let original: [i32; 64] = sc.q[offset..][..64].try_into().unwrap();
    let weight = spatial_weight::<STEER>(target, &original, steps);
    let (positions, len) = queue(&original, sc.nz[bc], target, steps);
    let mut trials = 0;
    for &k in &positions[..len] {
        let i = ZIGZAG[k] as usize;
        let current: [i32; 64] = sc.q[offset..][..64].try_into().unwrap();
        let current_nz = sc.nz[bc];
        let cache = prices.change_cache(sc, b, ch);
        let neighbors = magnitude_neighbors(original[i], target[i]);
        let candidates = if NEIGHBOR {
            [original[i], 0, neighbors[0], neighbors[1]]
        } else {
            [original[i], 0, original[i], original[i]]
        };
        let mut best_delta = 0.0;
        let mut best = current;
        let mut best_nz = current_nz;
        let t = target[i] as f64 / steps[i] as f64;
        let old_d = (t - current[i] as f64).powi(2);
        for (candidate_index, &v) in candidates.iter().enumerate() {
            if v == current[i] || candidates[..candidate_index].contains(&v) {
                continue;
            }
            trials += 1;
            let mut candidate = current;
            candidate[i] = v;
            let candidate_nz = if v == 0 {
                current_nz & !(1u64 << k)
            } else {
                current_nz | (1u64 << k)
            };
            let delta = weight * ((t - v as f64).powi(2) - old_d)
                + lambda * prices.changed_delta(&cache, &candidate, candidate_nz);
            if delta.is_finite() && delta < best_delta {
                best_delta = delta;
                best = candidate;
                best_nz = candidate_nz;
            }
        }
        sc.q[offset..][..64].copy_from_slice(&best);
        sc.nz[bc] = best_nz;
    }
    // One whole-AC-zero trial after the reverse coordinate sweep.
    let current: [i32; 64] = sc.q[offset..][..64].try_into().unwrap();
    if sc.nz[bc] & !1 != 0 {
        trials += 1;
        let mut candidate = current;
        candidate[1..].fill(0);
        let candidate_nz = sc.nz[bc] & 1;
        let mut distortion = 0.0;
        for i in 1..64 {
            let t = target[i] as f64 / steps[i] as f64;
            distortion += t * t - (t - current[i] as f64).powi(2);
        }
        let cache = prices.change_cache(sc, b, ch);
        let delta =
            weight * distortion + lambda * prices.changed_delta(&cache, &candidate, candidate_nz);
        if delta.is_finite() && delta < 0.0 {
            sc.q[offset..][..64].copy_from_slice(&candidate);
            sc.nz[bc] = candidate_nz;
        }
    }
    debug_assert!(len <= 4 && trials <= if NEIGHBOR { 13 } else { 5 });
    (len, trials, weight)
}

/// Second FDCT pass, row-sized scratch only. Coding units already reflect
/// column grouping, so provisional models and lane contexts match the writer.
/// Each unit recomputes its block rows through the encoder's own row pipeline
/// `rows` (planes `pw` samples wide, `image_bw` blocks: input conversion,
/// color, deringing), so the source is the pass-1 transform input.
/// `dc`: the pass-1 DC rounding (`dc_row`), `Some(extremes)` when it ran,
/// `extremes` whether luma used the white/black blocks.
#[allow(clippy::too_many_arguments)]
pub(super) fn optimize(
    stripes: Vec<StripeCoefs>,
    rows: RowPlanes,
    pw: usize,
    image_bw: usize,
    cfg: &LossyConfig,
    steps: &[[f32; 64]; 3],
    group_cols: usize,
    effort: crate::effort::Effort,
    dc: Option<bool>,
    stop: &dyn how_far::Stop,
) -> Result<Vec<StripeCoefs>, Error> {
    macro_rules! run {
        ($classic:literal, $steer:literal, $neighbor:literal) => {
            optimize_impl::<$classic, $steer, $neighbor>(
                stripes, rows, pw, image_bw, cfg, steps, group_cols, effort, dc, stop,
            )
        };
    }
    match cfg.rdoq_policy {
        0 => run!(true, false, false),
        1 => run!(false, false, false),
        2 => run!(false, true, false),
        3 => run!(false, false, true),
        4 => run!(false, true, true),
        _ => unreachable!("lab policy validated before analysis"),
    }
}

#[allow(clippy::too_many_arguments)]
fn optimize_impl<const CLASSIC: bool, const STEER: bool, const NEIGHBOR: bool>(
    stripes: Vec<StripeCoefs>,
    rows: RowPlanes,
    pw: usize,
    image_bw: usize,
    cfg: &LossyConfig,
    steps: &[[f32; 64]; 3],
    group_cols: usize,
    effort: crate::effort::Effort,
    _dc: Option<bool>,
    stop: &dyn how_far::Stop,
) -> Result<Vec<StripeCoefs>, Error> {
    stop.check().map_err(|_| Error::Cancelled)?;
    // Use the normal table-count decision and explicit prices for RDOQ,
    // including when the file will use a different seed policy.
    let (tables, explicit) = super::ans_tables_for_decisions(&stripes, cfg, effort);
    let tables = explicit.unwrap_or(tables);
    let prices = Prices::new(&tables);
    drop(tables);
    let group_bw = group_cols / 8;
    let ng = if group_bw == 0 {
        1
    } else {
        image_bw.div_ceil(group_bw)
    };
    let cm = dct::dct_matrix();
    let optimized = crate::par::par_map(stripes, cfg.threads, |u, mut sc| {
        stop.check().map_err(|_| Error::Cancelled)?;
        let bx0 = if group_bw == 0 { 0 } else { u % ng * group_bw };
        let mut planes = vec![0.0f32; NCH * 8 * pw];
        let mut extremes: Vec<(u64, bool)> = Vec::with_capacity(image_bw);
        let mut source = vec![0.0f32; NCH * sc.bw * 64];
        for r in 0..sc.nby {
            stop.check().map_err(|_| Error::Cancelled)?;
            rows(sc.by0 + r, &mut planes, &mut extremes);
            for ch in 0..NCH {
                enc_simd::fdct_row(
                    &planes[ch * 8 * pw + bx0 * 8..],
                    pw,
                    sc.bw,
                    &cm,
                    &mut source[ch * sc.bw * 64..][..sc.bw * 64],
                );
            }
            // Both transforms use fdct_row's selected lab policy. Debug
            // requantization (pass 1's quantizer and DC rounding) proves
            // source, deringing, padding and group offsets reproduce the
            // initial coefficients before any row decisions are applied.
            #[cfg(debug_assertions)]
            {
                let mut q = vec![0; sc.bw * NCH * 64];
                let mut nz = vec![0; sc.bw * NCH];
                for (ch, st) in steps.iter().enumerate() {
                    let inv = st.map(|v| 1.0 / v);
                    let src = &source[ch * sc.bw * 64..][..sc.bw * 64];
                    enc_simd::quant_row(
                        src,
                        sc.bw,
                        ch,
                        &inv,
                        &[],
                        cfg.round.clamp(0.0, 0.5),
                        &mut q,
                        &mut nz,
                    );
                    if let Some(white) = _dc {
                        let ext = (ch == 0 && white).then(|| &extremes[bx0..][..sc.bw]);
                        dc_row(src, sc.bw, ch, inv[0], ext, &mut q, &mut nz);
                    }
                }
                debug_assert_eq!(q, sc.q[r * sc.bw * NCH * 64..][..sc.bw * NCH * 64]);
                debug_assert_eq!(nz, sc.nz[r * sc.bw * NCH..][..sc.bw * NCH]);
            }
            for x in 0..sc.bw {
                stop.check().map_err(|_| Error::Cancelled)?;
                let b = r * sc.bw + x;
                for (ch, st) in steps.iter().enumerate() {
                    let offset = (ch * sc.bw + x) * 64;
                    if CLASSIC {
                        zero_channel(
                            &mut sc,
                            b,
                            ch,
                            &source[offset..offset + 64],
                            st,
                            cfg.rdoq_lambda as f64,
                            &prices,
                        );
                    } else {
                        round_channel::<STEER, NEIGHBOR>(
                            &mut sc,
                            b,
                            ch,
                            &source[offset..offset + 64],
                            st,
                            cfg.rdoq_lambda as f64,
                            &prices,
                        );
                    }
                }
            }
        }
        Ok::<_, Error>(sc)
    })
    .into_iter()
    .collect::<Result<Vec<_>, Error>>()?;
    #[cfg(feature = "std")]
    {
        let mut stats = [0u64; 3];
        for sc in &optimized {
            stripe_tokens(sc, sc.bw, Group::ALL, |_, _, _, _, _| stats[0] += 1);
            for &nz in &sc.nz {
                let eob = support(nz & !1);
                stats[1] += (eob as u64).div_ceil(4);
                stats[2] += (eob != 0) as u64;
            }
        }
        prof::side_stats(&stats);
    }
    stop.check().map_err(|_| Error::Cancelled)?;
    Ok(optimized)
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    #[test]
    fn rdoq_cancellation_interrupts_inside_the_first_stripe() {
        use core::sync::atomic::{AtomicUsize, Ordering};
        struct StopInRow {
            rows: AtomicUsize,
            checks: AtomicUsize,
        }
        impl how_far::Stop for StopInRow {
            fn check(&self) -> Result<(), how_far::StopReason> {
                if self.rows.load(Ordering::Relaxed) > 0
                    && self.checks.fetch_add(1, Ordering::Relaxed) >= 4
                {
                    Err(how_far::StopReason::Cancelled)
                } else {
                    Ok(())
                }
            }
        }
        let stop = StopInRow {
            rows: AtomicUsize::new(0),
            checks: AtomicUsize::new(0),
        };
        let bw = 32;
        let sc = StripeCoefs {
            by0: 0,
            nby: 4,
            bw,
            q: vec![0; bw * 4 * NCH * 64],
            nz: vec![0; bw * 4 * NCH],
            aq: Vec::new(),
            modes: Vec::new(),
            splits: Vec::new(),
        };
        let rows = |_: usize, planes: &mut [f32], _: &mut Vec<(u64, bool)>| {
            stop.rows.fetch_add(1, Ordering::Relaxed);
            planes.fill(0.0);
        };
        let cfg = LossyConfig::default().with_threads(1);
        let result = optimize(
            vec![sc],
            &rows,
            bw * 8,
            bw,
            &cfg,
            &[[1.0; 64]; 3],
            0,
            crate::effort::Effort::new(9.0).unwrap(),
            None,
            &stop,
        );
        assert!(matches!(result, Err(Error::Cancelled)));
        assert_eq!(
            stop.rows.load(Ordering::Relaxed),
            1,
            "cancellation must not finish all rows of the stripe"
        );
    }

    fn fixture(bw: usize, nby: usize) -> StripeCoefs {
        let mut sc = StripeCoefs {
            by0: 0,
            nby,
            bw,
            q: vec![0; bw * nby * NCH * 64],
            nz: vec![0; bw * nby * NCH],
            aq: vec![],
            modes: vec![],

            splits: Vec::new(),
        };
        for bc in 0..bw * nby * NCH {
            sc.q[bc * 64] = 17;
            sc.nz[bc] = 1;
            for (k, v) in [(1, 1), (6, -5), (23, 12), (63, -2)] {
                sc.q[bc * 64 + ZIGZAG[k] as usize] = v;
                sc.nz[bc] |= 1 << k;
            }
        }
        sc
    }
    fn test_prices(sc: &StripeCoefs) -> Prices {
        let (t, e) = ans_tables(
            core::slice::from_ref(sc),
            &LossyConfig::default(),
            crate::effort::Effort::new(10.0).unwrap(),
        );
        Prices::new(&e.unwrap_or(t))
    }
    #[test]
    fn affected_rate_matches_writer_including_following_context() {
        for bw in [1, 7, 16, 17, 33] {
            let mut sc = fixture(bw, 2);
            let cfg = LossyConfig::default();
            let (tables, explicit) = ans_tables(
                core::slice::from_ref(&sc),
                &cfg,
                crate::effort::Effort::new(10.0).unwrap(),
            );
            let tables = explicit.unwrap_or(tables);
            let prices = Prices::new(&tables);
            let total = |sc: &StripeCoefs| {
                let mut rate = 0.0;
                stripe_tokens(sc, bw, Group::ALL, |_, ctx, tok, n, _| {
                    rate += prices.token(ctx, tok) + n as f64;
                });
                rate
            };
            for b in 0..bw * 2 {
                for ch in 0..NCH {
                    let before = (total(&sc), prices.affected(&sc, b));
                    let bc = b * NCH + ch;
                    let q: [i32; 64] = sc.q[bc * 64..][..64].try_into().unwrap();
                    let nz = sc.nz[bc];
                    sc.q[bc * 64 + 1..bc * 64 + 64].fill(0);
                    sc.nz[bc] &= 1;
                    let after = (total(&sc), prices.affected(&sc, b));
                    assert!(((after.0 - before.0) - (after.1 - before.1)).abs() < 1e-8);
                    sc.q[bc * 64..bc * 64 + 64].copy_from_slice(&q);
                    sc.nz[bc] = nz;
                }
            }
        }
    }
    #[test]
    fn zeroing_keeps_dc_and_retains_candidate_at_low_lambda() {
        let mut sc = fixture(1, 1);
        let prices = test_prices(&sc);
        let target: Vec<f32> = sc.q[..64].iter().map(|&q| q as f32).collect();
        let original = sc.q.clone();
        zero_channel(&mut sc, 0, 0, &target, &[1.0; 64], 1e-9, &prices);
        assert_eq!(sc.q, original);
        zero_channel(&mut sc, 0, 0, &[0.0; 64], &[1.0; 64], 1.0, &prices);
        assert_eq!(sc.q[0], 17);
        assert_eq!(sc.nz[0], 1);
    }
    #[test]
    fn cached_candidate_deltas_match_full_writer() {
        for bw in [1, 7, 16, 17, 33] {
            let mut sc = fixture(bw, 2);
            let prices = test_prices(&sc);
            let total = |s: &StripeCoefs| {
                let mut rate = 0.0;
                stripe_tokens(s, bw, Group::ALL, |_, ctx, tok, n, _| {
                    rate += prices.token(ctx, tok) + n as f64
                });
                rate
            };
            for b in 0..bw * 2 {
                for ch in 0..NCH {
                    let bc = b * NCH + ch;
                    let original: [i32; 64] = sc.q[bc * 64..][..64].try_into().unwrap();
                    let original_nz = sc.nz[bc];
                    let before = total(&sc);
                    let cache = prices.change_cache(&sc, b, ch);
                    for k in [1, 3, 4, 6, 23, 63] {
                        for v in [0, -1, 1, 127, 128, -MAX_MAGNITUDE, MAX_MAGNITUDE] {
                            let mut q = original;
                            q[ZIGZAG[k] as usize] = v;
                            let nz = if v == 0 {
                                original_nz & !(1 << k)
                            } else {
                                original_nz | (1 << k)
                            };
                            sc.q[bc * 64..][..64].copy_from_slice(&q);
                            sc.nz[bc] = nz;
                            let delta = prices.changed_delta(&cache, &q, nz);
                            assert!(
                                (delta - (total(&sc) - before)).abs() < 1e-8,
                                "bw={bw} b={b} ch={ch} k={k} v={v}"
                            );
                        }
                    }
                    let mut q = original;
                    q[1..].fill(0);
                    sc.q[bc * 64..][..64].copy_from_slice(&q);
                    sc.nz[bc] = original_nz & 1;
                    assert!(
                        (prices.changed_delta(&cache, &q, original_nz & 1) - (total(&sc) - before))
                            .abs()
                            < 1e-8
                    );
                    sc.q[bc * 64..][..64].copy_from_slice(&original);
                    sc.nz[bc] = original_nz;
                }
            }
        }
    }
    #[test]
    fn queue_is_fixed_capped_and_ties_prefer_reverse_zigzag() {
        let (q, n) = queue(&[0; 64], 0, &[0.0; 64], &[1.0; 64]);
        assert_eq!((q, n), ([63, 62, 0, 0], 2));
        let sc = fixture(1, 1);
        let original = sc.q[..64].try_into().unwrap();
        let (q, n) = queue(&original, sc.nz[0], &[0.0; 64], &[1.0; 64]);
        assert_eq!(n, 4);
        assert!(q[..n].contains(&63));
        assert!(q[..n].contains(&23));
        for (i, &k) in q[..n].iter().enumerate() {
            assert!((1..64).contains(&k));
            assert!(!q[..i].contains(&k));
        }
        assert_eq!(
            magnitude_neighbors(MAX_MAGNITUDE, 1.0),
            [MAX_MAGNITUDE - 1, MAX_MAGNITUDE]
        );
        assert_eq!(
            magnitude_neighbors(-MAX_MAGNITUDE, -1.0),
            [-MAX_MAGNITUDE + 1, -MAX_MAGNITUDE]
        );
        assert_eq!(magnitude_neighbors(0, -0.1), [0, -1]);
    }
    fn objective<const STEER: bool, const NEIGHBOR: bool>() {
        let mut sc = fixture(17, 2);
        let prices = test_prices(&sc);
        let steps = core::array::from_fn(|i| 1.0 + (i % 7) as f32);
        for b in 0..34 {
            for ch in 0..NCH {
                let bc = b * NCH + ch;
                let original: [i32; 64] = sc.q[bc * 64..][..64].try_into().unwrap();
                let target: [f32; 64] = core::array::from_fn(|i| {
                    (original[i] as f32 + (((i * 37 + b * 11 + ch) % 19) as f32 - 9.0) * 0.06)
                        * steps[i]
                });
                let distortion = |q: &[i32]| {
                    q.iter()
                        .enumerate()
                        .skip(1)
                        .map(|(i, &v)| (target[i] as f64 / steps[i] as f64 - v as f64).powi(2))
                        .sum::<f64>()
                };
                let weight = spatial_weight::<STEER>(&target, &original, &steps);
                let before = weight * distortion(&original) + 0.08 * prices.affected(&sc, b);
                let (len, trials, w) = round_channel::<STEER, NEIGHBOR>(
                    &mut sc, b, ch, &target, &steps, 0.08, &prices,
                );
                let after =
                    weight * distortion(&sc.q[bc * 64..][..64]) + 0.08 * prices.affected(&sc, b);
                assert!(after <= before + 1e-8);
                assert_eq!(weight, w);
                assert!((1.0..=2.0).contains(&w));
                assert!(len <= 4 && trials <= if NEIGHBOR { 13 } else { 5 });
                assert_eq!(sc.q[bc * 64], original[0]);
                assert_eq!(sc.nz[bc] & 1, 1);
                for (k, &i) in ZIGZAG.iter().enumerate().skip(1) {
                    let v = sc.q[bc * 64 + i as usize];
                    assert!(v.unsigned_abs() <= MAX_MAGNITUDE as u32);
                    assert_eq!((sc.nz[bc] >> k) & 1, (v != 0) as u64);
                    if v != 0 {
                        assert_eq!(v.is_negative(), target[i as usize].is_sign_negative());
                    }
                }
            }
        }
    }
    #[test]
    fn four_arms_improve_frozen_objective_and_preserve_invariants() {
        objective::<false, false>();
        objective::<true, false>();
        objective::<false, true>();
        objective::<true, true>();
    }
    #[test]
    fn original_wins_exact_ties_and_neighbors_can_insert() {
        let mut sc = fixture(1, 1);
        sc.q.fill(0);
        sc.nz.fill(0);
        sc.q[0] = 17;
        sc.nz[0] = 1;
        let prices = test_prices(&sc);
        let mut target = [0.0; 64];
        target[0] = 17.0;
        target[ZIGZAG[63] as usize] = 0.5;
        let original = sc.q.clone();
        round_channel::<false, true>(&mut sc, 0, 0, &target, &[1.0; 64], 0.0, &prices);
        assert_eq!(sc.q, original);
        target[ZIGZAG[63] as usize] = 0.55;
        round_channel::<false, false>(&mut sc, 0, 0, &target, &[1.0; 64], 0.0, &prices);
        assert_eq!(sc.q, original);
        round_channel::<false, true>(&mut sc, 0, 0, &target, &[1.0; 64], 0.0, &prices);
        assert_eq!(sc.q[ZIGZAG[63] as usize], 1);
        assert_eq!(sc.q[0], 17);
    }
    #[test]
    fn weight_energy_matches_independent_plane_reconstruction() {
        let cm = dct::dct_matrix();
        for pattern in 0..5 {
            let plane: [f32; 64] = core::array::from_fn(|i| match pattern {
                0 => 0.0,
                1 => 7.0,
                2 => i as f32 / 7.0,
                3 => {
                    if i % 2 == 0 {
                        -5.0
                    } else {
                        5.0
                    }
                }
                _ => ((i * 37) % 97) as f32 - 48.0,
            });
            let target = dct::fdct8x8(&cm, &plane);
            let steps = core::array::from_fn(|i| 1.0 + (i % 11) as f32 / 4.0);
            let original = core::array::from_fn(|i| {
                let magnitude = (target[i].abs() / steps[i] + 0.4).floor() as i32;
                if target[i] < 0.0 {
                    -magnitude
                } else {
                    magnitude
                }
            });
            let dq = dct::dequant_factors(&steps);
            let mut decoded = core::array::from_fn(|i| original[i] as f32 * dq[i]);
            dct::idct8x8(&mut decoded);
            let error = plane
                .iter()
                .zip(decoded)
                .map(|(&a, b)| (a as f64 - b as f64).powi(2))
                .sum::<f64>()
                / 64.0;
            let mean = plane.iter().map(|&v| v as f64).sum::<f64>() / 64.0;
            let variance = plane
                .iter()
                .map(|&v| (v as f64 - mean).powi(2))
                .sum::<f64>()
                / 64.0;
            let expected =
                1.0 + (error / (variance + (steps[1] as f64 / 8.0).powi(2))).clamp(0.0, 1.0);
            assert!((spatial_weight::<true>(&target, &original, &steps) - expected).abs() < 1e-4);
            assert_eq!(spatial_weight::<false>(&target, &original, &steps), 1.0);
        }
    }
}
