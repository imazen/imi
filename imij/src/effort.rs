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
/// Lossy effort from which the trellis runs two rate-model passes
/// (3.3-7.6x the effort-7.5 encode; 3.6% / 4.6%).
#[cfg(feature = "encode")]
pub(crate) const TRELLIS_TWO_PASSES: f32 = 10.0;
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

    /// Residual R-G/B-G decorrelation is a better cheap RGB default than none.
    /// The analyzer still handles ineligible layouts and explicit caller masks.
    #[cfg(feature = "encode")]
    pub(crate) fn decorrelations(self, allowed: u8) -> u8 {
        self.candidates(allowed & 0x0f, &[1, 0, 2, 3])
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
