//! Lab harness for the bilevel template-context rANS prototype
//! (`--features lab`).
//!
//! `bilevel_lab size  PBM_DIR OUT.tsv CFG...` encodes every P4 page with
//! every config, decodes scalar, checks exact pixels and writes one row per
//! page and config (bytes, table bytes, stream bytes).
//! `bilevel_lab check PBM_DIR CFG...` decodes every page scalar and SIMD and
//! requires both to match the source.
//! `bilevel_lab time  PBM_DIR OUT.tsv CFG THREADS TIER` times our decode and,
//! for comparison, IMI lossless on the same page (sig-1 Gray u8): one
//! warm-up and the median of three runs, exactness checked on every run.
//! Configs: a, b, a16, b16 and an optional `:N` run_min (`b:2`).
use imij::bilevel_lab::{self as bilevel, Cfg, Variant};
use imij::{Channels, ImageInfo, ImiEncoderConfig, Metadata, SampleType};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn parse_cfg(s: &str) -> Cfg {
    let (name, run_min) = match s.split_once(':') {
        Some((n, r)) => (n, r.parse().expect("run_min")),
        None => (s, 4),
    };
    let (variant, template16) = match name {
        "a" => (Variant::Bits, false),
        "b" => (Variant::Runs, false),
        "a16" => (Variant::Bits, true),
        "b16" => (Variant::Runs, true),
        other => panic!("unknown config {other}"),
    };
    Cfg {
        variant,
        template16,
        run_min,
        stripe_rows: None,
    }
}

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

fn simd_tier() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "v3"
    } else if cfg!(target_arch = "aarch64") {
        "neon"
    } else if cfg!(target_arch = "wasm32") {
        "wasm128"
    } else {
        "scalar"
    }
}

fn imij_encode(plane: &[u8], w: usize, h: usize) -> Vec<u8> {
    let info = ImageInfo::new(w as u32, h as u32, Channels::Gray, SampleType::U8)
        .with_significant_bits(&[1])
        .unwrap();
    ImiEncoderConfig::default()
        .encode_samples(
            imij::ImageRef::new(plane, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .expect("imij encode")
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn cmd_size(dir: &Path, out: &str, cfgs: &[String]) {
    let mut t = std::fs::File::create(out).unwrap();
    writeln!(
        t,
        "name\tcfg\twidth\theight\tbytes\ttable_bytes\tstream_bytes\tbpp\texact"
    )
    .unwrap();
    let mut total = std::collections::HashMap::<String, u64>::new();
    let mut px = 0u64;
    for (name, w, h, plane) in pages(dir) {
        px += (w * h) as u64;
        for cs in cfgs {
            let cfg = parse_cfg(cs);
            let file = bilevel::encode(&plane, w, h, &cfg, 4);
            let (dw, dh, bytes, table, streams) = bilevel::size_info(&file).unwrap();
            assert_eq!((dw, dh), (w, h));
            let mut back = vec![7u8; w * h];
            bilevel::decode(&file, &mut back, 4, false).unwrap();
            let exact = back == plane;
            assert!(exact, "{name} {cs} not exact");
            *total.entry(cs.clone()).or_default() += bytes as u64;
            writeln!(
                t,
                "{name}\t{cs}\t{w}\t{h}\t{bytes}\t{table}\t{streams}\t{}\t{exact}",
                bytes as f64 * 8.0 / (w * h) as f64
            )
            .unwrap();
            t.flush().unwrap();
        }
    }
    writeln!(t, "#total_px\t{px}").unwrap();
    for cs in cfgs {
        let b = total[cs];
        writeln!(
            t,
            "#total_bytes\t{cs}\t{b}\tbpp\t{}",
            b as f64 * 8.0 / px as f64
        )
        .unwrap();
    }
}

fn cmd_check(dir: &Path, cfgs: &[String]) {
    for (name, w, h, plane) in pages(dir) {
        for cs in cfgs {
            let cfg = parse_cfg(cs);
            let file = bilevel::encode(&plane, w, h, &cfg, 4);
            let mut a = vec![9u8; w * h];
            let mut b = vec![9u8; w * h];
            bilevel::decode(&file, &mut a, 1, false).unwrap();
            bilevel::decode(&file, &mut b, 4, true).unwrap();
            assert_eq!(a, plane, "{name} {cs} scalar not exact");
            assert_eq!(b, plane, "{name} {cs} simd not exact");
            if cs == "a" {
                for tier in [
                    "fast", "v4", "port", "port3", "portS", "ahead", "ahead4", "aheadS",
                ] {
                    let mut c = vec![9u8; w * h];
                    bilevel::decode_tier(&file, &mut c, 3, tier).unwrap();
                    assert_eq!(c, plane, "{name} {cs} {tier} not exact");
                }
            }
            println!("{name} {cs} ok {} bytes", file.len());
        }
    }
}

fn cmd_time(dir: &Path, out: &str, cs: &str, threads: usize, tier: &str) {
    let cfg = parse_cfg(cs);
    let mut t = std::fs::File::create(out).unwrap();
    writeln!(
        t,
        "name\tcfg\tthreads\ttier\tpx\tbilevel_bytes\tbilevel_enc_ms\tbilevel_dec_ms\tbilevel_mpx_s\timij_enc_ms\timij_dec_ms\timij_mpx_s\texact"
    )
    .unwrap();
    let (mut bilevel_ms, mut imij_ms) = (0.0f64, 0.0f64);
    let mut px = 0u64;
    for (name, w, h, plane) in pages(dir) {
        px += (w * h) as u64;
        // Bilevel: encode once, then warm-up + median of 3.
        let start = Instant::now();
        let file = bilevel::encode(&plane, w, h, &cfg, threads);
        let enc_ms = start.elapsed().as_secs_f64() * 1000.0;
        let mut back = vec![7u8; w * h];
        bilevel::decode_tier(&file, &mut back, threads, tier).unwrap();
        assert_eq!(back, plane, "{name} bilevel not exact");
        let mut dec = Vec::new();
        for _ in 0..3 {
            let start = Instant::now();
            bilevel::decode_tier(&file, &mut back, threads, tier).unwrap();
            dec.push(start.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(back, plane, "{name} bilevel not exact (timed)");
        }
        let dms = median(dec);
        // IMI comparison on the same page, same thread count.
        let start = Instant::now();
        let imij_file = imij_encode(&plane, w, h);
        let ienc_ms = start.elapsed().as_secs_f64() * 1000.0;
        let row = w;
        let mut iback = vec![0u8; w * h];
        if threads == 1 {
            imij::decode_into(&imij_file, &mut iback, row).unwrap();
        } else {
            imij::decode_into_threads(&imij_file, &mut iback, row, threads).unwrap();
        }
        assert_eq!(iback, plane, "{name} imij not exact");
        let mut idec = Vec::new();
        for _ in 0..3 {
            let start = Instant::now();
            if threads == 1 {
                imij::decode_into(&imij_file, &mut iback, row).unwrap();
            } else {
                imij::decode_into_threads(&imij_file, &mut iback, row, threads).unwrap();
            }
            idec.push(start.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(iback, plane, "{name} imij not exact (timed)");
        }
        let idms = median(idec);
        bilevel_ms += dms;
        imij_ms += idms;
        let p = (w * h) as f64;
        writeln!(
            t,
            "{name}\t{cs}\t{threads}\t{tier}\t{}\t{}\t{enc_ms:.3}\t{dms:.3}\t{:.1}\t{ienc_ms:.3}\t{idms:.3}\t{:.1}\ttrue",
            w * h,
            file.len(),
            p / (dms / 1000.0) / 1e6,
            p / (idms / 1000.0) / 1e6
        )
        .unwrap();
        t.flush().unwrap();
    }
    let p = px as f64;
    writeln!(
        t,
        "#total\t{cs}\t{threads}\t{tier}\t{px}\t\t\t{bilevel_ms:.3}\t{:.1}\t\t{imij_ms:.3}\t{:.1}\ttrue",
        p / (bilevel_ms / 1000.0) / 1e6,
        p / (imij_ms / 1000.0) / 1e6
    )
    .unwrap();
    eprintln!(
        "total bilevel {:.1} Mpx/s, imij {:.1} Mpx/s ({} threads, {tier}, our tier {})",
        p / (bilevel_ms / 1000.0) / 1e6,
        p / (imij_ms / 1000.0) / 1e6,
        threads,
        simd_tier()
    );
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(&a[2]);
    match a[1].as_str() {
        "size" => cmd_size(&dir, &a[3], &a[4..]),
        "check" => cmd_check(&dir, &a[3..]),
        "time" => cmd_time(&dir, &a[3], &a[4], a[5].parse().expect("threads"), &a[6]),
        "stats" => {
            for (name, w, h, plane) in pages(&dir) {
                for cs in &a[3..] {
                    let (syms, runs, skipped) = bilevel::counts(&plane, w, h, &parse_cfg(cs));
                    println!(
                        "{name}\t{cs}\tpx={}\tsyms={syms}\truns={runs}\tskipped={skipped}\tbits={}",
                        w * h,
                        syms - runs
                    );
                }
            }
        }
        other => panic!("usage: bilevel_lab size|check|time|stats ... ({other})"),
    }
}
