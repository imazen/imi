//! Complete numeric V4 files, sharing the ordinary outer chunk/checksum rules.
//!
//! Opening checks declarations and directory structure. Entropy, physical tile
//! framing and directory reconciliation are checked when their objects are read.
//! This module does not claim lossy or sequence support.

#[cfg(feature = "encode")]
use alloc::vec::Vec;

use super::{Directory, DirectoryPlacement, NumericEvent, NumericGroupDecoder, TileLimits};
use crate::decode_resources::DecodeSession;
use crate::{ByteReader, Error, Header, Metadata};

pub(crate) fn check_tail_locator(payload: &[u8], chunk_len: u64) -> Result<&[u8], Error> {
    let at = payload
        .len()
        .checked_sub(12)
        .ok_or(Error::Corrupt("short V4 index locator"))?;
    let footer = &payload[at..];
    let mut locator = ByteReader::new(footer);
    let length = crate::trace::tf!(
        "index chunk bytes",
        locator,
        u64::from_le_bytes(locator.bytes(8)?.try_into().unwrap())
    );
    let magic = crate::trace::tf!("index locator magic", locator, locator.bytes(4)?);
    if magic != b"uIDX" || length != chunk_len {
        return Err(Error::Corrupt("V4 index chunk locator"));
    }
    Ok(&payload[..at])
}

fn objects_len(index: &[u8]) -> Result<u64, Error> {
    let suffix = index
        .get(
            index
                .len()
                .checked_sub(8)
                .ok_or(Error::Corrupt("short V4 directory"))?..,
        )
        .ok_or(Error::Corrupt("short V4 directory"))?;
    Ok(u64::from_le_bytes(suffix.try_into().unwrap()))
}

// An index supplies a checked boundary hint, never proof of physical objects.
// Without one the common structural parser finds the last tile, charging its
// model/object work but never reconstructing pixels.
pub(crate) fn open_data_len<C: Fn() -> bool + Sync>(
    header: &Header,
    declared: crate::Features,
    data: &[u8],
    session: &DecodeSession<C>,
) -> Result<usize, Error> {
    let mut r = ByteReader::new(data);
    let setup_len = r.var_usize()?;
    r.bytes(setup_len)?;
    let placement = DirectoryPlacement::try_from(r.varint()?)?;
    let index = match placement {
        DirectoryPlacement::Tail => {
            let footer = data
                .get(
                    data.len()
                        .checked_sub(12)
                        .ok_or(Error::Corrupt("missing V4 tail index"))?..,
                )
                .ok_or(Error::Corrupt("missing V4 tail index"))?;
            if &footer[8..] != b"uIDX" {
                return Err(Error::Corrupt("missing V4 tail index"));
            }
            let chunk_len = u64::from_le_bytes(footer[..8].try_into().unwrap());
            let start = data
                .len()
                .checked_sub(usize::try_from(chunk_len).map_err(|_| Error::TooLarge)?)
                .ok_or(Error::Corrupt("V4 index locator bounds"))?;
            let mut chunk = ByteReader::new(&data[start..]);
            let (tag, len) = crate::parse::chunks::read_head(&mut chunk)?;
            if tag != *b"uIDX" {
                return Err(Error::Corrupt("V4 index locator tag"));
            }
            let payload = crate::parse::chunks::payload(
                &mut chunk,
                len.ok_or(Error::Corrupt("open V4 index"))?,
            )?;
            if chunk.remaining() != 0 {
                return Err(Error::Corrupt("V4 index locator extent"));
            }
            let index = check_tail_locator(payload, chunk_len)?;
            let end = (r.pos as u64)
                .checked_add(objects_len(index)?)
                .ok_or(Error::TooLarge)?;
            if end > start as u64 {
                return Err(Error::Corrupt("V4 DATA/index overlap"));
            }
            session.adapter_work(1)?;
            return usize::try_from(end).map_err(|_| Error::TooLarge);
        }
        DirectoryPlacement::Front => {
            let capacity = r.var_usize()?;
            let slot = r.bytes(capacity)?;
            let mut active = ByteReader::new(slot);
            let length = active.var_usize()?;
            if length == 0 {
                return Err(Error::Corrupt("unfinished V4 front directory"));
            }
            active.bytes(length)?
        }
        DirectoryPlacement::None => {
            // There is no directory length to use as a boundary hint. Locate
            // it with the same structural parser, without decoding entropy.
            // The real traversal rechecks these objects, so both visits count
            // as work/units; supplied input remains charged exactly once.
            return crate::trace::without(|| unindexed_end(header, declared, data, session));
        }
    };
    session.adapter_work(1)?;
    let end = (r.pos as u64)
        .checked_add(objects_len(index)?)
        .ok_or(Error::TooLarge)?;
    if end > data.len() as u64 {
        return Err(Error::Corrupt("V4 DATA extent"));
    }
    usize::try_from(end).map_err(|_| Error::TooLarge)
}

fn unindexed_end<C: Fn() -> bool + Sync>(
    header: &Header,
    declared: crate::Features,
    data: &[u8],
    session: &DecodeSession<C>,
) -> Result<usize, Error> {
    let _scope = session.v4_model_scope();
    let _input = session.v4_precharged()?;
    let limits = TileLimits {
        max_object_bytes: u64::MAX,
        max_tile_bytes: u64::MAX,
    };
    match header.coding {
        crate::Coding::Lossless => {
            let (mut decoder, mut offset) =
                NumericGroupDecoder::new(header.info.clone(), declared, data, limits, session)?;
            let mut tiles = 0;
            loop {
                let event = decoder.read(
                    data.get(offset..)
                        .ok_or(Error::Corrupt("open V4 DATA extent"))?,
                )?;
                offset = offset
                    .checked_add(
                        usize::try_from(event.entry().entry.record_len)
                            .map_err(|_| Error::TooLarge)?,
                    )
                    .ok_or(Error::TooLarge)?;
                if matches!(event, NumericEvent::End { .. }) {
                    tiles += 1;
                }
                if tiles == decoder.layout().tile_count() {
                    decoder.finish_structure()?;
                    return Ok(offset);
                }
            }
        }
        crate::Coding::Lossy => {
            let color = header
                .lossy_color
                .ok_or(Error::Corrupt("V4 lossy color missing"))?;
            let (mut decoder, mut offset) = super::LossyGroupDecoder::new(
                header.info.clone(),
                color,
                declared,
                data,
                limits,
                session,
            )?;
            let mut tiles = 0;
            loop {
                let event = decoder.read(
                    data.get(offset..)
                        .ok_or(Error::Corrupt("open V4 DATA extent"))?,
                )?;
                offset = offset
                    .checked_add(
                        usize::try_from(event.entry().entry.record_len)
                            .map_err(|_| Error::TooLarge)?,
                    )
                    .ok_or(Error::TooLarge)?;
                if matches!(event, super::LossyEvent::End { .. }) {
                    tiles += 1;
                }
                if tiles == decoder.setup().layout().tile_count() {
                    decoder.finish_structure()?;
                    return Ok(offset);
                }
            }
        }
    }
}

pub(crate) fn parse_file<'a, C: Fn() -> bool + Sync>(
    data: &'a [u8],
    session: &DecodeSession<C>,
) -> Result<crate::container::File<'a>, Error> {
    session.v4_check_input(data.len())?;
    let file = crate::container::parse_v4_checked(
        data,
        &|header, declared, payload| open_data_len(header, declared, payload, session),
        &session.v4_models(),
        session.v4_verify_mode(),
        session.v4_header_limit(),
    )?;
    crate::frames::admit_headers(&file, &session.v4_models())?;
    if let Some(pieces) = file.checksum.and_then(|c| c.pieces) {
        session.adapter_work(pieces.table.covered_len as u128 + pieces.table.pieces() as u128)?;
        for i in 0..pieces.table.pieces() {
            let mut hash = crate::Hasher::new(pieces.table.algo);
            let (a, b) = pieces.covered.bytes(pieces.table.piece(i));
            for bytes in [a, b] {
                for chunk in bytes.chunks(64 * 1024) {
                    session.adapter_work(0)?;
                    hash.update(chunk);
                }
            }
            if !pieces.table.matches(i, &hash.finish()) {
                return Err(crate::checksum::MISMATCH);
            }
        }
    }
    Ok(file)
}

pub(crate) fn read_directory<'a, C: Fn() -> bool + Sync>(
    data_length: usize,
    file: &crate::container::File<'a>,
    object_start: usize,
    layout: super::TileLayout,
    integrity: super::ObjectIntegrity,
    storage: super::DirectoryPrefix<'a>,
    session: &DecodeSession<C>,
) -> Result<(&'a [u8], Option<Directory<'a>>), Error> {
    let objects = &file.data[object_start..];
    let placement = storage.placement;
    let index = match placement {
        DirectoryPlacement::None => {
            if file.trailing_index.is_some() {
                return Err(Error::Corrupt("unexpected V4 tail directory"));
            }
            None
        }
        DirectoryPlacement::Tail => {
            let (payload, len) = file
                .trailing_index
                .ok_or(Error::Corrupt("missing V4 tail directory"))?;
            Some(check_tail_locator(payload, len)?)
        }
        DirectoryPlacement::Front => {
            if file.trailing_index.is_some() {
                return Err(Error::Corrupt("competing V4 directories"));
            }
            Some(
                storage
                    .front
                    .ok_or(Error::Corrupt("unfinished V4 front directory"))?,
            )
        }
    };
    let directory = index
        .map(|index| {
            let read = if placement == DirectoryPlacement::Front {
                Directory::read_admitted
            } else {
                Directory::read
            };
            read(index, layout, integrity, objects.len() as u64, session)
        })
        .transpose()?;
    if file.data_open
        && directory
            .as_ref()
            .is_some_and(|index| !index.ends_at_tile())
    {
        return Err(Error::Corrupt("open V4 DATA must end with its final tile"));
    }
    // DATA setup/objects and directory bodies are charged by their readers.
    // Charge the remaining outer framing, metadata, and locator exactly once.
    let mut outer_bytes = data_length
        .checked_sub(file.data.len())
        .ok_or(Error::Corrupt("V4 outer input extent"))?;
    // A front index is already contained/charged within the DATA prefix.
    if placement != DirectoryPlacement::Front {
        outer_bytes = outer_bytes
            .checked_sub(index.map_or(0, <[u8]>::len))
            .ok_or(Error::Corrupt("V4 outer input accounting"))?;
    }
    session.v4_input(outer_bytes)?;
    Ok((objects, directory))
}

/// Borrowed numeric file with checked outer framing and directory. Opening does
/// not validate unread object prefixes or entropy. `decode_into` reconciles the
/// entire physical stream before successful completion.
pub struct NumericFile<'a, 's, C> {
    file: crate::container::File<'a>,
    decoder: NumericGroupDecoder<'s, C>,
    objects: &'a [u8],
    directory: Option<Directory<'a>>,
    session: &'s DecodeSession<C>,
}

impl<'a, 's, C: Fn() -> bool + Sync> NumericFile<'a, 's, C> {
    pub fn open(
        data: &'a [u8],
        limits: TileLimits,
        session: &'s DecodeSession<C>,
    ) -> Result<Self, Error> {
        let file = parse_file(data, session)?;
        Self::from_parsed(file, data.len(), limits, session)
    }

    pub(crate) fn from_parsed(
        file: crate::container::File<'a>,
        file_length: usize,
        limits: TileLimits,
        session: &'s DecodeSession<C>,
    ) -> Result<Self, Error> {
        if file.header.coding != crate::Coding::Lossless {
            return Err(Error::Unsupported(
                "V4 numeric file requires lossless coding",
            ));
        }
        let (decoder, object_start, storage) = NumericGroupDecoder::with_directory(
            file.header.info.clone(),
            file.declared,
            file.data,
            limits,
            session,
        )?;
        let (objects, directory) = read_directory(
            file_length,
            &file,
            object_start,
            decoder.layout(),
            decoder.integrity(),
            storage,
            session,
        )?;
        Ok(Self {
            file,
            decoder,
            objects,
            directory,
            session,
        })
    }

    pub fn header(&self) -> &Header {
        &self.file.header
    }
    pub fn metadata(&self) -> Result<Metadata, Error> {
        self.file.metadata()
    }

    /// Deliver independent numeric work. Successful finish checks physical
    /// framing and directory bindings; bitmap checks require reconstructed pixels.
    pub fn visit_groups(
        self,
        mut group: impl FnMut(super::NumericWork<'a>) -> Result<(), Error>,
    ) -> Result<(Header, Metadata), Error> {
        self.visit_events(|event| {
            if let NumericEvent::Group(work) = event {
                group(work)?;
            }
            Ok(())
        })
    }

    pub(crate) fn visit_events(
        mut self,
        mut event: impl FnMut(NumericEvent<'a>) -> Result<(), Error>,
    ) -> Result<(Header, Metadata), Error> {
        let metadata = self.file.metadata()?;
        let mut offset = 0usize;
        let mut consume =
            |offset: usize, expected: Option<super::LocatedEntry>| -> Result<usize, Error> {
                let available = if let Some(entry) = expected {
                    if entry.offset != offset as u64 {
                        return Err(Error::Corrupt("V4 physical/index offset"));
                    }
                    let length =
                        usize::try_from(entry.entry.record_len).map_err(|_| Error::TooLarge)?;
                    let end = offset.checked_add(length).ok_or(Error::TooLarge)?;
                    self.objects
                        .get(offset..end)
                        .ok_or(Error::Corrupt("V4 indexed object bounds"))?
                } else {
                    &self.objects[offset..]
                };
                let next = self.decoder.read(available)?;
                let observed = next.entry();
                if expected.is_some_and(|expected| expected != observed) {
                    return Err(Error::Corrupt("V4 physical/directory mismatch"));
                }
                let end = offset
                    .checked_add(
                        usize::try_from(observed.entry.record_len).map_err(|_| Error::TooLarge)?,
                    )
                    .ok_or(Error::TooLarge)?;
                event(next)?;
                Ok(end)
            };
        if let Some(directory) = &self.directory {
            directory.visit(self.session, |entry, _| {
                offset = consume(offset, Some(entry))?;
                Ok(true)
            })?;
        } else {
            while offset < self.objects.len() {
                offset = consume(offset, None)?;
            }
        }
        if offset != self.objects.len() {
            return Err(Error::Corrupt("V4 trailing object bytes"));
        }
        self.decoder.finish_structure()?;
        Ok((self.file.header, metadata))
    }

    /// Immutable declared scopes, including suites this decoder cannot verify.
    pub fn checks(&self) -> super::Checks<'_> {
        self.check_config()
            .map_or_else(super::Checks::default, |c| c.get())
    }

    pub(crate) fn check_config(&self) -> Option<&alloc::sync::Arc<super::OwnedChecks>> {
        self.decoder.check_config()
    }

    pub(crate) fn visit_checked(
        self,
        mut visit: impl FnMut(super::decode::ImageEvent<'a>) -> Result<(), Error>,
    ) -> Result<(Header, Metadata), Error> {
        let checks = self.check_config().cloned();
        let layout = self.layout();
        let model = self.decoder.shared_model();
        let verify_tile = self.session.v4_pixel_checks(super::CheckScope::TilePixels);
        self.visit_events(|event| {
            match event {
                NumericEvent::Group(work) => visit(super::decode::ImageEvent::Group(
                    super::decode::GroupWork::Numeric(work),
                ))?,
                NumericEvent::End {
                    tile,
                    pixel_results,
                    ..
                } => {
                    if verify_tile
                        && let Some(checks) = &checks
                        && checks.width(super::CheckScope::TilePixels) != 0
                    {
                        visit(super::decode::ImageEvent::Tile(super::TilePixelCheck {
                            checks: checks.clone(),
                            id: tile.tile_id,
                            rect: super::tile_pixel_rect(layout, tile.tile_id, |id| {
                                model.group_rect(id)
                            })?,
                            expected: pixel_results,
                        }))?;
                    }
                }
                _ => {}
            }
            Ok(())
        })
    }

    pub(crate) fn pixelsum(&self) -> crate::pixelsum::Table<'a> {
        self.file.pixelsum
    }
    pub(crate) fn layout(&self) -> super::TileLayout {
        self.decoder.layout()
    }
    pub(crate) fn group_rect(&self, id: u64) -> Result<crate::planes::Rect, Error> {
        self.decoder.group_rect(id)
    }

    /// Decode strided pixels, completing full physical and bitmap validation.
    pub fn decode_into(
        self,
        output: &mut [u8],
        stride: usize,
    ) -> Result<(Header, Metadata), Error> {
        let session = self.session;
        super::decode::run_file(
            super::decode::ImageFile::Numeric(self),
            output,
            stride,
            session,
            None,
            1,
        )
    }
}

/// Encode a complete fixed-model numeric V4 file with a trailing directory.
/// This allocating convenience path retains its encoded file and directory;
/// forward-only gigapixel writers use caller-owned scratch instead.
#[cfg(feature = "encode")]
pub fn encode_numeric(
    info: crate::ImageInfo,
    pixels: &[u8],
    stride: usize,
    metadata: &Metadata,
    config: &crate::ImiEncoderConfig,
    spatial: super::SpatialConfig,
) -> Result<Vec<u8>, Error> {
    encode_numeric_checked(
        info,
        pixels,
        stride,
        metadata,
        config,
        spatial,
        super::Checks::default(),
    )
}

/// Encode with independent group/tile pixel and encoded-object checks. Tile
/// hashes traverse the supplied raster without retaining another tile copy.
#[cfg(feature = "encode")]
pub fn encode_numeric_checked(
    info: crate::ImageInfo,
    pixels: &[u8],
    stride: usize,
    metadata: &Metadata,
    config: &crate::ImiEncoderConfig,
    spatial: super::SpatialConfig,
    checks: super::Checks<'_>,
) -> Result<Vec<u8>, Error> {
    let row = (info.width as usize)
        .checked_mul(info.bytes_per_pixel())
        .ok_or(Error::TooLarge)?;
    let extent = (info.height as usize)
        .checked_sub(1)
        .and_then(|n| n.checked_mul(stride))
        .and_then(|n| n.checked_add(row))
        .ok_or(Error::TooLarge)?;
    if stride < row || pixels.len() < extent {
        return Err(Error::InvalidInput("V4 source stride/extent"));
    }
    let (mut encoder, mut payload) = super::NumericGroupEncoder::with_placement_checked(
        info.clone(),
        config,
        spatial,
        DirectoryPlacement::Tail,
        checks,
        false,
    )?;
    let features = encoder.features();
    let mut directory = super::DirectoryWriter::new(encoder.layout(), spatial.integrity);
    let mut entries = Vec::new();
    let mut publish = |encoder: &mut super::NumericGroupEncoder,
                       entry: super::LocatedEntry|
     -> Result<(), Error> {
        let mut buffer = [0; 40];
        let count = directory.push(entry.entry, &mut buffer)?;
        entries.try_reserve(count).map_err(|_| Error::TooLarge)?;
        entries.extend_from_slice(&buffer[..count]);
        payload
            .try_reserve(encoder.output().len())
            .map_err(|_| Error::TooLarge)?;
        payload.extend_from_slice(encoder.output());
        encoder.consume_output(encoder.output().len())
    };
    for ordinal in 0..encoder.layout().tile_count() {
        let begin = encoder.begin_tile()?;
        publish(&mut encoder, begin)?;
        while let Some(rect) = encoder.next_group() {
            let at = (rect.y as usize)
                .checked_mul(stride)
                .and_then(|n| n.checked_add(rect.x as usize * info.bytes_per_pixel()))
                .ok_or(Error::TooLarge)?;
            let entry = encoder.push_group(&pixels[at..], stride)?;
            publish(&mut encoder, entry)?;
        }
        let rect = encoder.tile_rect(encoder.layout().tile_at(ordinal)?.id)?;
        let mut local = info.clone();
        local.width = rect.width;
        local.height = rect.height;
        let at = rect.y as usize * stride + rect.x as usize * info.bytes_per_pixel();
        let (end, _) =
            encoder.end_tile_with_pixels(crate::ImageRef::new(&pixels[at..], stride, &local))?;
        publish(&mut encoder, end)?;
    }
    encoder.finish()?;
    let (count, objects_len) = directory.finish()?;
    let mut index = Vec::new();
    index
        .try_reserve_exact(entries.len().checked_add(30).ok_or(Error::TooLarge)?)
        .map_err(|_| Error::TooLarge)?;
    crate::put_varint(&mut index, count);
    index.extend_from_slice(&entries);
    index.extend_from_slice(&objects_len.to_le_bytes());
    let index_payload = index.len().checked_add(12).ok_or(Error::TooLarge)?;
    let chunk_len = 4 + crate::varint_len(index_payload as u64 + 1) as u64 + index_payload as u64;
    index.extend_from_slice(&chunk_len.to_le_bytes());
    index.extend_from_slice(b"uIDX");
    let mut file = Vec::new();
    crate::container::write_group_head_features(
        &mut file,
        &info,
        metadata,
        crate::Coding::Lossless,
        None,
        config.decode_speed().unwrap_or_default(),
        features,
        4,
    )?;
    file.try_reserve(
        payload
            .len()
            .checked_add(index.len())
            .and_then(|n| n.checked_add(28))
            .ok_or(Error::TooLarge)?,
    )
    .map_err(|_| Error::TooLarge)?;
    crate::container::put_data(&mut file, &payload);
    crate::container::put_chunk(&mut file, b"uIDX", &index);
    crate::checksum::finish_file(file, config.checksum().then_some(config.hash_algorithm()))
}
