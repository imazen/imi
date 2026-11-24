//! Unified sequence prediction and metadata use the same renderer for pages and animation.
use imij::item_metadata::{ItemMetadata, Value};
use imij::{
    Channels, Error, FrameConfig, FrameDecoder, FrameDuration, ImageInfo, ImageReference, Metadata,
    SampleType, SequenceEncoder, SequenceItem, SequenceKind,
};

fn config() -> FrameConfig {
    FrameConfig::new({
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.with_effort(4.0);
        cfg = cfg.with_threads(1);
        cfg
    })
}

#[test]
fn indexed_frame_prefix_requires_a_supported_outer_version() {
    // A bounded fRAM: five control bytes and one byte of nested data,
    // whose bytes are fetched separately from this prefix.
    let prefix = b"fRAM\x07\x00\x00\x00\x01\x00";
    for version in [2, 3, 4] {
        imij::check_indexed_frame_prefix(prefix, version, 1, 1, None).unwrap();
    }
    for version in [0, 1, 5, 255] {
        assert_eq!(
            imij::check_indexed_frame_prefix(prefix, version, 1, 1, None),
            Err(Error::Unsupported("unknown header version")),
        );
    }
}
fn random(info: &ImageInfo, seed: u32) -> Vec<u8> {
    let mut state = seed;
    let mut bytes: Vec<u8> = (0..info.checked_buffer_len().unwrap())
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect();
    if info.sample == SampleType::F32 {
        for (out, bits) in bytes.as_chunks_mut::<4>().0.iter_mut().zip([
            0x8000_0000u32, // negative zero
            0x7f80_0000,    // positive infinity
            0xff80_0000,    // negative infinity
            0x7fc0_1234,    // NaN payload
            1,              // subnormal
        ]) {
            out.copy_from_slice(&bits.to_ne_bytes());
        }
    }
    bytes
}
fn item_meta(index: usize) -> Metadata {
    let mut values = ItemMetadata::default();
    values.filename = Some(format!("variant-{index}.png"));
    values
        .attributes
        .insert("quality".into(), Value::Unsigned(index as u64));
    values
        .attributes
        .insert("codec".into(), Value::Text("imij".into()));
    let mut meta = Metadata {
        exif: Some(vec![index as u8]),
        orientation: (index % 8 + 1) as u8,
        ..Default::default()
    };
    values.write_metadata(&mut meta).unwrap();
    meta
}
fn document(kind: SequenceKind, info: &ImageInfo) -> (Vec<u8>, Vec<Vec<u8>>) {
    document_config(kind, info, config())
}
fn document_config(
    kind: SequenceKind,
    info: &ImageInfo,
    config: FrameConfig,
) -> (Vec<u8>, Vec<Vec<u8>>) {
    let source0 = random(info, 711);
    let source1 = random(info, 983);
    let mut changed = source0.clone();
    changed[13] ^= 3;
    let originals = vec![source0, source1, changed];
    let mut encoder = SequenceEncoder::new(kind, config);
    for (i, pixels) in originals.iter().enumerate() {
        let meta = item_meta(i);
        let mut item = SequenceItem::new(pixels, info.row_bytes(), info)
            .with_metadata(&meta)
            .with_duration(FrameDuration {
                num: i as u64 + 1,
                den: 73,
            });
        if i == 2 {
            item = item.with_reference(ImageReference {
                index: 0,
                pixels: &originals[0],
                stride: info.row_bytes(),
            });
        }
        encoder.push(item).unwrap();
    }
    (encoder.finish().unwrap(), originals)
}

#[test]
fn arbitrary_earlier_references_and_metadata_share_one_strided_renderer() {
    for kind in [SequenceKind::Animation, SequenceKind::Pages] {
        for (channels, sample) in [
            (Channels::Rgba, SampleType::U8),
            (Channels::GrayAlpha, SampleType::U16),
            (Channels::Rgb, SampleType::F32),
            (Channels::OklabAlpha, SampleType::F32),
            (Channels::Cmyk, SampleType::F32),
            #[cfg(feature = "extended")]
            (Channels::Rgba, SampleType::U32),
            #[cfg(feature = "extended")]
            (Channels::GrayAlpha, SampleType::I32),
        ] {
            let info = ImageInfo::new(129, 33, channels, sample);
            let (file, expected) = document(kind, &info);
            for checked in [false, true] {
                let file = if checked {
                    imij::add_pixel_checksum(file.clone()).unwrap()
                } else {
                    file.clone()
                };
                let mut decoder = FrameDecoder::new(&file).unwrap();
                for (i, image) in expected.iter().enumerate() {
                    let header = decoder.next_frame().unwrap().unwrap();
                    assert_eq!(
                        header.duration,
                        FrameDuration {
                            num: i as u64 + 1,
                            den: 73
                        }
                    );
                    let meta = decoder.frame_metadata().unwrap();
                    assert_eq!(meta.exif, Some(vec![i as u8]));
                    assert_eq!(meta.orientation, (i % 8 + 1) as u8);
                    assert_eq!(
                        ItemMetadata::from_metadata(meta).unwrap(),
                        ItemMetadata::from_metadata(&item_meta(i)).unwrap()
                    );
                    assert_eq!(decoder.reference_index(), (i == 2).then_some(0));
                    let stride = info.row_bytes() + 17;
                    let mut out = vec![0xa5; stride * info.height as usize];
                    if i == 2 {
                        let original_out = out.clone();
                        assert_eq!(
                            decoder.decode_into(&mut out, stride, 1),
                            Err(Error::Corrupt("reference raster checksum mismatch"))
                        );
                        assert_eq!(out, original_out);
                        let ref_stride = info.row_bytes() + 9;
                        let mut reference = vec![0x5a; ref_stride * info.height as usize];
                        for y in 0..info.height as usize {
                            reference[y * ref_stride..y * ref_stride + info.row_bytes()]
                                .copy_from_slice(
                                    &expected[0][y * info.row_bytes()..(y + 1) * info.row_bytes()],
                                );
                        }
                        decoder
                            .decode_into_with_reference(&reference, ref_stride, &mut out, stride, 4)
                            .unwrap();
                    } else {
                        decoder.decode_into(&mut out, stride, 2).unwrap();
                    }
                    for y in 0..info.height as usize {
                        assert_eq!(
                            &out[y * stride..y * stride + info.row_bytes()],
                            &image[y * info.row_bytes()..(y + 1) * info.row_bytes()]
                        );
                        assert!(
                            out[y * stride + info.row_bytes()..(y + 1) * stride]
                                .iter()
                                .all(|&b| b == 0xa5)
                        );
                    }
                    if checked {
                        assert_eq!(
                            decoder.pixel_checksum_status(),
                            imij::PixelChecksumStatus::Verified
                        );
                    }
                }
                assert!(decoder.next_frame().unwrap().is_none());
            }
        }
    }
}

#[test]
fn pages_default_to_keyframes_and_accept_different_formats() {
    let mut encoder = SequenceEncoder::new(SequenceKind::Pages, config());
    let descriptions = [
        ImageInfo::new(17, 9, Channels::Rgb, SampleType::U8),
        ImageInfo::new(3, 7, Channels::Gray, SampleType::F32),
    ];
    let pixels: Vec<_> = descriptions
        .iter()
        .enumerate()
        .map(|(i, info)| random(info, 77 + i as u32))
        .collect();
    for (info, data) in descriptions.iter().zip(&pixels) {
        encoder
            .push(SequenceItem::new(data, info.row_bytes(), info))
            .unwrap();
    }
    let file = encoder.finish().unwrap();
    let mut decoder = FrameDecoder::new(&file).unwrap();
    for (info, data) in descriptions.iter().zip(pixels) {
        decoder.next_frame().unwrap().unwrap();
        assert_eq!(decoder.reference_index(), None);
        assert_eq!(decoder.frame_info().unwrap(), *info);
        let mut out = vec![0; data.len()];
        decoder.decode_into(&mut out, info.row_bytes(), 1).unwrap();
        assert_eq!(out, data);
    }
}

#[test]
fn reference_identity_errors_are_transactional_and_interval_forces_intra() {
    let info = ImageInfo::new(129, 33, Channels::Rgb, SampleType::U8);
    let pixels = random(&info, 982);
    let mut cfg = config();
    cfg.keyframe_interval = 1;
    let mut encoder = SequenceEncoder::new(SequenceKind::Animation, cfg);
    encoder
        .push(SequenceItem::new(&pixels, info.row_bytes(), &info))
        .unwrap();
    let mut wrong = pixels.clone();
    wrong[0] ^= 1;
    for (index, reference) in [(1, &pixels), (0, &wrong)] {
        assert!(
            encoder
                .push(
                    SequenceItem::new(&pixels, info.row_bytes(), &info).with_reference(
                        ImageReference {
                            index,
                            pixels: reference,
                            stride: info.row_bytes()
                        }
                    )
                )
                .is_err()
        );
    }
    encoder
        .push(
            SequenceItem::new(&pixels, info.row_bytes(), &info).with_reference(ImageReference {
                index: 0,
                pixels: &pixels,
                stride: info.row_bytes(),
            }),
        )
        .unwrap();
    let file = encoder.finish().unwrap();
    let mut decoder = FrameDecoder::new(&file).unwrap();
    assert_eq!(decoder.frame_count(), 2);
    while decoder.next_frame().unwrap().is_some() {
        assert_eq!(decoder.reference_index(), None);
    }
}

#[test]
fn reconstructed_lossy_references_are_returned_for_caller_owned_caches() {
    let info = ImageInfo::new(129, 41, Channels::Rgb, SampleType::U8);
    let original = random(&info, 633);
    let mut lossy = imij::ImiEncoderConfig::lossy(60.0);
    lossy = lossy.with_threads(1);
    let mut encoder = SequenceEncoder::new(SequenceKind::Pages, FrameConfig::new(lossy));
    let reference = encoder
        .push(SequenceItem::new(&original, info.row_bytes(), &info))
        .unwrap();
    assert_ne!(reference, original);
    let second = encoder
        .push(
            SequenceItem::new(&reference, info.row_bytes(), &info).with_reference(ImageReference {
                index: 0,
                pixels: &reference,
                stride: info.row_bytes(),
            }),
        )
        .unwrap();
    let file = encoder.finish().unwrap();
    let mut decoder = FrameDecoder::new(&file).unwrap();
    let mut out = vec![0; reference.len()];
    decoder.next_frame().unwrap();
    decoder.decode_into(&mut out, info.row_bytes(), 1).unwrap();
    assert_eq!(out, reference);
    decoder.next_frame().unwrap();
    assert_eq!(decoder.reference_index(), Some(0));
    decoder.decode_into(&mut out, info.row_bytes(), 4).unwrap();
    assert_eq!(out, second);
    assert_eq!(second, reference);
}

#[test]
fn legacy_unchanged_frames_still_obey_keyframe_policy() {
    let info = ImageInfo::new(17, 13, Channels::Rgb, SampleType::U8);
    let image = random(&info, 388);
    for inter in [true, false] {
        let mut cfg = config();
        cfg.keyframe_interval = 1;
        cfg.inter = inter;
        let mut encoder = imij::FrameEncoder::animation(info.clone(), Metadata::default(), cfg);
        for _ in 0..3 {
            encoder
                .push_frame(&image, info.row_bytes(), FrameDuration::from_millis(40))
                .unwrap();
        }
        let file = encoder.finish().unwrap();
        let mut decoder = FrameDecoder::new(&file).unwrap();
        while let Some(header) = decoder.next_frame().unwrap() {
            assert!(!header.uses_previous);
            assert!(header.coding.is_some());
            let mut out = vec![0; image.len()];
            decoder.decode_into(&mut out, info.row_bytes(), 1).unwrap();
            assert_eq!(out, image);
        }
    }
}

#[test]
fn grouped_32_bit_references_preserve_raw_samples_across_threads() {
    for sample in [
        SampleType::F32,
        #[cfg(feature = "extended")]
        SampleType::U32,
        #[cfg(feature = "extended")]
        SampleType::I32,
    ] {
        for channels in [
            Channels::Gray,
            Channels::Rgba,
            Channels::Cmyk,
            Channels::OklabAlpha,
        ] {
            let info = ImageInfo::new(129, 33, channels, sample);
            for kind in [SequenceKind::Pages, SequenceKind::Animation] {
                let config = FrameConfig::new({
                    let mut cfg = imij::ImiEncoderConfig::default();
                    cfg = cfg.with_column_groups(Some(32));
                    cfg = cfg.tune_lossless(|t| {
                        t.stripe_rows = Some(4);
                    });
                    cfg = cfg.tune_lossless(|t| {
                        t.palette = false;
                    });
                    cfg = cfg.with_effort(4.0);
                    cfg = cfg.with_threads(1);
                    cfg
                });
                let (file, expected) = document_config(kind, &info, config);
                let file = imij::add_pixel_checksum(file).unwrap();
                for threads in [1, 2, 4, 8] {
                    let mut decoder = FrameDecoder::new(&file).unwrap();
                    for (index, image) in expected.iter().enumerate() {
                        decoder.next_frame().unwrap().unwrap();
                        assert_eq!(decoder.reference_index(), (index == 2).then_some(0));
                        let mut out = vec![0; image.len()];
                        if index == 2 {
                            decoder
                                .decode_into_with_reference(
                                    &expected[0],
                                    info.row_bytes(),
                                    &mut out,
                                    info.row_bytes(),
                                    threads,
                                )
                                .unwrap();
                        } else {
                            decoder
                                .decode_into(&mut out, info.row_bytes(), threads)
                                .unwrap();
                        }
                        assert_eq!(out, *image, "{sample:?} {channels:?} {kind:?} {threads}");
                    }
                    assert!(decoder.next_frame().unwrap().is_none());
                }
            }
        }
    }
}

#[test]
fn random_selection_preserves_reference_admission_metadata_and_checksums() {
    let info = ImageInfo::new(129, 33, Channels::Rgb, SampleType::U8);
    for kind in [SequenceKind::Animation, SequenceKind::Pages] {
        let (file, pixels) = document(kind, &info);
        let file = imij::add_pixel_checksum(file).unwrap();
        let mut borrowed = FrameDecoder::new(&file).unwrap();
        let mut owned = FrameDecoder::from_owned(file.clone(), &imij::Limits::DEFAULT).unwrap();
        for decoder in [&mut borrowed, &mut owned] {
            let location = decoder.frame_seek_info(2).unwrap();
            assert_eq!(location.reference_index, Some(0));
            assert!(location.reference_hash.is_some());
            let span = location.encoded.unwrap();
            assert!(span.start > 0 && span.end <= file.len() as u64);
            assert_eq!(
                &file[span.start as usize..span.start as usize + 4],
                &file[..4]
            );
            assert_eq!(decoder.seek_frame(2).unwrap().index, 2);
            assert_eq!(decoder.frame_metadata().unwrap().exif, Some(vec![2]));
            assert_eq!(
                decoder.pixel_checksum_status(),
                imij::PixelChecksumStatus::Pending
            );
            let stride = info.row_bytes() + 11;
            let mut out = vec![0xa5; stride * info.height as usize];
            let before = out.clone();
            assert_eq!(
                decoder.decode_into(&mut out, stride, 1),
                Err(Error::Corrupt("reference raster checksum mismatch"))
            );
            assert_eq!(out, before);
            let ref_stride = info.row_bytes() + 7;
            let mut reference = vec![0x5a; ref_stride * info.height as usize];
            for y in 0..info.height as usize {
                reference[y * ref_stride..y * ref_stride + info.row_bytes()]
                    .copy_from_slice(&pixels[0][y * info.row_bytes()..(y + 1) * info.row_bytes()]);
            }
            decoder
                .decode_into_with_reference(&reference, ref_stride, &mut out, stride, 4)
                .unwrap();
            for y in 0..info.height as usize {
                assert_eq!(
                    &out[y * stride..y * stride + info.row_bytes()],
                    &pixels[2][y * info.row_bytes()..(y + 1) * info.row_bytes()]
                );
                assert!(
                    out[y * stride + info.row_bytes()..(y + 1) * stride]
                        .iter()
                        .all(|&b| b == 0xa5)
                );
            }
            assert_eq!(
                decoder.pixel_checksum_status(),
                imij::PixelChecksumStatus::Verified
            );
            // Invalid ordinals and read-only lookup preserve the current selection.
            assert!(decoder.seek_frame(3).is_err());
            assert!(decoder.frame_seek_info(3).is_err());
            assert_eq!(
                decoder.frame_seek_info(0).unwrap().encoded,
                Some(0..file.len() as u64)
            );
            assert_eq!(decoder.frame_metadata().unwrap().exif, Some(vec![2]));
            for _ in 0..2 {
                assert_eq!(decoder.seek_frame(0).unwrap().index, 0);
                assert_eq!(decoder.reference_index(), None);
                assert_eq!(decoder.frame_metadata().unwrap().exif, Some(vec![0]));
                let mut out = vec![0; pixels[0].len()];
                decoder.decode_into(&mut out, info.row_bytes(), 1).unwrap();
                assert_eq!(out, pixels[0]);
                assert_eq!(
                    decoder.pixel_checksum_status(),
                    imij::PixelChecksumStatus::Verified
                );
                assert_eq!(decoder.next_frame().unwrap().unwrap().index, 1);
            }
        }
    }
}

#[test]
fn legacy_empty_selection_verifies_the_complete_supplied_canvas() {
    let info = ImageInfo::new(32, 17, Channels::Rgb, SampleType::U8);
    let first = random(&info, 77);
    let mut second = first.clone();
    second[100] ^= 1;
    let mut encoder = imij::FrameEncoder::animation(info.clone(), Metadata::default(), config());
    for pixels in [&first, &second, &second] {
        encoder
            .push_frame(pixels, info.row_bytes(), FrameDuration::from_millis(20))
            .unwrap();
    }
    let file = imij::add_pixel_checksum(encoder.finish().unwrap()).unwrap();
    let mut decoder = FrameDecoder::new(&file).unwrap();
    let location = decoder.frame_seek_info(2).unwrap();
    assert_eq!(location.encoded, None);
    assert_eq!(location.reference_index, Some(1));
    assert_eq!(decoder.seek_frame(2).unwrap().coding, None);
    let mut wrong = second.clone();
    wrong[0] ^= 1;
    assert!(
        decoder
            .decode_into(&mut wrong, info.row_bytes(), 1)
            .is_err()
    );
    decoder.seek_frame(2).unwrap();
    let mut correct = second;
    decoder
        .decode_into(&mut correct, info.row_bytes(), 1)
        .unwrap();
    assert_eq!(
        decoder.pixel_checksum_status(),
        imij::PixelChecksumStatus::Verified
    );
    decoder.seek_frame(0).unwrap();
    decoder
        .decode_into(&mut correct, info.row_bytes(), 1)
        .unwrap();
    assert_eq!(correct, first);
}

#[test]
fn page_selection_changes_dimensions_format_and_metadata() {
    let descriptions = [
        ImageInfo::new(17, 9, Channels::Rgb, SampleType::U8),
        ImageInfo::new(3, 7, Channels::Gray, SampleType::F32),
    ];
    let pixels: Vec<_> = descriptions
        .iter()
        .enumerate()
        .map(|(i, info)| random(info, i as u32 + 93))
        .collect();
    let mut encoder = SequenceEncoder::new(SequenceKind::Pages, config());
    for (i, (info, data)) in descriptions.iter().zip(&pixels).enumerate() {
        encoder
            .push(SequenceItem::new(data, info.row_bytes(), info).with_metadata(&item_meta(i)))
            .unwrap();
    }
    let file = encoder.finish().unwrap();
    let mut decoder = FrameDecoder::new(&file).unwrap();
    for index in [1, 0, 1, 0] {
        decoder.seek_frame(index).unwrap();
        assert_eq!(decoder.frame_info().unwrap(), descriptions[index]);
        assert_eq!(
            decoder.frame_metadata().unwrap().exif,
            Some(vec![index as u8])
        );
        let mut out = vec![0; pixels[index].len()];
        decoder
            .decode_into(&mut out, descriptions[index].row_bytes(), 1)
            .unwrap();
        assert_eq!(out, pixels[index]);
    }
}
