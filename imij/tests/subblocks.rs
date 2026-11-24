//! Public-path parity and malformed-input coverage for subblocks and sampling.
#![cfg(feature = "std")]
mod common;
use common::data_payload;

/// The file's header says lossy coding (the payload repeats no version).
fn is_lossy(file: &[u8]) -> bool {
    imij::header(file).unwrap().coding == imij::Coding::Lossy
}

use imij::{
    ChannelSampling as Sampling, Channels, ImageInfo, ImiEncoderConfig,
    LossyChannelLayout as Layout, LossyColor, LossyEntropy, LossyTransform as Transform, Metadata,
    SampleType,
};

const ENTROPY: [LossyEntropy; 2] = [LossyEntropy::RunLevel, LossyEntropy::SplitHuf];
const SAMPLING: [Sampling; 4] = [
    Sampling::Full,
    Sampling::HalfHorizontal,
    Sampling::HalfVertical,
    Sampling::HalfBoth,
];
const TRANSFORMS: [Transform; 4] = [
    Transform::Dct8,
    Transform::Dct4,
    Transform::Dct2,
    Transform::Adaptive4,
];

fn pixels(info: &ImageInfo) -> Vec<u8> {
    let mut out = Vec::with_capacity(info.row_bytes() * info.height as usize);
    for y in 0..info.height as usize {
        for x in 0..info.width as usize {
            for c in 0..info.channels.count() {
                let v =
                    ((x * (17 + c * 7) + y * (11 + c * 13) + ((x / 3) ^ (y / 5)) * 23) % 256) as u8;
                match info.sample {
                    SampleType::U8 => out.push(v),
                    SampleType::U16 => out.extend_from_slice(&(u16::from(v) * 257).to_ne_bytes()),
                    SampleType::F16 => {
                        out.extend_from_slice(&(0x3000u16 + u16::from(v) * 11).to_ne_bytes())
                    }
                    SampleType::F32 => out.extend_from_slice(&(f32::from(v) / 255.0).to_ne_bytes()),
                    _ => unreachable!(),
                }
            }
        }
    }
    out
}

fn encode(info: &ImageInfo, source: &[u8], cfg: &ImiEncoderConfig) -> Vec<u8> {
    cfg.encode_legacy_samples(
        imij::ImageRef::new(source, info.row_bytes(), info),
        &Metadata::default(),
    )
    .unwrap()
}

fn parity(file: &[u8], source: &[u8], info: &ImageInfo) -> Vec<u8> {
    let (got_info, expected) = imij::decode(file).unwrap();
    assert_eq!(&got_info, info);
    assert_eq!(expected.len(), source.len());
    if info.channels.has_alpha() {
        let bpp = info.bytes_per_pixel();
        let bps = info.sample.bytes();
        for (a, b) in source.chunks_exact(bpp).zip(expected.chunks_exact(bpp)) {
            assert_eq!(&a[bpp - bps..], &b[bpp - bps..], "lossless alpha");
        }
    }
    for threads in [1, 4] {
        let stride = info.row_bytes() + 13;
        let mut padded = vec![0xa5; stride * info.height as usize];
        imij::decode_into_threads(file, &mut padded, stride, threads).unwrap();
        for (a, b) in padded
            .chunks_exact(stride)
            .zip(expected.chunks_exact(info.row_bytes()))
        {
            assert_eq!(&a[..info.row_bytes()], b, "thread count {threads}");
            assert!(a[info.row_bytes()..].iter().all(|&v| v == 0xa5));
        }
    }
    let mut stream = imij::StreamDecoder::new(file).unwrap();
    let mut row = vec![0; info.row_bytes()];
    for b in expected.chunks_exact(info.row_bytes()) {
        assert!(stream.next_row(&mut row).unwrap());
        assert_eq!(row, b);
    }
    assert!(!stream.next_row(&mut row).unwrap());
    let mut reader_pixels = Vec::new();
    let mut next = 0;
    let (read_info, read_meta) = imij::decode_reader(file, &imij::Limits::DEFAULT, |i, y, row| {
        assert_eq!(i, info);
        assert_eq!(y, next);
        next += 1;
        reader_pixels.extend_from_slice(row);
        Ok(())
    })
    .unwrap();
    assert_eq!(read_info, *info);
    assert_eq!(read_meta, imij::probe(file).unwrap().1);
    assert_eq!(reader_pixels, expected);
    expected
}

#[test]
fn each_channel_can_use_every_transform_and_sampling() {
    let info = ImageInfo::new(19, 25, Channels::Rgba, SampleType::U8);
    let source = pixels(&info);
    // Full independent coverage of channel × transform × sampling; entropy,
    // AQ and color vary across cases without multiplying the entire matrix.
    for channel in 0..3 {
        for (ti, transform) in TRANSFORMS.into_iter().enumerate() {
            for (si, sampling) in SAMPLING.into_iter().enumerate() {
                let mut layouts = [Layout::default(); 3];
                layouts[channel] = Layout::new(transform, sampling);
                let mut cfg = imij::ImiEncoderConfig::lossy(83.0)
                    .with_effort(0.0)
                    .tune_lossy(|t| {
                        t.color = if ti % 2 == 0 {
                            LossyColor::Xyb
                        } else {
                            LossyColor::YCoCg
                        }
                    })
                    .tune_lossy(|t| t.entropy = ENTROPY[(channel + ti + si) % ENTROPY.len()])
                    .tune_lossy(|t| t.channel_layouts = layouts)
                    .tune_lossy(|t| t.compact_streams = true);
                cfg = cfg.tune_lossy(|t| {
                    t.aq_gamma = if (ti + si) % 2 == 0 { 0.4 } else { 0.0 };
                });
                let file = encode(&info, &source, &cfg);
                assert!(is_lossy(&file));
                parity(&file, &source, &info);
            }
        }
    }
}

#[test]
fn mixed_layouts_cross_groups_stripes_channels_and_sample_formats() {
    let layouts = [
        Layout::new(Transform::Adaptive4, Sampling::Full),
        Layout::new(Transform::Dct2, Sampling::HalfVertical),
        Layout::new(Transform::Dct4, Sampling::HalfHorizontal),
    ];
    for channels in [
        Channels::Rgb,
        Channels::Rgba,
        Channels::Gray,
        Channels::GrayAlpha,
    ] {
        for entropy in ENTROPY {
            let info = ImageInfo::new(37, 73, channels, SampleType::U8);
            let source = pixels(&info);
            let mut cfg = imij::ImiEncoderConfig::lossy(75.0)
                .with_effort(0.0)
                .tune_lossy(|t| t.entropy = entropy)
                .tune_lossy(|t| t.channel_layouts = layouts)
                .tune_lossy(|t| t.compact_streams = true);
            cfg = cfg.tune_lossy(|t| {
                t.aq_gamma = 0.5;
            });
            for (group, stripe_rows) in [(0, 1), (16, 3)] {
                cfg = cfg.with_column_groups(Some(group));
                cfg = cfg.tune_lossy(|t| {
                    t.stripe_block_rows = stripe_rows;
                });
                let file = encode(&info, &source, &cfg);
                parity(&file, &source, &info);
                cfg = cfg.with_threads(4);
                assert_eq!(
                    encode(&info, &source, &cfg),
                    file,
                    "encoder schedule changed bytes"
                );
                cfg = cfg.with_threads(1);
            }
        }
    }
    for sample in [SampleType::U16, SampleType::F16, SampleType::F32] {
        for channels in [Channels::Rgba, Channels::GrayAlpha] {
            let info = ImageInfo::new(17, 19, channels, sample);
            let source = pixels(&info);
            let cfg = imij::ImiEncoderConfig::lossy(90.0)
                .with_effort(0.0)
                .tune_lossy(|t| {
                    t.color = if sample == SampleType::U16 {
                        LossyColor::YCoCg
                    } else {
                        LossyColor::Xyb
                    }
                })
                .tune_lossy(|t| t.channel_layouts = layouts)
                .tune_lossy(|t| t.compact_streams = true);
            parity(&encode(&info, &source, &cfg), &source, &info);
        }
    }
}

#[test]
fn crop_views_and_metadata_do_not_change_pixel_phase() {
    let info = ImageInfo::new(13, 11, Channels::Rgba, SampleType::U8);
    let source = pixels(&info);
    let cfg = imij::ImiEncoderConfig::lossy(88.0)
        .with_effort(0.0)
        .tune_lossy(|t| {
            t.channel_layouts = [
                Layout::new(Transform::Dct2, Sampling::Full),
                Layout::new(Transform::Dct4, Sampling::HalfBoth),
                Layout::new(Transform::Adaptive4, Sampling::HalfHorizontal),
            ]
        });
    let meta = Metadata {
        exif: Some(b"subblock crop test".to_vec()),
        orientation: 6,
        ..Metadata::default()
    };
    let packed = cfg
        .encode_legacy_samples(imij::ImageRef::new(&source, info.row_bytes(), &info), &meta)
        .unwrap();
    for oy in 0..8 {
        for ox in 0..8 {
            let stride = (info.width as usize + 9) * info.bytes_per_pixel() + 3;
            let start = oy * stride + ox * info.bytes_per_pixel();
            let mut storage = vec![0x5a; start + stride * info.height as usize];
            for y in 0..info.height as usize {
                storage[start + y * stride..start + y * stride + info.row_bytes()]
                    .copy_from_slice(&source[y * info.row_bytes()..(y + 1) * info.row_bytes()]);
            }
            let file = cfg
                .encode_legacy_samples(imij::ImageRef::new(&storage[start..], stride, &info), &meta)
                .unwrap();
            assert_eq!(file, packed, "view origin {ox},{oy}");
        }
    }
    let (_, got_meta) = imij::probe(&packed).unwrap();
    assert_eq!(got_meta, meta);
    parity(&packed, &source, &info);
}

#[test]
fn full_restart_states_and_default_compaction_keep_pixels() {
    for entropy in ENTROPY {
        for color in [LossyColor::Xyb, LossyColor::YCoCg] {
            let info = ImageInfo::new(17, 9, Channels::Rgb, SampleType::U8);
            for source in [
                pixels(&info),
                vec![128; info.row_bytes() * info.height as usize],
            ] {
                let cfg = imij::ImiEncoderConfig::lossy(80.0)
                    .with_effort(0.0)
                    .tune_lossy(|t| t.color = color)
                    .tune_lossy(|t| t.entropy = entropy);
                let original = encode(
                    &info,
                    &source,
                    &cfg.clone().tune_lossy(|t| t.compact_streams = false),
                );
                assert!(is_lossy(&original));
                let explicit = cfg
                    .clone()
                    .tune_lossy(|t| {
                        t.channel_layouts = [Layout::new(Transform::Dct8, Sampling::Full); 3]
                    })
                    .tune_lossy(|t| t.compact_streams = false);
                assert_eq!(encode(&info, &source, &explicit), original);
                let compact = encode(&info, &source, &cfg);
                assert_eq!(
                    compact,
                    encode(
                        &info,
                        &source,
                        &cfg.tune_lossy(|t| t.compact_streams = true)
                    )
                );
                assert!(is_lossy(&compact));
                assert_eq!(
                    imij::decode(&compact).unwrap(),
                    imij::decode(&original).unwrap()
                );
                // Restart-state compaction exists for run-level rANS only;
                // it must save bytes on flat content, and the split modes
                // must come out identical either way.
                if entropy == LossyEntropy::RunLevel {
                    if source.iter().all(|&b| b == 128) {
                        assert!(
                            compact.len() < original.len(),
                            "flat compact {} >= full states {} for {entropy:?}",
                            compact.len(),
                            original.len()
                        );
                    }
                } else {
                    assert_eq!(compact, original, "{entropy:?}");
                }
            }
        }
    }
}

#[test]
fn unsampled_small_transforms_retain_high_quality_colored_edges() {
    let info = ImageInfo::new(31, 27, Channels::Rgb, SampleType::U8);
    for transform in [Transform::Dct4, Transform::Dct2, Transform::Adaptive4] {
        for pattern in 0..2 {
            let mut source = vec![0; info.row_bytes() * info.height as usize];
            for y in 0..info.height as usize {
                for x in 0..info.width as usize {
                    let rgb = if pattern == 0 {
                        [
                            (x * 255 / 30) as u8,
                            (y * 255 / 26) as u8,
                            ((x + y) * 255 / 56) as u8,
                        ]
                    } else if (x % 7 == 1 || y % 9 == 2) && (x + y) % 3 != 0 {
                        [241, 19, 167]
                    } else {
                        [13, 225, 71]
                    };
                    source[y * info.row_bytes() + x * 3..y * info.row_bytes() + x * 3 + 3]
                        .copy_from_slice(&rgb);
                }
            }
            let cfg = imij::ImiEncoderConfig::lossy(116.0)
                .with_effort(0.0)
                .tune_lossy(|t| t.color = LossyColor::YCoCg)
                .tune_lossy(|t| t.channel_layouts = [Layout::new(transform, Sampling::Full); 3]);
            let file = cfg
                .encode_legacy_samples(
                    imij::ImageRef::new(&source, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap();
            let decoded = parity(&file, &source, &info);
            let max_error = source
                .iter()
                .zip(&decoded)
                .map(|(&a, &b)| a.abs_diff(b))
                .max()
                .unwrap();
            assert!(
                max_error <= 2,
                "{transform:?} pattern {pattern} max error {max_error}"
            );
        }
    }
}

fn rejected(file: &[u8]) {
    assert!(imij::decode(file).is_err());
    assert!(imij::decode_reader(file, &imij::Limits::DEFAULT, |_, _, _| Ok(())).is_err());
    let result = imij::StreamDecoder::new(file).and_then(|mut decoder| {
        let mut row = vec![0; decoder.info().row_bytes()];
        while decoder.next_row(&mut row)? {}
        Ok(())
    });
    assert!(result.is_err());
}

/// SPEC 2.8: entropy values 1 (packed) and 2 (split) are retired — every
/// decode route reports `Unsupported`, not a generic corruption.
#[test]
fn retired_entropy_values_are_unsupported() {
    let info = ImageInfo::new(3, 5, Channels::Rgb, SampleType::U8);
    let cfg = imij::ImiEncoderConfig::lossy(75.0)
        .with_effort(0.0)
        .tune_lossy(|t| t.entropy = LossyEntropy::SplitHuf)
        .tune_lossy(|t| t.compact_streams = true);
    let file = encode(&info, &pixels(&info), &cfg);
    let data = data_payload(&file).start;
    let p = common::payload(&file[data..], &info, true);
    let scale_at = data + p.present_at + common::varint(&file[data + p.present_at..]).1;
    let entropy_at = scale_at + 4 + usize::from(p.present & 1 != 0);
    assert_eq!(file[entropy_at], LossyEntropy::SplitHuf as u8);
    for v in [1, 2] {
        let mut bad = file.clone();
        bad[entropy_at] = v;
        let want = Err(imij::Error::Unsupported("retired entropy mode"));
        assert_eq!(imij::decode(&bad).map(|_| ()), want, "entropy {v}");
        assert_eq!(
            imij::decode_reader(&bad[..], &imij::Limits::DEFAULT, |_, _, _| Ok(())).map(|_| ()),
            want,
            "entropy {v}"
        );
        let stream = imij::StreamDecoder::new(&bad).and_then(|mut decoder| {
            let mut row = vec![0; decoder.info().row_bytes()];
            while decoder.next_row(&mut row)? {}
            Ok(())
        });
        assert_eq!(stream, want, "entropy {v}");
    }
}

#[test]
fn truncations_and_bad_envelope_fields_are_rejected() {
    let info = ImageInfo::new(3, 5, Channels::Rgb, SampleType::U8);
    let cfg = imij::ImiEncoderConfig::lossy(75.0)
        .with_effort(0.0)
        .tune_lossy(|t| t.entropy = LossyEntropy::SplitHuf)
        .tune_lossy(|t| t.channel_layouts = [Layout::new(Transform::Dct4, Sampling::Full); 3])
        .tune_lossy(|t| t.compact_streams = true);
    let file = encode(&info, &pixels(&info), &cfg);
    for cut in 0..file.len() {
        rejected(&file[..cut]);
    }
    assert!(is_lossy(&file));
    // SPEC 2.8: the parameters (`present`, the quantizer scale, the
    // fields), the tables, the head records (here the channel layouts,
    // record 27), then the nested heads.
    let data = data_payload(&file).start;
    let p = common::payload(&file[data..], &info, true);
    let present = p.present;
    let present_at = data + p.present_at;
    let scale_at = present_at + common::varint(&file[present_at..]).1;
    let seed_at = scale_at + 4;
    let entropy_at = seed_at + usize::from(present & 1 != 0);
    assert_ne!(present & 2, 0, "non-default entropy is written");
    assert_eq!(file[entropy_at], LossyEntropy::SplitHuf as u8);
    let layouts = p.head_record(27).expect("a layouts record");
    let layout_at = data + layouts.start;
    let (list_len_at, _) = p.list.clone().unwrap();
    let list_len_at = data + list_len_at;
    for (offset, value) in [
        (list_len_at, 0),
        (list_len_at, 255),
        // The seed byte is present exactly when bit 0 is set.
        (present_at, file[present_at] ^ 1),
        (present_at, file[present_at] & !16),
        (layout_at, 128),
        (layout_at + 1, 12),
        (layout_at + 2, 16 | 4),
        (entropy_at, 255),
        (entropy_at, 1),
        (entropy_at, 2),
    ] {
        let mut bad = file.clone();
        bad[offset] = value;
        rejected(&bad);
    }
    for value in [f32::NAN, f32::INFINITY, -1.0] {
        let mut bad = file.clone();
        bad[scale_at..scale_at + 4].copy_from_slice(&value.to_le_bytes());
        rejected(&bad);
    }
    if present & 1 != 0 {
        // Reserved bit, no scope, bank 0, unknown bank.
        let seed = file[seed_at];
        for value in [seed | 0x80, seed & 0x0F, seed & 0x70, (seed & 0x70) | 2] {
            let mut bad = file.clone();
            bad[seed_at] = value;
            rejected(&bad);
        }
    }
}
