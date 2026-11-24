//! Common animation encoding agrees with native framing and retains admission failures.
use imij::{Error, FrameConfig, FrameDecoder, FrameDuration, FrameEncoder};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use zencodec::encode::{AnimationFrameEncoder, EncodeJob, EncoderConfig};
use zencodec::{Metadata, MetadataPolicy, ResourceLimits, StopToken, ThreadingPolicy};
use zenpixels::{ColorContext, PixelDescriptor, PixelSlice};

fn padded(desc: PixelDescriptor, frame: usize) -> (Vec<u8>, usize, Vec<u8>) {
    let row = 33 * desc.bytes_per_pixel();
    let stride = row + 3 * desc.bytes_per_pixel();
    let mut bytes = vec![0xee; stride * 17];
    let mut tight = Vec::new();
    for y in 0..17 {
        for x in 0..row {
            bytes[y * stride + x] = (x * 37 + y * 31 + frame * 13) as u8;
        }
        tight.extend_from_slice(&bytes[y * stride..y * stride + row]);
    }
    (bytes, stride, tight)
}

fn native_frames(file: &[u8]) -> Vec<Vec<u8>> {
    let mut decoder = FrameDecoder::new(file).unwrap();
    let info = decoder.info().clone();
    let mut canvas = vec![0; info.checked_buffer_len().unwrap()];
    let mut frames = Vec::new();
    while decoder.next_frame().unwrap().is_some() {
        decoder
            .decode_into(&mut canvas, info.row_bytes(), 1)
            .unwrap();
        frames.push(canvas.clone());
    }
    frames
}

#[test]
#[allow(deprecated)] // Explicit legacy metadata setter compatibility control.
fn padded_baseline_frames_match_native_lossless_and_lossy_encoding() {
    fn send_static<T: Send + 'static>(value: T) -> T {
        value
    }
    for (desc, sample) in [
        (PixelDescriptor::RGB8_SRGB, imij::SampleType::U8),
        (PixelDescriptor::RGB16_SRGB, imij::SampleType::U16),
        (PixelDescriptor::RGBF32_LINEAR, imij::SampleType::F32),
    ] {
        let sources: Vec<_> = (0..3)
            .map(|i| padded(desc, if i == 2 { 1 } else { i }))
            .collect();
        let cicp = zencodec::Cicp::from_descriptor(&desc).unwrap();
        let mut info = imij::ImageInfo::new(33, 17, imij::Channels::Rgb, sample);
        info.cicp = imij::Cicp {
            color_primaries: cicp.color_primaries,
            transfer_characteristics: cicp.transfer_characteristics,
            matrix_coefficients: cicp.matrix_coefficients,
            full_range: cicp.full_range,
        };
        let native_meta = imij::Metadata {
            xmp: Some(b"<animation/>".to_vec()),
            orientation: 1,
            ..Default::default()
        };
        let common_meta = Metadata::none().with_xmp(&b"<animation/>"[..]);
        let lossless = {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.with_effort(0.0);
            cfg = cfg.with_threads(1);
            cfg
        };
        let mut lossy = imij::ImiEncoderConfig::lossy(60.0).with_effort(0.0);
        lossy = lossy.with_threads(1);
        // Native lossy encoding is exercised on its ordinary RGB8 route.
        let modes = if sample == imij::SampleType::U8 { 2 } else { 1 };
        for mode in 0..modes {
            let coding = if mode == 0 {
                lossless.clone()
            } else {
                lossy.clone()
            };
            let cfg = if mode == 0 {
                lossless.clone()
            } else {
                lossy.clone()
            };
            let mut frame_cfg = FrameConfig::new(coding);
            frame_cfg.loop_count = 3;
            let mut native = FrameEncoder::animation(info.clone(), native_meta.clone(), frame_cfg);
            let mut adapter = send_static(
                cfg.job()
                    .with_canvas_size(33, 17)
                    .with_loop_count(Some(3))
                    .with_metadata_policy(common_meta.clone(), MetadataPolicy::PreserveExact)
                    .with_limits(ResourceLimits::none().with_threading(ThreadingPolicy::Sequential))
                    .animation_frame_encoder()
                    .unwrap(),
            );
            for (i, (bytes, stride, _)) in sources.iter().enumerate() {
                let ms = [33, 0, 7][i];
                native
                    .push_frame(bytes, *stride, FrameDuration::from_millis(ms))
                    .unwrap();
                adapter
                    .push_frame(
                        PixelSlice::new(bytes, 33, 17, *stride, desc).unwrap(),
                        ms,
                        None,
                    )
                    .unwrap();
                for y in 0..17 {
                    assert!(
                        bytes[y * stride + info.row_bytes()..(y + 1) * stride]
                            .iter()
                            .all(|&b| b == 0xee)
                    );
                }
            }
            let direct = native.finish().unwrap();
            let output = adapter.finish(None).unwrap();
            assert_eq!(output.data(), direct);
            assert_eq!(native_frames(output.data()), native_frames(&direct));
            if mode == 0 {
                assert_eq!(
                    native_frames(output.data()),
                    sources.iter().map(|s| s.2.clone()).collect::<Vec<_>>()
                );
            }
            let mut decoder = FrameDecoder::new(output.data()).unwrap();
            assert_eq!(decoder.loop_count(), 3);
            assert_eq!(decoder.frame_count(), 3);
            assert_eq!(decoder.metadata().xmp, native_meta.xmp);
            for ms in [33, 0, 7] {
                assert_eq!(
                    decoder.next_frame().unwrap().unwrap().duration,
                    FrameDuration::from_millis(ms)
                );
            }
            assert!(decoder.next_frame().unwrap().is_none());
        }
    }
}

#[test]
fn first_frame_establishes_canvas_and_empty_finish_rejects() {
    let (bytes, stride, tight) = padded(PixelDescriptor::RGB8_SRGB, 0);
    let mut encoder = imij::ImiEncoderConfig::default()
        .job()
        .with_loop_count(Some(0))
        .animation_frame_encoder()
        .unwrap();
    encoder
        .push_frame(
            PixelSlice::new(&bytes, 33, 17, stride, PixelDescriptor::RGB8_SRGB).unwrap(),
            0,
            None,
        )
        .unwrap();
    let output = encoder.finish(None).unwrap();
    assert_eq!(native_frames(output.data()), vec![tight]);
    assert_eq!(FrameDecoder::new(output.data()).unwrap().loop_count(), 0);
    assert!(matches!(
        imij::ImiEncoderConfig::default()
            .job()
            .animation_frame_encoder()
            .unwrap()
            .finish(None),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn dimension_descriptor_and_color_changes_fail_stickily() {
    let (bytes, stride, _) = padded(PixelDescriptor::RGB8_SRGB, 0);
    let original = PixelSlice::new(&bytes, 33, 17, stride, PixelDescriptor::RGB8_SRGB).unwrap();
    let mismatches = [
        PixelSlice::new(&bytes, 32, 17, stride, PixelDescriptor::RGB8_SRGB).unwrap(),
        PixelSlice::new(
            &bytes,
            33,
            17,
            stride,
            PixelDescriptor::RGB8_SRGB.with_transfer(zenpixels::TransferFunction::Linear),
        )
        .unwrap(),
        original
            .clone()
            .with_color_context(Arc::new(ColorContext::from_icc(&b"distinct ICC"[..]))),
    ];
    for bad in mismatches {
        let mut encoder = imij::ImiEncoderConfig::default()
            .job()
            .animation_frame_encoder()
            .unwrap();
        encoder.push_frame(original.clone(), 1, None).unwrap();
        assert!(encoder.push_frame(bad, 1, None).is_err());
        assert!(encoder.push_frame(original.clone(), 1, None).is_err());
        assert!(encoder.finish(None).is_err());
    }
    let mut encoder = imij::ImiEncoderConfig::default()
        .job()
        .with_canvas_size(32, 17)
        .animation_frame_encoder()
        .unwrap();
    assert!(encoder.push_frame(original, 1, None).is_err());
    assert!(encoder.finish(None).is_err());
}

#[test]
fn cumulative_limits_and_final_output_bound_are_enforced() {
    let (bytes, stride, _) = padded(PixelDescriptor::RGB8_SRGB, 0);
    let image = PixelSlice::new(&bytes, 33, 17, stride, PixelDescriptor::RGB8_SRGB).unwrap();
    let input = image.as_strided_bytes().len() as u64;
    for limits in [
        ResourceLimits::none().with_max_frames(1),
        ResourceLimits::none().with_max_input_bytes(input * 2 - 1),
        ResourceLimits::none().with_max_total_pixels(33 * 17 * 2 - 1),
        ResourceLimits::none().with_max_animation_ms(65),
    ] {
        let mut encoder = imij::ImiEncoderConfig::default()
            .job()
            .with_limits(limits)
            .animation_frame_encoder()
            .unwrap();
        encoder.push_frame(image.clone(), 33, None).unwrap();
        assert!(matches!(
            encoder.push_frame(image.clone(), 33, None),
            Err(Error::TooLarge)
        ));
        assert!(encoder.push_frame(image.clone(), 0, None).is_err());
        assert!(encoder.finish(None).is_err());
    }
    let mut encoder = imij::ImiEncoderConfig::default()
        .job()
        .with_limits(
            ResourceLimits::none()
                .with_max_frames(2)
                .with_max_input_bytes(input * 2)
                .with_max_total_pixels(33 * 17 * 2)
                .with_max_animation_ms(66),
        )
        .animation_frame_encoder()
        .unwrap();
    for _ in 0..2 {
        encoder.push_frame(image.clone(), 33, None).unwrap();
    }
    let output = encoder.finish(None).unwrap();
    for (cap, succeeds) in [
        (output.data().len() as u64, true),
        (output.data().len() as u64 - 1, false),
    ] {
        let mut encoder = imij::ImiEncoderConfig::default()
            .job()
            .with_limits(ResourceLimits::none().with_max_output(cap))
            .animation_frame_encoder()
            .unwrap();
        for _ in 0..2 {
            encoder.push_frame(image.clone(), 33, None).unwrap();
        }
        let result = encoder.finish(None);
        if succeeds {
            assert_eq!(result.unwrap().data(), output.data());
        } else {
            assert!(matches!(result, Err(Error::TooLarge)));
        }
    }
    assert!(matches!(
        imij::ImiEncoderConfig::default()
            .job()
            .with_limits(ResourceLimits::none().with_max_memory(64 << 20))
            .animation_frame_encoder(),
        Err(Error::Unsupported(_))
    ));
}

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

#[test]
// Also checks the legacy job `with_stop`, deprecated in zencodec 0.2; imij keeps it.
#[allow(deprecated)]
fn cancellation_before_and_after_native_push_is_sticky() {
    let (bytes, stride, _) = padded(PixelDescriptor::RGB8_SRGB, 0);
    let image = PixelSlice::new(&bytes, 33, 17, stride, PixelDescriptor::RGB8_SRGB).unwrap();
    for job_stop in [false, true] {
        let flag = Flag(Arc::new(AtomicBool::new(false)));
        let job = imij::ImiEncoderConfig::default().job();
        let job = if job_stop {
            job.with_stop(StopToken::new(flag.clone()))
        } else {
            job
        };
        let mut encoder = job.animation_frame_encoder().unwrap();
        encoder.push_frame(image.clone(), 1, None).unwrap();
        flag.0.store(true, Ordering::SeqCst);
        assert!(matches!(
            encoder.push_frame(
                image.clone(),
                1,
                (!job_stop).then_some(&how_far::StopOnly::borrowed(&flag) as &dyn zencodec::Pulse)
            ),
            Err(Error::Cancelled)
        ));
        flag.0.store(false, Ordering::SeqCst);
        assert!(encoder.push_frame(image.clone(), 1, None).is_err());
        assert!(encoder.finish(None).is_err());
    }
    struct After(AtomicUsize);
    impl zencodec::enough::Stop for After {
        fn check(&self) -> Result<(), zencodec::enough::StopReason> {
            if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(())
            } else {
                Err(zencodec::enough::StopReason::Cancelled)
            }
        }
    }
    let mut encoder = imij::ImiEncoderConfig::default()
        .job()
        .animation_frame_encoder()
        .unwrap();
    let after = After(AtomicUsize::new(0));
    assert!(matches!(
        encoder.push_frame(image.clone(), 1, Some(&how_far::StopOnly::borrowed(&after))),
        Err(Error::Cancelled)
    ));
    assert!(after.0.load(Ordering::SeqCst) >= 2);
    assert!(encoder.push_frame(image, 1, None).is_err());
    assert!(encoder.finish(None).is_err());

    let flag = Flag(Arc::new(AtomicBool::new(false)));
    let mut encoder = imij::ImiEncoderConfig::default()
        .job()
        .animation_frame_encoder()
        .unwrap();
    encoder
        .push_frame(
            PixelSlice::new(&bytes, 33, 17, stride, PixelDescriptor::RGB8_SRGB).unwrap(),
            1,
            None,
        )
        .unwrap();
    flag.0.store(true, Ordering::SeqCst);
    assert!(matches!(
        encoder.finish(Some(&how_far::StopOnly::borrowed(&flag))),
        Err(Error::Cancelled)
    ));
    assert!(matches!(
        imij::ImiEncoderConfig::default()
            .job()
            .with_stop(StopToken::new(flag))
            .animation_frame_encoder(),
        Err(Error::Cancelled)
    ));
}

#[test]
fn exact_common_timing_roundtrips_strided_pixels_and_native_durations() {
    use std::borrow::Cow;
    use zencodec::animation::FrameDuration as Duration;
    use zencodec::decode::{AnimationFrameDecoder, DecodeJob, DecoderConfig};
    let durations = [
        Duration::new(1, 60).unwrap(),
        Duration::new(1, 2000).unwrap(),
        Duration::new(3003, 90000).unwrap(),
        Duration::new(0, 100).unwrap(),
        Duration::new(u64::MAX, 1).unwrap(),
    ];
    let (bytes, stride, tight) = padded(PixelDescriptor::RGB8_SRGB, 0);
    let image = PixelSlice::new(&bytes, 33, 17, stride, PixelDescriptor::RGB8_SRGB).unwrap();
    let mut encoder = imij::ImiEncoderConfig::default()
        .job()
        .with_limits(ResourceLimits::none())
        .animation_frame_encoder()
        .unwrap();
    for duration in durations {
        encoder
            .push_frame_timed(image.clone(), duration, None)
            .unwrap();
    }
    let file = encoder.finish(None).unwrap().into_vec();
    assert_eq!(native_frames(&file), vec![tight; durations.len()]);
    let mut native = FrameDecoder::new(&file).unwrap();
    let mut common = imij::codec::DecodeConfig::default()
        .job()
        .with_limits(ResourceLimits::none())
        .animation_frame_decoder(Cow::Borrowed(&file), &[])
        .unwrap();
    for duration in durations {
        let header = native.next_frame().unwrap().unwrap();
        assert_eq!(
            header.duration,
            FrameDuration {
                num: duration.numerator(),
                den: duration.denominator(),
            }
        );
        assert_eq!(
            common.render_next_frame(None).unwrap().unwrap().duration(),
            duration
        );
    }
    assert!(common.render_next_frame(None).unwrap().is_none());
}

#[test]
fn exact_timing_admission_rounds_up_without_changing_encoded_time() {
    use zencodec::animation::FrameDuration as Duration;
    let raw = [3, 4, 5];
    let image = PixelSlice::new(&raw, 1, 1, 3, PixelDescriptor::RGB8_SRGB).unwrap();
    let duration = Duration::new(1, 2000).unwrap();
    let mut encoder = imij::ImiEncoderConfig::default()
        .job()
        .with_limits(ResourceLimits::none().with_max_animation_ms(1))
        .animation_frame_encoder()
        .unwrap();
    encoder
        .push_frame_timed(image.clone(), duration, None)
        .unwrap();
    assert!(matches!(
        encoder.push_frame_timed(image.clone(), duration, None),
        Err(Error::TooLarge)
    ));
    assert!(matches!(
        encoder.push_frame_timed(image.clone(), Duration::from_millis(0), None),
        Err(Error::TooLarge)
    ));
    assert!(matches!(encoder.finish(None), Err(Error::TooLarge)));
    let mut encoder = imij::ImiEncoderConfig::default()
        .job()
        .with_limits(ResourceLimits::none().with_max_animation_ms(u64::MAX))
        .animation_frame_encoder()
        .unwrap();
    assert!(matches!(
        encoder.push_frame_timed(image, Duration::new(u64::MAX, 1).unwrap(), None),
        Err(Error::TooLarge)
    ));
}

#[test]
fn lossless_animation_progress_counts_completed_intra_inter_and_empty_frames() {
    use how_far::{Complete, Outcome, Pulse};
    use how_far_along::{FnPulse, Status};
    for descriptor in [
        PixelDescriptor::RGB8_SRGB,
        PixelDescriptor::RGB16_SRGB,
        PixelDescriptor::RGBF32_LINEAR,
    ] {
        let (base, stride, _) = padded(descriptor, 0);
        let mut changed = base.clone();
        // A small update requires the inter candidate, then repeats as empty.
        changed[stride + descriptor.bytes_per_pixel()] ^= 31;
        let inputs = [&base, &changed, &changed, &base];
        let config = {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.with_effort(0.0);
            cfg = cfg.tune_lossless(|t| {
                t.stripe_rows = Some(4);
            });
            cfg
        };
        let pulse = FnPulse::new("completed frames", |_| Ok(()));
        pulse.set_total(how_far::Total::Exact(4)).unwrap();
        let observer = pulse.observer();
        let mut ordinary = config.clone().job().animation_frame_encoder().unwrap();
        let mut observed = config
            .job()
            .with_progress(pulse.share().unwrap())
            .animation_frame_encoder()
            .unwrap();
        for (i, bytes) in inputs.into_iter().enumerate() {
            let pixels = PixelSlice::new(bytes, 33, 17, stride, descriptor).unwrap();
            ordinary.push_frame(pixels.clone(), 7, None).unwrap();
            observed.push_frame(pixels, 7, None).unwrap();
            let snapshot = observer.snapshot();
            assert_eq!(snapshot.completed, (i + 1) as u64);
            assert!(snapshot.children.is_empty());
            assert!(!snapshot.overrun());
            assert!(!matches!(snapshot.status, Status::Finished(_)));
        }
        let output = pulse.complete(observed.finish(None)).unwrap();
        assert_eq!(output.data(), ordinary.finish(None).unwrap().data());
        let frames = native_frames(output.data());
        for (actual, source) in frames.iter().zip(inputs) {
            let tight: Vec<_> = source
                .chunks(stride)
                .flat_map(|r| r[..33 * descriptor.bytes_per_pixel()].iter().copied())
                .collect();
            assert_eq!(*actual, tight);
        }
        let mut decoder = FrameDecoder::new(output.data()).unwrap();
        decoder.next_frame().unwrap().unwrap();
        assert!(decoder.next_frame().unwrap().unwrap().uses_previous);
        assert!(decoder.next_frame().unwrap().unwrap().coding.is_none());
        assert_eq!(
            observer.snapshot().status,
            Status::Finished(Outcome::Succeeded)
        );
    }
}

#[test]
// Also checks the legacy job `with_stop`, deprecated in zencodec 0.2; imij keeps it.
#[allow(deprecated)]
fn animation_progress_cancellation_and_per_frame_stop_are_sticky() {
    use how_far::{Outcome, Pulse, StopReason};
    use how_far_along::FnPulse;
    let (bytes, stride, _) = padded(PixelDescriptor::RGB8_SRGB, 0);
    let pixels = || PixelSlice::new(&bytes, 33, 17, stride, PixelDescriptor::RGB8_SRGB).unwrap();
    let pulse = FnPulse::new("frames", |checkpoint| {
        if checkpoint.snapshot().completed == 1 {
            Err(StopReason::Cancelled)
        } else {
            Ok(())
        }
    });
    let observer = pulse.observer();
    let mut encoder = imij::ImiEncoderConfig::default()
        .job()
        .with_progress(pulse.share().unwrap())
        .animation_frame_encoder()
        .unwrap();
    assert_eq!(
        encoder.push_frame(pixels(), 1, None).unwrap_err(),
        Error::Cancelled
    );
    assert_eq!(observer.snapshot().completed, 1);
    assert_eq!(
        encoder.push_frame(pixels(), 1, None).unwrap_err(),
        Error::Cancelled
    );
    assert_eq!(encoder.finish(None).unwrap_err(), Error::Cancelled);
    pulse.finish(Outcome::Cancelled).unwrap();

    struct StopAfter(AtomicUsize);
    impl how_far::Stop for StopAfter {
        fn check(&self) -> Result<(), StopReason> {
            if self.0.fetch_add(1, Ordering::SeqCst) >= 8 {
                Err(StopReason::Cancelled)
            } else {
                Ok(())
            }
        }
    }
    for per_frame in [false, true] {
        let stop = Arc::new(StopAfter(AtomicUsize::new(0)));
        let mut job = imij::ImiEncoderConfig::default().job();
        if !per_frame {
            job = job.with_stop(StopToken::new(stop.clone()));
        }
        let mut encoder = job.animation_frame_encoder().unwrap();
        let call = how_far::StopOnly::borrowed(&*stop);
        let local = per_frame.then_some(&call as &dyn how_far::Pulse);
        assert_eq!(
            encoder.push_frame(pixels(), 1, local).unwrap_err(),
            Error::Cancelled
        );
        assert_eq!(stop.0.load(Ordering::SeqCst), 9);
        assert_eq!(
            encoder.push_frame(pixels(), 1, None).unwrap_err(),
            Error::Cancelled
        );
        assert_eq!(encoder.finish(None).unwrap_err(), Error::Cancelled);
    }
}

#[test]
fn lossy_animation_progress_matches_native_frames_and_is_cancellable() {
    use how_far::{Complete, Outcome, Pulse, StopReason};
    use how_far_along::{FnPulse, Status};
    let (base, stride, _) = padded(PixelDescriptor::RGB8_SRGB, 0);
    let mut changed = base.clone();
    changed[stride + 3] ^= 31;
    let inputs = [&base, &changed, &changed, &base];
    for entropy in [imij::LossyEntropy::RunLevel, imij::LossyEntropy::SplitHuf] {
        let config = imij::ImiEncoderConfig::lossy(90.0)
            .tune_lossy(|t| t.entropy = entropy)
            .with_effort(2.5);
        let pulse = FnPulse::new("completed frames", |_| Ok(()));
        pulse.set_total(how_far::Total::Exact(4)).unwrap();
        let observer = pulse.observer();
        let mut ordinary = config.clone().job().animation_frame_encoder().unwrap();
        let mut observed = config
            .clone()
            .job()
            .with_progress(pulse.share().unwrap())
            .animation_frame_encoder()
            .unwrap();
        for (i, bytes) in inputs.into_iter().enumerate() {
            let pixels =
                PixelSlice::new(bytes, 33, 17, stride, PixelDescriptor::RGB8_SRGB).unwrap();
            ordinary.push_frame(pixels.clone(), 7, None).unwrap();
            observed.push_frame(pixels, 7, None).unwrap();
            let snapshot = observer.snapshot();
            assert_eq!(snapshot.completed, (i + 1) as u64);
            assert!(snapshot.children.is_empty());
            assert!(!snapshot.overrun());
            assert!(!matches!(snapshot.status, Status::Finished(_)));
        }
        let output = pulse.complete(observed.finish(None)).unwrap();
        let ordinary = ordinary.finish(None).unwrap();
        assert_eq!(output.data(), ordinary.data());
        assert_eq!(native_frames(output.data()), native_frames(ordinary.data()));
        let mut decoder = FrameDecoder::new(output.data()).unwrap();
        decoder.next_frame().unwrap().unwrap();
        assert!(decoder.next_frame().unwrap().unwrap().uses_previous);
        assert!(decoder.next_frame().unwrap().unwrap().coding.is_none());
        assert_eq!(
            observer.snapshot().status,
            Status::Finished(Outcome::Succeeded)
        );
        let pulse = FnPulse::new("frames", |p| {
            if p.snapshot().completed == 1 {
                Err(StopReason::Cancelled)
            } else {
                Ok(())
            }
        });
        let observer = pulse.observer();
        let mut encoder = config
            .job()
            .with_progress(pulse.share().unwrap())
            .animation_frame_encoder()
            .unwrap();
        let pixels = PixelSlice::new(&base, 33, 17, stride, PixelDescriptor::RGB8_SRGB).unwrap();
        assert_eq!(
            encoder.push_frame(pixels.clone(), 1, None).unwrap_err(),
            Error::Cancelled
        );
        assert_eq!(observer.snapshot().completed, 1);
        assert_eq!(
            encoder.push_frame(pixels, 1, None).unwrap_err(),
            Error::Cancelled
        );
        assert_eq!(encoder.finish(None).unwrap_err(), Error::Cancelled);
        pulse.finish(Outcome::Cancelled).unwrap();
    }
}
