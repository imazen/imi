//! Lossy table counts chosen by coded size (benchmarks/coefgap_2026-10-09):
//! on by default from effort 7.5 for run-level and split-Huffman files (the
//! `lab::set_table_k_mode(3)` rule), the effort's budgets below 7.5 and for
//! split (its tables are never searched); `lab::set_table_k_mode(0)` turns
//! it off for A/B. One test,
//! so no other test of this binary encodes while the process-global switch is
//! set; it ends at the default.
#![cfg(feature = "lab")]

mod common;

use common::matrix::{Case, observe_all};
use imij::{
    Channels, DecodeSpeed, ImageInfo, ImiEncoderConfig, Limits, LossyColor, LossyEntropy, Metadata,
    SampleType,
};

/// Smooth ramps, texture and hard edges (as `tests/trellis_lab_switches.rs`).
fn image(info: &ImageInfo) -> Vec<u8> {
    let (w, h, n) = (
        info.width as usize,
        info.height as usize,
        info.channels.count(),
    );
    let mut px = vec![0u8; info.row_bytes() * h];
    for y in 0..h {
        for x in 0..w {
            for c in 0..n {
                let ramp = (x * 3 + y * 2 + c * 40) as f32;
                let tex = 25.0 * ((x as f32 * 0.9 + c as f32).sin() * (y as f32 * 0.7).cos());
                let edge = if (x / 11 + y / 7) % 3 == 0 { 60.0 } else { 0.0 };
                px[(y * w + x) * n + c] = ((ramp + tex + edge) % 256.0).clamp(0.0, 255.0) as u8;
            }
        }
    }
    px
}

/// The configurations the study measured (run-level XYB with and without
/// seeds, run-level YCoCg, split-Huffman, the tiers) and split, which it
/// did not, with whether production searches them.
fn configs(q: f32, effort: f32) -> Vec<(&'static str, ImiEncoderConfig, bool)> {
    let base = || imij::ImiEncoderConfig::lossy(q).with_effort(effort);
    let mut noseed = base();
    noseed = noseed.tune_lossy(|t| t.lab.seed_bank = 0);
    vec![
        ("xyb run-level", base(), true),
        ("xyb run-level, no seeds", noseed, true),
        (
            "ycocg run-level",
            base().tune_lossy(|t| t.color = LossyColor::YCoCg),
            true,
        ),
        (
            "xyb split-Huffman",
            base().tune_lossy(|t| t.entropy = LossyEntropy::SplitHuf),
            true,
        ),
        (
            "compact",
            base().with_decode_speed(DecodeSpeed::Compact),
            true,
        ),
        (
            "balanced",
            base().with_decode_speed(DecodeSpeed::Balanced),
            true,
        ),
        (
            "fastest",
            base().with_decode_speed(DecodeSpeed::Fastest),
            true,
        ),
    ]
}

fn encode(px: &[u8], info: &ImageInfo, cfg: &ImiEncoderConfig) -> Vec<u8> {
    cfg.encode_samples(
        imij::ImageRef::new(px, info.row_bytes(), info),
        &Metadata::default(),
    )
    .unwrap()
}

/// Every decode route of the cross-route matrix (`common::matrix::ROUTES`)
/// and every whole-image entry point (`common::decode_all`) give the same
/// pixels; returns them.
fn decoded_everywhere(name: &str, file: &[u8], info: &ImageInfo) -> Vec<u8> {
    let reference = common::decode_all(file, info);
    let case = Case {
        name: name.into(),
        file: file.to_vec(),
        info: info.clone(),
        reference: reference.clone(),
        frames: None,
        limits: Limits::DEFAULT,
    };
    for (route, outcome) in observe_all(&case) {
        assert!(outcome == "ok", "{name}: route {route}: {outcome}");
    }
    reference
}

#[test]
fn table_counts_by_coded_size() {
    // Small images, where the tables weigh (64 px: the study's largest gain).
    let shapes = [(64, 48), (37, 64)];
    let (mut searched, mut run_level, mut run_level_changed) = (0, 0, 0);
    for (w, h) in shapes {
        let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
        let px = image(&info);
        for q in [20.0, 70.0] {
            for effort in [0.0, 5.0, 7.4, 7.5, 9.0, 10.0] {
                for (name, cfg, production) in configs(q, effort) {
                    let what = format!("{name} {w}x{h} q{q} effort {effort}");
                    imij::lab::reset_table_k_mode();
                    let default = encode(&px, &info, &cfg);
                    imij::lab::set_table_k_mode(0);
                    let budgets = encode(&px, &info, &cfg);
                    imij::lab::set_table_k_mode(3);
                    let mode3 = encode(&px, &info, &cfg);
                    imij::lab::reset_table_k_mode();
                    if effort < 7.5 || !production {
                        assert_eq!(default, budgets, "{what}: the budgets unless searched");
                        continue;
                    }
                    assert_eq!(default, mode3, "{what}: production is lab mode 3");
                    searched += (default != budgets) as usize;
                    let px_default = decoded_everywhere(&what, &default, &info);
                    if effort < 9.0 {
                        // Below the trellis the coefficients do not depend on
                        // the tables: a pure re-coding.
                        let px_budgets = decoded_everywhere(&what, &budgets, &info);
                        assert!(px_default == px_budgets, "{what}: pixels changed");
                    }
                    if name.ends_with("run-level") {
                        run_level += 1;
                        run_level_changed += (default != budgets) as usize;
                    }
                }
            }
        }
    }
    eprintln!("searched files changed: {searched}; run-level {run_level_changed} of {run_level}");
    // On these small images the run-level search changes every file (2026-10-09:
    // 24 of 24; 76 searched files of all coders changed).
    assert_eq!(
        run_level_changed, run_level,
        "a run-level file kept its budgets"
    );
}
