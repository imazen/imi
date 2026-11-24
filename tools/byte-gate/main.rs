//! Encoded-bytes gate: hashes (FNV-1a 64 + length) every encode of a fixed
//! grid, lossy and lossless, per (image, effort, quality, configuration),
//! and the pixels each encode decodes to (FNV-1a 64), so a change that must
//! move bytes but not pixels (a container or grammar change) is checked
//! cell by cell (`compare.py`).
//! `run.sh` builds this program against two source trees (default and `lab`
//! features) and compares the outputs: an encoder change that should not
//! move bytes must leave them identical. Images: raw RGB8 files named
//! `<name>_<width>_<height>.rgb8` (see `README.md`). Grid from the AQ +
//! trellis study's gate (`benchmarks/aq_trellis_2026-10-09/gate`), plus the
//! lossless encoder.
include!("grid.rs");

/// The decoded pixels' hash of an encode.
fn decoded(file: &[u8]) -> u64 {
    fnv(&imij::decode(file).unwrap().1)
}

fn fnv(b: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &x in b {
        h = (h ^ x as u64).wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// Images run on `IMIJ_GATE_THREADS` worker threads (default: available
/// parallelism, at most 6); lines are printed in argument order, so the
/// output does not depend on the thread count. wasm32-wasip1 has no
/// `std::thread`: serial, same line order, so a wasmtime build can be
/// compared against the native hash lists.
#[cfg(target_arch = "wasm32")]
fn main() {
    for path in std::env::args().skip(1) {
        let mut out = String::new();
        gate(&path, &mut out);
        print!("{out}");
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let paths: Vec<String> = std::env::args().skip(1).collect();
    let threads = std::env::var("IMIJ_GATE_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get().min(6)))
        .max(1);
    let next = AtomicUsize::new(0);
    let outs: Vec<std::sync::Mutex<Option<String>>> =
        paths.iter().map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = paths.get(i) else { break };
                    let mut out = String::new();
                    gate(path, &mut out);
                    *outs[i].lock().unwrap() = Some(out);
                }
            });
        }
    });
    for o in outs {
        print!("{}", o.into_inner().unwrap().unwrap());
    }
}

/// Every encode of one image, one line each.
fn gate(path: &str, lines: &mut String) {
    use std::fmt::Write;
    grid(path, |key, out| {
        writeln!(lines, "{key}\t{}\t{:016x}\t{:016x}", out.len(), fnv(&out), decoded(&out)).unwrap();
    });
}
