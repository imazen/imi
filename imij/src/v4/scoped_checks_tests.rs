use super::*;
use crate::decode_resources::{DecodePolicy, DecodeSession};
use crate::{
    Channels, Error, HashAlgorithm, ImageInfo, ImiEncoderConfig, Metadata, RowGroups, SampleType,
};
use alloc::{vec, vec::Vec};

fn spatial(mode: ObjectIntegrity) -> SpatialConfig {
    SpatialConfig {
        tile_columns: 2,
        tile_rows: 2,
        order: TileOrder::Morton,
        integrity: mode,
        limits: TileLimits {
            max_object_bytes: 1 << 20,
            max_tile_bytes: 8 << 20,
        },
    }
}
fn manifest(flags: u8) -> Vec<u8> {
    let both = [
        CheckDescriptor::xxh3(HashAlgorithm::Xxh3_64),
        CheckDescriptor::xxh3(HashAlgorithm::Xxh3_128),
    ];
    let mut bytes = Vec::new();
    Checks::append(
        &mut bytes,
        core::array::from_fn(|i| {
            if flags & (1 << i) != 0 {
                &both[..]
            } else {
                &[]
            }
        }),
        CheckLimits::default(),
    )
    .unwrap();
    bytes
}
fn fixture(
    sample: SampleType,
    quick: bool,
    flags: u8,
    mode: ObjectIntegrity,
) -> (ImageInfo, Vec<u8>, usize, Vec<u8>) {
    let info = ImageInfo::new(41, 31, Channels::Rgba, sample);
    let stride = info.row_bytes() + 13;
    let pixels: Vec<_> = (0..info.checked_stride_len(stride).unwrap())
        .map(|i| (i * 73 + i / 7 * 41) as u8)
        .collect();
    let cfg = if quick {
        ImiEncoderConfig::quick()
    } else {
        ImiEncoderConfig::lossless()
    }
    .with_row_groups(RowGroups::new(9, 7, 8 << 20));
    let bytes = manifest(flags);
    let (checks, _) = Checks::read(&bytes, CheckLimits::default()).unwrap();
    let encoded = encode_numeric_checked(
        info.clone(),
        &pixels,
        stride,
        &Metadata::default(),
        &cfg,
        spatial(mode),
        checks,
    )
    .unwrap();
    if flags == 0 {
        assert_eq!(
            encoded,
            encode_numeric(
                info.clone(),
                &pixels,
                stride,
                &Metadata::default(),
                &cfg,
                spatial(mode)
            )
            .unwrap()
        );
    }
    (info, pixels, stride, encoded)
}

#[test]
fn scoped_numeric_files_all_scopes_storage_bits_strides_and_worker_counts() {
    for sample in [
        SampleType::U8,
        SampleType::U16,
        SampleType::U32,
        SampleType::U64,
        SampleType::I8,
        SampleType::I16,
        SampleType::I32,
        SampleType::I64,
        SampleType::F16,
        SampleType::F32,
        SampleType::F64,
    ] {
        for quick in [false, true] {
            for flags in 0..8 {
                let (info, pixels, stride, encoded) =
                    fixture(sample, quick, flags, ObjectIntegrity::ObjectsAndTiles);
                for threads in [1, 2, 4, 8] {
                    let out_stride = info.row_bytes() + 17;
                    let mut output = vec![0xa7; out_stride * info.height as usize];
                    crate::decode_into_threads(&encoded, &mut output, out_stride, threads).unwrap();
                    for y in 0..info.height as usize {
                        assert_eq!(
                            &output[y * out_stride..][..info.row_bytes()],
                            &pixels[y * stride..][..info.row_bytes()]
                        );
                        assert!(
                            output[y * out_stride + info.row_bytes()..(y + 1) * out_stride]
                                .iter()
                                .all(|&b| b == 0xa7)
                        );
                    }
                }
            }
        }
    }
}

fn corrupt_tag(encoded: &mut [u8], scope: CheckScope) {
    let session = DecodeSession::new(DecodePolicy::default());
    let file = super::file::parse_file(encoded, &session).unwrap();
    let (mut decoder, start, _) = NumericGroupDecoder::with_directory(
        file.header.info.clone(),
        file.declared,
        file.data,
        spatial(ObjectIntegrity::None).limits,
        &session,
    )
    .unwrap();
    let base = file.data.as_ptr() as usize - encoded.as_ptr() as usize;
    let mut offset = start;
    loop {
        let event = decoder.read(&file.data[offset..]).unwrap();
        let length = event.entry().entry.record_len as usize;
        let selected = match scope {
            CheckScope::GroupPixels => matches!(event, NumericEvent::Group(_)),
            CheckScope::TilePixels => matches!(event, NumericEvent::End { .. }),
            CheckScope::EncodedObject => true,
        };
        offset += length;
        if selected {
            encoded[base + offset - 1] ^= 1;
            break;
        }
    }
}

#[test]
fn scoped_pixel_tampering_is_rejected_without_an_outer_or_legacy_checksum() {
    for (flag, scope) in [
        (1, CheckScope::GroupPixels),
        (2, CheckScope::TilePixels),
        (4, CheckScope::EncodedObject),
    ] {
        let (info, _, _, mut encoded) = fixture(SampleType::U8, false, flag, ObjectIntegrity::None);
        corrupt_tag(&mut encoded, scope);
        for threads in [1, 2, 4, 8] {
            let mut output = vec![0; info.row_bytes() * info.height as usize];
            let error =
                crate::decode_into_threads(&encoded, &mut output, info.row_bytes(), threads)
                    .unwrap_err();
            assert!(
                matches!(
                    error,
                    Error::Corrupt("V4 pixel check mismatch")
                        | Error::Corrupt("V4 encoded check mismatch")
                ),
                "{scope:?}: {error:?}"
            );
        }
    }
}

#[test]
fn scoped_checks_survive_reader_feed_and_owned_row_suspensions() {
    for flags in 1..8 {
        let (info, pixels, stride, encoded) =
            fixture(SampleType::U16, false, flags, ObjectIntegrity::None);
        let packed: Vec<_> = (0..info.height as usize)
            .flat_map(|y| pixels[y * stride..][..info.row_bytes()].iter().copied())
            .collect();
        super::feed_tests::check_stream(&encoded, &packed, &info);
        for owned in [false, true] {
            let mut session = DecodeSession::new(DecodePolicy::default());
            let mut state = if owned {
                session.owned_row_state(encoded.clone())
            } else {
                session.row_state(&encoded)
            }
            .unwrap();
            let mut row = vec![0xa7; info.row_bytes() + 17];
            for y in 0..info.height as usize {
                assert!(state.next(&mut row, &|| Ok(())).unwrap());
                assert_eq!(
                    &row[..info.row_bytes()],
                    &packed[y * info.row_bytes()..][..info.row_bytes()]
                );
                assert!(row[info.row_bytes()..].iter().all(|&b| b == 0xa7));
            }
            assert!(!state.next(&mut row, &|| Ok(())).unwrap());
        }
    }
    for (flag, scope) in [(1, CheckScope::GroupPixels), (2, CheckScope::TilePixels)] {
        let (_, _, _, mut encoded) = fixture(SampleType::U8, false, flag, ObjectIntegrity::None);
        corrupt_tag(&mut encoded, scope);
        let mut session = DecodeSession::new(DecodePolicy::default());
        assert_eq!(
            session
                .decode_reader(&encoded[..], |_, _, _| Ok(()))
                .unwrap_err(),
            Error::Corrupt("V4 pixel check mismatch")
        );
        let mut session = DecodeSession::new(DecodePolicy::default());
        let mut state = session.row_state(&encoded).unwrap();
        let mut row = vec![0; 41 * 4];
        loop {
            match state.next(&mut row, &|| Ok(())) {
                Ok(true) => {}
                Ok(false) => panic!("accepted corrupt scoped pixels"),
                Err(e) => {
                    assert_eq!(e, Error::Corrupt("V4 pixel check mismatch"));
                    break;
                }
            }
        }
    }
}

#[test]
fn checked_forward_writer_has_identical_tags_without_a_full_raster_owner() {
    use std::io::Cursor;
    for flags in 1..8 {
        let (info, pixels, stride, expected) = fixture(
            SampleType::U16,
            false,
            flags,
            ObjectIntegrity::ObjectsAndTiles,
        );
        let cfg = ImiEncoderConfig::lossless().with_row_groups(RowGroups::new(9, 7, 8 << 20));
        let bytes = manifest(flags);
        let checks = Checks::read(&bytes, CheckLimits::default()).unwrap().0;
        let mut writer = NumericFileEncoder::new_checked(
            info.clone(),
            &Metadata::default(),
            &cfg,
            spatial(ObjectIntegrity::ObjectsAndTiles),
            Vec::new(),
            Cursor::new(Vec::new()),
            1 << 20,
            8 << 20,
            checks,
        )
        .unwrap();
        for _ in 0..writer.layout().tile_count() {
            writer.begin_tile().unwrap();
            while let Some(rect) = writer.next_group() {
                writer
                    .push_group(
                        &pixels
                            [rect.y as usize * stride + rect.x as usize * info.bytes_per_pixel()..],
                        stride,
                    )
                    .unwrap();
            }
            writer.end_tile().unwrap();
        }
        let (encoded, _) = writer.finish().unwrap();
        assert_eq!(
            encoded,
            super::file_tests::rewrite_placement(&expected, DirectoryPlacement::Tail, true)
        );
    }
}

#[test]
fn scoped_lossy_tags_hash_reconstruction_and_preserve_both_entropy_coders() {
    let info = ImageInfo::new(79, 67, Channels::Rgba, SampleType::U8);
    let source: Vec<_> = (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 73 + i / 11) as u8)
        .collect();
    for coder in [crate::LossyEntropy::RunLevel, crate::LossyEntropy::SplitHuf] {
        let original = ImiEncoderConfig::lossy(71.0)
            .with_column_groups(Some(32))
            .with_checksum(true)
            .tune_lossy(|t| {
                t.entropy = coder;
                t.stripe_block_rows = 2;
            })
            .encode_samples(
                crate::ImageRef::packed(&source, &info),
                &Metadata::default(),
            )
            .unwrap();
        let expected = crate::decode(&original).unwrap().1;
        assert_ne!(source, expected);
        for flags in 1..8 {
            let bytes = manifest(flags);
            let checks = Checks::read(&bytes, CheckLimits::default()).unwrap().0;
            let session = DecodeSession::new(DecodePolicy::default());
            let encoded = add_scoped_checks(&original, checks, &session, 8 << 20).unwrap();
            for workers in [1, 2, 4, 8] {
                let mut output = vec![0xa7; expected.len()];
                crate::decode_into_threads(&encoded, &mut output, info.row_bytes(), workers)
                    .unwrap();
                assert_eq!(output, expected);
            }
            super::feed_tests::check_stream(&encoded, &expected, &info);
            let mut session = DecodeSession::new(DecodePolicy::default());
            let mut output = vec![0; expected.len()];
            session
                .decode_rows(&encoded, &mut vec![0; info.row_bytes()], |_, y, row| {
                    output[y as usize * info.row_bytes()..][..info.row_bytes()]
                        .copy_from_slice(row);
                    Ok(())
                })
                .unwrap();
            assert_eq!(output, expected);
            let session = DecodeSession::new(DecodePolicy::default());
            assert!(matches!(
                add_scoped_checks(&encoded, checks, &session, 8 << 20),
                Err(Error::InvalidInput("image already has scoped checks"))
            ));
        }
    }
}

#[test]
fn group_and_tile_validation_are_independent_caller_choices() {
    for scope in [CheckScope::GroupPixels, CheckScope::TilePixels] {
        let (info, _, _, mut encoded) = fixture(SampleType::U8, false, 3, ObjectIntegrity::None);
        corrupt_tag(&mut encoded, scope);
        for groups in [false, true] {
            for tiles in [false, true] {
                let policy = DecodePolicy::default().with_scoped_pixel_checks(groups, tiles);
                let mut session = DecodeSession::new(policy);
                let mut output = vec![0; info.checked_buffer_len().unwrap()];
                let result = session.decode_into(&encoded, &mut output, info.row_bytes());
                let required = match scope {
                    CheckScope::GroupPixels => groups,
                    _ => tiles,
                };
                assert_eq!(
                    result.is_err(),
                    required,
                    "scope {scope:?}, group {groups}, tile {tiles}: {result:?}"
                );
                let mut session = DecodeSession::new(policy);
                assert_eq!(
                    session
                        .decode_reader(&encoded[..], |_, _, _| Ok(()))
                        .is_err(),
                    required
                );
                let mut session = DecodeSession::new(policy);
                let mut state = session.row_state(&encoded).unwrap();
                let mut row = vec![0; info.row_bytes()];
                let result = loop {
                    match state.next(&mut row, &|| Ok(())) {
                        Ok(true) => {}
                        other => break other,
                    }
                };
                assert_eq!(result.is_err(), required);
            }
        }
    }
}

#[test]
fn scoped_checks_match_across_routes_and_directory_placements() {
    use crate::planes::{Rect, SliceReader};
    use crate::region::RegionReader;
    use crate::validation::{ValidationLimits, validate};
    use std::io::Cursor;

    let validate_file = |file: &[u8]| {
        validate(
            &mut SliceReader::new(file),
            &mut Cursor::new(Vec::new()),
            &ValidationLimits::default(),
        )
    };
    for flags in 0..8 {
        let (info, pixels, stride, original) =
            fixture(SampleType::U8, false, flags, ObjectIntegrity::None);
        let expected: Vec<_> = (0..info.height as usize)
            .flat_map(|y| pixels[y * stride..][..info.row_bytes()].iter().copied())
            .collect();
        for placement in [
            DirectoryPlacement::None,
            DirectoryPlacement::Front,
            DirectoryPlacement::Tail,
        ] {
            for open in [false, true] {
                let file = super::file_tests::rewrite_placement(&original, placement, open);
                let mut whole = vec![0; expected.len()];
                crate::decode_into_threads(&file, &mut whole, info.row_bytes(), 4).unwrap();
                assert_eq!(whole, expected);
                super::feed_tests::check_stream(&file, &expected, &info);
                assert_eq!(validate_file(&file).unwrap().images, 1);

                let rect = Rect::new(6, 8, 19, 17);
                let row = rect.width() as usize * info.bytes_per_pixel();
                let mut crop = vec![0; row * rect.height() as usize];
                RegionReader::open(SliceReader::new(&file))
                    .unwrap()
                    .decode_region_threads(rect, &mut crop, row, 4)
                    .unwrap();
                for (i, pixels) in crop.chunks_exact(row).enumerate() {
                    let offset = (rect.y() as usize + i) * info.row_bytes()
                        + rect.x() as usize * info.bytes_per_pixel();
                    assert_eq!(pixels, &expected[offset..offset + row]);
                }

                if flags == 7 {
                    let mut damaged = file;
                    corrupt_tag(&mut damaged, CheckScope::EncodedObject);
                    assert!(crate::decode(&damaged).is_err());
                    assert!(super::feed_tests::feed(&damaged, 17).is_err());
                    assert!(validate_file(&damaged).is_err());
                    let region =
                        RegionReader::open(SliceReader::new(&damaged)).and_then(|mut r| {
                            r.decode_region_threads(
                                Rect::new(0, 0, info.width, info.height),
                                &mut whole,
                                info.row_bytes(),
                                4,
                            )
                        });
                    assert!(region.is_err());
                }
            }
        }
    }
}

#[test]
fn strict_validation_requires_all_scoped_checks() {
    use crate::planes::SliceReader;
    use crate::validation::{ValidationLimits, validate};
    use std::io::Cursor;

    let mut limits = ValidationLimits::default();
    limits.decode = limits.decode.with_scoped_pixel_checks(false, false);
    let validate_file = |file: &[u8]| {
        validate(
            &mut SliceReader::new(file),
            &mut Cursor::new(Vec::new()),
            &limits,
        )
    };
    for (flag, scope) in [
        (1, CheckScope::GroupPixels),
        (2, CheckScope::TilePixels),
        (4, CheckScope::EncodedObject),
    ] {
        let (_, _, _, encoded) = fixture(SampleType::U8, false, flag, ObjectIntegrity::None);
        validate_file(&encoded).unwrap();

        let mut bad_result = encoded.clone();
        corrupt_tag(&mut bad_result, scope);
        assert!(matches!(
            validate_file(&bad_result),
            Err(Error::Corrupt(
                "V4 pixel check mismatch" | "V4 encoded check mismatch"
            ))
        ));

        let declarations = manifest(flag);
        let session = DecodeSession::new(DecodePolicy::default());
        let file = super::file::parse_file(&encoded, &session).unwrap();
        let mut data = crate::ByteReader::new(file.data);
        let len = data.var_usize().unwrap();
        let setup = data.bytes(len).unwrap();
        let mut positions = setup
            .windows(declarations.len())
            .enumerate()
            .filter_map(|(i, bytes)| (bytes == declarations).then_some(i));
        let manifest_at =
            setup.as_ptr() as usize - encoded.as_ptr() as usize + positions.next().unwrap();
        assert_eq!(positions.next(), None);
        let mut unknown = encoded;
        unknown[manifest_at + 2] = 99;
        unknown[manifest_at + 5] = 100;
        crate::decode(&unknown).unwrap();
        assert_eq!(
            validate_file(&unknown),
            Err(Error::Unsupported(
                "V4 integrity suite in strict validation"
            ))
        );
    }
}

#[test]
fn sparse_reads_check_selected_groups_and_only_complete_tile_rasters() {
    use crate::planes::{Rect, SliceReader};
    for scope in [CheckScope::GroupPixels, CheckScope::TilePixels] {
        let flag = if scope == CheckScope::GroupPixels {
            1
        } else {
            2
        };
        let (info, _, _, mut encoded) = fixture(SampleType::U8, false, flag, ObjectIntegrity::None);
        corrupt_tag(&mut encoded, scope);
        for workers in [1, 2, 4, 8] {
            let mut reader = crate::region::RegionReader::open(SliceReader::new(&encoded)).unwrap();
            // Corruption belongs to tile 0; unrelated selection remains readable.
            let rect = Rect::new(40, 30, 1, 1);
            reader
                .decode_region_threads(rect, &mut [0; 4], 4, workers)
                .unwrap();
            let rect = Rect::new(0, 0, 1, 1);
            let result = reader.decode_region_threads(rect, &mut [0; 4], 4, workers);
            assert_eq!(result.is_err(), scope == CheckScope::GroupPixels);
            let rect = Rect::new(0, 0, 18, 14); // Complete 2x2 coding-group tile.
            let mut output = vec![0; 18 * 14 * 4];
            assert_eq!(
                reader
                    .decode_region_threads(rect, &mut output, 18 * 4, workers)
                    .unwrap_err(),
                Error::Corrupt("V4 pixel check mismatch")
            );
            let mut full = vec![0; info.checked_buffer_len().unwrap()];
            assert_eq!(
                reader
                    .decode_region_threads(
                        Rect::new(0, 0, info.width, info.height),
                        &mut full,
                        info.row_bytes(),
                        workers
                    )
                    .unwrap_err(),
                Error::Corrupt("V4 pixel check mismatch")
            );
        }
    }
}

#[test]
fn encoded_check_bytes_are_admitted_before_publishing_a_tile_begin() {
    let bytes = manifest(4);
    let checks = Checks::read(&bytes, CheckLimits::default())
        .unwrap()
        .0
        .owned()
        .unwrap();
    let mut writer = TileWriter::new(
        TileLayout::new(1, 1, 1, 1, TileOrder::Raster).unwrap(),
        ObjectIntegrity::None,
        SetupContext::from_declaration(b"scoped admission"),
        TileLimits {
            max_object_bytes: 100,
            max_tile_bytes: 16,
        },
    )
    .with_checks(checks, crate::registry::UNIT_STRIPE);
    assert_eq!(writer.begin(&[]), Err(Error::TooLarge));
    assert!(writer.output().is_empty());
}

#[test]
fn opaque_future_slots_are_visible_and_never_treated_as_an_existing_suite() {
    let (info, pixels, stride, mut encoded) =
        fixture(SampleType::U8, false, 1, ObjectIntegrity::None);
    let session = DecodeSession::new(DecodePolicy::default());
    let parsed = super::file::parse_file(&encoded, &session).unwrap();
    let mut r = crate::ByteReader::new(parsed.data);
    let length = r.var_usize().unwrap();
    let body = r.bytes(length).unwrap();
    // Same-width unassigned suite 127 replaces the final XXH3-128 descriptor.
    let at = body.as_ptr() as usize - encoded.as_ptr() as usize + body.len() - 3;
    assert_eq!(&encoded[at..at + 3], &[1, 16, 0]);
    encoded[at] = 127;
    corrupt_tag(&mut encoded, CheckScope::GroupPixels); // Changes opaque result only.
    let session = DecodeSession::new(DecodePolicy::default());
    let file =
        NumericFile::open(&encoded, spatial(ObjectIntegrity::None).limits, &session).unwrap();
    assert_eq!(
        file.checks()
            .list(CheckScope::GroupPixels)
            .iter()
            .map(|c| c.suite_id)
            .collect::<Vec<_>>(),
        [0, 127]
    );
    let mut output = vec![0; info.checked_buffer_len().unwrap()];
    file.decode_into(&mut output, info.row_bytes()).unwrap();
    for y in 0..info.height as usize {
        assert_eq!(
            &output[y * info.row_bytes()..][..info.row_bytes()],
            &pixels[y * stride..][..info.row_bytes()]
        );
    }
}

#[cfg(feature = "blake3")]
#[test]
fn blake3_checks_round_trip_and_reject_corruption_in_each_scope() {
    let info = ImageInfo::new(19, 17, Channels::Scalar, SampleType::U8);
    let pixels: Vec<_> = (0..19 * 17).map(|i| (i * 17) as u8).collect();
    let group = [CheckDescriptor {
        suite_id: 2,
        result_bytes: 32,
        parameters: &[],
    }];
    let tile = [CheckDescriptor {
        suite_id: 5,
        result_bytes: 128,
        parameters: &[],
    }];
    let encoded = [CheckDescriptor {
        suite_id: 5,
        result_bytes: 64,
        parameters: &[],
    }];
    let coding = ImiEncoderConfig::lossless().with_row_groups(RowGroups::new(9, 7, 8 << 20));
    let encode = |scopes| {
        let mut declaration = Vec::new();
        Checks::append(&mut declaration, scopes, CheckLimits::default()).unwrap();
        let (checks, _) = Checks::read(&declaration, CheckLimits::default()).unwrap();
        encode_numeric_checked(
            info.clone(),
            &pixels,
            info.row_bytes(),
            &Metadata::default(),
            &coding,
            spatial(ObjectIntegrity::None),
            checks,
        )
        .unwrap()
    };
    let file = encode([&group, &tile, &encoded]);
    assert_eq!(crate::decode(&file).unwrap().1, pixels);
    super::feed_tests::check_stream(&file, &pixels, &info);
    let mut session = DecodeSession::new(DecodePolicy::default());
    let mut output = vec![0; pixels.len()];
    session
        .decode_rows(&file, &mut vec![0; info.row_bytes()], |_, y, row| {
            output[y as usize * info.row_bytes()..][..info.row_bytes()].copy_from_slice(row);
            Ok(())
        })
        .unwrap();
    assert_eq!(output, pixels);
    for (scope, descriptors, mismatch) in [
        (CheckScope::GroupPixels, &group, "V4 pixel check mismatch"),
        (CheckScope::TilePixels, &tile, "V4 pixel check mismatch"),
        (
            CheckScope::EncodedObject,
            &encoded,
            "V4 encoded check mismatch",
        ),
    ] {
        let mut scopes = [&[][..]; 3];
        scopes[scope as usize] = descriptors;
        let mut file = encode(scopes);
        assert_eq!(crate::decode(&file).unwrap().1, pixels);
        corrupt_tag(&mut file, scope);
        assert!(
            matches!(crate::decode(&file), Err(Error::Corrupt(message)) if message == mismatch)
        );
    }
}
