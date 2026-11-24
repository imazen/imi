//! zenjpeg's AC trellis quantization, run by the lossy encoder at high
//! effort (`crate::effort::Effort::trellis_passes`) on XYB files whose AC is
//! coded run-level or split-Huffman, each priced with its own rate model (in
//! YCoCg it lost 18-20% Butteraugli max). Encoder only: the
//! bitstream, the decoder and the DC are unchanged; the decoded pixels
//! change.
//!
//! A port of the trellis zenjpeg's `auto_optimize` ("hybrid") mode runs at
//! q50+ (`zenjpeg/src/encode/trellis/ac.rs`, from mozjpeg's
//! `jcdctmgr.c`), with IMI's own rate model in place of JPEG Huffman code
//! lengths. Measured in `benchmarks/hybrid_trellis_2026-10-07` (lab study)
//! and `benchmarks/trellis_effort_2026-10-08` (effort thresholds).
//!
//! * **Distortion** is zenjpeg's: an AC coefficient at `v` quantizer steps
//!   coded as `c` costs `64 * lambda * (c - v)^2` (flat weights, mozjpeg's
//!   mode 1), with `lambda = 2^s1 / (2^s2 + norm)` and `norm` the block's
//!   mean squared AC coefficient in libjpeg's integer DCT units, 8 times
//!   IMI's orthonormal coefficients (IMI's planes are close to 8-bit
//!   units: XYB luma is 300 Y, jpegli's 302 Y). zenjpeg: `s1` 14.5, `s2`
//!   16.5. Flat 8-bit blocks then trade one bit for 1/16 of a squared step.
//!   Per-block AQ multipliers scale the AC steps as the quantizer does.
//! * **Rate** is IMI's: token costs `log2(M / freq)` from the explicit
//!   rANS tables clustered from the image's own coefficients exactly as the
//!   writer clusters them (pass 1: the default quantization; each later
//!   pass: the previous pass's output), plus the raw sign and remainder
//!   bits. An absent symbol costs one table slot (`log2 M` bits). The
//!   writer then builds its tables (and seed choices) from the final
//!   coefficients, in whichever entropy mode the configuration or tier
//!   picks.
//! * **Contexts** follow the writer. AC token contexts depend on the
//!   block's EOB (positions left before it), so every candidate EOB gets its
//!   own Viterbi pass; the EOB token is priced in its lane context. A luma
//!   EOB also prices this block's two chroma EOB tokens and the next luma
//!   EOB token of the lane (their previous-pass values).
//! * **Candidates** per coefficient: the nearest value and the largest
//!   magnitude of every smaller run-level magnitude class (mozjpeg: 1, 3,
//!   7, ... for JPEG's bit categories), or zero through the run structure.
//!   A candidate whose extra distortion exceeds the most it could save in
//!   bits is dropped (exact). zenjpeg's default `Adaptive` speed limits
//!   apply to blocks with more than 48 nonzero candidates unless
//!   `thorough`.
//! * **Memory**: each pass recomputes a stripe's coefficients one block row
//!   at a time through the encoder's own row pipeline (no image-wide copy).
//! * Inter-frame "keep" blocks stay as coded; subblock layouts, other
//!   entropy modes and colors, and the lab RDOQ / LF prediction modes skip
//!   the effort's trellis (the lab override runs it on other modes and
//!   colors). Small tier images code their split-Huffman candidate from the
//!   pass-1 coefficients.
//! * **AQ coupling** (lab; zenjpeg's `AqCoupling`, off in its
//!   `auto_optimize`): with `aq_coupling != 0` each block's `s1` moves by
//!   `clamp(aq * aq_coupling, +-aq_clamp)`, `aq` jpegli's AQ strength
//!   `max(0, 0.6 / qf - 1)` of the block (`aq_field_lab.rs`; U8 input).
//! * **Split-Huffman rate**: split-Huffman files (explicit, or a tier's
//!   choice) priced with their own code lengths, from the Huffman tables the
//!   writer would build for the previous pass ([`HufPrices`]): per block
//!   channel the group count and, per group, the width and its values
//!   ([`huf_channel`], exact under frozen lengths); the luma count also
//!   prices the tokens whose contexts it sets. Measured in
//!   `benchmarks/aq_trellis_2026-10-09` (rule R2); lab
//!   `lab::set_trellis_huf_rate(false)` restores split-Huffman without the
//!   trellis.
use super::*;
use archmage::prelude::*;
use coef::PK_GROUPS;

/// Trellis settings. The encoder derives them from effort; the lab
/// `LossyConfig::trellis` overrides them for experiments.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LossyTrellis {
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

impl Default for LossyTrellis {
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

    /// Non-zero lab AQ lambda exponent: `block` then adjusts `(set, pw)`
    /// per AQ multiplier, so no block-invariant settings exist to cache.
    #[inline]
    fn aq_lambda_exp(&self) -> f32 {
        #[cfg(feature = "lab")]
        return self.aq_exp;
        #[cfg(not(feature = "lab"))]
        0.0
    }
}

/// The settings for channel `ch` (lab: with its channel offset).
#[inline]
fn for_channel(set: &LossyTrellis, ch: usize, lab: &Lab) -> LossyTrellis {
    #[cfg(feature = "lab")]
    {
        let d = lab.shift[ch];
        if d != 0.0 {
            return LossyTrellis {
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
pub(super) fn for_effort(effort: crate::effort::Effort) -> Option<LossyTrellis> {
    let passes = effort.trellis_passes();
    (passes > 0).then(|| LossyTrellis {
        passes,
        ..LossyTrellis::default()
    })
}

impl LossyTrellis {
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

    /// `pow(2, s1)` and `pow(2, s2)` of a settings value: `lambda` needs
    /// them per block channel but they are constant per settings (same
    /// `pow` inputs, so bit-identical results).
    fn powers(&self) -> Powers {
        let two = |x: f32| crate::lossy::transfer::pow(2.0, x as f64);
        if self.lambda_log_scale2 > 0.0 {
            Powers {
                num: two(self.lambda_log_scale1),
                den: two(self.lambda_log_scale2),
            }
        } else {
            Powers {
                num: two(self.lambda_log_scale1 - 12.0),
                den: 0.0,
            }
        }
    }

    /// `lambda` with precomputed [`Self::powers`].
    fn lambda_p(&self, norm: f64, p: Powers) -> f64 {
        if self.lambda_log_scale2 > 0.0 {
            p.num / (p.den + norm)
        } else {
            p.num
        }
    }

    /// zenjpeg's per-block lambda for a mean squared AC coefficient `norm`
    /// (libjpeg integer DCT units).
    pub(super) fn lambda(&self, norm: f64) -> f64 {
        self.lambda_p(norm, self.powers())
    }
}

/// [`LossyTrellis::powers`].
#[derive(Clone, Copy)]
struct Powers {
    /// `2^s1`, or the whole lambda `2^(s1 - 12)` when `s2 <= 0`.
    num: f64,
    /// `2^s2` (unused when `s2 <= 0`).
    den: f64,
}

// The trellis's f64 SIMD runs on magetypes lanes (`f64x4` of the same
// IEEE adds/mins the scalar expressions emit): `#[magetypes]` generates a
// variant per dispatch tier (`v3`, `neon`, `wasm128`, `scalar`) from one
// body, so every tier is bit-identical by construction and the token only
// changes how a value is computed, never what.

/// Token costs in bits from frozen tables.
pub(super) struct Prices {
    map: Vec<u8>,
    bits: Vec<[f64; coef::ALPHA]>,
    /// `channel_impl`'s initial `tk` per channel: the saturated
    /// remaining-positions class for every position. Fixed per `map`;
    /// the DP copies it and rewrites only the tail per candidate EOB.
    tk0: [[u8; 64]; NCH],
}

impl Prices {
    pub(super) fn new(tables: &Tables) -> Self {
        let map = tables.ctx_map.clone();
        let mut tk0 = [[0u8; 64]; NCH];
        for (ch, tk) in tk0.iter_mut().enumerate() {
            for (k, t) in tk.iter_mut().enumerate().skip(1) {
                *t = map[coef::EOB_CTX
                    + (ch * coef::K_CLASSES + k - 1) * coef::REM_CLASSES
                    + coef::REM_CLASSES
                    - 1];
            }
        }
        Self {
            map,
            bits: tables
                .freq
                .iter()
                .map(|f| {
                    core::array::from_fn(|s| {
                        crate::mathx::log2(M as f64 / f.get(s).copied().unwrap_or(0).max(1) as f64)
                    })
                })
                .collect(),
            tk0,
        }
    }
    #[inline]
    pub(super) fn token(&self, ctx: usize, symbol: u32) -> f64 {
        self.bits[self.map[ctx] as usize][symbol as usize]
    }
}

/// Split-Huffman code lengths in bits per context, from the tables the
/// writer would build for the units (explicit, no seeds); a symbol the
/// tables lack costs [`huf::LOG`] bits. `wid` rows are padded to [`WID_PAD`]
/// lanes for the V3 scan (lanes past `WID_ALPHA` are `INFINITY`).
const WID_PAD: usize = 24;

pub(super) struct HufPrices {
    pub(super) luma: [[f64; huf::CNT_ALPHA]; huf::LUMA_CTX],
    pub(super) chroma: [[f64; huf::CNT_ALPHA]; huf::CHROMA_CTX],
    pub(super) wid: [[f64; WID_PAD]; huf::WID_CTX],
}

impl HufPrices {
    /// The first pass's prices: each unit's counts and widths scan into
    /// section symbols one at a time (later passes take the frequencies
    /// the trellis emitted while it rewrote the blocks).
    /// `k_mode`: the encode's table-count mode ([`super::table_k_mode`]).
    pub(super) fn new(
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
        Self::from_tables(&t)
    }

    /// Prices from the (context, symbol) frequencies the previous pass's
    /// block rewrites emitted — the same tables `new` derives from a
    /// rescan. `k_mode`: the encode's table-count mode
    /// ([`super::table_k_mode`]).
    pub(super) fn from_freqs(f: &Freqs, effort: crate::effort::Effort, k_mode: u8) -> Self {
        let (t, _) = huf::HufTables::build(&f[0], &f[1], &f[2], effort, None, k_mode);
        Self::from_tables(&t)
    }

    fn from_tables(t: &huf::HufTables) -> Self {
        let bits = |table: u8, s: usize| match t.lens[table as usize].get(s) {
            Some(&n) if n > 0 => n as f64,
            _ => huf::LOG as f64,
        };
        Self {
            luma: core::array::from_fn(|c| core::array::from_fn(|s| bits(t.luma_map[c], s))),
            chroma: core::array::from_fn(|c| core::array::from_fn(|s| bits(t.chroma_map[c], s))),
            wid: core::array::from_fn(|c| {
                core::array::from_fn(|s| {
                    if s < huf::WID_ALPHA {
                        bits(t.wid_map[c], s)
                    } else {
                        f64::INFINITY
                    }
                })
            }),
        }
    }
}

/// Split-Huffman (context, symbol) frequencies a pass emits for the next
/// pass's [`HufPrices`]: the same multiset `stripe_syms` would list and
/// `huffman_tables` would count, accumulated per worker as the trellis
/// rewrites the blocks (fixed ~23KB where a retained per-block record
/// would grow with the image).
pub(super) type Freqs = [Vec<Vec<u64>>; 3];

/// A [`Freqs`] as shared atomic cells: each worker merges its per-stripe
/// frequencies into one accumulator at stripe end, so nothing sized by
/// the image survives the pass.
type SharedFreqs = [Vec<Vec<core::sync::atomic::AtomicU64>>; 3];

pub(super) fn freqs_new() -> Freqs {
    [
        vec![vec![0; huf::CNT_ALPHA]; huf::LUMA_CTX],
        vec![vec![0; huf::CNT_ALPHA]; huf::CHROMA_CTX],
        vec![vec![0; huf::WID_ALPHA]; huf::WID_CTX],
    ]
}

fn shared_freqs() -> SharedFreqs {
    let rows = |ctx: usize, alpha: usize| {
        (0..ctx)
            .map(|_| {
                (0..alpha)
                    .map(|_| core::sync::atomic::AtomicU64::new(0))
                    .collect()
            })
            .collect()
    };
    [
        rows(huf::LUMA_CTX, huf::CNT_ALPHA),
        rows(huf::CHROMA_CTX, huf::CNT_ALPHA),
        rows(huf::WID_CTX, huf::WID_ALPHA),
    ]
}

/// Fold block `b`'s final split-Huffman counts and widths into `f`: the
/// same (context, symbol) pairs `stripe_syms` lists for the block.
fn emit_huf(f: &mut Freqs, sc: &StripeCoefs, b: usize, hw: Option<&[(u32, [u8; PK_GROUPS]); NCH]>) {
    // `hw` carries the channel loop's chosen (count, widths); blocks it
    // skipped (aq-0) or a run-level call rescan `nz`/`q` for the same
    // values (`at` lifts a lane to `2^(w-1)`, so the scan reads `w` back).
    let count = |bc: usize, c: usize| match hw {
        Some(hw) => hw[c].0 as usize,
        None => group_count(sc.nz[bc] & !1) as usize,
    };
    let n0 = count(b * NCH, 0);
    let prev = if b.is_multiple_of(huf::slice_len(sc.nby * sc.bw)) {
        0
    } else {
        group_count(sc.nz[(b - 1) * NCH] & !1) as usize
    };
    f[0][huf::class(prev as u32)][n0] += 1;
    let cl = huf::class(n0 as u32);
    for c in 1..NCH {
        f[1][(c - 1) * huf::CLASSES + cl][count(b * NCH + c, c)] += 1;
    }
    for c in 0..NCH {
        let bc = b * NCH + c;
        let n = count(bc, c);
        let scan;
        let ws: &[u8; PK_GROUPS] = match hw {
            Some(hw) => &hw[c].1,
            None => {
                scan = group_widths(&sc.q[bc * 64..][..64], n as u32);
                &scan
            }
        };
        for (gi, &w) in ws.iter().enumerate().take(n) {
            f[2][huf::wid_ctx(c, gi, n0 as u32)][w as usize] += 1;
        }
    }
}

/// The trellis's rate model: run-level tokens, or split-Huffman counts and
/// widths (lab).
// One `Rates` per encode, read in the hot loop: keep `Prices` inline.
#[allow(clippy::large_enum_variant)]
enum Rates {
    RunLevel(Prices),
    Huf(alloc::boxed::Box<HufPrices>),
}

/// Group count of an AC support mask (`nz & !1`), as split-Huffman codes
/// it.
#[inline]
fn group_count(ac: u64) -> u32 {
    support(ac).div_ceil(4)
}

/// Split-Huffman widths of a block channel's coded groups. The width is
/// the max magnitude's bit length, which is also the OR's top bit — one
/// reduction instead of a max chain.
fn group_widths(q: &[i32], n: u32) -> [u8; PK_GROUPS] {
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
fn huf_channel(
    token: Token,
    src: &[f32],
    steps: &[f32; 64],
    set: &LossyTrellis,
    pw: Powers,
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
    let lw = 64.0 * set.lambda_p(norm * 64.0 / 63.0, pw);
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
#[magetypes(v3, neon, wasm128, scalar)]
#[allow(unused_variables)]
fn huf_block_channel(
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
            huf_channel(
                src,
                steps,
                set,
                pw,
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
            huf_channel(
                src,
                steps,
                set,
                pw,
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

/// The most any token can cost under [`Prices`] (one slot of M).
const TOKEN_MAX_BITS: f64 = LOG_M as f64;
const MAX_MAGNITUDE: u32 = (1 << 22) - 1;

pub(super) fn support(ac: u64) -> u32 {
    if ac == 0 { 0 } else { 63 - ac.leading_zeros() }
}

/// The lane-order neighbours of block `b` in a unit `bw` blocks wide and
/// `nby` rows tall: the block whose luma EOB is `b`'s EOB context, and the
/// block that takes `b`'s luma EOB as its context (`stripe_tokens` order).
pub(super) fn lane_neighbors(b: usize, bw: usize, nby: usize) -> (Option<usize>, Option<usize>) {
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
fn class(a: u32) -> u32 {
    if a <= 4 {
        a - 1
    } else {
        4 + (32 - (a - 5).leading_zeros())
    }
}

/// The largest magnitude of class `c`.
#[inline]
fn class_max(c: u32) -> u32 {
    match c {
        0..=4 => c + 1,
        _ => 4 + (1 << (c - 4)),
    }
}

/// Raw bits (sign and remainder) of a magnitude-`a` coefficient.
#[inline]
fn raw_bits(a: u32) -> u32 {
    coef::rl_extra_bits(class(a))
}

/// zenjpeg's `TrellisSpeedMode` limits: (lookback positions, candidates).
fn limits(nonzero: usize, thorough: bool) -> (usize, usize) {
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
struct Cands {
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
fn setup(
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
fn channel(
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
#[magetypes(v3, neon, wasm128, scalar)]
#[allow(clippy::too_many_arguments, clippy::needless_range_loop)]
fn channel_impl<const REUSE: bool, const PRUNE: bool>(
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
    // Zigzag magnitudes in steps, nearest quantized values, block norm.
    let mut a = [0f64; 64];
    let mut qn = [0u32; 64];
    let norm = incant!(setup(src, steps, &mut a, &mut qn) with token, [v3, neon, wasm128, scalar]);
    // libjpeg's integer DCT is 8x the orthonormal one.
    let w = 64.0 * set.lambda_p(norm * 64.0 / 63.0, pw);
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

/// One block of a unit: luma, then chroma. `src(ch)`: the block's canonical
/// FDCT coefficients of channel `ch` (raster). `pw`: `set.powers()` (the
/// pow inputs are constant per settings). A block code (`sc.aq`) of 0
/// keeps the block as coded (inter frames); other codes scale its AC steps
/// by `a / 64`, as the quantizer does.
#[magetypes(v3, neon, wasm128, scalar)]
#[allow(unused_variables, clippy::too_many_arguments)]
fn block<'a>(
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
                    huf_block_channel(
                        sc,
                        b,
                        ch,
                        src(ch),
                        &steps[ch],
                        hp,
                        &cs,
                        pw,
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
            channel_impl::<true, true>(
                src(ch),
                &steps[ch],
                ch,
                reference,
                &la,
                prices,
                &cs,
                pw,
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

/// Runs the trellis on every coding unit (stripe-major, `group_bw`-block
/// column groups in order; 0 = no groups). Each stripe recomputes its block
/// rows' coefficients through `rows` (planes `pw` samples wide, `image_bw`
/// blocks). `aq`: lab AQ-coupling strengths per image block (raster), empty
/// otherwise. `huf_model`: price split-Huffman counts and widths
/// ([`HufPrices`], lab) instead of run-level tokens.
#[allow(clippy::too_many_arguments)]
/// One rate-model pass: its prices and the settings every stripe shares.
pub(super) struct Pass {
    rates: Rates,
    lab: Lab,
    pw0: Powers,
    cspw: Option<[(LossyTrellis, Powers); NCH]>,
}

impl Pass {
    fn new(rates: Rates, set: &LossyTrellis, cfg: &LossyConfig, huf_model: bool) -> Self {
        let lab = Lab::of(cfg);
        // `2^s1`/`2^s2` are constant for the whole call; lab AQ coupling or a
        // channel shift recompute them per settings value.
        let pw0 = set.powers();
        // Channel settings/powers are block-invariant without AQ coupling
        // or a lab AQ lambda exponent (which adjusts `set` per block).
        let cspw: Option<[(LossyTrellis, Powers); NCH]> =
            (huf_model && set.aq_coupling == 0.0 && lab.aq_lambda_exp() == 0.0).then(|| {
                core::array::from_fn(|ch| {
                    let cs = for_channel(set, ch, &lab);
                    (cs, if cs == *set { pw0 } else { cs.powers() })
                })
            });
        Self {
            rates,
            lab,
            pw0,
            cspw,
        }
    }

    /// A run-level pass priced from run-level token counts
    /// (`hist[ctx * ALPHA + token]` as `ans_tables` counts them): the tables
    /// `ans_tables` builds from the same units, explicit when seeded.
    pub(super) fn run_level(
        hist: &[u64],
        set: &LossyTrellis,
        cfg: &LossyConfig,
        effort: crate::effort::Effort,
    ) -> Self {
        let rows: Vec<Vec<u64>> = hist
            .as_chunks::<{ super::coef::ALPHA }>()
            .0
            .iter()
            .map(|row| row.to_vec())
            .collect();
        let (tables, explicit) = Tables::build_seeded(
            &rows,
            4,
            cfg.max_ac_tables,
            LossyEntropy::RunLevel,
            effort,
            crate::seeds::bank(crate::seeds::DEFAULT_BANK),
            super::table_k_mode(cfg),
        );
        Self::new(
            Rates::RunLevel(Prices::new(&explicit.unwrap_or(tables))),
            set,
            cfg,
            false,
        )
    }

    /// A split-Huffman pass priced from (context, symbol) frequencies: the
    /// pass-1 symbol counts, or the previous pass's rewrites.
    pub(super) fn split_huf(
        freqs: &Freqs,
        set: &LossyTrellis,
        cfg: &LossyConfig,
        effort: crate::effort::Effort,
    ) -> Self {
        Self::new(
            Rates::Huf(alloc::boxed::Box::new(HufPrices::from_freqs(
                freqs,
                effort,
                super::table_k_mode(cfg),
            ))),
            set,
            cfg,
            true,
        )
    }
}

/// Rewrites one stripe's units (its column groups in order) with `pass`:
/// recomputes each block row through `rows`, then re-decides every block's
/// AC coefficients. `freqs` gathers the split-Huffman frequencies the next
/// pass prices from. `index` is the pass number (0: debug source checks).
#[allow(clippy::too_many_arguments)]
pub(super) fn rewrite_stripe(
    pass: &Pass,
    stripe: &mut [StripeCoefs],
    rows: RowPlanes,
    pw: usize,
    image_bw: usize,
    group_bw: usize,
    steps: &[[f32; 64]; 3],
    cfg: &LossyConfig,
    set: &LossyTrellis,
    aq: &[f32],
    bias: Option<&[[[f32; 64]; 2]; 3]>,
    index: usize,
    mut freqs: Option<&mut Freqs>,
    stop: &dyn how_far::Stop,
) -> Result<(), Error> {
    stop.check().map_err(|_| Error::Cancelled)?;
    let cm = dct::dct_matrix();
    let (by0, nby) = (stripe[0].by0, stripe[0].nby);
    let mut planes = vec![0.0f32; NCH * 8 * pw];
    let mut extremes = Vec::with_capacity(image_bw);
    let mut coefs = vec![0.0f32; NCH * image_bw * 64];
    // Small transforms: a block's 4x4 planes take their 4x4
    // coefficients and steps (SPEC 2.11).
    let any_split = stripe.iter().any(|sc| sc.splits.iter().any(|&v| v != 0));
    let mut coefs4 = vec![0.0f32; if any_split { NCH * 64 } else { 0 }];
    let mut cands: alloc::boxed::Box<Cands> = alloc::boxed::Box::default();
    for r in 0..nby {
        stop.check().map_err(|_| Error::Cancelled)?;
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
            if cfg!(debug_assertions) && index == 0 {
                check_sources(sc, r, &coefs, image_bw, bx0, steps, cfg);
            }
            for x in 0..sc.bw {
                stop.check().map_err(|_| Error::Cancelled)?;
                let b = r * sc.bw + x;
                // Merged parents keep the selector's quantization.
                if sc.modes.get(b).is_some_and(|&m| m != 0) {
                    continue;
                }
                let split = sc.splits.get(b).copied().unwrap_or(0);
                let local;
                let steps = if split == 0 {
                    steps
                } else {
                    for ch in 0..NCH {
                        if split >> ch & 1 == 0 {
                            continue;
                        }
                        let px: [f32; 64] = core::array::from_fn(|i| {
                            planes[ch * 8 * pw + (i / 8) * pw + (bx0 + x) * 8 + i % 8]
                        });
                        coefs4[ch * 64..][..64]
                            .copy_from_slice(&super::small::forward_cfg(cfg, &px));
                    }
                    local = core::array::from_fn(|ch| {
                        if split >> ch & 1 != 0 {
                            super::small::steps4(&steps[ch])
                        } else {
                            steps[ch]
                        }
                    });
                    &local
                };
                // 4x4 planes reconstruct without the bias (B = 0).
                let local_bias;
                let bias = match bias {
                    Some(t) if split != 0 => {
                        local_bias = core::array::from_fn::<_, NCH, _>(|ch| {
                            if split >> ch & 1 != 0 {
                                [[0.0f32; 64]; 2]
                            } else {
                                t[ch]
                            }
                        });
                        Some(&local_bias)
                    }
                    b => b,
                };
                let coupled;
                let (set, pw) = if set.aq_coupling != 0.0 {
                    let strength = aq
                        .get((by0 + r) * image_bw + bx0 + x)
                        .copied()
                        .unwrap_or(0.0);
                    coupled = set.coupled(strength);
                    (&coupled, coupled.powers())
                } else {
                    (set, pass.pw0)
                };
                let at = |ch: usize| -> &[f32] {
                    if split >> ch & 1 != 0 {
                        &coefs4[ch * 64..][..64]
                    } else {
                        &coefs[(ch * image_bw + bx0 + x) * 64..][..64]
                    }
                };
                let hw = incant!(
                    block(
                        sc,
                        b,
                        at,
                        steps,
                        &pass.rates,
                        set,
                        &pass.lab,
                        pw,
                        bias,
                        &mut cands,
                        pass.cspw.as_ref()
                    ),
                    [v3, neon, wasm128, scalar]
                );
                if let Some(f) = &mut freqs {
                    emit_huf(f, sc, b, hw.as_ref());
                }
            }
        }
    }
    stop.check().map_err(|_| Error::Cancelled)
}

pub(super) fn optimize(
    mut units: Vec<StripeCoefs>,
    rows: RowPlanes,
    pw: usize,
    image_bw: usize,
    group_bw: usize,
    steps: &[[f32; 64]; 3],
    cfg: &LossyConfig,
    effort: crate::effort::Effort,
    set: &LossyTrellis,
    aq: &[f32],
    bias: Option<&[[[f32; 64]; 2]; 3]>,
    huf_model: bool,
    stop: &dyn how_far::Stop,
) -> Result<Vec<StripeCoefs>, Error> {
    stop.check().map_err(|_| Error::Cancelled)?;
    let groups = if group_bw == 0 {
        1
    } else {
        image_bw.div_ceil(group_bw)
    };
    debug_assert_eq!(units.len() % groups, 0);
    // Split-Huffman (context, symbol) frequencies a pass's rewritten
    // blocks emitted: the next pass's `HufPrices` builds its tables from
    // them (small and fixed-size where a retained per-block record would
    // grow with the image).
    let mut freqs: Option<Freqs> = None;
    for index in 0..set.passes {
        stop.check().map_err(|_| Error::Cancelled)?;
        // Price explicit frequencies using the usual table-count decision,
        // independently of the bank selected for the written file.
        let rates = if huf_model {
            let hp = match freqs.take() {
                Some(f) => HufPrices::from_freqs(&f, effort, super::table_k_mode(cfg)),
                None => HufPrices::new(&units, cfg.threads, effort, super::table_k_mode(cfg)),
            };
            Rates::Huf(alloc::boxed::Box::new(hp))
        } else {
            let (tables, explicit) = super::ans_tables_for_decisions(&units, cfg, effort);
            Rates::RunLevel(Prices::new(&explicit.unwrap_or(tables)))
        };
        let pass = Pass::new(rates, set, cfg, huf_model);
        // A stripe's column groups share each block row's transform.
        let mut stripes = Vec::with_capacity(units.len() / groups);
        let mut it = units.into_iter();
        while let Some(first) = it.next() {
            let mut stripe = Vec::with_capacity(groups);
            stripe.push(first);
            stripe.extend(it.by_ref().take(groups - 1));
            stripes.push(stripe);
        }
        // Only a pass with a successor emits frequencies for it, into one
        // shared accumulator (workers merge per-stripe rows at stripe end).
        let emit = huf_model && index + 1 < set.passes;
        let shared = emit.then(shared_freqs);
        let done = crate::par::par_map(stripes, cfg.threads, |_, mut stripe| {
            let mut freqs = emit.then(freqs_new);
            rewrite_stripe(
                &pass,
                &mut stripe,
                rows,
                pw,
                image_bw,
                group_bw,
                steps,
                cfg,
                set,
                aq,
                bias,
                index as usize,
                freqs.as_mut(),
                stop,
            )?;
            if let (Some(f), Some(shared)) = (freqs, &shared) {
                for (g, l) in shared.iter().zip(f) {
                    for (g, l) in g.iter().zip(l) {
                        for (g, v) in g.iter().zip(l) {
                            g.fetch_add(v, core::sync::atomic::Ordering::Relaxed);
                        }
                    }
                }
            }
            Ok::<_, Error>(stripe)
        });
        units = done
            .into_iter()
            .collect::<Result<Vec<_>, Error>>()?
            .into_iter()
            .flatten()
            .collect();
        freqs = shared.map(|shared| {
            shared.map(|ctx| {
                ctx.into_iter()
                    .map(|row| row.into_iter().map(|a| a.into_inner()).collect())
                    .collect()
            })
        });
    }
    stop.check().map_err(|_| Error::Cancelled)?;
    Ok(units)
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
fn check_sources(
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
    let base_round = cfg.round.clamp(0.0, 0.5);
    let round = if cfg.freq_zero_bias {
        -base_round.max(0.40)
    } else {
        base_round
    };
    for (ch, st) in steps.iter().enumerate() {
        let inv = st.map(|v| 1.0 / v);
        enc_simd::quant_row(
            &coefs[(ch * image_bw + bx0) * 64..][..bw * 64],
            bw,
            ch,
            &inv,
            &aq,
            round,
            &mut q,
            &mut nz,
        );
    }
    for x in 0..bw {
        let b = r * bw + x;
        if sc.aq.get(b) == Some(&0) || sc.modes.get(b).is_some_and(|&m| m != 0) {
            continue;
        }
        for ch in 0..NCH {
            // 4x4 planes were quantized from their own transform.
            if sc.splits.get(b).is_some_and(|&v| v >> ch & 1 != 0) {
                continue;
            }
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

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    #[test]
    fn trellis_cancellation_interrupts_inside_the_first_stripe() {
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
        let set = for_effort(crate::effort::Effort::new(9.0).unwrap()).unwrap();
        let result = optimize(
            vec![sc],
            &rows,
            bw * 8,
            bw,
            0,
            &[[1.0; 64]; 3],
            &cfg,
            crate::effort::Effort::new(9.0).unwrap(),
            &set,
            &[],
            None,
            false,
            &stop,
        );
        assert!(matches!(result, Err(Error::Cancelled)));
        assert_eq!(
            stop.rows.load(Ordering::Relaxed),
            1,
            "cancellation must not finish all rows of the stripe"
        );
    }

    /// Every block of a unit in raster order; `src` laid out like `sc.q`.
    fn unit(
        sc: &mut StripeCoefs,
        src: &[f32],
        steps: &[[f32; 64]; 3],
        rates: &Rates,
        set: &LossyTrellis,
        _aq: impl Fn(usize) -> f32,
    ) {
        let mut cands: alloc::boxed::Box<Cands> = alloc::boxed::Box::default();
        let pw = set.powers();
        let lab = Lab::default();
        let cspw: [(LossyTrellis, Powers); NCH] = core::array::from_fn(|ch| {
            let cs = for_channel(set, ch, &lab);
            (cs, if cs == *set { pw } else { cs.powers() })
        });
        for b in 0..sc.nby * sc.bw {
            let at = |ch: usize| &src[(b * NCH + ch) * 64..][..64];
            block_scalar(
                ScalarToken,
                sc,
                b,
                at,
                steps,
                rates,
                set,
                &lab,
                pw,
                None,
                &mut cands,
                Some(&cspw),
            );
        }
    }

    fn prices_for(sc: &StripeCoefs) -> Prices {
        let (tables, explicit) = ans_tables(
            core::slice::from_ref(sc),
            &LossyConfig::default(),
            crate::effort::Effort::new(10.0).unwrap(),
        );
        Prices::new(&explicit.unwrap_or(tables))
    }

    /// Exact cost of a unit's tokens under `prices` (the writer's sequence).
    fn rate(sc: &StripeCoefs, prices: &Prices) -> f64 {
        let mut bits = 0.0;
        stripe_tokens(sc, sc.bw, Group::ALL, |_, ctx, tok, n, _| {
            bits += prices.token(ctx, tok) + n as f64;
        });
        bits
    }

    fn lcg(seed: &mut u64) -> f64 {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (*seed >> 11) as f64 / (1u64 << 53) as f64
    }

    /// A unit of random coefficients: AC nonzero with probability `dense`,
    /// mostly small, some large.
    fn fixture_with(
        bw: usize,
        nby: usize,
        dense: f64,
        seed: &mut u64,
    ) -> (StripeCoefs, Vec<f32>, [[f32; 64]; 3]) {
        let steps: [[f32; 64]; 3] = core::array::from_fn(|c| {
            core::array::from_fn(|i| 1.0 + ((i * 7 + c) % 13) as f32 / 3.0)
        });
        let nb = bw * nby;
        let mut src = vec![0f32; nb * NCH * 64];
        let mut q = vec![0i32; nb * NCH * 64];
        let mut nz = vec![0u64; nb * NCH];
        for bc in 0..nb * NCH {
            let ch = bc % NCH;
            for (k, &z) in ZIGZAG.iter().enumerate() {
                let z = z as usize;
                let r = lcg(seed);
                let mag = if k == 0 {
                    40.0 * lcg(seed)
                } else if r >= dense {
                    0.0
                } else if r < 0.78 * dense {
                    1.6 * lcg(seed) / (1.0 + k as f64 / 16.0)
                } else {
                    9.0 * lcg(seed)
                };
                let v = if lcg(seed) < 0.5 { -mag } else { mag };
                src[bc * 64 + z] = (v * steps[ch][z] as f64) as f32;
                let m = (mag + 0.4).floor() as i32;
                q[bc * 64 + z] = if v < 0.0 { -m } else { m };
                if m != 0 {
                    nz[bc] |= 1 << k;
                }
            }
        }
        let sc = StripeCoefs {
            by0: 0,
            nby,
            bw,
            q,
            nz,
            aq: vec![],
            modes: vec![],

            splits: Vec::new(),
        };
        (sc, src, steps)
    }

    fn fixture(bw: usize, nby: usize, seed: &mut u64) -> (StripeCoefs, Vec<f32>, [[f32; 64]; 3]) {
        fixture_with(bw, nby, 0.45, seed)
    }

    fn block_distortion(src: &[f32], steps: &[f32; 64], q: &[i32], w: f64) -> f64 {
        (1..64)
            .map(|i| {
                let v = src[i] as f64 / steps[i] as f64;
                w * (v - q[i] as f64).powi(2)
            })
            .sum()
    }

    /// Enumerate every candidate combination of one block channel (each
    /// position: zero or one of its candidates) and check the trellis
    /// reaches the minimum of distortion + the writer's exact rate (this
    /// channel's tokens, its EOB token and the EOB tokens whose context it
    /// sets).
    #[test]
    fn trellis_matches_exhaustive_search() {
        let mut seed = 7u64;
        let set = LossyTrellis {
            thorough: true,
            ..LossyTrellis::default()
        };
        let mut checked = 0;
        for trial in 0..400 {
            let (bw, nby) = [(1, 1), (3, 2), (17, 2), (20, 1)][trial % 4];
            // Sparse blocks keep the enumeration small; some dense ones
            // around them shape the tables and contexts.
            let (mut sc, src, steps) = fixture_with(bw, nby, 0.07, &mut seed);
            let prices = prices_for(&sc);
            let b = (lcg(&mut seed) * (bw * nby) as f64) as usize;
            let ch = trial % NCH;
            let bc = b * NCH + ch;
            // Keep enumeration small: at most 5 positions with a candidate.
            let s = &src[bc * 64..][..64];
            let nonzero: Vec<usize> = (1..64)
                .filter(|&k| {
                    let z = ZIGZAG[k] as usize;
                    (s[z].abs() / steps[ch][z] + 0.5).floor() >= 1.0
                })
                .collect();
            if nonzero.len() > 5 || nonzero.is_empty() {
                continue;
            }
            let norm = (1..64).map(|i| (s[i] as f64).powi(2)).sum::<f64>() * 64.0 / 63.0;
            let w = 64.0 * set.lambda(norm);
            // Process blocks before b with the trellis so the lane state is
            // what `unit` would see; then compare at block b, channel ch.
            let objective = |sc: &StripeCoefs| {
                block_distortion(&src[bc * 64..][..64], &steps[ch], &sc.q[bc * 64..][..64], w)
                    + rate(sc, &prices)
            };
            // Candidate magnitudes per position (zero included).
            let options: Vec<Vec<u32>> = nonzero
                .iter()
                .map(|&k| {
                    let z = ZIGZAG[k] as usize;
                    let a = s[z].abs() as f64 / steps[ch][z] as f64;
                    let top = (a + 0.5).floor() as u32;
                    let mut o = vec![0, top];
                    for c in 0..class(top) {
                        o.push(class_max(c));
                    }
                    o
                })
                .collect();
            let mut best = f64::INFINITY;
            let total: usize = options.iter().map(Vec::len).product();
            let original = sc.q[bc * 64..][..64].to_vec();
            let original_nz = sc.nz[bc];
            for mut idx in 0..total {
                let mut nzm = original_nz & 1;
                for (o, &k) in options.iter().zip(&nonzero) {
                    let m = o[idx % o.len()] as i32;
                    idx /= o.len();
                    let z = ZIGZAG[k] as usize;
                    sc.q[bc * 64 + z] = if s[z] < 0.0 { -m } else { m };
                    if m != 0 {
                        nzm |= 1 << k;
                    }
                }
                for k in 1..64 {
                    if !nonzero.contains(&k) {
                        sc.q[bc * 64 + ZIGZAG[k] as usize] = 0;
                    }
                }
                sc.nz[bc] = nzm;
                best = best.min(objective(&sc));
            }
            sc.q[bc * 64..][..64].copy_from_slice(&original);
            sc.nz[bc] = original_nz;
            // The trellis on this channel alone, in its lane context.
            let (previous, next) = lane_neighbors(b, bw, nby);
            let reference = if ch == 0 {
                previous.map_or(0, |p| support(sc.nz[p * NCH] & !1))
            } else {
                support(sc.nz[b * NCH] & !1)
            };
            let la: [f64; coef::EOB_CLASSES] = if ch == 0 {
                let chroma = [
                    support(sc.nz[b * NCH + 1] & !1),
                    support(sc.nz[b * NCH + 2] & !1),
                ];
                core::array::from_fn(|cls| {
                    let e = if cls == 0 { 0 } else { 1u32 << (cls - 1) };
                    prices.token(eob_ctx(1, e), chroma[0])
                        + prices.token(eob_ctx(2, e), chroma[1])
                        + next.map_or(0.0, |n| {
                            prices.token(eob_ctx(0, e), support(sc.nz[n * NCH] & !1))
                        })
                })
            } else {
                [0.0; coef::EOB_CLASSES]
            };
            let mut q = sc.q[bc * 64..][..64].to_vec();
            let mut nzm = sc.nz[bc];
            let mut cands = alloc::boxed::Box::<Cands>::default();
            channel(
                s,
                &steps[ch],
                ch,
                reference,
                &la,
                &prices,
                &set,
                set.powers(),
                &mut cands,
                &mut q,
                &mut nzm,
            );
            sc.q[bc * 64..][..64].copy_from_slice(&q);
            sc.nz[bc] = nzm;
            let got = objective(&sc);
            assert!(
                (got - best).abs() <= 1e-9 * best.abs().max(1.0),
                "trial {trial}: trellis {got} vs exhaustive {best}"
            );
            assert_eq!(sc.q[bc * 64], original[0], "DC kept");
            checked += 1;
        }
        assert!(checked > 100, "{checked}");
    }

    #[test]
    fn huge_lambda_rounds_to_nearest_and_tiny_lambda_zeroes() {
        let mut seed = 3u64;
        let (sc0, src, steps) = fixture(17, 2, &mut seed);
        let prices = Rates::RunLevel(prices_for(&sc0));
        let near = LossyTrellis {
            lambda_log_scale1: 40.0,
            lambda_log_scale2: 0.0,
            thorough: true,
            ..LossyTrellis::default()
        };
        let mut sc = StripeCoefs {
            q: sc0.q.clone(),
            nz: sc0.nz.clone(),
            aq: vec![],
            modes: vec![],

            splits: Vec::new(),
            ..sc0
        };
        unit(&mut sc, &src, &steps, &prices, &near, |_| 0.0);
        for bc in 0..sc.nz.len() {
            let ch = bc % NCH;
            for k in 1..64 {
                let z = ZIGZAG[k] as usize;
                let v = src[bc * 64 + z] / steps[ch][z];
                let m = (v.abs() as f64 + 0.5).floor() as i32;
                assert_eq!(sc.q[bc * 64 + z], if v < 0.0 { -m } else { m });
                assert_eq!((sc.nz[bc] >> k) & 1, (m != 0) as u64);
            }
        }
    }

    fn huf_prices_for(sc: &StripeCoefs) -> HufPrices {
        HufPrices::new(
            core::slice::from_ref(sc),
            1,
            crate::effort::Effort::new(10.0).unwrap(),
            0,
        )
    }

    /// Exact split-Huffman bits of a unit under `hp`: the three sections'
    /// codes, magnitude bits and sign bits (the writer's sequence).
    fn huf_rate_of(sc: &StripeCoefs, hp: &HufPrices) -> f64 {
        let (counts, widths) = stripe_counts_widths(sc, sc.bw);
        let syms = huf::stripe_syms(&counts, &widths, sc.nby * sc.bw);
        let mut bits = 0.0;
        for &(c, v) in &syms.luma {
            bits += hp.luma[c as usize][v as usize];
        }
        for &(c, v) in &syms.chroma {
            bits += hp.chroma[c as usize][v as usize];
        }
        for &(c, v) in &syms.wid {
            bits += hp.wid[c as usize][v as usize];
        }
        for bc in 0..counts.len() {
            for gi in 0..counts[bc] as usize {
                let w = widths[bc * PK_GROUPS + gi] as usize;
                bits += (coef::pk_len(gi) * w) as f64;
                bits += (0..coef::pk_len(gi))
                    .filter(|&j| sc.q[bc * 64 + ZIGZAG[1 + 4 * gi + j] as usize] != 0)
                    .count() as f64;
            }
        }
        bits
    }

    /// Enumerate every magnitude 0..=nearest of the nonzero candidates of one
    /// block channel and check the split-Huffman trellis reaches at most
    /// that minimum of distortion + the writer's exact bits (its search also
    /// covers one width above).
    #[test]
    fn huf_trellis_matches_exhaustive_search() {
        let mut seed = 11u64;
        let set = LossyTrellis::default();
        let mut checked = 0;
        for trial in 0..600 {
            let (bw, nby) = [(1, 1), (3, 2), (17, 2), (9, 1)][trial % 4];
            let (mut sc, src, steps) = fixture_with(bw, nby, 0.06, &mut seed);
            let hp = huf_prices_for(&sc);
            let b = (lcg(&mut seed) * (bw * nby) as f64) as usize;
            let ch = trial % NCH;
            let bc = b * NCH + ch;
            let s = &src[bc * 64..][..64];
            let nearest: Vec<(usize, u32)> = (1..64)
                .filter_map(|k| {
                    let z = ZIGZAG[k] as usize;
                    let m = (s[z].abs() / steps[ch][z] + 0.5).floor() as u32;
                    (m >= 1).then_some((k, m))
                })
                .collect();
            let total: usize = nearest.iter().map(|&(_, m)| m as usize + 1).product();
            if nearest.is_empty() || total > 3000 {
                continue;
            }
            let norm = (1..64).map(|i| (s[i] as f64).powi(2)).sum::<f64>() * 64.0 / 63.0;
            let w = 64.0 * set.lambda(norm);
            let objective = |sc: &StripeCoefs| {
                block_distortion(s, &steps[ch], &sc.q[bc * 64..][..64], w) + huf_rate_of(sc, &hp)
            };
            let original = sc.q[bc * 64..][..64].to_vec();
            let original_nz = sc.nz[bc];
            let mut best = f64::INFINITY;
            for mut idx in 0..total {
                let mut nzm = original_nz & 1;
                for k in 1..64 {
                    sc.q[bc * 64 + ZIGZAG[k] as usize] = 0;
                }
                for &(k, top) in &nearest {
                    let m = (idx % (top as usize + 1)) as i32;
                    idx /= top as usize + 1;
                    let z = ZIGZAG[k] as usize;
                    sc.q[bc * 64 + z] = if s[z] < 0.0 { -m } else { m };
                    if m != 0 {
                        nzm |= 1 << k;
                    }
                }
                sc.nz[bc] = nzm;
                best = best.min(objective(&sc));
            }
            sc.q[bc * 64..][..64].copy_from_slice(&original);
            sc.nz[bc] = original_nz;
            huf_block_channel_scalar(
                ScalarToken,
                &mut sc,
                b,
                ch,
                s,
                &steps[ch],
                &hp,
                &set,
                set.powers(),
                None,
            );
            let got = objective(&sc);
            let scalar_out = (sc.q[bc * 64..][..64].to_vec(), sc.nz[bc]);
            sc.q[bc * 64..][..64].copy_from_slice(&original);
            sc.nz[bc] = original_nz;
            // The vector lanes compute the same values where the CPU
            // offers a tier (bit-identical is what the byte gate checks).
            #[cfg(target_arch = "x86_64")]
            if let Some(t) = <archmage::X64V3Token as archmage::SimdToken>::summon() {
                huf_block_channel_v3(
                    t,
                    &mut sc,
                    b,
                    ch,
                    s,
                    &steps[ch],
                    &hp,
                    &set,
                    set.powers(),
                    None,
                );
                assert_eq!(
                    (sc.q[bc * 64..][..64].to_vec(), sc.nz[bc]),
                    scalar_out,
                    "trial {trial}: V3 differs from scalar"
                );
            }
            #[cfg(target_arch = "aarch64")]
            if let Some(t) = <archmage::NeonToken as archmage::SimdToken>::summon() {
                huf_block_channel_neon(
                    t,
                    &mut sc,
                    b,
                    ch,
                    s,
                    &steps[ch],
                    &hp,
                    &set,
                    set.powers(),
                    None,
                );
                assert_eq!(
                    (sc.q[bc * 64..][..64].to_vec(), sc.nz[bc]),
                    scalar_out,
                    "trial {trial}: NEON differs from scalar"
                );
            }
            #[cfg(target_arch = "wasm32")]
            if let Some(t) = <archmage::Wasm128Token as archmage::SimdToken>::summon() {
                huf_block_channel_wasm128(
                    t,
                    &mut sc,
                    b,
                    ch,
                    s,
                    &steps[ch],
                    &hp,
                    &set,
                    set.powers(),
                    None,
                );
                assert_eq!(
                    (sc.q[bc * 64..][..64].to_vec(), sc.nz[bc]),
                    scalar_out,
                    "trial {trial}: wasm128 differs from scalar"
                );
            }
            assert!(
                got <= best + 1e-9 * best.abs().max(1.0),
                "trial {trial}: trellis {got} vs exhaustive {best}"
            );
            assert_eq!(scalar_out.0[0], original[0], "DC kept");
            checked += 1;
        }
        assert!(checked > 150, "{checked}");
    }

    #[test]
    fn huf_trellis_limits() {
        let mut seed = 13u64;
        let (sc0, src, steps) = fixture(17, 2, &mut seed);
        let rates = Rates::Huf(alloc::boxed::Box::new(huf_prices_for(&sc0)));
        let fresh = || StripeCoefs {
            q: sc0.q.clone(),
            nz: sc0.nz.clone(),
            aq: vec![],
            modes: vec![],

            splits: Vec::new(),
            ..sc0
        };
        let near = LossyTrellis {
            lambda_log_scale1: 40.0,
            lambda_log_scale2: 0.0,
            ..LossyTrellis::default()
        };
        let mut sc = fresh();
        unit(&mut sc, &src, &steps, &rates, &near, |_| 0.0);
        for bc in 0..sc.nz.len() {
            let ch = bc % NCH;
            for k in 1..64 {
                let z = ZIGZAG[k] as usize;
                let v = src[bc * 64 + z] / steps[ch][z];
                let m = (v.abs() as f64 + 0.5).floor() as i32;
                assert_eq!(sc.q[bc * 64 + z], if v < 0.0 { -m } else { m });
                assert_eq!((sc.nz[bc] >> k) & 1, (m != 0) as u64);
            }
        }
        let zero = LossyTrellis {
            lambda_log_scale1: 0.0,
            lambda_log_scale2: 0.0,
            ..LossyTrellis::default()
        };
        // A luma count also prices its block's previous-pass chroma tokens,
        // so a luma group may stay; chroma has no lookahead.
        let mut sc = fresh();
        unit(&mut sc, &src, &steps, &rates, &zero, |_| 0.0);
        let ones = |nz: &[u64], ch: usize| -> u32 {
            nz.iter()
                .skip(ch)
                .step_by(NCH)
                .map(|v| (v & !1).count_ones())
                .sum()
        };
        for bc in 0..sc.nz.len() {
            assert_eq!(sc.q[bc * 64], sc0.q[bc * 64], "DC kept");
            if bc % NCH != 0 {
                assert_eq!(sc.nz[bc] & !1, 0, "block channel {bc}");
            }
        }
        assert!(
            ones(&sc.nz, 0) * 20 <= ones(&sc0.nz, 0),
            "{}",
            ones(&sc.nz, 0)
        );
    }

    #[test]
    fn shared_prefix_and_pruning_match_the_plain_search() {
        let mut seed = 5u64;
        for (dense, s1) in [
            (0.45, 14.5),
            (0.9, 14.5),
            (0.9, 12.0),
            (0.97, 17.0),
            (0.6, 15.5),
        ] {
            let (sc, src, steps) = fixture_with(17, 2, dense, &mut seed);
            let prices = prices_for(&sc);
            for thorough in [false, true] {
                let set = LossyTrellis {
                    lambda_log_scale1: s1,
                    thorough,
                    ..LossyTrellis::default()
                };
                let mut cands = alloc::boxed::Box::<Cands>::default();
                for bc in 0..sc.nz.len() {
                    let ch = bc % NCH;
                    let la = [0.5; coef::EOB_CLASSES];
                    let pw = set.powers();
                    let run = |variant: u8, cands: &mut Cands, v3: bool| {
                        let mut q = sc.q[bc * 64..][..64].to_vec();
                        let mut nzm = sc.nz[bc];
                        let s = &src[bc * 64..][..64];
                        macro_rules! ci {
                            ($f:ident, $t:expr) => {
                                match variant {
                                    0 => $f::<false, false>(
                                        $t, s, &steps[ch], ch, 3, &la, &prices, &set, pw, None,
                                        cands, &mut q, &mut nzm,
                                    ),
                                    1 => $f::<true, false>(
                                        $t, s, &steps[ch], ch, 3, &la, &prices, &set, pw, None,
                                        cands, &mut q, &mut nzm,
                                    ),
                                    2 => $f::<false, true>(
                                        $t, s, &steps[ch], ch, 3, &la, &prices, &set, pw, None,
                                        cands, &mut q, &mut nzm,
                                    ),
                                    _ => $f::<true, true>(
                                        $t, s, &steps[ch], ch, 3, &la, &prices, &set, pw, None,
                                        cands, &mut q, &mut nzm,
                                    ),
                                }
                            };
                        }
                        #[cfg(target_arch = "x86_64")]
                        if v3 {
                            ci!(
                                channel_impl_v3,
                                <archmage::X64V3Token as archmage::SimdToken>::summon().unwrap()
                            );
                            return (q, nzm);
                        }
                        let _ = v3;
                        ci!(channel_impl_scalar, ScalarToken);
                        (q, nzm)
                    };
                    let plain = run(0, &mut cands, false);
                    for variant in 1..4 {
                        assert_eq!(
                            run(variant, &mut cands, false),
                            plain,
                            "dense {dense} s1 {s1} bc {bc} v{variant}"
                        );
                    }
                    // The V3 lane eval computes the same values where the
                    // CPU offers it.
                    #[cfg(target_arch = "x86_64")]
                    if <archmage::X64V3Token as archmage::SimdToken>::summon().is_some() {
                        for variant in 0..4 {
                            assert_eq!(
                                run(variant, &mut cands, true),
                                plain,
                                "v3 dense {dense} s1 {s1} bc {bc} v{variant}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn class_maxima_follow_rl_token() {
        for a in 1..5000u32 {
            let (s, _, n) = coef::rl_token(0, a as i32);
            assert_eq!(s & 31, class(a));
            assert_eq!(n, raw_bits(a));
            assert!(class_max(class(a)) >= a);
            assert_eq!(class(class_max(class(a))), class(a));
            if class(a) > 0 {
                assert!(class_max(class(a) - 1) < a);
            }
        }
    }
}

/// Review scratch (2026-10-09): differential fuzz of this tree's run-level
/// and split-Huffman trellis against the 35ab183b code (`trellis_base`), on
/// the vector arm and the scalar arm. `DIFF_ITERS` per test (default
/// 20000), `DIFF_SEED` base seed. Mismatches print the case and fail.
#[cfg(all(test, feature = "encode"))]
mod difffuzz {
    use super::*;
    use crate::lossy::trellis_base as tb;

    struct Rng(u64);
    impl Rng {
        fn u(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let x = self.0;
            (x ^ (x >> 29)).wrapping_mul(0xbf58476d1ce4e5b9) >> 11
        }
        fn f(&mut self) -> f64 {
            self.u() as f64 / (1u64 << 53) as f64
        }
        fn below(&mut self, n: u64) -> u64 {
            self.u() % n.max(1)
        }
    }

    fn iters() -> usize {
        std::env::var("DIFF_ITERS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20000)
    }
    fn seed0() -> u64 {
        std::env::var("DIFF_SEED")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1)
    }

    /// Steps: random, powers of two (exact quotients), equal, or tiny.
    fn steps(r: &mut Rng) -> [f32; 64] {
        // DIFF_STEPSCALE: multiply every step (a caller's tiny `channel_mul`).
        let k: f32 = std::env::var("DIFF_STEPSCALE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0);
        steps_base(r).map(|s| s * k)
    }

    fn steps_base(r: &mut Rng) -> [f32; 64] {
        match r.below(5) {
            0 => core::array::from_fn(|_| (0.05 + 60.0 * r.f()) as f32),
            1 => core::array::from_fn(|_| (2f32).powi(r.below(8) as i32 - 2)),
            2 => [(0.25 + 10.0 * r.f()) as f32; 64],
            3 => core::array::from_fn(|_| (0.004 + 0.05 * r.f()) as f32),
            _ => core::array::from_fn(|i| 1.0 + ((i * 7) % 13) as f32 / 3.0),
        }
    }

    /// One block channel's raster coefficients in units of `st`, from one of
    /// several shapes: dense, sparse, half-integer ties, integer values,
    /// class-boundary values, one late coefficient, huge, all tiny.
    fn coefs(r: &mut Rng, st: &[f32; 64]) -> [f32; 64] {
        let mode = r.below(9);
        let dense = [0.02, 0.1, 0.3, 0.6, 0.95][r.below(5) as usize];
        let scale = [0.7, 2.0, 6.0, 40.0, 900.0, 3.0e5][r.below(6) as usize];
        let mut a = [0f64; 64];
        for k in 1..64 {
            if r.f() >= dense {
                continue;
            }
            a[k] = match mode {
                0 | 1 => scale * r.f() * r.f(),
                2 => r.below(scale as u64 + 1) as f64 + 0.5,
                3 => r.below(scale as u64 + 1) as f64,
                4 => {
                    let c = r.below(12) as u32;
                    let m = tb::class_max(c) as f64;
                    m + [-0.5, -0.25, 0.0, 0.25, 0.5, 0.49999][r.below(6) as usize]
                }
                5 => {
                    if k > 40 {
                        scale * (0.5 + r.f())
                    } else {
                        0.3 * r.f()
                    }
                }
                6 => 0.4999 * r.f(),
                7 => scale * 1e3 * r.f(),
                _ => (r.below(4) as f64) * 0.5 + 0.5 * r.f() * r.f(),
            };
        }
        let mut src = [0f32; 64];
        for k in 1..64 {
            let z = ZIGZAG[k] as usize;
            let v = (a[k] * st[z] as f64) as f32;
            src[z] = if r.below(2) == 0 { -v } else { v };
        }
        src[0] = (r.f() * 400.0) as f32;
        src
    }

    fn settings(r: &mut Rng) -> (LossyTrellis, tb::BaseTrellis) {
        let s1 = match r.below(4) {
            0 | 1 => 14.5,
            2 => (40.0 * r.f()) as f32,
            _ => [0.0, 8.0, 11.0, 17.0, 24.0, 40.0][r.below(6) as usize],
        };
        let s2 = match r.below(4) {
            0 | 1 => 16.5,
            2 => 0.0,
            _ => (40.0 * r.f()) as f32,
        };
        let thorough = r.below(3) == 0;
        let n = LossyTrellis {
            lambda_log_scale1: s1,
            lambda_log_scale2: s2,
            thorough,
            ..LossyTrellis::default()
        };
        (n, tb::base_set(s1, s2, thorough))
    }

    /// A random unit and its realistic run-level tables, then (mode 1/2)
    /// the frequencies replaced by power-of-two counts (integer prices,
    /// many ties) or sparse ones (absent symbols cost a whole slot).
    fn rl_tables(r: &mut Rng) -> Tables {
        let (bw, nby) = (1 + r.below(12) as usize, 1 + r.below(2) as usize);
        let (sc, _) = unit(r, bw, nby);
        let (t, e) = ans_tables(
            core::slice::from_ref(&sc),
            &LossyConfig::default(),
            crate::effort::Effort::new(10.0).unwrap(),
        );
        let mut t = e.unwrap_or(t);
        match r.below(3) {
            0 => {}
            1 => {
                for f in t.freq.iter_mut() {
                    for v in f.iter_mut() {
                        *v = 1 << r.below(10);
                    }
                }
            }
            _ => {
                for f in t.freq.iter_mut() {
                    for v in f.iter_mut() {
                        *v = if r.below(3) == 0 {
                            0
                        } else {
                            1 + r.below(50) as u32
                        };
                    }
                }
            }
        }
        if r.below(4) == 0 {
            let nt = t.freq.len() as u64;
            for m in t.ctx_map.iter_mut() {
                *m = r.below(nt) as u8;
            }
        }
        t
    }

    /// A unit of random nearest-quantized coefficients and its sources.
    fn unit(r: &mut Rng, bw: usize, nby: usize) -> (StripeCoefs, Vec<f32>) {
        let nb = bw * nby;
        let mut src = vec![0f32; nb * NCH * 64];
        let mut q = vec![0i32; nb * NCH * 64];
        let mut nz = vec![0u64; nb * NCH];
        let st = steps(r);
        for bc in 0..nb * NCH {
            let s = coefs(r, &st);
            src[bc * 64..][..64].copy_from_slice(&s);
            for k in 0..64 {
                let z = ZIGZAG[k] as usize;
                let a = (s[z].abs() / st[z]) as f64;
                let m = (a + 0.5).floor().min(tb::MAX_MAGNITUDE as f64) as i32;
                q[bc * 64 + z] = if s[z] < 0.0 { -m } else { m };
                if m != 0 {
                    nz[bc] |= 1 << k;
                }
            }
        }
        let sc = StripeCoefs {
            by0: 0,
            nby,
            bw,
            q,
            nz,
            aq: vec![],
            modes: vec![],

            splits: Vec::new(),
        };
        (sc, src)
    }

    /// Which tiers each loop runs: scalar always, plus the host's vector
    /// tier where its token summons.
    fn arms() -> &'static [&'static str] {
        #[cfg(target_arch = "x86_64")]
        {
            assert!(
                <archmage::X64V3Token as archmage::SimdToken>::summon().is_some(),
                "run on an AVX2 host"
            );
            return &["scalar", "v3"];
        }
        #[cfg(target_arch = "aarch64")]
        {
            assert!(
                <archmage::NeonToken as archmage::SimdToken>::summon().is_some(),
                "run on a NEON host"
            );
            return &["scalar", "neon"];
        }
        #[cfg(target_arch = "wasm32")]
        {
            assert!(
                <archmage::Wasm128Token as archmage::SimdToken>::summon().is_some(),
                "run on a wasm128 host"
            );
            return &["scalar", "wasm128"];
        }
        #[allow(unreachable_code)]
        &["scalar"]
    }

    /// `channel_impl::<true, true>` on the named tier.
    #[allow(clippy::too_many_arguments)]
    fn channel_arm(
        arm: &str,
        src: &[f32],
        st: &[f32; 64],
        ch: usize,
        ref_eob: u32,
        la: &[f64; coef::EOB_CLASSES],
        pn: &Prices,
        sn: &LossyTrellis,
        cn: &mut Cands,
        qn: &mut [i32],
        nzn: &mut u64,
    ) {
        let pw = sn.powers();
        match arm {
            #[cfg(target_arch = "x86_64")]
            "v3" => channel_impl_v3::<true, true>(
                <archmage::X64V3Token as archmage::SimdToken>::summon().unwrap(),
                src,
                st,
                ch,
                ref_eob,
                la,
                pn,
                sn,
                pw,
                None,
                cn,
                qn,
                nzn,
            ),
            #[cfg(target_arch = "aarch64")]
            "neon" => channel_impl_neon::<true, true>(
                <archmage::NeonToken as archmage::SimdToken>::summon().unwrap(),
                src,
                st,
                ch,
                ref_eob,
                la,
                pn,
                sn,
                pw,
                None,
                cn,
                qn,
                nzn,
            ),
            #[cfg(target_arch = "wasm32")]
            "wasm128" => channel_impl_wasm128::<true, true>(
                <archmage::Wasm128Token as archmage::SimdToken>::summon().unwrap(),
                src,
                st,
                ch,
                ref_eob,
                la,
                pn,
                sn,
                pw,
                None,
                cn,
                qn,
                nzn,
            ),
            _ => channel_impl_scalar::<true, true>(
                archmage::ScalarToken,
                src,
                st,
                ch,
                ref_eob,
                la,
                pn,
                sn,
                pw,
                None,
                cn,
                qn,
                nzn,
            ),
        }
    }

    /// `huf_block_channel` on the named tier.
    #[allow(clippy::too_many_arguments)]
    fn huf_arm(
        arm: &str,
        s: &mut StripeCoefs,
        b: usize,
        ch: usize,
        src: &[f32],
        st: &[f32; 64],
        hn: &HufPrices,
        sn: &LossyTrellis,
    ) -> (u32, [u8; PK_GROUPS]) {
        let pw = sn.powers();
        match arm {
            #[cfg(target_arch = "x86_64")]
            "v3" => huf_block_channel_v3(
                <archmage::X64V3Token as archmage::SimdToken>::summon().unwrap(),
                s,
                b,
                ch,
                src,
                st,
                hn,
                sn,
                pw,
                None,
            ),
            #[cfg(target_arch = "aarch64")]
            "neon" => huf_block_channel_neon(
                <archmage::NeonToken as archmage::SimdToken>::summon().unwrap(),
                s,
                b,
                ch,
                src,
                st,
                hn,
                sn,
                pw,
                None,
            ),
            #[cfg(target_arch = "wasm32")]
            "wasm128" => huf_block_channel_wasm128(
                <archmage::Wasm128Token as archmage::SimdToken>::summon().unwrap(),
                s,
                b,
                ch,
                src,
                st,
                hn,
                sn,
                pw,
                None,
            ),
            _ => {
                huf_block_channel_scalar(archmage::ScalarToken, s, b, ch, src, st, hn, sn, pw, None)
            }
        }
    }

    #[test]
    fn rl_channel_matches_base() {
        let mut r = Rng(seed0());
        let mut cn: alloc::boxed::Box<Cands> = alloc::boxed::Box::default();
        let mut cb = tb::new_cands();
        let mut tables = rl_tables(&mut r);
        let (mut pn, mut pb) = (Prices::new(&tables), tb::prices_from(&tables));
        let mut fails = 0;
        for it in 0..iters() {
            if it % 200 == 0 {
                tables = rl_tables(&mut r);
                (pn, pb) = (Prices::new(&tables), tb::prices_from(&tables));
            }
            let st = steps(&mut r);
            let src = coefs(&mut r, &st);
            let (sn, sb) = settings(&mut r);
            let ch = r.below(3) as usize;
            let ref_eob = r.below(64) as u32;
            let la: [f64; coef::EOB_CLASSES] = match r.below(3) {
                0 => [0.0; coef::EOB_CLASSES],
                1 => core::array::from_fn(|_| r.below(24) as f64),
                _ => core::array::from_fn(|_| 20.0 * r.f()),
            };
            let q0: [i32; 64] =
                core::array::from_fn(|i| if i == 0 { 17 } else { r.below(3) as i32 });
            let nz0 = 1 | (r.u() & !1);
            let (mut qb, mut nzb) = (q0, nz0);
            tb::channel(
                &src, &st, ch, ref_eob, &la, &pb, &sb, &mut cb, &mut qb, &mut nzb,
            );
            for &arm in arms() {
                let (mut qn, mut nzn) = (q0, nz0);
                channel_arm(
                    arm, &src, &st, ch, ref_eob, &la, &pn, &sn, &mut cn, &mut qn, &mut nzn,
                );
                if qn != qb || nzn != nzb {
                    fails += 1;
                    if fails <= 5 {
                        std::eprintln!(
                            "RL MISMATCH it {it} arm {arm} set {sn:?} ch {ch} ref {ref_eob} la {la:?}\nsrc {src:?}\nsteps {st:?}\nbase {qb:?} {nzb:#x}\nnew  {qn:?} {nzn:#x}"
                        );
                    }
                }
            }
        }
        assert_eq!(fails, 0, "run-level mismatches");
    }

    fn huf_tables(r: &mut Rng) -> huf::HufTables {
        let (bw, nby) = (1 + r.below(12) as usize, 1 + r.below(2) as usize);
        let (sc, _) = unit(r, bw, nby);
        let (counts, widths) = stripe_counts_widths(&sc, sc.bw);
        let syms = [huf::stripe_syms(&counts, &widths, sc.nby * sc.bw)];
        let (mut t, _) =
            huffman_tables(&syms, 1, crate::effort::Effort::new(10.0).unwrap(), None, 0);
        match r.below(3) {
            0 => {}
            1 => {
                for l in t.lens.iter_mut() {
                    for v in l.iter_mut() {
                        *v = r.below(10) as u8;
                    }
                }
            }
            _ => {
                let v = 1 + r.below(9) as u8;
                for l in t.lens.iter_mut() {
                    for x in l.iter_mut() {
                        *x = if r.below(4) == 0 { 0 } else { v };
                    }
                }
            }
        }
        t
    }

    #[test]
    fn huf_block_channel_matches_base() {
        let mut r = Rng(seed0() ^ 0x5eed);
        let mut t = huf_tables(&mut r);
        let (mut hn, mut hb) = (HufPrices::from_tables(&t), tb::huf_prices_from(&t));
        let mut fails = 0;
        for it in 0..iters() {
            if it % 200 == 0 {
                t = huf_tables(&mut r);
                (hn, hb) = (HufPrices::from_tables(&t), tb::huf_prices_from(&t));
            }
            let (bw, nby) = (1 + r.below(9) as usize, 1 + r.below(2) as usize);
            let (sc0, _) = unit(&mut r, bw, nby);
            let st = steps(&mut r);
            let src = coefs(&mut r, &st);
            let (sn, sb) = settings(&mut r);
            let b = r.below((bw * nby) as u64) as usize;
            let ch = r.below(3) as usize;
            let bc = b * NCH + ch;
            // Previous-pass state of this block channel: nearest values, or
            // zeroed (the all-zero fast path needs `nz` AC empty).
            let mut start = StripeCoefs {
                q: sc0.q.clone(),
                nz: sc0.nz.clone(),
                aq: vec![],
                modes: vec![],

                splits: Vec::new(),
                ..sc0
            };
            if r.below(3) == 0 {
                start.q[bc * 64 + 1..bc * 64 + 64].fill(0);
                start.nz[bc] &= 1;
            }
            let mut sb_ = StripeCoefs {
                q: start.q.clone(),
                nz: start.nz.clone(),
                aq: vec![],
                modes: vec![],

                splits: Vec::new(),
                ..start
            };
            tb::huf_block_channel(&mut sb_, b, ch, &src, &st, &hb, &sb);
            for &arm in arms() {
                let mut s = StripeCoefs {
                    q: start.q.clone(),
                    nz: start.nz.clone(),
                    aq: vec![],
                    modes: vec![],

                    splits: Vec::new(),
                    ..start
                };
                let (n, w) = huf_arm(arm, &mut s, b, ch, &src, &st, &hn, &sn);
                let ok = s.q[bc * 64..][..64] == sb_.q[bc * 64..][..64] && s.nz[bc] == sb_.nz[bc];
                let n_scan = group_count(s.nz[bc] & !1);
                let w_scan = group_widths(&s.q[bc * 64..][..64], n_scan);
                let hw_ok = n == n_scan && w == w_scan;
                if !ok || !hw_ok {
                    fails += 1;
                    if fails <= 5 {
                        std::eprintln!(
                            "HUF MISMATCH it {it} arm {arm} ok {ok} hw_ok {hw_ok} set {sn:?} b {b} ch {ch} bw {bw} nby {nby}\nsrc {src:?}\nsteps {st:?}\nbase {:?} {:#x}\nnew  {:?} {:#x} ret ({n}, {w:?}) scan ({n_scan}, {w_scan:?})",
                            &sb_.q[bc * 64..][..64],
                            sb_.nz[bc],
                            &s.q[bc * 64..][..64],
                            s.nz[bc]
                        );
                    }
                }
            }
        }
        assert_eq!(fails, 0, "split-Huffman mismatches");
    }
}
