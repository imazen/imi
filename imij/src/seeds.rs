//! Built-in seed tables for entropy coding.
//!
//! The entropy tables a lossy file carries (the AC tables of the lossy head,
//! and the slot tables of the nested DC and alpha images) may each be replaced
//! by a seed: a table built into every decoder, named by index, with no table
//! bytes in the file (docs/SUBBLOCKS_AND_COMPACT_STREAMS.md, "Seed tables").
//! This module holds the table kinds, bank 1 and its encoder costs, and, under
//! `lab` + `std`, a per-thread sink that records the histogram and explicit
//! cost of every table an encode builds (bank training data, head statistics).

/// Table kinds. A seed of one kind is only ever used for tables of that kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Kind {
    /// Run-level end-of-block tables (rANS, symbols 0..64).
    RlEob = 0,
    /// Run-level AC token tables (rANS, run * 32 + class, ZRL).
    RlAc = 1,
    /// Split-Huffman count tables (code lengths, 17 symbols).
    HufCount = 4,
    /// Split-Huffman width tables (code lengths, 23 symbols).
    HufWidth = 5,
    /// Lossless slot / palette-index tables (rANS; the token scheme
    /// (k, m, depth) fixes the alphabet).
    Lossless = 6,
}

impl Kind {
    /// Kind of a lossy rANS table (`first` = the EOB group); the retired
    /// rANS modes' tables have no seeds and split-Huffman's are Huffman
    /// kinds.
    pub(crate) fn of_lossy(mode: crate::lossy::LossyEntropy, first: bool) -> Option<Kind> {
        match mode {
            crate::lossy::LossyEntropy::RunLevel if first => Some(Kind::RlEob),
            crate::lossy::LossyEntropy::RunLevel => Some(Kind::RlAc),
            _ => None,
        }
    }
}

/// Where a table lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
#[cfg_attr(not(all(feature = "lab", feature = "std")), allow(dead_code))]
pub(crate) enum Role {
    /// The lossy head's AC tables.
    Ac = 0,
    /// The nested DC image of a lossy file.
    Dc = 1,
    /// The nested alpha image of a lossy file.
    Alpha = 2,
    /// A standalone lossless image.
    Lossless = 3,
    /// The nested exact-block image of a lossy file (explicit tables).
    Exact = 4,
}

#[cfg(all(feature = "lab", feature = "std"))]
pub use sink::{Captured, GroupRec, SeedRec, TableRec, capture};

#[cfg(all(feature = "lab", feature = "std"))]
pub(crate) use sink::{record_group, record_seed, record_table, with_role};

/// Without the lab sink: no recording.
#[cfg(not(all(feature = "lab", feature = "std")))]
mod nosink {
    use super::Kind;
    #[cfg(feature = "encode")]
    use super::Role;
    #[inline(always)]
    #[cfg(feature = "encode")]
    pub(crate) fn with_role<R>(_: Role, f: impl FnOnce() -> R) -> R {
        f()
    }
    #[inline(always)]
    pub(crate) fn record_table(_: Kind, _: [u8; 3], _: &[u64], _: impl FnOnce() -> (u64, f64)) {}
    #[inline(always)]
    #[cfg(feature = "encode")]
    pub(crate) fn record_group(_: u8, _: impl FnOnce() -> (u64, u64, u32)) {}
    #[inline(always)]
    pub(crate) fn record_seed(_: Kind, _: [u8; 3], _: u8, _: f64) {}
}
#[cfg(all(feature = "encode", not(all(feature = "lab", feature = "std"))))]
pub(crate) use nosink::{record_group, with_role};
#[cfg(not(all(feature = "lab", feature = "std")))]
pub(crate) use nosink::{record_seed, record_table};

#[cfg(all(feature = "lab", feature = "std"))]
mod sink {
    use super::{Kind, Role};
    use std::cell::{Cell, RefCell};
    use std::vec::Vec;

    /// One entropy table an encode built.
    #[derive(Clone, Debug)]
    pub struct TableRec {
        pub role: u8,
        pub kind: u8,
        /// Lossless token scheme (k, m, depth); zero for lossy kinds.
        pub tc: [u8; 3],
        /// Symbol counts the table codes.
        pub counts: Vec<u64>,
        /// Bits of the explicit table as written.
        pub table_bits: u64,
        /// Ideal coded bits of `counts` under the explicit table.
        pub data_bits: f64,
    }

    /// One table blob (all tables of a role, plus their context maps).
    #[derive(Clone, Debug)]
    pub struct GroupRec {
        pub role: u8,
        /// Entropy mode (lossy) or 255 (lossless models).
        pub mode: u8,
        /// Bits of the whole blob before byte padding.
        pub bits: u64,
        /// Of which explicit table bits.
        pub table_bits: u64,
        pub tables: u32,
    }

    /// A table an encode replaced by a seed.
    #[derive(Clone, Debug)]
    pub struct SeedRec {
        pub role: u8,
        pub kind: u8,
        pub tc: [u8; 3],
        pub seed: u8,
        /// Estimated bits saved (explicit - seed - index bits).
        pub gain: f64,
    }

    /// What an encode recorded.
    #[derive(Default)]
    pub struct Captured {
        pub tables: Vec<TableRec>,
        pub groups: Vec<GroupRec>,
        pub seeds: Vec<SeedRec>,
    }

    type Sink = Captured;

    std::thread_local! {
        static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
        static ROLE: Cell<u8> = const { Cell::new(Role::Lossless as u8) };
    }

    /// Runs `f` with this thread's sink enabled and returns what it
    /// recorded. Encoders must run on this thread (`threads = 1`).
    pub fn capture<R>(f: impl FnOnce() -> R) -> (R, Captured) {
        SINK.with(|s| *s.borrow_mut() = Some(Sink::default()));
        let r = f();
        let s = SINK.with(|s| s.borrow_mut().take()).unwrap_or_default();
        (r, s)
    }

    /// Runs `f` with lossless tables attributed to `role`.
    pub(crate) fn with_role<R>(role: Role, f: impl FnOnce() -> R) -> R {
        let old = ROLE.with(|r| r.replace(role as u8));
        let out = f();
        ROLE.with(|r| r.set(old));
        out
    }

    fn enabled() -> bool {
        SINK.with(|s| s.borrow().is_some())
    }

    /// Records a table; lossless kinds take the current role.
    pub(crate) fn record_table(
        kind: Kind,
        tc: [u8; 3],
        counts: &[u64],
        cost: impl FnOnce() -> (u64, f64),
    ) {
        if !enabled() {
            return;
        }
        let (table_bits, data_bits) = cost();
        let role = if kind == Kind::Lossless {
            ROLE.with(|r| r.get())
        } else {
            Role::Ac as u8
        };
        SINK.with(|s| {
            if let Some(s) = s.borrow_mut().as_mut() {
                s.tables.push(TableRec {
                    role,
                    kind: kind as u8,
                    tc,
                    counts: counts.to_vec(),
                    table_bits,
                    data_bits,
                });
            }
        });
    }

    /// Records a seed an encode chose; lossless kinds take the current role.
    pub(crate) fn record_seed(kind: Kind, tc: [u8; 3], seed: u8, gain: f64) {
        if !enabled() {
            return;
        }
        let role = if kind == Kind::Lossless {
            ROLE.with(|r| r.get())
        } else {
            Role::Ac as u8
        };
        SINK.with(|s| {
            if let Some(s) = s.borrow_mut().as_mut() {
                s.seeds.push(SeedRec {
                    role,
                    kind: kind as u8,
                    tc,
                    seed,
                    gain,
                });
            }
        });
    }

    /// Records a written table blob, `(bits, table bits, tables)`; `mode`
    /// 255 = lossless models.
    pub(crate) fn record_group(mode: u8, stats: impl FnOnce() -> (u64, u64, u32)) {
        if !enabled() {
            return;
        }
        let (bits, table_bits, tables) = stats();
        let role = if mode == 255 {
            ROLE.with(|r| r.get())
        } else {
            Role::Ac as u8
        };
        SINK.with(|s| {
            if let Some(s) = s.borrow_mut().as_mut() {
                s.groups.push(GroupRec {
                    role,
                    mode,
                    bits,
                    table_bits,
                    tables,
                });
            }
        });
    }
}

/// Ideal bits of `counts` coded with rANS frequencies `freq` (sum `M`);
/// `None` when a counted symbol has no frequency.
pub(crate) fn rans_data_bits(counts: &[u64], freq: &[u32]) -> Option<f64> {
    let mut bits = 0.0;
    for (i, &c) in counts.iter().enumerate() {
        if c == 0 {
            continue;
        }
        let f = *freq.get(i)?;
        if f == 0 {
            return None;
        }
        bits += c as f64 * (crate::rans::LOG_M as f64 - crate::mathx::log2(f as f64));
    }
    Some(bits)
}

// ---- seed banks -------------------------------------------------------------
//
// New lossy heads select a varint bank ID and scope mask (presence bit 9,
// feature seed_bank_ids). Internal scope constants retain bits 4..6; wire
// scopes use bits 0..2. Legacy presence bit 0 is read as a packed bank/scope
// byte under feature seed_bank_1. A published bank's contents are immutable.
// In a table blob with selectors, every table is preceded by one bit: 0 =
// the explicit table follows as before, 1 = an index of `index_bits` bits
// (ceil(log2(count)) for the bank's seeds of that kind) names the seed, and
// no table bytes follow. A seed is used exactly as an explicit table with
// the same contents.

use alloc::vec::Vec;
use core::ops::Range;

use crate::Error;
use crate::bits::BitReader;
#[cfg(feature = "encode")]
use crate::bits::BitWriter;

mod costs;

/// Seed-byte scope bits.
pub(crate) const SCOPE_AC: u8 = 1 << 4;
pub(crate) const SCOPE_DC: u8 = 1 << 5;
pub(crate) const SCOPE_ALPHA: u8 = 1 << 6;

/// File seed selection. The identifier is separate from table-scope flags.
/// Legacy packed fields remain readable/remuxable; new encoders use varints.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Selection {
    pub bank: u64,
    pub scope: u8,
    pub legacy: bool,
}

impl Selection {
    pub const NONE: Self = Self {
        bank: 0,
        scope: 0,
        legacy: false,
    };

    pub const fn new(bank: u64, scope: u8) -> Self {
        Self {
            bank,
            scope,
            legacy: false,
        }
    }

    pub const fn legacy(byte: u8) -> Self {
        Self {
            bank: (byte & 15) as u64,
            scope: byte & 0x70,
            legacy: true,
        }
    }

    #[cfg(feature = "encode")]
    pub const fn is_some(self) -> bool {
        self.scope != 0
    }

    pub fn scoped(self, flag: u8) -> Option<&'static Bank> {
        bank(self.bank).filter(|_| self.scope & flag != 0)
    }
}

/// Smallest saving (bits, after selector bits) for which a table blob turns
/// selectors on. Final complete-file comparison charges actual bank/scope
/// varints and feature/presence framing.
pub(crate) const MIN_GAIN_BITS: f64 = 8.0;

/// Bank blob layout (every integer little-endian; the blob is 4-byte
/// aligned data whose length is a multiple of 4), `imij/src/seeds/bank1.bin`:
///
/// ```text
/// magic     4 bytes  "IMSB"
/// version   u8       1 (this layout)
/// bank      u8       bank id, 1..=15 (the seed byte's bits 0-3)
/// kinds     u8       K
/// reserved  u8       0
/// K directory entries, 3 x u32 each, in increasing (kind, k, m, depth):
///   bytes 0-3   kind, k, m, depth: kind 0 run-level EOB, 1 run-level AC,
///               4 split-Huffman count, 5 split-Huffman width, 6 lossless
///               (k, m, depth its token scheme; 0, 0, 0 for other kinds)
///   u32         n seeds (1..=256)
///   u32         byte offset of the kind's section (a multiple of 4)
/// sections: (n + 1) u32 offsets relative to the end of the offset list,
///   increasing; seed i is bytes [off_i, off_i+1); the section is zero-padded
///   to a multiple of 4 bytes.
/// ```
///
/// A seed is one table in its kind's own grammar, starting byte-aligned:
/// rANS kinds the weights of SPEC 3.3 (`last`, `prec - 1`, exponents and
/// mantissas) for the kind's alphabet (128 symbols for run-level, the token
/// scheme's for lossless), split-Huffman kinds 4 bits per code length (17 or
/// 23 symbols), LSB-first. Decoders view the blob in place as 4-byte
/// little-endian words (no copy, no parse ahead of use) and decode only the
/// seeds a file selects.
pub(crate) struct Bank {
    pub id: u64,
    header_id: u8,
    /// The blob as words (`as_chunks` once, at compile time).
    words: &'static [[u8; 4]],
}

const MAGIC: [u8; 4] = *b"IMSB";
const LAYOUT: u8 = 1;

/// A bank blob as words; the build fails unless its length is a multiple
/// of 4.
const fn words(blob: &'static [u8]) -> &'static [[u8; 4]] {
    let (words, rest) = blob.as_chunks::<4>();
    assert!(rest.is_empty(), "bank blob length is not a multiple of 4");
    words
}

impl Bank {
    /// Header check: magic, layout version and bank id.
    fn valid(&self) -> bool {
        matches!(self.words, [magic, [layout, id, ..], ..]
            if *magic == MAGIC && *layout == LAYOUT && *id == self.header_id)
    }

    /// Directory entries: kind and token scheme, seed count, section byte
    /// offset. Bounded by the blob, and the first section must start right
    /// after the directory (a wrong kind count gives no entries).
    fn directory(&self) -> &'static [[[u8; 4]; 3]] {
        let kinds = self.words.get(1).map_or(0, |w| w[2] as usize);
        let end = 2 + 3 * kinds;
        match self.words.get(2..end).map(|d| d.as_chunks::<3>().0) {
            Some(d @ [[_, _, first], ..]) if u32::from_le_bytes(*first) as usize == 4 * end => d,
            _ => &[],
        }
    }

    /// The seeds of `kind` / `tc`, if the bank has any.
    pub(crate) fn kind(&self, kind: Kind, tc: [u8; 3]) -> Option<KindSeeds> {
        self.find(kind, tc).map(|(s, _)| s)
    }

    /// The seeds of `kind` / `tc` and where their encoder costs are in the
    /// bank's cost table. A directory entry outside 1..=256 seeds, or a
    /// section outside the blob, has no seeds.
    fn find(&self, kind: Kind, tc: [u8; 3]) -> Option<(KindSeeds, Range<usize>)> {
        let mut cost_at = 0usize;
        for &[key, n, at] in self.directory() {
            let (k, t) = (key[0], [key[1], key[2], key[3]]);
            let n = u32::from_le_bytes(n) as usize;
            let cost_len = n.checked_mul(costs::width(k, t))?;
            if k != kind as u8 || t != tc {
                cost_at = cost_at.checked_add(cost_len)?;
                continue;
            }
            // Checked once per kind: count, alignment, offsets inside the blob.
            let at = u32::from_le_bytes(at) as usize;
            if !(1..=256).contains(&n) || !at.is_multiple_of(4) {
                return None;
            }
            let section = self.words.get(at / 4..)?;
            let seeds = KindSeeds {
                kind,
                offs: section.get(..n + 1)?,
                data: section.get(n + 1..)?.as_flattened(),
            };
            return Some((seeds, cost_at..cost_at.checked_add(cost_len)?));
        }
        None
    }
}

/// A bank's seeds of one kind (and, for lossless tables, one token scheme).
#[derive(Clone, Copy)]
pub(crate) struct KindSeeds {
    pub kind: Kind,
    /// n + 1 increasing byte offsets into `data`.
    offs: &'static [[u8; 4]],
    data: &'static [u8],
}

impl KindSeeds {
    pub(crate) fn len(&self) -> usize {
        self.offs.len() - 1
    }

    /// Bits of a seed index.
    pub(crate) fn index_bits(&self) -> u32 {
        let n = self.len() as u32;
        if n <= 1 {
            0
        } else {
            32 - (n - 1).leading_zeros()
        }
    }

    /// Seed `i`'s bytes (its offsets checked once, here).
    fn bytes(&self, i: usize) -> Result<&'static [u8], Error> {
        let bad = Error::Corrupt("seed bank layout");
        let Some(&[a, b]) = self.offs.get(i..i.wrapping_add(2)) else {
            return Err(bad);
        };
        let (a, b) = (
            u32::from_le_bytes(a) as usize,
            u32::from_le_bytes(b) as usize,
        );
        self.data.get(a..b).ok_or(bad)
    }

    /// rANS kinds: the seed's weights (alphabet `alpha`) and precision.
    pub(crate) fn weights(&self, i: usize, alpha: usize) -> Result<(Vec<u32>, u32), Error> {
        crate::rans::read_weights(&mut BitReader::new(self.bytes(i)?), alpha)
    }

    /// Huffman kinds: the seed's code lengths.
    pub(crate) fn lengths(&self, i: usize, alpha: usize) -> Result<Vec<u8>, Error> {
        let mut r = BitReader::new(self.bytes(i)?);
        Ok((0..alpha).map(|_| r.read_refill(4) as u8).collect())
    }
}

/// Bank 1 (trained 2026-10-07; benchmarks/entropy_seeds_2026-10-07).
const BANK1_BYTES: &[u8] = include_bytes!("seeds/bank1.bin");
// Decodes every seed at compile time: long, by design.
#[allow(long_running_const_eval)]
static BANK1_COSTS: [u8; costs::len(BANK1_BYTES)] = costs::table(BANK1_BYTES);
static BANK1: Bank = Bank {
    id: 1,
    header_id: 1,
    words: words(BANK1_BYTES),
};

/// Encoder costs of every seed of bank `id`, kinds in directory order
/// (`costs.rs`). Only the encoder reaches this, so decoders do not link the
/// table.
fn bank_costs(id: u64) -> &'static [u8] {
    match id {
        1 => &BANK1_COSTS,
        #[cfg(all(feature = "lab", feature = "std"))]
        _ => lab_banks::BANKS.get(id).map_or(&[], |b| b.1),
        #[cfg(not(all(feature = "lab", feature = "std")))]
        _ => &[],
    }
}

/// The bank with id `id`, if this build has it (and its header is valid).
pub(crate) fn bank(id: u64) -> Option<&'static Bank> {
    if id == BANK1.id {
        return BANK1.valid().then_some(&BANK1);
    }
    #[cfg(all(feature = "lab", feature = "std"))]
    if let Some((b, _, _)) = lab_banks::BANKS.get(id) {
        return Some(b);
    }
    None
}

#[cfg(all(feature = "encode", feature = "lab"))]
pub(crate) fn validate_id(id: u64) -> Result<(), Error> {
    if id != 0 && bank(id).is_none() {
        return Err(Error::Unsupported("unknown seed bank"));
    }
    Ok(())
}

/// Largest cost table a lab-registered bank may need (bank 1's is 94,208
/// bytes; a 255-seed (5, 2, 8) lossless bank's 38,760).
#[cfg(all(feature = "lab", feature = "std"))]
const LAB_MAX_COSTS: usize = 1 << 22;

/// Checks a bank blob, borrowed, before anything is allocated or leaked for
/// it: header (magic, layout, `id`, reserved byte), a directory of known
/// kinds in increasing (kind, k, m, depth) order with valid token schemes,
/// 1..=256 seeds each, contiguous 4-byte aligned sections in directory
/// order whose offsets increase and stay inside the section, zero padding,
/// nothing after the last section. Returns the length of the bank's
/// encoder cost table (checked arithmetic, at most `LAB_MAX_COSTS`).
#[cfg(all(feature = "lab", feature = "std"))]
fn check_blob(words: &[[u8; 4]], id: u8) -> Result<usize, Error> {
    let bad = Error::InvalidInput("bad seed bank blob");
    let [magic, [layout, bid, kinds, reserved], ..] = words else {
        return Err(bad);
    };
    if *magic != MAGIC || *layout != LAYOUT || *bid != id || *reserved != 0 || *kinds == 0 {
        return Err(bad);
    }
    let end = 2 + 3 * *kinds as usize;
    let dir = words.get(2..end).ok_or(bad.clone())?.as_chunks::<3>().0;
    let (mut at, mut cost_len, mut prev) = (4 * end, 0usize, None);
    for &[key, n, off] in dir {
        let (kind, tc) = (key[0], [key[1], key[2], key[3]]);
        let ok_kind = match kind {
            0 | 1 | 4 | 5 => tc == [0; 3],
            6 => {
                let (k, m, depth) = (tc[0] as u32, tc[1] as u32, tc[2] as u32);
                let t = crate::tokens::TokenCfg::v1(k, m, depth);
                m <= 4
                    && k <= 16
                    && (1..=32).contains(&depth)
                    && t.k == k
                    && t.alphabet() <= 256
                    && costs::width(kind, tc) == t.alphabet() as usize
            }
            _ => false,
        };
        if !ok_kind || prev.is_some_and(|p| p >= key) {
            return Err(bad);
        }
        prev = Some(key);
        let (n, off) = (
            u32::from_le_bytes(n) as usize,
            u32::from_le_bytes(off) as usize,
        );
        if !(1..=256).contains(&n) || off != at {
            return Err(bad);
        }
        let offs = words.get(at / 4..at / 4 + n + 1).ok_or(bad.clone())?;
        let data = at + 4 * (n + 1);
        let mut last = 0;
        for (i, w) in offs.iter().enumerate() {
            let o = u32::from_le_bytes(*w) as usize;
            if (i == 0 && o != 0) || (i > 0 && o <= last) {
                return Err(bad);
            }
            last = o;
        }
        let sec_end = data.checked_add(last).ok_or(bad.clone())?;
        let padded = sec_end.checked_next_multiple_of(4).ok_or(bad.clone())?;
        let bytes = words.as_flattened();
        if padded > bytes.len() || bytes[sec_end..padded].iter().any(|&b| b != 0) {
            return Err(bad);
        }
        at = padded;
        cost_len = n
            .checked_mul(costs::width(kind, tc))
            .and_then(|c| cost_len.checked_add(c))
            .filter(|&c| c <= LAB_MAX_COSTS)
            .ok_or(bad.clone())?;
    }
    if at != 4 * words.len() {
        return Err(bad);
    }
    Ok(cost_len)
}

/// A v2 registry envelope carries a full-width ID and an unchanged v1 table
/// asset. The table directory remains relative to that inner asset.
#[cfg(all(feature = "lab", feature = "std"))]
fn blob_parts(blob: &[u8]) -> Result<(u64, &[u8]), Error> {
    let bad = Error::InvalidInput("bad seed bank blob");
    if blob.get(..4) != Some(&MAGIC) {
        return Err(bad);
    }
    match blob.get(4) {
        Some(1) => {
            let id = *blob.get(5).ok_or(bad.clone())?;
            if !(2..=15).contains(&id) {
                return Err(bad);
            }
            Ok((u64::from(id), blob))
        }
        Some(2) => {
            let mut r = crate::ByteReader::new(blob);
            r.pos = 5;
            let id = r.varint().map_err(|_| bad.clone())?;
            let n = r.var_usize().map_err(|_| bad.clone())?;
            let inner = r.bytes(n).map_err(|_| bad.clone())?;
            if r.remaining() != 0 || inner.get(..5) != Some(&b"IMSB\x01"[..]) {
                return Err(bad);
            }
            Ok((id, inner))
        }
        _ => Err(bad),
    }
}

/// Lab: banks loaded at run time (the experimental standalone lossless
/// bank), for this process's encoder and decoder alike.
#[cfg(all(feature = "lab", feature = "std"))]
mod lab_banks {
    use super::{Bank, costs};
    use std::sync::Mutex;
    use std::vec::Vec;

    type Entry = (&'static Bank, &'static [u8], &'static [u8]);

    /// Full-width IDs register once. The registry retains at most 14 banks
    /// and cost tables; resource capacity is independent of ID magnitude.
    pub(super) struct Registry(Mutex<Vec<Entry>>);

    /// The process's registry: `seeds::register_bank` fills it, `bank` and
    /// `bank_costs` read it.
    pub(super) static BANKS: Registry = Registry::new();

    impl Registry {
        pub(super) const fn new() -> Self {
            Self(Mutex::new(Vec::new()))
        }

        pub(super) fn get(&self, id: u64) -> Option<Entry> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .find(|e| e.0.id == id)
                .copied()
        }

        /// Registers `blob` under its logical bank ID: checked
        /// while borrowed, refused if that id is registered already, leaked
        /// only once accepted.
        pub(super) fn register(&self, blob: Vec<u8>) -> Result<u64, crate::Error> {
            let bad = crate::Error::InvalidInput("bad seed bank blob");
            let (id, inner) = super::blob_parts(&blob)?;
            let (words, rest) = inner.as_chunks::<4>();
            if id < 2 || !rest.is_empty() {
                return Err(bad);
            }
            let header_id = *inner.get(5).ok_or(bad.clone())?;
            if !(1..=15).contains(&header_id) {
                return Err(bad);
            }
            let offset = blob.len() - inner.len();
            let cost_len = super::check_blob(words, header_id)?;
            let mut banks = self.0.lock().unwrap();
            if banks.iter().any(|e| e.0.id == id) {
                return Err(crate::Error::InvalidInput(
                    "seed bank id already registered",
                ));
            }
            if banks.len() >= 14 {
                return Err(crate::Error::TooLarge);
            }
            let mut c = std::vec![costs::ABSENT; cost_len];
            costs::fill(inner, &mut c);
            let bytes: &'static [u8] = Vec::leak(blob);
            let words = bytes[offset..].as_chunks::<4>().0;
            let bank: &'static Bank = Box::leak(Box::new(Bank {
                id,
                header_id,
                words,
            }));
            banks.push((bank, Vec::leak(c), bytes));
            Ok(id)
        }
    }
}

/// Lab: registers a v1 asset (IDs 2..=15) or v2 envelope (full-width ID),
/// once per ID per process; returns its logical ID. A malformed blob or
/// an id registered already is `InvalidInput`.
#[cfg(all(feature = "lab", feature = "std"))]
pub fn register_bank(blob: Vec<u8>) -> Result<u64, Error> {
    lab_banks::BANKS.register(blob)
}

/// The bank the encoder seeds from by default.
#[cfg(feature = "encode")]
pub(crate) const DEFAULT_BANK: u64 = 1;

/// Reads a table selector: `Some(i)` = seed `i` of `seeds`, `None` = an
/// explicit table follows.
pub(crate) fn read_selector(
    r: &mut BitReader<'_>,
    seeds: Option<KindSeeds>,
) -> Result<Option<usize>, Error> {
    if r.read_refill(1) == 0 {
        return Ok(None);
    }
    let s = seeds.ok_or(Error::Corrupt("seed for a table kind without seeds"))?;
    let i = r.read_refill(s.index_bits()) as usize;
    if i >= s.len() {
        return Err(Error::Corrupt("seed index out of range"));
    }
    Ok(Some(i))
}

/// Writes a table selector; an explicit table must follow a `None`.
#[cfg(feature = "encode")]
pub(crate) fn write_selector(w: &mut BitWriter, seeds: Option<KindSeeds>, seed: Option<u8>) {
    match seed {
        None => w.put(0, 1),
        Some(i) => {
            w.put(1, 1);
            w.put(i as u32, seeds.unwrap().index_bits());
        }
    }
}

/// A seed as the table it stands for: weights, precision and frequencies
/// (rANS) or code lengths (Huffman).
pub(crate) struct Expanded {
    pub weights: Vec<u32>,
    pub prec: u32,
    pub freq: Vec<u32>,
    // Retained in the shared parsed representation for wire re-emission.
    #[cfg_attr(not(feature = "encode"), allow(dead_code))]
    pub lens: Vec<u8>,
}

/// The seeds of a kind with their encoder costs (a view into static data).
pub(crate) struct KindSet {
    pub seeds: KindSeeds,
    alpha: usize,
    /// Costs of `width` symbols per seed, sixteenths of a bit, 255 = the
    /// seed cannot code the symbol.
    cost: &'static [u8],
    width: usize,
}

impl KindSet {
    /// The cheapest seed for `counts`: (index, estimated data bits).
    pub(crate) fn best(&self, counts: &[u64]) -> Option<(usize, f64)> {
        let mut used: Vec<(usize, u64)> = Vec::new();
        for (s, &c) in counts.iter().enumerate() {
            if c > 0 {
                if s >= self.width {
                    return None;
                }
                used.push((s, c));
            }
        }
        let mut best: Option<(usize, u64)> = None;
        'seeds: for (i, row) in self.cost.chunks_exact(self.width).enumerate() {
            let mut q = 0u64;
            for &(s, c) in &used {
                let v = row[s];
                if v == costs::ABSENT {
                    continue 'seeds;
                }
                q += c * v as u64;
                // q only grows, so a partial sum already at the running
                // best can never satisfy `q < b.1`: the row is out.
                if best.is_some_and(|b| q >= b.1) {
                    continue 'seeds;
                }
            }
            if best.is_none_or(|b| q < b.1) {
                best = Some((i, q));
            }
        }
        best.map(|(i, q)| (i, q as f64 / 16.0))
    }

    /// Seed `i` as a table (decoded exactly as the decoder does).
    pub(crate) fn table(&self, i: usize) -> Expanded {
        if matches!(self.seeds.kind, Kind::HufCount | Kind::HufWidth) {
            let lens = self
                .seeds
                .lengths(i, self.alpha)
                .expect("bank seed decodes");
            return Expanded {
                weights: Vec::new(),
                prec: 0,
                freq: Vec::new(),
                lens,
            };
        }
        let (weights, prec) = self
            .seeds
            .weights(i, self.alpha)
            .expect("bank seed decodes");
        let freq = crate::rans::weights_to_freq(&weights).expect("bank seed normalizes");
        Expanded {
            weights,
            prec,
            freq,
            lens: Vec::new(),
        }
    }
}

/// The seeds of `kind` / `tc` in `bank` with their costs, for tables of
/// `alpha` symbols.
pub(crate) fn kind_set(bank: &Bank, kind: Kind, tc: [u8; 3], alpha: usize) -> Option<KindSet> {
    let (seeds, at) = bank.find(kind, tc)?;
    let cost = bank_costs(bank.id).get(at)?;
    Some(KindSet {
        seeds,
        alpha,
        cost,
        width: costs::width(kind as u8, tc),
    })
}

/// Lab: the entropy mode (0 run-level .. 3 split-Huffman; 1 and 2 are
/// retired), full-width bank ID (zero when absent), and scope mask
/// (bit 0 AC, bit 1 DC, bit 2 alpha) of a lossy file.
#[cfg(feature = "lab")]
pub fn lossy_entropy_seed(file: &[u8]) -> Result<(u8, u64, u8), Error> {
    crate::lossy::lab_entropy_seed(file).map(|(e, s)| (e as u8, s.bank, s.scope >> 4))
}

/// Lab: the compiled blob of bank `id`.
#[cfg(feature = "lab")]
pub fn bank_blob(id: u64) -> Option<&'static [u8]> {
    if id == BANK1.id {
        return Some(BANK1_BYTES);
    }
    #[cfg(feature = "std")]
    if let Some((_, _, blob)) = lab_banks::BANKS.get(id) {
        return Some(blob);
    }
    None
}

/// Lab helpers for building banks outside the library (the table grammar
/// functions the decoder uses).
#[cfg(feature = "lab")]
pub mod codec {
    use alloc::vec::Vec;

    /// Alphabet of tables of `kind` (0 run-level EOB, 1 run-level AC, 4/5
    /// split-Huffman count/width, 6 lossless with token scheme `tc`).
    pub fn alphabet(kind: u8, tc: [u8; 3]) -> usize {
        match kind {
            4 => crate::lossy::huf::CNT_ALPHA,
            5 => crate::lossy::huf::WID_ALPHA,
            6 => crate::tokens::TokenCfg::v1(tc[0] as u32, tc[1] as u32, tc[2] as u32).alphabet()
                as usize,
            _ => crate::lossy::coef::ALPHA,
        }
    }

    /// A count quantized to `prec` significant bits, as tables store weights.
    pub fn quantize_weight(c: u64, prec: u32) -> u32 {
        crate::rans::quantize_weight(c, prec)
    }

    /// Normalized frequencies (sum 1024) of weights, as the decoder derives
    /// them.
    pub fn weights_to_freq(w: &[u32]) -> Option<Vec<u32>> {
        crate::rans::weights_to_freq(w)
    }

    /// A rANS table (weights at precision `prec`) in the table grammar.
    pub fn write_weights(w: &[u32], prec: u32) -> Vec<u8> {
        let mut bw = crate::bits::BitWriter::new();
        crate::rans::write_weights(&mut bw, w, prec);
        bw.finish()
    }

    /// Reads a rANS table of `alpha` symbols: (weights, precision).
    pub fn read_weights(b: &[u8], alpha: usize) -> Result<(Vec<u32>, u32), crate::Error> {
        crate::rans::read_weights(&mut crate::bits::BitReader::new(b), alpha)
    }

    /// Length-limited Huffman code lengths for `counts`, as split-Huffman
    /// tables use.
    pub fn huffman_lengths(counts: &[u64]) -> Vec<u8> {
        crate::lossy::huf::lengths(counts)
    }
}

#[cfg(all(all(test, feature = "encode"), feature = "lab", feature = "std"))]
mod tests;

#[cfg(all(all(test, feature = "encode"), feature = "lab", feature = "std"))]
mod stream_tests;
