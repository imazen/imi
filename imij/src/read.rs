//! Reader-based decoding: pixels from any [`std::io::Read`], rows pushed
//! to a callback as they are decoded. Holds the header, the tables, one
//! record of each stream (lossy stripe, DC stripe, alpha stripe) and one
//! stripe of output rows. Metadata is retained, including chunks after the
//! pixel data; its cumulative size is not bounded by the output limit.

use alloc::vec::Vec;
use std::io::{ErrorKind, Read};

pub(crate) mod input;
mod v4;
use input::Input;

use crate::container;
use crate::parse::chunks::{self, Kind, Rules};
use crate::parse::header::{Coding, SIGNATURE};
use crate::parse::payload::{self, Head, HeadInput};
use crate::{ByteReader, Error, ImageInfo, Limits, Metadata};

/// Byte source with a per-read size limit, an input budget and an optional
/// DATA boundary.
///
/// A suspended byte operation must retain and repoll its original future.
/// Dropping it discards partial-operation state: abandon the entire source,
/// never restart an operation on that source after cancellation.
pub(crate) struct Src<R> {
    r: R,
    max: u64,
    /// Total input bytes this decode may consume (a session's remaining
    /// cumulative allowance; `u64::MAX` for the unrestricted reader).
    budget: u64,
    /// Bytes served to the decoder so far.
    pub(crate) pos: u64,
    /// Bytes taken from the reader (served, plus read ahead to finish a
    /// checksum piece).
    pulled: u64,
    /// A bounded DATA chunk may not consume the following chunk's bytes.
    end: Option<u64>,
    /// The unit records' grammar (SPEC 2.7) while a payload's records are
    /// read, with the head's hint.
    walk: Option<crate::parse::records::Walk>,
    /// A check-tail payload (SPEC 2.7): its main record count while its
    /// records are read, then what its trailing unit index is checked
    /// against.
    tail: Tail,
    /// File checksum pieces, when the header announced them and checking is on.
    ck: Option<alloc::boxed::Box<Check>>,
    /// The announced algorithm id when checking is on but this build does not
    /// know the algorithm: the table must still name the same id.
    ck_unknown: Option<Option<u8>>,
    /// The header version the payload's record ids are read under.
    version: u8,
}

/// A check-tail payload's records, for its trailing unit index (SPEC 2.7).
#[derive(Default)]
pub(crate) enum Tail {
    #[default]
    None,
    /// The head said check tail: keep this many spans.
    Keep(usize),
    /// The payload ended: its spans, head bytes and payload bytes.
    Kept {
        spans: Vec<u64>,
        head_len: u64,
        data_len: u64,
    },
    V4(v4::Index),
}

/// Largest piece read ahead so it is checked before its bytes are served.
const MAX_AHEAD: u64 = 4 << 20;

/// Piecewise checking of the bytes a streamed decode consumes (SPEC 1.4).
struct Check {
    log2: u8,
    /// The header record's algorithm id (the table must name it too).
    algo_id: Option<u8>,
    /// Covered bytes hashed so far (the cSUM chunk is not covered).
    covered: u64,
    state: crate::Hasher,
    /// Digests of finished pieces whose table has not arrived yet.
    pending: Vec<crate::Digest>,
    table: Option<crate::checksum::OwnedTable>,
    /// Pieces compared with the table.
    checked: u64,
    /// Checked bytes read ahead and not yet served.
    ahead: Vec<u8>,
    ahead_pos: usize,
}

impl Check {
    fn piece(&self) -> u64 {
        1 << self.log2
    }

    /// Whether reads go through the read-ahead buffer.
    fn buffered(&self) -> bool {
        self.ahead_pos < self.ahead.len() || self.ahead_mode()
    }

    /// Whether pieces are read ahead and checked before they are served.
    fn ahead_mode(&self) -> bool {
        // Past the covered length, bytes come raw: any is one too many.
        self.table
            .as_ref()
            .is_some_and(|t| self.covered < t.covered_len)
            && self.piece() <= MAX_AHEAD
    }

    fn compare(&mut self, digest: &crate::Digest) -> Result<(), Error> {
        let t = self.table.as_ref().unwrap().view();
        if self.checked >= t.pieces() || !t.matches(self.checked, digest) {
            return Err(crate::checksum::MISMATCH);
        }
        self.checked += 1;
        Ok(())
    }

    fn finish_piece(&mut self, input: &mut impl Input) -> Result<(), Error> {
        let digest = self.state.finish();
        self.state.reset();
        if self.table.is_some() {
            self.compare(&digest)
        } else {
            input.reserve(&mut self.pending, 1)?;
            self.pending.push(digest);
            Ok(())
        }
    }

    /// Hashes covered bytes, comparing each piece as it completes.
    fn cover(&mut self, mut bytes: &[u8], input: &mut impl Input) -> Result<(), Error> {
        if bytes.is_empty() {
            return Ok(());
        }
        if let Some(t) = &self.table
            && self.covered + bytes.len() as u64 > t.covered_len
        {
            return Err(Error::Corrupt("cSUM length does not match the file"));
        }
        while !bytes.is_empty() {
            let room = self.piece() - (self.covered & (self.piece() - 1));
            // Clamp while still wide: a valid 2^32-byte piece must not
            // truncate to zero on a 32-bit reader.
            let n = room.min(bytes.len() as u64) as usize;
            self.state.update(&bytes[..n]);
            self.covered += n as u64;
            bytes = &bytes[n..];
            if self.covered & (self.piece() - 1) == 0 {
                self.finish_piece(input)?;
            }
        }
        // The short last piece completes at the covered length.
        if let Some(t) = &self.table
            && self.covered == t.covered_len
            && self.covered & (self.piece() - 1) != 0
        {
            self.finish_piece(input)?;
        }
        Ok(())
    }

    /// The table arrived: compares the pieces finished before it.
    fn set_table(
        &mut self,
        t: crate::checksum::Table<'_>,
        input: &mut impl Input,
    ) -> Result<(), Error> {
        t.check(self.log2, None)?;
        if t.covered_len < self.covered {
            return Err(Error::Corrupt("cSUM length does not match the file"));
        }
        input.account(t.digests.len())?;
        self.table = Some(crate::checksum::OwnedTable::new(&t)?);
        for d in core::mem::take(&mut self.pending) {
            self.compare(&d)?;
        }
        if self.covered == t.covered_len && self.covered & (self.piece() - 1) != 0 {
            self.finish_piece(input)?;
        }
        Ok(())
    }

    /// End of input: every covered byte arrived and every piece matched.
    fn finish(&self) -> Result<(), Error> {
        crate::checksum::presence(true, self.table.is_some())?;
        let t = self.table.as_ref().unwrap();
        if self.covered != t.covered_len {
            return Err(Error::Corrupt("cSUM length does not match the file"));
        }
        debug_assert_eq!(self.checked, t.covered_len.div_ceil(self.piece()));
        Ok(())
    }
}

impl<R: Input> Src<R> {
    #[cfg(feature = "lab")]
    pub(crate) fn component(reader: R, max: u64, budget: u64) -> Self {
        Self {
            r: reader,
            max,
            budget,
            pos: 0,
            pulled: 0,
            end: None,
            walk: None,
            tail: Tail::None,
            ck: None,
            ck_unknown: None,
            version: crate::parse::header::HEADER_VERSION,
        }
    }

    /// The same encoded checksum state serves packed and component readers.
    pub(crate) fn start_checksum(
        &mut self,
        announced: Option<crate::checksum::Announced>,
        verify: bool,
        prefix: &[u8],
    ) -> Result<(), Error> {
        if let (true, Some(a)) = (verify, announced) {
            let id = a.algorithm_id()?;
            let log2 = a.log2()?;
            self.ck_unknown = a.algorithm().is_none().then_some(id);
            if let Some(algo) = a.algorithm() {
                self.r.account(size_of::<Check>())?;
                let mut ck = alloc::boxed::Box::new(Check {
                    log2,
                    algo_id: id,
                    covered: 0,
                    state: crate::Hasher::new(algo),
                    pending: Vec::new(),
                    table: None,
                    checked: 0,
                    ahead: Vec::new(),
                    ahead_pos: 0,
                });
                ck.cover(prefix, &mut self.r)?;
                self.ck = Some(ck);
            }
        }
        Ok(())
    }

    #[cfg(feature = "lab")]
    pub(crate) fn component_boundary(&mut self, end: Option<u64>) -> Result<(), Error> {
        if self.end.is_some_and(|old| self.pos != old) {
            return Err(Error::Corrupt("component record boundary"));
        }
        self.end = end;
        Ok(())
    }

    #[cfg(feature = "lab")]
    pub(crate) fn finish_checksum(&self) -> Result<(), Error> {
        if let Some(ck) = &self.ck {
            ck.finish()?;
        }
        Ok(())
    }

    fn read_end(&self, len: u64) -> Result<u64, Error> {
        let end = self.pos.checked_add(len).ok_or(Error::TooLarge)?;
        if self.end.is_some_and(|bound| end > bound) {
            return Err(Error::Corrupt("read past DATA boundary"));
        }
        if end > self.budget {
            return Err(Error::TooLarge);
        }
        Ok(end)
    }

    /// Reads from the transport (counted against the budget, not hashed).
    async fn raw_async(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        let room = self.budget.saturating_sub(self.pulled);
        let n = (buf.len() as u64).min(room) as usize;
        if n == 0 && !buf.is_empty() {
            return Err(Error::TooLarge);
        }
        loop {
            match self.r.read(&mut buf[..n]).await {
                Ok(got) => {
                    self.pulled += got as u64;
                    return Ok(got);
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => return Err(Error::Read),
            }
        }
    }

    /// Refills the read-ahead buffer with the rest of the current piece and
    /// checks it. Returns the bytes now available (0 at the end of input).
    async fn refill_async(&mut self) -> Result<usize, Error> {
        let mut ck = self.ck.take().unwrap();
        let need = ck.piece() - (ck.covered & (ck.piece() - 1));
        let t = ck.table.as_ref().unwrap();
        let need = need.min(t.covered_len - ck.covered) as usize;
        let mut buf = core::mem::take(&mut ck.ahead);
        buf.clear();
        if self.r.reserve(&mut buf, need).is_err() {
            self.ck = Some(ck);
            return Err(Error::TooLarge);
        }
        buf.resize(need, 0);
        let mut got = 0;
        let result = loop {
            if got == need {
                break Ok(());
            }
            match self.raw_async(&mut buf[got..]).await {
                Ok(0) => break Ok(()),
                Ok(n) => got += n,
                Err(e) => break Err(e),
            }
        };
        buf.truncate(got);
        let checked = result.and_then(|_| ck.cover(&buf, &mut self.r));
        ck.ahead = buf;
        ck.ahead_pos = 0;
        self.ck = Some(ck);
        checked.map(|_| got)
    }

    /// Fills `buf` from read-ahead or the transport, hashing what it takes.
    /// Returns the bytes filled (short only at the end of input).
    async fn fill_async(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        let mut done = 0;
        while done < buf.len() {
            let Some(ck) = &mut self.ck else {
                match self.raw_async(&mut buf[done..]).await? {
                    0 => break,
                    n => done += n,
                }
                continue;
            };
            if ck.ahead_pos < ck.ahead.len() {
                let n = (ck.ahead.len() - ck.ahead_pos).min(buf.len() - done);
                buf[done..done + n].copy_from_slice(&ck.ahead[ck.ahead_pos..][..n]);
                ck.ahead_pos += n;
                done += n;
                continue;
            }
            if ck.ahead_mode() {
                if self.refill_async().await? == 0 {
                    break;
                }
                continue;
            }
            let n = self.raw_async(&mut buf[done..]).await?;
            if n == 0 {
                break;
            }
            self.ck
                .as_mut()
                .unwrap()
                .cover(&buf[done..done + n], &mut self.r)?;
            done += n;
        }
        Ok(done)
    }

    pub(crate) async fn exact_async(&mut self, buf: &mut [u8]) -> Result<(), Error> {
        let end = self.read_end(buf.len() as u64)?;
        if self.fill_async(buf).await? != buf.len() {
            return Err(Error::Corrupt("truncated file"));
        }
        self.pos = end;
        Ok(())
    }

    pub(crate) async fn u8_async(&mut self) -> Result<u8, Error> {
        let mut b = [0u8];
        self.exact_async(&mut b).await?;
        Ok(b[0])
    }

    /// The next byte, or None at a clean end of input.
    async fn try_u8_async(&mut self) -> Result<Option<u8>, Error> {
        let mut b = [0u8];
        if self.budget <= self.pulled
            && self
                .ck
                .as_ref()
                .is_none_or(|c| c.ahead_pos >= c.ahead.len())
        {
            // At the budget: only a clean end of input may follow.
            let mut probe = [0u8];
            return match self.r.read(&mut probe).await {
                Ok(0) => Ok(None),
                Ok(_) => Err(Error::TooLarge),
                Err(_) => Err(Error::Read),
            };
        }
        if self.fill_async(&mut b).await? == 0 {
            return Ok(None);
        }
        self.pos = self.read_end(1)?;
        Ok(Some(b[0]))
    }

    /// The next chunk tag, or None at a clean end of input. A `cSUM` tag
    /// before its table is known is not covered: the chunk's bytes are
    /// read with `uncovered_varint` and `checksum_chunk`.
    pub(crate) async fn tag_async(&mut self) -> Result<Option<[u8; 4]>, Error> {
        if self.ck.as_ref().is_some_and(|c| c.table.is_none()) {
            let mut ck = self.ck.take();
            let r = (async {
                let Some(b0) = self.try_u8_async().await? else {
                    return Ok(None);
                };
                let mut tag = [b0, 0, 0, 0];
                self.exact_async(&mut tag[1..]).await?;
                Ok(Some(tag))
            })
            .await;
            let covered = match (&r, &mut ck) {
                (Ok(Some(tag)), Some(c)) if *tag != crate::checksum::CSUM => {
                    c.cover(tag, &mut self.r)
                }
                _ => Ok(()),
            };
            self.ck = ck;
            covered?;
            return r;
        }
        let Some(b0) = self.try_u8_async().await? else {
            return Ok(None);
        };
        let mut tag = [b0, 0, 0, 0];
        self.exact_async(&mut tag[1..]).await?;
        Ok(Some(tag))
    }

    /// The cSUM length field is outside checksum coverage. Keep the state
    /// owned by this operation until it completes, including on read errors.
    pub(crate) async fn uncovered_varint(&mut self) -> Result<u64, Error> {
        let ck = self.ck.take();
        let r = self.varint_async(None).await;
        self.ck = ck;
        r
    }

    /// Appends `n` bytes to `out`; on error `out` keeps its original length.
    ///
    /// `n` is a declared length, and so is a bounded `DATA` payload's size:
    /// neither proves the transport will deliver the bytes. Spare capacity
    /// left by an earlier record is filled in place; beyond it, `out` grows
    /// only by the bytes the reader actually delivers, so a truncated tail
    /// cannot pin a multi-gigabyte reservation.
    pub(crate) async fn take_async(&mut self, n: u64, out: &mut Vec<u8>) -> Result<(), Error> {
        if n > self.max {
            return Err(Error::TooLarge);
        }
        // A declared record that crosses DATA must fail before reserving its
        // buffer, even if it fits the caller's per-read byte limit.
        self.read_end(n)?;
        let start = out.len();
        if let Ok(len) = usize::try_from(n)
            && out.capacity() - start >= len
        {
            out.resize(start + len, 0);
            let r = self.exact_async(&mut out[start..]).await;
            if r.is_err() {
                out.truncate(start);
            }
            return r;
        }
        if self.ck.as_ref().is_some_and(|c| c.buffered()) {
            // Checked pieces arrive through the read-ahead buffer: grow by
            // what is delivered, at most a piece at a time.
            let mut left = n;
            let r = loop {
                if left == 0 {
                    break Ok(());
                }
                let step = left.min(64 << 10) as usize;
                let at = out.len();
                if self.r.reserve(out, step).is_err() {
                    break Err(Error::TooLarge);
                }
                out.resize(at + step, 0);
                match self.fill_async(&mut out[at..]).await {
                    Ok(got) if got == step => left -= step as u64,
                    Ok(_) => break Err(Error::Corrupt("truncated file")),
                    Err(e) => break Err(e),
                }
            };
            if r.is_err() {
                out.truncate(start);
                return r;
            }
            self.pos = self.read_end(n)?;
            return Ok(());
        }
        let room = self.budget.saturating_sub(self.pulled);
        let r = self.r.append(n.min(room), out).await;
        // Bytes appended before an I/O error were consumed from the reader.
        let got = (out.len() - start) as u64;
        self.pulled += got;
        self.pos = self.pos.checked_add(got).ok_or(Error::TooLarge)?;
        if let Err(e) = r {
            out.truncate(start);
            return Err(match e.kind() {
                ErrorKind::OutOfMemory => Error::TooLarge,
                _ => Error::Read,
            });
        }
        if got != n {
            out.truncate(start);
            return Err(if n > room {
                Error::TooLarge
            } else {
                Error::Corrupt("truncated file")
            });
        }
        if let Some(ck) = &mut self.ck {
            ck.cover(&out[start..], &mut self.r)?;
        }
        Ok(())
    }

    /// Replaces `out` with the payload of the next main record, which must
    /// be `kind` (SPEC 2.7); its side record, if any, is read and skipped.
    pub(crate) async fn record_async(&mut self, out: &mut Vec<u8>, kind: u64) -> Result<(), Error> {
        self.record_admitted_async(out, kind, &|_| Ok(())).await
    }

    pub(crate) async fn record_admitted_async(
        &mut self,
        out: &mut Vec<u8>,
        kind: u64,
        admit: &impl Fn(usize) -> Result<(), Error>,
    ) -> Result<(), Error> {
        loop {
            let at = self.pos;
            #[cfg(feature = "lab")]
            if crate::trace::on() {
                let (label, region) = crate::trace::pop_next()
                    .unwrap_or_else(|| (alloc::string::String::from("record"), None));
                crate::trace::tspan!("record", "2.7", label, at);
                if let Some([x, y, w, h]) = region {
                    crate::trace::region(x, y, w, h);
                }
            }
            let id = self.varint_async(None).await?;
            crate::trace::tfield!("record id", id, at, self.pos);
            let len_at = self.pos;
            let n = self.varint_async(None).await?;
            if self.remaining().is_some_and(|room| n > room) {
                return Err(Error::Corrupt("record past end of DATA"));
            }
            crate::trace::tfield!("length", n, len_at, self.pos);
            admit(usize::try_from(n).map_err(|_| Error::TooLarge)?)?;
            out.clear();
            let body = self.pos;
            crate::trace::tspan!("payload", "2.4", alloc::format!("payload, {n} bytes"), body);
            self.take_async(n, out).await?;
            crate::trace::window(out, body);
            crate::trace::exit(self.pos);
            crate::trace::exit(self.pos);
            let walk = self
                .walk
                .get_or_insert_with(|| crate::parse::records::Walk::new(self.version, None));
            if walk.head(at, self.pos, id, kind)? {
                if walk.ordinals() {
                    let expected = (walk.used() - 1) as u64;
                    let (ord, ord_len) = {
                        let mut pr = ByteReader::new(out);
                        let ord = pr.varint()?;
                        (ord, pr.pos)
                    };
                    if ord != expected {
                        return Err(Error::Corrupt("unit record ordinal"));
                    }
                    crate::trace::tfield!("unit record ordinal", ord, body, body + ord_len as u64);
                    out.drain(..ord_len);
                    crate::trace::window(out, body + ord_len as u64);
                }
                return Ok(());
            }
        }
    }

    /// Starts a payload's records with the head's unit table hint, whose
    /// entries must fit the bytes left in a bounded payload.
    #[cfg(any(feature = "native-jpeg", test))]
    pub(crate) fn begin_records(&mut self, hint: Option<Vec<u64>>) -> Result<(), Error> {
        self.begin_records_with(hint, false)
    }

    pub(crate) fn begin_records_with(
        &mut self,
        hint: Option<Vec<u64>>,
        ordinals: bool,
    ) -> Result<(), Error> {
        if let Some(h) = &hint {
            let room = self.remaining().unwrap_or(u64::MAX);
            crate::parse::records::check_hint(h, h.len(), room)?;
        }
        let mut walk = crate::parse::records::Walk::with_ordinals(self.version, hint, ordinals);
        if let Tail::Keep(n) = self.tail {
            walk.keep_spans(n)?;
        }
        self.walk = Some(walk);
        Ok(())
    }

    /// Ends a payload's records (the end of DATA): the hint must be used
    /// up. Returns the spans kept for a trailing unit index.
    pub(crate) fn end_records(&mut self) -> Result<Option<Vec<u64>>, Error> {
        match self.walk.take() {
            Some(mut w) => w.finish().map(|()| w.take_spans()),
            None => Ok(None),
        }
    }

    /// Bytes left in a bounded `DATA` payload, or None when open.
    pub(crate) fn remaining(&self) -> Option<u64> {
        self.end.map(|e| e.saturating_sub(self.pos))
    }

    /// Replaces `out` with the payloads of the next `n` main records, of
    /// kind `kind`. When `out` already holds `n` records, their buffers are
    /// reused.
    ///
    /// `n` describes the stream, never its allocation: each record consumes at
    /// least one byte, so a bounded payload with fewer bytes left can never
    /// deliver `n` records and is rejected before `out` grows, while an open
    /// payload grows `out` only as records actually arrive.
    pub(crate) async fn records_async(
        &mut self,
        out: &mut Vec<Vec<u8>>,
        n: usize,
        kind: u64,
    ) -> Result<(), Error> {
        if out.len() == n {
            for rec in out.iter_mut() {
                self.record_async(rec, kind).await?;
            }
            return Ok(());
        }
        out.clear();
        if let Some(room) = self.remaining() {
            crate::parse::records::check_count(n, room)?;
        }
        while out.len() < n {
            if out.len() == out.capacity() {
                let grow = out.capacity().max(1).min(n - out.len());
                self.r.reserve(out, grow)?;
            }
            let mut rec = Vec::new();
            self.record_async(&mut rec, kind).await?;
            out.push(rec);
        }
        Ok(())
    }

    /// A LEB128 varint; its bytes are also appended to `raw` when given.
    pub(crate) async fn varint_async(
        &mut self,
        mut raw: Option<&mut Vec<u8>>,
    ) -> Result<u64, Error> {
        let mut bytes = [0u8; 10];
        for i in 0..bytes.len() {
            let b = self.u8_async().await?;
            bytes[i] = b;
            if let Some(r) = raw.as_deref_mut() {
                self.r.reserve(r, 1)?;
                r.push(b);
            }
            if b < 0x80 || i == 9 {
                let mut iter = bytes[..=i].iter();
                return crate::read_varint(|| Ok(*iter.next().unwrap()));
            }
        }
        unreachable!("ten bytes always finish a u64 varint")
    }
}

// Blocking transport oracles retain the exact same byte operations.
#[cfg(all(test, feature = "encode"))]
impl<R: Read> Src<R> {
    pub(crate) fn take(&mut self, n: u64, out: &mut Vec<u8>) -> Result<(), Error> {
        payload::ready(self.take_async(n, out))
    }
    pub(crate) fn record_admitted(
        &mut self,
        out: &mut Vec<u8>,
        kind: u64,
        admit: &impl Fn(usize) -> Result<(), Error>,
    ) -> Result<(), Error> {
        payload::ready(self.record_admitted_async(out, kind, admit))
    }
}

/// The trailing unit index in a `uIDX` chunk's `payload` (whole length
/// `chunk_len`) against the spans of a check-tail payload (SPEC 2.7), as the
/// slice parsers check it; nothing when the head did not say check tail or
/// the footer does not name the chunk.
fn check_index(tail: &Tail, payload: &[u8], chunk_len: u64) -> Result<(), Error> {
    use crate::parse::records;
    let Tail::Kept {
        spans,
        head_len,
        data_len,
    } = tail
    else {
        return Ok(());
    };
    let Some(index) = records::index_footer(payload, chunk_len) else {
        return Ok(());
    };
    let entries = records::index_entries(index, *data_len, *head_len, spans.len())?;
    if entries != *spans {
        return Err(records::MISMATCH);
    }
    Ok(())
}

/// A `cSUM` chunk of `n` payload bytes after its length field: the table
/// (compared with the pieces hashed so far) when checksums are verified;
/// its bytes are not covered.
pub(crate) async fn checksum_chunk<R: Input>(
    src: &mut Src<R>,
    kind: Kind,
    n: u64,
    payload: &mut Vec<u8>,
) -> Result<(), Error> {
    payload.clear();
    let ck = src.ck.take();
    let result = src.take_async(n, payload).await;
    src.ck = ck;
    result?;
    if kind == Kind::Checksum {
        if let Some(id) = src.ck_unknown {
            // An algorithm this decoder does not know: nothing to check
            // beyond the table naming the announced algorithm.
            return crate::checksum::same_algorithm(id, payload);
        }
        crate::checksum::presence(src.ck.is_some(), true)?;
        crate::checksum::same_algorithm(src.ck.as_ref().unwrap().algo_id, payload)?;
        match crate::checksum::parse_payload(payload)? {
            // A legacy record with an algorithm this decoder does not know:
            // nothing to check.
            None => src.ck = None,
            Some(t) => src.ck.as_mut().unwrap().set_table(t, &mut src.r)?,
        }
    }
    Ok(())
}

/// A mismatching piece explains a decode, parse or chunk-rule error (SPEC
/// 1.4) when the table came before the failing bytes: pieces up to 4 MiB
/// are checked before they are served, and a larger piece is hashed as it
/// is served, so the rest of the piece under the failing bytes is read and
/// compared, and a mismatch is reported in place of `e`. With the table
/// still to come, `e` stands: the reader does not read on to it. Rows were
/// delivered before `e` either way.
async fn explain<R: Input>(src: &mut Src<R>, e: Error) -> Error {
    let explainable = crate::checksum::explainable(&e);
    let Some(ck) = src.ck.as_deref() else {
        return e;
    };
    let partial = ck.covered & (ck.piece() - 1);
    let (true, Some(t), true) = (explainable, ck.table.as_ref(), partial != 0) else {
        return e;
    };
    let rest = (ck.piece() - partial).min(t.covered_len - ck.covered);
    // The piece's bytes after the failure, whatever chunk they belong to.
    src.walk = None;
    src.end = None;
    match skip(src, rest).await {
        Err(m) if m == crate::checksum::MISMATCH => m,
        _ => e,
    }
}

/// Reads and drops `n` bytes (hashed when checksums are verified).
async fn skip<R: Input>(src: &mut Src<R>, mut n: u64) -> Result<(), Error> {
    let mut scratch = [0u8; 8192];
    while n != 0 {
        let take = n.min(scratch.len() as u64) as usize;
        src.exact_async(&mut scratch[..take]).await?;
        n -= take as u64;
    }
    Ok(())
}

/// The stream as the input of a payload head (`parse::payload`): the head
/// parser reads exactly the head's bytes, so no reader-side copy of the
/// head's layout finds where it ends.
struct StreamHead<'s, R> {
    pending: Option<u8>,
    src: &'s mut Src<R>,
    /// What the last [`HeadInput::bytes`] returned.
    buf: Vec<u8>,
}

impl<'s, R: Input> StreamHead<'s, R> {
    fn new(src: &'s mut Src<R>) -> Self {
        StreamHead {
            pending: None,
            src,
            buf: Vec::new(),
        }
    }

    /// The payload's next byte, or None at its end: the `DATA` bound, or a
    /// clean end of input when the payload is open. Within a bounded
    /// payload the end of input truncates the file.
    async fn more_async(&mut self) -> Result<Option<u8>, Error> {
        if let Some(b) = self.pending.take() {
            return Ok(Some(b));
        }
        match self.src.remaining() {
            Some(0) => Ok(None),
            Some(_) => self.src.u8_async().await.map(Some),
            None => self.src.try_u8_async().await,
        }
    }

    /// The next byte; past the payload's end, the error a slice head reader
    /// ([`crate::ByteReader`]) gives.
    async fn next_async(&mut self) -> Result<u8, Error> {
        self.more_async().await?.ok_or(Error::Corrupt("truncated"))
    }

    /// The stream position of the next byte the head parser reads.
    fn at(&self) -> u64 {
        self.src.pos - u64::from(self.pending.is_some())
    }
}

impl<R: Input> payload::AsyncHeadInput for StreamHead<'_, R> {
    async fn peek_u8(&mut self) -> Result<u8, Error> {
        if self.pending.is_none() {
            self.pending = Some(self.next_async().await?);
        }
        Ok(self.pending.unwrap())
    }
    async fn u8(&mut self) -> Result<u8, Error> {
        self.next_async().await
    }
    async fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_le_bytes([
            self.next_async().await?,
            self.next_async().await?,
        ]))
    }
    async fn u32(&mut self) -> Result<u32, Error> {
        let mut b = [0u8; 4];
        for x in &mut b {
            *x = self.next_async().await?;
        }
        Ok(u32::from_le_bytes(b))
    }
    async fn bytes(&mut self, n: usize) -> Result<&[u8], Error> {
        let at = self.at();
        self.buf.clear();
        let mut rest = n as u64;
        if rest != 0
            && let Some(b) = self.pending.take()
        {
            self.src.r.reserve(&mut self.buf, 1)?;
            self.buf.push(b);
            rest -= 1;
        }
        match self.src.remaining() {
            Some(room) if room < rest => return Err(Error::Corrupt("truncated")),
            Some(_) => self.src.take_async(rest, &mut self.buf).await?,
            // An open payload ends at a clean end of input.
            None => {
                for _ in 0..rest {
                    let b = self
                        .more_async()
                        .await?
                        .ok_or(Error::Corrupt("truncated"))?;
                    self.src.r.reserve(&mut self.buf, 1)?;
                    self.buf.push(b);
                }
            }
        }
        crate::trace::window(&self.buf, at);
        Ok(&self.buf)
    }
    fn pos(&self) -> u64 {
        self.at()
    }
    fn mark(&self) -> crate::trace::Mark {
        crate::trace::Mark::Abs(self.at())
    }
}

impl<R: Read> HeadInput for StreamHead<'_, R> {
    fn peek_u8(&mut self) -> Result<u8, Error> {
        payload::ready(payload::AsyncHeadInput::peek_u8(self))
    }
    fn u8(&mut self) -> Result<u8, Error> {
        payload::ready(payload::AsyncHeadInput::u8(self))
    }
    fn u16(&mut self) -> Result<u16, Error> {
        payload::ready(payload::AsyncHeadInput::u16(self))
    }
    fn u32(&mut self) -> Result<u32, Error> {
        payload::ready(payload::AsyncHeadInput::u32(self))
    }
    fn varint(&mut self) -> Result<u64, Error> {
        payload::ready(payload::AsyncHeadInput::varint(self))
    }
    fn bytes(&mut self, n: usize) -> Result<&[u8], Error> {
        payload::ready(payload::AsyncHeadInput::bytes(self, n))
    }
    fn pos(&self) -> u64 {
        self.at()
    }
    fn mark(&self) -> crate::trace::Mark {
        crate::trace::Mark::Abs(self.at())
    }
}

/// Decodes an IMI file from `reader`, calling `on_row(info, y, row)` for
/// every row in order (rows hold native-endian samples, as
/// [`decode`](crate::decode) produces). Returns the image info and all
/// metadata, including chunks that follow the pixel data. Working memory
/// is one stripe (lossless: one coded stripe and one row; lossy: one coded
/// record of each kind and one stripe of rows), plus 8 bytes per unit record
/// of a payload whose head says check tail (SPEC 2.7): its trailing unit
/// index arrives after the rows and is compared with the records then.
/// Rows delivered before this function returns success are provisional:
/// later entropy validation or trailing chunks may still produce an error.
/// A `cSUM` file checksum (SPEC 1.4) is checked piece by piece as bytes
/// arrive. When its table precedes `DATA` (as this library writes it), each
/// piece of 4 MiB or less is read ahead and checked before any of its bytes
/// is decoded, so no row comes from a bad piece; a table at the end of the
/// file is compared when it arrives, after the rows, which a mismatch
/// (`Error::Corrupt("file checksum mismatch")`) cannot retract.
/// [`Limits::verify_checksum`] turns this off.
/// `limits` does not bound cumulative metadata or total working memory.
pub fn decode_reader<R: Read>(
    reader: R,
    limits: &Limits,
    mut on_row: impl FnMut(&ImageInfo, u32, &[u8]) -> Result<(), Error>,
) -> Result<(ImageInfo, Metadata), Error> {
    decode_reader_with(
        reader,
        limits,
        u64::MAX,
        &crate::parse::limits::Unrestricted,
        &mut on_row,
    )
    .0
}

/// [`decode_reader`] under an admission policy: `budget` caps the input
/// bytes consumed, `admission` charges headers, chunks, metadata, models
/// and planned working memory as they are parsed, and checks cancellation
/// per stripe. Returns the result and the input bytes consumed (charged even
/// when decoding fails).
pub(crate) fn decode_reader_with<R: Read, A: crate::parse::limits::Admission>(
    reader: R,
    limits: &Limits,
    budget: u64,
    admission: &A,
    on_row: &mut dyn FnMut(&ImageInfo, u32, &[u8]) -> Result<(), Error>,
) -> (Result<(ImageInfo, Metadata), Error>, u64) {
    decode_reader_rows_with(reader, limits, budget, admission, on_row)
}

pub(crate) fn decode_reader_rows_with<R: Read, A: crate::parse::limits::Admission>(
    reader: R,
    limits: &Limits,
    budget: u64,
    admission: &A,
    on_row: &mut (impl input::Rows + ?Sized),
) -> (Result<(ImageInfo, Metadata), Error>, u64) {
    let mut src = Src {
        r: reader,
        max: limits.max_bytes,
        budget,
        pos: 0,
        pulled: 0,
        end: None,
        walk: None,
        tail: Tail::None,
        ck: None,
        ck_unknown: None,
        version: crate::parse::header::HEADER_VERSION,
    };
    let result = payload::ready(read_file(&mut src, limits, admission, on_row));
    (result, src.pulled)
}

/// Own all input/framing state inside one retained operation.
pub(crate) async fn read_owned<R: Input, A: crate::parse::limits::Admission>(
    reader: R,
    limits: Limits,
    admission: A,
    mut output: impl input::Rows,
) -> Result<(ImageInfo, Metadata), Error> {
    let mut src = Src {
        r: reader,
        max: limits.max_bytes,
        budget: u64::MAX,
        pos: 0,
        pulled: 0,
        end: None,
        walk: None,
        tail: Tail::None,
        ck: None,
        ck_unknown: None,
        version: crate::parse::header::HEADER_VERSION,
    };
    read_file(&mut src, &limits, &admission, &mut output).await
}

async fn read_file<R: Input, A: crate::parse::limits::Admission>(
    src: &mut Src<R>,
    limits: &Limits,
    admission: &A,
    on_row: &mut (impl input::Rows + ?Sized),
) -> Result<(ImageInfo, Metadata), Error> {
    admission.check()?;
    crate::trace::tspan!("file", "1", "IMI file", 0);
    let mut pre = Vec::new();
    src.r.reserve(&mut pre, 8)?;
    pre.resize(8, 0);
    src.exact_async(&mut pre).await.map_err(|e| match e {
        Error::Corrupt(_) => Error::NotImij,
        e => e,
    })?;
    if pre[..] != SIGNATURE {
        return Err(Error::NotImij);
    }
    let n = src.varint_async(Some(&mut pre)).await?;
    src.r.header_bytes(n)?;
    src.take_async(n, &mut pre).await?;
    // `pre` holds file bytes 0.. so its addresses map to absolute offsets.
    crate::trace::window(&pre, 0);
    let hp = crate::parse::header::read_header_sized(&mut ByteReader::new(&pre))?;
    src.version = hp.header.version;
    let mixed_pages = hp.mixed_pages();
    let crate::parse::header::ParsedHeader {
        header,
        declared,
        checksum: announced,
        ..
    } = hp;
    // The order every route keeps: the header, what this build decodes,
    // the limits, then the chunks and the checksum.
    crate::parse::header::check_decodable(&header, declared)?;
    let info = header.info.clone();
    let bytes = usize::try_from(n).map_err(|_| Error::TooLarge)?;
    crate::parse::limits::check_limits(&info, limits)?;
    admission.header(&info, bytes)?;
    src.r.image(&info)?;
    src.start_checksum(announced.map(|a| a.announced), limits.verify_checksum, &pre)?;
    let mut meta = Metadata {
        orientation: header.orientation,
        ..Default::default()
    };
    let mut rules = Rules::new(mixed_pages, limits.verify_checksum)
        .with_sequence_items(declared.contains(crate::parse::header::Feature::SequenceItems));
    let mut payload = Vec::new();
    let mut pixel_check: Option<crate::pixelsum::RowCheck> = None;
    let mut pixel_count = None;
    let mut frame_chunks = 0u64;
    // The outcome of checking the last chunk when it was a trailing unit
    // index (SPEC 2.7: it counts only as the file's last chunk).
    let mut index: Option<Result<(), Error>> = None;
    // A decode, parse or chunk-rule error with the table read before it: a
    // mismatching piece under the bytes read explains it (SPEC 1.4).
    let walked: Result<(), Error> = async {
        while let Some(tag) = src.tag_async().await? {
            if header.version == 4 && index.is_some() {
                return Err(Error::Corrupt("V4 trailing directory must be final"));
            }
            index = None;
            admission.chunk_start()?;
            let chunk_at = src.pos - 4;
            crate::trace::tspan!(
                "chunk",
                "1.1",
                alloc::format!("{} chunk", alloc::string::String::from_utf8_lossy(&tag)),
                chunk_at
            );
            crate::trace::tfield!(
                "tag",
                alloc::string::String::from_utf8_lossy(&tag),
                chunk_at,
                src.pos
            );
            let len_at = src.pos;
            // A checksum chunk's own bytes are not covered.
            let n = if tag == chunks::CSUM {
                src.uncovered_varint().await?
            } else {
                src.varint_async(None).await?
            };
            crate::trace::tfield!(
                "length+1",
                alloc::format!(
                    "{n} ({})",
                    if n == 0 {
                        "open: runs to end of input".into()
                    } else {
                        alloc::format!("{} payload bytes", n - 1)
                    }
                ),
                len_at,
                src.pos
            );
            let len = n.checked_sub(1);
            let kind = rules.head(tag, len)?;
            if kind == Kind::Data {
                admission.chunk(tag, &[])?;
                let start = src.pos;
                src.end = match len {
                    None => None,
                    Some(l) => Some(start.checked_add(l).ok_or(Error::TooLarge)?),
                };
                if header.version == 4 {
                    let mut checked_row = input::CheckedRows {
                        check: &mut pixel_check,
                        admission,
                        output: on_row,
                    };
                    v4::rows(src, &header, declared, admission, &mut checked_row).await?;
                    if len.is_some_and(|l| src.pos - start != l) {
                        return Err(Error::Corrupt("trailing V4 DATA bytes"));
                    }
                    src.end = None;
                    crate::trace::exit(src.pos);
                    continue;
                }
                match header.coding {
                    Coding::Lossless => crate::trace::tspan!("head", "2", "lossless head", src.pos),
                    Coding::Lossy => crate::trace::tspan!("head", "2.8", "lossy head", src.pos),
                }
                let decoded: Result<(), Error> = async {
                    // The head through the same parser the slice decoders use:
                    // the stream supplies exactly the bytes it reads.
                    let head = payload::read_head_async(
                        &header,
                        declared,
                        &mut StreamHead::new(src),
                        admission,
                    )
                    .await?;
                    let head_len = src.pos - start;
                    // A check-tail head: the spans are kept for the
                    // trailing unit index, which comes after the rows.
                    if head.reserve().is_some_and(|r| r.check_tail()) {
                        let (map, stripes) = head.units();
                        let n = map
                            .count(stripes)
                            .map_err(|_| Error::Corrupt("unit table length"))?;
                        admission.hint(n.saturating_mul(8))?;
                        src.tail = Tail::Keep(n);
                    }
                    let mut checked_row = input::CheckedRows {
                        check: &mut pixel_check,
                        admission,
                        output: on_row,
                    };
                    match head {
                        #[cfg(feature = "native-jpeg")]
                        Head::Native(p) => {
                            crate::jpeg::wire::read_native(src, *p, admission, &mut checked_row)
                                .await?;
                        }
                        Head::Lossless(p) => {
                            crate::v1::read_lossless(src, p, admission, &mut checked_row).await?
                        }
                        Head::Lossy(p) => {
                            crate::lossy::read_lossy(src, p, admission, &mut checked_row).await?
                        }
                    }
                    let spans = src.end_records()?;
                    if len.is_some_and(|l| src.pos - start != l) {
                        return Err(Error::Corrupt("trailing DATA bytes"));
                    }
                    if let Some(spans) = spans {
                        src.tail = Tail::Kept {
                            spans,
                            head_len,
                            data_len: src.pos - start,
                        };
                    }
                    Ok(())
                }
                .await;
                decoded?;
                src.end = None;
                crate::trace::exit(src.pos);
                continue;
            }
            // Every other chunk is bounded (the rules refuse open ones).
            let n = len.unwrap_or(0);
            payload.clear();
            if tag == crate::pixelsum::PSUM {
                // Retain only a bounded header and digest 0, independent of
                // the number of animation frames or pages. No table allocation.
                let take = chunks::prefix_len(&tag, len);
                admission.hint(core::mem::size_of::<crate::pixelsum::Raster>())?;
                src.take_async(take, &mut payload).await?;
                rules.identity(tag, &payload)?;
                admission.chunk(tag, &payload)?;
                if let Some((count, offset, algo)) = crate::pixelsum::prefix(&payload, n)? {
                    pixel_count = Some(count);
                    admission.pixel_checksum(&info)?;
                    let d = payload
                        .get(offset..offset + crate::pixelsum::width_of(algo))
                        .ok_or(Error::Corrupt("short pSUM"))?;
                    pixel_check = crate::pixelsum::RowCheck::new(
                        crate::pixelsum::Table::Known(d, algo),
                        &info,
                    )?;
                }
                skip(src, n - take).await?;
                continue;
            }
            if kind == Kind::Frame {
                frame_chunks += 1;
            }
            if matches!(kind, Kind::Checksum | Kind::Ignored) {
                admission.chunk(tag, &[])?;
                checksum_chunk(src, kind, n, &mut payload).await?;
                crate::trace::exit(src.pos);
                continue;
            }
            if kind == Kind::Plane {
                admission.chunk(tag, &[])?;
                // A sequential RGB consumer need not retain auxiliary tile data.
                // Read and discard bounded slices; seeking consumers use planes::ReadAt.
                skip(src, n).await?;
                crate::trace::exit(src.pos);
                continue;
            }
            let body_at = src.pos;
            src.take_async(n, &mut payload).await?;
            crate::trace::tfield!("payload", alloc::format!("{n} bytes"), body_at, src.pos);
            // Retained metadata (and compressed-metadata expansion) is charged
            // once its bytes arrived and the rules its first bytes show
            // passed; the read itself is bounded by the per-read limit and the
            // input budget.
            if kind.has_body_rules(&tag) {
                rules.identity(tag, &payload)?;
            }
            admission.chunk(tag, &payload)?;
            chunks::record(tag, &payload)?;
            if kind == Kind::Meta {
                container::keep_metadata(&mut meta, tag, &payload)?;
            }
            if kind == Kind::UnitIndex {
                let len = crate::parse::records::chunk_len_of(n);
                index = Some(if header.version == 4 {
                    match &src.tail {
                        Tail::V4(index) => index.check(&payload, len, admission),
                        _ => Err(Error::Corrupt("unexpected V4 trailing directory")),
                    }
                } else {
                    check_index(&src.tail, &payload, len)
                });
            }
            crate::trace::exit(src.pos);
        }
        rules.finish()?;
        if pixel_count.is_some_and(|n| n != frame_chunks + 1) {
            return Err(Error::Corrupt("pSUM image count"));
        }
        if matches!(&src.tail, Tail::V4(tail) if tail.needs_tail()) && index.is_none() {
            return Err(Error::Corrupt("missing V4 trailing directory"));
        }
        index.take().unwrap_or(Ok(()))
    }
    .await;
    if let Err(e) = walked {
        if header.version == 4 {
            return Err(e);
        }
        return Err(explain(src, e).await);
    }
    if let Some(ck) = &src.ck {
        // Rows were already delivered; a short or extended file fails now.
        ck.finish()?;
    }
    if pixel_check.is_some() {
        admission.check()?;
    }
    crate::trace::exit(src.pos);
    Ok((info, meta))
}

/// Test the actual tables returned by a suspended head through production
/// reconstruction, independently of the head endpoint/geometry comparison.
#[cfg(all(test, feature = "encode"))]
pub(crate) fn rows_from_fragmented_head(
    data: &[u8],
    head: Head,
    at: usize,
    version: u8,
) -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(data);
    cursor.set_position(at as u64);
    let mut src = Src {
        r: cursor,
        max: u64::MAX,
        budget: u64::MAX,
        pos: at as u64,
        pulled: at as u64,
        end: Some(data.len() as u64),
        walk: None,
        tail: Tail::None,
        ck: None,
        ck_unknown: None,
        version,
    };
    let mut output = Vec::new();
    let mut on_row = |_: &ImageInfo, _: u32, row: &[u8]| {
        output.extend_from_slice(row);
        Ok(())
    };
    let admission = crate::parse::limits::Unrestricted;
    match head {
        Head::Lossless(p) => payload::ready(crate::v1::read_lossless(
            &mut src,
            p,
            &admission,
            &mut on_row,
        )),
        Head::Lossy(p) => payload::ready(crate::lossy::read_lossy(
            &mut src,
            p,
            &admission,
            &mut on_row,
        )),
        #[cfg(feature = "native-jpeg")]
        Head::Native(p) => payload::ready(crate::jpeg::wire::read_native(
            &mut src,
            *p,
            &admission,
            &mut on_row,
        )),
    }
    .unwrap();
    src.end_records().unwrap();
    assert_eq!(src.pos, data.len() as u64);
    output
}

#[cfg(all(test, feature = "encode"))]
mod fragmented;

#[cfg(all(test, feature = "encode"))]
pub(crate) fn file_from_fragments(file: &[u8]) {
    fragmented::file_pixels(file);
}

#[cfg(all(test, feature = "encode"))]
pub(crate) fn head_from_fragments(
    header: &crate::parse::header::Header,
    declared: crate::parse::header::Features,
    data: &[u8],
    step: usize,
    open: bool,
) -> Result<(Head, usize), Error> {
    fragmented::head(header, declared, data, step, open)
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    #[test]
    fn oversized_compressed_metadata_rejected_before_body_read() {
        let info = ImageInfo::new(1, 1, crate::Channels::Gray, crate::SampleType::U8);
        let mut file = crate::encode(
            &[42],
            1,
            &info,
            &Metadata::default(),
            &crate::EncodeConfig::default(),
        )
        .unwrap();
        file.extend_from_slice(&crate::parse::chunks::MCMP);
        crate::put_varint(
            &mut file,
            crate::parse::chunks::MAX_PACKED_METADATA_RECORD as u64 + 2,
        );
        // There is deliberately no body: a size error proves preflight happens
        // before allocating/reading the attacker-declared compressed record.
        let result = decode_reader(file.as_slice(), &Limits::NONE, |_, _, _| Ok(()));
        assert!(matches!(result, Err(Error::TooLarge)));
    }

    /// The payload head parser reads the same bytes and reaches the same
    /// outcome through a slice ([`ByteReader`]) and through a stream
    /// ([`StreamHead`]), bounded by `DATA` or open to the end of input: real
    /// lossless and lossy heads, every truncation and single-byte changes.
    #[test]
    fn head_input_slice_and_stream_agree() {
        use crate::parse::limits::Unrestricted;
        use crate::parse::payload::read_head;
        use crate::{Channels, EncodeConfig, LossyColor, LossyConfig, LossyEntropy, SampleType};
        let mut files = Vec::new();
        for (w, h, ch) in [
            (9, 7, Channels::Rgb),
            (5, 3, Channels::Rgba),
            (33, 2, Channels::Gray),
        ] {
            let info = ImageInfo::new(w, h, ch, SampleType::U8);
            let px: Vec<u8> = (0..info.row_bytes() * h as usize)
                .map(|i| (i * 37 % 251) as u8)
                .collect();
            let rb = info.row_bytes();
            files.push(
                crate::encode(
                    &px,
                    rb,
                    &info,
                    &Metadata::default(),
                    &EncodeConfig::default(),
                )
                .unwrap(),
            );
            for cfg in [
                LossyConfig::new(85.0),
                LossyConfig::new(40.0).with_color(LossyColor::YCoCg),
                LossyConfig::new(90.0).with_entropy(LossyEntropy::SplitHuf),
            ] {
                if let Ok(f) = crate::encode_lossy(&px, rb, &info, &Metadata::default(), &cfg) {
                    files.push(f);
                }
            }
        }
        assert!(files.len() >= 9);
        let mut compared = 0;
        for file in &files {
            crate::parse::payload::fragmented_tests::decoded_pixels(file);
            let ph = crate::parse::header::read_header_sized(&mut ByteReader::new(file)).unwrap();
            let payload = crate::container::parse(file, false, &crate::v1::payload_len)
                .unwrap()
                .data;
            let mut complete_head = ByteReader::new(payload);
            read_head(&ph.header, ph.declared, &mut complete_head, &Unrestricted).unwrap();
            // Preserve earlier payload witnesses and cover every head EOF boundary.
            let last_cut = complete_head.pos.max(payload.len().min(200));
            let mut variants: Vec<Vec<u8>> =
                (0..=last_cut).map(|n| payload[..n].to_vec()).collect();
            variants.push(payload.to_vec());
            for i in 0..payload.len().min(64) {
                for m in [1u8, 0x80, 0xff] {
                    let mut v = payload.to_vec();
                    v[i] ^= m;
                    variants.push(v);
                }
            }
            for b in &variants {
                crate::parse::payload::fragmented_tests::compare(&ph.header, ph.declared, b, 1);
                crate::parse::payload::fragmented_tests::compare(&ph.header, ph.declared, b, 17);
                for open in [false, true] {
                    let fragmented = head_from_fragments(&ph.header, ph.declared, b, 7, open)
                        .map(|(_, at)| at as u64);
                    let mut r = ByteReader::new(b);
                    let slice = read_head(&ph.header, ph.declared, &mut r, &Unrestricted)
                        .map(|_| r.pos as u64);
                    let mut src = Src {
                        r: std::io::Cursor::new(b.as_slice()),
                        max: u64::MAX,
                        budget: u64::MAX,
                        pos: 0,
                        pulled: 0,
                        end: (!open).then_some(b.len() as u64),
                        walk: None,
                        tail: Tail::None,
                        ck: None,
                        ck_unknown: None,
                        version: crate::parse::header::HEADER_VERSION,
                    };
                    let mut sh = StreamHead::new(&mut src);
                    let stream =
                        read_head(&ph.header, ph.declared, &mut sh, &Unrestricted).map(|_| sh.at());
                    assert_eq!(fragmented, slice, "fragmented transport, open {open}");
                    assert_eq!(
                        slice,
                        stream,
                        "{} bytes, open {open}: {:02x?}",
                        b.len(),
                        &b[..b.len().min(24)]
                    );
                    compared += 1;
                }
            }
        }
        assert!(compared > 5000, "{compared}");
    }

    #[test]
    fn bounded_record_is_rejected_before_buffer_growth() {
        let mut source = Src {
            r: std::io::Cursor::new([0; 8]),
            max: u64::MAX,
            budget: u64::MAX,
            pos: 0,
            pulled: 0,
            end: Some(1),
            walk: None,
            tail: Tail::None,
            ck: None,
            ck_unknown: None,
            version: crate::parse::header::HEADER_VERSION,
        };
        let mut out = Vec::new();
        assert!(source.take(4096, &mut out).is_err());
        assert_eq!(out.capacity(), 0);
        assert_eq!(source.r.position(), 0);
    }
}
