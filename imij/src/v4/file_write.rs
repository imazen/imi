//! Forward-only numeric files with explicit caller-owned directory scratch.

use alloc::vec::Vec;
use std::io::{Read, Seek, Write};

use super::{
    DirectoryPlacement, DirectoryWriter, LocatedEntry, NumericGroupEncoder, SpatialConfig,
    TileExtent,
};
use crate::stream_encode::{GroupRect, StreamEncodeError, spool::Spool};
use crate::{Error, ImageInfo, ImiEncoderConfig, Metadata};

/// Known-size numeric V4 file writer. The supplier follows next_group() in
/// tile-major order. Only one compressed group and model state live in the
/// codec; the complete directory is spooled to caller-owned seekable storage.
/// A raster-order source owns any tile-band reorder scratch separately.
///
/// Each successful object call flushes the destination so groups can be consumed
/// before later input, tile close or final index. Such pixels remain provisional
/// until their tile/file validation scopes finish. An I/O failure poisons this
/// writer; its partial destination cannot be finalized successfully.
pub struct NumericFileEncoder<W, S> {
    encoder: NumericGroupEncoder,
    directory: DirectoryWriter,
    index: Spool<S>,
    writer: W,
    written: u64,
    max_output: u64,
    failed: bool,
}

fn write_bytes<W: Write>(
    writer: &mut W,
    written: &mut u64,
    limit: u64,
    bytes: &[u8],
) -> Result<(), StreamEncodeError> {
    let next = written
        .checked_add(bytes.len() as u64)
        .ok_or(Error::TooLarge)?;
    if next > limit {
        return Err(Error::TooLarge.into());
    }
    writer.write_all(bytes)?;
    *written = next;
    Ok(())
}

impl<W: Write, S: Read + Write + Seek> NumericFileEncoder<W, S> {
    /// Scratch is appended, not truncated. Its retained prefix belongs to the
    /// caller. The scratch limit covers appended entry bytes; the output limit
    /// covers the complete file, including its final directory and locator.
    pub fn new(
        info: ImageInfo,
        metadata: &Metadata,
        config: &ImiEncoderConfig,
        spatial: SpatialConfig,
        writer: W,
        scratch: S,
        max_scratch_bytes: u64,
        max_output_bytes: u64,
    ) -> Result<Self, StreamEncodeError> {
        Self::new_checked(
            info,
            metadata,
            config,
            spatial,
            writer,
            scratch,
            max_scratch_bytes,
            max_output_bytes,
            super::Checks::default(),
        )
    }

    /// Independent built-in pixel/object checks. Tile hashing admits one tile
    /// raster against the configured working budget; groups still flush promptly.
    pub fn new_checked(
        info: ImageInfo,
        metadata: &Metadata,
        config: &ImiEncoderConfig,
        spatial: SpatialConfig,
        writer: W,
        scratch: S,
        max_scratch_bytes: u64,
        max_output_bytes: u64,
        checks: super::Checks<'_>,
    ) -> Result<Self, StreamEncodeError> {
        if config.checksum() {
            return Err(Error::InvalidInput(
                "checksum output requires ChecksummedNumericFileEncoder",
            )
            .into());
        }
        let (mut result, header, setup) = Self::unwritten(
            info,
            metadata,
            config,
            spatial,
            writer,
            scratch,
            max_scratch_bytes,
            max_output_bytes,
            None,
            checks,
        )?;
        result.start(&header, &setup)?;
        Ok(result)
    }

    fn unwritten(
        info: ImageInfo,
        metadata: &Metadata,
        config: &ImiEncoderConfig,
        spatial: SpatialConfig,
        writer: W,
        scratch: S,
        max_scratch_bytes: u64,
        max_output_bytes: u64,
        checksum: Option<(u8, crate::HashAlgorithm)>,
        checks: super::Checks<'_>,
    ) -> Result<(Self, Vec<u8>, Vec<u8>), StreamEncodeError> {
        let (encoder, setup) = NumericGroupEncoder::with_placement_checked(
            info.clone(),
            config,
            spatial,
            DirectoryPlacement::Tail,
            checks,
            true,
        )?;
        let index = Spool::new(scratch, max_scratch_bytes, "V4 directory scratch changed")?;
        let mut header = Vec::new();
        crate::container::write_group_head_features(
            &mut header,
            &info,
            metadata,
            crate::Coding::Lossless,
            None,
            config.decode_speed().unwrap_or_default(),
            encoder.features(),
            4,
        )?;
        if let Some((piece_log2, algorithm)) = checksum {
            header = crate::container::with_extension(
                header,
                crate::checksum::EXT_CHECKSUM,
                Some(&[piece_log2, algorithm.wire()]),
            )?;
        }
        let initial = (header.len() as u64)
            .checked_add(5)
            .and_then(|n| n.checked_add(setup.len() as u64))
            .ok_or(Error::TooLarge)?;
        if initial > max_output_bytes {
            return Err(Error::TooLarge.into());
        }
        let directory = DirectoryWriter::new(encoder.layout(), spatial.integrity);
        let result = Self {
            encoder,
            directory,
            index,
            writer,
            written: 0,
            max_output: max_output_bytes,
            failed: false,
        };
        Ok((result, header, setup))
    }

    fn start(&mut self, header: &[u8], setup: &[u8]) -> Result<(), StreamEncodeError> {
        self.write(header)?;
        self.write(b"DATA\0")?;
        self.write(setup)?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn layout(&self) -> super::TileLayout {
        self.encoder.layout()
    }
    pub const fn bytes_written(&self) -> u64 {
        self.written
    }
    pub fn next_group(&self) -> Option<GroupRect> {
        if self.failed {
            None
        } else {
            self.encoder.next_group()
        }
    }

    fn ready(&self) -> Result<(), StreamEncodeError> {
        if self.failed {
            Err(Error::InvalidInput("failed V4 file writer").into())
        } else {
            Ok(())
        }
    }
    fn write(&mut self, bytes: &[u8]) -> Result<(), StreamEncodeError> {
        write_bytes(&mut self.writer, &mut self.written, self.max_output, bytes)
    }
    fn publish(&mut self, entry: LocatedEntry) -> Result<(), StreamEncodeError> {
        let mut buffer = [0; 40];
        let count = self.directory.push(entry.entry, &mut buffer)?;
        self.index.append(&buffer[..count])?;
        write_bytes(
            &mut self.writer,
            &mut self.written,
            self.max_output,
            self.encoder.output(),
        )?;
        self.writer.flush()?;
        self.encoder.consume_output(self.encoder.output().len())?;
        Ok(())
    }

    pub fn begin_tile(&mut self) -> Result<LocatedEntry, StreamEncodeError> {
        self.ready()?;
        let result = (|| {
            let entry = self.encoder.begin_tile()?;
            self.publish(entry)?;
            Ok(entry)
        })();
        self.failed |= result.is_err();
        result
    }

    /// Consume the requested local strided rectangle and publish it immediately.
    pub fn push_group(
        &mut self,
        pixels: &[u8],
        stride: usize,
    ) -> Result<LocatedEntry, StreamEncodeError> {
        self.ready()?;
        let result = (|| {
            let entry = self.encoder.push_group(pixels, stride)?;
            self.publish(entry)?;
            Ok(entry)
        })();
        self.failed |= result.is_err();
        result
    }

    pub fn end_tile(&mut self) -> Result<TileExtent, StreamEncodeError> {
        self.ready()?;
        let result = (|| {
            let (entry, tile) = self.encoder.end_tile()?;
            self.publish(entry)?;
            Ok(tile)
        })();
        self.failed |= result.is_err();
        result
    }

    /// Emit the final canonical directory and locator. No per-image vector or
    /// whole-raster replay is introduced. Scratch mutation/read failures remain
    /// errors even after earlier groups have been published.
    pub fn finish(mut self) -> Result<(W, S), StreamEncodeError> {
        self.ready()?;
        let (writer, written, limit) = (&mut self.writer, &mut self.written, self.max_output);
        replay_index(
            &self.encoder,
            &self.directory,
            &mut self.index,
            limit.checked_sub(*written).ok_or(Error::TooLarge)?,
            |bytes| write_bytes(writer, written, limit, bytes),
        )?;
        self.writer.flush()?;
        Ok((self.writer, self.index.into_inner()))
    }
}

// The checksum hashes one verified replay, then final output uses the same
// encoder/directory authority and framing on its second verified replay.
fn replay_index<S: Read + Write + Seek>(
    encoder: &NumericGroupEncoder,
    directory: &DirectoryWriter,
    index: &mut Spool<S>,
    remaining: u64,
    mut consume: impl FnMut(&[u8]) -> Result<(), StreamEncodeError>,
) -> Result<(), StreamEncodeError> {
    encoder.finish()?;
    let (count, objects_len) = directory.finish()?;
    let payload_len = (crate::varint_len(count) as u64)
        .checked_add(index.len())
        .and_then(|n| n.checked_add(20))
        .ok_or(Error::TooLarge)?;
    let length_field = payload_len.checked_add(1).ok_or(Error::TooLarge)?;
    let chunk_len = (4 + crate::varint_len(length_field) as u64)
        .checked_add(payload_len)
        .ok_or(Error::TooLarge)?;
    if chunk_len > remaining {
        return Err(Error::TooLarge.into());
    }
    let mut head = Vec::new();
    head.try_reserve_exact(24).map_err(|_| Error::TooLarge)?;
    head.extend_from_slice(b"uIDX");
    crate::put_varint(&mut head, length_field);
    crate::put_varint(&mut head, count);
    consume(&head)?;
    index.replay(&mut consume)?;
    consume(&objects_len.to_le_bytes())?;
    consume(&chunk_len.to_le_bytes())?;
    consume(b"uIDX")
}

#[path = "file_write_checked.rs"]
mod checked;
pub use checked::ChecksummedNumericFileEncoder;
