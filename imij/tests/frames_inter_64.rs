//! Inter predictors 7..=9 read the previous frame as coded values of at most
//! 32 bits, so an image of 64-bit samples whose stripes use them is corrupt
//! (SPEC section 4). Crafted inter frames of u64/i64/f64 with predictor 7,
//! 8 or 9 fail with `Corrupt` in every frame decoding path; the same crafted
//! frames of u32/i32/f32 still decode as before, and unmodified frames of
//! every type decode exactly.
//!
//! Every case needs the `extended` feature (32- and 64-bit samples).
// Literal V2/V3 wire mutation controls select the retained grammar explicitly.
#![cfg(feature = "extended")]

mod common;

use imij::{
    Channels, Error, FrameConfig, FrameDecoder, FrameDuration, FrameEncoder, ImageInfo, Metadata,
    SampleType,
};

const W: u32 = 40;
const H: u32 = 48;
/// Stripe height of the frames' nested images: the changed rectangle (up
/// to 36 rows) spans several stripes, so threaded decoding has work to
/// share.
const ROWS: u32 = 4;

const WIDE: [SampleType; 3] = [SampleType::U64, SampleType::I64, SampleType::F64];
const NARROW: [SampleType; 3] = [SampleType::U32, SampleType::I32, SampleType::F32];

/// A sample of value index `k` (0..61): integers with low zero bits,
/// integer-valued floats of both signs.
fn sample(st: SampleType, k: u32) -> u64 {
    let s = k as i64 - 30;
    match st {
        SampleType::U64 => (k as u64) << 40,
        SampleType::I64 => (s << 40) as u64,
        SampleType::F64 => (s as f64 * 2.0).to_bits(),
        SampleType::U32 => (k as u64) << 8,
        SampleType::I32 => ((s << 8) as i32) as u32 as u64,
        _ => (s as f32 * 2.0).to_bits() as u64,
    }
}

fn frame(st: SampleType, ch: Channels, changed: bool) -> Vec<u8> {
    let mut px = Vec::new();
    for y in 0..H {
        for x in 0..W {
            for c in 0..ch.count() as u32 {
                let inside = changed && (6..42).contains(&y) && (3..17).contains(&x);
                let k = if inside {
                    (x * 5 + y + c) % 61
                } else {
                    (x + y * 3 + c * 7) % 61
                };
                let v = sample(st, k);
                match st.bytes() {
                    4 => px.extend_from_slice(&(v as u32).to_ne_bytes()),
                    _ => px.extend_from_slice(&v.to_ne_bytes()),
                }
            }
        }
    }
    px
}

/// A two-frame animation whose second frame is an inter frame (flag bit 0)
/// over the changed rectangle.
fn animation(st: SampleType, ch: Channels) -> (ImageInfo, Vec<Vec<u8>>, Vec<u8>) {
    let info = ImageInfo::new(W, H, ch, st);
    let frames = vec![frame(st, ch, false), frame(st, ch, true)];
    let cfg = FrameConfig::new({
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some(ROWS);
        });
        cfg = cfg.tune_lossless(|t| {
            t.palette = false;
        });
        cfg
    });
    let mut enc = FrameEncoder::animation(info.clone(), Metadata::default(), cfg);
    for f in &frames {
        enc.push_frame(f, info.row_bytes(), FrameDuration::from_millis(40))
            .unwrap();
    }
    (info, frames, enc.finish_legacy().unwrap())
}

/// Offsets of the predictor bytes of every stripe of the nested image of
/// the first `fRAM` chunk (an inter frame), parsed per SPEC 1.3 and 2.
fn predictor_bytes(file: &[u8], _info: &ImageInfo) -> Vec<usize> {
    let (_, at, end) = common::chunks(file)
        .into_iter()
        .find(|c| &c.0 == b"fRAM")
        .expect("an fRAM chunk");
    let mut p = at + 4 + common::varint(&file[at + 4..]).1;
    for _ in 0..4 {
        p += common::varint(&file[p..]).1; // x0, y0, duration
    }
    assert_eq!(file[p] & 1, 1, "frame 2 is an inter frame");
    let base = p + 1;
    let nested = &file[base..end];
    // The nested image is the changed rectangle.
    let rect = imij::header(nested).unwrap().info;
    assert!(rect.height >= 2 * ROWS, "{rect:?}");
    let data = common::data_payload(nested);
    let payload = common::lossless_payload(&nested[data.clone()], &rect);
    assert_eq!(payload.payload.present & 1, 0, "palette flag");
    assert_eq!(payload.records.len(), rect.height.div_ceil(ROWS) as usize);
    payload
        .records
        .iter()
        .map(|r| {
            let at = base + data.start + r.start;
            assert_ne!(file[at], 0x80, "palette stripe");
            at + 1
        })
        .collect()
}

/// `file` with predictor `pred` on stripe row 0 (7) or row 1 (8, 9) of
/// every stripe of the inter frame; row 0 stays W for 8 and 9.
fn crafted(file: &[u8], info: &ImageInfo, pred: u8) -> Vec<u8> {
    let mut f = file.to_vec();
    for p in predictor_bytes(file, info) {
        f[p] = match pred {
            7 => (f[p] & 0xF0) | 7,
            _ => (pred << 4) | (f[p] & 0x0F),
        };
    }
    assert_ne!(f, file);
    f
}

/// Every frame, decoded into one canvas through `FrameDecoder::decode_into`
/// with `threads` (std builds: serial or threaded stripe decoding; builds
/// without std: the serial in-place path).
fn decode_frames(file: &[u8], info: &ImageInfo, threads: usize) -> Result<Vec<Vec<u8>>, Error> {
    let mut dec = FrameDecoder::new(file)?;
    let mut canvas = vec![0u8; info.row_bytes() * H as usize];
    let mut out = Vec::new();
    while dec.next_frame()?.is_some() {
        dec.decode_into(&mut canvas, info.row_bytes(), threads)?;
        out.push(canvas.clone());
    }
    Ok(out)
}

#[test]
fn unmodified_inter_frames_decode_exactly() {
    for st in WIDE.into_iter().chain(NARROW) {
        for ch in [Channels::Gray, Channels::Rgba] {
            let (info, frames, file) = animation(st, ch);
            predictor_bytes(&file, &info); // frame 2 is an inter frame
            for threads in [1, 4] {
                assert!(
                    decode_frames(&file, &info, threads).unwrap() == frames,
                    "{st:?} {ch:?} {threads} threads"
                );
            }
        }
    }
}

#[test]
fn inter_predictors_on_64_bit_samples_are_corrupt() {
    for st in WIDE {
        for ch in [Channels::Gray, Channels::Rgba] {
            let (info, _, file) = animation(st, ch);
            for pred in 7..=9u8 {
                let f = crafted(&file, &info, pred);
                for threads in [1, 4] {
                    let r = decode_frames(&f, &info, threads);
                    assert_eq!(
                        r.as_ref().err(),
                        Some(&Error::Corrupt("bad predictor")),
                        "{st:?} {ch:?} predictor {pred}, {threads} threads"
                    );
                }
            }
        }
    }
}

/// 32-bit frames keep accepting predictors 7..=9 (the reference path reads
/// 32-bit samples): the crafted files decode, to the same canvas at every
/// thread count, and the untouched first frame is exact.
#[test]
fn inter_predictors_on_32_bit_samples_still_decode() {
    for st in NARROW {
        for ch in [Channels::Gray, Channels::Rgba] {
            let (info, frames, file) = animation(st, ch);
            for pred in 7..=9u8 {
                let f = crafted(&file, &info, pred);
                let one = decode_frames(&f, &info, 1)
                    .unwrap_or_else(|e| panic!("{st:?} {ch:?} predictor {pred}: {e:?}"));
                assert_eq!(one.len(), 2);
                assert!(one[0] == frames[0], "{st:?} {ch:?} predictor {pred}");
                assert!(
                    decode_frames(&f, &info, 4).unwrap() == one,
                    "{st:?} {ch:?} predictor {pred}: thread counts differ"
                );
            }
        }
    }
}
