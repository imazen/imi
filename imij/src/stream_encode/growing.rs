//! Encode groups before the final image height is known.

use super::spool::{REPLAY_BYTES, Spool};
use super::{
    GroupEncodeConfig, GroupRect, NativeGroupEncodeSession as GroupEncodeSession, StreamEncodeError,
};
use crate::{Error, ImageInfo, Metadata};
use std::io::{Read, Seek, Write};

/// Bounded-memory lossless group encoder for a height learned at finish.
///
/// `info.height` must be zero at construction. The width and every other image
/// declaration are final. Groups are encoded as input arrives, in the same
/// stripe-major order as [`super::IndexedGroupEncoder`]. The encoded records
/// and index spans go to separate caller-owned `Read + Write + Seek` scratch
/// objects. `finish` writes a valid ordinary indexed IMI file to `writer`:
/// final header, replayed records, then trailing `uIDX`. No output is written
/// to `writer` before finish, and an incomplete writer is not a valid image.
///
/// Call [`Self::set_final_height`] at a stripe boundary before supplying a
/// final partial stripe. Complete stripes may be written before the height is
/// known. The two scratch limits are explicit byte caps, not preallocation;
/// neither scratch object is truncated or deleted. This mode trades an extra
/// compressed-data write/read for bounded codec memory and an unchanged wire.
/// A source that can provide only full-width raster rows may still need its
/// own reorder buffer or disk spool to supply column groups in stripe order.
pub struct GrowingHeightGroupEncoder<W, D, I> {
    writer: W,
    data: Spool<D>,
    index: Spool<I>,
    session: GroupEncodeSession,
    info: ImageInfo,
    meta: Metadata,
    cfg: GroupEncodeConfig,
    final_height: Option<u32>,
    failed: bool,
}

impl<W: Write, D: Read + Write + Seek, I: Read + Write + Seek> GrowingHeightGroupEncoder<W, D, I> {
    /// Validate the fixed image declaration and scratch limits before input.
    ///
    /// `info.height == 0` denotes the height to be supplied later. Working
    /// storage admits the full configured group height, even if the final image
    /// later ends in a shorter stripe. The byte caps exclude existing scratch bytes and caller-owned sink buffering.
    pub fn new(
        writer: W,
        data_scratch: D,
        index_scratch: I,
        info: ImageInfo,
        meta: &Metadata,
        cfg: &crate::ImiEncoderConfig,
        max_data_bytes: u64,
        max_index_bytes: u64,
    ) -> Result<Self, StreamEncodeError> {
        let mut cfg = GroupEncodeConfig::required(cfg)?;
        if info.height != 0 {
            return Err(Error::InvalidInput("growing image height must be zero").into());
        }
        if max_data_bytes == 0 || max_index_bytes < 10 {
            return Err(Error::TooLarge.into());
        }
        cfg.max_working_bytes = cfg
            .max_working_bytes
            .checked_sub(REPLAY_BYTES)
            .ok_or(Error::TooLarge)?;
        // Height is unknown, but a complete configured group may arrive first.
        // Check that geometry before constructing the smaller provisional header
        // or touching either caller-owned scratch stream.
        let mut group_info = info.clone();
        group_info.height = cfg.rows;
        GroupEncodeSession::check_working(&group_info, meta, cfg)?;
        // Build the fixed model/header with a one-row placeholder. A maximal
        // placeholder would make an otherwise representable final image fail
        // the existing header's checked_buffer_len admission before any input.
        let mut provisional = info.clone();
        provisional.height = 1;
        let (mut session, _) = GroupEncodeSession::new_inner(provisional, meta, cfg, true)?;
        let header_len = session.output().len();
        session.consume_output(header_len)?;
        session.info.height = cfg.rows;
        Ok(Self {
            writer,
            data: Spool::new(data_scratch, max_data_bytes, "group data scratch changed")?,
            index: Spool::new(
                index_scratch,
                max_index_bytes,
                "group index scratch changed",
            )?,
            session,
            info,
            meta: meta.clone(),
            cfg,
            final_height: None,
            failed: false,
        })
    }

    /// Next required rectangle. Before the final height is known, only full
    /// stripes are exposed. After [`Self::set_final_height`], the final stripe
    /// is clipped to the supplied height.
    pub fn next_group(&self) -> Option<GroupRect> {
        if self.failed {
            return None;
        }
        let rect = self.session.next_group()?;
        if self.final_height.is_none() && rect.height != self.cfg.rows {
            None
        } else {
            Some(rect)
        }
    }

    /// Record the actual height at a stripe boundary. Invalid heights leave
    /// the encoder unchanged, so the caller may retry. A positive height equal
    /// to the number of complete rows already written finishes without a
    /// partial stripe.
    pub fn set_final_height(&mut self, height: u32) -> Result<(), StreamEncodeError> {
        if self.failed || self.final_height.is_some() {
            return Err(Error::InvalidInput("growing height already set or writer failed").into());
        }
        if height == 0 || self.session.x != 0 || height < self.session.y {
            return Err(Error::InvalidInput("final height needs a stripe boundary").into());
        }
        self.session.info.height = height;
        self.final_height = Some(height);
        Ok(())
    }

    /// Encode the next strided rectangle to caller-owned compressed scratch.
    /// Invalid pixel extents can be corrected and retried; a scratch I/O or
    /// capacity error makes the writer permanently failed.
    pub fn write_group(&mut self, pixels: &[u8], stride: usize) -> Result<(), StreamEncodeError> {
        if self.next_group().is_none() {
            return Err(Error::InvalidInput("growing encoder has no next group").into());
        }
        self.session.push_group(pixels, stride)?;
        self.failed = true;
        let record = self.session.output();
        let record_size = record.len();
        let record_len = record_size as u64;
        self.data.append(record)?;
        let mut span = [0u8; 10];
        let mut remaining = record_len;
        let mut used = 0;
        loop {
            span[used] = (remaining & 127) as u8;
            remaining >>= 7;
            if remaining != 0 {
                span[used] |= 128;
            }
            used += 1;
            if remaining == 0 {
                break;
            }
        }
        self.index.append(&span[..used])?;
        self.session.consume_output(record_size)?;
        if self.final_height.is_none() && self.session.y == self.session.info.height {
            self.session.info.height = self.session.info.height.saturating_add(self.cfg.rows);
        }
        self.failed = false;
        Ok(())
    }

    /// Finalize the ordinary IMI header, DATA and trailing index. Any scratch
    /// mutation or output failure makes the partial destination unusable.
    pub fn finish(mut self) -> Result<(W, D, I), StreamEncodeError> {
        let Some(height) = self.final_height else {
            return Err(Error::InvalidInput("final height not set").into());
        };
        if self.failed || !self.session.is_finished() {
            return Err(Error::InvalidInput("incomplete growing image").into());
        }
        drop(self.session);
        self.info.height = height;
        let (head, head_data_len) =
            GroupEncodeSession::new_inner(self.info, &self.meta, self.cfg, true)?;
        let data_len = head_data_len
            .checked_add(self.data.len())
            .ok_or(Error::TooLarge)?;
        let payload_len = self.index.len().checked_add(20).ok_or(Error::TooLarge)?;
        let encoded_len = payload_len.checked_add(1).ok_or(Error::TooLarge)?;
        let index_len = payload_len
            .checked_add(4 + crate::varint_len(encoded_len) as u64)
            .ok_or(Error::TooLarge)?;
        let mut index_head = Vec::with_capacity(14);
        index_head.extend_from_slice(&crate::parse::records::UIDX);
        crate::put_varint(&mut index_head, encoded_len);
        let mut footer = [0u8; 20];
        footer[..8].copy_from_slice(&data_len.to_le_bytes());
        footer[8..16].copy_from_slice(&index_len.to_le_bytes());
        footer[16..].copy_from_slice(&crate::parse::records::UIDX);

        self.writer.write_all(head.output())?;
        self.data.replay(|bytes| {
            self.writer.write_all(bytes)?;
            Ok(())
        })?;
        self.writer.write_all(&index_head)?;
        self.index.replay(|bytes| {
            self.writer.write_all(bytes)?;
            Ok(())
        })?;
        self.writer.write_all(&footer)?;
        self.writer.flush()?;
        Ok((self.writer, self.data.into_inner(), self.index.into_inner()))
    }
}
