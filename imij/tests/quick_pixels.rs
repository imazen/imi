//! Exact quick units use the same public decode routes and container services.
#[cfg(feature = "std")]
mod common;
use imij::decode_resources::{DecodePolicy, DecodeSession};
use imij::planes::{Rect, SliceReader};
use imij::region::RegionReader;
use imij::*;

fn source(info: &ImageInfo, kind: usize) -> Vec<u8> {
    let mut state = 0x8e513aac_u32;
    (0..info.checked_buffer_len().unwrap())
        .map(|i| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            match kind {
                0 => 0,
                1 => (i / info.bytes_per_pixel() / 77) as u8,
                _ => state as u8,
            }
        })
        .collect()
}
fn routes(file: &[u8], info: &ImageInfo, raw: &[u8]) {
    assert_eq!(decode(file).unwrap(), (info.clone(), raw.to_vec()));
    let stride = info.row_bytes() + 7;
    for threads in [1, 2, 4, 8] {
        let mut out = vec![0xa5; stride * info.height as usize];
        #[cfg(feature = "std")]
        decode_into_threads(file, &mut out, stride, threads).unwrap();
        #[cfg(not(feature = "std"))]
        {
            let _ = threads;
            decode_into(file, &mut out, stride).unwrap();
        }
        for (got, want) in out.chunks(stride).zip(raw.chunks(info.row_bytes())) {
            assert_eq!(&got[..info.row_bytes()], want);
            assert!(got[info.row_bytes()..].iter().all(|&v| v == 0xa5));
        }
    }
    let mut stream = StreamDecoder::new(file).unwrap();
    let mut row = vec![0; info.row_bytes()];
    for want in raw.chunks(info.row_bytes()) {
        assert!(stream.next_row(&mut row).unwrap());
        assert_eq!(row, want);
    }
    assert!(!stream.next_row(&mut row).unwrap());
    let mut owned = FrameDecoder::from_owned(file.to_vec(), &Limits::DEFAULT).unwrap();
    owned.next_frame().unwrap().unwrap();
    let mut out = vec![0; raw.len()];
    owned.decode_into(&mut out, info.row_bytes(), 2).unwrap();
    assert_eq!(out, raw);
    let mut session = DecodeSession::new(DecodePolicy::default());
    assert_eq!(session.decode(file).unwrap().1, raw);
    let mut region = RegionReader::open(SliceReader::new(file)).unwrap();
    let rect = Rect::new(1, 1, info.width - 2, info.height - 2);
    let rb = rect.width() as usize * info.bytes_per_pixel();
    let mut cropped = vec![0; rb * rect.height() as usize];
    region.decode_region(rect, &mut cropped, rb).unwrap();
    for y in 0..rect.height() as usize {
        let start = (y + 1) * info.row_bytes() + info.bytes_per_pixel();
        assert_eq!(&cropped[y * rb..(y + 1) * rb], &raw[start..start + rb]);
    }
    #[cfg(feature = "std")]
    {
        let mut read = Vec::new();
        decode_reader(file, &Limits::DEFAULT, |_, _, row| {
            read.extend_from_slice(row);
            Ok(())
        })
        .unwrap();
        assert_eq!(read, raw);
    }
}
#[test]
fn storage_bits_padding_checksums_and_independent_units() {
    let mut samples = vec![
        SampleType::U8,
        SampleType::U16,
        SampleType::F16,
        SampleType::F32,
    ];
    if cfg!(feature = "extended") {
        samples.extend([
            SampleType::I8,
            SampleType::I16,
            SampleType::U32,
            SampleType::I32,
            SampleType::U64,
            SampleType::I64,
            SampleType::F64,
        ]);
    }
    for sample in samples {
        for channels in [
            Channels::Gray,
            Channels::GrayAlpha,
            Channels::Rgb,
            Channels::Rgba,
        ] {
            let info = ImageInfo::new(17, 19, channels, sample);
            for kind in 0..3 {
                let raw = source(&info, kind);
                let stride = info.row_bytes() + 11;
                let mut input = vec![0x5a; stride * 18 + info.row_bytes()];
                for y in 0..19 {
                    input[y * stride..][..info.row_bytes()]
                        .copy_from_slice(&raw[y * info.row_bytes()..][..info.row_bytes()]);
                }
                let mut cfg = imij::ImiEncoderConfig::quick();
                cfg = cfg.tune_lossless(|t| {
                    t.stripe_rows = Some(7);
                });
                cfg = cfg.with_column_groups(Some(9));
                cfg = cfg.with_checksum(true);
                let file = cfg
                    .encode_samples(
                        imij::ImageRef::new(&input, stride, &info),
                        &Metadata::default(),
                    )
                    .unwrap();
                cfg = cfg.with_threads(8);
                assert_eq!(
                    cfg.encode_samples(
                        imij::ImageRef::new(&input, stride, &info),
                        &Metadata::default()
                    )
                    .unwrap(),
                    file
                );
                let checked = add_pixel_checksum(file).unwrap();
                routes(&checked, &info, &raw);
            }
        }
    }
}

#[test]
fn contiguous_bands_keep_partial_rows_and_checksum_schedules() {
    // Small full-width stripes exercise bulk raw copying and short final rows.
    for sample in [SampleType::U8, SampleType::U16, SampleType::F32] {
        for rows in [3, 19, 131] {
            let info = ImageInfo::new(63, rows, Channels::Rgba, sample);
            for kind in [0, 1, 2] {
                let raw = source(&info, kind);
                for checksummed in [false, true] {
                    let mut cfg = imij::ImiEncoderConfig::quick();
                    cfg = cfg.tune_lossless(|t| {
                        t.stripe_rows = Some(7);
                    });
                    cfg = cfg.with_checksum(checksummed);
                    let file = cfg
                        .encode_samples(
                            imij::ImageRef::new(&raw, info.row_bytes(), &info),
                            &Metadata::default(),
                        )
                        .unwrap();
                    let file = add_pixel_checksum(file).unwrap();
                    for threads in [1, 2, 4, 8] {
                        let mut tight = vec![0x91; raw.len()];
                        #[cfg(feature = "std")]
                        decode_into_threads(&file, &mut tight, info.row_bytes(), threads).unwrap();
                        #[cfg(not(feature = "std"))]
                        {
                            let _ = threads;
                            decode_into(&file, &mut tight, info.row_bytes()).unwrap();
                        }
                        assert_eq!(tight, raw);
                        let stride = info.row_bytes() + 13;
                        let extent = (rows as usize - 1) * stride + info.row_bytes();
                        let mut padded = vec![0xa5; extent];
                        #[cfg(feature = "std")]
                        decode_into_threads(&file, &mut padded, stride, threads).unwrap();
                        #[cfg(not(feature = "std"))]
                        decode_into(&file, &mut padded, stride).unwrap();
                        for (got, expected) in
                            padded.chunks(stride).zip(raw.chunks(info.row_bytes()))
                        {
                            assert_eq!(&got[..info.row_bytes()], expected);
                            assert!(got[info.row_bytes()..].iter().all(|&v| v == 0xa5));
                        }
                    }
                }
            }
        }
    }
}

#[cfg(feature = "std")]
#[test]
fn large_raw_bands_keep_partial_stripes_padding_and_checksums() {
    let info = ImageInfo::new(1025, 8209, Channels::Gray, SampleType::F32);
    let raw = source(&info, 2);
    assert!(raw.len() > 32 * 1024 * 1024);
    for checksum in [false, true] {
        let mut cfg = imij::ImiEncoderConfig::quick();
        cfg = cfg.with_checksum(checksum);
        let file = cfg
            .encode_legacy_samples(
                imij::ImageRef::new(&raw, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        let file = add_pixel_checksum(file).unwrap();
        let data = &file[common::data_payload(&file)];
        let units = common::lossless_payload(data, &info);
        assert_eq!(units.records.len(), 129);
        assert!(units.records.iter().all(|r| data[r.start] == 0));
        for threads in [1, 2, 4, 8] {
            let mut tight = vec![0xa5; raw.len()];
            decode_into_threads(&file, &mut tight, info.row_bytes(), threads).unwrap();
            assert_eq!(tight, raw);
            let stride = info.row_bytes() + 17;
            let extent = (info.height as usize - 1) * stride + info.row_bytes();
            let mut padded = vec![0xa5; extent];
            decode_into_threads(&file, &mut padded, stride, threads).unwrap();
            for (got, expected) in padded.chunks(stride).zip(raw.chunks(info.row_bytes())) {
                assert_eq!(&got[..info.row_bytes()], expected);
                assert!(got[info.row_bytes()..].iter().all(|&v| v == 0xa5));
            }
        }
    }
}

#[cfg(feature = "std")]
#[test]
fn rgb_units_match_independent_rows_and_padding() {
    // Includes paired units, an odd unpaired unit, and a short second unit.
    for width in [1, 2, 61, 62, 63, 127] {
        for height in [8, 9, 10, 17] {
            let info = ImageInfo::new(width, height, Channels::Rgb, SampleType::U8);
            for kind in [0, 1, 2] {
                let raw = source(&info, kind);
                let mut cfg = imij::ImiEncoderConfig::quick();
                cfg = cfg.tune_lossless(|t| {
                    t.stripe_rows = Some(5);
                });
                cfg = cfg.with_checksum(true);
                let file = add_pixel_checksum(
                    cfg.encode_samples(
                        imij::ImageRef::new(&raw, info.row_bytes(), &info),
                        &Metadata::default(),
                    )
                    .unwrap(),
                )
                .unwrap();
                let mut tight = vec![0; raw.len()];
                decode_into_threads(&file, &mut tight, info.row_bytes(), 1).unwrap();
                assert_eq!(tight, raw);
                let mut stream = StreamDecoder::new(&file).unwrap();
                let mut row = vec![0; info.row_bytes()];
                for expected in raw.chunks(info.row_bytes()) {
                    assert!(stream.next_row(&mut row).unwrap());
                    assert_eq!(row, expected);
                }
                assert!(!stream.next_row(&mut row).unwrap());
                let stride = info.row_bytes() + 7;
                let mut padded = vec![0xa5; stride * height as usize];
                decode_into_threads(&file, &mut padded, stride, 1).unwrap();
                for (got, expected) in padded.chunks(stride).zip(raw.chunks(info.row_bytes())) {
                    assert_eq!(&got[..info.row_bytes()], expected);
                    assert!(got[info.row_bytes()..].iter().all(|&v| v == 0xa5));
                }
            }
        }
    }
}
#[test]
fn bounded_group_output_and_backpressure() {
    use imij::stream_encode::GroupEncodeSession;
    let info = ImageInfo::new(23, 13, Channels::Rgba, SampleType::F32);
    let raw = source(&info, 2);
    let mut enc = GroupEncodeSession::new(
        info.clone(),
        &Metadata::default(),
        &imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(8, 4, 2 << 20)))
            .with_quick(imij::QuickBitSmarts::Off),
    )
    .unwrap();
    let mut file = Vec::new();
    while !enc.is_finished() {
        if !enc.output().is_empty() {
            let n = enc.output().len().min(3);
            file.extend_from_slice(&enc.output()[..n]);
            enc.consume_output(n).unwrap();
        } else {
            let g = enc.next_group().unwrap();
            let start = g.y as usize * info.row_bytes() + g.x as usize * info.bytes_per_pixel();
            enc.push_group(&raw[start..], info.row_bytes()).unwrap();
        }
    }
    enc.finish().unwrap();
    routes(&file, &info, &raw);
}
#[test]
fn session_budget_denies_before_output() {
    let info = ImageInfo::new(17, 19, Channels::Rgb, SampleType::U8);
    let raw = source(&info, 1);
    let file = imij::ImiEncoderConfig::quick()
        .encode_samples(
            imij::ImageRef::new(&raw, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let mut session = DecodeSession::new(DecodePolicy::default().with_max_work_units(1));
    let mut out = vec![0xa5; raw.len()];
    assert_eq!(
        session.decode_into(&file, &mut out, info.row_bytes()),
        Err(Error::TooLarge)
    );
    assert!(out.iter().all(|&v| v == 0xa5));
}

#[test]
fn animation_crops_empty_frames_and_pages_keep_exact_canvas() {
    for policy in [
        QuickBitSmarts::Off,
        QuickBitSmarts::Packed,
        QuickBitSmarts::Packets,
    ] {
        let info = ImageInfo::new(31, 17, Channels::Rgba, SampleType::U8);
        let first = source(&info, 1);
        let mut second = first.clone();
        second[8 * info.row_bytes() + 12..8 * info.row_bytes() + 16].copy_from_slice(&[1, 2, 3, 4]);
        let frames = [first.clone(), second.clone(), second, first];
        let mut cfg = imij::ImiEncoderConfig::quick();
        cfg = cfg.with_quick(policy);
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some(5);
        });
        let mut enc = FrameEncoder::animation(
            info.clone(),
            Metadata::default(),
            FrameConfig::new(cfg.clone()),
        );
        for frame in &frames {
            enc.push_frame(frame, info.row_bytes(), FrameDuration { num: 1, den: 30 })
                .unwrap();
        }
        let file = add_pixel_checksum(enc.finish().unwrap()).unwrap();
        let mut d = FrameDecoder::from_owned(file, &Limits::DEFAULT).unwrap();
        let mut canvas = vec![0; frames[0].len()];
        for expected in &frames {
            d.next_frame().unwrap().unwrap();
            d.decode_into(&mut canvas, info.row_bytes(), 1).unwrap();
            assert_eq!(&canvas, expected);
            assert_eq!(d.pixel_checksum_status(), PixelChecksumStatus::Verified);
        }
        assert!(d.next_frame().unwrap().is_none());
        let mut pages =
            FrameEncoder::pages(info.clone(), Metadata::default(), FrameConfig::new(cfg));
        for frame in &frames {
            pages.push_page(frame, info.row_bytes(), &info).unwrap();
        }
        let file = pages.finish().unwrap();
        let mut d = FrameDecoder::new(&file).unwrap();
        for expected in &frames {
            d.next_frame().unwrap().unwrap();
            d.decode_into(&mut canvas, info.row_bytes(), 1).unwrap();
            assert_eq!(&canvas, expected);
        }
    }
}

#[cfg(feature = "std")]
#[test]
fn indexed_append_keeps_old_units_and_matches_fresh_file() {
    for policy in [
        QuickBitSmarts::Off,
        QuickBitSmarts::Packed,
        QuickBitSmarts::Packets,
    ] {
        use imij::stream_encode::{AppendRowsEncoder, IndexedGroupEncoder};
        use std::io::Cursor;
        let info = ImageInfo::new(17, 12, Channels::Rgb, SampleType::U16);
        let raw = source(&info, 1);
        let cfg = imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(7, 4, 2 << 20)))
            .with_quick(policy);
        let fixed = |h| {
            let mut i = info.clone();
            i.height = h;
            let mut e = IndexedGroupEncoder::new(
                Vec::new(),
                Cursor::new(Vec::new()),
                i,
                &Metadata::default(),
                &cfg,
                4096,
            )
            .unwrap();
            while let Some(g) = e.next_group() {
                e.write_group(
                    &raw[g.y as usize * info.row_bytes() + g.x as usize * info.bytes_per_pixel()..],
                    info.row_bytes(),
                )
                .unwrap();
            }
            e.finish().unwrap().0
        };
        let before = fixed(8);
        let mut e = AppendRowsEncoder::new(
            Cursor::new(before),
            Vec::new(),
            Cursor::new(Vec::new()),
            4,
            &cfg,
            4096,
            DecodePolicy::default(),
        )
        .unwrap();
        while let Some(g) = e.next_group() {
            e.write_group(
                &raw[g.y as usize * info.row_bytes() + g.x as usize * info.bytes_per_pixel()..],
                info.row_bytes(),
            )
            .unwrap();
        }
        let file = e.finish().unwrap().0;
        assert_eq!(file, fixed(12));
        routes(&file, &info, &raw);
    }
}

#[test]
fn zencodec_configuration_progress_and_color_storage() {
    for policy in [
        QuickBitSmarts::Off,
        QuickBitSmarts::Packed,
        QuickBitSmarts::Packets,
    ] {
        use how_far::{Complete, Outcome, Pulse};
        use how_far_along::{FnPulse, Status};
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
        use zenpixels::PixelSlice;
        for &descriptor in imij::ImiEncoderConfig::supported_descriptors() {
            let row = 13 * descriptor.bytes_per_pixel();
            let input: Vec<u8> = (0..row * 7).map(|i| (i * 79) as u8).collect();
            let pixels = PixelSlice::new(&input, 13, 7, row, descriptor).unwrap();
            let pulse = FnPulse::new("quick", |_| Ok(()));
            let observer = pulse.observer();
            let result = {
                let mut c = imij::ImiEncoderConfig::quick();
                c = c.with_quick(policy);
                c
            }
            .job()
            .with_progress(pulse.share().unwrap())
            .encoder()
            .unwrap()
            .encode(pixels);
            let file = pulse.complete(result).unwrap();
            assert_eq!(decode(file.data()).unwrap().1, input);
            assert_eq!(
                observer.snapshot().status,
                Status::Finished(Outcome::Succeeded)
            );
            assert_eq!(observer.snapshot().children[0].completed, 1);
        }
    }
}

#[test]
fn quick_zencodec_rows_match_whole_group_encoding() {
    for policy in [
        QuickBitSmarts::Off,
        QuickBitSmarts::Packed,
        QuickBitSmarts::Packets,
    ] {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
        use zenpixels::{PixelDescriptor, PixelSlice};
        for descriptor in [
            PixelDescriptor::RGB8_SRGB,
            PixelDescriptor::RGB16_SRGB,
            PixelDescriptor::RGBF32_LINEAR,
        ] {
            let row = 19 * descriptor.bytes_per_pixel();
            let stride = row + 3 * descriptor.bytes_per_pixel();
            let input: Vec<u8> = (0..stride * 7).map(|i| (i * 79) as u8).collect();
            let cfg = imij::ImiEncoderConfig::quick()
                .with_quick(policy)
                .with_row_groups(Some(imij::RowGroups::new(5, 1, 2 << 20)));
            let full = cfg
                .clone()
                .job()
                .encoder()
                .unwrap()
                .encode(PixelSlice::new(&input, 19, 7, stride, descriptor).unwrap())
                .unwrap();
            let expected: Vec<u8> = input
                .chunks_exact(stride)
                .flat_map(|r| r[..row].iter().copied())
                .collect();
            assert_eq!(decode(full.data()).unwrap().1, expected);
            for batch in [1, 2, 4, 7] {
                let mut encoder = cfg.clone().job().with_canvas_size(19, 7).encoder().unwrap();
                let mut y = 0;
                while y < 7 {
                    let rows = batch.min(7 - y);
                    encoder
                        .push_rows(
                            PixelSlice::new(
                                &input[y as usize * stride..],
                                19,
                                rows,
                                stride,
                                descriptor,
                            )
                            .unwrap(),
                        )
                        .unwrap();
                    y += rows;
                }
                assert_eq!(encoder.finish().unwrap().data(), full.data());
            }
        }
    }
}

#[test]
fn significant_bits_and_padding_declarations_do_not_discard_storage() {
    for policy in [
        QuickBitSmarts::Off,
        QuickBitSmarts::Packed,
        QuickBitSmarts::Packets,
    ] {
        for bits in 2..=7 {
            for sample in [SampleType::U8, SampleType::U16] {
                let mut info = ImageInfo::new(37, 11, Channels::Rgba, sample);
                info.significant_bits = [bits; 4];
                info.alpha_mode = AlphaMode::Undefined;
                // Keep bits outside the declaration too: significance is metadata,
                // not authorization for a lossless encoder to clear caller storage.
                let raw = source(&info, 2);
                let file = {
                    let mut c = imij::ImiEncoderConfig::quick();
                    c = c.with_quick(policy);
                    c
                }
                .encode_samples(
                    imij::ImageRef::new(&raw, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap();
                let (decoded, pixels) = decode(&file).unwrap();
                assert_eq!(decoded, info);
                assert_eq!(pixels, raw);
            }
        }
    }
}

#[test]
fn bit_smart_policies_preserve_all_storage_bits_and_routes() {
    let mut samples = vec![
        SampleType::U8,
        SampleType::U16,
        SampleType::F16,
        SampleType::F32,
    ];
    if cfg!(feature = "extended") {
        samples.extend([
            SampleType::I8,
            SampleType::I16,
            SampleType::U32,
            SampleType::I32,
            SampleType::U64,
            SampleType::I64,
            SampleType::F64,
        ]);
    }
    for bits in [QuickBitSmarts::Packed, QuickBitSmarts::Packets] {
        for sample in &samples {
            for channels in [
                Channels::Gray,
                Channels::GrayAlpha,
                Channels::Rgb,
                Channels::Rgba,
            ] {
                let info = ImageInfo::new(33, 5, channels, *sample);
                let raw: Vec<u8> = (0..info.checked_buffer_len().unwrap())
                    .map(|i| {
                        let byte = i % info.bytes_per_pixel();
                        let pixel = i / info.bytes_per_pixel();
                        if byte % 4 == 3 {
                            0xa5
                        } else {
                            (((pixel / 3 + byte) % 16) as u8) << 3 | 2
                        }
                    })
                    .collect();
                let stride = info.row_bytes() + 7;
                let mut padded = vec![0x67; stride * info.height as usize];
                for (dst, src) in padded
                    .chunks_exact_mut(stride)
                    .zip(raw.chunks_exact(info.row_bytes()))
                {
                    dst[..src.len()].copy_from_slice(src);
                }
                let mut cfg = imij::ImiEncoderConfig::quick();
                cfg = cfg.tune_lossless(|t| {
                    t.stripe_rows = Some(3);
                });
                cfg = cfg.with_column_groups(Some(17));
                cfg = cfg.with_quick(bits);
                let file = cfg
                    .encode_samples(
                        imij::ImageRef::new(&padded, stride, &info),
                        &Metadata::default(),
                    )
                    .unwrap();
                assert!(
                    header(&file)
                        .unwrap()
                        .features
                        .contains(Feature::QuickBitSmarts)
                );
                routes(&file, &info, &raw);
                for threads in [1, 2, 4, 8] {
                    cfg = cfg.with_threads(threads);
                    assert_eq!(
                        cfg.encode_samples(
                            imij::ImageRef::new(&raw, info.row_bytes(), &info),
                            &Metadata::default()
                        )
                        .unwrap(),
                        file
                    );
                }
            }
        }
    }
}
#[test]
fn bit_smart_float_special_values_are_storage_exact() {
    for bits in [QuickBitSmarts::Packed, QuickBitSmarts::Packets] {
        let info = ImageInfo::new(33, 5, Channels::Rgba, SampleType::F32);
        let values = [
            0u32, 0x80000000, 0x7f800000, 0xff800000, 0x7fc01234, 0x7fa00001, 0xffc05678,
            0x3f800000,
        ];
        let raw: Vec<u8> = (0..33 * 5 * 4)
            .flat_map(|i| values[i % values.len()].to_ne_bytes())
            .collect();
        let mut cfg = imij::ImiEncoderConfig::quick();
        cfg = cfg.with_quick(bits);
        cfg = cfg.with_checksum(true);
        let file = cfg
            .encode_samples(
                imij::ImageRef::new(&raw, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        let file = add_pixel_checksum(file).unwrap();
        routes(&file, &info, &raw);
    }
}
#[cfg(feature = "std")]
#[test]
fn bit_smart_bounded_group_writer_and_reader_budget() {
    use imij::stream_encode::GroupEncodeSession;
    let info = ImageInfo::new(33, 5, Channels::Gray, SampleType::U16);
    let raw: Vec<u8> = (0..33 * 5)
        .flat_map(|i| ((i % 4) as u16 * 0x5555).to_ne_bytes())
        .collect();
    for bits in [QuickBitSmarts::Packed, QuickBitSmarts::Packets] {
        // The shared group writer has a fixed 1 MiB admission allowance.
        let denied = imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(17, 3, 128 * 1024)))
            .with_quick(bits);
        assert!(matches!(
            GroupEncodeSession::new(info.clone(), &Metadata::default(), &denied),
            Err(Error::TooLarge)
        ));
        let cfg = imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(17, 3, 2 << 20)))
            .with_quick(bits);
        let mut enc = GroupEncodeSession::new(info.clone(), &Metadata::default(), &cfg).unwrap();
        let mut file = Vec::new();
        loop {
            file.extend_from_slice(enc.output());
            let n = enc.output().len();
            enc.consume_output(n).unwrap();
            let Some(rect) = enc.next_group() else { break };
            let start =
                rect.y as usize * info.row_bytes() + rect.x as usize * info.bytes_per_pixel();
            enc.push_group(&raw[start..], info.row_bytes()).unwrap();
        }
        enc.finish().unwrap();
        assert_eq!(decode(&file).unwrap().1, raw);
        let mut callbacks = 0;
        let mut session = DecodeSession::new(DecodePolicy::default().with_max_work_units(1));
        assert!(matches!(
            session.decode_reader(file.as_slice(), |_, _, _| {
                callbacks += 1;
                Ok(())
            }),
            Err(Error::TooLarge)
        ));
        assert_eq!(callbacks, 0);
    }
}

#[cfg(feature = "std")]
#[test]
fn bit_smart_reader_denies_specific_preprocessing_work_before_data() {
    let info = ImageInfo::new(17, 5, Channels::Gray, SampleType::U16);
    let raw: Vec<u8> = (0..17 * 5)
        .flat_map(|i| ((i % 4) as u16 * 0x5555).to_ne_bytes())
        .collect();
    let plain = imij::ImiEncoderConfig::quick()
        .encode_samples(
            imij::ImageRef::new(&raw, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let mut cfg = imij::ImiEncoderConfig::quick();
    cfg = cfg.with_quick(QuickBitSmarts::Packed);
    let smart = cfg
        .encode_samples(
            imij::ImageRef::new(&raw, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let extra = 17 * 5 * 2 * 16 + 2 * 2048;
    let mut ordinary = DecodeSession::new(DecodePolicy::default().with_max_work_units(extra - 1));
    let mut ordinary_callbacks = 0;
    ordinary
        .decode_reader(plain.as_slice(), |_, _, _| {
            ordinary_callbacks += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(ordinary_callbacks, 5);
    let mut limited = DecodeSession::new(DecodePolicy::default().with_max_work_units(extra - 1));
    let mut callbacks = 0;
    assert!(matches!(
        limited.decode_reader(smart.as_slice(), |_, _, _| {
            callbacks += 1;
            Ok(())
        }),
        Err(Error::TooLarge)
    ));
    assert_eq!(callbacks, 0);
}

#[cfg(feature = "std")]
#[test]
fn bit_smart_discovered_height_matches_fixed_indexed_bytes() {
    use imij::stream_encode::{GrowingHeightGroupEncoder, IndexedGroupEncoder};
    use std::io::Cursor;
    let info = ImageInfo::new(33, 5, Channels::Gray, SampleType::U16);
    let raw: Vec<u8> = (0..165)
        .flat_map(|i| ((i % 4) as u16 * 0x5555).to_ne_bytes())
        .collect();
    for bits in [QuickBitSmarts::Packed, QuickBitSmarts::Packets] {
        let cfg = imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(17, 3, 2 << 20)))
            .with_quick(bits);
        let mut fixed = IndexedGroupEncoder::new(
            Vec::new(),
            Cursor::new(Vec::new()),
            info.clone(),
            &Metadata::default(),
            &cfg,
            4096,
        )
        .unwrap();
        let mut unknown = info.clone();
        unknown.height = 0;
        let mut growing = GrowingHeightGroupEncoder::new(
            Vec::new(),
            Cursor::new(Vec::new()),
            Cursor::new(Vec::new()),
            unknown,
            &Metadata::default(),
            &cfg,
            128 << 10,
            4096,
        )
        .unwrap();
        while let Some(rect) = fixed.next_group() {
            if rect.y == 3 && rect.x == 0 {
                growing.set_final_height(5).unwrap();
            }
            assert_eq!(growing.next_group(), Some(rect));
            let start =
                rect.y as usize * info.row_bytes() + rect.x as usize * info.bytes_per_pixel();
            growing
                .write_group(&raw[start..], info.row_bytes())
                .unwrap();
            fixed.write_group(&raw[start..], info.row_bytes()).unwrap();
        }
        assert_eq!(growing.next_group(), None);
        let (expected, _) = fixed.finish().unwrap();
        let (file, _, _) = growing.finish().unwrap();
        assert_eq!(file, expected);
        routes(&file, &info, &raw);
    }
}
