//! Lab harness for the k-bit template-context coder (`--features lab`,
//! benchmarks/lowbit_2026-10-11).
//!
//! `kbit_lab encode LIST OUT.tsv` codes every raw image of LIST (lowbit_eval's
//! list: `tag channels width height raw_path`, u8 samples) that has 2 to 32
//! colors, decodes it with every decoder, requires exact pixels from each,
//! writes `<tag>.kb` beside OUT.tsv and one row per image: colors, file bytes,
//! table, palette and stream bytes. `encode-tp` does the same with typical
//! rows (`<tag>.kt`), `encode-rank` and `encode-rank-tp` with the rank
//! variant (`<tag>.kr`, `<tag>.ks`; not the first vector decoder).
//! `kbit_lab time FILE RAW THREADS REPS [ref|vec|fast|adapt|adapt-lane]`
//! decodes one file REPS times (default: the fast vector decoder) after a
//! warm-up, checks the pixels every time and prints
//! `file threads pixels median_ms Mpx/s` like `lowbit_eval time`.
//! `kbit_lab stats LIST OUT.tsv` writes the coding's event counts per image
//! (`imij::kbit_lab::EventStats`).
//! `kbit_lab check LIST DIR [EXT]` decodes every `DIR/<tag>.kb` (or `.EXT`) of
//! LIST with the reference, the first vector and every fast decoder at 1 and
//! 3 threads and requires the raw pixels from each; prints one line per file.
use std::io::Write as _;
use std::time::Instant;

use imij::kbit_lab::Decoder;

const DECODERS: [Decoder; 5] = [
    Decoder::Reference,
    Decoder::Vector,
    Decoder::Fast,
    Decoder::FastAdaptive,
    Decoder::FastAdaptiveLaneEscape,
];

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a[1].as_str() {
        "encode" | "encode-tp" | "encode-rank" | "encode-rank-tp" => {
            let tp = a[1].ends_with("-tp");
            let rank = a[1].starts_with("encode-rank");
            let dir = std::path::Path::new(&a[3]).parent().unwrap().to_path_buf();
            let mut t = std::fs::File::create(&a[3]).unwrap();
            writeln!(
                t,
                "tag\tcolors\twidth\theight\tbytes\ttable\tpalette\tstreams\tenc_ms"
            )
            .unwrap();
            for line in std::fs::read_to_string(&a[2]).unwrap().lines() {
                let f: Vec<&str> = line.split('\t').collect();
                let ch: usize = f[1].parse().unwrap();
                let (w, h): (usize, usize) = (f[2].parse().unwrap(), f[3].parse().unwrap());
                let raw = std::fs::read(f[4]).unwrap();
                let start = Instant::now();
                let file = match (rank, tp) {
                    (true, _) => imij::kbit_lab::encode_rank(&raw, w, h, ch, 1, tp),
                    (false, true) => imij::kbit_lab::encode_typical_rows(&raw, w, h, ch, 1),
                    (false, false) => imij::kbit_lab::encode(&raw, w, h, ch, 1),
                };
                let Some(file) = file else {
                    continue;
                };
                let ms = start.elapsed().as_secs_f64() * 1000.0;
                for d in DECODERS {
                    let mut out = vec![0u8; raw.len()];
                    if rank && d == Decoder::Vector {
                        continue;
                    }
                    imij::kbit_lab::decode_with(&file, &mut out, 1, d).unwrap();
                    assert!(out == raw, "{} not exact ({d:?})", f[0]);
                }
                let (n, table, pal, streams) = imij::kbit_lab::size_info(&file).unwrap();
                let ext = match (rank, tp) {
                    (false, false) => "kb",
                    (false, true) => "kt",
                    (true, false) => "kr",
                    (true, true) => "ks",
                };
                std::fs::write(dir.join(format!("{}.{ext}", f[0])), &file).unwrap();
                writeln!(
                    t,
                    "{}\t{n}\t{w}\t{h}\t{}\t{table}\t{pal}\t{streams}\t{ms:.3}",
                    f[0],
                    file.len()
                )
                .unwrap();
                t.flush().unwrap();
            }
        }
        "stats" => {
            let mut t = std::fs::File::create(&a[3]).unwrap();
            writeln!(
                t,
                "tag\tcolors\tpixels\tsteps\tmiss_px\tmiss_steps\tcand_phases\tescape_px\tescape_steps\tbinary_events\trepeated_rows\trows"
            )
            .unwrap();
            for line in std::fs::read_to_string(&a[2]).unwrap().lines() {
                let f: Vec<&str> = line.split('\t').collect();
                let ch: usize = f[1].parse().unwrap();
                let (w, h): (usize, usize) = (f[2].parse().unwrap(), f[3].parse().unwrap());
                let raw = std::fs::read(f[4]).unwrap();
                let Some(s) = imij::kbit_lab::event_stats(&raw, w, h, ch) else {
                    continue;
                };
                writeln!(
                    t,
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    f[0],
                    s.colors,
                    s.pixels,
                    s.steps,
                    s.miss_px,
                    s.miss_steps,
                    s.cand_phases,
                    s.escape_px,
                    s.escape_steps,
                    s.binary_events,
                    s.repeated_rows,
                    s.rows
                )
                .unwrap();
            }
        }
        "check" => {
            let dir = std::path::Path::new(&a[3]);
            let ext = a.get(4).map_or("kb", String::as_str);
            for line in std::fs::read_to_string(&a[2]).unwrap().lines() {
                let f: Vec<&str> = line.split('\t').collect();
                let Ok(file) = std::fs::read(dir.join(format!("{}.{ext}", f[0]))) else {
                    continue;
                };
                let raw = std::fs::read(f[4]).unwrap();
                for d in DECODERS {
                    // The first vector decoder predates the rank variant.
                    if matches!(ext, "kr" | "ks") && d == Decoder::Vector {
                        continue;
                    }
                    for th in [1, 3] {
                        let mut out = vec![0u8; raw.len()];
                        imij::kbit_lab::decode_with(&file, &mut out, th, d).unwrap();
                        assert!(out == raw, "{} not exact ({d:?}, {th} threads)", f[0]);
                    }
                }
                println!("{}\texact", f[0]);
            }
        }
        "time" => {
            let data = std::fs::read(&a[2]).unwrap();
            let raw = std::fs::read(&a[3]).unwrap();
            let (threads, reps): (usize, usize) = (a[4].parse().unwrap(), a[5].parse().unwrap());
            let d = match a.get(6).map(String::as_str) {
                None | Some("fast") => imij::kbit_lab::Decoder::Fast,
                Some("adapt") => imij::kbit_lab::Decoder::FastAdaptive,
                Some("adapt-lane") => imij::kbit_lab::Decoder::FastAdaptiveLaneEscape,
                Some("vec") => imij::kbit_lab::Decoder::Vector,
                Some("ref") => imij::kbit_lab::Decoder::Reference,
                Some(o) => panic!("decoder {o}"),
            };
            let mut out = vec![0u8; raw.len()];
            let (w, h, _) = imij::kbit_lab::decode_with(&data, &mut out, threads, d).unwrap();
            assert!(out == raw, "not exact");
            let mut ms = Vec::with_capacity(reps);
            for _ in 0..reps {
                out.fill(0);
                let start = Instant::now();
                imij::kbit_lab::decode_with(&data, &mut out, threads, d).unwrap();
                ms.push(start.elapsed().as_secs_f64() * 1000.0);
                assert!(out == raw, "not exact (timed)");
            }
            ms.sort_by(f64::total_cmp);
            let m = ms[ms.len() / 2];
            let px = (w * h) as f64;
            println!(
                "{}\t{threads}\t{}\t{m:.4}\t{:.1}",
                a[2],
                w * h,
                px / (m / 1000.0) / 1e6
            );
        }
        _ => panic!(
            "usage: kbit_lab encode[-rank][-tp] LIST OUT.tsv | stats LIST OUT.tsv | check LIST DIR [EXT] | time FILE RAW THREADS REPS [ref|vec|fast|adapt|adapt-lane]"
        ),
    }
}
