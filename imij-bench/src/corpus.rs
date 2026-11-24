//! Corpus loading and synthetic variants.

use std::path::Path;

use imij::{Channels, Cicp, ImageInfo, SampleType};

pub struct Img {
    pub class: String,
    pub name: String,
    pub info: ImageInfo,
    pub pixels: Vec<u8>,
    /// Size of the source PNG file (for reference).
    pub png_bytes: usize,
}

impl Img {
    pub fn raw_bytes(&self) -> usize {
        self.pixels.len()
    }
    pub fn pixels_count(&self) -> usize {
        self.info.width as usize * self.info.height as usize
    }
}

pub fn load_png(class: &str, path: &Path) -> Result<Img, String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let out = zenpng::decode(
        &data,
        &zenpng::PngDecodeConfig::default(),
        &enough::Unstoppable,
    )
    .map_err(|e| format!("{}: {e:?}", path.display()))?;
    let w = out.info.width;
    let h = out.info.height;
    let bytes = out.pixels.copy_to_contiguous_bytes();
    let depth16 = out.info.bit_depth == 16;
    let bpp = bytes.len() / (w as usize * h as usize);
    let nch = if depth16 { bpp / 2 } else { bpp };
    let channels = match nch {
        1 => Channels::Gray,
        2 => Channels::GrayAlpha,
        3 => Channels::Rgb,
        4 => Channels::Rgba,
        _ => return Err(format!("{}: {nch} channels", path.display())),
    };
    let sample = if depth16 {
        SampleType::U16
    } else {
        SampleType::U8
    };
    let mut info = ImageInfo::new(w, h, channels, sample);
    if let Some(c) = out.info.cicp {
        info.cicp = Cicp {
            color_primaries: c.color_primaries,
            transfer_characteristics: c.transfer_characteristics,
            matrix_coefficients: c.matrix_coefficients,
            full_range: c.full_range,
        };
    }
    Ok(Img {
        class: class.to_string(),
        name: path.file_stem().unwrap().to_string_lossy().into_owned(),
        info,
        pixels: bytes,
        png_bytes: data.len(),
    })
}

pub fn load_manifest(path: &str, filter: Option<&str>) -> Vec<(String, String)> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let mut it = l.split('\t');
            (
                it.next().unwrap().to_string(),
                it.next().unwrap().to_string(),
            )
        })
        .filter(|(c, _)| filter.is_none_or(|f| f.split(',').any(|x| x == c)))
        .collect()
}

fn u16s(px: &[u8]) -> impl Iterator<Item = u16> + '_ {
    px.as_chunks::<2>()
        .0
        .iter()
        .copied()
        .map(u16::from_ne_bytes)
}

fn from_u16s(v: impl Iterator<Item = u16>) -> Vec<u8> {
    v.flat_map(|x| x.to_ne_bytes()).collect()
}

/// SMPTE ST 2084 EOTF: PQ code value in [0,1] -> linear in [0,1] (1 = 10000 nits).
fn pq_eotf(e: f32) -> f32 {
    let m1 = 0.159_301_76f32;
    let m2 = 78.843_75f32;
    let c1 = 0.835_937_5f32;
    let c2 = 18.851_563f32;
    let c3 = 18.6875f32;
    let p = e.max(0.0).powf(1.0 / m2);
    ((p - c1).max(0.0) / (c2 - c3 * p)).powf(1.0 / m1)
}

fn f32_to_f16(v: f32) -> u16 {
    // Round-to-nearest-even conversion for finite, non-negative-ish values.
    let b = v.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xFF) as i32;
    let man = b & 0x7F_FFFF;
    if exp == 0xFF {
        return sign | 0x7C00 | if man != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 31 {
        return sign | 0x7C00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = man | 0x80_0000;
        let shift = (14 - e) as u32;
        let half = 1u32 << (shift - 1);
        let rem = m & ((1 << shift) - 1);
        let mut r = m >> shift;
        if rem > half || (rem == half && r & 1 == 1) {
            r += 1;
        }
        return sign | r as u16;
    }
    let rem = man & 0x1FFF;
    let mut r = ((e as u32) << 10) | (man >> 13);
    if rem > 0x1000 || (rem == 0x1000 && r & 1 == 1) {
        r += 1;
    }
    sign | r as u16
}

/// Derived test variants for 16-bit/HDR/alpha behavior.
pub fn variants(img: &Img) -> Vec<Img> {
    let mut out = Vec::new();
    let mk = |suffix: &str, class: &str, info: ImageInfo, pixels: Vec<u8>| Img {
        class: class.to_string(),
        name: format!("{}{}", img.name, suffix),
        info,
        pixels,
        png_bytes: 0,
    };
    match img.info.sample {
        SampleType::U16 if img.info.channels == Channels::Rgb => {
            // 10/12-bit in 16-bit containers (from the top bits of the source).
            let shift10 = from_u16s(u16s(&img.pixels).map(|v| (v >> 6) << 6));
            out.push(mk(".pq10msb", "hdr10", img.info.clone(), shift10));
            let lsb10 = from_u16s(u16s(&img.pixels).map(|v| v >> 6));
            out.push(mk(".pq10lsb", "hdr10", img.info.clone(), lsb10));
            let rep12 = from_u16s(u16s(&img.pixels).map(|v| {
                let s = v >> 4;
                (s << 4) | (s >> 8)
            }));
            out.push(mk(".pq12rep", "hdr12", img.info.clone(), rep12));
            // Linear-light f16 (PQ decoded, 1.0 = 10000 nits scaled to 1.0 = 203 nits).
            let lin = from_u16s(
                u16s(&img.pixels)
                    .map(|v| f32_to_f16(pq_eotf(v as f32 / 65535.0) * (10000.0 / 203.0))),
            );
            let mut fi = img.info.clone();
            fi.sample = SampleType::F16;
            fi.cicp = Cicp {
                color_primaries: 1,
                transfer_characteristics: 8,
                matrix_coefficients: 0,
                full_range: true,
            };
            out.push(mk(".linf16", "hdrf16", fi, lin));
            if std::env::var_os("IMIJ_EXT32").is_some() {
                let mut i32i = img.info.clone();
                i32i.sample = SampleType::F32;
                let linf32: Vec<u8> = u16s(&img.pixels)
                    .flat_map(|v| {
                        (pq_eotf(v as f32 / 65535.0) * (10000.0 / 203.0))
                            .to_bits()
                            .to_ne_bytes()
                    })
                    .collect();
                out.push(mk(".linf32", "ext_hdr_f32lin", i32i.clone(), linf32));
                let un16: Vec<u8> = u16s(&img.pixels)
                    .flat_map(|v| (v as f32 / 65535.0).to_bits().to_ne_bytes())
                    .collect();
                out.push(mk(".f32unorm16", "ext_hdr_f32unorm16", i32i.clone(), un16));
                i32i.sample = SampleType::U32;
                let sh: Vec<u8> = u16s(&img.pixels)
                    .flat_map(|v| ((v as u32) << 16).to_ne_bytes())
                    .collect();
                out.push(mk(".u32shl16", "ext_hdr_u32shl16", i32i, sh));
            }
        }
        SampleType::U8
            if img.info.channels == Channels::Rgb && std::env::var_os("IMIJ_EXT32").is_some() =>
        {
            let mut fi = img.info.clone();
            fi.sample = SampleType::F32;
            let f: Vec<u8> = img
                .pixels
                .iter()
                .flat_map(|&v| (v as f32 / 255.0).to_bits().to_ne_bytes())
                .collect();
            out.push(mk(".f32unorm8", "ext_photo_f32unorm8", fi.clone(), f));
            fi.sample = SampleType::U32;
            let u: Vec<u8> = img
                .pixels
                .iter()
                .flat_map(|&v| (v as u32 * 0x0101_0101).to_ne_bytes())
                .collect();
            out.push(mk(".u32rep8", "ext_photo_u32rep8", fi, u));
        }
        SampleType::U8 if img.info.channels == Channels::Rgba => {
            let mut fi = img.info.clone();
            fi.sample = SampleType::U16;
            let p16 = from_u16s(img.pixels.iter().map(|&v| v as u16 * 257));
            out.push(mk(".x257", "alpha16", fi, p16));
        }
        _ => {}
    }
    out
}
