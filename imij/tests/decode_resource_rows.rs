//! Bounded synchronous rows reuse the real codec streams and caller storage.
mod common;
use common::{put_chunk, put_varint, read_varint};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use imij::decode_resources::{DecodePolicy, DecodeSession, DecodeUsage};
use imij::{
    Channels, Error, ImageInfo, ImiEncoderConfig, Limits, LossyEntropy, Metadata, SampleType,
};

fn pixels(info: &ImageInfo) -> Vec<u8> {
    let count = info.width as usize * info.height as usize * info.channels.count();
    let mut px = Vec::new();
    for i in 0..count {
        let value = (i as u32).wrapping_mul(1_003_003) ^ ((i / 13) as u32).wrapping_mul(71_911);
        match info.sample {
            SampleType::U8 => px.push(value as u8),
            SampleType::U16 => px.extend((value as u16).to_ne_bytes()),
            SampleType::F16 => px.extend((0x3000u16 + (i % 1024) as u16).to_ne_bytes()),
            SampleType::F32 => px.extend((0.25f32 + (i % 32) as f32 / 64.0).to_ne_bytes()),
            _ => px.extend(value.to_ne_bytes()),
        }
    }
    px
}
fn encoded(info: &ImageInfo, cfg: &ImiEncoderConfig) -> Vec<u8> {
    cfg.encode_legacy_samples(
        imij::ImageRef::new(&pixels(info), info.row_bytes(), info),
        &Metadata::default(),
    )
    .unwrap()
}
fn grouped() -> ImiEncoderConfig {
    let mut cfg = imij::ImiEncoderConfig::default();
    cfg = cfg.tune_lossless(|t| {
        t.stripe_rows = Some(3);
    });
    cfg = cfg.with_column_groups(Some(7));
    cfg = cfg.tune_lossless(|t| {
        t.palette = false;
    });
    cfg
}
fn rows(file: &[u8]) -> (Vec<u8>, DecodeUsage) {
    let (info, expected) = imij::decode(file).unwrap();
    let mut stream = imij::StreamDecoder::new(file).unwrap();
    let mut stream_row = vec![0; info.row_bytes()];
    let mut row = vec![0xa5; info.row_bytes() + 19];
    let mut actual = Vec::new();
    let mut count = 0;
    let mut session = DecodeSession::new(DecodePolicy::default());
    let returned = session
        .decode_rows(file, &mut row, |got, y, r| {
            assert_eq!(got, &info);
            assert_eq!(y, count);
            assert_eq!(r.len(), info.row_bytes());
            assert!(stream.next_row(&mut stream_row).unwrap());
            assert_eq!(r, stream_row);
            actual.extend_from_slice(r);
            count += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(returned, info);
    assert_eq!(count, info.height);
    assert_eq!(actual, expected);
    assert!(row[info.row_bytes()..].iter().all(|&b| b == 0xa5));
    assert!(!stream.next_row(&mut stream_row).unwrap());
    (actual, session.usage())
}
fn deny(file: &[u8], policy: DecodePolicy) {
    let info = imij::header(file).unwrap().info;
    let mut row = vec![0xa5; info.row_bytes() + 11];
    let mut callbacks = 0;
    assert_eq!(
        DecodeSession::new(policy).decode_rows(file, &mut row, |_, _, _| {
            callbacks += 1;
            Ok(())
        }),
        Err(Error::TooLarge)
    );
    assert_eq!(callbacks, 0);
    assert!(row.iter().all(|&b| b == 0xa5));
}

#[test]
fn all_lossless_samples_groups_lanes_and_native_row_order() {
    for sample in [
        SampleType::U8,
        SampleType::U16,
        SampleType::F16,
        SampleType::U32,
        SampleType::I32,
        SampleType::F32,
    ] {
        if sample.tier() == imij::Tier::Extended && !cfg!(feature = "extended") {
            continue;
        }
        for lanes in [1, 16] {
            let info = ImageInfo::new(17, 11, Channels::Rgba, sample);
            let cfg = {
                let mut cfg = grouped();
                cfg = cfg.tune_lossless(|t| {
                    t.lanes = Some(lanes);
                });
                cfg
            };
            let file = encoded(&info, &cfg);
            assert_eq!(rows(&file).0, pixels(&info));
        }
    }
}

#[test]
fn palette_and_narrow_mapped_f32_rows_match_slice() {
    let info = ImageInfo::new(33, 7, Channels::Rgba, SampleType::U8);
    let px: Vec<u8> = (0..33 * 7)
        .flat_map(|i| {
            if i % 3 == 0 {
                [4, 8, 12, 255]
            } else {
                [9, 18, 27, 255]
            }
        })
        .collect();
    let file = {
        let mut cfg = grouped();
        cfg = cfg.tune_lossless(|t| {
            t.palette = true;
        });
        cfg
    }
    .encode_legacy_samples(
        imij::ImageRef::new(&px, info.row_bytes(), &info),
        &Metadata::default(),
    )
    .unwrap();
    assert_eq!(rows(&file).0, px);
    let info = ImageInfo::new(23, 5, Channels::Gray, SampleType::F32);
    let file = encoded(&info, &grouped());
    assert_eq!(rows(&file).0, pixels(&info));
}

#[test]
fn lossy_entropy_nested_dc_alpha_aq_and_nondefault_layouts() {
    for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
        let info = ImageInfo::new(25, 27, Channels::Rgba, SampleType::U8);
        let mut cfg = imij::ImiEncoderConfig::lossy(55.0);
        cfg = cfg.with_column_groups(Some(8));
        cfg = cfg.tune_lossy(|t| {
            t.stripe_block_rows = 2;
        });
        cfg = cfg.tune_lossy(|t| {
            t.aq_gamma = 0.5;
        });
        cfg = cfg.tune_lossy(|t| {
            t.entropy = entropy;
        });
        let file = cfg
            .encode_legacy_samples(
                imij::ImageRef::new(&pixels(&info), info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        let (out, usage) = rows(&file);
        for (a, b) in out
            .as_chunks::<4>()
            .0
            .iter()
            .zip(pixels(&info).as_chunks::<4>().0)
        {
            assert_eq!(a[3], b[3]);
        }
        assert!(usage.units > 8); // outer units plus nested DC and alpha
    }
    let info = ImageInfo::new(25, 19, Channels::GrayAlpha, SampleType::U16);
    let mut cfg = imij::ImiEncoderConfig::lossy(55.0);
    cfg = cfg.with_column_groups(Some(8));
    cfg = cfg.tune_lossy(|t| {
        t.channel_layouts = [
            imij::LossyChannelLayout::default(),
            imij::LossyChannelLayout::new(
                imij::LossyTransform::Dct4,
                imij::ChannelSampling::HalfBoth,
            ),
            imij::LossyChannelLayout::new(imij::LossyTransform::Dct2, imij::ChannelSampling::Full),
        ];
    });
    let file = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&pixels(&info), info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    rows(&file);
}

#[test]
fn lossy_native_sample_widths_and_significant_bits() {
    for sample in [
        SampleType::U8,
        SampleType::U16,
        SampleType::F16,
        SampleType::F32,
        SampleType::U32,
    ] {
        if sample.tier() == imij::Tier::Extended && !cfg!(feature = "extended") {
            continue;
        }
        let info = ImageInfo::new(17, 13, Channels::Rgba, sample);
        let mut cfg = imij::ImiEncoderConfig::lossy(50.0);
        cfg = cfg.with_column_groups(Some(8));
        let file = cfg
            .encode_legacy_samples(
                imij::ImageRef::new(&pixels(&info), info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        rows(&file);
    }
    let info = ImageInfo::new(17, 13, Channels::Rgb, SampleType::U16)
        .with_significant_bits(&[10, 12, 9])
        .unwrap();
    let px: Vec<u8> = (0..17 * 13 * 3)
        .flat_map(|i| ((i as u16 * 11) & ((1 << [10, 12, 9][i % 3]) - 1)).to_ne_bytes())
        .collect();
    let mut cfg = imij::ImiEncoderConfig::lossy(55.0);
    cfg = cfg.with_column_groups(Some(8));
    let file = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    rows(&file);
}

#[test]
fn simultaneous_group_capacity_rejects_before_rows_and_exact_boundary_passes() {
    let info = ImageInfo::new(257, 7, Channels::Rgba, SampleType::U16);
    let file = encoded(&info, &{
        let mut cfg = grouped();
        cfg = cfg.with_column_groups(Some(8));
        cfg = cfg.tune_lossless(|t| {
            t.lanes = Some(1);
        });
        cfg
    });
    let mut serial = DecodeSession::new(DecodePolicy::default());
    serial.decode(&file).unwrap();
    let (_, streamed) = rows(&file);
    let serial_bytes = serial.usage().peak_planned_working_bytes;
    let row_bytes = streamed.peak_planned_working_bytes;
    assert!(row_bytes > serial_bytes);
    deny(
        &file,
        DecodePolicy::default().with_max_working_bytes(serial_bytes),
    );
    deny(
        &file,
        DecodePolicy::default().with_max_working_bytes(row_bytes - 1),
    );
    let mut row = vec![0; info.row_bytes()];
    DecodeSession::new(DecodePolicy::default().with_max_working_bytes(row_bytes))
        .decode_rows(&file, &mut row, |_, _, _| Ok(()))
        .unwrap();
    assert_eq!(streamed.input_bytes, serial.usage().input_bytes);
    assert_eq!(streamed.units, serial.usage().units);
    assert_eq!(streamed.work_units, serial.usage().work_units);
}

#[test]
fn nested_streaming_capacity_rejects_before_callbacks() {
    let info = ImageInfo::new(257, 73, Channels::Rgba, SampleType::U16);
    let mut cfg = imij::ImiEncoderConfig::lossy(50.0);
    cfg = cfg.with_column_groups(Some(8));
    cfg = cfg.tune_lossy(|t| {
        t.stripe_block_rows = 3;
    });
    cfg = cfg.tune_lossy(|t| {
        t.aq_gamma = 0.5;
    });
    let file = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&pixels(&info), info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let (_, usage) = rows(&file);
    let mut serial = DecodeSession::new(DecodePolicy::default());
    serial.decode(&file).unwrap();
    assert!(usage.peak_planned_working_bytes > serial.usage().peak_planned_working_bytes);
    deny(
        &file,
        DecodePolicy::default().with_max_working_bytes(serial.usage().peak_planned_working_bytes),
    );
    deny(
        &file,
        DecodePolicy::default().with_max_working_bytes(usage.peak_planned_working_bytes - 1),
    );
}

#[test]
fn short_row_and_full_image_limits_precede_mutation() {
    let info = ImageInfo::new(17, 11, Channels::Gray, SampleType::U8);
    let file = encoded(&info, &grouped());
    let mut row = vec![0xa5; info.row_bytes() - 1];
    let mut calls = 0;
    let mut session = DecodeSession::new(DecodePolicy::default());
    assert!(matches!(
        session.decode_rows(&file, &mut row, |_, _, _| {
            calls += 1;
            Ok(())
        }),
        Err(Error::InvalidInput(_))
    ));
    assert_eq!(calls, 0);
    assert!(row.iter().all(|&b| b == 0xa5));
    assert_eq!(session.usage().input_bytes, file.len() as u64);
    deny(
        &file,
        DecodePolicy::default().with_output_limits({
            let mut limits = Limits::DEFAULT;
            limits.max_pixels = u64::MAX;
            limits.max_bytes = (info.row_bytes() * info.height as usize - 1) as u64;
            limits
        }),
    );
}

#[test]
fn suffix_validation_and_metadata_admission_precede_callbacks() {
    let info = ImageInfo::new(17, 5, Channels::Gray, SampleType::U8);
    let mut file = encoded(&info, &grouped());
    put_chunk(&mut file, b"teST", &[42; 7]);
    put_chunk(&mut file, b"pRVW", &[1; 3]);
    let (_, usage) = rows(&file);
    assert_eq!(usage.metadata_bytes, 7);
    assert_eq!(usage.chunks, 3);
    deny(&file, DecodePolicy::default().with_max_metadata_bytes(6));
    file.extend(b"teST");
    put_varint(&mut file, 99);
    file.push(0);
    let mut row = vec![0xa5; info.row_bytes()];
    let mut calls = 0;
    assert!(
        DecodeSession::new(DecodePolicy::default())
            .decode_rows(&file, &mut row, |_, _, _| {
                calls += 1;
                Ok(())
            })
            .is_err()
    );
    assert_eq!(calls, 0);
    assert!(row.iter().all(|&b| b == 0xa5));
}

#[test]
fn callback_error_retains_charges_and_next_call_is_independent() {
    let info = ImageInfo::new(17, 5, Channels::Gray, SampleType::U8);
    let file = encoded(&info, &grouped());
    let mut row = vec![0; info.row_bytes()];
    let mut session = DecodeSession::new(DecodePolicy::default());
    let mut seen = Vec::new();
    assert_eq!(
        session.decode_rows(&file, &mut row, |_, y, r| {
            assert_eq!(r.len(), info.row_bytes());
            seen.push(y);
            if y == 1 {
                Err(Error::InvalidInput("caller stopped"))
            } else {
                Ok(())
            }
        }),
        Err(Error::InvalidInput("caller stopped"))
    );
    assert_eq!(seen, [0, 1]);
    let failed = session.usage();
    assert!(failed.units > 0);
    let mut calls = 0;
    session
        .decode_rows(&file, &mut row, |_, _, _| {
            calls += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(calls, info.height);
    assert_eq!(session.usage().input_bytes, 2 * failed.input_bytes);
    assert_eq!(session.usage().units, 2 * failed.units);
    assert_eq!(session.usage().work_units, 2 * failed.work_units);
    assert_eq!(
        session.usage().peak_planned_working_bytes,
        failed.peak_planned_working_bytes
    );
}

#[test]
fn cumulative_each_counter_denies_the_second_call_before_rows() {
    let info = ImageInfo::new(17, 5, Channels::Gray, SampleType::U8);
    let mut file = encoded(&info, &grouped());
    put_chunk(&mut file, b"teST", &[2; 7]);
    let (_, u) = rows(&file);
    for policy in [
        DecodePolicy::default().with_max_input_bytes(u.input_bytes),
        DecodePolicy::default().with_max_chunks(u.chunks),
        DecodePolicy::default().with_max_metadata_bytes(u.metadata_bytes),
        DecodePolicy::default().with_max_units(u.units),
        DecodePolicy::default().with_max_work_units(u.work_units),
    ] {
        let mut s = DecodeSession::new(policy);
        let mut row = vec![0; info.row_bytes()];
        s.decode_rows(&file, &mut row, |_, _, _| Ok(())).unwrap();
        row.fill(0xa5);
        let mut calls = 0;
        assert_eq!(
            s.decode_rows(&file, &mut row, |_, _, _| {
                calls += 1;
                Ok(())
            }),
            Err(Error::TooLarge)
        );
        assert_eq!(calls, 0);
        assert!(row.iter().all(|&b| b == 0xa5));
    }
}

#[test]
fn cancellation_at_groups_never_delivers_an_incomplete_row() {
    let info = ImageInfo::new(65, 7, Channels::Rgba, SampleType::U8);
    let file = encoded(&info, &grouped());
    let mut row = vec![0; info.row_bytes()];
    let checks = AtomicUsize::new(0);
    let mut first_checks = 0;
    DecodeSession::with_cancel(DecodePolicy::default(), || {
        checks.fetch_add(1, Ordering::Relaxed);
        false
    })
    .decode_rows(&file, &mut row, |_, y, _| {
        if y == 0 {
            first_checks = checks.load(Ordering::Relaxed);
        }
        Ok(())
    })
    .unwrap();
    for threshold in [first_checks - 4, first_checks - 14] {
        let checks = AtomicUsize::new(0);
        let mut calls = 0;
        let mut session = DecodeSession::with_cancel(DecodePolicy::default(), || {
            checks.fetch_add(1, Ordering::Relaxed) >= threshold
        });
        assert_eq!(
            session.decode_rows(&file, &mut row, |_, _, _| {
                calls += 1;
                Ok(())
            }),
            Err(Error::Cancelled)
        );
        assert_eq!(calls, 0);
        assert!(session.usage().units > 0);
    }
    let cancelled = AtomicBool::new(false);
    let mut s = DecodeSession::with_cancel(DecodePolicy::default(), || {
        cancelled.load(Ordering::Relaxed)
    });
    let mut calls = 0;
    assert_eq!(
        s.decode_rows(&file, &mut row, |_, y, _| {
            assert_eq!(y, 0);
            calls += 1;
            cancelled.store(true, Ordering::Relaxed);
            Ok(())
        }),
        Err(Error::Cancelled)
    );
    assert_eq!(calls, 1);
    cancelled.store(false, Ordering::Relaxed);
    s.decode_rows(&file, &mut row, |_, _, _| Ok(())).unwrap();
}

#[test]
fn pre_cancel_and_open_data_repeated_visits_preserve_counters() {
    let info = ImageInfo::new(17, 5, Channels::Gray, SampleType::U8);
    let file = encoded(&info, &grouped());
    let mut row = vec![0xa5; info.row_bytes()];
    let mut s = DecodeSession::with_cancel(DecodePolicy::default(), || true);
    assert_eq!(
        s.decode_rows(&file, &mut row, |_, _, _| panic!("pre-cancel callback")),
        Err(Error::Cancelled)
    );
    assert_eq!(s.usage(), DecodeUsage::default());
    assert!(row.iter().all(|&b| b == 0xa5));
    let (_, u) = rows(&file);
    let mut at = 8;
    let n = read_varint(&file, &mut at) as usize;
    at += n;
    assert_eq!(&file[at..at + 4], b"DATA");
    at += 4;
    let length_start = at;
    read_varint(&file, &mut at);
    let mut open = file[..length_start].to_vec();
    open.push(0);
    open.extend(&file[at..]);
    let (out, open_usage) = rows(&open);
    assert_eq!(out, pixels(&info));
    assert_eq!(open_usage.units, 2 * u.units);
    assert!(open_usage.work_units > u.work_units);
}

#[test]
fn cancellation_inside_nested_alpha_and_group_rows_suppresses_callback() {
    let info = ImageInfo::new(65, 19, Channels::Rgba, SampleType::U16);
    let mut cfg = imij::ImiEncoderConfig::lossy(55.0);
    cfg = cfg.with_column_groups(Some(8));
    cfg = cfg.tune_lossy(|t| {
        t.aq_gamma = 0.5;
    });
    let file = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&pixels(&info), info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let mut row = vec![0xa5; info.row_bytes() + 11];
    let checks = AtomicUsize::new(0);
    let mut first_checks = 0;
    DecodeSession::with_cancel(DecodePolicy::default(), || {
        checks.fetch_add(1, Ordering::Relaxed);
        false
    })
    .decode_rows(&file, &mut row, |_, y, _| {
        if y == 0 {
            first_checks = checks.load(Ordering::Relaxed);
        }
        Ok(())
    })
    .unwrap();
    for offset in [4, 14] {
        row.fill(0xa5);
        let checks = AtomicUsize::new(0);
        let mut calls = 0;
        let mut session = DecodeSession::with_cancel(DecodePolicy::default(), || {
            checks.fetch_add(1, Ordering::Relaxed) >= first_checks - offset
        });
        assert_eq!(
            session.decode_rows(&file, &mut row, |_, _, _| {
                calls += 1;
                Ok(())
            }),
            Err(Error::Cancelled)
        );
        assert_eq!(calls, 0);
        assert!(session.usage().units > 0);
        assert!(row[info.row_bytes()..].iter().all(|&b| b == 0xa5));
    }
}
