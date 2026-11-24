//! The AC trellis lossy effort runs from 9 on XYB run-level and
//! split-Huffman coding (`lossy/trellis.rs`): files decode with every decoder entry point, bytes
//! do not depend on encoder threads, pixels change only from the trellis
//! effort, and every configuration encodes (the trellis applies or is
//! skipped, never an error). Lab: the `ImiEncoderConfig::trellis` override.
use imij::{
    ChannelSampling, Channels, DecodeSpeed, FrameConfig, FrameDecoder, FrameDuration, FrameEncoder,
    ImageInfo, ImiEncoderConfig, LossyChannelLayout, LossyColor, LossyEntropy, LossyTransform,
    Metadata, SampleType,
};

/// The lowest effort that runs the trellis (`effort::TRELLIS_ONE_PASS`).
const TRELLIS_FROM: f32 = 9.0;
/// The highest effort without it.
const BELOW: f32 = 8.99;

/// Smooth ramps, texture and hard edges (photo-like at every quality).
fn image(info: &ImageInfo) -> Vec<u8> {
    let (w, h, n) = (
        info.width as usize,
        info.height as usize,
        info.channels.count(),
    );
    let bps = info.sample.bytes();
    let mut px = vec![0u8; info.row_bytes() * h];
    for y in 0..h {
        for x in 0..w {
            for c in 0..n {
                let ramp = (x * 3 + y * 2 + c * 40) as f32;
                let tex = 25.0 * ((x as f32 * 0.9 + c as f32).sin() * (y as f32 * 0.7).cos());
                let edge = if (x / 11 + y / 7) % 3 == 0 { 60.0 } else { 0.0 };
                let v = ((ramp + tex + edge) % 256.0).clamp(0.0, 255.0);
                let o = (y * w + x) * n * bps + c * bps;
                if bps == 1 {
                    px[o] = v as u8;
                } else {
                    px[o..o + 2].copy_from_slice(&((v * 257.0) as u16).to_ne_bytes());
                }
            }
        }
    }
    px
}

fn encode(px: &[u8], info: &ImageInfo, cfg: &ImiEncoderConfig) -> Result<Vec<u8>, imij::Error> {
    cfg.encode_samples(
        imij::ImageRef::new(px, info.row_bytes(), info),
        &Metadata::default(),
    )
}

fn mse(a: &[u8], b: &[u8]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(&x, &y)| (x as f64 - y as f64).powi(2))
        .sum::<f64>()
        / a.len() as f64
}

/// Encodes with 1 and 4 threads (same bytes), decodes through every entry
/// point (same pixels) and returns (file, pixels).
fn encode_checked(
    px: &[u8],
    info: &ImageInfo,
    cfg: &ImiEncoderConfig,
    what: &str,
) -> (Vec<u8>, Vec<u8>) {
    let file = encode(px, info, cfg).unwrap();
    let file4 = encode(px, info, &cfg.clone().with_threads(4)).unwrap();
    assert_eq!(file, file4, "{what}: bytes depend on threads");
    let (h, dec) = imij::decode(&file).unwrap();
    assert_eq!((h.width, h.height), (info.width, info.height), "{what}");
    let (r, rows) = (info.row_bytes(), info.height as usize);
    let mut out = vec![0u8; (r + 7) * rows];
    imij::decode_into_threads(&file, &mut out, r + 7, 4).unwrap();
    for y in 0..rows {
        assert_eq!(
            out[y * (r + 7)..][..r],
            dec[y * r..][..r],
            "{what}: row {y}"
        );
    }
    let mut streamed = Vec::new();
    imij::decode_reader(file.as_slice(), &imij::Limits::DEFAULT, |_, _, row| {
        streamed.extend_from_slice(row);
        Ok(())
    })
    .unwrap();
    assert_eq!(streamed, dec, "{what}: streamed");
    (file, dec)
}

#[test]
fn trellis_files_decode_and_ignore_thread_count() {
    for channels in [Channels::Rgb, Channels::Rgba, Channels::Gray] {
        let info = ImageInfo::new(139, 25, channels, SampleType::U8);
        let px = image(&info);
        for color in [LossyColor::Xyb, LossyColor::YCoCg] {
            for group in [0, 136, 16] {
                for q in [5.0, 20.0, 70.0, 95.0] {
                    let mut cfg = imij::ImiEncoderConfig::lossy(q)
                        .tune_lossy(|t| t.color = color)
                        .tune_lossy(|t| t.stripe_block_rows = 3)
                        .with_effort(BELOW);
                    cfg = cfg.with_column_groups(Some(group));
                    let off = encode(&px, &info, &cfg).unwrap();
                    let (_, dec_off) = imij::decode(&off).unwrap();
                    for effort in [TRELLIS_FROM, 10.0] {
                        let what = format!("{channels:?} {color:?} g{group} q{q} e{effort}");
                        let (_, dec) =
                            encode_checked(&px, &info, &cfg.clone().with_effort(effort), &what);
                        // Same syntax, sane pixels: never far worse than off.
                        let (e_on, e_off) = (mse(&px, &dec), mse(&px, &dec_off));
                        assert!(
                            e_on.is_finite() && e_on <= 4.0 * e_off + 4.0,
                            "{what}: mse {e_on} vs {e_off}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn pixels_change_only_from_the_trellis_effort() {
    for sample in [SampleType::U8, SampleType::U16] {
        let info = ImageInfo::new(64, 48, Channels::Rgb, sample);
        let px = image(&info);
        let cfg = imij::ImiEncoderConfig::lossy(70.0);
        let pixels = |effort: f32| {
            let file = encode(&px, &info, &cfg.clone().with_effort(effort)).unwrap();
            imij::decode(&file).unwrap().1
        };
        let below = pixels(BELOW);
        for effort in [0.0, 2.5, 5.0, 7.5] {
            assert_eq!(pixels(effort), below, "{sample:?} effort {effort}");
        }
        let one = pixels(TRELLIS_FROM);
        let two = pixels(10.0);
        assert_ne!(one, below, "{sample:?}: the trellis did not run");
        assert_ne!(two, one, "{sample:?}: the second pass changed nothing");
    }
}

#[test]
fn every_configuration_encodes_and_the_trellis_runs_on_xyb_run_level_and_split_huffman() {
    let info = ImageInfo::new(71, 43, Channels::Rgb, SampleType::U8);
    let px = image(&info);
    let base = imij::ImiEncoderConfig::lossy(40.0).tune_lossy(|t| t.stripe_block_rows = 2);
    let pixels =
        |cfg: &ImiEncoderConfig| imij::decode(&encode(&px, &info, cfg).unwrap()).unwrap().1;
    // (name, configuration, Some(whether the trellis runs) or None when the
    // tier's per-image cost model decides).
    let mut cfgs: Vec<(String, ImiEncoderConfig, Option<bool>)> = Vec::new();
    for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
        cfgs.push((
            format!("{entropy:?}"),
            base.clone().tune_lossy(|t| t.entropy = entropy),
            Some(true),
        ));
    }
    // Images up to 64K pixels under a tier also code split-Huffman (the
    // split-Huffman trellis of the pass-1 coefficients) and keep the smaller
    // file, so the tier decides;
    // Fastest codes YCoCg. Compact on larger images: below.
    for (speed, runs) in [
        (DecodeSpeed::Compact, None),
        (DecodeSpeed::Balanced, None),
        (DecodeSpeed::Fast, None),
        (DecodeSpeed::Fastest, Some(false)),
    ] {
        cfgs.push((
            format!("{speed:?}"),
            base.clone().with_decode_speed(speed),
            runs,
        ));
    }
    cfgs.push((
        "YCoCg".into(),
        base.clone().tune_lossy(|t| t.color = LossyColor::YCoCg),
        Some(false),
    ));
    // Adaptive quantization: the trellis uses each block's scaled steps.
    let mut aq = base.clone();
    aq = aq.tune_lossy(|t| {
        t.aq_gamma = 0.5;
    });
    cfgs.push(("aq".into(), aq, Some(true)));
    let mut grouped = base.clone();
    grouped = grouped.with_column_groups(Some(24));
    cfgs.push(("groups".into(), grouped, Some(true)));
    // Subblock layouts skip the trellis.
    let sub = LossyChannelLayout::new(LossyTransform::Adaptive4, ChannelSampling::Full);
    for layouts in [
        [
            sub,
            LossyChannelLayout::default(),
            LossyChannelLayout::default(),
        ],
        [LossyChannelLayout::default(), sub, sub],
    ] {
        cfgs.push((
            "layouts".into(),
            base.clone().tune_lossy(|t| t.channel_layouts = layouts),
            Some(false),
        ));
    }
    for (name, cfg, runs) in &cfgs {
        let below = pixels(&cfg.clone().with_effort(BELOW));
        let (_, on) = encode_checked(&px, &info, &cfg.clone().with_effort(10.0), name);
        match runs {
            Some(true) => assert_ne!(on, below, "{name}: the trellis did not run"),
            Some(false) => assert_eq!(on, below, "{name}: the trellis ran"),
            None => {}
        }
    }
    // Above 64K pixels Compact always codes run-level: the trellis runs.
    let big = ImageInfo::new(260, 260, Channels::Rgb, SampleType::U8);
    let big_px = image(&big);
    let compact = base.clone().with_decode_speed(DecodeSpeed::Compact);
    let below = imij::decode(&encode(&big_px, &big, &compact.clone().with_effort(BELOW)).unwrap())
        .unwrap()
        .1;
    let (_, on) = encode_checked(&big_px, &big, &compact.with_effort(10.0), "Compact 260x260");
    assert_ne!(on, below, "Compact 260x260: the trellis did not run");
}

/// Two small objects moving one pixel a frame over a static background.
fn frame(w: u32, h: u32, t: u32) -> Vec<u8> {
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let mut px = image(&info);
    for y in 0..h {
        for x in 0..w {
            let a = (2 + t..8 + t).contains(&x) && (3..9).contains(&y);
            let b = x + 12 + t >= w && x + 6 + t < w && y + 10 >= h && y + 4 < h;
            if a || b {
                let o = ((y * w + x) * 3) as usize;
                px[o..o + 3].copy_from_slice(&[255, 225, 195]);
            }
        }
    }
    px
}

#[test]
fn trellis_keeps_inter_frame_keep_blocks() {
    let (w, h) = (120u32, 72u32);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let frames: Vec<Vec<u8>> = (0..6).map(|t| frame(w, h, t)).collect();
    let mut sizes = [0usize; 2];
    let mut errors: [Vec<f64>; 2] = [Vec::new(), Vec::new()];
    for (k, inter) in [false, true].into_iter().enumerate() {
        let lossy = imij::ImiEncoderConfig::lossy(85.0).with_effort(10.0);
        let mut cfg = FrameConfig::new(lossy);
        cfg.inter = inter;
        let mut e = FrameEncoder::animation(info.clone(), Metadata::default(), cfg);
        for f in &frames {
            e.push_frame(f, info.row_bytes(), FrameDuration::from_millis(50))
                .unwrap();
        }
        let file = e.finish().unwrap();
        sizes[k] = file.len();
        let mut d = FrameDecoder::new(&file).unwrap();
        let mut canvas = vec![0u8; info.row_bytes() * h as usize];
        while d.next_frame().unwrap().is_some() {
            d.decode_into(&mut canvas, info.row_bytes(), 3).unwrap();
            errors[k].push(mse(&canvas, &frames[errors[k].len()]));
        }
        assert_eq!(errors[k].len(), frames.len());
    }
    for (t, (inter, intra)) in errors[1].iter().zip(&errors[0]).enumerate() {
        assert!(
            *inter <= intra * 1.1 + 0.25,
            "frame {t}: inter mse {inter} vs intra {intra}"
        );
    }
    assert!(
        sizes[1] * 2 < sizes[0],
        "inter {} vs intra {}",
        sizes[1],
        sizes[0]
    );
}

#[cfg(feature = "lab")]
mod lab {
    use super::*;
    use imij::LossyTrellis;

    #[test]
    fn override_turns_the_trellis_off_or_on_at_any_effort() {
        let info = ImageInfo::new(64, 48, Channels::Rgb, SampleType::U8);
        let px = image(&info);
        let cfg = imij::ImiEncoderConfig::lossy(40.0);
        let pixels =
            |cfg: &ImiEncoderConfig| imij::decode(&encode(&px, &info, cfg).unwrap()).unwrap().1;
        let below = pixels(&cfg.clone().with_effort(BELOW));
        let mut off = cfg.clone().with_effort(10.0);
        off = off.tune_lossy(|t| {
            t.lab.trellis = Some(LossyTrellis {
                passes: 0,
                ..LossyTrellis::default()
            });
        });
        assert_eq!(pixels(&off), below);
        // The override also runs where the effort's trellis does not.
        let shuf = cfg
            .clone()
            .tune_lossy(|t| t.entropy = LossyEntropy::SplitHuf);
        let mut forced = shuf.clone().with_effort(10.0);
        forced = forced.tune_lossy(|t| {
            t.lab.trellis = Some(LossyTrellis::default());
        });
        assert_ne!(pixels(&forced), pixels(&shuf.with_effort(10.0)));
        // Below its quality, the override keeps the default quantization:
        // byte-identical to the same effort without the override.
        let mut gated = cfg.clone().with_effort(5.0);
        gated = gated.tune_lossy(|t| {
            t.lab.trellis = Some(LossyTrellis {
                min_quality: 50.0,
                ..LossyTrellis::default()
            });
        });
        assert_eq!(
            encode(&px, &info, &gated).unwrap(),
            encode(&px, &info, &cfg.clone().with_effort(5.0)).unwrap()
        );
        gated = gated.with_quality(50.0);
        assert_ne!(
            pixels(&gated),
            pixels(&imij::ImiEncoderConfig::lossy(50.0).with_effort(5.0))
        );
    }

    /// The lab trellis's original matrix (2026-10-07): every channel layout,
    /// color, column-group setting and quality, with one and two passes and
    /// zenjpeg's AQ coupling both ways, forced by the override at the
    /// default effort. Files decode with every entry point, bytes ignore the
    /// thread count, and pixels are never far worse than without the trellis.
    #[test]
    fn override_matrix_decodes_and_ignores_thread_count() {
        for channels in [Channels::Rgb, Channels::Rgba, Channels::Gray] {
            let info = ImageInfo::new(139, 25, channels, SampleType::U8);
            let px = image(&info);
            for color in [LossyColor::Xyb, LossyColor::YCoCg] {
                for group in [0, 136, 16] {
                    for q in [5.0, 20.0, 70.0, 95.0] {
                        let mut cfg = imij::ImiEncoderConfig::lossy(q)
                            .tune_lossy(|t| t.color = color)
                            .tune_lossy(|t| t.stripe_block_rows = 3);
                        cfg = cfg.with_column_groups(Some(group));
                        let off = encode(&px, &info, &cfg).unwrap();
                        let (_, dec_off) = imij::decode(&off).unwrap();
                        for (passes, aq) in [(1, 0.0), (2, 0.0), (1, -4.0), (1, 4.0)] {
                            cfg = cfg.tune_lossy(|t| {
                                t.lab.trellis = Some(LossyTrellis {
                                    passes,
                                    aq_coupling: aq,
                                    ..LossyTrellis::default()
                                });
                            });
                            let what =
                                format!("{channels:?} {color:?} g{group} q{q} p{passes} aq{aq}");
                            let (_, dec) = encode_checked(&px, &info, &cfg, &what);
                            let (e_on, e_off) = (mse(&px, &dec), mse(&px, &dec_off));
                            assert!(
                                e_on.is_finite() && e_on <= 4.0 * e_off + 4.0,
                                "{what}: mse {e_on} vs {e_off}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Flat regions with sparse edges and a little noise: few tokens, the
    /// content small tier images most often code with split-Huffman.
    fn graphic(info: &ImageInfo) -> Vec<u8> {
        let (w, h) = (info.width as usize, info.height as usize);
        let mut px = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let base: [u8; 3] = if (x / 13 + y / 9) % 4 == 0 {
                    [30, 60, 200]
                } else if x > w / 2 {
                    [240, 240, 235]
                } else {
                    [200, 40, 40]
                };
                let n = ((x * 7 + y * 13) % 5) as u8;
                for c in 0..3 {
                    px[(y * w + x) * 3 + c] = base[c].saturating_add(n);
                }
            }
        }
        px
    }

    /// Small images under a tier code both modes and keep the smaller: a
    /// split-Huffman file at the trellis effort carries the split-Huffman
    /// trellis's pixels (the trellis of the pass-1 coefficients), so some
    /// differ from the effort-8.99 file; with that trellis switched off they
    /// all equal it (`tests/trellis_lab_switches.rs`).
    #[test]
    fn small_tier_split_huffman_files_carry_split_huffman_trellis_pixels() {
        let (mut split_huffman, mut moved) = (0, 0);
        for (w, h) in [
            (16, 16),
            (32, 24),
            (64, 64),
            (139, 25),
            (200, 150),
            (255, 255),
        ] {
            let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
            for px in [image(&info), graphic(&info)] {
                for speed in [
                    DecodeSpeed::Compact,
                    DecodeSpeed::Balanced,
                    DecodeSpeed::Fast,
                ] {
                    for q in [5.0, 20.0, 50.0, 90.0] {
                        let cfg = imij::ImiEncoderConfig::lossy(q).with_decode_speed(speed);
                        let what = format!("{w}x{h} {speed:?} q{q}");
                        let (on, p_on) =
                            encode_checked(&px, &info, &cfg.clone().with_effort(10.0), &what);
                        let (mode, ..) = imij::seeds::lossy_entropy_seed(&on).unwrap();
                        if mode != LossyEntropy::SplitHuf as u8 {
                            continue;
                        }
                        split_huffman += 1;
                        let below = encode(&px, &info, &cfg.with_effort(BELOW)).unwrap();
                        if p_on != imij::decode(&below).unwrap().1 {
                            moved += 1;
                        }
                    }
                }
            }
        }
        assert!(split_huffman > 0, "no case coded split-Huffman");
        assert!(moved > 0, "no split-Huffman tier file took the trellis");
        eprintln!("split-Huffman tier files: {split_huffman}, trellis pixels: {moved}");
    }

    #[test]
    fn lab_modes_encode_with_the_trellis_effort() {
        let info = ImageInfo::new(32, 32, Channels::Rgb, SampleType::U8);
        let px = image(&info);
        let base = imij::ImiEncoderConfig::lossy(70.0).with_effort(10.0);
        // A caller AQ map scales the steps the trellis prices. (Lab RDOQ
        // also replaces the trellis; its debug source check fails on main
        // since DC rounding and deringing changed, so it is not run here.)
        let mut c = base.clone();
        c = c.tune_lossy(|t| {
            t.lab.aq_map = Some((0..16).map(|i| [48u8, 64, 96, 128][i % 4]).collect());
        });
        encode_checked(&px, &info, &c, "aq_map");
    }

    #[test]
    fn invalid_trellis_settings_are_rejected() {
        let info = ImageInfo::new(32, 32, Channels::Rgb, SampleType::U8);
        let px = image(&info);
        let with = |t: LossyTrellis| {
            let mut c = imij::ImiEncoderConfig::lossy(70.0);
            c = c.tune_lossy(|tuning| tuning.lab.trellis = Some(t));
            c
        };
        for t in [
            LossyTrellis {
                lambda_log_scale1: f32::NAN,
                ..LossyTrellis::default()
            },
            LossyTrellis {
                passes: 5,
                ..LossyTrellis::default()
            },
        ] {
            assert!(matches!(
                encode(&px, &info, &with(t)),
                Err(imij::Error::InvalidInput(_))
            ));
        }
        // AQ coupling reads U8 luma only.
        let coupled = with(LossyTrellis {
            aq_coupling: -4.0,
            ..LossyTrellis::default()
        });
        let wide = ImageInfo::new(32, 32, Channels::Rgb, SampleType::U16);
        assert!(encode(&image(&wide), &wide, &coupled).is_err());
        assert!(encode(&px, &info, &coupled).is_ok());
    }
}

/// Lossy encode bytes are a cross-tier observable: every `incant!`/`summon`
/// fallback (FDCT, quant, reconstruction, trellis, entropy) must produce the
/// same file on the SIMD path as with tokens disabled. Encodes several
/// images once per token permutation and asserts byte identity.
#[test]
fn lossy_encode_bytes_identical_across_token_permutations() {
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
    let cases: Vec<(ImageInfo, ImiEncoderConfig)> = [
        // e10 split-Huffman + run-level trellis, XYB.
        (
            64,
            48,
            Channels::Rgb,
            SampleType::U8,
            70.0,
            10.0,
            LossyColor::Xyb,
        ),
        // e9 trellis, YCoCg.
        (
            139,
            25,
            Channels::Rgb,
            SampleType::U8,
            75.0,
            TRELLIS_FROM,
            LossyColor::YCoCg,
        ),
        // Below trellis: FDCT/quant/recon SIMD without the trellis.
        (
            40,
            32,
            Channels::Gray,
            SampleType::U8,
            50.0,
            7.0,
            LossyColor::YCoCg,
        ),
        (
            32,
            32,
            Channels::Rgba,
            SampleType::U8,
            90.0,
            BELOW,
            LossyColor::Xyb,
        ),
        // U16 sample path.
        (
            48,
            33,
            Channels::Rgb,
            SampleType::U16,
            60.0,
            10.0,
            LossyColor::Xyb,
        ),
    ]
    .into_iter()
    .map(|(w, h, ch, st, q, e, color)| {
        (
            ImageInfo::new(w, h, ch, st),
            imij::ImiEncoderConfig::lossy(q)
                .with_effort(e)
                .tune_lossy(|t| t.color = color),
        )
    })
    .collect();
    let want: Vec<Vec<u8>> = cases
        .iter()
        .map(|(info, cfg)| encode(&image(info), info, cfg).unwrap())
        .collect();
    let report = for_each_token_permutation(CompileTimePolicy::Warn, |perm| {
        for (i, (info, cfg)) in cases.iter().enumerate() {
            let got = encode(&image(info), info, cfg).unwrap();
            assert_eq!(
                got.len(),
                want[i].len(),
                "case {i} {}: length differs",
                perm.label
            );
            assert_eq!(got, want[i], "case {i} {}: bytes differ", perm.label);
        }
    });
    // Only x86_64 and aarch64 build disableable SIMD token slots
    // (archmage `build_token_slots`), so only they have a second
    // permutation to run: every other target — i686, wasm32 (its simd128
    // token is compile-time-guaranteed and cannot be disabled) — compiles
    // the scalar path alone and runs exactly once.
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    assert!(
        report.permutations_run >= 2,
        "token permutations did not cover a disabled tier: {report}"
    );
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    assert_eq!(
        report.permutations_run, 1,
        "this target compiles the scalar tier alone: {report}"
    );
    eprintln!("token permutations: {report}");
}
