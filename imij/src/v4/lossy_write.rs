//! Bounded lossy I/O selects directory policy independently from pixel coding.
use super::DirectoryPlacement;
use crate::stream_encode::{LossyUnitIndex, StreamEncodeError};
use crate::{ImageInfo, ImiEncoderConfig, Metadata};
use std::io::{Read, Seek, Write};

#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct BoundedLossyConfig {
    pub source_stride_bytes: u64,
    pub max_index_bytes: u64,
    pub directory: DirectoryPlacement,
    pub front_capacity_bytes: u64,
}
impl BoundedLossyConfig {
    pub const fn new(source_stride_bytes: u64, max_index_bytes: u64) -> Self {
        Self {
            source_stride_bytes,
            max_index_bytes,
            directory: DirectoryPlacement::Tail,
            front_capacity_bytes: 0,
        }
    }
    pub const fn without_directory(mut self) -> Self {
        self.directory = DirectoryPlacement::None;
        self.front_capacity_bytes = 0;
        self
    }
    /// Reserve this many bytes in the V4 front slot, including its length
    /// varint and padding. A slot too small fails finalization, without a
    /// second index authority or fallback grammar. Use the seekable entry point.
    pub const fn with_front_directory(mut self, capacity: u64) -> Self {
        self.directory = DirectoryPlacement::Front;
        self.front_capacity_bytes = capacity;
        self
    }
}
fn legacy(cfg: BoundedLossyConfig) -> crate::stream_encode::BoundedLossyConfig {
    crate::stream_encode::BoundedLossyConfig::new(
        cfg.source_stride_bytes,
        LossyUnitIndex::Trailing,
        cfg.max_index_bytes,
    )
}
/// Encode native lossy bands to a forward-only V4 sink. Source and caller-owned
/// index/checksum scratch are seekable; the destination needs only Write.
/// Supports no directory or a final directory. Reconstruction is identical to
/// the native bounded encoder; source padding and scratch prefixes are retained.
pub fn encode_lossy_bounded<R, W, S>(
    source: &mut R,
    writer: W,
    index_scratch: S,
    checksum_scratch: Option<S>,
    info: ImageInfo,
    metadata: &Metadata,
    encoder: &ImiEncoderConfig,
    cfg: BoundedLossyConfig,
    progress: &dyn how_far::Pulse,
) -> Result<(W, S, Option<S>), StreamEncodeError>
where
    R: Read + Seek,
    W: Write,
    S: Read + Write + Seek,
{
    if cfg.directory == DirectoryPlacement::Front {
        return Err(
            crate::Error::InvalidInput("front directory requires seekable V4 output").into(),
        );
    }
    let (output, index, checksum) = crate::stream_encode::lossy::encode_lossy_bounded_version(
        source,
        crate::stream_encode::lossy::ForwardOutput::new(writer),
        index_scratch,
        checksum_scratch,
        info,
        metadata,
        encoder,
        legacy(cfg),
        progress,
        Some((cfg.directory, cfg.front_capacity_bytes)),
    )?;
    Ok((output.inner, index, checksum))
}
/// As the forward entry point, with seekable output for front reservations.
/// This patches only the admitted directory slot, without replaying pixels or
/// collecting the compressed image. Caller scratch keeps its separate bounds.
pub fn encode_lossy_bounded_seekable<R, W, S>(
    source: &mut R,
    writer: W,
    index_scratch: S,
    checksum_scratch: Option<S>,
    info: ImageInfo,
    metadata: &Metadata,
    encoder: &ImiEncoderConfig,
    cfg: BoundedLossyConfig,
    progress: &dyn how_far::Pulse,
) -> Result<(W, S, Option<S>), StreamEncodeError>
where
    R: Read + Seek,
    W: Write + Seek,
    S: Read + Write + Seek,
{
    crate::stream_encode::lossy::encode_lossy_bounded_version(
        source,
        writer,
        index_scratch,
        checksum_scratch,
        info,
        metadata,
        encoder,
        legacy(cfg),
        progress,
        Some((cfg.directory, cfg.front_capacity_bytes)),
    )
}
