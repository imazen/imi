//! Small container walkers, independent of optional decoder routes.

use core::ops::Range;

/// A LEB128 varint at the start of `b`: (value, bytes read).
pub fn varint(b: &[u8]) -> (u64, usize) {
    let (mut v, mut s) = (0u64, 0);
    for (i, &x) in b.iter().enumerate() {
        v |= ((x & 0x7f) as u64) << s;
        if x & 0x80 == 0 {
            return (v, i + 1);
        }
        s += 7;
    }
    panic!("truncated varint")
}

/// The header record of a file (after the signature and length varint).
pub fn header_record(file: &[u8]) -> Range<usize> {
    let (n, k) = varint(&file[8..]);
    8 + k..8 + k + n as usize
}

/// The offset of the first chunk.
pub fn header_end(file: &[u8]) -> usize {
    header_record(file).end
}

pub fn chunks(file: &[u8]) -> Vec<([u8; 4], usize, usize)> {
    let mut at = header_end(file);
    let mut v = Vec::new();
    while at < file.len() {
        let tag: [u8; 4] = file[at..at + 4].try_into().unwrap();
        let (n, k) = varint(&file[at + 4..]);
        if n == 0 {
            v.push((tag, at, file.len()));
            break;
        }
        let end = at + 4 + k + n as usize - 1;
        v.push((tag, at, end));
        at = end;
    }
    v
}
