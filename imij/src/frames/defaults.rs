//! Advisory sequence defaults resolved separately from pixel prediction.
use alloc::vec::Vec;

use crate::item_metadata::{self, ItemMetadata};
use crate::parse::chunks;
use crate::{ByteReader, Chunk, Error, ImageInfo, Metadata};

pub(super) use crate::parse::chunks::SDEF as TAG;

fn check_tag(tag: [u8; 4]) -> Result<(), Error> {
    match &tag {
        b"sMDT" | b"aXES" | b"cALB" | b"cATG" => Ok(()),
        _ if item_metadata::is_tag(&tag) => Ok(()),
        _ => Err(Error::Unsupported("sequence default tag")),
    }
}

fn same_schema(a: &[u8], b: &[u8]) -> Result<bool, Error> {
    let a = crate::science::structured_identity(a)?;
    let b = crate::science::structured_identity(b)?;
    Ok(matches!((a, b), (Some(a), Some(b))
        if a.schema == b.schema && a.instance_key == b.instance_key))
}

#[cfg(feature = "encode")]
pub(super) fn serialize(values: &[Chunk]) -> Result<Vec<u8>, Error> {
    let limit = crate::decode_resources::DEFAULT_METADATA_BYTES;
    let mut size = 1usize;
    for value in values {
        check_tag(value.tag)?;
        size = size
            .checked_add(4 + crate::varint_len(value.data.len() as u64 + 1))
            .and_then(|n| n.checked_add(value.data.len()))
            .ok_or(Error::TooLarge)?;
        if size > limit {
            return Err(Error::TooLarge);
        }
    }
    let mut out = Vec::new();
    out.try_reserve_exact(size).map_err(|_| Error::TooLarge)?;
    out.push(1);
    for value in values {
        let tag = if item_metadata::is_tag(&value.tag) {
            item_metadata::TAG
        } else {
            value.tag
        };
        crate::container::put_chunk(&mut out, &tag, &value.data);
    }
    // Check the variant/identity constraints before publishing bytes.
    walk(&out, limit, |tag, bytes| {
        if item_metadata::is_tag(&tag) {
            ItemMetadata::decode_chunk(bytes, limit)?;
        }
        Ok(())
    })?;
    Ok(out)
}

fn walk(
    payload: &[u8],
    max: usize,
    mut read: impl FnMut([u8; 4], &[u8]) -> Result<(), Error>,
) -> Result<(), Error> {
    if payload.len() > max {
        return Err(Error::TooLarge);
    }
    let mut input = ByteReader::new(payload);
    if input.u8()? != 1 {
        return Err(Error::Unsupported("sequence defaults version"));
    }
    let mut singles = [false; 4];
    let mut structured = [None; 64];
    let mut count = 0;
    while input.remaining() != 0 {
        let (tag, len) = chunks::read_head(&mut input)?;
        let len = len.ok_or(Error::Corrupt("open sequence default"))?;
        check_tag(tag)?;
        let bytes = chunks::payload(&mut input, len)?;
        if item_metadata::is_tag(&tag) && bytes.get(1) == Some(&1) {
            return Err(Error::Corrupt("sequence default filename"));
        }
        if &tag == b"sMDT" {
            let id = crate::science::structured_identity(bytes)?
                .ok_or(Error::Unsupported("sequence default structured identity"))?;
            let key = (id.schema, id.instance_key);
            if count == structured.len() || structured[..count].contains(&Some(key)) {
                return Err(Error::Corrupt(
                    "sequence default structured count or duplicate",
                ));
            }
            structured[count] = Some(key);
            count += 1;
        } else {
            let i = match &tag {
                _ if item_metadata::is_tag(&tag) => 0,
                b"aXES" => 1,
                b"cALB" => 2,
                b"cATG" => 3,
                _ => unreachable!(),
            };
            if core::mem::replace(&mut singles[i], true) {
                return Err(Error::Corrupt("repeated sequence default"));
            }
        }
        read(tag, bytes)?;
    }
    Ok(())
}

pub(crate) fn resolve(
    payload: &[u8],
    item: &mut Metadata,
    info: &ImageInfo,
    primary: &ImageInfo,
    max: usize,
) -> Result<(), Error> {
    let same_domain = info.channels == primary.channels
        && info.sample == primary.sample
        && info.significant_bits == primary.significant_bits;
    walk(payload, max, |tag, bytes| {
        if matches!(&tag, b"aXES" | b"cALB" | b"cATG") && !same_domain {
            return Ok(());
        }
        if item_metadata::is_tag(&tag) {
            let mut defaults = ItemMetadata::decode_chunk(bytes, max)?;
            if defaults.filename.is_some() {
                return Err(Error::Corrupt("sequence default filename"));
            }
            let own = ItemMetadata::from_metadata_with_byte_limit(item, max)?;
            defaults.filename = own.filename;
            defaults.attributes.extend(own.attributes);
            return defaults.write_metadata_with_byte_limit(item, max);
        }
        for existing in item.chunks.iter().filter(|chunk| chunk.tag == tag) {
            if &tag != b"sMDT" || same_schema(bytes, &existing.data)? {
                return Ok(());
            }
        }
        let mut data = Vec::new();
        data.try_reserve_exact(bytes.len())
            .map_err(|_| Error::TooLarge)?;
        data.extend_from_slice(bytes);
        item.chunks.try_reserve(1).map_err(|_| Error::TooLarge)?;
        item.chunks.push(Chunk { tag, data });
        Ok(())
    })
}
