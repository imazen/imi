//! Sparse numeric access: one retained directory and one read per fetched tile.

use alloc::{sync::Arc, vec::Vec};
use core::ops::Range;

use super::{
    Directory, DirectoryPlacement, Entry, EntryKind, LocatedEntry, NumericEvent,
    NumericGroupDecoder, NumericWork, TileExtent, TileLimits,
};
use crate::decode_resources::{DecodeSession, V4Retained, V4Working};
use crate::parse::{chunks, limits::Admission};
use crate::planes::ReadAt;
use crate::{ByteReader, Error, Header};

/// Capacity follows the owner; adjacent scalar framing reads share one span.
#[derive(Default)]
struct UsedRanges {
    ranges: Vec<(u64, u64)>,
    memory: V4Retained,
}
impl UsedRanges {
    fn push<C: Fn() -> bool + Sync>(
        &mut self,
        a: u64,
        b: u64,
        session: &DecodeSession<C>,
    ) -> Result<(), Error> {
        if a == b {
            return Ok(());
        }
        if let Some(last) = self.ranges.last_mut()
            && a <= last.1
            && b >= last.0
        {
            last.0 = last.0.min(a);
            last.1 = last.1.max(b);
            return Ok(());
        }
        if self.ranges.len() == self.ranges.capacity() {
            let capacity = self
                .ranges
                .capacity()
                .max(4)
                .checked_mul(2)
                .ok_or(Error::TooLarge)?;
            let memory = session.v4_working(capacity as u128 * size_of::<(u64, u64)>() as u128)?;
            self.ranges
                .try_reserve_exact(capacity - self.ranges.len())
                .map_err(|_| Error::TooLarge)?;
            self.memory = memory.retain()?;
        }
        self.ranges.push((a, b));
        Ok(())
    }
}

struct FileCheck {
    pieces: crate::checksum::Fetched,
    opened: UsedRanges,
    _memory: V4Retained,
}

// Covered pieces can extend outside the selected tile. Reuse its already
// fetched bytes and stream only the remaining piece segments through scratch.
pub(crate) fn verify_interval<R: ReadAt, C: Fn() -> bool + Sync>(
    source: &mut R,
    pieces: &mut crate::checksum::Fetched,
    wanted: (u64, u64),
    tile: (u64, &[u8]),
    session: &DecodeSession<C>,
) -> Result<(), Error> {
    let (a, b) = (pieces.covered(wanted.0), pieces.covered(wanted.1));
    if a >= b {
        return Ok(());
    }
    let count = ((b - 1) >> pieces.log2()) - (a >> pieces.log2()) + 1;
    // pending() grows geometrically, with a small minimum Vec capacity.
    let _pending_memory = session.v4_working(count.max(4) as u128 * 16)?;
    session.adapter_work(count as u128 + 1)?;
    let pending = pieces.pending(&[wanted], true)?;
    let mut buffer = Vec::new();
    let mut buffer_memory = None;
    let tile_end = tile
        .0
        .checked_add(tile.1.len() as u64)
        .ok_or(Error::TooLarge)?;
    for i in pending {
        let mut hash = crate::Hasher::new(pieces.algo());
        for (mut at, end) in pieces.segments(i) {
            while at < end {
                session.adapter_work(0)?;
                if at >= tile.0 && at < tile_end {
                    let n = (end.min(tile_end) - at) as usize;
                    let start = (at - tile.0) as usize;
                    session.adapter_work(n as u128 + 1)?;
                    hash.update(&tile.1[start..start + n]);
                    at += n as u64;
                } else {
                    let next = if at < tile.0 { end.min(tile.0) } else { end };
                    let n = (next - at).min(64 * 1024) as usize;
                    if buffer.len() < n {
                        let next_memory = session.v4_working(n as u128)?;
                        buffer
                            .try_reserve_exact(n - buffer.len())
                            .map_err(|_| Error::TooLarge)?;
                        buffer.resize(n, 0);
                        buffer_memory = Some(next_memory);
                    }
                    session.v4_input(n)?;
                    session.adapter_work(n as u128 + 1)?;
                    source.read_exact_at(at, &mut buffer[..n])?;
                    hash.update(&buffer[..n]);
                    at += n as u64;
                }
            }
        }
        pieces.check(i, &hash.finish())?;
    }
    drop(buffer_memory);
    Ok(())
}

struct Cursor<'r, 's, R, C> {
    source: &'r mut R,
    length: u64,
    at: u64,
    session: &'s DecodeSession<C>,
    used: Option<UsedRanges>,
}
impl<R: ReadAt, C: Fn() -> bool + Sync> Cursor<'_, '_, R, C> {
    fn read(&mut self, output: &mut [u8]) -> Result<(), Error> {
        if output.is_empty() {
            return self.session.adapter_work(0);
        }
        let end = self
            .at
            .checked_add(output.len() as u64)
            .ok_or(Error::TooLarge)?;
        if end > self.length {
            return Err(Error::Corrupt("V4 positional read past EOF"));
        }
        if let Some(used) = &mut self.used {
            used.push(self.at, end, self.session)?;
        }
        self.session.v4_input(output.len())?;
        self.session.adapter_work(output.len() as u128 + 1)?;
        self.source.read_exact_at(self.at, output)?;
        self.at = end;
        Ok(())
    }
    fn varint(&mut self) -> Result<u64, Error> {
        let mut bytes = [0; 10];
        for i in 0..10 {
            self.read(&mut bytes[i..i + 1])?;
            if bytes[i] & 128 == 0 {
                return ByteReader::new(&bytes[..=i]).varint();
            }
        }
        Err(Error::Corrupt("V4 positional varint"))
    }
    fn skip(&mut self, count: u64) -> Result<(), Error> {
        self.at = self
            .at
            .checked_add(count)
            .filter(|&end| end <= self.length)
            .ok_or(Error::Corrupt("V4 positional chunk bounds"))?;
        self.session.adapter_work(1)
    }
    // The caller retains the corresponding capacity reservation.
    fn vector(&mut self, count: usize, _memory: &V4Working<'_, C>) -> Result<Vec<u8>, Error> {
        if self
            .at
            .checked_add(count as u64)
            .is_none_or(|n| n > self.length)
        {
            return Err(Error::Corrupt("V4 positional allocation past EOF"));
        }
        self.session.v4_check_input(count)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(count)
            .map_err(|_| Error::TooLarge)?;
        bytes.resize(count, 0);
        self.read(&mut bytes)?;
        Ok(bytes)
    }
}

#[derive(Clone, Copy)]
struct TileSpan {
    extent: TileExtent,
    entry_start: usize,
    entry_end: usize,
    first_ignorable: u64,
}

/// An opened sparse numeric image. Its bounded retained index has a direct
/// geometry-indexed tile accelerator, not a model-ID-sized array. Opening reads
/// header/setup/index and required metadata identities, never pixel entropy.
/// `read_tile` makes one positional read for the complete contiguous tile and
/// checks it while emitting independent group jobs.
/// It does not claim whole-file, unread-tile or decoded-bitmap validation.
///
/// An index is required. Supported outer cSUM pieces under used metadata and
/// the selected tile are checked before jobs are emitted. Rounded pieces can
/// require other encoded bytes; the selected tile's fetched bytes are reused.
/// Object/tile XXH3-128 verification remains separate. Metadata bodies are not
/// returned or expanded. The source must represent one image's exact byte span.
pub struct NumericTileReader<'s, R, C> {
    owned: IndexedFile<R>,
    session: &'s DecodeSession<C>,
}

// Retain fixed parser/hash state inline, without a fallible heap allocation
// for every numeric opening. No raster or count-sized arrays live here.
#[allow(clippy::large_enum_variant)]
enum Root {
    Numeric(super::numeric_stream::NumericState),
    Lossy {
        setup: Arc<super::LossySetup>,
        context: super::SetupContext,
    },
}
impl Root {
    pub(super) fn layout(&self) -> super::TileLayout {
        match self {
            Self::Numeric(root) => root.layout(),
            Self::Lossy { setup, .. } => setup.layout(),
        }
    }
    fn integrity(&self) -> super::ObjectIntegrity {
        match self {
            Self::Numeric(root) => root.integrity(),
            Self::Lossy { setup, .. } => setup.integrity(),
        }
    }
}

/// Request-free retained range state. The public borrowed adapter and the
/// ordinary owned region reader use the same index, framing and group driver.
pub(crate) struct IndexedFile<R> {
    source: R,
    file_len: u64,
    header: Header,
    root: Root,
    models: Vec<lossy::ModelSpan>,
    next_models: Vec<(u64, u8, u64)>,
    cached_models: Vec<usize>,
    cache_memory: V4Retained,
    _model_memory: V4Retained,
    directory: Vec<u8>,
    tiles: Vec<Option<TileSpan>>,
    objects_at: u64,
    limits: TileLimits,
    _index_memory: V4Retained,
    _tile_memory: V4Retained,
    check: Option<FileCheck>,
    primary_end: u64,
    stored_index: bool,
    unified_sequence: bool,
    checksum_declared: bool,
    checksum_chunk: Option<(u64, u64)>,
    sequence_defaults: Option<Range<u64>>,
    pixel_expected: Option<crate::Digest>,
    pixel_table: Option<crate::pixelsum::LocatedTable>,
    pixel_status: crate::PixelChecksumStatus,
}

impl<'s, R: ReadAt, C: Fn() -> bool + Sync> NumericTileReader<'s, R, C> {
    pub fn open(
        source: R,
        limits: TileLimits,
        session: &'s DecodeSession<C>,
    ) -> Result<Self, Error> {
        let owned = IndexedFile::open(source, limits, session, true)?;
        if owned.header.coding != crate::Coding::Lossless {
            return Err(Error::Unsupported("V4 sparse numeric coding"));
        }
        Ok(Self { owned, session })
    }
    pub fn header(&self) -> &Header {
        self.owned.header()
    }
    pub fn into_inner(self) -> R {
        self.owned.into_inner()
    }
    pub fn tile_range(&self, id: u64) -> Result<Range<u64>, Error> {
        self.owned.tile_range(id, self.session)
    }
    pub fn read_tile<'a>(
        &mut self,
        id: u64,
        output: &'a mut Vec<u8>,
        visit: impl FnMut(NumericWork<'a>) -> Result<(), Error>,
    ) -> Result<TileExtent, Error> {
        self.owned.read_tile(id, output, self.session, visit)
    }
}

impl<R: ReadAt> IndexedFile<R> {
    pub(crate) fn open<C: Fn() -> bool + Sync>(
        mut source: R,
        limits: TileLimits,
        session: &DecodeSession<C>,
        numeric_tiles_only: bool,
    ) -> Result<Self, Error> {
        let length = source.len()?;
        let mut input = Cursor {
            source: &mut source,
            length,
            at: 0,
            session,
            used: None,
        };
        let mut signature = [0; 8];
        input.read(&mut signature)?;
        if signature != crate::parse::header::SIGNATURE {
            return Err(Error::NotImij);
        }
        let header_length = input.varint()?;
        if header_length > session.v4_header_limit() {
            return Err(Error::TooLarge);
        }
        let header_memory = session.v4_working(2 * (header_length as u128 + 256))?;
        let header_body = input.vector(
            usize::try_from(header_length).map_err(|_| Error::TooLarge)?,
            &header_memory,
        )?;
        let mut header_bytes = Vec::new();
        header_bytes
            .try_reserve_exact(header_body.len() + 18)
            .map_err(|_| Error::TooLarge)?;
        header_bytes.extend_from_slice(&signature);
        crate::put_varint(&mut header_bytes, header_length);
        header_bytes.extend_from_slice(&header_body);
        let parsed = crate::parse::header::read_header_v4(
            &mut ByteReader::new(&header_bytes),
            session.v4_header_limit(),
        )?;
        session.v4_setup_admitted(parsed.len)?;
        crate::parse::header::check_decodable(&parsed.header, parsed.declared)?;
        let verify = session.v4_verify_mode() != crate::checksum::Verify::Off;
        let announced = parsed.checksum.map(|c| c.announced);
        if let Some(a) = announced.filter(|_| verify) {
            a.log2()?;
            a.algorithm_id()?;
            let mut used = UsedRanges::default();
            used.push(0, input.at, session)?;
            input.used = Some(used);
        }
        let mut checksum = None;
        let mut sequence_defaults = None;
        let mut pixel_expected = None;
        let mut pixel_table = None;
        let mut pixel_count = None;
        let mut frames = 0u64;
        let mut unified_sequence = false;
        let mut pixel_status = crate::PixelChecksumStatus::Absent;
        let mut rules = chunks::Rules::new(parsed.mixed_pages(), verify)
            .with_sequence_items(parsed.declared.contains(crate::Feature::SequenceItems));
        let (data_at, data_length) = loop {
            let chunk_at = input.at;
            let mut tag = [0; 4];
            input.read(&mut tag)?;
            let len = input.varint()?.checked_sub(1);
            session.admission().chunk_start()?;
            let kind = rules.head(tag, len)?;
            if kind == chunks::Kind::Data {
                break (input.at, len);
            }
            if numeric_tiles_only && matches!(kind, chunks::Kind::Anim | chunks::Kind::Frame) {
                return Err(Error::Unsupported("V4 sparse sequence integration"));
            }
            let len = len.ok_or(Error::Corrupt("open V4 metadata chunk"))?;
            if tag == chunks::SDEF {
                let end = input.at.checked_add(len).ok_or(Error::TooLarge)?;
                sequence_defaults = Some(input.at..end);
            }
            if kind == chunks::Kind::Checksum && verify {
                let whole = input
                    .at
                    .checked_sub(chunk_at)
                    .and_then(|n| n.checked_add(len))
                    .ok_or(Error::TooLarge)?;
                crate::checksum::check_chunk_len(whole, length)?;
                checksum = Some((chunk_at, whole));
            }
            let prefix = chunks::prefix_len(&tag, Some(len)) as usize;
            let mut bytes = [0; crate::pixelsum::PREFIX_BYTES];
            input.read(&mut bytes[..prefix])?;
            if kind.has_body_rules(&tag) {
                rules.identity(tag, &bytes[..prefix])?;
            }
            if tag == crate::pixelsum::PSUM {
                pixel_status = crate::PixelChecksumStatus::Unsupported;
                if let Some((count, offset, algo)) = crate::pixelsum::prefix(&bytes[..prefix], len)?
                {
                    pixel_count = Some(count);
                    pixel_table = Some(crate::pixelsum::LocatedTable {
                        first: input.at - prefix as u64 + offset as u64,
                        count,
                        algo,
                    });
                    pixel_expected = crate::Digest::from_bytes(
                        algo,
                        bytes
                            .get(offset..offset + algo.width())
                            .ok_or(Error::Corrupt("short pSUM"))?,
                    );
                    pixel_status = crate::PixelChecksumStatus::Pending;
                }
            }
            if kind == chunks::Kind::Anim {
                unified_sequence = bytes.first() == Some(&4);
            }
            if kind == chunks::Kind::Frame {
                frames = frames.checked_add(1).ok_or(Error::TooLarge)?;
            }
            input.skip(len - prefix as u64)?;
        };
        rules.finish_prefix()?;
        if let Some(len) = data_length
            && data_at.checked_add(len).is_none_or(|end| end > length)
        {
            return Err(Error::Corrupt("V4 DATA past EOF"));
        }
        let setup_len = input.varint()?;
        let setup_memory = session.v4_working(setup_len as u128)?;
        let setup = input.vector(
            usize::try_from(setup_len).map_err(|_| Error::TooLarge)?,
            &setup_memory,
        )?;
        session.v4_setup_admitted(setup.len() + crate::varint_len(setup_len))?;
        let root = match parsed.header.coding {
            crate::Coding::Lossless => Root::Numeric(
                NumericGroupDecoder::from_setup(
                    parsed.header.info.clone(),
                    parsed.declared,
                    &setup,
                    limits,
                    session,
                )?
                .into_state(),
            ),
            crate::Coding::Lossy => {
                let color = parsed
                    .header
                    .lossy_color
                    .ok_or(Error::Corrupt("V4 lossy color declaration"))?;
                let semantic = Arc::new(super::LossySetup::read(
                    parsed.header.info.clone(),
                    color,
                    parsed.declared,
                    &setup,
                    session,
                )?);
                let context = super::SetupContext::from_image_declaration(
                    &parsed.header.info,
                    crate::Coding::Lossy,
                    Some(color),
                    &setup,
                )?;
                Root::Lossy {
                    setup: semantic,
                    context,
                }
            }
        };
        let placement = DirectoryPlacement::try_from(input.varint()?)?;
        if placement == DirectoryPlacement::None && numeric_tiles_only {
            return Err(Error::Unsupported("V4 sparse reader needs a directory"));
        }
        let (directory, index_memory, objects_at, tail_at) =
            if placement == DirectoryPlacement::None {
                let objects_at = input.at;
                let data_end = data_length
                    .map(|n| data_at.checked_add(n).ok_or(Error::TooLarge))
                    .transpose()?;
                let (bytes, memory) = discover::directory(&mut input, &root, limits, data_end)?;
                (bytes, memory, objects_at, None)
            } else if placement == DirectoryPlacement::Front {
                let capacity = input.varint()?;
                if capacity == 0 {
                    return Err(Error::Corrupt("empty V4 directory capacity"));
                }
                let slot_start = input.at;
                let index_len = input.varint()?;
                if index_len == 0
                    || index_len
                        .checked_add(input.at - slot_start)
                        .is_none_or(|n| n > capacity)
                {
                    return Err(Error::Corrupt("unfinished/oversize V4 front directory"));
                }
                let memory = session.v4_working(index_len as u128)?;
                let bytes = input.vector(
                    usize::try_from(index_len).map_err(|_| Error::TooLarge)?,
                    &memory,
                )?;
                let end = slot_start
                    .checked_add(capacity)
                    .filter(|&end| end <= length)
                    .ok_or(Error::Corrupt("V4 front directory bounds"))?;
                let mut padding = [0; 8192];
                while input.at < end {
                    let count = (end - input.at).min(padding.len() as u64) as usize;
                    input.read(&mut padding[..count])?;
                    if padding[..count].iter().any(|&b| b != 0) {
                        return Err(Error::Corrupt("V4 directory reservation padding"));
                    }
                }
                (bytes, memory.retain()?, input.at, None)
            } else {
                let objects_at = input.at;
                input.at = length
                    .checked_sub(12)
                    .ok_or(Error::Corrupt("missing V4 tail directory"))?;
                let mut footer = [0; 12];
                input.read(&mut footer)?;
                if &footer[8..] != b"uIDX" {
                    return Err(Error::Corrupt("missing V4 tail directory"));
                }
                let chunk_len = u64::from_le_bytes(footer[..8].try_into().unwrap());
                let start = length
                    .checked_sub(chunk_len)
                    .ok_or(Error::Corrupt("V4 tail locator bounds"))?;
                if start < objects_at {
                    return Err(Error::Corrupt("V4 tail locator overlaps setup"));
                }
                input.at = start;
                let mut tag = [0; 4];
                input.read(&mut tag)?;
                let payload_len = input
                    .varint()?
                    .checked_sub(1)
                    .ok_or(Error::Corrupt("open V4 tail directory"))?;
                if tag != *b"uIDX" || input.at.checked_add(payload_len) != Some(length) {
                    return Err(Error::Corrupt("V4 tail locator framing"));
                }
                let index_len = payload_len
                    .checked_sub(12)
                    .ok_or(Error::Corrupt("short V4 tail directory"))?;
                let memory = session.v4_working(index_len as u128)?;
                let bytes = input.vector(
                    usize::try_from(index_len).map_err(|_| Error::TooLarge)?,
                    &memory,
                )?;
                // Footer bytes were already read and validated at their exact end.
                (
                    bytes,
                    memory.retain()?,
                    objects_at,
                    Some((start, payload_len)),
                )
            };
        let suffix = directory
            .len()
            .checked_sub(8)
            .ok_or(Error::Corrupt("short V4 directory"))?;
        let objects_len = u64::from_le_bytes(directory[suffix..].try_into().unwrap());
        let actual_data_len = objects_at
            .checked_sub(data_at)
            .and_then(|n| n.checked_add(objects_len))
            .ok_or(Error::TooLarge)?;
        if data_length.is_some_and(|n| n != actual_data_len) {
            return Err(Error::Corrupt("V4 DATA/directory extent"));
        }
        let objects_end = objects_at
            .checked_add(objects_len)
            .filter(|&end| end <= length)
            .ok_or(Error::Corrupt("V4 object bounds"))?;
        let trailing_end = tail_at.map_or(length, |(start, _)| start);
        if objects_end > trailing_end {
            return Err(Error::Corrupt("V4 objects/index overlap"));
        }
        input.at = objects_end;
        // A primary-region read is a partial decode (SPEC 1.1), just like
        // the V2/V3 region route: later sequence items are not its input.
        // aNIM and pSUM precede DATA, so their interpretation is known here.
        // Retain the complete walk when checksum discovery/count validation
        // needs it, and for the stricter numeric-tile reader.
        let primary_sequence_only = !numeric_tiles_only
            && unified_sequence
            && announced.is_none()
            && checksum.is_none()
            && pixel_status == crate::PixelChecksumStatus::Absent;
        while !primary_sequence_only && input.at < trailing_end {
            let chunk_at = input.at;
            let mut tag = [0; 4];
            input.read(&mut tag)?;
            let len = input.varint()?.checked_sub(1);
            session.admission().chunk_start()?;
            let kind = rules.head(tag, len)?;
            if kind == chunks::Kind::UnitIndex {
                return Err(Error::Corrupt("competing V4 directories"));
            }
            if numeric_tiles_only && matches!(kind, chunks::Kind::Anim | chunks::Kind::Frame) {
                return Err(Error::Unsupported("V4 sparse sequence integration"));
            }
            let len = len.ok_or(Error::Corrupt("open trailing V4 chunk"))?;
            if input.at.checked_add(len).is_none_or(|n| n > trailing_end) {
                return Err(Error::Corrupt("V4 trailing chunk bounds"));
            }
            if kind == chunks::Kind::Checksum && verify {
                let whole = input
                    .at
                    .checked_sub(chunk_at)
                    .and_then(|n| n.checked_add(len))
                    .ok_or(Error::TooLarge)?;
                crate::checksum::check_chunk_len(whole, length)?;
                checksum = Some((chunk_at, whole));
            }
            let prefix = chunks::prefix_len(&tag, Some(len)) as usize;
            let mut bytes = [0; crate::pixelsum::PREFIX_BYTES];
            input.read(&mut bytes[..prefix])?;
            if kind.has_body_rules(&tag) {
                rules.identity(tag, &bytes[..prefix])?;
            }
            if tag == crate::pixelsum::PSUM {
                pixel_status = crate::PixelChecksumStatus::Unsupported;
                if let Some((count, offset, algo)) = crate::pixelsum::prefix(&bytes[..prefix], len)?
                {
                    pixel_count = Some(count);
                    pixel_expected = crate::Digest::from_bytes(
                        algo,
                        bytes
                            .get(offset..offset + algo.width())
                            .ok_or(Error::Corrupt("short pSUM"))?,
                    );
                    pixel_status = crate::PixelChecksumStatus::Pending;
                }
            }
            if kind == chunks::Kind::Anim {
                unified_sequence = bytes.first() == Some(&4);
            }
            if kind == chunks::Kind::Frame {
                frames = frames.checked_add(1).ok_or(Error::TooLarge)?;
            }
            input.skip(len - prefix as u64)?;
        }
        if let Some((_, payload_len)) = tail_at {
            rules.head(*b"uIDX", Some(payload_len))?;
        }
        rules.finish()?;
        if pixel_count.is_some_and(|n| n != frames + 1) {
            return Err(Error::Corrupt("pSUM image count"));
        }
        let checked = Directory::read_admitted(
            &directory,
            root.layout(),
            root.integrity(),
            objects_len,
            session,
        )?;
        if data_length.is_none() && !checked.ends_at_tile() {
            return Err(Error::Corrupt("open V4 DATA must end with its final tile"));
        }
        let tile_count =
            usize::try_from(root.layout().tile_count()).map_err(|_| Error::TooLarge)?;
        let tile_memory = session
            .v4_working((tile_count as u128) * core::mem::size_of::<Option<TileSpan>>() as u128)?;
        session.adapter_work(tile_count as u128)?;
        let mut tiles = Vec::new();
        tiles
            .try_reserve_exact(tile_count)
            .map_err(|_| Error::TooLarge)?;
        tiles.resize(tile_count, None);
        let (models, next_models, model_memory) =
            lossy::index(&checked, &root, &mut tiles, session)?;
        drop(setup);
        drop(setup_memory);
        drop(header_bytes);
        drop(header_body);
        drop(header_memory);
        let check = if verify {
            crate::checksum::presence(announced.is_some(), checksum.is_some())?;
            if let (Some(a), Some((at, count))) = (announced, checksum) {
                let log2 = a.log2()?;
                let words = (length - count).div_ceil(1u64 << log2).div_ceil(64);
                let memory = session.v4_working(
                    count as u128 + words as u128 * 8 + size_of::<FileCheck>() as u128,
                )?;
                let mut pieces =
                    crate::checksum::Fetched::new(at, count, log2, a.algorithm_id()?, length)?;
                let table_memory = session.v4_working(count as u128)?;
                input.at = at;
                let table = input.vector(
                    usize::try_from(count).map_err(|_| Error::TooLarge)?,
                    &table_memory,
                )?;
                if pieces.set_table(&table)? {
                    Some(FileCheck {
                        pieces,
                        opened: input.used.take().unwrap_or_default(),
                        _memory: memory.retain()?,
                    })
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        let mut file = Self {
            source,
            file_len: length,
            header: parsed.header,
            root,
            models,
            next_models,
            cached_models: Vec::new(),
            cache_memory: V4Retained::default(),
            _model_memory: model_memory,
            directory,
            tiles,
            objects_at,
            limits,
            _index_memory: index_memory,
            _tile_memory: tile_memory.retain()?,
            check,
            primary_end: objects_end,
            stored_index: placement != DirectoryPlacement::None,
            unified_sequence,
            checksum_declared: announced.is_some(),
            checksum_chunk: checksum,
            sequence_defaults,
            pixel_expected,
            pixel_table,
            pixel_status,
        };
        file.load_roots(session)?;
        Ok(file)
    }

    pub(crate) fn dependency_ranges<C: Fn() -> bool + Sync>(
        &self,
        ranges: &mut Vec<Range<u64>>,
        session: &DecodeSession<C>,
    ) -> Result<V4Retained, Error> {
        let max = ranges.iter().map(|r| r.end).max().unwrap_or(0);
        let count = self
            .models
            .iter()
            .filter(|m| m.cached.is_none() && self.objects_at + m.entry.offset < max)
            .count();
        session.adapter_work(self.models.len() as u128 + 1)?;
        let memory = session
            .v4_working((ranges.len() as u128 + count as u128) * size_of::<Range<u64>>() as u128)?;
        ranges
            .try_reserve_exact(count)
            .map_err(|_| Error::TooLarge)?;
        for model in &self.models {
            let at = self
                .objects_at
                .checked_add(model.entry.offset)
                .ok_or(Error::TooLarge)?;
            let end = at
                .checked_add(model.entry.entry.record_len)
                .ok_or(Error::TooLarge)?;
            if model.cached.is_none()
                && at < max
                && !ranges.iter().any(|r| r.start <= at && r.end >= end)
            {
                ranges.push(at..end);
            }
        }
        memory.retain()
    }
    pub(crate) fn checksum_ranges<C: Fn() -> bool + Sync>(
        &self,
        ranges: &mut Vec<Range<u64>>,
        session: &DecodeSession<C>,
    ) -> Result<V4Retained, Error> {
        let Some(check) = &self.check else {
            return Ok(V4Retained::default());
        };
        let count = ranges
            .len()
            .checked_add(check.opened.ranges.len())
            .ok_or(Error::TooLarge)?;
        let _wanted_memory = session.v4_working(count as u128 * size_of::<(u64, u64)>() as u128)?;
        let mut wanted = Vec::new();
        wanted
            .try_reserve_exact(count)
            .map_err(|_| Error::TooLarge)?;
        wanted.extend(ranges.iter().map(|r| (r.start, r.end)));
        wanted.extend(check.opened.ranges.iter().copied());
        let mut pieces = 0u128;
        for &(a, b) in &wanted {
            let (a, b) = (check.pieces.covered(a), check.pieces.covered(b));
            if a < b {
                pieces = pieces
                    .checked_add(u128::from(
                        ((b - 1) >> check.pieces.log2()) - (a >> check.pieces.log2()) + 1,
                    ))
                    .ok_or(Error::TooLarge)?;
            }
        }
        let _pending_memory = session.v4_working(pieces.max(4) * 16)?;
        session.adapter_work(pieces + 1)?;
        let pending = check.pieces.pending(&wanted, true)?;
        let extra = pending.len().checked_mul(2).ok_or(Error::TooLarge)?;
        let memory = session
            .v4_working((ranges.len() as u128 + extra as u128) * size_of::<Range<u64>>() as u128)?;
        ranges
            .try_reserve_exact(extra)
            .map_err(|_| Error::TooLarge)?;
        for i in pending {
            for (a, b) in check.pieces.segments(i) {
                ranges.push(a..b);
            }
        }
        memory.retain()
    }
    pub(crate) fn evict_models(&mut self) {
        for index in self.cached_models.drain(..) {
            self.models[index].cached = None;
        }
    }
    pub(crate) fn source(&self) -> &R {
        &self.source
    }
    pub(crate) fn source_mut(&mut self) -> &mut R {
        &mut self.source
    }
    pub(crate) fn is_unified_sequence(&self) -> bool {
        self.unified_sequence
    }
    pub(crate) fn has_index(&self) -> bool {
        self.stored_index
    }
    pub(crate) fn primary_end(&self) -> u64 {
        self.primary_end
    }
    pub(crate) fn checksum_declared(&self) -> bool {
        self.checksum_declared
    }
    pub(crate) fn file_checksum_chunk(&self) -> Option<Range<u64>> {
        self.checksum_chunk.map(|(at, len)| at..at + len)
    }
    pub(crate) fn sequence_defaults_range(&self) -> Option<Range<u64>> {
        self.sequence_defaults.clone()
    }
    pub(crate) fn pixel_checksum(
        &self,
    ) -> (Option<crate::Digest>, Option<crate::pixelsum::LocatedTable>) {
        (self.pixel_expected, self.pixel_table)
    }
    pub(crate) fn read_verified_at<C: Fn() -> bool + Sync>(
        &mut self,
        at: u64,
        bytes: &mut [u8],
        session: &DecodeSession<C>,
    ) -> Result<(), Error> {
        let end = at.checked_add(bytes.len() as u64).ok_or(Error::TooLarge)?;
        if end > self.file_len {
            return Err(Error::Corrupt("range outside IMI file"));
        }
        if self.checksum_declared && self.check.is_none() {
            return Err(Error::Unsupported("outer file checksum unavailable"));
        }
        session.v4_input(bytes.len())?;
        session.adapter_work(1 + bytes.len() as u128)?;
        self.source.read_exact_at(at, bytes)?;
        self.verify_used((at, end), at, bytes, session)
    }
    pub(crate) fn pixel_status(&self) -> crate::PixelChecksumStatus {
        self.pixel_status
    }
    pub(crate) fn layout(&self) -> super::TileLayout {
        self.root.layout()
    }
    pub(crate) fn group_rect(&self, id: u64) -> Result<crate::planes::Rect, Error> {
        match &self.root {
            Root::Numeric(root) => root.group_rect(id),
            Root::Lossy { setup, .. } => setup.group_rect(id),
        }
    }
    /// Call only after the complete fetched tile passed physical reconciliation.
    /// Results are fixed suffixes; no second parse, read or entropy decode.
    pub(crate) fn tile_pixel_check<'a>(
        &self,
        id: u64,
        bytes: &'a [u8],
    ) -> Result<Option<super::TilePixelCheck<'a>>, Error> {
        let checks = match &self.root {
            Root::Numeric(root) => root.check_config(),
            Root::Lossy { setup, .. } => setup.checks.as_ref(),
        };
        let Some(checks) = checks.filter(|c| c.width(super::CheckScope::TilePixels) != 0) else {
            return Ok(None);
        };
        let end = bytes
            .len()
            .checked_sub(self.root.integrity().trailer_len())
            .and_then(|v| v.checked_sub(checks.width(super::CheckScope::EncodedObject)))
            .ok_or(Error::Corrupt("short V4 tile check result"))?;
        let start = end
            .checked_sub(checks.width(super::CheckScope::TilePixels))
            .ok_or(Error::Corrupt("short V4 tile check result"))?;
        Ok(Some(super::TilePixelCheck {
            checks: checks.clone(),
            id,
            rect: super::tile_pixel_rect(self.layout(), id, |id| self.group_rect(id))?,
            expected: &bytes[start..end],
        }))
    }

    pub(crate) fn verify_pixels<C: Fn() -> bool + Sync>(
        &mut self,
        rect: crate::planes::Rect,
        pixels: &[u8],
        stride: usize,
        session: &DecodeSession<C>,
    ) -> Result<(), Error> {
        if let Some(expected) = self.pixel_expected {
            self.pixel_status = crate::PixelChecksumStatus::Pending;
            let info = &self.header.info;
            if rect == crate::planes::Rect::new(0, 0, info.width, info.height) {
                session.admission().pixel_checksum(info)?;
                let got = crate::pixelsum::digest_checked(
                    expected.algorithm(),
                    info,
                    0,
                    pixels,
                    stride,
                    &mut || session.adapter_work(64 * 1024),
                )?;
                if got != expected {
                    return Err(crate::pixelsum::MISMATCH);
                }
                self.pixel_status = crate::PixelChecksumStatus::Verified;
            }
        }
        Ok(())
    }
    pub fn header(&self) -> &Header {
        &self.header
    }
    pub fn into_inner(self) -> R {
        self.source
    }
    fn span<C: Fn() -> bool + Sync>(
        &self,
        id: u64,
        session: &DecodeSession<C>,
    ) -> Result<TileSpan, Error> {
        session.adapter_work(1)?;
        self.root.layout().tile(id)?;
        self.tiles
            .get(usize::try_from(id).map_err(|_| Error::TooLarge)?)
            .copied()
            .flatten()
            .ok_or(Error::Corrupt("missing V4 tile accelerator entry"))
    }
    pub(crate) fn tile_range<C: Fn() -> bool + Sync>(
        &self,
        id: u64,
        session: &DecodeSession<C>,
    ) -> Result<Range<u64>, Error> {
        let tile = self.span(id, session)?.extent;
        let start = self
            .objects_at
            .checked_add(tile.offset)
            .ok_or(Error::TooLarge)?;
        Ok(start..start.checked_add(tile.len).ok_or(Error::TooLarge)?)
    }
    /// Fetch into caller-owned compressed storage. Exactly one ReadAt request is
    /// issued; no other tile's payload is read. The destination may be partially
    /// changed on an I/O error. Encoded buffers count toward the caller's queue
    /// budget and remain live until all emitted jobs finish.
    pub(crate) fn fetch_tile<C: Fn() -> bool + Sync>(
        &mut self,
        id: u64,
        output: &mut Vec<u8>,
        session: &DecodeSession<C>,
    ) -> Result<TileExtent, Error> {
        let span = self.span(id, session)?;
        if span.extent.len > self.limits.max_tile_bytes {
            return Err(Error::TooLarge);
        }
        let length = usize::try_from(span.extent.len).map_err(|_| Error::TooLarge)?;
        session.v4_check_input(length)?;
        if length > output.len() {
            output
                .try_reserve(length - output.len())
                .map_err(|_| Error::TooLarge)?;
        }
        output.resize(length, 0);
        session.v4_input(length)?;
        session.adapter_work(length as u128 + 1)?;
        self.source
            .read_exact_at(self.objects_at + span.extent.offset, output)?;
        Ok(span.extent)
    }
    /// Fetch adjacent complete selected tiles without reading gaps. This is
    /// transport coalescing only: each tile is still reconciled independently.
    pub(crate) fn fetch_tile_batch<C: Fn() -> bool + Sync>(
        &mut self,
        ids: impl Iterator<Item = u64>,
        output: &mut Vec<u8>,
        session: &DecodeSession<C>,
    ) -> Result<Range<u64>, Error> {
        let mut range: Option<Range<u64>> = None;
        for id in ids {
            if self.span(id, session)?.extent.len > self.limits.max_tile_bytes {
                return Err(Error::TooLarge);
            }
            let next = self.tile_range(id, session)?;
            match &mut range {
                Some(range) if range.end == next.start => range.end = next.end,
                Some(_) => return Err(Error::InvalidInput("nonadjacent V4 tile batch")),
                None => range = Some(next),
            }
        }
        let range = range.ok_or(Error::InvalidInput("empty V4 tile batch"))?;
        let length = usize::try_from(range.end - range.start).map_err(|_| Error::TooLarge)?;
        session.v4_check_input(length)?;
        output
            .try_reserve_exact(length.saturating_sub(output.len()))
            .map_err(|_| Error::TooLarge)?;
        output.resize(length, 0);
        session.v4_input(length)?;
        session.adapter_work(length as u128 + 1)?;
        self.source.read_exact_at(range.start, output)?;
        Ok(range)
    }
    /// Reconcile and locally verify the fetched tile, delivering each group as
    /// independent immutable-model work. Callbacks can retain those jobs while
    /// parsing continues. Earlier delivered pixels remain provisional if a later
    /// group, tile aggregate or callback fails. Worker sessions own their budgets.
    pub(crate) fn visit_tile<'a, C: Fn() -> bool + Sync>(
        &self,
        id: u64,
        bytes: &'a [u8],
        session: &DecodeSession<C>,
        mut visit: impl FnMut(NumericWork<'a>) -> Result<(), Error>,
    ) -> Result<TileExtent, Error> {
        let span = self.span(id, session)?;
        if bytes.len() as u64 != span.extent.len {
            return Err(Error::Corrupt("V4 fetched tile length"));
        }
        let Root::Numeric(root) = &self.root else {
            return Err(Error::Unsupported("V4 sparse numeric coding"));
        };
        let mut decoder = root.at_tile(session, id, span.extent.offset, span.first_ignorable)?;
        let mut entries = ByteReader::new(&self.directory[span.entry_start..span.entry_end]);
        let mut offset = 0usize;
        let mut closed = None;
        while entries.remaining() != 0 {
            let entry = Entry::read(&mut entries)?;
            session.adapter_work(1)?;
            let length = usize::try_from(entry.record_len).map_err(|_| Error::TooLarge)?;
            let end = offset.checked_add(length).ok_or(Error::TooLarge)?;
            let input = bytes
                .get(offset..end)
                .ok_or(Error::Corrupt("V4 fetched/indexed tile extent"))?;
            let event = decoder.read(input)?;
            let expected = LocatedEntry {
                entry,
                offset: span.extent.offset + offset as u64,
                tile_id: Some(id),
            };
            if event.entry() != expected {
                return Err(Error::Corrupt("V4 fetched/directory identity"));
            }
            match event {
                NumericEvent::Group(work) => visit(work)?,
                NumericEvent::End { tile, .. } => closed = Some(tile),
                _ => {}
            }
            offset = end;
        }
        if offset != bytes.len() || closed != Some(span.extent) {
            return Err(Error::Corrupt("V4 fetched tile completion"));
        }
        Ok(span.extent)
    }

    /// Fetch the complete tile with one ReadAt call and emit independently
    /// schedulable groups. Outer cSUM can add reads for covering pieces outside
    /// that tile; it reuses the tile bytes. The supplied buffer must remain live
    /// until all retained jobs end.
    /// It is caller-owned queue storage, bounded here by max_tile_bytes. Input is
    /// charged by the fetch exactly once; all parse/hash/decode work is separate.
    pub(crate) fn read_tile<'a, C: Fn() -> bool + Sync>(
        &mut self,
        id: u64,
        output: &'a mut Vec<u8>,
        session: &DecodeSession<C>,
        visit: impl FnMut(NumericWork<'a>) -> Result<(), Error>,
    ) -> Result<TileExtent, Error> {
        self.fetch_tile(id, output, session)?;
        let range = self.tile_range(id, session)?;
        self.verify_used((range.start, range.end), range.start, output, session)?;
        self.visit_tile(id, output.as_slice(), session, visit)
    }
    pub(crate) fn verify_used<C: Fn() -> bool + Sync>(
        &mut self,
        range: (u64, u64),
        tile_at: u64,
        bytes: &[u8],
        session: &DecodeSession<C>,
    ) -> Result<(), Error> {
        if let Some(check) = &mut self.check {
            for wanted in check
                .opened
                .ranges
                .iter()
                .copied()
                .chain(core::iter::once(range))
            {
                verify_interval(
                    &mut self.source,
                    &mut check.pieces,
                    wanted,
                    (tile_at, bytes),
                    session,
                )?;
            }
            check.opened = UsedRanges::default();
        }
        Ok(())
    }
}

mod lossy;

mod discover;
