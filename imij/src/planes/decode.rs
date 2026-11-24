//! Footer-indexed reads; native tile decoding is admitted only after bounds.
use super::*;
use crate::parse::chunks::{self, Kind, Rules};

/// Reads only the footer and bounded index at open, then requested records.
/// The source must remain immutable while this reader is used. Opening checks
/// directory structure; unrequested tile payloads are deliberately not read.
pub struct PlaneSetReader<R: ReadAt> {
    pub(super) source: R,
    pub(super) dir: Directory,
    pub(super) limits: PlaneSetLimits,
    check: Option<PlaneCheck>,
    remaining_read_bytes: Option<u64>,
    pub(super) page_cache: Option<(usize, Vec<Record>)>,
    pub(super) page_read_bytes: u64,
}
impl<R: ReadAt> PlaneSetReader<R> {
    /// A `cSUM` file checksum sits in the primary, before its `DATA` chunk;
    /// it is found through the primary's chunk framing. By default each
    /// fetched range (the index at open, then tile records and attachment
    /// ranges) verifies every piece it overlaps before its bytes are used,
    /// reading the rest of those pieces through a bounded buffer, so every
    /// byte the reader uses is verified;
    /// [`PlaneSetLimits::with_checksum_round_out`]`(false)` checks only the
    /// pieces wholly inside each range ([`Self::checksum_coverage`] reports
    /// what that leaves unverified) and
    /// [`PlaneSetLimits::with_checksum_verification`] skips the table.
    ///
    /// The reader validates the bytes it reads (SPEC 1.1): with a checksum
    /// to verify it walks the primary's chunks to the table and applies the
    /// rules their headers and first payload bytes show; without one it
    /// does not read the primary's chunks, so it does not validate them (a
    /// primary that full decoders refuse can still serve its tiles).
    pub fn open(mut source: R, limits: PlaneSetLimits) -> Result<Self, Error> {
        let len = source.len()?;
        Self::open_with_length(source, len, limits)
    }

    /// Open using the length of the same immutable source snapshot.
    /// The caller must bind the supplied length to the source version.
    pub fn open_with_length(
        mut source: R,
        len: u64,
        limits: PlaneSetLimits,
    ) -> Result<Self, Error> {
        if len > limits.max_file_bytes {
            return Err(Error::TooLarge);
        }
        let at = len
            .checked_sub(wire::FOOTER_LEN)
            .filter(|&n| n >= 8)
            .ok_or(Error::Corrupt("short plane file"))?;
        // The signature and, usually, the whole header in one read.
        let mut head = [0u8; 64];
        let hn = at.min(head.len() as u64) as usize;
        let mut footer = [0u8; 37];
        // These extents are independent once length is supplied. A remote
        // source can submit both immediately instead of serializing discovery.
        source.read_many_at(&mut [(0, &mut head[..hn]), (at, &mut footer)])?;
        if head[..8] != crate::parse::header::SIGNATURE {
            return Err(Error::Corrupt("plane primary signature"));
        }
        let (offset, size) = wire::locator(&footer, len)?;
        if size > (limits.max_index_bytes as u64).saturating_add(14) {
            return Err(Error::TooLarge);
        }
        let check = PlaneCheck::open(&mut source, &limits, len, &head[..hn])?;
        let n = usize::try_from(size).map_err(|_| Error::TooLarge)?;
        let mut packed = buffer(n)?;
        let mut r = Self {
            source,
            dir: Directory {
                images: None,
                pages: None,
                primary_len: 0,
                records: Vec::new(),
                planes: Vec::new(),
                attachments: Vec::new(),
            },
            limits,
            check,
            remaining_read_bytes: None,
            page_cache: None,
            page_read_bytes: 0,
        };
        r.read_at(offset, &mut packed)?;
        let payload = wire::payload(&packed, &wire::INDEX)?;
        if payload.len() > limits.max_index_bytes {
            return Err(Error::TooLarge);
        }
        let raw = crate::metadata_pack::unpack(payload, limits.max_index_bytes)?;
        r.dir = wire::decode(
            &raw,
            limits,
            offset,
            crate::parse::header::header_version(&head[..hn])?,
        )?;
        Ok(r)
    }

    pub fn has_record_pages(&self) -> bool {
        self.dir.pages.is_some()
    }
    pub(super) fn record_by_id(&mut self, id: u32) -> Result<Record, Error> {
        let Some(root) = &self.dir.pages else {
            return self
                .dir
                .records
                .get(id as usize)
                .copied()
                .ok_or(Error::Corrupt("plane record ID"));
        };
        if id >= root.count {
            return Err(Error::Corrupt("plane record ID"));
        }
        let page = (id / root.per_page) as usize;
        let within = (id % root.per_page) as usize;
        if self
            .page_cache
            .as_ref()
            .is_none_or(|(cached, _)| *cached != page)
        {
            let descriptor = root.pages[page].clone();
            let count = wire::chunk_len(descriptor.len)?;
            self.page_read_bytes = self
                .page_read_bytes
                .checked_add(count)
                .ok_or(Error::TooLarge)?;
            if self.page_read_bytes > self.limits.max_compressed_work {
                return Err(Error::TooLarge);
            }
            self.page_cache = None;
            let mut framed = buffer(usize::try_from(count).map_err(|_| Error::TooLarge)?)?;
            self.read_at(descriptor.offset, &mut framed)?;
            let records =
                self.dir
                    .pages
                    .as_ref()
                    .unwrap()
                    .decode_page(page, &framed, self.limits)?;
            self.page_cache = Some((page, records));
        }
        self.page_cache
            .as_ref()
            .unwrap()
            .1
            .get(within)
            .copied()
            .ok_or(Error::Corrupt("plane page record ID"))
    }

    /// Small independent directory reads can use one source batch.
    /// The ordinary single-page path handles larger plans and enclosing cSUM.
    pub(super) fn read_region_pages(
        &mut self,
        spans: &[core::ops::Range<u64>],
    ) -> Result<Vec<(usize, Vec<Record>)>, Error> {
        let Some(root) = &self.dir.pages else {
            return Ok(Vec::new());
        };
        if self.check.is_some() || !(2..=8).contains(&spans.len()) {
            return Ok(Vec::new());
        }
        let mut selected = Vec::new();
        selected
            .try_reserve_exact(spans.len())
            .map_err(|_| Error::TooLarge)?;
        let (mut fetched, mut working) = (0u64, 0u64);
        for span in spans {
            let page = root
                .pages
                .binary_search_by_key(&span.start, |p| p.offset)
                .map_err(|_| Error::Corrupt("native range page extent"))?;
            if self
                .page_cache
                .as_ref()
                .is_some_and(|(cached, _)| *cached == page)
            {
                continue;
            }
            let bytes = span.end - span.start;
            let records = (root.count - page as u32 * root.per_page).min(root.per_page);
            fetched = fetched.checked_add(bytes).ok_or(Error::TooLarge)?;
            working = working
                .checked_add(u64::from(records) * core::mem::size_of::<Record>() as u64)
                .ok_or(Error::TooLarge)?;
            selected.push((page, span.start, bytes));
        }
        if selected.len() < 2 || working + fetched > self.limits.max_index_bytes as u64 {
            return Ok(Vec::new());
        }
        if let Some(remaining) = &mut self.remaining_read_bytes {
            *remaining = remaining.checked_sub(fetched).ok_or(Error::TooLarge)?;
        }
        let mut buffers = Vec::new();
        buffers
            .try_reserve_exact(selected.len())
            .map_err(|_| Error::TooLarge)?;
        for &(_, _, bytes) in &selected {
            buffers.push(buffer(
                usize::try_from(bytes).map_err(|_| Error::TooLarge)?,
            )?);
        }
        let mut requests = Vec::new();
        requests
            .try_reserve_exact(selected.len())
            .map_err(|_| Error::TooLarge)?;
        for ((_, at, _), bytes) in selected.iter().zip(&mut buffers) {
            requests.push((*at, bytes.as_mut_slice()));
        }
        self.source.read_many_at(&mut requests)?;
        let mut result = Vec::new();
        result
            .try_reserve_exact(selected.len())
            .map_err(|_| Error::TooLarge)?;
        for ((page, _, _), bytes) in selected.into_iter().zip(buffers) {
            result.push((page, root.decode_page(page, &bytes, self.limits)?));
        }
        Ok(result)
    }
    pub(super) fn payload_len(&self, id: u32, record: Record) -> Result<u64, Error> {
        record
            .len
            .checked_sub(if self.dir.pages.is_some() {
                crate::varint_len(id as u64) as u64
            } else {
                0
            })
            .ok_or(Error::Corrupt("bound plane record length"))
    }

    /// What the reader has verified of the bytes it fetched so far (SPEC
    /// 1.4): the index at open, then the records and attachment ranges it
    /// read. Under the default piece policy every fetched byte of a file
    /// with a checksum it checks is verified.
    pub fn checksum_coverage(&self) -> ChecksumCoverage {
        match &self.check {
            Some(c) => ChecksumCoverage {
                checking: true,
                verified_bytes: c.verified,
                unverified_bytes: c.unverified,
            },
            None => ChecksumCoverage::default(),
        }
    }

    /// Reads `out` at file offset `at` and checks the file checksum pieces
    /// it covers.
    pub(super) fn read_at(&mut self, at: u64, out: &mut [u8]) -> Result<(), Error> {
        let count = out.len() as u64;
        let check = self
            .check
            .as_ref()
            .map(|check| check.plan(at, count))
            .transpose()?;
        if let Some(remaining) = &mut self.remaining_read_bytes {
            let reads = count
                .checked_add(check.as_ref().map_or(0, |check| check.extra_bytes))
                .ok_or(Error::TooLarge)?;
            *remaining = remaining.checked_sub(reads).ok_or(Error::TooLarge)?;
        }
        self.source.read_exact_at(at, out)?;
        if let (Some(state), Some(check)) = (&mut self.check, check) {
            state.verify_read(&mut self.source, out, check)?;
        }
        Ok(())
    }

    pub(super) fn with_read_request<T>(
        &mut self,
        read: impl FnOnce(&mut Self) -> Result<T, Error>,
    ) -> Result<T, Error> {
        if self.remaining_read_bytes.is_some() {
            return read(self);
        }
        self.remaining_read_bytes = Some(self.limits.max_compressed_work);
        let result = read(self);
        self.remaining_read_bytes = None;
        result
    }
    pub fn planes(&self) -> &[PlaneDescriptor] {
        &self.dir.planes
    }

    fn backing_precision(
        &self,
        record_start: u32,
        channels: Channels,
    ) -> Result<Option<[u8; 4]>, Error> {
        let mut bits = [0u8; 4];
        let mut mask = 0u8;
        // Once per ROI request, not per decoded tile. Index validation already
        // bounds all levels and checks complete, compatible shared backings.
        for plane in &self.dir.planes {
            for level in &plane.levels {
                if level.record_start == record_start {
                    bits[level.component as usize] = plane.precision;
                    mask |= 1 << level.component;
                }
            }
        }
        if mask != (1 << channels.count()) - 1 {
            return Err(Error::Corrupt("incomplete plane precision backing"));
        }
        Ok(Some(bits))
    }
    pub fn attachments(&self) -> &[AttachmentDescriptor] {
        &self.dir.attachments
    }
    pub fn plane_id(&self, index: usize) -> Option<PlaneId> {
        self.dir.planes.get(index).map(|_| PlaneId(index as u32))
    }
    pub fn attachment_id(&self, index: usize) -> Option<AttachmentId> {
        self.dir
            .attachments
            .get(index)
            .map(|_| AttachmentId(index as u32))
    }
    /// The effective science records of a plane (SPEC.md 1.5): for each
    /// kind, the plane's own record if it has one, else the plane set's
    /// default; empty when the plane set has none. Reads only the science
    /// attachment (MIME type `application/x.imij-science`). Malformed
    /// records are [`Error::Corrupt`]; tile decoding never reads them.
    pub fn science(&mut self, id: PlaneId) -> Result<ScienceRecords, Error> {
        if id.0 as usize >= self.dir.planes.len() {
            return Err(Error::InvalidInput("plane ID"));
        }
        let mut found = None;
        for (i, a) in self.dir.attachments.iter().enumerate() {
            if a.mime == crate::science::PLANE_SET_MIME {
                if found.is_some() {
                    return Err(Error::Corrupt("repeated science attachment"));
                }
                found = Some(AttachmentId(i as u32));
            }
        }
        let Some(attachment) = found else {
            return Ok(ScienceRecords::default());
        };
        let bytes = self.read_attachment(attachment)?;
        let planes = u32::try_from(self.dir.planes.len()).map_err(|_| Error::TooLarge)?;
        crate::science::decode_plane(
            &bytes,
            planes,
            id.0,
            Some(self.dir.planes[id.0 as usize].sample),
        )
    }
    /// The fallback may be a thumbnail; ROI decoding applies to tiled levels.
    pub fn primary_len(&self) -> u64 {
        self.dir.primary_len
    }
    pub fn into_inner(self) -> R {
        self.source
    }

    /// Decode a scalar component, extracting it from a shared color tile when
    /// needed. Padding bytes remain untouched. On corrupt later tiles earlier
    /// output is provisional; discard it if this method returns an error.
    pub fn decode_region(
        &mut self,
        id: PlaneId,
        level: u32,
        rect: Rect,
        out: &mut [u8],
        stride: usize,
    ) -> Result<(), Error> {
        self.decode_components(&[(id, level)], rect, out, stride, false)
    }
    /// Decode one shared GrayAlpha/RGB/RGBA backing once into interleaved output.
    /// IDs must describe all components in their native order at the same level
    /// index. Individual planes may have unrelated other level arrangements.
    pub fn decode_color_region(
        &mut self,
        ids: &[PlaneId],
        level: u32,
        rect: Rect,
        out: &mut [u8],
        stride: usize,
    ) -> Result<(), Error> {
        if ids.len() > 4 {
            return Err(Error::InvalidInput("color ROI component count"));
        }
        let mut components = [(PlaneId(0), level); 4];
        for (entry, &id) in components.iter_mut().zip(ids) {
            entry.0 = id;
        }
        self.decode_components(&components[..ids.len()], rect, out, stride, true)
    }
    /// As `decode_color_region`, with each component's own level index. This
    /// supports shared packed levels appended after different scalar levels.
    pub fn decode_color_levels(
        &mut self,
        components: &[(PlaneId, u32)],
        rect: Rect,
        out: &mut [u8],
        stride: usize,
    ) -> Result<(), Error> {
        self.decode_components(components, rect, out, stride, true)
    }
    fn decode_components(
        &mut self,
        components: &[(PlaneId, u32)],
        rect: Rect,
        out: &mut [u8],
        stride: usize,
        color: bool,
    ) -> Result<(), Error> {
        self.with_read_request(|reader| {
            reader.decode_components_inner(components, rect, out, stride, color)
        })
    }

    fn decode_components_inner(
        &mut self,
        components: &[(PlaneId, u32)],
        rect: Rect,
        out: &mut [u8],
        stride: usize,
        color: bool,
    ) -> Result<(), Error> {
        let (id, level) = *components
            .first()
            .ok_or(Error::InvalidInput("empty color ROI"))?;
        let p = self
            .dir
            .planes
            .get(id.0 as usize)
            .ok_or(Error::InvalidInput("plane ID"))?;
        let l = p
            .levels
            .get(level as usize)
            .ok_or(Error::InvalidInput("plane level"))?
            .clone();
        let sample = p.sample;
        let cicp = p.cicp;
        if color {
            if components.len() != l.channels.count() {
                return Err(Error::InvalidInput("color ROI component count"));
            }
            for (component, &(other, other_level)) in components.iter().enumerate() {
                let op = self
                    .dir
                    .planes
                    .get(other.0 as usize)
                    .ok_or(Error::InvalidInput("plane ID"))?;
                let ol = op
                    .levels
                    .get(other_level as usize)
                    .ok_or(Error::InvalidInput("plane level"))?;
                if op.sample != sample
                    || ol.component as usize != component
                    || ol.record_start != l.record_start
                    || (
                        ol.width,
                        ol.height,
                        ol.tile_width,
                        ol.tile_height,
                        ol.channels,
                        ol.coding,
                    ) != (
                        l.width,
                        l.height,
                        l.tile_width,
                        l.tile_height,
                        l.channels,
                        l.coding,
                    )
                {
                    return Err(Error::InvalidInput("color ROI backing mismatch"));
                }
            }
        } else if components.len() != 1 {
            return Err(Error::InvalidInput("scalar ROI component count"));
        }
        let area = region_plan::TileArea::new(&l, rect)?;
        let row = sample_bytes(rect.width, 1, sample)?
            .checked_mul(components.len())
            .ok_or(Error::TooLarge)?;
        let end = stride
            .checked_mul(rect.height as usize - 1)
            .and_then(|n| n.checked_add(row))
            .ok_or(Error::TooLarge)?;
        if stride < row || out.len() < end {
            return Err(Error::InvalidInput("ROI output stride/buffer"));
        }
        if (row as u64)
            .checked_mul(u64::from(rect.height))
            .is_none_or(|n| n > self.limits.max_decoded_work)
        {
            return Err(Error::TooLarge);
        }
        let (x0, x1, y0, y1) = (area.x0, area.x1, area.y0, area.y1);
        let nx = area.columns;
        self.page_read_bytes = 0;
        let paged = self.dir.pages.is_some();
        let mut planned = Vec::new();
        if paged {
            let count = u64::from(x1 - x0 + 1) * u64::from(y1 - y0 + 1);
            let bytes = count
                .checked_mul(core::mem::size_of::<Record>() as u64)
                .ok_or(Error::TooLarge)?;
            if bytes > self.limits.max_index_bytes as u64 {
                return Err(Error::TooLarge);
            }
            planned
                .try_reserve_exact(usize::try_from(count).map_err(|_| Error::TooLarge)?)
                .map_err(|_| Error::TooLarge)?;
        }
        let mut compressed = 0u64;
        let mut decoded = 0u64;
        // Complete request accounting precedes reads, native allocations and
        // output mutation. Shared color backing is charged once per tile.
        for ty in y0..=y1 {
            for tx in x0..=x1 {
                let ri = (u64::from(ty) * u64::from(nx) + u64::from(tx)) as usize;
                let id = l
                    .record_start
                    .checked_add(u32::try_from(ri).map_err(|_| Error::TooLarge)?)
                    .ok_or(Error::TooLarge)?;
                let rec = self.record_by_id(id)?;
                if self.payload_len(id, rec)? > self.limits.max_record_bytes {
                    return Err(Error::TooLarge);
                }
                if paged {
                    planned.push(rec)
                }
                compressed = compressed
                    .checked_add(wire::chunk_len(rec.len)?)
                    .ok_or(Error::TooLarge)?;
                let (w, h) = (
                    l.tile_width.min(l.width - tx * l.tile_width),
                    l.tile_height.min(l.height - ty * l.tile_height),
                );
                let n = sample_bytes(w, h, sample)?
                    .checked_mul(l.channels.count())
                    .ok_or(Error::TooLarge)?;
                decoded = decoded.checked_add(n as u64).ok_or(Error::TooLarge)?;
            }
        }
        if compressed
            .checked_add(self.page_read_bytes)
            .is_none_or(|v| v > self.limits.max_compressed_work)
            || decoded > self.limits.max_decoded_work
        {
            return Err(Error::TooLarge);
        }
        // Copy small immutable descriptor values; never clone its index arrays.
        let (width, height, tw, th, channels, coding, component) = (
            l.width,
            l.height,
            l.tile_width,
            l.tile_height,
            l.channels,
            l.coding,
            l.component,
        );
        let precision = self.backing_precision(l.record_start, channels)?;
        let mut batch = Vec::new();
        let mut pixels_scratch = Vec::new();
        let mut batch_at = 0;
        let mut batch_end = 0;
        for ty in y0..=y1 {
            for tx in x0..=x1 {
                let ri = (u64::from(ty) * u64::from(nx) + u64::from(tx)) as usize;
                let record_id = l
                    .record_start
                    .checked_add(u32::try_from(ri).map_err(|_| Error::TooLarge)?)
                    .ok_or(Error::TooLarge)?;
                let ordinal = (ty - y0) as usize * (x1 - x0 + 1) as usize + (tx - x0) as usize;
                let owned;
                let data = if paged {
                    let rec = planned[ordinal];
                    let len = wire::chunk_len(rec.len)?;
                    if ordinal == batch_end {
                        batch_at = rec.offset;
                        batch_end = ordinal + 1;
                        let mut total = len;
                        for next in &planned[batch_end..] {
                            if next.offset != batch_at.checked_add(total).ok_or(Error::TooLarge)? {
                                break;
                            }
                            let extended = total
                                .checked_add(wire::chunk_len(next.len)?)
                                .ok_or(Error::TooLarge)?;
                            if extended > self.limits.max_record_bytes {
                                break;
                            }
                            total = extended;
                            batch_end += 1;
                        }
                        let total = usize::try_from(total).map_err(|_| Error::TooLarge)?;
                        batch
                            .try_reserve_exact(total.saturating_sub(batch.len()))
                            .map_err(|_| Error::TooLarge)?;
                        batch.resize(total, 0);
                        self.read_at(batch_at, &mut batch)?;
                    }
                    let at = usize::try_from(rec.offset - batch_at).map_err(|_| Error::TooLarge)?;
                    let end = at
                        .checked_add(usize::try_from(len).map_err(|_| Error::TooLarge)?)
                        .ok_or(Error::TooLarge)?;
                    let framed = batch
                        .get(at..end)
                        .ok_or(Error::Corrupt("plane record span"))?;
                    record_payload(record_id, framed, true)?
                } else {
                    let rec = self.dir.records[record_id as usize];
                    owned = self.record(record_id, rec, self.limits.max_record_bytes)?;
                    &owned
                };
                let (x, y) = (tx * tw, ty * th);
                let (w, h) = (tw.min(width - x), th.min(height - y));
                let n = sample_bytes(w, h, sample)?
                    .checked_mul(channels.count())
                    .ok_or(Error::TooLarge)?;
                let limits = crate::Limits {
                    max_pixels: u64::from(w) * u64::from(h),
                    max_bytes: n as u64,
                    verify_checksum: self.limits.verify_checksum,
                };
                let verify_pixels = self.limits.verify_scoped_pixels;
                let session = crate::decode_resources::DecodeSession::new(
                    crate::decode_resources::DecodePolicy::default()
                        .with_output_limits(limits)
                        .with_scoped_pixel_checks(verify_pixels, verify_pixels),
                );
                let _input = if crate::parse::header::header_version(data)? == 4 {
                    session.admit_input(data)?;
                    Some(session.v4_precharged()?)
                } else {
                    None
                };
                let Profile {
                    header,
                    declared,
                    payload,
                } = native_profile_in_session(
                    data, w, h, channels, sample, coding, cicp, precision, &session,
                )?;
                crate::parse::header::check_decodable(&header, declared)?;
                crate::parse::limits::check_limits(&header.info, &limits)?;
                #[cfg(feature = "v3")]
                let legacy_pixels;
                let (info, pixels) = match payload {
                    TilePayload::V4(file) => {
                        // Publish cropped rows only after the entire tile has passed its
                        // pixel checks. Reuse the bounded tile raster across this ROI.
                        if pixels_scratch.capacity() == 0 {
                            pixels_scratch = bytemuck::allocation::try_zeroed_vec(n)
                                .map_err(|_| Error::TooLarge)?;
                        } else {
                            pixels_scratch
                                .try_reserve_exact(n.saturating_sub(pixels_scratch.len()))
                                .map_err(|_| Error::TooLarge)?;
                            pixels_scratch.resize(n, 0);
                        }
                        let row = sample_bytes(w, 1, sample)? * channels.count();
                        let info = crate::v4::decode::parsed_into(
                            data,
                            file,
                            &mut pixels_scratch,
                            row,
                            &session,
                            self.limits.decode_workers,
                        )?;
                        (info, pixels_scratch.as_slice())
                    }
                    #[cfg(feature = "v3")]
                    TilePayload::Legacy(body) => {
                        let (info, pixels) = crate::v1::decode_body_with_limits(
                            header.info,
                            header.version,
                            header.lossy_color,
                            declared,
                            body,
                            &limits.with_checksum_verification(false),
                        )?;
                        legacy_pixels = pixels;
                        (info, legacy_pixels.as_slice())
                    }
                };
                if info.width != w
                    || info.height != h
                    || info.channels != channels
                    || info.sample != sample
                    || pixels.len() != n
                {
                    return Err(Error::Corrupt("decoded plane tile geometry"));
                }
                let (sx, sy, xe, ye) = (
                    x.max(rect.x),
                    y.max(rect.y),
                    (x + w).min(area.end_x),
                    (y + h).min(area.end_y),
                );
                let b = sample.bytes();
                let source_pixel = b * channels.count();
                let output_pixel = b * components.len();
                for iy in sy..ye {
                    let dest =
                        (iy - rect.y) as usize * stride + (sx - rect.x) as usize * output_pixel;
                    let src = ((iy - y) as usize * w as usize + (sx - x) as usize) * source_pixel;
                    let count = (xe - sx) as usize;
                    if color {
                        out[dest..dest + count * source_pixel]
                            .copy_from_slice(&pixels[src..src + count * source_pixel]);
                    } else {
                        for ix in 0..count {
                            let at = src + ix * source_pixel + component as usize * b;
                            out[dest + ix * b..dest + (ix + 1) * b]
                                .copy_from_slice(&pixels[at..at + b]);
                        }
                    }
                }
            }
        }
        Ok(())
    }
    pub fn attachment_len(&self, id: AttachmentId) -> Result<u64, Error> {
        let a = self
            .dir
            .attachments
            .get(id.0 as usize)
            .ok_or(Error::InvalidInput("attachment ID"))?;
        if let Some(root) = &self.dir.pages {
            root.attachment_len(a.record)
                .ok_or(Error::Corrupt("plane attachment length declaration"))
        } else {
            Ok(self.dir.records[a.record as usize].len)
        }
    }
    /// Copies only a requested byte range of an opaque attachment. No whole
    /// clip allocation or parsing occurs; work bounds apply to this range.
    pub fn read_attachment_into_range(
        &mut self,
        id: AttachmentId,
        offset: u64,
        out: &mut [u8],
    ) -> Result<(), Error> {
        self.with_read_request(|reader| reader.read_attachment_range_inner(id, offset, out))
    }

    fn read_attachment_range_inner(
        &mut self,
        id: AttachmentId,
        offset: u64,
        out: &mut [u8],
    ) -> Result<(), Error> {
        let a = self
            .dir
            .attachments
            .get(id.0 as usize)
            .ok_or(Error::InvalidInput("attachment ID"))?;
        let record_id = a.record;
        let length = self.attachment_len(id)?;
        self.page_read_bytes = 0;
        let rec = self.record_by_id(record_id)?;
        if offset
            .checked_add(out.len() as u64)
            .is_none_or(|n| n > length)
        {
            return Err(Error::InvalidInput("attachment range"));
        }
        if out.len() as u64 > self.limits.max_decoded_work
            || (out.len() as u64)
                .checked_add(19 + self.page_read_bytes)
                .is_none_or(|n| n > self.limits.max_compressed_work)
        {
            return Err(Error::TooLarge);
        }
        let start = self.record_start(record_id, rec)?;
        self.read_at(start + offset, out)
    }
    /// Convenience whole-attachment copy, subject to per-request work limits.
    pub fn read_attachment(&mut self, id: AttachmentId) -> Result<Vec<u8>, Error> {
        self.with_read_request(|reader| reader.read_attachment_inner(id))
    }

    fn read_attachment_inner(&mut self, id: AttachmentId) -> Result<Vec<u8>, Error> {
        let n = self.attachment_len(id)?;
        if n > self.limits.max_decoded_work
            || n.checked_add(14)
                .is_none_or(|n| n > self.limits.max_compressed_work)
        {
            return Err(Error::TooLarge);
        }
        let record_id = self.dir.attachments[id.0 as usize].record;
        self.page_read_bytes = 0;
        let rec = self.record_by_id(record_id)?;
        if n.checked_add(19 + self.page_read_bytes)
            .is_none_or(|v| v > self.limits.max_compressed_work)
        {
            return Err(Error::TooLarge);
        }
        self.record(record_id, rec, self.limits.max_attachment_bytes)
            .map(RecordData::into_payload)
    }
    fn record_start(&mut self, id: u32, rec: Record) -> Result<u64, Error> {
        let mut expected = [0u8; 19];
        expected[..4].copy_from_slice(if self.dir.pages.is_some() {
            &record_pages::RECORD_TAG
        } else {
            &wire::DATA
        });
        let mut n = rec.len.checked_add(1).ok_or(Error::TooLarge)?;
        let mut len = 4;
        loop {
            expected[len] = (n & 127) as u8 | if n >= 128 { 128 } else { 0 };
            len += 1;
            n >>= 7;
            if n == 0 {
                break;
            }
        }
        if self.dir.pages.is_some() {
            let mut id = id;
            loop {
                expected[len] = (id & 127) as u8 | if id >= 128 { 128 } else { 0 };
                len += 1;
                id >>= 7;
                if id == 0 {
                    break;
                }
            }
        }
        let mut got = [0u8; 19];
        self.read_at(rec.offset, &mut got[..len])?;
        if got[..len] != expected[..len] {
            return Err(Error::Corrupt("plane record framing"));
        }
        rec.offset.checked_add(len as u64).ok_or(Error::TooLarge)
    }
    fn record(&mut self, id: u32, rec: Record, max: u64) -> Result<RecordData, Error> {
        let size = self.payload_len(id, rec)?;
        if size > max {
            return Err(Error::TooLarge);
        }
        let total = wire::chunk_len(rec.len)?;
        let mut bytes = buffer(usize::try_from(total).map_err(|_| Error::TooLarge)?)?;
        self.read_at(rec.offset, &mut bytes)?;
        let body = record_payload(id, &bytes, self.dir.pages.is_some())?;
        let start = body.as_ptr() as usize - bytes.as_ptr() as usize;
        Ok(RecordData { bytes, start })
    }
}

fn record_payload(id: u32, bytes: &[u8], paged: bool) -> Result<&[u8], Error> {
    let body = wire::payload(
        bytes,
        if paged {
            &record_pages::RECORD_TAG
        } else {
            &wire::DATA
        },
    )?;
    if !paged {
        return Ok(body);
    }
    let mut input = crate::ByteReader::new(body);
    if input.varint()? != u64::from(id) {
        return Err(Error::Corrupt("plane bound record identity"));
    }
    Ok(&body[input.pos..])
}
struct RecordData {
    bytes: Vec<u8>,
    start: usize,
}
impl core::ops::Deref for RecordData {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.bytes[self.start..]
    }
}
impl RecordData {
    fn into_payload(mut self) -> Vec<u8> {
        self.bytes.copy_within(self.start.., 0);
        self.bytes.truncate(self.bytes.len() - self.start);
        self.bytes
    }
}

/// Most primary chunks the indexed reader walks to find the checksum table.
const PRIMARY_CHUNKS: usize = 4096;

// The plane reader needs only the pSUM table header to check its identity.
const PRIMARY_PREFIX_BYTES: usize = 14 + 22;

fn admit_checksum_table(file_len: u64, len: u64, log2: u8, budget: u64) -> Result<(), Error> {
    crate::checksum::check_chunk_len(len, file_len)?;
    let words = (file_len - len).div_ceil(1 << log2).div_ceil(64);
    // Reading and owning the table can temporarily retain two copies.
    if u128::from(len) * 2 + u128::from(words) * 8 > u128::from(budget) {
        return Err(Error::TooLarge);
    }
    Ok(())
}

fn admit_checksum_pieces(
    table: &crate::checksum::Fetched,
    pieces: &[u64],
    budget: u64,
) -> Result<(), Error> {
    let bytes = pieces
        .iter()
        .map(|&id| {
            let piece = table.piece(id);
            u128::from(piece.end - piece.start)
        })
        .sum::<u128>();
    if bytes > u128::from(budget) {
        return Err(Error::TooLarge);
    }
    Ok(())
}

fn primary_chunk_prefix<R: ReadAt>(
    source: &mut R,
    pos: u64,
    end: u64,
) -> Result<([u8; PRIMARY_PREFIX_BYTES], usize), Error> {
    let mut frame = [0; PRIMARY_PREFIX_BYTES];
    let n = end
        .checked_sub(pos)
        .ok_or(Error::Corrupt("plane primary bounds"))?
        .min(frame.len() as u64) as usize;
    source.read_exact_at(pos, &mut frame[..n])?;
    Ok((frame, n))
}

/// What a [`PlaneSetReader`] has verified of the bytes it fetched
/// ([`PlaneSetReader::checksum_coverage`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ChecksumCoverage {
    /// The file has a checksum table this reader checks. `false` when it has
    /// none, when verification is off, or when the table's algorithm is one
    /// this reader does not know (SPEC 1.4: the table is then skipped).
    pub checking: bool,
    /// Covered bytes fetched that lie in pieces the reader verified.
    pub verified_bytes: u64,
    /// Covered bytes fetched that lie in pieces it did not verify: always 0
    /// under the default piece policy; under
    /// [`PlaneSetLimits::with_checksum_round_out`]`(false)`, the parts of
    /// pieces that no read held whole.
    pub unverified_bytes: u64,
}

/// A plane file's checksum pieces, whether a read checks every piece it
/// overlaps (`round_out`) or only those wholly inside it, and how many
/// fetched bytes each left verified or not.
struct PlaneCheck {
    pieces: crate::checksum::Fetched,
    max_work: u64,
    round_out: bool,
    verified: u64,
    unverified: u64,
}

struct ChecksumRead {
    covered: core::ops::Range<u64>,
    clear: bool,
    pending: Vec<u64>,
    extra_bytes: u64,
}
impl PlaneCheck {
    /// The table in the primary (a chunk before its `DATA`), checked against
    /// the header record. Only a file whose header record announces a table
    /// is searched (others cost no extra read): the primary's chunk framing
    /// up to the table, at most [`PRIMARY_CHUNKS`] chunks.
    fn open<R: ReadAt>(
        source: &mut R,
        limits: &PlaneSetLimits,
        file_len: u64,
        head: &[u8],
    ) -> Result<Option<PlaneCheck>, Error> {
        if !limits.verify_checksum {
            return Ok(None);
        }
        let mut r = crate::ByteReader::new(&head[8..]);
        let hl = r.var_usize()?;
        if hl > 64 << 10 {
            return Err(Error::TooLarge);
        }
        let need = 8 + r.pos + hl;
        let longer;
        let h = if need <= head.len() {
            &head[..need]
        } else {
            let mut v = buffer(need)?;
            source.read_exact_at(0, &mut v)?;
            longer = v;
            &longer[..]
        };
        let hp = crate::parse::header::read_header_sized(&mut crate::ByteReader::new(h))?;
        let Some(announced) = hp.checksum else {
            return Ok(None);
        };
        let announced = announced.announced;
        // The primary's chunks up to its DATA, under the chunk rules every
        // reader applies (their first bytes only: tiles are read through
        // the index): (offset, total length, framing length) of the cSUM
        // chunk among them.
        let mut rules = Rules::new(hp.mixed_pages(), true).with_sequence_items(
            hp.declared
                .contains(crate::parse::header::Feature::SequenceItems),
        );
        let end = file_len - wire::FOOTER_LEN;
        let mut pos = need as u64;
        let mut found = None;
        let mut steps = 0;
        // A rule error with the table read before it: a mismatching piece
        // under the chunks read explains it (SPEC 1.4).
        let walked = (|| -> Result<(), Error> {
            loop {
                steps += 1;
                if steps > PRIMARY_CHUNKS {
                    return Err(Error::TooLarge);
                }
                if pos >= end {
                    return Err(Error::Corrupt("plane primary without DATA"));
                }
                let (frame, n) = primary_chunk_prefix(source, pos, end)?;
                let mut f = crate::ByteReader::new(&frame[..n]);
                let (tag, len) = chunks::read_head(&mut f)?;
                let kind = rules.head(tag, len)?;
                if kind == Kind::Data {
                    break;
                }
                if kind.has_body_rules(&tag) {
                    let want = chunks::prefix_len(&tag, len) as usize;
                    rules.identity(tag, &frame[f.pos..n][..want.min(n - f.pos)])?;
                }
                // The rules refuse every other open chunk.
                let len = len.unwrap_or(0);
                let total = (f.pos as u64).checked_add(len).ok_or(Error::TooLarge)?;
                if kind == Kind::Checksum {
                    found = Some((pos, total, f.pos));
                }
                pos = pos.checked_add(total).ok_or(Error::TooLarge)?;
            }
            Ok(())
        })();
        if let Err(e) = walked {
            let Some((at, len, _)) = found else {
                return Err(e);
            };
            let to = pos
                .saturating_add(PRIMARY_PREFIX_BYTES as u64)
                .min(file_len);
            if to > limits.max_compressed_work {
                return Err(e);
            }
            if announced
                .log2()
                .and_then(|log2| {
                    admit_checksum_table(file_len, len, log2, limits.max_compressed_work)
                })
                .is_err()
            {
                return Err(e);
            }
            let source = core::cell::RefCell::new(source);
            return Err(crate::checksum::explain_fetched(
                e,
                at,
                len,
                announced.log2(),
                announced.algorithm_id(),
                file_len,
                to,
                || {
                    let mut chunk = buffer(usize::try_from(len).map_err(|_| Error::TooLarge)?)?;
                    source.borrow_mut().read_exact_at(at, &mut chunk)?;
                    Ok(chunk)
                },
                |pieces, pending| {
                    admit_checksum_pieces(pieces, pending, limits.max_compressed_work)?;
                    pending
                        .iter()
                        .map(|&i| read_covered(pieces, &mut **source.borrow_mut(), i))
                        .collect()
                },
            ));
        }
        crate::checksum::presence(true, found.is_some())?;
        let (at, len, _) = found.unwrap();
        let log2 = announced.log2()?;
        admit_checksum_table(file_len, len, log2, limits.max_compressed_work)?;
        let mut pieces =
            crate::checksum::Fetched::new(at, len, log2, announced.algorithm_id()?, file_len)?;
        let mut chunk = buffer(usize::try_from(len).map_err(|_| Error::TooLarge)?)?;
        source.read_exact_at(at, &mut chunk)?;
        if !pieces.set_table(&chunk)? {
            return Ok(None);
        }
        Ok(Some(PlaneCheck {
            pieces,
            max_work: limits.max_compressed_work,
            round_out: limits.checksum_round_out,
            verified: 0,
            unverified: 0,
        }))
    }

    /// Checks the pieces a read of `out` at file offset `at` covers: those
    /// wholly inside it, and with `round_out` the rest of the ones it
    /// overlaps, fetched from `source`.
    fn plan(&self, at: u64, count: u64) -> Result<ChecksumRead, Error> {
        let end = at.checked_add(count).ok_or(Error::TooLarge)?;
        let (ca, cb) = self.pieces.chunk();
        // A fetch wholly on one side of the table maps onto covered bytes.
        let clear = end <= ca || at >= cb;
        let (a, b) = (self.pieces.covered(at), self.pieces.covered(end));
        let pending = self.pieces.pending(&[(at, end)], self.round_out)?;
        admit_checksum_pieces(&self.pieces, &pending, self.max_work)?;
        let extra_bytes = pending
            .iter()
            .map(|&id| self.pieces.piece(id))
            .filter(|piece| !clear || piece.start < a || piece.end > b)
            .map(|piece| piece.end - piece.start)
            .sum();
        Ok(ChecksumRead {
            covered: a..b,
            clear,
            pending,
            extra_bytes,
        })
    }

    fn verify_read<R: ReadAt>(
        &mut self,
        source: &mut R,
        out: &[u8],
        check: ChecksumRead,
    ) -> Result<(), Error> {
        let a = check.covered.start;
        let b = check.covered.end;
        for i in check.pending {
            let r = self.pieces.piece(i);
            let digest = if check.clear && r.start >= a && r.end <= b {
                crate::checksum::digest_of(
                    self.pieces.algo(),
                    [&out[(r.start - a) as usize..(r.end - a) as usize]],
                )
            } else {
                self.read_covered(source, i)?
            };
            self.pieces.check(i, &digest)?;
        }
        // The covered bytes of this read, by whether their piece is checked.
        if a < b {
            let k = self.pieces.log2();
            for i in a >> k..=(b - 1) >> k {
                let r = self.pieces.piece(i);
                let n = r.end.min(b) - r.start.max(a);
                if self.pieces.is_done(i) {
                    self.verified = self.verified.saturating_add(n);
                } else {
                    self.unverified = self.unverified.saturating_add(n);
                }
            }
        }
        Ok(())
    }

    /// The table's digest of piece `i`, read from `source`.
    fn read_covered<R: ReadAt>(&self, source: &mut R, i: u64) -> Result<crate::Digest, Error> {
        read_covered(&self.pieces, source, i)
    }
}

/// The table's digest of piece `i` of `pieces`, read from `source` through a
/// buffer of at most 1 MiB.
fn read_covered<R: ReadAt>(
    pieces: &crate::checksum::Fetched,
    source: &mut R,
    i: u64,
) -> Result<crate::Digest, Error> {
    let mut h = crate::Hasher::new(pieces.algo());
    let parts = pieces.segments(i);
    let longest = parts.iter().map(|r| r.1 - r.0).max().unwrap_or(0);
    let mut buf = buffer(longest.min(1 << 20) as usize)?;
    for (mut pos, end) in parts.into_iter().filter(|r| r.0 < r.1) {
        while pos < end {
            let n = (end - pos).min(buf.len() as u64) as usize;
            source.read_exact_at(pos, &mut buf[..n])?;
            h.update(&buf[..n]);
            pos += n as u64;
        }
    }
    Ok(h.finish())
}

pub(super) fn buffer(n: usize) -> Result<Vec<u8>, Error> {
    let mut v = Vec::new();
    v.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
    v.resize(n, 0);
    Ok(v)
}

struct Profile<'a> {
    header: crate::Header,
    declared: crate::parse::header::Features,
    payload: TilePayload<'a>,
}

#[cfg_attr(
    feature = "v3",
    allow(clippy::large_enum_variant, reason = "keep the parsed tile inline")
)]
enum TilePayload<'a> {
    V4(crate::container::File<'a>),
    #[cfg(feature = "v3")]
    Legacy(&'a [u8]),
}

/// Check a plane tile's descriptor, framing and supported file checksum.
#[cfg(any(feature = "std", feature = "encode"))]
#[allow(clippy::too_many_arguments)]
pub(super) fn native_profile(
    data: &[u8],
    w: u32,
    h: u32,
    channels: Channels,
    sample: SampleType,
    coding: Coding,
    cicp: Cicp,
    precision: Option<[u8; 4]>,
) -> Result<(), Error> {
    let session = crate::decode_resources::DecodeSession::new(
        crate::decode_resources::DecodePolicy::default().with_output_limits(crate::Limits::NONE),
    );
    native_profile_in_session(
        data, w, h, channels, sample, coding, cicp, precision, &session,
    )
    .map(|_| ())
}

#[allow(clippy::too_many_arguments)]
fn native_profile_in_session<'a>(
    data: &'a [u8],
    w: u32,
    h: u32,
    channels: Channels,
    sample: SampleType,
    coding: Coding,
    cicp: Cicp,
    precision: Option<[u8; 4]>,
    session: &crate::decode_resources::DecodeSession,
) -> Result<Profile<'a>, Error> {
    let mut r = crate::ByteReader::new(data);
    let crate::parse::header::ParsedHeader {
        header, declared, ..
    } = crate::parse::header::read_header_sized(&mut r)?;
    if header.info.width != w
        || header.info.height != h
        || header.info.channels != channels
        || header.info.sample != sample
        || header.coding != coding
    {
        return Err(Error::Corrupt("native plane tile descriptor mismatch"));
    }
    if header.orientation != 1
        || header.info.alpha_mode != crate::AlphaMode::Straight
        || header.info.cicp != cicp
    {
        return Err(Error::Corrupt("undeclared plane tile color/orientation"));
    }
    if let Some(bits) = precision {
        if (0..channels.count()).any(|c| header.info.channel_bits(c) != Some(bits[c])) {
            return Err(Error::Corrupt("native plane significant bits mismatch"));
        }
    } else if header.info.has_custom_precision() {
        return Err(Error::Corrupt(
            "significant bits require a version2 plane index",
        ));
    }
    // Metadata, frames, previews, nested plane sets and temporal coding remain
    // outside the tile profile. V4 can retain standalone tile checksums and
    // its directory; the complete parser validates their framing and cSUM.
    let payload = match header.version {
        #[cfg(feature = "v3")]
        version if version != 4 => {
            if r.bytes(4)? != b"DATA" {
                return Err(Error::Corrupt("ancillary data in plane tile"));
            }
            let n = r.varint()?;
            if n != 0 && n - 1 != r.remaining() as u64 {
                return Err(Error::Corrupt("plane tile DATA size"));
            }
            TilePayload::Legacy(&data[r.pos..])
        }
        _ => {
            let file = crate::v4::file::parse_file(data, session)?;
            let mut seen = false;
            while r.remaining() != 0 {
                let (tag, length) = chunks::read_head(&mut r)?;
                if tag == *b"DATA" && !seen {
                    seen = true;
                    match length {
                        Some(n) => {
                            chunks::payload(&mut r, n)?;
                        }
                        None => {
                            r.bytes(file.data.len())?;
                        }
                    }
                } else if (tag == *b"uIDX" && seen) || (!seen && matches!(&tag, b"cSUM" | b"pSUM"))
                {
                    chunks::payload(
                        &mut r,
                        length.ok_or(Error::Corrupt("open plane tile ancillary chunk"))?,
                    )?;
                } else {
                    return Err(Error::Corrupt("ancillary data in plane tile"));
                }
            }
            TilePayload::V4(file)
        }
    };
    if coding == Coding::Lossy
        && sample == SampleType::U8
        && header.lossy_color == Some(crate::LossyColor::Xyb)
        && cicp.transfer_characteristics != 13
    {
        return Err(Error::Corrupt("lossy U8 XYB plane requires sRGB transfer"));
    }
    Ok(Profile {
        header,
        declared,
        payload,
    })
}

/// The strict validator visits every unique tile, not just one requested ROI.
/// Encoded tiles stay under the explicit tile/record bounds; the enclosing file
/// and its raster are never materialized. Attachments remain opaque.
#[cfg(feature = "std")]
pub(crate) fn validate_all_tiles(
    source: &mut dyn ReadAt,
    limits: PlaneSetLimits,
    chunks: &[crate::traversal::Chunk],
    metadata_cap: usize,
    mut validate: impl FnMut(&[u8]) -> Result<(), Error>,
) -> Result<(), Error> {
    let len = source.len()?;
    let source = crate::traversal::RangeSource {
        source,
        range: 0..len,
    };
    // The outer traversal has already verified every checksum piece.
    let mut reader = PlaneSetReader::open(source, limits.with_checksum_verification(false))?;
    let mut record = 0usize;
    let mut indexes = 0;
    let mut page_chunks = 0;
    let mut locators = 0;
    let mut first = None;
    for chunk in chunks {
        match &chunk.tag {
            b"pDAT" | b"pREC" => {
                first.get_or_insert(chunk.start);
                let rec =
                    reader.record_by_id(u32::try_from(record).map_err(|_| Error::TooLarge)?)?;
                if rec.offset != chunk.start || rec.len != chunk.body.end - chunk.body.start {
                    return Err(Error::Corrupt(
                        "plane record framing differs from directory",
                    ));
                }
                if reader.dir.pages.is_some() {
                    reader.record_start(record as u32, rec)?;
                }
                record += 1;
            }
            b"pIPG" => {
                let root = reader
                    .dir
                    .pages
                    .as_ref()
                    .ok_or(Error::Corrupt("unexpected plane record page"))?;
                let page = root
                    .pages
                    .get(page_chunks)
                    .ok_or(Error::Corrupt("extra plane record page"))?;
                if page.offset != chunk.start || page.len != chunk.body.end - chunk.body.start {
                    return Err(Error::Corrupt("plane page physical extent"));
                }
                page_chunks += 1;
            }
            b"pIDX" => {
                first.get_or_insert(chunk.start);
                indexes += 1;
            }
            b"sLOC" => {
                locators += 1;
                if chunk.body.end != len {
                    return Err(Error::Corrupt("plane locator is not final"));
                }
            }
            _ if first.is_some() => {
                return Err(Error::Corrupt("primary chunk after plane records"));
            }
            _ => {}
        }
    }
    if record != reader.dir.record_count()
        || page_chunks != reader.dir.pages.as_ref().map_or(0, |p| p.pages.len())
        || indexes != 1
        || locators != 1
        || first != Some(reader.dir.primary_len)
    {
        return Err(Error::Corrupt("plane directory does not match file chunks"));
    }
    let mut visited = Vec::new();
    visited
        .try_reserve_exact(record)
        .map_err(|_| Error::TooLarge)?;
    visited.resize(record, false);
    let mut science = None;
    for (i, a) in reader.dir.attachments.iter().enumerate() {
        if a.mime == crate::science::PLANE_SET_MIME && science.replace(i).is_some() {
            return Err(Error::Corrupt("repeated science attachment"));
        }
    }
    if let Some(i) = science {
        if reader.attachment_len(AttachmentId(i as u32))? > metadata_cap as u64 {
            return Err(Error::TooLarge);
        }
        let bytes = reader.read_attachment(AttachmentId(i as u32))?;
        crate::science::validate_plane_records(&bytes, &reader.dir.planes, metadata_cap)?;
    }
    for pi in 0..reader.dir.planes.len() {
        for li in 0..reader.dir.planes[pi].levels.len() {
            let plane = &reader.dir.planes[pi];
            let level = &plane.levels[li];
            let (nx, _, count) = level.tiles()?;
            let (width, height, tw, th, channels, coding, start, sample, cicp) = (
                level.width,
                level.height,
                level.tile_width,
                level.tile_height,
                level.channels,
                level.coding,
                level.record_start,
                plane.sample,
                plane.cicp,
            );
            let precision = reader.backing_precision(start, channels)?;
            for tile in 0..count {
                let ri = start as usize + tile;
                if visited[ri] {
                    continue;
                }
                let rec = reader.record_by_id(u32::try_from(ri).map_err(|_| Error::TooLarge)?)?;
                let data = reader.record(
                    ri as u32,
                    rec,
                    limits.max_record_bytes.min(limits.max_tile_bytes),
                )?;
                let x = (tile as u64 % u64::from(nx)) as u32 * tw;
                let y = (tile as u64 / u64::from(nx)) as u32 * th;
                native_profile(
                    &data,
                    tw.min(width - x),
                    th.min(height - y),
                    channels,
                    sample,
                    coding,
                    cicp,
                    precision,
                )?;
                validate(&data)?;
                visited[ri] = true;
            }
        }
    }
    Ok(())
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::EncodeConfig;

    #[test]
    fn large_addressable_files_still_admit_checksum_tables_before_allocation() {
        let limit = PlaneSetLimits::default().max_compressed_work;
        assert_eq!(admit_checksum_table(1 << 46, 64, 40, limit), Ok(()));
        assert_eq!(
            admit_checksum_table(1 << 46, 64, 10, limit),
            Err(Error::TooLarge)
        );
        assert_eq!(
            admit_checksum_table(1 << 46, 1 << 30, 10, limit),
            Err(Error::TooLarge)
        );
        let huge_piece = check_of(0, 40, 1 << 42);
        assert_eq!(
            admit_checksum_pieces(&huge_piece.pieces, &[0], limit),
            Err(Error::TooLarge)
        );
    }

    #[test]
    fn primary_prefix_covers_every_supported_table_count_boundary() {
        for count in [1u64, 127, 128, 16383, 16384, 262143, 262144, 1_000_000] {
            let mut payload = vec![1, 0, 8, 0];
            crate::put_varint(&mut payload, count);
            let payload_len = payload.len() as u64 + count * 8;
            payload.extend_from_slice(&0u64.to_le_bytes());
            let mut bytes = b"pSUM".to_vec();
            crate::put_varint(&mut bytes, payload_len + 1);
            bytes.extend_from_slice(&payload);
            let mut source = SliceReader::new(&bytes);
            let (frame, n) = primary_chunk_prefix(&mut source, 0, bytes.len() as u64).unwrap();
            let mut reader = crate::ByteReader::new(&frame[..n]);
            let (tag, len) = chunks::read_head(&mut reader).unwrap();
            let mut rules = Rules::new(false, true);
            rules.head(tag, len).unwrap();
            let prefix = &frame[reader.pos..n];
            rules.identity(tag, prefix).unwrap();
            assert_eq!(
                crate::pixelsum::prefix(prefix, payload_len)
                    .unwrap()
                    .unwrap()
                    .0,
                count
            );
            assert!(n <= PRIMARY_PREFIX_BYTES);
        }
    }

    // A sparse positional source exposes wide offsets without allocating or
    // hashing gigabytes. Stop at the first read and verify forward progress.
    struct WideReadProbe {
        len: u64,
        expected_offset: u64,
        expected_len: usize,
        calls: usize,
    }
    impl ReadAt for WideReadProbe {
        fn len(&mut self) -> Result<u64, Error> {
            Ok(self.len)
        }
        fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
            assert_eq!(offset, self.expected_offset);
            assert_eq!(out.len(), self.expected_len);
            self.calls += 1;
            Err(Error::Read)
        }
    }

    /// A check of `covered_len` bytes in `2^k` pieces whose 16-byte table
    /// sits at `at`, every read rounded out to whole pieces.
    fn check_of(at: u64, k: u8, covered_len: u64) -> PlaneCheck {
        let mut pieces = crate::checksum::Fetched::new(
            at,
            16,
            k,
            Some(crate::HashAlgorithm::Xxh3_64.wire()),
            covered_len + 16,
        )
        .unwrap();
        pieces.set_owned_table(crate::checksum::OwnedTable {
            algo: crate::HashAlgorithm::Xxh3_64,
            width: 8,
            log2: k,
            covered_len,
            digests: vec![0; 8],
        });
        PlaneCheck {
            pieces,
            max_work: u64::MAX,
            round_out: true,
            verified: 0,
            unverified: 0,
        }
    }

    #[test]
    fn read_request_charges_checksum_work_before_the_next_source_read() {
        let info = crate::ImageInfo::new(2, 2, Channels::Gray, SampleType::U8);
        let primary = crate::encode(
            &[1, 2, 3, 4],
            2,
            &info,
            &crate::Metadata::default(),
            &EncodeConfig::default(),
        )
        .unwrap();
        let file = PlaneSetBuilder::new(&primary).unwrap().finish().unwrap();
        let mut reader =
            PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
        // Model a piece spanning a 16-byte table at offset 8. Only reads
        // wholly inside that table can succeed on this short physical source.
        let check = check_of(8, 10, 1024);
        assert_eq!(check.plan(24, 2).unwrap().extra_bytes, 1024);
        reader.check = Some(check);
        reader.limits = reader.limits.with_work_bytes(1027, u64::MAX);
        let mut bytes = [0xa7; 2];
        assert_eq!(
            reader.with_read_request(|r| {
                r.read_at(8, &mut [0; 4])?;
                r.read_at(24, &mut bytes)
            }),
            Err(Error::TooLarge)
        );
        assert_eq!(bytes, [0xa7; 2]);
        assert!(reader.remaining_read_bytes.is_none());
    }

    #[test]
    fn wide_plane_offsets_keep_bounded_reads_nonempty() {
        let source = WideReadProbe {
            len: (1u64 << 32) + wire::FOOTER_LEN,
            expected_offset: 0,
            expected_len: 64,
            calls: 0,
        };
        assert!(matches!(
            PlaneSetReader::open(source, PlaneSetLimits::default()),
            Err(Error::Read)
        ));
        for k in [32, 33, 40] {
            let check = check_of(1u64 << k, k, 1u64 << k);
            let mut source = WideReadProbe {
                len: (1u64 << k) + 16,
                expected_offset: 0,
                expected_len: 1 << 20,
                calls: 0,
            };
            assert_eq!(check.read_covered(&mut source, 0), Err(Error::Read));
            assert_eq!(source.calls, 1);
        }
    }

    #[test]
    fn round_out_reads_the_last_piece_of_a_huge_source() {
        // A source reporting a length near 2^64 (a remote size, say): round-out
        // fetches the last piece from its start; its end must not wrap.
        let k = crate::checksum::MAX_PIECE_LOG2;
        let covered_len = u64::MAX - 100;
        let pieces = covered_len.div_ceil(1 << k);
        let last = (pieces - 1) << k;
        let mut check = check_of(covered_len, k, covered_len);
        let mut source = WideReadProbe {
            len: covered_len + 16,
            expected_offset: last,
            expected_len: 1 << 20,
            calls: 0,
        };
        assert_eq!(
            check
                .plan(covered_len - 8, 8)
                .and_then(|plan| check.verify_read(&mut source, &[0u8; 8], plan)),
            Err(Error::Read)
        );
        assert_eq!(source.calls, 1);
    }

    #[test]
    fn native_v4_tile_profile_accepts_and_checks_sealed_tiles() {
        let info = crate::ImageInfo::new(2, 2, Channels::Gray, SampleType::U8);
        let cfg = crate::ImiEncoderConfig::lossless()
            .with_effort(0.0)
            .with_checksum(true);
        let plain = cfg
            .encode_samples(
                crate::ImageRef::packed(&[1, 2, 3, 4], &info),
                &crate::Metadata::default(),
            )
            .unwrap();
        let mut sealed = crate::add_pixel_checksum(plain).unwrap();
        let profile = |data: &[u8]| {
            native_profile(
                data,
                2,
                2,
                Channels::Gray,
                SampleType::U8,
                Coding::Lossless,
                Cicp::SRGB,
                None,
            )
        };
        assert_eq!(profile(&sealed), Ok(()));
        assert_eq!(crate::decode(&sealed).unwrap().1, [1, 2, 3, 4]);
        // A one-bit change to the pSUM digest also invalidates cSUM.
        let at = sealed.windows(4).position(|tag| tag == b"pSUM").unwrap();
        sealed[at + 10] ^= 1;
        assert!(profile(&sealed).is_err());
    }

    #[test]
    fn tile_profile_rejects_geometry_metadata_and_undeclared_orientation() {
        let info = crate::ImageInfo::new(2, 2, Channels::Gray, SampleType::U8);
        let cfg = EncodeConfig::default().with_effort(0.0);
        let pixels = [1, 2, 3, 4];
        let plain = crate::encode(&pixels, 2, &info, &crate::Metadata::default(), &cfg).unwrap();
        native_profile(
            &plain,
            2,
            2,
            Channels::Gray,
            SampleType::U8,
            Coding::Lossless,
            Cicp::SRGB,
            None,
        )
        .unwrap();
        assert!(
            native_profile(
                &plain,
                1,
                1,
                Channels::Gray,
                SampleType::U8,
                Coding::Lossless,
                Cicp::SRGB,
                None,
            )
            .is_err()
        );
        let metadata = crate::Metadata {
            icc: Some(alloc::vec![0;4096]),
            ..Default::default()
        };
        let nested = crate::encode(&pixels, 2, &info, &metadata, &cfg).unwrap();
        assert!(
            native_profile(
                &nested,
                2,
                2,
                Channels::Gray,
                SampleType::U8,
                Coding::Lossless,
                Cicp::SRGB,
                None,
            )
            .is_err()
        );
        let oriented = crate::encode(
            &pixels,
            2,
            &info,
            &crate::Metadata {
                orientation: 2,
                ..Default::default()
            },
            &cfg,
        )
        .unwrap();
        assert!(
            native_profile(
                &oriented,
                2,
                2,
                Channels::Gray,
                SampleType::U8,
                Coding::Lossless,
                Cicp::SRGB,
                None,
            )
            .is_err()
        );
        assert!(
            native_profile(
                &plain,
                2,
                2,
                Channels::Gray,
                SampleType::U8,
                Coding::Lossless,
                Cicp::BT2100_PQ,
                None,
            )
            .is_err()
        );
    }
}
