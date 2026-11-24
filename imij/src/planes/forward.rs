//! Forward native plane/view output; only record lengths use caller scratch.
use super::*;
use crate::stream_encode::{StreamEncodeError, spool::Spool};
use crate::{ByteReader, ImageInfo};
use core::ops::Range;
use std::io::{Read, Seek, Write};

/// Declare a native layout, then publish independently encoded tiles in record
/// order. Encoded tile bytes remain caller-owned; the writer retains metadata,
/// one index page and a small page root, never a raster or complete output.
/// The scratch prefix is preserved and replay checks mutation. Completed
/// records flush before the final pages/root/locator. Failure poisons output.
///
/// The primary must be V4 without an enclosing cSUM. Page checks are mandatory;
/// child group/tile checks are retained verbatim. A complete-file cSUM requires
/// a separate replaying/seekable output policy and is refused before publication.
pub struct PlaneSetWriter<W, S> {
    writer: W,
    index: Spool<S>,
    dir: Directory,
    limits: PlaneSetLimits,
    per_page: u32,
    allow_packed_lengths: bool,
    declared: u32,
    next: u32,
    written: u64,
    started: bool,
    failed: bool,
    attachments: Vec<(u32, u64)>,
}
fn write_bytes<W: Write>(
    writer: &mut W,
    written: &mut u64,
    limit: u64,
    bytes: &[u8],
) -> Result<(), StreamEncodeError> {
    let end = written
        .checked_add(bytes.len() as u64)
        .filter(|&v| v <= limit)
        .ok_or(Error::TooLarge)?;
    writer.write_all(bytes)?;
    *written = end;
    Ok(())
}
fn chunk<W: Write>(
    writer: &mut W,
    written: &mut u64,
    limit: u64,
    tag: &[u8; 4],
    body: &[u8],
) -> Result<(), StreamEncodeError> {
    let total = wire::chunk_len(body.len() as u64)?;
    written
        .checked_add(total)
        .filter(|&v| v <= limit)
        .ok_or(Error::TooLarge)?;
    let mut head = Vec::new();
    head.try_reserve_exact(14).map_err(|_| Error::TooLarge)?;
    head.extend_from_slice(tag);
    crate::put_varint(&mut head, body.len() as u64 + 1);
    write_bytes(writer, written, limit, &head)?;
    write_bytes(writer, written, limit, body)
}
impl<W: Write, S: Read + Write + Seek> PlaneSetWriter<W, S> {
    pub fn new(
        primary: &[u8],
        mut writer: W,
        scratch: S,
        limits: PlaneSetLimits,
        records_per_page: u32,
        max_scratch_bytes: u64,
    ) -> Result<Self, StreamEncodeError> {
        record_pages::RecordPages::check_page_records(records_per_page)?;
        let (bytes, dir) = PlaneSetBuilder::forward_primary(primary, limits)?;
        let index = Spool::new(
            scratch,
            max_scratch_bytes,
            "native plane directory scratch changed",
        )?;
        let mut written = 0;
        write_bytes(&mut writer, &mut written, limits.max_file_bytes, &bytes)?;
        writer.flush()?;
        Ok(Self {
            writer,
            index,
            dir,
            limits,
            per_page: records_per_page,
            allow_packed_lengths: true,
            declared: 0,
            next: 0,
            written,
            started: false,
            failed: false,
            attachments: Vec::new(),
        })
    }
    #[cfg(test)]
    pub(super) fn use_legacy_lengths_for_test(&mut self) {
        self.allow_packed_lengths = false;
    }
    fn planning(&self) -> Result<(), Error> {
        if self.failed {
            return Err(Error::InvalidInput("failed native plane writer"));
        }
        if self.started {
            return Err(Error::InvalidInput("native plane declarations are closed"));
        }
        Ok(())
    }
    pub fn add_plane(&mut self, descriptor: PlaneDescriptor) -> Result<PlaneId, Error> {
        self.planning()?;
        validate_plane(&descriptor, self.limits)?;
        if !descriptor.levels.is_empty() {
            return Err(Error::InvalidInput(
                "declare native plane levels separately",
            ));
        }
        if self.dir.planes.len() >= self.limits.max_planes {
            return Err(Error::TooLarge);
        }
        if self
            .dir
            .planes
            .iter()
            .any(|p| p.group == descriptor.group && p.name == descriptor.name)
        {
            return Err(Error::InvalidInput("duplicate plane name in group"));
        }
        self.dir
            .planes
            .try_reserve(1)
            .map_err(|_| Error::TooLarge)?;
        let id = PlaneId(u32::try_from(self.dir.planes.len()).map_err(|_| Error::TooLarge)?);
        self.dir.planes.push(descriptor);
        Ok(id)
    }
    pub fn declare_level(
        &mut self,
        plane: PlaneId,
        level: LevelDescriptor,
        coding: Coding,
    ) -> Result<u32, Error> {
        let channels = self
            .dir
            .planes
            .get(plane.index() as usize)
            .ok_or(Error::InvalidInput("plane ID"))?
            .role
            .single_channel();
        Ok(self.declare_color_level(&[plane], level, channels, coding)?[0])
    }
    pub fn declare_color_level(
        &mut self,
        planes: &[PlaneId],
        mut level: LevelDescriptor,
        channels: Channels,
        coding: Coding,
    ) -> Result<Vec<u32>, Error> {
        self.planning()?;
        if planes.len() != channels.count() || planes.is_empty() || planes.len() > 4 {
            return Err(Error::InvalidInput("native level component count"));
        }
        let total = self
            .dir
            .planes
            .iter()
            .map(|p| p.levels.len())
            .sum::<usize>()
            .checked_add(planes.len())
            .ok_or(Error::TooLarge)?;
        if total > self.limits.max_levels {
            return Err(Error::TooLarge);
        }
        let count = u32::try_from(level.tiles()?.2).map_err(|_| Error::TooLarge)?;
        let next = self
            .declared
            .checked_add(count)
            .filter(|&n| n as usize <= self.limits.max_records)
            .ok_or(Error::TooLarge)?;
        let mut levels = Vec::new();
        levels
            .try_reserve_exact(planes.len())
            .map_err(|_| Error::TooLarge)?;
        for (component, &id) in planes.iter().enumerate() {
            if planes[..component].contains(&id) {
                return Err(Error::InvalidInput("duplicate color plane ID"));
            }
            let p = self
                .dir
                .planes
                .get_mut(id.0 as usize)
                .ok_or(Error::InvalidInput("plane ID"))?;
            if (p.role == PlaneRole::Scalar) != (channels == Channels::Scalar)
                || (channels == Channels::Scalar && coding != Coding::Lossless)
            {
                return Err(Error::InvalidInput("scalar plane level"));
            }
            p.levels.try_reserve(1).map_err(|_| Error::TooLarge)?;
            levels.push(u32::try_from(p.levels.len()).map_err(|_| Error::TooLarge)?);
        }
        level.record_start = self.declared;
        level.channels = channels;
        level.coding = coding;
        for (component, &id) in planes.iter().enumerate() {
            level.component = component as u8;
            self.dir.planes[id.0 as usize].levels.push(level.clone())
        }
        self.declared = next;
        Ok(levels)
    }
    pub fn record_range(&self, plane: PlaneId, level: u32) -> Result<Range<u32>, Error> {
        let level = self
            .dir
            .planes
            .get(plane.0 as usize)
            .and_then(|p| p.levels.get(level as usize))
            .ok_or(Error::InvalidInput("plane level"))?;
        let end = level
            .record_start
            .checked_add(u32::try_from(level.tiles()?.2).map_err(|_| Error::TooLarge)?)
            .ok_or(Error::TooLarge)?;
        Ok(level.record_start..end)
    }
    pub fn declare_attachment(
        &mut self,
        mut descriptor: AttachmentDescriptor,
    ) -> Result<AttachmentId, Error> {
        self.planning()?;
        if self.dir.attachments.len() >= self.limits.max_attachments
            || self.declared as usize >= self.limits.max_records
        {
            return Err(Error::TooLarge);
        }
        label(&descriptor.name, self.limits)?;
        label(&descriptor.mime, self.limits)?;
        if self
            .dir
            .attachments
            .iter()
            .any(|a| a.name == descriptor.name && a.group == descriptor.group)
        {
            return Err(Error::InvalidInput("duplicate attachment name in group"));
        }
        self.dir
            .attachments
            .try_reserve(1)
            .map_err(|_| Error::TooLarge)?;
        self.attachments
            .try_reserve(self.dir.attachments.len() + 1 - self.attachments.len())
            .map_err(|_| Error::TooLarge)?;
        let id =
            AttachmentId(u32::try_from(self.dir.attachments.len()).map_err(|_| Error::TooLarge)?);
        descriptor.record = self.declared;
        self.declared = self.declared.checked_add(1).ok_or(Error::TooLarge)?;
        self.dir.attachments.push(descriptor);
        Ok(id)
    }
    pub fn set_image_directory(&mut self, directory: ImageDirectory) -> Result<(), Error> {
        self.planning()?;
        directory.validate(&self.dir, self.limits)?;
        self.dir.images = Some(directory);
        Ok(())
    }
    pub fn bytes_written(&self) -> u64 {
        self.written
    }
    pub fn next_record(&self) -> Option<u32> {
        if self.failed || self.next >= self.declared {
            None
        } else {
            Some(self.next)
        }
    }
    fn start(&mut self) -> Result<(), Error> {
        if self.failed {
            return Err(Error::InvalidInput("failed native plane writer"));
        }
        if self.started {
            return Ok(());
        }
        if self.declared == 0 {
            return Err(Error::InvalidInput("paged record directory is empty"));
        }
        wire::validate_metadata(&self.dir, self.limits, self.declared as usize, None, |_| {
            None
        })?;
        let n = self.declared.div_ceil(self.per_page) as usize;
        let base = wire::encode(&self.dir, self.limits, 4)?.len();
        let reserve = n
            .checked_mul(36)
            .and_then(|v| v.checked_add(base))
            .and_then(|v| v.checked_add(50 + self.dir.attachments.len() * 15))
            .ok_or(Error::TooLarge)?;
        if reserve > self.limits.max_index_bytes
            || n.checked_mul(core::mem::size_of::<record_pages::Page>())
                .is_none_or(|v| v > self.limits.max_index_bytes)
        {
            return Err(Error::TooLarge);
        }
        self.started = true;
        Ok(())
    }
    pub fn write_tile(&mut self, record_id: u32, encoded: &[u8]) -> Result<(), StreamEncodeError> {
        let result = (|| {
            self.start()?;
            if record_id != self.next || record_id >= self.declared {
                return Err(Error::InvalidInput("native record publication order").into());
            }
            let mut expected = None;
            for plane in &self.dir.planes {
                for level in &plane.levels {
                    let (nx, _, count) = level.tiles()?;
                    if record_id >= level.record_start
                        && (record_id - level.record_start) < count as u32
                    {
                        let tile = record_id - level.record_start;
                        let x = (tile % nx) * level.tile_width;
                        let y = (tile / nx) * level.tile_height;
                        let mut info = ImageInfo::new(
                            level.tile_width.min(level.width - x),
                            level.tile_height.min(level.height - y),
                            level.channels,
                            plane.sample,
                        );
                        info.cicp = plane.cicp;
                        let mut bits = [0; 4];
                        for p in &self.dir.planes {
                            for l in &p.levels {
                                if l.record_start == level.record_start {
                                    bits[l.component as usize] = p.precision
                                }
                            }
                        }
                        let components = info.channels.count();
                        info = info.with_significant_bits(&bits[..components])?;
                        expected = Some((info, level.coding));
                        break;
                    }
                }
                if expected.is_some() {
                    break;
                }
            }
            let (info, coding) =
                expected.ok_or(Error::InvalidInput("record is not a native tile"))?;
            if encoded.len() as u64 > self.limits.max_record_bytes.min(self.limits.max_tile_bytes) {
                return Err(Error::TooLarge.into());
            }
            super::decode::native_profile(
                encoded,
                info.width,
                info.height,
                info.channels,
                info.sample,
                coding,
                info.cicp,
                Some(core::array::from_fn(|c| info.channel_bits(c).unwrap_or(0))),
            )?;
            self.publish(record_id, encoded)
        })();
        if result.is_err() {
            self.failed = true
        }
        result
    }
    pub fn write_attachment(
        &mut self,
        id: AttachmentId,
        bytes: &[u8],
    ) -> Result<(), StreamEncodeError> {
        let result = (|| {
            self.start()?;
            let record = self
                .dir
                .attachments
                .get(id.0 as usize)
                .ok_or(Error::InvalidInput("attachment ID"))?
                .record;
            if record != self.next {
                return Err(Error::InvalidInput("native record publication order").into());
            }
            if bytes.len() as u64 > self.limits.max_attachment_bytes {
                return Err(Error::TooLarge.into());
            }
            self.publish(record, bytes)?;
            self.attachments.push((record, bytes.len() as u64));
            Ok(())
        })();
        if result.is_err() {
            self.failed = true
        }
        result
    }
    fn publish(&mut self, id: u32, bytes: &[u8]) -> Result<(), StreamEncodeError> {
        let mut identity = Vec::new();
        identity.try_reserve_exact(5).map_err(|_| Error::TooLarge)?;
        crate::put_varint(&mut identity, id.into());
        let len = (bytes.len() as u64)
            .checked_add(identity.len() as u64)
            .ok_or(Error::TooLarge)?;
        let mut head = Vec::new();
        head.try_reserve_exact(19).map_err(|_| Error::TooLarge)?;
        head.extend_from_slice(&record_pages::RECORD_TAG);
        crate::put_varint(&mut head, len + 1);
        head.extend_from_slice(&identity);
        self.written
            .checked_add(head.len() as u64)
            .and_then(|v| v.checked_add(bytes.len() as u64))
            .filter(|&v| v <= self.limits.max_file_bytes)
            .ok_or(Error::TooLarge)?;
        let mut length = Vec::new();
        length.try_reserve_exact(10).map_err(|_| Error::TooLarge)?;
        crate::put_varint(&mut length, len);
        self.index.append(&length)?;
        write_bytes(
            &mut self.writer,
            &mut self.written,
            self.limits.max_file_bytes,
            &head,
        )?;
        write_bytes(
            &mut self.writer,
            &mut self.written,
            self.limits.max_file_bytes,
            bytes,
        )?;
        self.writer.flush()?;
        self.next += 1;
        Ok(())
    }
    pub fn finish(mut self) -> Result<(W, S), StreamEncodeError> {
        self.start()?;
        if self.next != self.declared {
            return Err(Error::InvalidInput("native plane records incomplete").into());
        }
        // Validate the complete scratch snapshot before publishing index pages.
        self.index.replay(|_| Ok(()))?;
        let data_end = self.written;
        let mut emitter = PageEmitter::new(
            &mut self.writer,
            &mut self.written,
            self.limits,
            self.dir.primary_len,
            self.declared,
            self.per_page,
            self.allow_packed_lengths,
        )?;
        self.index.replay(|bytes| emitter.feed(bytes))?;
        let (pages, packed_lengths) = emitter.complete(data_end)?;
        let root = record_pages::RecordPages {
            packed_lengths,
            count: self.declared,
            per_page: self.per_page,
            data_end,
            pages,
            lengths: self.attachments,
        };
        root.validate_attachments(&self.dir.attachments)?;
        let raw = wire::encode_with_pages(&self.dir, self.limits, 4, Some(&root))?;
        let packed = crate::metadata_pack::pack(&raw)?;
        if packed.len() > self.limits.max_index_bytes {
            return Err(Error::TooLarge.into());
        }
        let at = self.written;
        let length = wire::chunk_len(packed.len() as u64)?;
        let end = at
            .checked_add(length)
            .and_then(|v| v.checked_add(wire::FOOTER_LEN))
            .filter(|&v| v <= self.limits.max_file_bytes)
            .ok_or(Error::TooLarge)?;
        chunk(
            &mut self.writer,
            &mut self.written,
            self.limits.max_file_bytes,
            &wire::INDEX,
            &packed,
        )?;
        write_bytes(
            &mut self.writer,
            &mut self.written,
            self.limits.max_file_bytes,
            &wire::footer(at, length, end),
        )?;
        self.writer.flush()?;
        Ok((self.writer, self.index.into_inner()))
    }
}
struct PageEmitter<'a, W> {
    writer: &'a mut W,
    written: &'a mut u64,
    limits: PlaneSetLimits,
    total: u32,
    per: u32,
    first: u32,
    count: u32,
    data_at: u64,
    data_bytes: u64,
    lengths: Vec<u64>,
    allow_packed_lengths: bool,
    packed_lengths: bool,
    var: [u8; 10],
    used: usize,
    pages: Vec<record_pages::Page>,
}
impl<'a, W: Write> PageEmitter<'a, W> {
    fn new(
        writer: &'a mut W,
        written: &'a mut u64,
        limits: PlaneSetLimits,
        primary: u64,
        total: u32,
        per: u32,
        allow_packed_lengths: bool,
    ) -> Result<Self, Error> {
        let mut lengths = Vec::new();
        lengths
            .try_reserve_exact(per as usize)
            .map_err(|_| Error::TooLarge)?;
        let mut pages = Vec::new();
        pages
            .try_reserve_exact(total.div_ceil(per) as usize)
            .map_err(|_| Error::TooLarge)?;
        Ok(Self {
            writer,
            written,
            limits,
            total,
            per,
            first: 0,
            count: 0,
            data_at: primary,
            data_bytes: 0,
            lengths,
            allow_packed_lengths,
            packed_lengths: false,
            var: [0; 10],
            used: 0,
            pages,
        })
    }
    fn feed(&mut self, bytes: &[u8]) -> Result<(), StreamEncodeError> {
        for &b in bytes {
            if self.first >= self.total || self.used == 10 {
                return Err(Error::Corrupt("native index scratch length").into());
            }
            self.var[self.used] = b;
            self.used += 1;
            if b < 128 {
                let mut r = ByteReader::new(&self.var[..self.used]);
                let len = r.varint()?;
                self.lengths.push(len);
                self.used = 0;
                self.data_bytes = self
                    .data_bytes
                    .checked_add(wire::chunk_len(len)?)
                    .ok_or(Error::TooLarge)?;
                self.count += 1;
                if self.count == (self.total - self.first).min(self.per) {
                    self.emit()?
                }
            }
        }
        Ok(())
    }
    fn emit(&mut self) -> Result<(), StreamEncodeError> {
        let (packed, compact) = record_pages::pack_lengths(
            self.first.into(),
            self.lengths.iter().copied(),
            self.allow_packed_lengths,
        )?;
        self.packed_lengths |= compact;
        if packed.len() > record_pages::MAX_PAGE_BYTES {
            return Err(Error::TooLarge.into());
        }
        let mut page = record_pages::Page {
            data_offset: self.data_at,
            data_bytes: self.data_bytes,
            offset: *self.written,
            len: packed.len() as u64,
            digest: [0; 16],
        };
        page.digest =
            record_pages::page_digest(self.first.into(), self.count.into(), &page, &packed)?;
        chunk(
            self.writer,
            self.written,
            self.limits.max_file_bytes,
            &record_pages::PAGE_TAG,
            &packed,
        )?;
        self.pages.push(page);
        self.data_at = self
            .data_at
            .checked_add(self.data_bytes)
            .ok_or(Error::TooLarge)?;
        self.first += self.count;
        self.count = 0;
        self.data_bytes = 0;
        self.lengths.clear();
        Ok(())
    }
    fn complete(self, data_end: u64) -> Result<(Vec<record_pages::Page>, bool), Error> {
        if self.used != 0 || self.count != 0 || self.first != self.total || self.data_at != data_end
        {
            return Err(Error::Corrupt("native plane scratch coverage"));
        }
        Ok((self.pages, self.packed_lengths))
    }
}
