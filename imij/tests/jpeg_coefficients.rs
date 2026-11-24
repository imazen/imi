#![cfg(feature = "lab")]

use imij::{
    Channels, ImageInfo, ImiEncoderConfig, LossyEntropy, LossyLabSwitches, Metadata, SampleType,
};

fn config(entropy: LossyEntropy) -> ImiEncoderConfig {
    let mut cfg = imij::ImiEncoderConfig::lossy(50.0);
    cfg = cfg.tune_lossy(|t| {
        t.entropy = entropy;
    });
    cfg = cfg.tune_lossy(|t| {
        t.channel_mul = Some([1.0; 3]);
    });
    cfg = cfg.with_column_groups(Some(0));
    cfg = cfg.tune_lossy(|t| {
        t.exact_blocks = Some(0.0);
    });
    cfg = cfg.tune_lossy(|t| {
        t.lab.switches = LossyLabSwitches::defaults();
    });
    cfg = cfg.tune_lossy(|t| t.lab.switches.dc_step = Some([64; 2]));
    cfg = cfg.tune_lossy(|t| {
        t.lab.switches.explicit_matrix = Some(Box::new(core::array::from_fn(|ch| {
            core::array::from_fn(|i| (1 + i * 3 + ch * 97) as f32)
        })))
    });
    cfg = cfg.tune_lossy(|t| t.lab.switches.dering = Some(false));
    cfg = cfg.tune_lossy(|t| t.lab.switches.superblocks = Some(false));
    cfg
}

#[test]
fn imported_file_completion_matches_pixel_encoder_with_stride_and_checksum() {
    for channels in [Channels::Rgb, Channels::Gray] {
        let info = ImageInfo::new(19, 73, channels, SampleType::U8);
        let stride = info.row_bytes() + 11;
        let mut pixels = vec![0; stride * info.height as usize];
        for y in 0..info.height as usize {
            for x in 0..info.row_bytes() {
                pixels[y * stride + x] = ((x * 17 + y * 23) % 256) as u8;
            }
        }
        for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
            for checksum in [false, true] {
                let mut cfg = config(entropy);
                cfg = cfg.tune_lossy(|t| {
                    t.color = imij::LossyColor::YCoCg;
                });
                cfg = cfg.with_checksum(checksum);
                let normal = cfg
                    .encode_samples(
                        imij::ImageRef::new(&pixels, stride, &info),
                        &Metadata::default(),
                    )
                    .unwrap();
                let (_, _, q, _, _) = imij::lab::lossy_quantized(&normal).unwrap();
                let imported =
                    imij::lab::encode_lossy_quantized(&info, &Metadata::default(), &cfg, &q)
                        .unwrap();
                assert_eq!(
                    imported, normal,
                    "{channels:?} {entropy:?} checksum={checksum}"
                );
                assert_eq!(
                    imij::decode(&imported).unwrap(),
                    imij::decode(&normal).unwrap()
                );
            }
        }
    }
}

#[test]
fn imported_coefficients_preserve_signed_dc_and_every_frequency() {
    // Odd dimensions, multiple stripes, distinct tables, both DC signs,
    // sparse and dense AC blocks, and the full JPEG signed coefficient range.
    let info = ImageInfo::new(19, 73, Channels::Rgb, SampleType::U8);
    let mut q = vec![0; 3 * 10 * 192];
    for (b, block) in q.as_chunks_mut::<64>().0.iter_mut().enumerate() {
        block[0] = if b % 2 == 0 { -32768 } else { 32767 };
        for (i, value) in block.iter_mut().enumerate().skip(1) {
            *value = if b % 3 == 0 || i == b % 63 + 1 {
                ((i * 431 + b * 47) % 2047) as i32 - 1023
            } else {
                0
            };
        }
    }
    for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
        let cfg = config(entropy);
        let file =
            imij::lab::encode_lossy_quantized(&info, &Metadata::default(), &cfg, &q).unwrap();
        let (bw, bh, back, _, _) = imij::lab::lossy_quantized(&file).unwrap();
        assert_eq!((bw, bh), (3, 10));
        assert_eq!(back, q);
    }
}

#[test]
fn coefficient_input_rejects_stages_that_need_source_pixels() {
    let info = ImageInfo::new(8, 8, Channels::Rgb, SampleType::U8);
    let q = [0; 192];
    let cfg = config(LossyEntropy::RunLevel);
    let mut aq = cfg.clone();
    aq = aq.tune_lossy(|t| {
        t.aq_gamma = 0.5;
    });
    let mut exact = cfg.clone();
    exact = exact.tune_lossy(|t| {
        t.exact_blocks = Some(1.0);
    });
    let mut rdoq = cfg.clone();
    rdoq = rdoq.tune_lossy(|t| {
        t.lab.rdoq_lambda = 1.0;
    });
    let mut centroid = cfg.clone();
    centroid = centroid.tune_lossy(|t| t.lab.switches.recon_bias_centroid = Some(64));
    for bad in [aq, exact, rdoq, centroid] {
        assert!(imij::lab::encode_lossy_quantized(&info, &Metadata::default(), &bad, &q).is_err());
    }
    assert!(
        imij::lab::encode_lossy_quantized(&info, &Metadata::default(), &cfg, &q[..191]).is_err()
    );
}

#[test]
#[cfg(feature = "native-jpeg")] // This oracle is provided by the native-JPEG feature.
fn reused_aan_and_output_rounding_match_imported_pixel_decode() {
    let matrix = [
        [1.0, 0.0, 1.402, 128.0],
        [1.0, -0.344136, -0.714136, 128.0],
        [1.0, 1.772, 0.0, 128.0],
    ];
    let tables: [[u16; 64]; 3] =
        core::array::from_fn(|ch| core::array::from_fn(|i| (1 + (i * 3 + ch * 13) % 97) as u16));
    for channels in [Channels::Rgb, Channels::Gray] {
        let info = ImageInfo::new(8, 8, channels, SampleType::U8);
        for frequency in 0..64 {
            for sign in [-1, 1] {
                let mut blocks = [[0i16; 64]; 3];
                for (ch, block) in blocks.iter_mut().enumerate() {
                    if channels == Channels::Rgb || ch == 0 {
                        block[0] = sign * 7;
                        block[frequency] += sign;
                    }
                }
                let planes: [[f32; 64]; 3] =
                    core::array::from_fn(|ch| imij::lab::jpeg_idct(&blocks[ch], &tables[ch]));
                let mut expected = Vec::new();
                for i in 0..64 {
                    let rgb = imij::lab::linear_rgb(&matrix, planes.map(|p| p[i]));
                    if channels == Channels::Gray {
                        expected.push(rgb[1]);
                    } else {
                        expected.extend_from_slice(&rgb);
                    }
                }
                let q: Vec<i32> = blocks.into_iter().flatten().map(i32::from).collect();
                for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                    let mut cfg = config(entropy);
                    cfg = cfg.tune_lossy(|t| {
                        t.lab.switches.explicit_matrix =
                            Some(Box::new(tables.map(|t| t.map(f32::from))))
                    });
                    cfg = cfg.tune_lossy(|t| {
                        t.lab.switches.color_transform =
                            Some(imij::ColorTransform::Explicit(matrix))
                    });
                    let file =
                        imij::lab::encode_lossy_quantized(&info, &Metadata::default(), &cfg, &q)
                            .unwrap();
                    assert_eq!(
                        imij::decode(&file).unwrap().1,
                        expected,
                        "{channels:?} {entropy:?} frequency={frequency} sign={sign}"
                    );
                }
            }
        }
    }
}

#[test]
fn coefficient_admission_checks_ac_domain_and_geometry_before_entropy() {
    for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
        let cfg = config(entropy);
        let info = ImageInfo::new(8, 8, Channels::Rgb, SampleType::U8);
        for value in [1 << 22, -(1 << 22), i32::MIN, i32::MAX] {
            let mut q = vec![0; 192];
            q[1] = value;
            assert!(
                matches!(
                    imij::lab::encode_lossy_quantized(&info, &Metadata::default(), &cfg, &q),
                    Err(imij::Error::InvalidInput(_))
                ),
                "{entropy:?} {value}"
            );
        }
        // Last ordinary AC values stay legal; their signed reconstruction is
        // retained by the strict coefficient oracle rather than being clamped.
        for value in [(1 << 22) - 1, -((1 << 22) - 1)] {
            let mut q = vec![0; 192];
            q[1] = value;
            let file =
                imij::lab::encode_lossy_quantized(&info, &Metadata::default(), &cfg, &q).unwrap();
            assert_eq!(imij::lab::lossy_quantized(&file).unwrap().2, q);
        }
        for (w, h) in [(0, 8), (8, 0), (u32::MAX, u32::MAX), (65535, 65535)] {
            let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
            assert!(matches!(
                imij::lab::encode_lossy_quantized(&info, &Metadata::default(), &cfg, &[]),
                Err(imij::Error::InvalidInput(_))
            ));
        }
    }
}
