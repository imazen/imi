//! Stripe decoding and reconstruction (SPEC sections 2.3-2.5, 3, 4).

use crate::bits::Reader;
use crate::error::{Error, Result};
use crate::header::Header;
use crate::image::{Image, Reference};
use crate::maps::mask;
use crate::payload::LosslessHead;
use crate::rans::{Residuals, decode_stream};

fn sext(value: u32, bits: u32) -> i64 {
    let value = (value & mask(bits)) as i64;
    if bits >= 32 {
        if value & (1 << 31) != 0 {
            value - (1 << 32)
        } else {
            value
        }
    } else if value & (1 << (bits - 1)) != 0 {
        value - (1 << bits)
    } else {
        value
    }
}

#[allow(clippy::too_many_arguments)]
fn predict(id: u8, w: u32, n: u32, nw: u32, ne: u32, r: u32, rw: u32, rn: u32, bits: u32) -> u32 {
    match id {
        0 => w,
        1 => n,
        2 => ((u64::from(w) + u64::from(n)) >> 1) as u32,
        3 => {
            if nw >= w.max(n) {
                w.min(n)
            } else if nw <= w.min(n) {
                w.max(n)
            } else {
                // W + N - NW is between min and max of the two, but the sum
                // needs 33 bits at depth 32.
                (i64::from(w) + i64::from(n) - i64::from(nw)) as u32
            }
        }
        4 => (w.wrapping_add(n).wrapping_sub(nw)) & mask(bits),
        5 => {
            // Paeth's distances use the signed value of W + N - NW (it can be
            // negative at n-bit boundaries).
            let p = i64::from(w) + i64::from(n) - i64::from(nw);
            let pa = (p - i64::from(w)).abs();
            let pb = (p - i64::from(n)).abs();
            let pc = (p - i64::from(nw)).abs();
            if pa <= pb && pa <= pc {
                w
            } else if pb <= pc {
                n
            } else {
                nw
            }
        }
        6 => ((u64::from(w) + u64::from(ne)) >> 1) as u32,
        7 => r,
        8 => (w.wrapping_add(r).wrapping_sub(rw)) & mask(bits),
        9 => (n.wrapping_add(r).wrapping_sub(rn)) & mask(bits),
        _ => unreachable!("predictor validated"),
    }
}

/// Decode one unit record payload into `image`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_unit(
    data: &[u8],
    payload_start: usize,
    payload_len: usize,
    head: &LosslessHead,
    header: &Header,
    stripe: u32,
    group: u32,
    image: &mut Image,
    reference: Option<&dyn Reference>,
) -> Result<()> {
    let payload = &data[payload_start..payload_start + payload_len];
    let mut r = Reader::new(payload);

    let rows = (header.height - stripe * head.stripe_rows).min(head.stripe_rows) as usize;
    let group_cols = head.group_cols;
    let group_width = if group_cols > 0 {
        (header.width - group * group_cols).min(group_cols)
    } else {
        header.width
    };
    let x_base = group * group_cols;
    if head.quick {
        return crate::quick::decode(
            payload,
            header,
            head.quick_bits,
            x_base,
            stripe * head.stripe_rows,
            group_width,
            rows as u32,
            image,
        );
    }
    let lanes = head.lanes as usize;
    let lane_width = (group_width as usize).div_ceil(lanes);
    let widths: Vec<usize> = (0..lanes)
        .map(|lane| {
            let start = (lane * lane_width) as u32;
            (group_width
                .min(start + lane_width as u32)
                .saturating_sub(start)) as usize
        })
        .collect();
    let active = widths.iter().take_while(|width| **width > 0).count();
    let active_widths = &widths[..active];

    let decor = r.u8("stripe decorrelation")?;
    if decor == 0x81 || decor == 0x82 {
        let geometry = (stripe * head.stripe_rows, x_base, lane_width);
        return decode_bilevel(
            &mut r,
            decor,
            head,
            header,
            geometry,
            rows,
            active_widths,
            image,
        );
    }
    if decor == 0x83 || decor == 0x84 {
        let tables = head
            .template
            .as_ref()
            .ok_or_else(|| Error::corrupt("template palette stripe without template tables"))?;
        let entries = crate::template::decode(
            &mut r,
            decor,
            tables,
            &head.slot_depths,
            rows,
            active_widths,
            lane_width,
        )?;
        let y0 = stripe * head.stripe_rows;
        for (row, values) in entries.iter().enumerate() {
            for (column, entry) in values.iter().enumerate() {
                write_pixel(
                    image,
                    x_base + column as u32,
                    y0 + row as u32,
                    header,
                    head,
                    entry,
                );
            }
        }
        return Ok(());
    }
    let palette = decor == 0x80;
    let mut entries: Vec<Vec<u32>> = Vec::new();
    let mut adapt_weights: Option<([i16; 16], [i16; 16], [i16; 16])> = None;
    if palette {
        if !head.palette_allowed {
            return Err(Error::corrupt("palette stripe without its head bit"));
        }
        if head.slot_depths.iter().any(|depth| *depth > 16) {
            return Err(Error::corrupt("palette stripe with a deep slot"));
        }
        let count = r.u8("palette entries")? as usize + 1;
        for _ in 0..count {
            let mut entry = Vec::with_capacity(head.slot_depths.len());
            for depth in &head.slot_depths {
                let value = if *depth <= 8 {
                    u32::from(r.u8("palette entry")?)
                } else {
                    u32::from(u16::from_le_bytes(
                        r.take(2, "palette entry")?.try_into().unwrap(),
                    ))
                };
                if value >= (1u32 << *depth) {
                    return Err(Error::corrupt("palette entry out of range"));
                }
                entry.push(value);
            }
            entries.push(entry);
        }
    } else if decor == 4 {
        if header.set >> crate::registry::FEATURE_ADAPTIVE_COLOR & 1 == 0 {
            return Err(Error::corrupt(
                "adaptive color stripe without feature adaptive_color",
            ));
        }
        if head.bounds.is_some() {
            return Err(Error::corrupt(
                "adaptive color stripe with near-lossless bounds",
            ));
        }
        if head.slots.len() < 3
            || head.slots[..3] != [(1, 0), (0, 0), (2, 0)]
            || head.slot_depths[..3] != [8, 8, 8]
        {
            return Err(Error::corrupt("adaptive color stripe on ineligible layout"));
        }
        adapt_weights = Some(read_adaptive_weights(&mut r, active)?);
    } else {
        if decor > 3 {
            return Err(Error::corrupt("bad decorrelation mode"));
        }
        if decor != 0 {
            if head.slots.len() < 3 {
                return Err(Error::corrupt("decorrelation without color slots"));
            }
            let first_three: Vec<(usize, u8)> = head.slots[..3].to_vec();
            if first_three != [(1, 0), (0, 0), (2, 0)] {
                return Err(Error::corrupt("decorrelation on non-color slots"));
            }
            if head.slot_depths[0..3]
                .iter()
                .any(|d| *d != head.slot_depths[0])
            {
                return Err(Error::corrupt("decorrelation with unequal depths"));
            }
        }
    }

    let predictor_bytes = rows.div_ceil(2);
    let predictor_slice = r.take(predictor_bytes, "row predictors")?;
    let mut predictors = Vec::with_capacity(rows);
    for row in 0..rows {
        let byte = predictor_slice[row >> 1];
        let predictor = if row % 2 == 0 { byte & 0xF } else { byte >> 4 };
        predictors.push(predictor);
    }
    let inter = reference.is_some();
    let has_bounds = head.bounds.is_some();
    for (row, predictor) in predictors.iter().enumerate() {
        if *predictor > 9 {
            return Err(Error::corrupt("unknown predictor"));
        }
        if *predictor >= 7 {
            if !inter {
                return Err(Error::corrupt("inter predictor outside an inter frame"));
            }
            if adapt_weights.is_some() {
                return Err(Error::corrupt("inter predictor on adaptive color stripe"));
            }
            if header.sample.is_wide() {
                return Err(Error::corrupt("inter predictor on a 64-bit image"));
            }
            if has_bounds {
                return Err(Error::corrupt("inter predictor with near-lossless bounds"));
            }
            if row == 0 && *predictor != 7 {
                return Err(Error::corrupt("bad predictor on stripe row 0"));
            }
        } else if row == 0 && *predictor != 0 {
            return Err(Error::corrupt("bad predictor on stripe row 0"));
        }
    }

    // All stream lengths first, then the streams (SPEC section 2.4).
    let stream_count = if palette { 1 } else { head.slots.len() };
    let mut lengths = Vec::with_capacity(stream_count);
    for _ in 0..stream_count {
        lengths.push(r.varint("stream length")? as usize);
    }
    let mut streams = Vec::with_capacity(stream_count);
    for len in &lengths {
        streams.push(r.take(*len, "slot stream")?);
    }
    if r.remaining() != 0 {
        return Err(Error::corrupt("trailing bytes in a stripe record"));
    }

    // Entropy: one residual plane per slot.
    let mut residuals: Vec<Residuals> = Vec::with_capacity(stream_count);
    if palette {
        let model = head
            .palette_model
            .as_ref()
            .ok_or_else(|| Error::corrupt("palette stripe without a palette model"))?;
        residuals.push(decode_stream(
            streams[0],
            active_widths,
            rows,
            model,
            8,
            None,
        )?);
    } else {
        for (slot, model) in head.models.iter().enumerate() {
            let depth = head.slot_depths[slot];
            let cross = if model.cross && slot > 0 {
                residuals.first().map(|slot0| slot0 as &Residuals)
            } else {
                None
            };
            residuals.push(decode_stream(
                streams[slot],
                active_widths,
                rows,
                model,
                depth,
                cross,
            )?);
        }
    }

    // Reconstruct, lane by lane.
    for lane in 0..active {
        let width = widths[lane];
        let lane_x = x_base + (lane * lane_width) as u32;
        if palette {
            let previous: Vec<u32> = vec![0; width];
            let mut previous = previous;
            for row in 0..rows {
                let y = stripe * head.stripe_rows + row as u32;
                let predictor = predictors[row];
                let mut current = vec![0u32; width];
                for x in 0..width {
                    let residual = (crate::bits::unzigzag(u64::from(residuals[0][lane][row][x]))
                        & 0xFF) as u32;
                    let predicted = if row == 0 {
                        let w = if x > 0 { current[x - 1] } else { 0 };
                        predict(0, w, 0, 0, 0, 0, 0, 0, 8)
                    } else {
                        let n = previous[x];
                        let w = if x > 0 { current[x - 1] } else { n };
                        let nw = if x > 0 { previous[x - 1] } else { n };
                        let ne = if x + 1 < width { previous[x + 1] } else { n };
                        predict(predictor, w, n, nw, ne, 0, 0, 0, 8)
                    };
                    let value = (predicted + residual) & 0xFF;
                    current[x] = value;
                    let entry = &entries[(value as usize).min(entries.len() - 1)];
                    write_pixel(image, lane_x + x as u32, y, header, head, entry);
                }
                previous = current;
            }
            continue;
        }
        // Non-palette.
        let mut previous: Vec<Vec<u32>> = head.slots.iter().map(|_| vec![0u32; width]).collect();
        for row in 0..rows {
            let y = stripe * head.stripe_rows + row as u32;
            let predictor = predictors[row];
            let mut current: Vec<Vec<u32>> = head.slots.iter().map(|_| vec![0u32; width]).collect();
            for x in 0..width {
                // Step 1: unzigzag residuals; step 2: undo decorrelation.
                let mut deltas: Vec<u32> = Vec::with_capacity(head.slots.len());
                for (slot, plane) in residuals.iter().enumerate() {
                    let depth = head.slot_depths[slot];
                    deltas.push(
                        (crate::bits::unzigzag(u64::from(plane[lane][row][x])) as u32)
                            & mask(depth),
                    );
                }
                if decor != 0 && decor != 4 {
                    undo_decorrelation(&mut deltas, head.slot_depths[0], decor);
                }
                let px = lane_x + x as u32;
                for slot in 0..head.slots.len() {
                    let bits = head.slot_depths[slot];
                    let (n, w, nw, ne) = if row > 0 {
                        let n = previous[slot][x];
                        let w = if x > 0 { current[slot][x - 1] } else { n };
                        let nw = if x > 0 { previous[slot][x - 1] } else { n };
                        let ne = if x + 1 < width {
                            previous[slot][x + 1]
                        } else {
                            n
                        };
                        (n, w, nw, ne)
                    } else {
                        let w = if x > 0 { current[slot][x - 1] } else { 0 };
                        (0, w, 0, 0)
                    };
                    let mut r_value = 0u32;
                    let mut rw = 0u32;
                    let mut rn = 0u32;
                    if predictor >= 7 {
                        let reference = reference.expect("inter predictor validated");
                        r_value = reference_residual(head, header, slot, reference, px, y)?;
                        rn = if row == 0 {
                            r_value
                        } else {
                            reference_residual(head, header, slot, reference, px, y - 1)?
                        };
                        rw = if x > 0 {
                            reference_residual(head, header, slot, reference, px - 1, y)?
                        } else {
                            rn
                        };
                    }
                    let predicted = predict(predictor, w, n, nw, ne, r_value, rw, rn, bits);
                    let bound = head.slot_bound(slot);
                    let value = if bound > 0 {
                        let q = sext(deltas[slot], bits);
                        let candidate = predicted as i64 + q * (2 * bound as i64 + 1);
                        candidate.clamp(0, mask(bits) as i64) as u32
                    } else {
                        predicted.wrapping_add(deltas[slot]) & mask(bits)
                    };
                    current[slot][x] = value;
                }
                // Write this position's samples.
                let mut values: Vec<u32> =
                    (0..head.slots.len()).map(|slot| current[slot][x]).collect();
                if let Some((gr, gb, rb)) = &adapt_weights {
                    let g = values[0];
                    let gs = g as i32 - 128;
                    let r = values[1].wrapping_add(((i32::from(gr[lane]) * gs) >> 5) as u32) & 0xFF;
                    let rs = r as i32 - 128;
                    let b = values[2].wrapping_add(
                        ((i32::from(gb[lane]) * gs + i32::from(rb[lane]) * rs) >> 5) as u32,
                    ) & 0xFF;
                    values[1] = r;
                    values[2] = b;
                }
                write_pixel(image, px, y, header, head, &values);
            }
            previous = current;
        }
    }
    Ok(())
}

/// SPEC 2.4.1: read `3 * active` signed adaptive color weights (LSB-first).
fn read_adaptive_weights(
    r: &mut Reader<'_>,
    active: usize,
) -> Result<([i16; 16], [i16; 16], [i16; 16])> {
    if !(1..=16).contains(&active) {
        return Err(Error::corrupt("bad adaptive color active lanes"));
    }
    let mut buf = 0u32;
    let mut nbits = 0u32;
    let mut get = |r: &mut Reader<'_>, n: u32| -> Result<u32> {
        while nbits < n {
            buf |= u32::from(r.u8("adaptive color weights")?) << nbits;
            nbits += 8;
        }
        let v = buf & ((1u32 << n) - 1);
        buf >>= n;
        nbits -= n;
        Ok(v)
    };
    let mut gr = [0i16; 16];
    let mut gb = [0i16; 16];
    let mut rb = [0i16; 16];
    let channels: [(&mut [i16; 16], i16); 3] = [(&mut gr, 32), (&mut gb, 16), (&mut rb, 16)];
    for (arr, pred0) in channels {
        for lane in 0..active {
            let pred = if lane == 0 { pred0 } else { arr[lane - 1] };
            let flag = get(r, 1)?;
            let w = if flag == 0 {
                let delta = get(r, 4)? as i16 - 8;
                let w = pred + delta;
                if !(-127..=127).contains(&w) {
                    return Err(Error::corrupt("adaptive color weight out of range"));
                }
                w
            } else {
                let raw = get(r, 8)? as u8 as i8 as i16;
                if raw == -128 {
                    return Err(Error::corrupt("adaptive color weight out of range"));
                }
                raw
            };
            arr[lane] = w;
        }
    }
    if nbits > 0 && buf != 0 {
        return Err(Error::corrupt("non-zero adaptive color padding bits"));
    }
    Ok((gr, gb, rb))
}

/// SPEC 2.2 and 4: only the first three color slots are transformed.
/// For wide samples those slots are HI words; every LO slot stays unchanged.
fn undo_decorrelation(deltas: &mut [u32], bits: u32, decor: u8) {
    let g = deltas[0];
    let r_res = deltas[1].wrapping_add(g) & mask(bits);
    let b_res = match decor {
        1 => deltas[2].wrapping_add(g) & mask(bits),
        2 => deltas[2].wrapping_add(r_res) & mask(bits),
        _ => {
            let mean = ((sext(r_res, bits) + sext(g, bits)) >> 1) as u32;
            deltas[2].wrapping_add(mean) & mask(bits)
        }
    };
    deltas[1] = r_res;
    deltas[2] = b_res;
}

/// The ten template pixels `(dy, dx)` of bilevel stripes; pixel `k` is
/// context bit `k` (SPEC 2.10).
const TEMPLATE: [(isize, isize); 10] = [
    (-2, -1),
    (-2, 0),
    (-2, 1),
    (-1, -2),
    (-1, -1),
    (-1, 0),
    (-1, 1),
    (-1, 2),
    (0, -2),
    (0, -1),
];

/// A bilevel stripe (0x81) or bilevel palette stripe (0x82), SPEC 2.10.
/// `geometry` is (the stripe's first image row, the group's first column,
/// the lane width).
#[allow(clippy::too_many_arguments)]
fn decode_bilevel(
    r: &mut Reader<'_>,
    decor: u8,
    head: &LosslessHead,
    header: &Header,
    geometry: (u32, u32, usize),
    rows: usize,
    widths: &[usize],
    image: &mut Image,
) -> Result<()> {
    let (y0, x0, lane_width) = geometry;
    let (inv, probs) = head
        .bilevel
        .as_ref()
        .ok_or_else(|| Error::corrupt("bilevel stripe without the bilevel table"))?;
    let mut entries: Vec<Vec<u32>> = Vec::new();
    if decor == 0x82 {
        if head.slot_depths.iter().any(|depth| *depth > 16) {
            return Err(Error::corrupt("bilevel palette with a deep slot"));
        }
        let count = r.u8("bilevel palette entries")? as usize + 1;
        if count > 2 {
            return Err(Error::corrupt("bilevel palette with more than two entries"));
        }
        for _ in 0..count {
            let mut entry = Vec::new();
            for depth in &head.slot_depths {
                let value = if *depth <= 8 {
                    u32::from(r.u8("palette entry")?)
                } else {
                    u32::from(u16::from_le_bytes(
                        r.take(2, "palette entry")?.try_into().unwrap(),
                    ))
                };
                if value >= (1u32 << *depth) {
                    return Err(Error::corrupt("palette entry out of range"));
                }
                entry.push(value);
            }
            entries.push(entry);
        }
    } else if head.slot_depths != [1] {
        return Err(Error::corrupt("bilevel stripe without one 1-bit slot"));
    }
    let stream = r.take(r.remaining(), "bilevel stream")?;
    if stream.len() < 4 * widths.len() {
        return Err(Error::corrupt("short rANS state list"));
    }
    let mut states: Vec<u32> = (0..widths.len())
        .map(|lane| u32::from_le_bytes(stream[4 * lane..4 * lane + 4].try_into().unwrap()))
        .collect();
    if states.iter().any(|state| *state < 1 << 16) {
        return Err(Error::corrupt("rANS state below 2^16"));
    }
    let mut pos = 4 * widths.len();
    let group_width: usize = widths.iter().sum();
    let mut t = vec![vec![0u32; group_width]; rows];
    for row in 0..rows {
        for x in 0..lane_width {
            for (lane, width) in widths.iter().enumerate() {
                if x >= *width {
                    continue;
                }
                let lane0 = (lane * lane_width) as isize;
                let column = lane0 + x as isize;
                let mut ctx = 0usize;
                for (k, (dy, dx)) in TEMPLATE.iter().enumerate() {
                    let (mut y, c) = (row as isize + dy, column + dx);
                    // Current-row pixels left of the lane read the row above.
                    if *dy == 0 && c < lane0 {
                        y -= 1;
                    }
                    if y >= 0 && c >= 0 && (c as usize) < group_width {
                        ctx |= (t[y as usize][c as usize] as usize) << k;
                    }
                }
                let p = probs[ctx];
                let state = states[lane];
                let slot = state % 1024;
                let one = slot >= p;
                let mut next = if one {
                    (1024 - p) * (state >> 10) + slot - p
                } else {
                    p * (state >> 10) + slot
                };
                if next < 1 << 16 {
                    if pos + 2 > stream.len() {
                        return Err(Error::corrupt("rANS word underrun"));
                    }
                    next =
                        next << 16 | u32::from(u16::from_le_bytes([stream[pos], stream[pos + 1]]));
                    pos += 2;
                }
                states[lane] = next;
                t[row][column as usize] = u32::from(one);
            }
        }
    }
    for (row, values) in t.iter().enumerate() {
        for (column, value) in values.iter().enumerate() {
            let v = value ^ u32::from(*inv);
            let (x, y) = (x0 + column as u32, y0 + row as u32);
            if decor == 0x82 {
                let entry = &entries[(v as usize).min(entries.len() - 1)];
                write_pixel(image, x, y, header, head, entry);
            } else {
                write_pixel(image, x, y, header, head, &[v]);
            }
        }
    }
    Ok(())
}

/// Write one output position from per-slot reconstructed coded values (or from
/// a palette entry, which is also in coded values).
fn write_pixel(
    image: &mut Image,
    x: u32,
    y: u32,
    header: &Header,
    head: &LosslessHead,
    slot_values: &[u32],
) {
    let channels = header.channels as usize;
    let coded_channels = header.coded_channels();
    let mut words: Vec<Option<u32>> = vec![None; coded_channels];
    for (slot, (channel, word)) in head.slots.iter().enumerate() {
        let coded = if *word == 0 {
            *channel
        } else {
            *channel + channels
        };
        words[coded] = Some(slot_values[slot]);
    }
    for channel in 0..channels {
        if header.sample.is_wide() {
            let hi_map = &head.maps[channel];
            let lo_map = &head.maps[channel + channels];
            let hi = hi_map.expand_sample(words[channel].unwrap_or(hi_map.value));
            let lo = lo_map.expand_sample(words[channel + channels].unwrap_or(lo_map.value));
            image.put_sample(x, y, channel, (u64::from(hi) << 32) | u64::from(lo));
        } else {
            let map = &head.maps[channel];
            let coded = words[channel].unwrap_or(map.value);
            image.put_sample(x, y, channel, u64::from(map.expand_sample(coded)));
        }
    }
}

/// `R = reduce(order(sample)) mod 2^n` for one slot (SPEC section 4).
fn reference_residual(
    head: &LosslessHead,
    header: &Header,
    slot: usize,
    reference: &dyn Reference,
    x: u32,
    y: u32,
) -> Result<u32> {
    let (channel, word) = head.slots[slot];
    let full = reference.sample(x, y, channel);
    let coded: u32 = if header.sample.is_wide() {
        if word == 0 {
            (full >> 32) as u32
        } else {
            full as u32
        }
    } else {
        full as u32
    };
    let map_index = if word == 0 {
        channel
    } else {
        channel + header.channels as usize
    };
    head.maps[map_index].reduce(coded)
}

#[cfg(test)]
mod wide_tests {
    use super::*;
    use crate::{
        header::{coded_channel_kind, parse_header},
        maps::Map,
    };

    #[test]
    fn wide_hi_decorrelation_preserves_lo_and_raw_sample_bits() {
        // G/R/B are ordered HI words. Mode 3's signed mean is floor(-1/2).
        let hi = [0x8000_0001u32, 0x7fff_fffe, 0xffff_fff0, 0x1122_3344];
        let lo = [0x5566_7788u32, 0x99aa_bbcc, 0xddee_ff00, 0x1357_9bdf];
        for sample in [8, 9, 10] {
            for channels in [3, 4] {
                let features = if sample == 9 { 12 } else { 8 };
                let header = parse_header(&[2, 14, features, sample, channels, 1, 1]).unwrap();
                let order = if channels == 3 {
                    vec![1, 0, 2]
                } else {
                    vec![1, 0, 2, 3]
                };
                let slots = order
                    .iter()
                    .map(|c| (*c, 0))
                    .chain(order.iter().map(|c| (*c, 1)))
                    .collect();
                let maps = (0..header.coded_channels())
                    .map(|coded| {
                        Map::new(
                            0,
                            32,
                            0,
                            32,
                            coded_channel_kind(header.sample, coded, channels),
                            header.features,
                        )
                        .unwrap()
                    })
                    .collect();
                let head = LosslessHead {
                    quick: false,
                    quick_bits: false,
                    lanes: 1,
                    stripe_rows: 1,
                    group_cols: 0,
                    palette_allowed: false,
                    slots,
                    slot_depths: vec![32; channels as usize * 2],
                    maps,
                    models: vec![],
                    palette_model: None,
                    bounds: None,
                    hint: None,
                    bilevel: None,
                    template: None,
                    unit_ordinals: false,
                    unit_count: 1,
                };
                for (decor, encoded_b) in [(1, 0x7fff_ffef), (2, 0x7fff_fff2), (3, 0xffff_fff1)] {
                    let mut deltas = hi[..channels as usize].to_vec();
                    deltas[1] = 0xffff_fffd;
                    deltas[2] = encoded_b;
                    deltas.extend_from_slice(&lo[..channels as usize]);
                    undo_decorrelation(&mut deltas, 32, decor);
                    assert_eq!(&deltas[..channels as usize], &hi[..channels as usize]);
                    assert_eq!(&deltas[channels as usize..], &lo[..channels as usize]);
                    let mut image = Image::new(1, 1, channels, channels, 8).unwrap();
                    write_pixel(&mut image, 0, 0, &header, &head, &deltas);
                    for (slot, channel) in order.iter().enumerate() {
                        let raw_hi = match sample {
                            9 => hi[slot] ^ 0x8000_0000,
                            10 => hi[slot] ^ ((hi[slot] >> 31) * 0x7fff_ffff),
                            _ => hi[slot],
                        };
                        let expected = (u64::from(raw_hi) << 32) | u64::from(lo[slot]);
                        assert_eq!(
                            image.get_sample(0, 0, *channel),
                            expected,
                            "sample{sample} channels{channels} decor{decor} channel{channel}"
                        );
                    }
                }
            }
        }
    }
}
