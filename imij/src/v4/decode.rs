//! Ordinary decoding selects one grammar from the declared version.

use super::{LossyFile, LossyWork, NumericFile, NumericWork, TileLayout, TileLimits};
use crate::decode_resources::DecodeSession;
use crate::parse::limits::Admission;
use crate::planes::Rect;
use crate::{Error, Header, ImageInfo, Metadata};
use alloc::vec::Vec;
#[cfg(feature = "std")]
use how_far::ProgressExt;
use how_far::Pulse;

// These are fixed-size admitted views, matching the ordinary decoder's
// Prepared enum. Keep them on the stack rather than boxing a wire driver.
#[allow(clippy::large_enum_variant)]
pub(crate) enum ImageFile<'a, 's, C> {
    Numeric(NumericFile<'a, 's, C>),
    Lossy(LossyFile<'a, 's, C>),
}
pub(crate) enum GroupWork<'a> {
    Numeric(NumericWork<'a>),
    Lossy(LossyWork<'a>),
}
pub(crate) enum ImageEvent<'a> {
    Group(GroupWork<'a>),
    Tile(super::TilePixelCheck<'a>),
}
#[derive(Clone, Copy)]
pub(crate) struct Crop {
    pub rect: Rect,
    pub columns: u64,
    pub s0: u64,
    pub g0: u64,
    pub gn: u64,
}
impl Crop {
    fn ordinal(self, id: u64) -> Result<usize, Error> {
        let s = (id / self.columns)
            .checked_sub(self.s0)
            .ok_or(Error::Corrupt("V4 crop stripe"))?;
        let g = (id % self.columns)
            .checked_sub(self.g0)
            .filter(|&g| g < self.gn)
            .ok_or(Error::Corrupt("V4 crop column"))?;
        usize::try_from(
            s.checked_mul(self.gn)
                .and_then(|n| n.checked_add(g))
                .ok_or(Error::TooLarge)?,
        )
        .map_err(|_| Error::TooLarge)
    }
}
fn copy_group(
    rect: Rect,
    pixels: &[u8],
    row: usize,
    output: &mut [u8],
    stride: usize,
    bpp: usize,
    crop: Option<Crop>,
) -> Result<(), Error> {
    let target = crop.map_or(rect, |c| c.rect);
    let x0 = rect.x().max(target.x());
    let y0 = rect.y().max(target.y());
    let x1 = (rect.x() + rect.width()).min(target.x() + target.width());
    let y1 = (rect.y() + rect.height()).min(target.y() + target.height());
    if x0 >= x1 || y0 >= y1 {
        return Err(Error::Corrupt("V4 crop nonintersecting work"));
    }
    let width = (x1 - x0) as usize * bpp;
    for y in y0..y1 {
        let src = (y - rect.y()) as usize * row + (x0 - rect.x()) as usize * bpp;
        let dst = if crop.is_some() {
            (y - target.y()) as usize * stride + (x0 - target.x()) as usize * bpp
        } else {
            y as usize * stride + x0 as usize * bpp
        };
        output
            .get_mut(dst..dst + width)
            .ok_or(Error::InvalidInput("V4 output stride/extent"))?
            .copy_from_slice(&pixels[src..src + width]);
    }
    Ok(())
}

impl GroupWork<'_> {
    pub(crate) fn id(&self) -> u64 {
        match self {
            Self::Numeric(work) => work.entry.entry.id,
            Self::Lossy(work) => work.id(),
        }
    }
    pub(crate) fn rect(&self) -> Result<Rect, Error> {
        match self {
            Self::Numeric(work) => work.rect(),
            Self::Lossy(work) => work.rect(),
        }
    }
    pub(crate) fn decode<C: Fn() -> bool + Sync>(
        &self,
        output: &mut [u8],
        stride: usize,
        session: &DecodeSession<C>,
    ) -> Result<(), Error> {
        match self {
            Self::Numeric(work) => work.decode_into(output, stride, session),
            Self::Lossy(work) => work.decode_into(output, stride, session),
        }
        .map(|_| ())
    }
    /// Output already holds the reference rectangle. Retain only a local
    /// lossy group copy while reconstruction overwrites it.
    pub(crate) fn decode_reference<C: Fn() -> bool + Sync>(
        &self,
        info: &ImageInfo,
        output: &mut [u8],
        stride: usize,
        session: &DecodeSession<C>,
    ) -> Result<(), Error> {
        match self {
            Self::Numeric(work) => work.decode_reference(output, stride, session).map(|_| ()),
            Self::Lossy(work) => {
                let rect = work.rect()?;
                let mut local = info.clone();
                local.width = rect.width();
                local.height = rect.height();
                let row = local.row_bytes();
                let length = row
                    .checked_mul(local.height as usize)
                    .ok_or(Error::TooLarge)?;
                let _memory = session.v4_working(length as u128)?;
                let mut reference = Vec::new();
                reference
                    .try_reserve_exact(length)
                    .map_err(|_| Error::TooLarge)?;
                for y in 0..local.height as usize {
                    let at = y.checked_mul(stride).ok_or(Error::TooLarge)?;
                    reference.extend_from_slice(
                        output
                            .get(at..at.checked_add(row).ok_or(Error::TooLarge)?)
                            .ok_or(Error::InvalidInput("V4 reference output extent"))?,
                    );
                }
                work.decode_with_reference(
                    output,
                    stride,
                    Some(crate::ImageRef::packed(&reference, &local)),
                    session,
                )
                .map(|_| ())
            }
        }
    }
}
impl<'a, 's, C: Fn() -> bool + Sync> ImageFile<'a, 's, C> {
    #[cfg(feature = "std")]
    fn open(data: &'a [u8], session: &'s DecodeSession<C>) -> Result<Self, Error> {
        let file = super::file::parse_file(data, session)?;
        Self::from_parsed(file, data.len(), session, None)
    }
    pub(crate) fn from_parsed(
        file: crate::container::File<'a>,
        length: usize,
        session: &'s DecodeSession<C>,
        inherited: Option<&ImageInfo>,
    ) -> Result<Self, Error> {
        session.v4_output(&file.header.info)?;
        let limits = TileLimits {
            max_object_bytes: u64::MAX,
            max_tile_bytes: u64::MAX,
        };
        Ok(match file.header.coding {
            crate::Coding::Lossless => {
                Self::Numeric(NumericFile::from_parsed(file, length, limits, session)?)
            }
            crate::Coding::Lossy => Self::Lossy(LossyFile::from_inherited(
                file, length, limits, session, inherited,
            )?),
        })
    }
    fn header(&self) -> &Header {
        match self {
            Self::Numeric(file) => file.header(),
            Self::Lossy(file) => file.header(),
        }
    }
    fn pixelsum(&self) -> crate::pixelsum::Table<'a> {
        match self {
            Self::Numeric(file) => file.pixelsum(),
            Self::Lossy(file) => file.pixelsum(),
        }
    }
    fn layout(&self) -> TileLayout {
        match self {
            Self::Numeric(file) => file.layout(),
            Self::Lossy(file) => file.layout(),
        }
    }
    fn group_rect(&self, id: u64) -> Result<Rect, Error> {
        match self {
            Self::Numeric(file) => file.group_rect(id),
            Self::Lossy(file) => file.group_rect(id),
        }
    }
    pub(crate) fn visit(
        self,
        mut callback: impl FnMut(ImageEvent<'a>) -> Result<(), Error>,
    ) -> Result<(Header, Metadata), Error> {
        match self {
            Self::Numeric(file) => file.visit_checked(&mut callback),
            Self::Lossy(file) => file.visit_checked(&mut callback),
        }
    }
}

fn observing(pulse: Option<&dyn Pulse>) -> bool {
    #[cfg(feature = "std")]
    {
        pulse.is_some_and(|pulse| pulse.live().is_some())
    }
    #[cfg(not(feature = "std"))]
    {
        let _ = pulse;
        false
    }
}

pub(crate) fn into<C: Fn() -> bool + Sync>(
    data: &[u8],
    output: &mut [u8],
    stride: usize,
    session: &DecodeSession<C>,
) -> Result<(Header, Metadata), Error> {
    observed(data, output, stride, session, None)
}

pub(crate) fn observed<C: Fn() -> bool + Sync>(
    data: &[u8],
    output: &mut [u8],
    stride: usize,
    session: &DecodeSession<C>,
    pulse: Option<&dyn Pulse>,
) -> Result<(Header, Metadata), Error> {
    let (mut rows, mut checksum, _) =
        super::plan::Rows::new(alloc::borrow::Cow::Borrowed(data), session)?;
    let info = rows.info().clone();
    let extent = info
        .checked_stride_len(stride)
        .ok_or(Error::InvalidInput("V4 output stride/extent"))?;
    if output.len() < extent {
        return Err(Error::InvalidInput("V4 output stride/extent"));
    }
    let (progress, _progress_memory) = rows.progress(pulse)?;
    let cancel = session.v4_cancel();
    let check = || {
        if cancel.is_some_and(|cancel| cancel()) {
            return Err(Error::Cancelled);
        }
        if let Some(pulse) = pulse {
            pulse.check().map_err(|_| Error::Cancelled)?;
        }
        Ok(())
    };
    let result = (|| {
        for y in 0..info.height as usize {
            let row = &mut output[y * stride..][..info.row_bytes()];
            if !rows.next_row_observed(row, &check, progress.as_ref())? {
                return Err(Error::Corrupt("V4 primary image too short"));
            }
            if let Some(sum) = &mut checksum {
                sum.row_checked(row, &check)?;
            }
        }
        check()?;
        Ok(rows.description())
    })();
    session.v4_restore(rows.resume())?;
    result
}

/// Decode an admitted plane tile into private caller scratch.
/// Verify all tile and raster checks before the plane reader can publish it.
#[inline(never)]
pub(crate) fn parsed_into<C: Fn() -> bool + Sync>(
    data: &[u8],
    file: crate::container::File<'_>,
    output: &mut [u8],
    stride: usize,
    session: &DecodeSession<C>,
    workers: usize,
) -> Result<ImageInfo, Error> {
    let _models = session.v4_model_scope();
    let file = ImageFile::from_parsed(file, data.len(), session, None)?;
    run_file(file, output, stride, session, None, workers).map(|(header, _)| header.info)
}

#[cfg(feature = "std")]
fn run<C: Fn() -> bool + Sync>(
    data: &[u8],
    output: &mut [u8],
    stride: usize,
    session: &DecodeSession<C>,
    pulse: Option<&dyn Pulse>,
    requested: usize,
) -> Result<(Header, Metadata), Error> {
    let file = ImageFile::open(data, session)?;
    run_file(file, output, stride, session, pulse, requested)
}

pub(crate) fn run_file<C: Fn() -> bool + Sync>(
    file: ImageFile<'_, '_, C>,
    output: &mut [u8],
    stride: usize,
    session: &DecodeSession<C>,
    pulse: Option<&dyn Pulse>,
    requested: usize,
) -> Result<(Header, Metadata), Error> {
    let info = file.header().info.clone();
    let (columns, rows) = file.layout().group_grid();
    let count =
        usize::try_from(u64::from(columns) * u64::from(rows)).map_err(|_| Error::TooLarge)?;
    let workers = crate::par::worker_count(requested.min(available_workers()), count);
    let extent = info
        .checked_stride_len(stride)
        .ok_or(Error::InvalidInput("V4 output stride/extent"))?;
    if output.len() < extent {
        return Err(Error::InvalidInput("V4 output stride/extent"));
    }
    let sum = file.pixelsum();
    if matches!(sum, crate::pixelsum::Table::Known(..)) {
        session.admission().pixel_checksum(&info)?;
    }
    let mut progress_memory = None;
    let progress = if observing(pulse) {
        let pulse = pulse.unwrap();
        progress_memory = Some(session.v4_working(count as u128 * 1024)?);
        let mut shapes = Vec::new();
        shapes
            .try_reserve_exact(count)
            .map_err(|_| Error::TooLarge)?;
        for id in 0..count {
            let rect = file.group_rect(id as u64)?;
            shapes.push(crate::progress::UnitShape {
                stripe: id / columns as usize,
                group: id % columns as usize,
                groups: columns as usize,
                y: rect.y() as usize,
                rows: rect.height() as usize,
                x: rect.x() as usize,
                cols: rect.width() as usize,
            });
        }
        crate::progress::UnitProgress::plan(pulse, shapes.into_iter(), workers)
    } else {
        None
    };
    #[cfg(all(feature = "std", any(not(target_family = "wasm"), feature = "rayon")))]
    let result = if workers > 1 {
        let window = group_window(workers, count);
        let pending_memory =
            session.v4_working(window as u128 * core::mem::size_of::<GroupWork<'_>>() as u128)?;
        let mut pending = Vec::new();
        pending
            .try_reserve_exact(window)
            .map_err(|_| Error::TooLarge)?;
        let result = file.visit(|event| {
            let work = match event {
                ImageEvent::Group(work) => work,
                ImageEvent::Tile(tile) => {
                    flush(
                        &mut pending,
                        output,
                        stride,
                        &info,
                        session,
                        progress.as_ref(),
                        None,
                        workers,
                    )?;
                    tile.verify(&info, output, stride, session)?;
                    return Ok(());
                }
            };
            pending.push(work);
            if pending.len() == window {
                flush(
                    &mut pending,
                    output,
                    stride,
                    &info,
                    session,
                    progress.as_ref(),
                    None,
                    workers,
                )?;
            }
            Ok(())
        })?;
        flush(
            &mut pending,
            output,
            stride,
            &info,
            session,
            progress.as_ref(),
            None,
            workers,
        )?;
        drop(pending_memory);
        result
    } else {
        file.visit(|event| {
            let work = match event {
                ImageEvent::Group(work) => work,
                ImageEvent::Tile(tile) => {
                    tile.verify(&info, output, stride, session)?;
                    return Ok(());
                }
            };
            decode_one(
                work,
                output,
                stride,
                &info,
                session,
                progress.as_ref(),
                None,
            )
        })?
    };
    #[cfg(any(
        not(feature = "std"),
        all(target_family = "wasm", not(feature = "rayon"))
    ))]
    let result = file.visit(|event| {
        let work = match event {
            ImageEvent::Group(work) => work,
            ImageEvent::Tile(tile) => {
                tile.verify(&info, output, stride, session)?;
                return Ok(());
            }
        };
        decode_one(
            work,
            output,
            stride,
            &info,
            session,
            progress.as_ref(),
            None,
        )
    })?;
    sum.verify_checked(0, &info, output, stride, || session.adapter_work(64 * 1024))?;
    drop(progress_memory);
    Ok(result)
}

pub(crate) fn decode_one<C: Fn() -> bool + Sync>(
    work: GroupWork<'_>,
    output: &mut [u8],
    stride: usize,
    info: &ImageInfo,
    session: &DecodeSession<C>,
    progress: Option<&crate::progress::UnitProgress<'_>>,
    crop: Option<Crop>,
) -> Result<(), Error> {
    let rect = work.rect()?;
    let id = crop.map_or_else(
        || usize::try_from(work.id()).map_err(|_| Error::TooLarge),
        |crop| crop.ordinal(work.id()),
    )?;
    let decoded = (|| {
        if let Some(progress) = progress {
            progress.begin(id)?;
        }
        if crop.is_some() {
            let row = rect.width() as usize * info.bytes_per_pixel();
            let len = row
                .checked_mul(rect.height() as usize)
                .ok_or(Error::TooLarge)?;
            let _memory = session.v4_working(len as u128)?;
            let mut pixels = Vec::new();
            pixels.try_reserve_exact(len).map_err(|_| Error::TooLarge)?;
            pixels.resize(len, 0);
            work.decode(&mut pixels, row, session)?;
            copy_group(
                rect,
                &pixels,
                row,
                output,
                stride,
                info.bytes_per_pixel(),
                crop,
            )
        } else {
            let at = (rect.y() as usize)
                .checked_mul(stride)
                .and_then(|v| v.checked_add(rect.x() as usize * info.bytes_per_pixel()))
                .ok_or(Error::TooLarge)?;
            work.decode(&mut output[at..], stride, session)
        }
    })();
    if let Some(progress) = progress {
        progress.end(id, &decoded);
    }
    decoded
}

pub(crate) fn available_workers() -> usize {
    #[cfg(all(feature = "std", not(target_family = "wasm")))]
    {
        std::thread::available_parallelism().map_or(1, usize::from)
    }
    #[cfg(all(feature = "std", feature = "rayon", target_family = "wasm"))]
    {
        rayon::current_num_threads()
    }
    #[cfg(not(any(
        all(feature = "std", not(target_family = "wasm")),
        all(feature = "std", feature = "rayon", target_family = "wasm")
    )))]
    {
        1
    }
}

#[cfg(all(feature = "std", any(not(target_family = "wasm"), feature = "rayon")))]
pub(crate) fn group_window(workers: usize, groups: usize) -> usize {
    workers.saturating_mul(4).min(groups)
}

#[cfg(all(feature = "std", any(not(target_family = "wasm"), feature = "rayon")))]
fn flush_bands<C: Fn() -> bool + Sync>(
    pending: &mut Vec<GroupWork<'_>>,
    mut output: &mut [u8],
    stride: usize,
    info: &ImageInfo,
    session: &DecodeSession<C>,
    progress: Option<&crate::progress::UnitProgress<'_>>,
    crop: Option<Crop>,
    workers: usize,
) -> Result<bool, Error> {
    let target = crop.map_or(Rect::new(0, 0, info.width, info.height), |c| c.rect);
    let bottom = target.y() + target.height();
    let mut next_y = target.y();
    for work in pending.iter() {
        let rect = work.rect()?;
        if rect.x() != target.x()
            || rect.width() != target.width()
            || rect.height() == 0
            || rect.y() < next_y
            || rect.y() + rect.height() > bottom
        {
            return Ok(false);
        }
        next_y = rect.y() + rect.height();
    }

    let count = pending.len();
    let (policy, reserved) = session.v4_worker_policy(count as u128 * 512, workers)?;
    let memory = session.v4_working(u128::from(reserved))?;
    let mut bands = Vec::new();
    bands
        .try_reserve_exact(count)
        .map_err(|_| Error::TooLarge)?;
    let mut offset = 0;
    let row = target.width() as usize * info.bytes_per_pixel();
    for work in pending.drain(..) {
        let rect = work.rect()?;
        let start = (rect.y() - target.y()) as usize * stride;
        let len = (rect.height() as usize - 1) * stride + row;
        let rest = output
            .get_mut(start - offset..)
            .ok_or(Error::InvalidInput("V4 output stride/extent"))?;
        let (band, tail) = rest
            .split_at_mut_checked(len)
            .ok_or(Error::InvalidInput("V4 output stride/extent"))?;
        bands.push((work, band));
        output = tail;
        offset = start + len;
    }

    let cancel = session.v4_cancel();
    let budget = DecodeSession::<C>::v4_work_budget(policy);
    let results = crate::par::par_map(bands, workers, |_, (work, band)| {
        let worker = DecodeSession::v4_worker(policy, || cancel.is_some_and(|c| c()), &budget);
        decode_group(&work, band, stride, &worker, progress, crop)
    });
    session.v4_charge_workers(&budget)?;
    drop(memory);
    results.into_iter().collect::<Result<(), _>>()?;
    Ok(true)
}

#[cfg(all(feature = "std", any(not(target_family = "wasm"), feature = "rayon")))]
fn decode_group<C: Fn() -> bool + Sync>(
    work: &GroupWork<'_>,
    output: &mut [u8],
    stride: usize,
    session: &DecodeSession<C>,
    progress: Option<&crate::progress::UnitProgress<'_>>,
    crop: Option<Crop>,
) -> Result<(), Error> {
    let id = crop.map_or_else(
        || usize::try_from(work.id()).map_err(|_| Error::TooLarge),
        |crop| crop.ordinal(work.id()),
    )?;
    if let Some(progress) = progress {
        progress.begin(id)?;
    }
    let result = work.decode(output, stride, session);
    if let Some(progress) = progress {
        progress.end(id, &result);
    }
    result
}

#[cfg(all(feature = "std", any(not(target_family = "wasm"), feature = "rayon")))]
pub(crate) fn flush<C: Fn() -> bool + Sync>(
    pending: &mut Vec<GroupWork<'_>>,
    output: &mut [u8],
    stride: usize,
    info: &ImageInfo,
    session: &DecodeSession<C>,
    progress: Option<&crate::progress::UnitProgress<'_>>,
    crop: Option<Crop>,
    workers: usize,
) -> Result<(), Error> {
    if pending.is_empty() {
        return Ok(());
    }
    let workers = workers.min(pending.len());
    if flush_bands(
        pending, output, stride, info, session, progress, crop, workers,
    )? {
        return Ok(());
    }
    let bpp = info.bytes_per_pixel();
    let scratch = pending.iter().try_fold(0usize, |largest, work| {
        let rect = work.rect()?;
        let bytes = (rect.width() as usize)
            .checked_mul(rect.height() as usize)
            .and_then(|pixels| pixels.checked_mul(bpp))
            .ok_or(Error::TooLarge)?;
        Ok(largest.max(bytes))
    })?;
    let bytes = pending.len() as u128 * 512 + scratch as u128 * workers as u128;
    let (policy, reserved) = session.v4_worker_policy(bytes, workers)?;
    let _memory = session.v4_working(u128::from(reserved))?;
    let cancel = session.v4_cancel();
    let budget = DecodeSession::<C>::v4_work_budget(policy);
    let queue = std::sync::Mutex::new(pending.drain(..).enumerate());
    let output = std::sync::Mutex::new(output);
    let failure = std::sync::Mutex::new(None::<(usize, Error)>);
    let run = || {
        let worker = DecodeSession::v4_worker(policy, || cancel.is_some_and(|c| c()), &budget);
        let mut pixels = Vec::new();
        loop {
            let Some((index, work)) = queue.lock().unwrap().next() else {
                break;
            };
            let result = (|| {
                let rect = work.rect()?;
                let row = rect.width() as usize * bpp;
                let len = row * rect.height() as usize;
                if pixels.is_empty() {
                    pixels = bytemuck::allocation::try_zeroed_vec(scratch)
                        .map_err(|_| Error::TooLarge)?;
                }
                let group = &mut pixels[..len];
                decode_group(&work, group, row, &worker, progress, crop)?;
                copy_group(
                    rect,
                    group,
                    row,
                    &mut output.lock().unwrap(),
                    stride,
                    bpp,
                    crop,
                )
            })();
            if let Err(error) = result {
                let mut first = failure.lock().unwrap();
                if first.as_ref().is_none_or(|(earlier, _)| index < *earlier) {
                    *first = Some((index, error));
                }
            }
        }
    };
    if workers == 1 {
        run();
    } else {
        crate::par::scoped_reuse(workers, true, run);
    }
    session.v4_charge_workers(&budget)?;
    failure
        .into_inner()
        .unwrap()
        .map_or(Ok(()), |(_, error)| Err(error))
}

#[cfg(feature = "std")]
pub(crate) fn threaded(
    data: &[u8],
    output: &mut [u8],
    stride: usize,
    threads: usize,
    limits: &crate::Limits,
    pulse: Option<&dyn Pulse>,
) -> Result<ImageInfo, Error> {
    let session = DecodeSession::with_cancel(
        crate::decode_resources::DecodePolicy::default().with_output_limits(*limits),
        || pulse.is_some_and(|p| p.check().is_err()),
    );
    session.admit_input(data)?;
    let _input = session.v4_precharged()?;
    run(data, output, stride, &session, pulse, threads).map(|(header, _)| header.info)
}

pub(crate) fn allocating<C: Fn() -> bool + Sync>(
    data: &[u8],
    session: &DecodeSession<C>,
) -> Result<(ImageInfo, Vec<u8>), Error> {
    let header = super::header(data, session.v4_header_limit())?;
    session.v4_output(&header.info)?;
    let size = header.info.checked_buffer_len().ok_or(Error::TooLarge)?;
    let row = header.info.row_bytes();
    let mut output = bytemuck::allocation::try_zeroed_vec(size).map_err(|_| Error::TooLarge)?;
    let (header, _) = into(data, &mut output, row, session)?;
    Ok((header.info, output))
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::decode_resources::DecodePolicy;
    use crate::{Channels, ImageRef, ImiEncoderConfig, SampleType};
    use alloc::vec;
    fn fixture(lossy: bool, checksum: bool) -> (Vec<u8>, Vec<u8>, ImageInfo) {
        let info = ImageInfo::new(41, 31, Channels::Rgba, SampleType::U8);
        let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
            .map(|i| (i * 73 + i / 7 * 41) as u8)
            .collect();
        let config = if lossy {
            ImiEncoderConfig::lossy(71.0)
        } else {
            ImiEncoderConfig::lossless()
        }
        .with_checksum(checksum);
        let native = config
            .encode_samples(ImageRef::packed(&pixels, &info), &Metadata::default())
            .unwrap();
        let expected = crate::decode(&native).unwrap().1;
        (
            crate::v4::upgrade_image(&native, &crate::UpgradeLimits::default()).unwrap(),
            expected,
            info,
        )
    }
    #[test]
    fn ordinary_v4_decode_probe_metadata_and_cumulative_input_agree() {
        for lossy in [false, true] {
            for checksum in [false, true] {
                let (file, expected, info) = fixture(lossy, checksum);
                assert_eq!(crate::header(&file).unwrap().version, 4);
                assert_eq!(crate::decode(&file).unwrap().1, expected);
                let mut session = DecodeSession::new(
                    DecodePolicy::default().with_max_input_bytes(2 * file.len() as u64),
                );
                assert_eq!(session.decode(&file).unwrap().1, expected);
                assert_eq!(session.usage().input_bytes, file.len() as u64);
                let stride = info.row_bytes() + 13;
                let mut output = vec![0x9C; stride * info.height as usize];
                session.decode_into(&file, &mut output, stride).unwrap();
                assert_eq!(session.usage().input_bytes, 2 * file.len() as u64);
                for y in 0..info.height as usize {
                    assert_eq!(
                        &output[y * stride..][..info.row_bytes()],
                        &expected[y * info.row_bytes()..][..info.row_bytes()]
                    );
                    assert!(
                        output[y * stride + info.row_bytes()..(y + 1) * stride]
                            .iter()
                            .all(|&b| b == 0x9C)
                    );
                }
                assert!(matches!(session.decode(&file), Err(Error::TooLarge)));
                let mut probe = DecodeSession::new(DecodePolicy::default());
                let inspection = probe.inspect(&file, |_| Ok(true)).unwrap();
                assert_eq!(inspection.info, info);
                assert_eq!(probe.usage().input_bytes, file.len() as u64);
                let cancelled =
                    DecodeSession::with_cancel(DecodePolicy::default(), || true).decode(&file);
                assert!(matches!(cancelled, Err(Error::Cancelled)));
            }
        }
    }
    #[cfg(feature = "std")]
    #[test]
    fn parallel_groups_share_the_work_limit() {
        let parent = DecodeSession::new(
            crate::decode_resources::DecodePolicy::default().with_max_work_units(17),
        );
        parent.adapter_work(2).unwrap();
        let (policy, _) = parent.v4_worker_policy(0, 2).unwrap();
        let budget = DecodeSession::<fn() -> bool>::v4_work_budget(policy);
        let left = DecodeSession::v4_worker(policy, || false, &budget);
        let right = DecodeSession::v4_worker(policy, || false, &budget);

        left.v4_reserve_work(9).unwrap();
        left.adapter_work(9).unwrap();
        assert!(matches!(right.adapter_work(7), Err(Error::TooLarge)));
        right.adapter_work(6).unwrap();
        parent.v4_charge_workers(&budget).unwrap();
        assert_eq!(parent.usage().work_units, 17);
        assert!(matches!(parent.adapter_work(1), Err(Error::TooLarge)));
    }

    #[cfg(feature = "std")]
    #[test]
    fn ordinary_threaded_v4_batches_match_serial_pixels_and_padding() {
        for lossy in [false, true] {
            for checksum in [false, true] {
                let info = ImageInfo::new(67, 53, Channels::Rgba, SampleType::U8);
                let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
                    .map(|i| (i * 73 + i / 7 * 41) as u8)
                    .collect();
                let config = if lossy {
                    ImiEncoderConfig::lossy(71.0)
                        .with_lossy_tuning(crate::LossyTuning::default().with_stripe_block_rows(2))
                        .with_column_groups(Some(16))
                } else {
                    ImiEncoderConfig::lossless().with_row_groups(crate::RowGroups::new(
                        13,
                        9,
                        8 << 20,
                    ))
                }
                .with_checksum(checksum);
                let native = config
                    .encode_samples(ImageRef::packed(&pixels, &info), &Metadata::default())
                    .unwrap();
                let expected = crate::decode(&native).unwrap().1;
                let file =
                    crate::v4::upgrade_image(&native, &crate::UpgradeLimits::default()).unwrap();
                let stride = info.row_bytes() + 19;
                for threads in [1, 2, 4, 8, usize::MAX] {
                    let mut output = vec![0xA7; stride * info.height as usize];
                    assert_eq!(
                        crate::decode_into_threads_with_limits(
                            &file,
                            &mut output,
                            stride,
                            threads,
                            &crate::Limits::DEFAULT
                        )
                        .unwrap(),
                        info
                    );
                    for y in 0..info.height as usize {
                        assert_eq!(
                            &output[y * stride..][..info.row_bytes()],
                            &expected[y * info.row_bytes()..][..info.row_bytes()]
                        );
                        assert!(
                            output[y * stride + info.row_bytes()..(y + 1) * stride]
                                .iter()
                                .all(|&b| b == 0xA7)
                        );
                    }
                }
            }
        }
    }

    #[cfg(feature = "std")]
    #[test]
    fn parallel_full_width_bands_accept_an_unpadded_last_row() {
        let info = ImageInfo::new(67, 53, Channels::Rgb, SampleType::U8);
        let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
            .map(|i| (i * 73 + i / 7 * 41) as u8)
            .collect();
        let native = ImiEncoderConfig::lossy(71.0)
            .with_lossy_tuning(crate::LossyTuning::default().with_stripe_block_rows(2))
            .with_column_groups(None)
            .encode_samples(ImageRef::packed(&pixels, &info), &Metadata::default())
            .unwrap();
        let file = crate::v4::upgrade_image(&native, &crate::UpgradeLimits::default()).unwrap();
        let expected = crate::decode(&native).unwrap().1;
        let stride = info.row_bytes() + 19;
        let mut output = vec![0xa7; info.checked_stride_len(stride).unwrap()];
        let session = DecodeSession::new(DecodePolicy::default());
        let decoded = ImageFile::open(&file, &session).unwrap();
        assert_eq!(decoded.layout().group_grid(), (1, 4));
        crate::decode_into_threads(&file, &mut output, stride, 4).unwrap();
        for (row, expected) in output.chunks(stride).zip(expected.chunks(info.row_bytes())) {
            assert_eq!(&row[..info.row_bytes()], expected);
            assert!(row[info.row_bytes()..].iter().all(|&p| p == 0xa7));
        }
    }

    #[test]
    fn failed_v4_slice_keeps_its_input_charge_and_releases_precharged_scope() {
        let (file, _, _) = fixture(true, false);
        let mut broken = file.clone();
        let mut r = crate::ByteReader::new(&broken);
        crate::parse::header::read_header_v4(&mut r, 64 << 10).unwrap();
        let at = r.pos;
        broken[at] = b'Z';
        let mut session =
            DecodeSession::new(DecodePolicy::default().with_max_input_bytes(2 * file.len() as u64));
        assert!(session.decode(&broken).is_err());
        assert_eq!(session.usage().input_bytes, file.len() as u64);
        session.decode(&file).unwrap();
        assert_eq!(session.usage().input_bytes, 2 * file.len() as u64);
    }
    #[test]
    fn explicit_numeric_writer_finishes_and_verifies_outer_checksums() {
        let info = ImageInfo::new(41, 31, Channels::Rgba, SampleType::U16);
        let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
            .map(|i| (i * 73) as u8)
            .collect();
        let config = ImiEncoderConfig::quick()
            .with_row_groups(crate::RowGroups::new(9, 7, 8 << 20))
            .with_checksum(true);
        let spatial = crate::v4::SpatialConfig {
            tile_columns: 3,
            tile_rows: 2,
            order: crate::v4::TileOrder::Morton,
            integrity: crate::v4::ObjectIntegrity::ObjectsAndTiles,
            limits: TileLimits {
                max_object_bytes: 1 << 20,
                max_tile_bytes: 16 << 20,
            },
        };
        let file = crate::v4::encode_numeric(
            info.clone(),
            &pixels,
            info.row_bytes(),
            &Metadata::default(),
            &config,
            spatial,
        )
        .unwrap();
        assert_eq!(crate::decode(&file).unwrap().1, pixels);
        let check = DecodeSession::new(DecodePolicy::default());
        let parsed = crate::v4::file::parse_file(&file, &check).unwrap();
        assert!(parsed.checksum.and_then(|sum| sum.pieces).is_some());
        let mut broken = file.clone();
        let last = broken.len() - 1;
        broken[last] ^= 1;
        assert!(crate::decode(&broken).is_err());
    }
}
