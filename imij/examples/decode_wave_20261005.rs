//! decode_wave_20261005 sentinel + instrumentation harness (lane perf).
//!
//! `encode <raw> <w> <h> <channels> <sample> <mode> <color> <q> <enc_threads> <out.imi> [cicp=p,t,m,f] [sbit=b,b,b]`
//!   frozen-geometry encode of a raw sentinel source. `sample` is `u8` or
//!   `u16` (native-endian), `mode` `runlevel|splithuf`, `color` `xyb|ycocg`.
//!   `cicp` sets the exact H.273 code points (primaries, transfer, matrix,
//!   full_range) and `sbit` the per-channel source significant bits, both
//!   verbatim into ImageInfo. All input bounds are checked explicitly.
//! `pixels <threads> <file.imi>...`
//!   decode_into_threads; prints geometry + an fnv1a64 of the decoded
//!   pixels per file (parity is by sha256 of `dechash` output or `cmp`).
//! `stats <file.imi>...`  (lab builds)
//!   token mix / EOB histogram. Data only — no timing. Native timing is
//!   scheduled by the controller.
//!
//! No new dependencies; imij + std only.

use std::fmt::Write as _;

fn fnv1a64(b: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &v in b {
        h = (h ^ v as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn info(w: u32, h: u32, channels: u64, sample: u64) -> imij::ImageInfo {
    let ch = match channels {
        1 => imij::Channels::Gray,
        2 => imij::Channels::GrayAlpha,
        3 => imij::Channels::Rgb,
        4 => imij::Channels::Rgba,
        _ => panic!("channels must be 1..=4"),
    };
    let s = match sample {
        0 | 8 => imij::SampleType::U8,
        1 | 16 => imij::SampleType::U16,
        _ => panic!("sample must be u8 or u16"),
    };
    imij::ImageInfo::new(w, h, ch, s)
}

fn encode(args: &[String]) {
    let (raw, w, h) = (
        &args[0],
        args[1].parse::<u32>().unwrap(),
        args[2].parse::<u32>().unwrap(),
    );
    let (channels, sample) = (
        args[3].parse::<u64>().unwrap(),
        args[4].parse::<u64>().unwrap(),
    );
    let mut info = info(w, h, channels, sample);
    for kv in &args[10..] {
        let (k, v) = kv.split_once('=').expect("extra args are key=value");
        let f: Vec<u64> = v.split(',').map(|s| s.parse().unwrap()).collect();
        match k {
            "cicp" if f.len() == 4 => {
                info.cicp = imij::Cicp {
                    color_primaries: f[0] as u8,
                    transfer_characteristics: f[1] as u8,
                    matrix_coefficients: f[2] as u8,
                    full_range: f[3] != 0,
                };
            }
            "sbit" => {
                let bits: Vec<u8> = f.into_iter().map(|n| n as u8).collect();
                info = info.with_significant_bits(&bits).unwrap();
            }
            _ => panic!("unknown encode extra {kv}: expected cicp=|sbit="),
        }
    }
    let px = std::fs::read(raw).unwrap();
    let rb = info.row_bytes();
    assert_eq!(
        px.len(),
        rb * h as usize,
        "{raw}: {} bytes for {w}x{h} (row_bytes {rb}), expected {}",
        px.len(),
        rb * h as usize
    );
    let mut cfg = imij::ImiEncoderConfig::lossy(args[7].parse::<f32>().unwrap());
    cfg = cfg.tune_lossy(|t| {
        t.entropy = match args[5].as_str() {
            "runlevel" => imij::LossyEntropy::RunLevel,
            "splithuf" => imij::LossyEntropy::SplitHuf,
            m => panic!("mode {m}: expected runlevel|splithuf"),
        };
    });
    cfg = cfg.tune_lossy(|t| {
        t.color = match args[6].as_str() {
            "xyb" => imij::LossyColor::Xyb,
            "ycocg" => imij::LossyColor::YCoCg,
            c => panic!("color {c}: expected xyb|ycocg"),
        };
    });
    cfg = cfg.with_threads(args[8].parse::<usize>().unwrap());
    let enc = cfg
        .encode_samples(
            imij::ImageRef::new(&px, rb, &info),
            &imij::Metadata::default(),
        )
        .unwrap();
    std::fs::write(&args[9], &enc).unwrap();
    println!(
        "{}: {} bytes, fnv={:016x} ({} {} q{} {} {}t cicp={},{},{},{} sbit={:?})",
        args[9],
        enc.len(),
        fnv1a64(&enc),
        args[5],
        args[6],
        args[7],
        args[5],
        args[8],
        info.cicp.color_primaries,
        info.cicp.transfer_characteristics,
        info.cicp.matrix_coefficients,
        u8::from(info.cicp.full_range),
        (0..info.channels.count())
            .map(|c| info.channel_bits(c).unwrap())
            .collect::<Vec<_>>()
    );
}

fn pixels(args: &[String]) {
    let threads = args[0].parse::<usize>().unwrap();
    for path in &args[1..] {
        let data = std::fs::read(path).unwrap();
        let (info, _) = imij::decode(&data).unwrap();
        let mut buf = vec![0u8; info.row_bytes() * info.height as usize];
        imij::decode_into_threads(&data, &mut buf, info.row_bytes(), threads).unwrap();
        let mut line = String::new();
        write!(
            line,
            "{path}: {}x{} {:?} {:?} cicp={},{},{},{} sbit={:?} fnv1a64={:016x}",
            info.width,
            info.height,
            info.channels,
            info.sample,
            info.cicp.color_primaries,
            info.cicp.transfer_characteristics,
            info.cicp.matrix_coefficients,
            u8::from(info.cicp.full_range),
            (0..info.channels.count())
                .map(|c| info.channel_bits(c).unwrap())
                .collect::<Vec<_>>(),
            fnv1a64(&buf)
        )
        .unwrap();
        println!("{line}");
    }
}

#[cfg(feature = "lab")]
fn stats(args: &[String]) {
    for path in args {
        let data = std::fs::read(path).unwrap();
        print!("{path}: {}", imij::lab::lossy_token_mix(&data));
    }
}

fn raws(args: &[String]) {
    let (dir, threads) = (&args[0], args[1].parse::<usize>().unwrap());
    std::fs::create_dir_all(dir).unwrap();
    for path in &args[2..] {
        let data = std::fs::read(path).unwrap();
        let (info, _) = imij::decode(&data).unwrap();
        let mut buf = vec![0u8; info.row_bytes() * info.height as usize];
        imij::decode_into_threads(&data, &mut buf, info.row_bytes(), threads).unwrap();
        let name = std::path::Path::new(path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let out = std::path::Path::new(dir).join(name + ".raw");
        std::fs::write(&out, &buf).unwrap();
        println!("{}: {} bytes", out.display(), buf.len());
    }
}

#[cfg(feature = "lab")]
fn counts(args: &[String]) {
    for path in args {
        let data = std::fs::read(path).unwrap();
        match imij::lab::lossy_counts_histogram(&data) {
            Ok(s) => println!("{path}:\n{s}"),
            Err(e) => println!("{path}: {e}"),
        }
    }
}

#[cfg(feature = "lab")]
fn dump(args: &[String]) {
    let dir = &args[0];
    for path in &args[1..] {
        let data = std::fs::read(path).unwrap();
        let tag = std::path::Path::new(path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .replace(['.', '/'], "_");
        print!("{}", imij::lab::lossy_dump_streams(&data, dir, &tag));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("encode") => encode(&args[1..]),
        Some("pixels") => pixels(&args[1..]),
        Some("raws") => raws(&args[1..]),
        #[cfg(feature = "lab")]
        Some("stats") => stats(&args[1..]),
        #[cfg(feature = "lab")]
        Some("dump") => dump(&args[1..]),
        #[cfg(feature = "lab")]
        Some("counts") => counts(&args[1..]),
        #[cfg(feature = "lab")]
        Some("flat") => {
            for path in &args[1..] {
                let data = std::fs::read(path).unwrap();
                match imij::lab::lossy_flat_share(&data) {
                    Ok(s) => println!("{path}: {s}"),
                    Err(e) => println!("{path}: {e}"),
                }
            }
        }
        _ => eprintln!(
            "{}",
            std::env::var("CARGO_PKG_NAME").unwrap_or_else(|_| "?".into())
        ),
    }
}
