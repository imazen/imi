// `differential` target: decode each input with 1 thread and with 4
// threads; outputs must be byte-identical whenever both succeed, and both
// must fail together. Zero tolerance.
//
// The scalar tier is selected by the documented process-level `IMIJ_SCALAR`
// switch, which cannot be toggled inside a process (`std::env::set_var` is
// `unsafe`, and this crate forbids unsafe code). The scalar/AVX2 comparison
// therefore runs for every seed through the subprocess check in
// `imij/tests/fuzz_regression.rs` (child processes with `IMIJ_SCALAR=1` and
// `IMIJ_NO_AVX512=1` hash the same routes and the hashes are compared), and
// a campaign can be run with `IMIJ_SCALAR=1` to fuzz the scalar tier
// directly. Within a process this target always checks 1 vs 4 threads, plus
// the serial allocating route when it succeeds, and that no route writes
// into row padding.

include!("common_core.rs");

use imij::decode_resources::DecodeSession;
use imij::{decode_into_threads_with_limits, decode_with_limits, header};

/// Padding sentinel for the 1-thread buffer.
const PAD_ONE: u8 = 0x33;
/// Padding sentinel for the 4-thread buffer.
const PAD_FOUR: u8 = 0xc7;

/// Decoded logical pixels (no padding) of one route.
pub type Route = Result<Vec<u8>, ()>;

/// 1-thread and 4-thread outcomes for `data`, or `None` when the header is
/// unreadable or the image exceeds the fuzz limits (both routes would refuse
/// it identically; the subprocess check treats `None` as agreement too).
pub fn routes(data: &[u8]) -> Option<(Route, Route)> {
    let h = header(data).ok()?;
    let info = h.info;
    let len = info.checked_buffer_len()?;
    if info.width as u64 * info.height as u64 > MAX_PIXELS || len as u64 > MAX_BYTES {
        return None;
    }
    let rb = info.row_bytes();
    let height = info.height as usize;
    let max_pad = (MAX_BYTES as usize / height.max(1)).saturating_sub(rb);
    let stride = rb + (data.get(2).copied().unwrap_or(0) as usize % 16).min(max_pad);

    let run = |threads: usize, pad: u8| -> Route {
        let mut out = vec![pad; stride * height];
        match decode_into_threads_with_limits(data, &mut out, stride, threads, &decode_limits()) {
            Ok(_) => Ok(rows_tight(&out, stride, rb, height, pad)),
            Err(_) => Err(()),
        }
    };
    Some((run(1, PAD_ONE), run(4, PAD_FOUR)))
}

/// A decoded route's hash, or `Err` for a failed route. Unlike [`routes`],
/// this pair is only read by the stable tier check in `fuzz_regression.rs`.
#[allow(dead_code)]
pub type RouteHash = Result<u64, ()>;

/// Hashes of the two routes for the cross-process tier check. `None` when
/// [`routes`] is `None`. Only used by the stable tier check.
#[allow(dead_code)]
pub fn hash_routes(data: &[u8]) -> Option<(RouteHash, RouteHash)> {
    let (one, four) = routes(data)?;
    let hash = |r: Route| r.map(|px| fnv1a(&px));
    Some((hash(one), hash(four)))
}

pub fn run(data: &[u8]) {
    let Some((one, four)) = routes(data) else {
        return;
    };
    match (&one, &four) {
        (Ok(a), Ok(b)) => assert_eq!(a, b, "1-thread and 4-thread pixels differ"),
        (Err(()), Err(())) => {}
        _ => panic!("1-thread and 4-thread routes disagree on success"),
    }

    // The serial allocating route must agree with both.
    let serial = decode_with_limits(data, &decode_limits()).map(|(_, px)| px);
    match (&serial, &one) {
        (Ok(a), Ok(b)) => assert_eq!(a, b, "serial and threaded pixels differ"),
        (Err(_), Ok(_)) | (Ok(_), Err(())) => {
            panic!("serial and threaded routes disagree on success")
        }
        (Err(_), Err(())) => {}
    }

    // The session route must agree too.
    let mut session = DecodeSession::new(decode_policy());
    if let Ok((_, px)) = session.decode(data)
        && let Ok(reference) = &serial
    {
        assert_eq!(&px, reference, "session and serial pixels differ");
    }
}
