//! Significant-bit-1 gray page decode benchmark (P4 PBM scans).
//!
//! `graypage_bench encode PBM_DIR OUT_DIR` encodes every page as Gray u8 with
//! one significant bit (the default configuration), writes `OUT_DIR/<name>.imi`
//! and verifies that a decode of it reproduces the page exactly.
//!
//! `graypage_bench time PBM_DIR IMI_DIR OUT.tsv THREADS REPS` decodes every
//! `.imi` file once for warm-up, then REPS timed runs; it keeps the per-page
//! median, checks exactness on every run, and reports one TSV row per page plus
//! a `#total` row whose rate is computed from the sum of the per-page medians.
//!
//! The decoder's own switches apply: `IMIJ_SCALAR` forces the scalar
//! reference, `IMIJ_NO_AVX512` caps the x86 entropy kernels at AVX2.
//! `imij::simd_tier()` names the entropy tier in the output header.

use imij::{Channels, ImageInfo, Metadata, SampleType};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// P4 PBM: returns (width, height, plane) with 1 = black.
fn load_pbm(path: &Path) -> (usize, usize, Vec<u8>) {
    let b = std::fs::read(path).expect("read pbm");
    assert_eq!(&b[..2], b"P4", "not a P4 PBM");
    let mut i = 2usize;
    let mut nums = [0usize; 2];
    for n in &mut nums {
        loop {
            if b[i] == b'#' {
                while b[i] != b'\n' {
                    i += 1;
                }
            } else if b[i].is_ascii_whitespace() {
                i += 1;
            } else {
                break;
            }
        }
        let s = i;
        while b[i].is_ascii_digit() {
            i += 1;
        }
        *n = std::str::from_utf8(&b[s..i]).unwrap().parse().unwrap();
    }
    let (w, h) = (nums[0], nums[1]);
    let data = &b[i + 1..];
    let rb = w.div_ceil(8);
    assert!(data.len() >= rb * h);
    let mut plane = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            plane[y * w + x] = (data[y * rb + x / 8] >> (7 - x % 8)) & 1;
        }
    }
    (w, h, plane)
}

fn pages(dir: &Path) -> Vec<(String, usize, usize, Vec<u8>)> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .expect("pbm dir")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "pbm"))
        .collect();
    v.sort();
    v.iter()
        .map(|p| {
            let (w, h, plane) = load_pbm(p);
            (
                p.file_stem().unwrap().to_string_lossy().into_owned(),
                w,
                h,
                plane,
            )
        })
        .collect()
}

fn imij_encode(plane: &[u8], w: usize, h: usize) -> Vec<u8> {
    let info = ImageInfo::new(w as u32, h as u32, Channels::Gray, SampleType::U8)
        .with_significant_bits(&[1])
        .unwrap();
    imij::ImiEncoderConfig::default()
        .encode_samples(
            imij::ImageRef::new(plane, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .expect("imij encode")
}

fn decode(file: &[u8], out: &mut [u8], stride: usize, threads: usize) {
    if threads == 1 {
        imij::decode_into(file, out, stride).unwrap();
    } else {
        imij::decode_into_threads(file, out, stride, threads).unwrap();
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn switches() -> String {
    let mut s = String::new();
    for k in ["IMIJ_SCALAR", "IMIJ_NO_AVX512"] {
        if let Ok(v) = std::env::var(k) {
            let _ = write!(s, "{k}={v} ");
        }
    }
    s.trim_end().to_string()
}

fn cmd_encode(dir: &Path, out_dir: &Path) {
    std::fs::create_dir_all(out_dir).expect("create out dir");
    let mut px = 0u64;
    let mut bytes = 0u64;
    let mut n = 0usize;
    for (name, w, h, plane) in pages(dir) {
        n += 1;
        let file = imij_encode(&plane, w, h);
        let mut back = vec![7u8; w * h];
        decode(&file, &mut back, w, 1);
        assert_eq!(back, plane, "{name} encode decode not exact");
        decode(&file, &mut back, w, 4);
        assert_eq!(back, plane, "{name} encode decode not exact at 4 threads");
        std::fs::write(out_dir.join(format!("{name}.imi")), &file).unwrap();
        px += (w * h) as u64;
        bytes += file.len() as u64;
        println!("{name}\t{w}\t{h}\t{}", file.len());
    }
    eprintln!(
        "encoded {n} pages, {px} px, {bytes} bytes, {:.4} bpp",
        bytes as f64 * 8.0 / px as f64
    );
}

fn cmd_time(pbm_dir: &Path, imi_dir: &Path, out: &str, threads: usize, reps: usize) {
    let mut t = String::new();
    let _ = writeln!(
        t,
        "#tier\t{}\tthreads\t{threads}\treps\t{reps}\tswitches\t{}",
        imij::simd_tier(),
        switches()
    );
    let _ = writeln!(t, "name\twidth\theight\tpx\tbytes\tmedian_ms\tmpx_s\texact");
    let (mut total_ms, mut total_px) = (0.0f64, 0u64);
    for (name, w, h, plane) in pages(pbm_dir) {
        let file = std::fs::read(imi_dir.join(format!("{name}.imi"))).expect("read imi");
        let mut back = vec![0u8; w * h];
        decode(&file, &mut back, w, threads);
        assert_eq!(back, plane, "{name} not exact (warm-up)");
        let mut times = Vec::with_capacity(reps);
        for _ in 0..reps {
            let start = Instant::now();
            decode(&file, &mut back, w, threads);
            times.push(start.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(back, plane, "{name} not exact (timed)");
        }
        let ms = median(times);
        let pw = w as u64 * h as u64;
        total_ms += ms;
        total_px += pw;
        let _ = writeln!(
            t,
            "{name}\t{w}\t{h}\t{pw}\t{}\t{ms:.3}\t{:.1}\ttrue",
            file.len(),
            pw as f64 / (ms / 1000.0) / 1e6
        );
    }
    let _ = writeln!(
        t,
        "#total\t-\t-\t{total_px}\t-\t{total_ms:.3}\t{:.1}\ttrue",
        total_px as f64 / (total_ms / 1000.0) / 1e6
    );
    eprintln!(
        "total {} px, {total_ms:.1} ms, {:.1} Mpx/s ({} threads, tier {}, {})",
        total_px,
        total_px as f64 / (total_ms / 1000.0) / 1e6,
        threads,
        imij::simd_tier(),
        switches()
    );
    std::fs::write(out, t).expect("write tsv");
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("encode") if a.len() == 4 => {
            cmd_encode(&PathBuf::from(&a[2]), &PathBuf::from(&a[3]));
        }
        Some("time") if a.len() == 7 => {
            cmd_time(
                &PathBuf::from(&a[2]),
                &PathBuf::from(&a[3]),
                &a[4],
                a[5].parse().expect("threads"),
                a[6].parse().expect("reps"),
            );
        }
        _ => {
            eprintln!(
                "usage: graypage_bench encode PBM_DIR OUT_DIR\n       \
                 graypage_bench time PBM_DIR IMI_DIR OUT.tsv THREADS REPS"
            );
            std::process::exit(2);
        }
    }
}
