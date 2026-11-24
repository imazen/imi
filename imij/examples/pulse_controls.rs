//! Executable controls for imij's zencodec Pulse adapters, for validation runs (not a CI test).
//!
//! One-shot stops: a stop that reports `TimedOut` on exactly its k-th check, for every k up to
//! the number of checks the uncancelled operation makes, through the legacy job `with_stop` and
//! through `with_pulse(SharedPulse::new(StopOnly::new(..)))`. Every k should fail the operation;
//! a k after which it still succeeds means some check consumed the stop without honouring it.
//!
//! Quiet pulses (`may_report() == false`): planning (split calls and the parts they request) is
//! counted apart from cancellation polls, for a quiet pulse that may stop and one that cannot,
//! beside the polls the legacy stop sees.
//!
//! `cargo run --release -p imij --example pulse_controls` (`just pulse-controls`). Prints one line
//! per control and exits with status 1 if any one-shot stop was swallowed.

use how_far::{
    Child, Execution, PhaseSpec, PlanError, Pulse, Report, SharedPulse, Stop, StopOnly, StopReason,
    Total,
};
use imij::codec::DecodeConfig;
use imij::{Channels, ImageInfo, ImageRef, ImiEncoderConfig, Metadata, SampleType};
use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use zencodec::StopToken;
use zencodec::decode::{Decode, DecodeJob, DecoderConfig};
use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
use zenpixels::{PixelDescriptor, PixelSlice};

const W: u32 = 97;
const H: u32 = 41;

/// Reports `TimedOut` on exactly its `at`-th check (1-based); `at == 0` never stops.
#[derive(Clone)]
struct OneShot {
    at: usize,
    calls: Arc<AtomicUsize>,
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

/// Counts checks and never stops; `may_stop` is what it claims.
#[derive(Clone)]
struct Counted {
    calls: Arc<AtomicUsize>,
    may_stop: bool,
}
impl Stop for Counted {
    fn check(&self) -> Result<(), StopReason> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn may_stop(&self) -> bool {
        self.may_stop
    }
}

/// Reports nowhere (`may_report() == false`) and counts planning. Its children come from
/// `StopOnly`, so their checks reach the same counted stop.
struct Quiet {
    stop: Counted,
    inner: StopOnly<'static>,
    splits: Arc<AtomicUsize>,
    parts: Arc<AtomicUsize>,
}
impl Quiet {
    fn new(stop: Counted, splits: Arc<AtomicUsize>, parts: Arc<AtomicUsize>) -> Self {
        let inner = StopOnly::new(stop.clone());
        Self {
            stop,
            inner,
            splits,
            parts,
        }
    }
}
impl Stop for Quiet {
    fn check(&self) -> Result<(), StopReason> {
        self.inner.check()
    }
    fn may_stop(&self) -> bool {
        self.inner.may_stop()
    }
}
impl Report for Quiet {
    fn advance(&self, _: u64) {}
    fn may_report(&self) -> bool {
        false
    }
}
impl Pulse for Quiet {
    fn split(
        &self,
        execution: Execution,
        parts: &[PhaseSpec<'_>],
    ) -> Result<Vec<Child<'_>>, PlanError> {
        self.splits.fetch_add(1, Ordering::SeqCst);
        self.parts.fetch_add(parts.len(), Ordering::SeqCst);
        self.inner.split(execution, parts)
    }
    fn set_total(&self, total: Total) -> Result<(), PlanError> {
        self.inner.set_total(total)
    }
    fn share(&self) -> Result<SharedPulse, PlanError> {
        Ok(SharedPulse::new(Quiet::new(
            self.stop.clone(),
            self.splits.clone(),
            self.parts.clone(),
        )))
    }
}

#[derive(Clone, Copy)]
enum Op {
    DecodeLossless,
    DecodeLossy,
    EncodeLossless,
    EncodeLossy,
}
impl Op {
    fn name(self) -> &'static str {
        match self {
            Op::DecodeLossless => "decode-lossless",
            Op::DecodeLossy => "decode-lossy",
            Op::EncodeLossless => "encode-lossless",
            Op::EncodeLossy => "encode-lossy",
        }
    }
}

#[derive(Clone, Copy)]
enum Path {
    Legacy,
    Pulse,
}
impl Path {
    fn name(self) -> &'static str {
        match self {
            Path::Legacy => "legacy with_stop",
            Path::Pulse => "with_pulse(StopOnly)",
        }
    }
    fn control(self, stop: impl Stop + 'static) -> Control {
        match self {
            Path::Legacy => Control::Legacy(StopToken::new(stop)),
            Path::Pulse => Control::Pulse(SharedPulse::new(StopOnly::new(stop))),
        }
    }
}

enum Control {
    Legacy(StopToken),
    Pulse(SharedPulse),
}

struct Inputs {
    raw: Vec<u8>,
    lossless: Vec<u8>,
    lossy: Vec<u8>,
}

fn info() -> ImageInfo {
    ImageInfo::new(W, H, Channels::Rgb, SampleType::U8)
}

/// Six lossless units (8-row stripes), one worker.
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

fn inputs() -> Inputs {
    let info = info();
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
    Inputs {
        raw,
        lossless,
        lossy,
    }
}

/// Runs `op` under `control`; the error is rendered with its zencodec category.
#[allow(deprecated)] // The legacy job `with_stop` is one of the two paths under test.
fn run(op: Op, inputs: &Inputs, control: Control) -> Result<(), String> {
    use zencodec::CategorizedError;
    match op {
        Op::DecodeLossless | Op::DecodeLossy => {
            let file = match op {
                Op::DecodeLossless => &inputs.lossless,
                _ => &inputs.lossy,
            };
            let job = DecodeConfig::default().job();
            let job = match control {
                Control::Legacy(stop) => job.with_stop(stop),
                Control::Pulse(pulse) => job.with_pulse(pulse),
            };
            job.decoder(Cow::Borrowed(&file[..]), &[])
                .and_then(|decoder| decoder.decode())
                .map(|_| ())
                .map_err(|e| format!("{e:?} as {:?}", e.category()))
        }
        Op::EncodeLossless | Op::EncodeLossy => {
            let config = match op {
                Op::EncodeLossless => lossless_config(),
                _ => lossy_config(),
            };
            let pixels = PixelSlice::new(
                &inputs.raw,
                W,
                H,
                W as usize * 3,
                PixelDescriptor::RGB8_SRGB,
            )
            .unwrap();
            let job = config.job();
            let job = match control {
                Control::Legacy(stop) => job.with_stop(stop),
                Control::Pulse(pulse) => job.with_pulse(pulse),
            };
            job.encoder()
                .and_then(|encoder| encoder.encode(pixels))
                .map(|_| ())
                .map_err(|e| format!("{e:?} as {:?}", e.category()))
        }
    }
}

struct Sweep {
    checks: usize,
    swallowed: Vec<usize>,
    first_error: Option<String>,
}

/// One run per check position k of the uncancelled operation, stopping only at k.
fn one_shot(op: Op, path: Path, inputs: &Inputs) -> Sweep {
    let calls = Arc::new(AtomicUsize::new(0));
    let stop = OneShot {
        at: 0,
        calls: calls.clone(),
    };
    run(op, inputs, path.control(stop)).expect("uncancelled run succeeds");
    let checks = calls.load(Ordering::SeqCst);
    let mut swallowed = Vec::new();
    let mut first_error = None;
    for at in 1..=checks {
        let stop = OneShot {
            at,
            calls: Arc::new(AtomicUsize::new(0)),
        };
        match run(op, inputs, path.control(stop)) {
            Ok(()) => swallowed.push(at),
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }
    Sweep {
        checks,
        swallowed,
        first_error,
    }
}

/// Polls through the legacy stop.
fn legacy_checks(op: Op, inputs: &Inputs) -> usize {
    let calls = Arc::new(AtomicUsize::new(0));
    let stop = Counted {
        calls: calls.clone(),
        may_stop: true,
    };
    run(op, inputs, Path::Legacy.control(stop)).expect("legacy run succeeds");
    calls.load(Ordering::SeqCst)
}

/// (split calls, parts requested, checks) through a quiet pulse.
fn quiet(op: Op, inputs: &Inputs, may_stop: bool) -> (usize, usize, usize) {
    let calls = Arc::new(AtomicUsize::new(0));
    let splits = Arc::new(AtomicUsize::new(0));
    let parts = Arc::new(AtomicUsize::new(0));
    let stop = Counted {
        calls: calls.clone(),
        may_stop,
    };
    let pulse = SharedPulse::new(Quiet::new(stop, splits.clone(), parts.clone()));
    run(op, inputs, Control::Pulse(pulse)).expect("quiet run succeeds");
    (
        splits.load(Ordering::SeqCst),
        parts.load(Ordering::SeqCst),
        calls.load(Ordering::SeqCst),
    )
}

fn main() {
    let inputs = inputs();
    let mut swallowed_any = false;
    for op in [
        Op::DecodeLossless,
        Op::DecodeLossy,
        Op::EncodeLossless,
        Op::EncodeLossy,
    ] {
        for path in [Path::Legacy, Path::Pulse] {
            let sweep = one_shot(op, path, &inputs);
            swallowed_any |= !sweep.swallowed.is_empty();
            let shown = &sweep.swallowed[..sweep.swallowed.len().min(16)];
            println!(
                "one-shot {} via {}: checks={} swallowed={}/{} at k={:?} first error: {}",
                op.name(),
                path.name(),
                sweep.checks,
                sweep.swallowed.len(),
                sweep.checks,
                shown,
                sweep.first_error.as_deref().unwrap_or("none"),
            );
        }
        let legacy = legacy_checks(op, &inputs);
        let (splits, parts, checks) = quiet(op, &inputs, true);
        let (splits_off, parts_off, checks_off) = quiet(op, &inputs, false);
        println!(
            "quiet {}: legacy stop checks={legacy}; quiet pulse that may stop: splits={splits} \
             parts={parts} checks={checks}; quiet pulse that cannot stop: splits={splits_off} \
             parts={parts_off} checks={checks_off}",
            op.name(),
        );
    }
    if swallowed_any {
        println!("FAIL: a one-shot stop was consumed without stopping the operation");
        std::process::exit(1);
    }
    println!("ok: every one-shot stop failed its operation");
}
