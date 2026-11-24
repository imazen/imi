//! i8/i16 (feature `signed_samples`) and 64-bit u64/i64/f64 samples
//! (feature `wide_samples`): bit-exact lossless round trips of every pattern
//! through every decode path, their channel maps, header fields and lossy
//! rejection.

mod common;

use imij::{
    Channels, Error, ImageInfo, ImiEncoderConfig, Metadata, SampleType, StreamDecoder, decode,
};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

const NEW: [SampleType; 5] = [
    SampleType::I8,
    SampleType::I16,
    SampleType::U64,
    SampleType::I64,
    SampleType::F64,
];

const CHANNELS: [Channels; 4] = [
    Channels::Gray,
    Channels::GrayAlpha,
    Channels::Rgb,
    Channels::Rgba,
];

fn available(st: SampleType) -> bool {
    st.tier() == imij::Tier::Baseline || cfg!(feature = "extended")
}

/// Special f64 bit patterns: NaN payloads (quiet and signaling, both
/// signs), infinities, signed zeros, subnormals, normal extremes.
const F64_SPECIALS: [u64; 16] = [
    0x7FF8_0000_0000_0000,
    0x7FF8_0000_DEAD_BEEF,
    0xFFF8_1234_5678_9ABC,
    0x7FF0_0000_0000_0001,
    0xFFF7_FFFF_FFFF_FFFF,
    0x7FF0_0000_0000_0000,
    0xFFF0_0000_0000_0000,
    0x0000_0000_0000_0000,
    0x8000_0000_0000_0000,
    0x0000_0000_0000_0001,
    0x800F_FFFF_FFFF_FFFF,
    0x0010_0000_0000_0000,
    0x7FEF_FFFF_FFFF_FFFF,
    0xFFEF_FFFF_FFFF_FFFF,
    0x3FF0_0000_0000_0000,
    0xBFF0_0000_0000_0001,
];

/// One sample's raw bits (low `8 * bytes` bits meaningful) for content `kind`.
fn sample(kind: u8, st: SampleType, x: u32, y: u32, c: u32, r: &mut Rng) -> u64 {
    let g = (x * 37 + y * 11 + c * 1000) as i64;
    let noise = (r.next() % 9) as i64;
    let i = (x + 7 * y + 3 * c) as usize;
    match (kind, st) {
        (0, _) => r.next(),
        // Smooth signals.
        (1, SampleType::I8) => ((g % 200) - 100 + noise) as u64,
        (1, SampleType::I16) => (g * 3 - 30000 + noise) as u64,
        (1, SampleType::U64) => (0x1234_5678_0000_0000i64 + g * 1000 + noise) as u64,
        (1, SampleType::I64) => (-(1i64 << 40) + g * 100_003 + noise) as u64,
        (1, _) => ((g as f64 - 3000.0) * 0.001 + noise as f64 * 1e-9).to_bits(),
        // Flat blocks at the extremes (incl. -0.0 for floats).
        (2, st) => {
            let block = ((x / 8 + y / 8 + c) % 4) as usize;
            match st {
                SampleType::I8 => [0x80, 0x7F, 0, 0xFF][block],
                SampleType::I16 => [0x8000, 0x7FFF, 0, 0xFFFF][block],
                SampleType::F64 => [(-0.0f64).to_bits(), 0, (-1.5f64).to_bits(), u64::MAX][block],
                _ => [0, u64::MAX, 1 << 63, (1 << 63) - 1][block],
            }
        }
        // Low bits zero: i16 x16, 64-bit integers x2^40, f64 promoted from
        // f32 with both signs (raw low 29 bits zero).
        (3, SampleType::I8) => (((g % 16) - 8) * 16) as u64,
        (3, SampleType::I16) => ((g % 4096 - 2048) * 16) as u64,
        (3, SampleType::F64) => {
            { (((g as f32) - 3000.0) * 0.0371 + noise as f32 * 1e-3) as f64 }.to_bits()
        }
        (3, _) => (((g % 4096) - 2048) << 40) as u64,
        // Small ranges far from zero: CT-like i16, integer-valued f64.
        (4, SampleType::I8) => (-100 + g % 7 + noise) as u64,
        (4, SampleType::I16) => (-1024 + (g * 7 + noise) % 4096) as u64,
        (4, SampleType::U64) => (1_000_000_000_000 + g % 5000 + noise) as u64,
        (4, SampleType::I64) => (-5_000_000_000 + g % 5000 + noise) as u64,
        (4, _) => ((g % 5000 - 2500) as f64).to_bits(),
        // Specials.
        (5, SampleType::I8) => [0x80, 0x7F, 0, 0xFF, 1, 0x81][i % 6],
        (5, SampleType::I16) => [0x8000, 0x7FFF, 0, 0xFFFF, 1, 0x8001][i % 6],
        (5, SampleType::F64) => F64_SPECIALS[i % 16],
        (5, _) => [0, u64::MAX, 1 << 63, (1 << 63) - 1, 1, (1 << 32) - 1][i % 6],
        // Hi word constant-ish, lo word noise: values within 2^-20 relative.
        (_, SampleType::F64) => 1.0f64.to_bits() + (r.next() >> 40),
        (_, _) => r.next() >> 16,
    }
}

const KINDS: u8 = 7;

fn put(px: &mut Vec<u8>, st: SampleType, v: u64) {
    match st.bytes() {
        1 => px.push(v as u8),
        2 => px.extend_from_slice(&(v as u16).to_ne_bytes()),
        4 => px.extend_from_slice(&(v as u32).to_ne_bytes()),
        _ => px.extend_from_slice(&v.to_ne_bytes()),
    }
}

/// An image of content `kind`; alpha is binary (0 / all ones) unless the
/// content is noise, or constant when `kind` is 6.
fn image(
    w: u32,
    h: u32,
    ch: Channels,
    st: SampleType,
    kind: u8,
    seed: u64,
) -> (ImageInfo, Vec<u8>) {
    let info = ImageInfo::new(w, h, ch, st);
    let mut r = Rng(seed | 1);
    let n = ch.count() as u32;
    let mut px = Vec::with_capacity(info.row_bytes() * h as usize);
    for y in 0..h {
        for x in 0..w {
            for c in 0..n {
                let alpha = ch.has_alpha() && c == n - 1;
                let v = match (alpha, kind) {
                    (false, _) | (true, 0) => sample(kind, st, x, y, c, &mut r),
                    (true, 6) => 0x0123_4567_89AB_CDEF,
                    (true, _) => {
                        if x % 5 < 2 {
                            u64::MAX
                        } else {
                            0
                        }
                    }
                };
                put(&mut px, st, v);
            }
        }
    }
    (info, px)
}

/// Every decode path returns `px`: allocating, padded caller buffers at 1
/// and 4 threads, row streaming and the reader.
fn check(file: &[u8], info: &ImageInfo, px: &[u8]) {
    if !available(info.sample) {
        assert!(matches!(decode(file), Err(Error::Unsupported(_))));
        let mut out = vec![0xa7; px.len()];
        for threads in [1, 4] {
            assert!(matches!(
                imij::decode_into_threads(file, &mut out, info.row_bytes(), threads),
                Err(Error::Unsupported(_))
            ));
            assert!(out.iter().all(|&v| v == 0xa7));
        }
        assert!(matches!(
            StreamDecoder::new(file),
            Err(Error::Unsupported(_))
        ));
        let mut callbacks = 0;
        assert!(matches!(
            imij::decode_reader(file, &imij::Limits::DEFAULT, |_, _, _| {
                callbacks += 1;
                Ok(())
            }),
            Err(Error::Unsupported(_))
        ));
        assert_eq!(callbacks, 0);
        return;
    }
    let (got_info, got) = decode(file).unwrap_or_else(|error| panic!("{info:?}: {error}"));
    assert_eq!(got_info, *info);
    assert!(got == px, "{:?} {:?} decode", info.sample, info.channels);
    let row = info.row_bytes();
    let height = info.height as usize;
    let stride = row + 5;
    for threads in [1, 4] {
        let mut out = vec![0xa7; stride * height + 3];
        imij::decode_into_threads(file, &mut out, stride, threads).unwrap();
        for y in 0..height {
            assert_eq!(
                &out[y * stride..y * stride + row],
                &px[y * row..(y + 1) * row]
            );
            assert!(
                out[y * stride + row..(y + 1) * stride]
                    .iter()
                    .all(|&v| v == 0xa7)
            );
        }
    }
    let mut stream = StreamDecoder::new(file).unwrap();
    let mut out = vec![0; row];
    for expected in px.chunks_exact(row) {
        assert!(stream.next_row(&mut out).unwrap());
        assert!(out == expected);
    }
    assert!(!stream.next_row(&mut out).unwrap());
    let mut rows = Vec::new();
    imij::decode_reader(file, &imij::Limits::DEFAULT, |i, y, r| {
        assert_eq!(*i, *info);
        assert_eq!(y as usize * row, rows.len());
        rows.extend_from_slice(r);
        Ok(())
    })
    .unwrap();
    assert!(rows == px);
}

fn enc(info: &ImageInfo, px: &[u8], cfg: &ImiEncoderConfig) -> Vec<u8> {
    cfg.encode_samples(
        imij::ImageRef::new(px, info.row_bytes(), info),
        &Metadata::default(),
    )
    .unwrap()
}

#[test]
fn roundtrip_matrix() {
    for st in NEW {
        if !available(st) {
            continue;
        }
        for ch in CHANNELS {
            for kind in 0..KINDS {
                for (w, h) in [(1, 1), (37, 23), (130, 70)] {
                    let (info, px) = image(w, h, ch, st, kind, 7 + kind as u64 + w as u64);
                    let file = enc(&info, &px, &imij::ImiEncoderConfig::default());
                    check(&file, &info, &px);
                }
            }
        }
    }
}

#[test]
fn configs_lanes_stripes_groups_threads() {
    for st in NEW {
        if !available(st) {
            continue;
        }
        for ch in [Channels::Gray, Channels::Rgba] {
            for kind in [0, 1, 3, 5] {
                let (info, px) = image(301, 97, ch, st, kind, 99);
                for cfg in [
                    {
                        let mut cfg = imij::ImiEncoderConfig::default();
                        cfg = cfg.tune_lossless(|t| {
                            t.stripe_rows = Some(1);
                        });
                        cfg = cfg.tune_lossless(|t| {
                            t.lanes = Some(1);
                        });
                        cfg
                    },
                    {
                        let mut cfg = imij::ImiEncoderConfig::default();
                        cfg = cfg.tune_lossless(|t| {
                            t.stripe_rows = Some(13);
                        });
                        cfg = cfg.tune_lossless(|t| {
                            t.lanes = Some(5);
                        });
                        cfg = cfg.with_threads(4);
                        cfg
                    },
                    {
                        let mut cfg = imij::ImiEncoderConfig::default();
                        cfg = cfg.with_column_groups(Some(64));
                        cfg = cfg.tune_lossless(|t| {
                            t.stripe_rows = Some(7);
                        });
                        cfg = cfg.with_threads(3);
                        cfg
                    },
                    {
                        let mut cfg = imij::ImiEncoderConfig::default();
                        cfg = cfg.tune_lossless(|t| {
                            t.palette = false;
                        });
                        cfg = cfg.with_effort(0.0);
                        cfg
                    },
                    {
                        let mut cfg = imij::ImiEncoderConfig::default();
                        cfg = cfg.with_effort(10.0);
                        cfg
                    },
                ] {
                    let file = enc(&info, &px, &cfg);
                    check(&file, &info, &px);
                }
            }
        }
    }
}

#[test]
fn every_predictor_and_decorrelation() {
    for st in NEW {
        if !available(st) {
            continue;
        }
        for kind in [1, 3, 4] {
            let (info, px) = image(67, 29, Channels::Rgb, st, kind, 5);
            for p in 0..7u8 {
                for d in 0..4u8 {
                    let cfg = {
                        let mut cfg = imij::ImiEncoderConfig::default();
                        cfg = cfg.tune_lossless(|t| {
                            t.predictor_mask = 1 << p;
                        });
                        cfg = cfg.tune_lossless(|t| {
                            t.decor_mask = 1 | (1 << d);
                        });
                        cfg = cfg.tune_lossless(|t| {
                            t.palette = false;
                        });
                        cfg = cfg.with_effort(10.0);
                        cfg
                    };
                    check(&enc(&info, &px, &cfg), &info, &px);
                }
            }
        }
    }
}

#[test]
fn i8_and_i16_every_value() {
    // i8: all 256 values in every channel layout.
    for ch in CHANNELS {
        let n = ch.count();
        let info = ImageInfo::new(16, 16, ch, SampleType::I8);
        let px: Vec<u8> = (0..256 * n)
            .map(|i| ((i / n) as u8).wrapping_add((i % n) as u8 * 61))
            .collect();
        check(
            &enc(&info, &px, &imij::ImiEncoderConfig::default()),
            &info,
            &px,
        );
    }
    // i16: all 65536 values, in order and scrambled.
    let info = ImageInfo::new(256, 256, Channels::Gray, SampleType::I16);
    let ordered: Vec<u8> = (0..=u16::MAX).flat_map(|v| v.to_ne_bytes()).collect();
    check(
        &enc(&info, &ordered, &imij::ImiEncoderConfig::default()),
        &info,
        &ordered,
    );
    let scrambled: Vec<u8> = (0..=u16::MAX)
        .flat_map(|v| v.wrapping_mul(40503).rotate_left(5).to_ne_bytes())
        .collect();
    check(
        &enc(&info, &scrambled, &imij::ImiEncoderConfig::default()),
        &info,
        &scrambled,
    );
}

/// Signed samples order like integers: the coded values of a smooth signed
/// ramp crossing zero are contiguous, so it costs about as much as the same
/// ramp shifted to unsigned.
#[test]
fn signed_ramps_cost_like_unsigned() {
    for (st, ust, bytes) in [
        (SampleType::I8, SampleType::U8, 1usize),
        (SampleType::I16, SampleType::U16, 2),
    ] {
        let info = ImageInfo::new(256, 64, Channels::Gray, st);
        let uinfo = ImageInfo::new(256, 64, Channels::Gray, ust);
        let half = 1i32 << (8 * bytes - 1);
        let (mut s, mut u) = (Vec::new(), Vec::new());
        for y in 0..64i32 {
            for x in 0..256i32 {
                let v = ((x - 128) * (bytes as i32 * 2 - 1) + y % 3).clamp(-half, half - 1);
                put(&mut s, st, v as u64);
                put(&mut u, ust, (v + half) as u64);
            }
        }
        let a = enc(&info, &s, &imij::ImiEncoderConfig::default());
        let b = enc(&uinfo, &u, &imij::ImiEncoderConfig::default());
        check(&a, &info, &s);
        // The signed file adds its 5-byte required-profiles record, and i8
        // writes its sample byte (u8 is the header default).
        let overhead = 5 + usize::from(st == SampleType::I8);
        assert!(
            a.len() <= b.len() + overhead,
            "{st:?}: {} vs {}",
            a.len(),
            b.len()
        );
    }
}

/// f64 values held as f32 (`v` as f64), coded as f64 and as f32. The f32
/// side keeps the map set f64 has (no Scientific `FloatShift`, which
/// codes mixed-sign f32 with dead low bits smaller still).
#[cfg(feature = "extended")]
fn f64_and_f32(kind: u8) -> (usize, usize) {
    let (info, px) = image(256, 64, Channels::Gray, SampleType::F64, kind, 1);
    let wide = enc(&info, &px, &imij::ImiEncoderConfig::default());
    check(&wide, &info, &px);
    let narrow: Vec<u8> = px
        .as_chunks::<8>()
        .0
        .iter()
        .flat_map(|b| (f64::from_ne_bytes(*b) as f32).to_ne_bytes())
        .collect();
    let info32 = ImageInfo::new(256, 64, Channels::Gray, SampleType::F32);
    let same_maps = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.float_shift = false;
        });
        cfg
    };
    (wide.len(), enc(&info32, &narrow, &same_maps).len())
}

#[cfg(feature = "extended")]
#[test]
fn wide_words_take_compact_maps() {
    // f64 promoted from f32 with both signs: the lo word keeps 3 raw bits
    // (Shift), integer-valued f64 has all-zero lo words (Const). Either
    // costs about as much as the same values coded as f32.
    for kind in [3, 4] {
        let (wide, narrow) = f64_and_f32(kind);
        eprintln!("kind {kind}: f64 {wide} B, f32 {narrow} B");
        assert!(
            wide <= narrow + narrow / 8 + 64,
            "kind {kind}: {wide} vs {narrow}"
        );
    }
    // i64 holding small-range values codes compactly: Offset maps on the
    // hi word (constant) and the lo word.
    let (info, px) = image(256, 64, Channels::Gray, SampleType::I64, 4, 3);
    let wide = enc(&info, &px, &imij::ImiEncoderConfig::default());
    check(&wide, &info, &px);
    let (info16, px16) = image(256, 64, Channels::Gray, SampleType::I16, 4, 3);
    let narrow = enc(&info16, &px16, &imij::ImiEncoderConfig::default());
    eprintln!("i64 small range {} B, i16 {} B", wide.len(), narrow.len());
    assert!(wide.len() < info.row_bytes() * 64 / 4);
}

#[test]
fn significant_bits_are_informational() {
    for (st, bits) in [
        (SampleType::I8, 5u8),
        (SampleType::I16, 12),
        (SampleType::U64, 40),
        (SampleType::I64, 33),
    ] {
        if !available(st) {
            continue;
        }
        let (info, px) = image(41, 19, Channels::GrayAlpha, st, 0, 4);
        let info = info.with_significant_bits(&[bits, 1]).unwrap();
        let file = enc(&info, &px, &imij::ImiEncoderConfig::default());
        check(&file, &info, &px);
    }
    let info = ImageInfo::new(4, 4, Channels::Gray, SampleType::F64);
    assert!(info.clone().with_significant_bits(&[52]).is_err());
    assert!(info.with_significant_bits(&[64]).is_ok());
    assert!(
        ImageInfo::new(4, 4, Channels::Gray, SampleType::I16)
            .with_significant_bits(&[17])
            .is_err()
    );
}

/// Bytes of the masked header of a lossless file: signature, length, then
/// the header record (version, present mask, fields, extensions).
fn header_record(file: &[u8]) -> &[u8] {
    let n = file[8] as usize;
    assert!(n < 128);
    &file[9..9 + n]
}

#[test]
fn files_declare_their_feature() {
    use common::feature::{OFFSET_MAP, SIGNED_SAMPLES, WIDE_SAMPLES};
    for (st, feature) in [
        (SampleType::I8, SIGNED_SAMPLES),
        (SampleType::I16, SIGNED_SAMPLES),
        (SampleType::U64, WIDE_SAMPLES),
        (SampleType::I64, WIDE_SAMPLES),
        (SampleType::F64, WIDE_SAMPLES),
    ] {
        if !available(st) {
            continue;
        }
        let (info, px) = image(9, 5, Channels::Gray, st, 1, 2);
        let file = enc(&info, &px, &imij::ImiEncoderConfig::default());
        let h = header_record(&file);
        // Version 4, present (sample, features, channels), the feature
        // mask, sample, Gray, 9, 5. The mask holds the sample type's
        // feature, and `offset_map` when a 64-bit channel's words use it.
        let mask = common::feature_mask(&file);
        assert_eq!(mask & !OFFSET_MAP, feature, "{st:?}");
        let k = common::varint_bytes(mask).len();
        assert_eq!(&h[..2], &[4, 14]);
        assert_eq!(&h[2 + k..], &[st as u8, 1, 9, 5]);
        check(&file, &info, &px);

        // Without the feature the file is corrupt.
        let rest = mask & !feature;
        let bad = common::with_feature_mask(&file, (rest != 0).then_some(rest));
        assert!(matches!(decode(&bad), Err(Error::Corrupt(_))), "{st:?}");
        assert!(matches!(imij::probe(&bad), Err(Error::Corrupt(_))));
        // The other sample feature is not enough either.
        let other = common::with_feature_mask(
            &file,
            Some(rest | (SIGNED_SAMPLES ^ WIDE_SAMPLES ^ feature)),
        );
        assert!(matches!(decode(&other), Err(Error::Corrupt(_))), "{st:?}");
    }
    // Existing sample types declare nothing.
    let info = ImageInfo::new(9, 5, Channels::Gray, SampleType::U16);
    let file = enc(&info, &[7; 90], &imij::ImiEncoderConfig::default());
    assert_eq!(header_record(&file), &[4, 10, 1, 1, 9, 5]);
    // A sample type this decoder does not know is unsupported.
    let mut future = file.clone();
    future[9 + 2] = 11;
    assert!(matches!(decode(&future), Err(Error::Unsupported(_))));
}

/// Every way to state lossy quality: the bare scale and each `LossyQuality`
/// target (PR #7), including one resolved against the Fastest tier's color.
fn lossy_settings() -> [ImiEncoderConfig; 7] {
    use imij::{ButteraugliNorm, DecodeSpeed, LossyQuality};
    let target = |q| imij::ImiEncoderConfig::lossy(90.0).with_quality(q);
    [
        imij::ImiEncoderConfig::lossy(90.0),
        target(LossyQuality::Ssim2(70.0)),
        target(LossyQuality::Butteraugli {
            distance: 2.0,
            norm: ButteraugliNorm::Max,
        }),
        target(LossyQuality::Butteraugli {
            distance: 1.0,
            norm: ButteraugliNorm::P3,
        }),
        target(LossyQuality::JpegQuality(75.0)),
        target(LossyQuality::JxlDistance(2.0)),
        target(LossyQuality::Ssim2(70.0)).with_decode_speed(DecodeSpeed::Fastest),
    ]
}

/// Lossy coding rejects the new types however quality is stated, in stills,
/// animation frames, pages and mixed pages.
#[test]
fn lossy_rejects_new_samples() {
    use imij::{FrameConfig, FrameDuration, FrameEncoder};
    for st in NEW {
        for ch in [Channels::Gray, Channels::Rgb, Channels::Rgba] {
            let (info, px) = image(16, 16, ch, st, 1, 1);
            for cfg in lossy_settings() {
                let r = cfg.encode_samples(
                    imij::ImageRef::new(&px, info.row_bytes(), &info),
                    &Metadata::default(),
                );
                assert!(
                    matches!(r, Err(Error::Unsupported(_))),
                    "{st:?} {ch:?} {:?}: {r:?}",
                    cfg.coder()
                );
                let frames = FrameConfig::new(cfg.clone());
                let mut anim =
                    FrameEncoder::animation(info.clone(), Metadata::default(), frames.clone());
                let r = anim.push_frame(&px, info.row_bytes(), FrameDuration::from_millis(40));
                assert!(
                    matches!(r, Err(Error::Unsupported(_))),
                    "{st:?} frame: {r:?}"
                );
                let mut pages = FrameEncoder::pages(info.clone(), Metadata::default(), frames);
                let r = pages.push_page(&px, info.row_bytes(), &info);
                assert!(
                    matches!(r, Err(Error::Unsupported(_))),
                    "{st:?} page: {r:?}"
                );
                let lossless = FrameConfig::new(imij::ImiEncoderConfig::default());
                let r = FrameEncoder::mixed_pages(lossless).push_page_with(
                    &px,
                    info.row_bytes(),
                    &info,
                    &Metadata::default(),
                    &cfg,
                );
                assert!(
                    matches!(r, Err(Error::Unsupported(_))),
                    "{st:?} mixed page: {r:?}"
                );
            }
        }
    }
}

/// A baseline build rejects signed and 64-bit files; probing still works.
#[cfg(not(feature = "extended"))]
#[test]
fn baseline_build_rejects_64_bit() {
    let (info, px) = image(8, 8, Channels::Gray, SampleType::I16, 1, 1);
    let signed = enc(&info, &px, &imij::ImiEncoderConfig::default());
    assert_eq!(
        decode(&signed),
        Err(Error::Unsupported("signed samples (extended tier)"))
    );
    assert_eq!(imij::probe(&signed).unwrap().0.sample, SampleType::I16);
    let (info, px) = image(8, 8, Channels::Gray, SampleType::F64, 1, 1);
    let file = enc(&info, &px, &imij::ImiEncoderConfig::default());
    assert!(matches!(decode(&file), Err(Error::Unsupported(_))));
    assert_eq!(imij::probe(&file).unwrap().0.sample, SampleType::F64);
}

/// Few-color 64-bit images: every word fits 16 bits, so palette stripes
/// (at most four slots) compete with plain coding; both decode exactly.
#[cfg(feature = "extended")]
#[test]
fn palette_stripes_on_wide_words() {
    let colors: [[u64; 2]; 4] = [[0, 0], [5, u64::MAX], [9, 0], [3, u64::MAX]];
    for (st, ch) in [
        (SampleType::U64, Channels::GrayAlpha),
        (SampleType::I64, Channels::Gray),
        (SampleType::F64, Channels::GrayAlpha),
    ] {
        let (w, h) = (96u32, 70u32);
        let info = ImageInfo::new(w, h, ch, st);
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let c = colors[((x / 11 + y / 9) % 4) as usize];
                let v = |k: u64| match st {
                    SampleType::F64 => (k as f64).to_bits(),
                    _ => k,
                };
                put(&mut px, st, v(c[0]));
                if ch == Channels::GrayAlpha {
                    put(&mut px, st, c[1]);
                }
            }
        }
        let on = enc(&info, &px, &imij::ImiEncoderConfig::default());
        let off = enc(&info, &px, &{
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.tune_lossless(|t| {
                t.palette = false;
            });
            cfg
        });
        check(&on, &info, &px);
        check(&off, &info, &px);
        eprintln!(
            "{st:?} {ch:?}: palette {} B, plain {} B",
            on.len(),
            off.len()
        );
    }
}

/// Animations (inter frames for i8/i16; 64-bit frames are intra) and pages.
#[test]
fn frames_and_pages() {
    use imij::{FrameConfig, FrameDecoder, FrameDuration, FrameEncoder};
    for st in NEW {
        if !available(st) {
            continue;
        }
        let (info, base) = image(64, 40, Channels::Rgba, st, 1, 3);
        let frames: Vec<Vec<u8>> = (0..4)
            .map(|f| {
                let mut p = base.clone();
                let bpp = info.bytes_per_pixel();
                for y in 10..20 {
                    for b in &mut p[(y * 64 + 5 * f) * bpp..(y * 64 + 5 * f + 9) * bpp] {
                        *b = b.wrapping_add(17 * f as u8 + 1);
                    }
                }
                p
            })
            .collect();
        let cfg = FrameConfig::new(imij::ImiEncoderConfig::default());
        let mut enc = FrameEncoder::animation(info.clone(), Metadata::default(), cfg.clone());
        for f in &frames {
            enc.push_frame(f, info.row_bytes(), FrameDuration::from_millis(40))
                .unwrap();
        }
        let file = enc.finish().unwrap();
        let mut dec = FrameDecoder::new(&file).unwrap();
        let mut canvas = vec![0u8; info.row_bytes() * 40];
        for f in &frames {
            dec.next_frame().unwrap().unwrap();
            dec.decode_into(&mut canvas, info.row_bytes(), 2).unwrap();
            assert!(canvas == *f, "{st:?} frame");
        }
        assert!(dec.next_frame().unwrap().is_none());

        let mut pages = FrameEncoder::pages(info.clone(), Metadata::default(), cfg);
        let mut want = Vec::new();
        for (i, (w, h)) in [(64, 40), (13, 70), (1, 1)].into_iter().enumerate() {
            let (pi, px) = image(w, h, Channels::Rgba, st, i as u8 * 2, 9);
            pages.push_page(&px, pi.row_bytes(), &pi).unwrap();
            want.push(px);
        }
        let file = pages.finish().unwrap();
        let mut dec = FrameDecoder::new(&file).unwrap();
        for px in want {
            let h = dec.next_frame().unwrap().unwrap();
            let rb = h.width as usize * info.bytes_per_pixel();
            let mut out = vec![0u8; rb * h.height as usize];
            dec.decode_into(&mut out, rb, 1).unwrap();
            assert!(out == px, "{st:?} page");
        }
    }
}

/// Decode sessions (cumulative budgets, row callbacks, reader) and preview
/// layers carry the new types like any other.
#[test]
fn sessions_and_previews() {
    use imij::decode_resources::{DecodePolicy, DecodeSession};
    for st in NEW {
        if !available(st) {
            continue;
        }
        let (info, px) = image(150, 66, Channels::Rgb, st, 1, 8);
        let (pinfo, ppx) = image(37, 16, Channels::Rgb, st, 3, 9);
        let file = imij::ImiEncoderConfig::default()
            .encode_samples_with_previews(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
                &[imij::ImageRef {
                    pixels: &ppx,
                    stride: pinfo.row_bytes(),
                    info: &pinfo,
                }],
            )
            .unwrap();
        check(&file, &info, &px);
        let previews = imij::previews(&file).unwrap();
        assert_eq!(previews.len(), 1);
        check(previews[0], &pinfo, &ppx);

        let mut session = DecodeSession::new(DecodePolicy::default());
        assert!(session.decode(&file).unwrap().1 == px);
        let mut rows = Vec::new();
        let mut row = vec![0u8; info.row_bytes()];
        session
            .decode_rows(&file, &mut row, |_, _, r| {
                rows.extend_from_slice(r);
                Ok(())
            })
            .unwrap();
        assert!(rows == px);
        let mut rows = Vec::new();
        session
            .decode_reader(&file[..], |_, _, r| {
                rows.extend_from_slice(r);
                Ok(())
            })
            .unwrap();
        assert!(rows == px);
    }
}

/// Mutated files of the new types (bit flips, overwritten runs, truncation,
/// header and head bytes) return errors or garbage, never panic, through
/// the slice, streaming and reader decoders.
#[test]
fn mutated_new_type_files_do_not_panic() {
    let mut files = Vec::new();
    for st in NEW {
        for (ch, kind) in [
            (Channels::Rgba, 1),
            (Channels::Gray, 3),
            (Channels::GrayAlpha, 5),
        ] {
            let (info, px) = image(45, 21, ch, st, kind, 77);
            files.push(enc(&info, &px, &imij::ImiEncoderConfig::default()));
        }
    }
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    let iters = std::env::var("IMIJ_FUZZ_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3000);
    let mut errs = 0;
    for it in 0..iters {
        let mut f = files[it % files.len()].clone();
        match r.next() % 4 {
            0 => {
                for _ in 0..1 + r.next() % 8 {
                    let i = (r.next() as usize) % f.len();
                    f[i] ^= 1 << (r.next() % 8);
                }
            }
            1 => {
                let i = (r.next() as usize) % f.len();
                for j in i..(i + 1 + (r.next() % 16) as usize).min(f.len()) {
                    f[j] = r.next() as u8;
                }
            }
            2 => f.truncate((r.next() as usize) % f.len()),
            _ => {
                let i = (r.next() as usize) % f.len().min(120);
                f[i] = r.next() as u8;
            }
        }
        if decode(&f).is_err() {
            errs += 1;
        }
        if let Ok(mut s) = StreamDecoder::new(&f) {
            let mut row = vec![0u8; s.info().row_bytes()];
            while let Ok(true) = s.next_row(&mut row) {}
        }
        let _ = imij::decode_reader(&f[..], &imij::Limits::DEFAULT, |_, _, _| Ok(()));
    }
    eprintln!("{errs}/{iters} mutated files rejected");
}
