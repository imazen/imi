//! Exact positional caching. Read-ahead must be planned by the bounded reader,
//! rather than guessing a 256 KiB extent in the surrounding archive.
use imij::{Error, planes::ReadAt};
use std::collections::VecDeque;

const CACHE_BYTES: usize = 1 << 20;
const CACHE_RANGES: usize = 128;
pub(super) struct ExactReadCache<S> {
    inner: S,
    len: u64,
    windows: VecDeque<(u64, Vec<u8>)>,
    retained: usize,
}
impl<S: ReadAt> ExactReadCache<S> {
    pub(super) fn new(mut inner: S) -> Result<Self, Error> {
        let len = inner.len()?;
        Ok(Self {
            inner,
            len,
            windows: VecDeque::new(),
            retained: 0,
        })
    }
    fn prefix(&self, at: u64, out: &mut [u8]) -> usize {
        let mut done = 0;
        while done < out.len() {
            let cursor = at + done as u64;
            let Some((start, bytes)) =
                self.windows.iter().rev().find(|(start, bytes)| {
                    cursor >= *start && cursor - *start < bytes.len() as u64
                })
            else {
                break;
            };
            let offset = (cursor - start) as usize;
            let n = (bytes.len() - offset).min(out.len() - done);
            out[done..done + n].copy_from_slice(&bytes[offset..offset + n]);
            done += n;
        }
        done
    }
    fn remember(&mut self, at: u64, bytes: &[u8]) -> Result<(), Error> {
        if bytes.is_empty() || bytes.len() > CACHE_BYTES {
            return Ok(());
        }
        let end = at + bytes.len() as u64;
        self.windows.retain(|(start, old)| {
            if *start >= at && *start + old.len() as u64 <= end {
                self.retained -= old.len();
                false
            } else {
                true
            }
        });
        while self.retained + bytes.len() > CACHE_BYTES || self.windows.len() >= CACHE_RANGES {
            self.retained -= self.windows.pop_front().ok_or(Error::Read)?.1.len();
        }
        let mut copy = Vec::new();
        copy.try_reserve_exact(bytes.len())
            .map_err(|_| Error::TooLarge)?;
        copy.extend_from_slice(bytes);
        self.windows.try_reserve(1).map_err(|_| Error::TooLarge)?;
        self.retained += copy.len();
        self.windows.push_back((at, copy));
        Ok(())
    }
    fn check(&self, at: u64, len: usize) -> Result<(), Error> {
        at.checked_add(len as u64)
            .filter(|end| *end <= self.len)
            .ok_or(Error::Read)
            .map(|_| ())
    }
}
impl<S: ReadAt> ReadAt for ExactReadCache<S> {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.len)
    }
    fn read_exact_at(&mut self, at: u64, out: &mut [u8]) -> Result<(), Error> {
        self.check(at, out.len())?;
        let mut done = 0;
        while done < out.len() {
            done += self.prefix(at + done as u64, &mut out[done..]);
            if done == out.len() {
                break;
            }
            let cursor = at + done as u64;
            let next = self
                .windows
                .iter()
                .filter_map(|(start, _)| (*start > cursor).then_some(*start))
                .min()
                .unwrap_or(at + out.len() as u64)
                .min(at + out.len() as u64);
            let n = (next - cursor) as usize;
            self.inner.read_exact_at(cursor, &mut out[done..done + n])?;
            self.remember(cursor, &out[done..done + n])?;
            done += n;
        }
        Ok(())
    }
    fn read_many_at(&mut self, requests: &mut [(u64, &mut [u8])]) -> Result<(), Error> {
        for (at, out) in requests.iter() {
            self.check(*at, out.len())?
        }
        let mut missing = Vec::new();
        missing
            .try_reserve_exact(requests.len())
            .map_err(|_| Error::TooLarge)?;
        for (at, out) in requests.iter_mut() {
            let prefix = self.prefix(*at, out);
            if prefix < out.len() {
                missing.push((*at + prefix as u64, &mut out[prefix..]));
            }
        }
        self.inner.read_many_at(&mut missing)?;
        drop(missing);
        for (at, out) in requests.iter() {
            self.remember(*at, out)?
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Source {
        reads: Vec<(u64, usize)>,
    }
    impl ReadAt for Source {
        fn len(&mut self) -> Result<u64, Error> {
            Ok((CACHE_BYTES * 4) as u64)
        }
        fn read_exact_at(&mut self, at: u64, out: &mut [u8]) -> Result<(), Error> {
            self.reads.push((at, out.len()));
            for (i, b) in out.iter_mut().enumerate() {
                *b = (at + i as u64) as u8
            }
            Ok(())
        }
    }
    #[test]
    fn exact_reads_fill_only_gaps_and_reuse_multiple_windows() {
        let mut cache = ExactReadCache::new(Source { reads: vec![] }).unwrap();
        cache.read_exact_at(10, &mut [0; 10]).unwrap();
        cache.read_exact_at(30, &mut [0; 10]).unwrap();
        let mut out = [0; 30];
        cache.read_exact_at(10, &mut out).unwrap();
        assert_eq!(out, std::array::from_fn(|i| (10 + i) as u8));
        assert_eq!(cache.inner.reads, [(10, 10), (30, 10), (20, 10)]);
        cache.read_exact_at(10, &mut out).unwrap();
        assert_eq!(cache.inner.reads.len(), 3);
    }
    #[test]
    fn batched_partial_hits_do_not_refetch_cached_prefix() {
        let mut cache = ExactReadCache::new(Source { reads: vec![] }).unwrap();
        cache.read_exact_at(0, &mut [0; 10]).unwrap();
        let mut out = [0; 20];
        cache.read_many_at(&mut [(0, &mut out)]).unwrap();
        assert_eq!(cache.inner.reads, [(0, 10), (10, 10)]);
        assert_eq!(out, std::array::from_fn(|i| i as u8));
    }
    #[test]
    fn retained_cache_is_bounded_and_invalid_batches_do_no_io() {
        let mut cache = ExactReadCache::new(Source { reads: vec![] }).unwrap();
        for at in 0..256 {
            cache.read_exact_at(at * 4096, &mut [0; 4096]).unwrap();
            assert!(cache.retained <= CACHE_BYTES);
            assert!(cache.windows.len() <= CACHE_RANGES);
        }
        let before = cache.inner.reads.len();
        assert!(
            cache
                .read_many_at(&mut [(0, &mut [0; 4]), (u64::MAX, &mut [0; 4])])
                .is_err()
        );
        assert_eq!(before, cache.inner.reads.len());
    }
}
