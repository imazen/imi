//! Attach scoped checks while retaining compressed pixel and model payloads.
use super::emit::{PayloadWriter, extend};
use super::*;
use crate::decode_resources::DecodeSession;
use crate::{ByteReader, Coding, Error, ImageInfo, ImageRef};
use alloc::{sync::Arc, vec::Vec};

enum Root {
    Numeric(Arc<NumericModel>),
    Lossy(Arc<LossySetup>),
}
impl Root {
    fn group_rect(&self, id: u64) -> Result<crate::planes::Rect, Error> {
        match self {
            Self::Numeric(m) => m.group_rect(id),
            Self::Lossy(s) => s.group_rect(id),
        }
    }
}
fn results<C: Fn() -> bool + Sync>(
    checks: &OwnedChecks,
    scope: CheckScope,
    id: u64,
    rect: crate::planes::Rect,
    info: &ImageInfo,
    pixels: &[u8],
    session: &DecodeSession<C>,
) -> Result<Vec<u8>, Error> {
    let mut local = info.clone();
    local.width = rect.width();
    local.height = rect.height();
    let at = rect.y() as usize * info.row_bytes() + rect.x() as usize * info.bytes_per_pixel();
    let list = checks.get().list(scope);
    if list.is_empty() {
        return Ok(Vec::new());
    }
    let known = list.iter().filter(|d| d.algorithm().is_some()).count();
    session.adapter_work(
        u128::from(local.width)
            * u128::from(local.height)
            * local.bytes_per_pixel() as u128
            * known as u128,
    )?;
    let _hash_scratch = session.v4_working(PixelChecks::working_bytes(list) as u128)?;
    let mut hash = PixelChecks::new(
        list,
        scope,
        PixelCheckIdentity {
            image: 0,
            id,
            x: rect.x(),
            y: rect.y(),
        },
        &local,
    )?;
    use crate::parse::limits::Admission;
    hash.rows_checked(
        ImageRef::new(&pixels[at..], info.row_bytes(), &local),
        || session.v4_models().check(),
    )?;
    hash.results()
}

/// Add built-in scoped checks to one standalone V4 image, preserving entropy,
/// model definitions, metadata and existing whole-image checks. The actual
/// reconstruction is decoded once; lossy tags never hash the original source.
///
/// This allocating editor admits a complete raster against `session` and bounds
/// encoded input/output with `max_encoded_bytes`. Forward numeric producers use
/// `NumericFileEncoder::new_checked` instead. Existing scoped declarations,
/// frame/page collections and plane sets are refused before reconstruction:
/// those edits require their owners to rebuild nested identities and offsets.
/// Unknown suites can be framed by `Checks`, but cannot be generated here.
pub fn add_scoped_checks<C: Fn() -> bool + Sync>(
    file: &[u8],
    checks: Checks<'_>,
    session: &DecodeSession<C>,
    max_encoded_bytes: usize,
) -> Result<Vec<u8>, Error> {
    if file.len() > max_encoded_bytes {
        return Err(Error::TooLarge);
    }
    for scope in [
        CheckScope::GroupPixels,
        CheckScope::TilePixels,
        CheckScope::EncodedObject,
    ] {
        if checks.list(scope).iter().any(|d| d.algorithm().is_none()) {
            return Err(Error::Unsupported("V4 encoder integrity suite"));
        }
    }
    let Some(checks) = checks.owned()? else {
        let mut output = Vec::new();
        extend(&mut output, file, max_encoded_bytes)?;
        return Ok(output);
    };
    if crate::parse::header::header_version(file)? != 4 {
        return Err(Error::InvalidInput("scoped checks require V4"));
    }
    let parsed = super::file::parse_file(file, session)?;
    if parsed.anim.is_some() || !parsed.frames.is_empty() {
        return Err(Error::Unsupported(
            "scoped check editor requires a standalone image",
        ));
    }
    let mut outer = ByteReader::new(file);
    crate::parse::header::read_versioned(&mut outer, u64::MAX)?;
    let header_end = outer.pos;
    while outer.remaining() != 0 {
        let (tag, length) = crate::parse::chunks::read_head(&mut outer)?;
        if crate::parse::chunks::is_plane_chunk(&tag) {
            return Err(Error::Unsupported(
                "scoped check editor requires rebuilding plane indexes",
            ));
        }
        match length {
            Some(n) => {
                crate::parse::chunks::payload(&mut outer, n)?;
            }
            None => {
                outer.bytes(parsed.data.len())?;
            }
        }
    }
    let info = parsed.header.info.clone();
    let coding = parsed.header.coding;
    let color = parsed.header.lossy_color;
    let data = parsed.data;
    let summed = crate::container::ChecksumChunk::rewritable(parsed.checksum)?;
    let limits = TileLimits {
        max_object_bytes: max_encoded_bytes as u64,
        max_tile_bytes: max_encoded_bytes as u64,
    };
    let (root, layout, integrity, objects_at) = match coding {
        Coding::Lossless => {
            let (decoder, at, _) = NumericGroupDecoder::with_directory(
                info.clone(),
                parsed.declared,
                data,
                limits,
                session,
            )?;
            if decoder.check_config().is_some() {
                return Err(Error::InvalidInput("image already has scoped checks"));
            }
            (
                Root::Numeric(decoder.shared_model()),
                decoder.layout(),
                decoder.integrity(),
                at,
            )
        }
        Coding::Lossy => {
            let (decoder, at, _) = LossyGroupDecoder::with_directory(
                info.clone(),
                color.ok_or(Error::Corrupt("V4 lossy color missing"))?,
                parsed.declared,
                data,
                limits,
                session,
            )?;
            if decoder.setup().checks.is_some() {
                return Err(Error::InvalidInput("image already has scoped checks"));
            }
            (
                Root::Lossy(decoder.shared_setup()),
                decoder.setup().layout(),
                decoder.setup().integrity(),
                at,
            )
        }
    };
    let size = info.checked_buffer_len().ok_or(Error::TooLarge)?;
    let _raster = session.v4_working(size as u128)?;
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(size)
        .map_err(|_| Error::TooLarge)?;
    pixels.resize(size, 0);
    let image = super::decode::ImageFile::from_parsed(parsed, file.len(), session, None)?;
    super::decode::run_file(image, &mut pixels, info.row_bytes(), session, None, 1)?;
    let mut r = ByteReader::new(data);
    let setup_bytes = r.var_usize()?;
    let old_setup = r.bytes(setup_bytes)?;
    let old_context = SetupContext::from_image_declaration(&info, coding, color, old_setup)?;
    let mut setup = Vec::new();
    extend(&mut setup, old_setup, max_encoded_bytes)?;
    extend(&mut setup, checks.bytes(), max_encoded_bytes)?;
    let context = SetupContext::from_image_declaration(&info, coding, color, &setup)?;
    let group_kind = if coding == Coding::Lossless {
        crate::registry::UNIT_STRIPE
    } else {
        crate::registry::UNIT_AC
    };
    let mut writer = PayloadWriter::new(layout, integrity, context, &setup, max_encoded_bytes)?
        .with_checks(checks.clone(), group_kind);
    let objects = &data[objects_at..];
    let mut reader = TileReader::new(layout, integrity, old_context, limits, session);
    let mut at = 0;
    let mut ignorable = 0;
    while at < objects.len() {
        let event = reader.read(&objects[at..])?;
        match event {
            TileEvent::Begin { extent, exact_plan } => {
                writer.begin(
                    extent
                        .tile_id
                        .ok_or(Error::Corrupt("missing V4 tile identity"))?,
                    exact_plan,
                )?;
                at += extent.len as usize;
            }
            TileEvent::Object { extent, object } => {
                let mut body = ByteReader::new(object.body);
                let (kind, role, id) = if object.kind & 1 == 0 {
                    let id = ignorable;
                    ignorable += 1;
                    (EntryKind::Ignorable, 0, id)
                } else if object.kind == crate::registry::MODEL_DEFINITION {
                    let role = u8::try_from(body.varint()?)
                        .map_err(|_| Error::Corrupt("V4 model role"))?;
                    let id = body.varint()?;
                    (EntryKind::model_lifetime(body.varint()?)?, role, id)
                } else {
                    let role = match object.kind {
                        crate::registry::UNIT_STRIPE | crate::registry::UNIT_DC => 0,
                        crate::registry::UNIT_AC => 1,
                        crate::registry::UNIT_ALPHA => 2,
                        crate::registry::UNIT_EXACT => 3,
                        _ => return Err(Error::Unsupported("V4 checked object kind")),
                    };
                    (EntryKind::Unit, role, body.varint()?)
                };
                let tags = if object.kind == group_kind {
                    results(
                        &checks,
                        CheckScope::GroupPixels,
                        id,
                        root.group_rect(id)?,
                        &info,
                        &pixels,
                        session,
                    )?
                } else {
                    Vec::new()
                };
                writer.object_checked(object.kind, kind, role, id, object.body, &tags)?;
                at += extent.len as usize;
            }
            TileEvent::End { extent, tile, .. } => {
                let rect = tile_pixel_rect(layout, tile.tile_id, |id| root.group_rect(id))?;
                let tags = results(
                    &checks,
                    CheckScope::TilePixels,
                    tile.tile_id,
                    rect,
                    &info,
                    &pixels,
                    session,
                )?;
                writer.end_checked(tile.tile_id, &tags)?;
                at += extent.len as usize;
            }
        }
    }
    reader.finish()?;
    let converted = writer.finish()?;
    let mut output = Vec::new();
    extend(&mut output, &file[..header_end], max_encoded_bytes)?;
    let bound = crate::UpgradeLimits {
        max_encoded_bytes,
        ..Default::default()
    };
    let mut r = ByteReader::new(&file[header_end..]);
    while r.remaining() != 0 {
        let start = r.pos;
        let (tag, length) = crate::parse::chunks::read_head(&mut r)?;
        match length {
            Some(n) => {
                crate::parse::chunks::payload(&mut r, n)?;
            }
            None => {
                r.bytes(data.len())?;
            }
        }
        match &tag {
            b"DATA" => crate::upgrade::chunk(&mut output, b"DATA", &converted.data, &bound)?,
            b"cSUM" | b"uIDX" => {}
            _ => extend(
                &mut output,
                &file[header_end + start..header_end + r.pos],
                max_encoded_bytes,
            )?,
        }
    }
    crate::upgrade::chunk(&mut output, b"uIDX", &converted.index, &bound)?;
    output = crate::container::locate(output)?;
    if summed {
        output = crate::checksum::refinish(output)?;
    }
    if output.len() > max_encoded_bytes {
        return Err(Error::TooLarge);
    }
    Ok(output)
}
