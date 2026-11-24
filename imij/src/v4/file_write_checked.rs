//! Checked forward output reuses the ordinary incremental checksum engine.
use super::*;
use crate::stream_encode::{ChecksummedGroupConfig, checked::Pieces};

/// Forward V4 numeric output with independent caller-owned directory and
/// checksum scratch. Completed groups flush before tile/index finalization.
/// Their pixels remain provisional until the decoder validates tile/file scopes.
/// Both scratch prefixes are preserved and both replay streams detect mutation.
/// No complete image, compressed file or piece-sized buffer is retained.
pub struct ChecksummedNumericFileEncoder<W, I, C> {
    inner: NumericFileEncoder<W, I>,
    checksum: Pieces<C>,
}

impl<W: Write, I: Read + Write + Seek, C: Read + Write + Seek>
    ChecksummedNumericFileEncoder<W, I, C>
{
    /// Admit geometry, working storage and output prefix before publication.
    /// The shared group config sets separate index/checksum scratch bounds and
    /// checksum piece size. Both lossless and Quick accept strided group input.
    pub fn new(
        info: ImageInfo,
        metadata: &Metadata,
        spatial: SpatialConfig,
        writer: W,
        index_scratch: I,
        checksum_scratch: C,
        cfg: ChecksummedGroupConfig,
        max_output_bytes: u64,
    ) -> Result<Self, StreamEncodeError> {
        Self::new_checked(
            info,
            metadata,
            spatial,
            writer,
            index_scratch,
            checksum_scratch,
            cfg,
            max_output_bytes,
            crate::v4::Checks::default(),
        )
    }

    /// Add independent scoped pixel/object checks alongside file checksums.
    pub fn new_checked(
        info: ImageInfo,
        metadata: &Metadata,
        spatial: SpatialConfig,
        writer: W,
        index_scratch: I,
        checksum_scratch: C,
        cfg: ChecksummedGroupConfig,
        max_output_bytes: u64,
        checks: crate::v4::Checks<'_>,
    ) -> Result<Self, StreamEncodeError> {
        if !(crate::checksum::MIN_PIECE_LOG2..=crate::checksum::MAX_PIECE_LOG2)
            .contains(&cfg.piece_log2)
        {
            return Err(Error::InvalidInput("file checksum piece size").into());
        }
        let algorithm = cfg.encoder.hash_algorithm();
        if cfg.max_checksum_bytes < algorithm.width() as u64 {
            return Err(Error::TooLarge.into());
        }
        let mut groups = cfg
            .encoder
            .row_groups()
            .ok_or(Error::InvalidInput("row groups required"))?;
        groups.max_working_bytes = groups
            .max_working_bytes
            .checked_sub(4096)
            .ok_or(Error::TooLarge)?;
        let encoder = cfg.encoder.with_row_groups(groups);
        let (mut inner, header, setup) = NumericFileEncoder::unwritten(
            info,
            metadata,
            &encoder,
            spatial,
            writer,
            index_scratch,
            cfg.max_index_bytes,
            max_output_bytes,
            Some((cfg.piece_log2, algorithm)),
            checks,
        )?;
        let mut checksum = Pieces::new(
            checksum_scratch,
            cfg.piece_log2,
            algorithm,
            cfg.max_checksum_bytes,
        )?;
        for bytes in [&header[..], b"DATA\0", &setup[..]] {
            checksum.update(bytes)?;
        }
        inner.start(&header, &setup)?;
        Ok(Self { inner, checksum })
    }
    pub fn layout(&self) -> super::super::TileLayout {
        self.inner.layout()
    }
    pub fn bytes_written(&self) -> u64 {
        self.inner.bytes_written()
    }
    pub fn next_group(&self) -> Option<GroupRect> {
        self.inner.next_group()
    }

    fn publish(&mut self, entry: LocatedEntry) -> Result<(), StreamEncodeError> {
        self.checksum.update(self.inner.encoder.output())?;
        self.inner.publish(entry)
    }
    pub fn begin_tile(&mut self) -> Result<LocatedEntry, StreamEncodeError> {
        self.inner.ready()?;
        let result = (|| {
            let entry = self.inner.encoder.begin_tile()?;
            self.publish(entry)?;
            Ok(entry)
        })();
        self.inner.failed |= result.is_err();
        result
    }
    pub fn push_group(
        &mut self,
        pixels: &[u8],
        stride: usize,
    ) -> Result<LocatedEntry, StreamEncodeError> {
        self.inner.ready()?;
        let result = (|| {
            let entry = self.inner.encoder.push_group(pixels, stride)?;
            self.publish(entry)?;
            Ok(entry)
        })();
        self.inner.failed |= result.is_err();
        result
    }
    pub fn end_tile(&mut self) -> Result<TileExtent, StreamEncodeError> {
        self.inner.ready()?;
        let result = (|| {
            let (entry, extent) = self.inner.encoder.end_tile()?;
            self.publish(entry)?;
            Ok(extent)
        })();
        self.inner.failed |= result.is_err();
        result
    }
    /// Hash a verified replay of the final index, write the checksum table,
    /// then write a second verified index replay as the final chunk. Output
    /// failure or scratch mutation returns an error; partial output is discarded.
    pub fn finish(mut self) -> Result<(W, I, C), StreamEncodeError> {
        self.inner.ready()?;
        replay_index(
            &self.inner.encoder,
            &self.inner.directory,
            &mut self.inner.index,
            self.inner
                .max_output
                .checked_sub(self.inner.written)
                .ok_or(Error::TooLarge)?,
            |bytes| self.checksum.update(bytes),
        )?;
        self.checksum.finish_piece()?;
        self.checksum
            .write_table_with(|bytes| self.inner.write(bytes))?;
        let (writer, index) = self.inner.finish()?;
        Ok((writer, index, self.checksum.into_scratch()))
    }
}
