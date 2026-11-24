//! Source JPEG parsing and metadata extraction use zenjpeg and kamadak-exif.
//! IMI owns only the native coefficient representation and reconstruction.
#![forbid(unsafe_code)]
use imij::jpeg::{
    ChromaSiting, CoefficientImage, ComponentColor, ComponentInfo, QuantizedComponent,
};
use std::{fs, path::Path};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub(crate) const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];
pub(crate) fn coefficient_image(data: &[u8]) -> Result<CoefficientImage> {
    let source = zenjpeg::decoder::DecodeConfig::new().read_info(data)?;
    use zenjpeg::decoder::{ColorSpace, JpegMode, Subsampling};
    if source.precision != 8
        || source.is_xyb
        || (source.color_space == ColorSpace::YCbCr
            && !matches!(source.subsampling, Subsampling::S444 | Subsampling::S420))
        || !matches!(
            source.mode,
            JpegMode::Baseline | JpegMode::Extended | JpegMode::Progressive
        )
        || !matches!(
            source.color_space,
            ColorSpace::Grayscale | ColorSpace::YCbCr
        )
    {
        return Err(
            "JPEG requires pixel-lossless fallback (native import: 8-bit Gray/YCbCr Huffman)"
                .into(),
        );
    }
    let decoded =
        zenjpeg::decoder::DecodeConfig::new().decode_coefficients(data, enough::Unstoppable)?;
    let mut tables = [None; 4];
    if decoded.quant_tables.len() > tables.len() {
        return Err("too many quantizer slots".into());
    }
    tables[..decoded.quant_tables.len()].copy_from_slice(&decoded.quant_tables);
    let components = decoded
        .components
        .iter()
        .map(|p| -> Result<_> {
            let mut coefficients = vec![0i16; p.coeffs.len()];
            for (source, destination) in p
                .coeffs
                .as_chunks::<64>()
                .0
                .iter()
                .zip(coefficients.as_chunks_mut::<64>().0.iter_mut())
            {
                for (zigzag, &value) in source.iter().enumerate() {
                    destination[ZIGZAG[zigzag]] = value;
                }
            }
            Ok(QuantizedComponent {
                info: ComponentInfo {
                    id: p.id,
                    horizontal_sampling: p.h_samp,
                    vertical_sampling: p.v_samp,
                    quant_table: p.quant_table_idx,
                    blocks_wide: p.blocks_wide.try_into()?,
                    blocks_high: p.blocks_high.try_into()?,
                },
                coefficients,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let siting = source_siting(source.exif.as_deref())?;
    let image = CoefficientImage {
        width: decoded.width,
        height: decoded.height,
        color: if source.color_space == ColorSpace::Grayscale {
            ComponentColor::Gray
        } else {
            ComponentColor::YCbCr
        },
        siting,
        tables,
        components,
    };
    image.validate()?;
    Ok(image)
}

fn source_siting(raw: Option<&[u8]>) -> Result<ChromaSiting> {
    let Some(raw) = raw else {
        return Ok(ChromaSiting::Unspecified);
    };
    let tiff = raw.strip_prefix(b"Exif\0\0").unwrap_or(raw);
    let metadata = exif::Reader::new().read_raw(tiff.to_vec())?;
    match metadata
        .get_field(exif::Tag::YCbCrPositioning, exif::In::PRIMARY)
        .map(|f| f.value.get_uint(0))
    {
        None => Ok(ChromaSiting::Unspecified),
        Some(Some(1)) => Ok(ChromaSiting::Centered),
        Some(Some(2)) => Ok(ChromaSiting::Cosited),
        _ => Err("unsupported EXIF YCbCrPositioning".into()),
    }
}

pub(crate) fn metadata(data: &[u8]) -> Result<imij::Metadata> {
    let source = zenjpeg::decoder::DecodeConfig::new().read_info(data)?;
    let orientation = source
        .exif
        .as_deref()
        .and_then(zenjpeg::lossless::parse_exif_orientation)
        .unwrap_or(1);
    Ok(imij::Metadata {
        icc: source.icc_profile,
        exif: source
            .exif
            .map(|v| v.strip_prefix(b"Exif\0\0").unwrap_or(&v).to_vec()),
        xmp: source.xmp.map(String::into_bytes),
        orientation,
        ..imij::Metadata::default()
    })
}

/// Native coefficient import when supported, otherwise the established RGB8
/// pixel-lossless fallback. The fallback is decoded and verified before write.
pub(crate) fn import(args: &[String]) -> Result<()> {
    let Some((flag, args)) = args.split_first() else {
        return Err("native JPEG storage is experimental; pass --experimental-native explicitly (decode is currently slower than sequential JPEG with restart markers)".into());
    };
    if flag != "--experimental-native" {
        return Err("native JPEG storage is experimental; pass --experimental-native explicitly (decode is currently slower than sequential JPEG with restart markers)".into());
    }
    if !(2..=4).contains(&args.len()) {
        return Err(
            "jpeg-import --experimental-native INPUT.jpg OUTPUT.imij [MCU_ROWS=8] [rl|sh]".into(),
        );
    }
    let data = fs::read(&args[0])?;
    let rows = args
        .get(2)
        .map(|v| v.parse::<u16>())
        .transpose()?
        .unwrap_or(8);
    let entropy = match args.get(3).map(String::as_str).unwrap_or("rl") {
        "rl" => imij::LossyEntropy::RunLevel,
        "sh" => imij::LossyEntropy::SplitHuf,
        _ => return Err("entropy must be rl or sh".into()),
    };
    let meta = metadata(&data)?;
    let (file, route) = match coefficient_image(&data) {
        Ok(image)
            if image.siting != ChromaSiting::Cosited
                || image.color == ComponentColor::Gray
                || image.components[0].info.horizontal_sampling
                    == image.components[1].info.horizontal_sampling =>
        {
            let file = image.encode(&meta, rows, entropy, 7.5)?;
            if imij::jpeg::decode_coefficients(&file)? != image {
                return Err("native importer changed coefficients or tables".into());
            }
            let info = image.image_info();
            let mut expected = vec![0; info.checked_buffer_len().ok_or("image too large")?];
            image.reconstruct_into(&mut expected, info.row_bytes())?;
            if imij::decode(&file)?.1 != expected {
                return Err("native importer reconstruction differs".into());
            }
            (file, "native coefficient bands")
        }
        native => {
            eprintln!(
                "pixel-lossless fallback: {}",
                match native {
                    Err(e) => e.to_string(),
                    Ok(_) => "co-sited 420 render is unsupported".into(),
                }
            );
            let mut cfg = zenjpeg::decoder::DecodeConfig::new();
            cfg.output_format = Some(zenjpeg::decoder::PixelFormat::Rgb);
            let decoded = cfg.decode(&data, enough::Unstoppable)?;
            let pixels = decoded.pixels_u8().ok_or("fallback has no RGB8 pixels")?;
            let info = imij::ImageInfo::new(
                decoded.width,
                decoded.height,
                imij::Channels::Rgb,
                imij::SampleType::U8,
            );
            let file = imij::ImiEncoderConfig::default()
                .encode_samples(imij::ImageRef::new(pixels, info.row_bytes(), &info), &meta)?;
            if imij::decode(&file)? != (info, pixels.to_vec()) {
                return Err("JPEG fallback lossless roundtrip differs".into());
            }
            (file, "pixel-lossless fallback")
        }
    };
    if Path::new(&args[1]).exists() {
        return Err("output exists; choose a fresh path to preserve it".into());
    }
    use std::io::Write as _;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[1])?
        .write_all(&file)?;
    eprintln!("{route}: {} bytes", file.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_import_requires_opt_in_before_input_access() {
        let args = ["missing.jpg".into(), "never-written.imij".into()];
        assert!(
            import(&args)
                .unwrap_err()
                .to_string()
                .contains("pass --experimental-native")
        );
        assert!(
            import(&[])
                .unwrap_err()
                .to_string()
                .contains("pass --experimental-native")
        );
        let opted_in = ["--experimental-native".into()];
        assert!(
            import(&opted_in)
                .unwrap_err()
                .to_string()
                .starts_with("jpeg-import --experimental-native INPUT")
        );
    }
    fn tiff(position: u16) -> Vec<u8> {
        let mut data = b"II\x2a\0\x08\0\0\0".to_vec();
        data.extend_from_slice(&2u16.to_le_bytes());
        for (tag, value) in [(0x0112u16, 6u16), (0x0213, position)] {
            data.extend_from_slice(&tag.to_le_bytes());
            data.extend_from_slice(&3u16.to_le_bytes());
            data.extend_from_slice(&1u32.to_le_bytes());
            data.extend_from_slice(&value.to_le_bytes());
            data.extend_from_slice(&0u16.to_le_bytes());
        }
        data.extend_from_slice(&0u32.to_le_bytes());
        data
    }
    #[test]
    fn metadata_api_keeps_orientation_and_source_positioning() {
        for (value, siting) in [(1, ChromaSiting::Centered), (2, ChromaSiting::Cosited)] {
            let tiff = tiff(value);
            assert_eq!(source_siting(Some(&tiff)).unwrap(), siting);
            let mut app = b"Exif\0\0".to_vec();
            app.extend_from_slice(&tiff);
            assert_eq!(source_siting(Some(&app)).unwrap(), siting);
            assert_eq!(zenjpeg::lossless::parse_exif_orientation(&app), Some(6));
        }
        assert_eq!(source_siting(None).unwrap(), ChromaSiting::Unspecified);
        assert!(source_siting(Some(&tiff(3))).is_err());
        assert!(source_siting(Some(b"broken")).is_err());
    }
}
