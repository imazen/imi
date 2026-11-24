//! Low-bit-depth evaluation harness (benchmarks/lowbit_2026-10-11).
//!
//! `lowbit_eval encode LIST OUT.tsv EFFORT...` encodes every raw image of
//! LIST (tab-separated `tag channels width height raw_path`, u8 samples,
//! interleaved; an optional sixth column `u16`, `f16` or `f32` reads
//! native-endian samples of that type, and an optional seventh declares
//! that many significant bits per channel) at each effort, checks an exact decode, writes
//! `<tag>.e<EFFORT>.imi` beside OUT.tsv and one row per image and effort
//! (bytes, encode milliseconds).
//! `lowbit_eval time FILE RAW THREADS REPS [pulse]` decodes one file REPS times
//! after a warm-up, checks exact pixels on every run and prints the median
//! milliseconds (one file per process, docs/EVALUATION_STANDARD.md). With
//! `pulse` (lab builds) every decode reports its units to a live
//! `how_far_along::PulseTree`, built and finished inside the timed region.
//! `lowbit_eval encode-time FILE RAW REPS` re-encodes RAW with FILE's layout
//! and prints the median milliseconds and bytes. Lab builds read
//! `IMIJ_VALUE_TABLES=auto|off|forced` for the encoder's value-table maps.
use std::io::Write as _;
use std::time::Instant;

fn channels(n: &str) -> imij::Channels {
    match n {
        "1" => imij::Channels::Gray,
        "2" => imij::Channels::GrayAlpha,
        "3" => imij::Channels::Rgb,
        "4" => imij::Channels::Rgba,
        _ => panic!("channels"),
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn encode(list: &str, out: &str, efforts: &[String]) {
    let mut t = std::fs::File::create(out).unwrap();
    writeln!(
        t,
        "tag\tchannels\twidth\theight\teffort\tbytes\tenc_ms\tfeatures"
    )
    .unwrap();
    for line in std::fs::read_to_string(list).unwrap().lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let (w, h): (u32, u32) = (f[2].parse().unwrap(), f[3].parse().unwrap());
        let sample = match f.get(5) {
            None | Some(&"u8") => imij::SampleType::U8,
            Some(&"u16") => imij::SampleType::U16,
            Some(&"f16") => imij::SampleType::F16,
            Some(&"f32") => imij::SampleType::F32,
            Some(o) => panic!("sample type {o}"),
        };
        let mut info = imij::ImageInfo::new(w, h, channels(f[1]), sample);
        if let Some(b) = f.get(6) {
            let n: u8 = b.parse().unwrap();
            let count = info.channels.count();
            info = info.with_significant_bits(&vec![n; count]).unwrap();
        }
        let raw = std::fs::read(f[4]).unwrap();
        assert_eq!(raw.len(), info.row_bytes() * h as usize, "{}", f[0]);
        for e in efforts {
            let cfg = imij::ImiEncoderConfig::default().with_effort(e.parse().unwrap());
            let start = Instant::now();
            let file = cfg
                .encode_samples(
                    imij::ImageRef::new(&raw, info.row_bytes(), &info),
                    &imij::Metadata::default(),
                )
                .unwrap();
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            let (di, back) = imij::decode(&file).unwrap();
            assert!(di == info && back == raw, "{} effort {e} not exact", f[0]);
            let dir = std::path::Path::new(out).parent().unwrap();
            std::fs::write(dir.join(format!("{}.e{e}.imi", f[0])), &file).unwrap();
            writeln!(
                t,
                "{}\t{}\t{w}\t{h}\t{e}\t{}\t{ms:.3}\t{}",
                f[0],
                f[1],
                file.len(),
                imij::header(&file).unwrap().features.bits()
            )
            .unwrap();
        }
        t.flush().unwrap();
    }
}

fn time(file: &str, raw: &str, threads: usize, reps: usize, pulse: bool) {
    let data = std::fs::read(file).unwrap();
    let raw = std::fs::read(raw).unwrap();
    let info = imij::probe(&data).unwrap().0;
    let stride = info.row_bytes();
    let mut out = vec![0u8; raw.len()];
    #[cfg(not(feature = "lab"))]
    assert!(!pulse, "pulse timing needs the lab feature");
    let run = |out: &mut [u8]| {
        #[cfg(feature = "lab")]
        if pulse {
            let tree = how_far_along::PulseTree::new(
                how_far_along::Phase::new("decode", how_far_along::Total::Unknown),
                how_far::Unstoppable,
            );
            imij::lab::decode_into_threads_with_pulse(&data, out, stride, threads, &tree).unwrap();
            tree.finish(how_far::Outcome::Succeeded).unwrap();
            return;
        }
        if threads == 1 {
            imij::decode_into(&data, out, stride).unwrap();
        } else {
            imij::decode_into_threads(&data, out, stride, threads).unwrap();
        }
    };
    run(&mut out);
    assert!(out == raw, "not exact");
    let mut ms = Vec::with_capacity(reps);
    for _ in 0..reps {
        out.fill(0);
        let start = Instant::now();
        run(&mut out);
        ms.push(start.elapsed().as_secs_f64() * 1000.0);
        assert!(out == raw, "not exact (timed)");
    }
    let px = info.width as f64 * info.height as f64;
    let m = median(ms);
    println!(
        "{file}\t{threads}\t{}\t{m:.4}\t{:.1}",
        info.width as u64 * info.height as u64,
        px / (m / 1000.0) / 1e6
    );
}

/// `encode-time FILE RAW REPS`: encodes RAW (the pixels of FILE, an IMI file
/// whose header gives the layout) REPS times with default settings on one
/// thread, checks an exact decode and prints the median milliseconds and
/// the bytes.
fn encode_time(file: &str, raw: &str, reps: usize) {
    let info = imij::probe(&std::fs::read(file).unwrap()).unwrap().0;
    let raw = std::fs::read(raw).unwrap();
    let cfg = imij::ImiEncoderConfig::default();
    let enc = || {
        cfg.encode_samples(
            imij::ImageRef::new(&raw, info.row_bytes(), &info),
            &imij::Metadata::default(),
        )
    };
    let out = enc().unwrap();
    assert!(imij::decode(&out).unwrap().1 == raw, "not exact");
    let mut ms = Vec::with_capacity(reps);
    for _ in 0..reps {
        let start = Instant::now();
        let again = enc().unwrap();
        ms.push(start.elapsed().as_secs_f64() * 1000.0);
        assert!(again == out, "encode not deterministic");
    }
    let px = info.width as f64 * info.height as f64;
    let m = median(ms);
    println!(
        "{file}\t{}\t{m:.3}\t{:.2}",
        out.len(),
        px / (m / 1000.0) / 1e6
    );
}

fn main() {
    // Lab builds: IMIJ_VALUE_TABLES=auto|off|forced selects the encoder's
    // value-table maps (SPEC 2.1 map 9; lab::set_value_tables).
    #[cfg(feature = "lab")]
    if let Ok(mode) = std::env::var("IMIJ_VALUE_TABLES") {
        imij::lab::set_value_tables(match mode.as_str() {
            "auto" => 0,
            "off" => 1,
            "forced" => 2,
            m => panic!("IMIJ_VALUE_TABLES={m}"),
        });
    }
    let a: Vec<String> = std::env::args().collect();
    match a[1].as_str() {
        "encode" => encode(&a[2], &a[3], &a[4..]),
        "time" => time(
            &a[2],
            &a[3],
            a[4].parse().unwrap(),
            a[5].parse().unwrap(),
            a.get(6).is_some_and(|x| x == "pulse"),
        ),
        "encode-time" => encode_time(&a[2], &a[3], a[4].parse().unwrap()),
        _ => panic!(
            "usage: lowbit_eval encode LIST OUT EFFORT... | time FILE RAW THREADS REPS [pulse] | encode-time FILE RAW REPS"
        ),
    }
}
