//! Lossless decode benchmark over raw images listed in a manifest.
//!
//! Manifest: one tab-separated `tag channel sample width height raw_path` row
//! per image (`channel` is 3 or 4, `sample` is `u8` or `u16`, raw pixels are
//! interleaved in file order, native-endian for u16).
//!
//! `lossless_bench encode MANIFEST` encodes every raw image with the default
//! configuration and writes `<raw_path>.imi`, checking an exact decode.
//! `lossless_bench time MANIFEST OUT.tsv THREADS REPS` decodes each `.imi`
//! once for warm-up and REPS times, keeps the per-file median and checks
//! exactness against the raw source on every run.
//!
//! The decoder's own switches apply: `IMIJ_SCALAR` forces the scalar
//! reference, `IMIJ_NO_AVX512` caps the x86 entropy kernels at AVX2.
//! `imij::simd_tier()` names the entropy tier in the output header.

use imij::{Channels, ImageInfo, Metadata, SampleType};
use std::fmt::Write as _;
use std::path::Path;
use std::time::Instant;

struct Row {
    tag: String,
    channels: usize,
    sample: SampleType,
    width: usize,
    height: usize,
    raw: String,
}

fn rows(manifest: &Path) -> Vec<Row> {
    std::fs::read_to_string(manifest)
        .expect("read manifest")
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            assert_eq!(f.len(), 6, "bad manifest row: {l}");
            let channels: usize = f[1].parse().unwrap();
            let sample = match f[2] {
                "u8" => SampleType::U8,
                "u16" => SampleType::U16,
                other => panic!("sample {other}"),
            };
            Row {
                tag: f[0].to_string(),
                channels,
                sample,
                width: f[3].parse().unwrap(),
                height: f[4].parse().unwrap(),
                raw: f[5].to_string(),
            }
        })
        .collect()
}

fn info(r: &Row) -> ImageInfo {
    let ch = match r.channels {
        3 => Channels::Rgb,
        4 => Channels::Rgba,
        n => panic!("channels {n}"),
    };
    ImageInfo::new(r.width as u32, r.height as u32, ch, r.sample)
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn decode(file: &[u8], out: &mut [u8], stride: usize, threads: usize) {
    if threads == 1 {
        imij::decode_into(file, out, stride).unwrap();
    } else {
        imij::decode_into_threads(file, out, stride, threads).unwrap();
    }
}

fn cmd_encode(manifest: &Path) {
    for r in rows(manifest) {
        let raw = std::fs::read(&r.raw).expect("read raw");
        assert_eq!(raw.len(), info(&r).checked_buffer_len().unwrap());
        let file = imij::ImiEncoderConfig::default()
            .encode_samples(
                imij::ImageRef::new(&raw, info(&r).row_bytes(), &info(&r)),
                &Metadata::default(),
            )
            .expect("encode");
        let mut back = vec![0u8; raw.len()];
        decode(&file, &mut back, info(&r).row_bytes(), 1);
        assert_eq!(back, raw, "{} not exact", r.raw);
        std::fs::write(format!("{}.imi", r.raw), &file).unwrap();
        println!("{}\t{}\t{}", r.tag, r.raw, file.len());
    }
}

fn cmd_time(manifest: &Path, out: &str, threads: usize, reps: usize) {
    let mut t = String::new();
    let _ = writeln!(
        t,
        "#tier\t{}\tthreads\t{threads}\treps\t{reps}",
        imij::simd_tier()
    );
    let _ = writeln!(
        t,
        "tag\twidth\theight\tchannels\tsample\tpx\tbytes\tmedian_ms\tmpx_s\texact"
    );
    for r in rows(manifest) {
        let info = info(&r);
        let raw = std::fs::read(&r.raw).expect("read raw");
        let file = std::fs::read(format!("{}.imi", r.raw)).expect("read imi");
        let stride = info.row_bytes();
        let mut back = vec![0u8; raw.len()];
        decode(&file, &mut back, stride, threads);
        assert_eq!(back, raw, "{} not exact (warm-up)", r.raw);
        let mut times = Vec::with_capacity(reps);
        for _ in 0..reps {
            let start = Instant::now();
            decode(&file, &mut back, stride, threads);
            times.push(start.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(back, raw, "{} not exact (timed)", r.raw);
        }
        let ms = median(times);
        let px = r.width as u64 * r.height as u64;
        let sample = if r.sample == SampleType::U16 {
            "u16"
        } else {
            "u8"
        };
        let _ = writeln!(
            t,
            "{}\t{}\t{}\t{}\t{sample}\t{px}\t{}\t{ms:.3}\t{:.1}\ttrue",
            r.tag,
            r.width,
            r.height,
            r.channels,
            file.len(),
            px as f64 / (ms / 1000.0) / 1e6
        );
    }
    eprintln!(
        "{} files, tier {}, {threads} threads, {reps} reps",
        rows(manifest).len(),
        imij::simd_tier()
    );
    std::fs::write(out, t).expect("write tsv");
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("encode") if a.len() == 3 => cmd_encode(Path::new(&a[2])),
        Some("time") if a.len() == 6 => cmd_time(
            Path::new(&a[2]),
            &a[3],
            a[4].parse().expect("threads"),
            a[5].parse().expect("reps"),
        ),
        _ => {
            eprintln!(
                "usage: lossless_bench encode MANIFEST\n       \
                 lossless_bench time MANIFEST OUT.tsv THREADS REPS"
            );
            std::process::exit(2);
        }
    }
}
