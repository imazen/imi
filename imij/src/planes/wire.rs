//! The plane directory (docs/PLANES_AND_ROI.md): a record list whose records keep the
//! fixed little-endian layouts of directory wire version 2 (descriptor
//! precision binds the native tile components). Earlier directories, which
//! began with `PSET` and a version, are not read. The locator framing is
//! version 1 and unchanged.
use super::*;
use crate::parse::records::{head_records, put_record};
use alloc::string::String;

pub(super) const DATA: [u8; 4] = *b"pDAT";
pub(super) const INDEX: [u8; 4] = *b"pIDX";
pub(super) const LOCATOR: [u8; 4] = *b"sLOC";
pub(super) const FOOTER_LEN: u64 = 37;
/// The locator's magic and version (the directory has neither).
const MAGIC: [u8; 4] = *b"PSET";
const VERSION: u32 = 1;
/// Directory records, all critical: the primary file's length (varint),
/// the tile records, the planes with their levels, and the attachments
/// (absent when there are none). Even ids are ignorable extensions.
const D_PRIMARY: u64 = crate::registry::DIRECTORY_PRIMARY;
const D_TILES: u64 = crate::registry::DIRECTORY_TILES;
const D_PLANES: u64 = crate::registry::DIRECTORY_PLANES;
const D_ATTACHMENTS: u64 = crate::registry::DIRECTORY_ATTACHMENTS;

pub(super) fn chunk_len(payload: u64) -> Result<u64, Error> {
    let n = payload.checked_add(1).ok_or(Error::TooLarge)?;
    payload
        .checked_add(4 + crate::varint_len(n) as u64)
        .ok_or(Error::TooLarge)
}
pub(super) fn chunk(out: &mut Vec<u8>, tag: &[u8; 4], payload: &[u8]) -> Result<(), Error> {
    append(out, tag)?;
    let mut n = (payload.len() as u64)
        .checked_add(1)
        .ok_or(Error::TooLarge)?;
    loop {
        let byte = (n & 127) as u8 | if n >= 128 { 128 } else { 0 };
        append(out, &[byte])?;
        n >>= 7;
        if n == 0 {
            break;
        }
    }
    append(out, payload)
}

pub(super) fn footer(offset: u64, index_len: u64, file_len: u64) -> [u8; 37] {
    let mut out = [0u8; 37];
    out[..4].copy_from_slice(&LOCATOR);
    out[4] = 33; // A canonical one-byte chunk length: payload32 + 1.
    out[5..9].copy_from_slice(&MAGIC);
    out[9..13].copy_from_slice(&VERSION.to_le_bytes());
    out[13..21].copy_from_slice(&offset.to_le_bytes());
    out[21..29].copy_from_slice(&index_len.to_le_bytes());
    out[29..37].copy_from_slice(&file_len.to_le_bytes());
    out
}
/// The index offset and length; the index ends where the locator starts.
pub(super) fn locator(bytes: &[u8; 37], file_len: u64) -> Result<(u64, u64), Error> {
    if bytes[..4] != LOCATOR
        || bytes[4] != 33
        || bytes[5..9] != MAGIC
        || u32::from_le_bytes(bytes[9..13].try_into().unwrap()) != VERSION
    {
        return Err(Error::Corrupt("plane locator"));
    }
    let offset = u64::from_le_bytes(bytes[13..21].try_into().unwrap());
    let len = u64::from_le_bytes(bytes[21..29].try_into().unwrap());
    let size = u64::from_le_bytes(bytes[29..37].try_into().unwrap());
    if size != file_len
        || len < 5
        || offset < 8
        || offset.checked_add(len) != file_len.checked_sub(FOOTER_LEN)
    {
        return Err(Error::Corrupt("plane index locator bounds"));
    }
    Ok((offset, len))
}

pub(super) fn payload<'a>(bytes: &'a [u8], tag: &[u8; 4]) -> Result<&'a [u8], Error> {
    if bytes.get(..4) != Some(&tag[..]) {
        return Err(Error::Corrupt("plane record tag"));
    }
    let mut pos = 4;
    let mut n = 0u64;
    for i in 0..10 {
        let b = *bytes
            .get(pos)
            .ok_or(Error::Corrupt("short plane chunk length"))?;
        pos += 1;
        if i == 9 && b > 1 {
            return Err(Error::Corrupt("plane chunk length overflow"));
        }
        n |= u64::from(b & 127) << (i * 7);
        if b < 128 {
            if (i != 0 && b == 0) || n == 0 {
                return Err(Error::Corrupt("noncanonical plane chunk length"));
            }
            if n - 1 != (bytes.len() - pos) as u64 {
                return Err(Error::Corrupt("plane chunk size"));
            }
            return Ok(&bytes[pos..]);
        }
    }
    Err(Error::Corrupt("plane chunk length overflow"))
}

struct Writer {
    bytes: Vec<u8>,
    max: usize,
}
impl Writer {
    fn new(limits: PlaneSetLimits) -> Writer {
        Writer {
            bytes: Vec::new(),
            max: limits.max_index_bytes,
        }
    }
    /// Appends `body` as the directory record `id`.
    fn record(&mut self, id: u64, body: Writer) -> Result<(), Error> {
        let mut framed = Vec::new();
        framed
            .try_reserve_exact(body.bytes.len() + 20)
            .map_err(|_| Error::TooLarge)?;
        put_record(&mut framed, id, &body.bytes);
        self.put(&framed)
    }
    fn put(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|n| n > self.max)
        {
            return Err(Error::TooLarge);
        }
        append(&mut self.bytes, bytes)
    }
    fn byte(&mut self, n: u8) -> Result<(), Error> {
        self.put(&[n])
    }
    fn u16(&mut self, n: u16) -> Result<(), Error> {
        self.put(&n.to_le_bytes())
    }
    fn u32(&mut self, n: u32) -> Result<(), Error> {
        self.put(&n.to_le_bytes())
    }
    fn count(&mut self, n: usize) -> Result<(), Error> {
        self.u32(u32::try_from(n).map_err(|_| Error::TooLarge)?)
    }
    fn u64(&mut self, n: u64) -> Result<(), Error> {
        self.put(&n.to_le_bytes())
    }
    fn i64(&mut self, n: i64) -> Result<(), Error> {
        self.put(&n.to_le_bytes())
    }
    fn text(&mut self, s: &str) -> Result<(), Error> {
        self.u16(u16::try_from(s.len()).map_err(|_| Error::TooLarge)?)?;
        self.put(s.as_bytes())
    }
}
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    /// A directory record's payload, which its fields must fill exactly.
    fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, pos: 0 }
    }
    fn finish(&self) -> Result<(), Error> {
        if self.pos != self.bytes.len() {
            return Err(Error::Corrupt("plane index trailing bytes"));
        }
        Ok(())
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.pos.checked_add(n).ok_or(Error::TooLarge)?;
        let value = self
            .bytes
            .get(self.pos..end)
            .ok_or(Error::Corrupt("short plane index"))?;
        self.pos = end;
        Ok(value)
    }
    fn byte(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn i64(&mut self) -> Result<i64, Error> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn count(&mut self, max: usize, min_bytes: usize) -> Result<usize, Error> {
        let n = self.u32()? as usize;
        if n > max {
            return Err(Error::TooLarge);
        }
        if n.checked_mul(min_bytes)
            .is_none_or(|v| v > self.bytes.len() - self.pos)
        {
            return Err(Error::Corrupt("plane count exceeds index"));
        }
        Ok(n)
    }
    fn text(&mut self, limits: PlaneSetLimits) -> Result<String, Error> {
        let n = self.u16()? as usize;
        if n > limits.max_label_bytes {
            return Err(Error::TooLarge);
        }
        let s =
            core::str::from_utf8(self.take(n)?).map_err(|_| Error::Corrupt("plane label UTF8"))?;
        label(s, limits).map_err(|_| Error::Corrupt("plane label"))?;
        let mut out = String::new();
        out.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
        out.push_str(s);
        Ok(out)
    }
}

fn role_byte(role: &PlaneRole) -> u8 {
    match role {
        PlaneRole::Auxiliary => 0,
        PlaneRole::Luma => 1,
        PlaneRole::Red => 2,
        PlaneRole::Green => 3,
        PlaneRole::Blue => 4,
        PlaneRole::Alpha => 5,
        PlaneRole::Depth => 6,
        PlaneRole::Disparity => 7,
        PlaneRole::Confidence => 8,
        PlaneRole::PortraitMatte => 9,
        PlaneRole::Segmentation => 10,
        PlaneRole::Raw => 11,
        PlaneRole::ComplexReal => 12,
        PlaneRole::ComplexImaginary => 13,
        PlaneRole::Scalar => 14,
        PlaneRole::Custom(_) => 255,
    }
}
fn read_role(r: &mut Reader<'_>, limits: PlaneSetLimits) -> Result<PlaneRole, Error> {
    Ok(match r.byte()? {
        0 => PlaneRole::Auxiliary,
        1 => PlaneRole::Luma,
        2 => PlaneRole::Red,
        3 => PlaneRole::Green,
        4 => PlaneRole::Blue,
        5 => PlaneRole::Alpha,
        6 => PlaneRole::Depth,
        7 => PlaneRole::Disparity,
        8 => PlaneRole::Confidence,
        9 => PlaneRole::PortraitMatte,
        10 => PlaneRole::Segmentation,
        11 => PlaneRole::Raw,
        12 => PlaneRole::ComplexReal,
        13 => PlaneRole::ComplexImaginary,
        14 => PlaneRole::Scalar,
        255 => PlaneRole::Custom(r.text(limits)?),
        // Roles are a registry a later version may extend.
        _ => return Err(Error::Unsupported("unknown plane role")),
    })
}

pub(super) fn encode(
    dir: &Directory,
    limits: PlaneSetLimits,
    version: u8,
) -> Result<Vec<u8>, Error> {
    encode_with_pages(dir, limits, version, dir.pages.as_ref())
}

pub(super) fn encode_with_pages(
    dir: &Directory,
    limits: PlaneSetLimits,
    version: u8,
    pages: Option<&record_pages::RecordPages>,
) -> Result<Vec<u8>, Error> {
    crate::parse::header::check_header_version(version)?;
    let wire_id = |id| {
        crate::registry::directory_wire_id(version, id)
            .ok_or(Error::Unsupported("unknown plane directory record"))
    };
    let mut out = Writer::new(limits);
    let mut w = Writer::new(limits);
    let mut primary = Vec::new();
    crate::put_varint(&mut primary, dir.primary_len);
    w.put(&primary)?;
    out.record(wire_id(D_PRIMARY)?, w)?;
    if pages.is_none() {
        let mut w = Writer::new(limits);
        w.count(dir.records.len())?;
        for rec in &dir.records {
            w.u64(rec.offset)?;
            w.u64(rec.len)?;
        }
        out.record(wire_id(D_TILES)?, w)?;
    }
    let mut w = Writer::new(limits);
    w.count(dir.planes.len())?;
    for p in &dir.planes {
        w.text(&p.name)?;
        w.byte(p.sample as u8)?;
        w.byte(p.precision)?;
        w.byte(role_byte(&p.role))?;
        if let PlaneRole::Custom(name) = &p.role {
            w.text(name)?;
        }
        w.i64(p.origin.0)?;
        w.i64(p.origin.1)?;
        w.u32(p.group)?;
        w.u32(p.calibration.map_or(u32::MAX, |v| v.0))?;
        w.byte(p.units.is_some() as u8)?;
        if let Some(units) = &p.units {
            w.text(units)?;
        }
        for n in [
            p.cicp.color_primaries,
            p.cicp.transfer_characteristics,
            p.cicp.matrix_coefficients,
            p.cicp.full_range as u8,
        ] {
            w.byte(n)?;
        }
        w.u32(p.icc_profile.map_or(u32::MAX, |id| id.0))?;
        w.count(p.levels.len())?;
        for l in &p.levels {
            for n in [l.width, l.height, l.tile_width, l.tile_height] {
                w.u32(n)?;
            }
            w.byte(l.coding as u8)?;
            w.byte(match l.source {
                LevelSource::SuppliedReference => 0,
                LevelSource::DirectFromOriginal => 1,
            })?;
            w.byte(l.channels as u8)?;
            w.byte(l.component)?;
            w.u32(l.record_start)?;
        }
    }
    out.record(wire_id(D_PLANES)?, w)?;
    if !dir.attachments.is_empty() {
        let mut w = Writer::new(limits);
        w.count(dir.attachments.len())?;
        for a in &dir.attachments {
            w.text(&a.name)?;
            w.text(&a.mime)?;
            w.u32(a.group)?;
            w.u32(a.record)?;
            w.byte(a.timing.is_some() as u8)?;
            if let Some(t) = a.timing {
                w.i64(t.start_num)?;
                w.u32(t.start_den)?;
                w.u64(t.duration_num)?;
                w.u32(t.duration_den)?;
            }
        }
        out.record(wire_id(D_ATTACHMENTS)?, w)?;
    }
    if let Some(pages) = pages {
        if version != 4 {
            return Err(Error::Unsupported("paged plane directory requires V4"));
        }
        let mut w = Writer::new(limits);
        w.put(&pages.encode(limits)?)?;
        out.record(crate::registry::DIRECTORY_RECORD_PAGES, w)?;
    }
    if let Some(images) = &dir.images {
        if version != 4 {
            return Err(Error::Unsupported("native image directory requires V4"));
        }
        let mut w = Writer::new(limits);
        w.put(&images.encode(limits)?)?;
        out.record(crate::registry::DIRECTORY_IMAGES, w)?;
    }
    Ok(out.bytes)
}

/// `version` is the primary's header version: directory entry ids are
/// numbered for it (SPEC Appendix V3).
pub(super) fn decode(
    bytes: &[u8],
    limits: PlaneSetLimits,
    index_offset: u64,
    version: u8,
) -> Result<Directory, Error> {
    if bytes.len() > limits.max_index_bytes {
        return Err(Error::TooLarge);
    }
    let (mut primary, mut tiles, mut plane_list, mut attached) = (None, None, None, None);
    let mut images = None;
    let mut paged = None;
    head_records(bytes, |id, rec| {
        match crate::registry::directory_id(version, id) {
            D_PRIMARY => primary = Some(rec),
            D_TILES => tiles = Some(rec),
            D_PLANES => plane_list = Some(rec),
            D_ATTACHMENTS => attached = Some(rec),
            crate::registry::DIRECTORY_RECORD_PAGES if version == 4 => paged = Some(rec),
            crate::registry::DIRECTORY_IMAGES if version == 4 => {
                images = Some(ImageDirectory::read(rec, limits)?)
            }
            _ if id & 1 == 1 => {
                return Err(Error::Unsupported("unknown plane directory record"));
            }
            _ => {}
        }
        Ok(())
    })?;
    let (Some(primary), Some(plane_list)) = (primary, plane_list) else {
        return Err(Error::Corrupt("plane directory record missing"));
    };
    let mut p = crate::ByteReader::new(primary);
    let primary_len = p.varint()?;
    if p.remaining() != 0 {
        return Err(Error::Corrupt("plane index trailing bytes"));
    }
    let (pages, records) = match (tiles, paged) {
        (Some(tiles), None) => {
            let mut r = Reader::new(tiles);
            let n = r.count(limits.max_records, 16)?;
            let mut records = Vec::new();
            records.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
            for _ in 0..n {
                records.push(Record {
                    offset: r.u64()?,
                    len: r.u64()?,
                });
            }
            r.finish()?;
            (None, records)
        }
        (None, Some(root)) => (
            Some(record_pages::RecordPages::read(
                root,
                primary_len,
                index_offset,
                limits,
            )?),
            Vec::new(),
        ),
        (None, None) => return Err(Error::Corrupt("plane directory record missing")),
        (Some(_), Some(_)) => {
            return Err(Error::Corrupt(
                "plane directory requires one record authority",
            ));
        }
    };
    let mut r = Reader::new(plane_list);
    let n = r.count(limits.max_planes, 32)?;
    let mut planes = Vec::new();
    planes.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
    let mut total_levels = 0usize;
    for _ in 0..n {
        let name = r.text(limits)?;
        let sample = SampleType::from_u8(r.byte()?)
            .ok_or(Error::Unsupported("unknown plane sample type"))?;
        let precision = r.byte()?;
        let role = read_role(&mut r, limits)?;
        if role == PlaneRole::Scalar && version != 4 {
            return Err(Error::Unsupported("scalar planes require V4"));
        }
        let origin = (r.i64()?, r.i64()?);
        let group = r.u32()?;
        let calibration = r.u32()?;
        let units = match r.byte()? {
            0 => None,
            1 => Some(r.text(limits)?),
            _ => return Err(Error::Corrupt("plane units flag")),
        };
        let cicp = Cicp {
            color_primaries: r.byte()?,
            transfer_characteristics: r.byte()?,
            matrix_coefficients: r.byte()?,
            full_range: match r.byte()? {
                0 => false,
                1 => true,
                _ => return Err(Error::Corrupt("plane CICP range")),
            },
        };
        let icc = r.u32()?;
        let icc_profile = if icc == u32::MAX {
            None
        } else {
            Some(AttachmentId(icc))
        };
        let count = r.count(limits.max_levels, 24)?;
        total_levels = total_levels.checked_add(count).ok_or(Error::TooLarge)?;
        if total_levels > limits.max_levels {
            return Err(Error::TooLarge);
        }
        let mut levels = Vec::new();
        levels
            .try_reserve_exact(count)
            .map_err(|_| Error::TooLarge)?;
        for _ in 0..count {
            let (width, height, tile_width, tile_height) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?);
            let coding = match r.byte()? {
                0 => Coding::Lossless,
                1 => Coding::Lossy,
                _ => return Err(Error::Unsupported("unknown plane level coding")),
            };
            let source = match r.byte()? {
                0 => LevelSource::SuppliedReference,
                1 => LevelSource::DirectFromOriginal,
                _ => return Err(Error::Corrupt("plane level source")),
            };
            let channels =
                Channels::from_u8(r.byte()?).ok_or(Error::Corrupt("plane tile channels"))?;
            let component = r.byte()?;
            let record_start = r.u32()?;
            levels.push(LevelDescriptor {
                width,
                height,
                tile_width,
                tile_height,
                coding,
                source,
                channels,
                component,
                record_start,
            });
        }
        planes.push(PlaneDescriptor {
            name,
            sample,
            precision,
            role,
            origin,
            group,
            calibration: if calibration == u32::MAX {
                None
            } else {
                Some(AttachmentId(calibration))
            },
            units,
            cicp,
            icc_profile,
            levels,
        });
    }
    r.finish()?;
    let mut r = Reader::new(attached.unwrap_or(&[0; 4]));
    let n = r.count(limits.max_attachments, 13)?;
    let mut attachments = Vec::new();
    attachments
        .try_reserve_exact(n)
        .map_err(|_| Error::TooLarge)?;
    for _ in 0..n {
        let (name, mime, group, record) = (r.text(limits)?, r.text(limits)?, r.u32()?, r.u32()?);
        let timing = match r.byte()? {
            0 => None,
            1 => Some(
                AttachmentTiming::new(r.i64()?, r.u32()?, r.u64()?, r.u32()?)
                    .map_err(|_| Error::Corrupt("plane attachment timing"))?,
            ),
            _ => return Err(Error::Corrupt("plane attachment timing flag")),
        };
        attachments.push(AttachmentDescriptor {
            name,
            mime,
            group,
            timing,
            record,
        });
    }
    r.finish()?;
    let dir = Directory {
        images,
        pages,
        primary_len,
        records,
        planes,
        attachments,
    };
    validate(&dir, limits, index_offset)?;
    Ok(dir)
}

pub(super) fn validate(
    dir: &Directory,
    limits: PlaneSetLimits,
    index_offset: u64,
) -> Result<(), Error> {
    if dir.primary_len < 8
        || dir.primary_len > index_offset
        || dir.planes.len() > limits.max_planes
        || dir.record_count() > limits.max_records
        || dir.attachments.len() > limits.max_attachments
    {
        return Err(Error::Corrupt("plane directory bounds"));
    }
    if let Some(pages) = &dir.pages {
        if !dir.records.is_empty() {
            return Err(Error::Corrupt("duplicate plane record authorities"));
        }
        pages.validate_attachments(&dir.attachments)?;
    } else {
        let mut next = dir.primary_len;
        for rec in &dir.records {
            if rec.offset != next {
                return Err(Error::Corrupt(
                    "noncanonical or overlapping plane record offsets",
                ));
            }
            next = rec
                .offset
                .checked_add(chunk_len(rec.len)?)
                .ok_or(Error::TooLarge)?;
            if next > index_offset {
                return Err(Error::Corrupt("plane record reaches index"));
            }
        }
        if next != index_offset {
            return Err(Error::Corrupt("unindexed plane record bytes"));
        }
    }
    validate_metadata(
        dir,
        limits,
        dir.record_count(),
        if dir.pages.is_none() {
            Some(&dir.records)
        } else {
            None
        },
        |id| {
            if let Some(root) = &dir.pages {
                root.attachment_len(id)
            } else {
                dir.records.get(id as usize).map(|r| r.len)
            }
        },
    )
}

pub(super) fn validate_metadata(
    dir: &Directory,
    limits: PlaneSetLimits,
    record_count: usize,
    flat_records: Option<&[Record]>,
    attachment_length: impl Fn(u32) -> Option<u64>,
) -> Result<(), Error> {
    if record_count > limits.max_records
        || dir.planes.len() > limits.max_planes
        || dir.attachments.len() > limits.max_attachments
    {
        return Err(Error::TooLarge);
    }
    if let Some(images) = &dir.images {
        images.validate(dir, limits)?;
    }
    // Each tile is owned by exactly one scalar level or a complete, distinct
    // set of components of one packed color level. Attachments never alias.
    let mut spans = Vec::new();
    let mut names = Vec::new();
    names
        .try_reserve_exact(dir.planes.len())
        .map_err(|_| Error::TooLarge)?;
    for p in &dir.planes {
        names.push((p.group, p.name.as_str()));
    }
    names.sort_unstable();
    if names.windows(2).any(|v| v[0] == v[1]) {
        return Err(Error::Corrupt("duplicate plane name in group"));
    }
    check_complex_pairs(&dir.planes).map_err(Error::Corrupt)?;
    let mut levels = 0usize;
    let mut backing_planes = Vec::new();
    for (i, p) in dir.planes.iter().enumerate() {
        validate_plane(p, limits).map_err(|_| Error::Corrupt("plane descriptor"))?;
        if p.calibration
            .is_some_and(|v| v.0 as usize >= dir.attachments.len())
        {
            return Err(Error::Corrupt("plane calibration attachment ID"));
        }
        if p.icc_profile
            .is_some_and(|id| id.0 as usize >= dir.attachments.len())
        {
            return Err(Error::Corrupt("plane ICC attachment ID"));
        }
        if p.levels.is_empty() {
            return Err(Error::Corrupt("plane without levels"));
        }
        levels = levels.checked_add(p.levels.len()).ok_or(Error::TooLarge)?;
        if levels > limits.max_levels {
            return Err(Error::TooLarge);
        }
        for (li, l) in p.levels.iter().enumerate() {
            if l.channels.count() > 1 {
                backing_planes.try_reserve(1).map_err(|_| Error::TooLarge)?;
                backing_planes.push((l.record_start, i));
            }
            let (_, _, n) = l
                .tiles()
                .map_err(|_| Error::Corrupt("plane level geometry"))?;
            if (l.record_start as usize)
                .checked_add(n)
                .is_none_or(|end| end > record_count)
                || (li == 0
                    && (p.origin.0.checked_add(i64::from(l.width)).is_none()
                        || p.origin.1.checked_add(i64::from(l.height)).is_none()))
            {
                return Err(Error::Corrupt("plane tile count or origin"));
            }
            if u64::from(l.width) * u64::from(l.height) > limits.max_plane_pixels {
                return Err(Error::TooLarge);
            }
            if l.component as usize >= l.channels.count() {
                return Err(Error::Corrupt("plane component index"));
            }
            if (p.role == PlaneRole::Scalar) != (l.channels == Channels::Scalar)
                || (l.channels == Channels::Scalar && l.coding != Coding::Lossless)
            {
                return Err(Error::Corrupt("scalar plane level"));
            }
            let tile = sample_bytes(
                l.width.min(l.tile_width),
                l.height.min(l.tile_height),
                p.sample,
            )?
            .checked_mul(l.channels.count())
            .ok_or(Error::TooLarge)?;
            if tile as u64 > limits.max_tile_bytes {
                return Err(Error::TooLarge);
            }
            if p.role == PlaneRole::Raw && l.coding != Coding::Lossless {
                return Err(Error::Corrupt("lossy RAW plane"));
            }
            if l.coding == Coding::Lossy
                && (!p.sample.lossy_capable()
                    || (matches!(p.sample, SampleType::F16 | SampleType::F32)
                        && p.cicp.transfer_characteristics != 8))
            {
                return Err(Error::Corrupt("unsupported lossy plane type or transfer"));
            }
            spans.try_reserve(1).map_err(|_| Error::TooLarge)?;
            let end = l.record_start as usize + n;
            if flat_records.is_some_and(|records| {
                records[l.record_start as usize..end]
                    .iter()
                    .any(|r| r.len > limits.max_record_bytes)
            }) {
                return Err(Error::TooLarge);
            }
            spans.push(ownership::OwnerSpan {
                start: l.record_start as usize,
                end,
                plane: Some((i, li)),
            });
        }
    }
    backing_planes.sort_unstable();
    if backing_planes.windows(2).any(|v| v[0] == v[1]) {
        return Err(Error::Corrupt(
            "shared color components require distinct planes",
        ));
    }
    names.clear();
    names
        .try_reserve(dir.attachments.len())
        .map_err(|_| Error::TooLarge)?;
    for a in &dir.attachments {
        names.push((a.group, a.name.as_str()));
    }
    names.sort_unstable();
    if names.windows(2).any(|v| v[0] == v[1]) {
        return Err(Error::Corrupt("duplicate attachment name in group"));
    }
    for a in &dir.attachments {
        label(&a.name, limits).map_err(|_| Error::Corrupt("attachment name"))?;
        label(&a.mime, limits).map_err(|_| Error::Corrupt("attachment MIME"))?;
        if !a.mime.is_ascii() || a.mime.bytes().any(|v| v < 32 || v == 127) || !a.mime.contains('/')
        {
            return Err(Error::Corrupt("attachment MIME"));
        }
        if a.record as usize >= record_count {
            return Err(Error::Corrupt("attachment record ID"));
        }
        if attachment_length(a.record).is_some_and(|len| len > limits.max_attachment_bytes) {
            return Err(Error::TooLarge);
        }
        spans.try_reserve(1).map_err(|_| Error::TooLarge)?;
        spans.push(ownership::OwnerSpan {
            start: a.record as usize,
            end: a.record as usize + 1,
            plane: None,
        });
    }
    ownership::validate(dir, spans, record_count)?;

    Ok(())
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    fn directory(width: u32, height: u32, tw: u32, th: u32) -> (Directory, u64) {
        let mut l = LevelDescriptor::new(width, height, tw, th);
        let count = l.tiles().unwrap().2;
        let mut records = Vec::new();
        let mut offset = 8;
        for _ in 0..count {
            records.push(Record { offset, len: 1 });
            offset += chunk_len(1).unwrap();
        }
        l.record_start = 0;
        let mut p = PlaneDescriptor::new("scalar", SampleType::U8);
        p.levels.push(l);
        (
            Directory {
                images: None,
                pages: None,
                primary_len: 8,
                records,
                planes: alloc::vec![p],
                attachments: Vec::new(),
            },
            offset,
        )
    }
    #[test]
    fn directory_ids_follow_enclosing_header_version() {
        let (dir, end) = directory(8, 8, 4, 4);
        let limits = PlaneSetLimits::default();
        let current = encode(&dir, limits, crate::parse::header::HEADER_VERSION).unwrap();
        let mut legacy = Vec::new();
        head_records(&current, |id, payload| {
            let old_id = match id {
                D_PRIMARY => 1,
                D_TILES => 3,
                D_PLANES => 5,
                D_ATTACHMENTS => 7,
                _ => unreachable!(),
            };
            put_record(&mut legacy, old_id, payload);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            encode(&dir, limits, crate::parse::header::HEADER_VERSION_2).unwrap(),
            legacy
        );
        // The optional attachments record also had an id in version 2.
        put_record(&mut legacy, 7, &[0; 4]);

        let decoded_current =
            decode(&current, limits, end, crate::parse::header::HEADER_VERSION).unwrap();
        let decoded_legacy =
            decode(&legacy, limits, end, crate::parse::header::HEADER_VERSION_2).unwrap();
        assert_eq!(decoded_legacy.primary_len, decoded_current.primary_len);
        assert_eq!(decoded_legacy.planes, decoded_current.planes);
        assert_eq!(decoded_legacy.attachments, decoded_current.attachments);
        assert_eq!(decoded_legacy.records.len(), decoded_current.records.len());
        for (old, new) in decoded_legacy.records.iter().zip(&decoded_current.records) {
            assert_eq!((old.offset, old.len), (new.offset, new.len));
        }
        assert!(matches!(
            decode(&legacy, limits, end, crate::parse::header::HEADER_VERSION),
            Err(Error::Unsupported("unknown plane directory record"))
        ));
        assert!(matches!(
            decode(
                &current,
                limits,
                end,
                crate::parse::header::HEADER_VERSION_2
            ),
            Err(Error::Unsupported("unknown plane directory record"))
        ));
    }
    #[test]
    fn reject_overlap_gap_aliases_and_trailing_index_bytes() {
        let (mut d, end) = directory(8, 8, 4, 4);
        let limits = PlaneSetLimits::default();
        validate(&d, limits, end).unwrap();
        let saved = d.records[1].offset;
        for wrong in [saved - 1, saved + 1, u64::MAX] {
            d.records[1].offset = wrong;
            assert!(validate(&d, limits, end).is_err());
        }
        d.records[1].offset = saved;
        let mut duplicate = d.planes[0].clone();
        duplicate.name = "alias".into();
        d.planes.push(duplicate);
        assert!(validate(&d, limits, end).is_err());
        d.planes.pop();
        let mut encoded = encode(&d, limits, crate::parse::header::HEADER_VERSION).unwrap();
        encoded.push(0);
        assert!(decode(&encoded, limits, end, crate::parse::header::HEADER_VERSION).is_err());
        encoded.truncate(20);
        encoded[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&encoded, limits, end, crate::parse::header::HEADER_VERSION).is_err());
    }
    #[test]
    fn logical_200mp_geometry_requires_only_small_tile_index() {
        let (d, end) = directory(20000, 10000, 5000, 4000);
        assert_eq!(d.records.len(), 12);
        validate(&d, PlaneSetLimits::default(), end).unwrap();
        assert!(
            validate(
                &d,
                PlaneSetLimits::default().with_plane_pixels(120_000_000),
                end
            )
            .is_err()
        );
        assert!(
            validate(
                &d,
                PlaneSetLimits::default().with_tile_bytes(1_000_000),
                end
            )
            .is_err()
        );
    }
    #[test]
    fn packed_components_require_complete_coherent_backing() {
        let (mut d, end) = directory(8, 8, 4, 4);
        d.planes[0].levels[0].channels = Channels::Rgb;
        for c in 1..3 {
            let mut p = d.planes[0].clone();
            p.name = alloc::format!("component-{c}");
            p.levels[0].component = c;
            d.planes.push(p);
        }
        validate(&d, PlaneSetLimits::default(), end).unwrap();
        let third = d.planes[2].levels[0].clone();
        d.planes[1].levels.push(third);
        d.planes.pop();
        assert!(matches!(
            validate(&d, PlaneSetLimits::default(), end),
            Err(Error::Corrupt(
                "shared color components require distinct planes"
            ))
        ));
        let malformed = encode(
            &d,
            PlaneSetLimits::default(),
            crate::parse::header::HEADER_VERSION,
        )
        .unwrap();
        assert!(
            decode(
                &malformed,
                PlaneSetLimits::default(),
                end,
                crate::parse::header::HEADER_VERSION
            )
            .is_err()
        );
        let third = d.planes[1].levels.pop().unwrap();
        let mut p = d.planes[1].clone();
        p.name = "component-2".into();
        p.levels[0] = third;
        d.planes.push(p);
        d.planes[2].levels[0].component = 1;
        assert!(validate(&d, PlaneSetLimits::default(), end).is_err());
        d.planes[2].levels[0].component = 2;
        d.planes[2].sample = SampleType::U16;
        assert!(validate(&d, PlaneSetLimits::default(), end).is_err());
        d.planes[2].sample = SampleType::U8;
        d.planes.pop();
        assert!(validate(&d, PlaneSetLimits::default(), end).is_err());
    }
}
