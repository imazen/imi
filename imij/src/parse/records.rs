//! The unit records of a `DATA` payload (SPEC 2.7): one grammar, one walker
//! and one set of messages for every route (the slice parsers, the reader,
//! the lossy row driver and the region reader).
//!
//! After a payload head come its unit records, back to back, filling the
//! rest of the payload exactly: each `id varint, len varint, payload`. Main
//! records have odd ids (the kinds below); each may be preceded by its side
//! record (`registry::side_of`, even, ignorable, at most one). A payload
//! always ends with a main record; unit ids follow this positional rule,
//! not the increasing-id rule of ordinary record lists. The
//! head's geometry says which main records come, in which order
//! ([`UnitMap`]). Ids are read under the file's header version: a version 2
//! file's ids translate through the registry (SPEC Appendix V3).
//!
//! The unit table hint (head record 10, ignorable) lists, per main record,
//! the bytes of its span: its side record, if any, and itself. Readers that
//! walk the records check every span against its entry as they walk; the
//! region reader, which finds records through it, feeds every span's record
//! heads through the same [`Walk`] when it opens the file. So every route
//! reaches the same records or fails (`Corrupt`).
//!
//! A head naming more units than the bytes after it is `stripe count
//! exceeds data` before anything is allocated: every unit takes at least
//! one byte.

use alloc::vec::Vec;

use crate::{ByteReader, Error};

/// Main record kinds (SPEC 2.7).
pub(crate) const R_DC: u64 = crate::registry::UNIT_DC;
pub(crate) const R_ALPHA: u64 = crate::registry::UNIT_ALPHA;
pub(crate) const R_AC: u64 = crate::registry::UNIT_AC;
pub(crate) const R_EXACT: u64 = crate::registry::UNIT_EXACT;
pub(crate) const R_NATIVE: u64 = crate::registry::UNIT_NATIVE_COMPONENTS;
pub(crate) const R_STRIPE: u64 = crate::registry::UNIT_STRIPE;
/// Head record id of the unit table hint (both heads; even, ignorable).
pub(crate) const H_UNIT_TABLE: u64 = crate::registry::HEAD_UNIT_TABLE;
/// Head record id of the unit table reservation (both heads; even,
/// ignorable).
pub(crate) const H_UNIT_RESERVE: u64 = crate::registry::HEAD_UNIT_RESERVE;
/// Head record id of unit record ordinals (both heads; odd, critical).
pub(crate) const H_UNIT_ORDINALS: u64 = crate::registry::HEAD_UNIT_ORDINALS;
/// Reservation states (SPEC 2.7): reserved and not filled, the spans in the
/// trailing unit index, the spans in the record.
pub(crate) const RESERVE_EMPTY: u8 = 0;
pub(crate) const RESERVE_TAIL: u8 = 1;
pub(crate) const RESERVE_FILLED: u8 = 2;
/// Tag of the trailing unit index chunk (ancillary, public, unsafe to copy).
pub(crate) const UIDX: [u8; 4] = *b"uIDX";
/// Bytes of the trailing unit index's footer: `data_len` u64, `chunk_len`
/// u64, the tag.
pub(crate) const INDEX_FOOTER: usize = 20;

/// The smallest record: a one-byte id and a zero length.
const MIN_RECORD: u64 = 2;

/// Appends a record: `id varint, len varint, payload` (SPEC 1).
pub(crate) fn put_record(out: &mut Vec<u8>, id: u64, payload: &[u8]) {
    crate::put_varint(out, id);
    crate::put_varint(out, payload.len() as u64);
    out.extend_from_slice(payload);
}

/// Appends a main unit record, optionally prefixed with its 0-based ordinal
/// (`HEAD_UNIT_ORDINALS`, SPEC 2.7).
#[cfg(feature = "encode")]
pub(crate) fn put_main_record(out: &mut Vec<u8>, id: u64, ordinal: Option<u64>, payload: &[u8]) {
    if let Some(ord) = ordinal {
        let len = crate::varint_len(ord) + payload.len();
        crate::put_varint(out, id);
        crate::put_varint(out, len as u64);
        crate::put_varint(out, ord);
        out.extend_from_slice(payload);
    } else {
        put_record(out, id, payload);
    }
}

/// The whole bytes of a record of `len` payload bytes: its unit table hint
/// entry when it has no side record.
#[cfg(feature = "encode")]
pub(crate) fn framed_len(id: u64, len: usize) -> u64 {
    (crate::varint_len(id) + crate::varint_len(len as u64) + len) as u64
}

/// The whole bytes of a main unit record with optional 0-based `ordinal`.
#[cfg(feature = "encode")]
pub(crate) fn main_framed_len(id: u64, ordinal: Option<u64>, len: usize) -> u64 {
    framed_len(id, len + ordinal.map_or(0, crate::varint_len))
}

/// Refuses `units` units in `bytes` bytes: every unit takes at least one.
pub(crate) fn check_count(units: usize, bytes: u64) -> Result<(), Error> {
    if units as u64 > bytes {
        return Err(Error::Corrupt("stripe count exceeds data"));
    }
    Ok(())
}

/// Walks a head's record list (SPEC 1): records in strictly increasing
/// id order that fill `list` exactly, each passed to `each` (which decides
/// what an id means in its list).
pub(crate) fn head_records<'a>(
    list: &'a [u8],
    mut each: impl FnMut(u64, &'a [u8]) -> Result<(), Error>,
) -> Result<(), Error> {
    head_records_at(list, |id, rec, _| each(id, rec))
}

/// [`head_records`], also passing where each record's payload starts in
/// `list` (a writer fills a reservation in place).
pub(crate) fn head_records_at<'a>(
    list: &'a [u8],
    mut each: impl FnMut(u64, &'a [u8], usize) -> Result<(), Error>,
) -> Result<(), Error> {
    let mut r = ByteReader::new(list);
    let mut last = None;
    while r.remaining() != 0 {
        let id = r.varint()?;
        let n = r.varint()?;
        if n > r.remaining() as u64 {
            return Err(Error::Corrupt("head record past its list"));
        }
        let at = r.pos;
        let rec = r.bytes(n as usize)?;
        if last.is_some_and(|l| id <= l) {
            return Err(Error::Corrupt("head records out of order"));
        }
        last = Some(id);
        each(id, rec, at)?;
    }
    Ok(())
}

/// The `records` entries of a unit table hint record (one per main record
/// the head's geometry names): varints filling it exactly, each at least
/// [`MIN_RECORD`]. The record's length is checked against the count before
/// anything is allocated (an entry takes 1 to 10 bytes), so a hint costs
/// memory in proportion to the geometry, never to its own length. Entries
/// stay u64: every comparison with a span or a payload length happens
/// before any pointer-width conversion, so 32- and 64-bit hosts reach the
/// same outcome.
pub(crate) fn hint_entries(rec: &[u8], records: usize) -> Result<Vec<u64>, Error> {
    let (len, n) = (rec.len() as u64, records as u64);
    if len < n || len > n.saturating_mul(10) {
        return Err(Error::Corrupt("unit table length"));
    }
    let mut r = ByteReader::new(rec);
    let mut out = Vec::new();
    out.try_reserve_exact(records)
        .map_err(|_| Error::TooLarge)?;
    while r.remaining() != 0 {
        let e = r.varint()?;
        if e < MIN_RECORD {
            return Err(Error::Corrupt("unit table entry"));
        }
        if out.len() == records {
            return Err(Error::Corrupt("unit table length"));
        }
        out.push(e);
    }
    if out.len() != records {
        return Err(Error::Corrupt("unit table length"));
    }
    Ok(out)
}

/// A head's unit table reservation (head record 12, SPEC 2.7): where its
/// payload sits in the payload (a writer fills it in place) and its state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Reserve {
    pub state: u8,
    /// Offset of the record's payload from the start of the `DATA` payload
    /// (meaningful for heads read from a slice).
    pub at: u64,
    pub len: usize,
}

impl Reserve {
    /// The spans are in the trailing unit index (SPEC 2.7).
    pub(crate) fn check_tail(&self) -> bool {
        self.state == RESERVE_TAIL
    }
}

/// A unit table reservation record `rec` for `records` main records: its
/// state, and the entries when it is filled (each `width` bytes, at least
/// [`MIN_RECORD`]). A filled record's length is checked against the count
/// before anything is allocated, in u64, so a reservation costs memory in
/// proportion to the geometry and 32- and 64-bit hosts reach the same
/// outcome. Padding after an empty or check-tail state is zero.
pub(crate) fn reserve_entries(rec: &[u8], records: usize) -> Result<(u8, Option<Vec<u64>>), Error> {
    let (&state, rest) = rec
        .split_first()
        .ok_or(Error::Corrupt("unit table length"))?;
    match state {
        RESERVE_EMPTY | RESERVE_TAIL => {
            if rest.iter().any(|&b| b != 0) {
                return Err(Error::Corrupt("unit table padding"));
            }
            Ok((state, None))
        }
        RESERVE_FILLED => {
            let (&width, spans) = rest
                .split_first()
                .ok_or(Error::Corrupt("unit table length"))?;
            if !(1..=8).contains(&width) {
                return Err(Error::Corrupt("unit table width"));
            }
            if (records as u64).checked_mul(width as u64) != Some(spans.len() as u64) {
                return Err(Error::Corrupt("unit table length"));
            }
            let mut out = Vec::new();
            out.try_reserve_exact(records)
                .map_err(|_| Error::TooLarge)?;
            for e in spans.chunks_exact(width as usize) {
                let mut le = [0u8; 8];
                le[..e.len()].copy_from_slice(e);
                let e = u64::from_le_bytes(le);
                if e < MIN_RECORD {
                    return Err(Error::Corrupt("unit table entry"));
                }
                out.push(e);
            }
            Ok((state, Some(out)))
        }
        _ => Err(Error::Corrupt("unit table state")),
    }
}

/// A filled reservation's payload: state 2, `width`, then `spans` in
/// `width` little-endian bytes each; `None` when a span does not fit.
#[cfg(feature = "encode")]
pub(crate) fn filled_reserve(spans: &[u64], width: u8) -> Option<Vec<u8>> {
    let bits = 8 * width as u32;
    if spans.iter().any(|&e| bits < 64 && e >> bits != 0) {
        return None;
    }
    let mut out = Vec::with_capacity(2 + spans.len() * width as usize);
    out.push(RESERVE_FILLED);
    out.push(width);
    for &e in spans {
        out.extend_from_slice(&e.to_le_bytes()[..width as usize]);
    }
    Some(out)
}

/// Whole bytes of a chunk of `payload` bytes: tag, length field, payload.
#[cfg(any(feature = "v3", feature = "std", test))]
pub(crate) fn chunk_len_of(payload: u64) -> u64 {
    4 + crate::varint_len(payload + 1) as u64 + payload
}

/// The trailing unit index chunk (SPEC 2.7), whole, for `spans` of a `DATA`
/// payload of `data_len` bytes.
#[cfg(feature = "encode")]
pub(crate) fn index_chunk(spans: &[u64], data_len: u64) -> Vec<u8> {
    let mut payload = Vec::with_capacity(3 * spans.len() + INDEX_FOOTER);
    for &e in spans {
        crate::put_varint(&mut payload, e);
    }
    let n = payload.len() + INDEX_FOOTER;
    let chunk_len = chunk_len_of(n as u64);
    payload.extend_from_slice(&data_len.to_le_bytes());
    payload.extend_from_slice(&chunk_len.to_le_bytes());
    payload.extend_from_slice(&UIDX);
    let mut out = Vec::with_capacity(chunk_len as usize);
    out.extend_from_slice(&UIDX);
    crate::put_varint(&mut out, n as u64 + 1);
    out.extend_from_slice(&payload);
    out
}

/// A trailing unit index as found (SPEC 2.7): its entries' bytes and the
/// `DATA` payload length it is bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Index<'a> {
    pub entries: &'a [u8],
    pub data_len: u64,
}

/// The index in the payload of a `uIDX` chunk whose whole length is
/// `chunk_len` (tag, length field, payload): `None` when the footer does not
/// name it (too short, another `chunk_len`, another closing tag), and the
/// chunk is skipped.
pub(crate) fn index_footer(payload: &[u8], chunk_len: u64) -> Option<Index<'_>> {
    let at = payload.len().checked_sub(INDEX_FOOTER)?;
    let (entries, f) = payload.split_at(at);
    let le = |b: &[u8]| u64::from_le_bytes(b.try_into().unwrap());
    (f[16..] == UIDX && le(&f[8..16]) == chunk_len).then(|| Index {
        entries,
        data_len: le(&f[..8]),
    })
}

/// Checks a trailing unit index found after a payload of `data_len` bytes
/// whose head is `head_len` bytes and names `records` main records: the
/// binding, then the entries as [`hint_entries`] reads a hint, then their
/// sum, which must be the bytes after the head. Returns the entries, which
/// the caller compares with the records' spans.
pub(crate) fn index_entries(
    index: Index<'_>,
    data_len: u64,
    head_len: u64,
    records: usize,
) -> Result<Vec<u64>, Error> {
    if index.data_len != data_len {
        return Err(Error::Corrupt("unit index for another DATA"));
    }
    let entries = hint_entries(index.entries, records)?;
    let sum = entries.iter().try_fold(0u64, |t, &e| t.checked_add(e));
    if sum != data_len.checked_sub(head_len) {
        return Err(MISMATCH);
    }
    Ok(entries)
}

/// The error of a unit table (hint, reservation or trailing index) whose
/// spans are not the records'.
pub(crate) const MISMATCH: Error = Error::Corrupt("unit table does not match the records");

/// A hint's entries against the payload: one per main record (`records`),
/// summing to at most `room` (the bytes after the head; `u64::MAX` for an
/// open payload a stream reads). Returns the sum.
pub(crate) fn check_hint(hint: &[u64], records: usize, room: u64) -> Result<u64, Error> {
    if hint.len() != records {
        return Err(Error::Corrupt("unit table length"));
    }
    hint.iter()
        .try_fold(0u64, |t, &e| t.checked_add(e).filter(|&t| t <= room))
        .ok_or(Error::Corrupt("unit table exceeds DATA"))
}

/// The unit-record grammar at one reader. The reader feeds it every
/// record's head in file order; it tells the main record it expects from
/// that record's side record (skipped), and with a hint checks each main
/// record's span against its entry.
pub(crate) struct Walk {
    /// The header version the record ids are read under.
    version: u8,
    hint: Option<Vec<u64>>,
    /// Hint entries used.
    used: usize,
    /// Start of the current span: the side record read before the main
    /// record, if any.
    side_at: Option<u64>,
    /// Every span walked, kept for a trailing unit index that arrives after
    /// the records (a forward reader of a check-tail payload, SPEC 2.7).
    spans: Option<Vec<u64>>,
    #[cfg(feature = "std")]
    ordinals: bool,
}

impl Walk {
    pub(crate) fn new(version: u8, hint: Option<Vec<u64>>) -> Walk {
        Walk {
            version,
            hint,
            used: 0,
            side_at: None,
            spans: None,
            #[cfg(feature = "std")]
            ordinals: false,
        }
    }

    #[cfg(feature = "std")]
    pub(crate) fn with_ordinals(version: u8, hint: Option<Vec<u64>>, ordinals: bool) -> Walk {
        Walk {
            version,
            hint,
            used: 0,
            side_at: None,
            spans: None,
            ordinals,
        }
    }

    /// Main records walked so far.
    #[cfg(feature = "std")]
    pub(crate) fn used(&self) -> usize {
        self.used
    }

    #[cfg(feature = "std")]
    pub(crate) fn ordinals(&self) -> bool {
        self.ordinals
    }

    /// Keeps the spans of the `records` main records it walks.
    #[cfg(feature = "std")]
    pub(crate) fn keep_spans(&mut self, records: usize) -> Result<(), Error> {
        let mut v = Vec::new();
        v.try_reserve_exact(records).map_err(|_| Error::TooLarge)?;
        self.spans = Some(v);
        Ok(())
    }

    /// The spans kept by [`Self::keep_spans`].
    #[cfg(feature = "std")]
    pub(crate) fn take_spans(&mut self) -> Option<Vec<u64>> {
        self.spans.take()
    }

    /// The record `id` (as written, under this walk's header version)
    /// spanning `at..end`, read where the main record `kind` is expected:
    /// `Ok(true)` when it is that record, `Ok(false)` for its side record,
    /// which the reader skips.
    pub(crate) fn head(&mut self, at: u64, end: u64, id: u64, kind: u64) -> Result<bool, Error> {
        let id = crate::registry::unit_id(self.version, id);
        if id == kind {
            let start = self.side_at.take().unwrap_or(at);
            if let Some(h) = &self.hint {
                let entry = *h
                    .get(self.used)
                    .ok_or(Error::Corrupt("unit table too short"))?;
                if entry != end - start {
                    return Err(MISMATCH);
                }
            }
            self.used += 1;
            if let Some(v) = &mut self.spans {
                v.try_reserve(1).map_err(|_| Error::TooLarge)?;
                v.push(end - start);
            }
            return Ok(true);
        }
        if id == crate::registry::side_of(kind) && self.side_at.is_none() {
            self.side_at = Some(at);
            return Ok(false);
        }
        Err(
            if id & 1 == 0 || [R_DC, R_ALPHA, R_AC, R_EXACT, R_STRIPE, R_NATIVE].contains(&id) {
                Error::Corrupt("unit records out of order")
            } else {
                Error::Unsupported("unknown unit record")
            },
        )
    }

    /// After the last main record: the hint must be used up.
    pub(crate) fn finish(&self) -> Result<(), Error> {
        if self.hint.as_ref().is_some_and(|h| h.len() != self.used) {
            return Err(Error::Corrupt("unit table too long"));
        }
        Ok(())
    }
}

/// The next record of `r`: its id and payload, bounded by the reader's end.
pub(crate) fn read_record<'a>(r: &mut ByteReader<'a>) -> Result<(u64, &'a [u8]), Error> {
    let id = r.varint()?;
    let n = r.varint()?;
    // In u64: a length past the payload is the same error on every host.
    if n > r.remaining() as u64 {
        return Err(Error::Corrupt("record past end of DATA"));
    }
    Ok((id, r.bytes(n as usize)?))
}

/// The records of an in-memory payload, in record order, through one
/// [`Walk`].
pub(crate) struct Split<'r, 'a> {
    r: &'r mut ByteReader<'a>,
    walk: Walk,
    ordinals: bool,
}

impl<'r, 'a> Split<'r, 'a> {
    /// The records after a head (`r` just past it) of `units` units, with
    /// the head's hint, whose spans must fit the bytes left (as every route
    /// checks before reading a record).
    #[cfg(any(feature = "native-jpeg", test))]
    pub(crate) fn new(
        r: &'r mut ByteReader<'a>,
        version: u8,
        hint: Option<Vec<u64>>,
        units: usize,
    ) -> Result<Self, Error> {
        Self::with_ordinals(r, version, hint, units, false)
    }

    pub(crate) fn with_ordinals(
        r: &'r mut ByteReader<'a>,
        version: u8,
        hint: Option<Vec<u64>>,
        units: usize,
        ordinals: bool,
    ) -> Result<Self, Error> {
        check_count(units, r.remaining() as u64)?;
        if let Some(h) = &hint {
            check_hint(h, h.len(), r.remaining() as u64)?;
        }
        Ok(Split {
            r,
            walk: Walk::new(version, hint),
            ordinals,
        })
    }

    /// The payload of the next main record, which must be `kind`.
    pub(crate) fn next(&mut self, kind: u64) -> Result<&'a [u8], Error> {
        loop {
            let at = self.r.pos as u64;
            let (id, payload) = read_record(self.r)?;
            if self.walk.head(at, self.r.pos as u64, id, kind)? {
                if self.ordinals {
                    let mut pr = ByteReader::new(payload);
                    let ord = pr.varint()?;
                    if ord != (self.walk.used - 1) as u64 {
                        return Err(Error::Corrupt("unit record ordinal"));
                    }
                    return Ok(&payload[pr.pos..]);
                }
                return Ok(payload);
            }
        }
    }

    /// After the last main record.
    pub(crate) fn finish(&self) -> Result<(), Error> {
        self.walk.finish()
    }
}

/// The payloads of the main records `kinds` that fill `bytes` exactly (a
/// unit, or one span, the region reader fetched by itself).
#[cfg(any(feature = "native-jpeg", test))]
pub(crate) fn split_span<'b>(
    bytes: &'b [u8],
    version: u8,
    kinds: &[u64],
    out: &mut [&'b [u8]],
) -> Result<(), Error> {
    split_span_indexed(bytes, version, kinds, None, out)
}

/// Splits a unit span, also checking that each main record starts with its
/// expected 0-based ordinal `first_ordinal + i` when `first_ordinal` is
/// `Some`.
#[cfg(any(feature = "v3", test))]
pub(crate) fn split_span_indexed<'b>(
    bytes: &'b [u8],
    version: u8,
    kinds: &[u64],
    first_ordinal: Option<usize>,
    out: &mut [&'b [u8]],
) -> Result<(), Error> {
    let mut r = ByteReader::new(bytes);
    let mut walk = Walk::new(version, None);
    for (i, (o, &kind)) in out.iter_mut().zip(kinds).enumerate() {
        *o = loop {
            let at = r.pos as u64;
            let (id, payload) = read_record(&mut r)?;
            if walk.head(at, r.pos as u64, id, kind)? {
                if let Some(base) = first_ordinal {
                    let mut pr = ByteReader::new(payload);
                    let ord = pr.varint()?;
                    if ord != (base + i) as u64 {
                        return Err(Error::Corrupt("unit record ordinal"));
                    }
                    break &payload[pr.pos..];
                }
                break payload;
            }
        };
    }
    if r.remaining() != 0 {
        return Err(Error::Corrupt("records do not fill their unit"));
    }
    Ok(())
}

/// Where each unit's main records sit in record order (SPEC 2.7): per
/// unit, in unit order (stripe-major, then column group), the DC record of
/// a lossy unit whose stripe starts a DC stripe, its alpha record (lossy
/// with alpha), its own record, then its exact record (lossy with exact
/// blocks). Closed form: no table of unit starts.
#[derive(Clone, Debug)]
pub(crate) struct UnitMap {
    coverage: Option<alloc::sync::Arc<crate::lossy::dc_coverage::Coverage>>,
    groups: usize,
    native: bool,
    /// Lossy stripes per DC stripe (0: lossless, no DC records).
    k: usize,
    alpha: bool,
    exact: bool,
}

impl UnitMap {
    #[cfg(feature = "std")]
    pub(crate) fn groups(&self) -> usize {
        self.groups
    }

    pub(crate) fn lossless(groups: usize) -> UnitMap {
        UnitMap {
            coverage: None,
            groups,
            native: false,
            k: 0,
            alpha: false,
            exact: false,
        }
    }

    pub(crate) fn lossy(groups: usize, k: usize, alpha: bool, exact: bool) -> UnitMap {
        UnitMap {
            coverage: None,
            groups,
            native: false,
            k: k.max(1),
            alpha,
            exact,
        }
    }

    #[cfg(feature = "native-jpeg")]
    pub(crate) fn native() -> Self {
        Self {
            native: true,
            ..Self::lossless(1)
        }
    }

    pub(crate) fn with_coverage(
        mut self,
        coverage: Option<alloc::sync::Arc<crate::lossy::dc_coverage::Coverage>>,
    ) -> Self {
        self.coverage = coverage;
        self
    }

    pub(crate) fn starts_dc_at(&self, s: usize, g: usize) -> bool {
        self.coverage
            .as_ref()
            .map_or_else(|| self.starts_dc(s), |c| c.span(s, g).start == s)
    }

    pub(crate) fn kinds_at(&self, s: usize, g: usize) -> ([u64; 4], usize) {
        if self.coverage.is_none() {
            return self.kinds(s);
        }
        let mut kinds = [0; 4];
        let mut n = 0;
        for (on, kind) in [
            (self.starts_dc_at(s, g), R_DC),
            (self.alpha, R_ALPHA),
            (true, R_AC),
            (self.exact, R_EXACT),
        ] {
            if on {
                kinds[n] = kind;
                n += 1;
            }
        }
        (kinds, n)
    }

    #[cfg(all(feature = "lab", feature = "std"))]
    pub(crate) fn per_unit_at(&self, s: usize, g: usize) -> usize {
        1 + self.extra() + usize::from(self.starts_dc_at(s, g))
    }

    /// Whether stripe `s` starts a DC stripe (its units carry DC records).
    pub(crate) fn starts_dc(&self, s: usize) -> bool {
        self.k != 0 && s.is_multiple_of(self.k)
    }

    /// Records every unit has besides its own: alpha and exact.
    fn extra(&self) -> usize {
        usize::from(self.alpha) + usize::from(self.exact)
    }

    /// Main records of a unit in stripe `s`.
    pub(crate) fn per_unit(&self, s: usize) -> usize {
        1 + self.extra() + usize::from(self.starts_dc(s))
    }

    /// The main record kinds of a unit in stripe `s`, in record order.
    pub(crate) fn kinds(&self, s: usize) -> ([u64; 4], usize) {
        if self.k == 0 {
            return ([if self.native { R_NATIVE } else { R_STRIPE }, 0, 0, 0], 1);
        }
        let mut k = [0; 4];
        let mut n = 0;
        for (on, kind) in [
            (self.starts_dc(s), R_DC),
            (self.alpha, R_ALPHA),
            (true, R_AC),
            (self.exact, R_EXACT),
        ] {
            if on {
                k[n] = kind;
                n += 1;
            }
        }
        (k, n)
    }

    /// Index of unit `u`'s first record (`u` up to the unit count, whose
    /// record count [`Self::count`] checked: this cannot overflow).
    pub(crate) fn first(&self, u: usize) -> usize {
        let (s, g) = (u / self.groups, u % self.groups);
        if let Some(c) = &self.coverage {
            return u * (1 + self.extra()) + c.before(s, g);
        }
        (s * (1 + self.extra()) + self.dc_stripes(s)) * self.groups + g * self.per_unit(s)
    }

    /// DC stripes that start before stripe `s`.
    fn dc_stripes(&self, s: usize) -> usize {
        if self.k == 0 { 0 } else { s.div_ceil(self.k) }
    }

    /// Main records of a payload of `stripes` stripes.
    pub(crate) fn count(&self, stripes: usize) -> Result<usize, Error> {
        if let Some(c) = &self.coverage {
            return stripes
                .checked_mul(self.groups)
                .and_then(|v| v.checked_mul(1 + self.extra()))
                .and_then(|v| v.checked_add(c.rectangles.len()))
                .ok_or(Error::TooLarge);
        }
        stripes
            .checked_mul(1 + self.extra())
            .and_then(|n| n.checked_add(self.dc_stripes(stripes)))
            .and_then(|n| n.checked_mul(self.groups))
            .ok_or(Error::TooLarge)
    }

    /// Walks every main record of a payload of `stripes` stripes after its
    /// head (`r` just past it; the records must fill it), as every route
    /// reads them, with `hint` compared span by span. Returns each main
    /// record's span (its side record and itself).
    #[cfg(any(feature = "native-jpeg", test))]
    pub(crate) fn spans(
        &self,
        r: &mut ByteReader<'_>,
        version: u8,
        stripes: usize,
        hint: Option<Vec<u64>>,
    ) -> Result<Vec<u64>, Error> {
        self.spans_with_ordinals(r, version, stripes, hint, false)
    }

    pub(crate) fn spans_with_ordinals(
        &self,
        r: &mut ByteReader<'_>,
        version: u8,
        stripes: usize,
        hint: Option<Vec<u64>>,
        ordinals: bool,
    ) -> Result<Vec<u64>, Error> {
        let units = stripes.checked_mul(self.groups).ok_or(Error::TooLarge)?;
        check_count(units, r.remaining() as u64)?;
        if let Some(h) = &hint {
            check_hint(h, h.len(), r.remaining() as u64)?;
        }
        let mut walk = Walk::new(version, hint);
        let mut out = Vec::new();
        out.try_reserve_exact(self.count(stripes)?)
            .map_err(|_| Error::TooLarge)?;
        for u in 0..units {
            let (kinds, n) = self.kinds_at(u / self.groups, u % self.groups);
            for &kind in &kinds[..n] {
                let start = r.pos;
                loop {
                    let at = r.pos as u64;
                    let (id, payload) = read_record(r)?;
                    if walk.head(at, r.pos as u64, id, kind)? {
                        if ordinals {
                            let mut pr = ByteReader::new(payload);
                            if pr.varint()? != (walk.used - 1) as u64 {
                                return Err(Error::Corrupt("unit record ordinal"));
                            }
                        }
                        break;
                    }
                }
                out.push((r.pos - start) as u64);
            }
        }
        walk.finish()?;
        if r.remaining() != 0 {
            return Err(Error::Corrupt("trailing DATA bytes"));
        }
        Ok(out)
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::put_varint;
    use alloc::vec;

    fn varints(vals: &[u64]) -> Vec<u8> {
        let mut out = Vec::new();
        for &v in vals {
            put_varint(&mut out, v);
        }
        out
    }

    fn rec(id: u64, payload: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        put_record(&mut b, id, payload);
        b
    }

    #[test]
    fn records_are_framed_and_measured_alike() {
        for (id, len) in [(1u64, 0usize), (9, 127), (9, 128), (300, 70_000)] {
            let b = rec(id, &vec![7; len]);
            assert_eq!(framed_len(id, len), b.len() as u64);
            let mut r = ByteReader::new(&b);
            assert_eq!(read_record(&mut r), Ok((id, &b[b.len() - len..])));
            assert_eq!(r.remaining(), 0);
            // Cut short: the length runs past the end.
            if len > 0 {
                let mut r = ByteReader::new(&b[..b.len() - 1]);
                assert_eq!(
                    read_record(&mut r),
                    Err(Error::Corrupt("record past end of DATA"))
                );
            }
        }
    }

    #[test]
    fn head_records_increase_and_fill_their_list() {
        let mut list = rec(2, b"ab");
        list.extend(rec(19, b""));
        let mut seen = Vec::new();
        head_records(&list, |id, r| {
            seen.push((id, r.to_vec()));
            Ok(())
        })
        .unwrap();
        assert_eq!(seen, [(2, b"ab".to_vec()), (19, vec![])]);
        let mut back = rec(19, b"");
        back.extend(rec(2, b"ab"));
        assert_eq!(
            head_records(&back, |_, _| Ok(())),
            Err(Error::Corrupt("head records out of order"))
        );
        let mut twice = rec(2, b"");
        twice.extend(rec(2, b""));
        assert_eq!(
            head_records(&twice, |_, _| Ok(())),
            Err(Error::Corrupt("head records out of order"))
        );
        assert_eq!(
            head_records(&list[..list.len() - 1], |_, _| Ok(())),
            Err(Error::Corrupt("truncated"))
        );
        let mut over = vec![2, 5];
        over.extend_from_slice(b"ab");
        assert_eq!(
            head_records(&over, |_, _| Ok(())),
            Err(Error::Corrupt("head record past its list"))
        );
    }

    #[test]
    fn hint_entries_fill_the_record() {
        assert_eq!(
            hint_entries(&varints(&[2, 300, 70_000]), 3),
            Ok(vec![2, 300, 70_000])
        );
        assert_eq!(hint_entries(&[], 0), Ok(vec![]));
        // An entry smaller than a record (id and length).
        assert_eq!(
            hint_entries(&varints(&[5, 1]), 2),
            Err(Error::Corrupt("unit table entry"))
        );
        // A cut varint, a non-minimal one.
        assert!(hint_entries(&[0x85], 1).is_err());
        assert!(hint_entries(&[0x85, 0x00], 1).is_err());
        // The count the geometry names: more or fewer entries.
        let three = varints(&[2, 300, 70_000]);
        for n in [2, 4] {
            assert_eq!(
                hint_entries(&three, n),
                Err(Error::Corrupt("unit table length")),
                "{n}"
            );
        }
    }

    /// A hint's length is bounded by its count before anything is
    /// allocated (an entry takes 1 to 10 bytes): a long record for few
    /// units fails at once, whatever its contents, on every host.
    #[test]
    fn hint_length_is_bounded_by_the_count_before_allocating() {
        let long = alloc::vec![2u8; 4096];
        assert_eq!(
            hint_entries(&long, 1),
            Err(Error::Corrupt("unit table length"))
        );
        assert_eq!(
            hint_entries(&long[..10], 1),
            Err(Error::Corrupt("unit table length"))
        );
        assert_eq!(
            hint_entries(&[2], 2),
            Err(Error::Corrupt("unit table length"))
        );
        // A count past what usize-sized bytes could hold is still a length
        // error, not an allocation failure.
        assert_eq!(
            hint_entries(&[2], usize::MAX),
            Err(Error::Corrupt("unit table length"))
        );
    }

    /// The hint's sum is checked in u64 before any pointer-width conversion,
    /// so 32-bit hosts (wasm32, i686) reach the same error as 64-bit ones.
    #[test]
    fn check_hint_bounds_the_sum_by_the_room_on_every_host() {
        assert_eq!(check_hint(&[10, 20], 2, 30), Ok(30));
        assert_eq!(
            check_hint(&[10, 20], 2, 29),
            Err(Error::Corrupt("unit table exceeds DATA"))
        );
        assert_eq!(
            check_hint(&[10, 20], 3, u64::MAX),
            Err(Error::Corrupt("unit table length"))
        );
        // Entries past 4 GiB and a sum past u64::MAX: the same outcomes on
        // every host, never a wrapped or narrowed value.
        assert_eq!(check_hint(&[1 << 40, 2], 2, u64::MAX), Ok((1 << 40) + 2));
        assert_eq!(
            check_hint(&[u64::MAX, 2], 2, u64::MAX),
            Err(Error::Corrupt("unit table exceeds DATA"))
        );
        assert_eq!(
            check_hint(&[1 << 33], 1, 1 << 32),
            Err(Error::Corrupt("unit table exceeds DATA"))
        );
    }

    /// Main records with and without their side record, in the order the
    /// unit map gives, through a [`Split`].
    #[test]
    fn split_reads_main_records_and_skips_side_records() {
        let mut bytes = Vec::new();
        let mut spans = Vec::new();
        for (side, id, payload) in [
            (false, R_DC, &b"dc"[..]),
            (true, R_ALPHA, b"alpha"),
            (true, R_AC, b""),
            (false, R_AC, b"ac2"),
        ] {
            let start = bytes.len();
            if side {
                bytes.extend(rec(crate::registry::side_of(id), b"side"));
            }
            bytes.extend(rec(id, payload));
            spans.push((bytes.len() - start) as u64);
        }
        for hint in [None, Some(spans.clone())] {
            let mut r = ByteReader::new(&bytes);
            let mut s = Split::new(&mut r, crate::parse::header::HEADER_VERSION, hint, 2).unwrap();
            assert_eq!(s.next(R_DC), Ok(&b"dc"[..]));
            assert_eq!(s.next(R_ALPHA), Ok(&b"alpha"[..]));
            assert_eq!(s.next(R_AC), Ok(&b""[..]));
            assert_eq!(s.next(R_AC), Ok(&b"ac2"[..]));
            s.finish().unwrap();
            assert_eq!(r.remaining(), 0);
        }
        // A hint that disagrees with any span, or lists one record more or
        // less than the payload has.
        let mut wrong = spans.clone();
        wrong[1] -= 1;
        wrong[2] += 1;
        let mut r = ByteReader::new(&bytes);
        let mut s =
            Split::new(&mut r, crate::parse::header::HEADER_VERSION, Some(wrong), 2).unwrap();
        s.next(R_DC).unwrap();
        assert_eq!(
            s.next(R_ALPHA),
            Err(Error::Corrupt("unit table does not match the records"))
        );
        let mut r = ByteReader::new(&bytes);
        let mut s = Split::new(
            &mut r,
            crate::parse::header::HEADER_VERSION,
            Some(spans[..3].to_vec()),
            2,
        )
        .unwrap();
        for k in [R_DC, R_ALPHA, R_AC] {
            s.next(k).unwrap();
        }
        assert_eq!(s.next(R_AC), Err(Error::Corrupt("unit table too short")));
        // One entry more: past the bytes left before any record is read;
        // within them (an open payload's bytes run on), after the last.
        let mut longer = spans.clone();
        longer.push(2);
        let mut r = ByteReader::new(&bytes);
        assert!(matches!(
            Split::new(
                &mut r,
                crate::parse::header::HEADER_VERSION,
                Some(longer.clone()),
                2
            ),
            Err(Error::Corrupt("unit table exceeds DATA"))
        ));
        let mut open = bytes.clone();
        open.extend_from_slice(b"chunk after the payload");
        let mut r = ByteReader::new(&open);
        let mut s = Split::new(
            &mut r,
            crate::parse::header::HEADER_VERSION,
            Some(longer),
            2,
        )
        .unwrap();
        for k in [R_DC, R_ALPHA, R_AC, R_AC] {
            s.next(k).unwrap();
        }
        assert_eq!(s.finish(), Err(Error::Corrupt("unit table too long")));
        // More units than bytes.
        let mut r = ByteReader::new(&bytes);
        assert!(matches!(
            Split::new(
                &mut r,
                crate::parse::header::HEADER_VERSION,
                None,
                bytes.len() + 1
            ),
            Err(Error::Corrupt("stripe count exceeds data"))
        ));
    }

    #[test]
    fn the_walk_refuses_records_out_of_place() {
        let head = |w: &mut Walk, id, kind| w.head(0, 2, id, kind);
        // Two side records, a side record of another kind, an even id after
        // the main record's: corrupt.
        let mut w = Walk::new(crate::parse::header::HEADER_VERSION, None);
        let side = crate::registry::side_of(R_AC);
        assert_eq!(head(&mut w, side, R_AC), Ok(false));
        assert_eq!(
            head(&mut w, side, R_AC),
            Err(Error::Corrupt("unit records out of order"))
        );
        for id in [2, 4, 70, 72, 76, 78, 80] {
            assert_eq!(
                head(
                    &mut Walk::new(crate::parse::header::HEADER_VERSION, None),
                    id,
                    R_AC
                ),
                Err(Error::Corrupt("unit records out of order")),
                "{id}"
            );
        }
        // Another known kind where `kind` belongs: corrupt; an odd id no
        // kind has: a newer decoder.
        for id in [R_DC, R_ALPHA, R_EXACT, R_STRIPE] {
            assert_eq!(
                head(
                    &mut Walk::new(crate::parse::header::HEADER_VERSION, None),
                    id,
                    R_AC
                ),
                Err(Error::Corrupt("unit records out of order"))
            );
        }
        for id in [11, 13, 1001, u64::MAX] {
            assert_eq!(
                head(
                    &mut Walk::new(crate::parse::header::HEADER_VERSION, None),
                    id,
                    R_AC
                ),
                Err(Error::Unsupported("unknown unit record"))
            );
        }
        // The DC record's side record is id 70.
        let mut w = Walk::new(crate::parse::header::HEADER_VERSION, None);
        assert_eq!(
            head(&mut w, crate::registry::side_of(R_DC), R_DC),
            Ok(false)
        );
        assert_eq!(head(&mut w, R_DC, R_DC), Ok(true));
        // Header version 2 files are read with their own ids (SPEC Appendix
        // V3): 0 is the DC side record and 1 the DC record; a version 3 id
        // in a version 2 list is not accepted.
        let v2 = crate::parse::header::HEADER_VERSION_2;
        let mut w = Walk::new(v2, None);
        assert_eq!(head(&mut w, 0, R_DC), Ok(false));
        assert_eq!(head(&mut w, 1, R_DC), Ok(true));
        assert_eq!(
            head(&mut Walk::new(v2, None), R_DC, R_DC),
            Err(Error::Unsupported("unknown unit record"))
        );
        assert_eq!(
            head(&mut Walk::new(v2, None), 70, R_DC),
            Err(Error::Corrupt("unit records out of order"))
        );
    }

    #[test]
    fn spans_fill_exactly() {
        let mut unit = rec(R_DC, b"d");
        unit.extend(rec(crate::registry::side_of(R_AC), b"side"));
        unit.extend(rec(R_AC, b"a"));
        let mut out: [&[u8]; 2] = [&[]; 2];
        split_span(
            &unit,
            crate::parse::header::HEADER_VERSION,
            &[R_DC, R_AC],
            &mut out,
        )
        .unwrap();
        assert_eq!(out, [&b"d"[..], b"a"]);
        let mut more = unit.clone();
        more.extend(rec(R_AC, b"x"));
        assert_eq!(
            split_span(
                &more,
                crate::parse::header::HEADER_VERSION,
                &[R_DC, R_AC],
                &mut out
            ),
            Err(Error::Corrupt("records do not fill their unit"))
        );
        assert!(
            split_span(
                &unit[..unit.len() - 1],
                crate::parse::header::HEADER_VERSION,
                &[R_DC, R_AC],
                &mut out
            )
            .is_err()
        );
        assert_eq!(
            split_span(
                &unit,
                crate::parse::header::HEADER_VERSION,
                &[R_DC, R_EXACT],
                &mut out
            ),
            Err(Error::Corrupt("unit records out of order"))
        );
    }

    /// Record order by the letter of SPEC 2.7: per unit (stripe-major, then
    /// column group) its DC record when its stripe starts a DC stripe
    /// (lossy), its alpha record, its own record, then its exact record.
    /// Returns each unit's first record index and the total.
    fn walk(
        groups: usize,
        stripes: usize,
        lossy: Option<(usize, bool, bool)>,
    ) -> (Vec<usize>, usize, Vec<Vec<u64>>) {
        let (mut firsts, mut kinds) = (Vec::new(), Vec::new());
        let mut i = 0;
        for s in 0..stripes {
            for _ in 0..groups {
                firsts.push(i);
                let mut k = Vec::new();
                if let Some((dk, alpha, exact)) = lossy {
                    if s % dk.max(1) == 0 {
                        k.push(R_DC);
                    }
                    if alpha {
                        k.push(R_ALPHA);
                    }
                    k.push(R_AC);
                    if exact {
                        k.push(R_EXACT);
                    }
                } else {
                    k.push(R_STRIPE);
                }
                i += k.len();
                kinds.push(k);
            }
        }
        (firsts, i, kinds)
    }

    #[test]
    fn unit_map_matches_the_spec_record_order() {
        let check = |m: UnitMap, groups: usize, stripes: usize, lossy| {
            let (firsts, total, kinds) = walk(groups, stripes, lossy);
            assert_eq!(m.count(stripes), Ok(total), "{lossy:?}");
            for (u, &f) in firsts.iter().enumerate() {
                assert_eq!(m.first(u), f, "{lossy:?} u{u}");
                let s = u / groups;
                let (k, n) = m.kinds(s);
                assert_eq!(&k[..n], &kinds[u][..]);
                assert_eq!(m.per_unit(s), n);
            }
        };
        for groups in 1..5 {
            for stripes in 0..9 {
                check(UnitMap::lossless(groups), groups, stripes, None);
                for k in 1..5 {
                    for alpha in [false, true] {
                        for exact in [false, true] {
                            let m = UnitMap::lossy(groups, k, alpha, exact);
                            check(m, groups, stripes, Some((k, alpha, exact)));
                        }
                    }
                }
            }
        }
        // k = 0 behaves as 1 (every lossy stripe its own DC stripe).
        let (a, b) = (
            UnitMap::lossy(3, 0, true, false),
            UnitMap::lossy(3, 1, true, false),
        );
        assert_eq!(
            (0..12).map(|u| a.first(u)).collect::<Vec<_>>(),
            (0..12).map(|u| b.first(u)).collect::<Vec<_>>()
        );
        assert_eq!(UnitMap::lossless(usize::MAX).count(2), Err(Error::TooLarge));
    }

    /// Head record 12 (SPEC 2.7): filled entries at 1 to 8 bytes each, the
    /// empty and check-tail states with zero padding of any length, and
    /// every malformed form `Corrupt`.
    #[test]
    fn reservations_fill_in_place_and_keep_their_states() {
        let spans = [2u64, 300, 70_000];
        for w in 1..=8u8 {
            match filled_reserve(&spans, w) {
                Some(b) => {
                    assert!(w >= 3, "{w}");
                    assert_eq!(b.len(), 2 + 3 * w as usize);
                    assert_eq!(
                        reserve_entries(&b, 3),
                        Ok((RESERVE_FILLED, Some(spans.to_vec())))
                    );
                    // One entry more or less than the geometry names.
                    for n in [2, 4] {
                        assert_eq!(
                            reserve_entries(&b, n),
                            Err(Error::Corrupt("unit table length")),
                            "{w} {n}"
                        );
                    }
                    let mut long = b.clone();
                    long.push(0);
                    assert_eq!(
                        reserve_entries(&long, 3),
                        Err(Error::Corrupt("unit table length"))
                    );
                }
                // 70,000 needs three bytes.
                None => assert!(w < 3, "{w}"),
            }
        }
        assert_eq!(filled_reserve(&[u64::MAX], 8).map(|b| b.len()), Some(10));
        assert_eq!(filled_reserve(&[1 << 56], 7), None);
        for state in [RESERVE_EMPTY, RESERVE_TAIL] {
            for pad in [0, 1, 7, 300] {
                let mut rec = vec![state];
                rec.resize(1 + pad, 0);
                assert_eq!(reserve_entries(&rec, 5), Ok((state, None)), "{pad}");
                if pad > 0 {
                    rec[pad] = 1;
                    assert_eq!(
                        reserve_entries(&rec, 5),
                        Err(Error::Corrupt("unit table padding"))
                    );
                }
            }
        }
        assert_eq!(
            reserve_entries(&[], 1),
            Err(Error::Corrupt("unit table length"))
        );
        assert_eq!(
            reserve_entries(&[RESERVE_FILLED], 1),
            Err(Error::Corrupt("unit table length"))
        );
        for w in [0u8, 9, 255] {
            assert_eq!(
                reserve_entries(&[RESERVE_FILLED, w, 2], 1),
                Err(Error::Corrupt("unit table width"))
            );
        }
        for state in [3u8, 0x80, 255] {
            assert_eq!(
                reserve_entries(&[state, 0], 1),
                Err(Error::Corrupt("unit table state"))
            );
        }
        // An entry smaller than a record.
        assert_eq!(
            reserve_entries(&[RESERVE_FILLED, 2, 1, 0, 9, 0], 2),
            Err(Error::Corrupt("unit table entry"))
        );
    }

    /// A filled reservation's length is checked against the count in u64
    /// before anything is allocated: a count no host could hold is a length
    /// error, the same on 32- and 64-bit hosts, never an allocation failure.
    #[test]
    fn reservation_length_is_bounded_by_the_count_before_allocating() {
        let rec = [RESERVE_FILLED, 8, 2, 0, 0, 0, 0, 0, 0, 0];
        for n in [usize::MAX, usize::MAX / 8 + 1, 1 << 31, 2] {
            assert_eq!(
                reserve_entries(&rec, n),
                Err(Error::Corrupt("unit table length")),
                "{n}"
            );
        }
        assert_eq!(
            reserve_entries(&rec, 1),
            Ok((RESERVE_FILLED, Some(vec![2])))
        );
    }

    /// The trailing index chunk: its footer names it exactly, and its
    /// entries are checked as a hint is, bound to its payload's length.
    #[test]
    fn trailing_index_footer_names_its_chunk() {
        let spans = [2u64, 300, 70_000, 1 << 40];
        let data_len = 10 + spans.iter().sum::<u64>();
        let chunk = index_chunk(&spans, data_len);
        assert_eq!(&chunk[..4], &UIDX);
        assert_eq!(&chunk[chunk.len() - 4..], &UIDX);
        let mut r = ByteReader::new(&chunk);
        let (tag, n) = crate::parse::chunks::read_head(&mut r).unwrap();
        assert_eq!((tag, chunk_len_of(n.unwrap())), (UIDX, chunk.len() as u64));
        let payload = &chunk[r.pos..];
        let len = chunk.len() as u64;
        let index = index_footer(payload, len).unwrap();
        assert_eq!(index.data_len, data_len);
        assert_eq!(index_entries(index, data_len, 10, 4), Ok(spans.to_vec()));
        // Another payload length; a head that leaves other bytes; another
        // count.
        assert_eq!(
            index_entries(index, data_len + 1, 10, 4),
            Err(Error::Corrupt("unit index for another DATA"))
        );
        assert_eq!(index_entries(index, data_len, 11, 4), Err(MISMATCH));
        assert_eq!(
            index_entries(index, data_len, data_len + 1, 4),
            Err(MISMATCH)
        );
        assert_eq!(
            index_entries(index, data_len, 10, 5),
            Err(Error::Corrupt("unit table length"))
        );
        // A footer that does not name the chunk: no index.
        assert_eq!(index_footer(payload, len + 1), None);
        assert_eq!(index_footer(&payload[..19], 24), None);
        let mut other = payload.to_vec();
        *other.last_mut().unwrap() = b'Y';
        assert_eq!(index_footer(&other, len), None);
        // Entries past u64 when summed.
        let wide = index_chunk(&[u64::MAX, 2], 5);
        let p = &wide[5..];
        let i = index_footer(p, wide.len() as u64).unwrap();
        assert_eq!(index_entries(i, 5, 0, 2), Err(MISMATCH));
    }

    /// The spans walk of every route's grammar, with and without a hint.
    #[test]
    fn spans_walk_every_main_record() {
        let mut bytes = Vec::new();
        let mut want = Vec::new();
        for (side, id, payload) in [
            (false, R_DC, &b"dc"[..]),
            (true, R_AC, b"ac"),
            (false, R_AC, b"ac2"),
        ] {
            let start = bytes.len();
            if side {
                bytes.extend(rec(crate::registry::side_of(id), b"side"));
            }
            bytes.extend(rec(id, payload));
            want.push((bytes.len() - start) as u64);
        }
        let map = UnitMap::lossy(1, 2, false, false);
        let mut r = ByteReader::new(&bytes);
        assert_eq!(
            map.spans(&mut r, crate::parse::header::HEADER_VERSION, 2, None),
            Ok(want.clone())
        );
        let mut r = ByteReader::new(&bytes);
        assert_eq!(
            map.spans(
                &mut r,
                crate::parse::header::HEADER_VERSION,
                2,
                Some(want.clone())
            ),
            Ok(want.clone())
        );
        let mut swapped = want.clone();
        swapped.swap(0, 1);
        let mut r = ByteReader::new(&bytes);
        assert_eq!(
            map.spans(
                &mut r,
                crate::parse::header::HEADER_VERSION,
                2,
                Some(swapped)
            ),
            Err(MISMATCH)
        );
        let mut more = bytes.clone();
        more.push(0);
        let mut r = ByteReader::new(&more);
        assert_eq!(
            map.spans(&mut r, crate::parse::header::HEADER_VERSION, 2, None),
            Err(Error::Corrupt("trailing DATA bytes"))
        );
    }

    #[test]
    fn check_count_needs_a_byte_per_unit() {
        assert_eq!(check_count(4, 4), Ok(()));
        assert_eq!(
            check_count(5, 4),
            Err(Error::Corrupt("stripe count exceeds data"))
        );
        assert_eq!(check_count(0, 0), Ok(()));
    }
}
