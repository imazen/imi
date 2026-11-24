//! Deterministic float helpers usable without std (encoder cost estimates).

/// log2 for positive finite `x`, accurate to ~1e-4, identical in std and
/// no_std builds.
pub(crate) fn log2(x: f64) -> f64 {
    let bits = x.to_bits();
    let exp = ((bits >> 52) & 0x7FF) as i64 - 1023;
    // Mantissa in [1, 2).
    let m = f64::from_bits((bits & 0x000F_FFFF_FFFF_FFFF) | 0x3FF0_0000_0000_0000);
    // log2(m) via ln(m) series on t = (m - 1) / (m + 1).
    let t = (m - 1.0) / (m + 1.0);
    let t2 = t * t;
    let ln =
        2.0 * t * (1.0 + t2 * (1.0 / 3.0 + t2 * (1.0 / 5.0 + t2 * (1.0 / 7.0 + t2 * (1.0 / 9.0)))));
    exp as f64 + ln * core::f64::consts::LOG2_E
}

/// Round half up for non-negative values.
#[cfg(feature = "encode")]
pub(crate) fn round_pos(x: f64) -> f64 {
    let f = x as u64 as f64;
    if x - f >= 0.5 { f + 1.0 } else { f }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    #[test]
    fn log2_close() {
        for i in 1..20000u32 {
            let x = i as f64 * 0.37;
            assert!((super::log2(x) - x.log2()).abs() < 1e-4, "{x}");
        }
    }
}

/// The lab integer inverse requires IEEE ties-to-even before conversion.
/// Keep this extension absent from ordinary std production builds.
#[cfg(all(feature = "lab", any(test, not(feature = "std"))))]
pub(crate) trait RoundTiesEven {
    fn round_ties_even(self) -> Self;
}
#[cfg(all(feature = "lab", any(test, not(feature = "std"))))]
impl RoundTiesEven for f32 {
    fn round_ties_even(self) -> Self {
        libm::roundevenf(self)
    }
}
