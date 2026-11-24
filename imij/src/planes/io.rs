//! Portable positional I/O; implementations need not read other file regions.
use crate::Error;

pub trait ReadAt {
    fn len(&mut self) -> Result<u64, Error>;
    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error>;
    fn is_empty(&mut self) -> Result<bool, Error> {
        Ok(self.len()? == 0)
    }
    /// Reads several independent ranges, one request per range. Readers
    /// call it with every range they can ask for at once, so a source that
    /// serves requests concurrently (HTTP range requests, object stores)
    /// can overlap their round trips. The default reads them in order.
    fn read_many_at(&mut self, reqs: &mut [(u64, &mut [u8])]) -> Result<(), Error> {
        for (offset, out) in reqs.iter_mut() {
            self.read_exact_at(*offset, out)?;
        }
        Ok(())
    }
}

/// Use a zencodec positional source with an IMI reader. Known lengths and
/// whole independent range batches pass through the adapter. The error
/// mapper receives the original typed source error, including during open.
pub struct ZencodecReadAt<S, F> {
    source: S,
    map_error: F,
}

impl<S, F> ZencodecReadAt<S, F> {
    pub fn new(source: S, map_error: F) -> Self {
        Self { source, map_error }
    }
}

impl<S, F> ReadAt for ZencodecReadAt<S, F>
where
    S: zencodec::source::ReadAt,
    F: FnMut(S::Error) -> Error,
{
    fn len(&mut self) -> Result<u64, Error> {
        if let Some(length) = self.source.len_hint() {
            return Ok(length);
        }
        self.source.len().map_err(&mut self.map_error)
    }

    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
        self.source
            .read_exact_at(offset, out)
            .map_err(&mut self.map_error)
    }

    fn read_many_at(&mut self, requests: &mut [(u64, &mut [u8])]) -> Result<(), Error> {
        self.source
            .read_many_at(requests)
            .map_err(&mut self.map_error)
    }
}

pub struct SliceReader<'a> {
    bytes: &'a [u8],
}
impl<'a> SliceReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
}
impl ReadAt for SliceReader<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.bytes.len() as u64)
    }
    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
        let start = usize::try_from(offset).map_err(|_| Error::TooLarge)?;
        let end = start.checked_add(out.len()).ok_or(Error::TooLarge)?;
        out.copy_from_slice(
            self.bytes
                .get(start..end)
                .ok_or(Error::Corrupt("plane read past EOF"))?,
        );
        Ok(())
    }
}

/// Adapter for caller-owned std streams. Positional reads move their cursor.
#[cfg(feature = "std")]
pub struct SeekReader<R> {
    inner: R,
}
#[cfg(feature = "std")]
impl<R> SeekReader<R> {
    pub fn new(inner: R) -> Self {
        Self { inner }
    }
    pub fn into_inner(self) -> R {
        self.inner
    }
}
#[cfg(feature = "std")]
impl<R: std::io::Read + std::io::Seek> ReadAt for SeekReader<R> {
    fn len(&mut self) -> Result<u64, Error> {
        self.inner
            .seek(std::io::SeekFrom::End(0))
            .map_err(|_| Error::Read)
    }
    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
        self.inner
            .seek(std::io::SeekFrom::Start(offset))
            .map_err(|_| Error::Read)?;
        self.inner.read_exact(out).map_err(|_| Error::Read)
    }
}
