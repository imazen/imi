//! Streaming file checksums followed by the trailing unit index.
use super::spool::Spool;
use super::{GroupEncodeConfig, GroupRect, IndexedGroupEncoder, StreamEncodeError};
use crate::{Error, ImageInfo, Metadata};
use std::io::{Read, Seek, Write};

/// Memory, index-scratch and checksum-scratch budgets for checked group output.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ChecksummedGroupConfig {
    /// The encoder, with its row groups set.
    pub encoder: crate::ImiEncoderConfig,
    /// Conservative ten-byte-per-group admission, as in `IndexedGroupEncoder`.
    pub max_index_bytes: u64,
    /// Log2 of covered bytes per checksum piece, 10 through 40 inclusive.
    pub piece_log2: u8,
    /// Maximum bytes appended to checksum scratch (16 per piece).
    /// Charged before each digest write; exceeding it permanently fails output.
    pub max_checksum_bytes: u64,
}
impl ChecksummedGroupConfig {
    pub fn new(
        encoder: crate::ImiEncoderConfig,
        max_index_bytes: u64,
        piece_log2: u8,
        max_checksum_bytes: u64,
    ) -> Self {
        Self {
            encoder,
            max_index_bytes,
            piece_log2,
            max_checksum_bytes,
        }
    }
}

/// Bounded lossless groups, file checksum and unit index with caller-owned
/// scratch. The output sink only needs `Write`; each scratch object needs
/// `Read + Write + Seek`. Use separate disk files for bounded RAM. Existing
/// scratch prefixes are preserved and all three objects return at finish.
///
/// Pieces use incremental XXH3 state, not a piece-sized buffer. Finish hashes
/// one replay of the index, writes `cSUM`, then writes a second replay of the
/// index as the last chunk. Both scratch streams are checked during replay.
/// This preserves the existing SPEC 1.4/2.7 coverage and placement rules.
/// Scratch/output failures and exhausted scratch budgets are sticky. Discard
/// partial output on error. Invalid input extents may be corrected and retried.
/// A decoder streaming a trailing checksum may emit rows before verification;
/// callers must check its final result.
pub struct ChecksummedGroupEncoder<W, I, C> {
    encoder: IndexedGroupEncoder<W, I>,
    checksum: Pieces<C>,
}
impl<W: Write, I: Read + Write + Seek, C: Read + Write + Seek> ChecksummedGroupEncoder<W, I, C> {
    /// Admit group working storage and index capacity before output. Checksum
    /// scratch is capped incrementally because compressed size is unknown.
    /// At least 16 checksum bytes are required. The group working allowance
    /// additionally reserves 4 KiB for checksum state, beyond index replay.
    /// Caller/source buffers and allocator overhead are outside that allowance.
    pub fn new(
        writer: W,
        index_scratch: I,
        checksum_scratch: C,
        info: ImageInfo,
        meta: &Metadata,
        cfg: ChecksummedGroupConfig,
    ) -> Result<Self, StreamEncodeError> {
        let mut groups = GroupEncodeConfig::required(&cfg.encoder)?;
        if !(crate::checksum::MIN_PIECE_LOG2..=crate::checksum::MAX_PIECE_LOG2)
            .contains(&cfg.piece_log2)
        {
            return Err(Error::InvalidInput("file checksum piece size").into());
        }
        if cfg.max_checksum_bytes < cfg.encoder.hash_algorithm().width() as u64 {
            return Err(Error::TooLarge.into());
        }
        groups.max_working_bytes = groups
            .max_working_bytes
            .checked_sub(4096)
            .ok_or(Error::TooLarge)?;
        let mut encoder = IndexedGroupEncoder::new_unwritten(
            writer,
            index_scratch,
            info,
            meta,
            groups,
            cfg.max_index_bytes,
        )?;
        // Only the header changes; the DATA payload and its indexed length do not.
        encoder.session.output = crate::container::with_extension(
            core::mem::take(&mut encoder.session.output),
            crate::checksum::EXT_CHECKSUM,
            Some(&[cfg.piece_log2, cfg.encoder.hash_algorithm().wire()]),
        )?;
        let mut checksum = Pieces::new(
            checksum_scratch,
            cfg.piece_log2,
            cfg.encoder.hash_algorithm(),
            cfg.max_checksum_bytes,
        )?;
        checksum.update(encoder.session.output())?;
        encoder.drain()?;
        Ok(Self { encoder, checksum })
    }
    pub fn next_group(&self) -> Option<GroupRect> {
        self.encoder.next_group()
    }
    pub fn write_group(&mut self, pixels: &[u8], stride: usize) -> Result<(), StreamEncodeError> {
        self.encoder.prepare_group(pixels, stride)?;
        // prepare_group has already marked failure sticky until both scratch
        // and output complete; a checksum error cannot re-admit this group.
        self.checksum.update(self.encoder.session.output())?;
        self.encoder.drain()
    }
    /// Complete checksum coverage (including the index), append checksum/table,
    /// flush output and return output, index scratch and checksum scratch.
    pub fn finish(mut self) -> Result<(W, I, C), StreamEncodeError> {
        if self.encoder.failed || !self.encoder.session.is_finished() {
            return Err(Error::InvalidInput("incomplete group image").into());
        }
        self.encoder
            .visit_index(|bytes| self.checksum.update(bytes))?;
        self.checksum.finish_piece()?;
        self.checksum.write_table(&mut self.encoder.writer)?;
        let (writer, index) = self.encoder.finish()?;
        Ok((writer, index, self.checksum.digests.into_inner()))
    }
}

pub(crate) struct Pieces<S> {
    digests: Spool<S>,
    hash: crate::Hasher,
    piece_bytes: u64,
    covered: u64,
    log2: u8,
}
impl<S: Read + Write + Seek> Pieces<S> {
    pub(crate) fn into_scratch(self) -> S {
        self.digests.into_inner()
    }
    pub(crate) fn new(
        scratch: S,
        log2: u8,
        algo: crate::HashAlgorithm,
        limit: u64,
    ) -> Result<Self, StreamEncodeError> {
        Ok(Self {
            digests: Spool::new(scratch, limit, "group checksum scratch changed")?,
            hash: crate::Hasher::new(algo),
            piece_bytes: 0,
            covered: 0,
            log2,
        })
    }
    pub(crate) fn update(&mut self, mut bytes: &[u8]) -> Result<(), StreamEncodeError> {
        self.covered = self
            .covered
            .checked_add(bytes.len() as u64)
            .ok_or(Error::TooLarge)?;
        while !bytes.is_empty() {
            let count = ((1u64 << self.log2) - self.piece_bytes).min(bytes.len() as u64) as usize;
            self.hash.update(&bytes[..count]);
            self.piece_bytes += count as u64;
            bytes = &bytes[count..];
            if self.piece_bytes == 1u64 << self.log2 {
                self.finish_piece()?;
            }
        }
        Ok(())
    }
    pub(crate) fn finish_piece(&mut self) -> Result<(), StreamEncodeError> {
        if self.piece_bytes != 0 {
            self.digests.append(self.hash.finish().as_bytes())?;
            self.hash.reset();
            self.piece_bytes = 0;
        }
        Ok(())
    }
    fn write_table(&mut self, writer: &mut impl Write) -> Result<(), StreamEncodeError> {
        self.write_table_with(|bytes| {
            writer.write_all(bytes)?;
            Ok(())
        })
    }
    pub(crate) fn write_table_with(
        &mut self,
        mut write: impl FnMut(&[u8]) -> Result<(), StreamEncodeError>,
    ) -> Result<(), StreamEncodeError> {
        let payload = self
            .digests
            .len()
            .checked_add(3 + crate::varint_len(self.covered) as u64)
            .ok_or(Error::TooLarge)?;
        let mut head = Vec::with_capacity(27);
        head.extend_from_slice(&crate::checksum::CSUM);
        crate::put_varint(&mut head, payload.checked_add(1).ok_or(Error::TooLarge)?);
        let algo = self.hash.algorithm();
        head.extend_from_slice(&[algo.wire(), algo.width() as u8, self.log2]);
        crate::put_varint(&mut head, self.covered);
        write(&head)?;
        self.digests.replay(write)
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use std::io::Cursor;
    #[test]
    fn incremental_piece_boundaries_match_independent_xxh3() {
        for size in [1, 1023, 1024, 1025, 2048, 2049] {
            let bytes: Vec<_> = (0..size).map(|i| (i * 71) as u8).collect();
            for chunk in [1, 17, 1024, 4096] {
                let mut pieces = Pieces::new(
                    Cursor::new(Vec::new()),
                    10,
                    crate::HashAlgorithm::DEFAULT,
                    64,
                )
                .unwrap();
                for part in bytes.chunks(chunk) {
                    pieces.update(part).unwrap();
                }
                pieces.finish_piece().unwrap();
                assert_eq!(pieces.covered, size as u64);
                let digests = pieces.digests.into_inner().into_inner();
                let expected: Vec<_> = bytes
                    .chunks(1024)
                    .flat_map(|b| xxhash_rust::xxh3::xxh3_128(b).to_le_bytes())
                    .collect();
                assert_eq!(digests, expected, "size {size} chunk {chunk}");
            }
        }
    }
}
