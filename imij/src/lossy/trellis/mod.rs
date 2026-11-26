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
mod huf_dp;
mod rates;
mod rl_dp;
#[cfg(all(test, feature = "encode"))]
mod tests;

pub(in crate::lossy) use huf_dp::trellis_unit_sh;
pub(in crate::lossy) use rates::{Freqs, HufPrices, Prices, freqs_new};
pub(in crate::lossy) use rl_dp::{Cands, lane_neighbors, support, trellis_unit_rl};

use super::adaptive_rdo::{try_push, try_vec, try_vec_cap};
use super::*;
use rates::{Powers, Rates, emit_huf, shared_freqs};
use rl_dp::dispatch_block_norm;

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
pub(in crate::lossy) struct Lab {
    /// Exponent k of the lambda factor m^k per AQ multiplier m (0 = off).
    #[cfg(feature = "lab")]
    pub(super) aq_exp: f32,
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
pub(super) fn for_channel(set: &LossyTrellis, ch: usize, lab: &Lab) -> LossyTrellis {
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
pub(in crate::lossy) fn huf_rate(cfg: &LossyConfig) -> bool {
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
#[allow(dead_code)]
pub(in crate::lossy) fn for_effort(effort: crate::effort::Effort) -> Option<LossyTrellis> {
    for_effort_plan(effort.lossy_plan(crate::effort::AdaptiveMode::Off))
}

/// The trellis for a centralized [`crate::effort::LossyEffortPlan`].
pub(in crate::lossy) fn for_effort_plan(
    plan: crate::effort::LossyEffortPlan,
) -> Option<LossyTrellis> {
    (plan.trellis_passes > 0).then(|| LossyTrellis {
        passes: plan.trellis_passes,
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
    pub(in crate::lossy) fn lambda(&self, norm: f64) -> f64 {
        self.lambda_p(norm, self.powers())
    }
}

/// Runs the trellis on every coding unit (stripe-major, `group_bw`-block
/// column groups in order; 0 = no groups). Each stripe recomputes its block
/// rows' coefficients through `rows` (planes `pw` samples wide, `image_bw`
/// blocks). `aq`: lab AQ-coupling strengths per image block (raster), empty
/// otherwise. `huf_model`: price split-Huffman counts and widths
/// ([`HufPrices`], lab) instead of run-level tokens.
#[allow(clippy::too_many_arguments)]
/// One rate-model pass: its prices and the settings every stripe shares.
pub(in crate::lossy) struct Pass {
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
    pub(in crate::lossy) fn run_level(
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
    pub(in crate::lossy) fn split_huf(
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

/// Parameters shared across every block of a [`rewrite_stripe_lambda`] call.
pub(in crate::lossy) struct StripeRewrite<'a> {
    pub(in crate::lossy) rows: RowPlanes<'a>,
    pub(in crate::lossy) pw: usize,
    pub(in crate::lossy) image_bw: usize,
    pub(in crate::lossy) group_bw: usize,
    pub(in crate::lossy) steps: &'a [[f32; 64]; 3],
    pub(in crate::lossy) cfg: &'a LossyConfig,
    pub(in crate::lossy) set: &'a LossyTrellis,
    pub(in crate::lossy) aq: &'a [f32],
    pub(in crate::lossy) lambda_map: &'a [f32],
    pub(in crate::lossy) lambda_by0: usize,
    pub(in crate::lossy) merged_trellis: bool,
    pub(in crate::lossy) bias: Option<&'a [[[f32; 64]; 2]; 3]>,
    pub(in crate::lossy) pass_index: usize,
}

impl StripeRewrite<'_> {
    #[inline]
    fn lambda_at(&self, by: usize, bx: usize) -> f64 {
        if self.lambda_map.is_empty() {
            1.0
        } else {
            let rel_by = by.saturating_sub(self.lambda_by0);
            self.lambda_map
                .get(rel_by * self.image_bw + bx)
                .copied()
                .unwrap_or(1.0) as f64
        }
    }

    /// Resolves per-block AQ-coupled `(LossyTrellis, Powers)` when lab AQ
    /// coupling is enabled, else returns the pass-invariant settings.
    #[inline]
    fn block_settings<'a>(
        &'a self,
        pass: &Pass,
        by: usize,
        bx: usize,
        slot: &'a mut LossyTrellis,
    ) -> (&'a LossyTrellis, Powers) {
        if self.set.aq_coupling != 0.0 {
            let strength = self.aq.get(by * self.image_bw + bx).copied().unwrap_or(0.0);
            *slot = self.set.coupled(strength);
            (slot, slot.powers())
        } else {
            (self.set, pass.pw0)
        }
    }
}

/// Precomputed merged superblock parent units for one 2-block-row strip of a
/// column group, so raster-order trellis preserves exact lane/slice EOB
/// contexts while computing each merged block's spectrum once.
struct MergedStripUnits {
    src: Vec<Option<[[f32; 64]; NCH]>>,
    steps: Vec<[[f32; 64]; NCH]>,
    norms: Vec<[f64; NCH]>,
    idx: Vec<usize>,
}

impl MergedStripUnits {
    #[allow(clippy::too_many_arguments)]
    fn build(
        sc: &StripeCoefs,
        enc: &super::superblock::select::Enc,
        planes2: &[Vec<f32>; 2],
        pw: usize,
        r0: usize,
        rr: usize,
        bx0: usize,
        sp: &mut [super::superblock::select::Spectra; NCH],
    ) -> Result<Self, Error> {
        let sbw = sc.bw;
        let slots = rr.checked_mul(sbw).ok_or(Error::TooLarge)?;
        let mut src = try_vec(slots, None)?;
        let mut steps = Vec::new();
        let mut norms = Vec::new();
        let mut idx = try_vec(slots, usize::MAX)?;
        if sc.modes.is_empty() {
            return Ok(Self {
                src,
                steps,
                norms,
                idx,
            });
        }
        for gx in (0..sbw).step_by(2) {
            let cols = (sbw - gx).min(2);
            let mut has_m = false;
            for dy in 0..rr {
                for dx in 0..cols {
                    if sc.modes[(r0 + dy) * sbw + gx + dx] != 0 {
                        has_m = true;
                    }
                }
            }
            if !has_m {
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
            let Some(sym) = super::superblock::symbol_of(roles) else {
                continue;
            };
            enc.spectra_at(planes2, pw, rr, cols, bx0 + gx, sp);
            for &(s, py0, px0) in super::superblock::BLOCKS[sym as usize] {
                let si = s as usize;
                let gather = &enc.gather[si];
                let n_parents = (s.ph() * s.pw()) as f64;
                let mut norm = [0.0f64; NCH];
                for ch in 0..NCH {
                    let c = sp[ch].get(s, py0, px0);
                    let mut tot_e = 0.0f64;
                    for v in 0..s.h() {
                        for u in 0..s.w() {
                            if v < s.ph() && u < s.pw() {
                                continue;
                            }
                            let cv = c[v * 16 + u] as f64;
                            tot_e += cv * cv;
                        }
                    }
                    norm[ch] = tot_e / n_parents;
                }
                for py in 0..s.ph() {
                    for px in 0..s.pw() {
                        let p = py * s.pw() + px;
                        let mut usrc = [[0.0f32; 64]; NCH];
                        let mut usteps = [[1.0f32; 64]; NCH];
                        for ch in 0..NCH {
                            let c = sp[ch].get(s, py0, px0);
                            let st = &enc.steps[ch][si];
                            for z in 1..64 {
                                let gidx = gather[p * 64 + z] as usize;
                                usrc[ch][z] = c[gidx];
                                usteps[ch][z] = st[gidx];
                            }
                        }
                        let slot = (py0 + py) * sbw + (gx + px0 + px);
                        idx[slot] = steps.len();
                        src[slot] = Some(usrc);
                        try_push(&mut steps, usteps)?;
                        try_push(&mut norms, norm)?;
                    }
                }
            }
        }
        Ok(Self {
            src,
            steps,
            norms,
            idx,
        })
    }
}

/// Rewrites one unmerged 8x8 or 4x4-split block under `pass` and emits its
/// split-Huffman frequencies when `freqs` is active.
#[allow(clippy::too_many_arguments)]
fn rewrite_unmerged_block(
    pass: &Pass,
    args: &StripeRewrite<'_>,
    sc: &mut StripeCoefs,
    b: usize,
    by: usize,
    bx: usize,
    row_planes: &[f32],
    row_coefs: &[f32],
    coefs4: &mut [f32],
    cands: &mut Cands,
    freqs: &mut Option<&mut Freqs>,
) {
    let split = sc.splits.get(b).copied().unwrap_or(0);
    let local_steps;
    let steps = if split == 0 {
        args.steps
    } else {
        for ch in 0..NCH {
            if split >> ch & 1 == 0 {
                continue;
            }
            let px: [f32; 64] = core::array::from_fn(|i| {
                row_planes[ch * 8 * args.pw + (i / 8) * args.pw + bx * 8 + i % 8]
            });
            coefs4[ch * 64..][..64].copy_from_slice(&super::small::forward_cfg(args.cfg, &px));
        }
        local_steps = core::array::from_fn(|ch| {
            if split >> ch & 1 != 0 {
                super::small::steps4(&args.steps[ch])
            } else {
                args.steps[ch]
            }
        });
        &local_steps
    };
    // 4x4 planes reconstruct without the bias (B = 0).
    let local_bias;
    let bias = match args.bias {
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
    let mut coupled = *args.set;
    let (set, tpw) = args.block_settings(pass, by, bx, &mut coupled);
    let at = |ch: usize| -> &[f32] {
        if split >> ch & 1 != 0 {
            &coefs4[ch * 64..][..64]
        } else {
            &row_coefs[(ch * args.image_bw + bx) * 64..][..64]
        }
    };
    let lmul = args.lambda_at(by, bx);
    let hw = dispatch_block_norm(
        sc,
        b,
        at,
        steps,
        &pass.rates,
        set,
        &pass.lab,
        tpw,
        None,
        lmul,
        bias,
        cands,
        pass.cspw.as_ref(),
    );
    if let Some(f) = freqs {
        emit_huf(f, sc, b, hw.as_ref());
    }
}

/// Rewrites one stripe's units (its column groups in order) with `pass`:
/// recomputes each block row through `args.rows`, then re-decides every block's
/// AC coefficients. `freqs` gathers the split-Huffman frequencies the next
/// pass prices from. `args.pass_index` is the pass number (0: debug source checks).
///
/// `args.lambda_map` holds per-block trellis-lambda multipliers starting at
/// block row `args.lambda_by0` (whole-image map uses `lambda_by0 = 0`; bounded
/// streaming passes pass the current band's map with `lambda_by0 = band_by0`).
pub(in crate::lossy) fn rewrite_stripe_lambda(
    pass: &Pass,
    stripe: &mut [StripeCoefs],
    args: &StripeRewrite<'_>,
    mut freqs: Option<&mut Freqs>,
    stop: &dyn how_far::Stop,
) -> Result<(), Error> {
    stop.check().map_err(|_| Error::Cancelled)?;
    let cm = dct::dct_matrix();
    let (by0, nby) = (stripe[0].by0, stripe[0].nby);
    let mut extremes = try_vec_cap(args.image_bw)?;
    let any_split = stripe.iter().any(|sc| sc.splits.iter().any(|&v| v != 0));
    let any_merged =
        args.merged_trellis && stripe.iter().any(|sc| sc.modes.iter().any(|&m| m != 0));
    let plane_len = NCH
        .checked_mul(8)
        .and_then(|v| v.checked_mul(args.pw))
        .ok_or(Error::TooLarge)?;
    let coef_len = NCH
        .checked_mul(args.image_bw)
        .and_then(|v| v.checked_mul(64))
        .ok_or(Error::TooLarge)?;
    let mut coefs4 = try_vec(if any_split { NCH * 64 } else { 0 }, 0.0f32)?;
    let mut cands: alloc::boxed::Box<Cands> = alloc::boxed::Box::default();

    let fdct_into =
        |by: usize, planes: &mut [f32], coefs: &mut [f32], ext: &mut Vec<(u64, bool)>| {
            ext.clear();
            (args.rows)(by, planes, ext);
            for ch in 0..NCH {
                enc_simd::fdct_row(
                    &planes[ch * 8 * args.pw..][..8 * args.pw],
                    args.pw,
                    args.image_bw,
                    &cm,
                    &mut coefs[ch * args.image_bw * 64..][..args.image_bw * 64],
                );
            }
        };

    if any_merged {
        let enc = super::superblock::select::Enc::new(args.steps);
        let mut planes2 = [try_vec(plane_len, 0.0f32)?, try_vec(plane_len, 0.0f32)?];
        let mut coefs2 = [try_vec(coef_len, 0.0f32)?, try_vec(coef_len, 0.0f32)?];
        let mut sp = super::superblock::select::new_spectra();
        for r0 in (0..nby).step_by(2) {
            stop.check().map_err(|_| Error::Cancelled)?;
            let rr = if r0 + 1 < nby { 2 } else { 1 };
            for dy in 0..rr {
                fdct_into(
                    by0 + r0 + dy,
                    &mut planes2[dy],
                    &mut coefs2[dy],
                    &mut extremes,
                );
            }
            for (g, sc) in stripe.iter_mut().enumerate() {
                let bx0 = g * args.group_bw;
                let sbw = sc.bw;
                let merged =
                    MergedStripUnits::build(sc, &enc, &planes2, args.pw, r0, rr, bx0, &mut sp)?;
                for dy in 0..rr {
                    let r = r0 + dy;
                    if cfg!(debug_assertions) && args.pass_index == 0 {
                        check_sources(sc, r, &coefs2[dy], args.image_bw, bx0, args.steps, args.cfg);
                    }
                    for x in 0..sbw {
                        stop.check().map_err(|_| Error::Cancelled)?;
                        let b = r * sbw + x;
                        let slot = dy * sbw + x;
                        if let Some(usrc) = &merged.src[slot] {
                            let mi = merged.idx[slot];
                            let mut coupled = *args.set;
                            let (set, tpw) =
                                args.block_settings(pass, by0 + r, bx0 + x, &mut coupled);
                            let lmul = args.lambda_at(by0 + r, bx0 + x);
                            let at = |ch: usize| -> &[f32] { &usrc[ch] };
                            let hw = dispatch_block_norm(
                                sc,
                                b,
                                at,
                                &merged.steps[mi],
                                &pass.rates,
                                set,
                                &pass.lab,
                                tpw,
                                Some(&merged.norms[mi]),
                                lmul,
                                None,
                                &mut cands,
                                pass.cspw.as_ref(),
                            );
                            if let Some(f) = &mut freqs {
                                emit_huf(f, sc, b, hw.as_ref());
                            }
                            continue;
                        }
                        if sc.modes.get(b).is_some_and(|&m| m != 0) {
                            continue;
                        }
                        rewrite_unmerged_block(
                            pass,
                            args,
                            sc,
                            b,
                            by0 + r,
                            bx0 + x,
                            &planes2[dy],
                            &coefs2[dy],
                            &mut coefs4,
                            &mut cands,
                            &mut freqs,
                        );
                    }
                }
            }
        }
        return stop.check().map_err(|_| Error::Cancelled);
    }

    let mut planes = try_vec(plane_len, 0.0f32)?;
    let mut coefs = try_vec(coef_len, 0.0f32)?;
    for r in 0..nby {
        stop.check().map_err(|_| Error::Cancelled)?;
        fdct_into(by0 + r, &mut planes, &mut coefs, &mut extremes);
        for (g, sc) in stripe.iter_mut().enumerate() {
            let bx0 = g * args.group_bw;
            if cfg!(debug_assertions) && args.pass_index == 0 {
                check_sources(sc, r, &coefs, args.image_bw, bx0, args.steps, args.cfg);
            }
            for x in 0..sc.bw {
                stop.check().map_err(|_| Error::Cancelled)?;
                let b = r * sc.bw + x;
                // Merged parents keep the selector's quantization.
                if sc.modes.get(b).is_some_and(|&m| m != 0) {
                    continue;
                }
                rewrite_unmerged_block(
                    pass,
                    args,
                    sc,
                    b,
                    by0 + r,
                    bx0 + x,
                    &planes,
                    &coefs,
                    &mut coefs4,
                    &mut cands,
                    &mut freqs,
                );
            }
        }
    }
    stop.check().map_err(|_| Error::Cancelled)
}

pub(in crate::lossy) fn optimize(
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
    open_lambda_map: &[f32],
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
    let plan = effort.lossy_plan(super::adaptive_mode(cfg));
    let bh = units.iter().map(|sc| sc.by0 + sc.nby).max().unwrap_or(0);
    let nb = image_bw.checked_mul(bh).ok_or(Error::TooLarge)?;
    let mut lambda_map: Vec<f32> = if !open_lambda_map.is_empty() {
        let mut v = super::adaptive_rdo::try_vec_cap(open_lambda_map.len())?;
        v.extend_from_slice(open_lambda_map);
        v
    } else if plan.lambda_aq_open != 0.0 {
        super::adaptive_rdo::build_open_loop_lambda_map(
            rows,
            pw,
            image_bw,
            bh,
            steps,
            plan.lambda_aq_open,
            cfg.threads,
            stop,
        )?
    } else if plan.lambda_aq_closed_loop != 0.0 {
        super::adaptive_rdo::try_vec(nb, 1.0f32)?
    } else {
        Vec::new()
    };
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
        let lmap_ref: &[f32] = &lambda_map;
        let merged_trellis = plan.merged_trellis;
        let done = crate::par::par_map(stripes, cfg.threads, |_, mut stripe| {
            let mut freqs = emit.then(freqs_new);
            let args = StripeRewrite {
                rows,
                pw,
                image_bw,
                group_bw,
                steps,
                cfg,
                set,
                aq,
                lambda_map: lmap_ref,
                lambda_by0: 0,
                merged_trellis,
                bias,
                pass_index: index as usize,
            };
            rewrite_stripe_lambda(&pass, &mut stripe, &args, freqs.as_mut(), stop)?;
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
        // Closed-loop perceptual + unfiltered seam residual feedback updates
        // per-block trellis lambda before the final pass.
        if plan.lambda_aq_closed_loop != 0.0 && index + 1 < set.passes {
            super::adaptive_rdo::update_lambda_map_closed_loop(
                &mut lambda_map,
                &units,
                rows,
                pw,
                image_bw,
                bh,
                group_bw,
                steps,
                plan.lambda_aq_closed_loop,
                cfg.threads,
                stop,
            )?;
        }
    }
    stop.check().map_err(|_| Error::Cancelled)?;
    Ok(units)
}

/// Lab: [tokens, 4-position groups before the EOBs, coded block channels]
/// of coding units (the counts the decode-speed tiers model; column groups
/// do not change them).
#[cfg(all(feature = "lab", feature = "std"))]
pub(in crate::lossy) fn unit_stats(units: &[StripeCoefs], threads: usize) -> [u64; 3] {
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
pub(in crate::lossy) fn aq_strengths<F>(
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
