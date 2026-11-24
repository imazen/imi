//! Run tokens (SPEC 3.1, 3.2): a run covers at most 2^15 positions, and only
//! positions of its own lane in its unit. A model that gives weight to a run
//! token whose shortest run is longer, and a run that reaches past its
//! lane's last position, are `Corrupt` on every route and every decoder
//! tier. Before the model rule the narrow SIMD tables held a 16-bit run
//! token's extra-bit width as 15, so the scalar and AVX2/AVX-512 decoders
//! read the same file differently.
//!
//! The files come from an independent writer (`common::one_lane_gray8`,
//! SPEC 1.0, 2 and 3 alone). `just test-run-cap` runs this file on every
//! tier: native, AVX2 (`IMIJ_NO_AVX512`), scalar (`IMIJ_SCALAR`),
//! AVX-512 under SDE, NEON under qemu and wasm128 under wasmtime.

mod common;

use std::io::Cursor;

use common::{K4_ALPHABET, one_lane_gray8, one_lane_k4, run_token as run, value_token as value};
use imij::planes::Rect;
use imij::{Error, Limits, StreamDecoder};

/// Every decode route that runs everywhere (no threads): whole image,
/// stream rows, `std::io::Read`, region and frames. Each gives the pixels
/// or its error.
fn routes(file: &[u8]) -> Vec<(&'static str, Result<Vec<u8>, Error>)> {
    let info = imij::header(file).unwrap().info;
    let rb = info.row_bytes();
    let n = info.checked_buffer_len().unwrap();
    let mut out = Vec::new();
    out.push(("decode", imij::decode(file).map(|d| d.1)));
    out.push(("decode_into", {
        let mut px = vec![0u8; n];
        imij::decode_into_threads(file, &mut px, rb, 1).map(|_| px)
    }));
    out.push(("stream", {
        (|| {
            let mut d = StreamDecoder::new(file)?;
            let mut row = vec![0u8; rb];
            let mut px = Vec::new();
            while d.next_row(&mut row)? {
                px.extend_from_slice(&row);
            }
            Ok(px)
        })()
    }));
    out.push(("reader", {
        let mut px = Vec::new();
        imij::decode_reader(Cursor::new(file), &Limits::DEFAULT, |_, _, row: &[u8]| {
            px.extend_from_slice(row);
            Ok(())
        })
        .map(|_| px)
    }));
    out.push(("region", {
        let mut px = vec![0u8; n];
        let rect = Rect::new(0, 0, info.width, info.height);
        imij::region::decode_region(file, rect, &mut px, rb).map(|_| px)
    }));
    out.push(("frames", {
        (|| {
            let mut d = imij::FrameDecoder::new(file)?;
            let mut px = vec![0u8; n];
            d.next_frame()?;
            d.decode_into(&mut px, rb, 1)?;
            Ok(px)
        })()
    }));
    out
}

fn all_corrupt(file: &[u8], why: &str) {
    for (route, r) in routes(file) {
        match r {
            Err(Error::Corrupt(m)) => assert_eq!(m, why, "{route}"),
            other => panic!(
                "{route}: {:?}, expected Corrupt({why})",
                other.map(|p| p.len())
            ),
        }
    }
}

fn all_ok(file: &[u8], expect: &[u8]) {
    for (route, r) in routes(file) {
        assert_eq!(r.as_deref(), Ok(expect), "{route}");
    }
}

/// The writer's own check: runs that end on the lane's last position decode
/// on every route. Residual 5 is -3, so the first pixel is 253 (predicted
/// from W = 0) and every later one repeats it (zero residuals, predictor W,
/// and N at a lane's first column).
#[test]
fn runs_that_end_with_their_lane_decode() {
    all_ok(&one_lane_k4(8, 4, &[value(5), run(31)]), &[253; 32]);
    // The longest run a token may code fills a lane of 2^15 positions.
    let f = one_lane_k4(1 << 15, 1, &[run(1 << 15)]);
    all_ok(&f, &vec![0; 1 << 15]);
}

/// Runs of 2^15 + 1 and 2^16 + 1 positions: the first token past the cap
/// (whose 15 extra bits the narrow tables hold exactly) and one with 16
/// extra bits (which they did not). The model alone is `Corrupt`, before
/// any token decodes, so every tier fails the same way.
#[test]
fn runs_past_the_cap_are_corrupt() {
    for len in [(1 << 15) + 1, (1 << 16) + 1] {
        let f = one_lane_k4(len, 1, &[run(len)]);
        all_corrupt(&f, "run token past the run cap");
    }
    // Weight alone is enough: the run token is never decoded here.
    let mut w = vec![0u32; K4_ALPHABET];
    w[5] = 1;
    w[run((1 << 16) + 1).0 as usize] = 1;
    let f = one_lane_gray8(8, 4, 4, 0, &w, &[value(5); 32]);
    all_corrupt(&f, "run token past the run cap");
}

/// A run longer than the positions left in its lane (31 after the first
/// pixel of an 8 x 4 lane): `Corrupt` once the unit's rows are decoded.
#[test]
fn runs_past_their_lane_are_corrupt() {
    all_corrupt(
        &one_lane_k4(8, 4, &[value(5), run(40)]),
        "run past the end of its lane",
    );
    all_corrupt(
        &one_lane_k4(8, 4, &[value(5), run(32)]),
        "run past the end of its lane",
    );
}

/// Keep the original version 2 seeds and the version 3 seeds for the same
/// corrupt runs. The fuzz regression suite exercises both versions on every
/// target. `IMIJ_SEEDS_BLESS=1` rewrites only the version 3 files.
#[test]
fn fuzz_seeds_are_the_forged_files() {
    let len = (1 << 16) + 1;
    let seeds = [
        (
            "47-seed-v2-run-past-cap.imi",
            "47-seed-v3-run-past-cap.imi",
            "run token past the run cap",
            one_lane_k4(len, 1, &[run(len)]),
        ),
        (
            "48-seed-v2-run-past-lane.imi",
            "48-seed-v3-run-past-lane.imi",
            "run past the end of its lane",
            one_lane_k4(8, 4, &[value(5), run(40)]),
        ),
    ];
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fuzz/seeds");
    for (old_name, new_name, error, forged) in seeds {
        let old = std::fs::read(dir.join(old_name)).unwrap();
        let path = dir.join(new_name);
        if std::env::var_os("IMIJ_SEEDS_BLESS").is_some() {
            std::fs::write(&path, &forged).unwrap();
        }
        let new = std::fs::read(&path).unwrap();
        assert_eq!(old[9], 2, "{old_name}: header version");
        assert_eq!(new[9], 3, "{new_name}: header version");
        assert_eq!(new, forged, "{new_name}");
        all_corrupt(&old, error);
        all_corrupt(&new, error);
    }
}
