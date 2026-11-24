//! Source code-grid declarations, distinct from reversible-transform headroom.
use imij::{Channels, Cicp, ImageInfo, LossyColor, Metadata, SampleType};

fn max(bits: u8) -> u16 {
    ((1u32 << bits) - 1) as u16
}
fn words(bytes: &[u8]) -> Vec<u16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_ne_bytes([b[0], b[1]]))
        .collect()
}

#[test]
fn all_unsigned_depths_preserve_ranges_and_normalized_white() {
    for sample in [SampleType::U8, SampleType::U16] {
        let depth = sample.bytes() * 8;
        for bits in 1..=depth as u8 {
            let info = ImageInfo::new(17, 9, Channels::Rgb, sample)
                .with_significant_bits(&[bits; 3])
                .unwrap();
            for color in [LossyColor::YCoCg, LossyColor::Xyb] {
                let mut cfg = imij::ImiEncoderConfig::lossy(116.0);
                cfg = cfg.tune_lossy(|t| {
                    t.color = color;
                });
                cfg = cfg.tune_lossy(|t| {
                    t.compact_streams = true;
                });
                // Flat white must remain white independent of source storage width.
                let pixels: Vec<u8> = (0..17 * 9 * 3)
                    .flat_map(|_| {
                        if sample == SampleType::U8 {
                            vec![max(bits) as u8]
                        } else {
                            max(bits).to_ne_bytes().to_vec()
                        }
                    })
                    .collect();
                let file = cfg
                    .encode_samples(
                        imij::ImageRef::new(&pixels, info.row_bytes(), &info),
                        &Metadata::default(),
                    )
                    .unwrap();
                let (decoded_info, decoded) = imij::decode(&file).unwrap();
                assert_eq!(decoded_info, info);
                let values = if sample == SampleType::U8 {
                    decoded.iter().map(|&v| v as u16).collect()
                } else {
                    words(&decoded)
                };
                let tolerance = (max(bits) as u32 / 1000 + 1) as u16;
                assert!(
                    values
                        .iter()
                        .all(|&v| v <= max(bits) && max(bits) - v <= tolerance),
                    "{sample:?} {bits} {color:?}: {:?}",
                    &values[..3]
                );
            }
        }
    }
}

#[test]
fn mixed_ranges_hdr_alpha_strides_threads_and_streaming() {
    for bits in [[9, 10, 13, 1], [11, 12, 16, 8], [16, 8, 7, 12]] {
        for cicp in [Cicp::SRGB, Cicp::BT2100_PQ, Cicp::BT2100_HLG] {
            let mut info = ImageInfo::new(19, 17, Channels::Rgba, SampleType::U16)
                .with_significant_bits(&bits)
                .unwrap();
            info.cicp = cicp;
            let stride = info.row_bytes() + 7;
            let mut input = vec![0xab; stride * 17];
            for y in 0..17 {
                for x in 0..19 {
                    for (c, &n) in bits.iter().enumerate() {
                        let value = if c == 3 {
                            ((x + y) as u16) & max(n)
                        } else {
                            (max(n) as u32 * ((x + y) % 17) as u32 / 16) as u16
                        };
                        input[y * stride + (x * 4 + c) * 2..][..2]
                            .copy_from_slice(&value.to_ne_bytes());
                    }
                }
            }
            for color in [LossyColor::Xyb, LossyColor::YCoCg] {
                let mut cfg = imij::ImiEncoderConfig::lossy(100.0);
                cfg = cfg.tune_lossy(|t| {
                    t.color = color;
                });
                cfg = cfg.tune_lossy(|t| {
                    t.compact_streams = true;
                });
                cfg = cfg.with_column_groups(Some(16));
                let file = cfg
                    .encode_samples(
                        imij::ImageRef::new(&input, stride, &info),
                        &Metadata::default(),
                    )
                    .unwrap();
                cfg = cfg.with_threads(4);
                assert_eq!(
                    file,
                    cfg.encode_samples(
                        imij::ImageRef::new(&input, stride, &info),
                        &Metadata::default()
                    )
                    .unwrap()
                );
                let (actual_info, decoded) = imij::decode(&file).unwrap();
                assert_eq!(actual_info, info);
                for (i, value) in words(&decoded).into_iter().enumerate() {
                    assert!(value <= max(bits[i % 4]));
                    if i % 4 == 3 {
                        let pixel = i / 4;
                        assert_eq!(value, ((pixel % 19 + pixel / 19) as u16) & max(bits[3]));
                    }
                }
                let mut out = vec![0xcd; stride * 17];
                imij::decode_into_threads(&file, &mut out, stride, 4).unwrap();
                let mut stream = imij::StreamDecoder::new(&file).unwrap();
                let mut row = vec![0; info.row_bytes()];
                for y in 0..17 {
                    assert!(stream.next_row(&mut row).unwrap());
                    assert_eq!(row, decoded[y * info.row_bytes()..][..info.row_bytes()]);
                    assert_eq!(&out[y * stride..][..info.row_bytes()], &row);
                    assert!(
                        out[y * stride + info.row_bytes()..(y + 1) * stride]
                            .iter()
                            .all(|&v| v == 0xcd)
                    );
                }
            }
        }
    }
}

#[test]
fn declaration_never_silently_discards_storage_bits() {
    let info = ImageInfo::new(8, 8, Channels::Gray, SampleType::U16)
        .with_significant_bits(&[10])
        .unwrap();
    let pixels: Vec<u8> = (0..64u16)
        .flat_map(|v| (v | 0xf000).to_ne_bytes())
        .collect();
    let file = imij::ImiEncoderConfig::default()
        .encode_samples(
            imij::ImageRef::new(&pixels, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    assert_eq!(imij::decode(&file).unwrap(), (info.clone(), pixels.clone()));
    assert!(
        imij::ImiEncoderConfig::lossy(90.0)
            .encode_samples(
                imij::ImageRef::new(&pixels, info.row_bytes(), &info),
                &Metadata::default()
            )
            .is_err()
    );
    for bits in [&[0][..], &[17][..], &[10, 10][..]] {
        assert!(info.clone().with_significant_bits(bits).is_err());
    }
    assert!(
        ImageInfo::new(8, 8, Channels::Gray, SampleType::F16)
            .with_significant_bits(&[10])
            .is_err()
    );
    assert!(
        ImageInfo::new(8, 8, Channels::Gray, SampleType::F32)
            .with_significant_bits(&[24])
            .is_err()
    );
    // Header: version 2, present = sample | channels | significant bits
    // (0x8a, a two-byte varint), U16, Gray, then the one significant-bit
    // byte. Reject malformed widths before any image allocation or nested
    // entropy processing.
    assert!(file[8] < 128);
    assert_eq!(
        &file[9..15],
        &[
            4,
            0x8a,
            0x01,
            SampleType::U16 as u8,
            Channels::Gray as u8,
            10
        ]
    );
    for bad_bits in [0, 17, 255] {
        let mut corrupt = file.clone();
        corrupt[14] = bad_bits;
        assert!(imij::header(&corrupt).is_err());
        assert!(imij::decode(&corrupt).is_err());
    }
}

#[test]
fn full_width_declarations_preserve_existing_bytes() {
    for sample in [SampleType::U8, SampleType::U16] {
        let info = ImageInfo::new(9, 7, Channels::Rgb, sample);
        let explicit = info
            .clone()
            .with_significant_bits(&[sample.bytes() as u8 * 8; 3])
            .unwrap();
        assert_eq!(info, explicit);
        let pixels = vec![7; info.checked_buffer_len().unwrap()];
        for compact in [false, true] {
            let cfg =
                imij::ImiEncoderConfig::lossy(80.0).tune_lossy(|t| t.compact_streams = compact);
            assert_eq!(
                cfg.encode_samples(
                    imij::ImageRef::new(&pixels, info.row_bytes(), &info),
                    &Metadata::default()
                )
                .unwrap(),
                cfg.encode_samples(
                    imij::ImageRef::new(&pixels, info.row_bytes(), &explicit),
                    &Metadata::default()
                )
                .unwrap()
            );
        }
    }
}

#[test]
fn forged_float_precision_and_unknown_custom_transfer_are_rejected() {
    for (sample, depth) in [(SampleType::F16, 16u8), (SampleType::F32, 32u8)] {
        // Complete header: version 2, present = sample | channels | bits,
        // the sample type, Gray, its bit count, 1x1, default lossless. No
        // payload is needed to prove header validation precedes allocation.
        let mut file = b"IMIJ\r\n\x87\n".to_vec();
        file.extend_from_slice(&[
            8,
            2,
            0x8a,
            0x01,
            sample as u8,
            Channels::Gray as u8,
            depth,
            1,
            1,
        ]);
        assert!(imij::header(&file).is_ok());
        for bits in [1, depth - 1] {
            file[14] = bits;
            assert!(matches!(imij::header(&file), Err(imij::Error::Corrupt(_))));
            assert!(imij::decode(&file).is_err());
        }
    }
    for sample in [SampleType::U8, SampleType::U16] {
        let mut info = ImageInfo::new(8, 8, Channels::Rgb, sample)
            .with_significant_bits(&[7; 3])
            .unwrap();
        info.cicp.transfer_characteristics = 255;
        let pixels = vec![0; info.checked_buffer_len().unwrap()];
        assert!(matches!(
            imij::ImiEncoderConfig::lossy(90.0).encode_samples(
                imij::ImageRef::new(&pixels, info.row_bytes(), &info),
                &Metadata::default()
            ),
            Err(imij::Error::Unsupported(_))
        ));
        let mut cfg = imij::ImiEncoderConfig::lossy(90.0);
        cfg = cfg.tune_lossy(|t| {
            t.color = LossyColor::YCoCg;
        });
        let file = cfg
            .encode_samples(
                imij::ImageRef::new(&pixels, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        assert_eq!(imij::decode(&file).unwrap().1, pixels);
    }
}

#[test]
fn gray_alpha_ranges_and_precision_aware_pages_and_animation() {
    use imij::{FrameConfig, FrameDecoder, FrameDuration, FrameEncoder};
    let info = ImageInfo::new(19, 17, Channels::GrayAlpha, SampleType::U16)
        .with_significant_bits(&[11, 1])
        .unwrap();
    let original: Vec<u8> = (0..19 * 17)
        .flat_map(|i| {
            [((i * 31) % 2048) as u16, (i % 2) as u16]
                .into_iter()
                .flat_map(u16::to_ne_bytes)
        })
        .collect();
    for color in [LossyColor::Xyb, LossyColor::YCoCg] {
        let mut cfg = imij::ImiEncoderConfig::lossy(100.0);
        cfg = cfg.tune_lossy(|t| {
            t.color = color;
        });
        let file = cfg
            .encode_samples(
                imij::ImageRef::new(&original, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        let (got, pixels) = imij::decode(&file).unwrap();
        assert_eq!(got, info);
        for (source, decoded) in original
            .as_chunks::<4>()
            .0
            .iter()
            .zip(pixels.as_chunks::<4>().0)
        {
            assert_eq!(&source[2..], &decoded[2..]);
            assert!(u16::from_ne_bytes([decoded[0], decoded[1]]) <= 2047);
        }
    }
    let coding = FrameConfig::new(imij::ImiEncoderConfig::default());
    let mut pages = FrameEncoder::pages(info.clone(), Metadata::default(), coding.clone());
    pages.push_page(&original, info.row_bytes(), &info).unwrap();
    let wrong = info.clone().with_significant_bits(&[10, 1]).unwrap();
    assert!(
        pages
            .push_page(&original, info.row_bytes(), &wrong)
            .is_err()
    );
    pages.push_page(&original, info.row_bytes(), &info).unwrap();
    let page_file = pages.finish().unwrap();
    let mut reader = FrameDecoder::new(&page_file).unwrap();
    for _ in 0..2 {
        assert!(reader.next_frame().unwrap().is_some());
        let mut out = vec![0; original.len()];
        reader.decode_into(&mut out, info.row_bytes(), 4).unwrap();
        assert_eq!(out, original);
    }
    let mut forged = page_file.clone();
    let signature = &page_file[..8];
    let nested = page_file.windows(8).rposition(|v| v == signature).unwrap();
    assert!(nested > 0);
    // Nested page header: version 4, present 0x8a (sample, channels, bits),
    // U16, GrayAlpha, bits 11, 1.
    assert!(page_file[nested + 8] < 128);
    assert_eq!(
        &page_file[nested + 9..nested + 16],
        &[
            4,
            0x8a,
            0x01,
            SampleType::U16 as u8,
            Channels::GrayAlpha as u8,
            11,
            1
        ]
    );
    forged[nested + 14] = 10;
    let mut reader = FrameDecoder::new(&forged).unwrap();
    assert!(reader.next_frame().unwrap().is_some());
    assert!(reader.next_frame().is_err());

    let mut animation = FrameEncoder::animation(info.clone(), Metadata::default(), coding);
    let mut changed = original.clone();
    changed[(8 * 19 + 8) * 4..][..2].copy_from_slice(&1234u16.to_ne_bytes());
    let inputs = [&original, &original, &changed, &changed];
    for input in inputs {
        animation
            .push_frame(input, info.row_bytes(), FrameDuration { num: 1, den: 30 })
            .unwrap();
    }
    let file = animation.finish().unwrap();
    let mut reader = FrameDecoder::new(&file).unwrap();
    assert_eq!(reader.info(), &info);
    let mut canvas = vec![0; original.len()];
    for expected in inputs {
        assert!(reader.next_frame().unwrap().is_some());
        reader
            .decode_into(&mut canvas, info.row_bytes(), 4)
            .unwrap();
        assert_eq!(&canvas, expected);
    }
}

#[test]
fn lossy_precision_animation_preserves_unchanged_blocks_and_pool_parity() {
    use imij::{FrameConfig, FrameDecoder, FrameDuration, FrameEncoder};
    let mut info = ImageInfo::new(40, 24, Channels::Rgb, SampleType::U16)
        .with_significant_bits(&[9, 11, 13])
        .unwrap();
    info.cicp = Cicp::BT2100_PQ;
    let original: Vec<u8> = (0..40 * 24)
        .flat_map(|i| {
            [
                ((i * 7) % 512) as u16,
                ((i * 11) % 2048) as u16,
                ((i * 31) % 8192) as u16,
            ]
            .into_iter()
            .flat_map(u16::to_ne_bytes)
        })
        .collect();
    let mut changed = original.clone();
    // Two changed blocks surround a source-identical block inside the crop.
    for x in [8, 24] {
        let at = (8 * 40 + x) * 6;
        changed[at..at + 6].copy_from_slice(
            &[511u16, 2047, 8191]
                .into_iter()
                .flat_map(u16::to_ne_bytes)
                .collect::<Vec<_>>(),
        );
    }
    for color in [LossyColor::Xyb, LossyColor::YCoCg] {
        let mut cfg = imij::ImiEncoderConfig::lossy(90.0);
        cfg = cfg.tune_lossy(|t| {
            t.color = color;
        });
        let mut encoder =
            FrameEncoder::animation(info.clone(), Metadata::default(), FrameConfig::new(cfg));
        for source in [&original, &changed, &changed] {
            encoder
                .push_frame(source, info.row_bytes(), FrameDuration::from_millis(40))
                .unwrap();
        }
        let file = encoder.finish().unwrap();
        let mut serial = FrameDecoder::new(&file).unwrap();
        let mut parallel = FrameDecoder::new(&file).unwrap();
        let mut a = vec![0; original.len()];
        let mut b = a.clone();
        let mut previous = a.clone();
        for frame in 0..3 {
            assert!(serial.next_frame().unwrap().is_some());
            assert!(parallel.next_frame().unwrap().is_some());
            serial.decode_into(&mut a, info.row_bytes(), 1).unwrap();
            parallel.decode_into(&mut b, info.row_bytes(), 4).unwrap();
            assert_eq!(a, b);
            for (i, pixel) in a.as_chunks::<6>().0.iter().enumerate() {
                let values: [u16; 3] =
                    core::array::from_fn(|c| u16::from_ne_bytes([pixel[2 * c], pixel[2 * c + 1]]));
                assert!(values[0] <= 511 && values[1] <= 2047 && values[2] <= 8191);
                if frame == 1 {
                    let (x, y) = (i % 40, i / 40);
                    if !((8..16).contains(&y) && ((8..16).contains(&x) || (24..32).contains(&x))) {
                        assert_eq!(pixel, &previous[i * 6..i * 6 + 6]);
                    }
                }
            }
            if frame == 2 {
                assert_eq!(a, previous);
            }
            previous.copy_from_slice(&a);
        }
    }
}
