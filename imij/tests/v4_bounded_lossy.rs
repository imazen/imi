//! V4 serialization uses the native bounded encoder as its pixel oracle.
use imij::stream_encode::{BoundedLossyConfig, LossyUnitIndex};
use imij::{
    Channels, DecodeSpeed, ImageInfo, ImiEncoderConfig, LossyEntropy, Metadata, SampleType,
};
use std::io::Cursor;

fn raster(info: &ImageInfo, stride: usize) -> Vec<u8> {
    let mut source = vec![0xee; stride * info.height as usize];
    for y in 0..info.height as usize {
        for x in 0..info.width as usize {
            for c in 0..info.channels.count() {
                let v = match (x / 23 + y / 17) % 4 {
                    0 => 240,
                    1 if (x + y) % 3 == 0 => 20,
                    1 => 235,
                    2 => ((x * 7 + y * 13 + c * 91) % 251) as u8,
                    _ => ((x * 3 + y * 2 + c * 50) % 256) as u8,
                };
                let at = y * stride + (x * info.channels.count() + c) * info.sample.bytes();
                match info.sample {
                    SampleType::U8 => source[at] = v,
                    SampleType::U16 => {
                        source[at..at + 2].copy_from_slice(&(u16::from(v) * 257).to_ne_bytes())
                    }
                    SampleType::U32 => source[at..at + 4]
                        .copy_from_slice(&(u32::from(v) * 16_843_009).to_ne_bytes()),
                    SampleType::F16 => source[at..at + 2]
                        .copy_from_slice(&(0x3000 + u16::from(v) * 4).to_ne_bytes()),
                    SampleType::F32 => source[at..at + 4]
                        .copy_from_slice(&(0.125 + f32::from(v) / 512.0).to_ne_bytes()),
                    _ => panic!("unsupported fixture sample"),
                }
            }
        }
    }
    source
}
fn check(info: &ImageInfo, config: &ImiEncoderConfig) -> Vec<u8> {
    let stride = info.row_bytes() + 19;
    let source = raster(info, stride);
    check_source(info, config, &source, stride)
}
fn check_source(
    info: &ImageInfo,
    config: &ImiEncoderConfig,
    source: &[u8],
    stride: usize,
) -> Vec<u8> {
    let mut prefixed = vec![7; 11];
    prefixed.extend_from_slice(source);
    let mut a = Cursor::new(prefixed.clone());
    let mut b = Cursor::new(prefixed);
    a.set_position(11);
    b.set_position(11);
    let cfg = BoundedLossyConfig::new(stride as u64, LossyUnitIndex::Trailing, 8 << 20);
    let (native, _, _) = imij::stream_encode::encode_lossy_bounded(
        &mut a,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        config.checksum().then(|| Cursor::new(Vec::new())),
        info.clone(),
        &Metadata::default(),
        config,
        cfg,
        &how_far::NoPulse,
    )
    .unwrap();
    let expected = imij::decode(native.get_ref()).unwrap();
    let (file, index, checksum) = imij::v4::encode_lossy_bounded(
        &mut b,
        Vec::new(),
        Cursor::new(vec![11, 13]),
        config.checksum().then(|| Cursor::new(vec![17, 19])),
        info.clone(),
        &Metadata::default(),
        config,
        imij::v4::BoundedLossyConfig::new(stride as u64, 8 << 20),
        &how_far::NoPulse,
    )
    .unwrap();
    assert_eq!(imij::header(&file).unwrap().version, 4);
    assert_eq!(imij::decode(&file).unwrap(), expected);
    assert_eq!(&index.get_ref()[..2], &[11, 13]);
    assert_eq!(checksum.is_some(), config.checksum());
    if let Some(checksum) = checksum {
        assert_eq!(&checksum.get_ref()[..2], &[17, 19]);
    }
    for threads in [1, 2, 4, 8] {
        let stride = info.row_bytes() + 13;
        let mut out = vec![0xa7; stride * info.height as usize];
        imij::decode_into_threads(&file, &mut out, stride, threads).unwrap();
        for y in 0..info.height as usize {
            assert_eq!(
                &out[y * stride..][..info.row_bytes()],
                &expected.1[y * info.row_bytes()..][..info.row_bytes()]
            );
            assert!(
                out[y * stride + info.row_bytes()..(y + 1) * stride]
                    .iter()
                    .all(|&b| b == 0xa7)
            );
        }
    }
    file
}
#[test]
fn bounded_v4_matches_native_samples_coders_alpha_and_checksums() {
    for sample in [
        SampleType::U8,
        SampleType::U16,
        SampleType::U32,
        SampleType::F16,
        SampleType::F32,
    ] {
        for channels in [
            Channels::Gray,
            Channels::GrayAlpha,
            Channels::Rgb,
            Channels::Rgba,
        ] {
            let info = ImageInfo::new(67, 51, channels, sample);
            for coder in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                for checksum in [false, true] {
                    let config = ImiEncoderConfig::lossy(73.0)
                        .with_effort(0.0)
                        .with_column_groups(Some(32))
                        .with_checksum(checksum)
                        .tune_lossy(|t| {
                            t.entropy = coder;
                            t.stripe_block_rows = 2;
                            t.exact_blocks = Some(0.0);
                        });
                    check(&info, &config);
                }
            }
        }
    }
}
#[test]
fn bounded_v4_preserves_tiers_exact_small_layouts_and_local_models() {
    let info = ImageInfo::new(137, 99, Channels::Rgba, SampleType::U8);
    for coder in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
        for speed in [
            DecodeSpeed::Fastest,
            DecodeSpeed::Fast,
            DecodeSpeed::Balanced,
            DecodeSpeed::Compact,
        ] {
            let config = ImiEncoderConfig::lossy(80.0)
                .with_decode_speed(speed)
                .with_column_groups(Some(32))
                .with_checksum(true)
                .tune_lossy(|t| t.entropy = coder);
            check(&info, &config);
        }
        for mode in 0..4 {
            let config = ImiEncoderConfig::lossy(80.0)
                .with_effort(7.5)
                .with_column_groups(Some(32))
                .with_local_ans_contexts(mode == 1 && coder == LossyEntropy::RunLevel)
                .with_model_references(mode == 2)
                .with_coding_defaults(mode == 3)
                .tune_lossy(|t| {
                    t.entropy = coder;
                    t.exact_blocks = Some(1000.0);
                    t.stripe_block_rows = 2;
                });
            check(&info, &config);
        }
    }
}

#[test]
fn bounded_v4_preserves_fixed_adaptive_and_sampled_plane_layouts() {
    use imij::{
        ChannelSampling as Sampling, LossyChannelLayout as Layout, LossyTransform as Transform,
    };
    for sample in [
        SampleType::U8,
        SampleType::U16,
        SampleType::U32,
        SampleType::F16,
        SampleType::F32,
    ] {
        let info = ImageInfo::new(41, 29, Channels::Rgba, sample);
        for transform in [
            Transform::Dct8,
            Transform::Dct4,
            Transform::Dct2,
            Transform::Adaptive4,
        ] {
            for sampling in [
                Sampling::Full,
                Sampling::HalfHorizontal,
                Sampling::HalfVertical,
                Sampling::HalfBoth,
            ] {
                for coder in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                    let config = ImiEncoderConfig::lossy(73.0)
                        .with_effort(0.0)
                        .with_column_groups(Some(16))
                        .tune_lossy(|t| {
                            t.entropy = coder;
                            t.exact_blocks = Some(0.0);
                            t.stripe_block_rows = 2;
                            t.channel_layouts = [Layout::new(transform, sampling); 3];
                        });
                    check(&info, &config);
                }
            }
        }
    }
}

#[cfg(feature = "lab")]
#[test]
fn bounded_v4_dc_bands_are_distinct_contiguous_spatial_tiles() {
    let info = ImageInfo::new(137, 259, Channels::Rgba, SampleType::U8);
    let config = ImiEncoderConfig::lossy(73.0)
        .with_decode_speed(DecodeSpeed::Fastest)
        .with_column_groups(Some(32))
        .tune_lossy(|t| {
            t.exact_blocks = Some(0.0);
            t.stripe_block_rows = 2;
        });
    let file = check(&info, &config);
    let trace: serde_json::Value =
        serde_json::from_str(&imij::lab::codestream_trace(&file)).unwrap();
    assert_eq!(trace["decode"], "ok");
    let tiles = trace["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["kind"] == "tile")
        .count();
    assert!(tiles > 1);
    let expected = imij::decode(&file).unwrap().1;
    let rect = imij::planes::Rect::new(3, 5, 17, 19);
    let stride = rect.width() as usize * info.bytes_per_pixel() + 13;
    let mut output = vec![0xa7; stride * rect.height() as usize];
    imij::region::decode_region(&file, rect, &mut output, stride).unwrap();
    for y in 0..rect.height() as usize {
        let at =
            (rect.y() as usize + y) * info.row_bytes() + rect.x() as usize * info.bytes_per_pixel();
        assert_eq!(
            &output[y * stride..][..rect.width() as usize * info.bytes_per_pixel()],
            &expected[at..][..rect.width() as usize * info.bytes_per_pixel()]
        );
    }
}

#[test]
fn bounded_v4_front_tail_and_none_preserve_pixels_rows_regions_and_scratch() {
    use imij::v4::{BoundedLossyConfig as Config, DirectoryPlacement as Placement};
    let info = ImageInfo::new(137, 99, Channels::Rgba, SampleType::U8);
    let stride = info.row_bytes() + 19;
    let source = raster(&info, stride);
    for coder in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
        for checksum in [false, true] {
            let coding = ImiEncoderConfig::lossy(73.0)
                .with_decode_speed(DecodeSpeed::Compact)
                .with_column_groups(Some(32))
                .with_checksum(checksum)
                .tune_lossy(|t| t.entropy = coder);
            let baseline = check(&info, &coding);
            let expected = imij::decode(&baseline).unwrap().1;
            let cfg = Config::new(stride as u64, 8 << 20);
            for cfg in [
                cfg,
                cfg.without_directory(),
                cfg.with_front_directory(16 << 10),
            ] {
                let mut src = Cursor::new(source.clone());
                let (out, index, sums) = imij::v4::encode_lossy_bounded_seekable(
                    &mut src,
                    Cursor::new(vec![7, 11, 13]),
                    Cursor::new(vec![17, 19]),
                    checksum.then(|| Cursor::new(vec![23, 29])),
                    info.clone(),
                    &Metadata::default(),
                    &coding,
                    cfg,
                    &how_far::NoPulse,
                )
                .unwrap();
                // Cursor starts at zero above, so the output owns its whole file.
                let file = out.into_inner();
                assert_eq!(imij::decode(&file).unwrap().1, expected);
                assert_eq!(&index.get_ref()[..2], &[17, 19]);
                if let Some(sums) = sums {
                    assert_eq!(&sums.get_ref()[..2], &[23, 29]);
                }
                let mut rows = Vec::new();
                imij::decode_resources::DecodeSession::new(Default::default())
                    .decode_reader(file.as_slice(), |_, _, row| {
                        rows.extend_from_slice(row);
                        Ok(())
                    })
                    .unwrap();
                assert_eq!(rows, expected);
                let mut region =
                    imij::region::RegionReader::open(imij::planes::SliceReader::new(&file))
                        .unwrap();
                assert_eq!(region.has_unit_table(), cfg.directory != Placement::None);
                let rect = imij::planes::Rect::new(5, 7, 21, 19);
                let stride = rect.width() as usize * info.bytes_per_pixel() + 13;
                let mut out = vec![0xa7; stride * rect.height() as usize];
                region
                    .decode_region_threads(rect, &mut out, stride, 4)
                    .unwrap();
                for y in 0..rect.height() as usize {
                    let at = (rect.y() as usize + y) * info.row_bytes()
                        + rect.x() as usize * info.bytes_per_pixel();
                    assert_eq!(
                        &out[y * stride..][..rect.width() as usize * info.bytes_per_pixel()],
                        &expected[at..][..rect.width() as usize * info.bytes_per_pixel()]
                    );
                    assert!(
                        out[y * stride + rect.width() as usize * info.bytes_per_pixel()
                            ..(y + 1) * stride]
                            .iter()
                            .all(|&b| b == 0xa7)
                    );
                }
            }
        }
    }
}

#[test]
fn bounded_v4_front_admission_and_short_reservation_never_finalize() {
    let info = ImageInfo::new(41, 31, Channels::Rgb, SampleType::U8);
    let source = raster(&info, info.row_bytes());
    let coding = ImiEncoderConfig::lossy(73.0).with_column_groups(Some(16));
    let cfg = imij::v4::BoundedLossyConfig::new(info.row_bytes() as u64, 1 << 20);
    let mut output = Vec::new();
    assert!(
        imij::v4::encode_lossy_bounded(
            &mut Cursor::new(&source),
            &mut output,
            Cursor::new(Vec::new()),
            None,
            info.clone(),
            &Metadata::default(),
            &coding,
            cfg.with_front_directory(4096),
            &how_far::NoPulse
        )
        .is_err()
    );
    assert!(output.is_empty());
    let mut output = Cursor::new(Vec::new());
    let result = imij::v4::encode_lossy_bounded_seekable(
        &mut Cursor::new(&source),
        &mut output,
        Cursor::new(Vec::new()),
        None,
        info,
        &Metadata::default(),
        &coding,
        cfg.with_front_directory(1),
        &how_far::NoPulse,
    );
    assert!(matches!(
        result,
        Err(imij::stream_encode::StreamEncodeError::Codec(
            imij::Error::TooLarge
        ))
    ));
    assert!(imij::decode(output.get_ref()).is_err());
}

#[cfg(feature = "lab")]
#[test]
fn bounded_v4_exercises_inline_models_on_changing_band_statistics() {
    let info = ImageInfo::new(1025, 131, Channels::Rgba, SampleType::U8);
    let stride = info.row_bytes() + 13;
    let mut source = vec![0xa7; stride * info.height as usize];
    for y in 0..info.height as usize {
        for x in 0..info.row_bytes() {
            let raw = (x * 997 + y * 7907 + x * y * 11) & 255;
            source[y * stride + x] = if y < 64 { 63 + raw / 16 } else { raw } as u8;
        }
    }
    for references in [false, true] {
        let mut selected = false;
        for quality in [5.0, 40.0, 95.0] {
            let config = ImiEncoderConfig::lossy(quality)
                .with_effort(0.0)
                .with_column_groups(Some(512))
                .with_local_ans_contexts(!references)
                .with_model_references(references)
                .tune_lossy(|t| {
                    t.stripe_block_rows = 8;
                    t.exact_blocks = Some(0.0);
                });
            let file = check_source(&info, &config, &source, stride);
            let trace: serde_json::Value =
                serde_json::from_str(&imij::lab::codestream_trace(&file)).unwrap();
            assert_eq!(trace["decode"], "ok");
            selected |= trace["nodes"].as_array().unwrap().iter().any(|node| {
                node["label"] == "model ID"
                    && node["value"]
                        .as_str()
                        .and_then(|id| id.parse::<u64>().ok())
                        .is_some_and(|id| id != 0)
            });
        }
        assert!(
            selected,
            "fixture must actually select an inline model, references={references}"
        );
    }
}
