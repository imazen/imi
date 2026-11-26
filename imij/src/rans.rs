//! 16-lane interleaved rANS (format v1 entropy layer).
//!
//! Per stripe and slot, 16 lanes (column segments) each own a 32-bit rANS
//! state. Residual tokens (hybrid uint, run tokens share the scheme) are
//! coded with one of `NCTX` static tables selected by the context class of
//! the lane's west + north residual magnitudes. Renormalization reads
//! 16-bit words from one stream shared by all lanes, in decode-consumption
//! order: step ascending, then phase (token, extra bits), then lane.
//!
//! Decode table entry: `(freq - 1) | bias << 10 | sym << 20 | nb << 28`.

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use crate::Error;
use crate::bits::{BitReader, get_gamma};
#[cfg(feature = "encode")]
use crate::bits::{BitWriter, put_gamma};
use crate::tokens::{TokenCfg, hybrid_base};

pub(crate) const NL: usize = 16;
pub(crate) const NCTX: usize = 4;
pub(crate) const LOG_M: u32 = 10;
pub(crate) const M: u32 = 1 << LOG_M;
pub(crate) const CLASSES: usize = 16;
pub(crate) const RANS_L: u32 = 1 << 16;
/// Longest run coded by one token (keeps extra bits <= 14). Normative
/// (SPEC 3.2): a model that gives weight to a run token whose shortest run
/// is longer is `Corrupt`, so every decoder tier sees runs of at most this
/// length (the narrow SIMD tables hold 4-bit extra-bit widths).
pub(crate) const RUN_CAP: u32 = 1 << 15;
pub(crate) const TABLE: usize = NCTX << LOG_M;

#[inline(always)]
pub(crate) fn class_of(sum: u32) -> usize {
    ((32 - sum.leading_zeros()) as usize).min(CLASSES - 1)
}

/// Context term: a neighbor's residual after the model's shift, clamped to
/// 2^16 (any term >= 2^15 already forces the top class, so the clamp never
/// changes a class; it keeps sums inside 32-bit lanes). Baseline: shift 0
/// and u < 2^16, so the term is u itself.
#[inline(always)]
pub(crate) fn ctx_term(u: u32, shift: u32) -> u32 {
    (u >> shift).min(1 << 16)
}

/// The first symbol of scheme `tc` past the run cap: the run tokens from
/// here on code runs longer than [`RUN_CAP`] and must have weight 0.
pub(crate) fn run_cap_end(tc: &TokenCfg) -> usize {
    tc.run(RUN_CAP).0 as usize + 1
}

/// A model table's weights obey the run cap (SPEC 3.2).
fn check_run_cap(tc: &TokenCfg, wt: &[u32]) -> Result<(), Error> {
    if wt[run_cap_end(tc).min(wt.len())..].iter().any(|&w| w != 0) {
        return Err(Error::Corrupt("run token past the run cap"));
    }
    Ok(())
}

/// Zero out encoder-side weights past the run cap so the written model
/// passes the decoder's [`check_run_cap`].
#[cfg(feature = "encode")]
pub(crate) fn enforce_run_cap(tc: &TokenCfg, wt: &mut [u32]) {
    let end = run_cap_end(tc).min(wt.len());
    for w in &mut wt[end..] {
        *w = 0;
    }
}

/// Per-slot entropy model.
#[derive(Clone)]
pub(crate) struct Model {
    pub tc: TokenCfg,
    pub ctx_map: [u8; CLASSES],
    /// Normalized frequencies [ctx][sym], each context sums to M.
    pub freq: Vec<Vec<u32>>,
    /// Transmitted (quantized) weights the frequencies derive from.
    // Retained in the shared parsed representation for wire re-emission.
    #[cfg_attr(not(feature = "encode"), allow(dead_code))]
    pub weights: Vec<Vec<u32>>,
    /// Weight precision per context.
    // Retained in the shared parsed representation for wire re-emission.
    #[cfg_attr(not(feature = "encode"), allow(dead_code))]
    pub prec: Vec<u32>,
    pub cross: bool,
    /// Context sums are shifted right by this before classing (extended
    /// tier, depth > 16 only; always 0 in baseline).
    pub ctx_shift: u32,
    /// Seed of each context's table when written with selectors (`None` =
    /// explicit).
    // Retained in the shared parsed representation for wire re-emission.
    #[cfg_attr(not(feature = "encode"), allow(dead_code))]
    pub seeds: [Option<u8>; NCTX],
}

pub(crate) struct DecModel {
    /// Entry: (freq-1) | bias << 10 | sym << 20 | min(nb, 15) << 28.
    pub dec: Box<[u32; TABLE]>,
    /// Per symbol (base, extra bits, is_run) for the scalar path.
    pub tok: Vec<(u32, u32, bool)>,
    pub ctx_shift: u32,
    pub ctx_map: [u8; CLASSES],
    #[cfg_attr(
        all(not(target_arch = "x86_64"), not(target_arch = "aarch64")),
        allow(dead_code)
    )]
    pub k: u32,
    #[cfg_attr(
        all(not(target_arch = "x86_64"), not(target_arch = "aarch64")),
        allow(dead_code)
    )]
    pub kr: u32,
    #[cfg_attr(
        all(not(target_arch = "x86_64"), not(target_arch = "aarch64")),
        allow(dead_code)
    )]
    pub m: u32,
    #[cfg_attr(
        all(not(target_arch = "x86_64"), not(target_arch = "aarch64")),
        allow(dead_code)
    )]
    pub vt: u32,
    pub cross: bool,
}

impl Model {
    pub(crate) fn alphabet(&self) -> usize {
        self.tc.alphabet() as usize
    }

    pub(crate) fn cum(&self, c: usize) -> Vec<u32> {
        let f = &self.freq[c];
        let mut cm = vec![0u32; f.len() + 1];
        for i in 0..f.len() {
            cm[i + 1] = cm[i] + f[i];
        }
        cm
    }

    pub(crate) fn dec_model(&self) -> DecModel {
        let mut dec: Box<[u32; TABLE]> = vec![0u32; TABLE]
            .into_boxed_slice()
            .try_into()
            .ok()
            .unwrap();
        let vt = self.tc.value_tokens();
        for c in 0..NCTX {
            let cm = self.cum(c);
            for sym in 0..self.alphabet() {
                let f = self.freq[c][sym];
                if f == 0 {
                    continue;
                }
                let nb = token_base(sym as u32, &self.tc, vt).1.min(15);
                for slot in cm[sym]..cm[sym + 1] {
                    dec[(c << LOG_M) | slot as usize] =
                        (f - 1) | ((slot - cm[sym]) << 10) | ((sym as u32) << 20) | (nb << 28);
                }
            }
        }
        let tok = (0..self.alphabet() as u32)
            .map(|t| token_base(t, &self.tc, vt))
            .collect();
        DecModel {
            dec,
            tok,
            ctx_shift: self.ctx_shift,
            ctx_map: self.ctx_map,
            k: self.tc.k,
            kr: self.tc.rk,
            m: self.tc.m,
            vt,
            cross: self.cross,
        }
    }

    /// Seed-bank key of this model's tables: the token scheme.
    #[cfg(feature = "encode")]
    pub(crate) fn seed_key(&self) -> [u8; 3] {
        [self.tc.k as u8, self.tc.m as u8, self.tc.depth as u8]
    }

    /// Serializes k, m, ctx_map and the frequency tables (as quantized
    /// weights; see [`weights_to_freq`]); with `bank`, each table behind a
    /// selector.
    #[cfg(feature = "encode")]
    pub(crate) fn write(&self, w: &mut BitWriter, bank: Option<&'static crate::seeds::Bank>) {
        w.put(self.tc.k, 5);
        w.put(self.tc.m, 3);
        w.put(self.cross as u32, 1);
        if self.tc.depth > 16 {
            w.put(self.ctx_shift, 5);
        }
        for &c in &self.ctx_map {
            w.put(c as u32, 2);
        }
        let used_ctx = self.ctx_map.iter().copied().max().unwrap_or(0) as usize + 1;
        let seeds = bank.and_then(|b| b.kind(crate::seeds::Kind::Lossless, self.seed_key()));
        for c in 0..used_ctx {
            if bank.is_some() {
                crate::seeds::write_selector(w, seeds, self.seeds[c]);
                if self.seeds[c].is_some() {
                    continue;
                }
            }
            write_weights(w, &self.weights[c], self.prec[c]);
        }
    }

    pub(crate) fn read(
        r: &mut BitReader<'_>,
        depth: u32,
        bank: Option<&'static crate::seeds::Bank>,
    ) -> Result<Self, Error> {
        let k = r.read_refill(5);
        let m = r.read_refill(3);
        let cross = r.read_refill(1) != 0;
        if m > 4 || k > 16 {
            return Err(Error::Corrupt("bad token params"));
        }
        let tc = TokenCfg::v1(k, m, depth);
        if tc.k != k || tc.alphabet() > 256 {
            return Err(Error::Corrupt("bad token params"));
        }
        let ctx_shift = if depth > 16 { r.read_refill(5) } else { 0 };
        let mut ctx_map = [0u8; CLASSES];
        for c in ctx_map.iter_mut() {
            *c = r.read_refill(2) as u8;
        }
        let used_ctx = ctx_map.iter().copied().max().unwrap_or(0) as usize + 1;
        let a = tc.alphabet() as usize;
        let mut freq = vec![vec![0u32; a]; NCTX];
        let mut weights = vec![vec![0u32; a]; NCTX];
        let mut prec = vec![0u32; NCTX];
        let mut seeds = [None; NCTX];
        let key = [tc.k as u8, tc.m as u8, tc.depth as u8];
        let kind_seeds = bank.and_then(|b| b.kind(crate::seeds::Kind::Lossless, key));
        for c in 0..used_ctx {
            if bank.is_some()
                && let Some(i) = crate::seeds::read_selector(r, kind_seeds)?
            {
                let (wt, p) = kind_seeds.unwrap().weights(i, a)?;
                check_run_cap(&tc, &wt)?;
                freq[c] = weights_to_freq(&wt).ok_or(Error::Corrupt("empty table"))?;
                weights[c] = wt;
                prec[c] = p;
                seeds[c] = Some(i as u8);
                continue;
            }
            let (wt, p) = read_weights(r, a)?;
            check_run_cap(&tc, &wt)?;
            freq[c] = weights_to_freq(&wt).ok_or(Error::Corrupt("empty table"))?;
            weights[c] = wt;
            prec[c] = p;
        }
        for c in used_ctx..NCTX {
            freq[c][0] = M;
            weights[c][0] = 1;
        }
        Ok(Self {
            tc,
            ctx_map,
            freq,
            weights,
            prec,
            cross,
            ctx_shift,
            seeds,
        })
    }
}

/// Quantizes a count to `prec` significant bits (0 stays 0).
pub(crate) fn quantize_weight(c: u64, prec: u32) -> u32 {
    if c == 0 {
        return 0;
    }
    let c = c.min(u32::MAX as u64) as u32;
    let e = 32 - c.leading_zeros();
    if e <= prec {
        return c;
    }
    let sh = e - prec;
    // Round to nearest at the kept precision.
    let r = ((c >> (sh - 1)) + 1) >> 1;
    if r >> prec != 0 {
        (r >> 1) << (sh + 1)
    } else {
        r << sh
    }
}

/// Deterministic integer normalization of weights to sum `M`. Every
/// nonzero weight gets frequency >= 1.
pub(crate) fn weights_to_freq(wt: &[u32]) -> Option<Vec<u32>> {
    let mut f = vec![0u32; wt.len()];
    weights_to_freq_into(wt, &mut f, &mut Vec::new()).then_some(f)
}

/// [`weights_to_freq`] into `f` (`wt.len()` entries), `order` as scratch;
/// false when it would return `None`.
pub(crate) fn weights_to_freq_into(wt: &[u32], f: &mut [u32], order: &mut Vec<usize>) -> bool {
    order.clear();
    order.extend((0..wt.len()).filter(|&i| wt[i] > 0));
    if order.is_empty() || order.len() > M as usize {
        return false;
    }
    let total: u64 = wt.iter().map(|&v| v as u64).sum();
    for (fi, &v) in f.iter_mut().zip(wt) {
        *fi = if v == 0 {
            0
        } else {
            ((v as u64 * M as u64 / total) as u32).max(1)
        };
    }
    let mut sum: i64 = f.iter().map(|&v| v as i64).sum();
    if sum == M as i64 {
        return true;
    }
    // Largest weight first, ties by lower index.
    order.sort_by(|&a, &b| wt[b].cmp(&wt[a]).then(a.cmp(&b)));
    let mut i = 0;
    while sum < M as i64 {
        f[order[i % order.len()]] += 1;
        sum += 1;
        i += 1;
    }
    let mut i = 0;
    let mut guard = 0;
    while sum > M as i64 {
        let s = order[i % order.len()];
        if f[s] > 1 {
            f[s] -= 1;
            sum -= 1;
        }
        i += 1;
        guard += 1;
        if guard > 4 * M as usize * order.len() {
            return false;
        }
    }
    true
}

#[cfg(feature = "encode")]
pub(crate) fn write_weights(w: &mut BitWriter, wt: &[u32], prec: u32) {
    let last = wt.iter().rposition(|&v| v > 0).unwrap_or(0);
    w.put(last as u32, 8);
    w.put(prec - 1, 3);
    let mut prev_e = 0u32;
    for &v in &wt[..=last] {
        let e = if v == 0 { 0 } else { 32 - v.leading_zeros() };
        let d = e as i32 - prev_e as i32;
        let z = if d >= 0 {
            2 * d as u32
        } else {
            2 * (-d) as u32 - 1
        };
        put_gamma(w, z + 1);
        if e > 1 {
            let nb = (e - 1).min(prec - 1);
            // Mantissa bits below the leading one.
            w.put((v >> (e - 1 - nb)) & ((1 << nb) - 1), nb);
        }
        prev_e = e;
    }
}

pub(crate) fn read_weights(r: &mut BitReader<'_>, a: usize) -> Result<(Vec<u32>, u32), Error> {
    let mut wt = vec![0u32; a];
    let prec = read_weights_into(r, &mut wt)?;
    Ok((wt, prec))
}

/// [`read_weights`] into `wt` (the alphabet is `wt.len()`); returns the
/// precision.
pub(crate) fn read_weights_into(r: &mut BitReader<'_>, wt: &mut [u32]) -> Result<u32, Error> {
    let a = wt.len();
    wt.fill(0);
    let last = r.read_refill(8) as usize;
    let prec = r.read_refill(3) + 1;
    if last >= a {
        return Err(Error::Corrupt("table too long"));
    }
    let mut prev_e = 0i32;
    for v in wt[..=last].iter_mut() {
        let z = get_gamma(r)? - 1;
        let d = if z & 1 == 0 {
            (z / 2) as i32
        } else {
            -(z.div_ceil(2) as i32)
        };
        let e = prev_e + d;
        if !(0..=32).contains(&e) {
            return Err(Error::Corrupt("bad weight exponent"));
        }
        let e = e as u32;
        *v = if e == 0 {
            0
        } else if e == 1 {
            1
        } else {
            let nb = (e - 1).min(prec - 1);
            let mant = r.read_refill(nb);
            ((1u32 << nb) | mant) << (e - 1 - nb)
        };
        prev_e = e as i32;
    }
    Ok(prec)
}

pub(crate) fn gamma_len(v: u32) -> u32 {
    2 * (31 - v.leading_zeros()) + 1
}

/// Bits [`write_weights`] would use.
pub(crate) fn weights_cost(wt: &[u32], prec: u32) -> u64 {
    let last = wt.iter().rposition(|&v| v > 0).unwrap_or(0);
    let mut bits = 11u64;
    let mut prev_e = 0u32;
    for &v in &wt[..=last] {
        let e = if v == 0 { 0 } else { 32 - v.leading_zeros() };
        let d = e as i32 - prev_e as i32;
        let z = if d >= 0 {
            2 * d as u32
        } else {
            2 * (-d) as u32 - 1
        };
        bits += gamma_len(z + 1) as u64;
        if e > 1 {
            bits += (e - 1).min(prec - 1) as u64;
        }
        prev_e = e;
    }
    bits
}

/// Picks the weight precision minimizing table + data bits for `counts`.
pub(crate) fn choose_table(counts: &[u64]) -> (Vec<u32>, Vec<u32>, u32) {
    let mut best: Option<(f64, Vec<u32>, Vec<u32>, u32)> = None;
    for prec in 1..=8u32 {
        let wt: Vec<u32> = counts.iter().map(|&c| quantize_weight(c, prec)).collect();
        let Some(f) = weights_to_freq(&wt) else {
            continue;
        };
        let data: f64 = counts
            .iter()
            .zip(&f)
            .filter(|(c, _)| **c > 0)
            .map(|(&c, &q)| c as f64 * (LOG_M as f64 - crate::mathx::log2(q as f64)))
            .sum();
        let total = data + weights_cost(&wt, prec) as f64;
        if best.as_ref().is_none_or(|b| total < b.0) {
            best = Some((total, wt, f, prec));
        }
    }
    let (_, wt, f, p) = best.unwrap();
    (wt, f, p)
}

/// Token -> (base, extra bits, is_run).
#[inline(always)]
pub(crate) fn token_base(sym: u32, tc: &TokenCfg, vt: u32) -> (u32, u32, bool) {
    if sym < vt {
        let (b, nb) = hybrid_base(sym, tc.k, tc.m);
        (b, nb, false)
    } else {
        let (b, nb) = hybrid_base(sym - vt, tc.rk, tc.rm);
        (b, nb, true)
    }
}

// ---- encoder -----------------------------------------------------------------

/// `Sym::ctx` of a bits-only entry: `tok` is its pull phase, `nb` (1..=16)
/// bits of `extra` are pulled after the step's symbol.
#[cfg(feature = "encode")]
pub(crate) const RAW_CTX: u16 = u16::MAX;

/// One coded symbol of a lane.
#[derive(Clone, Copy)]
#[cfg(feature = "encode")]
pub(crate) struct Sym {
    pub step: u32,
    pub ctx: u16,
    pub tok: u8,
    pub nb: u8,
    pub extra: u32,
}

/// One forward scan of lane `l` of a step-major residual plane: context
/// classes plus run/value tokens, emitted through `emit` as
/// `(step, ctx, tok, nb, extra)`. Lane position (r, x) maps to
/// `u[r * lw * NL + x * NL + l]`; no intermediate vectors are built —
/// west/north/cross terms and the trailing-zero run are read directly.
/// `ctx` of each symbol holds the raw context class (0..CLASSES); map it
/// through the model's `ctx_map` before coding.
#[allow(clippy::too_many_arguments)]
#[inline]
#[cfg(feature = "encode")]
fn lane_scan<const STEP_MAJOR: bool, const CONTEXT: bool>(
    u: &[u32],
    row_stride: usize,
    cross: Option<&[u32]>,
    lw: usize,
    rows: usize,
    width: usize,
    l: usize,
    tc: &TokenCfg,
    run_min: u32,
    ctx_shift: u32,
    emit: &mut impl FnMut(u32, u16, u8, u8, u32),
) {
    if width == 0 {
        return;
    }
    let n = rows * width;
    // Element stride between lane rows and past one row of this lane.
    let row = if STEP_MAJOR { lw * NL } else { row_stride };
    let element = if STEP_MAJOR { NL } else { 1 };
    let start = if STEP_MAJOR { l } else { l * lw };
    let wrap = row - width * element;
    let mut j = 0usize;
    let (mut r, mut x) = (0usize, 0usize);
    let mut at = start;
    let mut step = 0u32; // r * lw + x
    // Zero-run length known at the current position: cached across the
    // single-symbol emissions below so a long zero run that never becomes
    // a run token is not rescanned once per element (that would be
    // quadratic where the old materialized `zrun` array was linear).
    let mut zleft = 0u32;
    // A lookahead is only useful when its result can reach `run_min`.
    let want_runs = run_min > 0 && run_min <= RUN_CAP;
    while j < n {
        let val = u[at];
        // Trailing zero count at this position, capped (previously a
        // materialized backward `zrun` array; same bound semantics).
        let mut len = zleft;
        if val == 0 && len == 0 && want_runs {
            let cap = (n - j).min(RUN_CAP as usize) as u32;
            let (mut x2, mut a2) = (x, at);
            loop {
                len += 1;
                if len == cap {
                    break;
                }
                x2 += 1;
                a2 += element;
                if x2 == width {
                    x2 = 0;
                    a2 += wrap;
                }
                if u[a2] != 0 {
                    break;
                }
            }
        }
        let cls = if CONTEXT {
            let uw = if x > 0 {
                ctx_term(u[at - element], ctx_shift)
            } else {
                0
            };
            let un = if r > 0 {
                ctx_term(u[at - row], ctx_shift)
            } else {
                0
            };
            let mut sum = ctx_sum(uw, un, x, r);
            if let Some(cr) = cross {
                sum += ctx_term(cr[at], ctx_shift);
            }
            class_of(sum) as u8
        } else {
            0
        };
        if run_min > 0 && len >= run_min {
            let (t, e, nb) = tc.run(len);
            emit(step, cls as u16, t as u8, nb as u8, e);
            j += len as usize;
            x += len as usize;
            at += len as usize * element;
            step += len;
            zleft = 0;
            if x >= width {
                let q = x / width;
                r += q;
                x -= q * width;
                step += q as u32 * (lw - width) as u32;
                at = r * row + x * element + start;
            }
            continue;
        }
        let (t, e, nb) = tc.value(val);
        emit(step, cls as u16, t as u8, nb as u8, e);
        j += 1;
        x += 1;
        at += element;
        step += 1;
        // A single-symbol zero emission leaves a known `len - 1` suffix:
        // exact when the lookahead found the run's end, and also at the
        // `n - j` plane-end cap (the only cap reachable below `run_min`,
        // since `len >= run_min` would have taken the run branch above).
        zleft = len.saturating_sub(1);
        if x == width {
            x = 0;
            r += 1;
            step += (lw - width) as u32;
            at += wrap;
        }
    }
}

/// Builds the symbol list of lane `l` from a step-major residual plane.
/// `ctx` of each symbol holds the raw context class (0..CLASSES); map it
/// through the model's `ctx_map` before coding.
#[allow(clippy::too_many_arguments)]
#[cfg(all(any(test, feature = "lab"), feature = "encode"))]
pub(crate) fn lane_symbols(
    u: &[u32],
    cross: Option<&[u32]>,
    lw: usize,
    rows: usize,
    width: usize,
    l: usize,
    tc: &TokenCfg,
    run_min: u32,
    ctx_shift: u32,
    out: &mut Vec<Sym>,
) {
    lane_scan::<true, true>(
        u,
        0,
        cross,
        lw,
        rows,
        width,
        l,
        tc,
        run_min,
        ctx_shift,
        &mut |step, ctx, tok, nb, extra| {
            out.push(Sym {
                step,
                ctx,
                tok,
                nb,
                extra,
            });
        },
    );
}

/// Row-strided residual scan with full neighbor/cross context classes, avoiding
/// any `step_major` transpose allocation. Symbol order, context classes and run
/// limits match [`lane_symbols`] identically.
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "encode")]
pub(crate) fn lane_symbols_rows(
    u: &[u32],
    row_stride: usize,
    cross: Option<&[u32]>,
    lw: usize,
    rows: usize,
    width: usize,
    l: usize,
    tc: &TokenCfg,
    run_min: u32,
    ctx_shift: u32,
    out: &mut Vec<Sym>,
) {
    lane_scan::<false, true>(
        u,
        row_stride,
        cross,
        lw,
        rows,
        width,
        l,
        tc,
        run_min,
        ctx_shift,
        &mut |step, ctx, tok, nb, extra| {
            out.push(Sym {
                step,
                ctx,
                tok,
                nb,
                extra,
            });
        },
    );
}

/// Row-strided residual scan for a model whose classes all select one table.
/// Symbol steps and run limits match [`lane_symbols`]; context classes are
/// deliberately zero because they cannot change a fixed model's frequencies.
/// A row's lane l begins at `l * lw`; `row_stride` is measured in u32 words.
#[allow(clippy::too_many_arguments)]
#[cfg(all(test, feature = "encode"))]
pub(crate) fn lane_symbols_rows_flat(
    u: &[u32],
    row_stride: usize,
    lw: usize,
    rows: usize,
    width: usize,
    l: usize,
    tc: &TokenCfg,
    run_min: u32,
    out: &mut Vec<Sym>,
) {
    lane_scan::<false, false>(
        u,
        row_stride,
        None,
        lw,
        rows,
        width,
        l,
        tc,
        run_min,
        0,
        &mut |step, ctx, tok, nb, extra| {
            out.push(Sym {
                step,
                ctx,
                tok,
                nb,
                extra,
            })
        },
    );
}

/// Same walk as [`lane_symbols`] but only counts `(ctx, tok)` occurrences:
/// the histogram pass needs no symbol list at all.
#[allow(clippy::too_many_arguments)]
#[cfg(all(test, feature = "encode"))]
pub(crate) fn lane_hist(
    u: &[u32],
    cross: Option<&[u32]>,
    lw: usize,
    rows: usize,
    width: usize,
    l: usize,
    tc: &TokenCfg,
    run_min: u32,
    ctx_shift: u32,
    hist: &mut [Vec<u64>],
) {
    lane_scan::<true, true>(
        u,
        0,
        cross,
        lw,
        rows,
        width,
        l,
        tc,
        run_min,
        ctx_shift,
        &mut |_step, ctx, tok, _nb, _extra| {
            hist[ctx as usize][tok as usize] += 1;
        },
    );
}

/// Row-strided equivalent of [`lane_hist`], avoiding `step_major` transposes.
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "encode")]
pub(crate) fn lane_hist_rows(
    u: &[u32],
    row_stride: usize,
    cross: Option<&[u32]>,
    lw: usize,
    rows: usize,
    width: usize,
    l: usize,
    tc: &TokenCfg,
    run_min: u32,
    ctx_shift: u32,
    hist: &mut [Vec<u64>],
) {
    lane_scan::<false, true>(
        u,
        row_stride,
        cross,
        lw,
        rows,
        width,
        l,
        tc,
        run_min,
        ctx_shift,
        &mut |_step, ctx, tok, _nb, _extra| {
            hist[ctx as usize][tok as usize] += 1;
        },
    );
}

/// Context sum of (clamped) terms for lane column `x`, stripe row `row`.
#[inline(always)]
pub(crate) fn ctx_sum(uw: u32, un: u32, x: usize, row: usize) -> u32 {
    match (x > 0, row > 0) {
        (true, true) => uw + un,
        (true, false) => 2 * uw,
        (false, true) => 2 * un,
        (false, false) => 0,
    }
}

/// Encoder-only information shared by every stripe using the same model.
/// Kept separate from `Model` so decoders never build reciprocals.
#[cfg(feature = "encode")]
pub(crate) struct EncodeTables<'a> {
    /// Context class to table; `None`: symbol contexts index tables directly.
    ctx_map: Option<&'a [u8]>,
    /// Flat `[ctx * alphabet + tok]` records (one indexed load per symbol).
    alphabet: usize,
    symbols: Vec<EncSymbol>,
}

#[cfg(feature = "encode")]
struct EncSymbol {
    freq: u32,
    cum: u32,
    recip: u64,
}

#[cfg(feature = "encode")]
impl<'a> EncodeTables<'a> {
    pub(crate) fn new(freq: &[Vec<u32>], ctx_map: &'a [u8]) -> Self {
        Self::with_map(freq, Some(ctx_map))
    }

    /// Tables indexed by the symbols' contexts themselves (bilevel stripes).
    pub(crate) fn direct(freq: &[Vec<u32>]) -> Self {
        Self::with_map(freq, None)
    }

    /// `model`'s tables indexed by class: each of the [`CLASSES`] classes gets
    /// a copy of the table its context map names. The symbols coded are
    /// identical to [`new`](Self::new)'s, but the tables borrow nothing, so a
    /// caller that codes many groups with one model builds them once.
    pub(crate) fn per_class(model: &Model) -> EncodeTables<'static> {
        let freq: Vec<Vec<u32>> = model
            .ctx_map
            .iter()
            .map(|&ctx| model.freq[ctx as usize].clone())
            .collect();
        EncodeTables::direct(&freq)
    }

    /// Lab selector: exact states and byte count, without collecting/sorting
    /// renormalization words. Qualified against the ordinary writer before use.
    #[cfg(feature = "encode")]
    pub(crate) fn encoded_size(
        &self,
        lanes: &[Vec<Sym>; NL],
        active: usize,
    ) -> Option<([u32; NL], usize)> {
        let mut states = [RANS_L; NL];
        let mut words = 0usize;
        for (l, lane) in lanes.iter().enumerate() {
            let mut x = RANS_L;
            for y in lane.iter().rev() {
                if y.ctx == RAW_CTX {
                    let n = y.nb as u32;
                    if x as u64 >= ((RANS_L as u64 >> n) << 16) {
                        words += 1;
                        x >>= 16;
                    }
                    x = (x << n) | y.extra;
                    continue;
                }
                let nb = y.nb as u32;
                let n2 = nb.saturating_sub(16);
                if n2 > 0 {
                    if x as u64 >= ((RANS_L as u64 >> n2) << 16) {
                        words += 1;
                        x >>= 16;
                    }
                    x = (x << n2) | (y.extra >> 16);
                }
                let n1 = nb.min(16);
                if n1 > 0 {
                    if x as u64 >= ((RANS_L as u64 >> n1) << 16) {
                        words += 1;
                        x >>= 16;
                    }
                    x = (x << n1) | (y.extra & crate::mask_of(n1));
                }
                let ctx = self
                    .ctx_map
                    .map_or(y.ctx as usize, |m| m[y.ctx as usize] as usize);
                let symbol = &self.symbols[ctx * self.alphabet + y.tok as usize];
                let f = symbol.freq;
                if f == 0 {
                    return None;
                }
                if x as u64 >= (f as u64) << 22 {
                    words += 1;
                    x >>= 16;
                }
                let q = if f == 1 {
                    x
                } else {
                    ((x as u128 * symbol.recip as u128) >> 64) as u32
                };
                x = (q << LOG_M) + (x - q * f) + symbol.cum;
            }
            states[l] = x;
        }
        Some((states, 4 * active + 2 * words))
    }

    fn with_map(freq: &[Vec<u32>], ctx_map: Option<&'a [u8]>) -> Self {
        let alphabet = freq.first().map_or(0, |t| t.len());
        let mut symbols = Vec::with_capacity(freq.len() * alphabet);
        for table in freq {
            assert!(table.len() == alphabet);
            let mut cum = 0;
            symbols.extend(table.iter().map(|&freq| {
                let symbol = EncSymbol {
                    freq,
                    cum,
                    // Exact division for 32-bit x: (x * ceil(2^64 / f)) >> 64.
                    recip: if freq <= 1 {
                        0
                    } else {
                        (u64::MAX / freq as u64) + 1
                    },
                };
                cum += freq;
                symbol
            }));
        }
        Self {
            ctx_map,
            alphabet,
            symbols,
        }
    }
}

/// Reusable per-lane renormalization buffer across [`encode_lanes_tables_scratch`] calls.
#[cfg(feature = "encode")]
#[derive(Default)]
pub(crate) struct RansScratch {
    lane_words: [Vec<(u32, u8, u16)>; NL],
}

/// Encodes the 16 lanes of one stripe slot: active initial states followed
/// by renormalization words in decoder consumption order.
/// Words are emitted in (step, phase, lane) order.
#[cfg(feature = "encode")]
pub(crate) fn encode_lanes_tables(
    tables: &EncodeTables<'_>,
    lanes: &[Vec<Sym>; NL],
    active: usize,
    out: &mut Vec<u8>,
) {
    let mut scratch = RansScratch::default();
    encode_lanes_tables_scratch(tables, lanes, active, out, &mut scratch);
}

/// [`encode_lanes_tables`] reusing caller-owned renormalization buffers across calls.
#[cfg(feature = "encode")]
pub(crate) fn encode_lanes_tables_scratch(
    tables: &EncodeTables<'_>,
    lanes: &[Vec<Sym>; NL],
    active: usize,
    out: &mut Vec<u8>,
    scratch: &mut RansScratch,
) {
    let mut states = [0u32; NL];
    // Per lane: (step, phase, word). Symbols are visited in reverse decode
    // order (states only propagate backwards), and within one symbol the
    // pull phases are pushed descending, so every lane's list is sorted by
    // its packed key in *descending* order — see the merge below.
    let lane_words = &mut scratch.lane_words;
    for lw in lane_words.iter_mut() {
        lw.clear();
    }
    // The sort key emitted per (lane, word): (step, phase, lane).
    let key =
        |st: u32, ph: u8, l: usize| -> u64 { ((((st as u64) << 4) | ph as u64) << 4) | l as u64 };
    // Every lane's list must be strictly descending in the packed key for
    // the merge below to equal the old sort's order. For `ph < 16` that is
    // exactly the lexicographic (step, phase) order, which is what is
    // tracked here; a `ph >= 16` would break the packing equivalence, so
    // it also marks the lists unordered (the sort handles it identically).
    let mut ordered = true;
    for l in 0..NL {
        let lw = &mut lane_words[l];
        let mut x = RANS_L;
        let mut last = (u32::MAX, u8::MAX);
        for y in lanes[l].iter().rev() {
            if y.ctx == RAW_CTX {
                let n = y.nb as u32;
                debug_assert!((1..=16).contains(&n));
                let x_max = ((RANS_L as u64) >> n) << 16;
                if x as u64 >= x_max {
                    ordered &= y.tok < 16 && (y.step, y.tok) < last;
                    last = (y.step, y.tok);
                    lw.push((y.step, y.tok, x as u16));
                    x >>= 16;
                }
                x = (x << n) | y.extra;
                continue;
            }
            // Extra bits: the decoder pulls min(nb, 16) low bits (phase 1),
            // then nb - 16 high bits (phase 2); encode in reverse.
            let nb = y.nb as u32;
            let n2 = nb.saturating_sub(16);
            if n2 > 0 {
                let x_max = ((RANS_L as u64) >> n2) << 16;
                if x as u64 >= x_max {
                    ordered &= (y.step, 2) < last;
                    last = (y.step, 2);
                    lw.push((y.step, 2, x as u16));
                    x >>= 16;
                }
                x = (x << n2) | (y.extra >> 16);
            }
            let n1 = nb.min(16);
            if n1 > 0 {
                let x_max = ((RANS_L as u64) >> n1) << 16;
                if x as u64 >= x_max {
                    ordered &= (y.step, 1) < last;
                    last = (y.step, 1);
                    lw.push((y.step, 1, x as u16));
                    x >>= 16;
                }
                x = (x << n1) | (y.extra & crate::mask_of(n1));
            }
            let ctx = tables
                .ctx_map
                .map_or(y.ctx as usize, |m| m[y.ctx as usize] as usize);
            let symbol = &tables.symbols[ctx * tables.alphabet + y.tok as usize];
            let f = symbol.freq;
            let c = symbol.cum;
            debug_assert!(f > 0);
            let x_max = (f as u64) << 22; // (RANS_L >> LOG_M) << 16 == 2^22
            if x as u64 >= x_max {
                ordered &= (y.step, 0) < last;
                last = (y.step, 0);
                lw.push((y.step, 0, x as u16));
                x >>= 16;
            }
            let q = if f == 1 {
                x
            } else {
                ((x as u128 * symbol.recip as u128) >> 64) as u32
            };
            debug_assert_eq!(q, x / f);
            x = (q << LOG_M) + (x - q * f) + c;
        }
        states[l] = x;
    }
    let total: usize = lane_words.iter().map(|v| v.len()).sum();
    out.reserve(4 * active + 2 * total);
    // Only lanes that can hold symbols carry an initial state.
    for (l, &x) in states.iter().enumerate() {
        if l < active {
            out.extend_from_slice(&x.to_le_bytes());
        } else {
            debug_assert_eq!(x, RANS_L);
        }
    }
    if ordered {
        // Words in (step, phase, lane) order: every lane list is
        // strictly descending in the packed key, so a 16-way merge of the
        // lists read backwards produces the global ascending order with no
        // materialized (u64, u16) vector and no sort. A 16-leaf winner tree
        // (leaves at tree[16..32]) resolves each word in four comparisons.
        let mut pos: [usize; NL] = core::array::from_fn(|l| lane_words[l].len());
        let mut leaf = [u64::MAX; NL];
        for l in 0..NL {
            if pos[l] > 0 {
                let &(st, ph, _) = &lane_words[l][pos[l] - 1];
                leaf[l] = key(st, ph, l);
            }
        }
        let mut tree = [0usize; 32];
        for (l, slot) in tree[16..32].iter_mut().enumerate() {
            *slot = l;
        }
        for i in (1..16).rev() {
            tree[i] = if leaf[tree[2 * i]] <= leaf[tree[2 * i + 1]] {
                tree[2 * i]
            } else {
                tree[2 * i + 1]
            };
        }
        for _ in 0..total {
            let l = tree[1];
            pos[l] -= 1;
            let (_, _, w) = lane_words[l][pos[l]];
            out.extend_from_slice(&w.to_le_bytes());
            leaf[l] = if pos[l] > 0 {
                let &(st, ph, _) = &lane_words[l][pos[l] - 1];
                key(st, ph, l)
            } else {
                u64::MAX
            };
            let mut i = (16 + l) / 2;
            while i > 0 {
                tree[i] = if leaf[tree[2 * i]] <= leaf[tree[2 * i + 1]] {
                    tree[2 * i]
                } else {
                    tree[2 * i + 1]
                };
                i /= 2;
            }
        }
        return;
    }
    // Fallback (unreachable for in-order symbol lists): materialize and sort.
    let mut all: Vec<(u64, u16)> = Vec::with_capacity(total);
    for (l, lw) in lane_words.iter().enumerate() {
        for &(st, ph, w) in lw {
            all.push((key(st, ph, l), w));
        }
    }
    all.sort_unstable_by_key(|e| e.0);
    for (_, w) in all {
        out.extend_from_slice(&w.to_le_bytes());
    }
}

// ---- decoder -----------------------------------------------------------------

/// Persistent per-(stripe, slot) lane decoder state.
pub(crate) struct LaneState {
    pub x: [u32; NL],
    pub run: [u32; NL],
    /// Next word index (u16 units) into `words`.
    pub wp: usize,
}

/// A stripe slot's word stream, read in place. Reads past the end come
/// from a zero-padded copy of the tail so SIMD refills may over-read.
pub(crate) struct Words<'a> {
    body: &'a [u8],
    /// Number of real words.
    pub len: usize,
    tail_start: usize,
    tail: [u8; 128],
}

impl<'a> Words<'a> {
    pub(crate) fn span(&self, source: &[u8]) -> Result<core::ops::Range<usize>, Error> {
        crate::owned::span(source, self.body)
    }

    pub(crate) fn from_bytes(b: &'a [u8], active: usize) -> Result<(Self, [u32; NL]), Error> {
        if b.len() < 4 * active || !(b.len() - 4 * active).is_multiple_of(2) {
            return Err(Error::Corrupt("short lane stream"));
        }
        let mut x = [RANS_L; NL];
        for (l, xi) in x.iter_mut().enumerate().take(active) {
            *xi = u32::from_le_bytes(b[4 * l..4 * l + 4].try_into().unwrap());
            if *xi < RANS_L {
                return Err(Error::Corrupt("bad initial state"));
            }
        }
        Ok((Self::from_body(&b[4 * active..])?, x))
    }

    /// Shared word reader for ordinary and compact restart-state envelopes.
    pub(crate) fn from_body(body: &'a [u8]) -> Result<Self, Error> {
        if !body.len().is_multiple_of(2) {
            return Err(Error::Corrupt("odd lane word stream"));
        }
        let len = body.len() / 2;
        let tail_start = len.saturating_sub(16);
        let mut tail = [0u8; 128];
        tail[..2 * (len - tail_start)].copy_from_slice(&body[2 * tail_start..]);
        Ok(Self {
            body,
            len,
            tail_start,
            tail,
        })
    }

    /// 16 words (32 bytes, little-endian) starting at word `wp`.
    #[inline(always)]
    pub(crate) fn chunk(&self, wp: usize) -> &[u8; 32] {
        let off = wp.saturating_mul(2);
        if let Some(c) = self.body.get(off..).and_then(|s| s.first_chunk::<32>()) {
            return c;
        }
        // Tail covers words [tail_start, tail_start + 64); clamp corrupt overruns.
        let o = wp.saturating_sub(self.tail_start).min(48);
        self.tail[2 * o..2 * o + 32].try_into().unwrap()
    }

    #[inline(always)]
    pub(crate) fn word(&self, wp: usize) -> u16 {
        let c = self.chunk(wp);
        u16::from_le_bytes([c[0], c[1]])
    }
}

/// One scalar renormalization step: an `x < RANS_L` lane pulls the next
/// stream word. Shared by the token residual row decoder and the lab's
/// bilevel row decoder; SIMD kernels refill through their mask loops.
#[inline(always)]
pub(crate) fn renorm_one(x: &mut u32, wp: &mut usize, words: &Words<'_>) {
    if *x < RANS_L {
        *x = (*x << 16) | words.word(*wp) as u32;
        *wp += 1;
    }
}

/// Scalar reference: decodes one row (lw steps) of one slot.
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_row_scalar<S: crate::px::Px>(
    md: &DecModel,
    words: &Words<'_>,
    st: &mut LaneState,
    widths: &[u32; NL],
    lw: usize,
    row: usize,
    prev: &[S],
    cross: Option<&[S]>,
    out: &mut [S],
) {
    let mut sym_l = [0u32; NL];
    let mut extra = [0u32; NL];
    let mut act = [false; NL];
    for x in 0..lw {
        // Token phase.
        for l in 0..NL {
            act[l] = false;
            if x as u32 >= widths[l] {
                out[x * NL + l] = S::put(0);
                continue;
            }
            if st.run[l] > 0 {
                st.run[l] -= 1;
                out[x * NL + l] = S::put(0);
                continue;
            }
            act[l] = true;
            let sh = md.ctx_shift;
            let uw = if x > 0 {
                ctx_term(out[(x - 1) * NL + l].get(), sh)
            } else {
                0
            };
            let un = if row > 0 {
                ctx_term(prev[x * NL + l].get(), sh)
            } else {
                0
            };
            let mut sum = ctx_sum(uw, un, x, row);
            if let Some(c) = cross {
                sum += ctx_term(c[x * NL + l].get(), sh);
            }
            let ctx = md.ctx_map[class_of(sum)] as usize & (NCTX - 1);
            let xs = st.x[l];
            let e = md.dec[(ctx << LOG_M) | (xs & (M - 1)) as usize];
            let mut nx = ((e & 1023) + 1) * (xs >> LOG_M) + ((e >> 10) & 1023);
            renorm_one(&mut nx, &mut st.wp, words);
            st.x[l] = nx;
            sym_l[l] = (e >> 20) & 255;
            extra[l] = 0;
        }
        // Extra bits, low part (<= 16 bits).
        for l in 0..NL {
            if !act[l] {
                continue;
            }
            let nb = md.tok[sym_l[l] as usize].1.min(16);
            if nb > 0 {
                extra[l] = st.x[l] & crate::mask_of(nb);
                st.x[l] >>= nb;
                renorm_one(&mut st.x[l], &mut st.wp, words);
            }
        }
        // Extra bits, high part (only beyond 16 bits; extended tier).
        for l in 0..NL {
            if !act[l] {
                continue;
            }
            let nb = md.tok[sym_l[l] as usize].1;
            if nb > 16 {
                let n2 = nb - 16;
                extra[l] |= (st.x[l] & crate::mask_of(n2)) << 16;
                st.x[l] >>= n2;
                renorm_one(&mut st.x[l], &mut st.wp, words);
            }
            let (base, _, is_run) = md.tok[sym_l[l] as usize];
            let u = base.wrapping_add(extra[l]);
            if is_run {
                st.run[l] = u;
                out[x * NL + l] = S::put(0);
            } else {
                out[x * NL + l] = S::put(u);
            }
        }
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) mod avx2 {
    use super::*;
    use archmage::prelude::*;

    /// Rank LUT for mask-driven refill.
    static RANK8: [[u32; 8]; 256] = {
        let mut t = [[0u32; 8]; 256];
        let mut m = 0;
        while m < 256 {
            let mut i = 0;
            let mut r = 0u32;
            while i < 8 {
                t[m][i] = r;
                if m & (1 << i) != 0 {
                    r += 1;
                }
                i += 1;
            }
            m += 1;
        }
        t
    };

    struct K {
        cmapv: __m256i,
        vt: __m256i,
        vtm1: __m256i,
        dlim_v: __m256i,
        dlim_r: __m256i,
        mpow: __m256i,
        mmask: __m256i,
    }

    #[rite(import_intrinsics)]
    #[inline(always)]
    pub(crate) fn refill(
        _t: X64V3Token,
        x: __m256i,
        need: __m256i,
        w: &Words<'_>,
        wp: &mut usize,
    ) -> __m256i {
        let m = _mm256_movemask_ps(_mm256_castsi256_ps(need)) as usize;
        let c = w.chunk(*wp);
        let wv: &[u8; 16] = c[..16].try_into().unwrap();
        let v = _mm256_cvtepu16_epi32(_mm_loadu_si128(wv));
        let v = _mm256_permutevar8x32_epi32(v, _mm256_loadu_si256(&RANK8[m]));
        *wp += m.count_ones() as usize;
        _mm256_blendv_epi8(x, _mm256_or_si256(_mm256_slli_epi32::<16>(x), v), need)
    }

    /// Context + table index for one half; also returns `act`.
    #[rite(import_intrinsics)]
    #[inline(always)]
    fn token_idx(
        _t: X64V3Token,
        k: &K,
        x: __m256i,
        run: &mut __m256i,
        sum: __m256i,
        inw: __m256i,
    ) -> (__m256i, __m256i) {
        let zero = _mm256_setzero_si256();
        let in_run = _mm256_and_si256(_mm256_cmpgt_epi32(*run, zero), inw);
        *run = _mm256_add_epi32(*run, in_run);
        let act = _mm256_andnot_si256(in_run, inw);
        let ex = _mm256_srli_epi32::<23>(_mm256_castps_si256(_mm256_cvtepi32_ps(sum)));
        let cls = _mm256_min_epi32(
            _mm256_max_epi32(_mm256_sub_epi32(ex, _mm256_set1_epi32(126)), zero),
            _mm256_set1_epi32(15),
        );
        let ctx = _mm256_and_si256(_mm256_shuffle_epi8(k.cmapv, cls), _mm256_set1_epi32(3));
        let idx = _mm256_or_si256(
            _mm256_slli_epi32::<10>(ctx),
            _mm256_and_si256(x, _mm256_set1_epi32(1023)),
        );
        (act, idx)
    }

    /// Context + table index when all 8 lanes are active (no runs, xi < min_w).
    #[rite(import_intrinsics)]
    #[inline(always)]
    fn token_idx_all(_t: X64V3Token, k: &K, x: __m256i, sum: __m256i) -> __m256i {
        let zero = _mm256_setzero_si256();
        let ex = _mm256_srli_epi32::<23>(_mm256_castps_si256(_mm256_cvtepi32_ps(sum)));
        let cls = _mm256_min_epi32(
            _mm256_max_epi32(_mm256_sub_epi32(ex, _mm256_set1_epi32(126)), zero),
            _mm256_set1_epi32(15),
        );
        let ctx = _mm256_and_si256(_mm256_shuffle_epi8(k.cmapv, cls), _mm256_set1_epi32(3));
        _mm256_or_si256(
            _mm256_slli_epi32::<10>(ctx),
            _mm256_and_si256(x, _mm256_set1_epi32(1023)),
        )
    }

    /// State update after the gather.
    #[rite(import_intrinsics)]
    #[inline(always)]
    fn token_update(
        t: X64V3Token,
        e: __m256i,
        act: __m256i,
        w: &Words<'_>,
        wp: &mut usize,
        x: &mut __m256i,
    ) {
        let zero = _mm256_setzero_si256();
        let f = _mm256_add_epi32(
            _mm256_and_si256(e, _mm256_set1_epi32(1023)),
            _mm256_set1_epi32(1),
        );
        let bias = _mm256_and_si256(_mm256_srli_epi32::<10>(e), _mm256_set1_epi32(1023));
        let nx = _mm256_add_epi32(_mm256_mullo_epi32(f, _mm256_srli_epi32::<10>(*x)), bias);
        let xx = _mm256_blendv_epi8(*x, nx, act);
        let need = _mm256_and_si256(act, _mm256_cmpeq_epi32(_mm256_srli_epi32::<16>(xx), zero));
        *x = refill(t, xx, need, w, wp);
    }

    /// State update when all 8 lanes are active.
    #[rite(import_intrinsics)]
    #[inline(always)]
    fn token_update_all(t: X64V3Token, e: __m256i, w: &Words<'_>, wp: &mut usize, x: &mut __m256i) {
        let zero = _mm256_setzero_si256();
        let f = _mm256_add_epi32(
            _mm256_and_si256(e, _mm256_set1_epi32(1023)),
            _mm256_set1_epi32(1),
        );
        let bias = _mm256_and_si256(_mm256_srli_epi32::<10>(e), _mm256_set1_epi32(1023));
        let nx = _mm256_add_epi32(_mm256_mullo_epi32(f, _mm256_srli_epi32::<10>(*x)), bias);
        let need = _mm256_cmpeq_epi32(_mm256_srli_epi32::<16>(nx), zero);
        *x = refill(t, nx, need, w, wp);
    }

    #[rite(import_intrinsics)]
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn extra_phase(
        t: X64V3Token,
        k: &K,
        w: &Words<'_>,
        wp: &mut usize,
        x: &mut __m256i,
        run: &mut __m256i,
        act: __m256i,
        e: __m256i,
    ) -> __m256i {
        let zero = _mm256_setzero_si256();
        let one = _mm256_set1_epi32(1);
        let nb = _mm256_srli_epi32::<28>(e);
        let sym = _mm256_and_si256(_mm256_srli_epi32::<20>(e), _mm256_set1_epi32(255));
        let ebits = _mm256_and_si256(*x, _mm256_sub_epi32(_mm256_sllv_epi32(one, nb), one));
        let xx = _mm256_blendv_epi8(*x, _mm256_srlv_epi32(*x, nb), act);
        let need = _mm256_and_si256(act, _mm256_cmpeq_epi32(_mm256_srli_epi32::<16>(xx), zero));
        *x = refill(t, xx, need, w, wp);
        let is_run = _mm256_cmpgt_epi32(sym, k.vtm1);
        let tt = _mm256_sub_epi32(sym, _mm256_and_si256(is_run, k.vt));
        let dlim = _mm256_blendv_epi8(k.dlim_v, k.dlim_r, is_run);
        let direct = _mm256_cmpgt_epi32(dlim, tt);
        let hb = _mm256_sllv_epi32(_mm256_or_si256(k.mpow, _mm256_and_si256(tt, k.mmask)), nb);
        let base = _mm256_blendv_epi8(hb, tt, direct);
        let u = _mm256_add_epi32(base, ebits);
        let run_now = _mm256_and_si256(act, is_run);
        *run = _mm256_blendv_epi8(*run, u, run_now);
        _mm256_andnot_si256(run_now, _mm256_and_si256(act, u))
    }

    #[rite(import_intrinsics)]
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn extra_phase_all(
        t: X64V3Token,
        k: &K,
        w: &Words<'_>,
        wp: &mut usize,
        x: &mut __m256i,
        run: &mut __m256i,
        e: __m256i,
    ) -> __m256i {
        let zero = _mm256_setzero_si256();
        let one = _mm256_set1_epi32(1);
        let nb = _mm256_srli_epi32::<28>(e);
        let sym = _mm256_and_si256(_mm256_srli_epi32::<20>(e), _mm256_set1_epi32(255));
        let ebits = _mm256_and_si256(*x, _mm256_sub_epi32(_mm256_sllv_epi32(one, nb), one));
        let xx = _mm256_srlv_epi32(*x, nb);
        let need = _mm256_cmpeq_epi32(_mm256_srli_epi32::<16>(xx), zero);
        *x = refill(t, xx, need, w, wp);
        let is_run = _mm256_cmpgt_epi32(sym, k.vtm1);
        let tt = _mm256_sub_epi32(sym, _mm256_and_si256(is_run, k.vt));
        let dlim = _mm256_blendv_epi8(k.dlim_v, k.dlim_r, is_run);
        let direct = _mm256_cmpgt_epi32(dlim, tt);
        let hb = _mm256_sllv_epi32(_mm256_or_si256(k.mpow, _mm256_and_si256(tt, k.mmask)), nb);
        let base = _mm256_blendv_epi8(hb, tt, direct);
        let u = _mm256_add_epi32(base, ebits);
        *run = _mm256_and_si256(is_run, u);
        _mm256_andnot_si256(is_run, u)
    }

    /// Extra phase when no active lane has extra bits: tokens are direct.
    #[rite(import_intrinsics)]
    #[inline(always)]
    fn no_extra(_t: X64V3Token, k: &K, run: &mut __m256i, act: __m256i, e: __m256i) -> __m256i {
        let sym = _mm256_and_si256(_mm256_srli_epi32::<20>(e), _mm256_set1_epi32(255));
        let is_run = _mm256_cmpgt_epi32(sym, k.vtm1);
        let u = _mm256_sub_epi32(sym, _mm256_and_si256(is_run, k.vt));
        let run_now = _mm256_and_si256(act, is_run);
        *run = _mm256_blendv_epi8(*run, u, run_now);
        _mm256_andnot_si256(run_now, _mm256_and_si256(act, u))
    }

    /// Extra phase when all 8 lanes are active and have nb == 0 (top 4 bits of e are 0).
    #[rite(import_intrinsics)]
    #[inline(always)]
    fn no_extra_all(_t: X64V3Token, k: &K, run: &mut __m256i, e: __m256i) -> __m256i {
        let sym = _mm256_srli_epi32::<20>(e);
        let is_run = _mm256_cmpgt_epi32(sym, k.vtm1);
        let u = _mm256_sub_epi32(sym, _mm256_and_si256(is_run, k.vt));
        *run = _mm256_and_si256(is_run, u);
        _mm256_andnot_si256(is_run, u)
    }

    #[rite(import_intrinsics)]
    #[inline(always)]
    fn load16_pair(_t: X64V3Token, p: &[u16; NL]) -> (__m256i, __m256i) {
        let v = _mm256_loadu_si256(p);
        let lo = _mm256_cvtepu16_epi32(_mm256_castsi256_si128(v));
        let hi = _mm256_cvtepu16_epi32(_mm256_extracti128_si256::<1>(v));
        (lo, hi)
    }

    #[rite(import_intrinsics)]
    #[inline(always)]
    fn store16_pair(_t: X64V3Token, dst: &mut [u16; NL], v0: __m256i, v1: __m256i) {
        let p = _mm256_permute4x64_epi64::<0b11011000>(_mm256_packus_epi32(v0, v1));
        _mm256_storeu_si256(dst, p);
    }

    #[rite(import_intrinsics)]
    #[inline(always)]
    fn ld32(_t: X64V3Token, a: &[u32]) -> __m256i {
        let r: &[u32; 8] = a[..8].try_into().unwrap();
        _mm256_loadu_si256(r)
    }

    #[rite(import_intrinsics)]
    #[inline(always)]
    fn st32(_t: X64V3Token, a: &mut [u32], v: __m256i) {
        let r: &mut [u32; 8] = (&mut a[..8]).try_into().unwrap();
        _mm256_storeu_si256(r, v);
    }

    /// One row decode in flight (AVX2: 16 lanes as two halves).
    struct Inst<'a, 'w> {
        k: K,
        dec: &'a [u32; TABLE],
        w: &'a Words<'w>,
        prev: &'a [[u16; NL]],
        cross: Option<&'a [[u16; NL]]>,
        x0: __m256i,
        x1: __m256i,
        r0: __m256i,
        r1: __m256i,
        w0: __m256i,
        w1: __m256i,
        wp: usize,
        xi: usize,
        row: usize,
    }

    #[rite(import_intrinsics)]
    #[inline(always)]
    fn inst<'a, 'w>(
        t: X64V3Token,
        md: &'a DecModel,
        w: &'a Words<'w>,
        st: &LaneState,
        prev: &'a [[u16; NL]],
        cross: Option<&'a [[u16; NL]]>,
        row: usize,
    ) -> Inst<'a, 'w> {
        let mut cm = [0u8; 32];
        cm[..16].copy_from_slice(&md.ctx_map);
        cm[16..].copy_from_slice(&md.ctx_map);
        Inst {
            k: K {
                cmapv: _mm256_loadu_si256(&cm),
                vt: _mm256_set1_epi32(md.vt as i32),
                vtm1: _mm256_set1_epi32(md.vt as i32 - 1),
                dlim_v: _mm256_set1_epi32(1 << md.k),
                dlim_r: _mm256_set1_epi32(1 << md.kr),
                mpow: _mm256_set1_epi32(1 << md.m),
                mmask: _mm256_set1_epi32((1 << md.m) - 1),
            },
            dec: &md.dec,
            w,
            prev,
            cross,
            x0: ld32(t, &st.x[..8]),
            x1: ld32(t, &st.x[8..]),
            r0: ld32(t, &st.run[..8]),
            r1: ld32(t, &st.run[8..]),
            w0: _mm256_setzero_si256(),
            w1: _mm256_setzero_si256(),
            wp: st.wp,
            xi: 0,
            row,
        }
    }

    #[rite(import_intrinsics)]
    #[inline(always)]
    fn finish(t: X64V3Token, i: &Inst<'_, '_>, st: &mut LaneState) {
        st32(t, &mut st.x[..8], i.x0);
        st32(t, &mut st.x[8..], i.x1);
        st32(t, &mut st.run[..8], i.r0);
        st32(t, &mut st.run[8..], i.r1);
        st.wp = i.wp;
    }

    #[rite(import_intrinsics)]
    fn step_slow(
        t: X64V3Token,
        i: &mut Inst<'_, '_>,
        wid0: __m256i,
        wid1: __m256i,
        lw: usize,
        out: &mut [[u16; NL]],
    ) {
        let zero = _mm256_setzero_si256();
        let (row, x) = (i.row, i.xi);
        let k = &i.k;
        let dec = i.dec;
        let w = i.w;
        let colv = _mm256_set1_epi32(x as i32);
        let inw0 = _mm256_cmpgt_epi32(wid0, colv);
        let inw1 = _mm256_cmpgt_epi32(wid1, colv);
        let ones = _mm256_set1_epi32(-1);
        // Compute busy once and reuse for idle0/idle1.
        let busy0 = _mm256_or_si256(_mm256_cmpgt_epi32(i.r0, zero), _mm256_xor_si256(inw0, ones));
        let busy1 = _mm256_or_si256(_mm256_cmpgt_epi32(i.r1, zero), _mm256_xor_si256(inw1, ones));
        let bm0 = _mm256_movemask_ps(_mm256_castsi256_ps(busy0));
        let bm1 = _mm256_movemask_ps(_mm256_castsi256_ps(busy1));
        if (bm0 & bm1) == 0xFF {
            let big = _mm256_set1_epi32(i32::MAX);
            let m0 = _mm256_blendv_epi8(
                big,
                _mm256_min_epu32(i.r0, _mm256_sub_epi32(wid0, colv)),
                inw0,
            );
            let m1 = _mm256_blendv_epi8(
                big,
                _mm256_min_epu32(i.r1, _mm256_sub_epi32(wid1, colv)),
                inw1,
            );
            let mut mm = [0u32; 8];
            _mm256_storeu_si256(&mut mm, _mm256_min_epu32(m0, m1));
            let kk = (mm.iter().copied().min().unwrap() as usize).min(lw - x);
            if kk > 0 {
                let kv = _mm256_set1_epi32(kk as i32);
                i.r0 = _mm256_sub_epi32(i.r0, _mm256_and_si256(kv, inw0));
                i.r1 = _mm256_sub_epi32(i.r1, _mm256_and_si256(kv, inw1));
                for step_row in &mut out[x..x + kk] {
                    *step_row = [0u16; NL];
                }
                i.w0 = zero;
                i.w1 = zero;
                i.xi += kk;
                return;
            }
        }
        let (mut s0, mut s1) = if row > 0 {
            let (n0, n1) = load16_pair(t, &i.prev[x]);
            if x > 0 {
                (_mm256_add_epi32(i.w0, n0), _mm256_add_epi32(i.w1, n1))
            } else {
                (_mm256_add_epi32(n0, n0), _mm256_add_epi32(n1, n1))
            }
        } else if x > 0 {
            (_mm256_add_epi32(i.w0, i.w0), _mm256_add_epi32(i.w1, i.w1))
        } else {
            (zero, zero)
        };
        if let Some(cr) = i.cross {
            let (c0, c1) = load16_pair(t, &cr[x]);
            s0 = _mm256_add_epi32(s0, c0);
            s1 = _mm256_add_epi32(s1, c1);
        }
        let idle0 = bm0 == 0xFF;
        let idle1 = bm1 == 0xFF;
        let (mut x0, mut x1, mut r0, mut r1) = (i.x0, i.x1, i.r0, i.r1);
        let mut wp = i.wp;
        let (w0, w1);
        if !idle0 && !idle1 {
            let (a0, i0) = token_idx(t, k, x0, &mut r0, s0, inw0);
            let (a1, i1) = token_idx(t, k, x1, &mut r1, s1, inw1);
            let mut ia = [0u32; 16];
            _mm256_storeu_si256(<&mut [u32; 8]>::try_from(&mut ia[..8]).unwrap(), i0);
            _mm256_storeu_si256(<&mut [u32; 8]>::try_from(&mut ia[8..]).unwrap(), i1);
            let ev: [u32; 16] = core::array::from_fn(|j| dec[ia[j] as usize & (TABLE - 1)]);
            let e0 = _mm256_loadu_si256(<&[u32; 8]>::try_from(&ev[..8]).unwrap());
            let e1 = _mm256_loadu_si256(<&[u32; 8]>::try_from(&ev[8..]).unwrap());
            token_update(t, e0, a0, w, &mut wp, &mut x0);
            token_update(t, e1, a1, w, &mut wp, &mut x1);
            let hi = _mm256_or_si256(_mm256_and_si256(e0, a0), _mm256_and_si256(e1, a1));
            if _mm256_testz_si256(hi, _mm256_set1_epi32(0xF000_0000u32 as i32)) != 0 {
                w0 = no_extra(t, k, &mut r0, a0, e0);
                w1 = no_extra(t, k, &mut r1, a1, e1);
            } else {
                w0 = extra_phase(t, k, w, &mut wp, &mut x0, &mut r0, a0, e0);
                w1 = extra_phase(t, k, w, &mut wp, &mut x1, &mut r1, a1, e1);
            }
        } else {
            let (xa, ra, sa, ia_, idle_x) = if idle0 {
                (&mut x1, &mut r1, s1, inw1, 0)
            } else {
                (&mut x0, &mut r0, s0, inw0, 1)
            };
            let (a, ix) = token_idx(t, k, *xa, ra, sa, ia_);
            let mut iv = [0u32; 8];
            _mm256_storeu_si256(&mut iv, ix);
            let ev: [u32; 8] = core::array::from_fn(|j| dec[iv[j] as usize & (TABLE - 1)]);
            let e = _mm256_loadu_si256(&ev);
            token_update(t, e, a, w, &mut wp, xa);
            let v = if _mm256_testz_si256(
                _mm256_and_si256(e, a),
                _mm256_set1_epi32(0xF000_0000u32 as i32),
            ) != 0
            {
                no_extra(t, k, ra, a, e)
            } else {
                extra_phase(t, k, w, &mut wp, xa, ra, a, e)
            };
            if idle_x == 0 {
                let in_run0 = _mm256_and_si256(_mm256_cmpgt_epi32(r0, zero), inw0);
                r0 = _mm256_add_epi32(r0, in_run0);
                w0 = zero;
                w1 = v;
            } else {
                let in_run1 = _mm256_and_si256(_mm256_cmpgt_epi32(r1, zero), inw1);
                r1 = _mm256_add_epi32(r1, in_run1);
                w0 = v;
                w1 = zero;
            }
        }
        store16_pair(t, &mut out[x], w0, w1);
        (i.x0, i.x1, i.r0, i.r1, i.w0, i.w1, i.wp) = (x0, x1, r0, r1, w0, w1, wp);
        i.xi += 1;
    }

    /// Fast 16-lane software gather packed via `[u64; 8]`: avoids slow `vpextrd`/`vpinsrd`
    /// chains on AVX2 and stops LLVM from lowering to microcoded `vpgatherdd`.
    #[rite(import_intrinsics)]
    #[inline(always)]
    fn gather16_avx2(
        _t: X64V3Token,
        dec: &[u32; TABLE],
        i0: __m256i,
        i1: __m256i,
    ) -> (__m256i, __m256i) {
        let mut ia = [0u64; 8];
        _mm256_storeu_si256(<&mut [u64; 4]>::try_from(&mut ia[..4]).unwrap(), i0);
        _mm256_storeu_si256(<&mut [u64; 4]>::try_from(&mut ia[4..]).unwrap(), i1);
        let mut ev = [0u64; 8];
        for j in 0..8 {
            let p = ia[j];
            let lo = dec[(p as usize) & (TABLE - 1)] as u64;
            let hi = dec[((p >> 32) as usize) & (TABLE - 1)] as u64;
            ev[j] = lo | (hi << 32);
        }
        (
            _mm256_loadu_si256(<&[u64; 4]>::try_from(&ev[..4]).unwrap()),
            _mm256_loadu_si256(<&[u64; 4]>::try_from(&ev[4..]).unwrap()),
        )
    }

    /// Inline macro for one full-width (`xi < min_w`) AVX2 step:
    /// - Fast path when all 16 lanes have `run == 0` (zero mask overhead).
    /// - Inline partial-run path when `1..=15` lanes are in runs (no call/spill).
    /// - Delegates to `step_slow` only when all 16 lanes are simultaneously in runs.
    macro_rules! avx2_step_inw {
        ($t:expr, $inst:expr, $wid0:expr, $wid1:expr, $lw:expr, $out_c:expr) => {{
            let zero = _mm256_setzero_si256();
            let ones = _mm256_set1_epi32(-1);
            let hi_mask = _mm256_set1_epi32(0xF000_0000u32 as i32);
            let or_r = _mm256_or_si256($inst.r0, $inst.r1);
            if _mm256_testz_si256(or_r, or_r) != 0 {
                let x = $inst.xi;
                let r = $inst.row;
                let (mut s0, mut s1) = if r > 0 {
                    let (n0, n1) = load16_pair($t, &$inst.prev[x]);
                    if x > 0 {
                        (
                            _mm256_add_epi32($inst.w0, n0),
                            _mm256_add_epi32($inst.w1, n1),
                        )
                    } else {
                        (_mm256_add_epi32(n0, n0), _mm256_add_epi32(n1, n1))
                    }
                } else if x > 0 {
                    (
                        _mm256_add_epi32($inst.w0, $inst.w0),
                        _mm256_add_epi32($inst.w1, $inst.w1),
                    )
                } else {
                    (zero, zero)
                };
                if let Some(cr) = $inst.cross {
                    let (c0, c1) = load16_pair($t, &cr[x]);
                    s0 = _mm256_add_epi32(s0, c0);
                    s1 = _mm256_add_epi32(s1, c1);
                }
                let i0 = token_idx_all($t, &$inst.k, $inst.x0, s0);
                let i1 = token_idx_all($t, &$inst.k, $inst.x1, s1);
                let (e0, e1) = gather16_avx2($t, $inst.dec, i0, i1);
                token_update_all($t, e0, $inst.w, &mut $inst.wp, &mut $inst.x0);
                token_update_all($t, e1, $inst.w, &mut $inst.wp, &mut $inst.x1);
                let hi = _mm256_or_si256(e0, e1);
                let (w0, w1) = if _mm256_testz_si256(hi, hi_mask) != 0 {
                    (
                        no_extra_all($t, &$inst.k, &mut $inst.r0, e0),
                        no_extra_all($t, &$inst.k, &mut $inst.r1, e1),
                    )
                } else {
                    (
                        extra_phase_all(
                            $t,
                            &$inst.k,
                            $inst.w,
                            &mut $inst.wp,
                            &mut $inst.x0,
                            &mut $inst.r0,
                            e0,
                        ),
                        extra_phase_all(
                            $t,
                            &$inst.k,
                            $inst.w,
                            &mut $inst.wp,
                            &mut $inst.x1,
                            &mut $inst.r1,
                            e1,
                        ),
                    )
                };
                store16_pair($t, &mut $out_c[x], w0, w1);
                $inst.w0 = w0;
                $inst.w1 = w1;
                $inst.xi = x + 1;
            } else {
                let in_r0 = _mm256_cmpgt_epi32($inst.r0, zero);
                let in_r1 = _mm256_cmpgt_epi32($inst.r1, zero);
                if _mm256_movemask_ps(_mm256_castsi256_ps(_mm256_and_si256(in_r0, in_r1))) == 0xFF {
                    step_slow($t, &mut $inst, $wid0, $wid1, $lw, $out_c);
                } else {
                    $inst.r0 = _mm256_add_epi32($inst.r0, in_r0);
                    $inst.r1 = _mm256_add_epi32($inst.r1, in_r1);
                    let a0 = _mm256_xor_si256(in_r0, ones);
                    let a1 = _mm256_xor_si256(in_r1, ones);
                    let x = $inst.xi;
                    let r = $inst.row;
                    let (mut s0, mut s1) = if r > 0 {
                        let (n0, n1) = load16_pair($t, &$inst.prev[x]);
                        if x > 0 {
                            (
                                _mm256_add_epi32($inst.w0, n0),
                                _mm256_add_epi32($inst.w1, n1),
                            )
                        } else {
                            (_mm256_add_epi32(n0, n0), _mm256_add_epi32(n1, n1))
                        }
                    } else if x > 0 {
                        (
                            _mm256_add_epi32($inst.w0, $inst.w0),
                            _mm256_add_epi32($inst.w1, $inst.w1),
                        )
                    } else {
                        (zero, zero)
                    };
                    if let Some(cr) = $inst.cross {
                        let (c0, c1) = load16_pair($t, &cr[x]);
                        s0 = _mm256_add_epi32(s0, c0);
                        s1 = _mm256_add_epi32(s1, c1);
                    }
                    let i0 = token_idx_all($t, &$inst.k, $inst.x0, s0);
                    let i1 = token_idx_all($t, &$inst.k, $inst.x1, s1);
                    let (e0, e1) = gather16_avx2($t, $inst.dec, i0, i1);
                    token_update($t, e0, a0, $inst.w, &mut $inst.wp, &mut $inst.x0);
                    token_update($t, e1, a1, $inst.w, &mut $inst.wp, &mut $inst.x1);
                    let hi = _mm256_or_si256(_mm256_and_si256(e0, a0), _mm256_and_si256(e1, a1));
                    let (w0, w1) = if _mm256_testz_si256(hi, hi_mask) != 0 {
                        (
                            no_extra($t, &$inst.k, &mut $inst.r0, a0, e0),
                            no_extra($t, &$inst.k, &mut $inst.r1, a1, e1),
                        )
                    } else {
                        (
                            extra_phase(
                                $t,
                                &$inst.k,
                                $inst.w,
                                &mut $inst.wp,
                                &mut $inst.x0,
                                &mut $inst.r0,
                                a0,
                                e0,
                            ),
                            extra_phase(
                                $t,
                                &$inst.k,
                                $inst.w,
                                &mut $inst.wp,
                                &mut $inst.x1,
                                &mut $inst.r1,
                                a1,
                                e1,
                            ),
                        )
                    };
                    store16_pair($t, &mut $out_c[x], w0, w1);
                    $inst.w0 = w0;
                    $inst.w1 = w1;
                    $inst.xi = x + 1;
                }
            }
        }};
    }

    /// AVX2 row decoder; bit-identical to [`decode_row_scalar`].
    /// Hot loop is macro-inlined directly into the `#[arcane]` body so lane states
    /// stay in YMM registers with zero per-step call/spill overhead.
    #[arcane(import_intrinsics)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn decode_row(
        t: X64V3Token,
        md: &DecModel,
        words: &Words<'_>,
        st: &mut LaneState,
        widths: &[u32; NL],
        lw: usize,
        row: usize,
        prev: &[u16],
        cross: Option<&[u16]>,
        out: &mut [u16],
    ) {
        let (wid0, wid1) = (ld32(t, &widths[..8]), ld32(t, &widths[8..]));
        let min_w = widths.iter().copied().min().unwrap_or(0) as usize;
        let prev_c = &prev.as_chunks::<NL>().0[..lw];
        let cross_c = cross.map(|c| &c.as_chunks::<NL>().0[..lw]);
        let out_c = &mut out.as_chunks_mut::<NL>().0[..lw];
        let mut a = inst(t, md, words, st, prev_c, cross_c, row);
        while a.xi < min_w {
            avx2_step_inw!(t, a, wid0, wid1, lw, out_c);
        }
        while a.xi < lw {
            step_slow(t, &mut a, wid0, wid1, lw, out_c);
        }
        finish(t, &a, st);
    }

    /// Two independent row decodes stepped together (macro-inlined fast path).
    #[arcane(import_intrinsics)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn decode_row2(
        t: X64V3Token,
        md: [&DecModel; 2],
        words: [&Words<'_>; 2],
        st: [&mut LaneState; 2],
        widths: &[u32; NL],
        lw: usize,
        rows: [usize; 2],
        prev: [&[u16]; 2],
        cross: [Option<&[u16]>; 2],
        out: [&mut [u16]; 2],
    ) {
        let (wid0, wid1) = (ld32(t, &widths[..8]), ld32(t, &widths[8..]));
        let min_w = widths.iter().copied().min().unwrap_or(0) as usize;
        let [sa, sb] = st;
        let [oa, ob] = out;
        let pa_c = &prev[0].as_chunks::<NL>().0[..lw];
        let pb_c = &prev[1].as_chunks::<NL>().0[..lw];
        let ca_c = cross[0].map(|c| &c.as_chunks::<NL>().0[..lw]);
        let cb_c = cross[1].map(|c| &c.as_chunks::<NL>().0[..lw]);
        let oa_c = &mut oa.as_chunks_mut::<NL>().0[..lw];
        let ob_c = &mut ob.as_chunks_mut::<NL>().0[..lw];
        let mut a = inst(t, md[0], words[0], sa, pa_c, ca_c, rows[0]);
        let mut b = inst(t, md[1], words[1], sb, pb_c, cb_c, rows[1]);
        while a.xi < min_w && b.xi < min_w {
            avx2_step_inw!(t, a, wid0, wid1, lw, oa_c);
            avx2_step_inw!(t, b, wid0, wid1, lw, ob_c);
        }
        while a.xi < min_w {
            avx2_step_inw!(t, a, wid0, wid1, lw, oa_c);
        }
        while b.xi < min_w {
            avx2_step_inw!(t, b, wid0, wid1, lw, ob_c);
        }
        while a.xi < lw {
            step_slow(t, &mut a, wid0, wid1, lw, oa_c);
        }
        while b.xi < lw {
            step_slow(t, &mut b, wid0, wid1, lw, ob_c);
        }
        finish(t, &a, sa);
        finish(t, &b, sb);
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) mod avx512 {
    use super::*;
    use archmage::prelude::*;

    #[rite(import_intrinsics)]
    #[inline(always)]
    fn ld16(_t: X64V4Token, p: &[u16; NL]) -> __m512i {
        _mm512_cvtepu16_epi32(_mm256_loadu_si256(p))
    }

    #[rite(import_intrinsics)]
    #[inline(always)]
    fn ld32(_t: X64V4Token, a: &[u32; NL]) -> __m512i {
        _mm512_loadu_si512(a)
    }

    #[rite(import_intrinsics)]
    #[inline(always)]
    fn refill(
        _t: X64V4Token,
        x: __m512i,
        need: __mmask16,
        w: &Words<'_>,
        wp: &mut usize,
    ) -> __m512i {
        let wv = w.chunk(*wp);
        let v = _mm512_maskz_expand_epi32(need, _mm512_cvtepu16_epi32(_mm256_loadu_si256(wv)));
        *wp += need.count_ones() as usize;
        _mm512_mask_mov_epi32(x, need, _mm512_or_si512(_mm512_slli_epi32::<16>(x), v))
    }

    /// 16-lane table lookup via single 512-bit hardware `vpgatherdd`.
    #[rite(import_intrinsics)]
    #[inline(always)]
    fn gather_dec(t: X64V4Token, dec: &[u32; TABLE], idx: __m512i) -> __m512i {
        use magetypes::simd::generic::u32x16;
        u32x16::<X64V4Token>::gather_wrapping(dec, u32x16::<X64V4Token>::from_raw_t(t, idx)).raw()
    }

    /// One row decode in flight: one slot's lane states and model.
    struct Inst<'a, 'w> {
        dec: &'a [u32; TABLE],
        w: &'a Words<'w>,
        prev: &'a [[u16; NL]],
        cross: Option<&'a [[u16; NL]]>,
        cmapv: __m512i,
        vt: __m512i,
        dlim_v: __m512i,
        dlim_r: __m512i,
        mpow: __m512i,
        mmask: __m512i,
        x: __m512i,
        run: __m512i,
        uw: __m512i,
        wp: usize,
        xi: usize,
        row: usize,
    }

    #[rite(import_intrinsics)]
    #[inline(always)]
    fn inst<'a, 'w>(
        _t: X64V4Token,
        md: &'a DecModel,
        w: &'a Words<'w>,
        st: &LaneState,
        prev: &'a [[u16; NL]],
        cross: Option<&'a [[u16; NL]]>,
        row: usize,
    ) -> Inst<'a, 'w> {
        let cm: [u32; 16] = core::array::from_fn(|i| md.ctx_map[i] as u32 & 3);
        Inst {
            dec: &md.dec,
            w,
            prev,
            cross,
            cmapv: _mm512_loadu_si512(&cm),
            vt: _mm512_set1_epi32(md.vt as i32),
            dlim_v: _mm512_set1_epi32(1 << md.k),
            dlim_r: _mm512_set1_epi32(1 << md.kr),
            mpow: _mm512_set1_epi32(1 << md.m),
            mmask: _mm512_set1_epi32((1 << md.m) - 1),
            x: _mm512_loadu_si512(&st.x),
            run: _mm512_loadu_si512(&st.run),
            uw: _mm512_setzero_si512(),
            wp: st.wp,
            xi: 0,
            row,
        }
    }

    #[rite(import_intrinsics)]
    #[inline(always)]
    fn finish(_t: X64V4Token, i: &Inst<'_, '_>, st: &mut LaneState) {
        _mm512_storeu_si512(&mut st.x, i.x);
        _mm512_storeu_si512(&mut st.run, i.run);
        st.wp = i.wp;
    }

    /// General step when some lane is in a run or past its width.
    #[rite(import_intrinsics)]
    fn step_slow(
        t: X64V4Token,
        i: &mut Inst<'_, '_>,
        widv: __m512i,
        lw: usize,
        out: &mut [[u16; NL]],
    ) {
        let row = i.row;
        let zero = _mm512_setzero_si512();
        let one = _mm512_set1_epi32(1);
        let c1023 = _mm512_set1_epi32(1023);
        let c15 = _mm512_set1_epi32(15);
        let c32 = _mm512_set1_epi32(32);
        let lim = _mm512_set1_epi32(RANS_L as i32);
        let xi = i.xi;
        let colv = _mm512_set1_epi32(xi as i32);
        let inw: __mmask16 = _mm512_cmpgt_epu32_mask(widv, colv);
        let running: __mmask16 = _mm512_cmpgt_epu32_mask(i.run, zero);
        if (running | !inw) == 0xFFFF {
            let rem = _mm512_min_epu32(i.run, _mm512_sub_epi32(widv, colv));
            let m = _mm512_mask_reduce_min_epu32(inw, rem);
            let k = (m as usize).min(lw - xi);
            if k > 0 {
                i.run = _mm512_mask_sub_epi32(i.run, inw, i.run, _mm512_set1_epi32(k as i32));
                for step_row in &mut out[xi..xi + k] {
                    *step_row = [0u16; NL];
                }
                i.uw = zero;
                i.xi += k;
                return;
            }
        }
        let in_run = running & inw;
        i.run = _mm512_mask_sub_epi32(i.run, in_run, i.run, one);
        let act = inw & !in_run;
        let mut sum = if row > 0 {
            let n = ld16(t, &i.prev[xi]);
            if xi > 0 {
                _mm512_add_epi32(i.uw, n)
            } else {
                _mm512_add_epi32(n, n)
            }
        } else if xi > 0 {
            _mm512_add_epi32(i.uw, i.uw)
        } else {
            zero
        };
        if let Some(cr) = i.cross {
            sum = _mm512_add_epi32(sum, ld16(t, &cr[xi]));
        }
        let cls = _mm512_min_epu32(_mm512_sub_epi32(c32, _mm512_lzcnt_epi32(sum)), c15);
        let ctx = _mm512_permutexvar_epi32(cls, i.cmapv);
        let idx = _mm512_or_si512(_mm512_slli_epi32::<10>(ctx), _mm512_and_si512(i.x, c1023));
        let e = gather_dec(t, i.dec, idx);
        let f = _mm512_add_epi32(_mm512_and_si512(e, c1023), one);
        let bias = _mm512_and_si512(_mm512_srli_epi32::<10>(e), c1023);
        let nx = _mm512_add_epi32(_mm512_mullo_epi32(f, _mm512_srli_epi32::<10>(i.x)), bias);
        let mut x = _mm512_mask_mov_epi32(i.x, act, nx);
        let need = _mm512_mask_cmplt_epu32_mask(act, x, lim);
        x = refill(t, x, need, i.w, &mut i.wp);
        let nb = _mm512_srli_epi32::<28>(e);
        let sym = _mm512_and_si512(_mm512_srli_epi32::<20>(e), _mm512_set1_epi32(255));
        let has_extra: __mmask16 =
            _mm512_mask_test_epi32_mask(act, e, _mm512_set1_epi32(0xF000_0000u32 as i32));
        let ebits = if has_extra != 0 {
            let ebits = _mm512_and_si512(x, _mm512_sub_epi32(_mm512_sllv_epi32(one, nb), one));
            x = _mm512_mask_mov_epi32(x, act, _mm512_srlv_epi32(x, nb));
            let need = _mm512_mask_cmplt_epu32_mask(act, x, lim);
            x = refill(t, x, need, i.w, &mut i.wp);
            ebits
        } else {
            zero
        };
        i.x = x;
        let run_now: __mmask16 = _mm512_mask_cmpge_epu32_mask(act, sym, i.vt);
        let tt = _mm512_mask_sub_epi32(sym, run_now, sym, i.vt);
        let dlim = _mm512_mask_mov_epi32(i.dlim_v, run_now, i.dlim_r);
        let direct: __mmask16 = _mm512_cmplt_epu32_mask(tt, dlim);
        let hb = _mm512_sllv_epi32(_mm512_or_si512(i.mpow, _mm512_and_si512(tt, i.mmask)), nb);
        let base = _mm512_mask_mov_epi32(hb, direct, tt);
        let u = _mm512_add_epi32(base, ebits);
        i.run = _mm512_mask_mov_epi32(i.run, run_now, u);
        let val = _mm512_maskz_mov_epi32(act & !run_now, u);
        i.uw = val;
        _mm256_storeu_si256(&mut out[xi], _mm512_cvtepi32_epi16(val));
        i.xi += 1;
    }

    macro_rules! avx512_prep_idx {
        ($i:expr, $act:ident, $idx:ident) => {
            let zero = _mm512_setzero_si512();
            let one = _mm512_set1_epi32(1);
            let c1023 = _mm512_set1_epi32(1023);
            let c15 = _mm512_set1_epi32(15);
            let c32 = _mm512_set1_epi32(32);
            let running: __mmask16 = _mm512_test_epi32_mask($i.run, $i.run);
            let $act: __mmask16 = !running;
            if running != 0 {
                $i.run = _mm512_mask_sub_epi32($i.run, running, $i.run, one);
            }
            let xi = $i.xi;
            let row = $i.row;
            let mut sum = if row > 0 {
                let n = _mm512_cvtepu16_epi32(_mm256_loadu_si256(&$i.prev[xi]));
                if xi > 0 {
                    _mm512_add_epi32($i.uw, n)
                } else {
                    _mm512_add_epi32(n, n)
                }
            } else if xi > 0 {
                _mm512_add_epi32($i.uw, $i.uw)
            } else {
                zero
            };
            if let Some(cr) = $i.cross {
                sum = _mm512_add_epi32(sum, _mm512_cvtepu16_epi32(_mm256_loadu_si256(&cr[xi])));
            }
            let cls = _mm512_min_epu32(_mm512_sub_epi32(c32, _mm512_lzcnt_epi32(sum)), c15);
            let ctx = _mm512_permutexvar_epi32(cls, $i.cmapv);
            let $idx = _mm512_or_si512(_mm512_slli_epi32::<10>(ctx), _mm512_and_si512($i.x, c1023));
        };
    }

    macro_rules! avx512_finish_step {
        ($i:expr, $act:expr, $e:expr, $out:expr) => {{
            let one = _mm512_set1_epi32(1);
            let c1023 = _mm512_set1_epi32(1023);
            let lim = _mm512_set1_epi32(RANS_L as i32);
            let xi = $i.xi;
            let f = _mm512_add_epi32(_mm512_and_si512($e, c1023), one);
            let bias = _mm512_and_si512(_mm512_srli_epi32::<10>($e), c1023);
            let nx = _mm512_add_epi32(_mm512_mullo_epi32(f, _mm512_srli_epi32::<10>($i.x)), bias);
            let mut x = _mm512_mask_mov_epi32($i.x, $act, nx);
            let need = _mm512_mask_cmplt_epu32_mask($act, x, lim);
            {
                let wv = $i.w.chunk($i.wp);
                let v = _mm512_maskz_expand_epi32(need, _mm512_cvtepu16_epi32(_mm256_loadu_si256(wv)));
                $i.wp += need.count_ones() as usize;
                x = _mm512_mask_mov_epi32(x, need, _mm512_or_si512(_mm512_slli_epi32::<16>(x), v));
            }
            let has_extra: __mmask16 =
                _mm512_mask_test_epi32_mask($act, $e, _mm512_set1_epi32(0xF000_0000u32 as i32));
            let val = if has_extra == 0 {
                // Top 4 bits of e are 0 on every active lane; inactive lanes are masked out.
                let sym = _mm512_srli_epi32::<20>($e);
                let run_now: __mmask16 = _mm512_mask_cmpge_epu32_mask($act, sym, $i.vt);
                let u = _mm512_mask_sub_epi32(sym, run_now, sym, $i.vt);
                $i.run = _mm512_mask_mov_epi32($i.run, run_now, u);
                _mm512_maskz_mov_epi32($act & !run_now, u)
            } else {
                let nb = _mm512_srli_epi32::<28>($e);
                let sym = _mm512_and_si512(_mm512_srli_epi32::<20>($e), _mm512_set1_epi32(255));
                let ebits = _mm512_and_si512(x, _mm512_sub_epi32(_mm512_sllv_epi32(one, nb), one));
                x = _mm512_mask_mov_epi32(x, $act, _mm512_srlv_epi32(x, nb));
                let need = _mm512_mask_cmplt_epu32_mask($act, x, lim);
                {
                    let wv = $i.w.chunk($i.wp);
                    let v = _mm512_maskz_expand_epi32(need, _mm512_cvtepu16_epi32(_mm256_loadu_si256(wv)));
                    $i.wp += need.count_ones() as usize;
                    x = _mm512_mask_mov_epi32(x, need, _mm512_or_si512(_mm512_slli_epi32::<16>(x), v));
                }
                let run_now: __mmask16 = _mm512_mask_cmpge_epu32_mask($act, sym, $i.vt);
                let tt = _mm512_mask_sub_epi32(sym, run_now, sym, $i.vt);
                let dlim = _mm512_mask_mov_epi32($i.dlim_v, run_now, $i.dlim_r);
                let direct: __mmask16 = _mm512_cmplt_epu32_mask(tt, dlim);
                let hb = _mm512_sllv_epi32(_mm512_or_si512($i.mpow, _mm512_and_si512(tt, $i.mmask)), nb);
                let base = _mm512_mask_mov_epi32(hb, direct, tt);
                let u = _mm512_add_epi32(base, ebits);
                $i.run = _mm512_mask_mov_epi32($i.run, run_now, u);
                _mm512_maskz_mov_epi32($act & !run_now, u)
            };
            $i.x = x;
            $i.uw = val;
            _mm256_storeu_si256(&mut $out[xi], _mm512_cvtepi32_epi16(val));
            $i.xi = xi + 1;
        }};
    }

    /// Inline macro for one full-width (`xi < min_w`) AVX-512 step, handling
    /// partial runs directly in ZMM mask registers without calling `step_slow`
    /// unless all 16 lanes are simultaneously in runs.
    macro_rules! avx512_step_inw {
        ($t:expr, $i:expr, $widv:expr, $lw:expr, $out:expr) => {{
            if _mm512_test_epi32_mask($i.run, $i.run) == 0xFFFF {
                step_slow($t, &mut $i, $widv, $lw, $out);
            } else {
                avx512_prep_idx!($i, act, idx);
                let e = gather_dec($t, $i.dec, idx);
                avx512_finish_step!($i, act, e, $out);
            }
        }};
    }

    /// AVX-512 row decoder; bit-identical to [`decode_row_scalar`].
    #[arcane(import_intrinsics)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn decode_row(
        t: X64V4Token,
        md: &DecModel,
        words: &Words<'_>,
        st: &mut LaneState,
        widths: &[u32; NL],
        lw: usize,
        row: usize,
        prev: &[u16],
        cross: Option<&[u16]>,
        out: &mut [u16],
    ) {
        let widv = ld32(t, widths);
        let min_w = widths.iter().copied().min().unwrap_or(0) as usize;
        let prev_c = &prev.as_chunks::<NL>().0[..lw];
        let cross_c = cross.map(|c| &c.as_chunks::<NL>().0[..lw]);
        let out_c = &mut out.as_chunks_mut::<NL>().0[..lw];
        let mut a = inst(t, md, words, st, prev_c, cross_c, row);
        while a.xi < min_w {
            avx512_step_inw!(t, a, widv, lw, out_c);
        }
        while a.xi < lw {
            step_slow(t, &mut a, widv, lw, out_c);
        }
        finish(t, &a, st);
    }

    /// Two independent row decodes stepped together with 2-way phase interleaving
    /// (prep indices -> gather both -> update both) to overlap hardware gathers.
    #[arcane(import_intrinsics)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn decode_row2(
        t: X64V4Token,
        md: [&DecModel; 2],
        words: [&Words<'_>; 2],
        st: [&mut LaneState; 2],
        widths: &[u32; NL],
        lw: usize,
        row: usize,
        prev: [&[u16]; 2],
        cross: [Option<&[u16]>; 2],
        out: [&mut [u16]; 2],
    ) {
        let widv = ld32(t, widths);
        let min_w = widths.iter().copied().min().unwrap_or(0) as usize;
        let [sa, sb] = st;
        let [oa, ob] = out;
        let pa_c = &prev[0].as_chunks::<NL>().0[..lw];
        let pb_c = &prev[1].as_chunks::<NL>().0[..lw];
        let ca_c = cross[0].map(|c| &c.as_chunks::<NL>().0[..lw]);
        let cb_c = cross[1].map(|c| &c.as_chunks::<NL>().0[..lw]);
        let oa_c = &mut oa.as_chunks_mut::<NL>().0[..lw];
        let ob_c = &mut ob.as_chunks_mut::<NL>().0[..lw];
        let mut a = inst(t, md[0], words[0], sa, pa_c, ca_c, row);
        let mut b = inst(t, md[1], words[1], sb, pb_c, cb_c, row);
        while a.xi < min_w && b.xi < min_w {
            if _mm512_test_epi32_mask(a.run, a.run) != 0xFFFF
                && _mm512_test_epi32_mask(b.run, b.run) != 0xFFFF
            {
                avx512_prep_idx!(a, act_a, idx_a);
                avx512_prep_idx!(b, act_b, idx_b);
                let ea = gather_dec(t, a.dec, idx_a);
                let eb = gather_dec(t, b.dec, idx_b);
                avx512_finish_step!(a, act_a, ea, oa_c);
                avx512_finish_step!(b, act_b, eb, ob_c);
            } else {
                avx512_step_inw!(t, a, widv, lw, oa_c);
                avx512_step_inw!(t, b, widv, lw, ob_c);
            }
        }
        while a.xi < min_w {
            avx512_step_inw!(t, a, widv, lw, oa_c);
        }
        while b.xi < min_w {
            avx512_step_inw!(t, b, widv, lw, ob_c);
        }
        while a.xi < lw {
            step_slow(t, &mut a, widv, lw, oa_c);
        }
        while b.xi < lw {
            step_slow(t, &mut b, widv, lw, ob_c);
        }
        finish(t, &a, sa);
        finish(t, &b, sb);
    }

    /// Three independent row decodes stepped together (slots 1 and 2 of a
    /// row with slot 0 of the next row) with 3-way phase interleaving
    /// (prep all 3 indices -> issue all 3 gathers -> update all 3 states)
    /// so all 3 512-bit hardware gathers pipeline in parallel before any
    /// data-dependent extra-bits branch.
    #[arcane(import_intrinsics)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn decode_row3(
        t: X64V4Token,
        md: [&DecModel; 3],
        words: [&Words<'_>; 3],
        st: [&mut LaneState; 3],
        widths: &[u32; NL],
        lw: usize,
        rows: [usize; 3],
        prev: [&[u16]; 3],
        cross: [Option<&[u16]>; 3],
        out: [&mut [u16]; 3],
    ) {
        let widv = ld32(t, widths);
        let min_w = widths.iter().copied().min().unwrap_or(0) as usize;
        let [sa, sb, sc] = st;
        let [oa, ob, oc] = out;
        let pa_c = &prev[0].as_chunks::<NL>().0[..lw];
        let pb_c = &prev[1].as_chunks::<NL>().0[..lw];
        let pc_c = &prev[2].as_chunks::<NL>().0[..lw];
        let ca_c = cross[0].map(|c| &c.as_chunks::<NL>().0[..lw]);
        let cb_c = cross[1].map(|c| &c.as_chunks::<NL>().0[..lw]);
        let cc_c = cross[2].map(|c| &c.as_chunks::<NL>().0[..lw]);
        let oa_c = &mut oa.as_chunks_mut::<NL>().0[..lw];
        let ob_c = &mut ob.as_chunks_mut::<NL>().0[..lw];
        let oc_c = &mut oc.as_chunks_mut::<NL>().0[..lw];
        let mut a = inst(t, md[0], words[0], sa, pa_c, ca_c, rows[0]);
        let mut b = inst(t, md[1], words[1], sb, pb_c, cb_c, rows[1]);
        let mut c = inst(t, md[2], words[2], sc, pc_c, cc_c, rows[2]);
        while a.xi < min_w && b.xi < min_w && c.xi < min_w {
            if _mm512_test_epi32_mask(a.run, a.run) != 0xFFFF
                && _mm512_test_epi32_mask(b.run, b.run) != 0xFFFF
                && _mm512_test_epi32_mask(c.run, c.run) != 0xFFFF
            {
                avx512_prep_idx!(a, act_a, idx_a);
                avx512_prep_idx!(b, act_b, idx_b);
                avx512_prep_idx!(c, act_c, idx_c);
                let ea = gather_dec(t, a.dec, idx_a);
                let eb = gather_dec(t, b.dec, idx_b);
                let ec = gather_dec(t, c.dec, idx_c);
                avx512_finish_step!(a, act_a, ea, oa_c);
                avx512_finish_step!(b, act_b, eb, ob_c);
                avx512_finish_step!(c, act_c, ec, oc_c);
            } else {
                avx512_step_inw!(t, a, widv, lw, oa_c);
                avx512_step_inw!(t, b, widv, lw, ob_c);
                avx512_step_inw!(t, c, widv, lw, oc_c);
            }
        }
        while a.xi < min_w {
            avx512_step_inw!(t, a, widv, lw, oa_c);
        }
        while b.xi < min_w {
            avx512_step_inw!(t, b, widv, lw, ob_c);
        }
        while c.xi < min_w {
            avx512_step_inw!(t, c, widv, lw, oc_c);
        }
        while a.xi < lw {
            step_slow(t, &mut a, widv, lw, oa_c);
        }
        while b.xi < lw {
            step_slow(t, &mut b, widv, lw, ob_c);
        }
        while c.xi < lw {
            step_slow(t, &mut c, widv, lw, oc_c);
        }
        finish(t, &a, sa);
        finish(t, &b, sb);
        finish(t, &c, sc);
    }
}

#[cfg(all(test, feature = "encode"))]
mod scan_tests {
    use super::*;
    use crate::tokens::TokenCfg;

    /// Plane where lane 0 sees `marks` (zeros elsewhere) at lane positions:
    /// element j of lane l lives at u[(j / width) * lw * NL + (j % width) * NL + l].
    fn plane(
        width: usize,
        lw: usize,
        rows: usize,
        lane: usize,
        marks: &[(usize, u32)],
    ) -> Vec<u32> {
        let mut u = vec![0u32; rows * lw * NL];
        for &(j, v) in marks {
            let (r, x) = (j / width, j % width);
            u[r * lw * NL + x * NL + lane] = v;
        }
        u
    }

    fn syms(u: &[u32], lw: usize, rows: usize, width: usize, run_min: u32) -> Vec<Sym> {
        let mut out = Vec::new();
        lane_symbols(
            u,
            None,
            lw,
            rows,
            width,
            0,
            &TokenCfg::v1(5, 2, 8),
            run_min,
            0,
            &mut out,
        );
        out
    }

    /// Counts (ctx, tok) via lane_hist; must equal lane_symbols' histogram.
    fn hist_matches_syms(
        u: &[u32],
        lw: usize,
        rows: usize,
        width: usize,
        tc: &TokenCfg,
        run_min: u32,
        got: &[Sym],
    ) {
        let mut hist = vec![vec![0u64; tc.alphabet() as usize]; CLASSES];
        lane_hist(u, None, lw, rows, width, 0, tc, run_min, 0, &mut hist);
        let mut want = vec![vec![0u64; tc.alphabet() as usize]; CLASSES];
        for y in got {
            want[y.ctx as usize][y.tok as usize] += 1;
        }
        assert_eq!(hist, want);
    }

    fn is_run(tc: &TokenCfg, tok: u8) -> bool {
        tok as u32 >= tc.value_tokens()
    }

    #[test]
    fn zero_runs_spanning_run_cap() {
        // 65536 zero elements in lane 0 (width 32 x 2048 rows): two RUN_CAP
        // windows. Other lanes are zero too but do not affect lane 0.
        let (w, lw, rows) = (32, 32, 2048);
        let n = w * rows; // 65536
        let tc = TokenCfg::v1(5, 2, 8);
        let u = plane(w, lw, rows, 0, &[]);
        for run_min in [0u32, 1, 4, RUN_CAP - 1, RUN_CAP, RUN_CAP + 1] {
            let got = syms(&u, lw, rows, w, run_min);
            hist_matches_syms(&u, lw, rows, w, &tc, run_min, &got);
            let runs: Vec<_> = got.iter().filter(|y| is_run(&tc, y.tok)).collect();
            if run_min == 0 || run_min > RUN_CAP {
                assert!(runs.is_empty(), "runs emitted at run_min={run_min}");
                assert_eq!(got.len(), n);
            } else {
                // Each emitted run is capped at RUN_CAP; steps still advance.
                for pair in runs.windows(2) {
                    assert!(pair[1].step > pair[0].step);
                }
                assert_eq!(got.len() - runs.len(), 0, "non-run symbols in flat lane");
                assert!(!got.is_empty());
            }
            // Every symbol's step stays inside the lane.
            for y in &got {
                assert!((y.step as usize) < n);
            }
        }
    }

    #[test]
    fn below_threshold_suffix() {
        // run_min=4: a 3-zero run (below threshold -> 3 singles), a nonzero,
        // a 10-zero run (-> 1 run token), nonzero, 5 zeros, nonzero.
        let (w, lw, rows) = (64, 64, 4);
        let tc = TokenCfg::v1(5, 2, 8);
        let marks = [(3, 9u32), (14, 9), (20, 9)];
        let u = plane(w, lw, rows, 0, &marks);
        let got = syms(&u, lw, rows, w, 4);
        hist_matches_syms(&u, lw, rows, w, &tc, 4, &got);
        let n_runs = got.iter().filter(|y| is_run(&tc, y.tok)).count();
        // Runs: 10 zeros after j=3's marker (j=4..14), 5 zeros (j=15..20),
        // plus trailing zeros j=21..256 (235 zeros >= 4 -> run) — and the
        // leading j=0..3 run is below threshold.
        assert_eq!(got[0].step, 0);
        assert_eq!(n_runs, 3, "run syms: {n_runs}");
        // Three singles for the below-threshold prefix.
        assert!(got[..3].iter().all(|y| !is_run(&tc, y.tok)));
    }

    #[test]
    fn threshold_edge_lengths() {
        // For each L around run_min: exactly L zeros then a marker. The run
        // is coded iff 1 <= run_min <= RUN_CAP && L >= run_min.
        let tc = TokenCfg::v1(5, 2, 8);
        let expect = |run_min: u32, l_: u32| (1..=RUN_CAP).contains(&run_min) && l_ >= run_min;
        for run_min in [1u32, 4] {
            let (w, lw, rows) = (64, 64, 8);
            for l_ in [run_min.saturating_sub(1), run_min, run_min + 1] {
                let u = plane(w, lw, rows, 0, &[(l_ as usize, 7)]);
                let got = syms(&u, lw, rows, w, run_min);
                hist_matches_syms(&u, lw, rows, w, &tc, run_min, &got);
                let coded_run = got.first().is_some_and(|y| is_run(&tc, y.tok));
                assert_eq!(coded_run, expect(run_min, l_), "run_min={run_min} L={l_}");
            }
        }
        // RUN_CAP boundaries need a plane that fits 32769+ zeros: 65536.
        for run_min in [RUN_CAP, RUN_CAP + 1] {
            let (w, lw, rows) = (32, 32, 2048);
            for l_ in [run_min - 1, run_min, run_min + 1] {
                let u = plane(w, lw, rows, 0, &[(l_ as usize, 7)]);
                let got = syms(&u, lw, rows, w, run_min);
                hist_matches_syms(&u, lw, rows, w, &tc, run_min, &got);
                let coded_run = got.first().is_some_and(|y| is_run(&tc, y.tok));
                assert_eq!(coded_run, expect(run_min, l_), "run_min={run_min} L={l_}");
            }
        }
    }
}

#[cfg(all(test, feature = "encode"))]
mod run_cap_tests {
    use super::*;
    use crate::bits::{BitReader, BitWriter};

    /// A one-table model of scheme `tc` weighting symbols 0 and `sym`,
    /// written and read back (SPEC 3.3).
    fn round_trip(tc: TokenCfg, sym: usize) -> Result<Model, Error> {
        let mut wt = vec![0u32; tc.alphabet() as usize];
        wt[0] = 1;
        wt[sym] = 1;
        let md = Model {
            tc,
            ctx_map: [0; CLASSES],
            freq: vec![weights_to_freq(&wt).unwrap(); NCTX],
            weights: vec![wt; NCTX],
            prec: vec![1; NCTX],
            cross: false,
            ctx_shift: 0,
            seeds: [None; NCTX],
        };
        let mut w = BitWriter::new();
        md.write(&mut w, None);
        let b = w.finish();
        Model::read(&mut BitReader::new(&b), tc.depth, None)
    }

    /// SPEC 3.2: in every valid scheme, the last run token a model may
    /// weight codes runs of at most `RUN_CAP` and the next one only longer
    /// runs; the reader accepts the first and rejects the second.
    #[test]
    fn models_weighting_runs_past_the_cap_are_corrupt() {
        let mut schemes = 0;
        for depth in [0, 1, 5, 8, 12, 16, 17, 24, 32] {
            for m in 0..=4 {
                for k in 0..=16 {
                    let tc = TokenCfg::v1(k, m, depth);
                    if tc.k != k || tc.alphabet() > 256 {
                        continue;
                    }
                    schemes += 1;
                    let vt = tc.value_tokens();
                    let end = run_cap_end(&tc);
                    assert!(end < tc.alphabet() as usize, "{tc:?}");
                    let (b, nb) = hybrid_base(end as u32 - 1 - vt, tc.rk, tc.rm);
                    assert_eq!(b + (1 << nb), RUN_CAP, "{tc:?}: the longest allowed run");
                    let (b, _) = hybrid_base(end as u32 - vt, tc.rk, tc.rm);
                    assert_eq!(
                        b, RUN_CAP,
                        "{tc:?}: the shortest run past the cap, minus one"
                    );
                    assert!(round_trip(tc, end - 1).is_ok(), "{tc:?}");
                    for sym in [end, tc.alphabet() as usize - 1] {
                        assert!(
                            matches!(
                                round_trip(tc, sym),
                                Err(Error::Corrupt("run token past the run cap"))
                            ),
                            "{tc:?} symbol {sym}"
                        );
                    }
                }
            }
        }
        assert!(schemes > 100);
    }
}

#[cfg(all(test, feature = "encode"))]
mod row_scan_tests;
