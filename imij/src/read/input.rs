//! Suspendable transport operations used by the shared reader framing.
//! Zero bytes means explicit EOF; temporary exhaustion returns Pending.

use alloc::vec::Vec;
use std::io::{self, Read};

pub(crate) trait Input {
    async fn read(&mut self, out: &mut [u8]) -> io::Result<usize>;

    /// Reserve transport-owned storage before consuming more input. Feed
    /// transports additionally charge capacity growth to their request.
    fn reserve<T>(&mut self, out: &mut Vec<T>, additional: usize) -> Result<(), crate::Error> {
        out.try_reserve(additional)
            .map_err(|_| crate::Error::TooLarge)
    }

    /// Fixed allocations and copies outside the reusable transport buffers.
    fn account(&mut self, _bytes: usize) -> Result<(), crate::Error> {
        Ok(())
    }

    fn image(&mut self, _info: &crate::ImageInfo) -> Result<(), crate::Error> {
        Ok(())
    }

    fn header_bytes(&self, _bytes: u64) -> Result<(), crate::Error> {
        Ok(())
    }

    /// Append at most `limit` delivered bytes. Grow in bounded steps instead
    /// of reserving a declared length. Reserve before pulling bytes so an
    /// allocation failure cannot hide input already consumed from the source.
    /// Earlier appended bytes remain available for consumed-input accounting.
    async fn append(&mut self, mut limit: u64, out: &mut Vec<u8>) -> io::Result<()> {
        while limit != 0 {
            let spare = out.capacity() - out.len();
            let step = limit.min(if spare == 0 {
                8192
            } else {
                spare.min(65536) as u64
            }) as usize;
            self.reserve(out, step)
                .map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
            let at = out.len();
            out.resize(at + step, 0);
            let result = self.read(&mut out[at..]).await;
            let got = match result {
                Ok(got) => got,
                Err(e) => {
                    out.truncate(at);
                    if e.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(e);
                }
            };
            out.truncate(at + got);
            if got == 0 {
                break;
            }
            limit -= got as u64;
        }
        Ok(())
    }
}

impl<R: Read> Input for R {
    async fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        Read::read(self, out)
    }
    async fn append(&mut self, limit: u64, out: &mut Vec<u8>) -> io::Result<()> {
        Read::take(self, limit).read_to_end(out).map(|_| ())
    }
}

/// A delivered native row remains borrowed until the consumer finishes.
/// Suspension applies backpressure without copying or accumulating a raster.
pub(crate) trait Rows {
    /// An inter-frame consumer supplies the reference rectangle one row at a time.
    /// Lossless predictors retain only their stripe state; lossy keep blocks
    /// prefill one RGB stripe. The consumer owns reference storage and may
    /// implement it with seeks rather than a resident raster.
    fn has_reference(&self) -> bool {
        false
    }
    async fn reference(
        &mut self,
        _info: &crate::ImageInfo,
        _y: u32,
        _pixels: &mut [u8],
    ) -> Result<(), crate::Error> {
        Err(crate::Error::Corrupt("missing reference raster"))
    }
    async fn row(
        &mut self,
        info: &crate::ImageInfo,
        y: u32,
        pixels: &[u8],
    ) -> Result<(), crate::Error>;
}

impl<F: ?Sized + FnMut(&crate::ImageInfo, u32, &[u8]) -> Result<(), crate::Error>> Rows for F {
    async fn row(
        &mut self,
        info: &crate::ImageInfo,
        y: u32,
        pixels: &[u8],
    ) -> Result<(), crate::Error> {
        self(info, y, pixels)
    }
}

/// Hash a delivered row exactly once, before applying consumer backpressure.
pub(super) struct CheckedRows<'a, A, S: ?Sized> {
    pub(super) check: &'a mut Option<crate::pixelsum::RowCheck>,
    pub(super) admission: &'a A,
    pub(super) output: &'a mut S,
}
impl<A: crate::parse::limits::Admission, S: Rows + ?Sized> Rows for CheckedRows<'_, A, S> {
    fn has_reference(&self) -> bool {
        self.output.has_reference()
    }
    async fn reference(
        &mut self,
        info: &crate::ImageInfo,
        y: u32,
        pixels: &mut [u8],
    ) -> Result<(), crate::Error> {
        self.admission.check()?;
        self.output.reference(info, y, pixels).await
    }
    async fn row(
        &mut self,
        info: &crate::ImageInfo,
        y: u32,
        pixels: &[u8],
    ) -> Result<(), crate::Error> {
        if let Some(c) = self.check {
            c.row_checked(pixels, || self.admission.check())?;
        }
        self.output.row(info, y, pixels).await
    }
}
