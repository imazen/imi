//! Bit-exact full-matrix read probe for a stored Gray/F32 scientific image.
//! Usage: scientific_access RAW ENCODED WIDTH HEIGHT THREADS HOT_RUNS

use imij::{Channels, ImageInfo, SampleType, decode_into_threads};
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

fn argument<T: std::str::FromStr>(
    args: &mut impl Iterator<Item = String>,
) -> Result<T, Box<dyn std::error::Error>>
where
    T::Err: std::error::Error + 'static,
{
    Ok(args.next().ok_or("missing argument")?.parse()?)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let raw = PathBuf::from(args.next().ok_or("missing raw path")?);
    let encoded_path = PathBuf::from(args.next().ok_or("missing encoded path")?);
    let width: u32 = argument(&mut args)?;
    let height: u32 = argument(&mut args)?;
    let threads: usize = argument(&mut args)?;
    let hot_runs: usize = argument(&mut args)?;
    if args.next().is_some() || width == 0 || height == 0 || threads == 0 || hot_runs == 0 {
        return Err("invalid scientific access arguments".into());
    }

    let info = ImageInfo::new(width, height, Channels::Gray, SampleType::F32);
    let stride = info.row_bytes();
    let expected_len = stride
        .checked_mul(height as usize)
        .ok_or("raw size overflow")?;
    let expected = fs::read(&raw)?;
    if expected.len() != expected_len {
        return Err("raw matrix size differs from the declared dimensions".into());
    }

    let started = Instant::now();
    let encoded = fs::read(&encoded_path)?;
    let read_ns = started.elapsed().as_nanos();
    let mut pixels = vec![0u8; expected_len];
    let started = Instant::now();
    let decoded_info = decode_into_threads(&encoded, &mut pixels, stride, threads)?;
    let first_decode_ns = started.elapsed().as_nanos();
    if decoded_info != info || pixels != expected {
        return Err("first decode differs from the original Float32 bits".into());
    }

    let mut hot_ns = Vec::with_capacity(hot_runs);
    for _ in 0..hot_runs {
        let started = Instant::now();
        let decoded_info = decode_into_threads(&encoded, &mut pixels, stride, threads)?;
        hot_ns.push(started.elapsed().as_nanos());
        if decoded_info != info || pixels != expected {
            return Err("repeated decode differs from the original Float32 bits".into());
        }
    }
    hot_ns.sort_unstable();
    println!(
        "mode=full file_bytes={} raw_bytes={} width={width} height={height} threads={threads} read_ns={read_ns} first_decode_ns={first_decode_ns} hot_median_ns={} hot_worst_ns={} hot_runs={hot_runs} exact=true",
        encoded.len(),
        expected.len(),
        hot_ns[hot_ns.len() / 2],
        hot_ns[hot_ns.len() - 1],
    );
    Ok(())
}
