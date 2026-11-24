//! Callgrind instruction counts of single-threaded decodes (gungraun):
//! deterministic, so kernel changes show up even on a loaded machine.
//! Run with `cargo bench -p imij --bench cg_decode` (needs
//! `gungraun-runner` 0.20.0 installed).
//!
//! `decode`: the whole image on one thread (512x384, and 64x64 for the
//! fixed per-call costs). `stream` and `reader`: the row routes
//! (`StreamDecoder`, `decode_reader`), whose drivers differ from the
//! whole-image one. `sb_*`: Compact files, which merge blocks (superblocks,
//! SPEC 2.9), so the merged-block kernel and traversal run.

use gungraun::prelude::*;
use imij::{
    Channels, DecodeSpeed, Feature, ImageInfo, LossyColor, LossyEntropy, Metadata, SampleType,
};
use std::hint::black_box;

/// Photo-like synthetic RGB8: smooth gradients, a sinusoidal texture,
/// hard-edged shapes and noise (deterministic).
fn image(w: u32, h: u32) -> Vec<u8> {
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

/// An encoded `w` x `h` image, its row bytes and an output buffer.
struct Input {
    enc: Vec<u8>,
    rb: usize,
    out: Vec<u8>,
}

fn input(w: u32, h: u32, lossy: Option<(f32, LossyColor)>) -> Input {
    input_ent(
        w,
        h,
        lossy.map(|(q, color)| (q, color, LossyEntropy::RunLevel)),
    )
}

fn input_ent(w: u32, h: u32, lossy: Option<(f32, LossyColor, LossyEntropy)>) -> Input {
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let px = image(w, h);
    let rb = w as usize * 3;
    let enc = match lossy {
        Some((q, color, entropy)) => {
            let cfg = imij::ImiEncoderConfig::lossy(q)
                .tune_lossy(|t| t.color = color)
                .tune_lossy(|t| t.entropy = entropy);
            cfg.encode_samples(imij::ImageRef::new(&px, rb, &info), &Metadata::default())
        }
        None => imij::ImiEncoderConfig::default()
            .encode_samples(imij::ImageRef::new(&px, rb, &info), &Metadata::default()),
    }
    .unwrap();
    Input {
        enc,
        rb,
        out: vec![0u8; rb * h as usize],
    }
}

/// A Compact file (it merges blocks from 128 px).
fn input_compact(w: u32, h: u32, q: f32, color: LossyColor) -> Input {
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let px = image(w, h);
    let rb = w as usize * 3;
    // The tier sets a color; `color` overrides it.
    let cfg = imij::ImiEncoderConfig::lossy(q)
        .with_decode_speed(DecodeSpeed::Compact)
        .tune_lossy(|t| t.color = color);
    let enc = cfg
        .encode_samples(imij::ImageRef::new(&px, rb, &info), &Metadata::default())
        .unwrap();
    let features = imij::header(&enc).unwrap().features;
    assert!(
        features.contains(Feature::Superblocks),
        "a file that merges"
    );
    Input {
        enc,
        rb,
        out: vec![0u8; rb * h as usize],
    }
}

#[library_benchmark]
#[bench::xyb_q50(input(512, 384, Some((50.0, LossyColor::Xyb))))]
#[bench::xyb_q85(input(512, 384, Some((85.0, LossyColor::Xyb))))]
#[bench::xyb_q95(input(512, 384, Some((95.0, LossyColor::Xyb))))]
#[bench::xyb_q85_shuf(input_ent(512, 384, Some((85.0, LossyColor::Xyb, LossyEntropy::SplitHuf))))]
#[bench::ycocg_q85(input(512, 384, Some((85.0, LossyColor::YCoCg))))]
#[bench::lossless(input(512, 384, None))]
#[bench::xyb_q85_64(input(64, 64, Some((85.0, LossyColor::Xyb))))]
#[bench::lossless_64(input(64, 64, None))]
#[bench::sb_xyb_q50(input_compact(512, 384, 50.0, LossyColor::Xyb))]
#[bench::sb_xyb_q85(input_compact(512, 384, 85.0, LossyColor::Xyb))]
#[bench::sb_ycocg_q50(input_compact(512, 384, 50.0, LossyColor::YCoCg))]
fn decode(i: Input) -> Vec<u8> {
    let Input { enc, rb, mut out } = i;
    imij::decode_into_threads(black_box(&enc), &mut out, rb, 1).unwrap();
    out
}

#[library_benchmark]
#[bench::xyb_q85(input(512, 384, Some((85.0, LossyColor::Xyb))))]
#[bench::lossless(input(512, 384, None))]
#[bench::xyb_q85_64(input(64, 64, Some((85.0, LossyColor::Xyb))))]
#[bench::lossless_64(input(64, 64, None))]
#[bench::sb_xyb_q50(input_compact(512, 384, 50.0, LossyColor::Xyb))]
fn stream(i: Input) -> Vec<u8> {
    let Input { enc, rb, mut out } = i;
    let mut d = imij::StreamDecoder::new(black_box(&enc)).unwrap();
    for row in out.chunks_exact_mut(rb) {
        assert!(d.next_row(row).unwrap());
    }
    out
}

#[library_benchmark]
#[bench::xyb_q85(input(512, 384, Some((85.0, LossyColor::Xyb))))]
#[bench::lossless(input(512, 384, None))]
#[bench::xyb_q85_64(input(64, 64, Some((85.0, LossyColor::Xyb))))]
#[bench::lossless_64(input(64, 64, None))]
#[bench::sb_xyb_q50(input_compact(512, 384, 50.0, LossyColor::Xyb))]
fn reader(i: Input) -> Vec<u8> {
    let Input { enc, rb, mut out } = i;
    imij::decode_reader(
        black_box(enc.as_slice()),
        &imij::Limits::DEFAULT,
        |_, y, row| {
            out[y as usize * rb..][..rb].copy_from_slice(row);
            Ok(())
        },
    )
    .unwrap();
    out
}

library_benchmark_group!(name = decoding, benchmarks = [decode, stream, reader]);

main!(library_benchmark_groups = decoding);
