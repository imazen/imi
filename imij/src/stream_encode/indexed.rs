//! A trailing unit index backed by caller-owned seekable scratch.

use super::spool::{REPLAY_BYTES, Spool};
use super::{
    GroupEncodeConfig, GroupRect, NativeGroupEncodeSession as GroupEncodeSession, StreamEncodeError,
};
use crate::{Error, ImageInfo, Metadata};
use std::io::{Read, Seek, Write};

/// Bounded-group lossless writer with an index for region access.
///
/// The output needs only `Write`. Index entries are appended to caller-owned
/// `Read + Write + Seek` scratch, then replayed with an 8 KiB buffer at finish.
/// Use disk-backed scratch to keep RAM independent of image dimensions. A
/// `Cursor<Vec<u8>>` is supported, but its storage grows with the group count.
/// Existing scratch bytes are preserved. No file is opened, truncated or removed.
///
/// The index uses existing check-tail record 12 and trailing `uIDX` (SPEC 2.7).
/// Pixel coding is identical to [`super::GroupEncoder`]. No checksum is added.
/// Any scratch/output error is sticky; discard partial output after failure.
/// Invalid pixel extents leave progress unchanged and may be retried.
pub struct IndexedGroupEncoder<W, S> {
    pub(super) writer: W,
    scratch: Spool<S>,
    pub(super) session: GroupEncodeSession,
    data_len: u64,
    pub(super) failed: bool,
}

impl<W: Write, S: Read + Write + Seek> IndexedGroupEncoder<W, S> {
    /// Admit geometry and scratch space before writing either destination.
    /// `max_index_bytes` must cover ten bytes per group (the maximum u64
    /// varint width). Actual scratch use is one varint per group. The budget
    /// excludes pre-existing scratch bytes; `max_working_bytes` additionally
    /// reserves the fixed replay buffer. Neither budget includes sink buffering
    /// or allocator overhead. Scratch remains exclusively borrowed/owned through
    /// finish; its contents must not be modified concurrently.
    pub fn new(
        writer: W,
        scratch: S,
        info: ImageInfo,
        meta: &Metadata,
        cfg: &crate::ImiEncoderConfig,
        max_index_bytes: u64,
    ) -> Result<Self, StreamEncodeError> {
        Self::with_groups(
            writer,
            scratch,
            info,
            meta,
            GroupEncodeConfig::required(cfg)?,
            max_index_bytes,
        )
    }

    pub(super) fn with_groups(
        writer: W,
        scratch: S,
        info: ImageInfo,
        meta: &Metadata,
        cfg: GroupEncodeConfig,
        max_index_bytes: u64,
    ) -> Result<Self, StreamEncodeError> {
        let mut encoder = Self::new_unwritten(writer, scratch, info, meta, cfg, max_index_bytes)?;
        encoder.drain()?;
        Ok(encoder)
    }

    pub(super) fn new_unwritten(
        writer: W,
        scratch: S,
        info: ImageInfo,
        meta: &Metadata,
        mut cfg: GroupEncodeConfig,
        max_index_bytes: u64,
    ) -> Result<Self, StreamEncodeError> {
        if cfg.columns == 0 || cfg.rows == 0 || info.width == 0 || info.height == 0 {
            return Err(Error::InvalidInput("group geometry must be nonzero").into());
        }
        let units = u64::from(info.width.div_ceil(cfg.columns))
            .checked_mul(u64::from(info.height.div_ceil(cfg.rows)))
            .ok_or(Error::TooLarge)?;
        let bound = units.checked_mul(10).ok_or(Error::TooLarge)?;
        if bound > max_index_bytes || bound.checked_add(34).is_none() {
            return Err(Error::TooLarge.into());
        }
        cfg.max_working_bytes = cfg
            .max_working_bytes
            .checked_sub(REPLAY_BYTES)
            .ok_or(Error::TooLarge)?;
        let (session, data_len) = GroupEncodeSession::new_inner(info, meta, cfg, true)?;
        let scratch = Spool::new(scratch, bound, "group index scratch changed")?;
        Ok(Self {
            writer,
            scratch,
            session,
            data_len,
            failed: false,
        })
    }

    pub(super) fn drain(&mut self) -> Result<(), StreamEncodeError> {
        self.failed = true;
        self.writer.write_all(self.session.output())?;
        self.session.consume_output(self.session.output().len())?;
        self.failed = false;
        Ok(())
    }

    /// The next required source rectangle; none after completion or failure.
    pub fn next_group(&self) -> Option<GroupRect> {
        if self.failed {
            None
        } else {
            self.session.next_group()
        }
    }

    /// Encode the next strided rectangle and spool its encoded record span.
    pub fn write_group(&mut self, pixels: &[u8], stride: usize) -> Result<(), StreamEncodeError> {
        self.prepare_group(pixels, stride)?;
        self.drain()
    }

    pub(super) fn prepare_group(
        &mut self,
        pixels: &[u8],
        stride: usize,
    ) -> Result<(), StreamEncodeError> {
        if self.failed {
            return Err(Error::InvalidInput("group encoder is finished or failed").into());
        }
        self.session.push_group(pixels, stride)?;
        self.failed = true;
        let size = self.session.output().len() as u64;
        self.append_span(size)?;
        Ok(())
    }

    fn append_span(&mut self, size: u64) -> Result<(), StreamEncodeError> {
        self.data_len = self.data_len.checked_add(size).ok_or(Error::TooLarge)?;
        // A fixed stack buffer, never one retained entry per group.
        let mut encoded = [0u8; 10];
        let mut rest = size;
        let mut count = 0;
        loop {
            encoded[count] = (rest & 127) as u8;
            rest >>= 7;
            if rest != 0 {
                encoded[count] |= 128;
            }
            count += 1;
            if rest == 0 {
                break;
            }
        }
        self.scratch.append(&encoded[..count])?;
        Ok(())
    }

    /// Copy a fully validated old lossless group while updating this file's
    /// group cursor and trailing-index scratch. A read or write failure is
    /// sticky; the partial destination must be discarded.
    pub(super) fn import_record<R: Read>(
        &mut self,
        source: &mut R,
        size: u64,
    ) -> Result<(), StreamEncodeError> {
        let group = self
            .next_group()
            .ok_or(Error::InvalidInput("no group left for imported record"))?;
        self.failed = true;
        let first_len = usize::try_from(size.min(20)).unwrap();
        if first_len < 2 {
            return Err(Error::Corrupt("short imported group record").into());
        }
        let mut first = [0u8; 20];
        source
            .read_exact(&mut first[..first_len])
            .map_err(|_| Error::Read)?;
        let mut record = crate::ByteReader::new(&first[..first_len]);
        let kind = record.varint()?;
        let payload = record.varint()?;
        if kind != crate::parse::records::R_STRIPE
            || (record.pos as u64).checked_add(payload) != Some(size)
        {
            return Err(Error::Corrupt("imported group record head").into());
        }
        self.append_span(size)?;
        self.writer.write_all(&first[..first_len])?;
        let mut remaining = size - first_len as u64;
        let mut block = [0u8; REPLAY_BYTES];
        while remaining != 0 {
            let n = usize::try_from(remaining.min(REPLAY_BYTES as u64)).unwrap();
            source
                .read_exact(&mut block[..n])
                .map_err(|_| Error::Read)?;
            self.writer.write_all(&block[..n])?;
            remaining -= n as u64;
        }
        self.session.advance_group(group);
        self.failed = false;
        Ok(())
    }

    pub(super) fn visit_index(
        &mut self,
        mut consume: impl FnMut(&[u8]) -> Result<(), StreamEncodeError>,
    ) -> Result<(), StreamEncodeError> {
        let (head, footer) = self.index_framing()?;
        consume(&head)?;
        self.scratch.replay(&mut consume)?;
        consume(&footer)
    }

    fn index_framing(&self) -> Result<(Vec<u8>, [u8; 20]), StreamEncodeError> {
        let payload_len = self.scratch.len().checked_add(20).ok_or(Error::TooLarge)?;
        let chunk_len = payload_len
            .checked_add(4 + crate::varint_len(payload_len + 1) as u64)
            .ok_or(Error::TooLarge)?;
        let mut head = Vec::with_capacity(14);
        head.extend_from_slice(&crate::parse::records::UIDX);
        crate::put_varint(&mut head, payload_len + 1);
        let mut footer = [0; 20];
        footer[..8].copy_from_slice(&self.data_len.to_le_bytes());
        footer[8..16].copy_from_slice(&chunk_len.to_le_bytes());
        footer[16..].copy_from_slice(&crate::parse::records::UIDX);
        Ok((head, footer))
    }

    /// Replay the index, write its footer, flush output and return both caller
    /// objects. Scratch is left at the end of the appended entries. A changed or
    /// truncated scratch stream fails rather than completing a different index.
    pub fn finish(mut self) -> Result<(W, S), StreamEncodeError> {
        if self.failed || !self.session.is_finished() {
            return Err(Error::InvalidInput("incomplete group image").into());
        }
        let (head, footer) = self.index_framing()?;
        self.writer.write_all(&head)?;
        self.scratch.replay(|bytes| {
            self.writer.write_all(bytes)?;
            Ok(())
        })?;
        self.writer.write_all(&footer)?;
        self.writer.flush()?;
        Ok((self.writer, self.scratch.into_inner()))
    }
}
