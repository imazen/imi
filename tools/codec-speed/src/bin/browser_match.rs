//! Source-bound IMI fixtures and persistent full-raster SSIMULACRA2 scoring.
#![forbid(unsafe_code)]
use std::{
    fs,
    io::{self, BufRead, Write},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<_> = std::env::args().collect();
    if a.len() < 5 {
        return Err("browser_match score RAW W H | encode MODE Q RAW W H OUTPUT REFERENCE".into());
    }
    if a[1] == "score" && a.len() == 5 {
        let raw = fs::read(&a[2])?;
        let w: u32 = a[3].parse()?;
        let h: u32 = a[4].parse()?;
        let source = fast_ssim2::PixelSlice::new(
            &raw,
            w,
            h,
            w as usize * 3,
            fast_ssim2::PixelDescriptor::RGB8_SRGB,
        )?;
        let reference = fast_ssim2::Ssimulacra2Reference::new(&source)?;
        for line in io::stdin().lock().lines() {
            let path = line?;
            let pixels = fs::read(path)?;
            let image = fast_ssim2::PixelSlice::new(
                &pixels,
                w,
                h,
                w as usize * 3,
                fast_ssim2::PixelDescriptor::RGB8_SRGB,
            )?;
            let score = reference.compare(&image)?;
            if !score.is_finite() {
                return Err("nonfinite metric".into());
            }
            println!("{score:.12}");
            io::stdout().flush()?;
        }
    } else if a[1] == "encode" && a.len() == 9 {
        let raw = fs::read(&a[4])?;
        let w: u32 = a[5].parse()?;
        let h: u32 = a[6].parse()?;
        let info = imij::ImageInfo::new(w, h, imij::Channels::Rgb, imij::SampleType::U8);
        let encoded = if a[2] == "lossless" {
            let config = {
                let mut cfg = imij::ImiEncoderConfig::default();
                cfg = cfg.with_threads(8);
                cfg = cfg.with_effort(10.);
                cfg
            };
            config.encode_samples(
                imij::ImageRef::new(&raw, w as usize * 3, &info),
                &imij::Metadata::default(),
            )?
        } else if a[2] == "lossy" {
            let q: f32 = a[3].parse()?;
            let config = imij::ImiEncoderConfig::lossy(90.0)
                .with_quality(imij::LossyQuality::JpegQuality(q))
                .with_effort(10.)
                .with_decode_speed(imij::DecodeSpeed::Fast)
                .with_threads(8);
            config.encode_samples(
                imij::ImageRef::new(&raw, w as usize * 3, &info),
                &imij::Metadata::default(),
            )?
        } else {
            return Err("unknown mode".into());
        };
        let (decoded_info, pixels) = imij::decode(&encoded)?;
        if decoded_info != info {
            return Err("IMI dimensions/format".into());
        }
        if a[2] == "lossless" && pixels != raw {
            return Err("lossless IMI pixels".into());
        }
        use std::fs::OpenOptions;
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&a[7])?
            .write_all(&encoded)?;
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&a[8])?
            .write_all(&pixels)?;
    } else {
        return Err("invalid action or arguments".into());
    }
    Ok(())
}
