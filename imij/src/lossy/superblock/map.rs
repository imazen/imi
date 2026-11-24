//! The partition map of a lossy unit (SPEC 2.9): one symbol per aligned
//! 2x2 parent group ([`super::map_groups`], raster order), coded with an
//! adaptive binary range coder.
//!
//! Per group, with the context `3 class(left) + class(up)` of the groups two
//! parents to the left and above (class 0: absent or no merge, 1: 16x16,
//! 2: any other symbol): one bit "merged" (probability `any[ctx]`), and for
//! a merged group the symbol minus one in three bits, most significant
//! first, through a binary tree (node 1, then `2 node + bit`; probability
//! `tree[node][ctx]`). Tree value 7 and symbols that need an absent parent
//! are corrupt.
//!
//! The coder is LZMA's binary range coder: a 32-bit range, 12-bit
//! probabilities starting at 1/2 and adapting by 1/32 toward each coded bit,
//! one byte of renormalization whenever the range drops below 2^24. The
//! decoder starts from four bytes, big-endian, and reads zero bytes past the
//! end of the map. The encoder ends with the code value of the final
//! interval that has the most trailing zero bits and drops its leading
//! carry byte and trailing zero bytes.

use super::{PARTITIONS, map_groups};
use crate::Error;
use alloc::vec::Vec;

const PROB_BITS: u32 = 12;
const PROB_ONE: u16 = 1 << PROB_BITS;
const SHIFT: u32 = 5;
const TOP: u32 = 1 << 24;

/// The adaptive probabilities of one map.
struct Model {
    /// The merged bit by context.
    any: [u16; 9],
    /// Tree nodes 1..=7 by context.
    tree: [[u16; 9]; 8],
}

impl Model {
    fn new() -> Self {
        Self {
            any: [PROB_ONE / 2; 9],
            tree: [[PROB_ONE / 2; 9]; 8],
        }
    }

    #[inline(always)]
    fn write_symbol(&mut self, e: &mut Encoder, ctx: usize, symbol: u8) {
        e.bit(&mut self.any[ctx], symbol != 0);
        if symbol != 0 {
            let mut node = 1;
            for shift in (0..3).rev() {
                let bit = (symbol - 1) >> shift & 1 != 0;
                e.bit(&mut self.tree[node][ctx], bit);
                node = node * 2 + usize::from(bit);
            }
        }
    }

    #[inline(always)]
    fn read_symbol(&mut self, d: &mut Decoder<'_>, ctx: usize) -> Result<u8, Error> {
        if !d.bit(&mut self.any[ctx]) {
            return Ok(0);
        }
        let mut node = 1;
        for _ in 0..3 {
            node = node * 2 + usize::from(d.bit(&mut self.tree[node][ctx]));
        }
        if node == 15 {
            return Err(Error::Corrupt("superblock map symbol"));
        }
        Ok((node - 7) as u8)
    }
}

/// Context class of a neighbouring group's symbol.
#[inline]
fn class(sym: Option<u8>) -> usize {
    match sym {
        None | Some(0) => 0,
        Some(1) => 1,
        _ => 2,
    }
}

/// Symbols of a unit's groups, by group row and column (`None`: no group or
/// not coded yet).
struct Grid {
    cols: usize,
    sym: Vec<u8>,
}

impl Grid {
    const NONE: u8 = 0xff;

    fn new(rows: usize, cols: usize) -> Result<Self, Error> {
        let (gr, gc) = (rows.div_ceil(2), cols.div_ceil(2));
        let n = gr.checked_mul(gc).ok_or(Error::TooLarge)?;
        let mut sym = Vec::new();
        sym.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
        sym.resize(n, Self::NONE);
        Ok(Self { cols: gc, sym })
    }

    fn get(&self, gr: usize, gc: usize) -> Option<u8> {
        Some(self.sym[gr * self.cols + gc]).filter(|&s| s != Self::NONE)
    }

    /// The context of group (gr, gc).
    fn ctx(&self, gr: usize, gc: usize) -> usize {
        let left = if gc > 0 { self.get(gr, gc - 1) } else { None };
        let up = if gr > 0 { self.get(gr - 1, gc) } else { None };
        class(left) * 3 + class(up)
    }
}

pub(super) struct Encoder {
    low: u64,
    range: u32,
    cache: u8,
    cache_size: u64,
    out: Vec<u8>,
}

impl Encoder {
    pub(super) fn new() -> Self {
        Self {
            low: 0,
            range: 0xffff_ffff,
            cache: 0,
            cache_size: 1,
            out: Vec::new(),
        }
    }

    fn shift_low(&mut self) {
        if (self.low as u32) < 0xff00_0000 || (self.low >> 32) != 0 {
            let carry = (self.low >> 32) as u8;
            let mut temp = self.cache;
            loop {
                self.out.push(temp.wrapping_add(carry));
                temp = 0xff;
                self.cache_size -= 1;
                if self.cache_size == 0 {
                    break;
                }
            }
            self.cache = ((self.low >> 24) & 0xff) as u8;
        }
        self.cache_size += 1;
        self.low = (self.low & 0x00ff_ffff) << 8;
    }

    pub(super) fn bit(&mut self, prob: &mut u16, bit: bool) {
        let bound = (self.range >> PROB_BITS) * *prob as u32;
        if !bit {
            self.range = bound;
            *prob += (PROB_ONE - *prob) >> SHIFT;
        } else {
            self.low += bound as u64;
            self.range -= bound;
            *prob -= *prob >> SHIFT;
        }
        while self.range < TOP {
            self.range <<= 8;
            self.shift_low();
        }
    }

    /// Ends the stream: the code value in [low, low + range) with the most
    /// trailing zero bits, five flush bytes, the leading carry byte (always
    /// 0) and trailing zero bytes dropped.
    #[cfg(feature = "encode")]
    pub(super) fn finish(mut self) -> Vec<u8> {
        let hi = self.low + self.range as u64;
        for k in (0..=32).rev() {
            let mask = (1u64 << k) - 1;
            let x = (self.low + mask) & !mask;
            if x < hi {
                self.low = x;
                break;
            }
        }
        let mut out = self.flush();
        while out.last() == Some(&0) {
            out.pop();
        }
        out
    }

    fn flush(mut self) -> Vec<u8> {
        for _ in 0..5 {
            self.shift_low();
        }
        self.out.remove(0);
        self.out
    }
}

pub(super) struct Decoder<'a> {
    bytes: &'a [u8],
    /// Bytes read, including the zeros read past the end.
    pos: usize,
    code: u32,
    range: u32,
}

impl<'a> Decoder<'a> {
    pub(super) fn new(bytes: &'a [u8]) -> Self {
        let mut d = Self {
            bytes,
            pos: 0,
            code: 0,
            range: 0xffff_ffff,
        };
        for _ in 0..4 {
            d.code = d.code << 8 | d.next() as u32;
        }
        d
    }

    #[inline]
    fn next(&mut self) -> u8 {
        let b = self.bytes.get(self.pos).copied().unwrap_or(0);
        self.pos += 1;
        b
    }

    pub(super) fn bit(&mut self, prob: &mut u16) -> bool {
        let bound = (self.range >> PROB_BITS) * *prob as u32;
        let bit = if self.code < bound {
            self.range = bound;
            *prob += (PROB_ONE - *prob) >> SHIFT;
            false
        } else {
            self.code -= bound;
            self.range -= bound;
            *prob -= *prob >> SHIFT;
            true
        };
        while self.range < TOP {
            self.range <<= 8;
            self.code = self.code << 8 | self.next() as u32;
        }
        bit
    }

    /// Bytes of the map the decoding never read.
    fn unread(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }
}

/// The group symbol of `roles` (one byte per parent of a `rows` x `cols`
/// unit) at group (r, x).
#[cfg(feature = "encode")]
fn group_symbol(roles: &[u8], cols: usize, r: usize, x: usize, has: [bool; 4]) -> u8 {
    let at = [(0, 0), (0, 1), (1, 0), (1, 1)];
    let g: [u8; 4] = core::array::from_fn(|i| {
        if has[i] {
            roles[(r + at[i].0) * cols + x + at[i].1]
        } else {
            0
        }
    });
    super::symbol_of(g).expect("aligned merges")
}

/// Codes a unit's map: `roles` holds one merge role per parent of the
/// `rows` x `cols` unit, every merge aligned and inside the unit. Returns
/// the map's bytes, empty when nothing merges.
#[cfg(feature = "encode")]
pub(crate) fn encode(roles: &[u8], rows: usize, cols: usize) -> Vec<u8> {
    debug_assert_eq!(roles.len(), rows * cols);
    if roles.iter().all(|&r| r == 0) {
        return Vec::new();
    }
    let mut m = Model::new();
    let mut e = Encoder::new();
    let mut grid = Grid::new(rows, cols).expect("encoder geometry");
    for (r, x, has) in map_groups(rows, cols) {
        let (gr, gc) = (r / 2, x / 2);
        let ctx = grid.ctx(gr, gc);
        let s = group_symbol(roles, cols, r, x, has);
        m.write_symbol(&mut e, ctx, s);
        grid.sym[gr * grid.cols + gc] = s;
    }
    e.finish()
}

/// Decodes a unit's map (`bytes`, not empty) into `roles` (one byte per
/// parent of the `rows` x `cols` unit, all zero on entry). Corrupt: a unit
/// without groups, tree value 7, a merge past the unit, a map that merges
/// nothing, or map bytes the decoding never reads.
pub(crate) fn decode(
    bytes: &[u8],
    rows: usize,
    cols: usize,
    roles: &mut [u8],
) -> Result<(), Error> {
    debug_assert!(!bytes.is_empty());
    debug_assert_eq!(roles.len(), rows * cols);
    let mut m = Model::new();
    let mut d = Decoder::new(bytes);
    let any = symbols(&mut d, &mut m, rows, cols, roles)?;
    if !any {
        return Err(Error::Corrupt("superblock map without a merge"));
    }
    if d.unread() != 0 {
        return Err(Error::Corrupt("superblock map has unread bytes"));
    }
    Ok(())
}

/// The partition symbols of a map (its first section, SPEC 2.9.2) into
/// `roles`; whether any group merges. Inlined so the range coder's state
/// stays in registers, as in one function.
#[inline(always)]
fn symbols(
    d: &mut Decoder<'_>,
    m: &mut Model,
    rows: usize,
    cols: usize,
    roles: &mut [u8],
) -> Result<bool, Error> {
    let mut grid = Grid::new(rows, cols)?;
    let mut any = false;
    for (r, x, has) in map_groups(rows, cols) {
        let (gr, gc) = (r / 2, x / 2);
        let ctx = grid.ctx(gr, gc);
        let s = m.read_symbol(d, ctx)?;
        any |= s != 0;
        let p = PARTITIONS[s as usize];
        if (0..4).any(|i| p[i] != 0 && !has[i]) {
            return Err(Error::Corrupt("superblock past its unit"));
        }
        grid.sym[gr * grid.cols + gc] = s;
        for i in 0..4 {
            if has[i] {
                roles[(r + i / 2) * cols + x + i % 2] = p[i];
            }
        }
    }
    Ok(any)
}

// V4 uses the same symbol model with a strict, untrimmed termination. Its
// complete code is the lower endpoint of the final interval, so the decoder
// must consume exactly the framed bytes and finish with a zero remainder.
fn v4_context(index: usize, col: usize, columns: usize, symbol: impl Fn(usize) -> u8) -> usize {
    let left = if col > 0 { symbol(index - 1) } else { 0 };
    let up = if index >= columns {
        symbol(index - columns)
    } else {
        0
    };
    usize::from(left.min(2)) * 3 + usize::from(up.min(2))
}

pub(crate) fn encode_v4_partitions(
    rows: usize,
    cols: usize,
    symbol: impl Fn(usize) -> u8,
) -> Result<Vec<u8>, Error> {
    let columns = cols.div_ceil(2);
    let mut encoder = Encoder::new();
    // At most four bits per cell and one refill per bit (p stays in
    // 31..=4065), plus the five-byte flush before removing the carry.
    let capacity = rows
        .div_ceil(2)
        .checked_mul(columns)
        .and_then(|n| n.checked_mul(4))
        .and_then(|n| n.checked_add(5))
        .ok_or(Error::TooLarge)?;
    encoder
        .out
        .try_reserve_exact(capacity)
        .map_err(|_| Error::TooLarge)?;
    write_v4_partition_symbols(&mut encoder, rows, cols, symbol);
    Ok(encoder.flush())
}

pub(super) fn write_v4_partition_symbols(
    encoder: &mut Encoder,
    rows: usize,
    cols: usize,
    symbol: impl Fn(usize) -> u8,
) {
    let mut model = Model::new();
    let columns = cols.div_ceil(2);
    for (y, x, _) in map_groups(rows, cols) {
        let col = x / 2;
        let index = y / 2 * columns + col;
        model.write_symbol(
            encoder,
            v4_context(index, col, columns, &symbol),
            symbol(index),
        );
    }
}

pub(crate) fn decode_v4_partitions(
    bytes: &[u8],
    rows: usize,
    cols: usize,
    symbols: &mut [u8],
    poll: impl FnMut() -> Result<(), Error>,
) -> Result<(), Error> {
    if bytes.len() < 4 {
        return Err(Error::Corrupt("V4 truncated partition range map"));
    }
    let mut decoder = Decoder::new(bytes);
    read_v4_partition_symbols(&mut decoder, rows, cols, symbols, poll)?;
    if decoder.pos != bytes.len() || decoder.code != 0 {
        return Err(Error::Corrupt("V4 partition range map termination"));
    }
    Ok(())
}

pub(super) fn read_v4_partition_symbols(
    decoder: &mut Decoder<'_>,
    rows: usize,
    cols: usize,
    symbols: &mut [u8],
    mut poll: impl FnMut() -> Result<(), Error>,
) -> Result<(), Error> {
    let columns = cols.div_ceil(2);
    let mut model = Model::new();
    let mut merged = false;
    for (y, x, present) in map_groups(rows, cols) {
        let col = x / 2;
        let index = y / 2 * columns + col;
        if index.is_multiple_of(32) {
            poll()?;
        }
        let ctx = v4_context(index, col, columns, |i| symbols[i]);
        let symbol = model.read_symbol(decoder, ctx)?;
        if decoder.pos > decoder.bytes.len() {
            return Err(Error::Corrupt("V4 truncated partition range map"));
        }
        if (0..4).any(|i| PARTITIONS[symbol as usize][i] != 0 && !present[i]) {
            return Err(Error::Corrupt("V4 merge crosses group edge"));
        }
        symbols[index] = symbol;
        merged |= symbol != 0;
    }
    if !merged {
        return Err(Error::Corrupt("V4 partition range map termination"));
    }
    Ok(())
}

// ---- the joint map of small transforms (SPEC 2.11.1) -------------------

/// Number of split-flag contexts: plane (3) x left (3) x up (3) x the
/// parent's luma flag (2).
const SPLIT_CTX: usize = 54;

/// The split-flag context of plane `plane` with the left and upper
/// parents' classes (0 or 1: their flag in this plane, 0 without one; 2:
/// absent, merged or bypassed) and the parent's luma flag (0 for luma
/// itself).
#[inline]
fn split_ctx(plane: usize, left: usize, up: usize, luma: usize) -> usize {
    ((plane * 3 + left) * 3 + up) * 2 + luma
}

/// Which parents of a unit carry flags: merged ones (a role) and bypassed
/// ones (`bypass(b)`: exact or kept) carry none; the others one per plane
/// with AC.
struct Flagged<'a> {
    roles: &'a [u8],
    bypass: &'a dyn Fn(usize) -> bool,
}

impl Flagged<'_> {
    #[inline]
    fn none(&self, b: usize) -> bool {
        self.roles[b] != 0 || (self.bypass)(b)
    }
    /// Parent `b`'s class for its neighbours' contexts in `plane`.
    #[inline]
    fn class(&self, splits: &[u8], b: usize, plane: usize) -> usize {
        if self.none(b) {
            2
        } else {
            (splits[b] >> plane & 1) as usize
        }
    }
}

/// Codes a unit's joint map (feature `small_transforms`): two header bits
/// (probability 1/2: the unit merges, the unit splits), the partition
/// symbols as [`encode`] does when it merges, then when it splits for every
/// parent in raster order that
/// is neither merged nor bypassed (`bypass(b)`: an exact or kept block) one
/// split flag per plane with AC (`ac[b]`, bit c; 1 = four 4x4 DCTs; a
/// plane without AC decodes the same either way and has no flag). `splits`
/// bits of planes without AC must be 0. Returns the map's bytes, empty when
/// nothing merges or splits.
#[cfg(feature = "encode")]
pub(crate) fn encode_joint(
    roles: &[u8],
    splits: &[u8],
    ac: &[u8],
    (rows, cols): (usize, usize),
    bypass: &dyn Fn(usize) -> bool,
) -> Vec<u8> {
    debug_assert_eq!(roles.len(), rows * cols);
    debug_assert_eq!(splits.len(), rows * cols);
    let merges = roles.iter().any(|&r| r != 0);
    let split = splits.iter().any(|&s| s != 0);
    if !merges && !split {
        return Vec::new();
    }
    let mut m = Model::new();
    let mut e = Encoder::new();
    // The unit header: which sections follow (fixed probability 1/2).
    e.bit(&mut (PROB_ONE / 2), merges);
    e.bit(&mut (PROB_ONE / 2), split);
    let mut grid = Grid::new(rows, cols).expect("encoder geometry");
    for (r, x, has) in map_groups(rows, cols).filter(|_| merges) {
        let (gr, gc) = (r / 2, x / 2);
        let ctx = grid.ctx(gr, gc);
        let s = group_symbol(roles, cols, r, x, has);
        m.write_symbol(&mut e, ctx, s);
        grid.sym[gr * grid.cols + gc] = s;
    }
    let f = Flagged { roles, bypass };
    let mut sp = [PROB_ONE / 2; SPLIT_CTX];
    for r in (0..rows).filter(|_| split) {
        for x in 0..cols {
            let b = r * cols + x;
            if f.none(b) {
                debug_assert_eq!(splits[b], 0);
                continue;
            }
            debug_assert_eq!(splits[b] & !ac[b], 0);
            let luma = (splits[b] & 1) as usize;
            for plane in 0..3 {
                if ac[b] >> plane & 1 == 0 {
                    continue;
                }
                let left = if x > 0 {
                    f.class(splits, b - 1, plane)
                } else {
                    2
                };
                let up = if r > 0 {
                    f.class(splits, b - cols, plane)
                } else {
                    2
                };
                let l = if plane == 0 { 0 } else { luma };
                e.bit(
                    &mut sp[split_ctx(plane, left, up, l)],
                    splits[b] >> plane & 1 != 0,
                );
            }
        }
    }
    e.finish()
}

/// Decodes a unit's joint map (`bytes`, not empty) into `roles` and
/// `splits` (one byte per parent of the `rows` x `cols` unit, all zero on
/// entry), flags as [`encode_joint`] codes them (`ac`, `bypass`). Returns
/// whether any group merges. Corrupt: as [`decode`], or a map that neither
/// merges nor splits.
pub(crate) fn decode_joint(
    bytes: &[u8],
    (rows, cols): (usize, usize),
    roles: &mut [u8],
    splits: &mut [u8],
    ac: &[u8],
    bypass: &dyn Fn(usize) -> bool,
) -> Result<bool, Error> {
    debug_assert!(!bytes.is_empty());
    debug_assert_eq!(roles.len(), rows * cols);
    debug_assert_eq!(splits.len(), rows * cols);
    let mut m = Model::new();
    let mut d = Decoder::new(bytes);
    let merges = d.bit(&mut (PROB_ONE / 2));
    let split = d.bit(&mut (PROB_ONE / 2));
    let merged = merges && symbols(&mut d, &mut m, rows, cols, roles)?;
    if merges != merged {
        return Err(Error::Corrupt(
            "partition map merges section without a merge",
        ));
    }
    let mut any_split = false;
    let f = Flagged { roles, bypass };
    let mut sp = [PROB_ONE / 2; SPLIT_CTX];
    for r in (0..rows).filter(|_| split) {
        for x in 0..cols {
            let b = r * cols + x;
            if f.none(b) {
                continue;
            }
            let mut v = 0u8;
            for plane in 0..3 {
                if ac[b] >> plane & 1 == 0 {
                    continue;
                }
                let left = if x > 0 {
                    f.class(splits, b - 1, plane)
                } else {
                    2
                };
                let up = if r > 0 {
                    f.class(splits, b - cols, plane)
                } else {
                    2
                };
                let l = if plane == 0 { 0 } else { (v & 1) as usize };
                v |= (d.bit(&mut sp[split_ctx(plane, left, up, l)]) as u8) << plane;
            }
            splits[b] = v;
            any_split |= v != 0;
        }
    }
    if split != any_split {
        return Err(Error::Corrupt(
            "partition map splits section without a split",
        ));
    }
    if !merged && !any_split {
        return Err(Error::Corrupt("partition map without a merge or split"));
    }
    if d.unread() != 0 {
        return Err(Error::Corrupt("partition map has unread bytes"));
    }
    Ok(merged)
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    fn xorshift(seed: &mut u32) -> u32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        *seed
    }

    #[test]
    fn range_coder_round_trips_random_bits_with_skewed_probabilities() {
        let mut seed = 0x1234_5678u32;
        for n in [0usize, 1, 2, 7, 100, 5000] {
            for skew in [1u32, 4, 50] {
                let bits: Vec<(usize, bool)> = (0..n)
                    .map(|_| {
                        let c = (xorshift(&mut seed) % 3) as usize;
                        (c, xorshift(&mut seed).is_multiple_of(skew))
                    })
                    .collect();
                let mut p = [PROB_ONE / 2; 3];
                let mut e = Encoder::new();
                for &(c, b) in &bits {
                    e.bit(&mut p[c], b);
                }
                let out = e.finish();
                let mut p = [PROB_ONE / 2; 3];
                let mut d = Decoder::new(&out);
                for (i, &(c, b)) in bits.iter().enumerate() {
                    assert_eq!(d.bit(&mut p[c]), b, "n {n} skew {skew} bit {i}");
                }
                assert_eq!(d.unread(), 0, "n {n} skew {skew}");
            }
        }
    }

    /// Random aligned maps of a unit, about one group in `1 + skew` merged
    /// (skew 0: every group random).
    fn random_roles(rows: usize, cols: usize, skew: u32, seed: &mut u32) -> Vec<u8> {
        let mut roles = alloc::vec![0u8; rows * cols];
        for (r, x, has) in map_groups(rows, cols) {
            let fits: Vec<usize> = (0..8)
                .filter(|&s| (0..4).all(|i| has[i] || PARTITIONS[s][i] == 0))
                .collect();
            let s = if skew == 0 || xorshift(seed).is_multiple_of(skew + 1) {
                fits[xorshift(seed) as usize % fits.len()]
            } else {
                0
            };
            for i in 0..4 {
                if has[i] {
                    roles[(r + i / 2) * cols + x + i % 2] = PARTITIONS[s][i];
                }
            }
        }
        roles
    }

    #[test]
    fn maps_round_trip() {
        let mut seed = 0x9e37_79b9u32;
        for (rows, cols) in [
            (1, 2),
            (1, 7),
            (2, 1),
            (2, 2),
            (3, 5),
            (4, 17),
            (8, 1),
            (5, 6),
            (8, 40),
            (64, 3),
        ] {
            for skew in [0u32, 1, 3, 8] {
                let roles = random_roles(rows, cols, skew, &mut seed);
                let bytes = encode(&roles, rows, cols);
                if roles.iter().all(|&r| r == 0) {
                    assert!(bytes.is_empty());
                    continue;
                }
                assert!(!bytes.is_empty());
                assert_ne!(bytes.last(), Some(&0));
                let mut back = alloc::vec![0u8; rows * cols];
                decode(&bytes, rows, cols, &mut back).unwrap();
                assert_eq!(back, roles, "{rows}x{cols} skew {skew}");
                // A trailing zero byte is read as the zeros past the end
                // would be; a further nonzero byte past what the decoding
                // reads is corrupt.
                let mut padded = bytes.clone();
                padded.extend_from_slice(&[0; 8]);
                let mut back = alloc::vec![0u8; rows * cols];
                assert!(decode(&padded, rows, cols, &mut back).is_err());
            }
        }
    }

    #[test]
    fn maps_reject_merges_past_the_unit_and_empty_merges() {
        // A 1 x 3 unit: groups (0, 0) with parents TL, TR and (0, 2) with
        // TL only (no symbol). Symbol 1 (16x16) needs absent parents.
        let mut e = Encoder::new();
        let mut m = Model::new();
        e.bit(&mut m.any[0], true);
        let mut node = 1;
        for b in (0..3).rev() {
            let bit = 0u8 >> b & 1 != 0;
            e.bit(&mut m.tree[node][0], bit);
            node = node * 2 + bit as usize;
        }
        let bytes = e.finish();
        let mut roles = [0u8; 3];
        assert_eq!(
            decode(&bytes, 1, 3, &mut roles),
            Err(Error::Corrupt("superblock past its unit"))
        );
        // A map whose only symbol is 0.
        let mut e = Encoder::new();
        let mut m = Model::new();
        e.bit(&mut m.any[0], false);
        let mut bytes = e.finish();
        if bytes.is_empty() {
            bytes.push(0);
        }
        let mut roles = [0u8; 4];
        assert_eq!(
            decode(&bytes, 2, 2, &mut roles),
            Err(Error::Corrupt("superblock map without a merge"))
        );
    }
}

/// V4's joint settings stream. The merge bit and optional partition symbols
/// precede the parent flags, with a single range endpoint for the group.
pub(crate) mod joint {
    #[cfg(feature = "encode")]
    use super::Encoder;
    use super::{Decoder, PARTITIONS, PROB_ONE, read_v4_partition_symbols};
    use crate::Error;
    #[cfg(feature = "encode")]
    use alloc::vec::Vec;

    struct ParentModel {
        nonzero: [u16; 4],
        plane: [u16; 24],
    }

    impl ParentModel {
        fn new() -> Self {
            Self {
                nonzero: [PROB_ONE / 2; 4],
                plane: [PROB_ONE / 2; 24],
            }
        }

        #[inline]
        fn mask(
            &mut self,
            left: u8,
            above: u8,
            mut bit: impl FnMut(&mut u16, usize) -> bool,
        ) -> u8 {
            let context = 2 * usize::from(left != 0) + usize::from(above != 0);
            // Index three identifies the presence decision to the encoder.
            if !bit(&mut self.nonzero[context], 3) {
                return 0;
            }
            let mut mask = 0;
            for plane in 0..3 {
                if plane == 2 && mask == 0 {
                    return 4;
                }
                let luma = usize::from(plane != 0 && mask & 1 != 0);
                let context = 8 * plane
                    + 4 * usize::from(left >> plane & 1)
                    + 2 * usize::from(above >> plane & 1)
                    + luma;
                mask |= u8::from(bit(&mut self.plane[context], plane)) << plane;
            }
            mask
        }
    }

    // One merge-presence decision, at most four decisions per cell and
    // four per parent. The probability floor allows at most one byte of
    // renormalization per decision. The endpoint contributes four bytes.
    pub(crate) const fn max_bytes(cells: u128, parents: u128) -> u128 {
        4 * (cells + parents) + 5
    }

    #[inline]
    fn merged(partition: u8, y: usize, x: usize) -> bool {
        PARTITIONS[partition as usize][2 * (y & 1) + (x & 1)] != 0
    }

    #[cfg(feature = "encode")]
    pub(crate) fn encode(
        rows: usize,
        columns: usize,
        partition: impl Fn(usize) -> u8,
        flag: impl Fn(usize, usize) -> u8,
    ) -> Result<Option<Vec<u8>>, Error> {
        let cell_columns = columns.div_ceil(2);
        let cells = rows.div_ceil(2) as u128 * cell_columns as u128;
        let parents = rows as u128 * columns as u128;
        let bound = usize::try_from(max_bytes(cells, parents) + 1).map_err(|_| Error::TooLarge)?;
        let mut encoder = Encoder::new();
        encoder
            .out
            .try_reserve_exact(bound)
            .map_err(|_| Error::TooLarge)?;
        let has_merges = (0..cells as usize).any(|i| partition(i) != 0);
        encoder.bit(&mut (PROB_ONE / 2), has_merges);
        if has_merges {
            super::write_v4_partition_symbols(&mut encoder, rows, columns, &partition);
        }
        let mut model = ParentModel::new();
        let mut any = false;
        for y in 0..rows {
            for x in 0..columns {
                if merged(partition(y / 2 * cell_columns + x / 2), y, x) {
                    continue;
                }
                let mask = flag(y, x);
                any |= mask != 0;
                let left =
                    if x == 0 || merged(partition(y / 2 * cell_columns + (x - 1) / 2), y, x - 1) {
                        0
                    } else {
                        flag(y, x - 1)
                    };
                let above =
                    if y == 0 || merged(partition((y - 1) / 2 * cell_columns + x / 2), y - 1, x) {
                        0
                    } else {
                        flag(y - 1, x)
                    };
                let recovered = model.mask(left, above, |probability, plane| {
                    let set = if plane == 3 {
                        mask != 0
                    } else {
                        mask >> plane & 1 != 0
                    };
                    encoder.bit(probability, set);
                    set
                });
                debug_assert_eq!(recovered, mask);
            }
        }
        Ok(any.then(|| encoder.flush()))
    }

    pub(crate) fn decode(
        bytes: &[u8],
        rows: usize,
        columns: usize,
        allow_merges: bool,
        partitions: &mut [u8],
        flags: &mut [u8],
        mut poll: impl FnMut() -> Result<(), Error>,
    ) -> Result<(), Error> {
        if bytes.len() < 4
            || bytes.len() as u128 > max_bytes(partitions.len() as u128, flags.len() as u128)
        {
            return Err(Error::Corrupt("V4 joint settings range length"));
        }
        let mut decoder = Decoder::new(bytes);
        if decoder.bit(&mut (PROB_ONE / 2)) {
            if !allow_merges {
                return Err(Error::Corrupt("V4 joint settings merge without capability"));
            }
            read_v4_partition_symbols(&mut decoder, rows, columns, partitions, &mut poll)?;
        }
        let cell_columns = columns.div_ceil(2);
        let mut model = ParentModel::new();
        let mut any = false;
        for y in 0..rows {
            for x in 0..columns {
                if x.is_multiple_of(32) {
                    poll()?;
                }
                if merged(partitions[y / 2 * cell_columns + x / 2], y, x) {
                    continue;
                }
                let index = y * columns + x;
                let left = if x == 0 { 0 } else { flags[index - 1] };
                let above = if y == 0 { 0 } else { flags[index - columns] };
                let mask = model.mask(left, above, |probability, _| decoder.bit(probability));
                if decoder.pos > bytes.len() {
                    return Err(Error::Corrupt("V4 truncated joint settings"));
                }
                flags[index] = mask;
                any |= mask != 0;
            }
        }
        if !any || decoder.pos != bytes.len() || decoder.code != 0 {
            return Err(Error::Corrupt("V4 joint settings range termination"));
        }
        Ok(())
    }
}
