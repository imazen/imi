use super::*;
use crate::codec::{DecodeConfig, FeedStatus};
use crate::{Channels, ImageInfo, ImiEncoderConfig, Metadata, RowGroups, SampleType};
use alloc::vec::Vec;
use zencodec::decode::DecoderConfig;

fn fixture(sample: SampleType, channels: Channels, quick: bool) -> (Vec<u8>, Vec<u8>, ImageInfo) {
    let info = ImageInfo::new(41, 31, channels, sample);
    let pixels: Vec<_> = (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 53 + i / 7) as u8)
        .collect();
    let config = if quick {
        ImiEncoderConfig::quick()
    } else {
        ImiEncoderConfig::lossless()
    }
    .with_row_groups(RowGroups::new(9, 7, 8 << 20));
    let file = encode_numeric(
        info.clone(),
        &pixels,
        info.row_bytes(),
        &Metadata::default(),
        &config,
        SpatialConfig {
            tile_columns: 3,
            tile_rows: 2,
            order: TileOrder::Morton,
            integrity: ObjectIntegrity::ObjectsAndTiles,
            limits: TileLimits {
                max_object_bytes: 1 << 20,
                max_tile_bytes: 16 << 20,
            },
        },
    )
    .unwrap();
    (file, pixels, info)
}

pub(crate) fn feed(file: &[u8], capacity: usize) -> Result<(Vec<u8>, u64), crate::Error> {
    let mut decoder = DecodeConfig::default()
        .job()
        .feed_decoder(capacity)
        .unwrap();
    let mut at = 0;
    let mut output = Vec::new();
    loop {
        match decoder.advance() {
            Ok(FeedStatus::NeedInput) => {
                if at == file.len() {
                    decoder.finish_input().unwrap();
                } else {
                    at += decoder.push(&file[at..]).unwrap();
                }
            }
            Ok(FeedStatus::Row { y }) => {
                let row = decoder.row().unwrap().unwrap();
                assert_eq!(output.len(), y as usize * row.row(0).len());
                output.extend_from_slice(row.row(0));
            }
            Ok(FeedStatus::Complete) => return Ok((output, decoder.usage().input_bytes)),
            Err(crate::codec::DecodeError::Codec(error)) => {
                assert!(decoder.advance().is_err());
                return Err(error);
            }
            Err(error) => panic!("unexpected adapter error: {error:?}"),
        }
    }
}

/// Extend existing frozen pixel witnesses through the actual incremental route.
pub(crate) fn check_stream(file: &[u8], expected: &[u8], info: &ImageInfo) {
    let mut output = Vec::new();
    let mut session = crate::decode_resources::DecodeSession::new(
        crate::decode_resources::DecodePolicy::default(),
    );
    let (actual, _) = session
        .decode_reader(file, |actual, y, row| {
            assert_eq!(actual, info);
            assert_eq!(output.len(), y as usize * info.row_bytes());
            output.extend_from_slice(row);
            Ok(())
        })
        .unwrap();
    assert_eq!(&actual, info);
    assert_eq!(output, expected);
    assert_eq!(session.usage().input_bytes, file.len() as u64);
    if matches!(
        info.sample,
        SampleType::U8 | SampleType::U16 | SampleType::F16 | SampleType::F32
    ) && matches!(
        info.channels,
        Channels::Gray | Channels::GrayAlpha | Channels::Rgb | Channels::Rgba
    ) {
        let (actual, bytes) = feed(file, 17).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(bytes, file.len() as u64);
    }
}

#[test]
fn numeric_reader_and_feed_preserve_bits_across_tile_order_and_placements() {
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
            for quick in [false, true] {
                let (original, expected, info) = fixture(sample, channels, quick);
                for placement in [
                    DirectoryPlacement::None,
                    DirectoryPlacement::Front,
                    DirectoryPlacement::Tail,
                ] {
                    for open in [false, true] {
                        let file = super::file_tests::rewrite_placement(&original, placement, open);
                        let mut rows = Vec::new();
                        let (actual, _) = crate::decode_reader(
                            file.as_slice(),
                            &crate::Limits::DEFAULT,
                            |got, y, row| {
                                assert_eq!(got, &info);
                                assert_eq!(rows.len(), y as usize * info.row_bytes());
                                rows.extend_from_slice(row);
                                Ok(())
                            },
                        )
                        .unwrap();
                        assert_eq!(actual, info);
                        assert_eq!(
                            rows, expected,
                            "{sample:?}/{channels:?}/{quick}/{placement:?}/{open}"
                        );
                        // Common pixel descriptors support these four channel layouts.
                        if matches!(
                            sample,
                            SampleType::U8 | SampleType::U16 | SampleType::F16 | SampleType::F32
                        ) && matches!(
                            channels,
                            Channels::Gray | Channels::GrayAlpha | Channels::Rgb | Channels::Rgba
                        ) {
                            let (rows, input) = feed(&file, 17).unwrap();
                            assert_eq!(rows, expected);
                            assert_eq!(input, file.len() as u64);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn numeric_feed_rejects_truncation_and_late_directory_errors() {
    let (file, _, _) = fixture(SampleType::U8, Channels::Rgb, false);
    for cut in [0, 8, file.len() / 2, file.len() - 1] {
        assert!(feed(&file[..cut], 7).is_err());
    }
    let mut corrupt = file.clone();
    *corrupt.last_mut().unwrap() ^= 1;
    assert!(feed(&corrupt, 7).is_err());
}

#[test]
fn caller_feed_v4_known_group_bound_keeps_consumed_work_and_sticky_failure() {
    let (file, _, _) = fixture(SampleType::U8, Channels::Rgb, false);
    let policy = crate::decode_resources::DecodePolicy::default().with_max_units(5);
    let mut decoder = DecodeConfig::default()
        .with_admission(policy)
        .job()
        .feed_decoder(file.len())
        .unwrap();
    assert_eq!(decoder.push(&file).unwrap(), file.len());
    assert!(matches!(
        decoder.advance(),
        Err(crate::codec::DecodeError::Codec(crate::Error::TooLarge))
    ));
    let usage = decoder.usage();
    assert_eq!(usage.input_bytes, file.len() as u64);
    assert_eq!(usage.units, 0);
    assert!(usage.work_units > 0);
    assert!(decoder.advance().is_err());
    assert_eq!(decoder.usage(), usage);
}

#[test]
fn lossy_feed_publishes_first_group_before_later_input() {
    let info = ImageInfo::new(41, 77, Channels::Rgba, SampleType::U8);
    let pixels: Vec<_> = (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 53 + i / 7) as u8)
        .collect();
    for coder in [crate::LossyEntropy::RunLevel, crate::LossyEntropy::SplitHuf] {
        let config = ImiEncoderConfig::lossy(73.0).tune_lossy(|t| {
            t.entropy = coder;
            t.stripe_block_rows = 2;
            t.exact_blocks = Some(0.0);
        });
        let native = config
            .encode_samples(
                crate::ImageRef::packed(&pixels, &info),
                &Metadata::default(),
            )
            .unwrap();
        let expected = crate::decode(&native).unwrap().1;
        let file = upgrade_image(&native, &crate::UpgradeLimits::default()).unwrap();
        let prefix = super::file_tests::rewrite_placement(&file, DirectoryPlacement::Tail, true);
        let mut r = crate::ByteReader::new(&prefix);
        crate::parse::header::read_header_v4(&mut r, 64 << 10).unwrap();
        crate::parse::chunks::read_head(&mut r).unwrap();
        let len = r.var_usize().unwrap();
        r.bytes(len).unwrap();
        r.varint().unwrap();
        loop {
            let kind = r.varint().unwrap();
            let len = r.var_usize().unwrap();
            r.bytes(len).unwrap();
            if kind == crate::registry::UNIT_AC {
                break;
            }
        }
        let stop = r.pos;
        let mut decoder = DecodeConfig::default().job().feed_decoder(stop).unwrap();
        assert_eq!(decoder.push(&prefix[..stop]).unwrap(), stop);
        for y in 0..16 {
            assert_eq!(decoder.advance().unwrap(), FeedStatus::Row { y });
            assert_eq!(
                decoder.row().unwrap().unwrap().row(0),
                &expected[y as usize * info.row_bytes()..][..info.row_bytes()]
            );
        }
        assert_eq!(decoder.advance().unwrap(), FeedStatus::NeedInput);
        let mut decoder = std::thread::spawn(move || decoder).join().unwrap();
        assert_eq!(decoder.advance().unwrap(), FeedStatus::NeedInput);
        decoder.finish_input().unwrap();
        assert!(decoder.advance().is_err());
    }
}

#[test]
fn numeric_feed_yields_first_group_before_tile_end_and_can_move_threads() {
    let info = ImageInfo::new(19, 15, Channels::Rgb, SampleType::U8);
    let pixels: Vec<_> = (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 53) as u8)
        .collect();
    let config = ImiEncoderConfig::lossless().with_row_groups(RowGroups::new(19, 5, 8 << 20));
    let spatial = SpatialConfig {
        tile_columns: 1,
        tile_rows: 3,
        order: TileOrder::Raster,
        integrity: ObjectIntegrity::ObjectsAndTiles,
        limits: TileLimits {
            max_object_bytes: 1 << 20,
            max_tile_bytes: 16 << 20,
        },
    };
    // Stop immediately after the first complete group: the tile remains open
    // and no trailing directory or file finalization is available.
    let file = encode_numeric(
        info.clone(),
        &pixels,
        info.row_bytes(),
        &Metadata::default(),
        &config,
        spatial,
    )
    .unwrap();
    let prefix = super::file_tests::rewrite_placement(&file, DirectoryPlacement::Tail, true);
    // Locate the group's compressed bytes in the open-DATA representation.
    let mut r = crate::ByteReader::new(&prefix);
    crate::parse::header::read_header_v4(&mut r, 64 << 10).unwrap();
    let (_, _) = crate::parse::chunks::read_head(&mut r).unwrap();
    let setup_len = r.var_usize().unwrap();
    r.bytes(setup_len).unwrap();
    r.varint().unwrap();
    for _ in 0..2 {
        r.varint().unwrap();
        let len = r.var_usize().unwrap();
        r.bytes(len).unwrap();
    }
    let stop = r.pos;
    let mut decoder = DecodeConfig::default().job().feed_decoder(stop).unwrap();
    assert_eq!(decoder.push(&prefix[..stop]).unwrap(), stop);
    for y in 0..5 {
        assert_eq!(decoder.advance().unwrap(), FeedStatus::Row { y });
        assert_eq!(
            decoder.row().unwrap().unwrap().row(0),
            &pixels[y as usize * info.row_bytes()..][..info.row_bytes()]
        );
    }
    assert_eq!(decoder.advance().unwrap(), FeedStatus::NeedInput);
    let mut decoder = std::thread::spawn(move || decoder).join().unwrap();
    assert_eq!(decoder.advance().unwrap(), FeedStatus::NeedInput);
    assert!(decoder.info().is_none());
    decoder.finish_input().unwrap();
    assert!(decoder.advance().is_err());

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::net::UnixStream;
        use std::sync::mpsc;
        use std::time::Duration;
        let (mut producer, consumer) = UnixStream::pair().unwrap();
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut pixels = Vec::new();
            let result = crate::decode_reader(consumer, &crate::Limits::DEFAULT, |_, y, row| {
                pixels.extend_from_slice(row);
                tx.send(y).unwrap();
                Ok(())
            });
            (result, pixels)
        });
        producer.write_all(&prefix[..stop]).unwrap();
        for y in 0..5 {
            assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), y);
        }
        assert_eq!(rx.try_recv(), Err(mpsc::TryRecvError::Empty));
        producer.write_all(&prefix[stop..]).unwrap();
        producer.shutdown(std::net::Shutdown::Write).unwrap();
        let (result, actual) = worker.join().unwrap();
        assert_eq!(result.unwrap().0, info);
        assert_eq!(actual, pixels);
    }
}
