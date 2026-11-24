//! Narrow, untimed import for the predeclared OpenJPH JPEG-origin supplements.
//! No resize, rotation, deblocking, HDR reconstruction or profile conversion.
//! Requires ICC-free RGB8 JPEG and explicit EXIF ColorSpace=1. Original JPEG
//! bytes, full decoded RGB8 and exact center crop are preserved independently.
#![forbid(unsafe_code)]

use enough::Unstoppable;
use serde_json::json;
use std::{fs, path::Path};
use zenjpeg::decoder::{Decoder, PixelFormat};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
struct Admission {
    width: u32,
    height: u32,
    orientation: Option<u32>,
}

fn take<'a>(bytes: &'a [u8], pos: &mut usize, n: usize) -> Result<&'a [u8]> {
    let end = pos.checked_add(n).ok_or("input offset overflow")?;
    let value = bytes.get(*pos..end).ok_or("truncated metadata")?;
    *pos = end;
    Ok(value)
}
fn number(bytes: &[u8], at: usize, n: usize, little: bool) -> Result<u32> {
    let mut pos = at;
    let b = take(bytes, &mut pos, n)?;
    Ok(match (n, little) {
        (2, true) => u16::from_le_bytes(b.try_into()?) as u32,
        (2, false) => u16::from_be_bytes(b.try_into()?) as u32,
        (4, true) => u32::from_le_bytes(b.try_into()?),
        (4, false) => u32::from_be_bytes(b.try_into()?),
        _ => return Err("metadata scalar width".into()),
    })
}
fn field(tiff: &[u8], offset: usize, key: u32, little: bool) -> Result<Option<u32>> {
    let count = number(tiff, offset, 2, little)? as usize;
    if count > 512 {
        return Err("too many TIFF entries".into());
    }
    let size = count
        .checked_mul(12)
        .and_then(|n| n.checked_add(6))
        .ok_or("TIFF table overflow")?;
    if offset.checked_add(size).is_none_or(|n| n > tiff.len()) {
        return Err("short TIFF table".into());
    }
    let mut found = None;
    for i in 0..count {
        let at = offset + 2 + i * 12;
        if number(tiff, at, 2, little)? != key {
            continue;
        }
        if found.is_some() || number(tiff, at + 4, 4, little)? != 1 {
            return Err("duplicate/non-scalar TIFF declaration".into());
        }
        let kind = number(tiff, at + 2, 2, little)?;
        let n = if key == 0x8769 {
            if kind != 4 {
                return Err("Exif IFD pointer type".into());
            }
            4
        } else {
            if kind != 3 {
                return Err("Exif declaration type".into());
            }
            2
        };
        found = Some(number(tiff, at + 8, n, little)?);
    }
    Ok(found)
}
fn exif(bytes: &[u8]) -> Result<Option<u32>> {
    let tiff = bytes.strip_prefix(b"Exif\0\0").ok_or("Exif signature")?;
    let little = match tiff.get(..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return Err("TIFF byte order".into()),
    };
    if number(tiff, 2, 2, little)? != 42 {
        return Err("TIFF magic".into());
    }
    let offset = number(tiff, 4, 4, little)? as usize;
    if offset < 8 {
        return Err("TIFF header overlap".into());
    }
    let orientation = field(tiff, offset, 0x112, little)?;
    if orientation.is_some_and(|n| !(1..=8).contains(&n)) {
        return Err("Exif orientation".into());
    }
    let sub = field(tiff, offset, 0x8769, little)?.ok_or("missing Exif IFD")? as usize;
    if sub < 8 || field(tiff, sub, 0xa001, little)? != Some(1) {
        return Err("explicit EXIF sRGB required".into());
    }
    Ok(orientation)
}
fn admission(bytes: &[u8]) -> Result<Admission> {
    if bytes.len() > 64 << 20 {
        return Err("encoded source exceeds 64MiB".into());
    }
    let mut pos = 0;
    if take(bytes, &mut pos, 2)? != b"\xff\xd8" {
        return Err("JPEG SOI".into());
    }
    let mut dimensions = None;
    let mut orientation = None;
    let mut has_exif = false;
    let mut scan = false;
    loop {
        let marker = loop {
            if scan {
                let skip = bytes
                    .get(pos..)
                    .ok_or("JPEG scan offset")?
                    .iter()
                    .position(|&b| b == 255)
                    .ok_or("unterminated JPEG scan")?;
                pos += skip + 1;
            } else if take(bytes, &mut pos, 1)? != [255] {
                return Err("JPEG marker prefix".into());
            }
            let mut m = take(bytes, &mut pos, 1)?[0];
            while m == 255 {
                m = take(bytes, &mut pos, 1)?[0];
            }
            if scan && (m == 0 || (0xd0..=0xd7).contains(&m)) {
                continue;
            }
            break m;
        };
        scan = false;
        if marker == 0xd9 {
            if pos != bytes.len() {
                return Err("trailing JPEG/auxiliary bytes not admitted".into());
            }
            break;
        }
        if marker == 0 || (0xd0..=0xd8).contains(&marker) {
            return Err("unexpected pre-scan JPEG marker".into());
        }
        let n = u16::from_be_bytes(take(bytes, &mut pos, 2)?.try_into()?) as usize;
        let payload = take(
            bytes,
            &mut pos,
            n.checked_sub(2).ok_or("JPEG segment length")?,
        )?;
        match marker {
            0xda => {
                scan = true;
            }
            0xe1 if payload.starts_with(b"Exif\0\0") => {
                if has_exif {
                    return Err("duplicate EXIF".into());
                }
                has_exif = true;
                orientation = exif(payload)?;
            }
            0xe1 if payload.starts_with(b"http://ns.adobe.com/xap/1.0/\0") => {
                let text = std::str::from_utf8(&payload[29..])?.to_ascii_lowercase();
                if [
                    "cicp",
                    "hdrgm:",
                    "hdrgainmap",
                    "gainmap",
                    "display p3",
                    "adobe rgb",
                ]
                .iter()
                .any(|key| text.contains(key))
                {
                    return Err("unsupported XMP color/HDR declaration".into());
                }
            }
            0xe0 if payload.starts_with(b"JFIF\0") => {}
            0xee if payload.starts_with(b"Adobe") => {}
            0xe0..=0xef => return Err("ICC/CICP/unknown APP metadata not admitted".into()),
            0xc0 | 0xc2 => {
                if dimensions.is_some() || payload.len() < 6 || payload[0] != 8 || payload[5] != 3 {
                    return Err("source must be one RGB8 JPEG frame".into());
                }
                let h = u16::from_be_bytes(payload[1..3].try_into()?) as u32;
                let w = u16::from_be_bytes(payload[3..5].try_into()?) as u32;
                if w == 0 || h == 0 || u64::from(w) * u64::from(h) > 50_000_000 {
                    return Err("source geometry limit".into());
                }
                dimensions = Some((w, h));
            }
            0xc1 | 0xc3 | 0xc5..=0xcb | 0xcd..=0xcf => {
                return Err("unsupported JPEG frame coding".into());
            }
            _ => {}
        }
    }
    if !has_exif {
        return Err("explicit EXIF sRGB required".into());
    }
    let (width, height) = dimensions.ok_or("missing JPEG frame")?;
    Ok(Admission {
        width,
        height,
        orientation,
    })
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: admit_photos INPUT.jpg NEW_OUTDIR".into());
    }
    let input = Path::new(&args[0]);
    let output = Path::new(&args[1]);
    if output.exists() {
        return Err("admission output already exists".into());
    }
    if fs::metadata(input)?.len() > 64 << 20 {
        return Err("encoded source exceeds 64MiB".into());
    }
    let jpeg = fs::read(input)?;
    let spec = admission(&jpeg)?;
    let decoded = Decoder::new()
        .auto_orient(false)
        .output_format(PixelFormat::Rgb)
        .max_pixels(50_000_000)
        .max_memory(512 << 20)
        .decode(&jpeg, Unstoppable)?;
    if (decoded.width, decoded.height) != (spec.width, spec.height)
        || decoded.format != PixelFormat::Rgb
    {
        return Err("stored-raster geometry/type changed".into());
    }
    let full = decoded.pixels_u8().ok_or("decoder did not return RGB8")?;
    let row = spec.width as usize * 3;
    if full.len() != row * spec.height as usize {
        return Err("decoded RGB length".into());
    }
    let (w, h) = (spec.width.min(1024), spec.height.min(1024));
    let (x, y) = ((spec.width - w) / 2, (spec.height - h) / 2);
    let mut raw = Vec::new();
    raw.try_reserve_exact(w as usize * h as usize * 3)?;
    for iy in y..y + h {
        let start = iy as usize * row + x as usize * 3;
        raw.extend_from_slice(&full[start..start + w as usize * 3]);
    }
    let pixels: Vec<rgb::Rgb<u8>> = raw
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| rgb::Rgb {
            r: p[0],
            g: p[1],
            b: p[2],
        })
        .collect();
    let image = imgref::Img::new(pixels.as_slice(), w as usize, h as usize);
    let png = zenpng::encode_rgb8(
        image,
        None,
        &zenpng::EncodeConfig::default(),
        &Unstoppable,
        &Unstoppable,
    )?;
    let round = zenpng::decode(&png, &zenpng::PngDecodeConfig::default(), &Unstoppable)?;
    if round.pixels.copy_to_contiguous_bytes() != raw
        || round.info.width != w
        || round.info.height != h
    {
        return Err("PNG changed admitted pixels".into());
    }
    fs::create_dir_all(output)?;
    fs::write(output.join("decoded-full.rgb8"), full)?;
    fs::write(output.join("source.rgb8"), &raw)?;
    fs::write(output.join("source.png"), png)?;
    let entry = json!({"original_jpeg":input,"source_width":spec.width,"source_height":spec.height,
        "exif_color_space":1,"exif_orientation":spec.orientation,"orientation_policy":"preserve stored raster; no rotation",
        "icc":"absent; all ICC/CICP/unknown APP payloads rejected","source_color":"explicit EXIF sRGB",
        "decoder":"zenjpeg local frozen dependency; default IDCT/upsampling, deblock off, no ICC or HDR conversion",
        "resize":false,"crop":{"x":x,"y":y,"width":w,"height":h},"width":w,"height":h,"stride_bytes":w*3,
        "full_rgb8":output.join("decoded-full.rgb8"),"source":output.join("source.rgb8"),"png":output.join("source.png"),
        "pixel_parity":"zenpng encoded then decoded source.png equals source.rgb8 exactly; crop copied from full RGB8"});
    fs::write(
        output.join("admission.json"),
        serde_json::to_vec_pretty(&entry)?,
    )?;
    println!("{}", serde_json::to_string(&entry)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_missing_metadata_and_bounded_tiff_offsets() {
        assert!(admission(b"\xff\xd8\xff\xda").is_err());
        assert!(exif(b"Exif\0\0II\x2a\0\xff\xff\xff\xff").is_err());
        assert!(exif(b"Exif\0\0MM\0\x2a\0\0\0\x08\xff\xff").is_err());
        assert!(admission(b"\xff\xd8\xff\xe2\0\x04xx\xff\xda").is_err());
    }
}
