//! Shared browser/WASI code-value preview. This does not apply CICP/ICC color
//! transforms or HDR tone mapping. Native color models and premultiplied alpha
//! require a converter; raw sample decoding remains available for those images.
//! Integers take their upper non-sign bits; floats clamp to [0, 1].

fn int_max(s: imij::SampleType) -> u64 {
    use imij::SampleType as S;
    match s {
        S::U8 | S::I8 => 0xFF,
        S::U16 | S::I16 => 0xFFFF,
        S::U32 | S::I32 => 0xFFFF_FFFF,
        S::U64 | S::I64 => u64::MAX,
        S::F16 | S::F32 | S::F64 => 0,
        s => unreachable!("no range for {s:?}"),
    }
}

fn read_code(row: &[u8], at: usize, s: imij::SampleType) -> u64 {
    match s.bytes() {
        1 => row[at] as u64,
        2 => u16::from_ne_bytes(row[at..at + 2].try_into().unwrap()) as u64,
        4 => u32::from_ne_bytes(row[at..at + 4].try_into().unwrap()) as u64,
        _ => u64::from_ne_bytes(row[at..at + 8].try_into().unwrap()),
    }
}

fn f16_to_f32(bits: u16) -> f32 {
    let sign = (bits >> 15) & 1;
    let exp = ((bits >> 10) & 0x1f) as i32;
    let man = (bits & 0x3ff) as f32;
    let v = match exp {
        0 => man * 5.960_464_5e-8,
        31 => {
            if man == 0.0 {
                f32::INFINITY
            } else {
                f32::NAN
            }
        }
        e => (1.0 + man / 1024.0) * 2f32.powi(e - 15),
    };
    match sign {
        1 => -v,
        _ => v,
    }
}

fn code_to_f32(v: u64, s: imij::SampleType) -> f32 {
    use imij::SampleType as S;
    match s {
        S::F16 => f16_to_f32(v as u16),
        S::F32 => f32::from_bits(v as u32),
        S::F64 => f64::from_bits(v) as f32,
        _ => v as f32 / int_max(s) as f32,
    }
}

pub(crate) fn to_rgba8(info: &imij::ImageInfo, pixels: &[u8]) -> Result<Vec<u8>, imij::Error> {
    if matches!(
        info.channels,
        imij::Channels::Cmyk | imij::Channels::Oklab | imij::Channels::OklabAlpha
    ) {
        return Err(imij::Error::Unsupported(
            "native color preview requires a color transform",
        ));
    }
    if info.channels.has_alpha() && info.alpha_mode == imij::AlphaMode::Premultiplied {
        return Err(imij::Error::Unsupported(
            "premultiplied preview requires an alpha transform",
        ));
    }
    use imij::SampleType as S;
    let (w, h) = (info.width as usize, info.height as usize);
    let (nc, sb) = (info.channels.count(), info.sample.bytes());
    let size = w
        .checked_mul(h)
        .and_then(|n| n.checked_mul(4))
        .ok_or(imij::Error::TooLarge)?;
    if size as u64 > imij::Limits::DEFAULT.max_bytes {
        return Err(imij::Error::TooLarge);
    }
    if pixels.len() != info.checked_buffer_len().ok_or(imij::Error::TooLarge)? {
        return Err(imij::Error::Corrupt("preview raster length"));
    }
    let mut out = Vec::new();
    out.try_reserve_exact(size)
        .map_err(|_| imij::Error::TooLarge)?;
    out.resize(size, 0);
    // Decoded RGB8 is already in preview code values. Keep sample-type and
    // channel dispatch outside the pixel loop on the common browser path.
    if info.sample == S::U8 && info.channels == imij::Channels::Rgb {
        garb::bytes::rgb_to_rgba(pixels, &mut out)
            .map_err(|_| imij::Error::Corrupt("preview raster length"))?;
        return Ok(out);
    }
    let tone = |v: u64| -> u8 {
        match info.sample {
            S::U8 => v as u8,
            S::U16 => (v >> 8) as u8,
            S::F16 | S::F32 | S::F64 => {
                (code_to_f32(v, info.sample).clamp(0.0, 1.0) * 255.0).round_ties_even() as u8
            }
            S::U32 => (v >> 24) as u8,
            S::U64 => (v >> 56) as u8,
            S::I8 => ((v as u8 as i8).max(0) as u8) << 1,
            S::I16 => ((v as u16 as i16).max(0) as u16 >> 7) as u8,
            S::I32 => ((v as u32 as i32).max(0) as u32 >> 23) as u8,
            S::I64 => ((v as i64).max(0) as u64 >> 55) as u8,
            s => unreachable!("no tone map for {s:?}"),
        }
    };
    for y in 0..h {
        for x in 0..w {
            let base = (y * w + x) * nc * sb;
            let ch = |c: usize| -> u64 {
                if c < nc {
                    read_code(&pixels[base..], c * sb, info.sample)
                } else {
                    0
                }
            };
            let (r, g, b, a) = match nc {
                1 => (ch(0), ch(0), ch(0), int_max(info.sample)),
                2 => (ch(0), ch(0), ch(0), ch(1)),
                3 => (ch(0), ch(1), ch(2), int_max(info.sample)),
                _ => (ch(0), ch(1), ch(2), ch(3)),
            };
            let o = (y * w + x) * 4;
            out[o] = tone(r);
            out[o + 1] = tone(g);
            out[o + 2] = tone(b);
            out[o + 3] = if !info.channels.has_alpha()
                || matches!(
                    info.alpha_mode,
                    imij::AlphaMode::Opaque | imij::AlphaMode::Undefined
                ) {
                255
            } else {
                tone(a)
            };
        }
    }
    Ok(out)
}

#[cfg(test)]
mod preview_tests {
    use super::*;

    #[test]
    fn rgb8_preview_preserves_every_code_and_row_boundary() {
        for width in [1, 2, 7, 8, 15, 16, 17, 255, 256, 257] {
            let info = imij::ImageInfo::new(width, 3, imij::Channels::Rgb, imij::SampleType::U8);
            let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
                .map(|i| i as u8)
                .collect();
            let rgba = to_rgba8(&info, &pixels).unwrap();
            assert_eq!(rgba.len(), width as usize * 3 * 4);
            let recovered: Vec<u8> = rgba
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| p[..3].iter().copied())
                .collect();
            assert_eq!(recovered, pixels);
            assert!(rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
            assert!(matches!(
                to_rgba8(&info, &pixels[..pixels.len() - 1]),
                Err(imij::Error::Corrupt("preview raster length"))
            ));
        }
    }

    #[test]
    fn native_models_are_not_mislabeled_as_rgb_or_alpha() {
        for channels in [
            imij::Channels::Cmyk,
            imij::Channels::Oklab,
            imij::Channels::OklabAlpha,
        ] {
            let info = imij::ImageInfo::new(1, 1, channels, imij::SampleType::U8);
            assert!(matches!(
                to_rgba8(&info, &[0; 4]),
                Err(imij::Error::Unsupported(_))
            ));
        }
        let info = imij::ImageInfo::new(1, 1, imij::Channels::Rgb, imij::SampleType::U8);
        assert_eq!(to_rgba8(&info, &[7, 21, 99]).unwrap(), [7, 21, 99, 255]);
    }

    #[test]
    fn signed_range_and_alpha_interpretation() {
        use imij::{AlphaMode, Channels, ImageInfo, SampleType};
        let info = ImageInfo::new(3, 1, Channels::Gray, SampleType::I32);
        let raw: Vec<u8> = [i32::MIN, 0, i32::MAX]
            .into_iter()
            .flat_map(i32::to_ne_bytes)
            .collect();
        assert_eq!(
            to_rgba8(&info, &raw).unwrap(),
            [0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 255]
        );
        let mut info = ImageInfo::new(1, 1, Channels::Rgba, SampleType::U8);
        for mode in [AlphaMode::Opaque, AlphaMode::Undefined] {
            info.alpha_mode = mode;
            assert_eq!(to_rgba8(&info, &[7, 21, 99, 0]).unwrap(), [7, 21, 99, 255]);
        }
        info.alpha_mode = AlphaMode::Straight;
        assert_eq!(to_rgba8(&info, &[7, 21, 99, 0]).unwrap(), [7, 21, 99, 0]);
        info.alpha_mode = AlphaMode::Premultiplied;
        assert!(matches!(
            to_rgba8(&info, &[7, 21, 99, 128]),
            Err(imij::Error::Unsupported(_))
        ));
    }
}
