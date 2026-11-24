//! Piecewise file checksum (SPEC 1.4): the `cSUM` chunk and header
//! extension 2.
//!
//! The covered stream is the file without its `cSUM` chunk (tag, length and
//! payload), so piece offsets do not depend on where the chunk sits. It is cut
//! into `2^k`-byte pieces (the last one short); each piece's digest under
//! the table's [`HashAlgorithm`] (XXH3-128 by default) is stored whole, or as
//! its low 8 or 4 bytes. Header extension 2 names the piece size and, since
//! 2026-10-25, the algorithm, so a streaming reader hashes pieces before the
//! table arrives. Header extension 2 announces the
//! chunk and its `k`, so a streaming reader can hash pieces before it reaches
//! the table and can tell a stripped trailer from a file without a checksum.

use alloc::vec::Vec;
use core::ops::Range;

use crate::hash::{Digest, HashAlgorithm, Hasher};
use crate::{ByteReader, Error};

/// Even (ignorable) header extension: the file has a `cSUM` chunk whose
/// pieces are `2^payload[0]` bytes.
pub(crate) const EXT_CHECKSUM: u64 = crate::registry::HEADER_CHECKSUM;
pub(crate) const CSUM: [u8; 4] = *b"cSUM";
pub(crate) const MIN_PIECE_LOG2: u8 = 10;
pub(crate) const MAX_PIECE_LOG2: u8 = 40;
/// Whether `width` stored bytes per piece are allowed for `algo`: the whole
/// digest, or its low 8 or 4 bytes.
fn width_allowed(algo: HashAlgorithm, width: u8) -> bool {
    let w = width as usize;
    w <= algo.width() && (w == algo.width() || matches!(w, 4 | 8 | 32))
}

pub(crate) const MISMATCH: Error = Error::Corrupt("file checksum mismatch");

/// How a decode treats a file checksum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verify {
    /// Check every piece while parsing, on up to this many workers.
    Now(usize),
    /// Check the framing; the caller checks the pieces ([`crate::container::File::checksum`]).
    Defer,
    /// Ignore the chunk and the header record, as a decoder without them does.
    Off,
}

impl Verify {
    pub(crate) fn from_flag(verify: bool, threads: usize) -> Verify {
        if verify {
            Verify::Now(threads)
        } else {
            Verify::Off
        }
    }
}

/// The header record (extension 2), as parsed; checked only when verifying.
/// `[k]` announces XXH3-64 pieces (files written before 2026-10-25);
/// `[k, algo]` names the algorithm by its id (SPEC 1.4.2): a table whose id
/// differs from the record's is corrupt, and an id this build does not know
/// means the table is skipped. `[k]` (`None`) binds XXH3-64 among the
/// algorithms this build knows and leaves an unknown one to be skipped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Announced {
    Pieces(u8, Option<u8>),
    Malformed,
}

impl Announced {
    /// `version` is the header's: the one-byte form was written only under
    /// header version 2.
    pub(crate) fn parse(rec: &[u8], version: u8) -> Announced {
        match rec {
            [k] if version == crate::parse::header::HEADER_VERSION_2
                && (MIN_PIECE_LOG2..=MAX_PIECE_LOG2).contains(k) =>
            {
                Announced::Pieces(*k, None)
            }
            [k, algo] if (MIN_PIECE_LOG2..=MAX_PIECE_LOG2).contains(k) => {
                Announced::Pieces(*k, Some(*algo))
            }
            _ => Announced::Malformed,
        }
    }

    pub(crate) fn log2(self) -> Result<u8, Error> {
        match self {
            Announced::Pieces(k, _) => Ok(k),
            Announced::Malformed => Err(Error::Corrupt("bad file checksum header record")),
        }
    }

    /// The announced algorithm (`None`: unknown to this build, or malformed);
    /// a legacy `[k]` record announces XXH3-64.
    pub(crate) fn algorithm(self) -> Option<HashAlgorithm> {
        match self {
            Announced::Pieces(_, Some(algo)) => HashAlgorithm::from_wire(algo),
            Announced::Pieces(_, None) => Some(HashAlgorithm::Xxh3_64),
            Announced::Malformed => None,
        }
    }

    /// The announced algorithm id, known to this build or not (`None` for a
    /// legacy `[k]` record).
    pub(crate) fn algorithm_id(self) -> Result<Option<u8>, Error> {
        match self {
            Announced::Pieces(_, algo) => Ok(algo),
            Announced::Malformed => Err(Error::Corrupt("bad file checksum header record")),
        }
    }
}

/// The table's algorithm id (its first payload byte) against the header
/// record's: a record that names an id must be matched whether or not this
/// build knows the algorithm; a legacy `[k]` record admits XXH3-64 or an
/// algorithm this build does not know (which is then skipped).
pub(crate) fn same_algorithm(announced: Option<u8>, payload: &[u8]) -> Result<(), Error> {
    let Some(&id) = payload.first() else {
        return Err(Error::Corrupt("empty cSUM chunk"));
    };
    let agrees = match announced {
        Some(a) => a == id,
        None => HashAlgorithm::from_wire(id).is_none_or(|a| a == HashAlgorithm::Xxh3_64),
    };
    if agrees {
        Ok(())
    } else {
        Err(Error::Corrupt("cSUM algorithm differs from the header"))
    }
}

/// A piece table.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Table<'a> {
    pub algo: HashAlgorithm,
    pub width: u8,
    pub log2: u8,
    pub covered_len: u64,
    pub digests: &'a [u8],
}

impl Table<'_> {
    pub(crate) fn pieces(&self) -> u64 {
        self.covered_len.div_ceil(1 << self.log2)
    }

    /// Covered byte range of piece `i`. A plane source may report a length
    /// near `u64::MAX`, so the last piece's end saturates instead of wrapping.
    pub(crate) fn piece(&self, i: u64) -> Range<u64> {
        let a = i << self.log2;
        a..a.saturating_add(1 << self.log2).min(self.covered_len)
    }

    /// Whether `digest` is the stored digest of piece `i`.
    pub(crate) fn matches(&self, i: u64, digest: &Digest) -> bool {
        let w = self.width as usize;
        digest.algorithm() == self.algo
            && digest.matches_prefix(&self.digests[i as usize * w..][..w])
    }

    /// The table against the header record's piece size and, when the
    /// reader knows it, the covered length (SPEC 1.4).
    pub(crate) fn check(&self, log2: u8, covered_len: Option<u64>) -> Result<(), Error> {
        if self.log2 != log2 {
            return Err(Error::Corrupt("cSUM piece size differs from the header"));
        }
        if covered_len.is_some_and(|c| c != self.covered_len) {
            return Err(Error::Corrupt("cSUM length does not match the file"));
        }
        Ok(())
    }
}

/// The header record and the `cSUM` chunk come together or not at all
/// (SPEC 1.4): a record without a table is a stripped or truncated
/// trailer, a table without a record a damaged header.
pub(crate) fn presence(record: bool, chunk: bool) -> Result<(), Error> {
    match (record, chunk) {
        (true, false) => Err(Error::Corrupt("missing file checksum")),
        (false, true) => Err(Error::Corrupt("file checksum without its header record")),
        _ => Ok(()),
    }
}

/// A [`Table`] that owns its digests: for readers that fetch or stream the
/// chunk rather than hold the file.
pub(crate) struct OwnedTable {
    pub algo: HashAlgorithm,
    pub width: u8,
    pub log2: u8,
    pub covered_len: u64,
    pub digests: Vec<u8>,
}

impl OwnedTable {
    pub(crate) fn new(t: &Table<'_>) -> Result<OwnedTable, Error> {
        let mut digests = Vec::new();
        digests
            .try_reserve_exact(t.digests.len())
            .map_err(|_| Error::TooLarge)?;
        digests.extend_from_slice(t.digests);
        Ok(OwnedTable {
            algo: t.algo,
            width: t.width,
            log2: t.log2,
            covered_len: t.covered_len,
            digests,
        })
    }

    pub(crate) fn view(&self) -> Table<'_> {
        Table {
            algo: self.algo,
            width: self.width,
            log2: self.log2,
            covered_len: self.covered_len,
            digests: &self.digests,
        }
    }
}

/// The most bytes a `cSUM` chunk of a `file_len`-byte file can take: one
/// full-width digest per smallest piece, and its framing. Readers that fetch the
/// chunk bound the fetch with it.
pub(crate) fn check_chunk_len(len: u64, file_len: u64) -> Result<(), Error> {
    let most = 4
        + 10
        + 3
        + 10
        + file_len.div_ceil(1 << MIN_PIECE_LOG2) * crate::Digest::MAX_RESULT_WIDTH as u64;
    if len > most || len >= file_len {
        return Err(Error::Corrupt("cSUM digest count"));
    }
    Ok(())
}

/// Pieces checked as a reader fetches ranges of a file (the region and
/// plane readers): where the `cSUM` chunk sits (its bytes are not covered),
/// the piece size and covered length, the table once read, and which
/// pieces were checked.
pub(crate) struct Fetched {
    /// File offset and whole length of the `cSUM` chunk.
    at: u64,
    len: u64,
    log2: u8,
    /// The header record's algorithm id, and the algorithm when this build
    /// knows it, used until the table arrives.
    algo_id: Option<u8>,
    algo: Option<HashAlgorithm>,
    covered_len: u64,
    table: Option<OwnedTable>,
    /// One bit per piece: checked.
    done: Vec<u64>,
}

impl Fetched {
    /// The pieces of a `file_len`-byte file whose `cSUM` chunk is the
    /// `len` bytes at `at`, with the header record's piece size.
    pub(crate) fn new(
        at: u64,
        len: u64,
        log2: u8,
        algo_id: Option<u8>,
        file_len: u64,
    ) -> Result<Fetched, Error> {
        check_chunk_len(len, file_len)?;
        let algo = algo_id.map_or(Some(HashAlgorithm::Xxh3_64), HashAlgorithm::from_wire);
        let covered_len = file_len - len;
        let words = usize::try_from(covered_len.div_ceil(1 << log2).div_ceil(64))
            .map_err(|_| Error::TooLarge)?;
        let mut done = Vec::new();
        done.try_reserve_exact(words).map_err(|_| Error::TooLarge)?;
        done.resize(words, 0);
        Ok(Fetched {
            at,
            len,
            log2,
            algo_id,
            algo,
            covered_len,
            table: None,
            done,
        })
    }

    /// The chunk's file range.
    pub(crate) fn chunk(&self) -> (u64, u64) {
        (self.at, self.at + self.len)
    }

    #[cfg(feature = "v3")]
    pub(crate) fn has_table(&self) -> bool {
        self.table.is_some()
    }

    /// The piece size's log2.
    pub(crate) fn log2(&self) -> u8 {
        self.log2
    }

    /// Tests: the table without a chunk to read it from.
    #[cfg(all(test, feature = "encode"))]
    pub(crate) fn set_owned_table(&mut self, table: OwnedTable) {
        self.table = Some(table);
    }

    /// Reads the table from the chunk's bytes: `false` for an algorithm
    /// this reader does not know (the chunk is then not checked).
    pub(crate) fn set_table(&mut self, chunk: &[u8]) -> Result<bool, Error> {
        let mut r = ByteReader::new(chunk);
        r.bytes(4)?;
        r.varint()?;
        same_algorithm(self.algo_id, &chunk[r.pos..])?;
        let Some(t) = parse_payload(&chunk[r.pos..])? else {
            return Ok(false);
        };
        t.check(self.log2, Some(self.covered_len))?;
        debug_assert_eq!(Some(t.algo), self.algo);
        self.table = Some(OwnedTable::new(&t)?);
        Ok(true)
    }

    fn pieces(&self) -> u64 {
        self.covered_len.div_ceil(1 << self.log2)
    }

    /// Covered offset of file offset `pos`: bytes before the chunk keep their
    /// offset, bytes after it move down by its length, and a position inside
    /// the chunk maps to the chunk's place (the chunk covers nothing), so a
    /// file range that straddles or starts inside the chunk maps to the
    /// covered bytes it touches.
    pub(crate) fn covered(&self, pos: u64) -> u64 {
        if pos <= self.at {
            pos
        } else {
            pos.saturating_sub(self.len).max(self.at)
        }
    }

    pub(crate) fn is_done(&self, i: u64) -> bool {
        self.done[(i / 64) as usize] >> (i % 64) & 1 == 1
    }

    /// The unchecked pieces under file ranges `ranges` (covered bytes,
    /// never inside the chunk), sorted, once each: every piece a range
    /// overlaps with `round_out`, else only those wholly inside one.
    pub(crate) fn pending(
        &self,
        ranges: &[(u64, u64)],
        round_out: bool,
    ) -> Result<Vec<u64>, Error> {
        let mut v = Vec::new();
        let k = self.log2;
        for &(a, b) in ranges.iter().filter(|r| r.0 < r.1) {
            let (ca, cb) = (self.covered(a), self.covered(b).min(self.covered_len));
            if ca >= cb {
                continue;
            }
            let first = if round_out {
                ca >> k
            } else {
                ca.div_ceil(1 << k)
            };
            let end = if !round_out && cb < self.covered_len {
                cb >> k
            } else {
                ((cb - 1) >> k) + 1
            };
            for i in first..end.min(self.pieces()) {
                if !self.is_done(i) {
                    v.try_reserve(1).map_err(|_| Error::TooLarge)?;
                    v.push(i);
                }
            }
        }
        v.sort_unstable();
        v.dedup();
        Ok(v)
    }

    /// The covered range of piece `i` (the last saturates, SPEC 1.4).
    pub(crate) fn piece(&self, i: u64) -> Range<u64> {
        let a = i << self.log2;
        a..a.saturating_add(1 << self.log2).min(self.covered_len)
    }

    /// The file ranges of piece `i`: two when it spans the chunk.
    pub(crate) fn segments(&self, i: u64) -> [(u64, u64); 2] {
        let r = self.piece(i);
        let (s, e) = (r.start, r.end);
        let (at, n) = (self.at, self.len);
        if e <= at {
            [(s, e), (0, 0)]
        } else if s >= at {
            [(s + n, e + n), (0, 0)]
        } else {
            [(s, at), (at + n, e + n)]
        }
    }

    /// Checks piece `i` against `digest` (under the table's algorithm), marking it
    /// checked. The table must have been read.
    /// The table's algorithm (the header record's before the table is known).
    pub(crate) fn algo(&self) -> HashAlgorithm {
        self.table
            .as_ref()
            .map_or(self.algo.unwrap_or(HashAlgorithm::DEFAULT), |t| t.algo)
    }

    pub(crate) fn check(&mut self, i: u64, digest: &Digest) -> Result<(), Error> {
        let t = self.table.as_ref().ok_or(MISMATCH)?;
        if !t.view().matches(i, digest) {
            return Err(MISMATCH);
        }
        self.done[(i / 64) as usize] |= 1 << (i % 64);
        Ok(())
    }
}

/// The `algo` digest of the file ranges `parts` (a piece's
/// [`Fetched::segments`]).
pub(crate) fn digest_of<'b>(
    algo: HashAlgorithm,
    parts: impl IntoIterator<Item = &'b [u8]>,
) -> Digest {
    let mut h = Hasher::new(algo);
    for p in parts {
        h.update(p);
    }
    h.finish()
}

/// A `cSUM` payload: its table, or None for an algorithm this decoder does
/// not know (the chunk is then not checked).
pub(crate) fn parse_payload(payload: &[u8]) -> Result<Option<Table<'_>>, Error> {
    let mut r = ByteReader::new(payload);
    let short = |_| Error::Corrupt("short cSUM chunk");
    let Some(algo) =
        HashAlgorithm::from_wire(r.u8().map_err(|_| Error::Corrupt("empty cSUM chunk"))?)
    else {
        return Ok(None);
    };
    let width = r.u8().map_err(short)?;
    if !width_allowed(algo, width) {
        return Err(Error::Corrupt("cSUM digest width"));
    }
    let log2 = r.u8().map_err(short)?;
    if !(MIN_PIECE_LOG2..=MAX_PIECE_LOG2).contains(&log2) {
        return Err(Error::Corrupt("cSUM piece size"));
    }
    let covered_len = r.varint().map_err(short)?;
    if covered_len == 0 {
        return Err(Error::Corrupt("cSUM covers nothing"));
    }
    let bytes = covered_len
        .div_ceil(1 << log2)
        .checked_mul(width as u64)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or(Error::Corrupt("cSUM digest count"))?;
    if r.remaining() != bytes {
        return Err(Error::Corrupt("cSUM digest count"));
    }
    Ok(Some(Table {
        algo,
        width,
        log2,
        covered_len,
        digests: r.bytes(bytes)?,
    }))
}

/// The covered bytes of an in-memory file: all but the chunk at
/// `at..at + chunk_len`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Covered<'a> {
    pub data: &'a [u8],
    pub at: usize,
    pub chunk_len: usize,
}

impl<'a> Covered<'a> {
    pub(crate) fn len(&self) -> u64 {
        (self.data.len() - self.chunk_len) as u64
    }

    /// Covered offset of file offset `pos` (outside the chunk).
    pub(crate) fn offset(&self, pos: usize) -> u64 {
        if pos <= self.at {
            pos as u64
        } else {
            (pos - self.chunk_len) as u64
        }
    }

    /// The file bytes of covered range `r`, as up to two slices.
    pub(crate) fn bytes(&self, r: Range<u64>) -> (&'a [u8], &'a [u8]) {
        let (a, b) = (r.start as usize, r.end as usize);
        let (at, n) = (self.at, self.chunk_len);
        if b <= at {
            (&self.data[a..b], &[])
        } else if a >= at {
            (&self.data[a + n..b + n], &[])
        } else {
            (&self.data[a..at], &self.data[at + n..b + n])
        }
    }
}

fn digest(algo: HashAlgorithm, (a, b): (&[u8], &[u8])) -> Digest {
    let mut h = Hasher::new(algo);
    h.update(a);
    h.update(b);
    h.finish()
}

/// Covered bytes per extra hashing worker: XXH3 runs at 40-100 GB/s from
/// cache, so a smaller file hashes in less time than waking workers takes
/// (12-thread lossy decodes of 1 MP files cost up to +7% with 64 KiB pieces
/// spread over workers; benchmarks/csum_pieces_2026-10-07).
const MIN_BYTES_PER_WORKER: u64 = 4 << 20;

/// A file's checksum ready to check: its covered bytes and table.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Pieces<'a> {
    pub covered: Covered<'a>,
    pub table: Table<'a>,
}

impl Pieces<'_> {
    /// Checks pieces `range`.
    pub(crate) fn verify(&self, range: Range<u64>) -> Result<(), Error> {
        for i in range {
            if !self.table.matches(
                i,
                &digest(self.table.algo, self.covered.bytes(self.table.piece(i))),
            ) {
                return Err(MISMATCH);
            }
        }
        Ok(())
    }

    /// Checks every piece on up to `threads` workers, one per started
    /// [`MIN_BYTES_PER_WORKER`] of covered bytes.
    pub(crate) fn verify_all(&self, threads: usize) -> Result<(), Error> {
        let n = self.table.pieces();
        let by_bytes = self.table.covered_len.div_ceil(MIN_BYTES_PER_WORKER);
        let workers =
            crate::par::worker_count(threads, n.min(by_bytes).min(usize::MAX as u64) as usize);
        if workers <= 1 {
            return self.verify(0..n);
        }
        // Contiguous blocks keep each worker's reads sequential.
        let blocks: Vec<Range<u64>> = (0..workers as u64)
            .map(|w| n * w / workers as u64..n * (w + 1) / workers as u64)
            .collect();
        crate::par::par_map(blocks, workers, |_, r| self.verify(r))
            .into_iter()
            .collect()
    }

    /// Pieces that lie entirely inside covered range `r`.
    pub(crate) fn inside(&self, r: Range<u64>) -> Range<u64> {
        let k = self.table.log2;
        let first = r.start.div_ceil(1 << k);
        let last = if r.end >= self.table.covered_len {
            self.table.pieces()
        } else {
            r.end >> k
        };
        first..last.max(first)
    }

    /// Pieces that overlap covered range `r`.
    pub(crate) fn overlapping(&self, r: Range<u64>) -> Range<u64> {
        if r.start >= r.end {
            return 0..0;
        }
        let k = self.table.log2;
        (r.start >> k)..((r.end - 1) >> k) + 1
    }
}

/// The checksum framing of a whole in-memory file, checked against its
/// header record. `chunk` is the `cSUM` chunk's position, length and
/// payload; `announced` the header record. None: nothing to check (an
/// unknown algorithm).
pub(crate) fn pieces<'a>(
    data: &'a [u8],
    announced: Option<Announced>,
    chunk: Option<(usize, usize, &'a [u8])>,
) -> Result<Option<Pieces<'a>>, Error> {
    let k = announced.map(Announced::log2).transpose()?;
    presence(k.is_some(), chunk.is_some())?;
    let (Some(k), Some((at, chunk_len, payload))) = (k, chunk) else {
        return Ok(None);
    };
    same_algorithm(announced.unwrap().algorithm_id()?, payload)?;
    // An algorithm this build does not know: the table is not checked.
    let Some(table) = parse_payload(payload)? else {
        return Ok(None);
    };
    let covered = Covered {
        data,
        at,
        chunk_len,
    };
    table.check(k, Some(covered.len()))?;
    Ok(Some(Pieces { covered, table }))
}

/// Whether `e` is an error a damaged byte can cause, which a mismatching
/// piece explains (SPEC 1.4).
pub(crate) fn explainable(e: &Error) -> bool {
    *e != MISMATCH
        && matches!(
            e,
            Error::Corrupt(_) | Error::Unsupported(_) | Error::TooLarge
        )
}

/// A parse or chunk-rule error `e` met by a reader that fetches what it
/// reads (the region and plane readers), explained by a mismatching piece
/// (SPEC 1.4): with the `cSUM` chunk at `at..at + len` before the failing
/// bytes, every piece under file bytes `0..to` is checked, rounded out to
/// whole pieces. `chunk` reads the chunk's bytes, `digests` the digest of
/// each listed piece under the table's algorithm (both from the reader's
/// source); a mismatch replaces
/// `e`, anything else keeps it. Error path only.
#[allow(clippy::too_many_arguments)]
pub(crate) fn explain_fetched(
    e: Error,
    at: u64,
    len: u64,
    log2: Result<u8, Error>,
    algo: Result<Option<u8>, Error>,
    file_len: u64,
    to: u64,
    chunk: impl FnOnce() -> Result<Vec<u8>, Error>,
    digests: impl FnOnce(&Fetched, &[u64]) -> Result<Vec<Digest>, Error>,
) -> Error {
    if !explainable(&e) || at >= to {
        return e;
    }
    let checked = (|| -> Result<(), Error> {
        let mut pieces = Fetched::new(at, len, log2?, algo?, file_len)?;
        if !pieces.set_table(&chunk()?)? {
            // An algorithm this reader does not know: nothing to check.
            return Ok(());
        }
        let pending = pieces.pending(&[(0, to.min(file_len))], true)?;
        let got = digests(&pieces, &pending)?;
        for (&i, d) in pending.iter().zip(got) {
            pieces.check(i, &d)?;
        }
        Ok(())
    })();
    match checked {
        Err(m) if m == MISMATCH => m,
        _ => e,
    }
}

/// A parse or chunk-rule error `e` met while reading an in-memory file,
/// explained by a mismatching piece (SPEC 1.4) when the table came before
/// the failing bytes: `chunk` is the `cSUM` chunk read so far (none: no
/// table yet), `end` the file offset read up to. The pieces holding bytes
/// before `end` are checked (only those the file holds whole), and a
/// mismatch is reported in place of `e`. Error path only.
pub(crate) fn explain_in_memory(
    data: &[u8],
    announced: Option<Announced>,
    chunk: Option<(usize, usize, &[u8])>,
    end: usize,
    e: Error,
) -> Error {
    let (true, Some(a), Some((at, chunk_len, payload))) = (explainable(&e), announced, chunk)
    else {
        return e;
    };
    let (Ok(k), Ok(Some(table))) = (a.log2(), parse_payload(payload)) else {
        return e;
    };
    if at >= end || table.check(k, None).is_err() {
        return e;
    }
    let covered = Covered {
        data,
        at,
        chunk_len,
    };
    let held = covered.len();
    let pieces = Pieces { covered, table };
    for i in pieces.overlapping(0..covered.offset(end.min(data.len()))) {
        if table.piece(i).end > held {
            break;
        }
        if pieces.verify(i..i + 1).is_err() {
            return MISMATCH;
        }
    }
    e
}

// ---- writing ---------------------------------------------------------------

/// Piece size for `len` covered bytes: one piece up to 64 KiB, at most 16
/// pieces of 64 KiB to 1 MiB up to 16 MiB, 1 MiB pieces beyond
/// (benchmarks/csum_pieces_2026-10-07).
pub(crate) fn piece_log2_for(len: u64) -> u8 {
    #[cfg(all(feature = "lab", feature = "std"))]
    if let Some(k) = crate::lab::csum_piece_log2() {
        return k;
    }
    if len <= 1 << 16 {
        return 16;
    }
    let target = len.div_ceil(16);
    ((64 - (target - 1).leading_zeros()) as u8).clamp(16, 20)
}

/// Digest bytes per piece a writer stores for `algo`: the whole digest, or
/// a lab-selected truncation.
pub(crate) fn writer_width(algo: HashAlgorithm) -> u8 {
    #[cfg(all(feature = "lab", feature = "std"))]
    if let Some(w) = crate::lab::csum_width().filter(|&w| width_allowed(algo, w)) {
        return w;
    }
    algo.width() as u8
}

fn payload_len(covered: u64, k: u8, width: u8) -> usize {
    3 + crate::varint_len(covered) + covered.div_ceil(1 << k) as usize * width as usize
}

/// Bytes of a `cSUM` chunk for `covered` bytes in `2^k` pieces.
pub(crate) fn chunk_len(covered: u64, k: u8, width: u8) -> usize {
    let payload = payload_len(covered, k, width);
    4 + crate::varint_len(payload as u64 + 1) + payload
}

/// The `cSUM` chunk for the covered stream `head ++ tail` in `2^k` pieces,
/// `width` bytes of each piece's `algo` digest.
pub(crate) fn chunk_bytes(
    head: &[u8],
    tail: &[u8],
    k: u8,
    algo: HashAlgorithm,
    width: u8,
) -> Vec<u8> {
    let len = (head.len() + tail.len()) as u64;
    let clen = chunk_len(len, k, width);
    let covered = Covered {
        data: head,
        at: head.len(),
        chunk_len: 0,
    };
    let table = Table {
        algo,
        width,
        log2: k,
        covered_len: len,
        digests: &[],
    };
    let mut chunk = Vec::with_capacity(clen);
    chunk.extend_from_slice(&CSUM);
    crate::put_varint(&mut chunk, payload_len(len, k, width) as u64 + 1);
    chunk.extend_from_slice(&[algo.wire(), width, k]);
    crate::put_varint(&mut chunk, len);
    for i in 0..table.pieces() {
        let r = table.piece(i);
        let d = if r.end as usize <= head.len() {
            digest(algo, covered.bytes(r))
        } else {
            let (a, b) = (r.start as usize, r.end as usize);
            let h = head.len();
            digest(
                algo,
                if a >= h {
                    (&tail[a - h..b - h], &[][..])
                } else {
                    (&head[a..], &tail[..b - h])
                },
            )
        };
        chunk.extend_from_slice(&d.as_bytes()[..width as usize]);
    }
    debug_assert_eq!(chunk.len(), clen);
    chunk
}

/// Adds the header record and the `cSUM` chunk (before `DATA`) to a complete
/// file `out` that has neither.
#[cfg(feature = "encode")]
pub(crate) fn finish_file(out: Vec<u8>, algo: Option<HashAlgorithm>) -> Result<Vec<u8>, Error> {
    let Some(algo) = algo else {
        return Ok(out);
    };
    // The encoders' own output: its header and chunks parse. The record goes
    // in extension id order; `seal` sets its piece size.
    let out =
        crate::container::with_extension(out, EXT_CHECKSUM, Some(&[MIN_PIECE_LOG2, algo.wire()]))?;
    seal(out, algo)
}

/// Re-adds the table to a file that keeps its header record but lost its
/// `cSUM` chunk (a rewritten file).
pub(crate) fn refinish(file: Vec<u8>) -> Result<Vec<u8>, Error> {
    // The record names the algorithm ([k] is XXH3-64): keep it.
    let algo = crate::parse::header::read_header_sized(&mut ByteReader::new(&file))?
        .checksum
        .ok_or(Error::InvalidInput("file has no checksum record"))?
        .announced
        .algorithm()
        .ok_or(Error::Unsupported("unknown file checksum algorithm"))?;
    seal(file, algo)
}

/// Passes [`seal`] may take: the locator settles in two or three.
const SEAL_PASSES: usize = 8;

/// `file` (a complete file without a `cSUM` chunk) with its DATA locator
/// (SPEC 1.0) written as if a `cSUM` chunk of `len` bytes sat just before
/// `DATA` (`len` 0: no chunk). Writers settle the locator with this, since
/// the chunk's length depends on the covered length, which includes the
/// locator.
pub(crate) fn locate_with_chunk(file: Vec<u8>, len: usize) -> Result<Vec<u8>, Error> {
    use crate::container::{data_at, locate};
    if len == 0 {
        return locate(file);
    }
    // A placeholder chunk of exactly `len` bytes: tag, length, zeros.
    let payload = (1..=10)
        .filter_map(|v| len.checked_sub(4 + v))
        .find(|&p| 4 + crate::varint_len(p as u64 + 1) + p == len)
        .ok_or(Error::InvalidInput("cSUM chunk length"))?;
    let mut placeholder = Vec::with_capacity(len);
    placeholder.extend_from_slice(&CSUM);
    crate::put_varint(&mut placeholder, payload as u64 + 1);
    placeholder.resize(len, 0);
    let data = data_at(&file)?;
    let mut placed = file;
    placed.splice(data..data, placeholder);
    let mut out = locate(placed)?;
    let d = data_at(&out)?;
    out.drain(d - len..d);
    Ok(out)
}

/// Sets the checksum record's piece size for `file` (which has the record
/// and no `cSUM` chunk) and inserts the chunk before `DATA`. A DATA locator
/// (SPEC 1.0) names every chunk before `DATA`, the `cSUM` chunk too, and is
/// itself covered; the chunk's length depends on the covered length, so the
/// locator is rewritten with a chunk of that length in place until the
/// covered length stops changing. The piece size is chosen once, from the
/// file as given (readers take it from the record): with it fixed the
/// chunk's length only grows with the covered length, and so does the
/// locator. Chosen again on every pass, a covered length on a piece-size
/// boundary flipped between two piece counts forever. Without a locator
/// the first pass is the result.
fn seal(mut file: Vec<u8>, algo: HashAlgorithm) -> Result<Vec<u8>, Error> {
    use crate::container::data_at;
    use crate::parse::header::read_header_sized;
    let width = writer_width(algo);
    let k = piece_log2_for(file.len() as u64);
    for _ in 0..SEAL_PASSES {
        let at = read_header_sized(&mut ByteReader::new(&file))?
            .checksum
            .map(|c| c.at)
            .ok_or(Error::InvalidInput("file has no checksum record"))?;
        file[at] = k;
        let clen = chunk_len(file.len() as u64, k, width);
        let next = locate_with_chunk(file.clone(), clen)?;
        if next == file {
            let data = data_at(&file)?;
            let chunk = chunk_bytes(&file[..data], &file[data..], k, algo, width);
            debug_assert_eq!(chunk.len(), clen);
            file.splice(data..data, chunk);
            return Ok(file);
        }
        file = next;
    }
    Err(Error::InvalidInput(
        "file checksum: DATA locator did not settle",
    ))
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use alloc::vec;

    /// `Fetched::pending` against a piece-by-piece reading of its contract:
    /// a piece is pending when it is unchecked and, for some non-empty range
    /// (file offsets, mapped to covered ones around the chunk), overlaps it
    /// (`round_out`) or lies wholly inside it (the last piece saturates, so
    /// a range reaching the end of the covered bytes holds it whole).
    #[test]
    fn fetched_pending_matches_brute_force() {
        let mut s = 0x6A09_E667_F3BC_C909u64;
        let mut next = |n: u64| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s % n
        };
        let log2 = MIN_PIECE_LOG2;
        let piece = 1u64 << log2;
        let mut cases = 0;
        for _ in 0..3000 {
            let file_len = 64 + next(12 * piece);
            let len = 1 + next(40.min(file_len / 2));
            let at = next(file_len - len + 1);
            let Ok(mut f) = Fetched::new(
                at,
                len,
                log2,
                Some(HashAlgorithm::Xxh3_128.wire()),
                file_len,
            ) else {
                continue;
            };
            let covered_len = file_len - len;
            let pieces = covered_len.div_ceil(piece);
            cases += 1;
            for i in 0..pieces {
                if next(3) == 0 {
                    f.done[(i / 64) as usize] |= 1 << (i % 64);
                }
            }
            // Ranges in covered bytes, then as file offsets around the chunk.
            let to_file = |c: u64, end: bool| {
                if c > at || (!end && c == at) {
                    c + len
                } else {
                    c
                }
            };
            let ranges: Vec<(u64, u64)> = (0..next(4))
                .map(|_| {
                    let a = next(covered_len + 1);
                    let b = next(covered_len + 1);
                    let (a, b) = (a.min(b), a.max(b));
                    (to_file(a, false), to_file(b, true))
                })
                .collect();
            for round_out in [false, true] {
                let got = f.pending(&ranges, round_out).unwrap();
                let mut want = Vec::new();
                for i in 0..pieces {
                    let (ps, pe) = (i * piece, ((i + 1) * piece).min(covered_len));
                    let hit = ranges.iter().any(|&(a, b)| {
                        let (ca, cb) = (f.covered(a), f.covered(b).min(covered_len));
                        ca < cb
                            && if round_out {
                                ps < cb && pe > ca
                            } else {
                                ps >= ca && pe <= cb
                            }
                    });
                    if hit && !f.is_done(i) {
                        want.push(i);
                    }
                }
                assert_eq!(
                    got, want,
                    "file {file_len} chunk {at}+{len} {ranges:?} round_out {round_out}"
                );
            }
        }
        assert!(cases > 2500, "{cases} cases");
    }

    /// File offsets inside the chunk (a region read whose span starts or ends
    /// in the `cSUM` bytes) map to the chunk's place, and the covered bytes
    /// such a read touches are still pending. Before the fix `covered`
    /// subtracted the chunk length from any position past its start, which
    /// underflowed for a read starting inside a chunk longer than its offset
    /// and dropped the range from `pending`.
    #[test]
    fn fetched_maps_positions_inside_the_chunk() {
        let log2 = MIN_PIECE_LOG2;
        let piece = 1u64 << log2;
        // A 120-byte chunk (15 digests) at offset 50: shorter offset than
        // length, the case that underflowed.
        let (at, len) = (50, 120);
        let file_len = at + len + 12 * piece;
        let f = Fetched::new(
            at,
            len,
            log2,
            Some(HashAlgorithm::Xxh3_128.wire()),
            file_len,
        )
        .unwrap();
        // Independent mapping: before the chunk, inside it, after it.
        for (pos, want) in [
            (0, 0),
            (at, at),
            (at + 1, at),
            (at + len - 1, at),
            (at + len, at),
            (at + len + 7, at + 7),
            (file_len, file_len - len),
        ] {
            assert_eq!(f.covered(pos), want, "pos {pos}");
        }
        // A read from inside the chunk to two pieces past it touches covered
        // bytes [at, at + 2 piece): pieces 0..=2 when rounding out, piece 1
        // (wholly inside) otherwise.
        let read = [(at + 5, at + len + 2 * piece)];
        assert_eq!(f.pending(&read, true).unwrap(), vec![0, 1, 2]);
        assert_eq!(f.pending(&read, false).unwrap(), vec![1]);
        // A read wholly inside the chunk touches no covered bytes.
        assert!(
            f.pending(&[(at + 1, at + len - 1)], true)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn checksum_insert_preserves_higher_extensions_and_strict_order() {
        use crate::{Channels, EncodeConfig, ImageInfo, Metadata, SampleType};
        let info = ImageInfo::new(9, 7, Channels::Rgb, SampleType::U8);
        let pixels: Vec<u8> = (0..info.row_bytes() * 7).map(|i| i as u8).collect();
        let original = crate::encode(
            &pixels,
            info.row_bytes(),
            &info,
            &Metadata::default(),
            &EncodeConfig::default(),
        )
        .unwrap();
        let mut r = ByteReader::new(&original[8..]);
        let n = r.var_usize().unwrap();
        let body_start = 8 + r.pos;
        let body_end = body_start + n;
        // Empty and multi-byte ids/payload lengths; cross the header-length
        // varint boundary while retaining every opaque extension byte.
        for payload_len in [0, 117, 128] {
            // An opaque even record below the checksum's id (2).
            let mut extensions = vec![0, 3, 7, 7, 7];
            let insert_offset = extensions.len();
            // Opaque even ids above the checksum's (4 is the DATA locator,
            // which writers rewrite).
            crate::put_varint(&mut extensions, 6);
            crate::put_varint(&mut extensions, payload_len);
            extensions.extend(core::iter::repeat_n(0xA5, payload_len as usize));
            extensions.extend_from_slice(&[8, 0]);
            crate::put_varint(&mut extensions, 128);
            extensions.extend_from_slice(&[3, 0x41, 0x42, 0x43]);
            let mut source = original[..8].to_vec();
            crate::put_varint(&mut source, (n + extensions.len()) as u64);
            source.extend_from_slice(&original[body_start..body_end]);
            source.extend_from_slice(&extensions);
            source.extend_from_slice(&original[body_end..]);
            assert_eq!(crate::decode(&source).unwrap().1, pixels);
            let finished = finish_file(source, Some(HashAlgorithm::Xxh3_128)).unwrap();
            assert_eq!(crate::decode(&finished).unwrap().1, pixels);
            let mut r = ByteReader::new(&finished[8..]);
            let written_n = r.var_usize().unwrap();
            let written_start = 8 + r.pos;
            let mut expected = original[body_start..body_end].to_vec();
            expected.extend_from_slice(&extensions[..insert_offset]);
            expected.extend_from_slice(&[2, 2, 16, 1]);
            expected.extend_from_slice(&extensions[insert_offset..]);
            assert_eq!(
                &finished[written_start..written_start + written_n],
                expected
            );

            // Refinish after removing only cSUM: higher header records survive.
            let chunk_start = written_start + written_n;
            let mut c = ByteReader::new(&finished[chunk_start..]);
            assert_eq!(c.bytes(4).unwrap(), b"cSUM");
            let size = c.var_usize().unwrap() - 1;
            c.bytes(size).unwrap();
            let mut without_table = finished[..chunk_start].to_vec();
            without_table.extend_from_slice(&finished[chunk_start + c.pos..]);
            let remuxed = refinish(without_table).unwrap();
            assert_eq!(remuxed, finished);
            assert_eq!(crate::decode(&remuxed).unwrap().1, pixels);

            // Strict ordering must still reject an extension moved after id 4.
            let checksum_at = written_start + n + insert_offset;
            let mut unordered = finished.clone();
            unordered[checksum_at] = 8;
            assert_eq!(
                crate::header(&unordered).err(),
                Some(Error::Corrupt("header records out of order"))
            );
        }
    }

    #[test]
    fn last_piece_ends_at_a_huge_covered_length() {
        // A plane source may report a length near u64::MAX: the last piece
        // ends at covered_len instead of wrapping past 2^64.
        for k in [MIN_PIECE_LOG2, 32, MAX_PIECE_LOG2] {
            let t = Table {
                algo: HashAlgorithm::Xxh3_128,
                width: 8,
                log2: k,
                covered_len: u64::MAX,
                digests: &[],
            };
            let last = t.pieces() - 1;
            assert_eq!(t.piece(last), last << k..u64::MAX, "k={k}");
            assert_eq!(t.piece(0), 0..1 << k, "k={k}");
        }
    }

    #[test]
    fn piece_size_by_length() {
        for (len, k) in [
            (1, 16),
            (1 << 16, 16),
            ((1 << 16) + 1, 16),
            (1 << 20, 16),
            (2 << 20, 17),
            (16 << 20, 20),
            (1 << 34, 20),
        ] {
            assert_eq!(piece_log2_for(len), k, "{len}");
        }
    }

    fn covered_file(head: &[u8], tail: &[u8], k: u8, width: u8) -> (Vec<u8>, usize) {
        let chunk = chunk_bytes(head, tail, k, HashAlgorithm::Xxh3_128, width);
        assert_eq!(
            chunk.len(),
            chunk_len((head.len() + tail.len()) as u64, k, width)
        );
        let mut file = head.to_vec();
        file.extend_from_slice(&chunk);
        file.extend_from_slice(tail);
        (file, chunk.len())
    }

    fn pieces_of(file: &[u8], at: usize, len: usize) -> Pieces<'_> {
        let mut r = ByteReader::new(&file[at..at + len]);
        assert_eq!(r.bytes(4).unwrap(), CSUM);
        let n = r.var_usize().unwrap();
        assert_eq!(n - 1, r.remaining());
        let table = parse_payload(&file[at + r.pos..at + len]).unwrap().unwrap();
        Pieces {
            covered: Covered {
                data: file,
                at,
                chunk_len: len,
            },
            table,
        }
    }

    #[test]
    fn chunks_round_trip_across_the_seam() {
        let bytes: Vec<u8> = (0..5000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 11) as u8)
            .collect();
        for split in [0, 1, 1023, 1024, 1025, 3000, 5000] {
            for (k, width) in [(10, 8), (10, 4), (11, 8), (16, 8)] {
                let (head, tail) = bytes.split_at(split);
                let (file, clen) = covered_file(head, tail, k, width);
                let p = pieces_of(&file, split, clen);
                assert_eq!(p.table.covered_len, 5000);
                assert_eq!(p.table.pieces(), 5000u64.div_ceil(1 << k));
                p.verify_all(1).unwrap();
                p.verify_all(4).unwrap();
                // Every covered byte belongs to the piece that detects it.
                for pos in [0, split.saturating_sub(1), 4999] {
                    let at = if pos < split { pos } else { pos + clen };
                    let mut bad = file.clone();
                    bad[at] ^= 1;
                    let q = pieces_of(&bad, split, clen);
                    let i = (pos as u64) >> k;
                    assert_eq!(q.verify(i..i + 1), Err(MISMATCH));
                    assert_eq!(q.verify(0..i), Ok(()));
                    assert_eq!(q.verify(i + 1..q.table.pieces()), Ok(()));
                }
            }
        }
    }

    #[test]
    fn large_files_spread_over_workers() {
        // 9 MiB: three workers at most; a changed byte in any of the 144
        // pieces still fails the whole check.
        let bytes: Vec<u8> = (0..9u32 << 20)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        let (file, clen) = covered_file(&bytes[..1000], &bytes[1000..], 16, 8);
        let p = pieces_of(&file, 1000, clen);
        assert_eq!(p.table.pieces(), 144);
        assert_eq!(p.table.covered_len.div_ceil(MIN_BYTES_PER_WORKER), 3);
        p.verify_all(1).unwrap();
        p.verify_all(8).unwrap();
        for pos in [0usize, 4 << 20, (9 << 20) - 1] {
            let at = if pos < 1000 { pos } else { pos + clen };
            let mut bad = file.clone();
            bad[at] ^= 0x10;
            assert_eq!(
                pieces_of(&bad, 1000, clen).verify_all(8),
                Err(MISMATCH),
                "{pos}"
            );
        }
    }

    #[test]
    fn payload_rules() {
        let ok = |w: u8, k: u8, len: u64, n: usize| {
            let mut p = vec![0, w, k];
            crate::put_varint(&mut p, len);
            p.extend(core::iter::repeat_n(7, n));
            p
        };
        assert!(parse_payload(&ok(8, 10, 2048, 16)).unwrap().is_some());
        assert!(parse_payload(&ok(4, 10, 2049, 12)).unwrap().is_some());
        for (p, why) in [
            (ok(5, 10, 2048, 10), "cSUM digest width"),
            (ok(8, 9, 2048, 32), "cSUM piece size"),
            (ok(8, 41, 2048, 8), "cSUM piece size"),
            (ok(8, 10, 0, 0), "cSUM covers nothing"),
            (ok(8, 10, 2048, 15), "cSUM digest count"),
            (ok(8, 10, 2048, 17), "cSUM digest count"),
            (vec![], "empty cSUM chunk"),
            (vec![0, 8], "short cSUM chunk"),
        ] {
            assert_eq!(parse_payload(&p).err(), Some(Error::Corrupt(why)), "{why}");
        }
        assert!(parse_payload(&[9, 1, 2, 3]).unwrap().is_none());
    }

    #[test]
    fn piece_ranges() {
        let bytes = vec![0u8; 5000];
        let (file, clen) = covered_file(&bytes[..10], &bytes[10..], 10, 8);
        let p = pieces_of(&file, 10, clen);
        assert_eq!(p.inside(0..1024), 0..1);
        assert_eq!(p.inside(1..2048), 1..2);
        assert_eq!(p.inside(1..2047), 1..1);
        assert_eq!(p.inside(4000..5000), 4..5);
        assert_eq!(p.overlapping(1..2047), 0..2);
        assert_eq!(p.overlapping(1024..1025), 1..2);
        assert_eq!(p.overlapping(5..5), 0..0);
        assert_eq!(p.covered.offset(10), 10);
        assert_eq!(p.covered.offset(10 + clen), 10);
    }
}
