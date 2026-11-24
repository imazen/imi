//! File IO and pixel verification stay outside the measured encode/decode.
#![forbid(unsafe_code)]
#[path = "core.rs"]
mod codec;
use codec::{Shape, decode_into, encode, header};
use std::{fs, hint::black_box, time::Instant};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    if a.len() == 4 && a[1] == "decode" {
        let encoded = fs::read(&a[2])?;
        let (s, _) = header(&encoded)?;
        let n = s.row()?.checked_mul(s.height).ok_or("size")?;
        // This is a measurement CLI, not a production allocation policy.
        if n > 1024 * 1024 * 1024 {
            return Err("probe raster cap".into());
        }
        let mut out = Vec::new();
        out.try_reserve_exact(n)?;
        out.resize(n, 0);
        decode_into(&encoded, &mut out, s.row()?, || Ok(()))?;
        fs::write(&a[3], out)?;
        return Ok(());
    }
    if a.len() != 6 {
        return Err("probe encode|measure RAW GEOMETRY qoi|wide OUTPUT".into());
    }
    if a[1] != "encode" && a[1] != "measure" {
        return Err("operation".into());
    }
    let g = fs::read_to_string(&a[3])?
        .split_whitespace()
        .map(str::parse::<usize>)
        .collect::<Result<Vec<_>, _>>()?;
    if g.len() != 4 {
        return Err("geometry".into());
    }
    let s = Shape {
        width: g[0],
        height: g[1],
        channels: g[2],
        bytes: g[3],
    };
    let mode = match a[4].as_str() {
        "qoi" => Some(0),
        "wide" => None,
        _ => return Err("mode".into()),
    };
    let raw = fs::read(&a[2])?;
    let row = s.row()?;
    let reference = || Ok(()).encode_samples(imij::ImageRef::new(&raw, row, s), mode)?;
    let mut output = vec![0; raw.len()];
    for _ in 0..4 {
        decode_into(&reference, &mut output, row, || Ok(()))?;
        black_box(&output);
    }
    if output != raw {
        return Err("pixels".into());
    }
    let start = Instant::now();
    let encoded = || Ok(()).encode_samples(imij::ImageRef::new(black_box(&raw), row, s), mode)?;
    let enc = start.elapsed();
    let start = Instant::now();
    decode_into(black_box(&encoded), &mut output, row, || Ok(()))?;
    let dec_into = start.elapsed();
    let start = Instant::now();
    let mut allocated = vec![0; raw.len()];
    decode_into(black_box(&encoded), &mut allocated, row, || Ok(()))?;
    let dec = start.elapsed();
    if allocated != raw {
        return Err("allocated mismatch".into());
    }
    if output != raw || encoded != reference {
        return Err("measured mismatch".into());
    }
    if a[5] != "-" {
        fs::write(&a[5], &encoded)?;
    }
    println!(
        "{{\"encode_seconds\":{},\"decode_seconds\":{},\"pixels\":{},\"bytes\":{}}}",
        enc.as_secs_f64(),
        dec.as_secs_f64(),
        s.width * s.height,
        encoded.len()
    );
    println!("encode_ns={}", enc.as_nanos());
    println!("decode_ns={}", dec.as_nanos());
    println!("decode_into_ns={}", dec_into.as_nanos());
    println!("check={{\"raw_bytes\":{}}}", raw.len());
    Ok(())
}
