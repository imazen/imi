//! Callgrind instruction counts of single-threaded lossy encodes with and
//! without the lab trellis (gungraun; deterministic on a loaded machine).
//! `cargo bench -p imij --features lab --bench cg_trellis` (needs
//! `gungraun-runner` 0.20.0). Inputs as `cg_encode`: photo 1407 1024x1024
//! and screen 8115 1024x900 raws from the 2026-10-03 encoder corpus runs,
//! else a synthetic image. Default effort (7.5, below the effort trellis)
//! and stripes; XYB, run-level.

use gungraun::prelude::*;
use imij::{Channels, ImageInfo, ImiEncoderConfig, LossyColor, LossyTrellis, Metadata, SampleType};
use std::hint::black_box;

const PHOTO: (&str, u32, u32) = (
    "local-scratch/zqi-encoder-2026-10-03/speed-run/artifacts/34d882eb7dccf2d341250988620414a71cb184003c09222130d146915172258c.raw",
    1024,
    1024,
);
const SCREEN: (&str, u32, u32) = (
    "local-scratch/zqi-encoder-2026-10-03/speed-run/artifacts/5d6375df00c3b0305332082d269a778f947f2e857a7a5f7b59d233dbd0957dd1.raw",
    1024,
    900,
);

/// Deterministic synthetic RGB8 fallback.
fn synth(w: u32, h: u32) -> Vec<u8> {
    (0..w * h * 3)
        .map(|i| ((i * 37 + i / 1999 * 91 + (i / 3 % w) * 2) % 256) as u8)
        .collect()
}

fn setup(
    (path, w, h): (&str, u32, u32),
    q: f32,
    passes: u8,
) -> (Vec<u8>, u32, u32, ImiEncoderConfig) {
    let px = match std::fs::read(path) {
        Ok(v) if v.len() == (w * h * 3) as usize => v,
        _ => {
            eprintln!("cg_trellis: {path} unavailable; synthetic {w}x{h}");
            synth(w, h)
        }
    };
    let mut cfg = imij::ImiEncoderConfig::lossy(q)
        .tune_lossy(|t| t.color = LossyColor::Xyb)
        .with_threads(1);
    cfg = cfg.tune_lossy(|t| {
        t.lab.trellis = (passes > 0).then(|| LossyTrellis {
            passes,
            ..LossyTrellis::default()
        });
    });
    (px, w, h, cfg)
}

#[library_benchmark]
#[bench::photo_q20_off(setup(PHOTO, 20.0, 0))]
#[bench::photo_q20_trellis(setup(PHOTO, 20.0, 1))]
#[bench::photo_q50_off(setup(PHOTO, 50.0, 0))]
#[bench::photo_q50_trellis(setup(PHOTO, 50.0, 1))]
#[bench::photo_q50_trellis_p2(setup(PHOTO, 50.0, 2))]
#[bench::photo_q85_off(setup(PHOTO, 85.0, 0))]
#[bench::photo_q85_trellis(setup(PHOTO, 85.0, 1))]
#[bench::screen_q20_off(setup(SCREEN, 20.0, 0))]
#[bench::screen_q20_trellis(setup(SCREEN, 20.0, 1))]
#[bench::screen_q85_off(setup(SCREEN, 85.0, 0))]
#[bench::screen_q85_trellis(setup(SCREEN, 85.0, 1))]
fn encode(input: (Vec<u8>, u32, u32, ImiEncoderConfig)) -> Vec<u8> {
    let (px, w, h, cfg) = input;
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    cfg.encode_samples(
        imij::ImageRef::new(black_box(&px), w as usize * 3, &info),
        &Metadata::default(),
    )
    .unwrap()
}

library_benchmark_group!(name = trellis, benchmarks = [encode]);
main!(library_benchmark_groups = trellis);
