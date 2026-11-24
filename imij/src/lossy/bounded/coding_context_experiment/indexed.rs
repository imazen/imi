//! One directory parser for resident decode and real, counted sparse reads.
use super::*;
use crate::planes::ReadAt;
use core::ops::Range;

pub(super) struct LocatedContext {
    pub body: Range<usize>,
    pub features: Features,
}
pub(super) struct Directory {
    pub header: Header,
    pub group_rows: usize,
    pub groups: Vec<LocatedContext>,
    pub header_bytes: usize,
    pub index_start: usize,
}

fn read_bytes(source: &mut impl ReadAt, at: usize, length: usize) -> Result<Vec<u8>, Error> {
    let end = at.checked_add(length).ok_or(Error::TooLarge)?;
    if end as u64 > source.len()? {
        return Err(Error::Corrupt("study source extent"));
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| Error::TooLarge)?;
    bytes.resize(length, 0);
    source.read_exact_at(at as u64, &mut bytes)?;
    Ok(bytes)
}
fn varint(source: &mut impl ReadAt, at: &mut usize) -> Result<u64, Error> {
    let mut bytes = [0u8; 10];
    for index in 0..bytes.len() {
        source.read_exact_at(*at as u64, &mut bytes[index..index + 1])?;
        *at = at.checked_add(1).ok_or(Error::TooLarge)?;
        if bytes[index] & 128 == 0 {
            return ByteReader::new(&bytes[..=index]).varint();
        }
    }
    Err(Error::Corrupt("study varint overflow"))
}
fn integer(source: &mut impl ReadAt, at: &mut usize) -> Result<usize, Error> {
    usize::try_from(varint(source, at)?).map_err(|_| Error::TooLarge)
}

pub(super) fn read_directory(source: &mut impl ReadAt) -> Result<Directory, Error> {
    let length = usize::try_from(source.len()?).map_err(|_| Error::TooLarge)?;
    if read_bytes(source, 0, 8)?.as_slice() != MAGIC {
        return Err(Error::Corrupt("study context signature"));
    }
    let mut at = 8;
    let header_length = integer(source, &mut at)?;
    if header_length > 1 << 20 {
        return Err(Error::TooLarge);
    }
    let common = read_bytes(source, at, header_length)?;
    at = at.checked_add(header_length).ok_or(Error::TooLarge)?;
    let mut common_reader = ByteReader::new(&common);
    let parsed = crate::parse::header::read_header_sized(&mut common_reader)?;
    if common_reader.pos != header_length
        || parsed.header.lossy_color.is_none()
        || parsed.checksum.is_some()
        || parsed.locator.is_some()
    {
        return Err(Error::Corrupt("study common declaration"));
    }
    crate::parse::limits::check_limits(&parsed.header.info, &crate::Limits::DEFAULT)?;
    let rows = integer(source, &mut at)?;
    if rows == 0 || !rows.is_multiple_of(8) {
        return Err(Error::Corrupt("study group rows"));
    }
    let header_bytes = at;
    let count = (parsed.header.info.height as usize).div_ceil(rows);
    if count > 1 << 20 {
        return Err(Error::TooLarge);
    }
    let footer_start = length
        .checked_sub(16)
        .ok_or(Error::Corrupt("study footer extent"))?;
    let footer = read_bytes(source, footer_start, 16)?;
    if &footer[8..] != FOOTER {
        return Err(Error::Corrupt("study footer"));
    }
    let index_start = usize::try_from(u64::from_le_bytes(footer[..8].try_into().unwrap()))
        .map_err(|_| Error::TooLarge)?;
    let index_length = count.checked_mul(16).ok_or(Error::TooLarge)?;
    if index_start < header_bytes || index_start.checked_add(index_length) != Some(footer_start) {
        return Err(Error::Corrupt("study index extent"));
    }
    let index = read_bytes(source, index_start, index_length)?;
    let mut groups = Vec::new();
    groups
        .try_reserve_exact(count)
        .map_err(|_| Error::TooLarge)?;
    let mut next = header_bytes;
    for (ordinal, entry) in index.as_chunks::<16>().0.iter().enumerate() {
        let start = usize::try_from(u64::from_le_bytes(entry[..8].try_into().unwrap()))
            .map_err(|_| Error::TooLarge)?;
        let size = usize::try_from(u64::from_le_bytes(entry[8..].try_into().unwrap()))
            .map_err(|_| Error::TooLarge)?;
        let end = start.checked_add(size).ok_or(Error::TooLarge)?;
        if start != next || end > index_start {
            return Err(Error::Corrupt("study index association"));
        }
        let mut position = start;
        if integer(source, &mut position)? != ordinal {
            return Err(Error::Corrupt("study context ordinal"));
        }
        let payload_length = integer(source, &mut position)?;
        let features = Features::from_bits(varint(source, &mut position)?)
            .ok_or(Error::Unsupported("study numeric feature"))?;
        if position.checked_add(payload_length) != Some(end) {
            return Err(Error::Corrupt("study context length"));
        }
        groups.push(LocatedContext {
            body: position..end,
            features,
        });
        next = end;
    }
    if next != index_start {
        return Err(Error::Corrupt("study context gap"));
    }
    Ok(Directory {
        header: parsed.header,
        group_rows: rows,
        groups,
        header_bytes,
        index_start,
    })
}

pub(super) fn center_pixel(source: &mut impl ReadAt) -> Result<Vec<u8>, Error> {
    let directory = read_directory(source)?;
    let info = &directory.header.info;
    let y = info.height as usize / 2;
    let ordinal = y / directory.group_rows;
    let group = &directory.groups[ordinal];
    let body = read_bytes(source, group.body.start, group.body.len())?;
    let mut local = info.clone();
    local.height = directory
        .group_rows
        .min(info.height as usize - ordinal * directory.group_rows) as u32;
    let mut pixels = vec![0; local.row_bytes() * local.height as usize];
    decode_group(
        local,
        &directory.header,
        group.features,
        &body,
        &mut pixels,
        info.row_bytes(),
    )?;
    let at = y % directory.group_rows * info.row_bytes()
        + info.width as usize / 2 * info.bytes_per_pixel();
    Ok(pixels[at..at + info.bytes_per_pixel()].to_vec())
}
