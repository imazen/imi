//! Error-bounded quantization of floating-point samples to integers: the
//! LERC-style recipe for float science data (docs/NEAR_LOSSLESS.md, "Error-
//! bounded floats"). Floats become integers on a grid of step
//! `2 max_error / (2 near + 1)`; coding the integers losslessly (`near` 0) or
//! near-lossless with bound `near` keeps every decoded value within
//! `max_error` of its source. Missing samples map to stored 0, below the
//! valid minimum. The quantization is described by a slope and an offset,
//! the calibration a `cALB` record carries; the samples themselves are plain
//! integers that any IMI decoder reads.

#[cfg(not(feature = "std"))]
use num_traits::Float as _;

use alloc::vec::Vec;

use crate::{Error, SampleType};

/// How one channel of float samples was quantized to integers, from
/// [`FloatQuantization::quantize`].
///
/// A decoded stored integer `v` stands for the value `slope * v + offset`,
/// computed as a binary64 multiply followed by a binary64 add (the `cALB`
/// calibration formula). Stored integers below `valid_min` are missing.
/// Coding the integers with `crate::LosslessTuning::with_max_error``(near)`
/// (or losslessly when `near` is 0) keeps every decoded value within the
/// requested maximum error of its source: the quantizer checks this for each
/// sample with that exact formula, for every integer a decoder may return.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct FloatQuantization {
    /// Value per stored step (the `cALB` slope).
    pub slope: f64,
    /// Value of stored 0 (the `cALB` offset).
    pub offset: f64,
    /// Stored integers below this are missing (the `cALB` valid minimum);
    /// `None` when no sample was missing.
    pub valid_min: Option<f64>,
    /// Near-lossless bound to code the integers with
    /// (`crate::LosslessTuning::with_max_error`); 0 codes them losslessly.
    pub near: u32,
    /// Sample type of the integers: `U16` when every stored value fits, else
    /// `U32` (Extended profile).
    pub sample: SampleType,
}

/// Safety margins on the grid step, tried in order: the step is shrunk by
/// this fraction so that binary64 rounding in `slope * v + offset` cannot
/// push a value past the bound.
const MARGINS: [f64; 3] = [
    1.0 / (1u64 << 26) as f64,
    1.0 / (1u64 << 16) as f64,
    1.0 / 256.0,
];

impl FloatQuantization {
    /// Quantizes one channel of samples (any order; NaN and samples equal to
    /// `nodata` are missing) so that every decoded value is within
    /// `max_error` of its source, when the integers are coded with
    /// [`Self::near`] (pass `near` 0 for lossless integer coding; a larger
    /// `near` uses a finer grid and near-lossless coding for the same bound).
    ///
    /// Returns the quantization and the integers as native-endian `U16` or
    /// `U32` samples, one per input value, ready for `crate::ImiEncoderConfig::encode_samples` as a
    /// Gray image of [`Self::sample`]. The slope is finite and positive and
    /// the offset finite in every successful result.
    ///
    /// Errors, all checked before anything is returned:
    /// * [`Error::InvalidInput`] when `max_error` is not finite and positive;
    ///   a sample is infinite (unless it equals `nodata`); the grid step
    ///   `max_error / (near + 1/2)` underflows to zero (a subnormal
    ///   `max_error` with a large `near`); the values span more than `2^32`
    ///   grid steps (including spans that overflow binary64); the offset
    ///   `min - step (2 near + 1)` that places missing samples below the valid
    ///   values is not finite; or binary64 rounding defeats the per-sample
    ///   check at every safety margin.
    /// * [`Error::TooLarge`] when the output buffer cannot be allocated.
    ///
    /// A `max_error` above `f64::MAX / 2` would make the step overflow; the
    /// step is then `f64::MAX` (a finer grid than needed, which only lowers
    /// the error), so such bounds work whenever the values allow it.
    pub fn quantize<T: Copy + Into<f64>>(
        values: &[T],
        nodata: Option<f64>,
        max_error: f64,
        near: u32,
    ) -> Result<(Self, Vec<u8>), Error> {
        if !(max_error.is_finite() && max_error > 0.0) {
            return Err(Error::InvalidInput("max_error must be finite and positive"));
        }
        let missing = |x: f64| x.is_nan() || nodata.is_some_and(|n| x == n);
        let (mut lo, mut hi, mut any_missing) = (f64::INFINITY, f64::NEG_INFINITY, false);
        for &v in values {
            let x: f64 = v.into();
            if missing(x) {
                any_missing = true;
            } else if x.is_infinite() {
                return Err(Error::InvalidInput("infinite sample"));
            } else {
                lo = lo.min(x);
                hi = hi.max(x);
            }
        }
        if lo > hi {
            // Nothing valid: every sample is missing.
            (lo, hi) = (0.0, 0.0);
        }
        // Missing samples are stored 0; valid ones start at base, so that a
        // near-lossless decoder's output (within `near`) stays on its side
        // of the valid minimum `near + 1`.
        let base = if any_missing { 2 * near as u64 + 1 } else { 0 };
        for margin in MARGINS {
            // 2 max_error / (2 near + 1), without forming 2 max_error (which
            // overflows above f64::MAX / 2); a step capped at f64::MAX is
            // finer than needed, which only lowers the error.
            let step = (max_error / (near as f64 + 0.5)).min(f64::MAX) * (1.0 - margin);
            if !step.is_finite() || step <= 0.0 {
                return Err(Error::InvalidInput(
                    "max_error too small: the grid step underflows to zero",
                ));
            }
            // An overflowing span gives an infinite count and fails here.
            let top = ((hi - lo) / step).round() + base as f64 + near as f64;
            if !top.is_finite() || top > u32::MAX as f64 {
                return Err(Error::InvalidInput(
                    "the values span more than 2^32 grid steps at this max_error",
                ));
            }
            let sample = if top <= u16::MAX as f64 {
                SampleType::U16
            } else {
                SampleType::U32
            };
            let offset = lo - step * base as f64;
            if !offset.is_finite() {
                return Err(Error::InvalidInput(
                    "max_error too large: the offset below the valid values overflows",
                ));
            }
            let q = Self {
                slope: step,
                offset,
                valid_min: any_missing.then_some(near as f64 + 1.0),
                near,
                sample,
            };
            if let Some(out) = q.fill(values, &missing, lo, base, max_error)? {
                return Ok((q, out));
            }
        }
        Err(Error::InvalidInput(
            "max_error too small for the values' magnitude (binary64 rounding)",
        ))
    }

    /// Stored integers for `values`, each checked against the bound for
    /// every integer a decoder may return; `None` if a sample fails.
    fn fill<T: Copy + Into<f64>>(
        &self,
        values: &[T],
        missing: &impl Fn(f64) -> bool,
        lo: f64,
        base: u64,
        max_error: f64,
    ) -> Result<Option<Vec<u8>>, Error> {
        let bytes = self.sample.bytes();
        let cap = if bytes == 2 {
            u16::MAX as i64
        } else {
            u32::MAX as i64
        };
        let len = values.len().checked_mul(bytes).ok_or(Error::TooLarge)?;
        let mut out = Vec::new();
        out.try_reserve_exact(len).map_err(|_| Error::TooLarge)?;
        let near = self.near as i64;
        // The decoded value is monotone in the stored integer, so the
        // extremes `v - near` and `v + near` bound the error. A non-finite
        // value fails the comparison.
        let ok = |v: i64, x: f64| {
            [v - near, v + near]
                .iter()
                .all(|&w| (self.slope * w as f64 + self.offset - x).abs() <= max_error)
        };
        for &v in values {
            let x: f64 = v.into();
            let s = if missing(x) {
                0
            } else {
                // At most the checked step count (< 2^32): no i64 overflow.
                let q = ((x - lo) / self.slope).round() as i64 + base as i64;
                let lowest = base as i64;
                let Some(s) = [q, q - 1, q + 1]
                    .into_iter()
                    .find(|&c| c >= lowest && c <= cap && ok(c, x))
                else {
                    return Ok(None);
                };
                s as u32
            };
            match bytes {
                2 => out.extend_from_slice(&(s as u16).to_ne_bytes()),
                _ => out.extend_from_slice(&s.to_ne_bytes()),
            }
        }
        Ok(Some(out))
    }

    /// The value a decoded stored integer stands for (`slope * v + offset`),
    /// or `None` when it is missing.
    pub fn value(&self, stored: u32) -> Option<f64> {
        if self.valid_min.is_some_and(|m| (stored as f64) < m) {
            return None;
        }
        Some(self.slope * stored as f64 + self.offset)
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn unit(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    fn stored(q: &FloatQuantization, out: &[u8], i: usize) -> u32 {
        match q.sample {
            SampleType::U16 => u16::from_ne_bytes([out[2 * i], out[2 * i + 1]]) as u32,
            _ => u32::from_ne_bytes(out[4 * i..4 * i + 4].try_into().unwrap()),
        }
    }

    /// Every integer a near-lossless decoder may return for each sample
    /// decodes within the bound (or as missing for missing samples).
    fn check(values: &[f64], nodata: Option<f64>, e: f64, near: u32) {
        let (q, out) = FloatQuantization::quantize(values, nodata, e, near).unwrap();
        let top = if q.sample == SampleType::U16 {
            u16::MAX as i64
        } else {
            u32::MAX as i64
        };
        for (i, &x) in values.iter().enumerate() {
            let s = stored(&q, &out, i) as i64;
            let miss = x.is_nan() || nodata == Some(x);
            for w in (s - near as i64).max(0)..=(s + near as i64).min(top) {
                match q.value(w as u32) {
                    None => assert!(miss, "x={x} s={s} w={w} e={e} near={near}"),
                    Some(y) => {
                        assert!(!miss, "missing x decodes as {y}");
                        assert!((y - x).abs() <= e, "x={x} y={y} e={e} near={near} {q:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn bound_holds_for_every_decodable_integer() {
        let mut r = Rng(0x5eed);
        for &(scale, shift) in &[
            (1.0, 0.0),
            (1e-3, 0.0),
            (300.0, 1e5),
            (1e4, -7e6),
            (2.0, 1e7),
        ] {
            for &e in &[1e-4, 0.01, 0.5, 7.5] {
                for near in [0u32, 1, 2, 5] {
                    let n = 600;
                    let mut v: Vec<f64> = (0..n)
                        .map(|i| shift + scale * ((i as f64 * 0.05).sin() + r.unit() * 0.3))
                        .collect();
                    // Too many steps for 2^32 at this bound: expected error.
                    let span = scale * 2.6 / (2.0 * e / (2.0 * near as f64 + 1.0));
                    if span > 4e9 {
                        assert!(FloatQuantization::quantize(&v, None, e, near).is_err());
                        continue;
                    }
                    check(&v, None, e, near);
                    for i in (0..n).step_by(17) {
                        v[i] = f64::NAN;
                    }
                    v[3] = -9999.0;
                    check(&v, Some(-9999.0), e, near);
                }
            }
        }
    }

    #[test]
    fn finite_extreme_bounds_preserve_calibration_and_error() {
        let tiny = f64::from_bits(1);
        let huge = f64::MAX;
        // These cases are representable, including the smallest positive
        // bound and bounds whose naive doubling overflows.
        for (v, e, near) in [
            (vec![1.0; 4], tiny, 0),
            (vec![0.0, tiny], tiny, 0),
            (vec![0.0, tiny, f64::NAN], tiny, 0),
            (vec![0.0, tiny], tiny, 1),
            (vec![-1.0, 0.0, 1e300], huge, 0),
            (vec![-1.0, 0.0, 1e300], huge, 1),
            (vec![-1.0, f64::NAN, 1e300], huge, 0),
            (vec![0.0, huge / 2.0], huge / 2.0, 0),
            (vec![f64::NAN; 3], tiny, 0),
        ] {
            let (q, _) = FloatQuantization::quantize(&v, None, e, near).unwrap();
            assert!(q.slope.is_finite() && q.slope > 0.0);
            assert!(q.offset.is_finite());
            check(&v, None, e, near);
        }
        // Unrepresentable steps, spans or missing-value offsets fail
        // explicitly; none may return an invalid successful calibration.
        for (v, e, near) in [
            (vec![0.0, tiny], tiny, 2),
            (vec![0.0, 1.0], tiny, 0),
            (vec![-1.0, f64::NAN, 1e300], huge, 3),
            (vec![-huge, huge], 1.0, 0),
            (vec![-huge, huge], huge, 0),
            (vec![f64::NAN; 3], huge, 4),
            (vec![1e308, -1e308], huge / 2.0, 0),
        ] {
            assert!(matches!(
                FloatQuantization::quantize(&v, None, e, near),
                Err(Error::InvalidInput(_))
            ));
        }
    }

    #[test]
    fn f32_inputs_extremes_and_errors() {
        let v: Vec<f32> = vec![0.0, -0.0, 1e-30, 3.5, f32::NAN, 1.0e6];
        let (q, out) = FloatQuantization::quantize(&v, None, 0.25, 1).unwrap();
        for (i, &x) in v.iter().enumerate() {
            match q.value(stored(&q, &out, i)) {
                None => assert!(x.is_nan()),
                Some(y) => assert!((y - x as f64).abs() <= 0.25),
            }
        }
        // All missing; a constant field.
        let (q, out) = FloatQuantization::quantize(&[f64::NAN; 4], None, 1.0, 2).unwrap();
        assert!((0..4).all(|i| q.value(stored(&q, &out, i)).is_none()));
        let (q, out) = FloatQuantization::quantize(&[42.0f64; 5], None, 1e-6, 0).unwrap();
        assert!((0..5).all(|i| q.value(stored(&q, &out, i)) == Some(42.0)));
        assert_eq!(q.sample, SampleType::U16);
        // Errors.
        for e in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(FloatQuantization::quantize(&[1.0f64], None, e, 0).is_err());
        }
        assert!(FloatQuantization::quantize(&[1.0, f64::INFINITY], None, 1.0, 0).is_err());
        assert!(FloatQuantization::quantize(&[0.0, 1e30], None, 1e-3, 0).is_err());
        // Wide but representable: U32.
        let (q, _) = FloatQuantization::quantize(&[0.0, 1e6], None, 1e-3, 0).unwrap();
        assert_eq!(q.sample, SampleType::U32);
    }
}
