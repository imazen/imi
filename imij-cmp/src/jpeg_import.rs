//! Frozen study witnesses for the same pure Rust importer used by imij-jpeg.
#[path = "../../tools/imij-jpeg/src/importer.rs"]
mod adapter;
use crate::rd::Result;
pub(crate) use adapter::import;
use adapter::{coefficient_image, metadata};
use imij::jpeg::ComponentColor;
use sha2::{Digest, Sha256};
use std::{fmt::Write as _, fs, path::Path};
/// Manifest rows: source JPEG, preserved f-clamp RGB8 artifact, frozen index.
/// Missing witnesses fail; source/index/pixel identities are checked before use.
pub(crate) fn check(args: &[String]) -> Result<()> {
    if args.len() != 2 {
        return Err("jpeg-components-check MANIFEST.tsv OUT.tsv".into());
    }
    let mut report = String::from(
        "input_sha256\twidth\theight\tnative_components\toutput_sha256\tfrozen_f_clamp\tcarrier_rl_bytes\tcarrier_sh_bytes\tband8_rl_bytes\tband8_sh_bytes\tband256_rl_bytes\tband256_sh_bytes\tcoefficient_roundtrip\n",
    );
    let mut count = 0;
    for line in fs::read_to_string(&args[0])?
        .lines()
        .filter(|s| !s.is_empty())
    {
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != 3 {
            return Err("native component check manifest needs three columns".into());
        }
        let jpeg = fs::read(fields[0])?;
        let input_sha = format!("{:x}", Sha256::digest(&jpeg));
        let index_bytes = fs::read(fields[2])?;
        let index: serde_json::Value = serde_json::from_slice(&index_bytes)?;
        if index["input_sha256"] != input_sha || index["interpolation_profile"] != "coalesced-clamp"
        {
            return Err("native component check has another source/profile index".into());
        }
        let index_sha = format!("{:x}", Sha256::digest(&index_bytes));
        if Path::new(fields[2]).file_name().and_then(|p| p.to_str())
            != Some(&format!("{input_sha}_{index_sha}.json"))
        {
            return Err("native component index identity differs".into());
        }
        let expected = fs::read(fields[1])?;
        let expected_sha = format!("{:x}", Sha256::digest(&expected));
        let image = coefficient_image(&jpeg)?;
        let mut carrier_bytes = [0usize; 2];
        for (coder, entropy) in [imij::LossyEntropy::RunLevel, imij::LossyEntropy::SplitHuf]
            .into_iter()
            .enumerate()
        {
            for component in &image.components {
                let p = component.info;
                let table = image.tables[p.quant_table as usize].as_ref().unwrap();
                let width = p.blocks_wide.checked_mul(8).ok_or("coded width overflow")?;
                let height = p
                    .blocks_high
                    .checked_mul(8)
                    .ok_or("coded height overflow")?;
                let file = imij::lab::encode_jpeg_component(
                    width,
                    height,
                    table,
                    &component.coefficients,
                    entropy,
                    7.5,
                )?;
                let back = imij::lab::decode_jpeg_component(&file, width, height, table)?;
                if back != component.coefficients {
                    return Err("native coefficient entropy roundtrip differs".into());
                }
                carrier_bytes[coder] += file.len();
            }
        }
        let info = image.image_info();
        let mut pixels = vec![0xA5; info.checked_buffer_len().ok_or("image too large")?];
        image.reconstruct_into(&mut pixels, info.row_bytes())?;
        let mut wire_bytes = [0usize; 4];
        for (cell, (rows, entropy)) in [
            (8, imij::LossyEntropy::RunLevel),
            (8, imij::LossyEntropy::SplitHuf),
            (256, imij::LossyEntropy::RunLevel),
            (256, imij::LossyEntropy::SplitHuf),
        ]
        .into_iter()
        .enumerate()
        {
            let wire = image.encode(&metadata(&jpeg)?, rows, entropy, 7.5)?;
            if imij::jpeg::decode_coefficients(&wire)? != image {
                return Err("native band wire changed coefficients/tables".into());
            }
            if imij::decode(&wire)?.1 != pixels {
                return Err("native band wire differs from frozen reconstruction".into());
            }
            wire_bytes[cell] = wire.len();
        }
        let rgb: Vec<_> = if image.color == ComponentColor::Gray {
            pixels.iter().flat_map(|&v| [v; 3]).collect()
        } else {
            pixels
        };
        let pixel_name = format!("{expected_sha}_{}_{}.rgb8", image.width, image.height);
        if index["arms"]["f-clamp"] != pixel_name
            || Path::new(fields[1]).file_name().and_then(|p| p.to_str()) != Some(&pixel_name)
            || index["width"] != image.width
            || index["height"] != image.height
        {
            return Err("native component frozen pixel identity/geometry differs".into());
        }
        if rgb != expected {
            let different = rgb.iter().zip(&expected).filter(|(a, b)| a != b).count();
            return Err(format!("native reconstruction differs from frozen f-clamp: {input_sha}, {different} samples").into());
        }
        // The strided route must also preserve padding on the same source.
        let stride = info.row_bytes() + 7;
        let mut strided = vec![0xA5; stride * info.height as usize];
        image.reconstruct_into(&mut strided, stride)?;
        for (y, row) in strided.chunks_exact(stride).enumerate() {
            let target = &rgb[y * image.width as usize * 3..(y + 1) * image.width as usize * 3];
            if image.color == ComponentColor::Gray {
                if row[..info.row_bytes()]
                    .iter()
                    .zip(target.as_chunks::<3>().0.iter())
                    .any(|(&a, b)| a != b[0])
                {
                    return Err("strided gray reconstruction differs".into());
                }
            } else if &row[..info.row_bytes()] != target {
                return Err("strided color reconstruction differs".into());
            }
            if row[info.row_bytes()..].iter().any(|&v| v != 0xA5) {
                return Err("strided native reconstruction changed padding".into());
            }
        }
        writeln!(
            report,
            "{input_sha}\t{}\t{}\t{}\t{expected_sha}\tok\t{}\t{}\t{}\t{}\t{}\t{}\tok",
            image.width,
            image.height,
            image.components.len(),
            carrier_bytes[0],
            carrier_bytes[1],
            wire_bytes[0],
            wire_bytes[1],
            wire_bytes[2],
            wire_bytes[3],
        )?;
        fs::write(&args[1], &report)?;
        count += 1;
    }
    if count == 0 {
        return Err("native component check manifest is empty".into());
    }
    eprintln!("native reconstruction matches frozen f-clamp for {count} sources");
    Ok(())
}
