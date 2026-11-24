//! Plane-set construction from independently supplied level samples.
use super::*;
#[cfg(feature = "encode")]
use crate::config::Lowered;

/// Appends ancillary plane records to a validated existing native primary.
/// Finish returns one complete file. Failed additions leave existing records
/// unchanged. Input buffers are validated and tiles use no inter prediction.
pub struct PlaneSetBuilder {
    bytes: Vec<u8>,
    /// Directory ids follow the retained primary's header version.
    pub(super) version: u8,
    pub(super) dir: Directory,
    pub(super) limits: PlaneSetLimits,
    /// File offset of the primary's file checksum record (its piece size) and
    /// the record's digest algorithm, when it has a checksum: `finish` writes
    /// a new table into the primary.
    checksum_at: Option<(usize, crate::HashAlgorithm)>,
    /// Science records by target (0: defaults, k + 1: plane k), sorted.
    science: Vec<(u32, ScienceRecords)>,
    // Opaque ignorable directory records retained by encoded-domain upgrades.
    extra_records: Vec<(u64, Vec<u8>)>,
    page_records: Option<u32>,
}
impl PlaneSetBuilder {
    #[cfg(feature = "encode")]
    pub fn new(primary: &[u8]) -> Result<Self, Error> {
        Self::new_with_limits(primary, PlaneSetLimits::default())
    }
    pub(super) fn encode_directory(&self) -> Result<Vec<u8>, Error> {
        self.encode_directory_pages(None)
    }
    fn encode_directory_pages(
        &self,
        pages: Option<&record_pages::RecordPages>,
    ) -> Result<Vec<u8>, Error> {
        let raw = if let Some(pages) = pages {
            wire::encode_with_pages(&self.dir, self.limits, self.version, Some(pages))?
        } else {
            wire::encode(&self.dir, self.limits, self.version)?
        };
        if self.extra_records.is_empty() {
            return Ok(raw);
        }
        let mut items: Vec<(u64, &[u8])> = Vec::new();
        crate::parse::records::head_records(&raw, |id, bytes| {
            items.try_reserve(1).map_err(|_| Error::TooLarge)?;
            items.push((id, bytes));
            Ok(())
        })?;
        items
            .try_reserve(self.extra_records.len())
            .map_err(|_| Error::TooLarge)?;
        items.extend(
            self.extra_records
                .iter()
                .map(|(id, bytes)| (*id, bytes.as_slice())),
        );
        items.sort_by_key(|item| item.0);
        let mut out = Vec::new();
        for (id, bytes) in items {
            let extra = crate::varint_len(id) + crate::varint_len(bytes.len() as u64) + bytes.len();
            if out.len().checked_add(extra).ok_or(Error::TooLarge)? > self.limits.max_index_bytes {
                return Err(Error::TooLarge);
            }
            out.try_reserve(extra).map_err(|_| Error::TooLarge)?;
            crate::parse::records::put_record(&mut out, id, bytes);
        }
        Ok(out)
    }
    /// Select the V4 paged record authority. Pixel payloads retain their codec
    /// bytes; each pREC carries its canonical record ID for local binding.
    /// This builder retains encoded bytes; the forward writer owns its scratch.
    #[cfg(feature = "encode")]
    pub fn set_record_pages(&mut self, records_per_page: u32) -> Result<(), Error> {
        if self.version != 4 {
            return Err(Error::InvalidInput("paged records require V4"));
        }
        record_pages::RecordPages::check_page_records(records_per_page)?;
        self.page_records = Some(records_per_page);
        Ok(())
    }
    fn bind_record_ids(&mut self) -> Result<(), Error> {
        if self.page_records.is_none() {
            return Ok(());
        }
        if self.dir.records.is_empty() {
            return Err(Error::InvalidInput("paged record directory is empty"));
        }
        let mut bytes = Vec::new();
        append(&mut bytes, &self.bytes[..self.dir.primary_len as usize])?;
        for (id, record) in self.dir.records.iter_mut().enumerate() {
            let end = record
                .offset
                .checked_add(wire::chunk_len(record.len)?)
                .ok_or(Error::TooLarge)?;
            let payload = wire::payload(
                self.bytes
                    .get(
                        record.offset as usize
                            ..usize::try_from(end).map_err(|_| Error::TooLarge)?,
                    )
                    .ok_or(Error::Corrupt("plane record extent"))?,
                &wire::DATA,
            )?;
            let mut identity = Vec::new();
            identity.try_reserve_exact(5).map_err(|_| Error::TooLarge)?;
            crate::put_varint(&mut identity, id as u64);
            let len = (payload.len() as u64)
                .checked_add(identity.len() as u64)
                .ok_or(Error::TooLarge)?;
            let limit = (bytes.len() as u64)
                .checked_add(wire::chunk_len(len)?)
                .ok_or(Error::TooLarge)?;
            if limit > self.limits.max_file_bytes {
                return Err(Error::TooLarge);
            }
            record.offset = bytes.len() as u64;
            record.len = len;
            append(&mut bytes, &record_pages::RECORD_TAG)?;
            let mut size = Vec::new();
            size.try_reserve_exact(10).map_err(|_| Error::TooLarge)?;
            crate::put_varint(&mut size, len + 1);
            append(&mut bytes, &size)?;
            append(&mut bytes, &identity)?;
            append(&mut bytes, payload)?;
        }
        self.bytes = bytes;
        Ok(())
    }
    fn packed_index(&self, data_end: u64, stored: bool) -> Result<(Vec<u8>, Vec<u8>), Error> {
        let (raw, pages) = if let Some(n) = self.page_records {
            let (root, pages) =
                record_pages::RecordPages::build(&self.dir, n, data_end, self.limits)?;
            (self.encode_directory_pages(Some(&root))?, pages)
        } else {
            (self.encode_directory()?, Vec::new())
        };
        let packed = if stored {
            crate::metadata_pack::pack_stored(&raw)?
        } else {
            crate::metadata_pack::pack(&raw)?
        };
        if packed.len() > self.limits.max_index_bytes {
            return Err(Error::TooLarge);
        }
        Ok((packed, pages))
    }

    #[cfg(all(feature = "std", feature = "encode"))]
    pub(super) fn forward_primary(
        primary: &[u8],
        limits: PlaneSetLimits,
    ) -> Result<(Vec<u8>, Directory), Error> {
        let result = Self::new_with_limits(primary, limits)?;
        if result.version != 4 {
            return Err(Error::InvalidInput("forward plane output requires V4"));
        }
        if result.checksum_at.is_some() {
            return Err(Error::InvalidInput(
                "forward plane output requires a primary without outer cSUM",
            ));
        }
        Ok((result.bytes, result.dir))
    }
    /// Set bounds before examining or copying a potentially large primary.
    ///
    /// A primary with a `cSUM` file checksum is verified; the finished file
    /// carries a new table over all of it, in the primary before its `DATA`
    /// chunk (records, index and locator keep the layout readers without
    /// checksums expect). An unknown checksum algorithm is refused
    /// ([`Error::Unsupported`]).
    #[cfg(feature = "encode")]
    pub fn new_with_limits(primary: &[u8], limits: PlaneSetLimits) -> Result<Self, Error> {
        if primary.len() as u64 > limits.max_record_bytes
            || primary.len() as u64 > limits.max_file_bytes
        {
            return Err(Error::TooLarge);
        }
        if primary.len() >= 37 && primary[primary.len() - 37..primary.len() - 33] == wire::LOCATOR {
            return Err(Error::InvalidInput(
                "primary already contains a plane locator",
            ));
        }
        let normalized;
        let is_v4 = crate::parse::header::header_version(primary)? == 4;
        let primary = if is_v4 {
            let migration = crate::UpgradeLimits {
                max_encoded_bytes: usize::try_from(
                    limits.max_record_bytes.min(limits.max_file_bytes),
                )
                .unwrap_or(usize::MAX),
                ..Default::default()
            };
            normalized = crate::v4::reframe::front_directory(primary, &migration)?;
            normalized.as_slice()
        } else {
            primary
        };
        // The chunk rules every reader applies (SPEC 1.1) and the container
        // framing, without decoding the full raster. Bound encoded input
        // before existing metadata parsing.
        let session = crate::decode_resources::DecodeSession::new(
            crate::decode_resources::DecodePolicy::default()
                .with_output_limits(crate::Limits::NONE),
        );
        let f = if is_v4 {
            crate::v4::file::parse_file(primary, &session)?
        } else {
            crate::container::parse(primary, false, &crate::v1::payload_len)?
        };
        // Reject orphan/interior plane chunks as well as a final locator
        // (the rules let plane chunks through). Walk only framing here;
        // opaque vendor payloads may contain these byte sequences and are
        // not searched or recursively interpreted.
        let mut scan = crate::ByteReader::new(primary);
        let crate::parse::header::ParsedHeader {
            header, declared, ..
        } = crate::parse::header::read_header_sized(&mut scan)?;
        while scan.remaining() != 0 {
            let (tag, len) = crate::parse::chunks::read_head(&mut scan)?;
            if crate::parse::chunks::is_plane_chunk(&tag) {
                return Err(Error::InvalidInput("primary already contains plane chunks"));
            }
            let size = match len {
                Some(n) => n,
                None => {
                    crate::parse::chunks::open_chunk(&tag)?;
                    if is_v4 {
                        f.data.len() as u64
                    } else {
                        crate::v1::payload_len(&header, declared, &primary[scan.pos..])? as u64
                    }
                }
            };
            crate::parse::chunks::payload(&mut scan, size)?;
        }
        let checksum = crate::container::ChecksumChunk::rewritable(f.checksum)?;
        // The primary without its checksum chunk; plane records follow it.
        let mut bytes = Vec::new();
        match f.checksum {
            Some(c) if checksum => {
                append(&mut bytes, &primary[..c.at])?;
                append(&mut bytes, &primary[c.at + c.len..])?;
            }
            _ => append(&mut bytes, primary)?,
        }
        let checksum_at = if checksum {
            let record =
                crate::parse::header::read_header_sized(&mut crate::ByteReader::new(&bytes))?
                    .checksum;
            match record {
                Some(c) => Some((
                    c.at,
                    c.announced
                        .algorithm()
                        .ok_or(Error::Unsupported("unknown file checksum algorithm"))?,
                )),
                None => None,
            }
        } else {
            None
        };
        Ok(Self {
            version: header.version,
            dir: Directory {
                images: None,
                pages: None,
                primary_len: bytes.len() as u64,
                records: Vec::new(),
                planes: Vec::new(),
                attachments: Vec::new(),
            },
            bytes,
            limits,
            checksum_at,
            science: Vec::new(),
            extra_records: Vec::new(),
            page_records: None,
        })
    }
    /// Configure bounds before adding descriptors or records.
    #[cfg(feature = "encode")]
    pub fn with_limits(mut self, limits: PlaneSetLimits) -> Result<Self, Error> {
        if !self.dir.planes.is_empty() || !self.dir.attachments.is_empty() {
            return Err(Error::InvalidInput("set plane limits before additions"));
        }
        if self.bytes.len() as u64 > limits.max_record_bytes
            || self.bytes.len() as u64 > limits.max_file_bytes
        {
            return Err(Error::TooLarge);
        }
        self.limits = limits;
        Ok(self)
    }
    #[cfg(feature = "encode")]
    pub fn planes(&self) -> &[PlaneDescriptor] {
        &self.dir.planes
    }
    #[cfg(feature = "encode")]
    pub(crate) fn limits(&self) -> PlaneSetLimits {
        self.limits
    }
    #[cfg(feature = "encode")]
    pub fn add_plane(&mut self, descriptor: PlaneDescriptor) -> Result<PlaneId, Error> {
        if descriptor.role == PlaneRole::Scalar && self.version != 4 {
            return Err(Error::Unsupported("scalar planes require V4"));
        }
        validate_plane(&descriptor, self.limits)?;
        if !descriptor.levels.is_empty() {
            return Err(Error::InvalidInput("plane descriptor already has levels"));
        }
        if self.dir.planes.len() >= self.limits.max_planes {
            return Err(Error::TooLarge);
        }
        if self
            .dir
            .planes
            .iter()
            .any(|p| p.name == descriptor.name && p.group == descriptor.group)
        {
            return Err(Error::InvalidInput("duplicate plane name in group"));
        }
        let id = u32::try_from(self.dir.planes.len()).map_err(|_| Error::TooLarge)?;
        self.dir
            .planes
            .try_reserve(1)
            .map_err(|_| Error::TooLarge)?;
        self.dir.planes.push(descriptor);
        Ok(PlaneId(id))
    }

    /// Samples are native-endian scalar values with an explicit byte stride.
    #[cfg(feature = "encode")]
    pub fn add_level(
        &mut self,
        id: PlaneId,
        level: LevelDescriptor,
        pixels: &[u8],
        stride: usize,
        coding: crate::ImiEncoderConfig,
    ) -> Result<u32, Error> {
        self.validate_level_request(id, &level, &coding)?;
        let channels = self.dir.planes[id.index() as usize].role.single_channel();
        Ok(self.add_color_level(&[id], level, pixels, stride, channels, coding)?[0])
    }

    /// Codes interleaved native channel layouts once, retaining the
    /// optimized native color path. Each named scalar plane references its own
    /// component of the same independently seekable tile records. IDs must be
    /// distinct and ordered like the interleaved source; sample types, group
    /// and registration origins must match. Existing levels need not match.
    #[cfg(feature = "encode")]
    pub fn add_color_level(
        &mut self,
        ids: &[PlaneId],
        mut level: LevelDescriptor,
        pixels: &[u8],
        stride: usize,
        channels: Channels,
        coding: crate::ImiEncoderConfig,
    ) -> Result<Vec<u32>, Error> {
        if ids.len() != channels.count() {
            return Err(Error::InvalidInput("color plane count"));
        }
        let first = self
            .dir
            .planes
            .get(ids[0].0 as usize)
            .ok_or(Error::InvalidInput("plane ID"))?;
        let sample = first.sample;
        let cicp = first.cicp;
        let icc_profile = first.icc_profile;
        let registration = (first.group, first.origin);
        let mut precision = [0u8; 4];
        for (i, &id) in ids.iter().enumerate() {
            if ids[..i].contains(&id) {
                return Err(Error::InvalidInput("duplicate color plane ID"));
            }
            let p = self
                .dir
                .planes
                .get(id.0 as usize)
                .ok_or(Error::InvalidInput("plane ID"))?;
            if (p.role == PlaneRole::Scalar) != (channels == Channels::Scalar) {
                return Err(Error::InvalidInput("scalar plane channel identity"));
            }
            if p.sample != sample
                || (p.group, p.origin) != registration
                || p.cicp != cicp
                || p.icc_profile != icc_profile
            {
                return Err(Error::InvalidInput("color plane registration/type"));
            }
            self.validate_level_request(id, &level, &coding)?;
            precision[i] = p.precision;
        }
        let kind = level_kind(&coding)?;
        let lowered = coding.lower();
        let (nx, ny, count) = level.tiles()?;
        if self
            .dir
            .planes
            .iter()
            .map(|p| p.levels.len())
            .sum::<usize>()
            .checked_add(ids.len())
            .is_none_or(|n| n > self.limits.max_levels)
        {
            return Err(Error::TooLarge);
        }
        let row = sample_bytes(level.width, 1, sample)?
            .checked_mul(channels.count())
            .ok_or(Error::TooLarge)?;
        let end = stride
            .checked_mul(level.height as usize - 1)
            .and_then(|n| n.checked_add(row))
            .ok_or(Error::TooLarge)?;
        if stride < row || pixels.len() < end {
            return Err(Error::InvalidInput("short color level buffer/stride"));
        }
        let tile_bytes = sample_bytes(
            level.width.min(level.tile_width),
            level.height.min(level.tile_height),
            sample,
        )?
        .checked_mul(channels.count())
        .ok_or(Error::TooLarge)?;
        if tile_bytes as u64 > self.limits.max_tile_bytes {
            return Err(Error::TooLarge);
        }
        self.dir
            .records
            .try_reserve(count)
            .map_err(|_| Error::TooLarge)?;
        let mut levels = Vec::new();
        levels
            .try_reserve_exact(ids.len())
            .map_err(|_| Error::TooLarge)?;
        let mut result_ids = Vec::new();
        result_ids
            .try_reserve_exact(ids.len())
            .map_err(|_| Error::TooLarge)?;
        for (component, &id) in ids.iter().enumerate() {
            let p = &mut self.dir.planes[id.0 as usize];
            p.levels.try_reserve(1).map_err(|_| Error::TooLarge)?;
            result_ids.push(u32::try_from(p.levels.len()).map_err(|_| Error::TooLarge)?);
            // Construct rather than clone caller-provided record arrays.
            let mut l = LevelDescriptor::new(
                level.width,
                level.height,
                level.tile_width,
                level.tile_height,
            )
            .with_source(level.source);
            l.record_start = u32::try_from(self.dir.records.len()).map_err(|_| Error::TooLarge)?;
            l.coding = kind;
            l.channels = channels;
            l.component = component as u8;
            levels.push(l);
        }
        level.coding = kind;
        let start = self.bytes.len();
        let rec_start = self.dir.records.len();
        let result = (|| {
            let mut tile = Vec::new();
            tile.try_reserve_exact(tile_bytes)
                .map_err(|_| Error::TooLarge)?;
            for ty in 0..ny {
                for tx in 0..nx {
                    let (x, y) = (tx * level.tile_width, ty * level.tile_height);
                    let (w, h) = (
                        level.tile_width.min(level.width - x),
                        level.tile_height.min(level.height - y),
                    );
                    let row = sample_bytes(w, 1, sample)?
                        .checked_mul(channels.count())
                        .ok_or(Error::TooLarge)?;
                    tile.resize(row * h as usize, 0);
                    for iy in 0..h as usize {
                        let src = (y as usize + iy) * stride
                            + x as usize * sample.bytes() * channels.count();
                        tile[iy * row..(iy + 1) * row].copy_from_slice(&pixels[src..src + row]);
                    }
                    let mut info = crate::ImageInfo::new(w, h, channels, sample)
                        .with_significant_bits(&precision[..channels.count()])?;
                    info.cicp = cicp;
                    // Tiles are nested files that admit only DATA: no
                    // checksum or DATA locator of their own.
                    let none = crate::Metadata::default();
                    let file = match &lowered {
                        Lowered::Lossless(c) => {
                            crate::v1::encode_frame(&tile, row, &info, &none, c, None)?
                        }
                        Lowered::Lossy(c) => {
                            crate::lossy::encode_lossy_frame(&tile, row, &info, &none, c, None)?
                        }
                        Lowered::Quick(_) => {
                            return Err(Error::Unsupported("quick coder in plane levels"));
                        }
                    };
                    let file = if self.version == 4 {
                        crate::v4::migrate::finish_encoded(file)?
                    } else {
                        file
                    };
                    if file.len() as u64 > self.limits.max_record_bytes {
                        return Err(Error::TooLarge);
                    }
                    self.push_record(&file)?;
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.bytes.truncate(start);
            self.dir.records.truncate(rec_start);
            return Err(error);
        }
        for (&id, l) in ids.iter().zip(levels) {
            self.dir.planes[id.0 as usize].levels.push(l);
        }
        Ok(result_ids)
    }

    /// Shared preflight for direct-original generation, before its allocation
    /// or caller callback. Encoded byte sizes are checked after encoding.
    #[cfg(feature = "encode")]
    pub(super) fn validate_level_request(
        &self,
        id: PlaneId,
        level: &LevelDescriptor,
        coding: &crate::ImiEncoderConfig,
    ) -> Result<(), Error> {
        let plane = self
            .dir
            .planes
            .get(id.0 as usize)
            .ok_or(Error::InvalidInput("plane ID"))?;
        let kind = level_kind(coding)?;
        if plane.role == PlaneRole::Raw && kind != Coding::Lossless {
            return Err(Error::InvalidInput("RAW levels must be lossless"));
        }
        if plane.role == PlaneRole::Scalar && kind != Coding::Lossless {
            return Err(Error::InvalidInput("scalar plane levels must be lossless"));
        }
        if plane.role.is_complex() && kind != Coding::Lossless {
            return Err(Error::InvalidInput("complex plane levels must be lossless"));
        }
        if kind == Coding::Lossy && !plane.sample.lossy_capable() {
            return Err(Error::Unsupported("lossy plane sample type"));
        }
        if kind == Coding::Lossy {
            if matches!(plane.sample, SampleType::F16 | SampleType::F32)
                && plane.cicp.transfer_characteristics != 8
            {
                return Err(Error::Unsupported(
                    "lossy float planes require linear-light CICP transfer 8",
                ));
            }
            if let Lowered::Lossy(config) = coding.lower()
                && plane.sample == SampleType::U8
                && config.color == crate::LossyColor::Xyb
                && plane.cicp.transfer_characteristics != 13
            {
                return Err(Error::Unsupported("lossy U8 XYB requires sRGB transfer"));
            }
        }
        let (_, _, count) = level.tiles()?;
        if u64::from(level.width) * u64::from(level.height) > self.limits.max_plane_pixels {
            return Err(Error::TooLarge);
        }
        if plane.levels.is_empty()
            && (plane.origin.0.checked_add(i64::from(level.width)).is_none()
                || plane
                    .origin
                    .1
                    .checked_add(i64::from(level.height))
                    .is_none())
        {
            return Err(Error::InvalidInput("plane origin overflow"));
        }
        let tile = sample_bytes(
            level.width.min(level.tile_width),
            level.height.min(level.tile_height),
            plane.sample,
        )?;
        if tile as u64 > self.limits.max_tile_bytes
            || self
                .dir
                .records
                .len()
                .checked_add(count)
                .is_none_or(|n| n > self.limits.max_records)
            || self
                .dir
                .planes
                .iter()
                .map(|p| p.levels.len())
                .sum::<usize>()
                >= self.limits.max_levels
            || (self.bytes.len() as u64)
                .checked_add((count as u64).checked_mul(5).ok_or(Error::TooLarge)?)
                .is_none_or(|n| n > self.limits.max_file_bytes)
        {
            return Err(Error::TooLarge);
        }
        Ok(())
    }

    pub fn add_attachment(
        &mut self,
        mut descriptor: AttachmentDescriptor,
        bytes: &[u8],
    ) -> Result<AttachmentId, Error> {
        label(&descriptor.name, self.limits)?;
        label(&descriptor.mime, self.limits)?;
        if !descriptor.mime.is_ascii()
            || !descriptor.mime.contains('/')
            || descriptor.mime.bytes().any(|v| v < 32 || v == 127)
        {
            return Err(Error::InvalidInput("attachment MIME"));
        }
        if self.dir.attachments.len() >= self.limits.max_attachments
            || self.dir.records.len() >= self.limits.max_records
            || bytes.len() as u64 > self.limits.max_attachment_bytes
        {
            return Err(Error::TooLarge);
        }
        if self
            .dir
            .attachments
            .iter()
            .any(|a| a.name == descriptor.name && a.group == descriptor.group)
        {
            return Err(Error::InvalidInput("duplicate attachment name in group"));
        }
        let id = u32::try_from(self.dir.attachments.len()).map_err(|_| Error::TooLarge)?;
        self.dir
            .attachments
            .try_reserve(1)
            .map_err(|_| Error::TooLarge)?;
        self.dir
            .records
            .try_reserve(1)
            .map_err(|_| Error::TooLarge)?;
        let start = self.bytes.len();
        descriptor.record = match self.push_record(bytes) {
            Ok(id) => id,
            Err(e) => {
                self.bytes.truncate(start);
                return Err(e);
            }
        };
        self.dir.attachments.push(descriptor);
        Ok(AttachmentId(id))
    }

    /// Science records (SPEC.md 1.5: calibration, nodata, units, axes,
    /// coordinates, categories, structured metadata) for one plane, or with `None` the
    /// defaults for every plane that has no record of that kind itself.
    /// Replaces earlier records for the target; empty records remove them.
    /// A plane's coordinates must match its own axes or the default axes, so
    /// set the defaults first; mask references are checked by `finish`,
    /// which writes all records as one attachment (MIME type
    /// `application/x.imij-science`). Without records nothing is added.
    #[cfg(feature = "encode")]
    pub fn set_science(
        &mut self,
        plane: Option<PlaneId>,
        records: ScienceRecords,
    ) -> Result<(), Error> {
        let target = match plane {
            None => 0,
            Some(id) if (id.0 as usize) < self.dir.planes.len() => id.0 + 1,
            Some(_) => return Err(Error::InvalidInput("plane ID")),
        };
        let default_axes = self
            .science
            .first()
            .filter(|(t, _)| *t == 0)
            .and_then(|(_, r)| r.axes.as_ref());
        let axes = records.axes.as_ref().or(default_axes);
        let sample = plane.map(|id| self.dir.planes[id.0 as usize].sample);
        records.encode_for(
            &crate::science::Target::plane_pending(target == 0, sample),
            axes,
        )?;
        let at = self.science.partition_point(|(t, _)| *t < target);
        if self.science.get(at).is_some_and(|(t, _)| *t == target) {
            self.science.remove(at);
        }
        if !records.is_empty() {
            self.science.try_reserve(1).map_err(|_| Error::TooLarge)?;
            self.science.insert(at, (target, records));
        }
        Ok(())
    }

    fn push_record(&mut self, payload: &[u8]) -> Result<u32, Error> {
        let len = wire::chunk_len(payload.len() as u64)?;
        if (self.bytes.len() as u64)
            .checked_add(len)
            .is_none_or(|n| n > self.limits.max_file_bytes)
        {
            return Err(Error::TooLarge);
        }
        let id = u32::try_from(self.dir.records.len()).map_err(|_| Error::TooLarge)?;
        let rec = Record {
            offset: self.bytes.len() as u64,
            len: payload.len() as u64,
        };
        wire::chunk(&mut self.bytes, &wire::DATA, payload)?;
        self.dir.records.push(rec);
        Ok(id)
    }

    pub fn finish(mut self) -> Result<Vec<u8>, Error> {
        check_complex_pairs(&self.dir.planes).map_err(Error::InvalidInput)?;
        if !self.science.is_empty() {
            if self
                .dir
                .attachments
                .iter()
                .any(|a| a.mime == crate::science::PLANE_SET_MIME)
            {
                return Err(Error::InvalidInput(
                    "science records and an attachment of their MIME type",
                ));
            }
            let planes = u32::try_from(self.dir.planes.len()).map_err(|_| Error::TooLarge)?;
            let bytes = crate::science::encode_plane_entries(&self.science, planes)?;
            self.add_attachment(
                AttachmentDescriptor::new(
                    crate::science::PLANE_SET_NAME,
                    crate::science::PLANE_SET_MIME,
                ),
                &bytes,
            )?;
        }
        wire::validate(&self.dir, self.limits, self.bytes.len() as u64)?;
        self.bind_record_ids()?;
        let Some((record_at, algo)) = self.checksum_at else {
            let (packed, pages) = self.packed_index(self.bytes.len() as u64, false)?;
            append(&mut self.bytes, &pages)?;
            let offset = self.bytes.len() as u64;
            let index_len = wire::chunk_len(packed.len() as u64)?;
            let size = offset
                .checked_add(index_len)
                .and_then(|n| n.checked_add(wire::FOOTER_LEN))
                .ok_or(Error::TooLarge)?;
            if size > self.limits.max_file_bytes {
                return Err(Error::TooLarge);
            }
            wire::chunk(&mut self.bytes, &wire::INDEX, &packed)?;
            append(&mut self.bytes, &wire::footer(offset, index_len, size))?;
            return Ok(self.bytes);
        };
        // The file checksum table goes into the primary, before its DATA
        // chunk, so the records still run from the primary's end to the index
        // and the locator still follows the index, as readers without
        // checksums require. The primary length and the record offsets shift
        // by the table's length, which depends on the covered length and so on
        // the index: iterate to a fixed point. A compressed index may change
        // size with the offsets; the stored form, whose size does not, ends it.
        // The primary's DATA locator names the table too (SPEC 1.0), so each
        // round rewrites it for a table of the current length; the piece
        // size is chosen once (as `checksum::seal` does), so the table and
        // the locator only grow.
        let base_primary = self.dir.primary_len;
        let base: Vec<u64> = self.dir.records.iter().map(|r| r.offset).collect();
        let tail = self.bytes.len() as u64 - base_primary;
        let width = crate::checksum::writer_width(algo);
        let mut table = 0u64;
        let mut k = None;
        let mut fixed = None;
        for round in 0..12 {
            let primary = crate::checksum::locate_with_chunk(
                self.bytes[..base_primary as usize].to_vec(),
                table as usize,
            )?;
            let plen = primary.len() as u64;
            self.dir.primary_len = plen + table;
            for (rec, &offset) in self.dir.records.iter_mut().zip(&base) {
                rec.offset = offset - base_primary + plen + table;
            }
            let (packed, pages) = self.packed_index(plen + table + tail, round >= 8)?;
            let index_len = wire::chunk_len(packed.len() as u64)?;
            let covered = (plen + tail)
                .checked_add(pages.len() as u64)
                .and_then(|n| n.checked_add(index_len))
                .and_then(|n| n.checked_add(wire::FOOTER_LEN))
                .ok_or(Error::TooLarge)?;
            let k = *k.get_or_insert_with(|| crate::checksum::piece_log2_for(covered));
            let next = crate::checksum::chunk_len(covered, k, width) as u64;
            if next == table {
                fixed = Some((primary, packed, pages, index_len, covered, k));
                break;
            }
            table = next;
        }
        let (primary, packed, pages, index_len, covered, k) = fixed.ok_or(Error::TooLarge)?;
        let size = covered.checked_add(table).ok_or(Error::TooLarge)?;
        if size > self.limits.max_file_bytes {
            return Err(Error::TooLarge);
        }
        // The primary with its settled locator; the checksum record (an
        // earlier extension than the locator) is found again, since the
        // header length may have changed.
        let record_at =
            crate::parse::header::read_header_sized(&mut crate::ByteReader::new(&primary))?
                .checksum
                .map(|c| c.at)
                .unwrap_or(record_at);
        let insert = crate::container::data_at(&primary)?;
        self.bytes.splice(..base_primary as usize, primary);
        self.bytes[record_at] = k;
        append(&mut self.bytes, &pages)?;
        let index_offset = self.bytes.len() as u64 + table;
        wire::chunk(&mut self.bytes, &wire::INDEX, &packed)?;
        append(
            &mut self.bytes,
            &wire::footer(index_offset, index_len, size),
        )?;
        let chunk = crate::checksum::chunk_bytes(
            &self.bytes[..insert],
            &self.bytes[insert..],
            k,
            algo,
            width,
        );
        debug_assert_eq!(chunk.len() as u64, table);
        self.bytes
            .try_reserve(chunk.len())
            .map_err(|_| Error::TooLarge)?;
        self.bytes.splice(insert..insert, chunk);
        Ok(self.bytes)
    }
}

/// The wire coding a level's encoder produces; quick is refused later, at
/// encode time, so it reports lossless here.
#[cfg(feature = "encode")]
fn level_kind(coding: &crate::ImiEncoderConfig) -> Result<Coding, Error> {
    Ok(if coding.coder().is_lossless() {
        Coding::Lossless
    } else {
        Coding::Lossy
    })
}

#[cfg(test)]
pub(crate) fn upgrade_image_planes(
    file: &[u8],
    parsed: crate::container::File<'_>,
    upgrade: &crate::UpgradeLimits,
    depth: usize,
) -> Result<Vec<u8>, Error> {
    upgrade_image_planes_with(
        file,
        parsed,
        upgrade,
        depth,
        crate::parse::header::HEADER_VERSION,
        crate::upgrade::rewrite_primary,
        crate::upgrade::rewrite,
    )
}

pub(crate) fn upgrade_image_planes_with(
    file: &[u8],
    parsed: crate::container::File<'_>,
    upgrade: &crate::UpgradeLimits,
    depth: usize,
    version: u8,
    primary_rewrite: fn(&[u8], &crate::UpgradeLimits, usize) -> Result<Vec<u8>, Error>,
    nested_rewrite: fn(&[u8], &crate::UpgradeLimits, usize) -> Result<Vec<u8>, Error>,
) -> Result<Vec<u8>, Error> {
    let limits = upgrade
        .plane_limits
        .with_file_bytes(
            upgrade
                .plane_limits
                .max_file_bytes
                .min(upgrade.max_encoded_bytes as u64),
        )
        .with_record_bytes(
            upgrade
                .plane_limits
                .max_record_bytes
                .min(upgrade.max_encoded_bytes as u64),
        );
    if file.len() as u64 > limits.max_file_bytes {
        return Err(Error::TooLarge);
    }
    let footer: &[u8; 37] = file[file.len() - 37..].try_into().unwrap();
    let (offset, len) = wire::locator(footer, file.len() as u64)?;
    let raw = crate::metadata_pack::unpack(
        wire::payload(
            &file[offset as usize..(offset + len) as usize],
            &wire::INDEX,
        )?,
        limits.max_index_bytes,
    )?;
    let mut dir = wire::decode(&raw, limits, offset, parsed.header.version)?;
    let page_records = dir.pages.as_ref().map(|p| p.per_page);
    if page_records.is_some() {
        let mut reader = PlaneSetReader::open(SliceReader::new(file), limits)?;
        let count = dir.record_count();
        dir.records
            .try_reserve_exact(count)
            .map_err(|_| Error::TooLarge)?;
        for id in 0..count {
            dir.records.push(reader.record_by_id(id as u32)?)
        }
        dir.pages = None;
    }
    if dir.primary_len > limits.max_record_bytes {
        return Err(Error::TooLarge);
    }
    let mut extra_records = Vec::new();
    crate::parse::records::head_records(&raw, |id, bytes| {
        if !matches!(
            crate::registry::directory_id(parsed.header.version, id),
            crate::registry::DIRECTORY_PRIMARY
                | crate::registry::DIRECTORY_TILES
                | crate::registry::DIRECTORY_PLANES
                | crate::registry::DIRECTORY_ATTACHMENTS
                | crate::registry::DIRECTORY_IMAGES
                | crate::registry::DIRECTORY_RECORD_PAGES
        ) {
            extra_records.try_reserve(1).map_err(|_| Error::TooLarge)?;
            let mut retained = Vec::new();
            crate::upgrade::append(&mut retained, bytes, upgrade)?;
            extra_records.push((id, retained));
        }
        Ok(())
    })?;
    let mut primary = Vec::new();
    crate::upgrade::append(&mut primary, &file[..dir.primary_len as usize], upgrade)?;
    let algorithm = if let Some(c) = parsed.checksum {
        if c.at + c.len > primary.len() {
            return Err(Error::Corrupt("plane checksum outside primary"));
        }
        let hp = crate::parse::header::read_header_sized(&mut crate::ByteReader::new(file))?;
        let algo = hp
            .checksum
            .and_then(|c| c.announced.algorithm())
            .ok_or(Error::Unsupported("unknown file checksum algorithm"))?;
        primary.drain(c.at..c.at + c.len);
        primary =
            crate::container::with_extension(primary, crate::registry::HEADER_CHECKSUM, None)?;
        Some(algo)
    } else {
        None
    };
    primary = primary_rewrite(&primary, upgrade, depth)?;
    if version == 4 {
        primary = crate::v4::reframe::front_directory(&primary, upgrade)?;
    }
    let checksum_at = if let Some(algo) = algorithm {
        primary = crate::container::with_extension(
            primary,
            crate::registry::HEADER_CHECKSUM,
            Some(&[10, algo.wire()]),
        )?;
        let hp = crate::parse::header::read_header_sized(&mut crate::ByteReader::new(&primary))?;
        Some((
            hp.checksum
                .ok_or(Error::Corrupt("missing checksum announcement"))?
                .at,
            algo,
        ))
    } else {
        None
    };
    if primary.len() as u64 > limits.max_record_bytes {
        return Err(Error::TooLarge);
    }
    dir.primary_len = primary.len() as u64;
    let mut bytes = primary;
    for (index, record) in dir.records.iter_mut().enumerate() {
        let end = record
            .offset
            .checked_add(wire::chunk_len(record.len)?)
            .ok_or(Error::TooLarge)?;
        let payload = wire::payload(
            &file[record.offset as usize..end as usize],
            if page_records.is_some() {
                &record_pages::RECORD_TAG
            } else {
                &wire::DATA
            },
        )?;
        if payload.len() as u64 != record.len {
            return Err(Error::Corrupt("plane record length"));
        }
        let payload = if page_records.is_some() {
            let mut r = crate::ByteReader::new(payload);
            if r.varint()? != index as u64 {
                return Err(Error::Corrupt("plane bound record identity"));
            }
            &payload[r.pos..]
        } else {
            payload
        };
        let attachment = dir.attachments.iter().any(|a| a.record as usize == index);
        let rewritten;
        let payload = if attachment {
            payload
        } else {
            rewritten = nested_rewrite(payload, upgrade, depth + 1)?;
            &rewritten
        };
        record.offset = bytes.len() as u64;
        record.len = payload.len() as u64;
        if (bytes.len() as u64)
            .checked_add(wire::chunk_len(record.len)?)
            .ok_or(Error::TooLarge)?
            > upgrade.max_encoded_bytes as u64
        {
            return Err(Error::TooLarge);
        }
        wire::chunk(&mut bytes, &wire::DATA, payload)?;
    }
    let out = PlaneSetBuilder {
        bytes,
        version,
        dir,
        limits,
        checksum_at,
        science: Vec::new(),
        extra_records,
        page_records,
    }
    .finish()?;
    if version == 4 {
        crate::v4::file::parse_file(
            &out,
            &crate::decode_resources::DecodeSession::new(
                crate::decode_resources::DecodePolicy::default()
                    .with_output_limits(crate::Limits::NONE),
            ),
        )?;
    } else {
        crate::upgrade::parse(&out)?;
    }
    Ok(out)
}

#[cfg(all(test, feature = "encode"))]
mod upgrade_tests {
    use super::*;
    #[test]
    fn plane_primary_cannot_hide_another_plane_set_at_the_same_depth() {
        let primary = include_bytes!("../../../fuzz/seeds/31-seed-v2-unit-hint.imi");
        let inner = PlaneSetBuilder::new(primary).unwrap().finish().unwrap();
        let mut outer = PlaneSetBuilder::new(primary).unwrap();
        assert!(outer.checksum_at.is_none());
        outer.dir.primary_len = inner.len() as u64;
        outer.bytes = inner;
        let invalid = outer.finish().unwrap();
        assert!(matches!(
            crate::upgrade_image(&invalid, &Default::default()),
            Err(Error::Corrupt("plane primary contains plane chunks"))
        ));
    }

    #[test]
    fn optional_directory_records_are_not_lost_by_migration() {
        let primary = include_bytes!("../../../fuzz/seeds/24-seed-csum-lossless.imi");
        let mut builder = PlaneSetBuilder::new(primary).unwrap();
        builder.extra_records.push((98, alloc::vec![0xa5; 256]));
        let file = builder.finish().unwrap();
        let new = crate::upgrade_image(&file, &crate::UpgradeLimits::default()).unwrap();
        let (offset, len) =
            wire::locator(new[new.len() - 37..].try_into().unwrap(), new.len() as u64).unwrap();
        let packed =
            wire::payload(&new[offset as usize..(offset + len) as usize], &wire::INDEX).unwrap();
        let raw = crate::metadata_pack::unpack(packed, 1 << 20).unwrap();
        let mut found = false;
        crate::parse::records::head_records(&raw, |id, bytes| {
            if id == 98 {
                assert_eq!(bytes, &[0xa5; 256]);
                found = true;
            }
            Ok(())
        })
        .unwrap();
        assert!(found);
        assert_eq!(crate::decode(&file).unwrap(), crate::decode(&new).unwrap());
    }
}

#[cfg(all(test, feature = "encode", feature = "std"))]
mod v4 {
    use super::*;
    #[test]
    fn historical_v2_plane_set_migrates_native_levels_and_attachments() {
        let input = include_bytes!("../../../fuzz/seeds/34-seed-v2-plane-directory.imi");
        let output = crate::v4::upgrade_image(input, &Default::default()).unwrap();
        assert_eq!(
            crate::decode(input).unwrap(),
            crate::decode(&output).unwrap()
        );
        let mut before = PlaneSetReader::open(SliceReader::new(input), Default::default()).unwrap();
        let mut after =
            PlaneSetReader::open(SliceReader::new(&output), Default::default()).unwrap();
        assert_eq!(before.planes(), after.planes());
        assert_eq!(before.attachments(), after.attachments());
        for (i, plane) in before.planes().to_vec().iter().enumerate() {
            let a = before.plane_id(i).unwrap();
            let b = after.plane_id(i).unwrap();
            for (l, level) in plane.levels().iter().enumerate() {
                let stride = level.width() as usize * plane.sample_type().bytes() + 13;
                let mut x = alloc::vec![0xa7; stride * level.height() as usize];
                let mut y = x.clone();
                let rect = Rect::new(0, 0, level.width(), level.height());
                before
                    .decode_region(a, l as u32, rect, &mut x, stride)
                    .unwrap();
                after
                    .decode_region(b, l as u32, rect, &mut y, stride)
                    .unwrap();
                assert_eq!(x, y);
            }
        }
        for i in 0..before.attachments().len() {
            let a = before.attachment_id(i).unwrap();
            let b = after.attachment_id(i).unwrap();
            assert_eq!(
                before.read_attachment(a).unwrap(),
                after.read_attachment(b).unwrap()
            );
        }
    }

    #[test]
    fn migration_rebuilds_plane_offsets_checksums_and_retains_attachments() {
        for checksum in [false, true] {
            for coding in [
                crate::ImiEncoderConfig::lossless(),
                crate::ImiEncoderConfig::lossy(73.0),
                crate::ImiEncoderConfig::lossy(73.0)
                    .tune_lossy(|t| t.entropy = crate::LossyEntropy::SplitHuf),
            ] {
                let info = crate::ImageInfo::new(41, 31, Channels::Rgb, SampleType::U8);
                let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
                    .map(|i| (i * 73 + i / 7 * 41) as u8)
                    .collect();
                let primary = crate::ImiEncoderConfig::lossless()
                    .with_checksum(checksum)
                    .encode_samples(crate::ImageRef::packed(&pixels, &info), &Default::default())
                    .unwrap();
                let mut builder = PlaneSetBuilder::new(&primary).unwrap();
                let plane = builder
                    .add_plane(PlaneDescriptor::new("detail", SampleType::U8))
                    .unwrap();
                let source: Vec<u8> = (0..49 * 37).map(|i| (i * 31 + i / 13) as u8).collect();
                builder
                    .add_level(
                        plane,
                        LevelDescriptor::new(49, 37, 17, 19),
                        &source,
                        49,
                        coding,
                    )
                    .unwrap();
                let attachment = builder
                    .add_attachment(
                        AttachmentDescriptor::new("note", "application/octet-stream"),
                        b"opaque payload",
                    )
                    .unwrap();
                builder.extra_records.push((98, alloc::vec![0xa5; 256]));
                let native = builder.finish().unwrap();
                let migrated = crate::v4::upgrade_image(&native, &Default::default()).unwrap();
                assert_eq!(crate::header(&migrated).unwrap().version, 4);
                assert_eq!(
                    crate::decode(&native).unwrap(),
                    crate::decode(&migrated).unwrap()
                );
                crate::v4::region_tests::check(&migrated, &pixels, &info);
                let mut old =
                    PlaneSetReader::open(SliceReader::new(&native), Default::default()).unwrap();
                let mut new =
                    PlaneSetReader::open(SliceReader::new(&migrated), Default::default()).unwrap();
                assert_eq!(old.planes(), new.planes());
                assert_eq!(old.attachments(), new.attachments());
                assert_eq!(
                    old.read_attachment(attachment).unwrap(),
                    new.read_attachment(attachment).unwrap()
                );
                for rect in [
                    Rect::new(0, 0, 49, 37),
                    Rect::new(15, 17, 21, 13),
                    Rect::new(48, 36, 1, 1),
                ] {
                    let stride = rect.width() as usize + 11;
                    let mut a = alloc::vec![0xa7; stride * rect.height() as usize];
                    let mut b = a.clone();
                    old.decode_region(plane, 0, rect, &mut a, stride).unwrap();
                    new.decode_region(plane, 0, rect, &mut b, stride).unwrap();
                    assert_eq!(a, b);
                }
                assert_eq!(
                    crate::v4::upgrade_image(&migrated, &Default::default()).unwrap(),
                    migrated
                );
                // A tail-indexed V4 primary can also be extended directly.
                let v4_primary = crate::v4::upgrade_image(&primary, &Default::default()).unwrap();
                let direct = PlaneSetBuilder::new(&v4_primary).unwrap().finish().unwrap();
                assert_eq!(crate::decode(&direct).unwrap().1, pixels);
            }
        }
    }
}
