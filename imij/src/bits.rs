//! LSB-first bit packing.

use alloc::vec::Vec;

/// Appends bits LSB-first into a byte vector.
pub(crate) struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl BitWriter {
    #[cfg(feature = "encode")]
    pub(crate) fn new() -> Self {
        Self {
            out: Vec::new(),
            acc: 0,
            n: 0,
        }
    }

    #[cfg(feature = "encode")]
    #[cfg_attr(not(feature = "lab"), allow(dead_code))]
    pub(crate) fn with_capacity(cap: usize) -> Self {
        Self {
            out: Vec::with_capacity(cap),
            acc: 0,
            n: 0,
        }
    }

    pub(crate) fn try_with_capacity(cap: usize) -> Result<Self, crate::Error> {
        let mut out = Vec::new();
        out.try_reserve_exact(cap)
            .map_err(|_| crate::Error::TooLarge)?;
        Ok(Self { out, acc: 0, n: 0 })
    }

    /// Writes the low `nbits` of `v` (nbits <= 32).
    #[inline(always)]
    pub(crate) fn put(&mut self, v: u32, nbits: u32) {
        debug_assert!(nbits <= 32);
        debug_assert!(nbits == 32 || v >> nbits == 0);
        self.acc |= (v as u64) << self.n;
        self.n += nbits;
        if self.n >= 32 {
            self.out.extend_from_slice(&(self.acc as u32).to_le_bytes());
            self.acc >>= 32;
            self.n -= 32;
        }
    }

    /// Bits written so far.
    #[cfg(feature = "encode")]
    pub(crate) fn bit_len(&self) -> u64 {
        self.out.len() as u64 * 8 + self.n as u64
    }

    /// Flushes partial bytes (zero padded) and returns the buffer.
    pub(crate) fn finish(mut self) -> Vec<u8> {
        while self.n > 0 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n = self.n.saturating_sub(8);
        }
        self.out
    }
}

/// Reads bits LSB-first. Reading past the end yields zero bits and sets
/// `overrun` once more bits were consumed than the stream holds.
pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u64,
    cnt: u32,
    /// Bits consumed beyond the end of `data` (virtual zero padding).
    phantom: u32,
}

impl<'a> BitReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        let mut r = Self {
            data,
            pos: 0,
            buf: 0,
            cnt: 0,
            phantom: 0,
        };
        r.refill();
        r
    }

    /// Ensures at least 56 bits are buffered.
    #[inline(always)]
    pub(crate) fn refill(&mut self) {
        if let Some(chunk) = self.data.get(self.pos..self.pos + 8) {
            let v = u64::from_le_bytes(chunk.try_into().unwrap());
            self.buf |= v << self.cnt;
            self.pos += ((63 - self.cnt) >> 3) as usize;
            self.cnt |= 56;
        } else {
            self.refill_slow();
        }
    }

    #[cold]
    #[inline(never)]
    fn refill_slow(&mut self) {
        while self.cnt <= 56 {
            match self.data.get(self.pos) {
                Some(&b) => {
                    self.buf |= (b as u64) << self.cnt;
                    self.pos += 1;
                }
                None => {
                    // Virtual zero byte.
                    self.phantom = self.phantom.saturating_add(8);
                }
            }
            self.cnt += 8;
        }
    }

    #[inline(always)]
    pub(crate) fn peek(&self) -> u64 {
        self.buf
    }

    #[inline(always)]
    pub(crate) fn consume(&mut self, n: u32) {
        self.buf >>= n;
        self.cnt -= n;
    }

    /// Reads `n` bits (n <= 32). Caller must have refilled.
    #[inline(always)]
    pub(crate) fn read(&mut self, n: u32) -> u32 {
        let v = (self.buf & ((1u64 << n) - 1)) as u32;
        self.consume(n);
        v
    }

    /// Reads `n` bits with an internal refill (any n <= 32).
    pub(crate) fn read_refill(&mut self, n: u32) -> u32 {
        self.refill();
        self.read(n)
    }

    /// Bits consumed so far (virtual zero padding included).
    pub(crate) fn bits_read(&self) -> u64 {
        self.pos as u64 * 8 + self.phantom as u64 - self.cnt as u64
    }

    /// True if more bits were consumed than the stream contains.
    pub(crate) fn overrun(&self) -> bool {
        // Buffered-but-unconsumed bits may include phantom zero bits.
        self.phantom > self.cnt
    }
}

// Shared gamma syntax for lossless weights and lossy context-map runs.
#[cfg(feature = "encode")]
pub(crate) fn put_gamma(w: &mut BitWriter, v: u32) {
    debug_assert!(v >= 1);
    let n = 31 - v.leading_zeros();
    w.put(0, n);
    w.put(1, 1);
    if n > 0 {
        w.put(v & ((1 << n) - 1), n);
    }
}

pub(crate) fn get_gamma(r: &mut BitReader<'_>) -> Result<u32, crate::Error> {
    r.refill();
    let z = (r.peek() as u32).trailing_zeros();
    if z > 24 {
        return Err(crate::Error::Corrupt("bad gamma"));
    }
    r.consume(z + 1);
    Ok((1 << z) | r.read(z))
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    #[test]
    fn gamma_wire_bits_and_boundaries() {
        // Independent bit-at-a-time spelling, including both sides of every
        // width change admitted by the reader. No encoder/decoder roundtrip
        // alone can detect a shared change to the wire representation.
        for width in 0..=24 {
            for value in [1u32 << width, (1u32 << (width + 1)) - 1] {
                let mut expected = alloc::vec![0u8; (2 * width + 1usize).div_ceil(8)];
                expected[width / 8] |= 1 << (width % 8);
                for bit in 0..width {
                    if value & (1 << bit) != 0 {
                        let at = width + 1 + bit;
                        expected[at / 8] |= 1 << (at % 8);
                    }
                }
                let mut writer = BitWriter::new();
                put_gamma(&mut writer, value);
                assert_eq!(writer.finish(), expected);
                assert_eq!(get_gamma(&mut BitReader::new(&expected)).unwrap(), value);
            }
        }
        assert!(matches!(
            get_gamma(&mut BitReader::new(&[0, 0, 0, 2])),
            Err(crate::Error::Corrupt("bad gamma"))
        ));
    }
}
