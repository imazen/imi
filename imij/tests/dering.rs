//! Overshoot deringing (lossy encoder): white source pixels must keep
//! decoding as white at least as often as without it, every integer layout
//! still round-trips through the decoder, and float inputs are untouched.
#![cfg(feature = "lab")]

use imij::{Channels, ImageInfo, ImiEncoderConfig, LossyColor, Metadata, SampleType, decode};

/// Dark glyph-like strokes on a pure white page.
fn text_on_white(w: usize, h: usize, nch: usize) -> Vec<u8> {
    let mut v = vec![255u8; w * h * nch];
    for y in 0..h {
        for x in 0..w {
            let stroke = (x % 11 < 2 && y % 17 > 3) || (y % 17 == 9 && x % 23 < 15);
            if stroke {
                for c in 0..nch.min(3) {
                    v[(y * w + x) * nch + c] = 20 + (c as u8) * 30;
                }
            }
        }
    }
    v
}

/// The deringing switch is process-global and tests run on parallel threads.
static SWITCH: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn encode(px: &[u8], info: &ImageInfo, q: f32, color: LossyColor, on: bool) -> Vec<u8> {
    let _g = SWITCH.lock().unwrap_or_else(|e| e.into_inner());
    let cfg = ImiEncoderConfig::lossy(q).tune_lossy(|t| t.color = color);
    imij::lab::set_lossy_dering(on);
    let f = cfg
        .encode_samples(
            imij::ImageRef::new(px, info.row_bytes(), info),
            &Metadata::default(),
        )
        .unwrap();
    imij::lab::set_lossy_dering(true);
    f
}

#[test]
fn white_stays_white_at_least_as_often() {
    let (w, h) = (96, 64);
    let info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8);
    let px = text_on_white(w, h, 3);
    for color in [LossyColor::Xyb, LossyColor::YCoCg] {
        for q in [10.0, 30.0, 60.0, 90.0] {
            let bad = |on: bool| {
                let (_, d) = decode(&encode(&px, &info, q, color, on)).unwrap();
                (0..w * h)
                    .filter(|&p| {
                        px[p * 3..p * 3 + 3] == [255; 3] && d[p * 3..p * 3 + 3] != [255; 3]
                    })
                    .count()
            };
            let (off, on) = (bad(false), bad(true));
            assert!(
                on <= off,
                "{color:?} q{q}: {on} white pixels lost with deringing vs {off} without"
            );
        }
    }
}

#[test]
fn every_integer_layout_round_trips() {
    let (w, h) = (40, 24);
    for (channels, nch) in [
        (Channels::Gray, 1),
        (Channels::GrayAlpha, 2),
        (Channels::Rgb, 3),
        (Channels::Rgba, 4),
    ] {
        let px8 = text_on_white(w, h, nch);
        let info = ImageInfo::new(w as u32, h as u32, channels, SampleType::U8);
        let (got, d) = decode(&encode(&px8, &info, 50.0, LossyColor::Xyb, true)).unwrap();
        assert_eq!((got, d.len()), (info.clone(), px8.len()));
        // 16-bit: the same picture at full scale.
        let px16: Vec<u8> = px8
            .iter()
            .flat_map(|&v| (v as u16 * 257).to_ne_bytes())
            .collect();
        let info16 = ImageInfo::new(w as u32, h as u32, channels, SampleType::U16);
        for color in [LossyColor::Xyb, LossyColor::YCoCg] {
            let f = encode(&px16, &info16, 50.0, color, true);
            assert_eq!(decode(&f).unwrap().0, info16);
        }
    }
}

#[test]
fn float_inputs_are_not_reshaped() {
    let (w, h) = (32, 16);
    let px8 = text_on_white(w, h, 3);
    let pxf: Vec<u8> = px8
        .iter()
        .flat_map(|&v| (v as f32 / 255.0).to_ne_bytes())
        .collect();
    let info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::F32);
    assert_eq!(
        encode(&pxf, &info, 50.0, LossyColor::Xyb, true),
        encode(&pxf, &info, 50.0, LossyColor::Xyb, false)
    );
}
