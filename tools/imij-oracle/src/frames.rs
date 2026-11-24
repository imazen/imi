//! Complete-file decoding: DATA plus frames and pages (SPEC sections 1.3, 2).

use crate::container::{SIGNATURE, resume_chunks, skip_record_list, walk_chunks};
use crate::error::{Error, Result};
use crate::header::{Header, check_features, parse_header};
use crate::image::{CanvasRef, Image, Reference};
use crate::payload::{parse_lossless_head, walk_units};
use crate::stripe::decode_unit;

/// Decode a complete file. `reference` is the previous canvas for an inter
/// frame (its origin is applied by the caller's [`Reference`]).
pub(crate) fn decode_file(
    data: &[u8],
    reference: Option<&dyn Reference>,
) -> Result<(Vec<Image>, Header)> {
    if data.len() < 8 || data[..8] != SIGNATURE {
        return Err(Error::corrupt("bad signature"));
    }
    let mut r = crate::bits::Reader::new(data);
    r.pos = 8;
    let header_len = r.varint("header length")?;
    let header_start = r.pos;
    if header_start + header_len as usize > data.len() {
        return Err(Error::corrupt("truncated header"));
    }
    let header = parse_header(&data[header_start..header_start + header_len as usize])?;
    check_features(&header)?;

    let mut packets = walk_chunks(data, header_start + header_len as usize)?;
    let (data_start, data_end) = packets
        .data
        .ok_or_else(|| Error::corrupt("no DATA chunk"))?;
    let primary = match data_end {
        Some(end) => decode_lossless_payload(data, data_start, Some(end), &header, reference)?,
        None => {
            // Open DATA: the head and unit records are self-delimiting, so
            // walking them finds the chunk end.
            let (head, units_off) = parse_lossless_head(data, data_start, &header)?;
            let spans = walk_units(data, &head, header.version, units_off, data.len())?;
            let end = spans.last().map(|span| span.end).unwrap_or(units_off);
            let image = decode_units(data, &head, &header, &spans, reference)?;
            resume_chunks(data, end, &mut packets)?;
            image
        }
    };

    let mut outputs = vec![primary];
    if let Some(anim) = packets.anim {
        outputs = decode_frames(&header, outputs, anim, &packets.frames)?;
    }
    Ok((outputs, header))
}

fn decode_lossless_payload(
    data: &[u8],
    start: usize,
    end: Option<usize>,
    header: &Header,
    reference: Option<&dyn Reference>,
) -> Result<Image> {
    let (head, units_off) = parse_lossless_head(data, start, header)?;
    let limit = end.unwrap_or(data.len());
    let spans = walk_units(data, &head, header.version, units_off, limit)?;
    if let Some(end) = end {
        let reached = spans.last().map(|span| span.end).unwrap_or(units_off);
        if reached != end {
            return Err(Error::corrupt("bytes after the last record"));
        }
    }
    decode_units(data, &head, header, &spans, reference)
}

fn decode_units(
    data: &[u8],
    head: &crate::payload::LosslessHead,
    header: &Header,
    spans: &[crate::payload::UnitSpan],
    reference: Option<&dyn Reference>,
) -> Result<Image> {
    let mut image = Image::new(
        header.width,
        header.height,
        header.channels,
        header.channel_layout,
        header.sample_bytes(),
    )?;
    let groups = if head.group_cols > 0 {
        header.width.div_ceil(head.group_cols) as u64
    } else {
        1
    };
    for (index, span) in spans.iter().enumerate() {
        let stripe = (index as u64 / groups) as u32;
        let group = (index as u64 % groups) as u32;
        decode_unit(
            data,
            span.payload_start,
            span.payload_len,
            head,
            header,
            stripe,
            group,
            &mut image,
            reference,
        )?;
    }
    Ok(image)
}

fn decode_frames(
    outer: &Header,
    mut outputs: Vec<Image>,
    anim: &[u8],
    frame_bytes: &[&[u8]],
) -> Result<Vec<Image>> {
    let mut r = crate::bits::Reader::new(anim);
    let kind = r.u8("aNIM kind")?;
    let _loop_count = r.varint("aNIM loop count")?;
    let frame_count = r.varint("aNIM frame count")?;
    let _num = r.varint("aNIM duration numerator")?;
    let den = r.varint("aNIM duration denominator")?;
    if den == 0 {
        return Err(Error::corrupt("zero duration denominator"));
    }
    let presentation = if kind == 4 {
        if outer.features & (1 << crate::registry::FEATURE_SEQUENCE_ITEMS) == 0 {
            return Err(Error::corrupt("unified sequence without feature"));
        }
        let value = r.u8("sequence presentation")?;
        if value > 1 {
            return Err(Error::unsupported("sequence presentation"));
        }
        value
    } else {
        0
    };
    if r.pos != anim.len() {
        return Err(Error::corrupt("aNIM trailing bytes"));
    }
    if kind == 0 || kind > 4 {
        return Err(Error::unsupported(format!("aNIM kind {kind}")));
    }
    if kind == 3 && outer.features >> crate::registry::FEATURE_MIXED_PAGES & 1 == 0 {
        return Err(Error::corrupt("mixed pages without its feature"));
    }
    if frame_count != frame_bytes.len() as u64 + 1 {
        return Err(Error::corrupt("frame count mismatch"));
    }
    let mut canvas = if kind == 1 {
        Some(outputs[0].clone())
    } else {
        None
    };
    for frame in frame_bytes {
        let mut r = crate::bits::Reader::new(frame);
        let x0 = r.varint("frame x0")?;
        let y0 = r.varint("frame y0")?;
        let _num = r.varint("frame duration numerator")?;
        let den = r.varint("frame duration denominator")?;
        if den == 0 {
            return Err(Error::corrupt("zero duration denominator"));
        }
        let presence = r.varint("frame presence")?;
        if presence >> 3 != 0 {
            return Err(Error::unsupported("frame presence bits"));
        }
        let mut selected_reference = None;
        if presence & 4 != 0 {
            let records_len = r.varint("frame records length")?;
            let records_end = r.pos + records_len as usize;
            if records_end > frame.len() {
                return Err(Error::corrupt("truncated frame record list"));
            }
            if kind == 4 {
                let mut records = crate::bits::Reader::new(&frame[r.pos..records_end]);
                let mut last = None;
                while records.remaining() != 0 {
                    let id = records.varint("frame record id")?;
                    if last.is_some_and(|v| v >= id) {
                        return Err(Error::corrupt("frame record order"));
                    }
                    last = Some(id);
                    let len = usize::try_from(records.varint("frame record length")?)
                        .map_err(|_| Error::corrupt("frame record size"))?;
                    if len > records.remaining() {
                        return Err(Error::corrupt("frame record size"));
                    }
                    let payload = records.take(len, "frame record")?;
                    let id = crate::registry::frame_id(outer.version, id);
                    if id == crate::registry::FRAME_REFERENCE {
                        let mut value = crate::bits::Reader::new(payload);
                        let index = value.varint("reference ordinal")?;
                        // A digest algorithm id and its width of bytes (SPEC
                        // 1.4.2), or eight bare XXH3-64 bytes under version 2.
                        if outer.version == 2 && value.remaining() == 8 {
                            value.take(8, "reference hash")?;
                        } else {
                            let width = match value.u8("reference digest algorithm")? {
                                0 => 8,
                                1 => 16,
                                other => {
                                    return Err(Error::unsupported(format!(
                                        "reference digest algorithm {other}"
                                    )));
                                }
                            };
                            value.take(width, "reference hash")?;
                        }
                        // Like ancillary cSUM/pSUM, hash verification is outside
                        // this independent pixel reconstruction oracle's scope.
                        if value.remaining() != 0 {
                            return Err(Error::corrupt("frame reference length"));
                        }
                        selected_reference = Some(index);
                    } else if id & 1 != 0 {
                        return Err(Error::unsupported("frame record"));
                    }
                }
            } else {
                skip_record_list(&frame[r.pos..records_end])?;
            }
            r.pos = records_end;
        }
        let image_bytes = &frame[r.pos..];
        if kind == 4 {
            if x0 != 0 || y0 != 0 || presence & 3 != 0 {
                return Err(Error::corrupt("unified item controls"));
            }
            let nested = nested_header(image_bytes)?;
            ensure_mask_covers(outer, &nested)?;
            if presentation == 1
                && (nested.width != outer.width
                    || nested.height != outer.height
                    || nested.channel_layout != outer.channel_layout
                    || nested.sample_code != outer.sample_code)
            {
                return Err(Error::corrupt("animation canvas changes"));
            }
            let reference = if let Some(index) = selected_reference {
                if index >= outputs.len() as u64 {
                    return Err(Error::corrupt("reference is not an earlier image"));
                }
                let image = &outputs[index as usize];
                if image.width != nested.width
                    || image.height != nested.height
                    || image.channel_layout != nested.channel_layout
                    || image.sample_bytes != nested.sample_bytes()
                {
                    return Err(Error::corrupt("reference pixel description differs"));
                }
                Some(image as &dyn Reference)
            } else {
                None
            };
            let (images, _) = decode_file(image_bytes, reference)?;
            if images.len() != 1 {
                return Err(Error::corrupt("nested sequence item"));
            }
            outputs.extend(images);
            continue;
        }
        if kind == 2 || kind == 3 {
            if presence & 3 != 0 {
                return Err(Error::corrupt("page frame with presence bits"));
            }
            if x0 != 0 || y0 != 0 {
                return Err(Error::corrupt("page frame with an origin"));
            }
            if image_bytes.is_empty() {
                return Err(Error::corrupt("page frame without an image"));
            }
            // Parse the nested header here to check the feature-mask rule
            // before decoding (SPEC 0.1).
            let nested_header = nested_header(image_bytes)?;
            ensure_mask_covers(outer, &nested_header)?;
            if kind == 2
                && (nested_header.channel_layout != outer.channel_layout
                    || nested_header.sample_code != outer.sample_code)
            {
                return Err(Error::corrupt("page channels or sample type differ"));
            }
            let (images, _) = decode_file(image_bytes, None)?;
            if images.len() != 1 {
                return Err(Error::corrupt("nested page document"));
            }
            outputs.extend(images);
            continue;
        }
        // Animation.
        let canvas_image = canvas.as_ref().expect("kind 1 builds a canvas");
        if presence & 2 != 0 {
            if presence & 1 == 0 {
                return Err(Error::corrupt("empty frame without the previous-frame bit"));
            }
            if !image_bytes.is_empty() {
                return Err(Error::corrupt("empty frame with an image"));
            }
            outputs.push(canvas_image.clone());
            continue;
        }
        if image_bytes.is_empty() {
            return Err(Error::corrupt("frame without an image"));
        }
        let uses_previous = presence & 1 != 0;
        let nested_header = nested_header(image_bytes)?;
        ensure_mask_covers(outer, &nested_header)?;
        if nested_header.channel_layout != outer.channel_layout
            || nested_header.sample_code != outer.sample_code
        {
            return Err(Error::corrupt("frame channels or sample type differ"));
        }
        // Validate the rectangle before decoding: the reference view indexes
        // the canvas with it.
        let canvas_size = (canvas_image.width, canvas_image.height);
        if x0 > u64::from(u32::MAX) || y0 > u64::from(u32::MAX) {
            return Err(Error::corrupt("frame origin is not a u32"));
        }
        if uses_previous {
            if x0 + u64::from(nested_header.width) > u64::from(canvas_size.0)
                || y0 + u64::from(nested_header.height) > u64::from(canvas_size.1)
            {
                return Err(Error::corrupt("frame rectangle outside the canvas"));
            }
        } else if x0 != 0
            || y0 != 0
            || nested_header.width != canvas_size.0
            || nested_header.height != canvas_size.1
        {
            return Err(Error::corrupt("intra frame does not cover the canvas"));
        }
        let canvas_ref = CanvasRef::new(canvas_image, x0 as u32, y0 as u32);
        let reference: Option<&dyn Reference> = if uses_previous {
            Some(&canvas_ref as &dyn Reference)
        } else {
            None
        };
        let (images, _frame_header) = decode_file(image_bytes, reference)?;
        let canvas_image = canvas.as_mut().expect("kind 1 builds a canvas");
        if uses_previous {
            canvas_image.blit(&images[0], x0 as u32, y0 as u32);
        } else {
            canvas_image.buf.copy_from_slice(&images[0].buf);
        }
        outputs.push(canvas_image.clone());
    }
    Ok(outputs)
}

fn nested_header(image_bytes: &[u8]) -> Result<Header> {
    if image_bytes.len() < 8 || image_bytes[..8] != SIGNATURE {
        return Err(Error::corrupt("bad nested signature"));
    }
    let mut r = crate::bits::Reader::new(image_bytes);
    r.pos = 8;
    let header_len = r.varint("nested header length")?;
    let start = r.pos;
    if start + header_len as usize > image_bytes.len() {
        return Err(Error::corrupt("truncated nested header"));
    }
    parse_header(&image_bytes[start..start + header_len as usize])
}

/// The outer header's feature set covers the frame's or page's: its mask
/// and what its fields imply (SPEC section 0.1).
fn ensure_mask_covers(outer: &Header, nested: &Header) -> Result<()> {
    if nested.set & !outer.set != 0 {
        return Err(Error::corrupt(
            "a frame or page needs a feature the outer header lacks",
        ));
    }
    Ok(())
}
