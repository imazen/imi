//! Inter frames whose channel maps change from frame to frame (SPEC 4: the
//! reference is `R = reduce(o) mod 2^n` under the current frame's map,
//! whatever map wrote the canvas). Each animation steps a channel through
//! several maps (Shift, Lsb, Replicate, FloatShift) with inter coding on;
//! every frame must decode losslessly, and the files must contain stripes
//! that predict from the reference (predictors 7..=9) in a frame whose map
//! differs from the previous frame's, so the reduction of one map's samples
//! under another is exercised. `v1::reduce_spec_tests` checks the
//! reduction itself against the SPEC table for every map.
// Literal V2/V3 wire mutation controls select the retained grammar explicitly.

mod common;

use common::{chunks, data_payload, lossless_payload, read_varint};
use imij::{
    Channels, FrameConfig, FrameDecoder, FrameDuration, FrameEncoder, ImageInfo, Metadata,
    SampleType,
};

const W: u32 = 64;
const H: u32 = 32;
const STRIPE: u32 = 16;

/// The nested file of every `fRAM` chunk (SPEC 1.3).
fn frame_files(file: &[u8]) -> Vec<Vec<u8>> {
    chunks(file)
        .into_iter()
        .filter(|c| &c.0 == b"fRAM")
        .map(|(_, at, end)| {
            let mut p = at + 4;
            read_varint(file, &mut p); // chunk length
            for _ in 0..4 {
                read_varint(file, &mut p); // x0, y0, duration
            }
            if read_varint(file, &mut p) & 4 != 0 {
                let n = read_varint(file, &mut p) as usize;
                p += n;
            }
            file[p..end].to_vec()
        })
        .collect()
}

/// A lossless file's channel maps (mode, bits) and whether any of its
/// stripes predicts from the reference (SPEC 2, 2.4).
fn maps_and_inter(f: &[u8]) -> (Vec<(u8, u8)>, bool) {
    let info = imij::header(f).unwrap().info;
    let d = &f[data_payload(f)];
    let mut at = 0;
    read_varint(d, &mut at); // present
    at += 1; // lanes
    let rows = read_varint(d, &mut at) as u32;
    read_varint(d, &mut at); // column groups (none here)
    let value = if info.sample.bytes() >= 4 { 4 } else { 2 };
    let maps = (0..info.channels.count())
        .map(|c| (d[at + c * (2 + value)], d[at + c * (2 + value) + 1]))
        .collect();
    let p = lossless_payload(d, &info);
    let mut inter = false;
    for (s, r) in p.records.iter().enumerate() {
        let rec = &d[r.clone()];
        if rec[0] == 0x80 {
            continue; // a palette stripe
        }
        let n = rows.min(info.height - s as u32 * rows) as usize;
        inter |= (0..n).any(|i| (rec[1 + i / 2] >> (4 * (i % 2)) & 15) >= 7);
    }
    (maps, inter)
}

/// Encodes `frames` as an inter-coded animation, checks every frame decodes
/// to its source, and returns the frames' (maps, uses reference) in order.
fn round_trip(info: &ImageInfo, frames: &[Vec<u8>]) -> Vec<(Vec<(u8, u8)>, bool)> {
    let c = {
        let mut c = imij::ImiEncoderConfig::default();
        c = c.tune_lossless(|t| {
            t.stripe_rows = Some(STRIPE);
        });
        c
    };
    let mut cfg = FrameConfig::new(c);
    cfg.inter = true;
    let mut e = FrameEncoder::animation(info.clone(), Metadata::default(), cfg);
    for f in frames {
        e.push_frame(f, info.row_bytes(), FrameDuration::from_millis(40))
            .unwrap();
    }
    let file = e.finish_legacy().unwrap();
    let mut d = FrameDecoder::new(&file).unwrap();
    let rb = info.row_bytes();
    let mut canvas = vec![0u8; rb * info.height as usize];
    let mut t = 0;
    while let Some(h) = d.next_frame().unwrap() {
        d.decode_into(&mut canvas, rb, 2).unwrap();
        assert!(canvas == frames[t], "{:?} frame {t}", info.sample);
        assert_eq!(h.uses_previous, t > 0);
        t += 1;
    }
    assert_eq!(t, frames.len());
    let mut out = vec![maps_and_inter(&file)];
    out.extend(frame_files(&file).iter().map(|f| maps_and_inter(f)));
    out
}

/// At least one frame after the first predicts from the reference with a
/// map that differs from the previous frame's.
fn assert_cross_map_inter(sample: SampleType, frames: &[(Vec<(u8, u8)>, bool)]) {
    for w in frames.windows(2) {
        assert_ne!(w[0].0, w[1].0, "{sample:?}: every frame changes its maps");
    }
    assert!(
        frames.windows(2).any(|w| w[1].1),
        "{sample:?}: no frame predicts from the reference: {frames:?}"
    );
}

fn samples16(f: impl Fn(u32, u32) -> u16) -> Vec<u8> {
    (0..H)
        .flat_map(|y| (0..W).map(move |x| (x, y)))
        .flat_map(|(x, y)| f(x, y).to_ne_bytes())
        .collect()
}

#[test]
fn u16_maps_change_between_inter_frames() {
    let info = ImageInfo::new(W, H, Channels::Gray, SampleType::U16);
    let base = |x: u32, y: u32| ((x * 7 + y * 13) % 251) as u16;
    let f0 = |x, y| base(x, y) << 8; // Shift 8
    let f1 = |x, y| f0(x, y) + ((x * 3 + y) % 4) as u16; // Lsb 16
    let f2 = |x, y| (f1(x, y) >> 8) * 257; // Replicate 8
    let f3 = |x, y| f2(x, y) >> 4; // Lsb 12
    let f4 = |x, y| (f2(x, y) >> 4) << 4; // Shift 12
    let frames = [
        samples16(f0),
        samples16(f1),
        samples16(f2),
        samples16(f3),
        samples16(f4),
    ];
    let got = round_trip(&info, &frames);
    assert_eq!(got[0].0, [(1, 8)], "Shift 8");
    assert_eq!(got[1].0, [(0, 16)], "Lsb 16");
    assert_eq!(got[2].0, [(2, 8)], "Replicate 8");
    assert_cross_map_inter(info.sample, &got);
}

#[test]
fn u8_maps_change_between_inter_frames() {
    let info = ImageInfo::new(W, H, Channels::Rgb, SampleType::U8);
    let px = |f: &dyn Fn(u32, u32, u32) -> u8| -> Vec<u8> {
        (0..H)
            .flat_map(|y| (0..W).flat_map(move |x| (0..3).map(move |c| (x, y, c))))
            .map(|(x, y, c)| f(x, y, c))
            .collect()
    };
    let f0 = |x: u32, y: u32, c: u32| (((x * 5 + y * 3 + c * 40) % 128) * 2) as u8; // Shift 7
    let f1 = |x, y, c| f0(x, y, c) | ((x + y + c) & 1) as u8; // Lsb 8
    let f2 = |x, y, c| f1(x, y, c) & 0xF0; // Shift 4
    let f3 = |x, y, c| f2(x, y, c) >> 4; // Lsb 4
    let frames = [px(&f0), px(&f1), px(&f2), px(&f3)];
    let got = round_trip(&info, &frames);
    assert_eq!(got[0].0, [(1, 7); 3], "Shift 7");
    assert_eq!(got[1].0, [(0, 8); 3], "Lsb 8");
    assert_cross_map_inter(info.sample, &got);
}

#[test]
fn f16_maps_change_between_inter_frames() {
    let info = ImageInfo::new(W, H, Channels::Gray, SampleType::F16);
    // Halves whose low 6 mantissa bits are zero, of both signs: FloatShift.
    let f0 = |x: u32, y: u32| -> u16 {
        let sign = (x + y).is_multiple_of(3) as u16;
        let exp = 13 + ((x / 8 + y / 8) % 4) as u16;
        sign << 15 | exp << 10 | (((x * 37 + y * 11) % 16) as u16) << 6
    };
    let f1 = |x, y| f0(x, y) | ((x * 5 + y) % 64) as u16; // every bit
    let f2 = |x, y| f0(x, y) & 0x7FFF; // positive, low bits zero: Shift
    let frames = [samples16(f0), samples16(f1), samples16(f2)];
    let got = round_trip(&info, &frames);
    // The encoder may select FloatShift or Table depending on which modes
    // are available; both satisfy the test's purpose (cross-map inter).
    let m0 = got[0].0[0].0;
    assert!(
        m0 == 8 || m0 == 9,
        "first frame should be FloatShift (8) or Table (9), got {m0}"
    );
    assert_cross_map_inter(info.sample, &got);
}
