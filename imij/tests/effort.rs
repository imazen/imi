use imij::{Channels, Error, ImageInfo, LossyColor, LossyEntropy, Metadata, SampleType, decode};

fn pixels(info: &ImageInfo) -> Vec<u8> {
    (0..info.width as usize * info.height as usize * info.channels.count())
        .flat_map(|i| {
            let n = (i * 37 + (i / 71) * 19) as u8;
            match info.sample {
                SampleType::U8 => vec![n],
                SampleType::U16 => (n as u16 * 257).to_ne_bytes().to_vec(),
                SampleType::F16 => (0x3000 + n as u16 * 12).to_ne_bytes().to_vec(),
                SampleType::F32 => (n as f32 / 255.0).to_ne_bytes().to_vec(),
                SampleType::U32 | SampleType::I32 => {
                    (n as u32 * 0x0101_0101).to_ne_bytes().to_vec()
                }
                SampleType::I8 => vec![n.wrapping_sub(128)],
                SampleType::I16 => ((n as i16 - 128) * 100).to_ne_bytes().to_vec(),
                SampleType::U64 | SampleType::I64 => {
                    (n as u64 * 0x0101_0101_0101_0101).to_ne_bytes().to_vec()
                }
                SampleType::F64 => (n as f64 / 255.0).to_ne_bytes().to_vec(),
                s => unreachable!("no pattern for {s:?}"),
            }
        })
        .collect()
}

#[test]
fn low_effort_matches_qualified_masks_and_retains_higher_efforts() {
    for sample in [
        SampleType::U8,
        SampleType::U16,
        SampleType::F16,
        SampleType::U32,
        SampleType::I32,
        SampleType::F32,
        SampleType::I8,
        SampleType::I16,
        SampleType::U64,
        SampleType::I64,
        SampleType::F64,
    ] {
        if !cfg!(feature = "extended") && sample.tier() == imij::Tier::Extended {
            continue;
        }
        let info = ImageInfo::new(73, 41, Channels::Rgba, sample);
        let px = pixels(&info);
        // Compare complete codec output against explicit public-mask controls,
        // including a fractional intermediate and unchanged effort-5+ policies.
        for (effort, predictors, decors) in [
            (0.0, 0x01, 0x02),
            (2.5, 0x09, 0x02),
            (4.9, 0x0b, 0x03),
            (5.0, 0x0f, 0x03),
            (7.5, 0x1f, 0x07),
            (10.0, 0x7f, 0x0f),
        ] {
            let cfg = {
                let mut cfg = imij::ImiEncoderConfig::default();
                cfg = cfg.with_effort(effort);
                cfg = cfg.tune_lossless(|t| {
                    t.stripe_rows = Some(3);
                });
                cfg = cfg.with_column_groups(Some(32));
                cfg
            };
            let encoded = cfg
                .encode_samples(
                    imij::ImageRef::new(&px, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap();
            let explicit = {
                let mut config = cfg.clone();
                config = config.with_effort(10.0);
                config = config.tune_lossless(|t| {
                    t.predictor_mask = predictors;
                });
                config = config.tune_lossless(|t| {
                    t.decor_mask = decors;
                });
                config
            };
            assert_eq!(
                encoded,
                explicit
                    .encode_samples(
                        imij::ImageRef::new(&px, info.row_bytes(), &info),
                        &Metadata::default()
                    )
                    .unwrap(),
                "{sample:?} effort {effort} differs from qualified masks"
            );
            assert_eq!(decode(&encoded).unwrap().1, px);
        }
    }
}

#[test]
fn effort_preserves_pixels_and_alpha() {
    for sample in [
        SampleType::U8,
        SampleType::U16,
        SampleType::F16,
        SampleType::F32,
    ] {
        for channels in [Channels::GrayAlpha, Channels::Rgba] {
            let info = ImageInfo::new(19, 17, channels, sample);
            let px = pixels(&info);
            // This test requires decoding its encoded output; baseline-only
            // decoders cannot reconstruct extended sample types.
            if !cfg!(feature = "extended") && sample.tier() == imij::Tier::Extended {
                continue;
            }
            for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                // The encoder codes F16/F32 input in XYB whatever the requested
                // color, so with run-level or split-Huffman AC those cells run
                // the effort trellis from 9 (its pixel changes: tests/trellis.rs)
                // and compare against effort 8.99. Every other cell is
                // YCoCg-coded and decodes identically at every
                // effort.
                let trellis = matches!(sample, SampleType::F16 | SampleType::F32)
                    && matches!(entropy, LossyEntropy::RunLevel | LossyEntropy::SplitHuf);
                let reference_effort = if trellis { 8.99 } else { 10.0 };
                let mut cfg = imij::ImiEncoderConfig::lossy(85.0)
                    .tune_lossy(|t| t.entropy = entropy)
                    .with_effort(reference_effort);
                cfg = cfg.tune_lossy(|t| {
                    t.color = LossyColor::YCoCg;
                });
                cfg = cfg.with_column_groups(Some(16));
                let reference = cfg
                    .encode_samples(
                        imij::ImageRef::new(&px, info.row_bytes(), &info),
                        &Metadata::default(),
                    )
                    .unwrap();
                let expected = decode(&reference).unwrap().1;
                let bps = sample.bytes();
                let pixel_bytes = channels.count() * bps;
                for effort in [0.0, 2.5, 5.0, 7.5, 8.99, 9.0, 9.9, 10.0] {
                    cfg = cfg.with_effort(effort);
                    let encoded = cfg
                        .encode_samples(
                            imij::ImageRef::new(&px, info.row_bytes(), &info),
                            &Metadata::default(),
                        )
                        .unwrap();
                    let decoded = decode(&encoded).unwrap().1;
                    if !trellis || effort < 9.0 {
                        assert_eq!(
                            decoded, expected,
                            "{sample:?}/{channels:?}/{entropy:?} effort {effort}"
                        );
                    }
                    if effort == reference_effort {
                        assert_eq!(encoded, reference);
                    }
                    // Alpha stays lossless at every effort.
                    for (src, out) in px
                        .chunks_exact(pixel_bytes)
                        .zip(decoded.chunks_exact(pixel_bytes))
                    {
                        assert_eq!(&src[pixel_bytes - bps..], &out[pixel_bytes - bps..]);
                    }
                    cfg = cfg.with_threads(4);
                    let threaded = cfg
                        .encode_samples(
                            imij::ImageRef::new(&px, info.row_bytes(), &info),
                            &Metadata::default(),
                        )
                        .unwrap();
                    assert_eq!(
                        encoded, threaded,
                        "effort {effort} changed bytes with threads"
                    );
                    cfg = cfg.with_threads(1);
                }
            }
        }
    }
}

#[test]
fn lossless_effort_respects_candidate_masks_and_threads() {
    let info = ImageInfo::new(73, 41, Channels::Rgba, SampleType::U8);
    let px = pixels(&info);
    for effort in [0.0, 2.5, 5.0, 7.5, 9.9, 10.0] {
        for (decor_mask, predictor_mask) in [(15, 127), (8, 64), (1, 1)] {
            let mut cfg = {
                let mut cfg = imij::ImiEncoderConfig::default();
                cfg = cfg.with_effort(effort);
                cfg = cfg.tune_lossless(|t| {
                    t.decor_mask = decor_mask;
                });
                cfg = cfg.tune_lossless(|t| {
                    t.predictor_mask = predictor_mask;
                });
                cfg = cfg.tune_lossless(|t| {
                    t.stripe_rows = Some(3);
                });
                cfg = cfg.with_column_groups(Some(32));
                cfg
            };
            let encoded = cfg
                .encode_samples(
                    imij::ImageRef::new(&px, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap();
            assert_eq!(decode(&encoded).unwrap().1, px);
            cfg = cfg.with_threads(4);
            assert_eq!(
                cfg.encode_samples(
                    imij::ImageRef::new(&px, info.row_bytes(), &info),
                    &Metadata::default()
                )
                .unwrap(),
                encoded
            );
        }
    }
}

#[test]
fn invalid_effort_is_rejected_before_reading_pixels() {
    let info = ImageInfo::new(8, 8, Channels::Rgb, SampleType::U8);
    for effort in [-0.1, 10.01, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let cfg = imij::ImiEncoderConfig::default().with_effort(effort);
        assert!(matches!(
            cfg.encode_samples(
                imij::ImageRef::new(&[], info.row_bytes(), &info),
                &Metadata::default()
            ),
            Err(Error::InvalidInput(_))
        ));
        let cfg = imij::ImiEncoderConfig::lossy(85.0).with_effort(effort);
        assert!(matches!(
            cfg.encode_samples(
                imij::ImageRef::new(&[], info.row_bytes(), &info),
                &Metadata::default()
            ),
            Err(Error::InvalidInput(_))
        ));
    }
}
