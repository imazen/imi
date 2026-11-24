//! Bounded-memory lossy encoding through the common job, with scratch the
//! codec owns: [`super::EncodeRequest::with_scratch`].
//!
//! The native encoder ([`crate::stream_encode::encode_lossy_bounded`]) reads
//! its source two or more times and takes caller-owned scratch. Here the
//! adapter supplies both: `encode` reads the caller's pixels in place, while
//! pushed and pulled rows go to a source spool first. Index, checksum and
//! source spools live in [`Scratch`]: heap buffers or temporary files.

use super::{EncodeRequest, encode_description, encode_description_for};
use crate::stream_encode::StreamEncodeError;
use crate::v4::BoundedLossyConfig;
use crate::{Error, ImageInfo, Metadata};
use alloc::vec::Vec;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use zenpixels::{PixelDescriptor, PixelSlice};

/// Where [`super::EncodeRequest::with_scratch`] keeps what it does not hold
/// in working memory: the unit index and spooled records, the file-checksum
/// digests, and (for pushed or pulled rows) the source raster.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Scratch {
    /// Heap buffers. They count against the job's `max_memory_bytes`; a
    /// spooled source raster is as large as the raster.
    Memory,
    /// Temporary files created in this directory and removed when the
    /// encode ends, also after an error. They do not count against
    /// `max_memory_bytes`.
    Dir(PathBuf),
}

/// One scratch stream.
pub(super) enum Store {
    Memory(Memory),
    File(TempFile),
}

/// A single account shared by source, index, checksum and output buffers.
/// The native working reservation is withheld before this account is created.
#[derive(Clone)]
pub(super) struct Budget {
    used: Arc<AtomicU64>,
    cap: u64,
}

impl Budget {
    pub(super) fn new(cap: u64) -> Self {
        Self {
            used: Arc::new(AtomicU64::new(0)),
            cap,
        }
    }
    fn claim(&self, bytes: usize) -> std::io::Result<()> {
        let mut used = self.used.load(Ordering::Relaxed);
        loop {
            let next = used
                .checked_add(bytes as u64)
                .filter(|&n| n <= self.cap)
                .ok_or(std::io::ErrorKind::OutOfMemory)?;
            match self
                .used
                .compare_exchange_weak(used, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return Ok(()),
                Err(actual) => used = actual,
            }
        }
    }
    fn release(&self, bytes: usize) {
        self.used.fetch_sub(bytes as u64, Ordering::Relaxed);
    }
}

/// Growth is admitted before allocation, including coexistence of old and
/// replacement buffers. Exact reservations avoid Vec's implicit doubling.
pub(super) struct Memory {
    cursor: Cursor<Vec<u8>>,
    budget: Budget,
}

impl Memory {
    pub(super) fn new(budget: Budget) -> Self {
        Self {
            cursor: Cursor::new(Vec::new()),
            budget,
        }
    }
    fn reserve_end(&mut self, end: usize) -> std::io::Result<()> {
        let old = self.cursor.get_ref().capacity();
        if end <= old {
            return Ok(());
        }
        let preferred = end.max(old.saturating_mul(2)).max(4096);
        let size = if self.budget.claim(preferred).is_ok() {
            preferred
        } else {
            self.budget.claim(end)?;
            end
        };
        let mut replacement = Vec::new();
        if replacement.try_reserve_exact(size).is_err() {
            self.budget.release(size);
            return Err(std::io::ErrorKind::OutOfMemory.into());
        }
        let capacity = replacement.capacity();
        if capacity > size
            && let Err(e) = self.budget.claim(capacity - size)
        {
            self.budget.release(size);
            return Err(e);
        }
        replacement.extend_from_slice(self.cursor.get_ref());
        *self.cursor.get_mut() = replacement;
        self.budget.release(old);
        Ok(())
    }
    pub(super) fn as_bytes(&self) -> &[u8] {
        self.cursor.get_ref()
    }
    /// Retain the allocation but release a caller-drained append-only prefix.
    /// Native absolute output offsets are translated by the retained adapter.
    pub(super) fn consume_prefix(&mut self, count: usize) -> usize {
        let count = count.min(self.cursor.get_ref().len());
        let remaining = self.cursor.get_ref().len() - count;
        self.cursor.get_mut().copy_within(count.., 0);
        self.cursor.get_mut().truncate(remaining);
        self.cursor
            .set_position(self.cursor.position().saturating_sub(count as u64));
        count
    }

    pub(super) fn bytes_mut(&mut self) -> &mut [u8] {
        self.cursor.get_mut()
    }
    pub(super) fn into_vec(mut self) -> Vec<u8> {
        self.budget.release(self.cursor.get_ref().capacity());
        core::mem::take(self.cursor.get_mut())
    }
}
impl Drop for Memory {
    fn drop(&mut self) {
        self.budget.release(self.cursor.get_ref().capacity());
    }
}
impl Write for Memory {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let end = usize::try_from(self.cursor.position())
            .ok()
            .and_then(|p| p.checked_add(buf.len()))
            .ok_or(std::io::ErrorKind::OutOfMemory)?;
        self.reserve_end(end)?;
        self.cursor.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl Read for Memory {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.cursor.read(buf)
    }
}
impl Seek for Memory {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.cursor.seek(pos)
    }
}

struct Output<'a> {
    memory: Memory,
    job: &'a EncodeRequest,
}
impl Write for Output<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let end = self
            .memory
            .cursor
            .position()
            .checked_add(buf.len() as u64)
            .ok_or(std::io::ErrorKind::OutOfMemory)?;
        self.job
            .limits
            .check_output_size(end)
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::OutOfMemory))?;
        self.memory.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl Seek for Output<'_> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.memory.seek(pos)
    }
}

/// A file removed on drop.
pub(super) struct TempFile {
    file: std::fs::File,
    path: PathBuf,
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Store {
    pub(super) fn open(scratch: &Scratch, budget: Budget) -> Result<Self, Error> {
        match scratch {
            Scratch::Memory => Ok(Self::Memory(Memory::new(budget))),
            Scratch::Dir(dir) => {
                use core::sync::atomic::{AtomicU64, Ordering};
                static NEXT: AtomicU64 = AtomicU64::new(0);
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos() as u64);
                for _ in 0..16 {
                    let n = NEXT.fetch_add(1, Ordering::Relaxed);
                    let path = dir.join(alloc::format!(
                        ".imij-scratch-{}-{nanos:x}-{n}",
                        std::process::id()
                    ));
                    match std::fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .create_new(true)
                        .open(&path)
                    {
                        Ok(file) => return Ok(Self::File(TempFile { file, path })),
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                        Err(_) => return Err(Error::InvalidInput("scratch directory")),
                    }
                }
                Err(Error::InvalidInput("scratch directory"))
            }
        }
    }
}

impl Read for Store {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Memory(c) => c.read(buf),
            Self::File(f) => f.file.read(buf),
        }
    }
}

impl Write for Store {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Memory(c) => c.write(buf),
            Self::File(f) => f.file.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Memory(_) => Ok(()),
            Self::File(f) => f.file.flush(),
        }
    }
}

impl Seek for Store {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        match self {
            Self::Memory(c) => c.seek(pos),
            Self::File(f) => f.file.seek(pos),
        }
    }
}

pub(super) fn codec_error(e: StreamEncodeError) -> Error {
    match e {
        StreamEncodeError::Codec(e) => e,
        StreamEncodeError::Read(e) | StreamEncodeError::Write(e)
            if e.kind() == std::io::ErrorKind::OutOfMemory =>
        {
            Error::TooLarge
        }
        // Scratch and output streams are the adapter's own; a failure there
        // is reported like a failed source read.
        StreamEncodeError::Read(_) | StreamEncodeError::Write(_) => Error::Read,
    }
}

impl EncodeRequest {
    pub(super) fn scratch(&self) -> Option<&Scratch> {
        self.scratch.as_ref()
    }

    fn bounded_budget(&self, info: &ImageInfo) -> Result<Budget, Error> {
        let working = crate::lossy::bounded_working_bytes(info, &self.config.lossy_config())?;
        let remaining = self
            .limits
            .max_memory_bytes
            .unwrap_or(u64::MAX)
            .checked_sub(working)
            .ok_or(Error::TooLarge)?;
        Ok(Budget::new(remaining))
    }

    /// Encodes the raster `source` holds (rows of `stride` bytes from its
    /// start) and returns the file.
    pub(super) fn encode_bounded<R: Read + Seek>(
        &self,
        source: &mut R,
        stride: usize,
        info: ImageInfo,
        meta: &Metadata,
        budget: Budget,
    ) -> Result<Vec<u8>, Error> {
        let scratch = self.scratch().ok_or(Error::InvalidInput("not bounded"))?;
        let lossy = &self.config.lossy_config();
        let index = Store::open(scratch, budget.clone())?;
        let checksum = if lossy.checksum {
            Some(Store::open(scratch, budget.clone())?)
        } else {
            None
        };
        let index_limit = if matches!(scratch, Scratch::Memory) {
            budget.cap
        } else {
            u64::MAX
        };
        let cfg = BoundedLossyConfig::new(stride as u64, index_limit);
        let output = Output {
            memory: Memory::new(budget),
            job: self,
        };
        let pulse = self
            .progress
            .as_ref()
            .map_or(&how_far::NoPulse as &dyn how_far::Pulse, |p| p.as_pulse());
        let pulse = how_far::WithStop::borrowed(pulse, &self.stop);
        let (out, _, _) = crate::v4::encode_lossy_bounded(
            source,
            output,
            index,
            checksum,
            info,
            meta,
            &self.config,
            cfg,
            &pulse,
        )
        .map_err(codec_error)?;
        let data = out.memory.into_vec();
        self.limits
            .check_output_size(data.len() as u64)
            .map_err(|_| Error::TooLarge)?;
        Ok(data)
    }

    /// `Encoder::encode` for a bounded job: the caller's pixels in place.
    pub(super) fn encode_bounded_slice(&self, pixels: &PixelSlice<'_>) -> Result<Vec<u8>, Error> {
        let (info, meta) = encode_description(
            pixels,
            &self.metadata,
            &self.policy,
            self.color_origin.as_ref(),
        )?;
        let mut source = Cursor::new(pixels.as_strided_bytes());
        let budget = self.bounded_budget(&info)?;
        self.encode_bounded(&mut source, pixels.stride(), info, &meta, budget)
    }
}

/// Pushed or pulled rows spooled until `finish`.
pub(super) struct RowSpool {
    store: Store,
    budget: Budget,
    descriptor: PixelDescriptor,
    info: ImageInfo,
    metadata: Metadata,
    rows: u32,
}

impl RowSpool {
    pub(super) fn new(job: &EncodeRequest, pixels: &PixelSlice<'_>) -> Result<Self, Error> {
        let scratch = job.scratch().ok_or(Error::InvalidInput("not bounded"))?;
        let (width, height) = job.canvas.ok_or(Error::InvalidInput(
            "row encoding requires canvas dimensions",
        ))?;
        if pixels.width() != width || pixels.rows() > height {
            return Err(Error::InvalidInput("row canvas mismatch"));
        }
        let (mut info, metadata) = encode_description(
            pixels,
            &job.metadata,
            &job.policy,
            job.color_origin.as_ref(),
        )?;
        info.height = height;
        Self::start(job, scratch, pixels.descriptor(), info, metadata)
    }

    pub(super) fn declared(
        job: &EncodeRequest,
        descriptor: PixelDescriptor,
    ) -> Result<Self, Error> {
        let canvas = job.canvas.ok_or(Error::InvalidInput("pull canvas"))?;
        job.limits
            .check_dimensions(canvas.0, canvas.1)
            .map_err(|_| Error::TooLarge)?;
        job.limits
            .check_total_pixels(u64::from(canvas.0) * u64::from(canvas.1))
            .map_err(|_| Error::TooLarge)?;
        let (info, metadata) = encode_description_for(
            descriptor,
            canvas,
            None,
            &job.metadata,
            &job.policy,
            job.color_origin.as_ref(),
        )?;
        let scratch = job.scratch().ok_or(Error::InvalidInput("not bounded"))?;
        Self::start(job, scratch, descriptor, info, metadata)
    }

    fn start(
        job: &EncodeRequest,
        scratch: &Scratch,
        descriptor: PixelDescriptor,
        info: ImageInfo,
        metadata: Metadata,
    ) -> Result<Self, Error> {
        let budget = job.bounded_budget(&info)?;
        Ok(Self {
            store: Store::open(scratch, budget.clone())?,
            budget,
            descriptor,
            info,
            metadata,
            rows: 0,
        })
    }

    pub(super) fn pull_buffer(&self, len: usize) -> Result<Memory, Error> {
        let mut memory = Memory::new(self.budget.clone());
        memory.reserve_end(len).map_err(|_| Error::TooLarge)?;
        memory.cursor.get_mut().resize(len, 0);
        Ok(memory)
    }

    pub(super) fn rows(&self) -> u32 {
        self.rows
    }

    pub(super) fn push(
        &mut self,
        job: &EncodeRequest,
        pixels: &PixelSlice<'_>,
    ) -> Result<(), Error> {
        if pixels.descriptor() != self.descriptor || pixels.width() != self.info.width {
            return Err(Error::InvalidInput("input descriptor mismatch"));
        }
        let rows = self
            .rows
            .checked_add(pixels.rows())
            .filter(|&r| r <= self.info.height)
            .ok_or(Error::InvalidInput("rows past the canvas height"))?;
        let rb = self.info.row_bytes();
        // Reserve the entire batch before writing any row.
        if let Store::Memory(memory) = &mut self.store {
            let end = (rows as usize).checked_mul(rb).ok_or(Error::TooLarge)?;
            memory
                .reserve_end(end)
                .map_err(|e| codec_error(StreamEncodeError::Write(e)))?;
        }
        for y in 0..pixels.rows() {
            job.check_stop()?;
            self.store
                .write_all(&pixels.row(y)[..rb])
                .map_err(|e| codec_error(StreamEncodeError::Write(e)))?;
        }
        self.rows = rows;
        Ok(())
    }

    pub(super) fn finish(mut self, job: &EncodeRequest) -> Result<Vec<u8>, Error> {
        if self.rows != self.info.height {
            return Err(Error::InvalidInput("fewer rows than the canvas height"));
        }
        self.store
            .seek(SeekFrom::Start(0))
            .map_err(|e| codec_error(StreamEncodeError::Read(e)))?;
        let rb = self.info.row_bytes();
        job.encode_bounded(
            &mut self.store,
            rb,
            self.info.clone(),
            &self.metadata,
            self.budget.clone(),
        )
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    #[test]
    fn shared_budget_refuses_growth_before_changing_buffers() {
        let budget = Budget::new(100);
        let mut source = Memory::new(budget.clone());
        let mut index = Memory::new(budget.clone());
        source.write_all(&[1; 60]).unwrap();
        index.write_all(&[2; 30]).unwrap();
        assert_eq!(budget.used.load(Ordering::Relaxed), 90);
        // Existing source and index storage leaves no room for this write.
        assert!(index.write_all(&[3; 20]).is_err());
        assert_eq!(index.as_bytes(), &[2; 30]);
        assert_eq!(index.cursor.position(), 30);
        assert_eq!(budget.used.load(Ordering::Relaxed), 90);
        // Replacement must coexist with the old allocation during copying.
        assert!(source.write_all(&[4]).is_err());
        assert_eq!(source.as_bytes(), &[1; 60]);
        drop(source);
        assert_eq!(budget.used.load(Ordering::Relaxed), 30);
        index.write_all(&[3; 20]).unwrap();
        let data = index.into_vec();
        assert_eq!(data.len(), 50);
        assert_eq!(budget.used.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn oversized_seek_write_never_allocates() {
        let budget = Budget::new(100);
        let mut memory = Memory::new(budget.clone());
        memory.seek(SeekFrom::Start(u64::MAX)).unwrap();
        assert!(memory.write_all(&[1]).is_err());
        assert!(memory.as_bytes().is_empty());
        assert_eq!(budget.used.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn output_limit_is_checked_before_allocation() {
        use zencodec::encode::{EncodeJob, EncoderConfig};
        let job = crate::ImiEncoderConfig::lossy(70.0)
            .job()
            .with_scratch(Scratch::Memory)
            .with_limits(zencodec::ResourceLimits::none().with_max_output(8));
        let budget = Budget::new(100);
        let mut output = Output {
            memory: Memory::new(budget.clone()),
            job: &job,
        };
        assert!(output.write_all(&[0; 9]).is_err());
        assert_eq!(budget.used.load(Ordering::Relaxed), 0);
        output.write_all(&[1; 8]).unwrap();
        output.seek(SeekFrom::Start(0)).unwrap();
        output.write_all(&[2; 4]).unwrap();
        assert_eq!(output.memory.as_bytes(), &[2, 2, 2, 2, 1, 1, 1, 1]);
    }
}
