//! Bounded positional cursor for traversal and selective metadata.

use alloc::vec::Vec;

use crate::Error;
use crate::parse::payload::HeadInput;
use crate::planes::ReadAt;

pub(crate) struct Cursor<'a> {
    pub source: &'a mut dyn ReadAt,
    pub pos: u64,
    end: u64,
    head_end: u64,
    buffer: Vec<u8>,
    buffer_at: u64,
    prefix: &'a [u8],
    read_ahead: usize,
}
impl<'a> Cursor<'a> {
    pub fn new(source: &'a mut dyn ReadAt, end: u64) -> Self {
        Self {
            source,
            pos: 0,
            end,
            head_end: end,
            buffer: Vec::new(),
            buffer_at: 0,
            prefix: &[],
            read_ahead: 8192,
        }
    }
    /// Selective metadata inspection must avoid speculative reads into bodies.
    pub fn exact(source: &'a mut dyn ReadAt, end: u64, prefix: &'a [u8]) -> Self {
        Self {
            prefix,
            read_ahead: 0,
            ..Self::new(source, end)
        }
    }
    pub fn limit_head(&mut self, bytes: u64) {
        self.head_end = self.pos.saturating_add(bytes).min(self.end);
    }
    pub fn clear_head_limit(&mut self) {
        self.head_end = self.end;
    }
    pub fn skip_to(&mut self, end: u64) -> Result<(), Error> {
        if end < self.pos || end > self.end {
            return Err(Error::Corrupt("record past image range"));
        }
        self.pos = end;
        Ok(())
    }
    pub fn vector(&mut self, n: u64, limit: usize) -> Result<Vec<u8>, Error> {
        let n = usize::try_from(n).map_err(|_| Error::TooLarge)?;
        if n > limit {
            return Err(Error::TooLarge);
        }
        let mut out = Vec::new();
        out.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
        out.extend_from_slice(self.bytes(n)?);
        Ok(out)
    }
}
impl HeadInput for Cursor<'_> {
    fn bytes(&mut self, n: usize) -> Result<&[u8], Error> {
        let end = self.pos.checked_add(n as u64).ok_or(Error::TooLarge)?;
        if end > self.end {
            return Err(Error::Corrupt("truncated image"));
        }
        if end > self.head_end {
            return Err(Error::TooLarge);
        }
        if end <= self.prefix.len() as u64 {
            let start = self.pos as usize;
            self.pos = end;
            return Ok(&self.prefix[start..end as usize]);
        }
        if self.pos < self.buffer_at || end > self.buffer_at + self.buffer.len() as u64 {
            let count = (self.end - self.pos).min(n.max(self.read_ahead) as u64) as usize;
            self.buffer.clear();
            self.buffer
                .try_reserve_exact(count)
                .map_err(|_| Error::TooLarge)?;
            self.buffer.resize(count, 0);
            self.source.read_exact_at(self.pos, &mut self.buffer)?;
            self.buffer_at = self.pos;
        }
        let start = (self.pos - self.buffer_at) as usize;
        self.pos = end;
        Ok(&self.buffer[start..start + n])
    }
    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.bytes(1)?[0])
    }
    fn peek_u8(&mut self) -> Result<u8, Error> {
        let b = self.u8()?;
        self.pos -= 1;
        Ok(b)
    }
    fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    fn varint(&mut self) -> Result<u64, Error> {
        crate::read_varint(|| self.u8())
    }
    fn pos(&self) -> u64 {
        self.pos
    }
    fn mark(&self) -> crate::trace::Mark {
        crate::trace::Mark::Abs(self.pos)
    }
}
