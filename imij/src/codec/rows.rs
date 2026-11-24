//! Row adapters around the caller-driven group writer.

use super::{EncodeRequest, ImageEncoder, encode_description, encode_description_for};
use crate::stream_encode::GroupEncodeSession;
use crate::{Error, ImageInfo, Metadata};
use alloc::vec::Vec;
use how_far::Report;
use zencodec::encode::{EncodeOutput, Encoder};
use zenpixels::{PixelDescriptor, PixelSlice, PixelSliceMut};

pub(super) struct RowEncoder {
    session: GroupEncodeSession,
    descriptor: PixelDescriptor,
    info: ImageInfo,
    metadata: Metadata,
    /// Encoded bytes not yet returned; `output[..sent]` were handed out
    /// through `pending` (only after `begin`).
    output: Vec<u8>,
    sent: usize,
    /// Bytes handed out and dropped from `output`, for the output limit.
    emitted: u64,
    /// `begin` declared the image: `pending` exposes the output.
    streaming: bool,
    row: u32,
    input: u64,
}

fn memory(job: &EncodeRequest, output: usize, extra: usize) -> Result<(), Error> {
    let group = job.group_config("row_level_encode")?;
    let planned = (group.max_working_bytes as u128)
        + output as u128
        + extra as u128
        + core::mem::size_of::<ImageEncoder>() as u128;
    if job
        .limits
        .max_memory_bytes
        .is_some_and(|cap| planned > u128::from(cap))
    {
        return Err(Error::TooLarge);
    }
    Ok(())
}

impl RowEncoder {
    fn new(job: &EncodeRequest, pixels: &PixelSlice<'_>, extra: usize) -> Result<Self, Error> {
        let (width, height) = job.canvas.ok_or(Error::InvalidInput(
            "row encoding requires canvas dimensions",
        ))?;
        if pixels.width() != width || pixels.rows() > height {
            return Err(Error::InvalidInput("row canvas mismatch"));
        }
        let (info, metadata) = encode_description(
            pixels,
            &job.metadata,
            &job.policy,
            job.color_origin.as_ref(),
        )?;
        Self::start(job, pixels.descriptor(), (info, metadata), extra)
    }

    /// Start from a declaration (`begin`): no pixels yet, so no pixel colour
    /// context; colour comes from the job.
    fn declared(
        job: &EncodeRequest,
        descriptor: PixelDescriptor,
        extra: usize,
    ) -> Result<Self, Error> {
        let canvas = job.canvas.ok_or(Error::InvalidInput(
            "row encoding requires canvas dimensions",
        ))?;
        let description = encode_description_for(
            descriptor,
            canvas,
            None,
            &job.metadata,
            &job.policy,
            job.color_origin.as_ref(),
        )?;
        let mut this = Self::start(job, descriptor, description, extra)?;
        this.streaming = true;
        Ok(this)
    }

    fn start(
        job: &EncodeRequest,
        descriptor: PixelDescriptor,
        (mut info, metadata): (ImageInfo, Metadata),
        extra: usize,
    ) -> Result<Self, Error> {
        job.check_stop()?;
        let cfg = job.group_config("row_level_encode")?;
        let (width, height) = job.canvas.ok_or(Error::InvalidInput(
            "row encoding requires canvas dimensions",
        ))?;
        if width == 0 || height == 0 {
            return Err(Error::InvalidInput("zero image width or height"));
        }
        job.limits
            .check_dimensions(width, height)
            .map_err(|_| Error::TooLarge)?;
        job.limits
            .check_total_pixels(u64::from(width) * u64::from(height))
            .map_err(|_| Error::TooLarge)?;
        job.limits.check_frames(1).map_err(|_| Error::TooLarge)?;
        memory(job, 0, extra)?;
        info.height = height;
        let session = GroupEncodeSession::with_groups(info.clone(), &metadata, cfg)?;
        let mut this = Self {
            session,
            descriptor,
            info,
            metadata,
            output: Vec::new(),
            sent: 0,
            emitted: 0,
            streaming: false,
            row: 0,
            input: 0,
        };
        this.drain(job, extra)?;
        Ok(this)
    }

    fn drain(&mut self, job: &EncodeRequest, extra: usize) -> Result<(), Error> {
        job.check_stop()?;
        let bytes = self.session.output();
        let length = self
            .output
            .len()
            .checked_add(bytes.len())
            .ok_or(Error::TooLarge)?;
        job.limits
            .check_output_size(self.emitted.saturating_add(length as u64))
            .map_err(|_| Error::TooLarge)?;
        let capacity = self.output.capacity();
        if length > capacity {
            // Grow geometrically, so a caller that never drains pays amortized
            // reallocation rather than one per group (an exact reserve per
            // group allocated 2,107x the file in total). Fall back to the
            // exact size when the memory limit cannot admit doubling.
            // Admission counts both old and replacement allocations.
            let doubled = length.max(capacity.saturating_mul(2));
            let target = match capacity.checked_add(doubled) {
                Some(both) if memory(job, both, extra).is_ok() => doubled,
                _ => {
                    memory(
                        job,
                        capacity.checked_add(length).ok_or(Error::TooLarge)?,
                        extra,
                    )?;
                    length
                }
            };
            self.output
                .try_reserve_exact(target - self.output.len())
                .map_err(|_| Error::TooLarge)?;
        }
        memory(job, self.output.capacity(), extra)?;
        self.output.extend_from_slice(bytes);
        self.session.consume_output(bytes.len())?;
        Ok(())
    }

    fn push(
        &mut self,
        job: &EncodeRequest,
        pixels: PixelSlice<'_>,
        extra: usize,
    ) -> Result<(), Error> {
        job.check_stop()?;
        let end = self.row.checked_add(pixels.rows()).ok_or(Error::TooLarge)?;
        if pixels.rows() == 0 || end > self.info.height || pixels.descriptor() != self.descriptor {
            return Err(Error::InvalidInput("row count or descriptor mismatch"));
        }
        let (mut info, metadata) = encode_description(
            &pixels,
            &job.metadata,
            &job.policy,
            job.color_origin.as_ref(),
        )?;
        info.height = self.info.height;
        if info != self.info || metadata != self.metadata {
            return Err(Error::InvalidInput("row description changed"));
        }
        let input = self
            .input
            .checked_add(pixels.as_strided_bytes().len() as u64)
            .ok_or(Error::TooLarge)?;
        job.limits
            .check_input_size(input)
            .map_err(|_| Error::TooLarge)?;
        let bpp = self.info.bytes_per_pixel();
        for y in 0..pixels.rows() {
            let row = pixels.row(y);
            while let Some(rect) = self.session.next_group() {
                if rect.y != self.row + y {
                    break;
                }
                job.check_stop()?;
                let start = (rect.x as usize).checked_mul(bpp).ok_or(Error::TooLarge)?;
                let length = (rect.width as usize)
                    .checked_mul(bpp)
                    .ok_or(Error::TooLarge)?;
                self.session
                    .push_group(&row[start..start + length], length)?;
                self.drain(job, extra)?;
            }
            // Count encoded work, not source callbacks or accepted batches.
            // No row earns progress until every column group has drained.
            if let Some(progress) = &job.progress {
                progress.advance(1);
            }
            job.check_stop()?;
        }
        job.check_stop()?;
        self.row = end;
        self.input = input;
        Ok(())
    }

    pub(super) fn finish(mut self) -> Result<Vec<u8>, Error> {
        if self.row != self.info.height {
            return Err(Error::InvalidInput("incomplete rows"));
        }
        self.session.finish()?;
        self.output.drain(..self.sent);
        Ok(self.output)
    }

    /// Encoded bytes ready to send, once `begin` declared the image.
    pub(super) fn pending(&self) -> &[u8] {
        if self.streaming {
            &self.output[self.sent..]
        } else {
            &[]
        }
    }

    /// Append the pending bytes to `into` and mark them sent. With `into`
    /// empty and nothing partly sent, the output buffer itself moves into
    /// `into`; the next group starts a new one, admitted against the memory
    /// limit as it grows.
    pub(super) fn take(&mut self, into: &mut Vec<u8>) {
        if !self.streaming || self.output.len() == self.sent {
            return;
        }
        if into.is_empty() && self.sent == 0 {
            self.emitted += self.output.len() as u64;
            *into = core::mem::take(&mut self.output);
        } else {
            let n = self.output.len() - self.sent;
            into.extend_from_slice(&self.output[self.sent..]);
            self.consume(n);
        }
    }

    /// Mark the first `n` pending bytes as sent.
    pub(super) fn consume(&mut self, n: usize) {
        if !self.streaming {
            return;
        }
        self.sent = (self.sent + n).min(self.output.len());
        // Reclaim sent bytes when all are gone (a driver draining after
        // every push), or when they dominate the buffer.
        if self.sent == self.output.len()
            || (self.sent >= 64 * 1024 && self.sent > self.output.len() / 2)
        {
            self.output.drain(..self.sent);
            self.emitted += self.sent as u64;
            self.sent = 0;
        }
    }
}

impl ImageEncoder {
    pub(super) fn begin_inner(
        &mut self,
        width: u32,
        height: u32,
        descriptor: PixelDescriptor,
    ) -> Result<zencodec::encode::OutputTiming, Error> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        match &self.rows {
            Some(rows) if rows.streaming => {
                return Err(Error::InvalidInput("begin called twice"));
            }
            Some(_) => return Err(Error::InvalidInput("begin after rows")),
            None => {}
        }
        self.job.check_stop()?;
        if self.job.config.row_groups().is_none() {
            // Whole-image encoders analyse every pixel before writing.
            return Err(Error::Unsupported(
                zencodec::UnsupportedOperation::RowLevelEncode.name(),
            ));
        }
        if self.job.canvas.is_some_and(|c| c != (width, height)) {
            return Err(Error::InvalidInput("row canvas mismatch"));
        }
        if self.job.input_descriptor.is_some_and(|d| d != descriptor) {
            return Err(Error::InvalidInput("input descriptor mismatch"));
        }
        let canvas = self.job.canvas.replace((width, height));
        let input = self.job.input_descriptor.replace(descriptor);
        match RowEncoder::declared(&self.job, descriptor, 0) {
            Ok(rows) => self.rows = Some(rows),
            Err(e) => {
                // Nothing started: leave the job as it was.
                self.job.canvas = canvas;
                self.job.input_descriptor = input;
                return Err(e);
            }
        }
        // Groups are one row tall and each is written as its row arrives.
        Ok(zencodec::encode::OutputTiming::streaming(0))
    }

    pub(super) fn push_rows_inner(
        &mut self,
        pixels: PixelSlice<'_>,
        extra: usize,
    ) -> Result<(), Error> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        self.job.check_stop()?;
        #[cfg(feature = "std")]
        if self.job.is_bounded() {
            let result = self.push_bounded(&pixels);
            if let Err(error) = &result {
                self.failed = Some(error.clone());
                self.spool = None;
            }
            return result;
        }
        if self.job.config.row_groups().is_none() {
            return Err(Error::Unsupported("row_level_encode"));
        }
        if self
            .job
            .input_descriptor
            .is_some_and(|d| d != pixels.descriptor())
        {
            return Err(Error::InvalidInput("input descriptor mismatch"));
        }
        // Admit even the first input batch before allocating the native header.
        let previous = self.rows.as_ref().map_or(0, |r| r.input);
        let input = previous
            .checked_add(pixels.as_strided_bytes().len() as u64)
            .ok_or(Error::TooLarge)?;
        self.job
            .limits
            .check_input_size(input)
            .map_err(|_| Error::TooLarge)?;
        if self.rows.is_none() {
            self.rows = Some(RowEncoder::new(&self.job, &pixels, extra)?);
        }
        self.rows
            .as_mut()
            .ok_or(Error::InvalidInput("missing row encoder"))?
            .push(&self.job, pixels, extra)
    }
}

pub(super) fn pull(
    mut encoder: ImageEncoder,
    source: &mut dyn FnMut(u32, PixelSliceMut<'_>) -> usize,
) -> Result<EncodeOutput, Error> {
    encoder.job.check_stop()?;
    if encoder.job.config.row_groups().is_none() {
        return Err(Error::Unsupported("pull_encode"));
    }
    if let Some(error) = encoder.failed {
        return Err(error);
    }
    if encoder.rows.is_some() {
        return Err(Error::InvalidInput("row encoding already started"));
    }
    let (width, height) = encoder.job.canvas.ok_or(Error::InvalidInput(
        "pull encoding requires canvas dimensions",
    ))?;
    let descriptor = encoder.job.input_descriptor.ok_or(Error::InvalidInput(
        "pull encoding requires input descriptor",
    ))?;
    if width == 0 || height == 0 {
        return Err(Error::InvalidInput("zero pull canvas"));
    }
    encoder
        .job
        .limits
        .check_dimensions(width, height)
        .map_err(|_| Error::TooLarge)?;
    encoder
        .job
        .limits
        .check_total_pixels(u64::from(width) * u64::from(height))
        .map_err(|_| Error::TooLarge)?;
    encoder
        .job
        .limits
        .check_frames(1)
        .map_err(|_| Error::TooLarge)?;
    let row = (width as usize)
        .checked_mul(descriptor.bytes_per_pixel())
        .ok_or(Error::TooLarge)?;
    encoder
        .job
        .limits
        .check_input_size(
            (row as u64)
                .checked_mul(u64::from(height))
                .ok_or(Error::TooLarge)?,
        )
        .map_err(|_| Error::TooLarge)?;
    memory(&encoder.job, 0, row)?;
    let mut buffer = Vec::new();
    buffer.try_reserve_exact(row).map_err(|_| Error::TooLarge)?;
    memory(&encoder.job, 0, buffer.capacity())?;
    buffer.resize(row, 0);
    // Validate metadata, format and group geometry before invoking the source.
    let first = PixelSlice::new(&buffer, width, 1, row, descriptor)
        .map_err(|_| Error::InvalidInput("pull row layout"))?;
    encoder.rows = Some(RowEncoder::new(&encoder.job, &first, buffer.capacity())?);
    for y in 0..height {
        encoder.job.check_stop()?;
        let pixels = PixelSliceMut::new(&mut buffer, width, 1, row, descriptor)
            .map_err(|_| Error::InvalidInput("pull row layout"))?;
        if source(y, pixels) != 1 {
            return Err(Error::InvalidInput("pull source must fill one row"));
        }
        encoder.job.check_stop()?;
        let pixels = PixelSlice::new(&buffer, width, 1, row, descriptor)
            .map_err(|_| Error::InvalidInput("pull row layout"))?;
        encoder.push_rows_inner(pixels, buffer.capacity())?;
    }
    drop(buffer);
    encoder.finish()
}

#[cfg(feature = "std")]
impl ImageEncoder {
    /// Bounded lossy: spools `pixels` (the job's limits and descriptor
    /// checks apply as to row groups).
    fn push_bounded(&mut self, pixels: &PixelSlice<'_>) -> Result<(), Error> {
        if self
            .job
            .input_descriptor
            .is_some_and(|d| d != pixels.descriptor())
        {
            return Err(Error::InvalidInput("input descriptor mismatch"));
        }
        let rows = self.spool.as_ref().map_or(0, |s| s.rows());
        let input = (u64::from(rows) + u64::from(pixels.rows()))
            .checked_mul(pixels.width() as u64 * pixels.descriptor().bytes_per_pixel() as u64)
            .ok_or(Error::TooLarge)?;
        self.job
            .limits
            .check_input_size(input)
            .map_err(|_| Error::TooLarge)?;
        if self.spool.is_none() {
            let (width, height) = self.job.canvas.ok_or(Error::InvalidInput(
                "row encoding requires canvas dimensions",
            ))?;
            self.job
                .limits
                .check_dimensions(width, height)
                .map_err(|_| Error::TooLarge)?;
            self.job
                .limits
                .check_total_pixels(u64::from(width) * u64::from(height))
                .map_err(|_| Error::TooLarge)?;
            self.job.configure_threads();
            self.spool = Some(super::bounded::RowSpool::new(&self.job, pixels)?);
        }
        self.spool
            .as_mut()
            .ok_or(Error::InvalidInput("missing row spool"))?
            .push(&self.job, pixels)
    }
}

/// `encode_from` for bounded lossy: every row pulled into the spool, then
/// encoded from it.
#[cfg(feature = "std")]
pub(super) fn pull_bounded(
    mut encoder: ImageEncoder,
    source: &mut dyn FnMut(u32, PixelSliceMut<'_>) -> usize,
) -> Result<EncodeOutput, Error> {
    encoder.job.check_stop()?;
    if let Some(error) = encoder.failed.take() {
        return Err(error);
    }
    if encoder.spool.is_some() {
        return Err(Error::InvalidInput("row encoding already started"));
    }
    let (width, height) = encoder.job.canvas.ok_or(Error::InvalidInput(
        "pull encoding requires canvas dimensions",
    ))?;
    let descriptor = encoder.job.input_descriptor.ok_or(Error::InvalidInput(
        "pull encoding requires input descriptor",
    ))?;
    if width == 0 || height == 0 {
        return Err(Error::InvalidInput("zero pull canvas"));
    }
    let row = (width as usize)
        .checked_mul(descriptor.bytes_per_pixel())
        .ok_or(Error::TooLarge)?;
    encoder.job.configure_threads();
    let spool = super::bounded::RowSpool::declared(&encoder.job, descriptor)?;
    let mut buffer = spool.pull_buffer(row)?;
    encoder.spool = Some(spool);
    for y in 0..height {
        encoder.job.check_stop()?;
        let pixels = PixelSliceMut::new(buffer.bytes_mut(), width, 1, row, descriptor)
            .map_err(|_| Error::InvalidInput("pull row layout"))?;
        if source(y, pixels) != 1 {
            return Err(Error::InvalidInput("pull source must fill one row"));
        }
        let pixels = PixelSlice::new(buffer.as_bytes(), width, 1, row, descriptor)
            .map_err(|_| Error::InvalidInput("pull row layout"))?;
        encoder.push_bounded(&pixels)?;
    }
    drop(buffer);
    encoder.finish()
}
