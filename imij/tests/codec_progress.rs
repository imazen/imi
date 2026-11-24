use how_far::{Complete, Outcome, Pulse, StopReason};
use how_far_along::{FnPulse, Snapshot, Status};
use imij::Error;
use imij::ImiEncoderConfig;
use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
use zencodec::{ResourceLimits, ThreadingPolicy};
use zenpixels::{PixelDescriptor, PixelSlice};

fn phase<'a>(snapshot: &'a Snapshot, name: &str) -> Option<&'a Snapshot> {
    if snapshot.name == name {
        Some(snapshot)
    } else {
        snapshot
            .children
            .iter()
            .find_map(|child| phase(child, name))
    }
}

fn all_succeeded(snapshot: &Snapshot) {
    assert_eq!(snapshot.status, Status::Finished(Outcome::Succeeded));
    assert!(!snapshot.overflowed);
    assert!(!snapshot.overrun());
    for child in &snapshot.children {
        all_succeeded(child);
    }
}

#[test]
fn observed_lossless_preserves_all_baseline_bits_and_completes_children() {
    for &descriptor in ImiEncoderConfig::supported_descriptors() {
        let row = 13 * descriptor.bytes_per_pixel();
        let stride = row + 2 * descriptor.bytes_per_pixel();
        let mut input = vec![0xee; stride * 19];
        let mut expected = Vec::new();
        for y in 0..19 {
            for x in 0..row {
                input[y * stride + x] = (x * 37 + y * 51) as u8;
            }
            expected.extend_from_slice(&input[y * stride..y * stride + row]);
        }
        let pixels = PixelSlice::new(&input, 13, 19, stride, descriptor).unwrap();
        let native = {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.tune_lossless(|t| {
                t.stripe_rows = Some(4);
            });
            cfg = cfg.with_checksum(true);
            cfg
        };
        let config = native;
        let limits = ResourceLimits::none().with_threading(ThreadingPolicy::Sequential);
        let ordinary = config
            .clone()
            .job()
            .with_limits(limits)
            .encoder()
            .unwrap()
            .encode(pixels.clone())
            .unwrap();
        let pulse = FnPulse::new("encode", |_| Ok(()));
        let observer = pulse.observer();
        let result = config
            .job()
            .with_limits(limits)
            .with_progress(pulse.share().unwrap())
            .encoder()
            .unwrap()
            .encode(pixels);
        // The job owns a shared view, never completion of the caller's root.
        assert!(!matches!(observer.snapshot().status, Status::Finished(_)));
        let output = pulse.complete(result).unwrap();
        assert_eq!(output.data(), ordinary.data(), "{descriptor:?}");
        assert_eq!(imij::decode(output.data()).unwrap().1, expected);
        let snapshot = observer.snapshot();
        all_succeeded(&snapshot);
        assert_eq!(phase(&snapshot, "pixel analysis").unwrap().completed, 19);
        assert_eq!(phase(&snapshot, "unit coding").unwrap().completed, 5);
    }
}

#[test]
fn cancellation_during_mapping_and_entropy_preserves_phase_outcomes() {
    let input: Vec<u8> = (0..97 * 41 * 3).map(|i| (i * 29 + i / 131) as u8).collect();
    let pixels = PixelSlice::new(&input, 97, 41, 291, PixelDescriptor::RGB8_SRGB).unwrap();
    for target in ["pixel analysis", "unit coding"] {
        for parallel in [false, true] {
            let pulse = FnPulse::new("encode", move |checkpoint| {
                let snapshot = checkpoint.snapshot();
                if checkpoint.phase_name() == target
                    && phase(&snapshot, target).is_some_and(|p| p.completed >= 1)
                {
                    Err(StopReason::Cancelled)
                } else {
                    Ok(())
                }
            });
            let observer = pulse.observer();
            let native = {
                let mut cfg = imij::ImiEncoderConfig::default();
                cfg = cfg.tune_lossless(|t| {
                    t.stripe_rows = Some(4);
                });
                cfg
            };
            let mut job = native.job().with_progress(pulse.share().unwrap());
            if !parallel {
                job = job.with_limits(
                    ResourceLimits::none().with_threading(ThreadingPolicy::Sequential),
                );
            }
            let result = job.encoder().unwrap().encode(pixels.clone());
            assert_eq!(result.unwrap_err(), Error::Cancelled);
            pulse.finish(Outcome::Cancelled).unwrap();
            let snapshot = observer.snapshot();
            let stopped = phase(&snapshot, target).unwrap();
            assert!(stopped.completed >= 1);
            assert_eq!(stopped.status, Status::Finished(Outcome::Cancelled));
            assert_eq!(
                phase(&snapshot, "unit index").unwrap().status,
                Status::Finished(Outcome::NotRun)
            );
            if target == "pixel analysis" {
                assert_eq!(stopped.completed, 1);
                assert_eq!(
                    phase(&snapshot, "unit coding").unwrap().status,
                    Status::Finished(Outcome::NotRun)
                );
            }
        }
    }
}

#[test]
// Also checks the legacy job `with_stop`, deprecated in zencodec 0.2; imij keeps it.
#[allow(deprecated)]
fn progress_does_not_hide_native_failure_or_legacy_stop() {
    let input = [0u8; 12];
    let pixels = PixelSlice::new(&input, 2, 2, 6, PixelDescriptor::RGB8_SRGB).unwrap();
    let pulse = FnPulse::new("encode", |_| Ok(()));
    let observer = pulse.observer();
    let native = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.with_effort(f32::NAN);
        cfg
    };
    let result = native
        .job()
        .with_progress(pulse.share().unwrap())
        .encoder()
        .unwrap()
        .encode(pixels);
    assert!(matches!(result, Err(Error::InvalidInput(_))));
    assert!(pulse.complete(result).is_err());
    let snapshot = observer.snapshot();
    assert_eq!(
        phase(&snapshot, "lossless payload").unwrap().status,
        Status::Finished(Outcome::Failed)
    );
    assert_eq!(
        phase(&snapshot, "unit index").unwrap().status,
        Status::Finished(Outcome::NotRun)
    );

    #[derive(Clone)]
    struct StopNow;
    impl how_far::Stop for StopNow {
        fn check(&self) -> Result<(), StopReason> {
            Err(StopReason::Cancelled)
        }
    }
    let pulse = FnPulse::new("stop", |_| Ok(()));
    assert!(matches!(
        imij::ImiEncoderConfig::default()
            .job()
            .with_progress(pulse.share().unwrap())
            .with_stop(zencodec::StopToken::new(StopNow))
            .encoder(),
        Err(Error::Cancelled)
    ));
    pulse.finish(Outcome::Cancelled).unwrap();
}

#[test]
fn observed_lossy_preserves_bytes_and_completes_phases() {
    for descriptor in [
        PixelDescriptor::RGB8_SRGB,
        PixelDescriptor::RGBA8_SRGB,
        PixelDescriptor::RGB16_SRGB,
        PixelDescriptor::RGBF32_LINEAR,
    ] {
        let row = 33 * descriptor.bytes_per_pixel();
        let stride = row + 2 * descriptor.bytes_per_pixel();
        let mut input = vec![0xee; stride * 65];
        for y in 0..65 {
            let out = &mut input[y * stride..y * stride + row];
            if descriptor == PixelDescriptor::RGBF32_LINEAR {
                for (x, bytes) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    bytes
                        .copy_from_slice(&(((x * 37 + y * 51) % 257) as f32 / 256.0).to_ne_bytes());
                }
            } else {
                for (x, byte) in out.iter_mut().enumerate() {
                    *byte = (x * 37 + y * 51) as u8;
                }
            }
        }
        let pixels = PixelSlice::new(&input, 33, 65, stride, descriptor).unwrap();
        for entropy in [imij::LossyEntropy::RunLevel, imij::LossyEntropy::SplitHuf] {
            let config = imij::ImiEncoderConfig::lossy(90.0)
                .tune_lossy(|t| t.entropy = entropy)
                .with_effort(2.5)
                .tune_lossy(|t| t.stripe_block_rows = 1)
                .with_checksum(true);
            let ordinary = config
                .clone()
                .job()
                .encoder()
                .unwrap()
                .encode(pixels.clone())
                .unwrap();
            let pulse = FnPulse::new("encode", |_| Ok(()));
            let observer = pulse.observer();
            let result = config
                .job()
                .with_progress(pulse.share().unwrap())
                .encoder()
                .unwrap()
                .encode(pixels.clone());
            assert!(!matches!(observer.snapshot().status, Status::Finished(_)));
            let output = pulse.complete(result).unwrap();
            assert_eq!(output.data(), ordinary.data(), "{descriptor:?} {entropy:?}");
            assert_eq!(
                imij::decode(output.data()).unwrap(),
                imij::decode(ordinary.data()).unwrap()
            );
            let snapshot = observer.snapshot();
            all_succeeded(&snapshot);
            assert_eq!(phase(&snapshot, "transform rows").unwrap().completed, 9);
            assert!(phase(&snapshot, "coefficient coding").unwrap().completed >= 9);
        }
    }
}

#[test]
fn lossy_cancellation_during_transform_and_coding_retains_phase_outcomes() {
    let input: Vec<u8> = (0..97 * 65 * 3).map(|i| (i * 29 + i / 131) as u8).collect();
    let pixels = PixelSlice::new(&input, 97, 65, 291, PixelDescriptor::RGB8_SRGB).unwrap();
    for entropy in [imij::LossyEntropy::RunLevel, imij::LossyEntropy::SplitHuf] {
        for target in ["transform rows", "coefficient coding"] {
            for parallel in [false, true] {
                let pulse = FnPulse::new("encode", move |checkpoint| {
                    if checkpoint.phase_name() == target
                        && phase(&checkpoint.snapshot(), target).is_some_and(|p| p.completed >= 1)
                    {
                        Err(StopReason::Cancelled)
                    } else {
                        Ok(())
                    }
                });
                let observer = pulse.observer();
                let mut job = imij::ImiEncoderConfig::lossy(90.0)
                    .tune_lossy(|t| t.entropy = entropy)
                    .with_effort(2.5)
                    .tune_lossy(|t| t.stripe_block_rows = 1)
                    .job()
                    .with_progress(pulse.share().unwrap());
                if !parallel {
                    job = job.with_limits(
                        ResourceLimits::none().with_threading(ThreadingPolicy::Sequential),
                    );
                }
                assert_eq!(
                    job.encoder().unwrap().encode(pixels.clone()).unwrap_err(),
                    Error::Cancelled
                );
                pulse.finish(Outcome::Cancelled).unwrap();
                let snapshot = observer.snapshot();
                let stopped = phase(&snapshot, target).unwrap();
                assert!(stopped.completed >= 1);
                assert_eq!(stopped.status, Status::Finished(Outcome::Cancelled));
                assert_eq!(
                    phase(&snapshot, "unit index").unwrap().status,
                    Status::Finished(Outcome::NotRun)
                );
                if target == "transform rows" {
                    assert_eq!(
                        phase(&snapshot, "coefficient coding").unwrap().status,
                        Status::Finished(Outcome::NotRun)
                    );
                    if !parallel {
                        assert_eq!(stopped.completed, 1);
                    }
                }
            }
        }
    }
}

#[test]
fn disabled_checksum_is_reported_as_skipped() {
    let input = [0u8; 12];
    let pixels = PixelSlice::new(&input, 2, 2, 6, PixelDescriptor::RGB8_SRGB).unwrap();
    let pulse = FnPulse::new("encode", |_| Ok(()));
    let observer = pulse.observer();
    let native = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.with_checksum(false);
        cfg
    };
    let result = native
        .job()
        .with_progress(pulse.share().unwrap())
        .encoder()
        .unwrap()
        .encode(pixels);
    let output = pulse.complete(result).unwrap();
    assert_eq!(imij::decode(output.data()).unwrap().1, input);
    assert_eq!(
        phase(&observer.snapshot(), "file checksum").unwrap().status,
        Status::Finished(Outcome::Skipped)
    );
}

#[test]
// Also checks the legacy job `with_stop`, deprecated in zencodec 0.2; imij keeps it.
#[allow(deprecated)]
fn legacy_stop_is_checked_during_native_lossless_work() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    #[derive(Clone)]
    struct AfterTen(Arc<AtomicUsize>);
    impl how_far::Stop for AfterTen {
        fn check(&self) -> Result<(), StopReason> {
            if self.0.fetch_add(1, Ordering::SeqCst) >= 10 {
                Err(StopReason::Cancelled)
            } else {
                Ok(())
            }
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let data = [17u8; 8 * 8 * 3];
    let pixels = PixelSlice::new(&data, 8, 8, 24, PixelDescriptor::RGB8_SRGB).unwrap();
    let result = imij::ImiEncoderConfig::default()
        .job()
        .with_limits(ResourceLimits::none().with_threading(ThreadingPolicy::Sequential))
        .with_stop(zencodec::StopToken::new(AfterTen(calls.clone())))
        .encoder()
        .unwrap()
        .encode(pixels);
    assert_eq!(result.unwrap_err(), Error::Cancelled);
    assert_eq!(calls.load(Ordering::SeqCst), 11);
}

#[test]
// Also checks the legacy job `with_stop`, deprecated in zencodec 0.2; imij keeps it.
#[allow(deprecated)]
fn legacy_stop_is_checked_during_native_lossy_work() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    #[derive(Clone)]
    struct AfterTen(Arc<AtomicUsize>);
    impl how_far::Stop for AfterTen {
        fn check(&self) -> Result<(), StopReason> {
            if self.0.fetch_add(1, Ordering::SeqCst) >= 10 {
                Err(StopReason::Cancelled)
            } else {
                Ok(())
            }
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let data = [17u8; 8 * 8 * 3];
    let pixels = PixelSlice::new(&data, 8, 8, 24, PixelDescriptor::RGB8_SRGB).unwrap();
    let result = imij::ImiEncoderConfig::default()
        .with_lossless(false)
        .job()
        .with_limits(ResourceLimits::none().with_threading(ThreadingPolicy::Sequential))
        .with_stop(zencodec::StopToken::new(AfterTen(calls.clone())))
        .encoder()
        .unwrap()
        .encode(pixels);
    assert_eq!(result.unwrap_err(), Error::Cancelled);
    assert_eq!(calls.load(Ordering::SeqCst), 11);
}
