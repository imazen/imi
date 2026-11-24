//! One-shot stops through every zencodec job control. A stop that reports
//! `TimedOut` on exactly its k-th check must fail the operation for every k of
//! the uncancelled run: through the legacy job `with_stop`, a quiet
//! (cancellation-only) pulse and a reporting tree. Failures stay typed: each is
//! a zencodec `Stopped` error, and a reporting tree records a stopped decode
//! unit as cancelled, never failed. Unit spans exist only with imij's `std`
//! feature; without it, none may be planned. Whole-image lossless and lossy
//! decode and encode, one worker.

use how_far::{Outcome, Pulse, SharedPulse, Stop, StopOnly, StopReason};
use how_far_along::{Phase, PulseTree, Snapshot, Status, Total};
use imij::codec::DecodeConfig;
use imij::{Channels, ImageInfo, ImageRef, ImiEncoderConfig, Metadata, SampleType};
use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use zencodec::decode::{Decode, DecodeJob, DecoderConfig};
use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
use zencodec::{CategorizedError, ErrorCategory, StopToken};
use zenpixels::{PixelDescriptor, PixelSlice};

const W: u32 = 97;
const H: u32 = 41;

/// Reports `TimedOut` on exactly its `at`-th check (1-based); `at == 0` never stops.
#[derive(Clone)]
struct OneShot {
    at: usize,
    calls: Arc<AtomicUsize>,
}
impl OneShot {
    fn at(at: usize) -> Self {
        Self {
            at,
            calls: Arc::default(),
        }
    }
}
impl Stop for OneShot {
    fn check(&self) -> Result<(), StopReason> {
        if self.calls.fetch_add(1, Ordering::SeqCst) + 1 == self.at {
            Err(StopReason::TimedOut)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Op {
    DecodeLossless,
    DecodeLossy,
    EncodeLossless,
    EncodeLossy,
}

#[derive(Clone, Copy, Debug)]
enum Control {
    LegacyStop,
    QuietPulse,
    ReportingTree,
}

enum Applied {
    Stop(StopToken),
    Pulse(SharedPulse),
}

struct Fixture {
    raw: Vec<u8>,
    lossless: Vec<u8>,
    lossy: Vec<u8>,
}

/// Six lossless units (8-row stripes).
fn lossless_config() -> ImiEncoderConfig {
    ImiEncoderConfig::default()
        .with_effort(0.0)
        .with_threads(1)
        .tune_lossless(|t| t.stripe_rows = Some(8))
}

fn lossy_config() -> ImiEncoderConfig {
    ImiEncoderConfig::lossy(80.0)
        .with_threads(1)
        .tune_lossy(|t| t.stripe_block_rows = 1)
}

fn fixture() -> Fixture {
    let info = ImageInfo::new(W, H, Channels::Rgb, SampleType::U8);
    let raw: Vec<u8> = (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 29 + i / 131) as u8)
        .collect();
    let encode = |config: ImiEncoderConfig| {
        config
            .encode_samples(
                ImageRef::new(&raw, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap()
    };
    let lossless = encode(lossless_config());
    let lossy = encode(lossy_config());
    Fixture {
        raw,
        lossless,
        lossy,
    }
}

/// Runs `op` with `stop` given through `control`. Returns the error category on
/// failure and, for a reporting tree, its final snapshot.
#[allow(deprecated)] // The legacy job `with_stop` is one of the controls under test.
fn run(
    f: &Fixture,
    op: Op,
    control: Control,
    stop: OneShot,
) -> (Result<(), ErrorCategory>, Option<Snapshot>) {
    let tree = matches!(control, Control::ReportingTree)
        .then(|| PulseTree::new(Phase::new("operation", Total::Unknown), stop.clone()));
    let observer = tree.as_ref().map(|t| t.observer());
    let applied = match (control, &tree) {
        (Control::LegacyStop, _) => Applied::Stop(StopToken::new(stop)),
        (Control::QuietPulse, _) => Applied::Pulse(SharedPulse::new(StopOnly::new(stop))),
        (Control::ReportingTree, Some(tree)) => Applied::Pulse(tree.share().unwrap()),
        (Control::ReportingTree, None) => unreachable!(),
    };
    let result = match op {
        Op::DecodeLossless | Op::DecodeLossy => {
            let file = match op {
                Op::DecodeLossless => &f.lossless,
                _ => &f.lossy,
            };
            let job = DecodeConfig::default().job();
            let job = match applied {
                Applied::Stop(stop) => job.with_stop(stop),
                Applied::Pulse(pulse) => job.with_pulse(pulse),
            };
            job.decoder(Cow::Borrowed(&file[..]), &[])
                .and_then(|decoder| decoder.decode())
                .map(|_| ())
                .map_err(|e| e.category())
        }
        Op::EncodeLossless | Op::EncodeLossy => {
            let config = match op {
                Op::EncodeLossless => lossless_config(),
                _ => lossy_config(),
            };
            let pixels =
                PixelSlice::new(&f.raw, W, H, W as usize * 3, PixelDescriptor::RGB8_SRGB).unwrap();
            let job = config.job();
            let job = match applied {
                Applied::Stop(stop) => job.with_stop(stop),
                Applied::Pulse(pulse) => job.with_pulse(pulse),
            };
            job.encoder()
                .and_then(|encoder| encoder.encode(pixels))
                .map(|_| ())
                .map_err(|e| e.category())
        }
    };
    if let Some(tree) = tree {
        let outcome = if result.is_ok() {
            Outcome::Succeeded
        } else {
            Outcome::Cancelled
        };
        let _ = tree.finish(outcome);
    }
    (result, observer.map(|o| o.snapshot()))
}

/// Statuses of every decode unit ("stripe ...") in a snapshot tree.
fn unit_statuses(node: &Snapshot, out: &mut Vec<Status>) {
    for child in &node.children {
        if child.name.starts_with("stripe ") {
            out.push(child.status);
        }
        unit_statuses(child, out);
    }
}

#[test]
fn one_shot_stops_fail_every_operation_through_every_control() {
    let f = fixture();
    // Every violation is collected, so one failure reports all of them.
    let mut violations = Vec::new();
    for op in [
        Op::DecodeLossless,
        Op::DecodeLossy,
        Op::EncodeLossless,
        Op::EncodeLossy,
    ] {
        for control in [
            Control::LegacyStop,
            Control::QuietPulse,
            Control::ReportingTree,
        ] {
            let count = OneShot::at(0);
            let (result, _) = run(&f, op, control, count.clone());
            assert_eq!(result, Ok(()), "{op:?} {control:?}: uncancelled run");
            let checks = count.calls.load(Ordering::SeqCst);
            assert!(checks > 0, "{op:?} {control:?}: no checks");
            let mut lost = Vec::new();
            let mut cancelled_units = 0;
            let mut unit_spans = 0;
            for at in 1..=checks {
                let (result, snapshot) = run(&f, op, control, OneShot::at(at));
                match result {
                    Ok(()) => lost.push(at),
                    Err(category) if !matches!(category, ErrorCategory::Stopped(_)) => violations
                        .push(format!(
                            "{op:?} via {control:?} k={at}: a stop surfaced as {category:?}"
                        )),
                    Err(_) => {}
                }
                if let (Some(snapshot), Op::DecodeLossless | Op::DecodeLossy) = (snapshot, op) {
                    let mut statuses = Vec::new();
                    unit_statuses(&snapshot, &mut statuses);
                    if statuses.contains(&Status::Finished(Outcome::Failed)) {
                        violations.push(format!(
                            "{op:?} k={at}: a stopped unit reported as failed: {statuses:?}"
                        ));
                    }
                    if statuses.contains(&Status::Finished(Outcome::Cancelled)) {
                        cancelled_units += 1;
                    }
                    unit_spans += statuses.len();
                }
            }
            // imij plans unit spans only with its `std` feature (`UnitProgress::plan`):
            // with it, a stop must reach a unit span as cancelled; without it, no unit
            // span may exist at all.
            if matches!(control, Control::ReportingTree)
                && matches!(op, Op::DecodeLossless | Op::DecodeLossy)
            {
                if cfg!(feature = "std") && cancelled_units == 0 {
                    violations.push(format!(
                        "{op:?} via {control:?}: no stop was recorded on a unit span"
                    ));
                }
                if !cfg!(feature = "std") && unit_spans > 0 {
                    violations.push(format!(
                        "{op:?} via {control:?}: {unit_spans} unit spans planned without std"
                    ));
                }
            }
            if !lost.is_empty() {
                violations.push(format!(
                    "{op:?} via {control:?}: {} of {checks} one-shot stops swallowed at k={lost:?}",
                    lost.len()
                ));
            }
        }
    }
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}
