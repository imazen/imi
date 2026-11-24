//! `profile-decode THREADS REPS [WARMUPS] < FILE.imi`: decode timing of one IMI file
//! with input and output in memory (no IO in the timed region), then the unit
//! timeline of one more decode observed through a profiling pulse.
//!
//! Prints one JSON object: the engine (`native` or `wasm32`), threads, the
//! per-repetition milliseconds of plain decodes after a warm-up, their median
//! and megapixels per second, the observed decode's milliseconds, and its
//! spans (`task`, `kind`, `start_us`, `end_us`; times from the observed
//! decode's start). Lossless units appear as `stripe S` or
//! `stripe S group G` tasks; lossy payloads report only the root phase.

use std::fmt::Write as _;
use std::time::Instant;

use how_far::{Outcome, Unstoppable};
use how_far_along::{Phase, PulseTree, Total};
use how_far_really::diagnostics::DiagnosticPulse;
use how_far_really::profile::{Profiler, StdClock};

pub fn run(argv: &[String]) -> Result<(), String> {
    let usage = "usage: imij-wasm profile-decode THREADS REPS [WARMUPS] < FILE.imi";
    let threads: usize = argv.first().and_then(|t| t.parse().ok()).ok_or(usage)?;
    let reps: usize = argv.get(1).and_then(|r| r.parse().ok()).ok_or(usage)?;
    let warmups: usize = argv
        .get(2)
        .map(|w| w.parse().map_err(|_| usage))
        .transpose()?
        .unwrap_or(1);
    let data = crate::stdin_bytes()?;
    let info = imij::probe(&data).map_err(|e| format!("probe: {e}"))?.0;
    let stride = info.row_bytes();
    let len = info.checked_buffer_len().ok_or("image too large")?;
    let mut out = vec![0u8; len];
    let decode = |out: &mut [u8]| {
        imij::decode_into_threads(&data, out, stride, threads)
            .map(|_| ())
            .map_err(|e| format!("decode: {e}"))
    };
    for _ in 0..warmups {
        decode(&mut out)?;
    }
    let mut ms = Vec::with_capacity(reps);
    for _ in 0..reps {
        let t = Instant::now();
        decode(&mut out)?;
        ms.push(t.elapsed().as_secs_f64() * 1e3);
    }
    let mut sorted = ms.clone();
    sorted.sort_by(f64::total_cmp);
    let median = if sorted.is_empty() {
        0.0
    } else {
        sorted[sorted.len() / 2]
    };
    let pixels = f64::from(info.width) * f64::from(info.height);

    let profiler = Profiler::new(StdClock::new(), 1 << 16);
    let pulse = DiagnosticPulse::new(
        PulseTree::new(Phase::new("decode", Total::Unknown), Unstoppable),
        &profiler,
    );
    let t = Instant::now();
    let result =
        imij::lab::decode_into_threads_with_pulse(&data, &mut out, stride, threads, &pulse);
    let observed_ms = t.elapsed().as_secs_f64() * 1e3;
    let outcome = if result.is_ok() {
        Outcome::Succeeded
    } else {
        Outcome::Failed
    };
    pulse.finish(outcome).map_err(|e| format!("pulse: {e:?}"))?;
    profiler.operation_returned();
    result.map_err(|e| format!("observed decode: {e}"))?;
    let trace = profiler.snapshot();
    let origin = trace
        .spans
        .iter()
        .map(|s| s.start)
        .min()
        .unwrap_or_default();

    let mut o = String::new();
    let _ = write!(
        o,
        "{{\"engine\":\"{}\",\"tier\":\"{}\",\"threads\":{threads},\"reps\":{reps},\"warmups\":{warmups},\"ms\":[",
        if cfg!(target_arch = "wasm32") {
            "wasm32"
        } else {
            "native"
        },
        imij::simd_tier()
    );
    for (i, m) in ms.iter().enumerate() {
        let _ = write!(o, "{}{m:.4}", if i > 0 { "," } else { "" });
    }
    let _ = write!(
        o,
        "],\"median_ms\":{median:.4},\"mpx_s\":{:.2},\"observed_ms\":{observed_ms:.4},\"dropped_spans\":{},\"spans\":[",
        if median > 0.0 { pixels / (median / 1e3) / 1e6 } else { 0.0 },
        trace.dropped_spans
    );
    for (i, s) in trace.spans.iter().enumerate() {
        let _ = write!(
            o,
            "{}{{\"task\":\"{}\",\"kind\":\"{:?}\",\"start_us\":{:.1},\"end_us\":{:.1}}}",
            if i > 0 { "," } else { "" },
            s.task.replace('\\', "\\\\").replace('"', "\\\""),
            s.kind,
            (s.start - origin).as_secs_f64() * 1e6,
            (s.end.saturating_sub(origin)).as_secs_f64() * 1e6
        );
    }
    o.push_str("]}\n");
    crate::stdout_bytes(o.as_bytes())
}
