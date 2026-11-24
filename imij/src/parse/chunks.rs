//! Chunks (SPEC 1.1): the tags, their framing, and the rules every reader
//! applies to a file's chunk sequence, whatever it reads from (a slice, a
//! stream, a random-access source) and whether or not it fetches the
//! payloads it does not use.
//!
//! [`Rules`] takes the chunks in file order: [`Rules::head`] the rules a
//! chunk header shows (tag syntax, open chunks, criticality, chunks allowed
//! once, `aNIM` before `DATA`, fixed sizes), [`Rules::identity`] those its
//! first payload bytes show ([`prefix_len`]: the `aNIM` kind, an `mCMP`
//! chunk's identity), [`Rules::body`] those plus an `mCMP` record's syntax,
//! and [`Rules::finish`] what holds for the whole file.
//!
//! Every reader reports the same error for the same chunk sequence, as far
//! as it reads it (SPEC 1.1 lets a partial decoder, such as the region
//! reader, `header` or `previews`, skip metadata payloads and stop at
//! `DATA`). Admission policies (`DecodeSession`) charge a chunk after the
//! rules its first bytes show and before an `mCMP` record's syntax.

use crate::{ByteReader, Error};

pub(crate) const DATA: [u8; 4] = *b"DATA";
pub(crate) const ICCP: [u8; 4] = *b"iCCP";
pub(crate) const EXIF: [u8; 4] = *b"eXIf";
pub(crate) const XMP: [u8; 4] = *b"xMPp";
pub(crate) const MDCV: [u8; 4] = *b"mDCv";
pub(crate) const DWHI: [u8; 4] = *b"dWHI";
pub(crate) const CLLI: [u8; 4] = *b"cLLI";
pub(crate) const PRVW: [u8; 4] = *b"pRVW";
pub(crate) const GMAP: [u8; 4] = *b"gMAP";
/// Animation / pages control (before `DATA`).
pub(crate) const ANIM: [u8; 4] = *b"aNIM";
/// Optional sequence-wide advisory metadata.
pub(crate) const SDEF: [u8; 4] = *b"sDEF";
/// A further frame or page.
pub(crate) const FRAM: [u8; 4] = *b"fRAM";
/// Compressed ICC, Exif or XMP; the original chunk identity remains unique.
pub(crate) const MCMP: [u8; 4] = *b"mCMP";
pub(crate) use crate::checksum::CSUM;
/// The trailing unit index (SPEC 2.7).
pub(crate) use crate::parse::records::UIDX;
/// Maximum expanded size of each compressed metadata block (wire profile cap).
pub(crate) const MAX_PACKED_METADATA: usize = 16 * 1024 * 1024;
/// Bound serialized allocation before a streaming reader sees the inner header.
pub(crate) const MAX_PACKED_METADATA_RECORD: usize = 2 * MAX_PACKED_METADATA + 64;

/// Chunks a file holds at most once (an `mCMP` chunk counts as the chunk
/// it compresses).
const ONCE: [[u8; 4]; 13] = [
    DATA,
    ICCP,
    EXIF,
    XMP,
    MDCV,
    CLLI,
    DWHI,
    GMAP,
    ANIM,
    SDEF,
    CSUM,
    UIDX,
    crate::pixelsum::PSUM,
];

/// Plane records are consumed by the indexed API, never retained as metadata.
pub(crate) fn is_plane_chunk(tag: &[u8; 4]) -> bool {
    matches!(tag, b"pDAT" | b"pREC" | b"pIPG" | b"pIDX" | b"sLOC")
}

#[inline]
pub(crate) fn is_critical(tag: &[u8; 4]) -> bool {
    tag[0].is_ascii_uppercase()
}

/// The rule for an open chunk (length field 0): only `DATA` may be open.
/// Another critical chunk is one this decoder does not know; an ancillary
/// one is malformed.
pub(crate) fn open_chunk(tag: &[u8; 4]) -> Result<(), Error> {
    if *tag == DATA {
        return Ok(());
    }
    Err(if is_critical(tag) {
        Error::Unsupported("unknown critical chunk")
    } else {
        Error::Corrupt("open ancillary chunk")
    })
}

/// The chunk header at `r`: its tag and payload length (`None`: open).
pub(crate) fn read_head(r: &mut ByteReader<'_>) -> Result<([u8; 4], Option<u64>), Error> {
    let tag: [u8; 4] = crate::trace::tfd!(
        "tag",
        r,
        r.bytes(4)?.try_into().unwrap(),
        |tag: &[u8; 4]| alloc::string::String::from_utf8_lossy(tag).into_owned()
    );
    let n = crate::trace::tf!("length+1", r, r.varint()?);
    Ok((tag, n.checked_sub(1)))
}

/// The payload of a bounded chunk of `len` bytes at `r`. The length stays
/// `u64` until it is checked against the bytes present, so a 32-bit reader
/// reports the same "chunk past end of file" as a 64-bit one.
pub(crate) fn payload<'a>(r: &mut ByteReader<'a>, len: u64) -> Result<&'a [u8], Error> {
    if len > r.remaining() as u64 {
        return Err(Error::Corrupt("chunk past end of file"));
    }
    r.bytes(len as usize)
}

/// Walks the chunks of an in-memory file from `r` (just after its header)
/// to `DATA`, framing only (writers editing a finished file): `each(at,
/// end)` sees every chunk before `DATA`, which must be bounded. Returns the
/// offset of the `DATA` chunk and its length field (`None`: open), with `r`
/// at its payload.
pub(crate) fn walk_to_data(
    r: &mut ByteReader<'_>,
    mut each: impl FnMut(usize, usize),
) -> Result<(usize, Option<u64>), Error> {
    loop {
        if r.remaining() == 0 {
            return Err(Error::Corrupt("no DATA chunk"));
        }
        let at = r.pos;
        let (tag, len) = read_head(r)?;
        if tag == DATA {
            return Ok((at, len));
        }
        // Only DATA may be open, and DATA ended the walk.
        let n = match len {
            Some(n) => n,
            None => open_chunk(&tag).map(|_| 0)?,
        };
        payload(r, n)?;
        each(at, r.pos);
    }
}

/// How many payload bytes [`Rules::identity`] reads at the start of a chunk
/// of payload length `len` (`None`: open): up to 4 for `aNIM` (its kind)
/// `mCMP` (the identity), or `dWHI` (the complete value).
pub(crate) fn prefix_len(tag: &[u8; 4], len: Option<u64>) -> u64 {
    if *tag == crate::pixelsum::PSUM {
        len.unwrap_or(0).min(crate::pixelsum::PREFIX_BYTES as u64)
    } else if *tag == ANIM {
        len.unwrap_or(0).min(21)
    } else if matches!(*tag, MCMP | DWHI) {
        len.unwrap_or(0).min(4)
    } else {
        0
    }
}

/// The payload rule beyond [`Rules::identity`]: an `mCMP` record's syntax,
/// validated without expanding it (`payload`: the whole chunk payload).
pub(crate) fn record(tag: [u8; 4], payload: &[u8]) -> Result<(), Error> {
    if tag == crate::pixelsum::PSUM {
        crate::pixelsum::parse(payload)?;
        return Ok(());
    }
    if tag == DWHI {
        diffuse_white(payload)?;
        return Ok(());
    }
    if tag != MCMP {
        return Ok(());
    }
    let packed = payload
        .get(4..)
        .ok_or(Error::Corrupt("short compressed metadata"))?;
    crate::metadata_pack::check(packed, MAX_PACKED_METADATA)
}

/// Validates before constructing the shared type, whose constructor is infallible
/// only for finite, strictly positive values.
pub(crate) fn diffuse_white(payload: &[u8]) -> Result<zenpixels::hdr::DiffuseWhite, Error> {
    let bytes = payload.try_into().map_err(|_| Error::Corrupt("bad dWHI"))?;
    let nits = f32::from_be_bytes(bytes);
    if !nits.is_finite() || nits <= 0.0 {
        return Err(Error::Corrupt("bad dWHI"));
    }
    Ok(zenpixels::hdr::DiffuseWhite::new(nits))
}

/// What a chunk is, for the reader that met it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Data,
    Preview,
    Anim,
    Frame,
    /// The file checksum table (when checksums are verified).
    Checksum,
    /// A `cSUM` chunk with checksums off: skipped, as by a decoder
    /// without them.
    Ignored,
    /// `pDAT`, `pIDX`, `sLOC`: plane records, read through the plane index.
    Plane,
    /// `uIDX`: the trailing unit index (SPEC 2.7), never kept as metadata;
    /// it counts only as the file's last chunk after a check-tail head.
    UnitIndex,
    /// Metadata: ICC, Exif, XMP (raw or `mCMP`), HDR fields, a gain map,
    /// science records and other ancillary chunks.
    Meta,
}

impl Kind {
    /// Whether [`Rules::body`] reads this chunk's payload.
    pub(crate) fn has_body_rules(self, tag: &[u8; 4]) -> bool {
        self == Kind::Anim || matches!(*tag, MCMP | DWHI) || *tag == crate::pixelsum::PSUM
    }
}

/// The chunk rules of SPEC 1.1 (and the parts of 1.3 and 1.4 the chunks
/// show), applied to a file's chunks in order.
pub(crate) struct Rules {
    /// Bit i: [`ONCE`]`[i]` (or its `mCMP` form) was seen.
    seen: u16,
    /// The header declares the feature `mixed_pages` (SPEC 0.1).
    mixed_pages: bool,
    sequence_items: bool,
    /// The `aNIM` chunk's kind byte (`Some(None)`: empty payload).
    anim: Option<Option<u8>>,
    /// `cSUM` chunks are checksum tables (verification on); off, they
    /// are skipped like any unknown ancillary chunk.
    checksums: bool,
    pixel_len: u64,
    pixel_count: Option<u64>,
    image_count: Option<Result<u64, Error>>,
}

impl Rules {
    pub(crate) fn new(mixed_pages: bool, checksums: bool) -> Rules {
        Rules {
            seen: 0,
            mixed_pages,
            sequence_items: false,
            anim: None,
            checksums,
            pixel_len: 0,
            pixel_count: None,
            image_count: None,
        }
    }

    pub(crate) fn with_sequence_items(mut self, present: bool) -> Self {
        self.sequence_items = present;
        self
    }

    fn once(&mut self, tag: &[u8; 4]) -> Result<(), Error> {
        if let Some(i) = ONCE.iter().position(|t| t == tag) {
            if self.seen >> i & 1 == 1 {
                return Err(Error::Corrupt("repeated chunk"));
            }
            self.seen |= 1 << i;
        }
        Ok(())
    }

    fn has(&self, tag: &[u8; 4]) -> bool {
        ONCE.iter()
            .position(|t| t == tag)
            .is_some_and(|i| self.seen >> i & 1 == 1)
    }

    /// The rules a chunk header shows; `len` is the payload length (`None`
    /// for an open chunk).
    pub(crate) fn head(&mut self, tag: [u8; 4], len: Option<u64>) -> Result<Kind, Error> {
        if len.is_none() {
            open_chunk(&tag)?;
        }
        if !tag.iter().all(u8::is_ascii_alphabetic) {
            return Err(Error::Corrupt("invalid chunk tag"));
        }
        if tag == MCMP {
            // The record's identity and contents follow in `body`.
            if len.is_some_and(|n| n > MAX_PACKED_METADATA_RECORD as u64) {
                return Err(Error::TooLarge);
            }
            return Ok(Kind::Meta);
        }
        if is_plane_chunk(&tag) {
            return Ok(Kind::Plane);
        }
        if tag == CSUM && !self.checksums {
            return Ok(Kind::Ignored);
        }
        if tag == crate::pixelsum::PSUM {
            self.pixel_len = len.unwrap_or(0);
            if self.has(&DATA) {
                return Err(Error::Corrupt("pSUM after DATA"));
            }
            if len.is_some_and(|n| n > crate::pixelsum::MAX_PAYLOAD) {
                return Err(Error::Corrupt("pSUM length"));
            }
        }
        if tag == ANIM && self.has(&DATA) {
            return Err(Error::Corrupt("aNIM after DATA"));
        }
        if tag == SDEF && self.has(&DATA) {
            return Err(Error::Corrupt("sDEF after DATA"));
        }
        if tag == UIDX && !self.has(&DATA) {
            return Err(Error::Corrupt("uIDX before DATA"));
        }
        self.once(&tag)?;
        Ok(match tag {
            DATA => {
                // aNIM precedes DATA: its kind is checked against the
                // declared features before any of the image is read.
                self.check_mixed_pages()?;
                if let Some(count) = self.pixel_count {
                    let images = self.image_count.clone().unwrap_or(Ok(1))?;
                    if count != images {
                        return Err(Error::Corrupt("pSUM image count"));
                    }
                }
                Kind::Data
            }
            CSUM => Kind::Checksum,
            UIDX => Kind::UnitIndex,
            PRVW => Kind::Preview,
            ANIM => Kind::Anim,
            FRAM => Kind::Frame,
            DWHI if len != Some(4) => return Err(Error::Corrupt("bad dWHI")),
            MDCV if len != Some(24) => return Err(Error::Corrupt("bad mDCv")),
            CLLI if !matches!(len, Some(4 | 8)) => return Err(Error::Corrupt("bad cLLI")),
            t if is_critical(&t) => return Err(Error::Unsupported("unknown critical chunk")),
            _ => Kind::Meta,
        })
    }

    /// The rules that read a payload ([`Kind::has_body_rules`]): an `mCMP`
    /// record's identity, uniqueness and syntax (validated, not expanded),
    /// and the `aNIM` kind.
    pub(crate) fn body(&mut self, tag: [u8; 4], payload: &[u8]) -> Result<(), Error> {
        self.identity(tag, payload)?;
        record(tag, payload)
    }

    /// The payload rules a chunk's first bytes show (`prefix`: at least its
    /// first 4 payload bytes, or the whole payload when shorter): the `aNIM`
    /// kind, `dWHI` luminance, and an `mCMP` record's identity and uniqueness. A reader that
    /// does not fetch the whole payload checks the rest with [`record`].
    pub(crate) fn identity(&mut self, tag: [u8; 4], prefix: &[u8]) -> Result<(), Error> {
        if tag == DWHI {
            diffuse_white(prefix)?;
            return Ok(());
        }
        if tag == crate::pixelsum::PSUM {
            self.pixel_count = crate::pixelsum::prefix(prefix, self.pixel_len)?.map(|(n, _, _)| n);
            return Ok(());
        }
        if tag == ANIM {
            self.image_count = Some(crate::pixelsum::image_count(Some(prefix)));
            self.anim = Some(prefix.first().copied());
            return Ok(());
        }
        if tag != MCMP {
            return Ok(());
        }
        let inner: [u8; 4] = prefix
            .get(..4)
            .ok_or(Error::Corrupt("short compressed metadata"))?
            .try_into()
            .unwrap();
        if !matches!(inner, ICCP | EXIF | XMP) {
            return Err(Error::Corrupt("invalid compressed metadata identity"));
        }
        if self.has(&inner) {
            return Err(Error::Corrupt("repeated chunk"));
        }
        self.once(&inner)
    }

    /// Whether `DATA` was seen.
    pub(crate) fn data_seen(&self) -> bool {
        self.has(&DATA)
    }

    /// `aNIM` kind 3 (mixed pages) is gated syntax: the header declares the
    /// feature `mixed_pages` (SPEC 0.1, 1.3). Declaring it without using
    /// it is allowed (over-declaring).
    fn check_mixed_pages(&self) -> Result<(), Error> {
        if self.has(&SDEF) && !matches!(self.anim.flatten(), Some(3 | 4)) {
            return Err(Error::Corrupt("sDEF needs mixed pages or sequence items"));
        }
        if self.anim.flatten() == Some(4) && !self.sequence_items {
            return Err(Error::Corrupt(
                "aNIM kind 4 needs the feature sequence_items",
            ));
        }
        if self.anim.flatten() == Some(3) && !self.mixed_pages {
            return Err(Error::Corrupt("aNIM kind 3 needs the feature mixed_pages"));
        }
        Ok(())
    }

    /// The end of a prefix read (`header`, `previews`), which stops at
    /// `DATA` or earlier: an `aNIM` it saw is checked against the declared
    /// features once `DATA` is reached, which `head` did; a prefix that ends
    /// before `DATA` may end before `aNIM` too, so nothing else holds.
    pub(crate) fn finish_prefix(&self) -> Result<(), Error> {
        if self.anim.is_some() && !self.data_seen() {
            return self.check_mixed_pages();
        }
        Ok(())
    }

    /// The end of a whole file: it has `DATA`.
    pub(crate) fn finish(&self) -> Result<(), Error> {
        if !self.data_seen() {
            self.check_mixed_pages()?;
            return Err(Error::Corrupt("no DATA chunk"));
        }
        Ok(())
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    /// A chunk for [`run`]: tag, payload (`None`: open).
    type Chunk<'a> = (&'a [u8; 4], Option<Vec<u8>>);

    fn mcmp(inner: &[u8; 4]) -> Vec<u8> {
        let mut p = inner.to_vec();
        p.extend(crate::metadata_pack::pack(b"metadata body").unwrap());
        p
    }

    /// Feeds `chunks` to the rules as a whole-file reader does (head, then
    /// body for the chunks with body rules), then `finish`.
    fn run(mixed: bool, checksums: bool, chunks: &[Chunk<'_>]) -> Result<Vec<Kind>, Error> {
        let mut rules = Rules::new(mixed, checksums);
        let mut kinds = Vec::new();
        for (tag, payload) in chunks {
            let len = payload.as_ref().map(|p| p.len() as u64);
            let kind = rules.head(**tag, len)?;
            if kind.has_body_rules(tag) {
                rules.body(**tag, payload.as_deref().unwrap_or(&[]))?;
            }
            kinds.push(kind);
        }
        rules.finish()?;
        Ok(kinds)
    }

    /// The same chunks as a partial reader sees them: [`Rules::identity`] on
    /// the first [`prefix_len`] bytes only, then [`record`] on the payload.
    fn run_prefixed(
        mixed: bool,
        checksums: bool,
        chunks: &[Chunk<'_>],
    ) -> Result<Vec<Kind>, Error> {
        let mut rules = Rules::new(mixed, checksums);
        let mut kinds = Vec::new();
        for (tag, payload) in chunks {
            let len = payload.as_ref().map(|p| p.len() as u64);
            let kind = rules.head(**tag, len)?;
            if kind.has_body_rules(tag) {
                let p = payload.as_deref().unwrap_or(&[]);
                rules.identity(**tag, &p[..prefix_len(tag, len) as usize])?;
                record(**tag, p)?;
            }
            kinds.push(kind);
        }
        rules.finish()?;
        Ok(kinds)
    }

    fn c<'a>(tag: &'a [u8; 4], payload: &[u8]) -> Chunk<'a> {
        (tag, Some(payload.to_vec()))
    }

    #[test]
    fn rules_over_chunk_sequences() {
        let mdcv = [0u8; 24];
        let clli = [0u8; 4];
        let ok = |chunks: &[Chunk<'_>]| run(false, true, chunks).map(|_| ());
        // A plain file, and every once-only chunk once.
        assert_eq!(ok(&[c(b"DATA", b"x")]), Ok(()));
        assert_eq!(
            ok(&[
                c(b"iCCP", b"i"),
                c(b"eXIf", b"e"),
                c(b"xMPp", b"x"),
                c(b"mDCv", &mdcv),
                c(b"cLLI", &clli),
                c(b"gMAP", b"g"),
                c(b"cSUM", b"c"),
                c(b"DATA", b"d"),
                c(b"pRVW", b"p"),
                c(b"pRVW", b"p"),
                c(b"abcd", b"a"),
                c(b"abcd", b"a"),
            ]),
            Ok(())
        );
        // Repeats, including through mCMP in either order.
        for tag in [ICCP, EXIF, XMP, DATA, GMAP, CSUM] {
            assert_eq!(
                ok(&[c(&tag, b"1"), c(&tag, b"2"), c(b"DATA", b"d")]),
                Err(Error::Corrupt("repeated chunk")),
                "{tag:?}"
            );
        }
        for inner in [ICCP, EXIF, XMP] {
            let m = mcmp(&inner);
            assert_eq!(
                ok(&[c(&inner, b"raw"), c(b"mCMP", &m), c(b"DATA", b"d")]),
                Err(Error::Corrupt("repeated chunk"))
            );
            assert_eq!(
                ok(&[c(b"mCMP", &m), c(&inner, b"raw"), c(b"DATA", b"d")]),
                Err(Error::Corrupt("repeated chunk"))
            );
            assert_eq!(
                ok(&[c(b"mCMP", &m), c(b"mCMP", &m), c(b"DATA", b"d")]),
                Err(Error::Corrupt("repeated chunk"))
            );
        }
        // No DATA; open chunks; tag syntax; unknown critical chunks.
        assert_eq!(
            ok(&[c(b"iCCP", b"i")]),
            Err(Error::Corrupt("no DATA chunk"))
        );
        assert_eq!(ok(&[(b"DATA", None)]), Ok(()));
        assert_eq!(
            ok(&[(b"iCCP", None), c(b"DATA", b"d")]),
            Err(Error::Corrupt("open ancillary chunk"))
        );
        assert_eq!(
            ok(&[(b"QQQQ", None), c(b"DATA", b"d")]),
            Err(Error::Unsupported("unknown critical chunk"))
        );
        assert_eq!(
            ok(&[c(b"QQQQ", b"q"), c(b"DATA", b"d")]),
            Err(Error::Unsupported("unknown critical chunk"))
        );
        assert_eq!(
            ok(&[c(b"ab1d", b"q"), c(b"DATA", b"d")]),
            Err(Error::Corrupt("invalid chunk tag"))
        );
        // Fixed sizes.
        assert_eq!(
            ok(&[c(b"mDCv", &mdcv[..23]), c(b"DATA", b"d")]),
            Err(Error::Corrupt("bad mDCv"))
        );
        assert_eq!(
            ok(&[c(b"cLLI", &clli[..3]), c(b"DATA", b"d")]),
            Err(Error::Corrupt("bad cLLI"))
        );
        // mCMP identity and syntax.
        assert_eq!(
            ok(&[c(b"mCMP", b"iCC"), c(b"DATA", b"d")]),
            Err(Error::Corrupt("short compressed metadata"))
        );
        let mut bad = mcmp(&ICCP);
        bad[..4].copy_from_slice(b"gMAP");
        assert_eq!(
            ok(&[c(b"mCMP", &bad), c(b"DATA", b"d")]),
            Err(Error::Corrupt("invalid compressed metadata identity"))
        );
        let mut huge = Rules::new(false, true);
        assert_eq!(
            huge.head(MCMP, Some(MAX_PACKED_METADATA_RECORD as u64 + 1)),
            Err(Error::TooLarge)
        );
        // aNIM goes before DATA; plane chunks and cSUM with checksums off
        // repeat freely.
        assert_eq!(
            ok(&[c(b"DATA", b"d"), c(b"aNIM", &[1])]),
            Err(Error::Corrupt("aNIM after DATA"))
        );
        assert_eq!(
            run(
                false,
                false,
                &[c(b"cSUM", b"1"), c(b"cSUM", b"2"), c(b"DATA", b"d")]
            ),
            Ok(vec![Kind::Ignored, Kind::Ignored, Kind::Data])
        );
        assert_eq!(
            ok(&[
                c(b"pDAT", b"1"),
                c(b"pDAT", b"2"),
                c(b"pIDX", b"3"),
                c(b"DATA", b"d")
            ]),
            Ok(())
        );
    }

    #[test]
    fn mixed_pages_feature_gates_anim_kind_3() {
        for mixed in [false, true] {
            for kind in [None, Some(0u8), Some(1), Some(2), Some(3), Some(9)] {
                let mut chunks = Vec::new();
                if let Some(k) = kind {
                    chunks.push(c(b"aNIM", &[k, 0, 0, 0]));
                }
                chunks.push(c(b"DATA", b"d"));
                // Kind 3 needs the feature; the feature without kind 3 is
                // over-declared, which is allowed. Unknown kinds are the
                // frame decoder's to refuse.
                let want = if kind == Some(3) && !mixed {
                    Err(Error::Corrupt("aNIM kind 3 needs the feature mixed_pages"))
                } else {
                    Ok(())
                };
                assert_eq!(
                    run(mixed, true, &chunks).map(|_| ()),
                    want,
                    "{mixed} {kind:?}"
                );
                // A prefix read that stops before DATA checks it at its end.
                let mut prefix = Rules::new(mixed, true);
                if let Some(k) = kind {
                    prefix.head(ANIM, Some(4)).unwrap();
                    prefix.identity(ANIM, &[k]).unwrap();
                }
                let want_prefix = if kind.is_some() { want.clone() } else { Ok(()) };
                assert_eq!(
                    prefix.finish_prefix(),
                    want_prefix,
                    "{mixed} {kind:?} prefix"
                );
            }
        }
        // An empty aNIM payload carries no kind.
        assert_eq!(
            run(true, true, &[c(b"aNIM", &[]), c(b"DATA", b"d")]).map(|_| ()),
            Ok(())
        );
    }

    /// A partial reader (identity on the first bytes, then the record
    /// syntax) and a whole-file reader reach the same outcome on random
    /// chunk sequences.
    #[test]
    fn partial_and_whole_readers_agree() {
        let alphabet: [Chunk<'static>; 14] = [
            c(b"DATA", b"d"),
            (b"DATA", None),
            c(b"iCCP", b"i"),
            c(b"eXIf", b"e"),
            c(b"mDCv", &[0; 24]),
            c(b"mDCv", &[0; 3]),
            c(b"aNIM", &[1, 0, 0, 0]),
            c(b"aNIM", &[3]),
            c(b"mCMP", &mcmp(&ICCP)),
            c(b"mCMP", &mcmp(&XMP)),
            c(b"mCMP", b"xM"),
            c(b"mCMP", &[b'x', b'M', b'P', b'p', 0xff, 0xff]),
            c(b"cSUM", b"c"),
            c(b"abcd", b"a"),
        ];
        let mut s = 0x2545_F491_4F6C_DD1Du64;
        for _ in 0..4000 {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            let n = (s % 6) as usize;
            let seq: Vec<Chunk<'static>> = (0..n)
                .map(|i| alphabet[((s >> (8 * i + 8)) % alphabet.len() as u64) as usize].clone())
                .collect();
            for (mixed, checksums) in [(false, true), (true, true), (false, false)] {
                assert_eq!(
                    run(mixed, checksums, &seq),
                    run_prefixed(mixed, checksums, &seq),
                    "{seq:?}"
                );
            }
        }
    }
}
