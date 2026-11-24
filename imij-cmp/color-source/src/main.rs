//! Replay the reviewed 8029 ICC matrix/TRC conversion to clipped sRGB.
#![forbid(unsafe_code)]
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};
use zenpixels_convert::{ColorPrimaries, ColorPrimariesExt, TransferFunction, TransferFunctionExt};
const REVIEWED_ICC_SHA256: &str =
    "b5ff90cbab6d61fe6b6fe7bc9be835bd92838c02754ed606af7814353f801903";
fn hash(p: &[u8]) -> String {
    format!("{:x}", Sha256::digest(p))
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        return Err("imij-color-source CORPUS_ENTRY_JSON NEW_OUTDIR".into());
    }
    let entry: Value = serde_json::from_slice(&fs::read(&args[1])?)?;
    let output = Path::new(&args[2]);
    if output.exists() {
        return Err("output exists".into());
    }
    let png = fs::read(entry["path"].as_str().ok_or("path absent")?)?;
    if entry["sha256"].as_str() != Some(hash(&png).as_str()) {
        return Err("PNG hash differs".into());
    }
    let decoded = zenpng::decode(
        &png,
        &zenpng::PngDecodeConfig::default(),
        &enough::Unstoppable,
    )?;
    let icc = decoded.info.icc_profile.as_ref().ok_or("ICC absent")?;
    fs::write(Path::new(&args[2]).with_extension("icc"), icc)?;
    let adapter = reviewed_adapter(icc)?;
    let identification = "exact reviewed 8029 ICC hash; RGB/XYZ matrix-shaper near Display P3, three sRGB parametric curves, no LUT/chad/unknown tags; exact source ICC coefficients used";
    let original = decoded.pixels.copy_to_contiguous_bytes();
    let sw = decoded.info.width;
    let sh = decoded.info.height;
    if decoded.info.bit_depth != 8 || original.len() != sw as usize * sh as usize * 3 {
        return Err("requires RGB8".into());
    }
    let coord = |k: &str| -> Result<u32, Box<dyn std::error::Error>> {
        Ok(u32::try_from(
            entry["crop"][k].as_u64().ok_or("crop absent")?,
        )?)
    };
    let (x, y, w, h) = (coord("x")?, coord("y")?, coord("width")?, coord("height")?);
    if x.checked_add(w).is_none_or(|v| v > sw)
        || y.checked_add(h).is_none_or(|v| v > sh)
        || w == 0
        || h == 0
    {
        return Err("invalid crop".into());
    }
    let mut corrected = vec![0u8; w as usize * h as usize * 3];
    let mut input = Vec::with_capacity(corrected.len());
    let mut clipped_channels = 0usize;
    for row in 0..h {
        let off = ((row + y) as usize * sw as usize + x as usize) * 3;
        let src = &original[off..off + w as usize * 3];
        input.extend_from_slice(src);
        let dst =
            &mut corrected[row as usize * w as usize * 3..(row + 1) as usize * w as usize * 3];
        for (source, target) in src
            .as_chunks::<3>()
            .0
            .iter()
            .zip(dst.as_chunks_mut::<3>().0)
        {
            let (pixel, clips) = convert_pixel(*source, &adapter);
            *target = pixel;
            clipped_channels += clips;
        }
    }
    let rgb = corrected
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| rgb::RGB8::new(c[0], c[1], c[2]))
        .collect::<Vec<_>>();
    let encoded = zenpng::encode_rgb8(
        imgref::Img::new(rgb, w as usize, h as usize).as_ref(),
        None,
        &zenpng::EncodeConfig::default(),
        &enough::Unstoppable,
        &enough::Unstoppable,
    )?;
    let roundtrip = zenpng::decode(
        &encoded,
        &zenpng::PngDecodeConfig::default(),
        &enough::Unstoppable,
    )?;
    if roundtrip.pixels.copy_to_contiguous_bytes() != corrected
        || roundtrip.info.icc_profile.is_some()
    {
        return Err("converted PNG roundtrip/metadata differs".into());
    }
    fs::create_dir_all(output)?;
    fs::write(output.join("original-p3-crop.rgb8"), &input)?;
    fs::write(output.join("source-srgb.rgb8"), &corrected)?;
    let png_path = output.join("8029-p3-to-srgb-clipped.png");
    fs::write(&png_path, &encoded)?;
    let result = json!({"class":entry["class"],"family":"8029-p3-to-srgb-clipped","original_family":entry["family"],"path":png_path,"sha256":hash(&encoded),"source_width":w,"source_height":h,"crop":{"x":0,"y":0,"width":w,"height":h},"original_entry":entry,"original_icc_sha256":hash(icc),"original_crop_sha256":hash(&input),"corrected_rgb_sha256":hash(&corrected),"conversion":{"source":"Display P3 D65 with sRGB transfer","target":"sRGB BT.709 D65 with sRGB transfer","icc_identification":identification,"implementation":"explicit ICC matrix/TRC adapter; zenpixels-convert gamut and precise transfer routines", "clipped_channel_samples":clipped_channels,"icc_max_named_colorant_deviation":adapter.max_deviation,"clip_out_of_gamut":true,"quantization":"RGB8","resize":false,"order":"exact crop then per-pixel conversion"}});
    fs::write(
        output.join("entry.json"),
        serde_json::to_vec_pretty(&result)?,
    )?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

/// Structural sanity check for this fixture, not a generic CMS admission rule.
/// A .002 colorant tolerance identifies near-P3; conversion uses exact ICC
/// coefficients. The public replay path additionally requires the reviewed hash.
struct IccAdapter {
    matrix: [[f32; 3]; 3],
    curves: [[f64; 7]; 3],
    max_deviation: f64,
}
fn reviewed_adapter(data: &[u8]) -> Result<IccAdapter, Box<dyn std::error::Error>> {
    if hash(data) != REVIEWED_ICC_SHA256 {
        return Err("unreviewed ICC hash; this utility only replays the 8029 conversion".into());
    }
    validate_p3_matrix_trc(data)
}
fn convert_pixel(source: [u8; 3], adapter: &IccAdapter) -> ([u8; 3], usize) {
    let mut linear = [0f32; 3];
    for c in 0..3 {
        let x = f64::from(source[c]) / 255.0;
        let p = adapter.curves[c];
        linear[c] = if x >= p[4] {
            (p[1] * x + p[2]).powf(p[0]) + p[5]
        } else {
            p[3] * x + p[6]
        } as f32;
    }
    zenpixels_convert::apply_matrix_f32(&mut linear, &adapter.matrix);
    let clips = linear.iter().filter(|x| !(0.0..=1.0).contains(*x)).count();
    let pixel = linear.map(|x| {
        (TransferFunction::Srgb.delinearize(x.clamp(0.0, 1.0)) * 255.0)
            .round()
            .clamp(0.0, 255.0) as u8
    });
    (pixel, clips)
}
fn validate_p3_matrix_trc(data: &[u8]) -> Result<IccAdapter, Box<dyn std::error::Error>> {
    let fail = || "ICC does not prove Display P3/sRGB matrix+TRC";
    let u32be = |o: usize| -> Option<u32> {
        Some(u32::from_be_bytes(data.get(o..o + 4)?.try_into().ok()?))
    };
    if data.get(16..24) != Some(b"RGB XYZ ".as_slice()) {
        return Err(fail().into());
    }
    if data.get(36..40) != Some(b"acsp".as_slice()) || u32be(0) != Some(data.len() as u32) {
        return Err(fail().into());
    }
    let n = u32be(128).ok_or_else(fail)? as usize;
    let mut tags = std::collections::HashMap::new();
    for i in 0..n {
        let o = 132 + i * 12;
        let signature = data.get(o..o + 4).ok_or_else(fail)?;
        let start = u32be(o + 4).ok_or_else(fail)? as usize;
        let len = u32be(o + 8).ok_or_else(fail)? as usize;
        let end = start.checked_add(len).ok_or_else(fail)?;
        if tags
            .insert(signature, data.get(start..end).ok_or_else(fail)?)
            .is_some()
        {
            return Err("duplicate ICC tag".into());
        }
    }
    if tags.keys().any(|k| {
        k.starts_with(b"A2B")
            || k.starts_with(b"B2A")
            || k.starts_with(b"D2B")
            || k.starts_with(b"B2D")
    }) {
        return Err(fail().into());
    }
    if tags.keys().any(|k| {
        ![
            b"desc", b"cprt", b"wtpt", b"rXYZ", b"gXYZ", b"bXYZ", b"rTRC", b"gTRC", b"bTRC",
        ]
        .iter()
        .any(|allowed| k == &allowed.as_slice())
    }) {
        return Err("ICC chad or unknown tag requires independent CMS review".into());
    }
    let fixed = |b: &[u8], o: usize| -> Result<f64, Box<dyn std::error::Error>> {
        Ok(i32::from_be_bytes(b.get(o..o + 4).ok_or_else(fail)?.try_into()?) as f64 / 65536.0)
    };
    let wtpt = *tags.get(b"wtpt".as_slice()).ok_or_else(fail)?;
    let white = [fixed(wtpt, 8)?, fixed(wtpt, 12)?, fixed(wtpt, 16)?];
    let d50 = [0.9642, 1.0, 0.8249];
    if white
        .iter()
        .zip(d50)
        .any(|(a, b)| (a - b).abs() > 2.0 / 65536.0)
    {
        return Err(fail().into());
    }
    let bradford = [
        [0.8951, 0.2664, -0.1614],
        [-0.7502, 1.7135, 0.0367],
        [0.0389, -0.0685, 1.0296],
    ];
    let inverse = [
        [0.9869929, -0.1470543, 0.1599627],
        [0.4323053, 0.5183603, 0.0492912],
        [-0.0085287, 0.0400428, 0.9684867],
    ];
    let d65 = [0.9504559270516716, 1.0, 1.0890577507598784];
    let mul = |m: &[[f64; 3]; 3], v: [f64; 3]| {
        m.map(|row| row.iter().zip(v).map(|(a, b)| a * b).sum::<f64>())
    };
    let a = mul(&bradford, d65);
    let b = mul(&bradford, white);
    let p3 = ColorPrimaries::DisplayP3.to_xyz_matrix().ok_or_else(fail)?;
    let mut colorants = [[0f64; 3]; 3];
    let mut max_deviation = 0f64;
    for (c, key) in [b"rXYZ", b"gXYZ", b"bXYZ"].into_iter().enumerate() {
        let tag = *tags.get(key.as_slice()).ok_or_else(fail)?;
        if tag.get(..4) != Some(b"XYZ ".as_slice()) {
            return Err(fail().into());
        }
        let native = [
            f64::from(p3[0][c]),
            f64::from(p3[1][c]),
            f64::from(p3[2][c]),
        ];
        let cone = mul(&bradford, native);
        let expected = mul(
            &inverse,
            [
                cone[0] * b[0] / a[0],
                cone[1] * b[1] / a[1],
                cone[2] * b[2] / a[2],
            ],
        );
        for (r, want) in expected.into_iter().enumerate() {
            colorants[r][c] = fixed(tag, 8 + r * 4)?;
            let difference = (colorants[r][c] - want).abs();
            max_deviation = max_deviation.max(difference);
            if difference > 0.002 {
                return Err(format!("ICC is not near Display P3 {c}/{r}").into());
            }
        }
    }
    let mut curves = [[0f64; 7]; 3];
    for (c, key) in [b"rTRC", b"gTRC", b"bTRC"].into_iter().enumerate() {
        let tag = *tags.get(key.as_slice()).ok_or_else(fail)?;
        if tag.get(..4) != Some(b"para".as_slice()) {
            return Err(fail().into());
        }
        let kind = u16::from_be_bytes(tag.get(8..10).ok_or_else(fail)?.try_into()?);
        if !matches!(kind, 3 | 4) {
            return Err(fail().into());
        }
        let expected = [
            2.4,
            1.0 / 1.055,
            0.055 / 1.055,
            1.0 / 12.92,
            0.04045,
            0.0,
            0.0,
        ];
        for (i, want) in expected
            .into_iter()
            .take(if kind == 4 { 7 } else { 5 })
            .enumerate()
        {
            curves[c][i] = fixed(tag, 12 + i * 4)?;
            if (curves[c][i] - want).abs() > 4.0 / 65536.0 {
                return Err("ICC TRC is not sRGB".into());
            }
        }
    }
    if tags.contains_key(b"chad".as_slice()) {
        return Err("unrecognized ICC chad route requires CMS".into());
    }
    let srgb = ColorPrimaries::Bt709.from_xyz_matrix().ok_or_else(fail)?;
    let mut matrix = [[0f32; 3]; 3];
    for c in 0..3 {
        let cone = mul(
            &bradford,
            [colorants[0][c], colorants[1][c], colorants[2][c]],
        );
        let xyz = mul(
            &inverse,
            [
                cone[0] * a[0] / b[0],
                cone[1] * a[1] / b[1],
                cone[2] * a[2] / b[2],
            ],
        );
        for r in 0..3 {
            matrix[r][c] = (0..3).map(|k| f64::from(srgb[r][k]) * xyz[k]).sum::<f64>() as f32;
        }
    }
    Ok(IccAdapter {
        matrix,
        curves,
        max_deviation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const ICC: &[u8] = include_bytes!("../tests/fixtures/8029-reviewed.icc");

    #[test]
    fn reviewed_neutrals_and_clipped_primaries() {
        let adapter = reviewed_adapter(ICC).unwrap();
        assert_eq!(convert_pixel([0; 3], &adapter).0, [0; 3]);
        assert_eq!(convert_pixel([255; 3], &adapter).0, [255; 3]);
        for gray in 0..=255 {
            let pixel = convert_pixel([gray; 3], &adapter).0;
            for channel in pixel {
                assert!(channel.abs_diff(gray) <= 1, "gray {gray}: {pixel:?}");
            }
        }
        for primary in [[255, 0, 0], [0, 255, 0], [0, 0, 255]] {
            let (pixel, clips) = convert_pixel(primary, &adapter);
            assert_eq!(pixel, primary);
            assert!(clips > 0);
        }
        // Independent expected linear Display-P3 -> BT.709 matrix; tolerance
        // allows this fixture's actual ICC coefficients and fixed precision.
        let expected = [
            [1.22494, -0.22494, 0.0],
            [-0.04206, 1.04206, 0.0],
            [-0.01964, -0.07864, 1.09827],
        ];
        for (actual, expected) in adapter
            .matrix
            .iter()
            .flatten()
            .zip(expected.iter().flatten())
        {
            assert!((actual - expected).abs() < 0.002);
        }
    }

    #[test]
    fn reject_unreviewed_lut_chad_unknown_and_bad_curve() {
        let mut changed = ICC.to_vec();
        changed[500] ^= 1;
        assert!(reviewed_adapter(&changed).is_err());
        for tag in [b"A2B0", b"B2A0", b"D2B0", b"B2D0", b"chad", b"????"] {
            let mut changed = ICC.to_vec();
            changed[132..136].copy_from_slice(tag);
            assert!(validate_p3_matrix_trc(&changed).is_err());
            assert!(reviewed_adapter(&changed).is_err());
        }
        let mut changed = ICC.to_vec();
        changed[432..436].copy_from_slice(&65536i32.to_be_bytes());
        assert!(validate_p3_matrix_trc(&changed).is_err());
        for len in [0, 40, 128, 131, 239, 419, 519] {
            assert!(reviewed_adapter(&ICC[..len]).is_err());
            assert!(validate_p3_matrix_trc(&ICC[..len]).is_err());
        }
    }
}
