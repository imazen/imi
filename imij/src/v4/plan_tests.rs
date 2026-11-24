use super::*;
use crate::decode_resources::{DecodePolicy, DecodeSession};
use crate::{Channels, Error, ImageRef, ImiEncoderConfig, Metadata, SampleType};
use alloc::{borrow::Cow, vec, vec::Vec};

fn check_rows(file: &[u8], expected: &[u8], info: &crate::ImageInfo) {
    #[cfg(feature = "std")]
    super::feed_tests::check_stream(file, expected, info);
    let mut session = DecodeSession::new(DecodePolicy::default());
    let mut row = vec![0xA7; info.row_bytes() + 19];
    let mut next = 0;
    assert_eq!(
        session
            .decode_rows(file, &mut row, |got, y, pixels| {
                assert_eq!(got, info);
                assert_eq!(y, next);
                assert_eq!(
                    pixels,
                    &expected[y as usize * info.row_bytes()..][..info.row_bytes()]
                );
                next += 1;
                Ok(())
            })
            .unwrap(),
        *info
    );
    assert_eq!(next, info.height);
    assert!(row[info.row_bytes()..].iter().all(|&b| b == 0xA7));
    assert_eq!(session.usage().input_bytes, file.len() as u64);

    // The original admission session goes away. All model reservations and
    // usage must survive with both borrowed and owned row states.
    for owned in [false, true] {
        let mut state = {
            let mut session = DecodeSession::new(DecodePolicy::default());
            if owned {
                session.owned_row_state(file.to_vec()).unwrap()
            } else {
                session.row_state(file).unwrap()
            }
        };
        for y in 0..info.height {
            assert!(state.next(&mut row, &|| Ok(())).unwrap());
            assert_eq!(
                &row[..info.row_bytes()],
                &expected[y as usize * info.row_bytes()..][..info.row_bytes()]
            );
            assert_eq!(state.rows_decoded(), y + 1);
        }
        assert!(!state.next(&mut row, &|| Ok(())).unwrap());
        assert_eq!(state.usage().unwrap().input_bytes, file.len() as u64);
    }
}

#[test]
fn numeric_rows_restore_tile_order_and_preserve_all_native_bits() {
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
        for channels in [
            Channels::Gray,
            Channels::GrayAlpha,
            Channels::Rgb,
            Channels::Rgba,
            Channels::Cmyk,
            Channels::Oklab,
            Channels::OklabAlpha,
        ] {
            let info = crate::ImageInfo::new(41, 31, channels, sample);
            let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
                .map(|i| (i * 73 + i / 7 * 41) as u8)
                .collect();
            for quick in [false, true] {
                let config = if quick {
                    ImiEncoderConfig::quick()
                } else {
                    ImiEncoderConfig::lossless()
                }
                .with_row_groups(crate::RowGroups::new(9, 7, 8 << 20));
                let spatial = SpatialConfig {
                    tile_columns: 3,
                    tile_rows: 2,
                    order: TileOrder::Morton,
                    integrity: ObjectIntegrity::ObjectsAndTiles,
                    limits: TileLimits {
                        max_object_bytes: 1 << 20,
                        max_tile_bytes: 16 << 20,
                    },
                };
                let file = encode_numeric(
                    info.clone(),
                    &pixels,
                    info.row_bytes(),
                    &Metadata::default(),
                    &config,
                    spatial,
                )
                .unwrap();
                check_rows(&file, &pixels, &info);
                let session = DecodeSession::new(DecodePolicy::default());
                let (mut rows, _, _) = plan::Rows::new(Cow::Borrowed(&file), &session).unwrap();
                // Constructor does not allocate a decoded raster or band.
                let mut row = vec![0; info.row_bytes()];
                assert!(rows.next_row_checked(&mut row, &|| Ok(())).unwrap());
                assert_eq!(row, pixels[..info.row_bytes()]);
            }
        }
    }
}

#[test]
fn lossy_rows_match_native_models_defaults_and_bitmap_checksums() {
    let info = crate::ImageInfo::new(137, 129, Channels::Rgba, SampleType::U8);
    let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 73 + i / 7 * 41) as u8)
        .collect();
    for entropy in [crate::LossyEntropy::RunLevel, crate::LossyEntropy::SplitHuf] {
        for checksum in [false, true] {
            let config = ImiEncoderConfig::lossy(71.0)
                .with_column_groups(Some(32))
                .with_checksum(checksum)
                .tune_lossy(|t| {
                    t.entropy = entropy;
                    t.stripe_block_rows = 2;
                });
            let native = config
                .encode_legacy_samples(ImageRef::packed(&pixels, &info), &Metadata::default())
                .unwrap();
            let native = crate::pixelsum::add_pixel_checksum(native).unwrap();
            let expected = crate::decode(&native).unwrap().1;
            let file = upgrade_image(&native, &crate::UpgradeLimits::default()).unwrap();
            check_rows(&file, &expected, &info);
            let mut session = DecodeSession::new(DecodePolicy::default());
            let mut state = session.owned_row_state(file).unwrap();
            assert_eq!(state.status(), crate::PixelChecksumStatus::Pending);
            let mut row = vec![0; info.row_bytes()];
            for _ in 0..info.height {
                assert!(state.next(&mut row, &|| Ok(())).unwrap());
            }
            assert_eq!(state.status(), crate::PixelChecksumStatus::Verified);
        }
    }
}

#[test]
fn row_failure_is_sticky_and_work_usage_survives_suspension() {
    let info = crate::ImageInfo::new(73, 97, Channels::Rgb, SampleType::U8);
    let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 73) as u8)
        .collect();
    let config = ImiEncoderConfig::lossy(71.0)
        .with_column_groups(Some(24))
        .tune_lossy(|t| t.stripe_block_rows = 1);
    let native = config
        .encode_legacy_samples(ImageRef::packed(&pixels, &info), &Metadata::default())
        .unwrap();
    let file = upgrade_image(&native, &crate::UpgradeLimits::default()).unwrap();
    let mut session = DecodeSession::new(DecodePolicy::default());
    let mut state = session.owned_row_state(file.clone()).unwrap();
    let before = state.usage().unwrap();
    let mut row = vec![0; info.row_bytes()];
    assert!(state.next(&mut row, &|| Ok(())).unwrap());
    assert_eq!(state.usage().unwrap().work_units, before.work_units);
    assert!(matches!(
        state.next(&mut row, &|| Err(Error::Cancelled)),
        Err(Error::Cancelled)
    ));
    assert!(matches!(
        state.next(&mut row, &|| Ok(())),
        Err(Error::Cancelled)
    ));
    assert_eq!(state.rows_decoded(), 1);
    let mut limited = DecodeSession::new(DecodePolicy::default().with_max_working_bytes(1));
    assert!(matches!(
        limited.owned_row_state(file),
        Err(Error::TooLarge)
    ));
}

#[test]
fn changed_bitmap_digest_fails_at_final_row_and_stays_failed() {
    let info = crate::ImageInfo::new(41, 31, Channels::Rgb, SampleType::U8);
    let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 73) as u8)
        .collect();
    let native = ImiEncoderConfig::lossless()
        .encode_legacy_samples(ImageRef::packed(&pixels, &info), &Metadata::default())
        .unwrap();
    let native = crate::pixelsum::add_pixel_checksum(native).unwrap();
    let mut file = upgrade_image(&native, &crate::UpgradeLimits::default()).unwrap();
    let mut reader = crate::ByteReader::new(&file);
    crate::parse::header::read_header_sized(&mut reader).unwrap();
    let at = loop {
        let (tag, length) = crate::parse::chunks::read_head(&mut reader).unwrap();
        let body = crate::parse::chunks::payload(&mut reader, length.unwrap()).unwrap();
        if tag == *b"pSUM" {
            break reader.pos - 1;
        }
        assert!(!body.is_empty() || tag != *b"DATA");
    };
    file[at] ^= 1;
    let mut session = DecodeSession::new(DecodePolicy::default());
    let mut state = session.owned_row_state(file).unwrap();
    let mut row = vec![0; info.row_bytes()];
    for y in 0..info.height - 1 {
        assert!(state.next(&mut row, &|| Ok(())).unwrap());
        assert_eq!(
            row,
            pixels[y as usize * info.row_bytes()..][..info.row_bytes()]
        );
    }
    assert!(matches!(
        state.next(&mut row, &|| Ok(())),
        Err(crate::pixelsum::MISMATCH)
    ));
    assert!(matches!(
        state.next(&mut row, &|| Ok(())),
        Err(crate::pixelsum::MISMATCH)
    ));
    assert_eq!(state.rows_decoded(), info.height - 1);
    assert_eq!(state.status(), crate::PixelChecksumStatus::Pending);
}

#[cfg(feature = "std")]
#[test]
fn common_row_decoder_preserves_v4_pixels_metadata_and_owned_thread_transfer() {
    use zencodec::decode::{DecoderConfig, StreamingDecode};
    use zenpixels::PixelDescriptor;
    let info = crate::ImageInfo::new(41, 31, Channels::Rgba, SampleType::U8);
    let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 73) as u8)
        .collect();
    let metadata = Metadata {
        xmp: Some(b"<v4-rows/>".to_vec()),
        ..Default::default()
    };
    for lossy in [false, true] {
        let config = if lossy {
            ImiEncoderConfig::lossy(71.0).with_column_groups(Some(16))
        } else {
            ImiEncoderConfig::lossless().with_row_groups(crate::RowGroups::new(9, 7, 8 << 20))
        };
        let native = config
            .encode_legacy_samples(ImageRef::packed(&pixels, &info), &metadata)
            .unwrap();
        let expected = crate::decode(&native).unwrap().1;
        let file = upgrade_image(&native, &crate::UpgradeLimits::default()).unwrap();
        let mut decoder = crate::codec::DecodeConfig::default()
            .job()
            .owned_row_decoder(file, &[PixelDescriptor::RGBA8_SRGB])
            .unwrap();
        let expected_xmp = metadata.xmp.clone();
        let height = info.height;
        let received = std::thread::spawn(move || {
            assert_eq!(decoder.native_metadata().metadata.xmp, expected_xmp);
            let before = decoder.usage().work_units;
            let mut result = Vec::new();
            let mut y = 0;
            while let Some((at, row)) = decoder.next_batch().unwrap() {
                assert_eq!(at, y);
                assert_eq!(row.rows(), 1);
                result.extend_from_slice(row.row(0));
                y += 1;
            }
            assert_eq!(y, height);
            assert_eq!(decoder.usage().work_units, before);
            result
        })
        .join()
        .unwrap();
        assert_eq!(received, expected);
    }
}

#[test]
fn public_slice_stream_preserves_v4_rows_native_bits_and_checksum_completion() {
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
        let info = crate::ImageInfo::new(41, 31, Channels::Rgba, sample);
        let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
            .map(|i| (i * 73 + i / 7 * 41) as u8)
            .collect();
        for quick in [false, true] {
            let config = if quick {
                ImiEncoderConfig::quick()
            } else {
                ImiEncoderConfig::lossless()
            };
            let file = config
                .with_checksum(true)
                .encode_samples(ImageRef::packed(&pixels, &info), &Metadata::default())
                .unwrap();
            assert_eq!(crate::header(&file).unwrap().version, 4);
            assert!(matches!(
                crate::StreamDecoder::with_limits(
                    &file,
                    &crate::Limits {
                        max_pixels: 0,
                        ..crate::Limits::DEFAULT
                    }
                ),
                Err(Error::TooLarge)
            ));
            let file = crate::add_pixel_checksum(file).unwrap();
            let mut decoder = crate::StreamDecoder::new(&file).unwrap();
            assert_eq!(decoder.info(), &info);
            assert_eq!(
                decoder.pixel_checksum_status(),
                crate::PixelChecksumStatus::Pending
            );
            let mut row = vec![0x75; info.row_bytes() + 13];
            for y in 0..info.height as usize {
                assert!(decoder.next_row(&mut row).unwrap());
                assert_eq!(
                    &row[..info.row_bytes()],
                    &pixels[y * info.row_bytes()..(y + 1) * info.row_bytes()]
                );
                assert!(row[info.row_bytes()..].iter().all(|&b| b == 0x75));
                assert_eq!(decoder.rows_decoded(), y as u32 + 1);
            }
            assert!(!decoder.next_row(&mut row).unwrap());
            assert_eq!(
                decoder.pixel_checksum_status(),
                crate::PixelChecksumStatus::Verified
            );
        }
    }
    let info = crate::ImageInfo::new(67, 51, Channels::Rgba, SampleType::U8);
    let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 73) as u8)
        .collect();
    for entropy in [crate::LossyEntropy::RunLevel, crate::LossyEntropy::SplitHuf] {
        let file = ImiEncoderConfig::lossy(71.0)
            .tune_lossy(|t| t.entropy = entropy)
            .with_checksum(true)
            .encode_samples(ImageRef::packed(&pixels, &info), &Metadata::default())
            .unwrap();
        let expected = crate::decode(&file).unwrap().1;
        let file = crate::add_pixel_checksum(file).unwrap();
        let mut decoder = crate::StreamDecoder::new(&file).unwrap();
        let mut row = vec![0; info.row_bytes()];
        let mut output = Vec::new();
        while decoder.next_row(&mut row).unwrap() {
            output.extend_from_slice(&row);
        }
        assert_eq!(output, expected);
        assert_eq!(
            decoder.pixel_checksum_status(),
            crate::PixelChecksumStatus::Verified
        );
    }
}
