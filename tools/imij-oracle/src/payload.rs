//! The lossless DATA head and unit records (SPEC sections 2, 2.6, 2.7).

use crate::bits::{BitReader, Reader};
use crate::error::{Error, Result};
use crate::header::{Header, coded_channel_kind};
use crate::maps::Map;
use crate::model::{Model, read_model};

pub(crate) struct LosslessHead {
    pub(crate) quick: bool,
    pub(crate) quick_bits: bool,
    pub(crate) lanes: u8,
    pub(crate) stripe_rows: u32,
    /// 0 = no column groups.
    pub(crate) group_cols: u32,
    pub(crate) palette_allowed: bool,
    /// (channel, word) for each coded slot, in slot order.
    pub(crate) slots: Vec<(usize, u8)>,
    /// Coded depth (bits) of each slot.
    pub(crate) slot_depths: Vec<u32>,
    /// One map per coded channel, in coded channel order.
    pub(crate) maps: Vec<Map>,
    pub(crate) models: Vec<Model>,
    pub(crate) palette_model: Option<Model>,
    /// Per-channel near-lossless bounds, when head record 19 is present.
    pub(crate) bounds: Option<Vec<u64>>,
    /// The unit table hint's spans, when head record 10 is present.
    pub(crate) hint: Option<Vec<u64>>,
    /// Bilevel stripes' table (head record 25, SPEC 2.10): `inv` and the
    /// probability of a 0 per context, in 1/1024.
    pub(crate) bilevel: Option<(bool, Vec<u32>)>,
    /// Template palette stripes' tables (head record 35, SPEC 2.14).
    pub(crate) template: Option<crate::template::Tables>,
    /// Whether main unit records carry 0-based ordinals (head record 37, SPEC 2.7).
    pub(crate) unit_ordinals: bool,
    pub(crate) unit_count: u64,
}

impl LosslessHead {
    pub(crate) fn slot_bound(&self, slot: usize) -> u64 {
        match &self.bounds {
            None => 0,
            Some(bounds) => bounds[self.slots[slot].0],
        }
    }
}

/// Parse a lossless head at `data[off..]`. Returns the head and the offset of
/// the first unit record.
pub(crate) fn parse_lossless_head(
    data: &[u8],
    off: usize,
    header: &Header,
) -> Result<(LosslessHead, usize)> {
    let mut r = Reader::new(data);
    r.pos = off;
    let present = r.varint("head present")?;
    if present & !27 != 0 {
        return Err(Error::unsupported("head present bits"));
    }
    let quick = present & 8 != 0;
    let quick_bits = present & 16 != 0;
    if quick_bits && !quick {
        return Err(Error::corrupt("bit smarts without quick pixels"));
    }
    if quick_bits && header.features & (1 << crate::registry::FEATURE_QUICK_BIT_SMARTS) == 0 {
        return Err(Error::corrupt("bit smarts without feature"));
    }
    if quick && header.features & (1 << crate::registry::FEATURE_QUICK_PIXELS) == 0 {
        return Err(Error::corrupt("quick pixels without feature"));
    }
    if quick && present & 1 != 0 {
        return Err(Error::corrupt("quick palette"));
    }
    let lanes = r.u8("lanes")?;
    if !(1..=16).contains(&lanes) || (quick && lanes != 1) {
        return Err(Error::corrupt("bad lane count"));
    }
    let stripe_rows = r.varint("stripe rows")?;
    if stripe_rows < 1 {
        return Err(Error::corrupt("bad stripe height"));
    }
    let stripe_rows =
        u32::try_from(stripe_rows).map_err(|_| Error::corrupt("stripe height too large"))?;
    let group_cols = r.varint("group cols")?;
    if group_cols != 0 && (group_cols < 1 || group_cols >= u64::from(header.width)) {
        return Err(Error::corrupt("bad column group width"));
    }
    let group_cols =
        u32::try_from(group_cols).map_err(|_| Error::corrupt("column group width too large"))?;

    let coded_channels = header.coded_channels();
    let value_bytes = if header.sample.storage_bits() > 16 {
        4
    } else {
        2
    };
    let mut maps = Vec::with_capacity(coded_channels);
    for coded in 0..coded_channels {
        if quick {
            let depth = header.sample.storage_bits().min(32);
            maps.push(Map::new(
                0,
                depth as u8,
                0,
                depth,
                coded_channel_kind(header.sample, coded, header.channels),
                header.features,
            )?);
            continue;
        }
        let mode = r.u8("channel map mode")?;
        let bits = r.u8("channel map bits")?;
        let value = match value_bytes {
            2 => u32::from(u16::from_le_bytes(
                r.take(2, "channel map value")?.try_into().unwrap(),
            )),
            _ => u32::from_le_bytes(r.take(4, "channel map value")?.try_into().unwrap()),
        };
        let word_bits = if header.sample.is_wide() {
            32
        } else {
            header.storage_bits()
        };
        let kind = coded_channel_kind(header.sample, coded, header.channels);
        let mut map = Map::new(mode, bits, value, word_bits, kind, header.features)?;
        if mode == 9 {
            map.read_table(|| r.varint("value table entry"))?;
        }
        maps.push(map);
    }
    let slots = build_slots(header, &maps);
    let slot_depths: Vec<u32> = slots
        .iter()
        .map(|(channel, word)| {
            let coded = if *word == 0 {
                *channel
            } else {
                *channel + header.channels as usize
            };
            u32::from(maps[coded].bits)
        })
        .collect();

    let models_len = if quick { 0 } else { r.varint("models length")? };
    let models_start = r.pos;
    if models_start + models_len as usize > data.len() {
        return Err(Error::corrupt("truncated models"));
    }
    let models_bytes = &data[models_start..models_start + models_len as usize];
    r.pos += models_len as usize;
    let mut bit_reader = BitReader::new(models_bytes);
    let mut models = Vec::with_capacity(slots.len());
    for depth in if quick { &[][..] } else { &slot_depths[..] } {
        models.push(read_model(&mut bit_reader, *depth, "slot model")?);
    }
    let palette_model = if present & 1 != 0 {
        Some(read_model(&mut bit_reader, 8, "palette model")?)
    } else {
        None
    };
    // The reference pads the models bit stream to a byte boundary; any
    // remaining bits are ignored (SPEC finding: padding is not specified).

    let mut hint: Option<Vec<u64>> = None;
    let mut bounds: Option<Vec<u64>> = None;
    let mut bilevel: Option<(bool, Vec<u32>)> = None;
    let mut template: Option<crate::template::Tables> = None;
    let mut unit_ordinals = false;
    if present & 2 != 0 {
        let records_len = r.varint("head records length")?;
        let records_end = r.pos + records_len as usize;
        if records_end > data.len() {
            return Err(Error::corrupt("truncated head records"));
        }
        let mut previous: Option<u64> = None;
        while r.pos < records_end {
            let id = r.varint("head record id")?;
            let len = r.varint("head record len")?;
            if let Some(prev) = previous
                && id <= prev
            {
                return Err(Error::corrupt("head record order"));
            }
            previous = Some(id);
            let start = r.pos;
            if start + len as usize > records_end {
                return Err(Error::corrupt("head record overruns its list"));
            }
            let payload = &data[start..start + len as usize];
            r.pos += len as usize;
            match id {
                crate::registry::HEAD_UNIT_TABLE => {
                    let mut hint_reader = Reader::new(payload);
                    let mut entries = Vec::new();
                    while hint_reader.pos < payload.len() {
                        let entry = hint_reader.varint("unit table entry")?;
                        if entry < 2 {
                            return Err(Error::corrupt("unit table entry below 2"));
                        }
                        entries.push(entry);
                    }
                    hint = Some(entries);
                }
                crate::registry::HEAD_NEAR => {
                    if header.features >> crate::registry::FEATURE_NEAR_LOSSLESS & 1 == 0 {
                        return Err(Error::corrupt("near-lossless record without its feature"));
                    }
                    if !matches!(
                        header.sample,
                        crate::header::SampleType::U8
                            | crate::header::SampleType::U16
                            | crate::header::SampleType::U32
                            | crate::header::SampleType::I32
                    ) {
                        return Err(Error::corrupt(
                            "near-lossless on an unsupported sample type",
                        ));
                    }
                    let mut bounds_reader = Reader::new(payload);
                    let mut values = Vec::with_capacity(header.channels as usize);
                    for _ in 0..header.channels {
                        values.push(bounds_reader.varint("near-lossless bound")?);
                    }
                    if bounds_reader.pos != payload.len() {
                        return Err(Error::corrupt("near-lossless record trailing bytes"));
                    }
                    let mut any = false;
                    for (channel, bound) in values.iter().enumerate() {
                        if *bound > 0 {
                            any = true;
                            let map = &maps[channel];
                            if map.is_const() {
                                return Err(Error::corrupt(
                                    "near-lossless bound on a const channel",
                                ));
                            }
                            if map.mode == 9 {
                                return Err(Error::corrupt(
                                    "near-lossless bound on a value table channel",
                                ));
                            }
                            let depth = u32::from(map.bits);
                            if depth < 1 || *bound > (1u64 << (depth - 1)) - 1 {
                                return Err(Error::corrupt("near-lossless bound too large"));
                            }
                        }
                    }
                    if !any {
                        return Err(Error::corrupt("near-lossless record without a bound"));
                    }
                    bounds = Some(values);
                }
                crate::registry::HEAD_BILEVEL => {
                    if header.features >> crate::registry::FEATURE_BILEVEL & 1 == 0 {
                        return Err(Error::corrupt("bilevel table without its feature"));
                    }
                    let mut bits = crate::bits::BitReader::new(payload);
                    let inv = bits.read(1, "bilevel inversion bit")? == 1;
                    let mut probs = vec![512u32; 1024];
                    let mut used = 1 + 1024;
                    for p in probs.iter_mut() {
                        if bits.read(1, "bilevel used bit")? == 1 {
                            *p = bits.read(11, "bilevel probability")? as u32;
                            used += 11;
                            if *p > 1024 {
                                return Err(Error::corrupt("bilevel probability above 1"));
                            }
                        }
                    }
                    if payload.len() != usize::div_ceil(used, 8) {
                        return Err(Error::corrupt("bilevel table length"));
                    }
                    bilevel = Some((inv, probs));
                }
                crate::registry::HEAD_TEMPLATE => {
                    if header.features >> crate::registry::FEATURE_TEMPLATE_PALETTES & 1 == 0 {
                        return Err(Error::corrupt("template tables without their feature"));
                    }
                    template = Some(crate::template::read_tables(payload, &slot_depths)?);
                }
                crate::registry::HEAD_UNIT_ORDINALS => {
                    if header.features >> crate::registry::FEATURE_UNIT_ORDINALS & 1 == 0 {
                        return Err(Error::corrupt("unit ordinals without their feature"));
                    }
                    if !payload.is_empty() {
                        return Err(Error::corrupt("unit ordinals record length"));
                    }
                    unit_ordinals = true;
                }
                other if other % 2 == 1 => {
                    return Err(Error::unsupported(format!("head record id {other}")));
                }
                _ => {}
            }
        }
        if r.pos != records_end {
            return Err(Error::corrupt("head record list length mismatch"));
        }
    }

    if quick && (bounds.is_some() || bilevel.is_some() || template.is_some()) {
        return Err(Error::corrupt("quick transformed samples"));
    }
    let stripes = u64::from(header.height).div_ceil(u64::from(stripe_rows));
    let groups = if group_cols > 0 {
        u64::from(header.width).div_ceil(u64::from(group_cols))
    } else {
        1
    };
    let unit_count = stripes * groups;
    if let Some(entries) = &hint
        && entries.len() as u64 != unit_count
    {
        return Err(Error::corrupt("unit table length"));
    }
    if unit_count > (data.len() - r.pos) as u64 {
        return Err(Error::corrupt("stripe count exceeds data"));
    }
    Ok((
        LosslessHead {
            quick,
            quick_bits,
            lanes,
            stripe_rows,
            group_cols,
            palette_allowed: present & 1 != 0,
            slots,
            slot_depths,
            maps,
            models,
            palette_model,
            bounds,
            hint,
            bilevel,
            template,
            unit_ordinals,
            unit_count,
        },
        r.pos,
    ))
}

/// Slot order (SPEC section 2.2): [G, R, B] (then alpha), and for 64-bit
/// samples the hi words first, then the lo words in the same order, each
/// unless its map is Const.
fn build_slots(header: &Header, maps: &[Map]) -> Vec<(usize, u8)> {
    let color_channels: Vec<usize> = match header.channel_layout {
        1 => vec![0],
        2 => vec![0, 1],
        3 => vec![1, 0, 2],
        4 => vec![1, 0, 2, 3],
        5 | 7 => vec![0, 1, 2, 3],
        6 => vec![0, 1, 2],
        _ => unreachable!("header checked the channel layout"),
    };
    let mut slots = Vec::new();
    if header.sample.is_wide() {
        for channel in &color_channels {
            if !maps[*channel].is_const() {
                slots.push((*channel, 0));
            }
        }
        for channel in &color_channels {
            if !maps[*channel + header.channels as usize].is_const() {
                slots.push((*channel, 1));
            }
        }
    } else {
        for channel in &color_channels {
            if !maps[*channel].is_const() {
                slots.push((*channel, 0));
            }
        }
    }
    slots
}

/// A unit record's span: (start, end, payload start, payload length).
pub(crate) struct UnitSpan {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) payload_start: usize,
    pub(crate) payload_len: usize,
}

/// Walk the lossless unit records (SPEC section 2.7), checking the unit table
/// hint when present. `limit` bounds the payload (the chunk end, or the whole
/// file for an open DATA chunk).
pub(crate) fn walk_units(
    data: &[u8],
    head: &LosslessHead,
    version: u8,
    start: usize,
    limit: usize,
) -> Result<Vec<UnitSpan>> {
    let mut r = Reader::new(data);
    r.pos = start;
    let mut spans = Vec::with_capacity(head.unit_count as usize);
    for index in 0..head.unit_count {
        let span_start = r.pos;
        let mut id = crate::registry::unit_id(version, r.varint("unit record id")?);
        let mut len = r.varint("unit record length")?;
        if id.is_multiple_of(2) {
            // The one expected side record: the stripe record's.
            if id != crate::registry::UNIT_STRIPE_SIDE {
                return Err(Error::corrupt("unexpected even unit record id"));
            }
            if r.pos + len as usize > limit {
                return Err(Error::corrupt("side record overruns the payload"));
            }
            r.pos += len as usize;
            id = crate::registry::unit_id(version, r.varint("unit record id")?);
            len = r.varint("unit record length")?;
        }
        match id {
            crate::registry::UNIT_STRIPE => {}
            crate::registry::UNIT_DC
            | crate::registry::UNIT_ALPHA
            | crate::registry::UNIT_AC
            | crate::registry::UNIT_EXACT => {
                return Err(Error::corrupt("unit record of the wrong kind"));
            }
            other if other % 2 == 1 => {
                return Err(Error::unsupported(format!("unit record id {other}")));
            }
            _ => {
                return Err(Error::corrupt("unexpected even unit record id"));
            }
        }
        let record_start = r.pos;
        if record_start + len as usize > limit {
            return Err(Error::corrupt("unit record overruns the payload"));
        }
        let record_end = record_start + len as usize;
        let payload_start = if head.unit_ordinals {
            let mut pr = Reader::new(&data[..record_end]);
            pr.pos = record_start;
            let ord = pr.varint("unit record ordinal")?;
            if ord != index {
                return Err(Error::corrupt("unit record ordinal"));
            }
            pr.pos
        } else {
            record_start
        };
        r.pos = record_end;
        spans.push(UnitSpan {
            start: span_start,
            end: r.pos,
            payload_start,
            payload_len: record_end - payload_start,
        });
    }
    if let Some(entries) = &head.hint {
        let total: u64 = entries.iter().sum();
        if total > (limit - start) as u64 {
            return Err(Error::corrupt("unit table exceeds the payload"));
        }
        for (index, span) in spans.iter().enumerate() {
            if (span.end - span.start) as u64 != entries[index] {
                return Err(Error::corrupt("unit table does not match the records"));
            }
        }
    }
    Ok(spans)
}
