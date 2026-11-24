//! Packet-core decode microbenchmark + verifier for the quick decoder.
//!
//! Standalone rustc build (mirrors probe.rs):
//!   rustc -O -o profiler_bench tools/codec-speed/quick-pixels/profiler_bench.rs
//!
//! Modes:
//!   encode  <raw> <geomfile> <out.iqp1>           encode raw to IQP1 container
//!   verify  <in.iqp1> <ref.raw>                   decode once, memcmp vs raw
//!   bench   <in.iqp1> <iters>                     time decode_into (reused out)
//!
//! Geomfile is whitespace-separated "W H C B" like probe.rs.
//! Bench reports min/median/max per-iteration ns plus an output checksum so
//! variants can be diffed mechanically.

#![forbid(unsafe_code)]

#[path = "core.rs"]
mod codec;
use codec::{Shape, decode_into, encode, header};
use std::{fs, hint::black_box, process, time::Instant};

fn geom(path: &str) -> Result<Shape, Box<dyn std::error::Error>> {
    let g: Vec<usize> = fs::read_to_string(path)?
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()?;
    if g.len() != 4 {
        return Err("geometry".into());
    }
    Ok(Shape {
        width: g[0],
        height: g[1],
        channels: g[2],
        bytes: g[3],
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("encode") if a.len() == 5 => {
            let s = geom(&a[3])?;
            let raw = fs::read(&a[2])?;
            let out = || Ok(()).encode_samples(imij::ImageRef::new(&raw, s.row()?, s), None)?;
            fs::write(&a[4], &out)?;
            eprintln!("encoded {} -> {} bytes", a[2], out.len());
        }
        Some("verify") if a.len() == 4 => {
            let enc = fs::read(&a[2])?;
            let raw = fs::read(&a[3])?;
            let (s, _) = header(&enc)?;
            let extent = s.row()?.checked_mul(s.height).ok_or("image extent")?;
            if raw.len() != extent {
                return Err("reference extent".into());
            }
            let mut out = vec![0u8; raw.len()];
            decode_into(&enc, &mut out, s.row()?, || Ok(()))?;
            match out.iter().zip(&raw).position(|(x, y)| x != y) {
                None => eprintln!("VERIFY OK {} bytes", raw.len()),
                Some(i) => {
                    eprintln!("MISMATCH at {i}: got {} want {}", out[i], raw[i]);
                    process::exit(1);
                }
            }
        }
        Some("bench") if a.len() == 4 => {
            let enc = fs::read(&a[2])?;
            let (s, _) = header(&enc)?;
            let iters: u32 = a[3].parse()?;
            if iters == 0 {
                return Err("iterations must be positive".into());
            }
            let row = s.row()?;
            let extent = row.checked_mul(s.height).ok_or("image extent")?;
            let mut out = vec![0u8; extent];
            decode_into(&enc, &mut out, row, || Ok(()))?;
            let sum: u64 = out.iter().map(|&b| b as u64).sum();
            let npx = (s.width * s.height) as f64;
            let mut ts = Vec::with_capacity(iters as usize);
            for _ in 0..iters {
                let t0 = Instant::now();
                decode_into(black_box(&enc), black_box(&mut out), row, || Ok(()))?;
                ts.push(t0.elapsed().as_nanos() as u64);
            }
            ts.sort_unstable();
            let (lo, med, hi) = (ts[0], ts[ts.len() / 2], ts[ts.len() - 1]);
            println!(
                "min={:.3} med={:.3} max={:.3} ms  {:.1}/{:.1} MPx/s  sum={:x}  ({}x{}x{}x{}b, {}B payload)",
                lo as f64 / 1e6,
                med as f64 / 1e6,
                hi as f64 / 1e6,
                npx / lo as f64 * 1e3,
                npx / med as f64 * 1e3,
                sum,
                s.width,
                s.height,
                s.channels,
                s.bytes,
                enc.len(),
            );
        }
        _ => {
            eprintln!("usage: profiler_bench encode|verify|bench ...");
            process::exit(2);
        }
    }
    Ok(())
}
