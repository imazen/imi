//! Ordinary positional V4 regions: bounded adjacent-tile reads and group batches.
use super::{ReadAt, ReadStats, Rect, Window};
use crate::decode_resources::{DecodePolicy, DecodeSession, V4Resume, V4Retained};
use crate::v4::decode::{self, Crop, GroupWork};
use crate::v4::read_at::IndexedFile;
use crate::{Error, ImageInfo, Limits};
use alloc::vec::Vec;

pub(super) struct Source<R> {
    pub inner: R,
    window: Window,
    pub stats: ReadStats,
    _memory: V4Retained,
}
impl<R: ReadAt> ReadAt for Source<R> {
    fn len(&mut self) -> Result<u64, Error> {
        self.inner.len()
    }
    fn read_exact_at(&mut self, at: u64, out: &mut [u8]) -> Result<(), Error> {
        let end = at.checked_add(out.len() as u64).ok_or(Error::TooLarge)?;
        if self.window.holds(at, end) {
            out.copy_from_slice(self.window.slice(at, end));
            return Ok(());
        }
        self.stats.reads = self.stats.reads.checked_add(1).ok_or(Error::TooLarge)?;
        self.stats.bytes = self
            .stats
            .bytes
            .checked_add(out.len() as u64)
            .ok_or(Error::TooLarge)?;
        self.stats.batches = self.stats.batches.checked_add(1).ok_or(Error::TooLarge)?;
        self.inner.read_exact_at(at, out)
    }
}
pub(super) struct Reader<R: ReadAt> {
    pub file: IndexedFile<Source<R>>,
    request: V4Resume,
    pub(super) limits: Limits,
    pub(super) verify_scoped_pixels: bool,
}
fn policy(limits: Limits) -> DecodePolicy {
    DecodePolicy::default()
        .with_output_limits(Limits {
            max_pixels: u64::MAX,
            ..limits
        })
        .with_max_input_bytes(limits.max_bytes)
        .with_max_working_bytes(limits.max_bytes)
        .with_max_units(u64::MAX)
        .with_max_work_units(u64::MAX)
}
impl<R: ReadAt> Reader<R> {
    pub(super) fn read_verified_at(&mut self, at: u64, bytes: &mut [u8]) -> Result<(), Error> {
        let session = self.request.request(policy(self.limits), || false);
        let result = self.file.read_verified_at(at, bytes, &session);
        self.request.update(&session);
        result
    }
    pub fn open(
        inner: R,
        window: Window,
        stats: ReadStats,
        limits: Limits,
        profile: Option<crate::v4::CodingProfile>,
    ) -> Result<Self, Error> {
        let session = DecodeSession::new(policy(limits).with_optional_v4_profile(profile));
        let memory = session
            .v4_working(window.buf.capacity() as u128)?
            .retain()?;
        let source = Source {
            inner,
            window,
            stats,
            _memory: memory,
        };
        let file = IndexedFile::open(
            source,
            crate::v4::TileLimits {
                max_object_bytes: limits.max_bytes,
                max_tile_bytes: limits.max_bytes,
            },
            &session,
            false,
        )?;
        Ok(Self {
            file,
            request: session.v4_resume(),
            limits,
            verify_scoped_pixels: true,
        })
    }
    fn selection<C: Fn() -> bool + Sync>(
        &self,
        rect: Rect,
        session: &DecodeSession<C>,
    ) -> Result<(Crop, Vec<(u64, u64)>, V4Retained), Error> {
        let info = &self.file.header().info;
        if rect.width() == 0
            || rect.height() == 0
            || rect
                .x()
                .checked_add(rect.width())
                .is_none_or(|v| v > info.width)
            || rect
                .y()
                .checked_add(rect.height())
                .is_none_or(|v| v > info.height)
        {
            return Err(Error::InvalidInput("region outside the image"));
        }
        let first = self.file.group_rect(0)?;
        let (gx, gy) = self.file.layout().group_grid();
        let g0 = rect.x() / first.width();
        let g1 = (rect.x() + rect.width()).div_ceil(first.width()).min(gx);
        let s0 = rect.y() / first.height();
        let s1 = (rect.y() + rect.height()).div_ceil(first.height()).min(gy);
        let crop = Crop {
            rect,
            columns: u64::from(gx),
            s0: u64::from(s0),
            g0: u64::from(g0),
            gn: u64::from(g1 - g0),
        };
        let layout = self.file.layout();
        let (tw, th) = layout.tile_shape();
        let (tx, _) = layout.tile_grid();
        let count = u64::from((g1 - 1) / tw - g0 / tw + 1) * u64::from((s1 - 1) / th - s0 / th + 1);
        let capacity = usize::try_from(count).map_err(|_| Error::TooLarge)?;
        let memory = session.v4_working(count as u128 * 16)?;
        session.adapter_work(count as u128 * 64 + 1)?;
        let mut tiles = Vec::new();
        tiles
            .try_reserve_exact(capacity)
            .map_err(|_| Error::TooLarge)?;
        for y in s0 / th..=(s1 - 1) / th {
            for x in g0 / tw..=(g1 - 1) / tw {
                let id = u64::from(y) * u64::from(tx) + u64::from(x);
                tiles.push((layout.tile_ordinal(id)?, id));
            }
        }
        tiles.sort_unstable();
        Ok((crop, tiles, memory.retain()?))
    }
    pub fn ranges(&mut self, rect: Rect) -> Result<Vec<core::ops::Range<u64>>, Error> {
        let session = self.request.request(policy(self.limits), || false);
        let (_, tiles, _tiles_memory) = self.selection(rect, &session)?;
        let _memory = session.v4_working(tiles.capacity() as u128 * 32)?;
        let mut ranges = Vec::new();
        ranges
            .try_reserve_exact(tiles.len())
            .map_err(|_| Error::TooLarge)?;
        for (_, id) in tiles {
            ranges.push(self.file.tile_range(id, &session)?);
        }
        // Model definitions can be separate metadata dependencies. Include all
        // unloaded ones conservatively; decode itself fetches selected models.
        let _dependencies = self.file.dependency_ranges(&mut ranges, &session)?;
        let _checksums = self.file.checksum_ranges(&mut ranges, &session)?;
        ranges.sort_unstable_by_key(|r| r.start);
        let _merged_memory = session
            .v4_working(ranges.len() as u128 * size_of::<core::ops::Range<u64>>() as u128)?;
        let mut merged: Vec<core::ops::Range<u64>> = Vec::new();
        merged
            .try_reserve_exact(ranges.len())
            .map_err(|_| Error::TooLarge)?;
        for range in ranges {
            if let Some(last) = merged.last_mut()
                && range.start <= last.end
            {
                last.end = last.end.max(range.end);
            } else {
                merged.push(range);
            }
        }
        Ok(merged)
    }
    pub fn decode(
        &mut self,
        rect: Rect,
        out: &mut [u8],
        stride: usize,
        threads: usize,
        pulse: Option<&dyn how_far::Pulse>,
    ) -> Result<(), Error> {
        let verify = self.verify_scoped_pixels;
        let policy = policy(self.limits).with_scoped_pixel_checks(verify, verify);
        let session = self
            .request
            .request(policy, || pulse.is_some_and(|p| p.check().is_err()));
        let result = self.decode_with(rect, out, stride, threads, pulse, &session);
        self.request.update(&session);
        result
    }
    fn decode_with<C: Fn() -> bool + Sync>(
        &mut self,
        rect: Rect,
        out: &mut [u8],
        stride: usize,
        threads: usize,
        pulse: Option<&dyn how_far::Pulse>,
        session: &DecodeSession<C>,
    ) -> Result<(), Error> {
        session.adapter_work(0)?;
        let mut info: ImageInfo = self.file.header().info.clone();
        info.width = rect.width();
        info.height = rect.height();
        crate::parse::limits::check_limits(&info, &self.limits)?;
        let required = info
            .checked_stride_len(stride)
            .ok_or(Error::InvalidInput("region output stride/extent"))?;
        if out.len() < required {
            return Err(Error::InvalidInput("region output stride/extent"));
        }
        let (crop, tiles, _tiles_memory) = self.selection(rect, session)?;
        let full_info = self.file.header().info.clone();
        let (columns, rows) = self.file.layout().group_grid();
        let first = self.file.group_rect(0)?;
        let s1 = (rect.y() + rect.height())
            .div_ceil(first.height())
            .min(rows);
        let count = (u64::from(s1) - crop.s0)
            .checked_mul(crop.gn)
            .ok_or(Error::TooLarge)?;
        let workers = crate::par::worker_count(
            threads.min(decode::available_workers()),
            usize::try_from(count).map_err(|_| Error::TooLarge)?,
        );
        let _plan_memory = session.v4_working(if pulse.is_some() {
            u128::from(count) * 1024
        } else {
            0
        })?;
        let progress = if let Some(pulse) = pulse {
            let mut shapes = Vec::new();
            shapes
                .try_reserve_exact(usize::try_from(count).map_err(|_| Error::TooLarge)?)
                .map_err(|_| Error::TooLarge)?;
            for s in crop.s0..u64::from(s1) {
                for g in crop.g0..crop.g0 + crop.gn {
                    let r = self.file.group_rect(s * u64::from(columns) + g)?;
                    shapes.push(crate::progress::UnitShape {
                        stripe: s as usize,
                        group: g as usize,
                        groups: columns as usize,
                        y: r.y() as usize,
                        rows: r.height() as usize,
                        x: r.x() as usize,
                        cols: r.width() as usize,
                    });
                }
            }
            crate::progress::UnitProgress::plan(pulse, shapes.into_iter(), workers)
        } else {
            None
        };
        let selected = |id: u64| {
            id / u64::from(columns) >= crop.s0
                && id / u64::from(columns) < u64::from(s1)
                && id % u64::from(columns) >= crop.g0
                && id % u64::from(columns) < crop.g0 + crop.gn
        };
        #[cfg(all(feature = "std", any(not(target_family = "wasm"), feature = "rayon")))]
        let window = decode::group_window(workers, count as usize);
        #[cfg(all(feature = "std", any(not(target_family = "wasm"), feature = "rayon")))]
        let _pending_memory =
            session.v4_working(window as u128 * size_of::<GroupWork<'_>>() as u128)?;
        // Merge adjacent selected tiles independently of worker count. The
        // archive reader uses the same 1 MiB maximum merged-range policy.
        // Explicit large tiles remain legal and are fetched individually.
        const MAX_MERGED_TILE_BYTES: u64 = 1 << 20;
        let mut first_tile = 0;
        while first_tile < tiles.len() {
            session.adapter_work(0)?;
            self.file.evict_models();
            let first_range = self.file.tile_range(tiles[first_tile].1, session)?;
            let mut end_tile = first_tile + 1;
            let mut range_end = first_range.end;
            if self.file.header().coding == crate::Coding::Lossless {
                while end_tile < tiles.len() {
                    let next = self.file.tile_range(tiles[end_tile].1, session)?;
                    if next.start != range_end
                        || next.end - first_range.start > MAX_MERGED_TILE_BYTES
                    {
                        break;
                    }
                    range_end = next.end;
                    end_tile += 1;
                }
            }
            let _compressed = loop {
                match session.v4_working(u128::from(range_end - first_range.start)) {
                    Ok(memory) => break memory,
                    Err(Error::TooLarge) if end_tile > first_tile + 1 => {
                        end_tile = first_tile + (end_tile - first_tile).div_ceil(2);
                        range_end = self.file.tile_range(tiles[end_tile - 1].1, session)?.end;
                    }
                    Err(error) => return Err(error),
                }
            };
            let mut bytes = Vec::new();
            let fetched_range = self.file.fetch_tile_batch(
                tiles[first_tile..end_tile].iter().map(|&(_, id)| id),
                &mut bytes,
                session,
            )?;
            self.file.verify_used(
                (fetched_range.start, fetched_range.end),
                fetched_range.start,
                &bytes,
                session,
            )?;
            #[cfg(all(feature = "std", any(not(target_family = "wasm"), feature = "rayon")))]
            let mut pending: Vec<GroupWork<'_>> = Vec::new();
            #[cfg(all(feature = "std", any(not(target_family = "wasm"), feature = "rayon")))]
            pending
                .try_reserve_exact(window)
                .map_err(|_| Error::TooLarge)?;
            let mut emit = |work| -> Result<(), Error> {
                #[cfg(all(feature = "std", any(not(target_family = "wasm"), feature = "rayon")))]
                if workers > 1 {
                    pending.push(work);
                    if pending.len() == window {
                        decode::flush(
                            &mut pending,
                            out,
                            stride,
                            &full_info,
                            session,
                            progress.as_ref(),
                            Some(crop),
                            workers,
                        )?;
                    }
                    return Ok(());
                }
                decode::decode_one(
                    work,
                    out,
                    stride,
                    &full_info,
                    session,
                    progress.as_ref(),
                    Some(crop),
                )
            };
            for &(_, tile) in &tiles[first_tile..end_tile] {
                let range = self.file.tile_range(tile, session)?;
                let start = usize::try_from(range.start - fetched_range.start)
                    .map_err(|_| Error::TooLarge)?;
                let end = usize::try_from(range.end - fetched_range.start)
                    .map_err(|_| Error::TooLarge)?;
                let bytes = &bytes[start..end];
                if self.file.header().coding == crate::Coding::Lossless {
                    self.file.visit_tile(tile, bytes, session, |work| {
                        if selected(work.entry.entry.id) {
                            emit(GroupWork::Numeric(work))?;
                        }
                        Ok(())
                    })?;
                } else {
                    let plan = self.file.plan_lossy_tile(tile, bytes, session)?;
                    let mut dc = crate::lossy::v4::NumericDependency::default();
                    let mut exact = crate::lossy::v4::NumericDependency::default();
                    for group in self.file.layout().groups(tile)? {
                        if selected(group.id) {
                            emit(GroupWork::Lossy(
                                plan.group(group.id, bytes, session, &mut dc, &mut exact)?,
                            ))?;
                        }
                    }
                }
            }
            #[cfg(all(feature = "std", any(not(target_family = "wasm"), feature = "rayon")))]
            decode::flush(
                &mut pending,
                out,
                stride,
                &full_info,
                session,
                progress.as_ref(),
                Some(crop),
                workers,
            )?;
            for &(_, tile) in &tiles[first_tile..end_tile] {
                let range = self.file.tile_range(tile, session)?;
                let start = usize::try_from(range.start - fetched_range.start)
                    .map_err(|_| Error::TooLarge)?;
                let end = usize::try_from(range.end - fetched_range.start)
                    .map_err(|_| Error::TooLarge)?;
                if session.v4_pixel_checks(crate::v4::CheckScope::TilePixels)
                    && let Some(check) = self.file.tile_pixel_check(tile, &bytes[start..end])?
                {
                    check.verify_cropped(&full_info, out, stride, rect, session)?;
                }
            }
            first_tile = end_tile;
        }
        self.file.verify_pixels(rect, out, stride, session)
    }
}
