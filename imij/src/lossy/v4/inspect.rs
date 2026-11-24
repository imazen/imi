//! Lab inspection over the production V4 schedule; no legacy-prefix probing.
use super::{Definition, LossyEvent, LossyFile};
use crate::decode_resources::{DecodePolicy, DecodeSession};
use crate::v4::{ObjectIntegrity, TileLimits};
use crate::{ByteReader, Error, ImageInfo, LossyEntropy};
use alloc::vec::Vec;

fn limits() -> TileLimits {
    TileLimits {
        max_object_bytes: u64::MAX,
        max_tile_bytes: u64::MAX,
    }
}
fn session() -> DecodeSession {
    DecodeSession::new(DecodePolicy::default().with_output_limits(crate::Limits::NONE))
}

#[cfg(feature = "encode")]
pub(crate) fn quantized(data: &[u8]) -> Result<(usize, usize, Vec<i32>, usize, usize), Error> {
    let session = session();
    let file = LossyFile::open(data, limits(), &session)?;
    let setup = file.inspection_setup();
    if setup.layout.group_grid().0 != 1 {
        return Err(Error::Unsupported(
            "lab coefficient export: one column group and default layouts",
        ));
    }
    let bw = setup.info.width.div_ceil(8) as usize;
    let bh = setup.info.height.div_ceil(8) as usize;
    let control = setup.control;
    let row = bw.checked_mul(3 * 64).ok_or(Error::TooLarge)?;
    let count = row.checked_mul(bh).ok_or(Error::TooLarge)?;
    let _memory = session.v4_working(count as u128 * 4)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(count)
        .map_err(|_| Error::TooLarge)?;
    output.resize(count, 0);
    let (mut ac, mut dc) = (0usize, 0usize);
    file.visit_units(|event| {
        if let LossyEvent::Unit { work, .. } = event {
            match work.role() {
                0 => {
                    dc = dc
                        .checked_add(work.inspection_entropy_bytes())
                        .ok_or(Error::TooLarge)?
                }
                1 => {
                    let rect = work.setup().group_rect(work.id())?;
                    let start = (rect.y() as usize / 8)
                        .checked_mul(row)
                        .ok_or(Error::TooLarge)?;
                    ac = ac
                        .checked_add(work.inspect_coefficients(
                            &mut output[start..],
                            row,
                            &session,
                        )?)
                        .ok_or(Error::TooLarge)?;
                }
                _ => {}
            }
        }
        Ok(())
    })?;
    let (info, pixels, _) = dc_image(data)?;
    let (mut values, mut scales) = (Vec::new(), Vec::new());
    super::super::dc_values(
        info.sample,
        control != 0,
        control == 2,
        &pixels,
        &mut values,
        &mut scales,
    )?;
    if values.len() != bw * bh * 3 {
        return Err(Error::Corrupt("V4 coefficient inspection DC count"));
    }
    for (block, dc) in output.as_chunks_mut::<64>().0.iter_mut().zip(values) {
        block[0] = dc;
    }
    Ok((bw, bh, output, ac, dc))
}

pub(crate) fn models(data: &[u8]) -> Result<(LossyEntropy, crate::seeds::Selection, u64), Error> {
    let session = session();
    let file = LossyFile::open(data, limits(), &session)?;
    let coder = file.inspection_setup().coder;
    let mut selection = crate::seeds::Selection::NONE;
    let mut table_bytes = 0u64;
    let digest = if file.inspection_setup().integrity() == ObjectIntegrity::None {
        0
    } else {
        16
    };
    let mut r = ByteReader::new(file.inspection_objects());
    while r.remaining() != 0 {
        let kind = r.varint()?;
        let length = r.var_usize()?;
        let body = r.bytes(length)?;
        let payload = body
            .get(
                ..length
                    .checked_sub(digest)
                    .ok_or(Error::Corrupt("V4 inspection object length"))?,
            )
            .ok_or(Error::Corrupt("V4 inspection object body"))?;
        if kind == crate::registry::MODEL_DEFINITION {
            let definition = Definition::read(file.inspection_setup(), payload)?;
            if definition.role == 1 {
                table_bytes = table_bytes
                    .checked_add(definition.inspection_bytes().len() as u64)
                    .ok_or(Error::TooLarge)?;
            }
            let bank = definition.inspection_bank();
            if bank != 0 {
                if selection.bank != 0 && selection.bank != bank {
                    return Err(Error::Unsupported("lab summary requires one V4 seed bank"));
                }
                selection.bank = bank;
                selection.scope |= match definition.role {
                    0 => 1 << 5,
                    1 => 1 << 4,
                    2 => 1 << 6,
                    _ => 0,
                };
            }
        }
    }
    // The summary succeeds only after the same framing, model, lifetime and
    // directory checks as ordinary complete-input delivery. Entropy stays opaque.
    file.visit_units(|_| Ok(()))?;
    Ok((coder, selection, table_bytes))
}

pub(crate) fn dc_image(data: &[u8]) -> Result<(ImageInfo, Vec<u8>, u32), Error> {
    let session = session();
    let file = LossyFile::open(data, limits(), &session)?;
    let geometry = file
        .inspection_setup()
        .numeric_geometry(0)
        .ok_or(Error::Corrupt("V4 inspection missing DC geometry"))?
        .clone();
    let info = geometry.info;
    let stride = info.row_bytes();
    let length = info.checked_buffer_len().ok_or(Error::TooLarge)?;
    let _memory = session.v4_working(length as u128)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(length)
        .map_err(|_| Error::TooLarge)?;
    output.resize(length, 0);
    file.visit_units(|event| {
        if let LossyEvent::Unit { work, .. } = event
            && work.role() == 0
        {
            let (id, rect) = work.setup().dc_for_group(work.id())?;
            if id != work.id() {
                return Err(Error::Corrupt("V4 DC inspection coverage identity"));
            }
            let at = (rect.y() as usize)
                .checked_mul(stride)
                .and_then(|n| n.checked_add(rect.x() as usize * info.bytes_per_pixel()))
                .ok_or(Error::TooLarge)?;
            work.decode_numeric_into(
                rect,
                output
                    .get_mut(at..)
                    .ok_or(Error::Corrupt("V4 DC inspection extent"))?,
                stride,
                &session,
            )?;
        }
        Ok(())
    })?;
    Ok((info, output, geometry.stripe_rows))
}
