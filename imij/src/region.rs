//! Region decoding of the primary image.
//!
//! Version 4 uses spatial tile extents and indexed immutable models. A fetched
//! tile supplies independent group work; only selected groups and their numeric
//! dependencies reconstruct pixels. Object/dependency checks cover fetched bytes,
//! while whole-file traversal has a separate validation scope. The ordinary
//! opener retains its initial 64 KiB prefetch. The following unit-hint and
//! record-head details describe the retained V2/V3 implementation.
//!
//! A rectangle of the primary image (the `DATA` image: frame 0 or page 1)
//! decodes from the units (stripe, column group) it intersects; nothing else
//! is decoded. [`RegionReader`](crate::region::RegionReader) reads through
//! [`ReadAt`](crate::planes::ReadAt), so a file on disk or behind HTTP range
//! requests is read only where needed: the header, the `DATA` head (models,
//! tables, the unit table hint), and the records of those units. A lossy
//! unit also needs the DC record of the DC stripe it lies in.
//!
//! Files with two or more units carry the unit table hint in their `DATA`
//! head (SPEC 2.7): the bytes of every record. The reader finds units
//! through it, and when it opens the file it checks every span against the
//! record heads there (as every full decoder checks the hint against the
//! records it walks), so a region and a full decode read the same records
//! or fail. The file header may also carry a DATA locator (SPEC 1.0, header
//! record 4) listing the length of every chunk before `DATA`; the reader
//! checks it against those chunks' headers, all requested in one batch, so
//! opening takes two batches of reads (the file's first 64 KiB, then the
//! chunk headers beyond it with the `DATA` head), then the record heads past
//! what those two returned (none for files that fit the first 64 KiB): one
//! head of up to 20 bytes per main record, in batches of 65,536, charged to
//! the byte limit (`Limits::max_bytes`); a region takes one more batch.
//! Checking the table is global, so opening's work grows with the unit count
//! whatever the region. Files without a locator are walked chunk by chunk.
//! A head whose unit table reservation (head record 4) says check tail sends
//! the reader to the end of the file for the trailing unit index (`uIDX`,
//! the file's last chunk): one batch for the last 64 KiB, one more for a
//! longer index, then the same record-head check as for the hint. Files with
//! neither (one unit, a writer that omits the table, an empty reservation, a
//! stripped index) are walked record by record when opened, one read per
//! record head, so bytes after the last record are refused as a full decode
//! refuses them.
//!
//! The reader decodes the primary image only and stops at the end of
//! `DATA` (SPEC 1.1): the chunks after it (further frames and pages, and
//! any others) are not read, so pages and animations open in the same two
//! batches, and a file whose later chunks a full decode refuses (a second
//! `DATA`, an unknown critical chunk) still yields its primary image here.
//! The chunks it does read follow the sequential rules.

use alloc::vec::Vec;

#[cfg(feature = "v3")]
use crate::parse::chunks::{self, Kind, Rules};
#[cfg(feature = "v3")]
use crate::parse::header::{Features, Locator};
#[cfg(feature = "v3")]
use crate::parse::payload::{self, Head};
#[cfg(feature = "v3")]
use crate::parse::records::{self, UnitMap};
use crate::planes::{ReadAt, Rect};
use crate::{ByteReader, Error, Header, ImageInfo, Limits};

/// Bytes read at once from the start of the file and of `DATA`.
const WINDOW: u64 = 64 * 1024;
/// Most units a payload may have for region reading (16 Mi; more is
/// `TooLarge`).
#[cfg(feature = "v3")]
const MAX_UNITS: usize = 1 << 24;
/// Largest `DATA` prefix (its head) a locator may name.
#[cfg(feature = "v3")]
const MAX_PREFIX: u64 = 1 << 30;

/// Reads a [`RegionReader`] made through its source, since it was opened
/// or its statistics were last reset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReadStats {
    /// Ranges requested.
    pub reads: u64,
    /// Bytes those requests returned.
    pub bytes: u64,
    /// [`ReadAt::read_many_at`] calls. Each depends on the previous one, so a
    /// source that serves a call's ranges concurrently needs one round trip
    /// per call.
    pub batches: u64,
}

/// File ranges a reader used, merged where consecutive ones touch.
#[derive(Default)]
#[cfg(feature = "v3")]
struct Used(Vec<(u64, u64)>);

#[cfg(feature = "v3")]
impl Used {
    fn push(&mut self, a: u64, b: u64) -> Result<(), Error> {
        if a >= b {
            return Ok(());
        }
        if let Some(r) = self.0.last_mut()
            && a <= r.1
            && b >= r.0
        {
            (r.0, r.1) = (r.0.min(a), r.1.max(b));
            return Ok(());
        }
        self.0.try_reserve(1).map_err(|_| Error::TooLarge)?;
        self.0.push((a, b));
        Ok(())
    }
}

/// A file checksum the reader verifies (SPEC 1.4): the pieces under every
/// byte it uses are checked before those bytes decode, in the batch that
/// fetches them. The table and what opening read are checked with the
/// first region, so verification adds no batch.
#[cfg(feature = "v3")]
struct Check {
    pieces: crate::checksum::Fetched,
    /// File ranges opening used, checked with the first region that
    /// succeeds.
    opened: Vec<(u64, u64)>,
}

/// Observation of one region decode: the caller's stop check before each
/// unit, and one progress child per unit of the region (stripes `s0..s1`,
/// groups `g0..g1`, in that order) for a live pulse.
#[cfg(feature = "v3")]
pub(crate) struct RegionObs<'a> {
    pulse: Option<&'a dyn how_far::Pulse>,
    units: Option<crate::progress::UnitProgress<'a>>,
    s0: usize,
    g0: usize,
    gn: usize,
}

#[cfg(feature = "v3")]
impl<'a> RegionObs<'a> {
    #[cfg(feature = "v3")]
    fn new(
        pulse: Option<&'a dyn how_far::Pulse>,
        geo: &Geo,
        (s0, s1): (usize, usize),
        (g0, g1): (usize, usize),
        threads: usize,
    ) -> Self {
        let units = pulse.and_then(|pulse| {
            let shapes = (s0..s1).flat_map(move |s| (g0..g1).map(move |g| (s, g)));
            let shapes: Vec<_> = shapes
                .map(|(s, g)| {
                    let (x, cols) = geo.group(g);
                    let y = s * geo.rows;
                    crate::progress::UnitShape {
                        stripe: s,
                        group: g,
                        groups: geo.groups,
                        y,
                        rows: geo.rows.min(geo.height - y),
                        x,
                        cols,
                    }
                })
                .collect();
            crate::progress::UnitProgress::plan(pulse, shapes.into_iter(), threads.max(1))
        });
        Self {
            pulse,
            units,
            s0,
            g0,
            gn: g1 - g0,
        }
    }

    /// No observer: decodes outside a region reader.
    #[allow(dead_code)]
    pub(crate) fn none() -> Self {
        Self {
            pulse: None,
            units: None,
            s0: 0,
            g0: 0,
            gn: 0,
        }
    }

    pub(crate) fn check(&self) -> Result<(), Error> {
        match self.pulse {
            Some(p) => p.check().map_err(|_| Error::Cancelled),
            None => Ok(()),
        }
    }

    /// Unit (stripe `s`, group `g`) starts: checks stop first.
    pub(crate) fn begin(&self, s: usize, g: usize) -> Result<(), Error> {
        self.check()?;
        if let Some(u) = &self.units {
            u.begin((s - self.s0) * self.gn + g - self.g0)?;
        }
        Ok(())
    }

    pub(crate) fn end(&self, s: usize, g: usize, result: &Result<(), Error>) {
        if let Some(u) = &self.units {
            u.end((s - self.s0) * self.gn + g - self.g0, result);
        }
    }
}

/// Unit geometry of the `DATA` payload.
#[derive(Clone, Debug)]
#[cfg(feature = "v3")]
struct Geo {
    width: usize,
    height: usize,
    /// Pixel rows per stripe.
    rows: usize,
    stripes: usize,
    groups: usize,
    /// Column group width in pixels (0 = none).
    gcols: usize,
    /// Lossy: stripes per DC stripe (1 for lossless).
    k: usize,
    lossy: bool,
    alpha: bool,
    /// Where each unit's records sit.
    map: UnitMap,
}

#[cfg(feature = "v3")]
impl Geo {
    fn units(&self) -> Result<usize, Error> {
        self.stripes
            .checked_mul(self.groups)
            .filter(|&n| n <= MAX_UNITS)
            .ok_or(Error::TooLarge)
    }
    /// First column and width of column group `g`.
    fn group(&self, g: usize) -> (usize, usize) {
        if self.gcols == 0 {
            (0, self.width)
        } else {
            let x0 = g * self.gcols;
            (x0, self.gcols.min(self.width - x0))
        }
    }
    fn group_of(&self, x: usize) -> usize {
        x.checked_div(self.gcols).unwrap_or(0)
    }
}

/// Parses the `DATA` head at the start of `bytes`: the head, its length
/// and the unit geometry. Payloads a region cannot decode are refused here,
/// at open: keep blocks (inter frames only). Exact blocks decode: the exact
/// head locates every nested stripe a unit needs (docs/LOSSY.md "Exact
/// blocks"), in whichever unit's record it sits.
#[cfg(feature = "v3")]
struct ModelBudget {
    used: core::cell::Cell<u128>,
    max: u128,
}
#[cfg(feature = "v3")]
impl crate::parse::limits::Admission for ModelBudget {
    fn wire_extra_bytes(&self, bytes: u128) -> Result<(), Error> {
        let next = self
            .used
            .get()
            .checked_add(bytes)
            .filter(|&n| n <= self.max)
            .ok_or(Error::TooLarge)?;
        self.used.set(next);
        Ok(())
    }
}

#[cfg(feature = "v3")]
fn parse_head(
    header: &Header,
    d: Features,
    bytes: &[u8],
    max_models: u64,
) -> Result<(Head, usize, Geo), Error> {
    let mut r = ByteReader::new(bytes);
    let budget = ModelBudget {
        used: core::cell::Cell::new(0),
        max: u128::from(max_models),
    };
    let head = payload::read_head(header, d, &mut r, &budget)?;
    let (width, height) = (header.info.width as usize, header.info.height as usize);
    let geo = match &head {
        Head::Lossy(p) => {
            if p.needs_reference() {
                return Err(Error::Corrupt(
                    "keep-reference blocks outside an inter frame",
                ));
            }
            let g = p.region_geometry();
            Geo {
                width,
                height,
                rows: g.rows,
                stripes: g.stripes,
                groups: g.groups,
                gcols: g.gcols,
                k: g.k,
                lossy: true,
                alpha: g.alpha,
                map: p.unit_map(),
            }
        }
        #[cfg(feature = "native-jpeg")]
        Head::Native(p) => Geo {
            width,
            height,
            rows: p.header.pixel_band_rows(p.rows),
            stripes: p.bands(),
            groups: 1,
            gcols: 0,
            k: 1,
            lossy: false,
            alpha: false,
            map: UnitMap::native(),
        },
        Head::Lossless(p) => Geo {
            width,
            height,
            rows: p.hs,
            stripes: p.stripe_count(),
            groups: p.ngroups(),
            gcols: p.gcols,
            k: 1,
            lossy: false,
            alpha: false,
            map: UnitMap::lossless(p.ngroups()),
        },
    };
    Ok((head, r.pos, geo))
}

/// The start of a `DATA` payload: the head, with the unit table hint (SPEC
/// 2.7) taken out of it when it has one.
#[cfg(feature = "v3")]
struct Prefix {
    head: Head,
    head_len: usize,
    geo: Geo,
    /// The span of every main record, from the hint.
    lens: Option<Vec<u64>>,
    /// The head's unit table reservation (head record 12), if any.
    reserve: Option<records::Reserve>,
}

/// Parses the head at the start of `bytes` (a prefix of a `DATA` payload of
/// `room` bytes at most).
#[cfg(feature = "v3")]
fn parse_prefix(
    header: &Header,
    d: Features,
    bytes: &[u8],
    room: u64,
    max_models: u64,
) -> Result<Prefix, Error> {
    let (mut head, head_len, geo) = parse_head(header, d, bytes, max_models)?;
    let units = geo.units()?;
    let room = room.saturating_sub(head_len as u64);
    // As the full parsers check before allocating (SPEC 2.7).
    records::check_count(units, room)?;
    let lens = head.take_hint();
    // The head parser matched the entries to the records; they must fit.
    if let Some(l) = &lens {
        records::check_hint(l, l.len(), room)?;
    }
    Ok(Prefix {
        reserve: head.reserve(),
        head,
        head_len,
        geo,
        lens,
    })
}

/// Units, DC records, exact stripes and file ranges one region decode uses.
#[cfg(feature = "v3")]
struct RegionPlan {
    s: (usize, usize),
    g: (usize, usize),
    d: (usize, usize),
    dc_only: bool,
    dc_ranges: Vec<(usize, u64, u64)>,
    stripe_ranges: Vec<(usize, u64, u64)>,
    file_runs: Vec<(u64, u64)>,
    pieces: Vec<u64>,
    streamed: Vec<u64>,
}

mod v4;

/// Reads native pixel regions through the grammar selected by the file version.
/// V2/V3 retain their global record-head index check. V4 uses spatial tile
/// extents and verifies fetched objects and their indexed model dependencies.
pub struct RegionReader<R: ReadAt> {
    backend: Backend<R>,
}
// Fixed parser/index state stays inline; neither variant contains pixel data.
#[allow(clippy::large_enum_variant)]
enum Backend<R: ReadAt> {
    #[cfg(feature = "v3")]
    Legacy(LegacyRegionReader<R>),
    V4(v4::Reader<R>),
}
impl<R: ReadAt> RegionReader<R> {
    /// Open a primary image with [`Limits::DEFAULT`]. Limits apply to each
    /// requested crop, retained codec state, compressed reads and working buffers.
    /// The source must remain unchanged for the reader's lifetime.
    ///
    /// V2/V3 retain their global unit-hint/record-head reconciliation. V4 checks
    /// directory structure at opening and fetched tile/model bindings when used.
    /// A checksum-free V4 unified sequence opens its primary without visiting
    /// unrelated later frame headers, retaining its authoritative directory.
    /// This is a partial read; full sequence validation belongs to the complete
    /// decoder. Outer checksum and pixel-checksum routes retain their checks.
    /// Without a stored V4 directory, opening discovers spans by walking framed
    /// record headers. Shared model definitions can add metadata reads outside
    /// a selected tile. The initial 64 KiB prefetch is included in
    /// [`Self::read_stats`].
    ///
    /// Supported outer checksums verify covering pieces before reconstruction;
    /// rounded pieces can include other encoded bytes. Opening reads metadata
    /// identities and required model definitions, without expanding arbitrary
    /// metadata payloads. A full bitmap checksum remains Pending for partial crops.
    pub fn open(src: R) -> Result<Self, Error> {
        Self::open_with_limits(src, &Limits::DEFAULT)
    }
    /// [`Self::open`] with `limits` applied to each region: its output, the
    /// buffer a lossy region decodes into, the working state of the units
    /// decoding at once and the bytes it fetches (its records, and with a
    /// file checksum the table and the pieces its reads round out to), each
    /// charged against `max_bytes` before any record is fetched: a region
    /// whose records and table exceed it is [`Error::TooLarge`], and pieces
    /// that do not fit with them are hashed through a buffer the budget
    /// leaves room for, so no request of a region exceeds `max_bytes`. This is a
    /// per-region budget: the whole-image decoders charge only their output,
    /// so a region of an image they decode under the same limits can still
    /// be [`Error::TooLarge`] (its units' state or records exceed it).
    /// [`Limits::verify_checksum`] off reads the file as a decoder without
    /// checksums does.
    pub fn open_with_limits(src: R, limits: &Limits) -> Result<Self, Error> {
        Self::open_profiled(src, limits, None)
    }

    /// Open an indexed V4 primary image with a coding profile. Each decoded
    /// region validates its selected groups and models under that profile.
    /// Opening does not certify unread regions.
    pub fn open_v4_profile(
        src: R,
        limits: &Limits,
        profile: crate::v4::CodingProfile,
    ) -> Result<Self, Error> {
        Self::open_profiled(src, limits, Some(profile))
    }

    pub(crate) fn open_profiled(
        mut src: R,
        limits: &Limits,
        profile: Option<crate::v4::CodingProfile>,
    ) -> Result<Self, Error> {
        let mut stats = ReadStats::default();
        let file_len = src.len()?;
        let mut win = Window::default();
        win.ensure(&mut src, &mut stats, file_len, 0, file_len.min(WINDOW))?;
        if win.buf.get(..8) != Some(&crate::parse::header::SIGNATURE[..]) {
            return Err(Error::NotImij);
        }
        let mut r = ByteReader::new(&win.buf[8..]);
        let n = r.varint().map_err(|_| Error::Corrupt("truncated header"))?;
        let header_end = 8u64
            .checked_add(r.pos as u64)
            .and_then(|v| v.checked_add(n))
            .ok_or(Error::Corrupt("bad header length"))?;
        let version = win.buf.get(8 + r.pos).copied();
        if version == Some(4) {
            if header_end > WINDOW {
                return Err(Error::TooLarge);
            }
            return v4::Reader::open(src, win, stats, *limits, profile).map(|reader| Self {
                backend: Backend::V4(reader),
            });
        }
        if profile.is_some() {
            return Err(Error::Unsupported("V4 coding profile requires version 4"));
        }

        #[cfg(feature = "v3")]
        return LegacyRegionReader::open_legacy(src, limits, file_len, win, stats, header_end);
        #[cfg(not(feature = "v3"))]
        {
            crate::parse::header::check_header_version(
                version.ok_or(Error::Corrupt("truncated header"))?,
            )?;
            // Version 4 returned above and all other versions are unsupported.
            Err(Error::Unsupported("unknown header version"))
        }
    }
    /// The file's header (the primary image's coding, size, color and
    /// profiles), as parsed at open. With a file checksum it is verified
    /// with the first region, not before.
    pub fn header(&self) -> &Header {
        match &self.backend {
            #[cfg(feature = "v3")]
            Backend::Legacy(r) => r.header(),
            Backend::V4(r) => r.file.header(),
        }
    }
    /// The primary image's dimensions, channels and sample type
    /// (`header().info`): what [`Self::decode_region`] writes.
    pub fn info(&self) -> &ImageInfo {
        match &self.backend {
            #[cfg(feature = "v3")]
            Backend::Legacy(r) => r.info(),
            Backend::V4(r) => &r.file.header().info,
        }
    }
    /// First byte after primary DATA. A sequence index can use it to locate
    /// the first following `fRAM` control without reading earlier pixels.
    pub fn primary_end(&self) -> u64 {
        match &self.backend {
            #[cfg(feature = "v3")]
            Backend::Legacy(r) => r.primary_end(),
            Backend::V4(r) => r.file.primary_end(),
        }
    }
    /// True when the pre-DATA `aNIM` declares unified sequence items (kind 4).
    /// Legacy animations and pages use different reconstruction rules.
    pub fn is_unified_sequence(&self) -> bool {
        match &self.backend {
            #[cfg(feature = "v3")]
            Backend::Legacy(r) => r.is_unified_sequence(),
            Backend::V4(r) => r.file.is_unified_sequence(),
        }
    }
    /// Whether this file declares a supported outer file checksum. Partial
    /// sequence consumers must verify the pieces under selected later items
    /// before relying on such a checksum.
    pub fn has_file_checksum(&self) -> bool {
        match &self.backend {
            #[cfg(feature = "v3")]
            Backend::Legacy(r) => r.has_file_checksum(),
            Backend::V4(r) => r.file.checksum_declared(),
        }
    }
    /// Absolute range of the complete cSUM chunk, if discovered during
    /// opening with file-checksum verification enabled.
    pub fn file_checksum_chunk(&self) -> Option<core::ops::Range<u64>> {
        match &self.backend {
            #[cfg(feature = "v3")]
            Backend::Legacy(r) => r.check.as_ref().map(|check| {
                let (start, end) = check.pieces.chunk();
                start..end
            }),
            Backend::V4(r) => r.file.file_checksum_chunk(),
        }
    }

    /// Fetch a source range while checking every declared cSUM piece it uses.
    /// With a cSUM declaration this fails closed if verification is unavailable.
    /// The source must remain immutable. Discard the output after any error.
    pub fn read_verified_at(&mut self, at: u64, bytes: &mut [u8]) -> Result<(), Error> {
        match &mut self.backend {
            #[cfg(feature = "v3")]
            Backend::Legacy(r) => r.read_verified_at(at, bytes),
            Backend::V4(r) => r.read_verified_at(at, bytes),
        }
    }

    /// Resolve V4 sequence defaults on demand for an item's own metadata.
    /// Beyond the ordinary opening window, the reader skips the sDEF body
    /// until this call. The caller's bound and the outer cSUM policy apply.
    pub fn resolve_sequence_metadata(
        &mut self,
        info: &ImageInfo,
        mut metadata: crate::Metadata,
        max_default_bytes: usize,
    ) -> Result<crate::Metadata, Error> {
        #[cfg(feature = "v3")]
        let Backend::V4(reader) = &mut self.backend else {
            return Ok(metadata);
        };
        #[cfg(not(feature = "v3"))]
        let Backend::V4(reader) = &mut self.backend;
        let Some(range) = reader.file.sequence_defaults_range() else {
            return Ok(metadata);
        };
        let size = usize::try_from(range.end - range.start).map_err(|_| Error::TooLarge)?;
        if size > max_default_bytes || size as u64 > reader.limits.max_bytes {
            return Err(Error::TooLarge);
        }
        let mut bytes = buffer(size)?;
        reader.read_verified_at(range.start, &mut bytes)?;
        crate::frames::defaults::resolve(
            &bytes,
            &mut metadata,
            info,
            &reader.file.header().info,
            max_default_bytes,
        )?;
        Ok(metadata)
    }

    /// Compare one complete reconstructed raster with its declared pSUM.
    /// Returns whether this item had a supported checksum. The caller supplies
    /// the item's actual decoded image descriptor, with no crop or conversion.
    pub fn verify_indexed_pixels(
        &mut self,
        ordinal: usize,
        info: &ImageInfo,
        pixels: &[u8],
        stride: usize,
    ) -> Result<bool, Error> {
        let Some(expected) = self.pixel_checksum_for(ordinal)? else {
            return Ok(false);
        };
        if crate::pixelsum::digest_with(expected.algorithm(), info, ordinal as u64, pixels, stride)?
            != expected
        {
            return Err(crate::pixelsum::MISMATCH);
        }
        Ok(true)
    }

    /// Stored pSUM digest for a sequence ordinal. Later ordinals may need
    /// one positional read. Use this value with the reconstructed raster.
    /// `None` covers both absence and unsupported versions or algorithms.
    /// Consult [`Self::pixel_checksum_status`] to distinguish the cases.
    pub fn pixel_checksum_for(&mut self, ordinal: usize) -> Result<Option<crate::Digest>, Error> {
        let (primary, table, max_bytes, verify) = match &self.backend {
            #[cfg(feature = "v3")]
            Backend::Legacy(r) => (
                r.pixel_expected,
                r.pixel_table,
                r.limits.max_bytes,
                r.limits.verify_checksum,
            ),
            Backend::V4(r) => {
                let (primary, table) = r.file.pixel_checksum();
                (primary, table, r.limits.max_bytes, r.limits.verify_checksum)
            }
        };
        if ordinal == 0 {
            return Ok(primary);
        }
        let Some(table) = table else {
            return Ok(None);
        };
        table.expected(ordinal, max_bytes, |at, bytes| {
            if verify {
                return self.read_verified_at(at, bytes);
            }
            match &mut self.backend {
                #[cfg(feature = "v3")]
                Backend::Legacy(r) => {
                    LegacyReads::new(&mut r.src, &mut r.stats).read_exact_at(at, bytes)
                }
                Backend::V4(r) => r.file.source_mut().read_exact_at(at, bytes),
            }
        })
    }
    /// Whether a V2/V3 unit hint or V4 front/tail directory is available.
    /// V2/V3 record heads are globally reconciled at opening. V4 validates
    /// directory structure at opening and binds fetched records when used.
    pub fn has_unit_table(&self) -> bool {
        match &self.backend {
            #[cfg(feature = "v3")]
            Backend::Legacy(r) => r.has_unit_table(),
            Backend::V4(r) => r.file.has_index(),
        }
    }
    /// True when the file's DATA locator matched its chunks and was used.
    pub fn used_locator(&self) -> bool {
        match &self.backend {
            #[cfg(feature = "v3")]
            Backend::Legacy(r) => r.used_locator(),
            Backend::V4(_) => false,
        }
    }
    /// Reads made through the source so far.
    pub fn read_stats(&self) -> ReadStats {
        match &self.backend {
            #[cfg(feature = "v3")]
            Backend::Legacy(r) => r.read_stats(),
            Backend::V4(r) => r.file.source().stats,
        }
    }
    /// Restarts [`Self::read_stats`] from zero.
    pub fn reset_read_stats(&mut self) {
        match &mut self.backend {
            #[cfg(feature = "v3")]
            Backend::Legacy(r) => r.reset_read_stats(),
            Backend::V4(r) => r.file.source_mut().stats = ReadStats::default(),
        }
    }
    /// Returns the source, for example to reuse a connection; the reader's
    /// parsed state is dropped.
    pub fn into_inner(self) -> R {
        match self.backend {
            #[cfg(feature = "v3")]
            Backend::Legacy(r) => r.into_inner(),
            Backend::V4(r) => r.file.into_inner().inner,
        }
    }
    #[cfg(feature = "native-jpeg")]
    /// Native component description for coefficient archive access.
    pub fn native_header(&self) -> Option<&crate::jpeg::NativeHeader> {
        match &self.backend {
            #[cfg(feature = "v3")]
            Backend::Legacy(r) => r.native_header(),
            Backend::V4(_) => None,
        }
    }
    /// Native component band height, when the historical component grammar is used.
    #[cfg(feature = "native-jpeg")]
    pub fn native_mcu_rows_per_band(&self) -> Option<u16> {
        match &self.backend {
            #[cfg(feature = "v3")]
            Backend::Legacy(r) => r.native_mcu_rows_per_band(),
            Backend::V4(_) => None,
        }
    }
    #[cfg(feature = "native-jpeg")]
    /// Complete native band span as an absolute file range, including its
    /// optional side record. Uses the verified unit index or walks record
    /// heads; it does not fetch or reconstruct coefficients/pixels.
    pub fn native_band_span(&mut self, band: usize) -> Result<core::ops::Range<u64>, Error> {
        match &mut self.backend {
            #[cfg(feature = "v3")]
            Backend::Legacy(r) => r.native_band_span(band),
            Backend::V4(_) => Err(Error::InvalidInput("file has no native bands")),
        }
    }
    /// Decodes `rect` of the primary image into `out` (rows of `stride`
    /// bytes, `rect.width()` pixels each, native-endian samples as
    /// [`crate::decode_into`] writes them).
    pub fn decode_region(
        &mut self,
        rect: Rect,
        out: &mut [u8],
        stride: usize,
    ) -> Result<(), Error> {
        match &mut self.backend {
            #[cfg(feature = "v3")]
            Backend::Legacy(r) => r.decode_region(rect, out, stride),
            Backend::V4(r) => r.decode(rect, out, stride, 1, None),
        }
    }
    pub(crate) fn set_scoped_pixel_verification(&mut self, verify: bool) {
        match &mut self.backend {
            Backend::V4(reader) => reader.verify_scoped_pixels = verify,
            #[cfg(feature = "v3")]
            Backend::Legacy(_) => {}
        }
    }
    /// [`Self::decode_region`] on up to `threads` threads (units decode
    /// independently). Without `std` (or on WASM) it runs on one.
    pub fn decode_region_threads(
        &mut self,
        rect: Rect,
        out: &mut [u8],
        stride: usize,
        threads: usize,
    ) -> Result<(), Error> {
        match &mut self.backend {
            #[cfg(feature = "v3")]
            Backend::Legacy(r) => r.decode_region_threads(rect, out, stride, threads),
            Backend::V4(r) => r.decode(rect, out, stride, threads, None),
        }
    }
    /// Decode a crop and verify its declared float guarantees before returning
    /// success. The destination may have been written if verification fails.
    /// Only the returned crop is verified. Query [`Self::header`] for its
    /// declarations, including whether any guarantees are present.
    pub fn decode_region_with_float_guarantees(
        &mut self,
        rect: Rect,
        out: &mut [u8],
        stride: usize,
        threads: usize,
    ) -> Result<(), Error> {
        self.decode_region_threads(rect, out, stride, threads)?;
        let header = self.header();
        let mut info = header.info.clone();
        info.width = rect.width();
        info.height = rect.height();
        header.verify_float_guarantees(crate::ImageRef::new(out, stride, &info))
    }

    /// [`Self::decode_region_threads`], observed: each decoded unit is
    /// reported to `pulse` as a child named like the codestream trace's
    /// units (`stripe 3 group 1`), and [`Error::Cancelled`] is returned when
    /// the pulse asks to stop. Stop is checked before the reads, after them,
    /// and before each unit; rows of units not yet decoded are left as they
    /// were.
    pub fn decode_region_observed(
        &mut self,
        rect: Rect,
        out: &mut [u8],
        stride: usize,
        threads: usize,
        pulse: &dyn how_far::Pulse,
    ) -> Result<(), Error> {
        match &mut self.backend {
            #[cfg(feature = "v3")]
            Backend::Legacy(r) => r.decode_region_observed(rect, out, stride, threads, pulse),
            Backend::V4(r) => r.decode(rect, out, stride, threads, Some(pulse)),
        }
    }
    /// The file byte ranges [`Self::decode_region`] of `rect` would fetch,
    /// sorted and merged, without reading them: a caller that holds its
    /// bytes elsewhere (an HTTP range cache, a browser worker) fetches these
    /// and then decodes in one batch. The same size and budget checks apply.
    ///
    /// With a file checksum the ranges include the checksum pieces that are
    /// not yet verified, as the decode would fetch them; once a region
    /// decodes, later plans shrink accordingly. Without a unit table hint,
    /// planning reads record heads as decoding does (the units past those
    /// walked so far, and a DC-only unit's DC record head); those reads lie
    /// inside the returned ranges.
    /// V4 returns complete selected tile extents plus conservative uncached
    /// shared-model metadata dependencies and pending checksum pieces. This may
    /// include unused shared definitions; reconstruction resolves only selected
    /// group dependencies. It does not scan unrelated entropy during planning.
    pub fn region_ranges(&mut self, rect: Rect) -> Result<Vec<core::ops::Range<u64>>, Error> {
        match &mut self.backend {
            #[cfg(feature = "v3")]
            Backend::Legacy(r) => r.region_ranges(rect),
            Backend::V4(r) => r.ranges(rect),
        }
    }
    /// Whole-primary decoded bitmap coverage. A partial crop remains Pending.
    pub fn pixel_checksum_status(&self) -> crate::pixelsum::PixelChecksumStatus {
        match &self.backend {
            #[cfg(feature = "v3")]
            Backend::Legacy(r) => r.pixel_checksum_status(),
            Backend::V4(r) => r.file.pixel_status(),
        }
    }
}

/// Reads regions of the primary image from a random-access source.
///
/// Opening reads the header, the chunks before `DATA` (named by the DATA
/// locator when the file has a valid one, else walked), the `DATA` head
/// with its unit table hint, and the headers of any chunks after `DATA` (as a sequential
/// decoder visits them: a second `DATA` or an unknown critical chunk is an
/// error here too). The source must stay unchanged while the reader is used.
#[cfg(feature = "v3")]
struct LegacyRegionReader<R: ReadAt> {
    src: R,
    header: Header,
    file_len: u64,
    unified_sequence: bool,
    file_checksum_declared: bool,
    /// First byte after the complete primary DATA payload.
    primary_end: u64,
    /// File offset of the `DATA` payload, and its length unless open.
    data_at: u64,
    data_len: Option<u64>,
    /// The `DATA` head, parsed once at open.
    head: Head,
    geo: Geo,
    /// With a unit table hint: the start of every span (relative to `data_at`)
    /// and the end of the last (`geo.map` finds a unit's records).
    table: Option<Vec<u64>>,
    /// Without one: the start of every unit and the end of the last (walked
    /// at open).
    offsets: Vec<u64>,
    /// With exact blocks: every unit's exact record payload (start relative
    /// to `data_at`, length), found when the record heads are checked at
    /// open; empty otherwise.
    exact_at: Vec<(u64, u64)>,
    located: bool,
    /// The file checksum being verified, if any.
    check: Option<Check>,
    limits: Limits,
    stats: ReadStats,
    pixel_expected: Option<crate::Digest>,
    pixel_table: Option<crate::pixelsum::LocatedTable>,
    pixel_status: crate::pixelsum::PixelChecksumStatus,
}

#[cfg(feature = "v3")]
impl<R: ReadAt> LegacyRegionReader<R> {
    fn read_verified_at(&mut self, at: u64, bytes: &mut [u8]) -> Result<(), Error> {
        use crate::decode_resources::{DecodePolicy, DecodeSession};
        let end = at.checked_add(bytes.len() as u64).ok_or(Error::TooLarge)?;
        if end > self.file_len {
            return Err(Error::Corrupt("range outside IMI file"));
        }
        if self.file_checksum_declared && self.check.is_none() {
            return Err(Error::Unsupported("outer file checksum unavailable"));
        }
        let session = DecodeSession::new(
            DecodePolicy::default()
                .with_max_input_bytes(self.limits.max_bytes)
                .with_max_working_bytes(self.limits.max_bytes),
        );
        session.v4_input(bytes.len())?;
        let mut source = LegacyReads::new(&mut self.src, &mut self.stats);
        if let Some(check) = &mut self.check {
            if !check.pieces.has_table() {
                let (start, end) = check.pieces.chunk();
                let size = usize::try_from(end - start).map_err(|_| Error::TooLarge)?;
                session.v4_input(size)?;
                let _memory = session.v4_working(size as u128 * 2)?;
                let mut table = buffer(size)?;
                source.read_exact_at(start, &mut table)?;
                if !check.pieces.set_table(&table)? {
                    return Err(Error::Unsupported("outer file checksum unavailable"));
                }
            }
            source.read_exact_at(at, bytes)?;
            for range in check
                .opened
                .iter()
                .copied()
                .chain(core::iter::once((at, end)))
            {
                crate::v4::read_at::verify_interval(
                    &mut source,
                    &mut check.pieces,
                    range,
                    (at, bytes),
                    &session,
                )?;
            }
            check.opened.clear();
        } else {
            source.read_exact_at(at, bytes)?;
        }
        Ok(())
    }
    /// Resolve a positional main-record ordinal using the existing unit index.
    fn model_record(&mut self, ordinal: u64, role: u64) -> Result<(usize, u64, u64), Error> {
        let ordinal = usize::try_from(ordinal).map_err(|_| Error::TooLarge)?;
        let units = self.geo.units()?;
        let (mut lo, mut hi) = (0, units);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.geo.map.first(mid) <= ordinal {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let u = lo
            .checked_sub(1)
            .ok_or(Error::Corrupt("model defining ordinal"))?;
        let first = self.geo.map.first(u);
        let (kinds, n) = self
            .geo
            .map
            .kinds_at(u / self.geo.groups, u % self.geo.groups);
        let index = ordinal - first;
        let wanted = if role == 0 {
            records::R_DC
        } else {
            records::R_AC
        };
        if index >= n || kinds[index] != wanted {
            return Err(Error::Corrupt("model reference record role"));
        }
        let (mut at, end) = if let Some(table) = &self.table {
            (table[ordinal], table[ordinal + 1])
        } else {
            self.walk_to(u)?;
            (self.offsets[u], self.offsets[u + 1])
        };
        let ordinals = self.head.unit_ordinals();
        if self.table.is_some() {
            let (payload, end) = self.main_record(at, end, wanted, ordinals.then_some(ordinal))?;
            return Ok((u, payload, end));
        }
        for (i, &kind) in kinds[..=index].iter().enumerate() {
            let (payload, next) = self.main_record(at, end, kind, ordinals.then_some(first + i))?;
            if i == index {
                return Ok((u, payload, next));
            }
            at = next;
        }
        Err(Error::Corrupt("model defining record"))
    }

    /// Read exactly one varint. This path never speculates into entropy bytes.
    fn model_varint(
        &mut self,
        at: &mut u64,
        end: u64,
        ranges: &mut Vec<(u64, u64)>,
        budget: &ModelBudget,
    ) -> Result<(u64, Vec<u8>), Error> {
        use crate::parse::limits::Admission;
        let mut bytes = Vec::new();
        for _ in 0..10 {
            if *at >= end {
                return Err(Error::Corrupt("truncated model prefix"));
            }
            budget.wire_extra_bytes(1)?;
            let a = self.data_at + *at;
            let b = fetch(&mut self.src, &mut self.stats, &[(a, a + 1)])?
                .pop()
                .unwrap();
            ranges.push((a, a + 1));
            *at += 1;
            bytes.push(b[0]);
            if b[0] & 128 == 0 {
                let value = ByteReader::new(&bytes).varint()?;
                return Ok((value, bytes));
            }
        }
        Err(Error::Corrupt("model prefix varint"))
    }

    /// Fetch just a first-use definition, omitting its entropy payload.
    fn model_definition_prefix(
        &mut self,
        ordinal: u64,
        role: u64,
        budget: &ModelBudget,
    ) -> Result<Vec<u8>, Error> {
        use crate::parse::limits::Admission;
        let (u, mut at, end) = self.model_record(ordinal, role)?;
        let (map, blocks, ans) = match &self.head {
            Head::Lossy(p) => {
                let (map, blocks) = p.model_map_prefix(u);
                (map, blocks, p.model_uses_ans())
            }
            _ => return Err(Error::Corrupt("model reference outside lossy image")),
        };
        let mut ranges = Vec::new();
        if role == 1 && map == 1 {
            let (n, _) = self.model_varint(&mut at, end, &mut ranges, budget)?;
            at = at
                .checked_add(n)
                .filter(|&n| n <= end)
                .ok_or(Error::Corrupt("model map extent"))?;
        } else if role == 1 && map == 2 {
            let (kind, _) = self.model_varint(&mut at, end, &mut ranges, budget)?;
            let n = match kind {
                0 => 1,
                1 => blocks.checked_mul(3).ok_or(Error::TooLarge)?.div_ceil(8) as u64,
                _ => return Err(Error::Corrupt("model layout map")),
            };
            at = at
                .checked_add(n)
                .filter(|&n| n <= end)
                .ok_or(Error::Corrupt("model layout extent"))?;
        }
        let mut out = Vec::new();
        let (selector, bytes) = self.model_varint(&mut at, end, &mut ranges, budget)?;
        if selector != 3 {
            return Err(Error::Corrupt("model reference target is not a definition"));
        }
        out.extend(bytes);
        for _ in 0..4 {
            let (_, bytes) = self.model_varint(&mut at, end, &mut ranges, budget)?;
            out.extend(bytes);
        }
        let (len, bytes) = self.model_varint(&mut at, end, &mut ranges, budget)?;
        out.extend(bytes);
        let to = at
            .checked_add(len)
            .filter(|&to| to <= end)
            .ok_or(Error::Corrupt("model definition extent"))?;
        budget.wire_extra_bytes(u128::from(len) * 2)?;
        let n = usize::try_from(len).map_err(|_| Error::TooLarge)?;
        out.try_reserve(n).map_err(|_| Error::TooLarge)?;
        let a = self.data_at + at;
        let b = self.data_at + to;
        out.extend_from_slice(
            &fetch(&mut self.src, &mut self.stats, &[(a, b)])?
                .pop()
                .unwrap(),
        );
        ranges.push((a, b));
        at = to;
        if role == 0 || ans {
            let (_, bytes) = self.model_varint(&mut at, end, &mut ranges, budget)?;
            out.extend(bytes);
        }
        if let Some(c) = &mut self.check {
            let pending = c.pieces.pending(&ranges, true)?;
            if !pending.is_empty() {
                let left = budget
                    .max
                    .saturating_sub(budget.used.get())
                    .min(u128::from(STREAM_BUFFER));
                if left == 0 {
                    return Err(Error::TooLarge);
                }
                let mut buf = buffer(left as usize)?;
                for i in pending {
                    let d =
                        digest_streamed(&mut self.src, &mut self.stats, &c.pieces, i, &mut buf)?;
                    c.pieces.check(i, &d)?;
                }
            }
        }
        if let Head::Lossy(p) = &self.head {
            p.check_first_use_prefix(role, &out)?;
        }
        Ok(out)
    }

    fn admit_region_models(
        &mut self,
        prefixes: &[(u64, u64, &[u8])],
        held: u64,
        groups: (usize, usize),
        threads: usize,
    ) -> Result<(), Error> {
        let used = match &self.head {
            Head::Lossy(p) if p.has_model_references() => {
                p.model_storage_bytes()
                    + u128::from(held)
                    + u128::from(crate::lossy::region_unit_bytes(p, groups.0, groups.1)?)
                        * (threads.clamp(1, prefixes.len().max(1)) as u128)
            }
            _ => return Ok(()),
        };
        let budget = ModelBudget {
            used: core::cell::Cell::new(used),
            max: u128::from(self.limits.max_bytes),
        };
        if used > budget.max {
            return Err(Error::TooLarge);
        }
        for &(ordinal, role, bytes) in prefixes {
            let dependency = match &self.head {
                Head::Lossy(p) => p.referenced_model(ordinal, role, bytes)?,
                _ => None,
            };
            if let Some(target) = dependency {
                let definition = self.model_definition_prefix(target, role, &budget)?;
                if let Head::Lossy(p) = &mut self.head {
                    p.admit_model_prefix(target, role, &definition, &budget)?;
                }
            }
            if let Head::Lossy(p) = &mut self.head {
                p.admit_model_prefix(ordinal, role, bytes, &budget)?;
            }
        }
        Ok(())
    }

    fn open_legacy(
        mut src: R,
        limits: &Limits,
        file_len: u64,
        mut win: Window,
        mut stats: ReadStats,
        header_end: u64,
    ) -> Result<RegionReader<R>, Error> {
        win.ensure(&mut src, &mut stats, file_len, 0, header_end)?;
        let mut r = ByteReader::new(win.slice(0, header_end));
        let hp = crate::parse::header::read_header_sized(&mut r)?;
        // What this build cannot decode is refused from the header (SPEC 0),
        // and so is a malformed checksum record (SPEC 1.4).
        crate::parse::header::check_decodable(&hp.header, hp.declared)?;
        if let (true, Some(c)) = (limits.verify_checksum, hp.checksum) {
            c.announced.log2()?;
        }
        let mixed_pages = hp.mixed_pages();
        let (header, declared, locator) = (hp.header, hp.declared, hp.locator);
        let announced = hp.checksum.map(|c| c.announced);
        let file_checksum_declared = announced.is_some();
        // The chunk rules, what opening reads (for a file checksum) and where
        // the cSUM chunk sits.
        let mut opened = Opened {
            rules: Rules::new(mixed_pages, limits.verify_checksum).with_sequence_items(
                declared.contains(crate::parse::header::Feature::SequenceItems),
            ),
            used: Used::default(),
            csum: None,
            unified_sequence: false,
            pixel_expected: None,
            pixel_table: None,
            pixel_status: crate::pixelsum::PixelChecksumStatus::Absent,
        };
        opened.used.push(0, header_end)?;
        // A rule or parse error with the table read before it: a mismatching
        // piece under the bytes read explains it (SPEC 1.4).
        let verify = limits.verify_checksum;
        let explain = |src: &mut R, stats: &mut ReadStats, csum, end: u64, e| {
            explain_open(
                src,
                stats,
                file_len,
                announced,
                verify,
                csum,
                end,
                e,
                limits.max_bytes,
            )
        };
        // Past the bytes opening used: the failing chunk's header.
        let read_to = |o: &Opened| o.used.0.iter().map(|r| r.1).max().unwrap_or(0) + HEAD;
        // The DATA payload's position and length, and the bytes of its head:
        // through the locator in one batch, else by walking.
        let mut found = None;
        if let Some(loc) = &locator {
            found = locate(
                &mut src,
                &mut stats,
                &win,
                file_len,
                header_end,
                loc,
                &header,
                declared,
                limits.max_bytes,
                &mut opened,
            )
            .map_err(|e| explain(&mut src, &mut stats, opened.csum, read_to(&opened), e))?;
        }
        let located = found.is_some();
        let (data_at, data_len, p) = match found {
            Some(f) => f,
            None => {
                let (data_at, data_len) = walk_chunks(
                    &mut src,
                    &mut stats,
                    &mut win,
                    file_len,
                    header_end,
                    &mut opened,
                )
                .map_err(|e| explain(&mut src, &mut stats, opened.csum, read_to(&opened), e))?;
                let room = data_len.unwrap_or(file_len - data_at);
                // The head and table: from the window the walk holds, then
                // in growing windows while they are cut short.
                let held = (win.at + win.buf.len() as u64).saturating_sub(data_at);
                let mut want = if win.holds(data_at, data_at) && held > 0 {
                    held.min(room)
                } else {
                    WINDOW.min(room)
                };
                let prefix = loop {
                    win.ensure(&mut src, &mut stats, file_len, data_at, data_at + want)?;
                    let bytes = win.slice(data_at, data_at + want);
                    match parse_prefix(&header, declared, bytes, room, limits.max_bytes) {
                        Ok(p) => break p,
                        Err(e) if truncated(&e) && want < room => {
                            want = (want * 2).max(WINDOW).min(room)
                        }
                        Err(e) => {
                            let end = data_at + want;
                            return Err(explain(&mut src, &mut stats, opened.csum, end, e));
                        }
                    }
                };
                (data_at, data_len, prefix)
            }
        };
        let Prefix {
            head,
            head_len,
            geo,
            lens,
            reserve,
        } = p;
        let prefix_end = head_len;
        opened.used.push(data_at, data_at + prefix_end as u64)?;
        let units = geo.units()?;
        let mut reader = Self {
            src,
            header,
            file_len,
            unified_sequence: opened.unified_sequence,
            file_checksum_declared,
            primary_end: 0,
            data_at,
            data_len,
            head,
            geo,
            table: None,
            offsets: alloc::vec![head_len as u64],
            exact_at: Vec::new(),
            located,
            check: None,
            limits: *limits,
            stats,
            pixel_expected: opened.pixel_expected,
            pixel_table: opened.pixel_table,
            pixel_status: opened.pixel_status,
        };
        // A check-tail head: the spans come from the trailing unit index
        // at the end of the file, when it holds one (SPEC 2.7).
        let mut lens = lens;
        // Where DATA ends by the trailing index the spans came from.
        let mut index_end = None;
        if lens.is_none() && reserve.is_some_and(|r| r.check_tail()) {
            let found = find_index(
                &mut reader.src,
                &mut reader.stats,
                &win,
                file_len,
                data_at,
                data_len,
                head_len as u64,
                &reader.geo,
            );
            match found {
                Ok(Some(f)) => {
                    opened.used.push(f.at, file_len)?;
                    reader.data_len = Some(f.data_len);
                    index_end = Some(data_at + f.data_len);
                    lens = Some(f.entries);
                }
                Ok(None) => {}
                Err(e) => {
                    // DATA's end: its length, or the one the footer claims.
                    let end = match data_len {
                        Some(l) => Some(data_at + l),
                        None => footer_data_len(&mut reader.src, &mut reader.stats, file_len)
                            .and_then(|l| data_at.checked_add(l)),
                    };
                    let (src, stats) = (&mut reader.src, &mut reader.stats);
                    return Err(explain_index(
                        src,
                        stats,
                        file_len,
                        (announced, verify, limits.max_bytes),
                        &mut opened,
                        end,
                        e,
                    ));
                }
            }
        }
        let data_len = reader.data_len;
        // DATA ends after the last record of the table, or of the walk; the
        // reader stops there (SPEC 1.1: chunks after DATA hold no part of
        // the primary image).
        let data_end = match lens {
            Some(lens) => {
                let mut lens_copy = Vec::new();
                lens_copy
                    .try_reserve_exact(lens.len())
                    .map_err(|_| Error::TooLarge)?;
                lens_copy.extend_from_slice(&lens);
                let mut starts = Vec::new();
                starts
                    .try_reserve_exact(lens.len() + 1)
                    .map_err(|_| Error::TooLarge)?;
                let mut at = prefix_end as u64;
                starts.push(at);
                for l in lens {
                    at += l;
                    starts.push(at);
                }
                // The hint's spans fit the payload (parse_prefix); they must
                // also fill it.
                let hint = lens_copy;
                if data_len.is_some_and(|l| l != at) {
                    let e = Error::Corrupt("trailing DATA bytes");
                    let end = data_at + prefix_end as u64;
                    return Err(explain(
                        &mut reader.src,
                        &mut reader.stats,
                        opened.csum,
                        end,
                        e,
                    ));
                }
                // With unit_ordinals (head record 37, SPEC 2.7), each main
                // record carries its 0-based ordinal, checked locally when a
                // region decodes; otherwise verify_hint checks every span's
                // record head at open.
                let ordinals = reader.head.unit_ordinals();
                let verified = if ordinals
                    && !matches!(&reader.head, Head::Lossy(p) if p.has_exact_blocks())
                {
                    if hint.len() != reader.geo.map.count(reader.geo.stripes)? {
                        Err(Error::Corrupt("unit table length"))
                    } else {
                        Ok(())
                    }
                } else {
                    verify_hint(
                        &mut reader.src,
                        &mut reader.stats,
                        &win,
                        data_at,
                        reader.header.version,
                        &starts,
                        hint,
                        &reader.geo,
                        ordinals,
                        limits.max_bytes,
                    )
                    .and_then(|exact_at| {
                        reader.exact_at = exact_at;
                        reader.check_exact_records()
                    })
                };
                if let Err(e) = verified {
                    let (src, stats) = (&mut reader.src, &mut reader.stats);
                    if index_end.is_some() {
                        let check = (announced, verify, limits.max_bytes);
                        let o = &mut opened;
                        return Err(explain_index(src, stats, file_len, check, o, index_end, e));
                    }
                    let end = data_at + at;
                    return Err(explain(src, stats, opened.csum, end, e));
                }
                reader.table = Some(starts);
                at
            }
            // No table: walk every record, so a payload with bytes after
            // its last record is corrupt here as in every full decoder.
            None => {
                let walked = reader.walk_to(units - 1).and_then(|()| {
                    reader.check_exact_records()?;
                    let end = reader.offsets[units];
                    match data_len {
                        Some(l) if l != end => Err(Error::Corrupt("trailing DATA bytes")),
                        _ => Ok(end),
                    }
                });
                let end = match walked {
                    Ok(end) => end,
                    Err(e) => {
                        // The record lengths read so far.
                        let to = data_at + reader.offsets.last().unwrap() + 10;
                        let src = (&mut reader.src, &mut reader.stats);
                        return Err(explain(src.0, src.1, opened.csum, to, e));
                    }
                };
                // The walk read every record's length.
                opened.used.push(data_at + head_len as u64, data_at + end)?;
                end
            }
        };
        reader.primary_end = data_at.checked_add(data_end).ok_or(Error::TooLarge)?;
        if limits.verify_checksum {
            reader.check = match (announced, opened.csum) {
                (None, None) => None,
                (None, Some(_)) => {
                    return Err(Error::Corrupt("file checksum without its header record"));
                }
                (Some(a), c) => {
                    let log2 = a.log2()?;
                    // A writer that streams puts the table after DATA: the
                    // one case the reader looks past DATA's end.
                    let (at, len) = match c {
                        Some(c) => c,
                        None => find_after(
                            &mut reader.src,
                            &mut reader.stats,
                            file_len,
                            data_at + data_end,
                            &mut opened,
                        )?
                        .ok_or(Error::Corrupt("missing file checksum"))?,
                    };
                    Some(Check {
                        pieces: crate::checksum::Fetched::new(
                            at,
                            len,
                            log2,
                            a.algorithm_id()?,
                            file_len,
                        )?,
                        opened: opened.used.0,
                    })
                }
            };
        }
        Ok(RegionReader {
            backend: Backend::Legacy(reader),
        })
    }

    /// The file's header (the primary image's coding, size, color and
    /// profiles), as parsed at open. With a file checksum it is verified
    /// with the first region, not before.
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// The primary image's dimensions, channels and sample type
    /// (`header().info`): what [`Self::decode_region`] writes.
    pub fn info(&self) -> &ImageInfo {
        &self.header.info
    }

    /// First byte after primary DATA. A sequence index can use it to locate
    /// the first following `fRAM` control without reading earlier pixels.
    pub fn primary_end(&self) -> u64 {
        self.primary_end
    }

    /// True when the pre-DATA `aNIM` declares unified sequence items (kind 4).
    /// Legacy animations and pages use different reconstruction rules.
    pub fn is_unified_sequence(&self) -> bool {
        self.unified_sequence
    }

    /// Whether this file declares a supported outer file checksum. Partial
    /// sequence consumers must verify the pieces under selected later items
    /// before relying on such a checksum.
    pub fn has_file_checksum(&self) -> bool {
        self.file_checksum_declared
    }

    /// True when the payload's head carries the unit table hint, or says
    /// check tail and the file ends with its trailing unit index (SPEC
    /// 2.7): every unit is found from it, without walking records; every
    /// span's record heads were checked against it at open.
    pub fn has_unit_table(&self) -> bool {
        self.table.is_some()
    }

    /// True when the file's DATA locator matched its chunks and was used.
    pub fn used_locator(&self) -> bool {
        self.located
    }

    /// Reads made through the source so far.
    pub fn read_stats(&self) -> ReadStats {
        self.stats
    }

    /// Restarts [`Self::read_stats`] from zero.
    pub fn reset_read_stats(&mut self) {
        self.stats = ReadStats::default();
    }

    /// Returns the source, for example to reuse a connection; the reader's
    /// parsed state is dropped.
    pub fn into_inner(self) -> R {
        self.src
    }

    /// Native component description for coefficient archive access.
    #[cfg(feature = "native-jpeg")]
    pub fn native_header(&self) -> Option<&crate::jpeg::NativeHeader> {
        match &self.head {
            #[cfg(feature = "native-jpeg")]
            Head::Native(p) => Some(&p.header),
            _ => None,
        }
    }

    #[cfg(feature = "native-jpeg")]
    pub fn native_mcu_rows_per_band(&self) -> Option<u16> {
        match &self.head {
            #[cfg(feature = "native-jpeg")]
            Head::Native(p) => Some(p.rows),
            _ => None,
        }
    }

    /// Complete native band span as an absolute file range, including its
    /// optional side record. Uses the verified unit index or walks record
    /// heads; it does not fetch or reconstruct coefficients/pixels.
    #[cfg(feature = "native-jpeg")]
    pub fn native_band_span(&mut self, band: usize) -> Result<core::ops::Range<u64>, Error> {
        if !matches!(self.head, Head::Native(_)) {
            return Err(Error::InvalidInput("file has no native bands"));
        }
        if band >= self.geo.stripes {
            return Err(Error::InvalidInput("native band index"));
        }
        let (start, end) = if let Some(table) = &self.table {
            (table[band], table[band + 1])
        } else {
            self.walk_to(band)?;
            (self.offsets[band], self.offsets[band + 1])
        };
        Ok(self.data_at + start..self.data_at + end)
    }

    /// Decodes `rect` of the primary image into `out` (rows of `stride`
    /// bytes, `rect.width()` pixels each, native-endian samples as
    /// [`crate::decode_into`] writes them).
    pub fn decode_region(
        &mut self,
        rect: Rect,
        out: &mut [u8],
        stride: usize,
    ) -> Result<(), Error> {
        self.decode_region_threads(rect, out, stride, 1)
    }

    /// [`Self::decode_region`] on up to `threads` threads (units decode
    /// independently). Without `std` (or on WASM) it runs on one.
    pub fn decode_region_threads(
        &mut self,
        rect: Rect,
        out: &mut [u8],
        stride: usize,
        threads: usize,
    ) -> Result<(), Error> {
        self.decode_region_with(rect, out, stride, threads, None)
    }

    /// [`Self::decode_region_threads`], observed: each decoded unit is
    /// reported to `pulse` as a child named like the codestream trace's
    /// units (`stripe 3 group 1`), and [`Error::Cancelled`] is returned when
    /// the pulse asks to stop. Stop is checked before the reads, after them,
    /// and before each unit; rows of units not yet decoded are left as they
    /// were.
    pub fn decode_region_observed(
        &mut self,
        rect: Rect,
        out: &mut [u8],
        stride: usize,
        threads: usize,
        pulse: &dyn how_far::Pulse,
    ) -> Result<(), Error> {
        self.decode_region_with(rect, out, stride, threads, Some(pulse))
    }

    fn decode_region_with(
        &mut self,
        rect: Rect,
        out: &mut [u8],
        stride: usize,
        threads: usize,
        pulse: Option<&dyn how_far::Pulse>,
    ) -> Result<(), Error> {
        if self.pixel_expected.is_some() {
            self.pixel_status = crate::pixelsum::PixelChecksumStatus::Pending;
        }
        let info = self.header.info.clone();
        let (x, y, w, h) = (
            rect.x() as usize,
            rect.y() as usize,
            rect.width() as usize,
            rect.height() as usize,
        );
        if w == 0
            || h == 0
            || x.checked_add(w).is_none_or(|e| e > info.width as usize)
            || y.checked_add(h).is_none_or(|e| e > info.height as usize)
        {
            return Err(Error::InvalidInput("region outside the image"));
        }
        let bpp = info.row_bytes() / info.width as usize;
        let rb = w.checked_mul(bpp).ok_or(Error::TooLarge)?;
        let extent = (h - 1).checked_mul(stride).and_then(|n| n.checked_add(rb));
        if stride < rb || extent.is_none_or(|n| out.len() < n) {
            return Err(Error::InvalidInput("output buffer too small"));
        }
        let RegionPlan {
            s: (s0, s1),
            g: (g0, g1),
            d: (d0, d1),
            dc_only,
            dc_ranges,
            stripe_ranges,
            file_runs,
            pieces,
            streamed,
        } = self.plan_region(rect, threads)?;
        let geo = self.geo.clone();
        let gn = g1 - g0;
        let ng = geo.groups;
        let first = |u: usize| geo.map.first(u);
        let data_at = self.data_at;
        let obs = RegionObs::new(pulse, &geo, (s0, s1), (g0, g1), threads);
        obs.check()?;
        // One batch: every run at once.
        let bufs = fetch(&mut self.src, &mut self.stats, &file_runs)?;
        obs.check()?;
        /// Bytes `[a, b)` of the file from the fetched runs.
        fn piece<'b>(runs: &[(u64, u64)], bufs: &'b [Vec<u8>], a: u64, b: u64) -> &'b [u8] {
            let i = runs.partition_point(|r| r.1 < b);
            let r = runs[i];
            &bufs[i][(a - r.0) as usize..(b - r.0) as usize]
        }
        let in_file = |a: u64, b: u64| piece(&file_runs, &bufs, a, b);
        if let Some(c) = &mut self.check {
            let (ca, cb) = c.pieces.chunk();
            if !c.pieces.has_table() && !c.pieces.set_table(in_file(ca, cb))? {
                // An algorithm this reader does not know: not checked.
                self.check = None;
            } else {
                for i in pieces {
                    let parts = c.pieces.segments(i);
                    let d = crate::checksum::digest_of(
                        c.pieces.algo(),
                        parts
                            .iter()
                            .filter(|r| r.0 < r.1)
                            .map(|&(a, b)| in_file(a, b)),
                    );
                    c.pieces.check(i, &d)?;
                }
                if !streamed.is_empty() {
                    let held = total(&file_runs);
                    let n = (self.limits.max_bytes - held).min(STREAM_BUFFER) as usize;
                    let mut buf = buffer(n)?;
                    for i in streamed {
                        let d = digest_streamed(
                            &mut self.src,
                            &mut self.stats,
                            &c.pieces,
                            i,
                            &mut buf,
                        )?;
                        c.pieces.check(i, &d)?;
                    }
                }
                c.opened.clear();
            }
        }
        // Resolve explicit definition dependencies before decoding any selected unit.
        // These slices borrow only the fetched buffers, not the retained head.
        let mut model_prefixes = Vec::new();
        if let Head::Lossy(p) = &self.head
            && p.has_model_references()
        {
            let count = (s1 - s0)
                .checked_mul(gn)
                .and_then(|n| n.checked_mul(2))
                .and_then(|n| n.checked_add(dc_ranges.len()))
                .ok_or(Error::TooLarge)?;
            let bytes = count
                .checked_mul(core::mem::size_of::<(u64, u64, &[u8])>())
                .ok_or(Error::TooLarge)?;
            if (bytes as u128) + u128::from(total(&file_runs)) > u128::from(self.limits.max_bytes) {
                return Err(Error::TooLarge);
            }
            model_prefixes
                .try_reserve_exact(count)
                .map_err(|_| Error::TooLarge)?;
            let slice = |a: u64, b: u64| in_file(data_at + a, data_at + b);
            let ordinals = self.head.unit_ordinals();
            let version = self.header.version;
            for s in s0..s1 {
                for g in g0..g1 {
                    let u = s * ng + g;
                    let first = self.geo.map.first(u);
                    let (kinds, n) = self.geo.map.kinds_at(s, g);
                    let mut recs = [&[][..]; 4];
                    if let Some(table) = &self.table {
                        for i in 0..n {
                            records::split_span_indexed(
                                slice(table[first + i], table[first + i + 1]),
                                version,
                                &[kinds[i]],
                                ordinals.then_some(first + i),
                                &mut recs[i..i + 1],
                            )?;
                        }
                    } else {
                        records::split_span_indexed(
                            slice(self.offsets[u], self.offsets[u + 1]),
                            version,
                            &kinds[..n],
                            ordinals.then_some(first),
                            &mut recs[..n],
                        )?;
                    }
                    for i in 0..n {
                        let role = match kinds[i] {
                            records::R_DC => 0,
                            records::R_AC => 1,
                            _ => continue,
                        };
                        model_prefixes.push((
                            (first + i) as u64,
                            role,
                            p.model_prefix(u, role, recs[i])?,
                        ));
                    }
                }
            }
            for &(ordinal, a, b) in &dc_ranges {
                let mut rec = [&[][..]];
                records::split_span_indexed(
                    slice(a, b),
                    version,
                    &[records::R_DC],
                    ordinals.then_some(ordinal),
                    &mut rec,
                )?;
                model_prefixes.push((ordinal as u64, 0, rec[0]));
            }
            model_prefixes.sort_unstable_by_key(|v| v.0);
            model_prefixes.dedup_by_key(|v| v.0);
        }
        self.admit_region_models(&model_prefixes, total(&file_runs), (g0, g1), threads)?;
        // Bytes `[a, b)` of `DATA`.
        let slice = |a: u64, b: u64| in_file(data_at + a, data_at + b);
        // The payload of main record `r` (of kind `kind`), from its span in
        // the hint, checked against the records it holds.
        let table = self.table.as_ref();
        let ordinals = self.head.unit_ordinals();
        let version = self.header.version;
        let record = |r: usize, kind: u64| -> Result<&[u8], Error> {
            let starts = table.unwrap();
            let mut out: [&[u8]; 1] = [&[]];
            records::split_span_indexed(
                slice(starts[r], starts[r + 1]),
                version,
                &[kind],
                ordinals.then_some(r),
                &mut out,
            )?;
            Ok(out[0])
        };
        match &self.head {
            #[cfg(feature = "native-jpeg")]
            Head::Native(p) => {
                let mut units = Vec::new();
                for s in s0..s1 {
                    let mut rec = [&[][..]];
                    match table {
                        Some(_) => rec[0] = record(first(s), records::R_NATIVE)?,
                        None => records::split_span(
                            slice(self.offsets[s], self.offsets[s + 1]),
                            version,
                            &[records::R_NATIVE],
                            &mut rec,
                        )?,
                    };
                    units.push(rec[0]);
                }
                crate::jpeg::wire::decode_region(p, s0, &units, (x, y, w, h), out, stride)
            }
            Head::Lossless(p) => {
                let mut units = Vec::with_capacity((s1 - s0) * gn);
                for s in s0..s1 {
                    for g in g0..g1 {
                        let u = s * ng + g;
                        let mut rec: [&[u8]; 1] = [&[]];
                        match table {
                            Some(_) => rec[0] = record(first(u), records::R_STRIPE)?,
                            None => records::split_span_indexed(
                                slice(self.offsets[u], self.offsets[u + 1]),
                                version,
                                &[records::R_STRIPE],
                                ordinals.then_some(first(u)),
                                &mut rec,
                            )?,
                        }
                        units.push(rec[0]);
                    }
                }
                crate::v1::decode_region_units(
                    p,
                    (x, y, w, h),
                    (s0, s1, g0, g1),
                    &units,
                    out,
                    stride,
                    threads,
                    &obs,
                )
            }
            Head::Lossy(p) => {
                let mut dc: Vec<(usize, &[u8])> = Vec::new();
                let mut units = Vec::with_capacity((s1 - s0) * gn);
                if dc_only {
                    for &(r, a, b) in &dc_ranges {
                        let mut rec: [&[u8]; 1] = [&[]];
                        records::split_span_indexed(
                            slice(a, b),
                            version,
                            &[records::R_DC],
                            ordinals.then_some(r),
                            &mut rec,
                        )?;
                        dc.push((r, rec[0]));
                    }
                }
                for s in s0..s1 {
                    for g in g0..g1 {
                        // The unit's DC record (when its stripe starts a DC
                        // stripe), alpha record (with alpha), AC record and
                        // exact record (with exact blocks; its stripes come
                        // through `stripe_ranges`).
                        let u = s * ng + g;
                        let (kinds, n) = geo.map.kinds_at(s, g);
                        let mut recs: [&[u8]; 4] = [&[]; 4];
                        let recs = &mut recs[..n];
                        match table {
                            Some(_) => {
                                for (i, r) in recs.iter_mut().enumerate() {
                                    *r = record(first(u) + i, kinds[i])?;
                                }
                            }
                            None => records::split_span_indexed(
                                slice(self.offsets[u], self.offsets[u + 1]),
                                version,
                                &kinds[..n],
                                ordinals.then_some(first(u)),
                                recs,
                            )?,
                        }
                        let recs: &[&[u8]] = if p.has_exact_blocks() {
                            &recs[..n - 1]
                        } else {
                            recs
                        };
                        let (ac, rest) = recs.split_last().unwrap();
                        let (alpha, rest) = match rest.split_last() {
                            Some((a, r)) if geo.alpha => (*a, r),
                            _ => (&[][..], rest),
                        };
                        if let Some(&dc_bytes) = rest.first() {
                            dc.push((first(u), dc_bytes));
                        }
                        units.push((alpha, *ac));
                    }
                }
                // DC records in (DC stripe, group) order: the DC-only ones
                // came first, the rest stripe by stripe.
                dc.sort_unstable_by_key(|&(id, _)| id);
                dc.dedup_by_key(|v| v.0);
                if p.dc_coverage.is_none() && dc.len() != (d1 - d0 + 1) * gn {
                    return Err(Error::Corrupt("DC records of the region"));
                }
                let (y0, y1) = (s0 * geo.rows, (s1 * geo.rows).min(geo.height));
                let (bx0, bx1) = (geo.group(g0).0, {
                    let (a, w) = geo.group(g1 - 1);
                    a + w
                });
                let bstride = (bx1 - bx0) * bpp;
                let blen = bstride
                    .checked_mul(y1 - y0)
                    .filter(|&n| n as u64 <= self.limits.max_bytes)
                    .ok_or(Error::TooLarge)?;
                let mut boxed = Vec::new();
                boxed.try_reserve_exact(blen).map_err(|_| Error::TooLarge)?;
                boxed.resize(blen, 0);
                let mut stripes = Vec::new();
                stripes
                    .try_reserve_exact(stripe_ranges.len())
                    .map_err(|_| Error::TooLarge)?;
                stripes.extend(stripe_ranges.iter().map(|&(t, a, b)| (t, slice(a, b))));
                crate::lossy::decode_lossy_region(
                    p,
                    (s0, s1, g0, g1),
                    &dc,
                    &units,
                    &stripes,
                    &mut boxed,
                    bstride,
                    threads,
                    &obs,
                )?;
                for (i, row) in out.chunks_mut(stride).take(h).enumerate() {
                    let src = (y + i - y0) * bstride + (x - bx0) * bpp;
                    row[..rb].copy_from_slice(&boxed[src..src + rb]);
                }
                Ok(())
            }
        }?;
        if x == 0
            && y == 0
            && w == info.width as usize
            && h == info.height as usize
            && let Some(expected) = self.pixel_expected
        {
            if crate::pixelsum::digest_with(expected.algorithm(), &info, 0, out, stride)?
                != expected
            {
                return Err(crate::pixelsum::MISMATCH);
            }
            self.pixel_status = crate::pixelsum::PixelChecksumStatus::Verified;
        }
        Ok(())
    }

    /// The file byte ranges [`Self::decode_region`] of `rect` would fetch,
    /// sorted and merged, without reading them: a caller that holds its
    /// bytes elsewhere (an HTTP range cache, a browser worker) fetches these
    /// and then decodes in one batch. The same size and budget checks apply.
    ///
    /// With a file checksum the ranges include the checksum pieces that are
    /// not yet verified, as the decode would fetch them; once a region
    /// decodes, later plans shrink accordingly. Without a unit table hint,
    /// planning reads record heads as decoding does (the units past those
    /// walked so far, and a DC-only unit's DC record head); those reads lie
    /// inside the returned ranges.
    pub fn region_ranges(&mut self, rect: Rect) -> Result<Vec<core::ops::Range<u64>>, Error> {
        Ok(self
            .plan_region(rect, 1)?
            .file_runs
            .into_iter()
            .map(|(a, b)| a..b)
            .collect())
    }

    /// Whole-primary decoded bitmap coverage. A partial crop remains Pending.
    pub fn pixel_checksum_status(&self) -> crate::pixelsum::PixelChecksumStatus {
        self.pixel_status
    }

    /// Walks the unit records from the last known unit start up to the
    /// start of unit `u + 1` (payloads without the unit table hint), one
    /// read per record head.
    /// Plans [`Self::decode_region_threads`] for `rect`: its size checks and
    /// working-state budget, and every file range it fetches.
    fn plan_region(&mut self, rect: Rect, threads: usize) -> Result<RegionPlan, Error> {
        let info = &self.header.info;
        let (x, y, w, h) = (
            rect.x() as usize,
            rect.y() as usize,
            rect.width() as usize,
            rect.height() as usize,
        );
        if w == 0
            || h == 0
            || x.checked_add(w).is_none_or(|e| e > info.width as usize)
            || y.checked_add(h).is_none_or(|e| e > info.height as usize)
        {
            return Err(Error::InvalidInput("region outside the image"));
        }
        let bpp = info.row_bytes() / info.width as usize;
        let rb = w.checked_mul(bpp).ok_or(Error::TooLarge)?;
        if w.checked_mul(h)
            .is_none_or(|n| n as u64 > self.limits.max_pixels)
            || rb
                .checked_mul(h)
                .is_none_or(|n| n as u64 > self.limits.max_bytes)
        {
            return Err(Error::TooLarge);
        }
        let geo = self.geo.clone();
        let (s0, s1) = (y / geo.rows, (y + h - 1) / geo.rows + 1);
        #[cfg(feature = "native-jpeg")]
        let (s0, s1) = if matches!(self.head, Head::Native(_)) {
            (s0.saturating_sub(1), (s1 + 1).min(geo.stripes))
        } else {
            (s0, s1)
        };
        let (g0, g1) = (geo.group_of(x), geo.group_of(x + w - 1) + 1);
        let gn = g1 - g0;
        let ng = geo.groups;
        // Lossy: DC stripes d0..=d1; the first may start above the region.
        let (d0, d1) = (s0 / geo.k, (s1 - 1) / geo.k);
        let custom_dc = matches!(&self.head,Head::Lossy(p) if p.dc_coverage.is_some());
        let dc_only = geo.lossy && (custom_dc || d0 * geo.k < s0);
        let mut dc_units = Vec::new();
        if custom_dc {
            if let Head::Lossy(p) = &self.head {
                for g in g0..g1 {
                    for s in s0..s1 {
                        dc_units.push(p.dc_span(s, g).start * ng + g);
                    }
                }
                dc_units.sort_unstable();
                dc_units.dedup();
            }
        } else if dc_only {
            dc_units.extend((g0..g1).map(|g| d0 * geo.k * ng + g));
        }
        // Each work item decoding at once (a unit; lossy: a group's stripes
        // of one DC stripe) holds working state as wide as its column group:
        // charged to the budget before anything is fetched.
        let (unit, items) = match &self.head {
            #[cfg(feature = "native-jpeg")]
            Head::Native(p) => (
                u64::try_from(p.header.plan(p.rows)?.0).map_err(|_| Error::TooLarge)?,
                1,
            ),
            Head::Lossless(p) => (crate::v1::region_unit_bytes(p, g0, g1)?, (s1 - s0) * gn),
            Head::Lossy(p) => (
                crate::lossy::region_unit_bytes(p, g0, g1)?,
                (d1 - d0 + 1) * gn,
            ),
        };
        unit.checked_mul(threads.clamp(1, items) as u64)
            .filter(|&n| n <= self.limits.max_bytes)
            .ok_or(Error::TooLarge)?;
        // Byte ranges (relative to DATA) to fetch: whole units, plus the DC
        // record of each DC-only unit.
        let mut wanted: Vec<(u64, u64)> = Vec::new();
        let mut dc_ranges = Vec::new();
        let first = |u: usize| geo.map.first(u);
        if let Some(starts) = &self.table {
            for s in s0..s1 {
                wanted.push((starts[first(s * ng + g0)], starts[first(s * ng + g1)]));
            }
            if dc_only {
                for &g in &dc_units {
                    let r = first(g);
                    dc_ranges.push((r, starts[r], starts[r + 1]));
                }
            }
        } else {
            // No unit table hint: the record walk at open found every unit.
            self.walk_to((s1 - 1) * ng + g1 - 1)?;
            for s in s0..s1 {
                wanted.push((self.offsets[s * ng + g0], self.offsets[s * ng + g1]));
            }
            if dc_only {
                for &u in &dc_units {
                    let (start, end) = (self.offsets[u], self.offsets[u + 1]);
                    dc_ranges.push((first(u), start, self.main_end(start, end, records::R_DC)?));
                }
            }
        }
        wanted.extend(dc_ranges.iter().map(|&(_, a, b)| (a, b)));
        // Exact blocks: the nested stripes the region's units need, in
        // whichever unit's exact record each sits (the exact head places
        // them; the record payloads were found at open).
        let mut stripe_ranges: Vec<(usize, u64, u64)> = Vec::new();
        if let Head::Lossy(p) = &self.head
            && let Some(e) = p.exact()
        {
            let mut need: Vec<usize> = Vec::new();
            for s in s0..s1 {
                for g in g0..g1 {
                    let ts = e.stripes_of(e.range(s * ng + g)?);
                    need.try_reserve(ts.len()).map_err(|_| Error::TooLarge)?;
                    need.extend(ts);
                }
            }
            need.sort_unstable();
            need.dedup();
            stripe_ranges
                .try_reserve_exact(need.len())
                .map_err(|_| Error::TooLarge)?;
            for t in need {
                let at = e.stripes_at[t];
                let (payload, _) = *self
                    .exact_at
                    .get(at.unit)
                    .ok_or(Error::Corrupt("exact record"))?;
                let a = payload + at.offset as u64;
                stripe_ranges.push((t, a, a + at.len as u64));
            }
            wanted
                .try_reserve(stripe_ranges.len())
                .map_err(|_| Error::TooLarge)?;
            wanted.extend(stripe_ranges.iter().map(|&(_, a, b)| (a, b)));
        }
        wanted.sort_unstable();
        let mut runs: Vec<(u64, u64)> = Vec::new();
        for (a, b) in wanted {
            match runs.last_mut() {
                Some(r) if a <= r.1 => r.1 = r.1.max(b),
                _ => runs.push((a, b)),
            }
        }
        // The records are fetched whole: charged to the budget too.
        let max = self.limits.max_bytes;
        if total(&runs) > max {
            return Err(Error::TooLarge);
        }
        let data_at = self.data_at;
        let mut file_runs: Vec<(u64, u64)> = runs
            .iter()
            .map(|&(a, b)| (data_at + a, data_at + b))
            .collect();
        // With a file checksum the first region also brings the table and
        // every unchecked piece under these runs and under what opening
        // read. The fetch plan is charged to the budget as a whole before
        // anything is allocated: the records and the table (held) must fit
        // it, else `TooLarge`; the pieces come whole in the same batch when
        // they fit too, else each is hashed after the batch through a
        // buffer of what the budget leaves (at most 1 MiB).
        let mut pieces = Vec::new();
        let mut streamed = Vec::new();
        if let Some(c) = &mut self.check {
            let mut under = c.opened.clone();
            under.extend_from_slice(&file_runs);
            pieces = c.pieces.pending(&under, true)?;
            if !c.pieces.has_table() {
                file_runs.push(c.pieces.chunk());
            }
            let held = total(&merged(file_runs.clone()));
            if held > max {
                return Err(Error::TooLarge);
            }
            let mut whole = file_runs.clone();
            for &i in &pieces {
                whole.extend(c.pieces.segments(i).into_iter().filter(|r| r.0 < r.1));
            }
            if total(&merged(whole.clone())) <= max {
                file_runs = whole;
            } else {
                if held == max {
                    return Err(Error::TooLarge);
                }
                streamed = core::mem::take(&mut pieces);
            }
        }
        let file_runs = merged(file_runs);
        Ok(RegionPlan {
            s: (s0, s1),
            g: (g0, g1),
            d: (d0, d1),
            dc_only,
            dc_ranges,
            stripe_ranges,
            file_runs,
            pieces,
            streamed,
        })
    }

    fn walk_to(&mut self, u: usize) -> Result<(), Error> {
        let end = self.data_len.unwrap_or(self.file_len - self.data_at);
        let ordinals = self.head.unit_ordinals();
        while self.offsets.len() <= u + 1 {
            let v = self.offsets.len() - 1;
            let mut at = *self.offsets.last().unwrap();
            let first = self.geo.map.first(v);
            let (kinds, n) = self
                .geo
                .map
                .kinds_at(v / self.geo.groups, v % self.geo.groups);
            for (i, &kind) in kinds[..n].iter().enumerate() {
                let (payload, rec_end) =
                    self.main_record(at, end, kind, ordinals.then_some(first + i))?;
                if kind == records::R_EXACT {
                    self.exact_at.try_reserve(1).map_err(|_| Error::TooLarge)?;
                    self.exact_at.push((payload, rec_end - payload));
                }
                at = rec_end;
            }
            // Grows as records are found, never ahead of them.
            self.offsets.try_reserve(1).map_err(|_| Error::TooLarge)?;
            self.offsets.push(at);
        }
        Ok(())
    }

    /// The end of main record `kind` read from `at` (relative to `DATA`; at
    /// most `end`), after its side record if it has one: the same
    /// [`records::Walk`] as every other route.
    fn main_end(&mut self, at: u64, end: u64, kind: u64) -> Result<u64, Error> {
        Ok(self.main_record(at, end, kind, None)?.1)
    }

    /// [`Self::main_end`], with where the main record's payload starts.
    fn main_record(
        &mut self,
        mut at: u64,
        end: u64,
        kind: u64,
        expected_ordinal: Option<usize>,
    ) -> Result<(u64, u64), Error> {
        let mut walk = records::Walk::new(self.header.version, None);
        let head_max = if expected_ordinal.is_some() { 30 } else { 20 };
        loop {
            // A record head: two varints (plus an optional ordinal varint).
            let n = (end - at).min(head_max);
            let a = self.data_at + at;
            let b = fetch(&mut self.src, &mut self.stats, &[(a, a + n)])?
                .pop()
                .unwrap();
            let mut r = ByteReader::new(&b);
            let id = r.varint()?;
            let len = r.varint()?;
            let rec_end = at
                .checked_add(r.pos as u64)
                .and_then(|n| n.checked_add(len))
                .filter(|&e| e <= end)
                .ok_or(Error::Corrupt("record past end of DATA"))?;
            let main = walk.head(at, rec_end, id, kind)?;
            let mut payload = at + r.pos as u64;
            if main && let Some(want) = expected_ordinal {
                let ord_start = r.pos;
                let ord = r.varint()?;
                let ord_len = (r.pos - ord_start) as u64;
                if ord_len > len || ord != want as u64 {
                    return Err(Error::Corrupt("unit record ordinal"));
                }
                payload += ord_len;
            }
            at = rec_end;
            if main {
                return Ok((payload, at));
            }
        }
    }

    /// With exact blocks: every unit's exact record holds exactly the
    /// nested stripes the exact head places in it (as the full decoders
    /// check when they split the records), so a stripe read through the
    /// head's offsets is the one a full decode reads.
    fn check_exact_records(&self) -> Result<(), Error> {
        let Head::Lossy(p) = &self.head else {
            return Ok(());
        };
        let Some(e) = p.exact() else {
            return Ok(());
        };
        if self.exact_at.len() != e.ranges.len() {
            return Err(Error::Corrupt("exact record"));
        }
        for (u, &(_, len)) in self.exact_at.iter().enumerate() {
            let want: u64 = e.carried_by(u).map(|t| e.stripes_at[t].len as u64).sum();
            if len != want {
                return Err(Error::Corrupt("exact record"));
            }
        }
        Ok(())
    }
}

#[cfg(feature = "v3")]
struct LegacyReads<'a, R> {
    source: &'a mut R,
    stats: &'a mut ReadStats,
}

#[cfg(feature = "v3")]
impl<'a, R> LegacyReads<'a, R> {
    fn new(source: &'a mut R, stats: &'a mut ReadStats) -> Self {
        Self { source, stats }
    }
}

#[cfg(feature = "v3")]
impl<R: ReadAt> ReadAt for LegacyReads<'_, R> {
    fn len(&mut self) -> Result<u64, Error> {
        self.source.len()
    }

    fn read_exact_at(&mut self, at: u64, out: &mut [u8]) -> Result<(), Error> {
        self.source.read_exact_at(at, out)?;
        self.stats.reads += 1;
        self.stats.bytes += out.len() as u64;
        self.stats.batches += 1;
        Ok(())
    }
}

/// True for the errors a parse of a cut-short prefix gives (more bytes
/// may fix them).
#[cfg(feature = "v3")]
fn truncated(e: &Error) -> bool {
    matches!(e, Error::Corrupt(m) if m.starts_with("truncated"))
}

/// The tag, payload length (`None`: open) and payload offset of the chunk
/// header at the start of `b` (read at file offset `at`).
#[cfg(feature = "v3")]
fn chunk_head(b: &[u8], at: u64) -> Result<([u8; 4], Option<u64>, u64), Error> {
    let mut r = ByteReader::new(b);
    let (tag, len) = chunks::read_head(&mut r)?;
    Ok((tag, len, at + r.pos as u64))
}

/// Bytes read for a chunk header: tag, a length field and, for the chunks
/// whose rules read their first payload bytes (`aNIM`, `mCMP`), those.
#[cfg(feature = "v3")]
const HEAD: u64 = 14;

/// What opening learns from the chunks it reads: the chunk rules a header
/// and the first payload bytes show, applied to each in file order (SPEC
/// 1.1: a partial decoder does not fetch metadata payloads, so an `mCMP`
/// record's syntax is not checked), the file ranges it used (for a file
/// checksum) and the `cSUM` chunk.
#[cfg(feature = "v3")]
struct Opened {
    rules: Rules,
    used: Used,
    /// File offset and whole length of the `cSUM` chunk.
    csum: Option<(u64, u64)>,
    unified_sequence: bool,
    pixel_expected: Option<crate::Digest>,
    pixel_table: Option<crate::pixelsum::LocatedTable>,
    pixel_status: crate::pixelsum::PixelChecksumStatus,
}

#[cfg(feature = "v3")]
impl Opened {
    /// Applies the rules to the chunk at `at` (tag, payload length, payload
    /// offset; `prefix`: its first [`chunks::prefix_len`] payload bytes).
    /// Returns its end, or `None` for `DATA`.
    fn chunk(
        &mut self,
        at: u64,
        (tag, len, payload_at): ([u8; 4], Option<u64>, u64),
        prefix: &[u8],
        file_len: u64,
    ) -> Result<Option<u64>, Error> {
        let kind = self.rules.head(tag, len)?;
        let end = match len {
            Some(n) => Some(
                payload_at
                    .checked_add(n)
                    .filter(|&e| e <= file_len)
                    .ok_or(Error::Corrupt("chunk past end of file"))?,
            ),
            None => None,
        };
        if kind == Kind::Data {
            self.used.push(at, payload_at)?;
            return Ok(None);
        }
        // The rules refuse every other open chunk.
        let end = end.unwrap_or(payload_at);
        if kind.has_body_rules(&tag) {
            self.rules.identity(tag, prefix)?;
            self.used
                .push(payload_at, payload_at + prefix.len() as u64)?;
        }
        if tag == chunks::ANIM {
            self.unified_sequence = prefix.first() == Some(&4);
        }
        if tag == crate::pixelsum::PSUM {
            self.pixel_status = crate::pixelsum::PixelChecksumStatus::Unsupported;
            if let Some((count, offset, algo)) = crate::pixelsum::prefix(prefix, len.unwrap())? {
                let d = prefix
                    .get(offset..offset + crate::pixelsum::width_of(algo))
                    .ok_or(Error::Corrupt("short pSUM"))?;
                self.pixel_expected =
                    Some(crate::Digest::from_bytes(algo, d).ok_or(Error::Corrupt("short pSUM"))?);
                self.pixel_table = Some(crate::pixelsum::LocatedTable {
                    first: payload_at + offset as u64,
                    count,
                    algo,
                });
                self.pixel_status = crate::pixelsum::PixelChecksumStatus::Pending;
            }
            self.used
                .push(payload_at, payload_at + prefix.len() as u64)?;
        }
        match kind {
            Kind::Checksum => self.csum = Some((at, end - at)),
            Kind::Ignored => {}
            _ => self.used.push(at, payload_at)?,
        }
        Ok(Some(end))
    }
}

/// Walks the chunks after the header up to `DATA`, applying the chunk
/// rules: its payload offset and length (`None`: open).
#[cfg(feature = "v3")]
fn walk_chunks<R: ReadAt>(
    src: &mut R,
    stats: &mut ReadStats,
    win: &mut Window,
    file_len: u64,
    header_end: u64,
    opened: &mut Opened,
) -> Result<(u64, Option<u64>), Error> {
    let mut at = header_end;
    loop {
        if at >= file_len {
            opened.rules.finish()?;
            return Err(Error::Corrupt("no DATA chunk"));
        }
        let e = (at + HEAD).min(file_len);
        win.ensure(src, stats, file_len, at, e)?;
        let head = chunk_head(win.slice(at, e), at)?;
        let (tag, len, payload_at) = head;
        let need = (payload_at + chunks::prefix_len(&tag, len)).min(file_len);
        win.ensure(src, stats, file_len, at, need.max(e))?;
        match opened.chunk(at, head, win.slice(payload_at.min(need), need), file_len)? {
            Some(end) => at = end,
            None => return Ok((payload_at, len)),
        }
    }
}

/// The chunks from `at` (the end of `DATA`) to the `cSUM` chunk, read one
/// header at a time under the chunk rules; the table's position and
/// length, or `None` when the file has none.
#[cfg(feature = "v3")]
fn find_after<R: ReadAt>(
    src: &mut R,
    stats: &mut ReadStats,
    file_len: u64,
    mut at: u64,
    opened: &mut Opened,
) -> Result<Option<(u64, u64)>, Error> {
    while at < file_len {
        let e = at + (file_len - at).min(HEAD);
        let b = fetch(src, stats, &[(at, e)])?.pop().unwrap();
        let head = chunk_head(&b, at)?;
        let (tag, len, payload_at) = head;
        let need = (payload_at + chunks::prefix_len(&tag, len)).min(file_len);
        let more;
        let prefix = if need <= e {
            &b[(payload_at.min(need) - at) as usize..(need - at) as usize]
        } else {
            more = fetch(src, stats, &[(payload_at, need)])?.pop().unwrap();
            &more[..]
        };
        let csum = opened.csum;
        // A second DATA is a repeated chunk (the rules saw the first).
        let end = opened
            .chunk(at, head, prefix, file_len)?
            .unwrap_or(payload_at);
        if opened.csum != csum {
            return Ok(opened.csum);
        }
        at = end;
    }
    Ok(None)
}

/// What a checked DATA locator gives: the `DATA` payload's offset and length,
/// and its parsed head with its unit table hint.
#[cfg(feature = "v3")]
type Located = (u64, Option<u64>, Prefix);

/// Uses a DATA locator: every chunk it names and the `DATA` prefix are read
/// in one batch (minus what the first window already holds) and checked
/// against the locator; `None` when anything disagrees (the caller then
/// walks the chunks, as without a locator). Once the locator matches, the
/// chunk rules apply to the chunks it names, in order, as to a walk.
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "v3")]
fn locate<R: ReadAt>(
    src: &mut R,
    stats: &mut ReadStats,
    win: &Window,
    file_len: u64,
    header_end: u64,
    loc: &Locator,
    header: &Header,
    d: Features,
    max_models: u64,
    opened: &mut Opened,
) -> Result<Option<Located>, Error> {
    // Chunk starts; every chunk takes at least 5 bytes.
    if loc.chunks.len() as u64 > file_len / 5 {
        return Ok(None);
    }
    let mut starts = Vec::with_capacity(loc.chunks.len());
    let mut at = header_end;
    for &l in &loc.chunks {
        starts.push(at);
        match at.checked_add(l).filter(|&e| l >= 5 && e <= file_len) {
            Some(e) => at = e,
            None => return Ok(None),
        }
    }
    let data_tag = at;
    let prefix_end = match data_tag.checked_add(loc.prefix) {
        Some(e) if e <= file_len && loc.prefix <= MAX_PREFIX && loc.prefix > 4 => e,
        _ => return Ok(None),
    };
    // One batch: the chunk headers outside the window, and the start of the
    // DATA prefix (at most a window: the rest is read once the head and the
    // table length name the same end, so a wrong prefix costs no more than
    // walking would).
    let first_end = if win.holds(data_tag, prefix_end) {
        prefix_end
    } else {
        prefix_end.min(data_tag + WINDOW)
    };
    // Ranges outside the window, in file order; headers that touch or
    // overlap share one range (many small chunks are one request), and each
    // chunk keeps the index of its range.
    let mut ranges: Vec<(u64, u64)> = Vec::new();
    let mut range_of = |a: u64, b: u64| -> Option<usize> {
        if win.holds(a, b) {
            return None;
        }
        match ranges.last_mut() {
            Some(r) if a <= r.1 => r.1 = r.1.max(b),
            _ => ranges.push((a, b)),
        }
        Some(ranges.len() - 1)
    };
    let mut heads = Vec::new();
    heads
        .try_reserve_exact(starts.len())
        .map_err(|_| Error::TooLarge)?;
    for &s in &starts {
        let e = (s + HEAD).min(file_len);
        heads.push((s, e, range_of(s, e)));
    }
    let data_range = range_of(data_tag, first_end);
    let got = if ranges.is_empty() {
        Vec::new()
    } else {
        fetch(src, stats, &ranges)?
    };
    let bytes_of = |a: u64, b: u64, i: Option<usize>| -> &[u8] {
        match i {
            None => win.slice(a, b),
            Some(i) => &got[i][(a - ranges[i].0) as usize..(b - ranges[i].0) as usize],
        }
    };
    // The chunk headers must be the chunks the locator names, each with its
    // first payload bytes in what was read.
    let mut parsed = Vec::new();
    parsed
        .try_reserve_exact(heads.len())
        .map_err(|_| Error::TooLarge)?;
    for (&(s, e, i), &l) in heads.iter().zip(&loc.chunks) {
        let Ok(head) = chunk_head(bytes_of(s, e, i), s) else {
            return Ok(None);
        };
        let (tag, len, payload_at) = head;
        let need = payload_at + chunks::prefix_len(&tag, len);
        if tag == chunks::DATA
            || len.and_then(|n| payload_at.checked_add(n)) != Some(s + l)
            || need > e
        {
            return Ok(None);
        }
        parsed.push((
            s,
            head,
            (payload_at - s) as usize..(need - s) as usize,
            i,
            e,
        ));
    }
    let first = bytes_of(data_tag, first_end, data_range);
    let Ok(data_head) = chunk_head(first, data_tag) else {
        return Ok(None);
    };
    let (tag, n, payload_at) = data_head;
    if tag != chunks::DATA || payload_at > first_end {
        return Ok(None);
    }
    let data_len = n;
    if data_len.is_some_and(|l| payload_at.checked_add(l).is_none_or(|e| e > file_len)) {
        return Ok(None);
    }
    let room = data_len.unwrap_or(file_len - payload_at);
    let skip = (payload_at - data_tag) as usize;
    let mut pre = Vec::new();
    pre.try_reserve_exact(first.len())
        .map_err(|_| Error::TooLarge)?;
    pre.extend_from_slice(first);
    if first_end < prefix_end {
        // The head reaches past the first window (a long unit table hint):
        // what was read must parse as a head cut short, before the rest is
        // read.
        match parse_head(header, d, &first[skip..], max_models) {
            Err(e) if truncated(&e) => {}
            _ => return Ok(None),
        }
        let rest = fetch(src, stats, &[(first_end, prefix_end)])?;
        pre.try_reserve_exact(rest[0].len())
            .map_err(|_| Error::TooLarge)?;
        pre.extend_from_slice(&rest[0]);
    }
    pre.drain(..skip);
    match parse_prefix(header, d, &pre, room, max_models) {
        Ok(p) if p.head_len == pre.len() && (p.lens.is_some() || p.reserve.is_some()) => {
            // The locator matches the file: its chunks follow the rules.
            for (s, head, prefix, i, e) in parsed {
                opened.chunk(s, head, &bytes_of(s, e, i)[prefix], file_len)?;
            }
            opened.chunk(data_tag, data_head, &[], file_len)?;
            Ok(Some((payload_at, data_len, p)))
        }
        _ => Ok(None),
    }
}

/// A buffered window of the file.
#[derive(Default)]
struct Window {
    at: u64,
    buf: Vec<u8>,
}

impl Window {
    fn holds(&self, a: u64, b: u64) -> bool {
        a >= self.at && b <= self.at + self.buf.len() as u64
    }
    /// Makes `[a, b)` available, reading a new window of at least
    /// [`WINDOW`] bytes (one batch) when it is not.
    fn ensure<R: ReadAt>(
        &mut self,
        src: &mut R,
        stats: &mut ReadStats,
        file_len: u64,
        a: u64,
        b: u64,
    ) -> Result<(), Error> {
        if b > file_len || a > b {
            return Err(Error::Corrupt("truncated"));
        }
        if self.holds(a, b) {
            return Ok(());
        }
        let e = a + (b - a).max(WINDOW).min(file_len - a);
        self.buf = fetch(src, stats, &[(a, e)])?.pop().unwrap();
        self.at = a;
        Ok(())
    }
    fn slice(&self, a: u64, b: u64) -> &[u8] {
        &self.buf[(a - self.at) as usize..(b - self.at) as usize]
    }
}

/// Bytes of the buffer a checksum piece is hashed through when the pieces
/// do not fit a region's budget with its records.
#[cfg(feature = "v3")]
const STREAM_BUFFER: u64 = 1 << 20;

/// Bytes in `ranges`, saturating.
#[cfg(feature = "v3")]
fn total(ranges: &[(u64, u64)]) -> u64 {
    ranges.iter().fold(0u64, |n, r| n.saturating_add(r.1 - r.0))
}

/// `ranges` sorted, those that touch or overlap merged.
#[cfg(feature = "v3")]
fn merged(mut ranges: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    ranges.sort_unstable();
    let mut out: Vec<(u64, u64)> = Vec::with_capacity(ranges.len());
    for (a, b) in ranges {
        match out.last_mut() {
            Some(r) if a <= r.1 => r.1 = r.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// A zeroed buffer of `n` bytes, or `TooLarge`.
fn buffer(n: usize) -> Result<Vec<u8>, Error> {
    let mut v = Vec::new();
    v.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
    v.resize(n, 0);
    Ok(v)
}

/// The table's digest of checksum piece `i`, read from `src` through `buf` (one read
/// per buffer's worth).
#[cfg(feature = "v3")]
fn digest_streamed<R: ReadAt>(
    src: &mut R,
    stats: &mut ReadStats,
    pieces: &crate::checksum::Fetched,
    i: u64,
    buf: &mut [u8],
) -> Result<crate::Digest, Error> {
    let mut h = crate::Hasher::new(pieces.algo());
    for (mut pos, end) in pieces.segments(i).into_iter().filter(|r| r.0 < r.1) {
        while pos < end {
            let n = (end - pos).min(buf.len() as u64) as usize;
            src.read_many_at(&mut [(pos, &mut buf[..n])])?;
            stats.reads += 1;
            stats.bytes += n as u64;
            stats.batches += 1;
            h.update(&buf[..n]);
            pos += n as u64;
        }
    }
    Ok(h.finish())
}

/// Reads `ranges` (file offsets) in one batch.
fn fetch<R: ReadAt>(
    src: &mut R,
    stats: &mut ReadStats,
    ranges: &[(u64, u64)],
) -> Result<Vec<Vec<u8>>, Error> {
    let mut bufs = Vec::new();
    bufs.try_reserve_exact(ranges.len())
        .map_err(|_| Error::TooLarge)?;
    let mut total = 0u64;
    for &(a, b) in ranges {
        let n = usize::try_from(b - a).map_err(|_| Error::TooLarge)?;
        let mut v = Vec::new();
        v.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
        v.resize(n, 0);
        bufs.push(v);
        total += n as u64;
    }
    let mut reqs: Vec<(u64, &mut [u8])> = ranges
        .iter()
        .zip(bufs.iter_mut())
        .map(|(r, b)| (r.0, &mut b[..]))
        .collect();
    src.read_many_at(&mut reqs)?;
    stats.reads += ranges.len() as u64;
    stats.bytes += total;
    stats.batches += 1;
    Ok(bufs)
}

/// A trailing unit index found at open: its entries, the `DATA` payload
/// length it is bound to, and the file offset of its chunk.
#[cfg(feature = "v3")]
struct Found {
    entries: Vec<u64>,
    data_len: u64,
    at: u64,
}

/// The trailing unit index of a check-tail payload (SPEC 2.7), found from
/// the end of the file: the last [`WINDOW`] bytes (one batch, none when
/// `win` holds them), then, for a longer index, the rest of its chunk in one
/// more batch. `None` when the end of the file does not name a `uIDX` chunk
/// (the caller walks the records). Once it does, the index must be this
/// payload's (`data_len`: its length when bounded), with as many entries as
/// the geometry names (its length bounded by the count before the rest is
/// fetched), summing to the payload after its head (`head_len`); the caller
/// checks every span against the records.
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "v3")]
fn find_index<R: ReadAt>(
    src: &mut R,
    stats: &mut ReadStats,
    win: &Window,
    file_len: u64,
    data_at: u64,
    data_len: Option<u64>,
    head_len: u64,
    geo: &Geo,
) -> Result<Option<Found>, Error> {
    use records::{INDEX_FOOTER, UIDX};
    let footer = INDEX_FOOTER as u64;
    // The smallest chunk: tag, a one-byte length, the footer.
    let after_head = data_at + head_len;
    if file_len < after_head + 5 + footer {
        return Ok(None);
    }
    let tail_at = file_len - (file_len - after_head).min(WINDOW);
    let fetched;
    let tail: &[u8] = if win.holds(tail_at, file_len) {
        win.slice(tail_at, file_len)
    } else {
        fetched = fetch(src, stats, &[(tail_at, file_len)])?;
        &fetched[0][..]
    };
    let f = &tail[tail.len() - INDEX_FOOTER..];
    let le = |b: &[u8]| u64::from_le_bytes(b.try_into().unwrap());
    let (index_data_len, chunk_len) = (le(&f[..8]), le(&f[8..16]));
    if f[16..] != UIDX || chunk_len < 5 + footer || chunk_len > file_len - after_head {
        return Ok(None);
    }
    let at = file_len - chunk_len;
    let records = geo.map.count(geo.stripes)?;
    // The longest chunk a valid index of this count makes: tag, a length
    // field, ten bytes per entry, the footer.
    let longest = 14 + (records as u64).saturating_mul(10) + footer;
    // The chunk's bytes before the tail window: the whole rest when a valid
    // index could be that long, else only its header (it is then found or
    // not, and refused by its length before more is fetched).
    let joined;
    let bytes: &[u8] = if at < tail_at {
        let end = if chunk_len <= longest {
            tail_at
        } else {
            (at + 14).min(tail_at)
        };
        let mut v = fetch(src, stats, &[(at, end)])?.pop().unwrap();
        if end == tail_at {
            v.try_reserve_exact(tail.len())
                .map_err(|_| Error::TooLarge)?;
            v.extend_from_slice(tail);
        }
        joined = v;
        &joined
    } else {
        &tail[(at - tail_at) as usize..]
    };
    let mut r = ByteReader::new(&bytes[..bytes.len().min(14)]);
    let Ok((tag, Some(n))) = chunks::read_head(&mut r) else {
        return Ok(None);
    };
    // A bounded DATA ends before any chunk after it.
    if tag != UIDX
        || records::chunk_len_of(n) != chunk_len
        || data_len.is_some_and(|l| at < data_at + l)
    {
        return Ok(None);
    }
    // Found: bound to this payload, then bounded by the geometry's count.
    let bound = match data_len {
        Some(l) => index_data_len == l,
        None => index_data_len >= head_len && index_data_len <= at - data_at,
    };
    if !bound {
        return Err(Error::Corrupt("unit index for another DATA"));
    }
    let entries_len = n - footer;
    if entries_len < records as u64 || entries_len > (records as u64).saturating_mul(10) {
        return Err(Error::Corrupt("unit table length"));
    }
    // The whole chunk is held: a longer one was refused by its length.
    let payload = &bytes[r.pos..];
    let index = records::Index {
        entries: &payload[..payload.len() - INDEX_FOOTER],
        data_len: index_data_len,
    };
    let entries = records::index_entries(index, index_data_len, head_len, records)?;
    Ok(Some(Found {
        entries,
        data_len: index_data_len,
        at,
    }))
}

/// Record heads fetched per batch when the hint is checked at open.
#[cfg(feature = "v3")]
const HINT_BATCH: usize = 1 << 16;
/// Bytes that hold any record head (id and length varints).
#[cfg(feature = "v3")]
const RECORD_HEAD: u64 = 20;

/// Checks the unit table hint against the payload's records (SPEC 2.7), as
/// the walking decoders do: every span (`starts`: offsets in the payload at
/// `data_at`, one more than the spans) must start with the record head, or
/// a side record and then the head, of the main record the geometry expects
/// there, and the record must end where the span ends. Heads inside `win`
/// (the bytes opening already holds: small files whole) are read from it;
/// the others come in batches of [`HINT_BATCH`] ranges of [`RECORD_HEAD`]
/// bytes, and the main records behind side records of a batch in one more
/// batch. The bytes requested are charged to `max` (the reader's byte
/// limit) before each batch: `TooLarge` when they would pass it. With the
/// sum check at open, a hint that passes names exactly the records a full
/// decode walks, so a region and a full decode read the same records or
/// both fail. Returns every exact record's payload (start, length), in unit
/// order.
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "v3")]
fn verify_hint<R: ReadAt>(
    src: &mut R,
    stats: &mut ReadStats,
    win: &Window,
    data_at: u64,
    version: u8,
    starts: &[u64],
    hint: Vec<u64>,
    geo: &Geo,
    ordinals: bool,
    max: u64,
) -> Result<Vec<(u64, u64)>, Error> {
    let spans = starts.len() - 1;
    let end = starts[spans];
    let mut walk = records::Walk::new(version, Some(hint));
    // Every exact record's payload (start, length), in unit order.
    let mut exact_at = Vec::new();
    let mut kinds = (0..geo.units()?).flat_map(|u| {
        let (k, n) = geo.map.kinds_at(u / geo.groups, u % geo.groups);
        k.into_iter().take(n)
    });
    // One record head at payload offset `at`: (id, payload start, end of
    // the record, header bytes consumed).
    let head = |b: &[u8], at: u64| -> Result<(u64, u64, u64, usize), Error> {
        let mut r = ByteReader::new(b);
        let id = r.varint()?;
        let n = r.varint()?;
        let rec_end = (at + r.pos as u64)
            .checked_add(n)
            .filter(|&e| e <= end)
            .ok_or(Error::Corrupt("record past end of DATA"))?;
        Ok((id, at + r.pos as u64, rec_end, r.pos))
    };
    // The file range of the record head at payload offset `a`.
    let head_len = if ordinals {
        RECORD_HEAD + 10
    } else {
        RECORD_HEAD
    };
    let range = |a: u64| (data_at + a, data_at + (a + head_len).min(end));
    // Bytes requested so far, charged to `max` before each batch.
    let mut charged = 0u64;
    let mut batch = |src: &mut R, stats: &mut ReadStats, ranges: &[(u64, u64)]| {
        charged = charged.saturating_add(total(ranges));
        if charged > max {
            return Err(Error::TooLarge);
        }
        if ranges.is_empty() {
            return Ok(Vec::new());
        }
        fetch(src, stats, ranges)
    };
    let mut i = 0;
    while i < spans {
        let j = (i + HINT_BATCH).min(spans);
        let mut kind = Vec::new();
        kind.try_reserve_exact(j - i).map_err(|_| Error::TooLarge)?;
        for _ in i..j {
            kind.push(kinds.next().ok_or(Error::Corrupt("unit table length"))?);
        }
        // The first record head of every span.
        let mut ranges = Vec::new();
        for x in i..j {
            let (a, b) = range(starts[x]);
            if !win.holds(a, b) {
                ranges.try_reserve(1).map_err(|_| Error::TooLarge)?;
                ranges.push((a, b));
            }
        }
        let bufs = batch(src, stats, &ranges)?;
        let mut fetched = bufs.iter();
        // (parsed head result, raw bytes for ordinal reading)
        let mut firsts: Vec<(Result<(u64, u64, u64, usize), Error>, Vec<u8>)> = Vec::new();
        firsts
            .try_reserve_exact(j - i)
            .map_err(|_| Error::TooLarge)?;
        for x in i..j {
            let (a, b) = range(starts[x]);
            let bytes: &[u8] = if win.holds(a, b) {
                win.slice(a, b)
            } else {
                &fetched.next().unwrap()[..]
            };
            firsts.push((head(bytes, starts[x]), bytes.to_vec()));
        }
        // The main record heads behind side records (id `kind - 1`), up
        // to the first span whose head does not parse: one more batch.
        let mut behind = Vec::new();
        for (k, (f, _)) in firsts.iter().enumerate() {
            let Ok((id, _, rec_end, _)) = *f else { break };
            if id == kind[k] - 1 {
                let (a, b) = range(rec_end);
                if !win.holds(a, b) {
                    behind.try_reserve(1).map_err(|_| Error::TooLarge)?;
                    behind.push((a, b));
                }
            }
        }
        let more = batch(src, stats, &behind)?;
        let mut more = more.iter();
        for (k, (f, first_bytes)) in firsts.into_iter().enumerate() {
            let (id, mut payload, rec_end, mut hlen) = f?;
            let at = starts[i + k];
            let mut main_bytes = first_bytes;
            let mut main_end = rec_end;
            if !walk.head(at, rec_end, id, kind[k])? {
                // A side record: the main record follows it.
                let (a, b) = range(rec_end);
                let bytes: &[u8] = if win.holds(a, b) {
                    win.slice(a, b)
                } else {
                    &more.next().unwrap()[..]
                };
                let (id, p, e, hl) = head(bytes, rec_end)?;
                if !walk.head(rec_end, e, id, kind[k])? {
                    return Err(Error::Corrupt("unit records out of order"));
                }
                (payload, main_end, hlen) = (p, e, hl);
                main_bytes = bytes.to_vec();
            }
            if ordinals {
                let mut pr = ByteReader::new(&main_bytes[hlen..]);
                let ord = pr.varint()?;
                let ord_len = pr.pos as u64;
                if payload + ord_len > main_end || ord != (i + k) as u64 {
                    return Err(Error::Corrupt("unit record ordinal"));
                }
                payload += ord_len;
            }
            if kind[k] == records::R_EXACT {
                exact_at.try_reserve(1).map_err(|_| Error::TooLarge)?;
                exact_at.push((payload, main_end - payload));
            }
        }
        i = j;
    }
    walk.finish()?;
    Ok(exact_at)
}

/// The `data_len` a trailing unit index's footer claims (the last 20 bytes
/// of the file read again: an error path), when the file is that long.
#[cfg(feature = "v3")]
fn footer_data_len<R: ReadAt>(src: &mut R, stats: &mut ReadStats, file_len: u64) -> Option<u64> {
    let footer = records::INDEX_FOOTER as u64;
    let at = file_len.checked_sub(footer)?;
    let b = fetch(src, stats, &[(at, file_len)]).ok()?.pop()?;
    Some(u64::from_le_bytes(b[..8].try_into().unwrap()))
}

/// An error met checking a trailing unit index (SPEC 2.7), explained by a
/// mismatching piece (SPEC 1.4) as the other routes explain it. The file
/// checksum's table may sit after `DATA`, before the index (a streaming
/// writer's layout): when opening has not met it, it is found by reading
/// the chunk headers from `DATA`'s end (`data_end`), and the pieces under
/// every byte to the end of the file are checked, within the byte budget
/// `max` ([`explain_open`]). Error path only.
#[cfg(feature = "v3")]
fn explain_index<R: ReadAt>(
    src: &mut R,
    stats: &mut ReadStats,
    file_len: u64,
    (announced, verify, max): (Option<crate::checksum::Announced>, bool, u64),
    opened: &mut Opened,
    data_end: Option<u64>,
    e: Error,
) -> Error {
    let mut csum = opened.csum;
    if verify
        && announced.is_some()
        && csum.is_none()
        && let Some(end) = data_end.filter(|&end| end <= file_len)
    {
        csum = find_after(src, stats, file_len, end, opened).ok().flatten();
    }
    explain_open(
        src, stats, file_len, announced, verify, csum, file_len, e, max,
    )
}

/// A parse or chunk-rule error met while opening, explained by a mismatching
/// piece (SPEC 1.4) when verifying and the `cSUM` chunk (`csum`: offset and
/// length) came before the failing bytes: the table and the pieces under the
/// covered bytes before `end` are fetched and checked, and a mismatch is
/// reported in place of `e`. Error path only. Within the byte budget `max`:
/// the pieces come in one batch when they fit it with the table, else each
/// is hashed through a buffer of at most 1 MiB; a table larger than the
/// budget leaves `e` unexplained.
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "v3")]
fn explain_open<R: ReadAt>(
    src: &mut R,
    stats: &mut ReadStats,
    file_len: u64,
    announced: Option<crate::checksum::Announced>,
    verify: bool,
    csum: Option<(u64, u64)>,
    end: u64,
    e: Error,
    max: u64,
) -> Error {
    let (true, Some(a), Some((at, len))) = (verify, announced, csum) else {
        return e;
    };
    if len >= max {
        return e;
    }
    let src = core::cell::RefCell::new((src, stats));
    let fetch_all = |runs: &[(u64, u64)]| {
        let (s, st) = &mut *src.borrow_mut();
        fetch(&mut **s, st, runs)
    };
    crate::checksum::explain_fetched(
        e,
        at,
        len,
        a.log2(),
        a.algorithm_id(),
        file_len,
        end,
        || Ok(fetch_all(&[(at, at + len)])?.pop().unwrap()),
        |pieces, pending| {
            let mut runs = Vec::new();
            for &i in pending {
                runs.extend(pieces.segments(i).into_iter().filter(|r| r.0 < r.1));
            }
            if total(&runs).saturating_add(len) > max {
                let mut buf = buffer((max - len).min(STREAM_BUFFER) as usize)?;
                let (s, st) = &mut *src.borrow_mut();
                return pending
                    .iter()
                    .map(|&i| digest_streamed(&mut **s, st, pieces, i, &mut buf))
                    .collect();
            }
            // One batch for every piece's covered runs.
            let bufs = fetch_all(&runs)?;
            let mut parts = bufs.iter();
            Ok(pending
                .iter()
                .map(|&i| {
                    let n = pieces.segments(i).iter().filter(|r| r.0 < r.1).count();
                    crate::checksum::digest_of(
                        pieces.algo(),
                        parts.by_ref().take(n).map(|b| &b[..]),
                    )
                })
                .collect())
        },
    )
}

/// Decodes `rect` of the primary image of the file `data` into `out` (rows
/// of `stride` bytes); see [`RegionReader`].
pub fn decode_region(
    data: &[u8],
    rect: Rect,
    out: &mut [u8],
    stride: usize,
) -> Result<ImageInfo, Error> {
    let mut r = RegionReader::open(crate::planes::SliceReader::new(data))?;
    r.decode_region(rect, out, stride)?;
    Ok(r.header().info.clone())
}

/// [`decode_region`] on up to `threads` threads.
pub fn decode_region_threads(
    data: &[u8],
    rect: Rect,
    out: &mut [u8],
    stride: usize,
    threads: usize,
) -> Result<ImageInfo, Error> {
    let mut r = RegionReader::open(crate::planes::SliceReader::new(data))?;
    r.decode_region_threads(rect, out, stride, threads)?;
    Ok(r.header().info.clone())
}
