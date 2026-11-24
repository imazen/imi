//! Residual and run tokenization (hybrid integer coding).
//!
//! A value `u` below `2^k` is its own token. Larger values are split into a
//! token carrying the position of the most significant bit plus the next
//! `m` bits, and `msb - m` raw extra bits. Run tokens follow the value
//! tokens in each slot's alphabet and code `run_len - 1` with the same
//! scheme (`rk` direct bits, `rm` msb bits), up to 2^24 pixels.

pub(crate) const MAX_RUN_BITS: u32 = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TokenCfg {
    /// Direct-token bits (values below 2^k are their own token).
    pub k: u32,
    /// Bits below the MSB carried in the token.
    pub m: u32,
    /// Sample bit depth (values are below 2^depth).
    pub depth: u32,
    pub rk: u32,
    pub rm: u32,
}

impl TokenCfg {
    #[cfg_attr(not(feature = "lab"), allow(dead_code))]
    pub(crate) fn new(k: u32, m: u32, depth: u32, rk: u32, rm: u32) -> Self {
        let k = k.max(m).min(depth);
        let rk = rk.max(rm);
        Self {
            k,
            m,
            depth,
            rk,
            rm,
        }
    }

    /// Format v1 scheme: values use (k clamped to depth, m); runs use
    /// (max(k, m), m) so run lengths always have a valid split.
    pub(crate) fn v1(k: u32, m: u32, depth: u32) -> Self {
        let kv = k.max(m).min(depth);
        Self {
            k: kv,
            m,
            depth,
            rk: kv.max(m),
            rm: m,
        }
    }

    /// Number of value tokens.
    pub(crate) fn value_tokens(&self) -> u32 {
        if self.k >= self.depth {
            1 << self.depth
        } else {
            (1 << self.k) + (self.depth - self.k) * (1 << self.m)
        }
    }

    pub(crate) fn run_tokens(&self) -> u32 {
        (1 << self.rk) + (MAX_RUN_BITS - self.rk) * (1 << self.rm)
    }

    pub(crate) fn alphabet(&self) -> u32 {
        self.value_tokens() + self.run_tokens()
    }

    /// (token, extra value, extra bit count)
    #[inline]
    #[cfg(feature = "encode")]
    pub(crate) fn value(&self, u: u32) -> (u32, u32, u32) {
        hybrid(u, self.k, self.m)
    }

    #[inline]
    pub(crate) fn run(&self, len: u32) -> (u32, u32, u32) {
        debug_assert!(len >= 1 && len - 1 < (1 << MAX_RUN_BITS));
        let (t, e, n) = hybrid(len - 1, self.rk, self.rm);
        (t + self.value_tokens(), e, n)
    }
}

#[inline]
pub(crate) fn hybrid(u: u32, k: u32, m: u32) -> (u32, u32, u32) {
    if u < (1 << k) {
        (u, 0, 0)
    } else {
        let msb = 31 - u.leading_zeros();
        let nextra = msb - m;
        let top = (u >> nextra) & ((1 << m) - 1);
        let tok = (1 << k) + (msb - k) * (1 << m) + top;
        (tok, u & ((1 << nextra) - 1), nextra)
    }
}

/// Inverse of [`hybrid`]: (base, extra bit count).
pub(crate) fn hybrid_base(tok: u32, k: u32, m: u32) -> (u32, u32) {
    if tok < (1 << k) {
        return (tok, 0);
    }
    let t = tok - (1 << k);
    let msb = k + t / (1 << m);
    let top = t % (1 << m);
    let nextra = msb - m;
    (((1 << m) | top) << nextra, nextra)
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    #[test]
    fn hybrid_roundtrip() {
        for (k, m) in [
            (4, 2),
            (6, 2),
            (8, 2),
            (4, 1),
            (2, 2),
            (1, 1),
            (1, 0),
            (3, 0),
        ] {
            for u in (0u32..70000).chain([u32::MAX >> 1, (1 << 31) + 12345]) {
                let (t, e, n) = hybrid(u, k, m);
                let (b, n2) = hybrid_base(t, k, m);
                assert_eq!(n, n2);
                assert_eq!(b + e, u, "k={k} m={m} u={u}");
            }
        }
    }
}
