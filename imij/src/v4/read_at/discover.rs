//! Header-only span discovery for ordinary unindexed region readers.
use super::*;

pub(super) fn directory<R: ReadAt, C: Fn() -> bool + Sync>(
    input: &mut Cursor<'_, '_, R, C>,
    root: &Root,
    limits: TileLimits,
    data_end: Option<u64>,
) -> Result<(Vec<u8>, V4Retained), Error> {
    let start = input.at;
    let mut writer = super::super::DirectoryWriter::new(root.layout(), root.integrity());
    let mut entries = Vec::new();
    let mut memory = None;
    let mut tiles = 0u64;
    let mut ignorable = 0u64;
    loop {
        if data_end == Some(input.at) {
            break;
        }
        if data_end.is_none() && tiles == root.layout().tile_count() {
            break;
        }
        let at = input.at;
        let kind = input.varint()?;
        let length = input.varint()?;
        let end = input
            .at
            .checked_add(length)
            .filter(|&n| n <= input.length && data_end.is_none_or(|max| n <= max))
            .ok_or(Error::Corrupt("V4 discovered frame past DATA"))?;
        let record_len = end.checked_sub(at).ok_or(Error::TooLarge)?;
        if record_len > limits.max_object_bytes {
            return Err(Error::TooLarge);
        }
        let body_end = end
            .checked_sub(root.integrity().trailer_len() as u64)
            .filter(|&n| n >= input.at)
            .ok_or(Error::Corrupt("short V4 integrity trailer"))?;
        // Identity varints may not borrow bytes from the digest/next object.
        let file_end = input.length;
        input.length = body_end;
        let (entry_kind, role, id) = match kind {
            crate::registry::MODEL_DEFINITION => {
                let role = u8::try_from(input.varint()?)
                    .map_err(|_| Error::Corrupt("V4 discovered model role"))?;
                let id = input.varint()?;
                (EntryKind::model_lifetime(input.varint()?)?, role, id)
            }
            65 => (EntryKind::TileBegin, 0, input.varint()?),
            67 => {
                tiles = tiles.checked_add(1).ok_or(Error::TooLarge)?;
                (EntryKind::TileEnd, 0, input.varint()?)
            }
            crate::registry::UNIT_DC | crate::registry::UNIT_STRIPE => {
                (EntryKind::Unit, 0, input.varint()?)
            }
            crate::registry::UNIT_AC => (EntryKind::Unit, 1, input.varint()?),
            crate::registry::UNIT_ALPHA => (EntryKind::Unit, 2, input.varint()?),
            crate::registry::UNIT_EXACT => (EntryKind::Unit, 3, input.varint()?),
            kind if kind & 1 == 0 => {
                let id = ignorable;
                ignorable = ignorable.checked_add(1).ok_or(Error::TooLarge)?;
                (EntryKind::Ignorable, 0, id)
            }
            _ => return Err(Error::Unsupported("unknown V4 critical object")),
        };
        input.length = file_end;
        input.skip(end - input.at)?;
        let mut encoded = [0; 40];
        let n = writer.push(
            Entry {
                kind: entry_kind,
                role,
                id,
                record_len,
            },
            &mut encoded,
        )?;
        let needed = entries.len().checked_add(n).ok_or(Error::TooLarge)?;
        if needed > entries.capacity() {
            let count = entries
                .capacity()
                .max(40)
                .checked_mul(2)
                .map(|v| v.max(needed))
                .ok_or(Error::TooLarge)?;
            let next = input.session.v4_working(count as u128)?;
            entries
                .try_reserve_exact(count - entries.len())
                .map_err(|_| Error::TooLarge)?;
            memory = Some(next);
        }
        entries.extend_from_slice(&encoded[..n]);
        input.session.adapter_work(n as u128 + 1)?;
    }
    let (count, objects) = writer.finish()?;
    if input.at.checked_sub(start) != Some(objects) {
        return Err(Error::Corrupt("V4 discovered DATA extent"));
    }
    let length = entries
        .len()
        .checked_add(crate::varint_len(count))
        .and_then(|v| v.checked_add(8))
        .ok_or(Error::TooLarge)?;
    let output_memory = input.session.v4_working(length as u128)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(length)
        .map_err(|_| Error::TooLarge)?;
    crate::put_varint(&mut output, count);
    output.extend_from_slice(&entries);
    output.extend_from_slice(&objects.to_le_bytes());
    drop(entries);
    drop(memory);
    Ok((output, output_memory.retain()?))
}
