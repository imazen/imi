//! A bounded adapter from a forward-only, interleaved raster to indexed groups.

use super::{GroupEncodeConfig, IndexedGroupEncoder, StreamEncodeError};
use crate::{Error, ImageInfo, Metadata};
use std::io::{Read, Seek, SeekFrom, Write};

/// Source admission for a forward-only raster with one-row column groups.
///
/// `source_stride_bytes` includes padding after each row except the final row.
/// The source must supply exactly the real pixels of the final row; its trailing
/// padding, if any, remains unread. The input limit covers all consumed pixel
/// and inter-row padding bytes. Source buffer and codec working limits are
/// separate; the caller must account for both in a process budget.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ForwardRowConfig {
    /// The encoder, with its row groups set.
    pub encoder: crate::ImiEncoderConfig,
    pub source_stride_bytes: u64,
    pub max_source_buffer_bytes: usize,
    pub max_source_bytes: u64,
    pub max_index_bytes: u64,
}

impl ForwardRowConfig {
    pub fn new(
        encoder: crate::ImiEncoderConfig,
        source_stride_bytes: u64,
        max_source_buffer_bytes: usize,
        max_source_bytes: u64,
        max_index_bytes: u64,
    ) -> Self {
        Self {
            encoder,
            source_stride_bytes,
            max_source_buffer_bytes,
            max_source_bytes,
            max_index_bytes,
        }
    }
}

/// Encode a known-size lossless raster from a forward-only `Read` source.
///
/// Reads one column group at a time without buffering a complete image row.
/// The caller supplies an output `Write` sink and `Read + Write + Seek` index
/// scratch. No file is opened or removed. The existing lossless DATA/unit-index
/// grammar and decoder are used unchanged. `groups.rows` must be exactly one;
/// larger groups require a row reorder buffer that this adapter does not hide.
///
/// All capacity and geometry checks precede source reads or output writes.
/// A later input or sink failure can leave partial output; discard it. Source
/// I/O errors are returned as [`StreamEncodeError::Read`]. This convenience
/// adapter performs no color conversion, lossy coding or progress callback.
pub fn encode_forward_rows_indexed<R: Read, W: Write, S: Read + Write + Seek>(
    source: &mut R,
    writer: W,
    index_scratch: S,
    info: ImageInfo,
    metadata: &Metadata,
    cfg: ForwardRowConfig,
) -> Result<(W, S), StreamEncodeError> {
    let groups = GroupEncodeConfig::required(&cfg.encoder)?;
    if info.width == 0 || info.height == 0 || groups.rows != 1 || groups.columns == 0 {
        return Err(Error::InvalidInput("forward row group geometry").into());
    }
    let bytes_per_pixel = info.bytes_per_pixel();
    let row_bytes = u64::from(info.width)
        .checked_mul(bytes_per_pixel as u64)
        .ok_or(Error::TooLarge)?;
    if cfg.source_stride_bytes < row_bytes {
        return Err(Error::InvalidInput("forward source stride").into());
    }
    let source_extent = u64::from(info.height - 1)
        .checked_mul(cfg.source_stride_bytes)
        .and_then(|n| n.checked_add(row_bytes))
        .ok_or(Error::TooLarge)?;
    if source_extent > cfg.max_source_bytes {
        return Err(Error::TooLarge.into());
    }
    let group_bytes = usize::try_from(
        u64::from(groups.columns.min(info.width))
            .checked_mul(bytes_per_pixel as u64)
            .ok_or(Error::TooLarge)?,
    )
    .map_err(|_| Error::TooLarge)?;
    if group_bytes > cfg.max_source_buffer_bytes {
        return Err(Error::TooLarge.into());
    }
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(group_bytes)
        .map_err(|_| Error::TooLarge)?;
    pixels.resize(group_bytes, 0);
    let mut encoder = IndexedGroupEncoder::with_groups(
        writer,
        index_scratch,
        info.clone(),
        metadata,
        groups,
        cfg.max_index_bytes,
    )?;
    for y in 0..info.height {
        while let Some(rect) = encoder.next_group() {
            if rect.y != y {
                break;
            }
            let length = rect.width as usize * bytes_per_pixel;
            source
                .read_exact(&mut pixels[..length])
                .map_err(StreamEncodeError::Read)?;
            encoder.write_group(&pixels[..length], length)?;
        }
        if y + 1 < info.height {
            let mut remaining = cfg.source_stride_bytes - row_bytes;
            while remaining != 0 {
                // Reuse the admitted group buffer instead of keeping a
                // second buffer merely to discard physical row padding.
                let count = remaining.min(pixels.len() as u64) as usize;
                source
                    .read_exact(&mut pixels[..count])
                    .map_err(StreamEncodeError::Read)?;
                remaining -= count as u64;
            }
        }
    }
    encoder.finish()
}

/// Source and scratch admission for forward raster input with taller groups.
///
/// The band scratch holds at most `min(groups.rows, height)` complete image
/// rows and is reused for every band. It must be empty at entry. The source
/// buffer holds one coded group or one I/O chunk, whichever is larger, and
/// never exceeds `max_source_buffer_bytes`. The caller must include source,
/// codec, index and sink buffering in its process-level memory policy.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ForwardBandConfig {
    /// The encoder, with its row groups set.
    pub encoder: crate::ImiEncoderConfig,
    pub source_stride_bytes: u64,
    pub max_source_buffer_bytes: usize,
    pub max_source_bytes: u64,
    pub max_band_scratch_bytes: u64,
    pub max_index_bytes: u64,
}

impl ForwardBandConfig {
    pub fn new(
        encoder: crate::ImiEncoderConfig,
        source_stride_bytes: u64,
        max_source_buffer_bytes: usize,
        max_source_bytes: u64,
        max_band_scratch_bytes: u64,
        max_index_bytes: u64,
    ) -> Self {
        Self {
            encoder,
            source_stride_bytes,
            max_source_buffer_bytes,
            max_source_bytes,
            max_band_scratch_bytes,
            max_index_bytes,
        }
    }
}

/// Encode a forward-only raster in taller column groups using caller scratch.
///
/// A complete physical band is written to `band_scratch`, then its column
/// groups are read back in wire order. This trades bounded disk I/O for the
/// larger groups that a row-ordered source cannot otherwise supply. The band
/// scratch starts empty and is overwritten from offset zero for every band;
/// after success it contains the last band, and its allocated length is at
/// most the admitted maximum band size. The separate index scratch retains
/// the ordinary [`IndexedGroupEncoder`] contract. Neither scratch is opened,
/// truncated or removed by this function.
///
/// All geometry and capacity checks precede source reads or output writes.
/// A later I/O error can leave partial output and scratch, which must be
/// discarded. Source and scratch-read errors use [`StreamEncodeError::Read`];
/// scratch and output writes use [`StreamEncodeError::Write`].
pub fn encode_forward_bands_indexed<
    R: Read,
    W: Write,
    S: Read + Write + Seek,
    B: Read + Write + Seek,
>(
    source: &mut R,
    writer: W,
    index_scratch: S,
    mut band_scratch: B,
    info: ImageInfo,
    metadata: &Metadata,
    cfg: ForwardBandConfig,
) -> Result<(W, S, B), StreamEncodeError> {
    let groups = GroupEncodeConfig::required(&cfg.encoder)?;
    if info.width == 0 || info.height == 0 || groups.columns == 0 || groups.rows == 0 {
        return Err(Error::InvalidInput("forward band group geometry").into());
    }
    let bytes_per_pixel = info.bytes_per_pixel();
    let row_bytes = u64::from(info.width)
        .checked_mul(bytes_per_pixel as u64)
        .ok_or(Error::TooLarge)?;
    if cfg.source_stride_bytes < row_bytes {
        return Err(Error::InvalidInput("forward source stride").into());
    }
    let source_extent = u64::from(info.height - 1)
        .checked_mul(cfg.source_stride_bytes)
        .and_then(|n| n.checked_add(row_bytes))
        .ok_or(Error::TooLarge)?;
    if source_extent > cfg.max_source_bytes {
        return Err(Error::TooLarge.into());
    }
    let band_rows = groups.rows.min(info.height);
    let band_bytes = u64::from(band_rows)
        .checked_mul(row_bytes)
        .ok_or(Error::TooLarge)?;
    if band_bytes > cfg.max_band_scratch_bytes {
        return Err(Error::TooLarge.into());
    }
    let group_bytes = usize::try_from(
        u64::from(groups.columns.min(info.width))
            .checked_mul(u64::from(band_rows))
            .and_then(|n| n.checked_mul(bytes_per_pixel as u64))
            .ok_or(Error::TooLarge)?,
    )
    .map_err(|_| Error::TooLarge)?;
    if group_bytes > cfg.max_source_buffer_bytes {
        return Err(Error::TooLarge.into());
    }
    let buffer_bytes = group_bytes.max(cfg.max_source_buffer_bytes.min(8192));
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(buffer_bytes)
        .map_err(|_| Error::TooLarge)?;
    pixels.resize(buffer_bytes, 0);
    if band_scratch.seek(SeekFrom::End(0))? != 0 {
        return Err(Error::InvalidInput("forward band scratch not empty").into());
    }
    band_scratch.seek(SeekFrom::Start(0))?;
    let mut encoder = IndexedGroupEncoder::with_groups(
        writer,
        index_scratch,
        info.clone(),
        metadata,
        groups,
        cfg.max_index_bytes,
    )?;
    for band_y in (0..info.height).step_by(groups.rows as usize) {
        let rows = groups.rows.min(info.height - band_y);
        band_scratch.seek(SeekFrom::Start(0))?;
        for local_y in 0..rows {
            let mut remaining = row_bytes;
            while remaining != 0 {
                let count = remaining.min(pixels.len() as u64) as usize;
                source
                    .read_exact(&mut pixels[..count])
                    .map_err(StreamEncodeError::Read)?;
                band_scratch.write_all(&pixels[..count])?;
                remaining -= count as u64;
            }
            if band_y + local_y + 1 < info.height {
                let mut padding = cfg.source_stride_bytes - row_bytes;
                while padding != 0 {
                    let count = padding.min(pixels.len() as u64) as usize;
                    source
                        .read_exact(&mut pixels[..count])
                        .map_err(StreamEncodeError::Read)?;
                    padding -= count as u64;
                }
            }
        }
        while let Some(rect) = encoder.next_group() {
            if rect.y != band_y {
                break;
            }
            let group_row_bytes = rect.width as usize * bytes_per_pixel;
            for local_y in 0..rect.height {
                let offset = u64::from(local_y)
                    .checked_mul(row_bytes)
                    .and_then(|n| n.checked_add(u64::from(rect.x) * bytes_per_pixel as u64))
                    .ok_or(Error::TooLarge)?;
                band_scratch.seek(SeekFrom::Start(offset))?;
                let start = local_y as usize * group_row_bytes;
                band_scratch
                    .read_exact(&mut pixels[start..start + group_row_bytes])
                    .map_err(StreamEncodeError::Read)?;
            }
            encoder.write_group(
                &pixels[..group_row_bytes * rect.height as usize],
                group_row_bytes,
            )?;
        }
    }
    let (writer, index_scratch) = encoder.finish()?;
    Ok((writer, index_scratch, band_scratch))
}
