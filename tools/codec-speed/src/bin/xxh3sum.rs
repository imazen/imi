//! `xxh3sum FILE...` prints `<xxh3-64 hex> <bytes> <path>` per file: XXH3-64, seed 0, default secret, the same
//! digest IMI used for `cSUM` pieces and `pSUM` until 2026-10-25 (they are XXH3-128 now, SPEC 1.4). `xxh3sum --table OUT FILE...` also writes the digests
//! as little-endian u64s (the explorer's `parts.xxh3` layout) and prints the table's own XXH3-64.
#![forbid(unsafe_code)]
use std::io::Write;
use xxhash_rust::xxh3::xxh3_64;

/// `xxh3sum --bench` times XXH3-64 and SHA-256 (sha2 crate, the one the tools use) over in-memory 32 MiB buffers.
fn bench() {
    use sha2::{Digest, Sha256};
    use std::time::Instant;
    let buf: Vec<u8> = (0..32u32 << 20)
        .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
        .collect();
    let (mut x, mut h) = (0u64, 0u8);
    let t = Instant::now();
    for _ in 0..40 {
        x ^= xxh3_64(&buf);
    }
    let a = t.elapsed().as_secs_f64();
    let t = Instant::now();
    for _ in 0..10 {
        h ^= Sha256::digest(&buf)[0];
    }
    let b = t.elapsed().as_secs_f64();
    let gb = |n: f64, s: f64| n * buf.len() as f64 / s / 1e9;
    println!(
        "XXH3-64 {:.1} GB/s, SHA-256 {:.2} GB/s over 32 MiB buffers (checksums {x:x} {h:x})",
        gb(40.0, a),
        gb(10.0, b)
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--bench") {
        bench();
        return Ok(());
    }
    let table = if args.first().is_some_and(|a| a == "--table") {
        args.remove(0);
        Some(args.remove(0))
    } else {
        None
    };
    if args.is_empty() {
        return Err("xxh3sum [--table OUT] FILE...".into());
    }
    let mut digests = Vec::new();
    for path in &args {
        let bytes = std::fs::read(path)?;
        let d = xxh3_64(&bytes);
        println!("{d:016x} {} {path}", bytes.len());
        digests.extend(d.to_le_bytes());
    }
    if let Some(out) = table {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&out)?
            .write_all(&digests)?;
        println!("{:016x} {} table:{out}", xxh3_64(&digests), digests.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matches_the_reference_vectors() {
        // XXH3-64 (seed 0) of the empty input and of "abc" from the xxHash reference implementation.
        assert_eq!(xxh3_64(b""), 0x2d06800538d394c2);
        assert_eq!(xxh3_64(b"abc"), 0x78af5f94892f3950);
    }
}
