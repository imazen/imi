//! The error-bounded float recipe end to end: quantize, encode the integers
//! (losslessly or near-lossless with the returned bound), decode through
//! every entry point, and check each value against the source.

use imij::{Channels, Error, FloatQuantization, ImageInfo, Metadata, SampleType, decode};

mod common;
use common::{Rng, decode_all};

/// A smooth f32 field with noise, NaN holes and a fill value.
fn field(w: usize, h: usize, base: f64, amp: f64, holes: bool, seed: u64) -> Vec<f32> {
    let mut r = Rng(seed | 1);
    (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f64, (i / w) as f64);
            if holes && ((x - 40.0).powi(2) + (y - 20.0).powi(2) < 120.0) {
                return f32::NAN;
            }
            if holes && i % 97 == 0 {
                return -9999.0;
            }
            (base + amp * ((x * 0.07).sin() * (y * 0.05).cos() + 0.02 * r.unit())) as f32
        })
        .collect()
}

fn stored(sample: SampleType, px: &[u8], i: usize) -> u32 {
    match sample {
        SampleType::U16 => u16::from_ne_bytes([px[2 * i], px[2 * i + 1]]) as u32,
        _ => u32::from_ne_bytes(px[4 * i..4 * i + 4].try_into().unwrap()),
    }
}

#[test]
fn float_recipe_round_trips_within_the_bound() {
    let (w, h) = (131usize, 57usize);
    let cases = [
        (280.0, 30.0, 0.01, true),        // temperature in K
        (101_000.0, 2_000.0, 5.0, false), // pressure in Pa
        (1500.0, 1200.0, 0.1, true),      // elevation in m
        (0.0, 1e-3, 1e-6, true),          // precipitation in m
    ];
    for (k, &(base, amp, e, holes)) in cases.iter().enumerate() {
        let v = field(w, h, base, amp, holes, 7 + k as u64);
        for near in [0u32, 1, 2, 4] {
            let (q, ints) = FloatQuantization::quantize(&v, Some(-9999.0), e, near).unwrap();
            let info = ImageInfo::new(w as u32, h as u32, Channels::Gray, q.sample);
            let cfg =
                imij::ImiEncoderConfig::default().tune_lossless(|t| t.max_error = [q.near; 4]);
            let file = cfg
                .encode_samples(
                    imij::ImageRef::new(&ints, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap();
            if q.sample == SampleType::U32 && !cfg!(feature = "extended") {
                assert!(matches!(decode(&file), Err(Error::Unsupported(_))));
                let mut out = vec![0u8; ints.len()];
                assert!(matches!(
                    imij::decode_into_threads(&file, &mut out, info.row_bytes(), 4),
                    Err(Error::Unsupported(_))
                ));
                continue;
            }
            let out = decode_all(&file, &info);
            let mut worst = 0.0f64;
            for (i, &x) in v.iter().enumerate() {
                let missing = x.is_nan() || x == -9999.0;
                match q.value(stored(q.sample, &out, i)) {
                    None => assert!(missing, "case {k} near {near}: valid sample {i} lost"),
                    Some(y) => {
                        assert!(!missing, "case {k} near {near}: missing sample {i} decoded");
                        worst = worst.max((y - x as f64).abs());
                    }
                }
            }
            assert!(worst <= e, "case {k} near {near}: error {worst} > {e}");
        }
    }
}

#[test]
fn quantized_floats_are_smaller_than_the_floats() {
    // A bound well above the field's noise: the integers code smaller than
    // the lossless f32 field.
    let (w, h) = (256usize, 128usize);
    let v = field(w, h, 280.0, 30.0, false, 3);
    let f32_info = ImageInfo::new(w as u32, h as u32, Channels::Gray, SampleType::F32);
    let raw: Vec<u8> = v.iter().flat_map(|x| x.to_ne_bytes()).collect();
    let lossless = imij::ImiEncoderConfig::default()
        .encode_samples(
            imij::ImageRef::new(&raw, f32_info.row_bytes(), &f32_info),
            &Metadata::default(),
        )
        .unwrap();
    let (q, ints) = FloatQuantization::quantize(&v, None, 0.05, 0).unwrap();
    let info = ImageInfo::new(w as u32, h as u32, Channels::Gray, q.sample);
    let file = imij::ImiEncoderConfig::default()
        .encode_samples(
            imij::ImageRef::new(&ints, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    assert!(
        file.len() * 2 < lossless.len(),
        "{} vs {}",
        file.len(),
        lossless.len()
    );
}

#[test]
fn bad_inputs_are_rejected() {
    let bad = |r: Result<_, Error>| matches!(r, Err(Error::InvalidInput(_)));
    assert!(bad(FloatQuantization::quantize(&[1.0f32], None, 0.0, 0)));
    assert!(bad(FloatQuantization::quantize(
        &[1.0f32, f32::INFINITY],
        None,
        0.1,
        0
    )));
    assert!(bad(FloatQuantization::quantize(
        &[0.0f64, 1e30],
        None,
        1e-3,
        1
    )));
}
