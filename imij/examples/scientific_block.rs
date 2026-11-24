//! Size and bit-exactness probe for one row-major Float32 feature block.
//! Usage: scientific_block RAW WIDTH HEIGHT EFFORT THREADS OUTPUT

use imij::{Channels, ImageInfo, Metadata, SampleType};
use std::{fs, io::Write, path::PathBuf, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let raw = PathBuf::from(args.next().ok_or("missing raw input")?);
    let width: u32 = args.next().ok_or("missing width")?.parse()?;
    let height: u32 = args.next().ok_or("missing height")?.parse()?;
    let effort: f32 = args.next().ok_or("missing effort")?.parse()?;
    let threads: usize = args.next().ok_or("missing threads")?.parse()?;
    let output = PathBuf::from(args.next().ok_or("missing output")?);
    if args.next().is_some() {
        return Err("unexpected extra argument".into());
    }
    let info = ImageInfo::new(width, height, Channels::Gray, SampleType::F32);
    let stride = info.row_bytes();
    let pixels = fs::read(&raw)?;
    if pixels.len()
        != stride
            .checked_mul(height as usize)
            .ok_or("raw size overflow")?
    {
        return Err("raw block size differs from width × height × 4".into());
    }
    let cfg = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.with_effort(effort);
        cfg = cfg.with_threads(threads);
        cfg = cfg.tune_lossless(|t| {
            t.palette = false;
        });
        cfg
    };
    let started = Instant::now();
    let encoded = cfg.encode_samples(
        imij::ImageRef::new(&pixels, stride, &info),
        &Metadata::default(),
    )?;
    let encode_seconds_diagnostic = started.elapsed().as_secs_f64();
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)?;
    file.write_all(&encoded)?;
    file.sync_all()?;
    let mut decoded = vec![0u8; pixels.len()];
    let started = Instant::now();
    let decoded_info = imij::decode_into_threads(&encoded, &mut decoded, stride, threads)?;
    let decode_seconds_diagnostic = started.elapsed().as_secs_f64();
    if decoded_info != info || decoded != pixels {
        return Err("IMIJ decoded Float32 bits differ from the Parquet block".into());
    }
    println!(
        "raw_bytes={} imij_bytes={} rows={} features={} effort={} threads={} exact=true encode_seconds_diagnostic={:.6} decode_seconds_diagnostic={:.6}",
        pixels.len(),
        encoded.len(),
        height,
        width,
        effort,
        threads,
        encode_seconds_diagnostic,
        decode_seconds_diagnostic,
    );
    Ok(())
}
