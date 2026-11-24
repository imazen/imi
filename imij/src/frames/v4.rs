//! Frame/nested consumers compare V4 with the existing V3 reconstruction oracle.
use super::*;
use crate::{Channels, ImageRef, ImiEncoderConfig, SampleType};
use alloc::{vec, vec::Vec};

pub(crate) fn check_single(file: &[u8], expected: &[u8], info: &ImageInfo) {
    for threads in [1, 2, 4, 8] {
        for owned in [false, true] {
            let mut decoder = if owned {
                FrameDecoder::from_owned(file.to_vec(), &crate::Limits::DEFAULT).unwrap()
            } else {
                FrameDecoder::new(file).unwrap()
            };
            assert_eq!(decoder.frame_count(), 1);
            assert_eq!(decoder.info(), info);
            let frame = decoder.next_frame().unwrap().unwrap();
            assert_eq!(frame.index, 0);
            let stride = info.row_bytes() + 17;
            let mut output = vec![0xA9; stride * info.height as usize];
            decoder.decode_into(&mut output, stride, threads).unwrap();
            for y in 0..info.height as usize {
                assert_eq!(
                    &output[y * stride..][..info.row_bytes()],
                    &expected[y * info.row_bytes()..][..info.row_bytes()]
                );
                assert!(
                    output[y * stride + info.row_bytes()..(y + 1) * stride]
                        .iter()
                        .all(|&b| b == 0xA9)
                );
            }
            assert!(decoder.next_frame().unwrap().is_none());
        }
    }
}

fn pixels(info: &ImageInfo) -> Vec<u8> {
    (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 73 + i / 7 * 41) as u8)
        .collect()
}
fn convert_nested(encoder: &mut FrameEncoder) {
    for payload in &mut encoder.frames {
        let nested = frame_file(payload, crate::parse::header::HEADER_VERSION)
            .unwrap()
            .unwrap();
        let start = nested.as_ptr() as usize - payload.as_ptr() as usize;
        let upgraded = crate::v4::upgrade_image(nested, &crate::UpgradeLimits::default()).unwrap();
        let mut next = payload[..start].to_vec();
        next.extend_from_slice(&upgraded);
        *payload = next;
    }
}
fn frames(file: &[u8]) -> Vec<(FrameHeader, ImageInfo, Vec<u8>)> {
    let mut decoder = FrameDecoder::new(file).unwrap();
    let mut output = Vec::new();
    let mut values = Vec::new();
    while let Some(frame) = decoder.next_frame().unwrap() {
        let info = if decoder.kind() == FrameKind::Animation {
            decoder.info().clone()
        } else {
            decoder.frame_info().unwrap()
        };
        let stride = info.row_bytes();
        output.resize(info.checked_buffer_len().unwrap(), 0);
        decoder.decode_into(&mut output, stride, 4).unwrap();
        values.push((frame, info, output.clone()));
    }
    values
}

#[test]
fn still_records_preserve_indexes_pixels_checksums_and_owned_thread_transfer() {
    let info = ImageInfo::new(41, 77, Channels::Rgba, SampleType::U8);
    let source = pixels(&info);
    for coding in [
        ImiEncoderConfig::lossless(),
        ImiEncoderConfig::quick(),
        ImiEncoderConfig::lossy(73.0),
    ] {
        let native = coding
            .with_column_groups(Some(16))
            .with_checksum(true)
            .encode_legacy_samples(ImageRef::packed(&source, &info), &Metadata::default())
            .unwrap();
        let native = crate::pixelsum::add_pixel_checksum(native).unwrap();
        let expected = crate::decode(&native).unwrap().1;
        let base = crate::v4::upgrade_image(&native, &crate::UpgradeLimits::default()).unwrap();
        for placement in [
            crate::v4::DirectoryPlacement::None,
            crate::v4::DirectoryPlacement::Front,
            crate::v4::DirectoryPlacement::Tail,
        ] {
            for open in [false, true] {
                let file = crate::v4::file_tests::rewrite_placement(&base, placement, open);
                check_single(&file, &expected, &info);
                let decoder = FrameDecoder::from_owned(file, &crate::Limits::DEFAULT).unwrap();
                let got = std::thread::spawn(move || {
                    let mut decoder = decoder;
                    decoder.next_frame().unwrap().unwrap();
                    let mut output = vec![0; decoder.info().checked_buffer_len().unwrap()];
                    let stride = decoder.info().row_bytes();
                    decoder.decode_into(&mut output, stride, 4).unwrap();
                    assert_eq!(
                        decoder.pixel_checksum_status(),
                        crate::PixelChecksumStatus::Verified
                    );
                    output
                })
                .join()
                .unwrap();
                assert_eq!(got, expected);
            }
        }
    }
}

#[test]
fn mixed_v3_v4_pages_and_reference_frames_keep_the_native_sequence_pixels() {
    for coding in [
        ImiEncoderConfig::lossless(),
        ImiEncoderConfig::quick(),
        ImiEncoderConfig::lossy(73.0),
    ] {
        let config = FrameConfig::new(coding.with_column_groups(Some(32)));
        let first = ImageInfo::new(97, 67, Channels::Rgba, SampleType::U8);
        let second = ImageInfo::new(71, 49, Channels::GrayAlpha, SampleType::U16);
        let mut pages = FrameEncoder::mixed_pages(config.clone());
        pages
            .push_page(&pixels(&first), first.row_bytes(), &first)
            .unwrap();
        pages
            .push_page_with(
                &pixels(&second),
                second.row_bytes(),
                &second,
                &Metadata::default(),
                &ImiEncoderConfig::lossless(),
            )
            .unwrap();
        let native = pages.clone_for_test();
        let expected = frames(&native);
        convert_nested(&mut pages);
        assert_eq!(frames(&pages.finish().unwrap()), expected);

        let mut hdr = ImageInfo::new(49, 37, Channels::Rgba, SampleType::U16);
        hdr.cicp.color_primaries = 9;
        hdr.cicp.transfer_characteristics = 16;
        let mut uniform = FrameEncoder::pages(hdr.clone(), Metadata::default(), config.clone());
        uniform
            .push_page(&pixels(&hdr), hdr.row_bytes(), &hdr)
            .unwrap();
        let mut small = hdr.clone();
        small.width = 41;
        small.height = 29;
        uniform
            .push_page(&pixels(&small), small.row_bytes(), &small)
            .unwrap();
        let expected_uniform = frames(&uniform.clone_for_test());
        convert_nested(&mut uniform);
        assert_eq!(frames(&uniform.finish().unwrap()), expected_uniform);

        let info = ImageInfo::new(257, 129, Channels::Rgba, SampleType::U8);
        let source = pixels(&info);
        let mut changed = source.clone();
        for (x, y) in [(1, 1), (255, 127), (117, 65)] {
            let at = (y * 257 + x) * 4;
            changed[at] ^= 37;
        }
        let mut animation = FrameEncoder::animation(info.clone(), Metadata::default(), config);
        animation
            .push_frame(&source, info.row_bytes(), FrameDuration::from_millis(33))
            .unwrap();
        animation
            .push_frame(&changed, info.row_bytes(), FrameDuration::from_millis(33))
            .unwrap();
        let native = animation.clone_for_test();
        let expected = frames(&native);
        assert_eq!(expected.len(), 2);
        assert!(
            expected[1].0.uses_previous,
            "fixture must select a reference"
        );
        assert!(
            frame_file(&animation.frames[0], crate::parse::header::HEADER_VERSION)
                .unwrap()
                .is_some(),
            "fixture must carry a nonempty referenced image"
        );
        convert_nested(&mut animation);
        assert_eq!(frames(&animation.finish().unwrap()), expected);
    }
}

#[test]
fn admitted_frame_reconstruction_preserves_unit_budget_and_failed_work() {
    let info = ImageInfo::new(41, 77, Channels::Rgba, SampleType::U8);
    let source = pixels(&info);
    let native = ImiEncoderConfig::lossless()
        .with_column_groups(Some(16))
        .encode_legacy_samples(ImageRef::packed(&source, &info), &Metadata::default())
        .unwrap();
    let file = crate::v4::upgrade_image(&native, &crate::UpgradeLimits::default()).unwrap();
    let session = crate::decode_resources::DecodeSession::new(
        crate::decode_resources::DecodePolicy::default().with_max_units(0),
    );
    session.v4_input(file.len()).unwrap();
    let mut decoder =
        FrameDecoder::from_owned_admitted(file, &crate::Limits::DEFAULT, &session.admission())
            .unwrap();
    decoder
        .next_frame_admitted(&session.admission())
        .unwrap()
        .unwrap();
    let before = session.usage();
    let mut output = vec![0xC7; source.len()];
    assert!(matches!(
        decoder.decode_into_admitted(&mut output, info.row_bytes(), &session.admission(), &|| Ok(
            ()
        )),
        Err(Error::TooLarge)
    ));
    assert!(output.iter().all(|&b| b == 0xC7));
    assert!(session.usage().work_units > before.work_units);
    assert_eq!(session.usage().input_bytes, before.input_bytes);
}

impl FrameEncoder {
    // Exercise exactly the production finish path while preserving the builder
    // for the nested-version replacement. No fixture bytes come from a decoder.
    fn clone_for_test(&self) -> Vec<u8> {
        Self {
            canvas: self.canvas.clone(),
            meta: self.meta.clone(),
            kind: self.kind,
            cfg: self.cfg.clone(),
            first: self.first.clone(),
            frames: self.frames.clone(),
            prev_src: self.prev_src.clone(),
            prev_rec: self.prev_rec.clone(),
            since_key: self.since_key,
            later_features: self.later_features,
        }
        .finish()
        .unwrap()
    }
}

#[test]
fn nested_output_limits_do_not_replace_the_separate_working_budget() {
    let info = ImageInfo::new(41, 77, Channels::Rgba, SampleType::U8);
    let source = pixels(&info);
    let native = ImiEncoderConfig::lossless()
        .with_column_groups(Some(16))
        .encode_legacy_samples(ImageRef::packed(&source, &info), &Metadata::default())
        .unwrap();
    let base = crate::v4::upgrade_image(&native, &crate::UpgradeLimits::default()).unwrap();
    let file =
        crate::v4::file_tests::rewrite_placement(&base, crate::v4::DirectoryPlacement::None, true);
    let limits = crate::Limits {
        max_pixels: u64::from(info.width) * u64::from(info.height),
        max_bytes: source.len() as u64,
        verify_checksum: true,
    };
    let session = crate::decode_resources::DecodeSession::new(
        crate::decode_resources::DecodePolicy::default()
            .with_output_limits(limits)
            .with_max_working_bytes(8 << 20),
    );
    session.v4_input(file.len()).unwrap();
    let mut decoder =
        FrameDecoder::from_owned_admitted(file, &limits, &session.admission()).unwrap();
    decoder
        .next_frame_admitted(&session.admission())
        .unwrap()
        .unwrap();
    let mut output = vec![0; source.len()];
    decoder
        .decode_into_admitted(&mut output, info.row_bytes(), &session.admission(), &|| {
            Ok(())
        })
        .unwrap();
    assert_eq!(output, source);
}

fn check_migrated_collection(native: &[u8]) {
    let expected = frames(native);
    let migrated = crate::v4::upgrade_image(native, &crate::UpgradeLimits::default()).unwrap();
    assert_eq!(crate::header(&migrated).unwrap().version, 4);
    assert_eq!(frames(&migrated), expected);
    assert_eq!(
        crate::v4::upgrade_image(&migrated, &crate::UpgradeLimits::default()).unwrap(),
        migrated,
        "a fully migrated collection is byte-stable"
    );
    let parsed = crate::v4::file::parse_file(
        &migrated,
        &crate::decode_resources::DecodeSession::new(Default::default()),
    )
    .unwrap();
    for frame in &parsed.frames {
        if let Some(nested) = frame_file(frame, 4).unwrap() {
            assert_eq!(crate::header(nested).unwrap().version, 4);
        }
    }
    let primary = crate::decode(native).unwrap();
    assert_eq!(crate::decode(&migrated).unwrap(), primary);
    crate::v4::region_tests::check(&migrated, &primary.1, &primary.0);
    let mut owned = FrameDecoder::from_owned(migrated, &crate::Limits::DEFAULT).unwrap();
    let mut out = Vec::new();
    for (frame, info, pixels) in expected {
        assert_eq!(owned.next_frame().unwrap().unwrap(), frame);
        out.resize(pixels.len(), 0);
        owned.decode_into(&mut out, info.row_bytes(), 4).unwrap();
        assert!(out == pixels, "owned frame {} differs", frame.index);
    }
    assert!(owned.next_frame().unwrap().is_none());
}

#[test]
fn recursive_collection_migration_preserves_references_pages_and_primary_sparse_reads() {
    for checksum in [false, true] {
        for coding in [
            ImiEncoderConfig::lossless(),
            ImiEncoderConfig::quick(),
            ImiEncoderConfig::lossy(73.0),
        ] {
            let config =
                FrameConfig::new(coding.with_checksum(checksum).with_column_groups(Some(32)));
            let info = ImageInfo::new(97, 67, Channels::Rgba, SampleType::U8);
            let source = pixels(&info);
            let mut mixed = FrameEncoder::mixed_pages(config.clone());
            mixed.push_page(&source, info.row_bytes(), &info).unwrap();
            let other = ImageInfo::new(53, 41, Channels::GrayAlpha, SampleType::U16);
            mixed
                .push_page_with(
                    &pixels(&other),
                    other.row_bytes(),
                    &other,
                    &Metadata::default(),
                    &ImiEncoderConfig::lossless().with_checksum(checksum),
                )
                .unwrap();
            let native = mixed.clone_for_test();
            check_migrated_collection(&native);
            let limits = crate::UpgradeLimits {
                max_depth: 0,
                ..Default::default()
            };
            assert!(matches!(
                crate::v4::upgrade_image(&native, &limits),
                Err(Error::TooLarge)
            ));
            // Also cover an outer V3 file containing a V4 child.
            convert_nested(&mut mixed);
            check_migrated_collection(&mixed.finish().unwrap());

            let mut hdr = ImageInfo::new(49, 37, Channels::Rgba, SampleType::U16);
            hdr.cicp.color_primaries = 9;
            hdr.cicp.transfer_characteristics = 16;
            let mut pages = FrameEncoder::pages(hdr.clone(), Metadata::default(), config.clone());
            pages
                .push_page(&pixels(&hdr), hdr.row_bytes(), &hdr)
                .unwrap();
            let mut small = hdr.clone();
            small.width = 41;
            small.height = 29;
            pages
                .push_page(&pixels(&small), small.row_bytes(), &small)
                .unwrap();
            check_migrated_collection(&pages.finish().unwrap());

            let mut animation = FrameEncoder::animation(info.clone(), Metadata::default(), config);
            animation
                .push_frame(&source, info.row_bytes(), FrameDuration::from_millis(33))
                .unwrap();
            let mut changed = source;
            changed[info.row_bytes() * 31 + 47 * 4] ^= 37;
            animation
                .push_frame(&changed, info.row_bytes(), FrameDuration::from_millis(41))
                .unwrap();
            check_migrated_collection(&animation.finish().unwrap());
        }
    }
}

#[test]
fn reference_rows_match_legacy_full_raster_and_preserve_read_failures() {
    super::row_tests::check_inter_reference_rows(true);
}

#[test]
fn reference_input_and_output_suspend_and_move_to_another_thread() {
    super::row_tests::suspended_v4_reference();
}
