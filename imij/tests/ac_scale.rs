//! Uniform AC control selected from PR89 at 98013cd7; offline search is a separate arm.
use imij::stream_encode::{BoundedLossyConfig, LossyUnitIndex, encode_lossy_bounded};
use imij::{
    Channels, ImageInfo, ImageRef, ImiEncoderConfig, LossyColor, LossyEntropy, Metadata, SampleType,
};
use std::io::Cursor;

fn fixture(w: u32, h: u32) -> (ImageInfo, Vec<u8>, usize) {
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let stride = info.row_bytes() + 13;
    let mut px = vec![0xA7; stride * (h as usize - 1) + info.row_bytes()];
    for y in 0..h as usize {
        for x in 0..w as usize {
            for c in 0..3 {
                px[y * stride + 3 * x + c] = ((x * 13 + y * 17 + c * 51 + x * y % 101) % 256) as u8;
            }
        }
    }
    (info, px, stride)
}
fn config(q: f32, color: LossyColor, entropy: LossyEntropy) -> ImiEncoderConfig {
    ImiEncoderConfig::lossy(q).with_effort(5.0).tune_lossy(|t| {
        t.color = color;
        t.entropy = entropy;
        t.small_transforms = false;
        t.adaptive_partitions = false;
        t.exact_blocks = Some(0.0);
    })
}

#[test]
fn default_ac_scale_preserves_bytes_and_configuration_budget() {
    let (info, px, stride) = fixture(35, 19);
    let image = ImageRef::new(&px, stride, &info);
    let meta = Metadata::default();
    let base = config(25.0, LossyColor::Xyb, LossyEntropy::RunLevel);
    assert_eq!(
        base.encode_samples(image, &meta).unwrap(),
        base.clone()
            .with_ac_scale(1.)
            .encode_samples(image, &meta)
            .unwrap()
    );
    assert_eq!(base.ac_scale(), 1.0);
    assert_eq!(
        base.clone()
            .with_ac_scale(0.5)
            .tune_lossy(|t| t.stripe_block_rows = 2)
            .ac_scale(),
        0.5
    );
    assert!(core::mem::size_of::<ImiEncoderConfig>() <= 128);
}

#[test]
fn ac_control_matches_bounded_and_region_routes() {
    use imij::planes::{Rect, SliceReader};
    use imij::region::RegionReader;
    let (info, mut px, stride) = fixture(67, 43);
    // Stream source includes trailing row padding; raw API also admits its omission.
    px.resize(stride * info.height as usize, 0xA7);
    for color in [LossyColor::Xyb, LossyColor::YCoCg] {
        for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
            for scale in [0.25, 0.5, 1., 2., 4.] {
                let cfg = config(40., color, entropy)
                    .with_ac_scale(scale)
                    .with_column_groups(Some(32));
                let whole = cfg
                    .encode_samples(ImageRef::new(&px, stride, &info), &Metadata::default())
                    .unwrap();
                let want = imij::decode(&whole).unwrap().1;
                let (out, _, _) = encode_lossy_bounded(
                    &mut Cursor::new(&px),
                    Cursor::new(Vec::new()),
                    Cursor::new(Vec::new()),
                    None,
                    info.clone(),
                    &Metadata::default(),
                    &cfg,
                    BoundedLossyConfig::new(stride as u64, LossyUnitIndex::Trailing, 1 << 20),
                    &how_far::NoPulse,
                )
                .unwrap();
                let file = out.into_inner();
                assert_eq!(imij::decode(&file).unwrap().1, want);
                let mut reader = RegionReader::open(SliceReader::new(&file)).unwrap();
                let r = Rect::new(7, 9, 23, 17);
                let pitch = 23 * 3 + 11;
                let mut got = vec![0xA7; pitch * 17];
                reader.decode_region(r, &mut got, pitch).unwrap();
                for y in 0..17 {
                    let at = ((y + 9) * 67 + 7) * 3;
                    assert_eq!(&got[y * pitch..y * pitch + 69], &want[at..at + 69]);
                    assert!(
                        got[y * pitch + 69..(y + 1) * pitch]
                            .iter()
                            .all(|&v| v == 0xA7)
                    );
                }
            }
        }
    }
}

#[cfg(feature = "lab")]
#[test]
fn header_ac_scale_reproduces_constant_aq_pixels_without_fourth_channel() {
    let (info, px, stride) = fixture(67, 43);
    let image = ImageRef::new(&px, stride, &info);
    for color in [LossyColor::Xyb, LossyColor::YCoCg] {
        for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
            for q in [5., 10., 40., 90., 100.] {
                let base = config(q, color, entropy);
                let cfg = base.clone().with_ac_scale(0.5);
                let map =
                    vec![32; info.width.div_ceil(8) as usize * info.height.div_ceil(8) as usize];
                let old = base
                    .clone()
                    .tune_lossy(|t| t.lab.aq_map = Some(map.into()))
                    .encode_samples(image, &Metadata::default())
                    .unwrap();
                let new = cfg.encode_samples(image, &Metadata::default()).unwrap();
                assert_eq!(
                    imij::decode(&new).unwrap().1,
                    imij::decode(&old).unwrap().1,
                    "q{q} {color:?} {entropy:?}"
                );
                assert_eq!(
                    imij::lab::lossy_dc_image(&new).unwrap().0.channels.count(),
                    3
                );
                assert_eq!(
                    imij::lab::lossy_dc_image(&old).unwrap().0.channels.count(),
                    4
                );
            }
        }
    }
}

#[cfg(feature = "lab")]
#[test]
fn dc_compensation_refuses_fractional_or_overflowing_wire_codes() {
    let (info, px, stride) = fixture(8, 8);
    let image = ImageRef::new(&px, stride, &info);
    for (scale, dc) in [(2.0, [47, 49]), (0.25, [255, 255])] {
        let cfg = ImiEncoderConfig::lossy(30.)
            .with_ac_scale(scale)
            .tune_lossy(|t| t.lab.switches.dc_step = Some(dc));
        assert!(cfg.encode_samples(image, &Metadata::default()).is_err());
    }
}

#[test]
fn ac_control_remains_decodable_with_tiers_trellis_and_alpha() {
    use imij::DecodeSpeed;
    let (info, px, stride) = fixture(129, 65);
    for tier in [
        DecodeSpeed::Compact,
        DecodeSpeed::Balanced,
        DecodeSpeed::Fast,
        DecodeSpeed::Fastest,
    ] {
        for effort in [5., 9., 10.] {
            for scale in [0.5, 2.] {
                let cfg = ImiEncoderConfig::lossy(40.)
                    .with_decode_speed(tier)
                    .with_effort(effort)
                    .with_ac_scale(scale)
                    .with_checksum(true);
                let file = cfg
                    .encode_samples(ImageRef::new(&px, stride, &info), &Metadata::default())
                    .unwrap();
                let (got, packed) = imij::decode(&file).unwrap();
                assert_eq!(got, info);
                let pitch = info.row_bytes() + 7;
                let mut strided = vec![0xA7; pitch * info.height as usize];
                imij::decode_into_threads(&file, &mut strided, pitch, 2).unwrap();
                for y in 0..info.height as usize {
                    assert_eq!(
                        &strided[y * pitch..y * pitch + info.row_bytes()],
                        &packed[y * info.row_bytes()..(y + 1) * info.row_bytes()]
                    );
                    assert!(
                        strided[y * pitch + info.row_bytes()..(y + 1) * pitch]
                            .iter()
                            .all(|&v| v == 0xA7)
                    );
                }
            }
        }
    }
    let rgba = ImageInfo::new(17, 9, Channels::Rgba, SampleType::U8);
    let stride = rgba.row_bytes() + 5;
    let mut px = vec![0xA7; stride * 9];
    for y in 0..9 {
        for x in 0..17 {
            px[y * stride + x * 4..y * stride + x * 4 + 4].copy_from_slice(&[
                (x * 13) as u8,
                (y * 23) as u8,
                127,
                (x * 7 + y * 3) as u8,
            ]);
        }
    }
    let file = ImiEncoderConfig::lossy(30.)
        .with_ac_scale(0.5)
        .encode_samples(ImageRef::new(&px, stride, &rgba), &Metadata::default())
        .unwrap();
    let got = imij::decode(&file).unwrap().1;
    for y in 0..9 {
        for x in 0..17 {
            assert_eq!(got[(y * 17 + x) * 4 + 3], px[y * stride + x * 4 + 3]);
        }
    }
}

#[cfg(feature = "lab")]
fn one_pass_with_live_rows(
    info: &ImageInfo,
    source: &[u8],
    stride: usize,
    cfg: &ImiEncoderConfig,
) -> (Vec<u8>, Vec<u8>) {
    use imij::codec::{DecodeConfig, FeedStatus};
    use imij::stream_encode::{BoundedLossySession, BoundedLossyStep, BoundedStream};
    use zencodec::decode::DecoderConfig as _;
    let mut encoder =
        BoundedLossySession::one_pass(info.clone(), &Metadata::default(), cfg, 1 << 20, 32 << 20)
            .unwrap();
    let mut decoder = DecodeConfig::default().job().feed_decoder(17).unwrap();
    let mut published = Vec::new();
    let mut row_count = 0u32;
    let pump =
        |decoder: &mut imij::codec::FeedDecoder, published: &mut Vec<u8>, row_count: &mut u32| {
            loop {
                match decoder.advance().unwrap() {
                    FeedStatus::NeedInput => break,
                    FeedStatus::Row { y } => {
                        assert_eq!(y, *row_count);
                        *row_count += 1;
                        published.extend_from_slice(decoder.row().unwrap().unwrap().row(0));
                    }
                    other => panic!("completion before explicit EOF: {other:?}"),
                }
            }
        };
    let (mut output, mut index, mut checksum) = (Vec::new(), Vec::new(), Vec::new());
    let mut supplied = 0u32;
    loop {
        match encoder.step().unwrap() {
            BoundedLossyStep::Rows { pass, y0, rows } => {
                assert_eq!(pass, 2);
                assert_eq!(y0, supplied, "source is consumed once and in order");
                assert_eq!(row_count, y0, "decoded rows precede the next source band");
                for (j, row) in rows.chunks_exact_mut(info.row_bytes()).enumerate() {
                    row.copy_from_slice(&source[(y0 as usize + j) * stride..][..info.row_bytes()]);
                    supplied += 1;
                }
            }
            BoundedLossyStep::Write {
                stream,
                offset,
                bytes,
            } => {
                let target = match stream {
                    BoundedStream::Output => {
                        assert_eq!(offset as usize, output.len(), "live output is append-only");
                        let mut at = 0;
                        while at < bytes.len() {
                            let n = decoder.push(&bytes[at..]).unwrap();
                            at += n;
                            pump(&mut decoder, &mut published, &mut row_count);
                            assert!(n != 0, "accepted input must drain before more bytes");
                        }
                        &mut output
                    }
                    BoundedStream::IndexScratch => &mut index,
                    BoundedStream::ChecksumScratch => &mut checksum,
                    _ => panic!("unexpected scratch stream"),
                };
                let at = offset as usize;
                target.resize(target.len().max(at + bytes.len()), 0);
                target[at..at + bytes.len()].copy_from_slice(bytes);
            }
            BoundedLossyStep::Read {
                stream,
                offset,
                buf,
            } => {
                let stored = match stream {
                    BoundedStream::Output => &output,
                    BoundedStream::IndexScratch => &index,
                    BoundedStream::ChecksumScratch => &checksum,
                    _ => panic!("unexpected scratch stream"),
                };
                buf.copy_from_slice(&stored[offset as usize..][..buf.len()]);
            }
            BoundedLossyStep::Done { len } => {
                assert_eq!(len as usize, output.len());
                assert_eq!(supplied, info.height);
                break;
            }
            _ => panic!("unexpected session request"),
        }
    }
    decoder.finish_input().unwrap();
    loop {
        match decoder.advance().unwrap() {
            FeedStatus::Row { y } => {
                assert_eq!(y, row_count);
                row_count += 1;
                published.extend_from_slice(decoder.row().unwrap().unwrap().row(0));
            }
            FeedStatus::Complete => break,
            other => panic!("valid complete file did not finish: {other:?}"),
        }
    }
    assert_eq!(row_count, info.height);
    assert_eq!(decoder.native_metadata().unwrap().image, *info);
    assert_eq!(imij::decode(&output).unwrap().1, published);
    (output, published)
}

#[cfg(feature = "lab")]
#[test]
fn ac_scale_one_pass_native_depths_keep_dc_alpha_and_pre_next_source_rows() {
    for bits in [8u8, 10, 12, 16] {
        for channels in [Channels::Gray, Channels::Rgba] {
            let info = ImageInfo::new(
                35,
                19,
                channels,
                if bits == 8 {
                    SampleType::U8
                } else {
                    SampleType::U16
                },
            )
            .with_significant_bits(&vec![bits; channels.count()])
            .unwrap();
            let stride = info.row_bytes() + 7;
            let mut source = vec![0xa7; stride * 18 + info.row_bytes()];
            let mut seed = 0x1469_a38du32;
            let bps = info.sample.bytes();
            for y in 0..19usize {
                for x in 0..35 * channels.count() {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    let value = seed & ((1u32 << bits) - 1);
                    let at = y * stride + x * bps;
                    if bps == 1 {
                        source[at] = value as u8;
                    } else {
                        source[at..at + 2].copy_from_slice(&(value as u16).to_ne_bytes());
                    }
                }
            }
            for color in [LossyColor::Xyb, LossyColor::YCoCg] {
                for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                    for stripe_rows in [1, 2] {
                        let base = ImiEncoderConfig::lossy(40.0)
                            .with_effort(0.0)
                            .with_threads(1)
                            .with_checksum(true)
                            .with_column_groups(Some(if stripe_rows == 1 { 16 } else { 32 }))
                            .tune_lossy(|t| {
                                t.color = color;
                                t.entropy = entropy;
                                t.aq_gamma = 0.0;
                                t.stripe_block_rows = stripe_rows;
                                t.small_transforms = false;
                                t.adaptive_partitions = false;
                            });
                        let (neutral, _) = one_pass_with_live_rows(&info, &source, stride, &base);
                        let dc = imij::lab::lossy_dc_image(&neutral).unwrap();
                        for scale in [0.25, 0.5, 1.0, 2.0, 4.0] {
                            let cfg = base.clone().with_ac_scale(scale);
                            let (file, rows) =
                                one_pass_with_live_rows(&info, &source, stride, &cfg);
                            assert_eq!(
                                imij::lab::lossy_dc_image(&file).unwrap(),
                                dc,
                                "DC changed at {bits} bits, {color:?}/{entropy:?}, scale {scale}"
                            );
                            if scale == 1.0 {
                                assert_eq!(file, neutral);
                            }
                            let ordinary = cfg
                                .encode_samples(
                                    ImageRef::new(&source, stride, &info),
                                    &Metadata::default(),
                                )
                                .unwrap();
                            assert_eq!(
                                rows,
                                imij::decode(&ordinary).unwrap().1,
                                "ordinary/streaming pixels differ at {bits} bits, {color:?}/{entropy:?}, scale {scale}"
                            );
                            if channels == Channels::Rgba {
                                for y in 0..19usize {
                                    for x in 0..35usize {
                                        assert_eq!(
                                            &rows[y * info.row_bytes() + (x * 4 + 3) * bps..]
                                                [..bps],
                                            &source[y * stride + (x * 4 + 3) * bps..][..bps]
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(feature = "lab")]
#[test]
fn invalid_ac_scale_refuses_before_any_one_pass_source_request() {
    use imij::stream_encode::{BoundedLossySession, StreamEncodeError};
    let info = ImageInfo::new(35, 19, Channels::Rgb, SampleType::U8);
    for scale in [f32::NAN, f32::INFINITY, 0.0, -1.0, 0.3] {
        let cfg = ImiEncoderConfig::lossy(40.0)
            .with_effort(0.0)
            .with_ac_scale(scale)
            .tune_lossy(|t| t.aq_gamma = 0.0);
        assert!(matches!(
            BoundedLossySession::one_pass(
                info.clone(),
                &Metadata::default(),
                &cfg,
                1 << 20,
                32 << 20
            ),
            Err(StreamEncodeError::Codec(imij::Error::InvalidInput(_)))
        ));
    }
}
