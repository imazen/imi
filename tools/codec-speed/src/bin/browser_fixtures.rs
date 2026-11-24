//! Prepare browser JPEG/PNG fixtures from an already sealed RGB8 crop.
#![forbid(unsafe_code)]
use enough::Unstoppable;
use std::{fs, path::Path, time::Instant};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<_> = std::env::args().collect();
    if a.len() != 5 {
        return Err("browser_fixtures RAW WIDTH HEIGHT FRESH_OUTPUT".into());
    }
    let w: usize = a[2].parse()?;
    let h: usize = a[3].parse()?;
    let raw = fs::read(&a[1])?;
    if raw.len()
        != w.checked_mul(h)
            .and_then(|n| n.checked_mul(3))
            .ok_or("geometry overflow")?
    {
        return Err("RGB8 extent".into());
    }
    let out = Path::new(&a[4]);
    fs::create_dir(out)?;
    let start = Instant::now();
    let cfg =
        zenjpeg::encoder::EncoderConfig::ycbcr(80.0, zenjpeg::encoder::ChromaSubsampling::Quarter)
            .optimization(zenjpeg::encoder::OptimizationPreset::HybridMaxCompression)
            .progressive(true);
    let mut enc = cfg.encode_from_bytes(
        w.try_into()?,
        h.try_into()?,
        zenjpeg::encoder::PixelLayout::Rgb8Srgb,
    )?;
    enc.push_packed(&raw, Unstoppable)?;
    let jpeg = enc.finish()?;
    fs::write(out.join("progressive.jpg"), &jpeg)?;
    println!(
        "zenjpeg Max progressive q80 420: {} bytes; {:.3} ms",
        jpeg.len(),
        start.elapsed().as_secs_f64() * 1000.0
    );
    let rgb: Vec<rgb::Rgb<u8>> = raw
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| rgb::Rgb::new(p[0], p[1], p[2]))
        .collect();
    let mut cfg = zenpng::EncodeConfig::default().with_compression(zenpng::Compression::Effort(30));
    cfg.max_threads = 8;
    cfg.decode_segments = 0;
    let start = Instant::now();
    let png = zenpng::encode_rgb8(
        imgref::ImgRef::new(&rgb, w, h),
        None,
        &cfg,
        &Unstoppable,
        &Unstoppable,
    )?;
    fs::write(out.join("compressed.png"), &png)?;
    println!(
        "zenpng effort30: {} bytes; {:.3} ms",
        png.len(),
        start.elapsed().as_secs_f64() * 1000.0
    );
    let decoded = zenpng::decode(
        &png,
        &zenpng::PngDecodeConfig::strict()
            .with_max_pixels(40_000_000)
            .with_max_memory(1 << 30),
        &Unstoppable,
    )?;
    if decoded.pixels.copy_to_contiguous_bytes() != raw {
        return Err("PNG lossless mismatch".into());
    }
    Ok(())
}
