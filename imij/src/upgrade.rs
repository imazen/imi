//! Encoded-domain migration. Entropy payloads and decoded-raster digests are opaque.
#[cfg(test)]
use crate::parse::chunks;
use crate::parse::{header, limits::Structure, records};
use crate::{ByteReader, Error, HashAlgorithm, container};
use alloc::vec::Vec;

/// Admission bounds for one encoded image and its nested images during upgrade.
/// No pixel buffers are allocated. Working storage includes the borrowed input,
/// rewritten output, parser indexes and nested rewrite buffers; this is not an
/// allocator/RSS limit. Archive callers apply the byte bound before reading a blob.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct UpgradeLimits {
    /// Maximum encoded bytes in each input and output file (default 64 MiB).
    pub max_encoded_bytes: usize,
    /// Maximum nested image depth; the outer file is depth zero (default 16).
    pub max_depth: usize,
    /// Plane directory/record admission bounds. Defaults to
    /// [`crate::planes::PlaneSetLimits::default`]. File and record byte limits
    /// are additionally capped by `max_encoded_bytes`; no raster is decoded.
    pub plane_limits: crate::planes::PlaneSetLimits,
}
impl Default for UpgradeLimits {
    fn default() -> Self {
        Self {
            max_encoded_bytes: 64 << 20,
            max_depth: 16,
            plane_limits: crate::planes::PlaneSetLimits::default(),
        }
    }
}

/// Upgrade a complete IMI v2/v3/v4 file and its known nested images to V4.
///
/// Rewrites framing without decoding or re-encoding pixels: unit record ids,
/// legacy digest declarations, frames/pages, previews, gain maps and plane tiles.
/// Rebuilds encoded-byte checksums, DATA locators and plane directories. Metadata,
/// entropy payloads, pixel checksums and reference-raster digests are preserved.
/// Unknown ancillary chunks/records remain opaque; unknown critical syntax and
/// unknown checksum algorithms that cannot be recomputed are refused. Native
/// JPEG/component files are refused even with the `native-jpeg` feature enabled.
///
/// This buffers one encoded file and bounded nested rewrites, never its raster.
/// Input/output byte limits and nesting limits are explicit. It checks framing
/// and known file checksums, not entropy decoding or pixel-checksum correctness.
/// External byte offsets/digests and archive sequence indexes must be rebuilt.
pub fn upgrade_image(file: &[u8], limits: &UpgradeLimits) -> Result<Vec<u8>, Error> {
    crate::v4::upgrade_image(file, limits)
}

pub(crate) fn append(out: &mut Vec<u8>, bytes: &[u8], limits: &UpgradeLimits) -> Result<(), Error> {
    if out.len().checked_add(bytes.len()).ok_or(Error::TooLarge)? > limits.max_encoded_bytes {
        return Err(Error::TooLarge);
    }
    out.try_reserve(bytes.len()).map_err(|_| Error::TooLarge)?;
    out.extend_from_slice(bytes);
    Ok(())
}
pub(crate) fn chunk(
    out: &mut Vec<u8>,
    tag: &[u8; 4],
    body: &[u8],
    limits: &UpgradeLimits,
) -> Result<(), Error> {
    let mut head = Vec::new();
    head.extend_from_slice(tag);
    crate::put_varint(&mut head, body.len() as u64 + 1);
    append(out, &head, limits)?;
    append(out, body, limits)
}
pub(crate) fn record(
    out: &mut Vec<u8>,
    id: u64,
    bytes: &[u8],
    limits: &UpgradeLimits,
) -> Result<(), Error> {
    let mut head = Vec::new();
    crate::put_varint(&mut head, id);
    crate::put_varint(&mut head, bytes.len() as u64);
    append(out, &head, limits)?;
    append(out, bytes, limits)
}
pub(crate) fn parse(file: &[u8]) -> Result<container::File<'_>, Error> {
    container::parse_checked(
        file,
        false,
        &|h, d, b| crate::v1::payload_len_checked(h, d, b, &Structure),
        &Structure,
        crate::checksum::Verify::Now(1),
    )
}
#[cfg(test)]
pub(crate) fn rewrite(file: &[u8], limits: &UpgradeLimits, depth: usize) -> Result<Vec<u8>, Error> {
    rewrite_inner(file, limits, depth, true)
}
#[cfg(test)]
pub(crate) fn rewrite_primary(
    file: &[u8],
    limits: &UpgradeLimits,
    depth: usize,
) -> Result<Vec<u8>, Error> {
    rewrite_inner(file, limits, depth, false)
}
#[cfg(test)]
fn rewrite_inner(
    file: &[u8],
    limits: &UpgradeLimits,
    depth: usize,
    allow_planes: bool,
) -> Result<Vec<u8>, Error> {
    if file.len() > limits.max_encoded_bytes || depth > limits.max_depth {
        return Err(Error::TooLarge);
    }
    // Reject the retired experiment before inspecting payloads or producing output,
    // including when the optional v2 reader is compiled in.
    let mut prefix = ByteReader::new(file);
    let source = header::read_header_sized(&mut prefix)?;
    if source.header.version == 4 {
        return Err(Error::Unsupported(
            "V4 requires its version-specific upgrader",
        ));
    }
    if source.declared.contains(crate::Feature::NativeComponents) {
        return Err(Error::Unsupported("native components cannot be upgraded"));
    }
    let parsed = parse(file)?;
    let summed = container::ChecksumChunk::rewritable(parsed.checksum)?;
    // The plane route owns absolute offsets and the checksum covering the whole set.
    let mut scan = ByteReader::new(file);
    let header = header::read_header_sized(&mut scan)?;
    while scan.remaining() != 0 {
        let (tag, length) = chunks::read_head(&mut scan)?;
        if chunks::is_plane_chunk(&tag) {
            if !allow_planes {
                return Err(Error::Corrupt("plane primary contains plane chunks"));
            }
            if file.len() < 37 {
                return Err(Error::Corrupt("missing plane locator"));
            }
            return crate::planes::upgrade_image_planes(file, parsed, limits, depth);
        }
        let n = match length {
            Some(n) => usize::try_from(n).map_err(|_| Error::TooLarge)?,
            None => crate::v1::payload_len_checked(
                &header.header,
                header.declared,
                &file[scan.pos..],
                &Structure,
            )?,
        };
        scan.bytes(n)?;
    }
    let mut r = ByteReader::new(file);
    let hp = header::read_header_sized(&mut r)?;
    let mut head = Vec::new();
    append(&mut head, &file[r.pos - hp.len..hp.ext_at], limits)?;
    head[0] = header::HEADER_VERSION;
    records::head_records(&file[hp.ext_at..r.pos], |id, bytes| {
        // Locators are regenerated after nested chunk lengths have settled.
        if id == header::EXT_LOCATOR {
            return Ok(());
        }
        if id == crate::registry::HEADER_CHECKSUM && bytes.len() == 1 {
            record(
                &mut head,
                id,
                &[bytes[0], HashAlgorithm::Xxh3_64.wire()],
                limits,
            )?;
        } else {
            record(&mut head, id, bytes, limits)?;
        }
        Ok(())
    })?;
    let mut out = Vec::new();
    append(&mut out, &header::SIGNATURE, limits)?;
    crate::put_varint(&mut out, head.len() as u64);
    append(&mut out, &head, limits)?;
    while r.remaining() != 0 {
        let start = r.pos;
        let (tag, length) = chunks::read_head(&mut r)?;
        let body = match length {
            Some(n) => chunks::payload(&mut r, n)?,
            None => {
                let n = crate::v1::payload_len_checked(
                    &hp.header,
                    hp.declared,
                    &file[r.pos..],
                    &Structure,
                )?;
                r.bytes(n)?
            }
        };
        match &tag {
            b"cSUM" => {}
            b"DATA" => {
                let data = data(body, &hp, limits)?;
                // Keep open/closed framing; unit table offsets are DATA-relative
                // and mapped ids all retain their one-byte width.
                append(&mut out, &file[start..r.pos - body.len()], limits)?;
                append(&mut out, &data, limits)?;
            }
            b"pRVW" => chunk(&mut out, &tag, &rewrite(body, limits, depth + 1)?, limits)?,
            b"fRAM" => chunk(
                &mut out,
                &tag,
                &frame(body, hp.header.version, limits, depth)?,
                limits,
            )?,
            b"gMAP" => chunk(&mut out, &tag, &gain_map(body, limits, depth)?, limits)?,
            b"pDAT" | b"pIDX" | b"sLOC" => {
                return Err(Error::Corrupt("plane chunks without final locator"));
            }
            _ => append(&mut out, &file[start..r.pos], limits)?,
        }
    }
    let out = container::locate(out)?;
    let out = if summed {
        crate::checksum::refinish(out)?
    } else {
        out
    };
    if out.len() > limits.max_encoded_bytes {
        return Err(Error::TooLarge);
    }
    parse(&out)?;
    Ok(out)
}
#[cfg(test)]
fn data(body: &[u8], hp: &header::ParsedHeader, limits: &UpgradeLimits) -> Result<Vec<u8>, Error> {
    let mut r = ByteReader::new(body);
    let mut head = crate::parse::payload::read_head(&hp.header, hp.declared, &mut r, &Structure)?;
    let start = r.pos;
    let (map, count) = head.units();
    map.spans_with_ordinals(
        &mut r,
        hp.header.version,
        count,
        head.take_hint(),
        head.unit_ordinals(),
    )?;
    let mut out = Vec::new();
    append(&mut out, body, limits)?;
    if hp.header.version == header::HEADER_VERSION_2 {
        let mut r = ByteReader::new(&body[start..]);
        while r.remaining() != 0 {
            let at = start + r.pos;
            let id = r.varint()?;
            let n = r.var_usize()?;
            r.bytes(n)?;
            let mapped = crate::registry::unit_id(hp.header.version, id);
            if id >= 128 || mapped >= 128 {
                return Err(Error::Unsupported("unmapped unit record"));
            }
            out[at] = mapped as u8;
        }
    }
    Ok(out)
}
#[cfg(test)]
fn frame(body: &[u8], version: u8, limits: &UpgradeLimits, depth: usize) -> Result<Vec<u8>, Error> {
    frame_with(body, version, limits, depth, &rewrite)
}

pub(crate) fn frame_with(
    body: &[u8],
    version: u8,
    limits: &UpgradeLimits,
    depth: usize,
    convert: &dyn Fn(&[u8], &UpgradeLimits, usize) -> Result<Vec<u8>, Error>,
) -> Result<Vec<u8>, Error> {
    let mut r = ByteReader::new(body);
    r.var_u32()?;
    r.var_u32()?;
    r.varint()?;
    r.var_u32()?;
    let flags = r.varint()?;
    if flags & !7 != 0 {
        return Err(Error::Unsupported("unknown frame field"));
    }
    let mut out = Vec::new();
    append(&mut out, &body[..r.pos], limits)?;
    if flags & 4 != 0 {
        let n = r.var_usize()?;
        let mut items = Vec::new();
        records::head_records(r.bytes(n)?, |id, payload| {
            let mapped = crate::registry::frame_id(version, id);
            let (id, payload) = if mapped == crate::registry::FRAME_REFERENCE {
                let mut rr = ByteReader::new(payload);
                let index = rr.varint()?;
                let mut bytes = Vec::new();
                crate::put_varint(&mut bytes, index);
                if version == header::HEADER_VERSION_2 && rr.remaining() == 8 {
                    bytes.push(HashAlgorithm::Xxh3_64.wire());
                } else {
                    let algo = HashAlgorithm::from_wire(rr.u8()?)
                        .ok_or(Error::Unsupported("reference digest algorithm"))?;
                    if rr.remaining() != algo.width() {
                        return Err(Error::Corrupt("reference digest length"));
                    }
                    bytes.push(algo.wire());
                }
                append(&mut bytes, rr.bytes(rr.remaining())?, limits)?;
                (mapped, bytes)
            } else if id & 1 != 0 {
                return Err(Error::Unsupported("unknown frame record"));
            } else {
                let mut bytes = Vec::new();
                append(&mut bytes, payload, limits)?;
                (id, bytes)
            };
            items.try_reserve(1).map_err(|_| Error::TooLarge)?;
            items.push((id, payload));
            Ok(())
        })?;
        items.sort_by_key(|item| item.0);
        let mut records = Vec::new();
        for (id, bytes) in items {
            record(&mut records, id, &bytes, limits)?;
        }
        crate::put_varint(&mut out, records.len() as u64);
        append(&mut out, &records, limits)?;
    }
    if flags & 2 == 0 {
        append(
            &mut out,
            &convert(&body[r.pos..], limits, depth + 1)?,
            limits,
        )?;
    } else if flags & 1 == 0 || r.remaining() != 0 {
        return Err(Error::Corrupt("empty frame payload"));
    }
    Ok(out)
}
#[cfg(test)]
fn gain_map(body: &[u8], limits: &UpgradeLimits, depth: usize) -> Result<Vec<u8>, Error> {
    gain_map_with(body, limits, depth, &rewrite)
}

pub(crate) fn gain_map_with(
    body: &[u8],
    limits: &UpgradeLimits,
    depth: usize,
    convert: &dyn Fn(&[u8], &UpgradeLimits, usize) -> Result<Vec<u8>, Error>,
) -> Result<Vec<u8>, Error> {
    let mut r = ByteReader::new(body);
    if r.u8()? != 0 {
        return Err(Error::Unsupported("gain map version during upgrade"));
    }
    let n = r.var_usize()?;
    r.bytes(n)?;
    let flags = r.varint()?;
    if flags & !3 != 0 {
        return Err(Error::Unsupported("gain map fields during upgrade"));
    }
    if flags & 1 != 0 {
        r.bytes(4)?;
    }
    if flags & 2 != 0 {
        let n = r.var_usize()?;
        r.bytes(n)?;
    }
    let mut out = Vec::new();
    append(&mut out, &body[..r.pos], limits)?;
    append(
        &mut out,
        &convert(&body[r.pos..], limits, depth + 1)?,
        limits,
    )?;
    Ok(out)
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use alloc::vec;
    #[test]
    fn legacy_reference_digest_and_unknown_frame_records_survive_renumbering() {
        let child = include_bytes!("../../fuzz/seeds/31-seed-v2-unit-hint.imi");
        let digest = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let mut reference = vec![0];
        reference.extend_from_slice(&digest);
        let mut list = Vec::new();
        records::put_record(&mut list, 33, &reference);
        records::put_record(&mut list, 100, b"unknown optional data");
        let mut old = vec![0, 0, 1, 1, 4];
        crate::put_varint(&mut old, list.len() as u64);
        old.extend_from_slice(&list);
        old.extend_from_slice(child);
        let new = frame(&old, 2, &UpgradeLimits::default(), 0).unwrap();
        let mut r = ByteReader::new(&new);
        r.bytes(5).unwrap();
        let length = r.var_usize().unwrap();
        let mut ids = Vec::new();
        records::head_records(r.bytes(length).unwrap(), |id, bytes| {
            ids.push(id);
            if id == 100 {
                assert_eq!(bytes, b"unknown optional data");
            }
            if id == 111 {
                assert_eq!(bytes, &[&[0, 0][..], &digest].concat());
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(ids, [100, 111]);
        assert_eq!(crate::header(&new[r.pos..]).unwrap().version, 3);
        assert_eq!(
            crate::decode(&new[r.pos..]).unwrap(),
            crate::decode(child).unwrap()
        );
    }
}
