#![cfg(feature = "lab")]
use imij::{
    Channels, Error, ImageInfo, ImiEncoderConfig, LossyColor, LossyEntropy, Metadata, SampleType,
};

#[test]
fn opt_in_roundtrip_alpha_threads_and_decode_apis() {
    for fdct in [0, 1, 2] {
        imij::lab::set_fdct_mode(fdct);
        for channels in [
            Channels::Rgb,
            Channels::Rgba,
            Channels::Gray,
            Channels::GrayAlpha,
        ] {
            for extreme_blocks in [false, true] {
                let info = ImageInfo::new(139, 25, channels, SampleType::U8);
                let pixels: Vec<u8> = (0..info.row_bytes() * info.height as usize)
                    .map(|i| {
                        let x = i / channels.count() % info.width as usize;
                        let y = i / info.row_bytes();
                        // Include complete white/black blocks and sharp edges so
                        // pass-1 deringing and nearest-DC rounding are exercised.
                        match if extreme_blocks {
                            (x / 8 + y / 8) % 4
                        } else {
                            2
                        } {
                            0 => 255,
                            1 => 0,
                            _ => ((i * 37 + i / 17 * 91) % 256) as u8,
                        }
                    })
                    .collect();
                let input_stride = info.row_bytes() + 13;
                let mut strided = vec![0xa5; input_stride * info.height as usize];
                for (src, dst) in pixels
                    .chunks_exact(info.row_bytes())
                    .zip(strided.chunks_exact_mut(input_stride))
                {
                    dst[..info.row_bytes()].copy_from_slice(src);
                }
                for color in [LossyColor::Xyb, LossyColor::YCoCg] {
                    for group in [0, 136, 16] {
                        let mut cfg = imij::ImiEncoderConfig::lossy(70.0)
                            .tune_lossy(|t| t.color = color)
                            .tune_lossy(|t| t.stripe_block_rows = 3);
                        cfg = cfg.with_column_groups(Some(group));
                        let enc = |cfg: &ImiEncoderConfig| {
                            cfg.encode_samples(
                                imij::ImageRef::new(&pixels, info.row_bytes(), &info),
                                &Metadata::default(),
                            )
                            .unwrap()
                        };
                        let baseline = enc(&cfg);
                        cfg = cfg.tune_lossy(|t| {
                            t.lab.rdoq_lambda = 0.0;
                        });
                        assert_eq!(enc(&cfg), baseline);
                        cfg = cfg.tune_lossy(|t| {
                            t.lab.rdoq_lambda = 0.2;
                        });
                        let encoded = enc(&cfg);
                        assert_eq!(
                            cfg.encode_samples(
                                imij::ImageRef::new(&strided, input_stride, &info),
                                &Metadata::default()
                            )
                            .unwrap(),
                            encoded,
                            "input stride changed RDOQ decisions"
                        );
                        let (header, expected) = imij::decode(&encoded).unwrap();
                        assert_eq!(header, info);
                        cfg = cfg.with_threads(4);
                        assert_eq!(enc(&cfg), encoded, "encoder threads changed RDOQ decisions");
                        let bpp = info.bytes_per_pixel();
                        if channels.has_alpha() {
                            for (a, b) in pixels.chunks_exact(bpp).zip(expected.chunks_exact(bpp)) {
                                assert_eq!(a[bpp - 1], b[bpp - 1]);
                            }
                        }
                        let stride = info.row_bytes() + 11;
                        let mut padded = vec![0xa5; stride * info.height as usize];
                        imij::decode_into_threads(&encoded, &mut padded, stride, 4).unwrap();
                        for (a, b) in padded
                            .chunks_exact(stride)
                            .zip(expected.chunks_exact(info.row_bytes()))
                        {
                            assert_eq!(&a[..info.row_bytes()], b);
                            assert!(a[info.row_bytes()..].iter().all(|&v| v == 0xa5));
                        }
                        let mut stream = imij::StreamDecoder::new(&encoded).unwrap();
                        let mut row = vec![0; info.row_bytes()];
                        for b in expected.chunks_exact(info.row_bytes()) {
                            assert!(stream.next_row(&mut row).unwrap());
                            assert_eq!(row, b);
                        }
                        assert!(!stream.next_row(&mut row).unwrap());
                        let mut reader_pixels = vec![];
                        imij::decode_reader(
                            encoded.as_slice(),
                            &imij::Limits::DEFAULT,
                            |_, _, row| {
                                reader_pixels.extend_from_slice(row);
                                Ok(())
                            },
                        )
                        .unwrap();
                        assert_eq!(reader_pixels, expected);
                    }
                }
            }
        }
    }
    imij::lab::set_fdct_mode(0);
}

#[test]
fn unsupported_policies_and_invalid_lambda_fail_early() {
    let info = ImageInfo::new(8, 8, Channels::Rgb, SampleType::U8);
    let fails = |cfg: ImiEncoderConfig, info: &ImageInfo| {
        cfg.encode_samples(
            imij::ImageRef::new(&[], info.row_bytes(), info),
            &Metadata::default(),
        )
    };
    for lambda in [f32::NAN, f32::INFINITY, -0.1, 4.1] {
        let mut cfg = imij::ImiEncoderConfig::lossy(90.0);
        cfg = cfg.tune_lossy(|t| {
            t.lab.rdoq_lambda = lambda;
        });
        assert!(matches!(fails(cfg, &info), Err(Error::InvalidInput(_))));
    }
    {
        let mut cfg = imij::ImiEncoderConfig::lossy(90.0);
        cfg = cfg.tune_lossy(|t| {
            t.lab.rdoq_lambda = 0.1;
        });
        cfg = cfg.tune_lossy(|t| {
            t.entropy = LossyEntropy::SplitHuf;
        });
        assert!(matches!(fails(cfg, &info), Err(Error::Unsupported(_))));
    }
    for sample in [SampleType::U16, SampleType::F16, SampleType::F32] {
        let info = ImageInfo::new(8, 8, Channels::Rgb, sample);
        let mut cfg = imij::ImiEncoderConfig::lossy(90.0);
        cfg = cfg.tune_lossy(|t| {
            t.lab.rdoq_lambda = 0.1;
        });
        assert!(matches!(fails(cfg, &info), Err(Error::Unsupported(_))));
    }
    for case in 0..3 {
        let mut cfg = imij::ImiEncoderConfig::lossy(90.0);
        cfg = cfg.tune_lossy(|t| {
            t.lab.rdoq_lambda = 0.1;
        });
        cfg = match case {
            0 => cfg.tune_lossy(|t| t.aq_gamma = 0.5),
            1 => cfg.with_decode_speed(imij::DecodeSpeed::Fast),
            _ => cfg.tune_lossy(|t| t.lab.lf_pred = 1),
        };
        assert!(matches!(fails(cfg, &info), Err(Error::Unsupported(_))));
    }
}
