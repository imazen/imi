//! Suspendable V4 framing uses owned parser state between input/output awaits.

use super::*;
use crate::decode_resources::{DecodeSession, V4Retained};
use crate::parse::limits::Admission;
use crate::v4::{
    Directory, DirectoryPlacement, Entry, NumericEvent, NumericGroupDecoder, ObjectIntegrity,
    TileLayout, TileLimits,
};
use alloc::collections::BTreeMap;

pub(crate) struct Index {
    placement: DirectoryPlacement,
    layout: TileLayout,
    integrity: ObjectIntegrity,
    entries: Vec<Entry>,
    objects_len: u64,
}

impl Index {
    pub(super) fn needs_tail(&self) -> bool {
        self.placement == DirectoryPlacement::Tail
    }
    pub(super) fn check(
        &self,
        payload: &[u8],
        chunk_len: u64,
        admission: &impl Admission,
    ) -> Result<(), Error> {
        if !self.needs_tail() {
            return Err(Error::Corrupt("unexpected V4 trailing directory"));
        }
        let body = crate::v4::file::check_tail_locator(payload, chunk_len)?;
        let snapshot = admission.v4_resume();
        let session = snapshot.session(admission.v4_cancel());
        let result = (|| {
            let _input = session.v4_precharged()?;
            let directory = Directory::read(
                body,
                self.layout,
                self.integrity,
                self.objects_len,
                &session,
            )?;
            if directory.entry_count() != self.entries.len() as u64 {
                return Err(Error::Corrupt("V4 streamed directory entry count"));
            }
            let mut at = 0;
            directory.visit(&session, |entry, _| {
                if self.entries.get(at) != Some(&entry.entry) {
                    return Err(Error::Corrupt("V4 streamed directory entry mismatch"));
                }
                at += 1;
                Ok(true)
            })?;
            Ok(())
        })();
        let committed = admission.v4_commit(session.v4_resume());
        result?;
        committed
    }
}

struct Band {
    pixels: Vec<u8>,
    rows: u32,
    groups: u32,
    _charge: V4Retained,
}

fn band<C: Fn() -> bool + Sync>(
    info: &ImageInfo,
    rows: u32,
    session: &DecodeSession<C>,
) -> Result<Band, Error> {
    let scope = session.v4_model_scope();
    let bytes = info
        .row_bytes()
        .checked_mul(rows as usize)
        .ok_or(Error::TooLarge)?;
    session.v4_models().hint(
        bytes
            .checked_add(size_of::<Band>() + 128)
            .ok_or(Error::TooLarge)?,
    )?;
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(bytes)
        .map_err(|_| Error::TooLarge)?;
    pixels.resize(bytes, 0);
    Ok(Band {
        pixels,
        rows,
        groups: 0,
        _charge: scope.commit()?,
    })
}

fn remember(
    entries: &mut Vec<Entry>,
    entry: Entry,
    admission: &impl Admission,
) -> Result<(), Error> {
    if entries.len() == entries.capacity() {
        let capacity = entries
            .capacity()
            .max(8)
            .checked_mul(2)
            .ok_or(Error::TooLarge)?;
        // Include the old and new allocation during a possible reallocation.
        // This conservative request plan remains charged until stream finish.
        admission.hint(
            capacity
                .checked_mul(size_of::<Entry>())
                .ok_or(Error::TooLarge)?,
        )?;
        entries
            .try_reserve_exact(capacity - entries.len())
            .map_err(|_| Error::TooLarge)?;
    }
    entries.push(entry);
    Ok(())
}

// These owners have no request reference. Only one grammar is active. Keeping
// their parser and pixel-dependency state together avoids an allocation per
// object when reattaching a request after input/output suspension.
struct State {
    checks: Option<alloc::sync::Arc<crate::v4::OwnedChecks>>,
    tile_hash: Option<crate::v4::RetainedPixelCheckState>,
    numeric: Option<crate::v4::NumericState>,
    lossy: Option<crate::lossy::v4::LossyState>,
    pixels: Option<crate::lossy::v4::AssemblerState>,
}

fn output_band<'a, C: Fn() -> bool + Sync>(
    bands: &'a mut BTreeMap<u32, Band>,
    info: &ImageInfo,
    rect: crate::planes::Rect,
    session: &DecodeSession<C>,
) -> Result<&'a mut Band, Error> {
    let slot = bands.entry(rect.y());
    let band = match slot {
        alloc::collections::btree_map::Entry::Vacant(slot) => {
            slot.insert(band(info, rect.height(), session)?)
        }
        alloc::collections::btree_map::Entry::Occupied(slot) => slot.into_mut(),
    };
    if band.rows != rect.height() {
        return Err(Error::Corrupt("V4 streamed row geometry"));
    }
    Ok(band)
}

impl State {
    fn read<'a, C: Fn() -> bool + Sync>(
        &mut self,
        bytes: &'a [u8],
        info: &ImageInfo,
        bands: &mut BTreeMap<u32, Band>,
        session: &DecodeSession<C>,
    ) -> Result<
        (
            crate::v4::LocatedEntry,
            bool,
            Option<crate::v4::decode::GroupWork<'a>>,
        ),
        Error,
    > {
        if let Some(state) = self.numeric.take() {
            let mut decoder = state.bind(session);
            let event = decoder.read(bytes)?;
            let entry = event.entry();
            let end = matches!(event, NumericEvent::End { .. });
            if let NumericEvent::End { pixel_results, .. } = &event {
                self.verify_tile(pixel_results, session)?;
            }
            let work = match event {
                NumericEvent::Group(work) => {
                    output_band(bands, info, work.rect()?, session)?;
                    Some(crate::v4::decode::GroupWork::Numeric(work))
                }
                _ => None,
            };
            self.numeric = Some(decoder.into_state());
            return Ok((entry, end, work));
        }
        let mut decoder = self
            .lossy
            .take()
            .ok_or(Error::Corrupt("missing V4 lossy state"))?
            .bind(session);
        let mut pixels = self
            .pixels
            .take()
            .ok_or(Error::Corrupt("missing V4 dependency state"))?
            .bind(session);
        let event = decoder.read(bytes)?;
        let entry = event.entry();
        let end = matches!(event, crate::v4::LossyEvent::End { .. });
        if let crate::v4::LossyEvent::End { pixel_results, .. } = &event {
            self.verify_tile(pixel_results, session)?;
        }
        let work = pixels
            .event(event)?
            .map(crate::v4::decode::GroupWork::Lossy);
        if let Some(work) = &work {
            output_band(bands, info, work.rect()?, session)?;
        }
        self.lossy = Some(decoder.into_state());
        self.pixels = Some(pixels.into_state()?);
        Ok((entry, end, work))
    }

    fn verify_tile<C: Fn() -> bool + Sync>(
        &mut self,
        expected: &[u8],
        session: &DecodeSession<C>,
    ) -> Result<(), Error> {
        if session.v4_pixel_checks(crate::v4::CheckScope::TilePixels)
            && let Some(checks) = &self.checks
            && checks.width(crate::v4::CheckScope::TilePixels) != 0
        {
            let (state, _charge) = self
                .tile_hash
                .take()
                .ok_or(Error::Corrupt("missing V4 streamed tile pixels"))?;
            crate::v4::PixelChecks::from_state(
                checks.get().list(crate::v4::CheckScope::TilePixels),
                state,
            )
            .verify(expected)?;
        }
        Ok(())
    }

    fn hash_band<C: Fn() -> bool + Sync>(
        &mut self,
        info: &ImageInfo,
        layout: TileLayout,
        id: u64,
        rect: crate::planes::Rect,
        band: &Band,
        session: &DecodeSession<C>,
    ) -> Result<(), Error> {
        if rect.height() != band.rows {
            return Err(Error::Corrupt("V4 tile row height"));
        }
        let Some(checks) = &self.checks else {
            return Ok(());
        };
        if !session.v4_pixel_checks(crate::v4::CheckScope::TilePixels) {
            return Ok(());
        }
        let list = checks.get().list(crate::v4::CheckScope::TilePixels);
        if list.is_empty() {
            return Ok(());
        }
        let tile = layout.tile_for_group(id)?;
        let last = u64::from(tile.group_x + tile.groups_wide - 1);
        if id % u64::from(layout.group_grid().0) != last {
            return Ok(());
        }
        // Physical group order is raster within the tile. At its last column,
        // every pixel in this tile-row band is reconstructed, before publication.
        // Recover exact group rectangles from the active checked setup.
        let group_rect = |gid| {
            if let Some(state) = &self.numeric {
                state.group_rect(gid)
            } else {
                self.lossy
                    .as_ref()
                    .ok_or(Error::Corrupt("missing V4 streamed setup"))?
                    .group_rect(gid)
            }
        };
        let tile_rect = crate::v4::tile_pixel_rect(layout, tile.id, group_rect)?;
        let mut local = info.clone();
        local.width = tile_rect.width();
        local.height = tile_rect.height();
        let _scratch = session.v4_working(crate::v4::PixelChecks::SCRATCH_BYTES as u128)?;
        let (mut hash, charge) = crate::v4::PixelChecks::tile_state(
            list,
            self.tile_hash.take(),
            crate::v4::PixelCheckIdentity {
                image: 0,
                id: tile.id,
                x: tile_rect.x(),
                y: tile_rect.y(),
            },
            &local,
            session,
        )?;
        local.height = band.rows;
        let known = list.iter().filter(|d| d.algorithm().is_some()).count();
        session.adapter_work(
            u128::from(local.width)
                * u128::from(local.height)
                * local.bytes_per_pixel() as u128
                * known as u128,
        )?;
        let at = tile_rect.x() as usize * info.bytes_per_pixel();
        hash.rows_checked(
            crate::ImageRef::new(&band.pixels[at..], info.row_bytes(), &local),
            || session.v4_models().check(),
        )?;
        self.tile_hash = Some((hash.into_state(), charge));
        Ok(())
    }

    fn finish<C: Fn() -> bool + Sync>(self, session: &DecodeSession<C>) -> Result<(), Error> {
        if let Some(state) = self.numeric {
            state.bind(session).finish_structure()
        } else {
            self.lossy
                .ok_or(Error::Corrupt("missing V4 lossy state"))?
                .bind(session)
                .finish_structure()
        }
    }
}

pub(super) async fn rows<R: Input, A: Admission>(
    src: &mut Src<R>,
    header: &crate::Header,
    declared: crate::Features,
    admission: &A,
    output: &mut (impl input::Rows + ?Sized),
) -> Result<(), Error> {
    let mut setup = Vec::new();
    let length = src.varint_async(None).await?;
    {
        let snapshot = admission.v4_resume();
        let session = snapshot.session(admission.v4_cancel());
        if length
            .checked_add(crate::varint_len(length) as u64)
            .is_none_or(|n| n > session.v4_header_limit())
        {
            return Err(Error::TooLarge);
        }
    }
    src.r.reserve(&mut setup, 10)?;
    crate::put_varint(&mut setup, length);
    src.take_async(length, &mut setup).await?;
    let placement = DirectoryPlacement::try_from(src.varint_async(None).await?)?;
    src.r.reserve(&mut setup, 10)?;
    crate::put_varint(&mut setup, placement.wire_value());
    if placement == DirectoryPlacement::Front {
        let capacity = src.varint_async(None).await?;
        src.r.reserve(&mut setup, 10)?;
        crate::put_varint(&mut setup, capacity);
        src.take_async(capacity, &mut setup).await?;
    }
    let (mut state, front, layout, integrity) = {
        let snapshot = admission.v4_resume();
        let session = snapshot.session(admission.v4_cancel());
        let parsed = (|| {
            let _input = session.v4_precharged()?;
            let limits = TileLimits {
                max_object_bytes: u64::MAX,
                max_tile_bytes: u64::MAX,
            };
            let (state, used, storage, layout, integrity) = match header.coding {
                crate::Coding::Lossless => {
                    let (decoder, used, storage) = NumericGroupDecoder::with_directory(
                        header.info.clone(),
                        declared,
                        &setup,
                        limits,
                        &session,
                    )?;
                    let layout = decoder.layout();
                    let integrity = decoder.integrity();
                    (
                        State {
                            checks: decoder.check_config().cloned(),
                            tile_hash: None,
                            numeric: Some(decoder.into_state()),
                            lossy: None,
                            pixels: None,
                        },
                        used,
                        storage,
                        layout,
                        integrity,
                    )
                }
                crate::Coding::Lossy => {
                    let color = header
                        .lossy_color
                        .ok_or(Error::Corrupt("V4 lossy color missing"))?;
                    let (decoder, used, storage) = crate::v4::LossyGroupDecoder::with_directory(
                        header.info.clone(),
                        color,
                        declared,
                        &setup,
                        limits,
                        &session,
                    )?;
                    let layout = decoder.setup().layout();
                    let integrity = decoder.setup().integrity();
                    let pixels = crate::lossy::v4::Assembler::new(decoder.shared_setup(), &session)
                        .into_state()?;
                    (
                        State {
                            checks: decoder.setup().checks.clone(),
                            tile_hash: None,
                            numeric: None,
                            lossy: Some(decoder.into_state()),
                            pixels: Some(pixels),
                        },
                        used,
                        storage,
                        layout,
                        integrity,
                    )
                }
            };
            if used != setup.len() {
                return Err(Error::Corrupt("V4 streamed setup extent"));
            }
            session.v4_check_units(layout.group_count())?;
            let front = storage
                .front
                .map(|bytes| {
                    let suffix = bytes
                        .get(
                            bytes
                                .len()
                                .checked_sub(8)
                                .ok_or(Error::Corrupt("short V4 front directory"))?..,
                        )
                        .ok_or(Error::Corrupt("short V4 front directory"))?;
                    let objects = u64::from_le_bytes(suffix.try_into().unwrap());
                    Directory::read_admitted(bytes, layout, integrity, objects, &session)
                })
                .transpose()?;
            if placement == DirectoryPlacement::Front && front.is_none() {
                return Err(Error::Corrupt("unfinished V4 front directory"));
            }
            Ok((state, front, layout, integrity))
        })();
        let committed = admission.v4_commit(session.v4_resume());
        let parsed = parsed?;
        committed?;
        parsed
    };
    let objects_at = src.pos;
    let mut object = Vec::new();
    let mut entries = Vec::new();
    let mut front_entries = front.as_ref().map(Directory::entries_reader);
    let mut front_count = 0;
    let mut bands = BTreeMap::new();
    let mut next_row = 0u32;
    let mut tiles = 0u64;
    loop {
        admission.check()?;
        if src.end == Some(src.pos) {
            break;
        }
        object.clear();
        let kind = src.varint_async(None).await?;
        let length = src.varint_async(None).await?;
        src.r.reserve(&mut object, 20)?;
        crate::put_varint(&mut object, kind);
        crate::put_varint(&mut object, length);
        src.take_async(length, &mut object).await?;
        let snapshot = admission.v4_resume();
        let (parsed, usage) = {
            let session = snapshot.session(admission.v4_cancel());
            let result = (|| {
                let _input = session.v4_precharged()?;
                state.read(&object, &header.info, &mut bands, &session)
            })();
            (result, session.v4_resume())
        };
        let committed = admission.v4_commit(usage);
        let (entry, closed_tile, work) = parsed?;
        committed?;
        if let Some(work) = work {
            let rect = work.rect()?;
            let band = bands
                .get_mut(&rect.y())
                .ok_or(Error::Corrupt("missing V4 output band"))?;
            let inter = output.has_reference();
            // No request/session borrow survives a reference-source suspension.
            // The consumer owns the reference raster; the codec owns one band.
            if inter && band.groups == 0 {
                for y in 0..band.rows {
                    admission.check()?;
                    let at = y as usize * header.info.row_bytes();
                    output
                        .reference(
                            &header.info,
                            rect.y() + y,
                            &mut band.pixels[at..at + header.info.row_bytes()],
                        )
                        .await?;
                }
            }
            let snapshot = admission.v4_resume();
            let session = snapshot.session(admission.v4_cancel());
            let x = rect.x() as usize * header.info.bytes_per_pixel();
            let result = if inter {
                work.decode_reference(
                    &header.info,
                    &mut band.pixels[x..],
                    header.info.row_bytes(),
                    &session,
                )
            } else {
                work.decode(&mut band.pixels[x..], header.info.row_bytes(), &session)
            };
            let result = result.and_then(|_| {
                state.hash_band(&header.info, layout, work.id(), rect, band, &session)
            });
            let committed = admission.v4_commit(session.v4_resume());
            result?;
            committed?;
            band.groups += 1;
        }
        if closed_tile {
            tiles += 1;
        }
        if placement == DirectoryPlacement::Tail {
            remember(&mut entries, entry.entry, admission)?;
        }
        if let Some(front) = &mut front_entries {
            if Entry::read(front)? != entry.entry {
                return Err(Error::Corrupt("V4 streamed front directory mismatch"));
            }
            front_count += 1;
        }
        while bands
            .get(&next_row)
            .is_some_and(|band| band.groups == layout.group_grid().0)
        {
            let band = bands
                .remove(&next_row)
                .ok_or(Error::Corrupt("missing ready V4 band"))?;
            for row in band.pixels.chunks_exact(header.info.row_bytes()) {
                admission.check()?;
                output.row(&header.info, next_row, row).await?;
                next_row += 1;
            }
        }
        if src.end.is_none() && tiles == layout.tile_count() {
            break;
        }
    }
    let snapshot = admission.v4_resume();
    let session = snapshot.session(admission.v4_cancel());
    let result = state.finish(&session);
    let committed = admission.v4_commit(session.v4_resume());
    result?;
    committed?;
    if next_row != header.info.height || !bands.is_empty() {
        return Err(Error::Corrupt("incomplete V4 streamed rows"));
    }
    let objects_len = src.pos.checked_sub(objects_at).ok_or(Error::TooLarge)?;
    if let Some(front) = front
        && (front.entry_count() != front_count || front.objects_len() != objects_len)
    {
        return Err(Error::Corrupt("V4 streamed front directory extent"));
    }
    src.tail = Tail::V4(Index {
        placement,
        layout,
        integrity,
        entries,
        objects_len,
    });
    Ok(())
}
