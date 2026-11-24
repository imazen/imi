//! Spatial predictors.
//!
//! Neighbors: `w` (left), `n` (above), `nw`, `ne`. Edge rules:
//! * row 0 always uses predictor `W`; its first pixel predicts 0.
//! * at x == 0: `w = n`, `nw = n`.
//! * at x == width-1: `ne = n`.

#[cfg(all(test, feature = "encode"))]
#[path = "predict_parity.rs"]
mod parity;

pub(crate) const NUM_PREDICTORS: u8 = 7;

pub(crate) const P_W: u8 = 0;
pub(crate) const P_N: u8 = 1;
pub(crate) const P_AVG_WN: u8 = 2;
pub(crate) const P_MED: u8 = 3;
pub(crate) const P_GRAD: u8 = 4;
pub(crate) const P_PAETH: u8 = 5;
pub(crate) const P_AVG_WNE: u8 = 6;
/// Inter frames only (a reference plane from the previous frame): the
/// reference value, W plus the reference's horizontal step, N plus the
/// reference's vertical step.
pub(crate) const P_REF: u8 = 7;
pub(crate) const P_REF_W: u8 = 8;
pub(crate) const P_REF_N: u8 = 9;
/// Predictors valid in inter frames.
pub(crate) const NUM_PREDICTORS_INTER: u8 = 10;

#[inline(always)]
pub(crate) fn predict(p: u8, w: u32, n: u32, nw: u32, ne: u32, mask: u32) -> u32 {
    match p {
        P_W => w,
        P_N => n,
        P_AVG_WN => avg(w, n),
        P_MED => med(w, n, nw),
        P_GRAD => (w.wrapping_add(n).wrapping_sub(nw)) & mask,
        P_PAETH => paeth(w, n, nw),
        P_AVG_WNE => avg(w, ne),
        _ => unreachable!(),
    }
}

#[inline(always)]
pub(crate) fn med(w: u32, n: u32, nw: u32) -> u32 {
    let (lo, hi) = if w < n { (w, n) } else { (n, w) };
    if nw >= hi {
        lo
    } else if nw <= lo {
        hi
    } else {
        w.wrapping_add(n).wrapping_sub(nw)
    }
}

/// floor((a + b) / 2) without overflow.
#[inline(always)]
pub(crate) fn avg(a: u32, b: u32) -> u32 {
    (a >> 1) + (b >> 1) + (a & b & 1)
}

#[inline(always)]
pub(crate) fn paeth(a: u32, b: u32, c: u32) -> u32 {
    let p = a as i64 + b as i64 - c as i64;
    let pa = (p - a as i64).abs();
    let pb = (p - b as i64).abs();
    let pc = (p - c as i64).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

#[cfg(all(test, feature = "encode"))]
#[inline(always)]
pub(crate) fn zigzag(d: u32, bits: u32) -> u32 {
    // d is an n-bit modular value; interpret as signed.
    if bits == 0 {
        return 0;
    }
    let mask = (1u32 << bits) - 1;
    let sign = (d >> (bits - 1)) & 1;
    ((d << 1) ^ (0u32.wrapping_sub(sign))) & mask
}

#[inline(always)]
pub(crate) fn unzigzag(u: u32, mask: u32) -> u32 {
    ((u >> 1) ^ (0u32.wrapping_sub(u & 1))) & mask
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    #[test]
    fn zigzag_roundtrip() {
        for bits in 1..=16u32 {
            let mask = (1u32 << bits) - 1;
            let mut seen = alloc::vec![false; 1 << bits];
            for d in 0..=mask {
                let u = zigzag(d, bits);
                assert!(u <= mask);
                assert!(!seen[u as usize]);
                seen[u as usize] = true;
                assert_eq!(unzigzag(u, mask), d);
            }
            // Small magnitudes map to small codes.
            assert_eq!(zigzag(0, bits), 0);
            if bits > 1 {
                assert_eq!(zigzag(mask, bits), 1); // -1
                assert_eq!(zigzag(1, bits), 2);
            }
        }
    }
}
