//! Boundary coverage for the rANS lane walker's zero-run lookahead:
//! `run_min` at 0 / 1 / 4 / RUN_CAP / RUN_CAP+1 over planes whose runs
//! reach, straddle, and exceed `RUN_CAP` (1 << 15). `stripe_rows` is set
//! so a stripe's lane holds ~65536 elements (the default 64-row stripe
//! would stay far below the cap), and `palette` is off so zero-dominant
//! inputs cannot detour through the palette lane path. Each case must
//! round trip exactly and produce identical bytes at 1 and 4 threads.

use imij::{Channels, ImageInfo, ImiEncoderConfig, Metadata, SampleType, decode};

const RUN_CAP: u32 = 1 << 15;

fn cfg(run_min: u32, threads: usize) -> ImiEncoderConfig {
    let mut cfg = imij::ImiEncoderConfig::default();
    cfg = cfg.tune_lossless(|t| {
        t.run_min = run_min;
    });
    cfg = cfg.with_threads(threads);
    cfg = cfg.tune_lossless(|t| {
        t.stripe_rows = Some(1024);
    });
    cfg = cfg.tune_lossless(|t| {
        t.palette = false;
    });
    cfg
}

fn rt(px: &[u8], w: u32, h: u32, c: &ImiEncoderConfig) -> Vec<u8> {
    let info = ImageInfo::new(w, h, Channels::Gray, SampleType::U8);
    let enc = c
        .encode_samples(
            imij::ImageRef::new(px, w as usize, &info),
            &Metadata::default(),
        )
        .unwrap();
    let (_, dec) = decode(&enc).unwrap();
    assert_eq!(
        dec,
        px,
        "roundtrip {w}x{h} run_min={}",
        c.lossless_tuning().cloned().unwrap_or_default().run_min
    );
    enc
}

/// Mostly-zero plane with nonzero markers around the RUN_CAP boundary in
/// raster order (lane order is strided but still runs long).
fn straddle(w: u32, h: u32) -> Vec<u8> {
    let mut px = vec![0u8; (w * h) as usize];
    let cap = RUN_CAP as usize;
    let mut pos = cap - 1;
    for (i, &len) in [cap - 1, cap, cap + 1, 4096usize].iter().enumerate() {
        if pos < px.len() {
            px[pos] = 8 + i as u8;
        }
        pos += len;
    }
    px
}

#[test]
fn run_min_boundary_roundtrips() {
    // With stripe_rows = h and lw = w / 16: 1024x1024 -> 65536 lane
    // elements (RUN_CAP hit twice), 1024x512 -> 32768 (exactly the cap).
    let cases: Vec<(Vec<u8>, u32, u32)> = vec![
        (vec![0u8; 1024 * 1024], 1024, 1024),
        (vec![0u8; 1024 * 512], 1024, 512),
        (straddle(1024, 1024), 1024, 1024),
        (
            {
                let mut px = vec![0u8; 512 * 512];
                for (i, p) in px.iter_mut().enumerate() {
                    if i % 8192 == 37 {
                        *p = (i % 251) as u8 + 1;
                    }
                }
                px
            },
            512,
            512,
        ),
    ];
    for run_min in [0, 1, 4, RUN_CAP - 1, RUN_CAP, RUN_CAP + 1] {
        for (px, w, h) in &cases {
            let a = rt(px, *w, *h, &cfg(run_min, 1));
            let b = rt(px, *w, *h, &cfg(run_min, 4));
            assert_eq!(a, b, "1t vs 4t bytes differ, run_min={run_min}");
        }
    }
}

#[test]
fn run_min_never_means_no_run_tokens() {
    // run_min > RUN_CAP can never fire (a run token caps at RUN_CAP), and
    // run_min = 0 disables run coding; all must still round trip.
    let px = vec![0u8; 1024 * 768];
    for run_min in [0, RUN_CAP + 1, u32::MAX] {
        rt(&px, 1024, 768, &cfg(run_min, 1));
    }
}
