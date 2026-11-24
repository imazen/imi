// Review scratch: the trellis at 35ab183b (base), for differential tests.
#![allow(dead_code, unused, clippy::all)]
// zenjpeg's AC trellis quantization, run by the lossy encoder at high
// effort (`crate::effort::Effort::trellis_passes`) on XYB files whose AC is
// coded run-level or split-Huffman, each priced with its own rate model (in
// YCoCg it lost 18-20% Butteraugli max). Encoder only: the
// bitstream, the decoder and the DC are unchanged; the decoded pixels
// change.
//
// A port of the trellis zenjpeg's `auto_optimize` ("hybrid") mode runs at
// q50+ (`zenjpeg/src/encode/trellis/ac.rs`, from mozjpeg's
// `jcdctmgr.c`), with IMI's own rate model in place of JPEG Huffman code
// lengths. Measured in `benchmarks/hybrid_trellis_2026-10-07` (lab study)
// and `benchmarks/trellis_effort_2026-10-08` (effort thresholds).
//
// * **Distortion** is zenjpeg's: an AC coefficient at `v` quantizer steps
//   coded as `c` costs `64 * lambda * (c - v)^2` (flat weights, mozjpeg's
//   mode 1), with `lambda = 2^s1 / (2^s2 + norm)` and `norm` the block's
//   mean squared AC coefficient in libjpeg's integer DCT units, 8 times
//   IMI's orthonormal coefficients (IMI's planes are close to 8-bit
//   units: XYB luma is 300 Y, jpegli's 302 Y). zenjpeg: `s1` 14.5, `s2`
//   16.5. Flat 8-bit blocks then trade one bit for 1/16 of a squared step.
//   Per-block AQ multipliers scale the AC steps as the quantizer does.
// * **Rate** is IMI's: token costs `log2(M / freq)` from the explicit
//   rANS tables clustered from the image's own coefficients exactly as the
//   writer clusters them (pass 1: the default quantization; each later
//   pass: the previous pass's output), plus the raw sign and remainder
//   bits. An absent symbol costs one table slot (`log2 M` bits). The
//   writer then builds its tables (and seed choices) from the final
//   coefficients, in whichever entropy mode the configuration or tier
//   picks.
// * **Contexts** follow the writer. AC token contexts depend on the
//   block's EOB (positions left before it), so every candidate EOB gets its
//   own Viterbi pass; the EOB token is priced in its lane context. A luma
//   EOB also prices this block's two chroma EOB tokens and the next luma
//   EOB token of the lane (their previous-pass values).
// * **Candidates** per coefficient: the nearest value and the largest
//   magnitude of every smaller run-level magnitude class (mozjpeg: 1, 3,
//   7, ... for JPEG's bit categories), or zero through the run structure.
//   A candidate whose extra distortion exceeds the most it could save in
//   bits is dropped (exact). zenjpeg's default `Adaptive` speed limits
//   apply to blocks with more than 48 nonzero candidates unless
//   `thorough`.
// * **Memory**: each pass recomputes a stripe's coefficients one block row
//   at a time through the encoder's own row pipeline (no image-wide copy).
// * Inter-frame "keep" blocks stay as coded; subblock layouts, other
//   entropy modes and colors, and the lab RDOQ / LF prediction modes skip
//   the effort's trellis (the lab override runs it on other modes and
//   colors). Small tier images code their split-Huffman candidate from the
//   pass-1 coefficients.
// * **AQ coupling** (lab; zenjpeg's `AqCoupling`, off in its
//   `auto_optimize`): with `aq_coupling != 0` each block's `s1` moves by
//   `clamp(aq * aq_coupling, +-aq_clamp)`, `aq` jpegli's AQ strength
//   `max(0, 0.6 / qf - 1)` of the block (`aq_field_lab.rs`; U8 input).
// * **Split-Huffman rate**: split-Huffman files (explicit, or a tier's
//   choice) priced with their own code lengths, from the Huffman tables the
//   writer would build for the previous pass ([`HufPrices`]): per block
//   channel the group count and, per group, the width and its values
//   ([`huf_channel`], exact under frozen lengths); the luma count also
//   prices the tokens whose contexts it sets. Measured in
//   `benchmarks/aq_trellis_2026-10-09` (rule R2); lab
//   `lab::set_trellis_huf_rate(false)` restores split-Huffman without the
//   trellis.
use super::*;
use coef::PK_GROUPS;

/// Trellis settings. The encoder derives them from effort; the lab
/// `LossyConfig::trellis` overrides them for experiments.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BaseTrellis {
    /// mozjpeg's `lambda_log_scale1`; zenjpeg's `auto_optimize`: 14.5.
    pub lambda_log_scale1: f32,
    /// mozjpeg's `lambda_log_scale2` (16.5). 0 = a fixed
    /// `lambda = 2^(s1 - 12)`, without the block norm.
    pub lambda_log_scale2: f32,
    /// Rate-model passes, 0..=4: tables from the default quantization,
    /// then from each previous trellis output; 0 = no trellis (a lab
    /// override that turns it off at any effort).
    pub passes: u8,
    /// Full search on every block (mozjpeg); else zenjpeg's `Adaptive`
    /// limits (lookback 16 / 8 positions, 4 / 3 candidates above 48 / 55
    /// nonzero candidates).
    pub thorough: bool,
    /// Qualities below this keep the default quantization (zenjpeg's
    /// `auto_optimize` trellis starts at q50); 0 = every quality.
    pub min_quality: f32,
    /// zenjpeg's `AqCoupling::scale`: lambda log scale 1 change per unit of
    /// jpegli AQ strength; 0 (default) = a fixed lambda.
    pub aq_coupling: f32,
    /// zenjpeg's `AqCoupling::max_adjustment` (its curated steps use 1.0).
    pub aq_clamp: f32,
}

impl Default for BaseTrellis {
    fn default() -> Self {
        Self {
            lambda_log_scale1: 14.5,
            lambda_log_scale2: 16.5,
            passes: 1,
            thorough: false,
            min_quality: 0.0,
            aq_coupling: 0.0,
            aq_clamp: 1.0,
        }
    }
}

/// LAB: the exponent k of the per-block lambda factor m^k for AQ multipliers
/// m (f32 bits; 0 = off). Process-global (`lab::set_trellis_aq_lambda_exp`).
#[cfg(feature = "lab")]
pub(crate) static AQ_LAMBDA_EXP: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// LAB: per-channel offsets of `lambda_log_scale1` (f32 bits; 0 = none),
/// distortion weights per channel (`lab::set_trellis_channel_shift`).
#[cfg(feature = "lab")]
pub(crate) static CH_SHIFT: [core::sync::atomic::AtomicU32; 3] =
    [const { core::sync::atomic::AtomicU32::new(0) }; 3];

/// Lab switches the trellis reads, fixed for one encode
/// ([`crate::lossy::LossyLabSwitches`]); empty without `lab`.
#[derive(Clone, Copy, Default)]
pub(super) struct Lab {
    /// Exponent k of the lambda factor m^k per AQ multiplier m (0 = off).
    #[cfg(feature = "lab")]
    aq_exp: f32,
    /// Per-channel lambda log scale 1 offsets.
    #[cfg(feature = "lab")]
    shift: [f32; 3],
}

impl Lab {
    pub(super) fn of(cfg: &LossyConfig) -> Self {
        #[cfg(feature = "lab")]
        {
            use core::sync::atomic::Ordering::Relaxed;
            Self {
                aq_exp: cfg
                    .lab
                    .trellis_aq_lambda_exp
                    .unwrap_or_else(|| f32::from_bits(AQ_LAMBDA_EXP.load(Relaxed))),
                shift: cfg.lab.trellis_channel_shift.unwrap_or_else(|| {
                    CH_SHIFT.each_ref().map(|s| f32::from_bits(s.load(Relaxed)))
                }),
            }
        }
        #[cfg(not(feature = "lab"))]
        {
            let _ = cfg;
            Self {}
        }
    }
}

/// The settings for channel `ch` (lab: with its channel offset).
#[inline]
pub(crate) fn for_channel(set: &BaseTrellis, ch: usize, lab: &Lab) -> BaseTrellis {
    #[cfg(feature = "lab")]
    {
        let d = lab.shift[ch];
        if d != 0.0 {
            return BaseTrellis {
                lambda_log_scale1: (set.lambda_log_scale1 + d).max(0.0),
                ..*set
            };
        }
    }
    let _ = (ch, lab);
    *set
}

/// LAB: off, split-Huffman files keep the default quantization (main before
/// 2026-10-09; `lab::set_trellis_huf_rate(false)`).
#[cfg(feature = "lab")]
pub(crate) static HUF_RATE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(true);

/// Whether the effort's trellis runs on XYB split-Huffman files (explicit
/// `SplitHuf`, tier images the cost model sends to split-Huffman, small tier
/// images' split-Huffman candidate), priced with their own code lengths
/// ([`HufPrices`]).
pub(super) fn huf_rate(cfg: &LossyConfig) -> bool {
    #[cfg(feature = "lab")]
    return cfg
        .lab
        .trellis_huf_rate
        .unwrap_or_else(|| HUF_RATE.load(core::sync::atomic::Ordering::Relaxed));
    #[cfg(not(feature = "lab"))]
    {
        let _ = cfg;
        true
    }
}

/// The trellis an encoder at `effort` runs: zenjpeg's lambda, with the
/// effort's rate-model passes (`Effort::trellis_passes`); none below its
/// threshold.
pub(super) fn for_effort(effort: crate::effort::Effort) -> Option<BaseTrellis> {
    let passes = effort.trellis_passes();
    (passes > 0).then(|| BaseTrellis {
        passes,
        ..BaseTrellis::default()
    })
}

impl BaseTrellis {
    #[cfg(feature = "lab")]
    pub(super) fn valid(&self) -> bool {
        self.lambda_log_scale1.is_finite()
            && (0.0..=40.0).contains(&self.lambda_log_scale1)
            && self.lambda_log_scale2.is_finite()
            && (0.0..=40.0).contains(&self.lambda_log_scale2)
            && self.passes <= 4
            && self.min_quality.is_finite()
            && self.aq_coupling.is_finite()
            && (-16.0..=16.0).contains(&self.aq_coupling)
            && self.aq_clamp.is_finite()
            && (0.0..=16.0).contains(&self.aq_clamp)
    }

    /// The block's settings under AQ coupling (zenjpeg's additive mode).
    fn coupled(&self, aq: f32) -> Self {
        let mut adj = aq * self.aq_coupling;
        if self.aq_clamp > 0.0 {
            adj = adj.clamp(-self.aq_clamp, self.aq_clamp);
        }
        Self {
            lambda_log_scale1: (self.lambda_log_scale1 + adj).max(0.0),
            ..*self
        }
    }

    /// zenjpeg's per-block lambda for a mean squared AC coefficient `norm`
    /// (libjpeg integer DCT units).
    pub(crate) fn lambda(&self, norm: f64) -> f64 {
        let two = |x: f32| crate::lossy::transfer::pow(2.0, x as f64);
        if self.lambda_log_scale2 > 0.0 {
            two(self.lambda_log_scale1) / (two(self.lambda_log_scale2) + norm)
        } else {
            two(self.lambda_log_scale1 - 12.0)
        }
    }
}

/// Token costs in bits from frozen tables.
pub(crate) struct Prices {
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
                        crate::mathx::log2(M as f64 / f.get(s).copied().unwrap_or(0).max(1) as f64)
                    })
                })
                .collect(),
        }
    }
    #[inline]
    fn token(&self, ctx: usize, symbol: u32) -> f64 {
        self.bits[self.map[ctx] as usize][symbol as usize]
    }
}

/// Split-Huffman code lengths in bits per context, from the tables the
/// writer would build for the units (explicit, no seeds); a symbol the
/// tables lack costs [`huf::LOG`] bits.
pub(crate) struct HufPrices {
    luma: [[f64; huf::CNT_ALPHA]; huf::LUMA_CTX],
    chroma: [[f64; huf::CNT_ALPHA]; huf::CHROMA_CTX],
    wid: [[f64; huf::WID_ALPHA]; huf::WID_CTX],
}

impl HufPrices {
    /// `k_mode`: the encode's table-count mode ([`super::table_k_mode`]).
    fn new(
        units: &[StripeCoefs],
        threads: usize,
        effort: crate::effort::Effort,
        k_mode: u8,
    ) -> Self {
        let syms: Vec<huf::Syms> = crate::par::par_map(units.iter().collect(), threads, |_, sc| {
            let (counts, widths) = stripe_counts_widths(sc, sc.bw);
            huf::stripe_syms(&counts, &widths, sc.nby * sc.bw)
        });
        let (t, _) = huffman_tables(&syms, threads, effort, None, k_mode);
        let bits = |table: u8, s: usize| match t.lens[table as usize].get(s) {
            Some(&n) if n > 0 => n as f64,
            _ => huf::LOG as f64,
        };
        Self {
            luma: core::array::from_fn(|c| core::array::from_fn(|s| bits(t.luma_map[c], s))),
            chroma: core::array::from_fn(|c| core::array::from_fn(|s| bits(t.chroma_map[c], s))),
            wid: core::array::from_fn(|c| core::array::from_fn(|s| bits(t.wid_map[c], s))),
        }
    }
}

/// The trellis's rate model: run-level tokens, or split-Huffman counts and
/// widths (lab).
pub(crate) enum Rates {
    RunLevel(Prices),
    Huf(alloc::boxed::Box<HufPrices>),
}

/// Group count of an AC support mask (`nz & !1`), as split-Huffman codes
/// it.
#[inline]
pub(crate) fn group_count(ac: u64) -> u32 {
    support(ac).div_ceil(4)
}

/// Split-Huffman widths of a block channel's coded groups.
pub(crate) fn group_widths(q: &[i32], n: u32) -> [u8; PK_GROUPS] {
    let mut w = [0u8; PK_GROUPS];
    for (gi, wi) in w.iter_mut().enumerate().take(n as usize) {
        let m = (0..coef::pk_len(gi))
            .map(|j| q[ZIGZAG[1 + 4 * gi + j] as usize].unsigned_abs())
            .max()
            .unwrap();
        *wi = (32 - m.leading_zeros()) as u8;
    }
    w
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
/// context it sets; `ctx(gi, n)`: group gi's width context under count n.
/// Distortion, lambda and the AQ step scaling are the run-level trellis's.
#[allow(clippy::too_many_arguments)]
pub(crate) fn huf_channel(
    src: &[f32],
    steps: &[f32; 64],
    set: &BaseTrellis,
    count_bits: &[f64; PK_GROUPS + 1],
    wid: &[[f64; huf::WID_ALPHA]; huf::WID_CTX],
    ctx: impl Fn(usize, u32) -> usize,
    q: &mut [i32],
    nz: &mut u64,
) {
    let mut a = [0f64; 64];
    let mut norm = 0f64;
    for k in 1..64 {
        let z = ZIGZAG[k] as usize;
        let c = src[z] as f64;
        norm += c * c;
        a[k] = c.abs() / steps[z] as f64;
    }
    let lw = 64.0 * set.lambda(norm * 64.0 / 63.0);
    // Nearest magnitudes (a >= 0: the cast floors).
    let near: [u32; 64] = core::array::from_fn(|k| (a[k] + 0.5).min(MAX_MAGNITUDE as f64) as u32);
    let nearest = |k: usize| near[k];
    let mut zero = [0f64; PK_GROUPS];
    for (gi, z) in zero.iter_mut().enumerate() {
        *z = (1 + 4 * gi..1 + 4 * gi + coef::pk_len(gi))
            .map(|k| lw * a[k] * a[k])
            .sum();
    }
    // Group gi at width w: (magnitude + sign bits + distortion, values).
    let at = |gi: usize, w: usize| -> (f64, [u32; 4]) {
        let mut vals = [0u32; 4];
        if w == 0 {
            return (zero[gi], vals);
        }
        let (lo, cap) = (1u32 << (w - 1), (1u32 << w) - 1);
        let len = coef::pk_len(gi);
        let mut cost = (len * w) as f64;
        let (mut reached, mut lift) = (false, (f64::INFINITY, 0usize, 0u32));
        for j in 0..len {
            let k = 1 + 4 * gi + j;
            let n = nearest(k);
            let d = |m: u32| lw * (m as f64 - a[k]) * (m as f64 - a[k]);
            let m = n.clamp(1, cap);
            let (z, nzc) = (d(0), d(m) + 1.0);
            let free = if nzc < z {
                vals[j] = m;
                nzc
            } else {
                z
            };
            cost += free;
            reached |= vals[j] >= lo;
            let high = n.clamp(lo, cap);
            let extra = d(high) + 1.0 - free;
            if extra < lift.0 {
                lift = (extra, j, high);
            }
        }
        if !reached {
            cost += lift.0;
            vals[lift.1] = lift.2;
        }
        (cost, vals)
    };
    // Per group, built when the count search first reaches it: the costs of
    // widths 0..ends[gi] (0 = not built yet).
    let mut cost = [[f64::INFINITY; huf::WID_ALPHA]; PK_GROUPS];
    let mut ends = [0usize; PK_GROUPS];
    // The cheapest width of group gi, memoized by (width context, last): a
    // group sees at most five contexts (luma: min(n - gi, 5)), chroma one.
    // The last coded group searches from width 1.
    const NO_KEY: usize = usize::MAX;
    let mut memo = [[(NO_KEY, 0f64, 0usize); 12]; PK_GROUPS];
    let mut choose = |gi: usize, n: u32| -> (f64, usize) {
        let from = usize::from(gi + 1 == n as usize);
        let c = ctx(gi, n);
        let key = c * 2 + from;
        let slot = memo[gi].iter().position(|e| e.0 == key || e.0 == NO_KEY);
        if let Some(s) = slot
            && memo[gi][s].0 == key
        {
            return (memo[gi][s].1, memo[gi][s].2);
        }
        if ends[gi] == 0 {
            let top = (1 + 4 * gi..1 + 4 * gi + coef::pk_len(gi))
                .map(nearest)
                .max()
                .unwrap();
            let wn = (32 - top.leading_zeros()) as usize;
            ends[gi] = (wn + 2).min(huf::WID_ALPHA);
            for w in 0..ends[gi] {
                cost[gi][w] = at(gi, w).0;
            }
        }
        let row = &wid[c];
        let mut best = (f64::INFINITY, 0);
        for w in from..ends[gi] {
            let v = cost[gi][w] + row[w];
            if v < best.0 {
                best = (v, w);
            }
        }
        if let Some(s) = slot {
            memo[gi][s] = (key, best.0, best.1);
        }
        best
    };
    // Every count: its tokens, the coded groups, the zeroed tail.
    let mut tail: f64 = zero.iter().sum();
    let mut best = (count_bits[0] + tail, 0u32);
    for n in 1..=PK_GROUPS as u32 {
        tail -= zero[n as usize - 1];
        let mut total = count_bits[n as usize] + tail;
        for gi in 0..n as usize {
            total += choose(gi, n).0;
            if total >= best.0 {
                break;
            }
        }
        if total < best.0 {
            best = (total, n);
        }
    }
    let n = best.1;
    let mut mask = *nz & 1;
    for &z in &ZIGZAG[1..] {
        q[z as usize] = 0;
    }
    for gi in 0..n as usize {
        let vals = at(gi, choose(gi, n).1).1;
        for (j, &m) in vals.iter().enumerate().take(coef::pk_len(gi)) {
            let k = 1 + 4 * gi + j;
            let z = ZIGZAG[k] as usize;
            q[z] = if src[z] < 0.0 { -(m as i32) } else { m as i32 };
            if m != 0 {
                mask |= 1 << k;
            }
        }
    }
    *nz = mask;
}

/// [`block`]'s split-Huffman search for channel `ch` of block `b`: luma
/// prices its count with the next luma count of its slice and this block's
/// chroma counts and widths (their previous-pass values); chroma prices
/// under the block's current luma count.
pub(crate) fn huf_block_channel(
    sc: &mut StripeCoefs,
    b: usize,
    ch: usize,
    src: &[f32],
    steps: &[f32; 64],
    hp: &HufPrices,
    set: &BaseTrellis,
) {
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
                    bits += hp.wid[huf::wid_ctx(1 + i, gi, n)][w as usize];
                }
            }
            bits
        });
        huf_channel(
            src,
            steps,
            set,
            &count_bits,
            &hp.wid,
            |gi, n| huf::wid_ctx(0, gi, n),
            &mut sc.q[bc * 64..][..64],
            &mut sc.nz[bc],
        );
    } else {
        let l = lum(sc, b);
        let count_bits = hp.chroma[(ch - 1) * huf::CLASSES + huf::class(l)];
        huf_channel(
            src,
            steps,
            set,
            &count_bits,
            &hp.wid,
            |gi, _| huf::wid_ctx(ch, gi, l),
            &mut sc.q[bc * 64..][..64],
            &mut sc.nz[bc],
        );
    }
}

/// The most any token can cost under [`Prices`] (one slot of M).
pub(crate) const TOKEN_MAX_BITS: f64 = LOG_M as f64;
pub(crate) const MAX_MAGNITUDE: u32 = (1 << 22) - 1;

pub(crate) fn support(ac: u64) -> u32 {
    if ac == 0 { 0 } else { 63 - ac.leading_zeros() }
}

/// The lane-order neighbours of block `b` in a unit `bw` blocks wide and
/// `nby` rows tall: the block whose luma EOB is `b`'s EOB context, and the
/// block that takes `b`'s luma EOB as its context (`stripe_tokens` order).
pub(crate) fn lane_neighbors(b: usize, bw: usize, nby: usize) -> (Option<usize>, Option<usize>) {
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
pub(crate) fn class(a: u32) -> u32 {
    if a <= 4 {
        a - 1
    } else {
        4 + (32 - (a - 5).leading_zeros())
    }
}

/// The largest magnitude of class `c`.
#[inline]
pub(crate) fn class_max(c: u32) -> u32 {
    match c {
        0..=4 => c + 1,
        _ => 4 + (1 << (c - 4)),
    }
}

/// Raw bits (sign and remainder) of a magnitude-`a` coefficient.
#[inline]
pub(crate) fn raw_bits(a: u32) -> u32 {
    coef::rl_extra_bits(class(a))
}

/// zenjpeg's `TrellisSpeedMode` limits: (lookback positions, candidates).
pub(crate) fn limits(nonzero: usize, thorough: bool) -> (usize, usize) {
    if thorough || nonzero <= 48 {
        (63, 32)
    } else if nonzero <= 55 {
        (16, 4)
    } else {
        (8, 3)
    }
}

pub(crate) const MAX_CANDS: usize = 28;

/// Candidates per position index: (magnitude, symbol class, raw bits +
/// distortion), reused across blocks.
pub(crate) type Cands = [[(u32, u32, f64); MAX_CANDS]; 64];

/// Trellis-quantizes the AC of one block channel. `src`: canonical FDCT
/// coefficients (raster), `steps`: the channel's steps (raster), `q`/`nz`:
/// the block channel's coefficients (raster) and zigzag support mask, whose
/// DC is kept. `ref_eob`: the EOB context reference; `lookahead[class]`:
/// extra bits other tokens cost when this channel's EOB has that class.
#[allow(clippy::too_many_arguments)]
pub(crate) fn channel(
    src: &[f32],
    steps: &[f32; 64],
    ch: usize,
    ref_eob: u32,
    lookahead: &[f64; coef::EOB_CLASSES],
    prices: &Prices,
    set: &BaseTrellis,
    cands: &mut Cands,
    q: &mut [i32],
    nz: &mut u64,
) {
    channel_impl::<true, true>(
        src, steps, ch, ref_eob, lookahead, prices, set, cands, q, nz,
    )
}

/// [`channel`]; `REUSE` shares the Viterbi prefix whose token contexts are
/// all 31 or more positions before the EOB (the last remaining-positions
/// class, the same for every EOB that far away) across candidate EOBs;
/// `PRUNE` stops the predecessor scan once the zeroed run alone costs the
/// best path found. Both are exact (tests compare all four variants).
#[allow(clippy::too_many_arguments, clippy::needless_range_loop)]
pub(crate) fn channel_impl<const REUSE: bool, const PRUNE: bool>(
    src: &[f32],
    steps: &[f32; 64],
    ch: usize,
    ref_eob: u32,
    lookahead: &[f64; coef::EOB_CLASSES],
    prices: &Prices,
    set: &BaseTrellis,
    cands: &mut Cands,
    q: &mut [i32],
    nz: &mut u64,
) {
    // Zigzag magnitudes in steps, nearest quantized values, block norm.
    let mut a = [0f64; 64];
    let mut qn = [0u32; 64];
    let mut norm = 0f64;
    for k in 1..64 {
        let z = ZIGZAG[k] as usize;
        let c = src[z] as f64;
        norm += c * c;
        a[k] = c.abs() / steps[z] as f64;
        qn[k] = (a[k] + 0.5).floor().min(MAX_MAGNITUDE as f64) as u32;
    }
    // libjpeg's integer DCT is 8x the orthonormal one.
    let w = 64.0 * set.lambda(norm * 64.0 / 63.0);
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
        let dist = |m: u32| w * (m as f64 - a[k]) * (m as f64 - a[k]);
        let top_cost = raw_bits(top) as f64 + dist(top);
        let list = &mut cands[i];
        list[0] = (top, class(top), top_cost);
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
            list[len] = (m, c, cost);
            len += 1;
        }
        ncand[i] = len;
        least[i] = list[..len]
            .iter()
            .map(|c| c.2)
            .fold(f64::INFINITY, f64::min);
    }

    let eob_cost = |e: u32| prices.token(eob_ctx(ch, ref_eob), e) + lookahead[coef::eob_class(e)];
    // Best path ending with a nonzero at position index i, given each start
    // position's table `tk`: (cost, predecessor index or u8::MAX, candidate).
    let step = |i: usize, tk: &[u8; 64], cost: &[f64; 64]| -> (f64, u8, u8) {
        let p = pos[i] as usize;
        let mut bc = f64::INFINITY;
        let (mut bj, mut bk) = (u8::MAX, 0u8);
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
                let mut start = j + 1;
                let mut run = p - j - 1;
                let mut r = prev + (zd[p - 1] - zd[j]);
                while run > coef::MAX_RUN as usize {
                    r += prices.bits[tk[start] as usize][coef::ZRL as usize];
                    start += 4;
                    run -= 4;
                }
                let row = &prices.bits[tk[start] as usize];
                let base = run as u32 * 32;
                for (ci, &(_, cls, extra)) in cands[i][..ncand[i]].iter().enumerate() {
                    let c = r + row[(base + cls) as usize] + extra;
                    if c < bc {
                        bc = c;
                        bj = if m == 0 { u8::MAX } else { (m - 1) as u8 };
                        bk = ci as u8;
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
    // Positions 31 or more before the EOB: one shared context class.
    const FAR: usize = 31;
    let (mut far_cost, mut far_back, mut far_pick) = ([0f64; 64], [0u8; 64], [0u8; 64]);
    let mut far_n = 0;
    if REUSE && n > 0 && pos[n - 1] as usize > FAR {
        let mut tk = [0u8; 64];
        for k in 1..64 {
            tk[k] = prices.map[ac_ctx(ch, k as u32, k as u32 + FAR as u32)];
        }
        while far_n < n && pos[far_n] as usize + FAR <= pos[n - 1] as usize {
            (far_cost[far_n], far_back[far_n], far_pick[far_n]) = step(far_n, &tk, &far_cost);
            far_n += 1;
        }
    }
    // EOB 0: every AC coefficient zero.
    let mut best_total = eob_cost(0) + zd[63];
    let mut best = [0u32; 64];
    let mut cost = [0f64; 64];
    let mut back = [0u8; 64];
    let mut pick = [0u8; 64];
    let mut tk = [0u8; 64];
    // Candidate EOBs from the last position down: the zeroed tail only
    // grows, so once it alone costs as much as the best total, stop.
    for ie in (0..n).rev() {
        let e = pos[ie] as usize;
        let tail = zd[63] - zd[e];
        if tail >= best_total {
            break;
        }
        for k in 1..=e {
            tk[k] = prices.map[ac_ctx(ch, k as u32, e as u32)];
        }
        // Shared prefix: positions with p + FAR <= e (every start s <= p
        // then has e - s >= FAR).
        let mut i0 = 0;
        while i0 < far_n && pos[i0] as usize + FAR <= e {
            i0 += 1;
        }
        cost[..i0].copy_from_slice(&far_cost[..i0]);
        back[..i0].copy_from_slice(&far_back[..i0]);
        pick[..i0].copy_from_slice(&far_pick[..i0]);
        for i in i0..=ie {
            (cost[i], back[i], pick[i]) = step(i, &tk, &cost);
        }
        let total = eob_cost(e as u32) + cost[ie] + tail;
        if total < best_total {
            best_total = total;
            best = [0; 64];
            let mut i = ie as u8;
            while i != u8::MAX {
                let ii = i as usize;
                best[pos[ii] as usize] = cands[ii][pick[ii] as usize].0;
                i = back[ii];
            }
        }
    }
    let mut mask = *nz & 1;
    for k in 1..64 {
        let z = ZIGZAG[k] as usize;
        let m = best[k] as i32;
        q[z] = if src[z] < 0.0 { -m } else { m };
        if m != 0 {
            mask |= 1 << k;
        }
    }
    *nz = mask;
}

/// One block of a unit: luma, then chroma. `src(ch)`: the block's canonical
/// FDCT coefficients of channel `ch` (raster). A block code (`sc.aq`) of 0
/// keeps the block as coded (inter frames); other codes scale its AC steps
/// by `a / 64`, as the quantizer does.
#[allow(clippy::too_many_arguments)]
pub(crate) fn block<'a>(
    sc: &mut StripeCoefs,
    b: usize,
    src: impl Fn(usize) -> &'a [f32],
    steps: &[[f32; 64]; 3],
    rates: &Rates,
    set: &BaseTrellis,
    lab: &Lab,
    cands: &mut Cands,
) {
    let scaled: [[f32; 64]; 3];
    let steps = match sc.aq.get(b) {
        Some(0) => return,
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
    #[cfg(feature = "lab")]
    let adjusted;
    #[cfg(feature = "lab")]
    let set = {
        let k = lab.aq_exp;
        let m = sc.aq.get(b).map_or(1.0, |&a| a as f32 * (1.0 / 64.0));
        if k != 0.0 && m != 1.0 {
            let log2m = crate::lossy::transfer::ln(m as f64) / core::f64::consts::LN_2;
            adjusted = BaseTrellis {
                lambda_log_scale1: set.lambda_log_scale1 + (k as f64 * log2m) as f32,
                ..*set
            };
            &adjusted
        } else {
            set
        }
    };
    let prices = match rates {
        Rates::RunLevel(p) => p,
        Rates::Huf(hp) => {
            for ch in 0..NCH {
                let set = for_channel(set, ch, lab);
                huf_block_channel(sc, b, ch, src(ch), &steps[ch], hp, &set);
            }
            return;
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
        channel(
            src(ch),
            &steps[ch],
            ch,
            reference,
            &la,
            prices,
            &for_channel(set, ch, lab),
            cands,
            &mut sc.q[bc * 64..][..64],
            &mut sc.nz[bc],
        );
    }
}

/// Runs the trellis on every coding unit (stripe-major, `group_bw`-block
/// column groups in order; 0 = no groups). Each stripe recomputes its block
/// rows' coefficients through `rows` (planes `pw` samples wide, `image_bw`
/// blocks). `aq`: lab AQ-coupling strengths per image block (raster), empty
/// otherwise. `huf_model`: price split-Huffman counts and widths
/// ([`HufPrices`], lab) instead of run-level tokens.
#[allow(clippy::too_many_arguments)]
pub(super) fn optimize(
    mut units: Vec<StripeCoefs>,
    rows: RowPlanes,
    pw: usize,
    image_bw: usize,
    group_bw: usize,
    steps: &[[f32; 64]; 3],
    cfg: &LossyConfig,
    effort: crate::effort::Effort,
    set: &BaseTrellis,
    aq: &[f32],
    huf_model: bool,
) -> Vec<StripeCoefs> {
    let groups = if group_bw == 0 {
        1
    } else {
        image_bw.div_ceil(group_bw)
    };
    debug_assert_eq!(units.len() % groups, 0);
    let cm = dct::dct_matrix();
    let lab = Lab::of(cfg);
    for pass in 0..set.passes {
        // Priced with the explicit tables (as RDOQ): seed tables never
        // change which coefficients the trellis keeps.
        let rates = if huf_model {
            Rates::Huf(alloc::boxed::Box::new(HufPrices::new(
                &units,
                cfg.threads,
                effort,
                super::table_k_mode(cfg),
            )))
        } else {
            let (tables, explicit) = ans_tables(&units, cfg, effort);
            Rates::RunLevel(Prices::new(&explicit.unwrap_or(tables)))
        };
        // A stripe's column groups share each block row's transform.
        let mut stripes = Vec::with_capacity(units.len() / groups);
        let mut it = units.into_iter();
        while let Some(first) = it.next() {
            let mut stripe = Vec::with_capacity(groups);
            stripe.push(first);
            stripe.extend(it.by_ref().take(groups - 1));
            stripes.push(stripe);
        }
        let done = crate::par::par_map(stripes, cfg.threads, |_, mut stripe: Vec<StripeCoefs>| {
            let (by0, nby) = (stripe[0].by0, stripe[0].nby);
            let mut planes = vec![0.0f32; NCH * 8 * pw];
            let mut extremes = Vec::with_capacity(image_bw);
            let mut coefs = vec![0.0f32; NCH * image_bw * 64];
            let mut cands: alloc::boxed::Box<Cands> =
                alloc::boxed::Box::new([[(0, 0, 0.0); MAX_CANDS]; 64]);
            for r in 0..nby {
                rows(by0 + r, &mut planes, &mut extremes);
                for ch in 0..NCH {
                    enc_simd::fdct_row(
                        &planes[ch * 8 * pw..][..8 * pw],
                        pw,
                        image_bw,
                        &cm,
                        &mut coefs[ch * image_bw * 64..][..image_bw * 64],
                    );
                }
                for (g, sc) in stripe.iter_mut().enumerate() {
                    let bx0 = g * group_bw;
                    if cfg!(debug_assertions) && pass == 0 {
                        check_sources(sc, r, &coefs, image_bw, bx0, steps, cfg);
                    }
                    for x in 0..sc.bw {
                        let coupled;
                        let set = if set.aq_coupling != 0.0 {
                            let strength = aq
                                .get((by0 + r) * image_bw + bx0 + x)
                                .copied()
                                .unwrap_or(0.0);
                            coupled = set.coupled(strength);
                            &coupled
                        } else {
                            set
                        };
                        let b = r * sc.bw + x;
                        let at = |ch: usize| &coefs[(ch * image_bw + bx0 + x) * 64..][..64];
                        block(sc, b, at, steps, &rates, set, &lab, &mut cands);
                    }
                }
            }
            stripe
        });
        units = done.into_iter().flatten().collect();
    }
    units
}

/// Lab: [tokens, 4-position groups before the EOBs, coded block channels]
/// of coding units (the counts the decode-speed tiers model; column groups
/// do not change them).
#[cfg(all(feature = "lab", feature = "std"))]
pub(super) fn unit_stats(units: &[StripeCoefs], threads: usize) -> [u64; 3] {
    let per = crate::par::par_map(units.iter().collect(), threads, |_, sc: &StripeCoefs| {
        let mut stats = [0u64; 3];
        stripe_tokens(sc, sc.bw, Group::ALL, |_, _, _, _, _| stats[0] += 1);
        for &nz in &sc.nz {
            let ac = nz & !1;
            if ac != 0 {
                stats[1] += (63 - ac.leading_zeros() as u64).div_ceil(4);
                stats[2] += 1;
            }
        }
        stats
    });
    per.iter()
        .fold([0; 3], |a, b| [a[0] + b[0], a[1] + b[1], a[2] + b[2]])
}

/// Debug builds: the recomputed coefficients of row `r` quantize to the
/// encoder's first-pass AC values (every block not kept).
pub(crate) fn check_sources(
    sc: &StripeCoefs,
    r: usize,
    coefs: &[f32],
    image_bw: usize,
    bx0: usize,
    steps: &[[f32; 64]; 3],
    cfg: &LossyConfig,
) {
    let bw = sc.bw;
    let aq: Vec<f32> = if sc.aq.is_empty() {
        Vec::new()
    } else {
        sc.aq[r * bw..][..bw]
            .iter()
            .map(|&a| a.max(1) as f32 * (1.0 / 64.0))
            .collect()
    };
    let mut q = vec![0i32; bw * NCH * 64];
    let mut nz = vec![0u64; bw * NCH];
    for (ch, st) in steps.iter().enumerate() {
        let inv = st.map(|v| 1.0 / v);
        enc_simd::quant_row(
            &coefs[(ch * image_bw + bx0) * 64..][..bw * 64],
            bw,
            ch,
            &inv,
            &aq,
            cfg.round.clamp(0.0, 0.5),
            &mut q,
            &mut nz,
        );
    }
    for x in 0..bw {
        if sc.aq.get(r * bw + x) == Some(&0) {
            continue;
        }
        for ch in 0..NCH {
            let o = (x * NCH + ch) * 64;
            let s = ((r * bw + x) * NCH + ch) * 64;
            debug_assert_eq!(
                q[o + 1..o + 64],
                sc.q[s + 1..s + 64],
                "row {r} block {x} ch {ch}"
            );
        }
    }
}

/// LAB: jpegli's AQ strength per block (raster) of a U8 image, from its
/// luma plane in jpegli units; `luma_rows(by, out)` writes block row `by`'s
/// 8 coded luma rows (`bw * 8` samples each).
#[cfg(feature = "lab")]
pub(super) fn aq_strengths<F>(
    bw: usize,
    bh: usize,
    xyb: bool,
    luma_step_01: f32,
    threads: usize,
    luma_rows: F,
) -> Vec<f32>
where
    F: Fn(usize, &mut [f32]) + Sync,
{
    let units = super::aq_field_lab::luma_units(xyb);
    let qf = super::aq_field_lab::field(bw, bh, luma_step_01 * units, true, threads, |by, out| {
        luma_rows(by, out);
        for v in out.iter_mut() {
            *v *= units;
        }
    });
    super::aq_field_lab::strengths(&qf)
}

pub(crate) fn prices_from(t: &Tables) -> Prices {
    Prices::new(t)
}

pub(crate) fn huf_prices_from(t: &huf::HufTables) -> HufPrices {
    let bits = |table: u8, s: usize| match t.lens[table as usize].get(s) {
        Some(&n) if n > 0 => n as f64,
        _ => huf::LOG as f64,
    };
    HufPrices {
        luma: core::array::from_fn(|c| core::array::from_fn(|s| bits(t.luma_map[c], s))),
        chroma: core::array::from_fn(|c| core::array::from_fn(|s| bits(t.chroma_map[c], s))),
        wid: core::array::from_fn(|c| core::array::from_fn(|s| bits(t.wid_map[c], s))),
    }
}

pub(crate) fn new_cands() -> alloc::boxed::Box<Cands> {
    alloc::boxed::Box::new([[(0, 0, 0.0); MAX_CANDS]; 64])
}

pub(crate) fn base_set(s1: f32, s2: f32, thorough: bool) -> BaseTrellis {
    BaseTrellis {
        lambda_log_scale1: s1,
        lambda_log_scale2: s2,
        thorough,
        ..BaseTrellis::default()
    }
}
