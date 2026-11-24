//! Callgrind instruction counts of single-threaded encodes (gungraun):
//! deterministic, so kernel changes show up even on a loaded machine.
//! Run with `cargo bench -p imij --bench cg_encode` (needs
//! `gungraun-runner` 0.20.0 installed).
//!
//! Real corpus input: the defaults below load the copied RGB8 raws from the
//! 2026-10-03 encoder corpus runs (`imij-encoder-2026-10-03/speed-run`), and
//! `IMIJ_CG_RAW="path:WIDTHxHEIGHT"` overrides them with any interleaved
//! RGB8 file. When a file is absent a deterministic synthetic image is
//! generated instead, so the bench still runs elsewhere.

use gungraun::prelude::*;
use imij::{Channels, ImageInfo, ImiEncoderConfig, LossyColor, LossyEntropy, Metadata, SampleType};
use std::hint::black_box;

/// photo 1407 cropped to 1024x1024 (case `0:` of the saved index).
const PHOTO: (&str, u32, u32) = (
    "local-scratch/zqi-encoder-2026-10-03/speed-run/artifacts/34d882eb7dccf2d341250988620414a71cb184003c09222130d146915172258c.raw",
    1024,
    1024,
);
/// screen 8115 cropped to 1024x900 (case `3:` of the saved index).
const SCREEN: (&str, u32, u32) = (
    "local-scratch/zqi-encoder-2026-10-03/speed-run/artifacts/5d6375df00c3b0305332082d269a778f947f2e857a7a5f7b59d233dbd0957dd1.raw",
    1024,
    900,
);

/// Deterministic synthetic RGB8 fallback (same generator as cg_decode).
fn synth(w: u32, h: u32) -> Vec<u8> {
    let mut s = 0x9E37_79B9u32;
    let mut px = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let n = (s % 13) as f32 - 6.0;
            let (fx, fy) = (x as f32, y as f32);
            let tex = 30.0 * ((fx / 7.0).sin() * (fy / 5.0).cos());
            let disc = ((fx - 300.0).powi(2) + (fy - 150.0).powi(2)) < 80.0 * 80.0;
            let edge = if disc { 70.0 } else { 0.0 };
            let r = fx * 0.4 + tex + edge + n;
            let g = fy * 0.5 - tex * 0.5 + n;
            let b = 120.0 + (fx + fy) * 0.15 + edge * 0.6 - n;
            px.extend([r, g, b].map(|v| v.clamp(0.0, 255.0) as u8));
        }
    }
    px
}

/// Loads `path:w x h` RGB8 (env `IMIJ_CG_RAW` overrides everything), else
/// the synthetic fallback.
fn pixels((path, w, h): (&str, u32, u32), fallback_wh: (u32, u32)) -> (Vec<u8>, u32, u32) {
    let spec = std::env::var("IMIJ_CG_RAW").ok();
    let (p, w, h) = match spec.as_deref().and_then(|s| s.split_once(':')) {
        Some((p, dim)) => {
            let (a, b) = dim.split_once('x').expect("IMIJ_CG_RAW wants path:WxH");
            (
                p.to_string(),
                a.parse().expect("IMIJ_CG_RAW width"),
                b.parse().expect("IMIJ_CG_RAW height"),
            )
        }
        None => (path.to_string(), w, h),
    };
    let need = (w * h * 3) as usize;
    match std::fs::read(&p) {
        Ok(v) if v.len() == need => (v, w, h),
        Ok(v) => panic!("{p}: {need} bytes expected, {} read", v.len()),
        Err(e) => {
            eprintln!(
                "cg_encode: {p} unreadable ({e}); synthetic {}x{}",
                fallback_wh.0, fallback_wh.1
            );
            (
                synth(fallback_wh.0, fallback_wh.1),
                fallback_wh.0,
                fallback_wh.1,
            )
        }
    }
}

fn info(w: u32, h: u32) -> ImageInfo {
    ImageInfo::new(w, h, Channels::Rgb, SampleType::U8)
}

fn lossy(
    at: (&str, u32, u32),
    q: f32,
    entropy: LossyEntropy,
) -> (Vec<u8>, u32, u32, ImiEncoderConfig) {
    let (px, w, h) = pixels(at, (1024, 768));
    let cfg = imij::ImiEncoderConfig::lossy(q)
        .tune_lossy(|t| t.color = LossyColor::Xyb)
        .tune_lossy(|t| t.entropy = entropy)
        .with_threads(1);
    (px, w, h, cfg)
}

fn lossless(at: (&str, u32, u32)) -> (Vec<u8>, u32, u32, ImiEncoderConfig) {
    let (px, w, h) = pixels(at, (1024, 768));
    (px, w, h, imij::ImiEncoderConfig::default())
}

#[library_benchmark]
#[bench::photo_runlevel(lossy(PHOTO, 85.0, LossyEntropy::RunLevel))]
#[bench::photo_huffman(lossy(PHOTO, 85.0, LossyEntropy::SplitHuf))]
#[bench::screen_runlevel(lossy(SCREEN, 85.0, LossyEntropy::RunLevel))]
fn encode_lossy(input: (Vec<u8>, u32, u32, ImiEncoderConfig)) -> Vec<u8> {
    let (px, w, h, cfg) = input;
    let i = info(w, h);
    cfg.encode_samples(
        imij::ImageRef::new(black_box(&px), w as usize * 3, &i),
        &Metadata::default(),
    )
    .unwrap()
}

#[library_benchmark]
#[bench::photo_lossless(lossless(PHOTO))]
#[bench::screen_lossless(lossless(SCREEN))]
fn encode_lossless(input: (Vec<u8>, u32, u32, ImiEncoderConfig)) -> Vec<u8> {
    let (px, w, h, cfg) = input;
    let i = info(w, h);
    cfg.encode_samples(
        imij::ImageRef::new(black_box(&px), w as usize * 3, &i),
        &Metadata::default(),
    )
    .unwrap()
}

library_benchmark_group!(name = lossy, benchmarks = [encode_lossy]);
library_benchmark_group!(name = lossless, benchmarks = [encode_lossless]);

main!(library_benchmark_groups = lossy, lossless);
