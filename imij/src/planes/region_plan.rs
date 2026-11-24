//! Metadata-only range planning for native image views.
use super::*;
use core::ops::Range;

pub(super) struct TileArea {
    pub columns: u32,
    pub end_x: u32,
    pub end_y: u32,
    pub x0: u32,
    pub x1: u32,
    pub y0: u32,
    pub y1: u32,
}
impl TileArea {
    pub fn new(level: &LevelDescriptor, rect: Rect) -> Result<Self, Error> {
        let ex = rect
            .x
            .checked_add(rect.width)
            .ok_or(Error::InvalidInput("ROI overflow"))?;
        let ey = rect
            .y
            .checked_add(rect.height)
            .ok_or(Error::InvalidInput("ROI overflow"))?;
        if rect.width == 0 || rect.height == 0 || ex > level.width || ey > level.height {
            return Err(Error::InvalidInput("ROI outside level"));
        }
        Ok(Self {
            columns: level.tiles()?.0,
            end_x: ex,
            end_y: ey,
            x0: rect.x / level.tile_width,
            x1: (ex - 1) / level.tile_width,
            y0: rect.y / level.tile_height,
            y1: (ey - 1) / level.tile_height,
        })
    }

    fn rows(&self, record_start: u32) -> impl Iterator<Item = Range<u64>> + '_ {
        (self.y0..=self.y1).map(move |y| {
            let base = u64::from(record_start) + u64::from(y) * u64::from(self.columns);
            base + u64::from(self.x0)..base + u64::from(self.x1) + 1
        })
    }
}

impl<R: ReadAt> PlaneSetReader<R> {
    /// Directory pages needed to plan this view rectangle, in file order.
    /// Returns no pages for a primary or flat backing. Does not read data.
    pub fn view_region_pages(&self, id: u64, rect: Rect) -> Result<Vec<Range<u64>>, Error> {
        let ViewBacking::Level { plane, level } = self.view_backing(id)? else {
            return Ok(Vec::new());
        };
        let level = &self.dir.planes[plane.0 as usize].levels[level as usize];
        let area = TileArea::new(level, rect)?;
        let mut result = Vec::<Range<u64>>::new();
        let Some(root) = &self.dir.pages else {
            return Ok(result);
        };
        let mut next_page = 0;
        let per_page = u64::from(root.per_page);
        let mut bytes = 0u64;
        for ids in area.rows(level.record_start) {
            for page in (ids.start / per_page).max(next_page)..=(ids.end - 1) / per_page {
                let descriptor = &root.pages[page as usize];
                let size = wire::chunk_len(descriptor.len)?;
                bytes = bytes.checked_add(size).ok_or(Error::TooLarge)?;
                if bytes > self.limits.max_compressed_work
                    || result.len()
                        >= self.limits.max_index_bytes / core::mem::size_of::<Range<u64>>()
                {
                    return Err(Error::TooLarge);
                }
                result.try_reserve(1).map_err(|_| Error::TooLarge)?;
                result.push(descriptor.offset..descriptor.offset + size);
                next_page = page + 1;
            }
        }
        Ok(result)
    }

    /// Complete framed tile ranges after reading the selected directory pages.
    /// Planning never reads native tile payloads. For a primary backing, the
    /// ordinary region reader opens its header and plans its ROI dependencies.
    /// Include the page ranges separately when supplying a fresh reader.
    /// Outer cSUM may require further reads to verify fetched byte ranges.
    pub fn view_region_ranges(&mut self, id: u64, rect: Rect) -> Result<Vec<Range<u64>>, Error> {
        self.with_read_request(|reader| reader.view_region_ranges_inner(id, rect))
    }

    fn view_region_ranges_inner(&mut self, id: u64, rect: Rect) -> Result<Vec<Range<u64>>, Error> {
        let ViewBacking::Level { plane, level } = self.view_backing(id)? else {
            return self.primary_region_reader()?.region_ranges(rect);
        };
        let plane = &self.dir.planes[plane.0 as usize];
        let sample = plane.sample;
        let level = plane.levels[level as usize].clone();
        let area = TileArea::new(&level, rect)?;
        let pages = self.view_region_pages(id, rect)?;
        let mut compressed = pages.iter().map(|p| p.end - p.start).sum::<u64>();
        let per_page = self.dir.pages.as_ref().map_or(1, |root| root.per_page);
        let prefetched = self.read_region_pages(&pages)?;
        let mut decoded = 0u64;
        let mut ranges = Vec::new();
        self.page_read_bytes = 0;
        for ids in area.rows(level.record_start) {
            for record_id in ids {
                let record_id = u32::try_from(record_id).map_err(|_| Error::TooLarge)?;
                let page = (record_id / per_page) as usize;
                let record = match prefetched.binary_search_by_key(&page, |(index, _)| *index) {
                    Ok(index) => prefetched[index].1[(record_id % per_page) as usize],
                    Err(_) => self.record_by_id(record_id)?,
                };
                let size = wire::chunk_len(record.len)?;
                compressed = compressed.checked_add(size).ok_or(Error::TooLarge)?;
                let ordinal = record_id - level.record_start;
                let w = level
                    .tile_width
                    .min(level.width - (ordinal % area.columns) * level.tile_width);
                let h = level
                    .tile_height
                    .min(level.height - (ordinal / area.columns) * level.tile_height);
                let tile_bytes = sample_bytes(w, h, sample)?
                    .checked_mul(level.channels.count())
                    .ok_or(Error::TooLarge)?;
                decoded = decoded
                    .checked_add(tile_bytes as u64)
                    .ok_or(Error::TooLarge)?;
                if compressed > self.limits.max_compressed_work
                    || decoded > self.limits.max_decoded_work
                    || self.payload_len(record_id, record)? > self.limits.max_record_bytes
                    || ranges.len()
                        >= self.limits.max_index_bytes / core::mem::size_of::<Range<u64>>()
                {
                    return Err(Error::TooLarge);
                }
                ranges.try_reserve(1).map_err(|_| Error::TooLarge)?;
                ranges.push(record.offset..record.offset + size);
            }
        }
        Ok(ranges)
    }
}
