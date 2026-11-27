//! Encoder-only work budgets. Effort never changes reconstruction math;
//! lossless effort never changes samples, lossy effort may change which
//! coefficients are coded (the AC trellis on XYB run-level and split-Huffman
//! coding, from [`TRELLIS_ONE_PASS`]) and how many entropy tables carry them
//! (counts chosen by coded size, from [`TABLE_COUNT_SEARCH`]).

use crate::Error;

/// Lossy effort from which the encoder runs the AC trellis with one
/// rate-model pass. benchmarks/trellis_effort_2026-10-08: one pass costs
/// 2.1-4.3x the effort-7.5 encode (so not at the default) for 3.1% at
/// CVVDP 8 / 3.9% at SSIMULACRA2 10-50 on the imazen-26 test split.
#[cfg(feature = "encode")]
pub(crate) const TRELLIS_ONE_PASS: f32 = 9.0;
/// Lossy effort from which adaptive-partition encodes (`adaptive_partitions`
/// or `DecodeSpeed::Compact`) run the fast two-pass empirical pilot and
/// closed-loop perceptual seam feedback with two trellis passes, without the
/// slower per-candidate `trellis_select` search enabled at [`TRELLIS_TWO_PASSES`].
#[cfg(feature = "encode")]
pub(crate) const ADAPTIVE_PILOT: f32 = 9.5;
/// Lossy effort from which the trellis runs two rate-model passes
/// (3.3-7.6x the effort-7.5 encode; 3.6% / 4.6%), and adaptive-partition
/// encodes add per-candidate `trellis_select`.
#[cfg(feature = "encode")]
pub(crate) const TRELLIS_TWO_PASSES: f32 = 10.0;
/// Default 4x4 split distortion weight below trellis efforts (`SMALL_KAPPA`).
#[cfg(feature = "encode")]
pub(crate) const SMALL_KAPPA_DEFAULT: f32 = 1.5;
/// Balanced 4x4 split distortion weight used by adaptive partitions at trellis
/// efforts: reduces low/mid-quality photo over-splitting while preserving
/// crisp screen/text edges.
#[cfg(feature = "encode")]
pub(crate) const SMALL_KAPPA_ADAPTIVE_TRELLIS: f32 = 1.2;

/// Adaptive-partition mode of a lossy encode.
#[cfg(feature = "encode")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AdaptiveMode {
    /// Default / `Fast` / `Fastest` / `Balanced`: fixed 8x8 blocks.
    Off,
    /// `DecodeSpeed::Compact`: retains classic fast superblocks below
    /// [`TRELLIS_ONE_PASS`] (`9.0`) and enables the full adaptive-partition RDO
    /// ladder (`9.0`, `9.5`, `10.0`) from `9.0` onward.
    Compact,
    /// Explicit `adaptive_partitions = true`: enables 9-context partition-map
    /// DP (`map_dp`) even below `9.0`, plus the full adaptive-partition RDO
    /// ladder from `9.0` onward.
    Explicit,
}

/// Centralized lossy encoder search policy derived once from [`Effort`] and
/// [`AdaptiveMode`].
///
/// Every lossy stage (`lossy/encode/`, `lossy/superblock/select/`,
/// `lossy/trellis/`, `lossy/adaptive_rdo/`, `lossy/bounded/`) consumes this
/// plan directly instead of duplicating effort threshold checks.
#[cfg(feature = "encode")]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LossyEffortPlan {
    /// AC trellis rate-model passes (0, 1, or 2).
    pub trellis_passes: u8,
    /// Whether lossy entropy-table counts are searched by coded size (`>= 7.5`).
    pub table_count_search: bool,
    /// Run the AC trellis on merged superblock parent units using the merged
    /// block's whole-block mean AC energy norm.
    pub merged_trellis: bool,
    /// Trellis-price both solo and merged candidates during layout selection
    /// when the AC trellis is active.
    pub trellis_select: bool,
    /// Exact 8-state row-strip Viterbi DP coupling the 9-context partition-map
    /// model across each column group row, with exact unit-level V4 partition
    /// map merge pruning.
    pub map_dp: bool,
    /// Two-pass pilot -> freeze learned post-trellis token prices, empirical
    /// 9-context partition-map costs, and empirical 4x4 flag probabilities.
    pub two_pass_pilot: bool,
    /// Zero-decode-overhead open-loop surrounding-block perceptual
    /// trellis-lambda AQ strength (`0.0` = off).
    pub lambda_aq_open: f32,
    /// Closed-loop residual & unfiltered-seam feedback weight for
    /// trellis-lambda AQ (`0.0` = off).
    pub lambda_aq_closed_loop: f32,
    /// Per-DC-stripe 3-state row-Viterbi 2D-MED DC rate-distortion
    /// optimization (encoder-only, zero wire/decode overhead).
    pub dc_rdo: bool,
    /// Base 4x4 split distortion weight.
    pub small_kappa: f32,
}

#[cfg(feature = "encode")]
impl Default for LossyEffortPlan {
    fn default() -> Self {
        Self {
            trellis_passes: 0,
            table_count_search: false,
            merged_trellis: false,
            trellis_select: false,
            map_dp: false,
            two_pass_pilot: false,
            lambda_aq_open: 0.0,
            lambda_aq_closed_loop: 0.0,
            dc_rdo: false,
            small_kappa: SMALL_KAPPA_DEFAULT,
        }
    }
}
/// Lossy effort from which the encoder chooses its entropy-table counts by
/// coded size where the tables weigh (run-level EOB/AC and split-Huffman
/// count/width tables; `lossy/coef.rs` `KSearch`) instead of always filling
/// the effort's budget. benchmarks/coefgap_2026-10-09: 3.0% (effort 7.5) /
/// 3.5% (effort 10) at mean(CVVDP 8, SSIMULACRA2 10-50) on imazen-26 VALIDATE
/// (3.1% / 3.4% on the test split), almost all below 256 px; searched encodes
/// take 1.02-1.5x the encode time of an unsearched encode at <= 256 px,
/// ~1.00x at 1024 px (benchmarks/kcount_encode_2026-10-09).
pub(crate) const TABLE_COUNT_SEARCH: f32 = 7.5;

/// Validated once at the encoder boundary, before allocating image scratch.
#[derive(Clone, Copy)]
pub(crate) struct Effort(f32);

impl Effort {
    pub(crate) fn new(value: f32) -> Result<Self, Error> {
        if !value.is_finite() || !(0.0..=10.0).contains(&value) {
            return Err(Error::InvalidInput("effort must be finite and in 0..=10"));
        }
        Ok(Self(value))
    }

    /// One candidate at effort zero, the full budget at ten. Discrete work
    /// has plateaus; fractional effort can cross boundaries between integers.
    pub(crate) fn budget(self, maximum: usize) -> usize {
        let maximum = maximum.max(1);
        1 + ((maximum - 1) as f32 * (self.0 / 10.0)) as usize
    }

    /// Lossy trellis rate-model passes at this effort (`lossy/trellis.rs`):
    /// none below [`TRELLIS_ONE_PASS`], one up to [`TRELLIS_TWO_PASSES`], two
    /// from there.
    #[cfg(feature = "encode")]
    pub(crate) fn trellis_passes(self) -> u8 {
        if self.0 >= TRELLIS_TWO_PASSES {
            2
        } else if self.0 >= TRELLIS_ONE_PASS {
            1
        } else {
            0
        }
    }

    /// Centralized lossy encoder search policy for this effort and
    /// [`AdaptiveMode`].
    ///
    /// Monotonic lossy ladder for fixed-8x8 encodes ([`AdaptiveMode::Off`]):
    /// - `< 9.0` (incl. default `7.5`): `trellis_passes = 0`; pixels unchanged.
    /// - `9.0..<10.0`: 1-pass AC trellis + open-loop lambda AQ + 2D-MED DC RDO.
    /// - `10.0`: 2-pass AC trellis + open-loop & closed-loop seam lambda AQ +
    ///   2D-MED DC RDO.
    ///
    /// Monotonic lossy ladder for adaptive encodes:
    /// - `< 9.0` (incl. default `7.5`): `trellis_passes = 0`; `map_dp` on for
    ///   [`AdaptiveMode::Explicit`], classic fast superblocks for
    ///   [`AdaptiveMode::Compact`].
    /// - `9.0..<9.5`: 1-pass merged AC trellis + `map_dp` + open-loop lambda AQ
    ///   + 2D-MED DC RDO.
    /// - `9.5..<10.0`: 2-pass merged AC trellis + two-pass empirical pilot +
    ///   open-loop & closed-loop seam lambda AQ + 2D-MED DC RDO (fast mid-tier
    ///   Pareto rung without per-candidate `trellis_select`).
    /// - `10.0`: adds per-candidate `trellis_select` (maximum compression).
    #[cfg(feature = "encode")]
    pub(crate) fn lossy_plan(self, mode: AdaptiveMode) -> LossyEffortPlan {
        let table_count_search = self.table_count_search();
        if mode == AdaptiveMode::Off {
            let trellis_passes = self.trellis_passes();
            let (lambda_aq_open, lambda_aq_closed_loop, dc_rdo) = if trellis_passes >= 2 {
                (0.30, 0.18, true)
            } else if trellis_passes >= 1 {
                (0.22, 0.0, true)
            } else {
                (0.0, 0.0, false)
            };
            return LossyEffortPlan {
                trellis_passes,
                table_count_search,
                lambda_aq_open,
                lambda_aq_closed_loop,
                dc_rdo,
                ..LossyEffortPlan::default()
            };
        }
        if self.0 >= TRELLIS_TWO_PASSES {
            LossyEffortPlan {
                trellis_passes: 2,
                table_count_search,
                merged_trellis: true,
                trellis_select: true,
                map_dp: true,
                two_pass_pilot: true,
                lambda_aq_open: 0.30,
                lambda_aq_closed_loop: 0.18,
                dc_rdo: true,
                small_kappa: SMALL_KAPPA_ADAPTIVE_TRELLIS,
            }
        } else if self.0 >= ADAPTIVE_PILOT {
            LossyEffortPlan {
                trellis_passes: 2,
                table_count_search,
                merged_trellis: true,
                trellis_select: false,
                map_dp: true,
                two_pass_pilot: true,
                lambda_aq_open: 0.30,
                lambda_aq_closed_loop: 0.18,
                dc_rdo: true,
                small_kappa: SMALL_KAPPA_ADAPTIVE_TRELLIS,
            }
        } else if self.0 >= TRELLIS_ONE_PASS {
            LossyEffortPlan {
                trellis_passes: 1,
                table_count_search,
                merged_trellis: true,
                trellis_select: false,
                map_dp: true,
                two_pass_pilot: false,
                lambda_aq_open: 0.30,
                lambda_aq_closed_loop: 0.0,
                dc_rdo: true,
                small_kappa: SMALL_KAPPA_ADAPTIVE_TRELLIS,
            }
        } else {
            LossyEffortPlan {
                trellis_passes: 0,
                table_count_search,
                merged_trellis: false,
                trellis_select: false,
                map_dp: mode == AdaptiveMode::Explicit,
                two_pass_pilot: false,
                lambda_aq_open: 0.0,
                lambda_aq_closed_loop: 0.0,
                dc_rdo: false,
                small_kappa: SMALL_KAPPA_DEFAULT,
            }
        }
    }

    /// Whether lossy entropy-table counts are chosen by coded size (from
    /// [`TABLE_COUNT_SEARCH`]).
    pub(crate) fn table_count_search(self) -> bool {
        self.0 >= TABLE_COUNT_SEARCH
    }

    /// Entropy-table clustering runs once per encode over whole-image
    /// histograms, so it is not throttled below effort 5: lower efforts search
    /// as effort 5 does (2026-10-06 study: effort 0 otherwise gave 8-13%
    /// larger files for 0-7% less encode time; docs/EFFORT_REPAIR.md).
    pub(crate) fn table_search(self) -> Self {
        Self(self.0.max(5.0))
    }

    /// Low budgets retain useful spatial predictors before spending work on
    /// averages. Effort >= 5 keeps the previously qualified candidate sets.
    #[cfg(feature = "encode")]
    pub(crate) fn predictors(self, allowed: u8) -> u8 {
        self.candidates(allowed & 0x7f, &[0, 3, 1, 2, 4, 5, 6])
    }

    /// Effort threshold from which lossless encoding considers per-lane adaptive
    /// pixel color transform (`decor = 4`, bit 4 of `decor_mask`).
    #[cfg(feature = "encode")]
    pub(crate) const ADAPTIVE_COLOR_EFFORT: f32 = 7.0;

    /// Residual R-G/B-G decorrelation is a better cheap RGB default than none.
    /// Bit 4 (`0x10`) enables per-lane adaptive pixel color transform from
    /// [`Self::ADAPTIVE_COLOR_EFFORT`] onward (or whenever no fixed mode is allowed).
    /// The analyzer still handles ineligible layouts and explicit caller masks.
    #[cfg(feature = "encode")]
    pub(crate) fn decorrelations(self, allowed: u8) -> u8 {
        let fixed_allowed = allowed & 0x0f;
        let adapt_allowed = allowed & (1 << crate::adapt_color::DECOR_ADAPT_PIX);
        let fixed = if fixed_allowed != 0 {
            self.candidates(fixed_allowed, &[1, 0, 2, 3])
        } else {
            0
        };
        let adapt = if adapt_allowed != 0 && (self.0 >= Self::ADAPTIVE_COLOR_EFFORT || fixed == 0) {
            adapt_allowed
        } else {
            0
        };
        fixed | adapt
    }

    #[cfg(feature = "encode")]
    fn candidates(self, allowed: u8, low_order: &[u8]) -> u8 {
        let mut count = self.budget(allowed.count_ones() as usize);
        let mut selected = 0;
        if self.0 < 5.0 {
            for &candidate in low_order {
                let bit = 1 << candidate;
                if allowed & bit != 0 {
                    selected |= bit;
                    count -= 1;
                    if count == 0 {
                        break;
                    }
                }
            }
            return selected;
        }
        let mut remaining = allowed;
        for _ in 0..count {
            let bit = remaining & remaining.wrapping_neg();
            selected |= bit;
            remaining &= !bit;
        }
        selected
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    #[test]
    fn lossy_plan_is_monotonic_across_rungs() {
        let rungs = [0.0f32, 2.5, 5.0, 7.5, 8.99, 9.0, 9.49, 9.5, 9.99, 10.0];
        for mode in [
            AdaptiveMode::Off,
            AdaptiveMode::Compact,
            AdaptiveMode::Explicit,
        ] {
            let mut prev = Effort::new(rungs[0]).unwrap().lossy_plan(mode);
            for &e in &rungs[1..] {
                let cur = Effort::new(e).unwrap().lossy_plan(mode);
                assert!(cur.trellis_passes >= prev.trellis_passes, "{mode:?} e={e}");
                assert!(
                    cur.table_count_search >= prev.table_count_search,
                    "{mode:?} e={e}"
                );
                assert!(cur.merged_trellis >= prev.merged_trellis, "{mode:?} e={e}");
                assert!(cur.trellis_select >= prev.trellis_select, "{mode:?} e={e}");
                assert!(cur.map_dp >= prev.map_dp, "{mode:?} e={e}");
                assert!(cur.two_pass_pilot >= prev.two_pass_pilot, "{mode:?} e={e}");
                assert!(cur.lambda_aq_open >= prev.lambda_aq_open, "{mode:?} e={e}");
                assert!(
                    cur.lambda_aq_closed_loop >= prev.lambda_aq_closed_loop,
                    "{mode:?} e={e}"
                );
                prev = cur;
            }
        }
        // Compact preserves fast classic superblocks below 9.0 and matches Explicit from 9.0+.
        let c75 = Effort::new(7.5).unwrap().lossy_plan(AdaptiveMode::Compact);
        assert_eq!(c75.trellis_passes, 0);
        assert!(!c75.map_dp && !c75.merged_trellis && !c75.two_pass_pilot);
        for e in [9.0f32, 9.5, 10.0] {
            assert_eq!(
                Effort::new(e).unwrap().lossy_plan(AdaptiveMode::Compact),
                Effort::new(e).unwrap().lossy_plan(AdaptiveMode::Explicit),
                "Compact and Explicit must match at e={e}"
            );
        }
        // Adaptive ladder separates fast pilot (9.5) from full trellis_select (10.0).
        let p75 = Effort::new(7.5).unwrap().lossy_plan(AdaptiveMode::Explicit);
        let p90 = Effort::new(9.0).unwrap().lossy_plan(AdaptiveMode::Explicit);
        let p95 = Effort::new(9.5).unwrap().lossy_plan(AdaptiveMode::Explicit);
        let p10 = Effort::new(10.0)
            .unwrap()
            .lossy_plan(AdaptiveMode::Explicit);
        assert_eq!(p75.trellis_passes, 0);
        assert!(p75.map_dp && !p75.merged_trellis && !p75.two_pass_pilot);
        assert_eq!(p90.trellis_passes, 1);
        assert!(p90.merged_trellis && !p90.two_pass_pilot && p90.lambda_aq_open > 0.0);
        assert_eq!(p95.trellis_passes, 2);
        assert!(p95.two_pass_pilot && p95.lambda_aq_closed_loop > 0.0 && !p95.trellis_select);
        assert_eq!(p10.trellis_passes, 2);
        assert!(p10.two_pass_pilot && p10.trellis_select);
        // Plain ladder stays strictly invariant with historical thresholds.
        assert_eq!(
            Effort::new(9.5)
                .unwrap()
                .lossy_plan(AdaptiveMode::Off)
                .trellis_passes,
            1
        );
        assert_eq!(
            Effort::new(10.0)
                .unwrap()
                .lossy_plan(AdaptiveMode::Off)
                .trellis_passes,
            2
        );
    }
}
