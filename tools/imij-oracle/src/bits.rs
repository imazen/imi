//! Byte and bit readers (SPEC section 0: little-endian, LSB-first bits).

use crate::error::{Error, Result};

/// A bounds-checked cursor over a byte slice.
pub(crate) struct Reader<'a> {
    pub(crate) data: &'a [u8],
    pub(crate) pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }

    pub(crate) fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub(crate) fn u8(&mut self, what: &str) -> Result<u8> {
        let b = *self
            .data
            .get(self.pos)
            .ok_or_else(|| Error::corrupt(format!("truncated {what}")))?;
        self.pos += 1;
        Ok(b)
    }

    /// A LEB128 varint, at most 10 bytes, minimal encoding required.
    pub(crate) fn varint(&mut self, what: &str) -> Result<u64> {
        let mut value: u64 = 0;
        let mut shift = 0u32;
        let mut count = 0usize;
        loop {
            let byte = *self
                .data
                .get(self.pos)
                .ok_or_else(|| Error::corrupt(format!("truncated {what}")))?;
            self.pos += 1;
            count += 1;
            if count > 10 {
                return Err(Error::corrupt(format!("overlong {what}")));
            }
            value |= u64::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                if count > 1 && byte == 0 {
                    return Err(Error::corrupt(format!("non-minimal {what}")));
                }
                return Ok(value);
            }
            shift += 7;
        }
    }

    /// The header's feature mask (SPEC section 1.0): an LEB128 bit string of
    /// any length, minimal encoding required. Returns bits 0-63 and whether
    /// any higher bit is set.
    pub(crate) fn feature_mask(&mut self) -> Result<(u64, bool)> {
        let mut low: u64 = 0;
        let mut high = false;
        let mut first = 0usize;
        loop {
            let byte = self.u8("features")?;
            let group = u64::from(byte & 0x7F);
            for b in 0..7 {
                if group >> b & 1 == 1 {
                    let id = first + b;
                    if id < 64 {
                        low |= 1 << id;
                    } else {
                        high = true;
                    }
                }
            }
            if byte & 0x80 == 0 {
                if first > 0 && byte == 0 {
                    return Err(Error::corrupt("non-minimal features"));
                }
                return Ok((low, high));
            }
            first += 7;
        }
    }

    pub(crate) fn take(&mut self, n: usize, what: &str) -> Result<&'a [u8]> {
        if self.pos + n > self.data.len() {
            return Err(Error::corrupt(format!("truncated {what}")));
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }
}

/// A bounds-checked LSB-first bit reader.
pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    bit: usize,
}

impl<'a> BitReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        BitReader { data, bit: 0 }
    }

    /// Read `n` bits (0..=64), LSB-first.
    pub(crate) fn read(&mut self, n: u32, what: &str) -> Result<u64> {
        if n == 0 {
            return Ok(0);
        }
        if self.bit + n as usize > self.data.len() * 8 {
            return Err(Error::corrupt(format!("truncated {what}")));
        }
        let mut value = 0u64;
        for i in 0..n {
            let byte = self.data[self.bit >> 3];
            let bit = (byte >> (self.bit & 7)) & 1;
            value |= u64::from(bit) << i;
            self.bit += 1;
        }
        Ok(value)
    }

    /// Bits read so far.
    pub(crate) fn position(&self) -> usize {
        self.bit
    }

    /// SPEC's `gamma(v)`: `z` zero bits, a one bit, then the low `z` bits of `v`.
    pub(crate) fn gamma(&mut self, what: &str) -> Result<u64> {
        let mut z = 0u32;
        while self.read(1, what)? == 0 {
            z += 1;
            if z > 60 {
                return Err(Error::corrupt(format!("overlong gamma in {what}")));
            }
        }
        let low = self.read(z, what)?;
        Ok((1u64 << z) | low)
    }
}

/// Decode SPEC's zigzag: `(z >> 1) ^ -(z & 1)`.
pub(crate) fn unzigzag(value: u64) -> i64 {
    ((value >> 1) as i64) ^ -((value & 1) as i64)
}

/// Zigzag-encode, for tests.
#[cfg(test)]
pub(crate) fn zigzag(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_minimal_and_bounds() {
        let mut r = Reader::new(&[0x00]);
        assert_eq!(r.varint("v").unwrap(), 0);
        let mut r = Reader::new(&[0x80, 0x00]);
        assert!(matches!(r.varint("v"), Err(Error::Corrupt(_))));
        let mut r = Reader::new(&[0xFF, 0x03]);
        assert_eq!(r.varint("v").unwrap(), 511);
        let mut r = Reader::new(&[0x80]);
        assert!(matches!(r.varint("v"), Err(Error::Corrupt(_))));
    }

    #[test]
    fn bits_are_lsb_first() {
        // 0b1010_0101, bit 0 first
        let mut b = BitReader::new(&[0xA5]);
        let got: Vec<u64> = (0..8).map(|_| b.read(1, "t").unwrap()).collect();
        assert_eq!(got, vec![1, 0, 1, 0, 0, 1, 0, 1]);
    }

    #[test]
    fn gamma_round_trips() {
        for value in 1u64..=64 {
            // Encode gamma MSB-first per the spec's bit order (LSB-first stream).
            let z = 63 - value.leading_zeros();
            let mut bits: Vec<bool> = Vec::new();
            bits.extend(core::iter::repeat_n(false, z as usize));
            bits.push(true);
            for i in 0..z {
                bits.push((value >> i) & 1 == 1);
            }
            let mut bytes = vec![0u8; bits.len().div_ceil(8)];
            for (i, bit) in bits.iter().enumerate() {
                if *bit {
                    bytes[i >> 3] |= 1 << (i & 7);
                }
            }
            let mut r = BitReader::new(&bytes);
            assert_eq!(r.gamma("g").unwrap(), value);
        }
    }

    #[test]
    fn zigzag_matches_unzigzag() {
        for value in -100i64..=100 {
            assert_eq!(unzigzag(zigzag(value)), value);
        }
    }
}
