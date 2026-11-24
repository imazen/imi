//! Decode one retained native-component file in a fresh process.
//! NATIVE.imij REFERENCE.rgb THREADS [REPETITIONS=10]

use std::hint::black_box;
use std::time::Instant;

fn decode(
    encoded: &[u8],
    output: &mut [u8],
    stride: usize,
    threads: usize,
) -> Result<imij::ImageInfo, imij::Error> {
    if threads == 1 {
        imij::decode_into(encoded, output, stride)
    } else {
        imij::decode_into_threads(encoded, output, stride, threads)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let encoded = std::fs::read(args.next().ok_or("missing encoded path")?)?;
    let reference = std::fs::read(args.next().ok_or("missing reference path")?)?;
    let threads: usize = args.next().ok_or("missing thread count")?.parse()?;
    let repetitions: usize = args.next().map(|v| v.parse()).transpose()?.unwrap_or(10);
    if args.next().is_some() || !matches!(threads, 1 | 2 | 4 | 8) || repetitions == 0 {
        return Err("invalid threads, repetitions or extra argument".into());
    }
    let info = imij::header(&encoded)?.info;
    if reference.len() != info.checked_buffer_len().ok_or("image too large")? {
        return Err("reference dimensions differ".into());
    }
    let stride = info.row_bytes();
    let mut output = vec![0u8; reference.len()];
    for _ in 0..3 {
        if decode(&encoded, &mut output, stride, threads)? != info || output != reference {
            return Err(imij::Error::Corrupt("native decode differs from retained pixels").into());
        }
    }
    let start = Instant::now();
    for _ in 0..repetitions {
        if decode(&encoded, &mut output, stride, threads)? != info {
            return Err(imij::Error::Corrupt("native decode changed its declaration").into());
        }
        black_box(&output);
    }
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0 / repetitions as f64;
    if output != reference {
        return Err(imij::Error::Corrupt("native decode differs from retained pixels").into());
    }
    println!("elapsed_ms={elapsed_ms:.9}");
    println!("check={{\"bytes\":{},\"exact\":true}}", reference.len());
    Ok(())
}
