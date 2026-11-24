//! Plain bit streams for the split-Huffman entropy mode: LSB-first, bit
//! `i` of the stream is bit `i % 8` of byte `i / 8`. Fields are at most 57
//! bits, so a reader loads one little-endian 64-bit window at the field's
//! byte and shifts; reads past the end see zeros (the decoder checks the
//! final offset against the stream length).

#[cfg(feature = "encode")]
use alloc::vec::Vec;

/// LSB-first bit writer.
#[derive(Default)]
#[cfg(feature = "encode")]
pub(crate) struct RawWriter {
    buf: Vec<u8>,
    acc: u64,
    n: u32,
}

#[cfg(feature = "encode")]
impl RawWriter {
    /// Appends the low `n` (<= 32) bits of `v`.
    pub(crate) fn put(&mut self, v: u32, n: u32) {
        debug_assert!(n <= 32 && (n == 32 || v >> n == 0));
        self.acc |= (v as u64) << self.n;
        self.n += n;
        if self.n >= 64 {
            self.buf.extend_from_slice(&self.acc.to_le_bytes());
            self.n -= 64;
            // The field's bits above the emitted word are not retained by
            // the left shift. Keep them for the next word; self.n < n <= 32.
            self.acc = u64::from(v) >> (n - self.n);
        }
    }

    /// Bits written so far.
    #[cfg(all(test, feature = "encode"))]
    pub(crate) fn bits(&self) -> u64 {
        self.buf.len() as u64 * 8 + self.n as u64
    }

    pub(crate) fn finish(mut self) -> Vec<u8> {
        self.buf
            .extend_from_slice(&self.acc.to_le_bytes()[..self.n.div_ceil(8) as usize]);
        self.buf
    }
}

/// The 64 bits starting at byte `i` (little-endian), zero past the end.
#[inline(always)]
pub(crate) fn window(buf: &[u8], i: usize) -> u64 {
    if let Some(b) = buf.get(i..i + 8) {
        u64::from_le_bytes(b.try_into().unwrap())
    } else {
        let mut t = [0u8; 8];
        if i < buf.len() {
            t[..buf.len() - i].copy_from_slice(&buf[i..]);
        }
        u64::from_le_bytes(t)
    }
}

/// `n` (<= 57) bits at bit offset `pos`.
#[inline(always)]
pub(crate) fn get(buf: &[u8], pos: u64, n: u32) -> u64 {
    let w = window(buf, (pos >> 3) as usize) >> (pos & 7);
    w & ((1u64 << n) - 1)
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    #[test]
    fn complete_bytes_match_independent_bit_writer_at_every_word_boundary() {
        let mut seed = 0x1234_5678u32;
        for prefix in 0..64u32 {
            let mut writer = RawWriter::default();
            let mut expected = Vec::new();
            let mut bits = 0usize;
            let mut append = |value: u32, width: u32| {
                writer.put(value, width);
                for k in 0..width {
                    if bits.is_multiple_of(8) {
                        expected.push(0u8);
                    }
                    expected[bits / 8] |= (((value >> k) & 1) as u8) << (bits % 8);
                    bits += 1;
                }
                assert_eq!(writer.bits(), bits as u64);
            };
            for _ in 0..prefix {
                append(1, 1);
            }
            for n in 0..4096u32 {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let width = n % 33;
                let mask = u32::MAX >> (32 - width).min(31);
                let value = if width == 0 { 0 } else { seed & mask };
                append(value, width);
            }
            assert_eq!(writer.finish(), expected, "initial bit phase {prefix}");
        }
    }

    #[test]
    fn roundtrip() {
        let mut w = RawWriter::default();
        let mut fields = Vec::new();
        let mut s = 0x1234_5678u32;
        for i in 0..5000u32 {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let n = i % 33;
            let v = if n == 32 { s } else { s & ((1u32 << n) - 1) };
            fields.push((v, n));
            w.put(v, n);
        }
        let total = w.bits();
        let buf = w.finish();
        assert_eq!(buf.len() as u64, total.div_ceil(8));
        let mut pos = 0u64;
        for &(v, n) in &fields {
            assert_eq!(get(&buf, pos, n) as u32, v);
            pos += n as u64;
        }
        assert_eq!(get(&buf, pos + 100, 20), 0);
    }
}
