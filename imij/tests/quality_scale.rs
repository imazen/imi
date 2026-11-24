//! `LossyQuality`: scale pass-through, calibrated tables, encode-time resolution.

use imij::{
    ButteraugliNorm, Channels, DecodeSpeed, ImageInfo, ImiEncoderConfig, LossyColor, LossyQuality,
    Metadata, SampleType, decode,
};

fn image() -> (ImageInfo, Vec<u8>) {
    let info = ImageInfo::new(61, 43, Channels::Rgb, SampleType::U8);
    let px = (0..61 * 43)
        .flat_map(|i| {
            let (x, y) = (i % 61, i / 61);
            let n = (x * 7 + y * 3 + (x * y) % 11) as u8;
            [n, n.wrapping_mul(3).wrapping_add(y as u8), 255 - n / 2]
        })
        .collect();
    (info, px)
}

fn enc(cfg: &ImiEncoderConfig) -> Vec<u8> {
    let (info, px) = image();
    cfg.encode_samples(
        imij::ImageRef::new(&px, info.row_bytes(), &info),
        &Metadata::default(),
    )
    .unwrap()
}

const ALL: [LossyColor; 2] = [LossyColor::Xyb, LossyColor::YCoCg];

fn ba(distance: f32, norm: ButteraugliNorm) -> LossyQuality {
    LossyQuality::Butteraugli { distance, norm }
}

#[test]
fn bare_number_is_the_scale_unchanged() {
    for q in [1.0, 7.5, 30.0, 50.0, 90.0, 104.0] {
        for color in ALL {
            let a = imij::ImiEncoderConfig::lossy(q).tune_lossy(|t| t.color = color);
            let b = imij::ImiEncoderConfig::lossy(90.0)
                .with_quality(q)
                .tune_lossy(|t| t.color = color);
            let c = imij::ImiEncoderConfig::lossy(90.0)
                .with_quality(LossyQuality::Ssim2(70.0))
                .with_quality(LossyQuality::Scale(q))
                .tune_lossy(|t| t.color = color);
            assert!(matches!(
                b.coder(),
                imij::Coder::Lossy(LossyQuality::Scale(_))
            ));
            assert!(matches!(
                c.coder(),
                imij::Coder::Lossy(LossyQuality::Scale(_))
            ));
            assert_eq!(b.scale_for(color, 61, 43), Some(q));
            assert_eq!(enc(&a), enc(&b));
            assert_eq!(enc(&a), enc(&c));
        }
    }
}

#[test]
fn targets_resolve_against_the_encoding_color() {
    for target in [
        LossyQuality::Ssim2(60.0),
        ba(2.0, ButteraugliNorm::Max),
        ba(1.0, ButteraugliNorm::P3),
        LossyQuality::JpegQuality(75.0),
        LossyQuality::JxlDistance(2.0),
    ] {
        for color in ALL {
            let before = imij::ImiEncoderConfig::lossy(90.0)
                .with_quality(target)
                .tune_lossy(|t| t.color = color);
            let after = imij::ImiEncoderConfig::lossy(90.0)
                .tune_lossy(|t| t.color = color)
                .with_quality(target);
            let explicit = imij::ImiEncoderConfig::lossy(target.to_scale(color, 61, 43))
                .tune_lossy(|t| t.color = color);
            assert_eq!(enc(&before), enc(&after));
            assert_eq!(enc(&before), enc(&explicit));
        }
    }
    // The Fastest tier codes YCoCg even when XYB is configured: the target
    // must follow it.
    let target = LossyQuality::Ssim2(50.0);
    let fastest = imij::ImiEncoderConfig::lossy(90.0)
        .with_quality(target)
        .with_decode_speed(DecodeSpeed::Fastest);
    let explicit = imij::ImiEncoderConfig::lossy(target.to_scale(LossyColor::YCoCg, 61, 43))
        .with_decode_speed(DecodeSpeed::Fastest);
    assert_eq!(enc(&fastest), enc(&explicit));
}

/// One longest side in each calibration bucket, at each edge.
const SIZES: [(u32, u32); 8] = [
    (64, 36),
    (128, 128),
    (129, 64),
    (384, 256),
    (385, 200),
    (768, 768),
    (769, 400),
    (1024, 1024),
];

/// Every setting maps into the scale's range, finite, and monotone in the
/// direction its unit improves.
fn assert_monotone(
    color: LossyColor,
    (w, h): (u32, u32),
    name: &str,
    values: &[f32],
    make: impl Fn(f32) -> LossyQuality,
    rising: bool,
) {
    let mut prev = if rising {
        f32::NEG_INFINITY
    } else {
        f32::INFINITY
    };
    for &v in values {
        let s = make(v).to_scale(color, w, h);
        assert!(
            s.is_finite() && (1.0..=100.0).contains(&s),
            "{color:?} {w}x{h} {name} {v} -> {s}"
        );
        assert!(
            if rising { s >= prev } else { s <= prev },
            "{color:?} {w}x{h} {name} not monotone at {v}"
        );
        prev = s;
    }
}

#[test]
fn tables_are_monotone_finite_and_invertible() {
    let quality: Vec<f32> = (1..=100).map(|q| q as f32).collect();
    let ssim2: Vec<f32> = (-20..=95).step_by(5).map(|t| t as f32).collect();
    let distances = [
        0.25f32, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 5.0, 8.0, 12.0, 20.0, 25.0, 40.0,
    ];
    for color in ALL {
        for size @ (w, h) in SIZES {
            assert_monotone(color, size, "ssim2", &ssim2, LossyQuality::Ssim2, true);
            assert_monotone(
                color,
                size,
                "ba max",
                &distances,
                |d| ba(d, ButteraugliNorm::Max),
                false,
            );
            assert_monotone(
                color,
                size,
                "ba p3",
                &distances,
                |d| ba(d, ButteraugliNorm::P3),
                false,
            );
            assert_monotone(
                color,
                size,
                "jpeg",
                &quality,
                LossyQuality::JpegQuality,
                true,
            );
            assert_monotone(
                color,
                size,
                "jxl",
                &distances,
                LossyQuality::JxlDistance,
                false,
            );

            // Inverses return the setting inside the calibrated range.
            for q in [20.0f32, 50.0, 75.0, 90.0] {
                let back = LossyQuality::JpegQuality(q).approx_jpeg_quality(color, w, h);
                assert!(
                    (back - q).abs() < 0.05,
                    "{color:?} {w}x{h} jpeg {q} -> {back}"
                );
            }
            for d in [1.0f32, 2.0, 4.0] {
                let back = LossyQuality::JxlDistance(d).approx_jxl_distance(color, w, h);
                assert!(
                    (back - d).abs() < 0.01,
                    "{color:?} {w}x{h} distance {d} -> {back}"
                );
            }
            for t in [40.0f32, 60.0, 75.0] {
                let back = LossyQuality::Ssim2(t).approx_ssim2(color, w, h);
                assert!(
                    (back - t).abs() < 0.05,
                    "{color:?} {w}x{h} ssim2 {t} -> {back}"
                );
            }
            for norm in [ButteraugliNorm::Max, ButteraugliNorm::P3] {
                for d in [1.5f32, 3.0] {
                    let back = ba(d, norm).approx_butteraugli(norm, color, w, h);
                    assert!(
                        (back - d).abs() < 0.01,
                        "{color:?} {w}x{h} {norm:?} {d} -> {back}"
                    );
                }
            }
            // Scales round-trip inside the range a unit reaches; outside,
            // they clamp to its ends.
            let lo = LossyQuality::JpegQuality(1.0).to_scale(color, w, h);
            let hi = LossyQuality::JpegQuality(100.0).to_scale(color, w, h);
            for scale in [2.0f32, 5.0, 15.0, 60.0, 99.0] {
                let s = LossyQuality::Scale(scale);
                let back = LossyQuality::JpegQuality(s.approx_jpeg_quality(color, w, h))
                    .to_scale(color, w, h);
                let expected = scale.clamp(lo, hi);
                assert!(
                    (back - expected).abs() < 0.05,
                    "{color:?} {w}x{h} scale {scale} -> {back} (range {lo}..{hi})"
                );
            }
            // Out-of-range settings clamp to the calibrated ends.
            assert_eq!(
                LossyQuality::JpegQuality(-5.0).to_scale(color, w, h),
                LossyQuality::JpegQuality(1.0).to_scale(color, w, h)
            );
            assert_eq!(
                LossyQuality::JxlDistance(100.0).to_scale(color, w, h),
                LossyQuality::JxlDistance(25.0).to_scale(color, w, h)
            );
        }
    }
}

#[test]
fn targets_follow_the_image_size() {
    // Encoding resolves against the image's own size bucket.
    let (info, px) = image();
    let target = LossyQuality::Ssim2(70.0);
    let cfg = imij::ImiEncoderConfig::lossy(90.0).with_quality(target);
    let explicit =
        imij::ImiEncoderConfig::lossy(target.to_scale(LossyColor::Xyb, info.width, info.height));
    let meta = Metadata::default();
    assert_eq!(
        cfg.encode_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &meta)
            .unwrap(),
        explicit
            .encode_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &meta)
            .unwrap()
    );
}

#[test]
fn nonfinite_targets_are_rejected() {
    let (info, px) = image();
    for target in [
        LossyQuality::Ssim2(f32::NAN),
        ba(f32::NAN, ButteraugliNorm::Max),
        ba(f32::NAN, ButteraugliNorm::P3),
        LossyQuality::JpegQuality(f32::NAN),
        LossyQuality::JxlDistance(f32::NAN),
        LossyQuality::Scale(f32::NAN),
    ] {
        let cfg = imij::ImiEncoderConfig::lossy(90.0).with_quality(target);
        assert!(
            cfg.encode_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default()
            )
            .is_err()
        );
    }
}

/// Calibration predates production trellis. Preserve its pixels below the
/// XYB activation boundary and at every YCoCg effort.
#[test]
fn calibration_effort_does_not_change_rgb8_pixels() {
    for color in ALL {
        for q in [1.0, 5.0, 12.0, 30.0, 70.0, 100.0] {
            let reference = enc(&imij::ImiEncoderConfig::lossy(q)
                .tune_lossy(|t| t.color = color)
                .with_effort(8.99));
            let expected = decode(&reference).unwrap().1;
            let efforts: &[f32] = if color == LossyColor::YCoCg {
                &[0.0, 5.0, 7.5, 8.99, 9.0, 9.9, 10.0]
            } else {
                &[0.0, 5.0, 7.5, 8.99]
            };
            for &effort in efforts {
                let e = enc(&imij::ImiEncoderConfig::lossy(q)
                    .tune_lossy(|t| t.color = color)
                    .with_effort(effort));
                assert_eq!(
                    decode(&e).unwrap().1,
                    expected,
                    "{color:?} q{q} effort {effort}"
                );
            }
        }
    }
}

#[cfg(feature = "lab")]
#[test]
fn calibration_pixels_match_at_every_effort_with_trellis_off() {
    for color in ALL {
        for q in [1.0, 5.0, 12.0, 30.0, 70.0, 100.0] {
            let config = |effort| {
                let mut cfg = imij::ImiEncoderConfig::lossy(q)
                    .tune_lossy(|t| t.color = color)
                    .with_effort(effort);
                cfg = cfg.tune_lossy(|t| {
                    t.lab.trellis = Some(imij::LossyTrellis {
                        passes: 0,
                        ..Default::default()
                    });
                });
                cfg
            };
            let expected = decode(&enc(&config(10.0))).unwrap().1;
            for effort in [0.0, 5.0, 7.5, 8.99, 9.0, 9.9, 10.0] {
                assert_eq!(
                    decode(&enc(&config(effort))).unwrap().1,
                    expected,
                    "{color:?} q{q} effort {effort}, trellis disabled"
                );
            }
        }
    }
}

#[test]
fn production_xyb_trellis_changes_pixels_and_preserves_target_resolution() {
    for q in [1.0, 5.0, 12.0, 30.0, 70.0, 100.0] {
        let expected = decode(&enc(&imij::ImiEncoderConfig::lossy(q).with_effort(8.99)))
            .unwrap()
            .1;
        for effort in [9.0, 9.9, 10.0] {
            assert_ne!(
                decode(&enc(&imij::ImiEncoderConfig::lossy(q).with_effort(effort)))
                    .unwrap()
                    .1,
                expected,
                "XYB q{q} effort {effort}"
            );
            for target in [
                LossyQuality::Ssim2(70.0),
                ba(2.0, ButteraugliNorm::Max),
                ba(1.0, ButteraugliNorm::P3),
                LossyQuality::JpegQuality(75.0),
                LossyQuality::JxlDistance(2.0),
            ] {
                let target_cfg = imij::ImiEncoderConfig::lossy(90.0)
                    .with_quality(target)
                    .with_effort(effort);
                let scale_cfg =
                    imij::ImiEncoderConfig::lossy(target.to_scale(LossyColor::Xyb, 61, 43))
                        .with_effort(effort);
                assert_eq!(enc(&target_cfg), enc(&scale_cfg));
            }
        }
    }
}

/// The explicit lab trellis's `min_quality` compares the scale a target
/// resolves to, not the raw `quality` field that `with_quality(target)` leaves
/// as it was.
#[cfg(feature = "lab")]
#[test]
fn lab_trellis_min_quality_follows_the_resolved_scale() {
    let trellis = imij::LossyTrellis {
        min_quality: 50.0,
        ..Default::default()
    };
    // Raw field below min_quality, target above it; and the reverse.
    for (raw, target) in [
        (10.0, LossyQuality::JpegQuality(90.0)),
        (90.0, LossyQuality::Ssim2(30.0)),
    ] {
        let scale = target.to_scale(LossyColor::Xyb, 61, 43);
        let mut by_target = imij::ImiEncoderConfig::lossy(raw).with_quality(target);
        by_target = by_target.tune_lossy(|t| {
            t.lab.trellis = Some(trellis);
        });
        let mut by_scale = imij::ImiEncoderConfig::lossy(scale);
        by_scale = by_scale.tune_lossy(|t| {
            t.lab.trellis = Some(trellis);
        });
        assert_eq!(
            enc(&by_target),
            enc(&by_scale),
            "raw {raw}, {target:?} -> scale {scale}"
        );
    }
    // Non-vacuous: at the first target's scale the lab trellis runs.
    let scale = LossyQuality::JpegQuality(90.0).to_scale(LossyColor::Xyb, 61, 43);
    let mut on = imij::ImiEncoderConfig::lossy(scale);
    on = on.tune_lossy(|t| {
        t.lab.trellis = Some(trellis);
    });
    let mut off = imij::ImiEncoderConfig::lossy(scale);
    off = off.tune_lossy(|t| {
        t.lab.trellis = Some(imij::LossyTrellis {
            passes: 0,
            ..Default::default()
        });
    });
    assert_ne!(enc(&on), enc(&off), "lab trellis at scale {scale}");
}
