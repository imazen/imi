use super::*;
use crate::{DecodeSpeed, ImiEncoderConfig};
use zencodec::encode::{EncodeJob, Encoder, EncoderConfig, RowFillError};
use zencodec::output_patch::OutputAccess;
use zenpixels::{ChannelType, PixelSlice, SignalRange};

fn config() -> ImiEncoderConfig {
    ImiEncoderConfig::lossy(70.0).with_decode_speed(DecodeSpeed::Fast)
}
fn job() -> EncodeRequest {
    config().job().with_scratch(crate::codec::Scratch::Memory)
}
fn declaration() -> EncodeDeclaration<'static> {
    EncodeDeclaration::new(129, 97, PixelDescriptor::RGB8_SRGB)
}
fn exact() -> ExactDepth {
    ExactDepth::new(ChannelType::U8, SignalRange::Full, 1, 8).unwrap()
}
fn payload(job: &EncodeRequest) -> *const u8 {
    job.prepared
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .native
        .metadata()
        .exif
        .as_ref()
        .unwrap()
        .as_ptr()
}

#[test]
fn repeated_plan_and_rejected_changes_retain_the_actual_metadata_owner() {
    let mut metadata = zencodec::Metadata::none();
    metadata.exif = Some(Arc::from(vec![7; 8192]));
    let job = legacy_metadata_for_controls(job(), metadata);
    let d = declaration();
    let facts = job.plan_encode(&d).unwrap();
    let ptr = payload(&job);
    let measured = allocation_counter::measure(|| {
        assert_eq!(facts, job.plan_encode(&d).unwrap());
    });
    assert_eq!(
        measured.count_total, 0,
        "cache hit must not copy metadata or reprepare"
    );
    let color = ColorContext::from_cicp(zencodec::Cicp::SRGB);
    for changed in [
        EncodeDeclaration::new(130, 97, d.descriptor),
        EncodeDeclaration::new(129, 98, d.descriptor),
        EncodeDeclaration::new(129, 97, PixelDescriptor::RGBA8_SRGB),
        d.with_color(&color),
        d.with_exact_depth(exact()),
        d.with_access(OutputAccess::SeekBack),
    ] {
        assert!(job.plan_encode(&changed).is_err());
        assert_eq!(payload(&job), ptr);
        assert_eq!(job.plan_encode(&d).unwrap(), facts);
    }
    let mut enc = job.encoder().unwrap();
    assert!(
        enc.begin_declared(&d.with_access(OutputAccess::SeekBack))
            .is_err()
    );
    assert_eq!(payload(&enc.job), ptr);
    assert_eq!(enc.begin_declared(&d).unwrap(), facts);
    let active = enc.retained.as_ref().unwrap();
    assert!(
        active.session.is_none(),
        "begin must not materialize native state"
    );
    assert_eq!(
        active
            .prepared
            .as_ref()
            .unwrap()
            .metadata()
            .exif
            .as_ref()
            .unwrap()
            .as_ptr(),
        ptr
    );
    assert!(enc.job.prepared.lock().unwrap().is_none());
    assert!(enc.begin_declared(&d).is_err());
}

#[test]
fn refusal_then_valid_plan_keeps_settings_and_legacy_fallback() {
    let job = job();
    assert!(
        job.plan_encode(&EncodeDeclaration::new(0, 97, PixelDescriptor::RGB8_SRGB))
            .is_err()
    );
    assert!(job.prepared.lock().unwrap().is_none());
    assert!(job.plan_encode(&declaration()).is_ok());
    // A successful plan does not steal metadata/settings from the ordinary route.
    let bytes = vec![0; 129 * 97 * 3];
    let pixels = PixelSlice::new(&bytes, 129, 97, 129 * 3, PixelDescriptor::RGB8_SRGB).unwrap();
    let expected = config()
        .job()
        .with_scratch(crate::codec::Scratch::Memory)
        .encoder()
        .unwrap()
        .encode(pixels.clone())
        .unwrap();
    assert_eq!(
        job.encoder().unwrap().encode(pixels).unwrap().data(),
        expected.data()
    );
    let ordinary = ImiEncoderConfig::default().job();
    assert!(ordinary.plan_encode(&declaration()).is_err());
    assert!(
        ordinary
            .encoder()
            .unwrap()
            .encode(PixelSlice::new(&[0; 3], 1, 1, 3, PixelDescriptor::RGB8_SRGB).unwrap())
            .is_ok()
    );
}

#[test]
fn every_job_setter_invalidates_and_requires_a_new_plan() {
    macro_rules! check {
        ($method:ident($($arg:expr),*)) => {{
            let job = job();
            job.plan_encode(&declaration()).unwrap();
            let job = job.$method($($arg),*);
            assert!(job.prepared.lock().unwrap().is_none(), stringify!($method));
            let mut enc = job.encoder().unwrap();
            assert!(enc.begin_declared(&declaration()).is_err(), stringify!($method));
        }};
    }
    {
        let job = job();
        job.plan_encode(&declaration()).unwrap();
        let job = legacy_metadata_for_controls(job, zencodec::Metadata::none());
        assert!(job.prepared.lock().unwrap().is_none());
        assert!(
            job.encoder()
                .unwrap()
                .begin_declared(&declaration())
                .is_err()
        );
    }
    // The checked none policy is a no-op, not an equivalent setter. Preserve
    // the legacy invalidation assertion separately without claiming enforcement.
    {
        let job = job();
        job.plan_encode(&declaration()).unwrap();
        let job = legacy_policy_for_controls(job, zencodec::encode::EncodePolicy::none());
        assert!(job.prepared.lock().unwrap().is_none(), "with_policy");
        let mut enc = job.encoder().unwrap();
        assert!(enc.begin_declared(&declaration()).is_err(), "with_policy");
    }
    check!(with_limits(zencodec::ResourceLimits::none()));
    check!(with_canvas_size(129, 97));
    check!(with_loop_count(Some(1)));
    check!(with_input_descriptor(PixelDescriptor::RGB8_SRGB));
    check!(with_color_origin(zenpixels::ColorOrigin::from_icc(vec![
        1, 2
    ])));
    check!(with_scratch(crate::codec::Scratch::Memory));
    check!(with_pulse(how_far::SharedPulse::new(&how_far::NoPulse)));
    check!(with_progress(how_far::SharedPulse::new(&how_far::NoPulse)));
    // Exercise the existing cancellation-only compatibility setter itself.
    {
        let job = job();
        job.plan_encode(&declaration()).unwrap();
        #[allow(deprecated)] // Only this explicit legacy setter control.
        let job = job.with_stop(zencodec::StopToken::new(Cancel(Arc::new(
            std::sync::atomic::AtomicBool::new(false),
        ))));
        assert!(job.prepared.lock().unwrap().is_none(), "with_stop");
        let mut enc = job.encoder().unwrap();
        assert!(enc.begin_declared(&declaration()).is_err(), "with_stop");
    }
    let job = job();
    job.plan_encode(&declaration()).unwrap();
    let changed = job.with_canvas_size(65, 17);
    assert!(changed.plan_encode(&declaration()).is_err());
    let d = EncodeDeclaration::new(65, 17, PixelDescriptor::RGB8_SRGB);
    let facts = changed.plan_encode(&d).unwrap();
    assert_eq!(
        changed.encoder().unwrap().begin_declared(&d).unwrap(),
        facts
    );
}

#[test]
fn current_color_and_exact_depth_are_not_overridden_by_job_metadata() {
    let d = declaration();
    let mut metadata = zencodec::Metadata::none();
    metadata.icc_profile = Some(Arc::from([1, 2, 3]));
    let job = legacy_metadata_for_controls(job(), metadata);
    assert!(job.plan_encode(&d).is_err());
    let context = ColorContext::from_icc(Arc::<[u8]>::from([1, 2, 3]));
    assert!(job.plan_encode(&d.with_color(&context)).is_err());
    let context = ColorContext::from_cicp(zencodec::Cicp::SRGB);
    let facts = super::tests::job()
        .plan_encode(&d.with_color(&context))
        .unwrap();
    assert!(facts.carriers.unwrap().icc.is_none());
    let mut metadata = zencodec::Metadata::none();
    metadata.cicp = Some(zencodec::Cicp::DISPLAY_P3);
    assert!(
        legacy_metadata_for_controls(super::tests::job(), metadata)
            .plan_encode(&d)
            .is_err()
    );
    let mismatched = ExactDepth::new(ChannelType::U16, SignalRange::Full, 8, 16).unwrap();
    assert!(
        super::tests::job()
            .plan_encode(&d.with_exact_depth(mismatched))
            .is_err()
    );
    assert!(
        super::tests::job()
            .plan_encode(&d.with_exact_depth(exact()))
            .is_ok()
    );
}

// These controls retain the published best-effort setter's behavior. They do
// not claim checked metadata retention, which the adapter does not implement.
#[allow(deprecated)]
fn legacy_metadata_for_controls(job: EncodeRequest, metadata: zencodec::Metadata) -> EncodeRequest {
    job.with_metadata_policy(metadata, zencodec::MetadataPolicy::PreserveExact)
}

// Preserve legacy invalidation and carrier observations explicitly; checked
// refusal is tested separately. No checked equivalent exists for nonempty requests: the native
// adapter inherits try_set_policy's default refusal. Do not infer enforcement
// of ColorEmitPolicy from these carrier observations.
#[allow(deprecated)]
fn legacy_policy_for_controls(
    job: EncodeRequest,
    policy: zencodec::encode::EncodePolicy,
) -> EncodeRequest {
    job.with_policy(policy)
}

#[test]
fn legacy_origin_carriers_are_truthful_but_specified_checked_policies_are_unsupported() {
    // Deliberately conflicting historical description must not become active.
    let mut origin = zenpixels::ColorOrigin::from_icc(vec![1, 2, 3, 4]);
    origin.cicp = Some(zencodec::Cicp::DISPLAY_P3);
    let mut admitted = 0;
    let mut refused = 0;
    // Explicit expected legacy-entry matrix, independent of carrier checks.
    // Checked setters above legacy entry still reject every specified policy.
    for (color, legacy_admitted) in [
        (None, true),
        (Some(zencodec::ColorEmitPolicy::Compatibility), false),
        (Some(zencodec::ColorEmitPolicy::Balanced), false),
        (Some(zencodec::ColorEmitPolicy::Compact), false),
        (Some(zencodec::ColorEmitPolicy::Verbatim), true),
    ] {
        for embed_icc in [None, Some(true), Some(false)] {
            let mut policy = zencodec::encode::EncodePolicy::none();
            policy.color = color;
            policy.embed_icc = embed_icc;
            let mut job = job().with_color_origin(origin.clone());
            let d = declaration();
            let before = job.plan_encode(&d).unwrap();
            let accepted = job.try_set_policy(policy);
            if policy == zencodec::encode::EncodePolicy::none() {
                assert_eq!(accepted, Ok(()));
            } else {
                assert_eq!(accepted, Err(zencodec::UnsupportedOperation::EncodePolicy));
            }
            // Checked acceptance of none and rejection of specified fields both
            // leave the prepared owner unchanged; neither applies this policy.
            assert!(job.prepared.lock().unwrap().is_some());
            assert_eq!(job.plan_encode(&d).unwrap(), before);
            let job = if policy == zencodec::encode::EncodePolicy::none() {
                job.try_with_policy(policy).unwrap()
            } else {
                // Continue the original legacy-carrier controls with every
                // assertion intact. The request above was explicitly refused.
                legacy_policy_for_controls(job, policy)
            };
            if !legacy_admitted {
                // plan_retained calls job.validate before cache/metadata lowering.
                // These legacy requests explicitly refuse, not silently downgrade.
                let error = job.plan_encode(&d).unwrap_err();
                assert_eq!(
                    error.downcast_ref::<Error>(),
                    Some(&Error::Unsupported(
                        "color-carrier rewriting in IMI adapter"
                    ))
                );
                assert!(job.prepared.lock().unwrap().is_none());
                assert_eq!(job.policy, policy);
                assert_eq!(job.color_origin.as_ref(), Some(&origin));
                refused += 1;
                continue;
            }
            admitted += 1;
            let facts = job.plan_encode(&d).unwrap();
            let carriers = facts.carriers.as_ref().unwrap();
            {
                let cache = job.prepared.lock().unwrap();
                let native = &cache.as_ref().unwrap().native;
                assert!(native.metadata().icc.is_none());
                assert!(carriers.icc.is_none());
                assert_eq!(carriers.cicp, Some(zencodec::Cicp::SRGB));
                assert_eq!(
                    native.metadata().color_origin().unwrap(),
                    (embed_icc != Some(false)).then(|| origin.clone())
                );
            }
            let mut enc = job.encoder().unwrap();
            assert_eq!(enc.begin_declared(&d).unwrap(), facts);
            let mut rows = raster(d.descriptor);
            while enc.pull_rows(&mut rows, None).unwrap() != PullProgress::Done {}
            let output = enc.finish().unwrap();
            let (info, metadata) = crate::probe(output.data()).unwrap();
            assert!(metadata.icc.is_none());
            assert_eq!(
                carriers.cicp,
                Some(zencodec::Cicp::new(
                    info.cicp.color_primaries,
                    info.cicp.transfer_characteristics,
                    info.cicp.matrix_coefficients,
                    info.cicp.full_range,
                ))
            );
            assert_eq!(
                metadata.color_origin().unwrap(),
                (embed_icc != Some(false)).then(|| origin.clone())
            );
        }
    }
    assert_eq!((admitted, refused), (6, 9));
}

#[test]
fn drop_before_start_releases_cache_and_shared_metadata() {
    let metadata: Arc<[u8]> = Arc::from(vec![8; 8192]);
    let weak = Arc::downgrade(&metadata);
    let mut meta = zencodec::Metadata::none();
    meta.exif = Some(metadata);
    let job = legacy_metadata_for_controls(job(), meta);
    job.plan_encode(&declaration()).unwrap();
    assert!(weak.upgrade().is_some());
    drop(job);
    assert!(weak.upgrade().is_none());
}

struct Raster {
    bytes: Vec<u8>,
    stride: usize,
    last: Option<(u32, u32)>,
    calls: usize,
    fail: bool,
    corrupt: bool,
    cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
}
impl EncodeRows for Raster {
    fn fill(&mut self, mut demand: RowDemand<'_>) -> Result<(), RowFillError> {
        if let Some((pass, end)) = self.last {
            if demand.pass == pass {
                assert_eq!(demand.first, end);
            } else {
                assert_eq!(demand.pass, pass + 1);
                assert_eq!(demand.first, 0);
                assert_eq!(end, 97);
            }
        } else {
            assert_eq!((demand.pass, demand.first), (0, 0));
        }
        self.calls += 1;
        if self.fail {
            return Err(RowFillError);
        }
        for y in 0..demand.rows.rows() {
            let source = (demand.first + y) as usize * self.stride;
            let row = demand.rows.row_mut(y);
            row.copy_from_slice(&self.bytes[source..source + row.len()]);
        }
        if self.corrupt {
            demand.rows.row_mut(0)[0] = 1;
        }
        if let Some(cancel) = &self.cancel {
            cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        self.last = Some((demand.pass, demand.first + demand.rows.rows()));
        Ok(())
    }
}
fn raster(descriptor: PixelDescriptor) -> Raster {
    let stride = (129 + 3) * descriptor.bytes_per_pixel();
    let mut bytes = vec![0xa5; stride * 97];
    for y in 0..97 {
        for x in 0..129 * descriptor.bytes_per_pixel() {
            bytes[y * stride + x] = ((x + y * 13) % 256) as u8;
        }
    }
    Raster {
        bytes,
        stride,
        calls: 0,
        last: None,
        fail: false,
        corrupt: false,
        cancel: None,
    }
}

#[test]
fn erased_native_session_moves_between_fills_and_matches_ordinary_bytes() {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    assert_send::<ImageEncoder>();
    assert_send::<EncodeRequest>();
    assert_sync::<EncodeRequest>();
    for descriptor in [
        PixelDescriptor::GRAY8_SRGB,
        PixelDescriptor::GRAYA8_SRGB,
        PixelDescriptor::RGB8_SRGB,
        PixelDescriptor::RGBA8_SRGB,
        PixelDescriptor::GRAY16_SRGB,
        PixelDescriptor::GRAYA16_SRGB,
        PixelDescriptor::RGB16_SRGB,
        PixelDescriptor::RGBA16_SRGB,
    ] {
        for checksum in [false, true] {
            for access in [OutputAccess::Forward, OutputAccess::SeekBack] {
                let mut source = raster(descriptor);
                let cfg = config().with_checksum(checksum);
                let make_job = || {
                    cfg.clone()
                        .job()
                        .with_scratch(crate::codec::Scratch::Memory)
                };
                let expected = make_job()
                    .encoder()
                    .unwrap()
                    .encode(
                        PixelSlice::new(&source.bytes, 129, 97, source.stride, descriptor).unwrap(),
                    )
                    .unwrap();
                let d = EncodeDeclaration::new(129, 97, descriptor).with_access(access);
                let job = make_job();
                let facts = job.plan_encode(&d).unwrap();
                assert_eq!(facts.timing, OutputTiming::AT_FINISH);
                let mut enc = job.dyn_encoder().unwrap();
                assert_eq!(enc.begin_declared(&d).unwrap(), facts);
                assert_eq!(
                    enc.pull_rows(&mut source, None).unwrap(),
                    PullProgress::Filled
                );
                assert!(enc.pending_output().is_empty());
                let output = std::thread::spawn(move || {
                    loop {
                        assert!(enc.pending_output().is_empty());
                        if enc.pull_rows(&mut source, None).unwrap() == PullProgress::Done {
                            break;
                        }
                    }
                    assert!(source.calls > 1);
                    enc.finish_output().unwrap().tail
                })
                .join()
                .unwrap();
                assert_eq!(output.data(), expected.data());
            }
        }
    }
}

#[derive(Clone)]
struct Cancel(Arc<std::sync::atomic::AtomicBool>);
impl zencodec::enough::Stop for Cancel {
    fn check(&self) -> Result<(), zencodec::enough::StopReason> {
        if self.0.load(std::sync::atomic::Ordering::SeqCst) {
            Err(zencodec::enough::StopReason::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[test]
fn failed_fill_bad_samples_and_cancellation_are_terminal_without_publication() {
    for mode in 0..4 {
        let d = if mode == 1 {
            declaration().with_exact_depth(exact())
        } else {
            declaration()
        };
        let job = job();
        job.plan_encode(&d).unwrap();
        let mut enc = job.encoder().unwrap();
        enc.begin_declared(&d).unwrap();
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(mode == 2));
        let pulse = how_far::StopOnly::new(Cancel(flag.clone()));
        let mut rows = raster(d.descriptor);
        rows.fail = mode == 0;
        rows.corrupt = mode == 1;
        if mode == 3 {
            rows.cancel = Some(flag.clone());
        }
        assert!(enc.pull_rows(&mut rows, Some(&pulse)).is_err());
        assert!(enc.retained.is_none());
        assert!(enc.pending_output().is_empty());
        let calls = rows.calls;
        assert!(enc.pull_rows(&mut rows, None).is_err());
        assert_eq!(rows.calls, calls);
        if mode == 0 {
            // A later job stop must not replace the first native failure.
            enc.job.stop = Some(zencodec::StopToken::new(Cancel(flag.clone())));
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            assert!(matches!(enc.finish(), Err(Error::Read)));
        } else {
            assert!(enc.finish().is_err());
        }
        if mode == 2 {
            assert_eq!(calls, 0);
        }
    }
}

#[test]
fn unqualified_memory_cap_refuses_without_consuming_legacy_settings() {
    let mut limits = zencodec::ResourceLimits::none();
    limits.max_memory_bytes = Some(1 << 30);
    let job = job().with_limits(limits);
    assert!(job.plan_encode(&declaration()).is_err());
    assert!(job.prepared.lock().unwrap().is_none());
    let bytes = vec![0; 129 * 97 * 3];
    assert!(
        job.encoder()
            .unwrap()
            .encode(PixelSlice::new(&bytes, 129, 97, 129 * 3, PixelDescriptor::RGB8_SRGB).unwrap())
            .is_ok()
    );
}

#[test]
fn scratch_start_refusal_keeps_preparation_for_the_same_declaration() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../target/retained-job-tests")
        .join(format!("{}-{unique}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let dir = root.join("not-created-yet");
    let mut metadata = zencodec::Metadata::none();
    metadata.exif = Some(Arc::from([4, 5, 6]));
    let job = legacy_metadata_for_controls(job(), metadata)
        .with_scratch(crate::codec::Scratch::Dir(dir.clone()));
    let d = declaration();
    let facts = job.plan_encode(&d).unwrap();
    let ptr = payload(&job);
    let mut enc = job.encoder().unwrap();
    assert!(enc.begin_declared(&d).is_err());
    assert_eq!(payload(&enc.job), ptr);
    assert!(enc.retained.is_none());
    assert!(enc.failed.is_none());
    std::fs::create_dir(&dir).unwrap();
    assert_eq!(enc.begin_declared(&d).unwrap(), facts);
    drop(enc);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    std::fs::remove_dir(&dir).unwrap();
    std::fs::remove_dir(&root).unwrap();
}

#[test]
fn a_later_contradictory_strip_and_callback_unwind_cannot_resume() {
    let job = job();
    let d = declaration().with_exact_depth(exact());
    job.plan_encode(&d).unwrap();
    let mut enc = job.encoder().unwrap();
    enc.begin_declared(&d).unwrap();
    let mut rows = raster(d.descriptor);
    rows.bytes.fill(0);
    assert_eq!(
        enc.pull_rows(&mut rows, None).unwrap(),
        PullProgress::Filled
    );
    rows.corrupt = true;
    assert!(enc.pull_rows(&mut rows, None).is_err());
    assert_eq!(rows.calls, 2);
    assert!(enc.retained.is_none());
    assert!(enc.pending_output().is_empty());
    assert!(enc.finish().is_err());

    struct PanicRows;
    impl EncodeRows for PanicRows {
        fn fill(&mut self, _: RowDemand<'_>) -> Result<(), RowFillError> {
            panic!("caller unwind")
        }
    }
    let job = super::tests::job();
    job.plan_encode(&declaration()).unwrap();
    let mut enc = job.encoder().unwrap();
    enc.begin_declared(&declaration()).unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = enc.pull_rows(&mut PanicRows, None);
    }));
    assert!(result.is_err());
    assert!(enc.retained.is_none());
    assert!(enc.failed.is_some());
    assert!(enc.pull_rows(&mut PanicRows, None).is_err());
    assert!(enc.pending_output().is_empty());
}

#[test]
fn output_limit_and_mixed_protocol_are_terminal() {
    let mut limits = zencodec::ResourceLimits::none();
    limits.max_output_bytes = Some(1);
    let job = job().with_limits(limits);
    job.plan_encode(&declaration()).unwrap();
    let mut enc = job.encoder().unwrap();
    enc.begin_declared(&declaration()).unwrap();
    let mut rows = raster(declaration().descriptor);
    loop {
        match enc.pull_rows(&mut rows, None) {
            Ok(PullProgress::Filled) => {}
            Err(Error::TooLarge) => break,
            other => panic!("expected output cap refusal, got {other:?}"),
        }
    }
    assert!(enc.retained.is_none());
    assert!(enc.pending_output().is_empty());
    assert!(enc.finish().is_err());
    let job = super::tests::job();
    job.plan_encode(&declaration()).unwrap();
    let mut enc = job.encoder().unwrap();
    enc.begin_declared(&declaration()).unwrap();
    let pixels = PixelSlice::new(&[0; 3], 1, 1, 3, PixelDescriptor::RGB8_SRGB).unwrap();
    assert!(enc.push_rows(pixels).is_err());
    assert!(enc.retained.is_none());
    assert!(enc.pull_rows(&mut rows, None).is_err());
}

#[test]
fn absent_or_noop_call_control_cannot_disable_retained_job_cancellation() {
    for call_control in [None, Some(&how_far::NoPulse as &dyn zencodec::Pulse)] {
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let job = job().with_pulse(how_far::SharedPulse::new(how_far::StopOnly::new(Cancel(
            flag.clone(),
        ))));
        let d = declaration();
        job.plan_encode(&d).unwrap();
        let mut encoder = job.encoder().unwrap();
        encoder.begin_declared(&d).unwrap();
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        let mut rows = raster(d.descriptor);
        assert!(matches!(
            encoder.pull_rows(&mut rows, call_control),
            Err(Error::Cancelled)
        ));
        assert_eq!(rows.calls, 0);
        assert!(encoder.retained.is_none());
        assert!(encoder.pending_output().is_empty());
        assert!(encoder.pull_rows(&mut rows, None).is_err());
        assert_eq!(rows.calls, 0);
    }
}

#[cfg(feature = "lab")]
mod streaming {
    use super::*;
    use crate::codec::{DecodeConfig, FeedDecoder, FeedStatus};
    use crate::stream_encode::{BoundedLossySession, BoundedLossyStep, BoundedStream};
    use zencodec::decode::DecoderConfig as _;

    fn config(entropy: crate::LossyEntropy) -> ImiEncoderConfig {
        ImiEncoderConfig::lossy(80.0)
            .with_effort(0.0)
            .with_prefer_streaming(true)
            .tune_lossy(|t| {
                t.aq_gamma = 0.0;
                t.stripe_block_rows = 1;
                t.entropy = entropy;
            })
    }

    fn native(cfg: &ImiEncoderConfig, source: &Raster, d: &EncodeDeclaration<'_>) -> Vec<u8> {
        let (info, meta) = encode_description_for(
            d.descriptor,
            (d.width, d.height),
            d.color,
            &zencodec::Metadata::none(),
            &zencodec::encode::EncodePolicy::none(),
            None,
        )
        .unwrap();
        let mut session =
            BoundedLossySession::one_pass(info, &meta, cfg, 16 << 20, 64 << 20).unwrap();
        let mut output = Vec::new();
        let mut index = Vec::new();
        let mut checksum = Vec::new();
        loop {
            match session.step().unwrap() {
                BoundedLossyStep::Rows { pass, y0, rows } => {
                    assert_eq!(pass, CODING_PASS);
                    let stride = d.width as usize * d.descriptor.bytes_per_pixel();
                    for (y, row) in rows.chunks_mut(stride).enumerate() {
                        let at = (y0 as usize + y) * source.stride;
                        row.copy_from_slice(&source.bytes[at..at + stride]);
                    }
                }
                BoundedLossyStep::Write {
                    stream,
                    offset,
                    bytes,
                } => {
                    let store = match stream {
                        BoundedStream::Output => &mut output,
                        BoundedStream::IndexScratch => &mut index,
                        BoundedStream::ChecksumScratch => &mut checksum,
                    };
                    assert_eq!(offset as usize, store.len());
                    store.extend_from_slice(bytes);
                }
                BoundedLossyStep::Read {
                    stream,
                    offset,
                    buf,
                } => {
                    let store = match stream {
                        BoundedStream::IndexScratch => &index,
                        BoundedStream::ChecksumScratch => &checksum,
                        _ => panic!("unknown native replay"),
                    };
                    buf.copy_from_slice(&store[offset as usize..][..buf.len()]);
                }
                BoundedLossyStep::Done { len } => {
                    assert_eq!(len as usize, output.len());
                    return output;
                }
            }
        }
    }

    fn feed(
        decoder: &mut FeedDecoder,
        input: &[u8],
        pixels: &mut Vec<u8>,
        row_bytes: usize,
    ) -> bool {
        let mut at = 0;
        loop {
            match decoder.advance().unwrap() {
                FeedStatus::NeedInput if at == input.len() => return false,
                FeedStatus::NeedInput => {
                    let n = decoder.push(&input[at..]).unwrap();
                    assert!(n > 0);
                    at += n;
                }
                FeedStatus::Row { y } => {
                    assert_eq!(y as usize, pixels.len() / row_bytes);
                    pixels.extend_from_slice(decoder.row().unwrap().unwrap().row(0));
                }
                FeedStatus::Complete => {
                    assert_eq!(at, input.len());
                    return true;
                }
            }
        }
    }

    #[test]
    fn common_one_pass_decodes_each_band_before_requesting_the_next_source() {
        for descriptor in [
            PixelDescriptor::GRAY8_SRGB,
            PixelDescriptor::GRAYA8_SRGB,
            PixelDescriptor::RGB8_SRGB,
            PixelDescriptor::RGBA8_SRGB,
            PixelDescriptor::GRAY16_SRGB,
            PixelDescriptor::GRAYA16_SRGB,
            PixelDescriptor::RGB16_SRGB,
            PixelDescriptor::RGBA16_SRGB,
        ] {
            for entropy in [crate::LossyEntropy::RunLevel, crate::LossyEntropy::SplitHuf] {
                for checksum in [false, true] {
                    let cfg = config(entropy).with_checksum(checksum);
                    let mut source = raster(descriptor);
                    let d = EncodeDeclaration::new(129, 97, descriptor);
                    let expected = native(&cfg, &source, &d);
                    let job = cfg.job().with_scratch(crate::codec::Scratch::Memory);
                    let facts = job.plan_encode(&d).unwrap();
                    assert_eq!(
                        facts.input,
                        zencodec::encode::InputRequirement::Replay(SourcePasses::ONCE)
                    );
                    assert_eq!(facts.timing, OutputTiming::streaming(8));
                    let mut enc = job.encoder().unwrap();
                    assert_eq!(enc.begin_declared(&d).unwrap(), facts);
                    let mut decoder = DecodeConfig::default().job().feed_decoder(17).unwrap();
                    let row_bytes = d.width as usize * descriptor.bytes_per_pixel();
                    let mut pixels = Vec::new();
                    let mut output = Vec::new();
                    loop {
                        let calls = source.calls;
                        let progress = enc.pull_rows(&mut source, None).unwrap();
                        if progress == PullProgress::Done {
                            assert_eq!(source.calls, calls);
                            break;
                        }
                        assert_eq!(source.calls, calls + 1, "one fill per return");
                        assert!(source.last.is_some_and(|(pass, _)| pass == 0));
                        let end = source.last.unwrap().1 as usize;
                        // Stall both producer and consumer here. Nothing runs
                        // in the background and no later source is requested.
                        assert!(!enc.pending_output().is_empty());
                        let pending = enc.pending_output().to_vec();
                        assert_eq!(enc.pending_output(), pending);
                        assert!(!feed(&mut decoder, &pending, &mut pixels, row_bytes));
                        assert_eq!(pixels.len() / row_bytes, end, "decode before next fill");
                        // Mix partial prefix drainage and the generic take API.
                        let first = pending.len() / 3;
                        output.extend_from_slice(&pending[..first]);
                        enc.consume_output(first);
                        enc.take_output(&mut output);
                        assert!(enc.pending_output().is_empty());
                        let active = enc.retained.as_ref().unwrap();
                        assert!(
                            active.output.as_bytes().is_empty(),
                            "drained bytes retained"
                        );
                        assert_eq!(active.published as usize, output.len());
                    }
                    output.extend_from_slice(enc.finish().unwrap().data());
                    assert_eq!(output, expected, "shared/native byte disagreement");
                    decoder.finish_input().unwrap();
                    assert!(feed(&mut decoder, &[], &mut pixels, row_bytes));
                    assert_eq!(pixels, crate::decode(&expected).unwrap().1);
                }
            }
        }
    }

    #[test]
    fn common_one_pass_refuses_unsupported_preflight_and_stops_after_published_failure() {
        let d = declaration();
        let unsupported = config(crate::LossyEntropy::RunLevel)
            .with_effort(7.5)
            .job()
            .with_scratch(crate::codec::Scratch::Memory);
        assert!(unsupported.plan_encode(&d).is_err());
        assert!(unsupported.prepared.lock().unwrap().is_none());
        for cancel in [false, true] {
            let job = config(crate::LossyEntropy::RunLevel)
                .job()
                .with_scratch(crate::codec::Scratch::Memory);
            job.plan_encode(&d).unwrap();
            let mut enc = job.encoder().unwrap();
            enc.begin_declared(&d).unwrap();
            let mut source = raster(d.descriptor);
            assert_eq!(
                enc.pull_rows(&mut source, None).unwrap(),
                PullProgress::Filled
            );
            let published = enc.pending_output().to_vec();
            assert!(!published.is_empty());
            enc.consume_output(published.len());
            let expected = if cancel {
                enc.job.stop = Some(zencodec::StopToken::new(Cancel(Arc::new(
                    std::sync::atomic::AtomicBool::new(true),
                ))));
                Error::Cancelled
            } else {
                source.fail = true;
                Error::Read
            };
            assert_eq!(enc.pull_rows(&mut source, None), Err(expected.clone()));
            let calls = source.calls;
            assert!(enc.pending_output().is_empty());
            assert_eq!(enc.pull_rows(&mut source, None), Err(expected.clone()));
            assert_eq!(source.calls, calls);
            assert!(matches!(enc.finish(), Err(e) if e == expected));
            assert!(
                crate::decode(&published).is_err(),
                "prefix must not validate as complete"
            );
        }
    }

    #[test]
    fn common_one_pass_retains_tail_and_enforces_total_output_after_drains() {
        let cfg = config(crate::LossyEntropy::RunLevel).with_checksum(true);
        let d = declaration();
        let mut source = raster(d.descriptor);
        let expected = native(&cfg, &source, &d);
        let job = cfg
            .clone()
            .job()
            .with_scratch(crate::codec::Scratch::Memory);
        job.plan_encode(&d).unwrap();
        let mut enc = job.encoder().unwrap();
        enc.begin_declared(&d).unwrap();
        enc.pull_rows(&mut source, None).unwrap();
        let first = enc.pending_output().to_vec();
        enc.consume_output(first.len());
        while enc.pull_rows(&mut source, None).unwrap() != PullProgress::Done {}
        let mut collected = first;
        collected.extend_from_slice(enc.finish().unwrap().data());
        assert_eq!(collected, expected);

        let job = cfg
            .job()
            .with_scratch(crate::codec::Scratch::Memory)
            .with_limits(
                zencodec::ResourceLimits::none().with_max_output((expected.len() - 1) as u64),
            );
        job.plan_encode(&d).unwrap();
        let mut enc = job.encoder().unwrap();
        enc.begin_declared(&d).unwrap();
        let mut source = raster(d.descriptor);
        let mut drained = 0;
        loop {
            match enc.pull_rows(&mut source, None) {
                Ok(PullProgress::Filled) => {
                    let n = enc.pending_output().len();
                    drained += n;
                    enc.consume_output(n);
                }
                Err(Error::TooLarge) => break,
                other => panic!("must refuse total output cap: {other:?}"),
            }
        }
        assert!(drained > 0, "exercise a failure after actual publication");
        assert!(enc.pending_output().is_empty());
        let calls = source.calls;
        assert_eq!(enc.pull_rows(&mut source, None), Err(Error::TooLarge));
        assert_eq!(source.calls, calls);
        assert!(matches!(enc.finish(), Err(Error::TooLarge)));
    }
}
