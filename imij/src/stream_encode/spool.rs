//! An append-only scratch range with fixed-buffer checked replay.
use super::StreamEncodeError;
use crate::Error;
use std::io::{Read, Seek, SeekFrom, Write};

pub(crate) const REPLAY_BYTES: usize = 8192;

pub(crate) struct Spool<S> {
    inner: S,
    start: u64,
    len: u64,
    limit: u64,
    hash: zenhash::Xxh3,
    changed: &'static str,
}
impl<S: Read + Write + Seek> Spool<S> {
    pub(crate) fn new(
        mut inner: S,
        limit: u64,
        changed: &'static str,
    ) -> Result<Self, StreamEncodeError> {
        let start = inner.seek(SeekFrom::End(0))?;
        start.checked_add(limit).ok_or(Error::TooLarge)?;
        Ok(Self {
            inner,
            start,
            len: 0,
            limit,
            hash: zenhash::Xxh3::new(),
            changed,
        })
    }
    pub(crate) fn len(&self) -> u64 {
        self.len
    }
    pub(crate) fn append(&mut self, bytes: &[u8]) -> Result<(), StreamEncodeError> {
        let len = self
            .len
            .checked_add(bytes.len() as u64)
            .ok_or(Error::TooLarge)?;
        if len > self.limit {
            return Err(Error::TooLarge.into());
        }
        self.inner.write_all(bytes)?;
        self.hash.update(bytes);
        self.len = len;
        Ok(())
    }
    pub(crate) fn replay(
        &mut self,
        mut consume: impl FnMut(&[u8]) -> Result<(), StreamEncodeError>,
    ) -> Result<(), StreamEncodeError> {
        self.inner.flush()?;
        self.inner.seek(SeekFrom::Start(self.start))?;
        let mut buffer = [0u8; REPLAY_BYTES];
        let mut remaining = self.len;
        let mut hash = zenhash::Xxh3::new();
        while remaining != 0 {
            let count = remaining.min(REPLAY_BYTES as u64) as usize;
            self.inner
                .read_exact(&mut buffer[..count])
                .map_err(StreamEncodeError::Read)?;
            hash.update(&buffer[..count]);
            consume(&buffer[..count])?;
            remaining -= count as u64;
        }
        if hash.digest() != self.hash.digest() {
            return Err(Error::InvalidInput(self.changed).into());
        }
        Ok(())
    }
    pub(crate) fn into_inner(self) -> S {
        self.inner
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use std::io::{self, Cursor};

    struct Unreadable(Cursor<Vec<u8>>);
    impl Read for Unreadable {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "scratch read",
            ))
        }
    }
    impl Write for Unreadable {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.write(bytes)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }
    impl Seek for Unreadable {
        fn seek(&mut self, at: SeekFrom) -> io::Result<u64> {
            self.0.seek(at)
        }
    }

    #[test]
    fn scratch_read_error_is_not_reported_as_a_destination_write_failure() {
        let mut spool = Spool::new(Unreadable(Cursor::new(Vec::new())), 100, "changed").unwrap();
        spool.append(b"payload").unwrap();
        let error = spool.replay(|_| panic!("no bytes were read")).unwrap_err();
        match error {
            StreamEncodeError::Read(error) => {
                assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
                assert_eq!(error.to_string(), "scratch read");
            }
            other => panic!("wrong operation: {other:?}"),
        }
    }
}
