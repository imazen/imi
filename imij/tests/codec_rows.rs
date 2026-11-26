//! Explicit group-row encoding preserves raw storage without buffering a raster.
use imij::stream_encode::GroupEncodeSession;
use imij::{Error, SampleType};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
use zencodec::{ResourceLimits, StopToken};
use zenpixels::{ColorContext, PixelDescriptor, PixelFormat, PixelSlice};

const WIDTH: u32 = 19;
const HEIGHT: u32 = 7;
const WORKING: usize = 16 << 20;

fn source(desc: PixelDescriptor, sample: SampleType) -> (Vec<u8>, usize, Vec<u8>) {
    let row = WIDTH as usize * desc.bytes_per_pixel();
    let stride = row + 3 * desc.bytes_per_pixel();
    let mut bytes = vec![0xee; stride * HEIGHT as usize];
    let mut tight = Vec::new();
    let patterns = [
        0u32, 0x80000000, 1, 0x3f800000, 0x7f800000, 0xff800000, 0x7f800001, 0x7fc12abc,
    ];
    let half = [0u16, 0x8000, 1, 0x3c00, 0x7c00, 0xfc00, 0x7c01, 0x7eab];
    for y in 0..HEIGHT as usize {
        let data: Vec<u8> = match sample {
            SampleType::F32 => (0..row / 4)
                .flat_map(|i| patterns[(i + y) % 8].to_ne_bytes())
                .collect(),
            SampleType::F16 => (0..row / 2)
                .flat_map(|i| half[(i + y) % 8].to_ne_bytes())
                .collect(),
            _ => (0..row).map(|i| (i * 71 + y * 37) as u8).collect(),
        };
        bytes[y * stride..y * stride + row].copy_from_slice(&data);
        tight.extend_from_slice(&data);
    }
    (bytes, stride, tight)
}

fn drain(session: &mut GroupEncodeSession, output: &mut Vec<u8>) {
    let count = session.output().len();
    output.extend_from_slice(session.output());
    session.consume_output(count).unwrap();
}

#[test]
fn padded_batches_full_and_pull_match_native_group_bytes_for_baseline_storage() {
    for (desc, sample) in [
        (PixelDescriptor::RGB8_SRGB, SampleType::U8),
        (PixelDescriptor::RGB16_SRGB, SampleType::U16),
        (PixelFormat::RgbF16.descriptor(), SampleType::F16),
        (PixelDescriptor::RGBF32_LINEAR, SampleType::F32),
    ] {
        let (bytes, stride, tight) = source(desc, sample);
        let whole = PixelSlice::new(&bytes, WIDTH, HEIGHT, stride, desc).unwrap();
        let full = imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
            .job()
            .with_limits(ResourceLimits::none().with_max_memory(64 << 20))
            .encoder()
            .unwrap()
            .encode(whole.clone())
            .unwrap();
        let (info, metadata) = imij::probe(full.data()).unwrap();
        assert_eq!(info.sample, sample);
        assert_eq!(imij::decode(full.data()).unwrap().1, tight);
        // The adapter's emitted description is fed to the native
        // group primitive; group framing and entropy bytes must agree exactly.
        let mut native = GroupEncodeSession::new(
            info.clone(),
            &metadata,
            &imij::ImiEncoderConfig::default()
                .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING))),
        )
        .unwrap();
        let mut expected = Vec::new();
        drain(&mut native, &mut expected);
        while let Some(rect) = native.next_group() {
            let start = rect.y as usize * stride + rect.x as usize * desc.bytes_per_pixel();
            let end = start + rect.width as usize * desc.bytes_per_pixel();
            native.push_group(&bytes[start..end], stride).unwrap();
            drain(&mut native, &mut expected);
        }
        native.finish().unwrap();
        assert_eq!(full.data(), expected);
        for batch in [1, 2, 4, HEIGHT] {
            let mut encoder = imij::ImiEncoderConfig::default()
                .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
                .job()
                .with_canvas_size(WIDTH, HEIGHT)
                .encoder()
                .unwrap();
            assert_eq!(encoder.preferred_strip_height(), 1);
            let mut y = 0;
            while y < HEIGHT {
                let rows = batch.min(HEIGHT - y);
                encoder
                    .push_rows(
                        PixelSlice::new(
                            &bytes[y as usize * stride..(y + rows) as usize * stride],
                            WIDTH,
                            rows,
                            stride,
                            desc,
                        )
                        .unwrap(),
                    )
                    .unwrap();
                y += rows;
            }
            let out = encoder.finish().unwrap();
            assert_eq!(out.data(), expected);
            assert_eq!(imij::decode(out.data()).unwrap().1, tight);
        }
        let mut calls = 0;
        let mut callback = |y: u32, mut out: zenpixels::PixelSliceMut<'_>| {
            assert_eq!(y, calls);
            assert_eq!(
                (out.width(), out.rows(), out.descriptor()),
                (WIDTH, 1, desc)
            );
            out.row_mut(0).copy_from_slice(
                &tight[y as usize * info.row_bytes()..(y as usize + 1) * info.row_bytes()],
            );
            calls += 1;
            1
        };
        let pull = imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
            .job()
            .with_input_descriptor(desc)
            .with_canvas_size(WIDTH, HEIGHT)
            .encoder()
            .unwrap()
            .encode_from(&mut callback)
            .unwrap();
        assert_eq!(calls, HEIGHT);
        assert_eq!(pull.data(), expected);
        for y in 0..HEIGHT as usize {
            assert!(
                bytes[y * stride + info.row_bytes()..(y + 1) * stride]
                    .iter()
                    .all(|&b| b == 0xee)
            );
        }
    }
}

#[test]
fn exact_output_limit_accepts_and_one_byte_short_fails_during_push() {
    let desc = PixelDescriptor::RGB8_SRGB;
    let (bytes, stride, _) = source(desc, SampleType::U8);
    let whole = PixelSlice::new(&bytes, WIDTH, HEIGHT, stride, desc).unwrap();
    let output = imij::ImiEncoderConfig::default()
        .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
        .job()
        .encoder()
        .unwrap()
        .encode(whole.clone())
        .unwrap();
    let final_len = output.data().len() as u64;
    let mut exact = imij::ImiEncoderConfig::default()
        .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
        .job()
        .with_limits(ResourceLimits::none().with_max_output(final_len))
        .with_canvas_size(WIDTH, HEIGHT)
        .encoder()
        .unwrap();
    exact.push_rows(whole.clone()).unwrap();
    assert_eq!(exact.finish().unwrap().data(), output.data());
    let mut short = imij::ImiEncoderConfig::default()
        .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
        .job()
        .with_limits(ResourceLimits::none().with_max_output(final_len - 1))
        .with_canvas_size(WIDTH, HEIGHT)
        .encoder()
        .unwrap();
    let first = PixelSlice::new(&bytes[..stride], WIDTH, 1, stride, desc).unwrap();
    // The cap admits the header and initial groups; failure must occur as
    // later encoded bytes are drained, before finish returns a file.
    short.push_rows(first).unwrap();
    let remaining = PixelSlice::new(&bytes[stride..], WIDTH, HEIGHT - 1, stride, desc).unwrap();
    assert!(matches!(short.push_rows(remaining), Err(Error::TooLarge)));
    assert!(short.finish().is_err());
}

#[test]
// Also checks the legacy job `with_stop`, deprecated in zencodec 0.2; imij keeps it.
#[allow(deprecated)]
fn pull_denial_precedes_callback_and_push_checks_explicit_descriptor() {
    let desc = PixelDescriptor::RGB8_SRGB;
    for limits in [
        ResourceLimits::none().with_max_frames(0),
        ResourceLimits::none().with_max_pixels(u64::from(WIDTH * HEIGHT) - 1),
        ResourceLimits::none().with_max_memory(1),
        ResourceLimits::none().with_max_input_bytes(0),
    ] {
        let mut callbacks = 0;
        let mut source = |_: u32, mut row: zenpixels::PixelSliceMut<'_>| {
            callbacks += 1;
            row.row_mut(0).fill(7);
            1
        };
        match imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
            .job()
            .with_limits(limits)
            .with_input_descriptor(desc)
            .with_canvas_size(WIDTH, HEIGHT)
            .encoder()
        {
            Err(e) => assert!(matches!(e, Error::TooLarge)),
            Ok(encoder) => assert!(matches!(
                encoder.encode_from(&mut source),
                Err(Error::TooLarge)
            )),
        }
        assert_eq!(callbacks, 0);
    }
    #[derive(Clone)]
    struct Cancel;
    impl zencodec::enough::Stop for Cancel {
        fn check(&self) -> Result<(), zencodec::enough::StopReason> {
            Err(zencodec::enough::StopReason::Cancelled)
        }
    }
    let mut callbacks = 0;
    let mut cancelled_source = |_: u32, _: zenpixels::PixelSliceMut<'_>| {
        callbacks += 1;
        1
    };
    match imij::ImiEncoderConfig::default()
        .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
        .job()
        .with_stop(StopToken::new(Cancel))
        .with_input_descriptor(desc)
        .with_canvas_size(WIDTH, HEIGHT)
        .encoder()
    {
        Err(e) => assert!(matches!(e, Error::Cancelled)),
        Ok(encoder) => assert!(matches!(
            encoder.encode_from(&mut cancelled_source),
            Err(Error::Cancelled)
        )),
    }
    assert_eq!(callbacks, 0);
    let (bytes, stride, _) = source(desc, SampleType::U8);
    let mut encoder = imij::ImiEncoderConfig::default()
        .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
        .job()
        .with_input_descriptor(PixelDescriptor::RGB16_SRGB)
        .with_canvas_size(WIDTH, HEIGHT)
        .encoder()
        .unwrap();
    assert!(matches!(
        encoder.push_rows(PixelSlice::new(&bytes, WIDTH, HEIGHT, stride, desc).unwrap()),
        Err(Error::InvalidInput(_))
    ));
    assert!(encoder.finish().is_err());
}

#[test]
fn row_contract_errors_are_sticky() {
    let desc = PixelDescriptor::RGB8_SRGB;
    let (bytes, stride, _) = source(desc, SampleType::U8);
    let row = PixelSlice::new(&bytes[..stride], WIDTH, 1, stride, desc).unwrap();
    let mut missing = imij::ImiEncoderConfig::default()
        .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
        .job()
        .encoder()
        .unwrap();
    assert!(matches!(
        missing.push_rows(row.clone()),
        Err(Error::InvalidInput(_))
    ));
    assert!(missing.finish().is_err());
    for bad in [
        PixelSlice::new(&bytes[..stride], WIDTH - 1, 1, stride, desc).unwrap(),
        PixelSlice::new(
            &bytes[..stride],
            WIDTH,
            1,
            stride,
            desc.with_transfer(zenpixels::TransferFunction::Linear),
        )
        .unwrap(),
        row.clone()
            .with_color_context(Arc::new(ColorContext::from_icc(&b"new profile"[..]))),
    ] {
        let mut encoder = imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
            .job()
            .with_canvas_size(WIDTH, HEIGHT)
            .encoder()
            .unwrap();
        encoder.push_rows(row.clone()).unwrap();
        assert!(matches!(
            encoder.push_rows(bad),
            Err(Error::InvalidInput(_))
        ));
        assert!(encoder.push_rows(row.clone()).is_err());
        assert!(encoder.finish().is_err());
    }
    let mut short = imij::ImiEncoderConfig::default()
        .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
        .job()
        .with_canvas_size(WIDTH, HEIGHT)
        .encoder()
        .unwrap();
    short.push_rows(row.clone()).unwrap();
    assert!(matches!(short.finish(), Err(Error::InvalidInput(_))));
    let mut extra = imij::ImiEncoderConfig::default()
        .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
        .job()
        .with_canvas_size(WIDTH, 1)
        .encoder()
        .unwrap();
    extra.push_rows(row.clone()).unwrap();
    assert!(matches!(extra.push_rows(row), Err(Error::InvalidInput(_))));
    assert!(extra.finish().is_err());
}

#[test]
fn pull_requires_description_and_exactly_one_row_per_call() {
    let desc = PixelDescriptor::RGB8_SRGB;
    let mut callback = |_: u32, _: zenpixels::PixelSliceMut<'_>| -> usize {
        panic!("incomplete pull description must reject before callback")
    };
    for encoder in [
        imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
            .job()
            .with_canvas_size(WIDTH, HEIGHT)
            .encoder()
            .unwrap(),
        imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
            .job()
            .with_input_descriptor(desc)
            .encoder()
            .unwrap(),
    ] {
        assert!(encoder.encode_from(&mut callback).is_err());
    }
    for returned in [0, 2] {
        let mut calls = 0;
        let mut source = |_: u32, mut out: zenpixels::PixelSliceMut<'_>| {
            calls += 1;
            out.row_mut(0).fill(7);
            returned
        };
        let encoder = imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
            .job()
            .with_input_descriptor(desc)
            .with_canvas_size(WIDTH, HEIGHT)
            .encoder()
            .unwrap();
        assert!(matches!(
            encoder.encode_from(&mut source),
            Err(Error::InvalidInput(_))
        ));
        assert_eq!(calls, 1);
    }
}

#[test]
fn limits_are_checked_during_push_including_exposed_input_padding() {
    let desc = PixelDescriptor::RGB8_SRGB;
    let (bytes, stride, _) = source(desc, SampleType::U8);
    let batch = PixelSlice::new(&bytes[..stride * 2], WIDTH, 2, stride, desc).unwrap();
    let charge = batch.as_strided_bytes().len() as u64;
    for limits in [
        ResourceLimits::none().with_max_width(WIDTH - 1),
        ResourceLimits::none().with_max_pixels(u64::from(WIDTH * HEIGHT) - 1),
        ResourceLimits::none().with_max_total_pixels(u64::from(WIDTH * HEIGHT) - 1),
        ResourceLimits::none().with_max_frames(0),
        ResourceLimits::none().with_max_output(1),
        ResourceLimits::none().with_max_memory(1),
    ] {
        match imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
            .job()
            .with_limits(limits)
            .with_canvas_size(WIDTH, HEIGHT)
            .encoder()
        {
            Err(e) => assert!(matches!(e, Error::TooLarge)),
            Ok(mut encoder) => {
                assert!(matches!(
                    encoder.push_rows(batch.clone()),
                    Err(Error::TooLarge)
                ));
                assert!(encoder.finish().is_err());
            }
        }
    }
    let mut encoder = imij::ImiEncoderConfig::default()
        .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
        .job()
        .with_limits(ResourceLimits::none().with_max_input_bytes(charge * 2 - 1))
        .with_canvas_size(WIDTH, HEIGHT)
        .encoder()
        .unwrap();
    encoder.push_rows(batch.clone()).unwrap();
    assert!(matches!(encoder.push_rows(batch), Err(Error::TooLarge)));
    assert!(encoder.finish().is_err());
    let mut encoder = imij::ImiEncoderConfig::default()
        .with_row_groups(Some(imij::RowGroups::new(5, 1, 1)))
        .job()
        .with_canvas_size(WIDTH, HEIGHT)
        .encoder()
        .unwrap();
    assert!(matches!(
        encoder.push_rows(PixelSlice::new(&bytes, WIDTH, HEIGHT, stride, desc).unwrap()),
        Err(Error::TooLarge)
    ));
    assert!(encoder.finish().is_err());
}

#[test]
fn explicit_incompatible_requests_remain_unsupported() {
    assert!(matches!(
        imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
            .with_lossless(false)
            .job()
            .encoder(),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
            .with_generic_effort(0)
            .job()
            .encoder(),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
            .job()
            .animation_frame_encoder(),
        Err(Error::Unsupported(_))
    ));
}

#[test]
// Also checks the legacy job `with_stop`, deprecated in zencodec 0.2; imij keeps it.
#[allow(deprecated)]
fn cancellation_between_groups_and_following_calls_is_sticky() {
    #[derive(Clone)]
    struct Flag(Arc<AtomicBool>);
    impl zencodec::enough::Stop for Flag {
        fn check(&self) -> Result<(), zencodec::enough::StopReason> {
            if self.0.load(Ordering::SeqCst) {
                Err(zencodec::enough::StopReason::Cancelled)
            } else {
                Ok(())
            }
        }
    }
    let flag = Flag(Arc::new(AtomicBool::new(false)));
    let desc = PixelDescriptor::RGB8_SRGB;
    let (bytes, stride, _) = source(desc, SampleType::U8);
    let row = PixelSlice::new(&bytes[..stride], WIDTH, 1, stride, desc).unwrap();
    let mut encoder = imij::ImiEncoderConfig::default()
        .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
        .job()
        .with_stop(StopToken::new(flag.clone()))
        .with_canvas_size(WIDTH, HEIGHT)
        .encoder()
        .unwrap();
    encoder.push_rows(row.clone()).unwrap();
    flag.0.store(true, Ordering::SeqCst);
    assert!(matches!(
        encoder.push_rows(row.clone()),
        Err(Error::Cancelled)
    ));
    flag.0.store(false, Ordering::SeqCst);
    assert!(encoder.push_rows(row).is_err());
    assert!(encoder.finish().is_err());

    struct CountStop(AtomicUsize);
    impl zencodec::enough::Stop for CountStop {
        fn check(&self) -> Result<(), zencodec::enough::StopReason> {
            if self.0.fetch_add(1, Ordering::SeqCst) + 1 >= 12 {
                Err(zencodec::enough::StopReason::Cancelled)
            } else {
                Ok(())
            }
        }
    }
    let stop = Arc::new(CountStop(AtomicUsize::new(0)));
    #[derive(Clone)]
    struct CountToken(Arc<CountStop>);
    impl zencodec::enough::Stop for CountToken {
        fn check(&self) -> Result<(), zencodec::enough::StopReason> {
            zencodec::enough::Stop::check(self.0.as_ref())
        }
    }
    let mut encoder = imij::ImiEncoderConfig::default()
        .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
        .job()
        .with_stop(StopToken::new(CountToken(stop.clone())))
        .with_canvas_size(WIDTH, HEIGHT)
        .encoder()
        .unwrap();
    assert!(matches!(
        encoder.push_rows(PixelSlice::new(&bytes, WIDTH, HEIGHT, stride, desc).unwrap()),
        Err(Error::Cancelled)
    ));
    assert!(stop.0.load(Ordering::SeqCst) >= 12);
    assert!(encoder.finish().is_err());
}

#[test]
fn progress_counts_encoded_rows_for_push_full_and_pull_without_changing_bytes() {
    use how_far::{Complete, Outcome, Pulse};
    use how_far_along::{FnPulse, Status};
    for (desc, sample) in [
        (PixelDescriptor::RGB8_SRGB, SampleType::U8),
        (PixelDescriptor::RGB16_SRGB, SampleType::U16),
        (PixelFormat::RgbF16.descriptor(), SampleType::F16),
        (PixelDescriptor::RGBF32_LINEAR, SampleType::F32),
    ] {
        let (bytes, stride, tight) = source(desc, sample);
        let pixels = PixelSlice::new(&bytes, WIDTH, HEIGHT, stride, desc).unwrap();
        let ordinary = imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
            .job()
            .encoder()
            .unwrap()
            .encode(pixels.clone())
            .unwrap();
        for route in 0..3 {
            let pulse = FnPulse::new("encoded rows", |_| Ok(()));
            let observer = pulse.observer();
            let encoder = imij::ImiEncoderConfig::default()
                .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
                .job()
                .with_canvas_size(WIDTH, HEIGHT)
                .with_input_descriptor(desc)
                .with_progress(pulse.share().unwrap())
                .encoder()
                .unwrap();
            let output = match route {
                0 => encoder.encode(pixels.clone()).unwrap(),
                1 => {
                    let mut encoder = encoder;
                    for y in 0..HEIGHT {
                        encoder
                            .push_rows(
                                PixelSlice::new(
                                    &bytes[y as usize * stride..],
                                    WIDTH,
                                    1,
                                    stride,
                                    desc,
                                )
                                .unwrap(),
                            )
                            .unwrap();
                        let snapshot = observer.snapshot();
                        assert_eq!(snapshot.completed, u64::from(y + 1));
                        assert!(!matches!(snapshot.status, Status::Finished(_)));
                    }
                    encoder.finish().unwrap()
                }
                _ => {
                    let mut calls = 0;
                    let output = encoder
                        .encode_from(&mut |y, mut row| {
                            assert_eq!(observer.snapshot().completed, u64::from(y));
                            let start = y as usize * row.row_mut(0).len();
                            let len = row.row_mut(0).len();
                            row.row_mut(0).copy_from_slice(&tight[start..start + len]);
                            calls += 1;
                            1
                        })
                        .unwrap();
                    assert_eq!(calls, HEIGHT);
                    output
                }
            };
            assert_eq!(output.data(), ordinary.data(), "route {route}, {desc:?}");
            assert_eq!(imij::decode(output.data()).unwrap().1, tight);
            let snapshot = observer.snapshot();
            assert_eq!(snapshot.completed, u64::from(HEIGHT));
            assert!(snapshot.children.is_empty());
            assert!(!snapshot.overrun());
            assert!(!matches!(snapshot.status, Status::Finished(_)));
            pulse.complete(Ok::<_, Error>(())).unwrap();
            assert_eq!(
                observer.snapshot().status,
                Status::Finished(Outcome::Succeeded)
            );
        }
    }
}

#[test]
fn row_progress_cancellation_stops_the_next_source_and_is_sticky() {
    use how_far::{Complete, Outcome, Pulse, StopReason};
    use how_far_along::{FnPulse, Status};
    let desc = PixelDescriptor::RGB8_SRGB;
    let (bytes, stride, _) = source(desc, SampleType::U8);
    for pull in [false, true] {
        let pulse = FnPulse::new("encoded rows", |checkpoint| {
            if checkpoint.snapshot().completed >= 2 {
                Err(StopReason::Cancelled)
            } else {
                Ok(())
            }
        });
        let observer = pulse.observer();
        let mut encoder = imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
            .job()
            .with_canvas_size(WIDTH, HEIGHT)
            .with_input_descriptor(desc)
            .with_progress(pulse.share().unwrap())
            .encoder()
            .unwrap();
        let result = if pull {
            let mut calls = 0;
            let result = encoder.encode_from(&mut |y, mut row| {
                assert_eq!(y, calls);
                row.row_mut(0).fill(7);
                calls += 1;
                1
            });
            assert_eq!(calls, 2);
            result
        } else {
            let pixels = PixelSlice::new(&bytes, WIDTH, HEIGHT, stride, desc).unwrap();
            assert!(matches!(
                encoder.push_rows(pixels.clone()),
                Err(Error::Cancelled)
            ));
            assert!(matches!(encoder.push_rows(pixels), Err(Error::Cancelled)));
            encoder.finish()
        };
        assert!(matches!(pulse.complete(result), Err(Error::Cancelled)));
        let snapshot = observer.snapshot();
        assert_eq!(snapshot.completed, 2);
        assert_eq!(snapshot.status, Status::Finished(Outcome::Cancelled));
    }
}

#[test]
fn failed_group_emission_does_not_count_a_row() {
    use how_far::{Complete, Outcome, Pulse};
    use how_far_along::{FnPulse, Status};
    let desc = PixelDescriptor::RGB8_SRGB;
    let (bytes, stride, _) = source(desc, SampleType::U8);
    let pixels = PixelSlice::new(&bytes, WIDTH, HEIGHT, stride, desc).unwrap();
    let full = imij::ImiEncoderConfig::default()
        .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
        .job()
        .encoder()
        .unwrap()
        .encode(pixels.clone())
        .unwrap();
    for (limit, completed) in [
        (1, 0),
        (full.data().len() as u64 - 1, u64::from(HEIGHT - 1)),
    ] {
        let pulse = FnPulse::new("encoded rows", |_| Ok(()));
        let observer = pulse.observer();
        let result = imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
            .job()
            .with_canvas_size(WIDTH, HEIGHT)
            .with_limits(ResourceLimits::none().with_max_output(limit))
            .with_progress(pulse.share().unwrap())
            .encoder()
            .unwrap()
            .encode(pixels.clone());
        assert!(matches!(pulse.complete(result), Err(Error::TooLarge)));
        assert_eq!(observer.snapshot().completed, completed);
        assert_eq!(
            observer.snapshot().status,
            Status::Finished(Outcome::Failed)
        );
    }
}

fn drain_common(encoder: &mut imij::codec::GroupEncoder, out: &mut Vec<u8>) {
    loop {
        let bytes = encoder.output().unwrap();
        if bytes.is_empty() {
            break;
        }
        let count = bytes.len().min(7);
        out.extend_from_slice(&bytes[..count]);
        encoder.consume_output(count).unwrap();
    }
}

#[test]
#[allow(deprecated)] // Explicit legacy metadata setter compatibility control.
fn caller_drained_groups_match_buffered_bytes_and_raw_baseline_samples() {
    for (desc, sample) in [
        (PixelDescriptor::RGB8_SRGB, SampleType::U8),
        (PixelDescriptor::RGB16_SRGB, SampleType::U16),
        (PixelFormat::RgbF16.descriptor(), SampleType::F16),
        (PixelDescriptor::RGBF32_LINEAR, SampleType::F32),
    ] {
        let (bytes, stride, tight) = source(desc, sample);
        let metadata = zencodec::Metadata::none().with_xmp(&b"group metadata"[..]);
        let cfg = imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)));
        let expected = cfg
            .clone()
            .job()
            .with_metadata_policy(metadata.clone(), zencodec::MetadataPolicy::PreserveExact)
            .encoder()
            .unwrap()
            .encode(PixelSlice::new(&bytes, WIDTH, HEIGHT, stride, desc).unwrap())
            .unwrap();
        let mut encoder = cfg
            .job()
            .with_metadata_policy(metadata, zencodec::MetadataPolicy::PreserveExact)
            .with_input_descriptor(desc)
            .with_canvas_size(WIDTH, HEIGHT)
            .with_limits(
                ResourceLimits::none()
                    .with_max_memory((WORKING + 4096) as u64)
                    .with_max_output(expected.data().len() as u64),
            )
            .group_encoder()
            .unwrap();
        assert!(encoder.next_group().is_none());
        let mut encoded = Vec::new();
        drain_common(&mut encoder, &mut encoded);
        while let Some(rect) = encoder.next_group() {
            let start = rect.y as usize * stride + rect.x as usize * desc.bytes_per_pixel();
            let end = start + rect.width as usize * desc.bytes_per_pixel();
            encoder
                .push_group(
                    PixelSlice::new(&bytes[start..end], rect.width, rect.height, stride, desc)
                        .unwrap(),
                )
                .unwrap();
            assert!(encoder.next_group().is_none());
            drain_common(&mut encoder, &mut encoded);
        }
        assert_eq!(encoder.bytes_produced(), encoded.len() as u64);
        encoder.finish().unwrap();
        assert_eq!(encoded, expected.data());
        assert_eq!(imij::decode(&encoded).unwrap().1, tight);
        assert_eq!(
            imij::probe(&encoded).unwrap().1.xmp.unwrap(),
            b"group metadata"
        );
    }
}

#[test]
fn drained_group_limits_are_cumulative_and_failures_are_sticky() {
    let desc = PixelDescriptor::RGB8_SRGB;
    let (bytes, stride, _) = source(desc, SampleType::U8);
    let cfg = imij::ImiEncoderConfig::default()
        .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)));
    let expected = cfg
        .clone()
        .job()
        .encoder()
        .unwrap()
        .encode(PixelSlice::new(&bytes, WIDTH, HEIGHT, stride, desc).unwrap())
        .unwrap();
    let mut encoder = cfg
        .clone()
        .job()
        .with_input_descriptor(desc)
        .with_canvas_size(WIDTH, HEIGHT)
        .with_limits(ResourceLimits::none().with_max_output(expected.data().len() as u64 - 1))
        .group_encoder()
        .unwrap();
    let mut transferred = Vec::new();
    drain_common(&mut encoder, &mut transferred);
    let mut failed = false;
    while let Some(rect) = encoder.next_group() {
        let start = rect.y as usize * stride + rect.x as usize * desc.bytes_per_pixel();
        let end = start + rect.width as usize * desc.bytes_per_pixel();
        match encoder
            .push_group(PixelSlice::new(&bytes[start..end], rect.width, 1, stride, desc).unwrap())
        {
            Ok(()) => drain_common(&mut encoder, &mut transferred),
            Err(Error::TooLarge) => {
                failed = true;
                break;
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }
    assert!(failed);
    assert!(transferred.len() < expected.data().len());
    assert!(matches!(encoder.output(), Err(Error::TooLarge)));
    assert!(matches!(encoder.consume_output(0), Err(Error::TooLarge)));
    assert!(matches!(encoder.finish(), Err(Error::TooLarge)));
    for limits in [
        ResourceLimits::none().with_max_input_bytes((WIDTH * HEIGHT * 3 - 1) as u64),
        ResourceLimits::none().with_max_memory(1),
        ResourceLimits::none().with_max_width(WIDTH - 1),
        ResourceLimits::none().with_max_output(1),
    ] {
        assert!(matches!(
            cfg.clone()
                .job()
                .with_input_descriptor(desc)
                .with_canvas_size(WIDTH, HEIGHT)
                .with_limits(limits)
                .group_encoder(),
            Err(Error::TooLarge)
        ));
    }
    // Canvas size does not require a full row/raster/output allocation:
    // bounded group headers check total byte length against `u64`, so both
    // 1 GPx and 1 TPx canvases admit within bounded memory even on 32-bit.
    for height in [1_000, 1_000_000] {
        let mut huge = cfg
            .clone()
            .job()
            .with_input_descriptor(desc)
            .with_canvas_size(1_000_000, height)
            .with_limits(ResourceLimits::none().with_max_memory((WORKING + 4096) as u64))
            .group_encoder()
            .unwrap();
        let header = huge.output().unwrap().len();
        huge.consume_output(header).unwrap();
        let rect = huge.next_group().unwrap();
        assert_eq!((rect.width, rect.height), (5, 1));
        huge.push_group(PixelSlice::new(&[0; 15], 5, 1, 15, desc).unwrap())
            .unwrap();
        assert!(huge.output().unwrap().len() < 4096);
        assert!(matches!(huge.finish(), Err(Error::InvalidInput(_))));
    }
    // A canvas whose total byte count genuinely overflows `u64::MAX` is still
    // rejected up front on every pointer width (`u32::MAX^2 * 3 > u64::MAX`).
    let overflow = cfg
        .clone()
        .job()
        .with_input_descriptor(desc)
        .with_canvas_size(u32::MAX, u32::MAX)
        .with_limits(ResourceLimits::none().with_max_memory((WORKING + 4096) as u64))
        .group_encoder();
    assert!(matches!(overflow, Err(Error::TooLarge)));
}

#[test]
// Also checks the legacy job `with_stop`, deprecated in zencodec 0.2; imij keeps it.
#[allow(deprecated)]
fn drained_group_stop_is_sticky_even_if_the_token_is_cleared() {
    #[derive(Clone)]
    struct Flag(Arc<AtomicBool>);
    impl zencodec::enough::Stop for Flag {
        fn check(&self) -> Result<(), zencodec::enough::StopReason> {
            if self.0.load(Ordering::SeqCst) {
                Err(zencodec::enough::StopReason::Cancelled)
            } else {
                Ok(())
            }
        }
    }
    let flag = Flag(Arc::new(AtomicBool::new(false)));
    let mut encoder = imij::ImiEncoderConfig::default()
        .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
        .job()
        .with_canvas_size(WIDTH, HEIGHT)
        .with_input_descriptor(PixelDescriptor::RGB8_SRGB)
        .with_stop(StopToken::new(flag.clone()))
        .group_encoder()
        .unwrap();
    flag.0.store(true, Ordering::SeqCst);
    assert!(matches!(encoder.output(), Err(Error::Cancelled)));
    flag.0.store(false, Ordering::SeqCst);
    assert!(matches!(encoder.output(), Err(Error::Cancelled)));
    assert!(matches!(encoder.finish(), Err(Error::Cancelled)));
}

#[test]
fn drained_groups_reject_changed_color_and_invalid_transfers_stickily() {
    let desc = PixelDescriptor::RGB8_SRGB;
    let open = || {
        imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
            .job()
            .with_canvas_size(WIDTH, HEIGHT)
            .with_input_descriptor(desc)
            .group_encoder()
            .unwrap()
    };
    for mode in 0..4 {
        let mut encoder = open();
        let header = encoder.output().unwrap().len();
        let result = if mode == 0 {
            encoder.consume_output(header + 1)
        } else if mode == 1 {
            encoder.push_group(PixelSlice::new(&[0; 15], 5, 1, 15, desc).unwrap())
        } else {
            encoder.consume_output(header).unwrap();
            let pixels = PixelSlice::new(&[0; 15], 5, 1, 15, desc).unwrap();
            if mode == 2 {
                encoder.push_group(pixels.with_color_context(Arc::new(ColorContext::from_icc(
                    &b"different profile"[..],
                ))))
            } else {
                let wrong =
                    zenpixels::PixelBuffer::try_new(5, 1, PixelDescriptor::RGB16_SRGB).unwrap();
                encoder.push_group(wrong.as_slice())
            }
        };
        assert!(matches!(result, Err(Error::InvalidInput(_))));
        assert!(matches!(encoder.output(), Err(Error::InvalidInput(_))));
        assert!(matches!(encoder.finish(), Err(Error::InvalidInput(_))));
    }
    // Both current and historical carrier copies are admitted before copying.
    let large = vec![0u8; WORKING / 4 + 1];
    assert!(matches!(
        imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
            .job()
            .with_canvas_size(WIDTH, HEIGHT)
            .with_input_descriptor(desc)
            .with_color_origin(zenpixels::ColorOrigin::from_icc(large.as_slice()))
            .group_encoder(),
        Err(Error::TooLarge)
    ));
    let mut encoder = open();
    let header = encoder.output().unwrap().len();
    encoder.consume_output(header).unwrap();
    let pixels = PixelSlice::new(&[0; 15], 5, 1, 15, desc)
        .unwrap()
        .with_color_context(Arc::new(ColorContext::from_icc(large.as_slice())));
    assert!(matches!(encoder.push_group(pixels), Err(Error::TooLarge)));
    assert!(matches!(encoder.output(), Err(Error::TooLarge)));
}

#[test]
fn drained_group_progress_counts_rows_and_cancels_after_completed_groups() {
    use how_far::{Pulse, StopReason};
    use how_far_along::FnPulse;
    let desc = PixelDescriptor::RGB8_SRGB;
    let pulse = FnPulse::new("encoded rows", |checkpoint| {
        if checkpoint.snapshot().completed >= 2 {
            Err(StopReason::Cancelled)
        } else {
            Ok(())
        }
    });
    let observer = pulse.observer();
    let mut encoder = imij::ImiEncoderConfig::default()
        .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)))
        .job()
        .with_canvas_size(WIDTH, HEIGHT)
        .with_input_descriptor(desc)
        .with_progress(pulse.share().unwrap())
        .group_encoder()
        .unwrap();
    let mut output = Vec::new();
    drain_common(&mut encoder, &mut output);
    let mut supplied = 0;
    while let Some(rect) = encoder.next_group() {
        assert_eq!(observer.snapshot().completed, u64::from(rect.y));
        let row = [0; 15];
        let pixels =
            PixelSlice::new(&row[..rect.width as usize * 3], rect.width, 1, 15, desc).unwrap();
        supplied += 1;
        let result = encoder.push_group(pixels);
        if supplied == 8 {
            assert!(matches!(result, Err(Error::Cancelled)));
            break;
        }
        result.unwrap();
        assert_eq!(
            observer.snapshot().completed,
            u64::from(rect.y + u32::from(rect.x + rect.width == WIDTH))
        );
        drain_common(&mut encoder, &mut output);
    }
    assert_eq!(supplied, 8);
    assert_eq!(observer.snapshot().completed, 2);
    assert!(matches!(encoder.output(), Err(Error::Cancelled)));
    assert!(matches!(encoder.finish(), Err(Error::Cancelled)));
}
