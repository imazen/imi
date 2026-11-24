//! Shared decode timing loop of the `cSUM` examples (2026-10-07). Uses only
//! APIs older than the checksum, so it builds on commits without it.

use std::time::Instant;

/// One decode path: decodes `data` (header `info`) into `out` and returns a
/// value folded into a sink so the work is not optimized away.
pub type DecodePath = (
    &'static str,
    fn(&[u8], &imij::ImageInfo, &mut [u8], usize) -> u64,
);

pub fn read_files(paths: impl IntoIterator<Item = String>) -> Vec<(String, Vec<u8>)> {
    paths
        .into_iter()
        .map(|p| {
            let d = std::fs::read(&p).unwrap();
            (p, d)
        })
        .collect()
}

/// Times every path on every file: each repetition decodes every file once
/// per path, so files (one without and one with a checksum) interleave.
/// Output buffers are allocated and written once per file and one warm-up
/// decode per path runs before timing; `CSUM_PATHS=a,b` times only the
/// named paths. Prints
/// `path<TAB>file<TAB>bytes<TAB>pixels<TAB>min_ns<TAB>median_ns`.
pub fn dtime(paths: &[DecodePath], files: &[(String, Vec<u8>)], reps: usize, threads: usize) {
    let only = std::env::var("CSUM_PATHS").ok();
    let paths: Vec<DecodePath> = paths
        .iter()
        .copied()
        .filter(|(n, _)| {
            only.as_deref()
                .is_none_or(|o| o.split(',').any(|x| x == *n))
        })
        .collect();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap();
    let infos: Vec<imij::ImageInfo> = files
        .iter()
        .map(|(_, d)| imij::header(d).unwrap().info)
        .collect();
    let mut bufs: Vec<Vec<u8>> = infos
        .iter()
        .map(|i| vec![0x5a; i.checked_buffer_len().unwrap()])
        .collect();
    let mut times = vec![vec![Vec::with_capacity(reps); paths.len()]; files.len()];
    let mut sink = 0u64;
    pool.install(|| {
        for (fi, (_, data)) in files.iter().enumerate() {
            for (_, f) in &paths {
                sink ^= f(data, &infos[fi], &mut bufs[fi], threads);
            }
        }
        for _ in 0..reps {
            for (fi, file_times) in times.iter_mut().enumerate() {
                for ((_, f), path_times) in paths.iter().zip(file_times.iter_mut()) {
                    let t = Instant::now();
                    sink ^= f(&files[fi].1, &infos[fi], &mut bufs[fi], threads);
                    path_times.push(t.elapsed().as_nanos() as u64);
                }
            }
        }
    });
    for (fi, (name, data)) in files.iter().enumerate() {
        let info = &infos[fi];
        for (pi, (path, _)) in paths.iter().enumerate() {
            let t = &mut times[fi][pi];
            t.sort_unstable();
            println!(
                "{path}\t{name}\t{}\t{}\t{}\t{}",
                data.len(),
                info.width as u64 * info.height as u64,
                t[0],
                t[t.len() / 2]
            );
        }
    }
    eprintln!("sink {sink}");
}

/// Folds a decoded row into a value.
pub fn fold(row: &[u8]) -> u64 {
    row.first().copied().unwrap_or(0) as u64 ^ row.len() as u64
}
