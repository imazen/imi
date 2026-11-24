//! Transfer functions for XYB coding of unsigned integer images.
//!
//! The encoder maps code values to linear light (1.0 = SDR reference white)
//! with these functions. The decoder maps linear light back to unrounded
//! 16-bit code units through a table over the f32 bit pattern with linear
//! interpolation; the table is normative and is generated from the
//! functions below, which use only IEEE f64 arithmetic (series for ln/exp,
//! a correctly rounded software sqrt), so every platform builds the same
//! table. U32 output normalizes those units and rounds directly to its declared
//! grid in f64; the fixed table does not provide 32-bit color accuracy.

use alloc::vec::Vec;

/// Transfers supported for integer XYB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Transfer {
    Srgb,
    Pq,
    Hlg,
}

impl Transfer {
    /// From the CICP transfer characteristics.
    pub(crate) fn from_cicp(tc: u8) -> Option<Transfer> {
        match tc {
            13 => Some(Transfer::Srgb),
            16 => Some(Transfer::Pq),
            18 => Some(Transfer::Hlg),
            _ => None,
        }
    }

    /// Largest linear value a code value maps to.
    fn linear_max(self) -> f64 {
        match self {
            Transfer::Srgb => 1.0,
            Transfer::Pq => 10000.0 / PQ_WHITE_NITS,
            Transfer::Hlg => HLG_SCALE,
        }
    }

    /// Code value in [0, 1] -> linear light (1.0 = reference white).
    #[cfg(feature = "encode")]
    pub(crate) fn decode(self, c: f64) -> f64 {
        let c = c.clamp(0.0, 1.0);
        match self {
            Transfer::Srgb => {
                if c <= 0.04045 {
                    c / 12.92
                } else {
                    pow((c + 0.055) / 1.055, 2.4)
                }
            }
            Transfer::Pq => {
                let p = pow(c, 1.0 / PQ_M2);
                let num = (p - PQ_C1).max(0.0);
                pow(num / (PQ_C2 - PQ_C3 * p), 1.0 / PQ_M1) * (10000.0 / PQ_WHITE_NITS)
            }
            Transfer::Hlg => {
                let e = if c <= 0.5 {
                    c * c / 3.0
                } else {
                    (exp((c - HLG_C) / HLG_A) + HLG_B) / 12.0
                };
                e * HLG_SCALE
            }
        }
    }

    /// Linear light -> code value in [0, 1].
    fn encode(self, l: f64) -> f64 {
        let l = l.clamp(0.0, self.linear_max());
        match self {
            Transfer::Srgb => {
                if l <= 0.003_130_8 {
                    l * 12.92
                } else {
                    1.055 * pow(l, 1.0 / 2.4) - 0.055
                }
            }
            Transfer::Pq => {
                let y = l * PQ_WHITE_NITS / 10000.0;
                if y <= 0.0 {
                    return pow(PQ_C1, PQ_M2);
                }
                let yp = pow(y, PQ_M1);
                pow((PQ_C1 + PQ_C2 * yp) / (1.0 + PQ_C3 * yp), PQ_M2)
            }
            Transfer::Hlg => {
                let e = l / HLG_SCALE;
                if e <= 1.0 / 12.0 {
                    magetypes::nostd_math::sqrt(3.0 * e)
                } else {
                    HLG_A * ln(12.0 * e - HLG_B) + HLG_C
                }
            }
        }
        .clamp(0.0, 1.0)
    }

    /// Normative decode table: entry i is the 16-bit code (unrounded, f32)
    /// of the linear value whose f32 bits are `(TABLE_BASE + i) << 16`.
    pub(crate) fn table(self) -> Vec<f32> {
        (0..=TABLE_N)
            .map(|i| {
                let x = f32::from_bits(((TABLE_BASE + i) as u32) << 16) as f64;
                (self.encode(x) * 65535.0) as f32
            })
            .collect()
    }

    /// Clamp bound applied before the table lookup.
    pub(crate) fn linear_max_f32(self) -> f32 {
        self.linear_max() as f32
    }
}

/// Table covers linear values from 2^-38 up to 2^6 with 128 segments per
/// octave (f32 bits >> 16 = exponent and 7 mantissa bits).
pub(crate) const TABLE_BASE: i32 = (127 - 38) << 7;
pub(crate) const TABLE_N: i32 = 44 << 7;

/// Normative linear -> 16-bit code value.
#[inline]
pub(crate) fn encode16(table: &[f32], lmax: f32, l: f32) -> u16 {
    encode_code(table, lmax, l, 65535.0)
}

/// Round once to the declared integer code grid. Full16 retains exactly the
/// historical table arithmetic; smaller grids scale the unrounded result.
#[inline]
pub(crate) fn encode_code(table: &[f32], lmax: f32, l: f32, max: f32) -> u16 {
    let v = unrounded_code(table, lmax, l);
    let v = if max == 65535.0 {
        v
    } else {
        v * (max / 65535.0)
    };
    super::color::rne_pos(v.clamp(0.0, max)) as u16
}

/// Shared normative interpolation before integer quantization. U32 output
/// scales this directly to its code grid, without rounding through U16.
#[inline]
pub(crate) fn unrounded_code(table: &[f32], lmax: f32, l: f32) -> f32 {
    let b = l.clamp(0.0, lmax).to_bits() as i32;
    let i = ((b >> 16) - TABLE_BASE).clamp(0, TABLE_N - 1) as usize;
    let frac = (b & 0xFFFF) as f32 * (1.0 / 65536.0);
    let t0 = table[i];
    let t1 = table[i + 1];
    t0 + (t1 - t0) * frac
}

/// Linear light of 1.0 for PQ, in nits: about the BT.2408 reference white
/// (203), chosen so 10000 nits (49.25) lies on a table grid point.
const PQ_WHITE_NITS: f64 = 10000.0 / 49.25;
const PQ_M1: f64 = 0.159_301_757_812_5;
const PQ_M2: f64 = 78.843_75;
const PQ_C1: f64 = 0.835_937_5;
const PQ_C2: f64 = 18.851_562_5;
const PQ_C3: f64 = 18.687_5;
const HLG_A: f64 = 0.178_832_77;
const HLG_B: f64 = 0.284_668_92;
const HLG_C: f64 = 0.559_910_73;
/// HLG scene light times this is linear light with reference white
/// (code 0.75) near 1.0; a table grid point (2 + 114/64) so the clamp at
/// full scale does not fall inside an interpolation segment.
const HLG_SCALE: f64 = 3.781_25;

const LN2: f64 = core::f64::consts::LN_2;

/// Natural log for x > 0 (pure f64 arithmetic, ~1e-16 relative).
pub(crate) fn ln(x: f64) -> f64 {
    if x <= 0.0 {
        return f64::NEG_INFINITY;
    }
    let bits = x.to_bits();
    let mut e = ((bits >> 52) & 0x7FF) as i64 - 1023;
    let mut m = f64::from_bits((bits & 0x000F_FFFF_FFFF_FFFF) | 0x3FF0_0000_0000_0000);
    if e == -1023 {
        // Subnormal input: normalize.
        let y = x * (1u64 << 54) as f64;
        return ln(y) - 54.0 * LN2;
    }
    if m > core::f64::consts::SQRT_2 {
        m *= 0.5;
        e += 1;
    }
    // ln(m) = 2 atanh(t), t = (m - 1) / (m + 1), |t| <= 0.1716.
    let t = (m - 1.0) / (m + 1.0);
    let t2 = t * t;
    let mut s = 0.0;
    let mut k = 25.0;
    while k > 1.0 {
        s = s * t2 + 1.0 / k;
        k -= 2.0;
    }
    let lnm = 2.0 * t * (s * t2 + 1.0);
    e as f64 * LN2 + lnm
}

/// e^x (pure f64 arithmetic, ~1e-16 relative) for |x| < 700.
pub(crate) fn exp(x: f64) -> f64 {
    if x < -700.0 {
        return 0.0;
    }
    if x > 700.0 {
        return f64::INFINITY;
    }
    // x = k ln2 + r, |r| <= ln2 / 2.
    let kf = x / LN2;
    let k = if kf >= 0.0 {
        (kf + 0.5) as i64
    } else {
        (kf - 0.5) as i64
    };
    let r = x - k as f64 * LN2;
    let mut s = 1.0;
    let mut n = 20.0;
    while n >= 1.0 {
        s = 1.0 + s * r / n;
        n -= 1.0;
    }
    s * f64::from_bits(((k + 1023) as u64) << 52)
}

/// x^y for x > 0.
pub(crate) fn pow(x: f64, y: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    exp(y * ln(x))
}

#[cfg(all(test, feature = "std", feature = "encode"))]
mod tests {
    use super::*;

    #[test]
    fn ln_exp_accuracy() {
        for i in 1..20000 {
            let x = i as f64 * 0.0137 + 1e-9;
            assert!(
                (ln(x) - x.ln()).abs() <= 1e-14 * x.ln().abs().max(1.0),
                "{x}"
            );
            let y = (i as f64 - 10000.0) * 0.01;
            assert!((exp(y) / y.exp() - 1.0).abs() < 1e-14, "{y}");
        }
    }

    #[test]
    fn transfers_roundtrip_and_tables_are_accurate() {
        for t in [Transfer::Srgb, Transfer::Pq, Transfer::Hlg] {
            let table = t.table();
            let lmax = t.linear_max_f32();
            let mut worst = 0i32;
            for c in (0..65536).step_by(7) {
                let l = t.decode(c as f64 / 65535.0);
                let back = (t.encode(l) * 65535.0).round() as i32;
                assert!((back - c).abs() <= 1, "{t:?} {c} -> {l} -> {back}");
                let d = encode16(&table, lmax, l as f32) as i32;
                worst = worst.max((d - c).abs());
            }
            assert!(worst <= 2, "{t:?}: table worst {worst} codes");
        }
    }
}
