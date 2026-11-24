//! Decode timing of one file per process (superblock port, 2026-10-10,
//! `benchmarks/sbport_2026-10-10`): the file decoded `--iters` times into
//! one buffer with `--threads` workers inside a rayon pool of that size.
//!
//! `sb_time_20261010 [--iters N] [--threads T] FILE` prints
//! `file threads iters width height best_us median_us`. Kernel selection
//! follows the decoder's switches (`IMIJ_SCALAR`, `IMIJ_NO_AVX512`).
use std::time::Instant;

fn main() {
    let (mut iters, mut threads, mut file) = (20usize, 1usize, None);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--iters" => iters = args.next().unwrap().parse().unwrap(),
            "--threads" => threads = args.next().unwrap().parse().unwrap(),
            _ => file = Some(a),
        }
    }
    let path = file.expect("FILE");
    let data = std::fs::read(&path).unwrap();
    let info = imij::header(&data).unwrap().info;
    let (w, h) = (info.width as usize, info.height as usize);
    let stride = info.row_bytes();
    let mut out = vec![0u8; stride * h];
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap();
    let mut times = Vec::with_capacity(iters);
    pool.install(|| {
        for _ in 0..iters {
            let t = Instant::now();
            imij::decode_into_threads(&data, &mut out, stride, threads).unwrap();
            times.push(t.elapsed().as_secs_f64() * 1e6);
        }
    });
    std::hint::black_box(&out);
    times.sort_by(|a, b| a.total_cmp(b));
    println!(
        "{path}\t{threads}\t{iters}\t{w}\t{h}\t{:.1}\t{:.1}",
        times[0],
        times[times.len() / 2]
    );
}
