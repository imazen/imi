//! Test envelopes over the production packet core. No implementation source copied.
#![forbid(unsafe_code)]
extern crate alloc;
use alloc::vec::Vec;
// This standalone envelope uses a subset of the shared production entry points.
#[allow(
    dead_code,
    reason = "Byte-plane entry points are used by production, not this packet-only envelope"
)]
#[path = "../../../imij/src/quick/pixels.rs"]
mod pixels;
pub use pixels::Shape;
type Result<T> = core::result::Result<T, &'static str>;
const END: [u8; 8] = [0, 0, 0, 0, 0, 0, 0, 1];
/// `qoi_colorspace`: Some(0/1) writes standard QOI; None writes experimental IQP1.
/// The extension is color-agnostic. Its channel order and raw sample bits are unchanged.
pub fn encode(
    data: &[u8],
    stride: usize,
    shape: Shape,
    qoi_colorspace: Option<u8>,
    mut checkpoint: impl FnMut() -> Result<()>,
) -> Result<Vec<u8>> {
    shape.check(data.len(), stride)?;
    if let Some(c) = qoi_colorspace
        && (c > 1 || shape.bytes != 1 || !(3..=4).contains(&shape.channels))
    {
        return Err("QOI format");
    }
    let pixels = shape.width.checked_mul(shape.height).ok_or("pixels")?;
    let limit = pixels
        .checked_mul(shape.channels * shape.bytes + 1)
        .and_then(|n| n.checked_add(24))
        .ok_or("encoded size")?;
    checkpoint()?;
    let mut out = Vec::new();
    out.try_reserve_exact(limit).map_err(|_| "allocation")?;
    if let Some(c) = qoi_colorspace {
        out.extend_from_slice(b"qoif");
        out.extend_from_slice(&(shape.width as u32).to_be_bytes());
        out.extend_from_slice(&(shape.height as u32).to_be_bytes());
        out.extend_from_slice(&[shape.channels as u8, c]);
    } else {
        out.extend_from_slice(b"IQP1");
        out.extend_from_slice(&(shape.width as u32).to_le_bytes());
        out.extend_from_slice(&(shape.height as u32).to_le_bytes());
        out.extend_from_slice(&[shape.channels as u8, shape.bytes as u8, 0, 0]);
    }
    pixels::encode_into(data, stride, shape, &mut out, &mut checkpoint)?;
    out.extend_from_slice(&END);
    Ok(out)
}

pub fn header(data: &[u8]) -> Result<(Shape, usize)> {
    let h = data.get(..14).ok_or("header")?;
    let (width, height, bytes, start) = match &h[..4] {
        b"qoif" if (3..=4).contains(&h[12]) && h[13] <= 1 => (
            u32::from_be_bytes(h[4..8].try_into().unwrap()),
            u32::from_be_bytes(h[8..12].try_into().unwrap()),
            1,
            14,
        ),
        b"IQP1" if data.get(14..16) == Some(&[0, 0]) => (
            u32::from_le_bytes(h[4..8].try_into().unwrap()),
            u32::from_le_bytes(h[8..12].try_into().unwrap()),
            h[13] as usize,
            16,
        ),
        _ => return Err("header"),
    };
    let shape = Shape {
        width: width as usize,
        height: height as usize,
        channels: h[12] as usize,
        bytes,
    };
    shape.row()?;
    Ok((shape, start))
}

/// No raster allocation; padding is untouched. Errors may follow partial output.
/// Caller supplies and can bound storage before parsing any packet.
pub fn decode_into(
    data: &[u8],
    output: &mut [u8],
    stride: usize,
    mut checkpoint: impl FnMut() -> Result<()>,
) -> Result<Shape> {
    let (shape, start) = header(data)?;
    shape.check(output.len(), stride)?;
    let end = data.len().checked_sub(8).ok_or("footer")?;
    if data.get(end..) != Some(&END) {
        return Err("footer");
    }
    let payload = data.get(start..end).ok_or("payload bounds")?;
    let mut decoder = pixels::Decoder::new(shape)?;
    let row = shape.row()?;
    for y in 0..shape.height {
        checkpoint()?;
        decoder.next(payload, &mut output[y * stride..y * stride + row])?;
    }
    checkpoint()?;
    Ok(shape)
}
