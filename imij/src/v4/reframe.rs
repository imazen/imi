//! Encoded-domain directory relocation before appending a plane-set suffix.
//! Relative object offsets and setup identity remain unchanged.
use crate::decode_resources::{DecodePolicy, DecodeSession};
use crate::parse::{chunks, header};
use crate::{ByteReader, Error, UpgradeLimits};
use alloc::vec::Vec;

/// Assemble an admitted tail-layout payload with its directory at the front.
/// The index excludes the tail locator. No raster or entropy is reconstructed.
pub(crate) fn front_payload(
    data: &[u8],
    index: &[u8],
    limits: &UpgradeLimits,
) -> Result<Vec<u8>, Error> {
    let mut setup = ByteReader::new(data);
    let length = setup.var_usize()?;
    setup.bytes(length)?;
    let immutable_end = setup.pos;
    if super::DirectoryPlacement::try_from(setup.varint()?)? != super::DirectoryPlacement::Tail {
        return Err(Error::Corrupt("front relocation needs a tail payload"));
    }
    let mut payload = Vec::new();
    crate::upgrade::append(&mut payload, &data[..immutable_end], limits)?;
    let capacity = index
        .len()
        .checked_add(crate::varint_len(index.len() as u64))
        .ok_or(Error::TooLarge)?;
    super::append_directory_prefix(
        &mut payload,
        super::DirectoryPlacement::Front,
        capacity as u64,
        Some(index),
        limits.max_encoded_bytes as u64,
    )?;
    crate::upgrade::append(&mut payload, &data[setup.pos..], limits)?;
    Ok(payload)
}

pub(crate) fn front_directory(file: &[u8], limits: &UpgradeLimits) -> Result<Vec<u8>, Error> {
    if file.len() > limits.max_encoded_bytes {
        return Err(Error::TooLarge);
    }
    let session =
        DecodeSession::new(DecodePolicy::default().with_output_limits(crate::Limits::NONE));
    let parsed = super::file::parse_file(file, &session)?;
    let mut setup = ByteReader::new(parsed.data);
    let length = setup.var_usize()?;
    setup.bytes(length)?;
    let placement = super::DirectoryPlacement::try_from(setup.varint()?)?;
    if placement != super::DirectoryPlacement::Tail {
        let mut output = Vec::new();
        crate::upgrade::append(&mut output, file, limits)?;
        return Ok(output);
    }
    let (tail, chunk_len) = parsed
        .trailing_index
        .ok_or(Error::Corrupt("missing V4 tail directory"))?;
    let index = super::file::check_tail_locator(tail, chunk_len)?;
    let payload = front_payload(parsed.data, index, limits)?;
    let mut r = ByteReader::new(file);
    header::read_versioned(&mut r, u64::MAX)?;
    let mut output = Vec::new();
    crate::upgrade::append(&mut output, &file[..r.pos], limits)?;
    let summed = crate::container::ChecksumChunk::rewritable(parsed.checksum)?;
    while r.remaining() != 0 {
        let start = r.pos;
        let (tag, length) = chunks::read_head(&mut r)?;
        match length {
            Some(n) => {
                chunks::payload(&mut r, n)?;
            }
            None => {
                r.bytes(parsed.data.len())?;
            }
        }
        match &tag {
            b"DATA" => crate::upgrade::chunk(&mut output, &tag, &payload, limits)?,
            b"uIDX" | b"cSUM" => {}
            _ => crate::upgrade::append(&mut output, &file[start..r.pos], limits)?,
        }
    }
    if summed {
        output = crate::checksum::refinish(output)?;
    }
    if output.len() > limits.max_encoded_bytes {
        return Err(Error::TooLarge);
    }
    super::file::parse_file(&output, &session)?;
    Ok(output)
}
