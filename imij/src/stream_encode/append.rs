//! Extend a canonical indexed group image without recoding its old groups.

use super::{
    GroupEncodeConfig, GroupRect, IndexedGroupEncoder,
    NativeGroupEncodeSession as GroupEncodeSession, StreamEncodeError,
};
use crate::decode_resources::{DecodePolicy, DecodeSession};
use crate::{ByteReader, Error};
use std::io::{Read, Seek, SeekFrom, Write};

/// Writes a new indexed lossless file with additional bottom rows.
///
/// The old image must be a standalone file written by [`IndexedGroupEncoder`]
/// with the same group geometry and metadata serialization as this build. Its
/// height must end on a group-stripe boundary. Construction checks the complete
/// old file under `decode_policy`, retains its metadata, matches its exact
/// header and entropy-model bytes, and copies its compressed group records
/// into `writer` in fixed-size blocks. Later groups are encoded normally.
/// The old source is never modified; `writer` must be a distinct destination.
///
/// A file checksum, pixel checksum, animation/page item, noncanonical header,
/// or different group geometry is rejected before the destination is written.
/// Caller-owned index scratch grows with the number of groups. A source or
/// destination failure after output begins leaves a partial file to discard.
pub struct AppendRowsEncoder<R, W, S> {
    source: R,
    writer: IndexedGroupEncoder<W, S>,
}

impl<R: Read + Seek, W: Write, S: Read + Write + Seek> AppendRowsEncoder<R, W, S> {
    /// Copy validated old compressed groups, then accept `added_rows` more.
    /// `cfg` must match the old file's group geometry. `decode_policy` bounds
    /// the full source-validation pass, including metadata and working state.
    /// No destination bytes are written until that pass and the exact prefix,
    /// model, index and geometry checks have succeeded.
    pub fn new(
        mut source: R,
        destination: W,
        index_scratch: S,
        added_rows: u32,
        cfg: &crate::ImiEncoderConfig,
        max_index_bytes: u64,
        decode_policy: DecodePolicy,
    ) -> Result<Self, StreamEncodeError> {
        let cfg = GroupEncodeConfig::required(cfg)?;
        if added_rows == 0 || cfg.rows == 0 || cfg.columns == 0 {
            return Err(
                Error::InvalidInput("append rows and group geometry must be nonzero").into(),
            );
        }
        let file_len = source.seek(SeekFrom::End(0)).map_err(|_| Error::Read)?;
        source.seek(SeekFrom::Start(0)).map_err(|_| Error::Read)?;
        let mut validation = DecodeSession::new(decode_policy);
        let (old_info, metadata) = validation.decode_reader(&mut source, |_, _, _| Ok(()))?;
        if source.seek(SeekFrom::End(0)).map_err(|_| Error::Read)? != file_len {
            return Err(Error::Corrupt("append source length changed").into());
        }
        if old_info.height < cfg.rows || old_info.height % cfg.rows != 0 {
            return Err(Error::InvalidInput("old height must end on a group stripe").into());
        }
        let mut new_info = old_info.clone();
        new_info.height = new_info
            .height
            .checked_add(added_rows)
            .ok_or(Error::TooLarge)?;

        let (old_head, old_data_head_len) =
            GroupEncodeSession::new_inner(old_info.clone(), &metadata, cfg, true)?;
        let old_prefix = old_head.output();
        let data_at = (old_prefix.len() as u64)
            .checked_sub(old_data_head_len)
            .ok_or(Error::TooLarge)?;
        if old_prefix.len() as u64 > file_len {
            return Err(Error::Corrupt("short indexed append source").into());
        }
        let mut block = [0u8; 8192];
        let mut prefix_at = 0u64;
        for expected in old_prefix.chunks(block.len()) {
            read_at(&mut source, prefix_at, &mut block[..expected.len()])?;
            if block[..expected.len()] != *expected {
                return Err(Error::Unsupported(
                    "append source is not a canonical indexed group file",
                )
                .into());
            }
            prefix_at += expected.len() as u64;
        }
        let (new_head, new_data_head_len) =
            GroupEncodeSession::new_inner(new_info.clone(), &metadata, cfg, true)?;
        let old_data_head = &old_prefix[data_at as usize..];
        let new_data_start = new_head.output().len() - new_data_head_len as usize;
        if old_data_head != &new_head.output()[new_data_start..] {
            return Err(Error::Unsupported("append group model or geometry differs").into());
        }

        let index = IndexLayout::read(
            &mut source,
            file_len,
            data_at,
            old_data_head_len,
            old_info.width,
            old_info.height,
            cfg,
        )?;
        // The walking decoder validates DATA but does not need the trailing
        // index. Check every span before the destination receives its header.
        let mut index_cursor = index.entries_at;
        let mut data_cursor = old_prefix.len() as u64;
        for _ in 0..index.groups {
            let span = read_span(&mut source, &mut index_cursor, index.entries_end)?;
            let end = data_cursor
                .checked_add(span)
                .filter(|&end| span != 0 && end <= index.index_at)
                .ok_or(Error::Corrupt("append source group span exceeds DATA"))?;
            check_group_head(&mut source, data_cursor, span)?;
            data_cursor = end;
        }
        if index_cursor != index.entries_end || data_cursor != index.index_at {
            return Err(Error::Corrupt("append source index coverage").into());
        }
        let mut writer = IndexedGroupEncoder::with_groups(
            destination,
            index_scratch,
            new_info,
            &metadata,
            cfg,
            max_index_bytes,
        )?;
        let mut index_cursor = index.entries_at;
        let mut data_cursor = old_prefix.len() as u64;
        for _ in 0..index.groups {
            let span = read_span(&mut source, &mut index_cursor, index.entries_end)?;
            if span == 0
                || data_cursor
                    .checked_add(span)
                    .is_none_or(|end| end > index.index_at)
            {
                return Err(Error::Corrupt("append source group span exceeds DATA").into());
            }
            source
                .seek(SeekFrom::Start(data_cursor))
                .map_err(|_| Error::Read)?;
            writer.import_record(&mut source, span)?;
            data_cursor += span;
        }
        if index_cursor != index.entries_end || data_cursor != index.index_at {
            return Err(Error::Corrupt("append source index coverage").into());
        }
        if !writer
            .next_group()
            .is_some_and(|g| g.x == 0 && g.y == old_info.height)
        {
            return Err(Error::Corrupt("append source group geometry").into());
        }
        Ok(Self { source, writer })
    }

    /// The next newly supplied rectangle. All old groups are already copied.
    pub fn next_group(&self) -> Option<GroupRect> {
        self.writer.next_group()
    }

    /// Encode a strided rectangle of the added rows.
    pub fn write_group(&mut self, pixels: &[u8], stride: usize) -> Result<(), StreamEncodeError> {
        self.writer.write_group(pixels, stride)
    }

    /// Write the new trailing index and return the caller-owned streams.
    pub fn finish(self) -> Result<(W, S, R), StreamEncodeError> {
        let (writer, scratch) = self.writer.finish()?;
        Ok((writer, scratch, self.source))
    }
}

fn read_at<R: Read + Seek>(source: &mut R, at: u64, bytes: &mut [u8]) -> Result<(), Error> {
    source.seek(SeekFrom::Start(at)).map_err(|_| Error::Read)?;
    source.read_exact(bytes).map_err(|_| Error::Read)
}

fn read_span<R: Read + Seek>(source: &mut R, at: &mut u64, end: u64) -> Result<u64, Error> {
    source.seek(SeekFrom::Start(*at)).map_err(|_| Error::Read)?;
    let mut bytes = [0u8; 10];
    for i in 0..bytes.len() {
        if *at == end {
            return Err(Error::Corrupt("short append source index"));
        }
        source
            .read_exact(&mut bytes[i..i + 1])
            .map_err(|_| Error::Read)?;
        *at += 1;
        if bytes[i] < 128 {
            return ByteReader::new(&bytes[..=i]).varint();
        }
    }
    Err(Error::Corrupt("bad append source index varint"))
}

fn check_group_head<R: Read + Seek>(source: &mut R, at: u64, span: u64) -> Result<(), Error> {
    let first_len = usize::try_from(span.min(20)).unwrap();
    if first_len < 2 {
        return Err(Error::Corrupt("short append source group record"));
    }
    let mut first = [0u8; 20];
    read_at(source, at, &mut first[..first_len])?;
    let mut record = ByteReader::new(&first[..first_len]);
    let kind = record.varint()?;
    let payload = record.varint()?;
    if kind != crate::parse::records::R_STRIPE
        || (record.pos as u64).checked_add(payload) != Some(span)
    {
        return Err(Error::Corrupt("append source group record head"));
    }
    Ok(())
}

struct IndexLayout {
    index_at: u64,
    entries_at: u64,
    entries_end: u64,
    groups: u64,
}

impl IndexLayout {
    fn read<R: Read + Seek>(
        source: &mut R,
        file_len: u64,
        data_at: u64,
        data_head_len: u64,
        width: u32,
        height: u32,
        cfg: GroupEncodeConfig,
    ) -> Result<Self, Error> {
        if file_len < 25 {
            return Err(Error::Corrupt("short append source index"));
        }
        let mut footer = [0u8; 20];
        read_at(source, file_len - 20, &mut footer)?;
        if &footer[16..] != b"uIDX" {
            return Err(Error::Unsupported(
                "append source lacks trailing unit index",
            ));
        }
        let data_len = u64::from_le_bytes(footer[..8].try_into().unwrap());
        let index_len = u64::from_le_bytes(footer[8..16].try_into().unwrap());
        let index_at = file_len
            .checked_sub(index_len)
            .ok_or(Error::Corrupt("append index length"))?;
        if index_at != data_at.checked_add(data_len).ok_or(Error::TooLarge)?
            || data_len < data_head_len
            || index_len < 25
        {
            return Err(Error::Corrupt("append index/data boundary"));
        }
        let mut head = [0u8; 14];
        read_at(source, index_at, &mut head)?;
        if &head[..4] != b"uIDX" {
            return Err(Error::Corrupt("append index tag"));
        }
        let mut r = ByteReader::new(&head[4..]);
        let encoded_len = r.varint()?;
        let payload_len = encoded_len
            .checked_sub(1)
            .ok_or(Error::Corrupt("append index payload"))?;
        let head_len = 4 + r.pos as u64;
        if payload_len < 20 || head_len.checked_add(payload_len) != Some(index_len) {
            return Err(Error::Corrupt("append index payload length"));
        }
        let entries_at = index_at + head_len;
        let entries_end = file_len - 20;
        let groups = u64::from(width.div_ceil(cfg.columns))
            .checked_mul(u64::from(height / cfg.rows))
            .ok_or(Error::TooLarge)?;
        let entry_bytes = entries_end - entries_at;
        if entry_bytes < groups || entry_bytes > groups.checked_mul(10).ok_or(Error::TooLarge)? {
            return Err(Error::Corrupt("append index entry count"));
        }
        Ok(Self {
            index_at,
            entries_at,
            entries_end,
            groups,
        })
    }
}
