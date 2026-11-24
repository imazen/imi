//! Error-bounded near-lossless coding and the float map (Scientific 0.1,
//! SPEC 2.6 and 2.1): the bound holds on every decode path, files list the
//! profile, malformed heads are rejected, bound 0 is plain lossless.
// Literal V2/V3 wire mutation controls select the retained grammar explicitly.

use imij::{
    Channels, Error, FrameConfig, FrameDecoder, FrameDuration, FrameEncoder, ImageInfo,
    ImiEncoderConfig, Metadata, SampleType, decode,
};

mod common;
use common::{Rng, SCIENTIFIC_0_1, decode_all, listed_beyond_table, listed_profiles};

/// Container sample `c` of pixel `i` as i64 (i32 signed, others unsigned).
fn sample(px: &[u8], info: &ImageInfo, i: usize, c: usize) -> i64 {
    let n = info.channels.count();
    let b = info.sample.bytes();
    let o = (i * n + c) * b;
    match info.sample {
        SampleType::U8 => px[o] as i64,
        SampleType::U16 | SampleType::F16 => u16::from_ne_bytes([px[o], px[o + 1]]) as i64,
        SampleType::I32 => i32::from_ne_bytes(px[o..o + 4].try_into().unwrap()) as i64,
        _ => u32::from_ne_bytes(px[o..o + 4].try_into().unwrap()) as i64,
    }
}

fn put(px: &mut [u8], info: &ImageInfo, i: usize, c: usize, v: i64) {
    let n = info.channels.count();
    let b = info.sample.bytes();
    let o = (i * n + c) * b;
    match b {
        1 => px[o] = v as u8,
        2 => px[o..o + 2].copy_from_slice(&(v as u16).to_ne_bytes()),
        _ => px[o..o + 4].copy_from_slice(&(v as u32).to_ne_bytes()),
    }
}

/// Largest absolute error per channel.
fn max_err(a: &[u8], b: &[u8], info: &ImageInfo) -> [i64; 4] {
    let mut e = [0i64; 4];
    for i in 0..(info.width * info.height) as usize {
        for (c, ec) in e.iter_mut().enumerate().take(info.channels.count()) {
            *ec = (*ec).max((sample(a, info, i, c) - sample(b, info, i, c)).abs());
        }
    }
    e
}

/// A smooth field with noise (`noise` = amplitude) in `lo..=hi`.
fn field(info: &ImageInfo, lo: i64, hi: i64, noise: i64, seed: u64) -> Vec<u8> {
    let mut r = Rng(seed | 1);
    let (w, h) = (info.width as usize, info.height as usize);
    let mut px = vec![0u8; info.row_bytes() * h];
    let span = (hi - lo) as f64;
    for y in 0..h {
        for x in 0..w {
            for c in 0..info.channels.count() {
                let t = ((x as f64 * 0.05 + c as f64).sin() + (y as f64 * 0.07).cos() + 2.0) / 4.0;
                let n = if noise > 0 {
                    (r.next() % (2 * noise as u64 + 1)) as i64 - noise
                } else {
                    0
                };
                let v = (lo as f64 + t * span) as i64 + n;
                put(&mut px, info, y * w + x, c, v.clamp(lo, hi));
            }
        }
    }
    px
}

fn full_range(st: SampleType) -> (i64, i64) {
    match st {
        SampleType::U8 => (0, 255),
        SampleType::U16 => (0, 65535),
        SampleType::U32 => (0, u32::MAX as i64),
        _ => (i32::MIN as i64, i32::MAX as i64),
    }
}

#[test]
fn bound_holds_on_every_sample_type_layout_and_path() {
    let types = [
        SampleType::U8,
        SampleType::U16,
        SampleType::U32,
        SampleType::I32,
    ];
    let chans = [
        Channels::Gray,
        Channels::GrayAlpha,
        Channels::Rgb,
        Channels::Rgba,
    ];
    let cfgs: Vec<(&str, ImiEncoderConfig)> = vec![
        ("default", imij::ImiEncoderConfig::default()),
        ("lanes3-rows7", {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.tune_lossless(|t| {
                t.lanes = Some(3);
            });
            cfg = cfg.tune_lossless(|t| {
                t.stripe_rows = Some(7);
            });
            cfg
        }),
        ("groups", {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.with_column_groups(Some(40));
            cfg = cfg.with_threads(4);
            cfg
        }),
        (
            "effort0",
            imij::ImiEncoderConfig::default().with_effort(0.0),
        ),
        (
            "effort10",
            imij::ImiEncoderConfig::default().with_effort(10.0),
        ),
    ];
    for st in types {
        // Without the `extended` feature the decoder is baseline-only.
        if st.tier() == imij::Tier::Extended && !cfg!(feature = "extended") {
            continue;
        }
        let (lo, hi) = full_range(st);
        let deltas: &[u32] = match st {
            SampleType::U8 => &[1, 2, 4],
            SampleType::U16 => &[1, 2, 4, 8, 16],
            _ => &[1, 16, 1000, 1 << 20],
        };
        for ch in chans {
            let info = ImageInfo::new(97, 41, ch, st);
            let px = field(&info, lo, hi, (hi - lo) / 400, 7);
            for (name, cfg) in &cfgs {
                for &delta in deltas {
                    let cfg = cfg.clone().tune_lossless(|t| t.max_error = [delta; 4]);
                    let enc = cfg
                        .encode_legacy_samples(
                            imij::ImageRef::new(&px, info.row_bytes(), &info),
                            &Metadata::default(),
                        )
                        .unwrap_or_else(|e| panic!("{st:?} {ch:?} {name} {delta}: {e:?}"));
                    assert_eq!(
                        listed_beyond_table(&enc),
                        [SCIENTIFIC_0_1],
                        "{st:?} {ch:?} {name}"
                    );
                    let out = decode_all(&enc, &info);
                    let e = max_err(&px, &out, &info);
                    for (c, &ec) in e.iter().enumerate().take(ch.count()) {
                        assert!(
                            ec <= delta as i64,
                            "{st:?} {ch:?} {name} delta {delta}: channel {c} error {ec}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn bound_zero_is_plain_lossless() {
    for st in [SampleType::U8, SampleType::U16, SampleType::I32] {
        if st.tier() == imij::Tier::Extended && !cfg!(feature = "extended") {
            continue;
        }
        let info = ImageInfo::new(120, 33, Channels::Rgba, st);
        let (lo, hi) = full_range(st);
        let px = field(&info, lo, hi, (hi - lo) / 300, 3);
        let plain = imij::ImiEncoderConfig::default()
            .encode_legacy_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        let zero = imij::ImiEncoderConfig::default()
            .tune_lossless(|t| t.max_error = [0; 4])
            .encode_legacy_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        assert_eq!(plain, zero);
        assert!(listed_profiles(&plain).is_empty());
        assert_eq!(decode(&plain).unwrap().1, px);
    }
}

#[test]
fn per_channel_bounds_and_exact_channels() {
    let info = ImageInfo::new(150, 70, Channels::Rgba, SampleType::U16);
    let px = field(&info, 0, 65535, 300, 11);
    let bounds = [6, 0, 40, 0];
    let cfg = imij::ImiEncoderConfig::default().tune_lossless(|t| t.max_error = bounds);
    let enc = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let out = decode_all(&enc, &info);
    let e = max_err(&px, &out, &info);
    for c in 0..4 {
        assert!(
            e[c] <= bounds[c] as i64,
            "channel {c}: {} > {}",
            e[c],
            bounds[c]
        );
    }
    assert_eq!(e[1], 0);
    assert_eq!(e[3], 0);
    // Entries past the channel count are ignored.
    let gray = ImageInfo::new(64, 64, Channels::Gray, SampleType::U8);
    let gpx = field(&gray, 0, 255, 3, 2);
    let cfg = imij::ImiEncoderConfig::default().tune_lossless(|t| t.max_error = [2, 9, 9, 9]);
    let enc = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&gpx, gray.row_bytes(), &gray),
            &Metadata::default(),
        )
        .unwrap();
    assert!(max_err(&gpx, &decode_all(&enc, &gray), &gray)[0] <= 2);
}

#[test]
fn larger_bounds_give_smaller_files() {
    let info = ImageInfo::new(256, 256, Channels::Rgb, SampleType::U16);
    let px = field(&info, 0, 65535, 200, 5);
    let mut last = usize::MAX;
    for delta in [0u32, 1, 2, 4, 8, 16] {
        let cfg = imij::ImiEncoderConfig::default().tune_lossless(|t| t.max_error = [delta; 4]);
        let enc = cfg
            .encode_legacy_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        assert!(enc.len() < last, "delta {delta}: {} >= {last}", enc.len());
        last = enc.len();
    }
}

#[test]
fn shift_mapped_channels_bound_in_sample_units() {
    // 12-bit values MSB-aligned in u16: the coded step is 16 sample units.
    let info = ImageInfo::new(80, 60, Channels::Rgb, SampleType::U16);
    let mut px = field(&info, 0, 4095, 20, 9);
    for i in 0..(80 * 60) {
        for c in 0..3 {
            let v = sample(&px, &info, i, c);
            put(&mut px, &info, i, c, v << 4);
        }
    }
    for delta in [8u32, 15, 16, 40, 100] {
        let cfg = imij::ImiEncoderConfig::default().tune_lossless(|t| t.max_error = [delta; 4]);
        let enc = cfg
            .encode_legacy_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        let out = decode_all(&enc, &info);
        let e = max_err(&px, &out, &info);
        assert!(e.iter().all(|&x| x <= delta as i64), "delta {delta}: {e:?}");
        if delta < 16 {
            // No coded step fits: the channel stays exact (plain lossless).
            assert_eq!(e, [0; 4]);
            assert!(listed_profiles(&enc).is_empty());
        }
    }
}

#[test]
fn declared_precision_range_is_kept() {
    // 10 significant bits in u16, values up to 1023, including a sparse
    // channel whose exact map would be Shift (multiples of 512).
    let info = ImageInfo::new(64, 48, Channels::Rgb, SampleType::U16)
        .with_significant_bits(&[10, 10, 10])
        .unwrap();
    let mut px = field(&info, 0, 1023, 8, 4);
    for i in 0..(64 * 48) {
        put(&mut px, &info, i, 2, if i % 3 == 0 { 512 } else { 0 });
    }
    for delta in [3u32, 600, 5000] {
        let cfg = imij::ImiEncoderConfig::default().tune_lossless(|t| t.max_error = [delta; 4]);
        let enc = cfg
            .encode_legacy_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        let out = decode_all(&enc, &info);
        let e = max_err(&px, &out, &info);
        assert!(e.iter().all(|&x| x <= delta as i64), "{e:?}");
        for i in 0..(64 * 48) {
            for c in 0..3 {
                assert!(sample(&out, &info, i, c) <= 1023, "delta {delta}");
            }
        }
    }
}

#[cfg(feature = "extended")]
#[test]
fn signed_values_at_the_top_of_the_range_do_not_wrap() {
    let info = ImageInfo::new(70, 30, Channels::Gray, SampleType::I32);
    let px = field(&info, i32::MAX as i64 - 5000, i32::MAX as i64, 50, 8);
    for delta in [1u32, 100, 4000, 1 << 30] {
        let cfg = imij::ImiEncoderConfig::default().tune_lossless(|t| t.max_error = [delta; 4]);
        let enc = cfg
            .encode_legacy_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        let e = max_err(&px, &decode_all(&enc, &info), &info);
        assert!(e[0] <= delta as i64, "delta {delta}: {}", e[0]);
    }
    let info = ImageInfo::new(70, 30, Channels::Gray, SampleType::I32);
    let px = field(&info, i32::MIN as i64, i32::MIN as i64 + 3000, 50, 8);
    let cfg = imij::ImiEncoderConfig::default().tune_lossless(|t| t.max_error = [77; 4]);
    let enc = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    assert!(max_err(&px, &decode_all(&enc, &info), &info)[0] <= 77);
}

#[test]
fn palette_stripes_stay_exact_in_near_files() {
    // Few colors: palette stripes code exact indices; the rest is near.
    let (w, h) = (200u32, 128u32);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let mut px = field(&info, 0, 255, 3, 21);
    for y in 0..64 {
        for x in 0..w as usize {
            for c in 0..3 {
                put(
                    &mut px,
                    &info,
                    y * w as usize + x,
                    c,
                    ((x / 25) * 30 + c * 7) as i64,
                );
            }
        }
    }
    let cfg = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some(32);
        });
        cfg
    }
    .tune_lossless(|t| t.max_error = [3; 4]);
    let enc = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let out = decode_all(&enc, &info);
    assert!(max_err(&px, &out, &info).iter().all(|&e| e <= 3));
}

#[test]
fn palette_only_images_stay_plain_lossless() {
    // Few colors: every stripe is a palette stripe (exact), so a bound buys
    // nothing and the file needs no Scientific profile.
    let (w, h) = (256u32, 96u32);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let mut px = vec![0u8; info.row_bytes() * h as usize];
    for y in 0..h as usize {
        for x in 0..w as usize {
            let k = (x / 16 + y / 8) % 6;
            for c in 0..3 {
                put(
                    &mut px,
                    &info,
                    y * w as usize + x,
                    c,
                    (k * 40 + c * 3) as i64,
                );
            }
        }
    }
    let plain = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let near = imij::ImiEncoderConfig::default()
        .tune_lossless(|t| t.max_error = [4; 4])
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    assert!(listed_beyond_table(&near).is_empty());
    assert!(!near_record(&near));
    assert_eq!(decode_all(&near, &info), px);
    assert_eq!(decode(&plain).unwrap().1, px);
}

/// Whether a lossless file's head carries near-lossless bounds (head
/// record 19).
fn near_record(file: &[u8]) -> bool {
    let info = imij::header(file).unwrap().info;
    let d = &file[common::data_payload(file)];
    common::payload(d, &info, false).head_record(19).is_some()
}

#[test]
fn floats_need_integer_quantization() {
    for st in [SampleType::F16, SampleType::F32] {
        let info = ImageInfo::new(16, 16, Channels::Gray, st);
        let px = vec![0u8; info.row_bytes() * 16];
        let r = imij::ImiEncoderConfig::default()
            .tune_lossless(|t| t.max_error = [1; 4])
            .encode_legacy_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            );
        assert!(matches!(r, Err(Error::InvalidInput(_))), "{st:?}");
    }
}

#[test]
fn near_lossless_is_defined_for_u8_u16_u32_i32_only() {
    // i8, i16 and 64-bit samples have no near-lossless coding (SPEC 2.6):
    // a nonzero bound is InvalidInput; bound 0 codes them losslessly, with
    // no Scientific profile.
    let mut types = vec![SampleType::I8, SampleType::I16];
    if cfg!(feature = "extended") {
        types.extend([SampleType::U64, SampleType::I64, SampleType::F64]);
    }
    for st in types {
        let info = ImageInfo::new(24, 10, Channels::Rgb, st);
        let mut r = Rng(7);
        let px: Vec<u8> = (0..info.row_bytes() * 10).map(|_| r.next() as u8).collect();
        let rb = info.row_bytes();
        let meta = Metadata::default();
        let near = imij::ImiEncoderConfig::default()
            .tune_lossless(|t| t.max_error = [1; 4])
            .encode_legacy_samples(imij::ImageRef::new(&px, rb, &info), &meta);
        assert!(matches!(near, Err(Error::InvalidInput(_))), "{st:?}");
        let plain = imij::ImiEncoderConfig::default()
            .encode_legacy_samples(imij::ImageRef::new(&px, rb, &info), &meta)
            .unwrap();
        assert!(!listed_profiles(&plain).contains(&SCIENTIFIC_0_1), "{st:?}");
        if cfg!(feature = "extended") {
            assert_eq!(decode_all(&plain, &info), px, "{st:?}");
        } else {
            assert_eq!(
                decode(&plain),
                Err(Error::Unsupported("signed samples (extended tier)"))
            );
        }
    }
}

#[cfg(feature = "extended")]
#[test]
fn float_shift_is_for_f16_and_f32_only() {
    // BitRound-ed mixed-sign f64 keeps the two-word maps: no float map, no
    // Scientific profile, exact round trip.
    let info = ImageInfo::new(64, 16, Channels::Gray, SampleType::F64);
    let px: Vec<u8> = (0..64 * 16)
        .flat_map(|i| {
            let v = (i as f64 * 0.37).sin() * 1e3;
            f64::from_bits(v.to_bits() & !((1u64 << 40) - 1)).to_ne_bytes()
        })
        .collect();
    let file = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    assert!(!listed_profiles(&file).contains(&SCIENTIFIC_0_1));
    assert_eq!(decode_all(&file, &info), px);
}

#[test]
fn near_animation_frames_hold_the_bound() {
    let info = ImageInfo::new(90, 50, Channels::Rgb, SampleType::U16);
    let frames: Vec<Vec<u8>> = (0..4)
        .map(|i| field(&info, 0, 65535, 100, 30 + i))
        .collect();
    let cfg =
        FrameConfig::new(imij::ImiEncoderConfig::default().tune_lossless(|t| t.max_error = [5; 4]));
    let mut enc = FrameEncoder::animation(info.clone(), Metadata::default(), cfg);
    for f in &frames {
        enc.push_frame(f, info.row_bytes(), FrameDuration::from_millis(40))
            .unwrap();
    }
    // A repeated frame (empty) after the first ones.
    enc.push_frame(&frames[3], info.row_bytes(), FrameDuration::from_millis(40))
        .unwrap();
    let file = enc.finish().unwrap();
    assert_eq!(listed_profiles(&file), [SCIENTIFIC_0_1]);
    let mut dec = FrameDecoder::new(&file).unwrap();
    let mut canvas = vec![0u8; info.row_bytes() * 50];
    let mut i = 0;
    while dec.next_frame().unwrap().is_some() {
        dec.decode_into(&mut canvas, info.row_bytes(), 2).unwrap();
        let src = &frames[i.min(3)];
        let e = max_err(src, &canvas, &info);
        assert!(e.iter().all(|&x| x <= 5), "frame {i}: {e:?}");
        i += 1;
    }
    assert_eq!(i, 5);
}

#[test]
fn outer_header_lists_profiles_frames_need() {
    // Frame 0 exact (bound 0 on these Shift-coded values), a later frame
    // near-lossless: the outer header still lists Scientific.
    let info = ImageInfo::new(40, 30, Channels::Gray, SampleType::U16);
    let mut f0 = field(&info, 0, 4095, 5, 1);
    for i in 0..1200 {
        let v = sample(&f0, &info, i, 0);
        put(&mut f0, &info, i, 0, v << 4);
    }
    let f1 = field(&info, 0, 65535, 300, 2);
    let cfg =
        FrameConfig::new(imij::ImiEncoderConfig::default().tune_lossless(|t| t.max_error = [9; 4]));
    let mut enc = FrameEncoder::pages(info.clone(), Metadata::default(), cfg);
    enc.push_page(&f0, info.row_bytes(), &info).unwrap();
    enc.push_page(&f1, info.row_bytes(), &info).unwrap();
    let file = enc.finish().unwrap();
    assert_eq!(listed_profiles(&file), [SCIENTIFIC_0_1]);
    let mut dec = FrameDecoder::new(&file).unwrap();
    let mut buf = vec![0u8; info.row_bytes() * 30];
    let mut pages = 0;
    while dec.next_frame().unwrap().is_some() {
        dec.decode_into(&mut buf, info.row_bytes(), 1).unwrap();
        let src = if pages == 0 { &f0 } else { &f1 };
        assert!(max_err(src, &buf, &info)[0] <= 9);
        pages += 1;
    }
    assert_eq!(pages, 2);
}

#[test]
fn malformed_near_heads_are_rejected() {
    let info = ImageInfo::new(64, 20, Channels::Rgb, SampleType::U8);
    let px = field(&info, 0, 255, 4, 6);
    let cfg = imij::ImiEncoderConfig::default().tune_lossless(|t| t.max_error = [2, 3, 4, 0]);
    let enc = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let d = common::data_payload(&enc);
    let p = common::payload(&enc[d.clone()], &info, false);
    // The bounds record (head record 19): one varint per channel.
    let bounds = p.head_record(19).expect("a bounds record");
    let b = d.start + bounds.start;
    assert_eq!(&enc[b..b + 3], &[2, 3, 4]);
    let corrupt = |f: &[u8]| matches!(decode(f), Err(Error::Corrupt(_)));
    // All bounds zero.
    let mut f = enc.clone();
    f[b..b + 3].copy_from_slice(&[0, 0, 0]);
    assert!(corrupt(&f));
    // A bound above 2^(bits-1) - 1 (bits <= 8 here: cap 127).
    let mut f = enc.clone();
    f[b] = 0x7F + 1;
    assert!(decode(&f).is_err());
    // The record under an odd id no decoder knows: a newer decoder.
    let id_at = d.start + bounds.start - 2;
    assert_eq!(enc[id_at], 19);
    let mut f = enc.clone();
    f[id_at] = 23;
    assert_eq!(
        decode(&f).map(|_| ()),
        Err(Error::Unsupported("unknown lossless head record"))
    );
    // A presence bit this version does not define.
    let mut f = enc.clone();
    f[d.start] |= 32; // Bits 3/4 are quick/bit-smart coding; bit 5 remains unknown.
    assert!(matches!(decode(&f), Err(Error::Unsupported(_))));
    // Without the record-list bit the list is misparsed and rejected.
    let mut f = enc.clone();
    f[d.start] &= !2;
    assert!(decode(&f).is_err());
}

#[test]
fn near_lossless_files_declare_the_feature() {
    // A near-lossless file declares `near_lossless` (SPEC 0.1); an
    // unassigned feature bit next to it needs a newer decoder.
    let info = ImageInfo::new(16, 16, Channels::Gray, SampleType::U8);
    let px = field(&info, 0, 255, 2, 1);
    let enc = imij::ImiEncoderConfig::default()
        .tune_lossless(|t| t.max_error = [1; 4])
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let near = common::feature::NEAR_LOSSLESS;
    assert_eq!(common::feature_mask(&enc), near);
    let f = common::with_feature_mask(&enc, Some(near | common::unassigned_feature()));
    assert_eq!(decode(&f), Err(Error::Unsupported("unknown feature")));
}

// ---- float map (FloatShift) ----------------------------------------------------

/// BitRound: round to nearest, keeping `keep` explicit mantissa bits.
fn bitround32(v: f32, keep: u32) -> f32 {
    let b = v.to_bits();
    let drop = 23 - keep;
    if drop == 0 || !v.is_finite() {
        return v;
    }
    let half = 1u32 << (drop - 1);
    let r = b.wrapping_add(half - 1 + ((b >> drop) & 1)) & !((1u32 << drop) - 1);
    f32::from_bits(r)
}

fn f32_image(w: u32, h: u32, keep: u32, offset: f32) -> (ImageInfo, Vec<u8>) {
    let info = ImageInfo::new(w, h, Channels::Gray, SampleType::F32);
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    let mut r = Rng(99);
    for y in 0..h {
        for x in 0..w {
            let v = 3.0 * ((x as f32 * 0.03).sin() * (y as f32 * 0.02).cos())
                + ((r.next() % 1000) as f32 / 1000.0 - 0.5) * 0.1
                + offset;
            px.extend_from_slice(&bitround32(v, keep).to_ne_bytes());
        }
    }
    (info, px)
}

#[test]
fn float_shift_codes_bitround_mixed_sign_floats_exactly_and_smaller() {
    // Regular maps only: these sparse floats also code as value tables
    // (Scientific 0.2); this test is about map 8.
    let maps = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.value_tables = false;
        });
        cfg
    };
    for keep in [4u32, 7, 10] {
        let (info, px) = f32_image(128, 96, keep, 0.0);
        let on = maps
            .encode_legacy_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        let off = {
            let mut cfg = maps.clone();
            cfg = cfg.tune_lossless(|t| {
                t.float_shift = false;
            });
            cfg
        }
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
        assert_eq!(decode_all(&on, &info), px, "keep {keep}");
        assert_eq!(decode(&off).unwrap().1, px);
        assert_eq!(listed_beyond_table(&on), [SCIENTIFIC_0_1]);
        assert!(listed_beyond_table(&off).is_empty());
        // Sizes are measured by experiments/science_bitround_2026-10-07.
        assert!(
            on.len() < off.len(),
            "keep {keep}: {} vs {}",
            on.len(),
            off.len()
        );
        eprintln!(
            "keep {keep}: float map {} bytes, without {}",
            on.len(),
            off.len()
        );
    }
    // All positive: the existing Shift map already codes it; no new map.
    let (info, px) = f32_image(64, 64, 7, 8.0);
    let on = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    assert!(listed_profiles(&on).is_empty());
}

#[test]
fn float_shift_keeps_every_special_f32_pattern_bit_exact() {
    // NaN payloads, signed zeros, infinities, subnormals, all with the low
    // 12 raw bits zero, in RGBA f32.
    let specials32: Vec<u32> = vec![
        0x0000_0000,
        0x8000_0000,
        0x7F80_0000,
        0xFF80_0000,
        0x7FC0_0000,
        0xFFC0_1000,
        0x7F80_1000,
        0xFFBF_F000,
        0x0000_1000,
        0x8000_1000,
        0x007F_F000,
        0x807F_F000,
        0x3F80_0000,
        0xBF80_0000,
        0x4049_0000,
        0xC049_0000,
    ];
    let info = ImageInfo::new(37, 13, Channels::Rgba, SampleType::F32);
    let mut r = Rng(5);
    let mut px = Vec::new();
    for _ in 0..37 * 13 * 4 {
        let v = if r.next().is_multiple_of(3) {
            specials32[(r.next() % specials32.len() as u64) as usize]
        } else {
            (r.next() as u32) & !0xFFF
        };
        px.extend_from_slice(&v.to_ne_bytes());
    }
    let enc = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    assert_eq!(listed_profiles(&enc), [SCIENTIFIC_0_1]);
    assert_eq!(decode_all(&enc, &info), px);
}

#[test]
fn float_shift_keeps_every_f16_pattern_bit_exact() {
    // Every f16 pattern with the low 3 bits zero, both signs: NaN payloads,
    // signed zeros, infinities, subnormals.
    let pats: Vec<u16> = (0..=u16::MAX).filter(|v| v & 7 == 0).collect();
    let w = 256u32;
    let h = (pats.len() as u32).div_ceil(w);
    let info = ImageInfo::new(w, h, Channels::Gray, SampleType::F16);
    let mut px = Vec::new();
    for i in 0..(w * h) as usize {
        px.extend_from_slice(&pats[i % pats.len()].to_ne_bytes());
    }
    let enc = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    assert_eq!(listed_profiles(&enc), [SCIENTIFIC_0_1]);
    assert_eq!(decode_all(&enc, &info), px);
}

#[test]
fn float_shift_with_wide_and_narrow_coded_widths() {
    // Coded widths above 16 bits (32-bit writer) and at most 16 (table).
    for keep in [2u32, 6, 14, 20] {
        let (info, px) = f32_image(70, 40, keep, 0.0);
        let enc = imij::ImiEncoderConfig::default()
            .encode_legacy_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        assert_eq!(decode_all(&enc, &info), px, "keep {keep}");
    }
}

#[test]
fn plane_levels_carry_their_own_bound() {
    use imij::planes::*;
    let primary = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(
            imij::ImageRef::new(
                &[1, 2, 3, 4],
                2,
                &ImageInfo::new(2, 2, Channels::Gray, SampleType::U8),
            ),
            &Metadata::default(),
        )
        .unwrap();
    let info = ImageInfo::new(200, 150, Channels::Gray, SampleType::U16);
    let px = field(&info, 0, 65535, 400, 13);
    let half = ImageInfo::new(100, 75, Channels::Gray, SampleType::U16);
    let small = field(&half, 0, 65535, 400, 14);
    let mut b = PlaneSetBuilder::new(&primary).unwrap();
    let id = b
        .add_plane(PlaneDescriptor::new("counts", SampleType::U16).with_units("adu"))
        .unwrap();
    let near = imij::ImiEncoderConfig::default().tune_lossless(|t| t.max_error = [7; 4]);
    b.add_level(id, LevelDescriptor::new(200, 150, 64, 64), &px, 400, near)
        .unwrap();
    let exact = imij::ImiEncoderConfig::default();
    b.add_level(
        id,
        LevelDescriptor::new(100, 75, 64, 64),
        &small,
        200,
        exact,
    )
    .unwrap();
    let file = b.finish().unwrap();
    // The primary image is unchanged and needs no profile.
    assert_eq!(decode(&file).unwrap().1, [1, 2, 3, 4]);
    let mut r = PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
    let mut out = vec![0u8; 400 * 150];
    r.decode_region(id, 0, Rect::new(0, 0, 200, 150), &mut out, 400)
        .unwrap();
    let e = max_err(&px, &out, &info)[0];
    assert!(e <= 7 && e > 0, "near level error {e}");
    let mut crop = vec![0u8; 140 * 40];
    r.decode_region(id, 0, Rect::new(50, 60, 70, 40), &mut crop, 140)
        .unwrap();
    for y in 0..40 {
        assert_eq!(
            crop[y * 140..(y + 1) * 140],
            out[(60 + y) * 400 + 100..(60 + y) * 400 + 240]
        );
    }
    let mut out2 = vec![0u8; 200 * 75];
    r.decode_region(id, 1, Rect::new(0, 0, 100, 75), &mut out2, 200)
        .unwrap();
    assert_eq!(out2, small);
}

#[test]
fn corrupted_near_files_do_not_panic() {
    // Random byte, bit and truncation damage to near-lossless files: the
    // new reconstruction arithmetic (sign extension, multiply, clamp) must
    // return garbage pixels or an error, never panic.
    let mut files = Vec::new();
    let mut cases = vec![
        (Channels::Rgb, SampleType::U8, 2u32),
        (Channels::Rgba, SampleType::U16, 40),
        (Channels::Gray, SampleType::U16, 1),
    ];
    if cfg!(feature = "extended") {
        cases.push((Channels::GrayAlpha, SampleType::U32, 5000));
        cases.push((Channels::Rgb, SampleType::I32, 1 << 30));
    }
    for (ch, st, delta) in cases {
        let info = ImageInfo::new(57, 23, ch, st);
        let (lo, hi) = full_range(st);
        let px = field(&info, lo, hi, (hi - lo) / 100, 17);
        let cfg = {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.tune_lossless(|t| {
                t.stripe_rows = Some(8);
            });
            cfg
        }
        .tune_lossless(|t| t.max_error = [delta; 4]);
        files.push(
            cfg.encode_legacy_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap(),
        );
    }
    let mut r = Rng(0x0dd_ba11);
    for _ in 0..3000 {
        let f = &files[(r.next() % files.len() as u64) as usize];
        let mut g = f.clone();
        match r.next() % 3 {
            0 => {
                let i = (r.next() % g.len() as u64) as usize;
                g[i] = r.next() as u8;
            }
            1 => {
                let i = (r.next() % g.len() as u64) as usize;
                g[i] ^= 1 << (r.next() % 8);
            }
            _ => g.truncate((r.next() % g.len() as u64) as usize),
        }
        let _ = decode(&g);
        let _ = imij::StreamDecoder::new(&g).map(|mut d| {
            let mut row = vec![0u8; d.info().row_bytes()];
            while let Ok(true) = d.next_row(&mut row) {}
        });
    }
}

#[cfg(feature = "lab")]
#[test]
fn scientific_profile_survives_standalone_seed_candidates() {
    let info = ImageInfo::new(64, 48, Channels::Gray, SampleType::U16);
    let px = field(&info, 0, 65535, 31, 19);
    for (seed_bank, seed_exact) in [(0, true), (1, true), (1, false)] {
        let cfg = {
            let mut cfg = imij::ImiEncoderConfig::default().tune_lossless(|t| t.max_error = [4; 4]);
            cfg = cfg.tune_lossless(|t| {
                t.seed_bank = seed_bank;
                t.seed_exact = seed_exact;
            });
            cfg
        };
        let file = cfg
            .encode_legacy_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        assert!(max_err(&px, &decode_all(&file, &info), &info)[0] <= 4);
    }
}
