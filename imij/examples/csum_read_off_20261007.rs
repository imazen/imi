//! `cSUM` decode timing with checksum verification off (2026-10-07):
//! `csum_read_off_20261007 dtime REPS THREADS FILE...` times the paths of
//! `csum_read_20261007` through their `Limits` entry points with
//! `Limits::with_checksum_verification(false)`.

#[path = "csum_timing/mod.rs"]
mod csum_timing;
use csum_timing::{DecodePath, dtime, fold, read_files};

const OFF: imij::Limits = imij::Limits::NONE.with_checksum_verification(false);

const PATHS: [DecodePath; 4] = [
    ("decode", |d, i, out, _| {
        imij::decode_into_threads_with_limits(d, out, i.row_bytes(), 1, &OFF).unwrap();
        fold(&out[out.len() / 2..])
    }),
    ("threads", |d, i, out, t| {
        imij::decode_into_threads_with_limits(d, out, i.row_bytes(), t, &OFF).unwrap();
        fold(&out[out.len() / 3..])
    }),
    ("reader", |d, _, _, _| {
        let mut s = 0;
        imij::decode_reader(d, &OFF, |_, _, row| {
            s ^= fold(row);
            Ok(())
        })
        .unwrap();
        s
    }),
    ("stream", |d, i, out, _| {
        let mut s = imij::StreamDecoder::with_limits(d, &OFF).unwrap();
        let row = &mut out[..i.row_bytes()];
        let mut acc = 0;
        while s.next_row(row).unwrap() {
            acc ^= fold(row);
        }
        acc
    }),
];

fn main() {
    let mut args = std::env::args().skip(1);
    assert_eq!(args.next().as_deref(), Some("dtime"));
    let reps = args.next().expect("REPS").parse().unwrap();
    let threads = args.next().expect("THREADS").parse().unwrap();
    dtime(&PATHS, &read_files(args), reps, threads);
}
