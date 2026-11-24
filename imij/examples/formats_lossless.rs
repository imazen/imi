//! Evaluation-only raw-sample encoder and exact roundtrip checker.
//! Usage: formats_lossless INPUT W H CHANNELS SAMPLE OUT_PREFIX
//! Input is tightly packed, native endian, with byte stride W*CHANNELS*sample_bytes.
//! SAMPLE: u8, u16, f16, u32, i32, f32. No color conversion is performed.
#![forbid(unsafe_code)]

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 6 {
        return Err("usage: formats_lossless INPUT W H CHANNELS SAMPLE OUT_PREFIX".into());
    }
    let channels = match args[3].as_str() {
        "1" => imij::Channels::Gray,
        "2" => imij::Channels::GrayAlpha,
        "3" => imij::Channels::Rgb,
        "4" => imij::Channels::Rgba,
        _ => return Err("invalid channel count".into()),
    };
    let sample = match args[4].as_str() {
        "u8" => imij::SampleType::U8,
        "u16" => imij::SampleType::U16,
        "f16" => imij::SampleType::F16,
        "u32" => imij::SampleType::U32,
        "i32" => imij::SampleType::I32,
        "f32" => imij::SampleType::F32,
        _ => return Err("invalid sample type".into()),
    };
    let info = imij::ImageInfo::new(args[1].parse()?, args[2].parse()?, channels, sample);
    let pixels = std::fs::read(&args[0])?;
    if Some(pixels.len()) != info.checked_buffer_len() {
        return Err("input length does not match tightly packed dimensions".into());
    }
    for (name, palette) in [("default", true), ("no_palette", false)] {
        let cfg = {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.tune_lossless(|t| {
                t.palette = palette;
            });
            cfg
        };
        let encoded = cfg.encode_samples(
            imij::ImageRef::new(&pixels, info.row_bytes(), &info),
            &Default::default(),
        )?;
        let (decoded_info, decoded) = imij::decode(&encoded)?;
        if decoded_info != info || decoded != pixels {
            return Err("lossless roundtrip mismatch".into());
        }
        std::fs::write(format!("{}.{name}.imi", args[5]), &encoded)?;
        std::fs::write(format!("{}.{name}.decoded.raw", args[5]), &decoded)?;
        println!("{name}\t{}\texact", encoded.len());
    }
    Ok(())
}
