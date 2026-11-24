//! Extended 32-bit sample coding for U32, I32 and F32, bit-exactly,
//! including every 32-bit channel map and content kind.

use imij::{Channels, ImageInfo, Metadata, SampleType, decode};
#[cfg(feature = "extended")]
use imij::{StreamDecoder, Tier};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// Generates one 32-bit sample (as raw bits) for content `kind`.
fn sample(kind: u8, st: SampleType, x: u32, y: u32, c: u32, r: &mut Rng) -> u32 {
    let grad = x * 37 + y * 11 + c * 1000;
    match (kind, st) {
        (0, _) => r.next() as u32, // full noise
        (1, SampleType::F32) => ((grad as f32) * 0.001 + (r.next() % 7) as f32 * 1e-4).to_bits(),
        (1, SampleType::I32) => (grad as i32 - 2000 + (r.next() % 9) as i32) as u32,
        (1, _) => grad * 3 + (r.next() % 9) as u32, // smooth u32
        (2, SampleType::F32) => (((x / 8 + y / 8) % 3) as f32 * -1.5).to_bits(), // flat blocks incl. -0
        (2, _) => {
            if (x / 8 + y / 8).is_multiple_of(2) {
                u32::MAX
            } else {
                0
            }
        }
        (3, SampleType::F32) => (((grad + (r.next() % 3) as u32) % 256) as f32 / 255.0).to_bits(), // unorm8 in f32
        (3, _) => ((grad + (r.next() % 3) as u32) % 4096) << 20, // MSB-aligned 12-bit
        (4, SampleType::F32) => {
            f32::from_bits(widen_half(((grad + (r.next() % 50) as u32) % 30000) as u16)).to_bits()
        } // f16 in f32
        (4, _) => 100_000 + (grad + (r.next() % 50) as u32) % 5000, // offset small range
        (5, SampleType::F32) => {
            // Specials: NaN payloads, infinities, subnormals, negative zero.
            [
                f32::NAN.to_bits() | 0x1234,
                f32::INFINITY.to_bits(),
                f32::NEG_INFINITY.to_bits(),
                1,
                0x8000_0000,
                0x0080_0000,
            ][((x + y + c) % 6) as usize]
        }
        _ => (grad as i64 * 77_777) as u32,
    }
}

fn widen_half(h: u16) -> u32 {
    // Same exact widening as the codec (normal numbers only in this test).
    let sign = ((h >> 15) as u32) << 31;
    let exp = ((h >> 10) & 0x1F) as u32;
    let man = (h & 0x3FF) as u32;
    if exp == 0 || exp == 31 {
        return sign;
    }
    sign | ((exp + 112) << 23) | (man << 13)
}

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
                let v = if alpha && kind != 0 {
                    // Binary alpha in the container's terms.
                    match st {
                        SampleType::F32 => {
                            if x % 5 < 2 {
                                1.0f32.to_bits()
                            } else {
                                0
                            }
                        }
                        _ => {
                            if x % 5 < 2 {
                                u32::MAX
                            } else {
                                0
                            }
                        }
                    }
                } else {
                    sample(kind, st, x, y, c, &mut r)
                };
                px.extend_from_slice(&v.to_ne_bytes());
            }
        }
    }
    (info, px)
}

#[cfg(feature = "extended")]
#[test]
fn ext32_roundtrip_matrix() {
    for st in [SampleType::U32, SampleType::I32, SampleType::F32] {
        assert_eq!(st.tier(), Tier::Extended);
        for ch in [
            Channels::Gray,
            Channels::GrayAlpha,
            Channels::Rgb,
            Channels::Rgba,
        ] {
            for kind in 0..6u8 {
                for &(w, h) in &[(1u32, 1u32), (37, 9), (300, 21)] {
                    let (info, px) = image(w, h, ch, st, kind, (w * 7 + h) as u64 + kind as u64);
                    let enc = imij::ImiEncoderConfig::default()
                        .encode_samples(
                            imij::ImageRef::new(&px, info.row_bytes(), &info),
                            &Metadata::default(),
                        )
                        .unwrap();
                    let (dinfo, dec) = decode(&enc).unwrap();
                    assert_eq!(dinfo, info);
                    assert!(dec == px, "{st:?} {ch:?} kind {kind} {w}x{h}");
                    // Streaming decode agrees too.
                    let mut d = StreamDecoder::new(&enc).unwrap();
                    let mut row = vec![0u8; info.row_bytes()];
                    let mut y = 0;
                    while d.next_row(&mut row).unwrap() {
                        assert_eq!(
                            &row[..],
                            &px[y * info.row_bytes()..(y + 1) * info.row_bytes()]
                        );
                        y += 1;
                    }
                }
            }
        }
    }
}

#[test]
fn ext32_maps_shrink_output() {
    // unorm8-in-f32 and f16-in-f32 must cost about as much as their real depth.
    for (kind, max_bpp) in [(3u8, 9.0f64), (4, 17.0)] {
        let (info, px) = image(256, 64, Channels::Gray, SampleType::F32, kind, 3);
        let enc = imij::ImiEncoderConfig::default()
            .encode_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        let bpp = enc.len() as f64 * 8.0 / (256.0 * 64.0);
        assert!(bpp < max_bpp, "kind {kind}: {bpp:.2} bits/sample");
    }
}

#[cfg(feature = "extended")]
#[test]
fn ext32_configs_and_threads() {
    for cfg in [
        {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.tune_lossless(|t| {
                t.lanes = Some(1);
            });
            cfg
        },
        {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.tune_lossless(|t| {
                t.stripe_rows = Some(5);
            });
            cfg = cfg.tune_lossless(|t| {
                t.max_ctx = 1;
            });
            cfg = cfg.tune_lossless(|t| {
                t.run_min = 0;
            });
            cfg
        },
        {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.tune_lossless(|t| {
                t.k_high = 2;
            });
            cfg = cfg.tune_lossless(|t| {
                t.m = 1;
            });
            cfg
        },
        {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.tune_lossless(|t| {
                t.predictor_mask = 1 << 5;
            });
            cfg = cfg.tune_lossless(|t| {
                t.decor_mask = 1 << 3;
            });
            cfg
        },
        {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.tune_lossless(|t| {
                t.predictor_mask = 1 << 2;
            });
            cfg = cfg.tune_lossless(|t| {
                t.decor_mask = 1 << 2;
            });
            cfg
        },
    ] {
        for kind in [0u8, 1, 4] {
            let (info, px) = image(129, 33, Channels::Rgb, SampleType::I32, kind, 9);
            let enc = cfg
                .encode_samples(
                    imij::ImageRef::new(&px, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap();
            let mut out = vec![0u8; px.len()];
            imij::decode_into_threads(&enc, &mut out, info.row_bytes(), 4).unwrap();
            assert!(out == px, "{cfg:?} kind {kind}");
        }
    }
}

/// A baseline-only decoder must reject extended files cleanly.
#[cfg(not(feature = "extended"))]
#[test]
fn baseline_build_rejects_extended() {
    let (info, px) = image(16, 4, Channels::Rgb, SampleType::U32, 1, 1);
    let enc = imij::ImiEncoderConfig::default()
        .encode_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    assert!(matches!(decode(&enc), Err(imij::Error::Unsupported(_))));
    // Probing still reports the header.
    assert_eq!(imij::probe(&enc).unwrap().0.sample, SampleType::U32);
}

/// A baseline-only build still writes (and remuxes) extended files with two
/// or more units: finding the unit table's end for the DATA locator parses
/// structure, not samples.
#[cfg(not(feature = "extended"))]
#[test]
fn baseline_build_writes_multi_unit_extended_files() {
    let (info, px) = image(300, 200, Channels::Rgb, SampleType::U32, 1, 1);
    let cfg = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some(16);
        });
        cfg
    };
    let meta = Metadata {
        xmp: Some(b"<x:xmpmeta/>".repeat(64)),
        ..Default::default()
    };
    let enc = cfg
        .encode_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &meta)
        .unwrap();
    assert!(matches!(decode(&enc), Err(imij::Error::Unsupported(_))));
    assert_eq!(imij::probe(&enc).unwrap().0.sample, SampleType::U32);
    let remuxed = imij::compress_metadata(&enc, 1 << 16).unwrap();
    assert!(matches!(decode(&remuxed), Err(imij::Error::Unsupported(_))));
    assert_eq!(imij::probe(&remuxed).unwrap().1.xmp, meta.xmp);
}

#[cfg(feature = "extended")]
#[test]
fn ext32_every_predictor_and_decor() {
    for p in 0..7u8 {
        for decor in 0..4u8 {
            let cfg = {
                let mut cfg = imij::ImiEncoderConfig::default();
                cfg = cfg.tune_lossless(|t| {
                    t.predictor_mask = 1 << p;
                });
                cfg = cfg.tune_lossless(|t| {
                    t.decor_mask = 1 << decor;
                });
                cfg
            };
            for (st, kind) in [
                (SampleType::U32, 0u8),
                (SampleType::I32, 1),
                (SampleType::F32, 1),
                (SampleType::F32, 5),
                (SampleType::U32, 6),
            ] {
                for ch in [Channels::Rgb, Channels::Rgba, Channels::GrayAlpha] {
                    let (info, px) = image(71, 13, ch, st, kind, p as u64 * 31 + decor as u64);
                    let enc = cfg
                        .encode_samples(
                            imij::ImageRef::new(&px, info.row_bytes(), &info),
                            &Metadata::default(),
                        )
                        .unwrap();
                    let (_, dec) = decode(&enc).unwrap();
                    assert!(
                        dec == px,
                        "pred {p} decor {decor} {st:?} kind {kind} {ch:?}"
                    );
                }
            }
        }
    }
}
