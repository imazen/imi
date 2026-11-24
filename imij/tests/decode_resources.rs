//! Public bounded decoding regressions using small valid hostile layouts.
//! The ordinary reader, stream and frame APIs retain their own contracts.
mod common;
use common::{put_chunk, put_varint, read_varint};
use core::sync::atomic::{AtomicUsize, Ordering};
use imij::decode_resources::{DecodePolicy, DecodeSession};
use imij::{
    Channels, Error, ImageInfo, ImiEncoderConfig, Limits, LossyEntropy, Metadata, SampleType,
};

fn pixels(info: &ImageInfo) -> Vec<u8> {
    (0..info.checked_buffer_len().unwrap())
        .map(|i| (i.wrapping_mul(37) ^ (i / 17).wrapping_mul(91)) as u8)
        .collect()
}

fn lossless(info: &ImageInfo, cfg: &ImiEncoderConfig) -> (Vec<u8>, Vec<u8>) {
    let px = pixels(info);
    let file = cfg
        .encode_samples(
            imij::ImageRef::new(&px, info.row_bytes(), info),
            &Metadata::default(),
        )
        .unwrap();
    (file, px)
}

fn legacy_lossless(info: &ImageInfo, cfg: &ImiEncoderConfig) -> (Vec<u8>, Vec<u8>) {
    let px = pixels(info);
    let file = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), info),
            &Metadata::default(),
        )
        .unwrap();
    (file, px)
}

fn assert_denied_without_output(file: &[u8], policy: DecodePolicy) {
    let info = imij::header(file).unwrap().info;
    let stride = info.row_bytes() + 13;
    let mut output = vec![0xA5; stride * info.height as usize];
    let before = output.clone();
    assert!(matches!(
        DecodeSession::new(policy).decode_into(file, &mut output, stride),
        Err(Error::TooLarge)
    ));
    assert_eq!(output, before, "admission denial must precede pixel writes");
}

fn assert_parity(file: &[u8], expected: &[u8]) {
    let mut session = DecodeSession::new(DecodePolicy::default());
    let (info, actual) = session.decode(file).unwrap();
    assert_eq!(actual, expected);
    let stride = info.row_bytes() + 13;
    let mut output = vec![0xA5; stride * info.height as usize];
    session.decode_into(file, &mut output, stride).unwrap();
    for (row, expected) in output
        .chunks_exact(stride)
        .zip(expected.chunks_exact(info.row_bytes()))
    {
        assert_eq!(&row[..info.row_bytes()], expected);
        assert!(row[info.row_bytes()..].iter().all(|&b| b == 0xA5));
    }
}

#[test]
fn identity_u16_does_not_require_an_expansion_table_budget() {
    let info = ImageInfo::new(64, 1, Channels::Gray, SampleType::U16);
    // Odd values spanning the full range prevent short Shift/Scale/Replicate
    // mappings. Identity output needs no 256 KiB expansion table.
    let pixels: Vec<u8> = (0..64u16).flat_map(|i| (i * 997).to_ne_bytes()).collect();
    let config = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.palette = false;
        });
        cfg
    };
    let file = config
        .encode_samples(
            imij::ImageRef::new(&pixels, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let mut session =
        DecodeSession::new(DecodePolicy::default().with_max_working_bytes(128 * 1024));
    let (actual_info, actual) = session.decode(&file).unwrap();
    assert_eq!(actual_info, info);
    assert_eq!(actual, pixels);
    assert!(session.usage().peak_planned_working_bytes <= 128 * 1024);
}

#[test]
fn bounded_decode_preserves_lossless_samples_and_padding() {
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
            let info = ImageInfo::new(17, 19, Channels::Rgba, sample);
            let (file, px) = lossless(&info, &{
                let mut cfg = imij::ImiEncoderConfig::default();
                cfg = cfg.tune_lossless(|t| {
                    t.lanes = Some(lanes);
                });
                cfg = cfg.tune_lossless(|t| {
                    t.stripe_rows = Some(3);
                });
                cfg = cfg.with_column_groups(Some(7));
                cfg = cfg.tune_lossless(|t| {
                    t.palette = false;
                });
                cfg
            });
            assert_parity(&file, &px);
        }
    }
}

#[test]
fn bounded_lossy_matches_legacy_and_preserves_nested_alpha() {
    for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
        let info = ImageInfo::new(17, 19, Channels::Rgba, SampleType::U8);
        let px = pixels(&info);
        // One-block-row stripes: the unit count below assumes three.
        let mut cfg = imij::ImiEncoderConfig::lossy(55.0)
            .tune_lossy(|t| t.entropy = entropy)
            .tune_lossy(|t| t.stripe_block_rows = 1);
        cfg = cfg.with_column_groups(Some(8));
        let file = cfg
            .encode_legacy_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        let expected = imij::decode(&file).unwrap().1;
        assert_parity(&file, &expected);
        for (source, output) in px
            .as_chunks::<4>()
            .0
            .iter()
            .zip(expected.as_chunks::<4>().0)
        {
            assert_eq!(source[3], output[3]);
        }
        let mut session = DecodeSession::new(DecodePolicy::default());
        session.decode(&file).unwrap();
        // Three outer stripes times three groups, nine alpha records,
        // and one DC stripe times three groups must all share the cap.
        assert_eq!(session.usage().units, 9 + 9 + 3);
        assert_eq!(
            DecodeSession::new(DecodePolicy::default().with_max_units(21))
                .decode(&file)
                .unwrap()
                .1,
            expected
        );
        assert_denied_without_output(&file, DecodePolicy::default().with_max_units(20));
    }
}

#[test]
fn exact_block_stripes_share_the_unit_cap() {
    // Text-like ink on a flat page: exact blocks for sure.
    let info = ImageInfo::new(40, 24, Channels::Rgba, SampleType::U8);
    let mut px = vec![255u8; info.checked_buffer_len().unwrap()];
    for (i, p) in px.chunks_mut(4).enumerate() {
        let (x, y) = (i % 40, i / 40);
        if x % 7 < 2 && y % 5 != 0 {
            p[..3].copy_from_slice(&[10, 20, 80]);
        }
    }
    let mut cfg = imij::ImiEncoderConfig::lossy(70.0).tune_lossy(|t| t.stripe_block_rows = 1);
    cfg = cfg.with_column_groups(Some(16));
    cfg = cfg.tune_lossy(|t| {
        t.exact_blocks = Some(3.0);
    });
    let file = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    cfg = cfg.tune_lossy(|t| {
        t.exact_blocks = Some(0.0);
    });
    let plain = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    assert_ne!(file, plain);
    let expected = imij::decode(&file).unwrap().1;
    assert_parity(&file, &expected);
    let units = |f: &[u8]| {
        let mut session = DecodeSession::new(DecodePolicy::default());
        session.decode(f).unwrap();
        session.usage().units
    };
    // Three stripes times three groups (lossy and alpha), one DC stripe
    // times three groups; the exact image's nested stripes come on top.
    assert_eq!(units(&plain), 9 + 9 + 3);
    let n = units(&file);
    assert!(n > 9 + 9 + 3, "{n} units");
    assert_eq!(
        DecodeSession::new(DecodePolicy::default().with_max_units(n))
            .decode(&file)
            .unwrap()
            .1,
        expected
    );
    assert_denied_without_output(&file, DecodePolicy::default().with_max_units(n - 1));
}

#[test]
fn physical_lane_padding_is_charged_separately_from_output() {
    let info = ImageInfo::new(4097, 1, Channels::Gray, SampleType::U8);
    let mut cfg = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.palette = false;
        });
        cfg = cfg.with_column_groups(Some(0));
        cfg
    };
    cfg = cfg.tune_lossless(|t| {
        t.lanes = Some(16);
    });
    let (compact, px) = lossless(&info, &cfg);
    let mut session = DecodeSession::new(DecodePolicy::default());
    session.decode(&compact).unwrap();
    let compact_working = session.usage().peak_planned_working_bytes;
    cfg = cfg.tune_lossless(|t| {
        t.lanes = Some(1);
    });
    let (padded, _) = lossless(&info, &cfg);
    let mut session = DecodeSession::new(DecodePolicy::default());
    session.decode(&padded).unwrap();
    assert!(session.usage().peak_planned_working_bytes > compact_working);
    assert_denied_without_output(
        &padded,
        DecodePolicy::default().with_max_working_bytes(compact_working),
    );
    let mut compact_session =
        DecodeSession::new(DecodePolicy::default().with_max_working_bytes(compact_working));
    for _ in 0..2 {
        assert_eq!(compact_session.decode(&compact).unwrap().1, px);
        assert_eq!(
            compact_session.usage().peak_planned_working_bytes,
            compact_working
        );
    }
}

#[test]
fn one_pixel_units_are_counted_before_output_mutation() {
    let info = ImageInfo::new(17, 19, Channels::Gray, SampleType::U8);
    let (file, px) = lossless(&info, &{
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some(1);
        });
        cfg = cfg.with_column_groups(Some(1));
        cfg = cfg.tune_lossless(|t| {
            t.palette = false;
        });
        cfg
    });
    let units = u64::from(info.width) * u64::from(info.height);
    let mut session = DecodeSession::new(DecodePolicy::default().with_max_units(units));
    assert_eq!(session.decode(&file).unwrap().1, px);
    assert_eq!(session.usage().units, units);
    assert_denied_without_output(&file, DecodePolicy::default().with_max_units(units - 1));
}

#[test]
fn open_data_charges_both_actual_model_and_record_visits() {
    let info = ImageInfo::new(17, 19, Channels::Gray, SampleType::U8);
    let (file, px) = legacy_lossless(&info, &{
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some(3);
        });
        cfg = cfg.tune_lossless(|t| {
            t.palette = false;
        });
        cfg
    });
    let mut at = 8;
    let header_bytes = read_varint(&file, &mut at) as usize;
    at += header_bytes;
    assert_eq!(&file[at..at + 4], b"DATA");
    at += 4;
    let length_start = at;
    read_varint(&file, &mut at);
    let mut open = file[..length_start].to_vec();
    open.push(0);
    open.extend(&file[at..]);
    let mut bounded_session = DecodeSession::new(DecodePolicy::default());
    bounded_session.decode(&file).unwrap();
    let mut open_session = DecodeSession::new(DecodePolicy::default());
    assert_eq!(open_session.decode(&open).unwrap().1, px);
    // Open DATA first parses to locate the next chunk, then retains the
    // validated model for reconstruction. Admission charges both visits.
    assert_eq!(
        open_session.usage().units,
        2 * bounded_session.usage().units
    );
    assert!(open_session.usage().work_units > bounded_session.usage().work_units);
    assert_denied_without_output(
        &open,
        DecodePolicy::default().with_max_units(open_session.usage().units - 1),
    );
}

#[test]
fn chunks_and_metadata_after_data_share_cumulative_budgets() {
    let info = ImageInfo::new(1, 1, Channels::Gray, SampleType::U8);
    let px = pixels(&info);
    let meta = Metadata {
        chunks: (0..11)
            .map(|_| imij::Chunk {
                tag: *b"teST",
                data: vec![42; 7],
            })
            .collect(),
        ..Default::default()
    };
    let mut file = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(imij::ImageRef::new(&px, 1, &info), &meta)
        .unwrap();
    for _ in 0..22 {
        put_chunk(&mut file, b"teST", &[42; 7]);
    }
    let mut session = DecodeSession::new(DecodePolicy::default());
    assert_eq!(session.decode(&file).unwrap().1, px);
    assert_eq!(session.usage().metadata_bytes, 33 * 7);
    assert_eq!(session.usage().chunks, 34);
    assert_denied_without_output(
        &file,
        DecodePolicy::default().with_max_metadata_bytes(33 * 7 - 1),
    );
    assert_denied_without_output(&file, DecodePolicy::default().with_max_chunks(33));
    let mut empty = legacy_lossless(&info, &imij::ImiEncoderConfig::default()).0;
    for _ in 0..33 {
        put_chunk(&mut empty, b"teST", &[]);
    }
    assert_denied_without_output(&empty, DecodePolicy::default().with_max_chunks(33));
    let mut session = DecodeSession::new(DecodePolicy::default().with_max_metadata_bytes(0));
    session.decode(&empty).unwrap();
    assert_eq!(session.usage().metadata_bytes, 0);
    assert_eq!(session.usage().chunks, 34);
}

#[test]
fn borrowed_previews_frames_and_ignored_planes_still_count_as_chunks() {
    let info = ImageInfo::new(1, 1, Channels::Gray, SampleType::U8);
    let (mut file, px) = legacy_lossless(&info, &imij::ImiEncoderConfig::default());
    // The ordinary still-image decoder borrows these descriptors or ignores
    // auxiliary plane records; it does not decode their nested payloads.
    for tag in [b"pRVW", b"fRAM", b"pDAT", b"pIDX", b"sLOC"] {
        put_chunk(&mut file, tag, &[42; 7]);
    }
    let mut session = DecodeSession::new(DecodePolicy::default().with_max_chunks(6));
    assert_eq!(session.decode(&file).unwrap().1, px);
    assert_eq!(session.usage().chunks, 6);
    assert_denied_without_output(&file, DecodePolicy::default().with_max_chunks(5));
}

#[test]
fn compressed_metadata_is_charged_at_expanded_size() {
    let info = ImageInfo::new(1, 1, Channels::Gray, SampleType::U8);
    let (mut file, px) = legacy_lossless(&info, &imij::ImiEncoderConfig::default());
    // Seeded LZ4: 8 literal bytes, a 504-byte offset-1 match, 8 final
    // literals. The 520-byte expansion is far larger than this record.
    let mut packed = b"iCCP".to_vec();
    packed.extend([1, 1]); // method and immutable seed
    put_varint(&mut packed, 520);
    packed.extend([0x8F]);
    packed.extend([42; 8]);
    packed.extend([1, 0, 255, 230]); // 4 + 15 + 255 + 230 = 504
    packed.push(0x80);
    packed.extend([42; 8]);
    assert!(packed.len() < 40);
    put_chunk(&mut file, b"mCMP", &packed);
    assert_eq!(imij::decode(&file).unwrap().1, px);
    let mut session = DecodeSession::new(DecodePolicy::default().with_max_metadata_bytes(520));
    session.decode(&file).unwrap();
    assert_eq!(session.usage().metadata_bytes, 520);
    assert_denied_without_output(&file, DecodePolicy::default().with_max_metadata_bytes(519));
}

#[test]
fn gainmap_fallback_reserves_transient_and_retained_payloads() {
    let info = ImageInfo::new(1, 1, Channels::Gray, SampleType::U8);
    let (mut file, px) = legacy_lossless(&info, &imij::ImiEncoderConfig::default());
    // Unknown gain-map version is kept as an ordinary ancillary chunk.
    put_chunk(&mut file, b"gMAP", &[255, 42]);
    let mut session = DecodeSession::new(DecodePolicy::default().with_max_metadata_bytes(4));
    assert_eq!(session.decode(&file).unwrap().1, px);
    assert_eq!(session.usage().metadata_bytes, 4);
    assert_denied_without_output(&file, DecodePolicy::default().with_max_metadata_bytes(3));
}

#[test]
fn all_scalar_budget_boundaries_reject_before_output_writes() {
    let info = ImageInfo::new(17, 19, Channels::Gray, SampleType::U8);
    let (file, px) = lossless(&info, &{
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.palette = false;
        });
        cfg
    });
    let mut probe = DecodeSession::new(DecodePolicy::default());
    probe.decode(&file).unwrap();
    let usage = probe.usage();
    let work = usage.work_units;
    let working = usage.peak_planned_working_bytes;
    assert!(work > 0 && working > 0);
    let mut at = 8;
    let header_bytes = read_varint(&file, &mut at);
    let bytes = file.len() as u64;
    for policy in [
        DecodePolicy::default().with_max_input_bytes(bytes - 1),
        DecodePolicy::default().with_max_header_bytes(header_bytes - 1),
        DecodePolicy::default().with_max_work_units(work - 1),
        DecodePolicy::default().with_max_working_bytes(working - 1),
        DecodePolicy::default().with_output_limits({
            let mut limits = Limits::DEFAULT;
            limits.max_pixels = 17 * 19 - 1;
            limits.max_bytes = u64::MAX;
            limits
        }),
        DecodePolicy::default().with_output_limits({
            let mut limits = Limits::DEFAULT;
            limits.max_pixels = u64::MAX;
            limits.max_bytes = px.len() as u64 - 1;
            limits
        }),
    ] {
        assert_denied_without_output(&file, policy);
    }
    let policy = DecodePolicy::default()
        .with_max_input_bytes(bytes)
        .with_max_header_bytes(header_bytes)
        .with_max_work_units(work)
        .with_max_working_bytes(working)
        .with_output_limits({
            let mut limits = Limits::DEFAULT;
            limits.max_pixels = 17 * 19;
            limits.max_bytes = px.len() as u64;
            limits
        });
    assert_eq!(DecodeSession::new(policy).decode(&file).unwrap().1, px);
}

#[test]
fn sequential_images_share_input_metadata_unit_and_work_allowances() {
    let info = ImageInfo::new(17, 19, Channels::Gray, SampleType::U8);
    let (mut file, px) = legacy_lossless(&info, &{
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.palette = false;
        });
        cfg
    });
    put_chunk(&mut file, b"teST", &[42; 7]);
    let mut probe = DecodeSession::new(DecodePolicy::default());
    probe.decode(&file).unwrap();
    let units = probe.usage().units;
    let work = probe.usage().work_units;
    for policy in [
        DecodePolicy::default().with_max_input_bytes(2 * file.len() as u64 - 1),
        DecodePolicy::default().with_max_metadata_bytes(13),
        DecodePolicy::default().with_max_chunks(3),
        DecodePolicy::default().with_max_units(2 * units - 1),
        DecodePolicy::default().with_max_work_units(2 * work - 1),
    ] {
        let mut session = DecodeSession::new(policy);
        assert_eq!(session.decode(&file).unwrap().1, px);
        let stride = info.row_bytes() + 13;
        let mut output = vec![0xA5; stride * info.height as usize];
        assert!(matches!(
            session.decode_into(&file, &mut output, stride),
            Err(Error::TooLarge)
        ));
        assert!(output.iter().all(|&b| b == 0xA5));
    }
}

#[test]
fn truncation_is_an_error_for_every_bounded_prefix() {
    let info = ImageInfo::new(3, 3, Channels::Gray, SampleType::U8);
    let (file, _) = lossless(&info, &{
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.palette = false;
        });
        cfg
    });
    for end in 0..file.len() {
        let mut session = DecodeSession::new(DecodePolicy::default());
        let mut output = vec![0xA5; 9];
        assert!(
            session.decode_into(&file[..end], &mut output, 3).is_err(),
            "prefix {end}"
        );
        assert_eq!(
            output, [0xA5; 9],
            "incomplete record layout at prefix {end}"
        );
    }
}

#[test]
fn cancellation_is_checked_before_admission_and_during_chunk_scan() {
    let info = ImageInfo::new(1, 1, Channels::Gray, SampleType::U8);
    let (mut file, _) = lossless(&info, &imij::ImiEncoderConfig::default());
    for _ in 0..33 {
        put_chunk(&mut file, b"teST", &[]);
    }
    let mut output = [0xA5];
    let mut session = DecodeSession::with_cancel(DecodePolicy::default(), || true);
    assert!(matches!(
        session.decode_into(&file, &mut output, 1),
        Err(Error::Cancelled)
    ));
    assert_eq!(output, [0xA5]);
    let checkpoints = AtomicUsize::new(0);
    let mut session = DecodeSession::with_cancel(DecodePolicy::default(), || {
        let next = checkpoints.fetch_add(1, Ordering::Relaxed) + 1;
        next >= 12
    });
    assert!(matches!(
        session.decode_into(&file, &mut output, 1),
        Err(Error::Cancelled)
    ));
    assert_eq!(checkpoints.load(Ordering::Relaxed), 12);
    assert_eq!(output, [0xA5]);
}

#[test]
fn cancellation_can_interrupt_reconstruction_and_leave_padding_intact() {
    let info = ImageInfo::new(17, 19, Channels::Gray, SampleType::U8);
    let (file, _) = lossless(&info, &{
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some(1);
        });
        cfg = cfg.tune_lossless(|t| {
            t.palette = false;
        });
        cfg
    });
    let checkpoints = AtomicUsize::new(0);
    let stride = info.row_bytes() + 13;
    let mut output = vec![0xA5; stride * info.height as usize];
    let mut probe = DecodeSession::with_cancel(DecodePolicy::default(), || {
        checkpoints.fetch_add(1, Ordering::Relaxed);
        false
    });
    probe.decode_into(&file, &mut output, stride).unwrap();
    let all_checkpoints = checkpoints.load(Ordering::Relaxed);
    assert!(all_checkpoints > info.height as usize);
    let stop_at = all_checkpoints - info.height as usize / 2;
    checkpoints.store(0, Ordering::Relaxed);
    let mut session = DecodeSession::with_cancel(DecodePolicy::default(), || {
        checkpoints.fetch_add(1, Ordering::Relaxed) + 1 >= stop_at
    });
    output.fill(0xA5);
    assert!(matches!(
        session.decode_into(&file, &mut output, stride),
        Err(Error::Cancelled)
    ));
    assert!(
        output
            .chunks_exact(stride)
            .all(|row| row[info.row_bytes()..].iter().all(|&b| b == 0xA5))
    );
    assert!(
        output
            .chunks_exact(stride)
            .any(|row| row[..info.row_bytes()].iter().any(|&b| b != 0xA5)),
        "must reach reconstruction before cancellation"
    );
}

#[test]
fn lossy_cancellation_reaches_nested_alpha_rows() {
    let info = ImageInfo::new(17, 73, Channels::Rgba, SampleType::U8);
    let file = imij::ImiEncoderConfig::lossy(55.0)
        .encode_samples(
            imij::ImageRef::new(&pixels(&info), info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let stride = info.row_bytes() + 13;
    let mut output = vec![0xA5; stride * info.height as usize];
    let checkpoints = AtomicUsize::new(0);
    let mut probe = DecodeSession::with_cancel(DecodePolicy::default(), || {
        checkpoints.fetch_add(1, Ordering::Relaxed);
        false
    });
    probe.decode_into(&file, &mut output, stride).unwrap();
    let all_checkpoints = checkpoints.load(Ordering::Relaxed);
    // Row checks inside lossless alpha add at least one per source row.
    assert!(all_checkpoints > info.height as usize);
    checkpoints.store(0, Ordering::Relaxed);
    let mut session = DecodeSession::with_cancel(DecodePolicy::default(), || {
        checkpoints.fetch_add(1, Ordering::Relaxed) + 1 >= all_checkpoints - 5
    });
    output.fill(0xA5);
    assert!(matches!(
        session.decode_into(&file, &mut output, stride),
        Err(Error::Cancelled)
    ));
    assert!(
        output
            .chunks_exact(stride)
            .all(|row| row[info.row_bytes()..].iter().all(|&b| b == 0xA5))
    );
    assert!(
        output
            .chunks_exact(stride)
            .any(|row| row[..info.row_bytes()].iter().any(|&b| b != 0xA5))
    );
}
