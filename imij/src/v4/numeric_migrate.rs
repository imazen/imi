//! Preserve standalone numeric/Quick maps, models and unit entropy.

use super::emit::{Converted, PayloadWriter, extend};
use super::{EntryKind, ObjectIntegrity, SetupContext, TileLayout, TileOrder};
use crate::{ByteReader, Error, Features, ImageInfo};
use alloc::vec::Vec;

pub(super) fn payload(
    info: ImageInfo,
    version: u8,
    declared: Features,
    body: &[u8],
    max_bytes: usize,
    session: &crate::decode_resources::DecodeSession,
) -> Result<Converted, Error> {
    let _descriptors = session.v4_working(body.len() as u128 * 8)?;
    let parsed = crate::v1::parse_complete_body_checked(
        info.clone(),
        version,
        declared,
        body,
        &session.v4_models(),
    )?;
    let mut r = ByteReader::new(body);
    let head = crate::v1::parse_head_checked(info.clone(), &mut r, &session.v4_models(), declared)?;
    let head_end = r.pos;
    let mut prefix = ByteReader::new(&body[..head_end]);
    let mut present = prefix.varint()?;
    if present & crate::v1::H_LAB_BANK != 0 {
        return Err(Error::Unsupported("V4 standalone experimental model bank"));
    }
    let fields = prefix.pos;
    let mut records = Vec::new();
    let end = if let Some(span) = &head.head_records_span {
        let mut r = ByteReader::new(&body[span.clone()]);
        let length = r.var_usize()?;
        crate::parse::records::head_records(r.bytes(length)?, |id, bytes| {
            if ![
                crate::v1::H_UNIT_TABLE,
                crate::v1::H_UNIT_RESERVE,
                crate::v1::H_UNIT_ORDINALS,
            ]
            .contains(&id)
            {
                crate::parse::records::put_record(&mut records, id, bytes);
            }
            Ok(())
        })?;
        span.start
    } else {
        head_end
    };
    if records.is_empty() {
        present &= !crate::v1::H_RECORDS;
    }
    let mut model = Vec::new();
    model
        .try_reserve_exact(head_end)
        .map_err(|_| Error::TooLarge)?;
    crate::put_varint(&mut model, present);
    extend(&mut model, &body[fields..end], max_bytes)?;
    if !records.is_empty() {
        crate::put_varint(&mut model, records.len() as u64);
        extend(&mut model, &records, max_bytes)?;
    }
    // Tiny caller-selected stripes still share a bounded 64-row tile band.
    // A marker/digest pair per pixel row can make retained compressed input
    // exceed the existing row decoder's memory contract.
    let tile_rows = 64usize.div_ceil(parsed.hs).max(1) as u32;
    let layout = TileLayout::new(
        parsed.ngroups() as u32,
        parsed.stripe_count() as u32,
        1,
        tile_rows,
        TileOrder::Raster,
    )?;
    let mut setup = Vec::new();
    setup
        .try_reserve_exact(model.len().checked_add(22).ok_or(Error::TooLarge)?)
        .map_err(|_| Error::TooLarge)?;
    // Default tiles are one group wide and at least one native stripe tall.
    // Explicit tiled writers retain caller-selected grouping and physical order.
    for value in [1, u64::from(tile_rows), 0, 1] {
        crate::put_varint(&mut setup, value);
    }
    extend(&mut setup, &model, max_bytes)?;
    let context =
        SetupContext::from_image_declaration(&info, crate::Coding::Lossless, None, &setup)?;
    let mut writer =
        PayloadWriter::new(layout, ObjectIntegrity::Objects, context, &setup, max_bytes)?;
    for ordinal in 0..layout.tile_count() {
        let tile = layout.tile_at(ordinal)?;
        writer.begin(tile.id, &[])?;
        for group in layout.groups(tile.id)? {
            let id = group.id;
            let entropy = parsed.stripes[usize::try_from(id).map_err(|_| Error::TooLarge)?];
            let mut unit = Vec::new();
            unit.try_reserve_exact(entropy.len().checked_add(10).ok_or(Error::TooLarge)?)
                .map_err(|_| Error::TooLarge)?;
            crate::put_varint(&mut unit, id);
            extend(&mut unit, entropy, max_bytes)?;
            writer.object(crate::registry::UNIT_STRIPE, EntryKind::Unit, 0, id, &unit)?;
        }
        writer.end(tile.id)?;
    }
    writer.finish()
}
