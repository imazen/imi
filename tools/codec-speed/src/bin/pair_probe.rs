//! Internal metrics and correctness contract for zenbench PR 33's fresh processes.
#![forbid(unsafe_code)]
use enough::Unstoppable;
use sha2::{Digest, Sha256};
use std::{fs, hint::black_box, time::Instant};
const SIDE: usize = 1024;
fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn tier_config(codec: &str, quality: f32) -> imij::ImiEncoderConfig {
    let cfg = imij::ImiEncoderConfig::lossy(quality)
        .with_threads(1)
        .with_effort(7.5);
    match codec {
        "imij-compact" => cfg.with_decode_speed(imij::DecodeSpeed::Compact),
        "imij-balanced" => cfg.with_decode_speed(imij::DecodeSpeed::Balanced),
        "imij-fast" => cfg.with_decode_speed(imij::DecodeSpeed::Fast),
        "imij-huffman" => cfg.tune_lossy(|t| t.entropy = imij::LossyEntropy::SplitHuf),
        "imij-default" => cfg,
        _ => panic!("unknown policy"),
    }
}

fn is_tier(codec: &str) -> bool {
    matches!(
        codec,
        "imij-compact" | "imij-balanced" | "imij-fast" | "imij-huffman" | "imij-default"
    )
}

fn encode(codec: &str, setting: f32, raw: &[u8], rgb: &[rgb::Rgb<u8>]) -> Vec<u8> {
    let info = imij::ImageInfo::new(
        SIDE as u32,
        SIDE as u32,
        imij::Channels::Rgb,
        imij::SampleType::U8,
    );
    if is_tier(codec) {
        return tier_config(codec, setting)
            .encode_samples(
                imij::ImageRef::new(raw, SIDE * 3, &info),
                &imij::Metadata::default(),
            )
            .unwrap();
    }
    match codec {
        "imij-lossless" => {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.with_effort(setting);
            cfg = cfg.with_threads(1);
            cfg
        }
        .encode_samples(
            imij::ImageRef::new(raw, SIDE * 3, &info),
            &imij::Metadata::default(),
        )
        .unwrap(),
        "qoi" => zenbitmaps::encode_qoi(
            raw,
            SIDE as u32,
            SIDE as u32,
            zenbitmaps::PixelLayout::Rgb8,
            Unstoppable,
        )
        .unwrap(),
        "imij" if setting == 0.0 => imij::ImiEncoderConfig::default()
            .encode_samples(
                imij::ImageRef::new(raw, SIDE * 3, &info),
                &imij::Metadata::default(),
            )
            .unwrap(),
        "imij" => imij::ImiEncoderConfig::lossy(setting)
            .encode_samples(
                imij::ImageRef::new(raw, SIDE * 3, &info),
                &imij::Metadata::default(),
            )
            .unwrap(),
        "zenjpeg" => {
            let cfg = zenjpeg::encoder::EncoderConfig::ycbcr(
                setting,
                zenjpeg::encoder::ChromaSubsampling::Quarter,
            )
            .progressive(false)
            .restart_mcu_rows(4);
            let mut e = cfg
                .encode_from_bytes(
                    SIDE as u32,
                    SIDE as u32,
                    zenjpeg::encoder::PixelLayout::Rgb8Srgb,
                )
                .unwrap();
            e.push_packed(raw, Unstoppable).unwrap();
            e.finish().unwrap()
        }
        "zenpng" => {
            assert!(
                setting.is_finite() && setting.fract() == 0.0 && (0.0..=30.0).contains(&setting)
            );
            let mut cfg = zenpng::EncodeConfig::default()
                .with_compression(zenpng::Compression::Effort(setting as u32));
            cfg.max_threads = 1;
            cfg.parallel = false;
            cfg.decode_segments = 0;
            zenpng::encode_rgb8(
                imgref::ImgRef::new(rgb, SIDE, SIDE),
                None,
                &cfg,
                &Unstoppable,
                &Unstoppable,
            )
            .unwrap()
        }
        _ => panic!("unknown codec"),
    }
}

fn pixels(codec: &str, bytes: &[u8], threads: usize, pool: &rayon::ThreadPool) -> Vec<u8> {
    match codec {
        "qoi" => {
            assert_eq!(threads, 1);
            let output = zenbitmaps::decode_qoi(bytes, Unstoppable).unwrap();
            assert_eq!((output.width, output.height), (SIDE as u32, SIDE as u32));
            assert_eq!(output.layout, zenbitmaps::PixelLayout::Rgb8);
            output.pixels().to_vec()
        }
        "imij" | "imij-lossless" | "imij-compact" | "imij-balanced" | "imij-fast"
        | "imij-huffman" | "imij-default" => {
            let mut p = vec![0; SIDE * SIDE * 3];
            pool.install(|| imij::decode_into_threads(bytes, &mut p, SIDE * 3, threads))
                .unwrap();
            p
        }
        "zenjpeg" => pool
            .install(|| {
                zenjpeg::decoder::Decoder::new()
                    .num_threads(threads)
                    .decode(bytes, Unstoppable)
            })
            .unwrap()
            .into_pixels_u8()
            .unwrap(),
        "zenpng" => zenpng::decode(
            bytes,
            &zenpng::PngDecodeConfig::default().with_max_threads(threads),
            &Unstoppable,
        )
        .unwrap()
        .pixels
        .copy_to_contiguous_bytes(),
        _ => panic!("unknown codec"),
    }
}

fn decode(codec: &str, bytes: &[u8], threads: usize, pool: &rayon::ThreadPool) {
    match codec {
        "qoi" => {
            assert_eq!(threads, 1);
            black_box(zenbitmaps::decode_qoi(bytes, Unstoppable).unwrap());
        }
        "imij" | "imij-lossless" | "imij-compact" | "imij-balanced" | "imij-fast"
        | "imij-huffman" | "imij-default" => {
            let mut p = vec![0; SIDE * SIDE * 3];
            pool.install(|| imij::decode_into_threads(bytes, &mut p, SIDE * 3, threads))
                .unwrap();
            black_box(p);
        }
        "zenjpeg" => {
            black_box(
                pool.install(|| {
                    zenjpeg::decoder::Decoder::new()
                        .num_threads(threads)
                        .decode(bytes, Unstoppable)
                })
                .unwrap(),
            );
        }
        "zenpng" => {
            black_box(
                zenpng::decode(
                    bytes,
                    &zenpng::PngDecodeConfig::default().with_max_threads(threads),
                    &Unstoppable,
                )
                .unwrap(),
            );
        }
        _ => panic!("unknown codec"),
    }
}

fn feed_rows(encoded: &[u8], mut row: impl FnMut(&[u8])) {
    use imij::codec::{DecodeConfig, FeedStatus};
    use zencodec::decode::DecoderConfig;
    let mut decoder = DecodeConfig::default().job().feed_decoder(65536).unwrap();
    let mut at = 0;
    let mut eof = false;
    loop {
        match decoder.advance().unwrap() {
            FeedStatus::NeedInput => {
                assert!(!eof, "explicit EOF must finish");
                if at == encoded.len() {
                    decoder.finish_input().unwrap();
                    eof = true;
                } else {
                    at += decoder.push(&encoded[at..]).unwrap();
                }
            }
            FeedStatus::Row { .. } => row(decoder.row().unwrap().unwrap().row(0)),
            FeedStatus::Complete => break,
            _ => panic!("new feed status requires explicit handling"),
        }
    }
    assert_eq!(decoder.usage().input_bytes, encoded.len() as u64);
}

fn main() {
    let a: Vec<_> = std::env::args().collect();
    assert_eq!(
        a.len(),
        7,
        "pair_probe prepare|encode|decode|reader|feed|profile-encode|profile-decode CODEC SETTING THREADS RAW ENCODED"
    );
    let action = &a[1];
    let codec = &a[2];
    let setting = a[3].parse().unwrap();
    let threads = a[4].parse().unwrap();
    let raw = fs::read(&a[5]).unwrap();
    assert_eq!(raw.len(), SIDE * SIDE * 3);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(8)
        .build()
        .unwrap();
    if action == "prepare" {
        assert_eq!(threads, 1);
        assert!(matches!(codec.as_str(), "imij-lossless" | "qoi") || is_tier(codec));
        let rgb = Vec::new(); // These byte-input IMIJ/QOI encoders do not use RGB structs.
        let start = Instant::now();
        let encoded = encode(codec, setting, &raw, &rgb);
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        let reference = if is_tier(codec) {
            pixels(codec, &encoded, 1, &pool)
        } else {
            raw.clone()
        };
        for &threads in if codec == "qoi" {
            &[1][..]
        } else {
            &[1, 2, 4, 8][..]
        } {
            assert_eq!(pixels(codec, &encoded, threads, &pool), reference);
        }
        use std::io::Write;
        let mut output = fs::File::create_new(&a[6]).unwrap();
        output.write_all(&encoded).unwrap();
        println!("encode_total_ms={ms}");
        println!("encoded_bytes={}", encoded.len());
        println!("source_sha256={}", digest(&raw));
        println!("encoded_sha256={}", digest(&encoded));
        return;
    }
    let encoded = fs::read(&a[6]).unwrap();
    if action == "feed" {
        assert_eq!(codec, "imij-lossless");
        assert_eq!(threads, 1);
        let mut actual = Vec::new();
        feed_rows(&encoded, |row| actual.extend_from_slice(row));
        assert_eq!(actual, raw);
        let read = || {
            feed_rows(&encoded, |row| {
                black_box(row);
            })
        };
        for _ in 0..2 {
            read();
        }
        let start = Instant::now();
        for _ in 0..10 {
            read();
        }
        println!(
            "elapsed_ms={}",
            start.elapsed().as_secs_f64() * 1000.0 / 10.0
        );
        println!(
            "check={{\"sha256\":\"{}\",\"bytes\":{}}}",
            digest(&actual),
            actual.len()
        );
        return;
    }
    if action == "reader" {
        assert_eq!(codec, "imij-lossless");
        assert_eq!(threads, 1);
        let mut actual = Vec::new();
        imij::decode_reader(encoded.as_slice(), &imij::Limits::DEFAULT, |_, _, row| {
            actual.extend_from_slice(row);
            Ok(())
        })
        .unwrap();
        assert_eq!(actual, raw);
        let read = || {
            imij::decode_reader(encoded.as_slice(), &imij::Limits::DEFAULT, |_, _, row| {
                black_box(row);
                Ok(())
            })
            .unwrap();
        };
        for _ in 0..2 {
            read();
        }
        let start = Instant::now();
        for _ in 0..10 {
            read();
        }
        println!(
            "elapsed_ms={}",
            start.elapsed().as_secs_f64() * 1000.0 / 10.0
        );
        println!(
            "check={{\"sha256\":\"{}\",\"bytes\":{}}}",
            digest(&actual),
            actual.len()
        );
        return;
    }
    if action == "profile-encode" || action == "profile-decode" {
        assert_eq!(threads, 1, "diagnostic profile is single-threaded");
        let rgb: Vec<_> = raw
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| rgb::Rgb::new(p[0], p[1], p[2]))
            .collect();
        let reference = pixels(codec, &encoded, threads, &pool);
        if matches!(codec.as_str(), "zenpng" | "imij-lossless" | "qoi")
            || (codec == "imij" && setting == 0.0)
        {
            assert_eq!(reference, raw);
        }
        assert_eq!(encode(codec, setting, &raw, &rgb), encoded);
        let start = Instant::now();
        let mut iterations = 0;
        while start.elapsed().as_secs_f64() < 2.0 {
            if action == "profile-encode" {
                black_box(encode(codec, setting, &raw, &rgb));
            } else {
                decode(codec, &encoded, threads, &pool);
            }
            iterations += 1;
        }
        println!("diagnostic_seconds={}", start.elapsed().as_secs_f64());
        println!("diagnostic_iterations={iterations}");
        println!("reference_sha256={}", digest(&reference));
        return;
    }
    if action == "encode" {
        assert_eq!(threads, 1);
        let rgb: Vec<_> = raw
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| rgb::Rgb::new(p[0], p[1], p[2]))
            .collect();
        let warm_start = Instant::now();
        let warm = encode(codec, setting, &raw, &rgb);
        let warm_ms = warm_start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(warm, encoded, "warm encode differs from retained fixture");
        drop(warm);
        let start = Instant::now();
        let actual = encode(codec, setting, &raw, &rgb);
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(
            actual, encoded,
            "measured encode differs from retained fixture"
        );
        println!("elapsed_ms={ms}");
        println!("encode_total_ms={}", warm_ms + ms);
        println!(
            "check={{\"source_sha256\":\"{}\",\"bytes\":{},\"matches_prepared\":true}}",
            digest(&raw),
            raw.len()
        );
    } else {
        assert_eq!(action, "decode");
        let p = pixels(codec, &encoded, threads, &pool);
        assert_eq!(p.len(), raw.len());
        if matches!(codec.as_str(), "zenpng" | "imij-lossless" | "qoi")
            || (codec == "imij" && setting == 0.0)
        {
            assert_eq!(p, raw);
        }
        if is_tier(codec) {
            assert_eq!(
                p,
                pixels(codec, &encoded, 1, &pool),
                "threaded pixels differ from serial"
            );
        }
        let sha = digest(&p);
        drop(p);
        // Three warmups per fresh process. The first above also validates pixels.
        for _ in 0..2 {
            decode(codec, &encoded, threads, &pool);
        }
        let start = Instant::now();
        for _ in 0..10 {
            decode(codec, &encoded, threads, &pool);
        }
        println!(
            "elapsed_ms={}",
            start.elapsed().as_secs_f64() * 1000.0 / 10.0
        );
        println!("check={{\"sha256\":\"{sha}\",\"bytes\":{}}}", raw.len());
    }
}
