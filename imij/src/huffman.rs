#![allow(
    dead_code,
    unused_mut,
    unused_variables,
    private_interfaces,
    clippy::all
)]
//! Canonical, length-limited prefix codes (LSB-first, bit-reversed codewords).

use alloc::vec;
use alloc::vec::Vec;

use crate::Error;
use crate::bits::{BitReader, BitWriter};

/// Maximum codeword length allowed by the format.
pub(crate) const MAX_LEN: u32 = 12;

/// Builds length-limited code lengths for `freqs`. Symbols with zero
/// frequency get length 0. If exactly one symbol is used it gets length 0
/// (it is implied and costs no bits).
pub(crate) fn build_lengths(freqs: &[u64], max_len: u32) -> Vec<u8> {
    let n = freqs.len();
    let mut lens = vec![0u8; n];
    let used: Vec<usize> = (0..n).filter(|&i| freqs[i] > 0).collect();
    if used.len() <= 1 {
        return lens;
    }
    // Huffman via sorted merge (two-queue).
    let mut leaves: Vec<(u64, usize)> = used.iter().map(|&i| (freqs[i], i)).collect();
    leaves.sort_unstable();
    // Nodes: leaves 0..m, internal m..2m-1. parent[] links.
    let m = leaves.len();
    let mut weight: Vec<u64> = leaves.iter().map(|l| l.0).collect();
    weight.reserve(m);
    let mut parent = vec![usize::MAX; 2 * m];
    let (mut li, mut ii) = (0usize, m);
    let mut next = m;
    let mut pick = |weight: &Vec<u64>, li: &mut usize, ii: &mut usize, next: usize| -> usize {
        if *li < m && (*ii >= next || weight[*li] <= weight[*ii]) {
            *li += 1;
            *li - 1
        } else {
            *ii += 1;
            *ii - 1
        }
    };
    while next < 2 * m - 1 {
        let a = pick(&weight, &mut li, &mut ii, next);
        let b = pick(&weight, &mut li, &mut ii, next);
        weight.push(weight[a] + weight[b]);
        parent[a] = next;
        parent[b] = next;
        next += 1;
    }
    // Depths.
    let root = 2 * m - 2;
    let mut depth = vec![0u32; 2 * m - 1];
    for node in (0..root).rev() {
        depth[node] = depth[parent[node]] + 1;
    }
    let mut l: Vec<u32> = (0..m).map(|i| depth[i]).collect();

    // Length limiting.
    if l.iter().any(|&d| d > max_len) {
        for d in l.iter_mut() {
            *d = (*d).min(max_len);
        }
        let target: u64 = 1 << max_len;
        let kraft = |l: &[u32]| -> u64 { l.iter().map(|&d| 1u64 << (max_len - d)).sum() };
        let mut k = kraft(&l);
        // Leaves are sorted by ascending frequency: lengthen rare symbols first.
        while k > target {
            // Pick the lowest-frequency leaf with length < max_len whose
            // lengthening does not overshoot more than necessary.
            let mut best = usize::MAX;
            for i in 0..m {
                if l[i] < max_len {
                    let dec = 1u64 << (max_len - l[i] - 1);
                    if dec <= k - target {
                        best = i;
                        break;
                    }
                    if best == usize::MAX {
                        best = i;
                    }
                }
            }
            k -= 1u64 << (max_len - l[best] - 1);
            l[best] += 1;
        }
        // Use any slack: shorten the most frequent symbols where it fits.
        for i in (0..m).rev() {
            while l[i] > 1 {
                let inc = 1u64 << (max_len - l[i]);
                if k + inc <= target {
                    k += inc;
                    l[i] -= 1;
                } else {
                    break;
                }
            }
        }
    }
    for (i, &(_, sym)) in leaves.iter().enumerate() {
        lens[sym] = l[i] as u8;
    }
    lens
}

/// Bit-reversed canonical codewords for `lens`.
pub(crate) fn canonical_codes(lens: &[u8]) -> Vec<u16> {
    let mut bl_count = [0u32; 16];
    for &l in lens {
        bl_count[l as usize] += 1;
    }
    bl_count[0] = 0;
    let mut next = [0u32; 16];
    let mut code = 0u32;
    for b in 1..16 {
        code = (code + bl_count[b - 1]) << 1;
        next[b] = code;
    }
    let mut codes = vec![0u16; lens.len()];
    for (s, &l) in lens.iter().enumerate() {
        if l > 0 {
            let c = next[l as usize];
            next[l as usize] += 1;
            codes[s] = reverse_bits(c, l as u32) as u16;
        }
    }
    codes
}

#[inline]
fn reverse_bits(mut v: u32, n: u32) -> u32 {
    let mut r = 0;
    for _ in 0..n {
        r = (r << 1) | (v & 1);
        v >>= 1;
    }
    r
}

/// Cost in bits of coding `freqs` with `lens`.
pub(crate) fn cost_bits(freqs: &[u64], lens: &[u8]) -> u64 {
    freqs.iter().zip(lens).map(|(&f, &l)| f * l as u64).sum()
}

// ---- Serialization -------------------------------------------------------

const META_SYMS: usize = 16;
const META_MAX_LEN: u32 = 7;
const REP_PREV: u8 = 13; // repeat previous length 3..=6 (2 bits)
const ZEROS_SHORT: u8 = 14; // 3..=10 zeros (3 bits)
const ZEROS_LONG: u8 = 15; // 11..=138 zeros (7 bits)

/// Run-length tokens for a length vector: (meta symbol, extra value, extra bits).
fn length_tokens(lens: &[u8]) -> Vec<(u8, u32, u32)> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut prev: Option<u8> = None;
    while i < lens.len() {
        let l = lens[i];
        let mut run = 1;
        while i + run < lens.len() && lens[i + run] == l {
            run += 1;
        }
        if l == 0 {
            let mut r = run;
            while r > 0 {
                if r >= 11 {
                    let k = r.min(138);
                    out.push((ZEROS_LONG, (k - 11) as u32, 7));
                    r -= k;
                } else if r >= 3 {
                    out.push((ZEROS_SHORT, (r - 3) as u32, 3));
                    r = 0;
                } else {
                    out.push((0, 0, 0));
                    r -= 1;
                }
            }
            prev = Some(0);
        } else {
            let mut r = run;
            if prev != Some(l) {
                out.push((l, 0, 0));
                r -= 1;
            }
            while r > 0 {
                if r >= 3 {
                    let k = r.min(6);
                    out.push((REP_PREV, (k - 3) as u32, 2));
                    r -= k;
                } else {
                    out.push((l, 0, 0));
                    r -= 1;
                }
            }
            prev = Some(l);
        }
        i += run;
    }
    out
}

/// Shape of a serialized table.
pub(crate) enum TableKind {
    /// No symbol used; never decoded.
    Empty,
    /// Exactly one symbol; costs zero bits per use.
    Single(u16),
    /// Coded lengths.
    Coded,
}

pub(crate) fn table_kind(lens: &[u8], freqs: &[u64]) -> TableKind {
    let used: Vec<usize> = (0..freqs.len()).filter(|&i| freqs[i] > 0).collect();
    match used.len() {
        0 => TableKind::Empty,
        1 => TableKind::Single(used[0] as u16),
        _ => {
            debug_assert!(lens.iter().any(|&l| l > 0));
            TableKind::Coded
        }
    }
}

/// Writes a group of tables sharing one meta code. `tables` holds
/// (lens, freqs) per table; `alpha_bits` is the bit width of a symbol index.
pub(crate) fn write_tables(w: &mut BitWriter, tables: &[(&[u8], &[u64])], alpha_bits: u32) {
    let mut meta_freq = [0u64; META_SYMS];
    let mut toks: Vec<Vec<(u8, u32, u32)>> = Vec::with_capacity(tables.len());
    for (lens, freqs) in tables {
        if let TableKind::Coded = table_kind(lens, freqs) {
            let last = lens.iter().rposition(|&l| l > 0).unwrap();
            let t = length_tokens(&lens[..=last]);
            for &(s, _, _) in &t {
                meta_freq[s as usize] += 1;
            }
            toks.push(t);
        } else {
            toks.push(Vec::new());
        }
    }
    let meta_lens = {
        let mut l = build_lengths(&meta_freq, META_MAX_LEN);
        // A meta code with a single used symbol still needs a 1-bit code so
        // the reader can stay generic.
        let used = meta_freq.iter().filter(|&&f| f > 0).count();
        if used <= 1 {
            let s = meta_freq.iter().position(|&f| f > 0).unwrap_or(0);
            l[s] = 1;
            l[(s + 1) % META_SYMS] = 1;
        }
        l
    };
    for &l in &meta_lens {
        w.put(l as u32, 3);
    }
    let meta_codes = canonical_codes(&meta_lens);
    for (ti, (lens, freqs)) in tables.iter().enumerate() {
        match table_kind(lens, freqs) {
            TableKind::Empty => w.put(0, 2),
            TableKind::Single(s) => {
                w.put(1, 2);
                w.put(s as u32, alpha_bits);
            }
            TableKind::Coded => {
                w.put(2, 2);
                let last = lens.iter().rposition(|&l| l > 0).unwrap();
                w.put(last as u32, alpha_bits);
                for &(s, v, nb) in &toks[ti] {
                    w.put(meta_codes[s as usize] as u32, meta_lens[s as usize] as u32);
                    if nb > 0 {
                        w.put(v, nb);
                    }
                }
            }
        }
    }
}

/// Reads tables written by [`write_tables`]. Returns per-table lengths
/// (length vector of size `alpha_size`; a Single table is represented as a
/// single symbol with length 0 and `Some(sym)`).
pub(crate) fn read_tables(
    r: &mut BitReader<'_>,
    count: usize,
    alpha_size: usize,
    alpha_bits: u32,
) -> Result<Vec<ReadTable>, Error> {
    let mut meta_lens = [0u8; META_SYMS];
    for l in meta_lens.iter_mut() {
        *l = r.read_refill(3) as u8;
    }
    let meta = DecodeTable::build(&meta_lens)?;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let kind = r.read_refill(2);
        match kind {
            0 => out.push(ReadTable::Empty),
            1 => {
                let s = r.read_refill(alpha_bits) as usize;
                if s >= alpha_size {
                    return Err(Error::Corrupt("table symbol out of range"));
                }
                out.push(ReadTable::Single(s as u16));
            }
            2 => {
                let last = r.read_refill(alpha_bits) as usize;
                if last >= alpha_size {
                    return Err(Error::Corrupt("table length out of range"));
                }
                let mut lens = vec![0u8; alpha_size];
                let mut i = 0;
                let mut prev = 0u8;
                while i <= last {
                    r.refill();
                    let s = meta.decode(r) as u8;
                    match s {
                        0..=12 => {
                            lens[i] = s;
                            prev = s;
                            i += 1;
                        }
                        REP_PREV => {
                            let k = 3 + r.read(2) as usize;
                            if i + k > last + 1 || prev == 0 {
                                return Err(Error::Corrupt("bad length repeat"));
                            }
                            lens[i..i + k].fill(prev);
                            i += k;
                        }
                        ZEROS_SHORT | ZEROS_LONG => {
                            let k = if s == ZEROS_SHORT {
                                3 + r.read(3) as usize
                            } else {
                                11 + r.read(7) as usize
                            };
                            if i + k > last + 1 {
                                return Err(Error::Corrupt("bad zero run"));
                            }
                            i += k;
                            prev = 0;
                        }
                        _ => return Err(Error::Corrupt("bad meta symbol")),
                    }
                }
                out.push(ReadTable::Coded(lens));
            }
            _ => return Err(Error::Corrupt("bad table kind")),
        }
    }
    if r.overrun() {
        return Err(Error::Corrupt("truncated tables"));
    }
    Ok(out)
}

pub(crate) enum ReadTable {
    Empty,
    Single(u16),
    Coded(Vec<u8>),
}

/// Single-level lookup table: entry = symbol << 4 | length.
pub(crate) struct DecodeTable {
    pub(crate) bits: u32,
    pub(crate) entries: Vec<u16>,
}

impl DecodeTable {
    /// Builds a lookup table from code lengths. Validates completeness.
    pub(crate) fn build(lens: &[u8]) -> Result<Self, Error> {
        let max = lens.iter().copied().max().unwrap_or(0) as u32;
        if max == 0 || max > MAX_LEN {
            return Err(Error::Corrupt("bad code lengths"));
        }
        let kraft: u64 = lens
            .iter()
            .filter(|&&l| l > 0)
            .map(|&l| 1u64 << (max - l as u32))
            .sum();
        if kraft != 1u64 << max {
            return Err(Error::Corrupt("incomplete prefix code"));
        }
        let codes = canonical_codes(lens);
        let size = 1usize << max;
        let mut entries = vec![0u16; size];
        for (s, &l) in lens.iter().enumerate() {
            if l == 0 {
                continue;
            }
            let e = ((s as u16) << 4) | l as u16;
            let step = 1usize << l;
            let mut i = codes[s] as usize;
            while i < size {
                entries[i] = e;
                i += step;
            }
        }
        Ok(Self { bits: max, entries })
    }

    /// Single-symbol table: zero-length code.
    pub(crate) fn single(sym: u16) -> Self {
        Self {
            bits: 0,
            entries: vec![sym << 4],
        }
    }

    #[inline(always)]
    pub(crate) fn decode(&self, r: &mut BitReader<'_>) -> u32 {
        let idx = (r.peek() & ((1u64 << self.bits) - 1)) as usize;
        let e = self.entries[idx];
        r.consume((e & 15) as u32);
        (e >> 4) as u32
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    #[test]
    fn limited_lengths_are_complete() {
        // Fibonacci-like frequencies force deep trees.
        let mut f = vec![0u64; 40];
        let (mut a, mut b) = (1u64, 1u64);
        for x in f.iter_mut() {
            *x = a;
            let c = a + b;
            a = b;
            b = c;
        }
        for max in [8, 10, 12] {
            let l = build_lengths(&f, max);
            assert!(l.iter().all(|&x| x as u32 <= max && x > 0));
            let k: u64 = l.iter().map(|&x| 1u64 << (max - x as u32)).sum();
            assert_eq!(k, 1 << max);
            DecodeTable::build(&l).unwrap();
        }
    }

    #[test]
    fn roundtrip_tables() {
        let mut freqs = vec![0u64; 300];
        for (i, f) in freqs.iter_mut().enumerate() {
            if i % 7 != 3 {
                *f = (1000 / (i + 1)) as u64 + 1;
            }
        }
        let lens = build_lengths(&freqs, 12);
        let single = {
            let mut f = vec![0u64; 300];
            f[17] = 5;
            f
        };
        let slens = build_lengths(&single, 12);
        let empty = vec![0u64; 300];
        let elens = vec![0u8; 300];
        let mut w = BitWriter::new();
        write_tables(
            &mut w,
            &[(&lens, &freqs), (&slens, &single), (&elens, &empty)],
            9,
        );
        // Encode a few symbols.
        let codes = canonical_codes(&lens);
        let syms = [0usize, 5, 299, 100, 1];
        for &s in &syms {
            w.put(codes[s] as u32, lens[s] as u32);
        }
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes);
        let t = read_tables(&mut r, 3, 300, 9).unwrap();
        let ReadTable::Coded(l2) = &t[0] else {
            panic!()
        };
        assert_eq!(l2, &lens);
        assert!(matches!(t[1], ReadTable::Single(17)));
        assert!(matches!(t[2], ReadTable::Empty));
        let dt = DecodeTable::build(l2).unwrap();
        for &s in &syms {
            r.refill();
            assert_eq!(dt.decode(&mut r), s as u32);
        }
    }
}
