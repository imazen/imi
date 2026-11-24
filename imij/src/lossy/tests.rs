use super::*;
use alloc::vec::Vec;

// Exercise the available caller-buffer API in each build. The no_std codec
// has no public threaded entry point; its scalar/SIMD checks still run here.
fn decode_test_pool(data: &[u8], out: &mut [u8], stride: usize, threads: usize) {
    #[cfg(feature = "std")]
    crate::decode_into_threads(data, out, stride, threads).unwrap();
    #[cfg(not(feature = "std"))]
    {
        let _ = threads;
        crate::decode_into(data, out, stride).unwrap();
    }
}

/// Deterministic photo-like test image: smooth gradients, texture, edges.
pub(crate) fn test_image(w: u32, h: u32, seed: u32) -> Vec<u8> {
    let mut s = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    let mut px = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let n = (s % 9) as i32 - 4;
            let edge = if (x / 11 + y / 7) % 3 == 0 { 60 } else { 0 };
            let r = (x * 255 / w.max(1)) as i32 + n + edge;
            let g = (y * 255 / h.max(1)) as i32 - n;
            let b = ((x + y) * 128 / (w + h)) as i32 + 64 + edge / 2;
            px.extend([r, g, b].map(|v| v.clamp(0, 255) as u8));
        }
    }
    px
}

/// SSIMULACRA2 of 8-bit sRGB (`nch` 3) or gray (`nch` 1) samples.
fn ssim2(a: &[u8], b: &[u8], w: u32, nch: usize) -> f64 {
    let (w, h) = (w as usize, a.len() / nch / w as usize);
    let r = if nch == 1 {
        fast_ssim2::compute_ssimulacra2(imgref::Img::new(a, w, h), imgref::Img::new(b, w, h))
    } else {
        let (pa, pb) = (a.as_chunks::<3>().0, b.as_chunks::<3>().0);
        fast_ssim2::compute_ssimulacra2(imgref::Img::new(pa, w, h), imgref::Img::new(pb, w, h))
    };
    r.unwrap()
}

/// SSIMULACRA2 of native-endian 16-bit RGB samples, read as sRGB.
fn ssim2_16(a: &[u8], b: &[u8], w: u32) -> f64 {
    let px = |s: &[u8]| -> Vec<[u16; 3]> {
        s.as_chunks::<6>()
            .0
            .iter()
            .map(|p| core::array::from_fn(|c| u16::from_ne_bytes([p[2 * c], p[2 * c + 1]])))
            .collect()
    };
    let (pa, pb) = (px(a), px(b));
    let (w, h) = (w as usize, pa.len() / w as usize);
    fast_ssim2::compute_ssimulacra2(
        imgref::Img::new(&pa[..], w, h),
        imgref::Img::new(&pb[..], w, h),
    )
    .unwrap()
}

fn roundtrip(w: u32, h: u32, cfg: &LossyConfig) -> (Vec<u8>, Vec<u8>, usize) {
    let px = test_image(w, h, w * 31 + h);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let enc = encode_lossy(&px, w as usize * 3, &info, &Metadata::default(), cfg).unwrap();
    let (di, out) = crate::decode(&enc).unwrap();
    assert_eq!(di.width, w);
    assert_eq!(di.height, h);
    (px, out, enc.len())
}

#[test]
fn quality_increases_with_q() {
    for color in [LossyColor::Xyb, LossyColor::YCoCg] {
        let mut last = 0.0;
        for q in [30.0, 60.0, 90.0, 100.0, 108.0] {
            let cfg = LossyConfig::new(q).with_color(color);
            let (a, b, n) = roundtrip(160, 96, &cfg);
            let p = ssim2(&a, &b, 160, 3);
            assert!(p > last - 1.0, "{color:?} q{q}: ssim2 {p} after {last}");
            last = p;
            if q >= 100.0 {
                assert!(p > 90.0, "{color:?} q{q}: ssim2 {p} ({n} bytes)");
            }
        }
    }
}

#[test]
fn odd_sizes_and_stripes() {
    for (w, h) in [
        (1, 1),
        (7, 5),
        (8, 8),
        (9, 17),
        (33, 70),
        (130, 41),
        (257, 9),
    ] {
        for sb in [1, 3, 8] {
            let cfg = LossyConfig::new(92.0).with_stripe_block_rows(sb);
            let (a, b, _) = roundtrip(w, h, &cfg);
            let p = ssim2(&a, &b, w, 3);
            // This content scores ~57-58 at q92 when a side is <= 70 px
            // (steep gradients and edge texture per pixel); ~97 on thin strips.
            assert!(p > 50.0, "{w}x{h} sb{sb}: ssim2 {p}");
        }
    }
}

#[test]
fn threads_do_not_change_output() {
    // One stripe, uneven worker partitions, and short final column groups.
    for (w, h, sb, group) in [(7, 5, 1, None), (137, 73, 1, Some(64)), (300, 200, 2, None)] {
        let px = test_image(w, h, 5);
        let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
        for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
            let mut cfg = LossyConfig::new(85.0).with_stripe_block_rows(sb);
            cfg.entropy = entropy;
            cfg.column_groups = group;
            let e1 =
                encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), &cfg).unwrap();
            for threads in [2, 4, 7] {
                let en = encode_lossy(
                    &px,
                    info.row_bytes(),
                    &info,
                    &Metadata::default(),
                    &cfg.clone().with_threads(threads),
                )
                .unwrap();
                assert_eq!(e1, en, "{w}x{h} {entropy:?} threads={threads}");
            }
            #[cfg(feature = "std")]
            {
                let (_, d1) = crate::decode(&e1).unwrap();
                let mut d4 = vec![0u8; d1.len()];
                crate::decode_into_threads(&e1, &mut d4, info.row_bytes(), 4).unwrap();
                assert_eq!(d1, d4);
            }
        }
    }
}

#[test]
fn corrupt_streams_do_not_panic() {
    let (w, h) = (64, 40);
    let px = test_image(w, h, 9);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let enc = encode_lossy(
        &px,
        w as usize * 3,
        &info,
        &Metadata::default(),
        &LossyConfig::default(),
    )
    .unwrap();
    let mut s = 12345u32;
    for i in 0..3000 {
        let mut d = enc.clone();
        for _ in 0..1 + i % 4 {
            s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let at = (s >> 8) as usize % d.len();
            d[at] ^= 1 << (s % 8);
        }
        if i % 7 == 0 {
            d.truncate((s as usize >> 3) % d.len());
        }
        let _ = crate::decode(&d);
    }
}

/// Decodes with the kernels and with the scalar reference.
pub(crate) fn both(enc: &[u8]) -> (Result<Vec<u8>, Error>, Result<Vec<u8>, Error>) {
    let run = |simd: bool| {
        let (info, color, body, lossy) =
            crate::v1::lossy_body(enc)?.ok_or(Error::Corrupt("not a lossy file"))?;
        if info.width as u64 * info.height as u64 > 1 << 20 {
            return Err(Error::TooLarge);
        }
        let mut out = vec![0u8; info.checked_buffer_len().ok_or(Error::TooLarge)?];
        let rb = info.row_bytes();
        decode_lossy_impl(
            info,
            crate::parse::header::HEADER_VERSION,
            color,
            lossy,
            body,
            &mut out,
            rb,
            1,
            &crate::Limits::DEFAULT,
            simd,
            false,
        )?;
        Ok(out)
    };
    (run(true), run(false))
}

/// Every build supports internal 32-bit DCs; all requested encodes must succeed.
fn required_encode(_q: f32, r: Result<Vec<u8>, Error>) -> Option<Vec<u8>> {
    Some(r.unwrap())
}

/// Full-range content (alternating white runs and a ramp) at every quality up
/// to 100 keeps the DC image 16-bit, so it encodes without `extended`.
#[test]
fn dc_fits_16_bits_up_to_q100() {
    let (w, h) = (64u32, 64u32);
    for st in [SampleType::U8, SampleType::U16] {
        let n = st.bytes();
        let px: Vec<u8> = (0..(w * h * 3) as usize * n)
            .map(|i| {
                if (i / (24 * n)) % 2 == 0 {
                    255
                } else {
                    (i * 7 % 256) as u8
                }
            })
            .collect();
        let info = ImageInfo::new(w, h, Channels::Rgb, st);
        for color in [LossyColor::Xyb, LossyColor::YCoCg] {
            for q in [50.0, 85.0, 90.0, 95.0, 97.0, 99.0, 100.0] {
                let cfg = LossyConfig::new(q).with_color(color);
                let enc = encode_lossy(&px, w as usize * 3 * n, &info, &Metadata::default(), &cfg)
                    .unwrap_or_else(|e| panic!("{st:?} {color:?} q{q}: {e:?}"));
                #[cfg(feature = "lab")]
                {
                    let (dc, _, _) = crate::lab::lossy_dc_image(&enc).unwrap();
                    assert_eq!(dc.sample, SampleType::U16, "{st:?} {color:?} q{q}");
                }
                #[cfg(not(feature = "lab"))]
                let _ = enc;
            }
        }
    }
}

#[test]
fn simd_matches_reference() {
    for color in [LossyColor::Xyb, LossyColor::YCoCg] {
        for q in [10.0, 50.0, 85.0, 97.0, 100.0, 110.0] {
            for (w, h) in [(1, 1), (13, 9), (64, 64), (200, 75)] {
                let px = test_image(w, h, w + h * 7);
                let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
                let cfg = LossyConfig::new(q).with_color(color);
                let Some(enc) = required_encode(
                    q,
                    encode_lossy(&px, w as usize * 3, &info, &Metadata::default(), &cfg),
                ) else {
                    continue;
                };
                let (a, b) = both(&enc);
                assert_eq!(a.unwrap(), b.unwrap(), "{color:?} q{q} {w}x{h}");
            }
        }
    }
}

#[test]
fn simd_matches_reference_on_corrupt_streams() {
    let (w, h) = (48, 40);
    let px = test_image(w, h, 3);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    for color in [LossyColor::Xyb, LossyColor::YCoCg] {
        let enc = encode_lossy(
            &px,
            w as usize * 3,
            &info,
            &Metadata::default(),
            &LossyConfig::new(90.0).with_color(color),
        )
        .unwrap();
        let mut s = 99u32;
        let mut compared = 0;
        let mut errs = alloc::collections::BTreeMap::new();
        for _ in 0..400 {
            let mut d = enc.clone();
            s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let at = (s >> 8) as usize % d.len();
            d[at] ^= 1 << (s % 8);
            let (a, b) = both(&d);
            if let Err(e) = &a {
                *errs.entry(alloc::format!("{e:?}")).or_insert(0) += 1;
            }
            assert_eq!(a.is_ok(), b.is_ok(), "byte {at}");
            if let (Ok(a), Ok(b)) = (a, b) {
                assert_eq!(a, b, "byte {at}");
                compared += 1;
            }
        }
        // Desynchronized lanes are mostly rejected as overruns; see
        // `kernels_match_on_random_lane_data` for the raw comparison.
        assert!(compared > 40, "{compared}: {errs:?}");
    }
}

/// Random stripe payloads: the SIMD and scalar entropy decoders must agree
/// on coefficients, EOBs and words consumed, overrun or not, for every
/// entropy mode (run-level, split-Huffman side info + unpack).
#[test]
fn kernels_match_on_random_lane_data() {
    let (w, h) = (300, 50);
    let px = test_image(w, h, 11);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
        for color in [LossyColor::Xyb, LossyColor::YCoCg] {
            let enc = encode_lossy(
                &px,
                w as usize * 3,
                &info,
                &Metadata::default(),
                &LossyConfig::new(80.0)
                    .with_color(color)
                    .with_entropy(entropy),
            )
            .unwrap();
            let mut p = parse_lossy(&enc).unwrap();
            let mut seed = 0x1234_5678u64;
            let mut bufs: Vec<Vec<u8>> = Vec::new();
            for trial in 0..60 {
                let len = 64 + 2 * (trial * 37 % 900);
                let b: Vec<u8> = (0..len)
                    .map(|_| {
                        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                        (seed >> 56) as u8
                    })
                    .collect();
                bufs.push(b);
            }
            for b in &bufs {
                for s in 0..p.stripes.len() {
                    p.stripes[s] = b;
                    // Drop the spare write-target slots before comparing.
                    let trim = |x: Result<(Vec<i32>, Vec<u8>, usize, usize), Error>| {
                        x.map(|(mut q, mut e, wp, len)| {
                            q.truncate(q.len() - 64);
                            e.truncate(e.len() - 1);
                            (q, e, wp, len)
                        })
                    };
                    let a = trim(decode_stripe_raw(
                        &p,
                        s,
                        Group::ALL,
                        true,
                        Vec::new(),
                        Vec::new(),
                    ));
                    let r = trim(decode_stripe_raw(
                        &p,
                        s,
                        Group::ALL,
                        false,
                        Vec::new(),
                        Vec::new(),
                    ));
                    match (a, r) {
                        (Ok(a), Ok(r)) => {
                            if a != r {
                                let qd = a.0.iter().zip(&r.0).position(|(x, y)| x != y);
                                let ed = a.1.iter().zip(&r.1).position(|(x, y)| x != y);
                                panic!(
                                    "{entropy:?} {color:?} stripe {s} len {}: q@{qd:?} {:?} eob@{ed:?} wp {} vs {} (len {})",
                                    b.len(),
                                    qd.map(|i| (a.0[i], r.0[i], i / 64, i % 64)),
                                    a.2,
                                    r.2,
                                    a.3
                                );
                            }
                        }
                        (a, r) => assert_eq!(a.is_ok(), r.is_ok()),
                    }
                }
            }
        }
    }
}

/// Valid stripes: the SIMD entropy kernels and the scalar reference agree
/// on coefficients, EOBs and words consumed for every entropy mode.
#[test]
fn entropy_tier_parity_on_valid_stripes() {
    let (w, h) = (200u32, 75u32);
    let px = test_image(w, h, 5);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
        for color in [LossyColor::Xyb, LossyColor::YCoCg] {
            let enc = encode_lossy(
                &px,
                w as usize * 3,
                &info,
                &Metadata::default(),
                &LossyConfig::new(85.0)
                    .with_color(color)
                    .with_entropy(entropy),
            )
            .unwrap();
            let p = parse_lossy(&enc).unwrap();
            // Drop the spare write-target slots before comparing.
            let trim = |x: Result<(Vec<i32>, Vec<u8>, usize, usize), Error>| {
                x.map(|(mut q, mut e, wp, len)| {
                    q.truncate(q.len() - 64);
                    e.truncate(e.len() - 1);
                    (q, e, wp, len)
                })
                .unwrap()
            };
            for s in 0..p.stripes.len() {
                let a = trim(decode_stripe_raw(
                    &p,
                    s,
                    Group::ALL,
                    true,
                    Vec::new(),
                    Vec::new(),
                ));
                let r = trim(decode_stripe_raw(
                    &p,
                    s,
                    Group::ALL,
                    false,
                    Vec::new(),
                    Vec::new(),
                ));
                assert_eq!(a, r, "{entropy:?} {color:?} stripe {s}");
            }
        }
    }
}

/// 16-bit test image: smooth 16-bit gradients plus texture.
fn test_image16(w: u32, h: u32, seed: u32) -> Vec<u8> {
    let p8 = test_image(w, h, seed);
    let mut out = Vec::with_capacity(p8.len() * 2);
    for (i, &v) in p8.iter().enumerate() {
        let fine = ((i as u32).wrapping_mul(2_654_435_761) >> 26) as u16; // 0..63
        let v16 = (v as u16) * 257 - (v as u16 > 0) as u16 * fine.min(v as u16 * 257);
        out.extend_from_slice(&v16.to_ne_bytes());
    }
    out
}

#[test]
fn sixteen_bit_roundtrip() {
    for (w, h) in [(1, 1), (13, 7), (100, 60)] {
        let px = test_image16(w, h, 3);
        let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U16);
        let mut last = 0.0;
        for q in [50.0, 90.0, 100.0, 108.0, 116.0] {
            let Some(enc) = required_encode(
                q,
                encode_lossy(
                    &px,
                    w as usize * 6,
                    &info,
                    &Metadata::default(),
                    &LossyConfig::new(q),
                ),
            ) else {
                continue;
            };
            let (di, out) = crate::decode(&enc).unwrap();
            assert_eq!(di.sample, SampleType::U16);
            let p = ssim2_16(&px, &out, w);
            // Monotone in q, except 1x1: one DC sample per channel, where
            // nearly equal steps (q90 vs q100 under the jpegli DC base) round
            // either way by chance.
            if w > 1 {
                assert!(p > last - 1.0, "{w}x{h} q{q}: {p} after {last}");
            }
            last = p;
            let (a, b) = both(&enc);
            assert_eq!(a.unwrap(), b.unwrap(), "{w}x{h} q{q}");
        }
        if w > 1 {
            assert!(last > 95.0, "{w}x{h}: ssim2 at q116 {last}");
        }
    }
}

/// Linear HDR test image (1.0 = SDR white, highlights up to ~40).
fn test_image_linear(w: u32, h: u32) -> Vec<f32> {
    let p8 = test_image(w, h, 17);
    p8.iter()
        .enumerate()
        .map(|(i, &v)| {
            let l = (v as f32 / 255.0).powi(2);
            let px = i / 3;
            let (x, y) = (px as u32 % w, px as u32 / w);
            if (x / 9 + y / 5) % 7 == 0 {
                l * 40.0
            } else {
                l
            }
        })
        .collect()
}

/// Mean |log2| ratio of decoded to original linear values (above a floor).
fn log_err(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len() as f64;
    a.iter()
        .zip(b)
        .map(|(&x, &y)| {
            let f = 1e-3f32;
            ((y.max(0.0) + f) / (x + f)).log2().abs() as f64
        })
        .sum::<f64>()
        / n
}

#[test]
fn float_linear_roundtrip() {
    let (w, h) = (90, 50);
    let lin = test_image_linear(w, h);
    for sample in [SampleType::F32, SampleType::F16] {
        let bytes: Vec<u8> = match sample {
            SampleType::F32 => lin.iter().flat_map(|v| v.to_ne_bytes()).collect(),
            _ => lin
                .iter()
                .flat_map(|&v| color::f32_to_f16_rne(v).to_ne_bytes())
                .collect(),
        };
        let src: Vec<f32> = match sample {
            SampleType::F32 => lin.clone(),
            _ => bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| {
                    f32::from_bits(crate::mapping::f16_to_f32_bits(u16::from_ne_bytes([
                        c[0], c[1],
                    ])))
                })
                .collect(),
        };
        let info = ImageInfo::new(w, h, Channels::Rgb, sample);
        let rb = info.row_bytes();
        let mut last = f64::MAX;
        // q116 on 40x highlights exercises internal 32-bit DCs in every build.
        let qs: &[f32] = &[60.0, 90.0, 100.0, 116.0];
        for &q in qs {
            let enc = encode_lossy(
                &bytes,
                rb,
                &info,
                &Metadata::default(),
                &LossyConfig::new(q),
            )
            .unwrap();
            let (di, out) = crate::decode(&enc).unwrap();
            assert_eq!(di.sample, sample);
            let dec: Vec<f32> = match sample {
                SampleType::F32 => out
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| f32::from_ne_bytes(*c))
                    .collect(),
                _ => out
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| {
                        f32::from_bits(crate::mapping::f16_to_f32_bits(u16::from_ne_bytes([
                            c[0], c[1],
                        ])))
                    })
                    .collect(),
            };
            let e = log_err(&src, &dec);
            assert!(e < last * 1.05 + 1e-4, "{sample:?} q{q}: {e} after {last}");
            last = e;
            let (a, b) = both(&enc);
            assert_eq!(a.unwrap(), b.unwrap(), "{sample:?} q{q}");
        }
        assert!(
            last < 0.05,
            "{sample:?}: mean log2 error at top quality {last}"
        );
    }
}

/// Bright HDR values at high quality need 32-bit DCs (I32 DC image).
#[test]
fn hdr_float_uses_i32_dc_image() {
    let (w, h) = (16, 8);
    let lin: Vec<f32> = (0..w * h * 3)
        .map(|i| 10.0 + (i % 7) as f32 * 0.01)
        .collect();
    let bytes: Vec<u8> = lin.iter().flat_map(|v| v.to_ne_bytes()).collect();
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::F32);
    let enc = encode_lossy(
        &bytes,
        w as usize * 12,
        &info,
        &Metadata::default(),
        &LossyConfig::new(116.0),
    )
    .unwrap();
    let p = parse_lossy(&enc).unwrap();
    assert_eq!(p.dc.as_ref().unwrap().info.sample, SampleType::I32);
    let (_, out) = crate::decode(&enc).unwrap();
    for (c, &v) in out.as_chunks::<4>().0.iter().zip(&lin) {
        let d = f32::from_ne_bytes(*c);
        assert!((d - v).abs() < 0.01 * v, "{d} vs {v}");
    }
    let (a, b) = both(&enc);
    assert_eq!(a.unwrap(), b.unwrap());
}

/// Row streaming produces exactly the full decode, for every pixel format.
#[test]
fn stream_matches_full_decode() {
    let (w, h) = (75, 70);
    let lin = test_image_linear(w, h);
    let cases: Vec<(SampleType, Vec<u8>, LossyColor)> = vec![
        (SampleType::U8, test_image(w, h, 4), LossyColor::Xyb),
        (SampleType::U8, test_image(w, h, 4), LossyColor::YCoCg),
        (SampleType::U16, test_image16(w, h, 4), LossyColor::YCoCg),
        (
            SampleType::F32,
            lin.iter().flat_map(|v| v.to_ne_bytes()).collect(),
            LossyColor::Xyb,
        ),
    ];
    for (sample, px, color) in cases {
        for sb in [1, 3, 8] {
            let info = ImageInfo::new(w, h, Channels::Rgb, sample);
            let rb = info.row_bytes();
            let cfg = LossyConfig::new(90.0)
                .with_color(color)
                .with_stripe_block_rows(sb);
            let enc = encode_lossy(&px, rb, &info, &Metadata::default(), &cfg).unwrap();
            let (_, full) = crate::decode(&enc).unwrap();
            let mut sd = crate::StreamDecoder::new(&enc).unwrap();
            let mut row = vec![0u8; rb];
            let mut y = 0;
            while sd.next_row(&mut row).unwrap() {
                assert_eq!(row, full[y * rb..(y + 1) * rb], "{sample:?} sb{sb} row {y}");
                y += 1;
            }
            assert_eq!(y, h as usize);
            assert_eq!(sd.rows_decoded(), h);
        }
    }
}

/// Gray, gray+alpha and RGBA for every sample type: alpha is exact, color
/// decodes like RGB, SIMD = scalar, streaming = full decode.
#[test]
fn gray_and_alpha_layouts() {
    let (w, h) = (37, 29);
    let rgb8 = test_image(w, h, 21);
    for sample in [
        SampleType::U8,
        SampleType::U16,
        SampleType::F16,
        SampleType::F32,
    ] {
        let bps = sample.bytes();
        // One sample from an 8-bit value (alpha: mostly 0/max with a ramp).
        let enc_s = |v: u8| -> Vec<u8> {
            match sample {
                SampleType::U8 => vec![v],
                SampleType::U16 => (v as u16 * 257).to_ne_bytes().to_vec(),
                SampleType::F16 => color::f32_to_f16_rne(v as f32 / 255.0)
                    .to_ne_bytes()
                    .to_vec(),
                _ => (v as f32 / 255.0).to_ne_bytes().to_vec(),
            }
        };
        for ch in [Channels::Gray, Channels::GrayAlpha, Channels::Rgba] {
            let nco = ch.count();
            let mut px = Vec::with_capacity((w * h) as usize * nco * bps);
            for i in 0..(w * h) as usize {
                let p = &rgb8[i * 3..i * 3 + 3];
                let a = if i % 7 == 0 {
                    (i % 251) as u8
                } else if i % 3 == 0 {
                    0
                } else {
                    255
                };
                match ch {
                    Channels::Gray => px.extend(enc_s(p[1])),
                    Channels::GrayAlpha => {
                        px.extend(enc_s(p[1]));
                        px.extend(enc_s(a));
                    }
                    _ => {
                        for &c in p {
                            px.extend(enc_s(c));
                        }
                        px.extend(enc_s(a));
                    }
                }
            }
            let info = ImageInfo::new(w, h, ch, sample);
            let rb = info.row_bytes();
            let enc = encode_lossy(
                &px,
                rb,
                &info,
                &Metadata::default(),
                &LossyConfig::new(95.0),
            )
            .unwrap();
            let (di, out) = crate::decode(&enc).unwrap();
            assert_eq!((di.channels, di.sample), (ch, sample));
            // Alpha is lossless.
            if ch.has_alpha() {
                for i in 0..(w * h) as usize {
                    let o = (i * nco + nco - 1) * bps;
                    assert_eq!(out[o..o + bps], px[o..o + bps], "{ch:?} {sample:?} px {i}");
                }
            }
            // Gray stays close to the source (integer samples).
            if ch == Channels::Gray && sample == SampleType::U8 {
                let p = ssim2(&px, &out, w, 1);
                assert!(p > 90.0, "gray: ssim2 {p}");
            }
            let (a, b) = both(&enc);
            assert_eq!(a.unwrap(), b.unwrap(), "{ch:?} {sample:?}");
            let mut sd = crate::StreamDecoder::new(&enc).unwrap();
            let mut row = vec![0u8; rb];
            let mut y = 0;
            while sd.next_row(&mut row).unwrap() {
                assert_eq!(row, out[y * rb..(y + 1) * rb], "{ch:?} {sample:?} row {y}");
                y += 1;
            }
            assert_eq!(y, h as usize);
        }
    }
}

/// 16-bit XYB through each supported transfer, YCoCg on request, and the
/// YCoCg fallback for an unsupported transfer.
#[test]
fn sixteen_bit_transfers() {
    let (w, h) = (48, 40);
    let px = test_image16(w, h, 8);
    for (tc, color, want) in [
        (13u8, LossyColor::Xyb, LossyColor::Xyb),
        (16, LossyColor::Xyb, LossyColor::Xyb),
        (18, LossyColor::Xyb, LossyColor::Xyb),
        (16, LossyColor::YCoCg, LossyColor::YCoCg),
        (1, LossyColor::Xyb, LossyColor::YCoCg),
    ] {
        let mut info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U16);
        info.cicp.transfer_characteristics = tc;
        let enc = encode_lossy(
            &px,
            w as usize * 6,
            &info,
            &Metadata::default(),
            &LossyConfig::new(100.0).with_color(color),
        )
        .unwrap();
        let p = parse_lossy(&enc).unwrap();
        assert_eq!(p.color, want, "tc {tc}");
        let (_, out) = crate::decode(&enc).unwrap();
        let ps = ssim2_16(&px, &out, w);
        // Sanity bound only (catches a broken decode): the code values are
        // scored as if sRGB, which they are not for PQ/HLG; quality is judged
        // in imij-bench.
        assert!(ps > 75.0, "tc {tc} {color:?}: ssim2 {ps}");
        let (a, b) = both(&enc);
        assert_eq!(a.unwrap(), b.unwrap(), "tc {tc}");
    }
}

/// Adaptive quantization: per-block multipliers in the DC image's fourth
/// channel; SIMD = scalar, streaming = full decode, quality stays sane.
#[test]
fn adaptive_quantization() {
    let (w, h) = (96, 72);
    let px = test_image(w, h, 31);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let mut cfg = LossyConfig::new(90.0).with_stripe_block_rows(3);
    cfg.aq_gamma = 0.4;
    cfg.aq_r0 = 2.0;
    let enc = encode_lossy(&px, w as usize * 3, &info, &Metadata::default(), &cfg).unwrap();
    let p = parse_lossy(&enc).unwrap();
    assert!(p.aq);
    let (_, out) = crate::decode(&enc).unwrap();
    let p = ssim2(&px, &out, w, 3);
    assert!(p > 70.0, "aq: ssim2 {p}");
    let (a, b) = both(&enc);
    assert_eq!(a.unwrap(), b.unwrap());
    let mut sd = crate::StreamDecoder::new(&enc).unwrap();
    let mut row = vec![0u8; w as usize * 3];
    let mut y = 0;
    while sd.next_row(&mut row).unwrap() {
        assert_eq!(row, out[y * row.len()..(y + 1) * row.len()]);
        y += 1;
    }
}

/// Split-Huffman codes the same quantized coefficients as
/// run-level, so it must decode to identical pixels (also through the
/// SIMD dispatch path).
#[test]
fn entropy_modes_match_run_level() {
    let cases: Vec<(u32, u32, Channels, SampleType, Vec<u8>)> = vec![
        (1, 1, Channels::Rgb, SampleType::U8, test_image(1, 1, 3)),
        (13, 9, Channels::Rgb, SampleType::U8, test_image(13, 9, 5)),
        (
            200,
            75,
            Channels::Rgb,
            SampleType::U8,
            test_image(200, 75, 9),
        ),
        (
            64,
            40,
            Channels::Rgb,
            SampleType::U16,
            test_image16(64, 40, 4),
        ),
    ];
    for (w, h, ch, st, px) in &cases {
        let info = ImageInfo::new(*w, *h, *ch, *st);
        let stride = *w as usize * 3 * st.bytes();
        for color in [LossyColor::Xyb, LossyColor::YCoCg] {
            for q in [10.0, 50.0, 85.0, 97.0, 100.0, 108.0, 116.0] {
                let cfg = LossyConfig::new(q).with_color(color);
                let enc = |e: LossyEntropy| {
                    required_encode(
                        q,
                        encode_lossy(
                            px,
                            stride,
                            &info,
                            &Metadata::default(),
                            &cfg.clone().with_entropy(e),
                        ),
                    )
                };
                // The DC image is shared by every entropy mode: all need
                // `extended` or none does.
                let Some(rl) = enc(LossyEntropy::RunLevel) else {
                    continue;
                };
                let a = crate::decode(&rl).unwrap().1;
                let mode = LossyEntropy::SplitHuf;
                let pk = enc(mode).unwrap();
                let b = crate::decode(&pk).unwrap().1;
                assert_eq!(a, b, "{w}x{h} {st:?} {color:?} q{q} {mode:?}");
                let (s, r) = both(&pk);
                assert_eq!(
                    s.unwrap(),
                    r.unwrap(),
                    "{w}x{h} {st:?} {color:?} q{q} {mode:?} simd"
                );
                let mut d4 = vec![0u8; a.len()];
                decode_test_pool(&pk, &mut d4, stride, 4);
                assert_eq!(a, d4, "{mode:?} threads");
                let mut sd = crate::StreamDecoder::new(&pk).unwrap();
                let rb = info.row_bytes();
                let mut row = vec![0u8; rb];
                let mut y = 0;
                while sd.next_row(&mut row).unwrap() {
                    assert_eq!(row, a[y * rb..(y + 1) * rb], "{mode:?} stream row {y}");
                    y += 1;
                }
            }
        }
    }
}

#[test]
fn entropy_corrupt_streams_do_not_panic() {
    corrupt_streams_case(LossyEntropy::SplitHuf);
}

fn corrupt_streams_case(mode: LossyEntropy) {
    let (w, h) = (40u32, 24u32);
    let px = test_image(w, h, 7);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let enc = encode_lossy(
        &px,
        w as usize * 3,
        &info,
        &Metadata::default(),
        &LossyConfig::new(90.0).with_entropy(mode),
    )
    .unwrap();
    let mut seed = 1u32;
    for i in 0..400 {
        let mut d = enc.clone();
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
        let pos = 20 + (seed as usize >> 8) % (d.len() - 20);
        d[pos] ^= 1 << (i % 8);
        let (a, b) = both(&d);
        if let (Ok(a), Ok(b)) = (a, b) {
            assert_eq!(a, b);
        }
        let _ = crate::decode(&d);
    }
}

/// Both AVX-512 entropy kernels (hardware gather/scatter and software
/// lookups) match the scalar reference. Runs the AVX-512 paths only on CPUs
/// (or emulators) that have AVX-512.
#[cfg(target_arch = "x86_64")]
#[test]
fn avx512_kernel_variants_match_reference() {
    for (w, h, q) in [(200u32, 75u32, 90.0f32), (64, 64, 100.0), (37, 41, 50.0)] {
        let px = test_image(w, h, w * 3 + h);
        let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
        let enc = encode_lossy(
            &px,
            w as usize * 3,
            &info,
            &Metadata::default(),
            &LossyConfig::new(q),
        )
        .unwrap();
        for force in [Some(true), Some(false)] {
            super::force_gather(force);
            let (s, r) = both(&enc);
            assert_eq!(s.unwrap(), r.unwrap(), "{w}x{h} q{q} gather={force:?}");
        }
        super::force_gather(None);
    }
}

/// Merged blocks reconstruct the same for every chunk (panel) width, SIMD
/// and scalar: panels stay at least two blocks wide and even, so a 16-wide
/// merge never spans two.
#[cfg(feature = "lab")]
#[test]
fn superblock_recon_chunks_agree() {
    let _override_guard = super::scoped_recon_chunk_override();
    let (w, h) = (203u32, 37u32);
    let px = test_image(w, h, 9);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    for q in [10.0f32, 60.0] {
        let mut cfg = LossyConfig::new(q).with_stripe_block_rows(3);
        cfg.lab.superblocks = Some(true);
        let enc = encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), &cfg).unwrap();
        assert!(
            crate::lab::lossy_superblock_stats(&enc).unwrap()[1..4]
                .iter()
                .sum::<u64>()
                > 0
        );
        super::set_recon_chunk(usize::MAX);
        let (whole, _) = both(&enc);
        let whole = whole.unwrap();
        for n in [1, 2, 3, 5, 32] {
            super::set_recon_chunk(n);
            let (a, b) = both(&enc);
            assert_eq!(a.unwrap(), whole, "q{q} chunk {n} simd");
            assert_eq!(b.unwrap(), whole, "q{q} chunk {n} scalar");
        }
        super::set_recon_chunk(0);
    }
}

/// The normative sRGB table (9-bit mantissa buckets, rounded at the bucket
/// midpoint) stays within 0.58 codes of the exact transfer over a dense
/// sweep of f32 values in [0, 1] (measured maximum over every f32: 0.576).
#[cfg(feature = "std")]
#[test]
fn srgb8_transfer_accuracy() {
    let one = 1.0f32.to_bits();
    let mut b = 0u32;
    let mut worst = 0.0f64;
    while b <= one {
        let l = f32::from_bits(b) as f64;
        let ex = 255.0
            * if l <= 0.0031308 {
                12.92 * l
            } else {
                1.055 * l.powf(1.0 / 2.4) - 0.055
            };
        let v = color::lin_to_srgb8(f32::from_bits(b)) as f64;
        worst = worst.max((v - ex).abs());
        b += 997;
    }
    assert!(worst < 0.58, "max error {worst} codes");
    assert_eq!(color::lin_to_srgb8(1.0), 255);
    assert_eq!(color::lin_to_srgb8(0.0), 0);
    assert_eq!(color::lin_to_srgb8(-1.0), 0);
    assert_eq!(color::lin_to_srgb8(2.0), 255);
}

/// SIMD XYB rows (every tier, odd widths for the tails) equal the scalar
/// normative inverse pixel by pixel on random plane values spanning and
/// exceeding the output range.
#[test]
fn xyb_rows_match_scalar() {
    let inv = [1.0 / 300.0, 1.0 / 2400.0, 1.0 / 300.0];
    let mut s = 0x1234_5678u32;
    let mut rnd = |r: f32| {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        (s as f32 / u32::MAX as f32 * 2.0 - 1.0) * r
    };
    for w in [1usize, 7, 15, 16, 17, 31, 33, 255, 4099] {
        for _ in 0..(1 << 20) / w.max(64) {
            let p0: Vec<f32> = (0..w + 64).map(|_| rnd(400.0) + 150.0).collect();
            let p1: Vec<f32> = (0..w + 64).map(|_| rnd(90.0)).collect();
            let p2: Vec<f32> = (0..w + 64).map(|_| rnd(400.0)).collect();
            let mut out = vec![0u8; w * 3];
            simd::color_row(LossyColor::Xyb, &p0, &p1, &p2, inv, &mut out, w);
            for x in 0..w {
                let e = color::inverse_px(LossyColor::Xyb, inv, [p0[x], p1[x], p2[x]]);
                assert_eq!(out[x * 3..x * 3 + 3], e, "w {w} x {x}");
            }
        }
    }
}

/// Two-stripe work items (the interleaved AVX-512 entropy kernel where it
/// runs, else sequential) decode to the same pixels as single stripes, for
/// odd and even stripe counts, 1 and 4 threads, both entropy modes.
#[test]
fn stripe_pairs_agree() {
    for gather in [false, true] {
        #[cfg(target_arch = "x86_64")]
        super::force_gather(Some(gather));
        #[cfg(not(target_arch = "x86_64"))]
        let _ = gather;
        stripe_pairs_case();
    }
    #[cfg(target_arch = "x86_64")]
    super::force_gather(None);
}

fn stripe_pairs_case() {
    for (w, h, sb) in [
        (203u32, 121u32, 2u32),
        (64, 70, 1),
        (40, 300, 8),
        (17, 9, 1),
    ] {
        for (ch, sample) in [
            (Channels::Rgb, SampleType::U8),
            (Channels::Rgba, SampleType::U8),
            (Channels::Rgb, SampleType::U16),
        ] {
            let info = ImageInfo::new(w, h, ch, sample);
            let px: Vec<u8> = match (ch, sample) {
                (Channels::Rgb, SampleType::U8) => test_image(w, h, 3),
                (Channels::Rgba, _) => test_image(w, h, 4)
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .enumerate()
                    .flat_map(|(i, c)| [c[0], c[1], c[2], (i * 7) as u8])
                    .collect(),
                _ => test_image16(w, h, 5),
            };
            for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                let cfg = LossyConfig::new(88.0)
                    .with_stripe_block_rows(sb)
                    .with_entropy(entropy);
                let enc =
                    encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), &cfg).unwrap();
                let rb = info.row_bytes();
                let dec = |pairs: u8, threads: usize| {
                    super::set_pairs(pairs);
                    let mut out = alloc::vec![0u8; rb * h as usize];
                    decode_test_pool(&enc, &mut out, rb, threads);
                    out
                };
                let base = dec(2, 1);
                for (pairs, threads) in [(1, 1), (1, 4), (0, 1), (0, 4), (2, 4)] {
                    assert_eq!(
                        dec(pairs, threads),
                        base,
                        "{w}x{h} sb{sb} {ch:?} {sample:?} {entropy:?} pairs {pairs} threads {threads}"
                    );
                }
            }
        }
    }
    super::set_pairs(0);
}

/// Splitting a DC stripe's lossy stripes over several work items (short
/// wide images on many threads) gives the same pixels.
#[cfg(feature = "std")]
#[test]
fn dc_group_split_agrees() {
    for (w, h, sb) in [
        (1100u32, 64u32, 1u32),
        (900, 40, 1),
        (517, 70, 3),
        (130, 9, 1),
    ] {
        let px = test_image(w, h, w ^ h);
        let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
        let cfg = LossyConfig::new(80.0).with_stripe_block_rows(sb);
        let enc = encode_lossy(&px, w as usize * 3, &info, &Metadata::default(), &cfg).unwrap();
        let (_, want) = crate::decode(&enc).unwrap();
        for split in [0usize, 1, 2, 3, 4] {
            super::set_dc_split(split);
            for threads in [2, 5, 12] {
                let mut got = vec![0u8; want.len()];
                crate::decode_into_threads(&enc, &mut got, w as usize * 3, threads).unwrap();
                assert!(
                    got == want,
                    "{w}x{h} sb {sb} split {split} threads {threads}"
                );
            }
        }
        super::set_dc_split(0);
    }
}

/// Column groups change only how coefficients are entropy coded: grouped
/// and ungrouped files decode to the same pixels on every decode path, for
/// every layout, sample type, entropy mode and stripe height.
#[cfg(feature = "std")]
#[test]
fn column_groups_same_pixels() {
    for (w, h) in [(203u32, 37u32), (130, 9), (400, 70)] {
        let rgb = test_image(w, h, w ^ (h << 3));
        let rgba: Vec<u8> = rgb
            .as_chunks::<3>()
            .0
            .iter()
            .enumerate()
            .flat_map(|(i, c)| [c[0], c[1], c[2], (i * 13 % 256) as u8])
            .collect();
        let gray: Vec<u8> = rgb.as_chunks::<3>().0.iter().map(|c| c[1]).collect();
        let p16 = test_image16(w, h, 5);
        let cases: [(Channels, SampleType, &[u8]); 4] = [
            (Channels::Rgb, SampleType::U8, &rgb),
            (Channels::Rgba, SampleType::U8, &rgba),
            (Channels::Gray, SampleType::U8, &gray),
            (Channels::Rgb, SampleType::U16, &p16),
        ];
        for (ch, st, data) in cases {
            let info = ImageInfo::new(w, h, ch, st);
            for (entropy, sb, aq) in [
                (LossyEntropy::RunLevel, 1, false),
                (LossyEntropy::RunLevel, 3, true),
                (LossyEntropy::SplitHuf, 1, true),
                (LossyEntropy::SplitHuf, 2, false),
            ] {
                let mut cfg = LossyConfig::new(80.0)
                    .with_entropy(entropy)
                    .with_stripe_block_rows(sb);
                if aq {
                    cfg.aq_gamma = 0.5;
                }
                let what = format!("{w}x{h} {ch:?} {st:?} {entropy:?} sb {sb} aq {aq}");
                let mut c0 = cfg.clone();
                c0.column_groups = Some(0);
                let Ok(plain) =
                    encode_lossy(data, info.row_bytes(), &info, &Metadata::default(), &c0)
                else {
                    continue;
                };
                let (_, want) = crate::decode(&plain).unwrap();
                for gcols in [8u32, 64, 96, 200] {
                    let mut cg = cfg.clone();
                    cg.column_groups = Some(gcols);
                    let enc =
                        encode_lossy(data, info.row_bytes(), &info, &Metadata::default(), &cg)
                            .unwrap();
                    let what = format!("{what} groups {gcols}");
                    assert!(enc != plain || gcols >= w, "{what}: groups not used");
                    for (simd, threads) in [(true, 1), (false, 1), (true, 5), (false, 3)] {
                        let got = decode_ways(&enc, simd, threads).unwrap();
                        assert!(got == want, "{what}: simd {simd} threads {threads}");
                    }
                    assert!(stream_decode(&enc).unwrap() == want, "{what}: stream");
                    let mut got = Vec::new();
                    crate::decode_reader(&enc[..], &crate::Limits::DEFAULT, |_, _, r| {
                        got.extend_from_slice(r);
                        Ok(())
                    })
                    .unwrap();
                    assert!(got == want, "{what}: reader");
                    let mut s = 0x2545_f491u32 ^ gcols;
                    for _ in 0..30 {
                        let mut bad = enc.clone();
                        s ^= s << 13;
                        s ^= s >> 17;
                        s ^= s << 5;
                        let i = s as usize % bad.len();
                        bad[i] ^= (s >> 24) as u8 | 1;
                        let _ = decode_ways(&bad, true, 3);
                        let _ = stream_decode(&bad);
                    }
                }
            }
        }
    }
}

/// Full decode (threads) through `decode_lossy_impl`.
#[cfg(feature = "std")]
fn decode_ways(enc: &[u8], simd: bool, threads: usize) -> Result<Vec<u8>, Error> {
    let (info, color, body, lossy) =
        crate::v1::lossy_body(enc)?.ok_or(Error::Corrupt("not a lossy file"))?;
    let mut out = vec![0u8; info.checked_buffer_len().ok_or(Error::TooLarge)?];
    let rb = info.row_bytes();
    decode_lossy_impl(
        info,
        crate::parse::header::HEADER_VERSION,
        color,
        lossy,
        body,
        &mut out,
        rb,
        threads,
        &crate::Limits::DEFAULT,
        simd,
        false,
    )?;
    Ok(out)
}

#[cfg(feature = "std")]
fn stream_decode(enc: &[u8]) -> Result<Vec<u8>, Error> {
    let (info, color, body, lossy) =
        crate::v1::lossy_body(enc)?.ok_or(Error::Corrupt("not a lossy file"))?;
    let mut sd = LossyStream::new(
        info.clone(),
        crate::parse::header::HEADER_VERSION,
        color,
        lossy,
        body,
    )?;
    let rb = info.row_bytes();
    let mut out = vec![0u8; rb * info.height as usize];
    for y in 0..info.height as usize {
        sd.next_row(&mut out[y * rb..(y + 1) * rb])?;
    }
    Ok(out)
}

/// Images wider than 8192 get column groups by default (same pixels).
#[cfg(feature = "std")]
#[test]
fn column_groups_automatic() {
    let (w, h) = (8200u32, 16u32);
    let px = test_image(w, h, 3);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let auto = encode_lossy(
        &px,
        w as usize * 3,
        &info,
        &Metadata::default(),
        &LossyConfig::new(70.0),
    )
    .unwrap();
    let mut c0 = LossyConfig::new(70.0);
    c0.column_groups = Some(0);
    let none = encode_lossy(&px, w as usize * 3, &info, &Metadata::default(), &c0).unwrap();
    assert!(auto != none);
    assert_eq!(
        crate::decode(&auto).unwrap().1,
        crate::decode(&none).unwrap().1
    );
}

/// Inter frames: blocks coded "keep" leave the reference pixels untouched,
/// every decode path agrees, and the file is not decodable without its
/// reference.
#[cfg(feature = "std")]
#[test]
fn keep_reference_blocks() {
    let (w, h) = (203u32, 45u32);
    let reference = test_image(w, h, 7);
    // The current frame: the reference with a changed band.
    let mut cur = reference.clone();
    for y in 10..25 {
        for x in 60..140 {
            for c in 0..3 {
                cur[((y * w + x) * 3 + c) as usize] ^= 0x5A;
            }
        }
    }
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let rb = info.row_bytes();
    for cfg in [
        LossyConfig::new(85.0),
        LossyConfig::new(60.0).with_stripe_block_rows(2),
        {
            let mut c = LossyConfig::new(90.0);
            c.aq_gamma = 0.5;
            c.column_groups = Some(64);
            c
        },
    ] {
        // The reference as a decoder would hold it: an intra frame's decode.
        let rf = encode_lossy(&reference, rb, &info, &Metadata::default(), &cfg).unwrap();
        let (_, ref_rec) = crate::decode(&rf).unwrap();
        let enc = encode_lossy_frame(
            &cur,
            rb,
            &info,
            &Metadata::default(),
            &cfg,
            Some((&reference, rb)),
        )
        .unwrap();
        assert!(
            crate::decode(&enc).is_err(),
            "decodes without its reference"
        );
        let (body_info, color, body, lossy) = crate::v1::lossy_body(&enc).unwrap().unwrap();
        let mut want: Option<Vec<u8>> = None;
        for (simd, threads) in [(true, 1), (false, 1), (true, 3), (false, 4)] {
            let mut out = ref_rec.clone();
            decode_lossy_impl(
                body_info.clone(),
                crate::parse::header::HEADER_VERSION,
                color,
                lossy,
                body,
                &mut out,
                rb,
                threads,
                &crate::Limits::DEFAULT,
                simd,
                true,
            )
            .unwrap();
            match &want {
                None => want = Some(out),
                Some(wv) => assert!(&out == wv, "paths differ: simd {simd} threads {threads}"),
            }
        }
        let out = want.unwrap();
        // Rows outside the changed band are kept blocks: exactly the reference.
        let kept = (0..h as usize)
            .filter(|&y| !(8..32).contains(&y))
            .all(|y| out[y * rb..(y + 1) * rb] == ref_rec[y * rb..(y + 1) * rb]);
        assert!(kept, "unchanged rows were not kept");
        let maxerr = |px: &[u8]| {
            px.iter()
                .zip(&cur)
                .map(|(a, b)| (*a as i32 - *b as i32).unsigned_abs())
                .max()
                .unwrap()
        };
        let intra = encode_lossy(&cur, rb, &info, &Metadata::default(), &cfg).unwrap();
        let (e_inter, e_intra) = (maxerr(&out), maxerr(&crate::decode(&intra).unwrap().1));
        assert!(
            e_inter <= e_intra + 8,
            "inter max error {e_inter} vs intra {e_intra}"
        );
    }
}

#[test]
fn subblocks_scalar_simd_and_hostile_records_agree() {
    let (w, h) = (73, 25);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let px = test_image(w, h, 57);
    for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
        let cfg = LossyConfig::new(43.0)
            .with_entropy(entropy)
            .with_compact_streams(true)
            .with_channel_layouts([
                LossyChannelLayout::new(LossyTransform::Adaptive4, ChannelSampling::Full),
                LossyChannelLayout::new(LossyTransform::Dct2, ChannelSampling::HalfHorizontal),
                LossyChannelLayout::new(LossyTransform::Dct4, ChannelSampling::HalfBoth),
            ]);
        let file = encode_lossy(&px, w as usize * 3, &info, &Metadata::default(), &cfg).unwrap();
        let (_, color, body, lossy) = crate::v1::lossy_body(&file).unwrap().unwrap();
        for i in 0..256 {
            let mut data = body.to_vec();
            if i != 0 {
                let at = (i * 797) % data.len();
                data[at] ^= 1 << (i % 8);
            }
            let mut reference = vec![0; px.len()];
            let mut optimized = vec![0; px.len()];
            let a = decode_lossy_impl(
                info.clone(),
                crate::parse::header::HEADER_VERSION,
                color,
                lossy,
                &data,
                &mut reference,
                w as usize * 3,
                1,
                &crate::Limits::DEFAULT,
                false,
                false,
            );
            let b = decode_lossy_impl(
                info.clone(),
                crate::parse::header::HEADER_VERSION,
                color,
                lossy,
                &data,
                &mut optimized,
                w as usize * 3,
                4,
                &crate::Limits::DEFAULT,
                true,
                false,
            );
            assert_eq!(a.is_ok(), b.is_ok(), "{entropy:?} mutation {i}");
            if a.is_ok() {
                assert_eq!(reference, optimized, "{entropy:?} mutation {i}");
            }
        }
    }
}

#[test]
fn automatic_stripes_cap_stripe_area_per_decode_tier() {
    use crate::DecodeSpeed as D;
    let rows = |speed: Option<D>, w: u32| {
        let mut cfg = LossyConfig::new(50.0);
        cfg.decode_speed = speed;
        cfg.stripe_rows_for(w) * 8
    };
    // Balanced (also the default): 64K pixels, 8..=64 rows.
    for (w, px) in [
        (64, 64),
        (256, 64),
        (1024, 64),
        (1920, 32),
        (2048, 32),
        (4096, 16),
        (8192, 8),
    ] {
        assert_eq!(rows(None, w), px, "default {w}");
        assert_eq!(rows(Some(D::Balanced), w), px, "balanced {w}");
    }
    // Compact: 128K pixels; Fast/Fastest: 32K pixels and at most 32 rows.
    for (w, c, f) in [
        (512, 64, 32),
        (1024, 64, 32),
        (2048, 64, 16),
        (4096, 32, 8),
        (8192, 16, 8),
    ] {
        assert_eq!(rows(Some(D::Compact), w), c, "compact {w}");
        assert_eq!(rows(Some(D::Fast), w), f, "fast {w}");
        assert_eq!(rows(Some(D::Fastest), w), f, "fastest {w}");
    }
    // Wider than the automatic column group: the group width counts.
    assert_eq!(rows(None, 20_000), 8);
    // Explicit heights win.
    assert_eq!(
        LossyConfig::new(50.0)
            .with_stripe_block_rows(3)
            .stripe_rows_for(1024),
        3
    );
    // The encoded file carries the automatic height.
    let info = ImageInfo::new(300, 40, Channels::Rgb, SampleType::U8);
    let px: Vec<u8> = (0..300 * 40 * 3).map(|i| (i * 7 % 251) as u8).collect();
    let file = crate::encode_lossy(
        &px,
        info.row_bytes(),
        &info,
        &crate::Metadata::default(),
        &LossyConfig::new(50.0),
    )
    .unwrap();
    let (_, _, body) = crate::v1::parse_header(&file).unwrap();
    // SPEC 2.8: the lossy head starts with the stripe height.
    assert_eq!(body[0], 8, "300 wide: 64-row stripes (8 block rows)");
    assert_eq!(crate::decode(&file).unwrap().1.len(), px.len());
}

#[test]
fn dc_stripes_keep_a_minimum_count_per_tier() {
    use crate::DecodeSpeed as D;
    // 4096 wide: above Compact's one-DC-stripe size.
    let px = |speed: Option<D>, sb: u32, h: u32| {
        let mut cfg = LossyConfig::new(50.0);
        cfg.decode_speed = speed;
        dc_stripe_rows(&cfg, sb, 4096, h) * 8
    };
    // Balanced/default: >= 32 DC stripes; floor 64 rows.
    for (h, rows) in [
        (64, 64),
        (256, 64),
        (1024, 64),
        (2048, 64),
        (4096, 128),
        (8192, 256),
    ] {
        assert_eq!(px(None, 8, h), rows, "default {h}");
        assert_eq!(px(Some(D::Balanced), 2, h), rows, "balanced {h}");
    }
    // Compact: >= 8 DC stripes; >= 16 with a decoder memory budget.
    let budget = |h: u32| {
        let cfg = LossyConfig::new(50.0)
            .with_decode_speed(D::Compact)
            .with_decoder_memory_limit(crate::DecoderMemoryLimit::new(u64::MAX));
        dc_stripe_rows(&cfg, 8, 4096, h) * 8
    };
    for (h, rows, budgeted) in [
        (512, 64, 64),
        (1024, 128, 64),
        (2048, 256, 128),
        (4096, 512, 256),
        (8192, 1024, 512),
    ] {
        assert_eq!(px(Some(D::Compact), 8, h), rows, "compact {h}");
        assert_eq!(budget(h), budgeted, "compact budgeted {h}");
    }
    // Fast/Fastest: unchanged 8 block rows; whole lossy stripes always.
    assert_eq!(px(Some(D::Fast), 4, 8192), 64);
    assert_eq!(px(Some(D::Fastest), 1, 8192), 64);
    assert_eq!(px(None, 3, 8192) % 24, 0);
    // Compact: one DC stripe up to 65,536 pixels, without a memory budget.
    let one = |w: u32, h: u32, sb: u32, budget: bool| {
        let mut cfg = LossyConfig::new(50.0).with_decode_speed(D::Compact);
        if budget {
            cfg = cfg.with_decoder_memory_limit(crate::DecoderMemoryLimit::new(u64::MAX));
        }
        dc_stripe_rows(&cfg, sb, w, h) * 8
    };
    assert_eq!(one(256, 256, 8, false), 256);
    assert_eq!(one(64, 1024, 8, false), 1024);
    assert_eq!(one(128, 512, 1, false), 512);
    assert_eq!(one(1024, 64, 8, false), 64);
    assert_eq!(one(200, 100, 8, false), 128, "whole lossy stripes");
    assert_eq!(one(257, 256, 8, false), 64, "above 65,536 pixels");
    assert_eq!(one(256, 256, 8, true), 64, "memory budget");
    let mut balanced = LossyConfig::new(50.0);
    balanced.decode_speed = Some(D::Balanced);
    assert_eq!(dc_stripe_rows(&balanced, 8, 256, 256) * 8, 64);
    // An encoded small Compact image has one DC stripe and decodes the same
    // on one and four threads (one shared DC unit).
    let info = ImageInfo::new(160, 400, Channels::Rgb, SampleType::U8);
    let px = test_image(160, 400, 3);
    let cfg = LossyConfig::new(30.0).with_decode_speed(D::Compact);
    let file = encode_lossy(&px, 480, &info, &Metadata::default(), &cfg).unwrap();
    let p = parse_lossy(&file).unwrap();
    assert!(p.dc.as_ref().unwrap().hs >= p.g.bh, "one DC stripe");
    assert!(p.g.stripes() > 1);
    let (_, want) = crate::decode(&file).unwrap();
    let mut got = vec![0u8; want.len()];
    decode_test_pool(&file, &mut got, 480, 4);
    assert!(got == want);
    // An encoded tall image decodes with its taller DC stripes.
    let info = ImageInfo::new(16, 4100, Channels::Rgb, SampleType::U8);
    let pixels: Vec<u8> = (0..16 * 4100 * 3).map(|i| (i * 13 % 247) as u8).collect();
    let file = crate::encode_lossy(
        &pixels,
        info.row_bytes(),
        &info,
        &crate::Metadata::default(),
        &LossyConfig::new(40.0),
    )
    .unwrap();
    assert_eq!(crate::decode(&file).unwrap().1.len(), pixels.len());
}

/// Serializes the tests that switch the process-global DC scheduling mode.
#[cfg(feature = "std")]
static DC_MODE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Shared DC units (each DC stripe decoded once, its rows handed to the
/// items of its lossy stripes) give the same pixels as per-item DC decodes
/// and the serial decode: tall DC stripes, DC stripes declaring rows past
/// the image, column groups, alpha, gray and 16-bit samples.
#[cfg(feature = "std")]
#[test]
fn shared_dc_same_pixels() {
    let _lock = DC_MODE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    use crate::DecodeSpeed::{Balanced, Compact, Fast};
    use Channels::{Gray, Rgb, Rgba};
    use SampleType::{U8, U16};
    let cases = [
        (96u32, 1100u32, Rgb, U8, 1u32, Compact, None),
        (80, 777, Rgb, U8, 2, Compact, None),
        (300, 40, Rgb, U8, 1, Fast, None),
        (200, 333, Rgba, U8, 1, Compact, Some(64u32)),
        (130, 520, Gray, U16, 1, Balanced, Some(48)),
    ];
    for (w, h, ch, sample, sb, speed, groups) in cases {
        let info = ImageInfo::new(w, h, ch, sample);
        let rb = info.row_bytes();
        let px: Vec<u8> = (0..rb * h as usize)
            .map(|i| ((i / rb) * 7 + (i % rb) * 3 + (i * 131 % 23)) as u8)
            .collect();
        let mut cfg = LossyConfig::new(60.0)
            .with_stripe_block_rows(sb)
            .with_decode_speed(speed);
        cfg.column_groups = groups;
        let enc = encode_lossy(&px, rb, &info, &Metadata::default(), &cfg).unwrap();
        let p = parse_lossy(&enc).unwrap();
        assert!(
            p.dc.as_ref().unwrap().hs > p.g.sb,
            "{w}x{h}: DC stripes span several"
        );
        // One worker decodes each lossy stripe's DC rows when it needs them;
        // the streaming decoder and the bounded session agree.
        let (_, want) = crate::decode(&enc).unwrap();
        let mut sd = crate::StreamDecoder::new(&enc).unwrap();
        let mut row = vec![0u8; rb];
        for y in 0..h as usize {
            assert!(sd.next_row(&mut row).unwrap());
            assert!(row == want[y * rb..][..rb], "{w}x{h} {ch:?} stream row {y}");
        }
        let mut got = vec![0u8; want.len()];
        crate::decode_resources::DecodeSession::new(Default::default())
            .decode_into(&enc, &mut got, rb)
            .unwrap();
        assert!(got == want, "{w}x{h} {ch:?} session");
        // Shared DC is worker scheduling: no thread host, nothing to share.
        #[cfg(not(target_family = "wasm"))]
        for shared in [2, 1, 0] {
            super::set_shared_dc(shared);
            for threads in [2, 3, 8, 24] {
                let used = super::SHARED_DC_USED.with(|n| n.get());
                let mut got = vec![0u8; want.len()];
                crate::decode_into_threads(&enc, &mut got, rb, threads).unwrap();
                assert!(
                    got == want,
                    "{w}x{h} {ch:?} shared {shared} threads {threads}"
                );
                // Every case has too few DC stripes for 24 threads.
                if threads == 24 {
                    let now = super::SHARED_DC_USED.with(|n| n.get());
                    assert_eq!(now > used, shared != 0, "{w}x{h} {ch:?} shared {shared}");
                }
            }
        }
        super::set_shared_dc(super::SHARED_DC);
    }
}

/// A failed shared DC decode (here: cancellation part way down a DC stripe)
/// returns its error from every threaded decode instead of leaving the
/// items that wait for its rows blocked.
#[cfg(all(feature = "std", not(target_family = "wasm")))]
#[test]
fn shared_dc_error_returns() {
    let _lock = DC_MODE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    // 64 lossy stripes, 8 DC stripes: too few for 8 or 24 threads.
    let (w, h) = (64u32, 512u32);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let px = test_image(w, h, 7);
    let cfg = LossyConfig::new(50.0)
        .with_stripe_block_rows(1)
        .with_decode_speed(crate::DecodeSpeed::Compact);
    let enc = encode_lossy(&px, w as usize * 3, &info, &Metadata::default(), &cfg).unwrap();
    let (dinfo, _, _) = crate::v1::parse_header(&enc).unwrap();
    let rb = dinfo.row_bytes();
    let mut out = vec![0u8; rb * h as usize];
    for (mode, threads, after) in [
        (2u8, 8usize, 5usize),
        (2, 8, 20),
        (2, 8, 60),
        (2, 24, 40),
        (1, 8, 5),
        (1, 8, 40),
        (1, 24, 60),
    ] {
        super::set_shared_dc(mode);
        let calls = core::sync::atomic::AtomicUsize::new(0);
        let check = || {
            if calls.fetch_add(1, core::sync::atomic::Ordering::Relaxed) >= after {
                Err(Error::Cancelled)
            } else {
                Ok(())
            }
        };
        let p = parse_lossy(&enc).unwrap();
        assert!(p.dc.as_ref().unwrap().hs > p.g.sb);
        let used = super::SHARED_DC_USED.with(|n| n.get());
        let r = decode_lossy_parsed_checked::<false>(
            p,
            &mut out,
            rb,
            threads,
            &crate::Limits::default(),
            true,
            false,
            &check,
        );
        assert!(
            matches!(r, Err(Error::Cancelled)),
            "mode {mode} threads {threads} after {after}: {r:?}"
        );
        assert!(super::SHARED_DC_USED.with(|n| n.get()) > used, "not shared");
    }
    super::set_shared_dc(super::SHARED_DC);
}

/// LAB: jpegli's field as an AQ map (`aq_field`): codes reach the DC
/// image's AQ channel constant per cell and within [16, 255], files decode
/// (SIMD = scalar), and the effort trellis's AQ lambda exponent changes the
/// bytes only where multipliers differ from 1.
#[cfg(feature = "lab")]
#[test]
fn aq_field_map() {
    let (w, h) = (170u32, 122u32);
    let px = test_image(w, h, 7);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let (bw, bh) = (w.div_ceil(8) as usize, h.div_ceil(8) as usize);
    let enc =
        |cfg: &LossyConfig| encode_lossy(&px, w as usize * 3, &info, &Metadata::default(), cfg);
    let codes = |file: &[u8]| -> Vec<u8> {
        let (dinfo, dc, _) = dc_image_lab(file).unwrap();
        assert_eq!(dinfo.channels, Channels::Rgba);
        dc.as_chunks::<8>()
            .0
            .iter()
            .map(|p| u16::from_ne_bytes([p[6], p[7]]) as u8)
            .collect()
    };
    for color in [LossyColor::Xyb, LossyColor::YCoCg] {
        for (cell, levels, relative, fade) in [
            (1u8, 0u8, true, true),
            (4, 4, true, false),
            (2, 2, false, true),
        ] {
            let mut cfg = LossyConfig::new(30.0)
                .with_color(color)
                .with_stripe_block_rows(2);
            cfg.aq_field = Some(LossyAqField {
                strength: 1.5,
                levels,
                cell,
                relative,
                fade,
            });
            let file = enc(&cfg).unwrap();
            let got = codes(&file);
            assert_eq!(got.len(), bw * bh);
            let c = cell as usize;
            for (i, &a) in got.iter().enumerate() {
                assert!((16..=255).contains(&a), "{color:?} cell {cell}: code {a}");
                let top_left = (i / bw / c * c) * bw + (i % bw) / c * c;
                assert_eq!(a, got[top_left], "{color:?} cell {cell}: block {i}");
            }
            if levels > 0 {
                for &a in &got {
                    let steps = (a as f64 / 64.0).log2() * levels as f64;
                    assert!(
                        (steps - steps.round()).abs() < 0.05 || a == 16 || a == 255,
                        "{a}"
                    );
                }
            }
            assert!(
                got.iter().any(|&a| a != got[0]),
                "{color:?} cell {cell}: flat map"
            );
            let (a, b) = both(&file);
            assert_eq!(a.unwrap(), b.unwrap(), "{color:?} cell {cell}");
            // The trellis's lambda exponent (a process-global switch) is
            // tested alone in tests/trellis_lab_switches.rs.
        }
    }
    // Invalid: with the activity rule, a zero cell, too many levels.
    for f in [
        LossyAqField {
            strength: 1.0,
            levels: 0,
            cell: 0,
            relative: true,
            fade: true,
        },
        LossyAqField {
            strength: 1.0,
            levels: 17,
            cell: 1,
            relative: true,
            fade: true,
        },
        LossyAqField {
            strength: 5.0,
            levels: 0,
            cell: 1,
            relative: true,
            fade: true,
        },
    ] {
        let mut cfg = LossyConfig::new(30.0);
        cfg.aq_field = Some(f);
        assert!(enc(&cfg).is_err(), "{f:?}");
    }
    let mut cfg = LossyConfig::new(30.0);
    cfg.aq_field = Some(LossyAqField {
        strength: 1.0,
        levels: 0,
        cell: 1,
        relative: true,
        fade: true,
    });
    cfg.aq_gamma = 0.5;
    assert!(enc(&cfg).is_err());
}

/// LAB: a caller-supplied AQ map is written to the DC image's AQ channel as
/// given, decodes (SIMD = scalar), and a map of all 64 (multiplier 1)
/// reconstructs exactly the pixels of the same encode without AQ.
#[cfg(feature = "lab")]
#[test]
fn caller_aq_map() {
    let (w, h) = (85u32, 61u32);
    let px = test_image(w, h, 7);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let (bw, bh) = (w.div_ceil(8) as usize, h.div_ceil(8) as usize);
    let enc =
        |cfg: &LossyConfig| encode_lossy(&px, w as usize * 3, &info, &Metadata::default(), cfg);
    for color in [LossyColor::Xyb, LossyColor::YCoCg] {
        let base = LossyConfig::new(60.0)
            .with_color(color)
            .with_stripe_block_rows(2);
        let plain = enc(&base).unwrap();
        let (_, plain_px) = crate::decode(&plain).unwrap();
        let mut unit = base.clone();
        unit.aq_map = Some(vec![64u8; bw * bh].into());
        let unit = enc(&unit).unwrap();
        assert!(unit.len() > plain.len());
        assert_eq!(crate::decode(&unit).unwrap().1, plain_px, "{color:?}");
        let map: Vec<u8> = (0..bw * bh)
            .map(|i| [32u8, 45, 64, 91, 128, 200][i * 7 % 6])
            .collect();
        let mut cfg = base.clone();
        cfg.aq_map = Some(map.clone().into());
        let file = enc(&cfg).unwrap();
        let (dinfo, dc, _) = dc_image_lab(&file).unwrap();
        assert_eq!(dinfo.channels, Channels::Rgba);
        let got: Vec<u8> = dc
            .as_chunks::<8>()
            .0
            .iter()
            .map(|p| u16::from_ne_bytes([p[6], p[7]]) as u8)
            .collect();
        assert_eq!(got, map);
        let (a, b) = both(&file);
        assert_eq!(a.unwrap(), b.unwrap());
        // Finer steps where the map is below 64 cost bytes and buy fidelity.
        let (_, out) = crate::decode(&file).unwrap();
        let mut fine = base.clone();
        fine.aq_map = Some(vec![32u8; bw * bh].into());
        let fine = enc(&fine).unwrap();
        assert!(fine.len() > unit.len());
        let (_, fine_px) = crate::decode(&fine).unwrap();
        assert!(ssim2(&px, &fine_px, w, 3) > ssim2(&px, &plain_px, w, 3));
        assert!(ssim2(&px, &out, w, 3) > 30.0);
    }
    // Wrong length, a zero code, or a map together with the activity rule.
    for (map, gamma) in [
        (vec![64u8; bw * bh - 1], 0.0),
        ((0..bw * bh).map(|i| (i != 5) as u8 * 64).collect(), 0.0),
        (vec![64u8; bw * bh], 0.5),
    ] {
        let mut cfg = LossyConfig::new(60.0);
        cfg.aq_map = Some(map.into());
        cfg.aq_gamma = gamma;
        assert!(matches!(enc(&cfg), Err(Error::InvalidInput(_))));
    }
}

/// Text-like ink on a flat page next to a texture: exact blocks for sure.
fn exact_page(w: usize, h: usize, n: usize) -> Vec<u8> {
    let mut v = alloc::vec![255u8; w * h * n];
    for y in 0..h {
        for x in 0..w {
            let o = (y * w + x) * n;
            if x < w / 2 {
                if (x % 9 < 2 && y % 13 > 2) || (y % 13 == 7 && x % 17 < 11) {
                    v[o..o + 3].copy_from_slice(&[20, 30, 90]);
                }
            } else {
                for c in 0..3 {
                    v[o + c] = ((x * 31 + y * 17 + c * 50) % 200) as u8 + 20;
                }
            }
            if n == 4 {
                v[o + 3] = (x + y) as u8;
            }
        }
    }
    v
}

#[test]
fn exact_blocks_simd_matches_reference() {
    for (ch, n) in [(Channels::Rgb, 3), (Channels::Rgba, 4)] {
        for (w, h) in [(13usize, 9usize), (64, 64), (200, 75)] {
            for q in [30.0, 70.0, 95.0] {
                let px = exact_page(w, h, n);
                let info = ImageInfo::new(w as u32, h as u32, ch, SampleType::U8);
                let mut cfg = LossyConfig::new(q);
                cfg.exact_blocks = Some(3.0);
                let enc = encode_lossy(&px, w * n, &info, &Metadata::default(), &cfg).unwrap();
                let p = parse_lossy(&enc).unwrap();
                assert!(p.exact.is_some(), "{w}x{h} {ch:?} q{q}: no exact block");
                let (a, b) = both(&enc);
                assert_eq!(a.unwrap(), b.unwrap(), "{w}x{h} {ch:?} q{q}");
            }
        }
    }
}

/// LAB: the per-encode switch defaults are the process-global switches'
/// initial values (imij-cmp starts every cell from them).
#[cfg(feature = "lab")]
#[test]
fn lab_switch_defaults_are_the_global_initial_values() {
    assert_eq!(
        crate::LossyLabSwitches::default().pinned(),
        crate::LossyLabSwitches::defaults()
    );
}

/// Chunked reconstruction (IDCT + color per chunk of blocks) matches whole
/// block rows for every chunk width, SIMD and scalar, across layouts and the
/// Lossy 0.4 dequantization fields (presets 3/4/5, explicit tables, bias).
#[test]
fn recon_chunks_agree() {
    #[cfg(all(feature = "lab", feature = "std"))]
    let _override_guard = super::scoped_recon_chunk_override();
    let (w, h) = (203u32, 37u32);
    let px = test_image(w, h, 9);
    let lin = test_image_linear(w, h);
    let px16 = test_image16(w, h, 4);
    let linb: Vec<u8> = lin.iter().flat_map(|v| v.to_ne_bytes()).collect();
    // Writers share the resource-capacity test's scoped guard. Other tests
    // may decode concurrently: every chunk width must give the same pixels.
    let cases: [(SampleType, &[u8], LossyColor); 4] = [
        (SampleType::U8, &px, LossyColor::Xyb),
        (SampleType::U8, &px, LossyColor::YCoCg),
        (SampleType::U16, &px16, LossyColor::Xyb),
        (SampleType::F32, &linb, LossyColor::Xyb),
    ];
    for (sample, data, color) in cases {
        let info = ImageInfo::new(w, h, Channels::Rgb, sample);
        let configs = alloc::vec![
            LossyConfig::new(90.0).with_color(color),
            {
                let mut c = LossyConfig::new(90.0).with_color(color);
                c.matrix = 3;
                c
            },
            {
                let mut c = LossyConfig::new(90.0).with_color(color);
                c.matrix = 4;
                c
            },
            {
                let mut c = LossyConfig::new(90.0).with_color(color);
                c.matrix = 5;
                c
            },
        ];
        #[cfg(feature = "lab")]
        let configs = {
            let mut configs = configs;
            let mut c = LossyConfig::new(90.0).with_color(color);
            c.lab.recon_bias = Some(ReconBias {
                k: 8,
                b: [[[16, 8], [4, -4]], [[12, 6], [3, -3]], [[12, 6], [3, -3]]],
            });
            configs.push(c);
            let mut c = LossyConfig::new(90.0).with_color(color);
            let mut t = [[0.0f32; 64]; 3];
            for (ci, tc) in t.iter_mut().enumerate() {
                for (i, v) in tc.iter_mut().enumerate() {
                    *v = 6.0 + (i % 8 + ci) as f32 * 1.5;
                }
            }
            c.lab.explicit_matrix = Some(alloc::boxed::Box::new(t));
            configs.push(c);
            let mut c = LossyConfig::new(90.0).with_color(color);
            c.lab.recon_bias_centroid = Some(64);
            configs.push(c);
            configs
        };
        for cfg in configs {
            let enc =
                encode_lossy(data, info.row_bytes(), &info, &Metadata::default(), &cfg).unwrap();
            super::set_recon_chunk(usize::MAX);
            let (whole, _) = both(&enc);
            let whole = whole.unwrap();
            for n in [1, 2, 3, 5, 32] {
                super::set_recon_chunk(n);
                let (a, b) = both(&enc);
                assert_eq!(
                    a.unwrap(),
                    whole,
                    "{sample:?} {color:?} {cfg:?} chunk {n} simd"
                );
                assert_eq!(
                    b.unwrap(),
                    whole,
                    "{sample:?} {color:?} {cfg:?} chunk {n} scalar"
                );
            }
            super::set_recon_chunk(0);
        }
    }
}

/// A nonzero reconstruction bias moves the pixels of default-layout (8x8)
/// files, identically in the SIMD kernels (AVX-512, AVX2, NEON, wasm128 by
/// host) and the scalar path; a zero bias is the unbiased file. (The format
/// version 2 port of Lossy 0.4 first applied it on the sub-block path only.)
#[cfg(feature = "lab")]
#[test]
fn recon_bias_moves_default_layout_pixels() {
    let (w, h) = (137u32, 70u32);
    let px = test_image(w, h, 77);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let enc = |c: &LossyConfig| {
        encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), c).unwrap()
    };
    for color in [LossyColor::Xyb, LossyColor::YCoCg] {
        for b in [
            [[[64i8, 64], [64, 64]]; 3],
            [[[-40, 20], [16, -8]], [[30, 0], [-12, 6]], [[8, 8], [8, 8]]],
        ] {
            let base = LossyConfig::new(60.0).with_color(color);
            let mut biased = base.clone();
            biased.lab.recon_bias = Some(ReconBias { k: 10, b });
            let (plain, file) = (enc(&base), enc(&biased));
            let (p0, s0) = both(&plain);
            let (p1, s1) = both(&file);
            let (p0, p1) = (p0.unwrap(), p1.unwrap());
            assert_eq!(s0.unwrap(), p0, "{color:?}: unbiased, scalar differs");
            assert_eq!(s1.unwrap(), p1, "{color:?} {b:?}: biased, scalar differs");
            let moved = p0.iter().zip(&p1).filter(|(a, b)| a != b).count();
            assert!(
                moved > p0.len() / 20,
                "{color:?} {b:?}: {moved} values moved"
            );
        }
    }
}

/// Superblocks with a reconstruction bias (user decision 2026-10-10): one
/// file declares both; the bias applies to 8x8 blocks only, merged blocks
/// are dequantized without it (SPEC 2.9). With the coefficients and merges
/// fixed (no exact blocks; the selector prices without the bias), every
/// merged block decodes the same with and without the bias and 8x8 blocks
/// move; the SIMD kernels equal the scalar path.
#[cfg(feature = "lab")]
#[test]
fn superblocks_with_recon_bias() {
    use crate::parse::header::Feature;
    let (w, h) = (256u32, 160u32);
    let px = test_image(w, h, 5);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let enc = |c: &LossyConfig| {
        encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), c).unwrap()
    };
    for color in [LossyColor::Xyb, LossyColor::YCoCg] {
        for q in [20.0f32, 50.0] {
            let what = alloc::format!("{color:?} q{q}");
            let mut base = LossyConfig::new(q)
                .with_color(color)
                .with_decode_speed(crate::DecodeSpeed::Compact);
            base.exact_blocks = Some(0.0);
            base.lab.superblocks = Some(true);
            let mut biased = base.clone();
            biased.lab.recon_bias = Some(ReconBias {
                k: 10,
                b: [
                    [[64, 48], [32, 16]],
                    [[48, 32], [24, 12]],
                    [[48, 32], [24, 12]],
                ],
            });
            let (plain, file) = (enc(&base), enc(&biased));
            let declared = crate::v1::parse_header_lossy(&file).unwrap().3;
            assert!(declared.allows(Feature::Superblocks), "{what}");
            assert!(declared.allows(Feature::ReconBias), "{what}");
            let (d0, s0) = both(&plain);
            let (d1, s1) = both(&file);
            let (d0, d1) = (d0.unwrap(), d1.unwrap());
            assert_eq!(s0.unwrap(), d0, "{what}: unbiased, scalar differs");
            assert_eq!(s1.unwrap(), d1, "{what}: biased, scalar differs");
            // Per block: merged parents unchanged, some 8x8 block moved.
            let roles = |f: &[u8]| {
                let p = parse_lossy(f).unwrap();
                assert_eq!(p.ngroups(), 1);
                let mut all = alloc::vec![0u8; p.g.bw * p.g.bh];
                let mut r = Vec::new();
                for s in 0..p.g.stripes() {
                    let (a, b) = p.g.rows(s);
                    super::superblock::read_map(p.stripes[s], b - a, p.g.bw, &mut r).unwrap();
                    if !r.is_empty() {
                        all[a * p.g.bw..b * p.g.bw].copy_from_slice(&r);
                    }
                }
                (all, p.g.bw)
            };
            let ((rl, bw), (rl0, _)) = (roles(&file), roles(&plain));
            assert_eq!(rl, rl0, "{what}: merges differ with the bias");
            let rb = info.row_bytes();
            let block_eq = |bx: usize, by: usize| {
                (by * 8..(by * 8 + 8).min(h as usize)).all(|y| {
                    let x0 = bx * 8 * 3;
                    let x1 = ((bx * 8 + 8).min(w as usize)) * 3;
                    d0[y * rb + x0..y * rb + x1] == d1[y * rb + x0..y * rb + x1]
                })
            };
            let (mut merged, mut moved) = (0, 0);
            for (i, &role) in rl.iter().enumerate() {
                let (bx, by) = (i % bw, i / bw);
                if role != 0 {
                    merged += 1;
                    assert!(block_eq(bx, by), "{what}: merged block {bx},{by} moved");
                } else if !block_eq(bx, by) {
                    moved += 1;
                }
            }
            assert!(merged > 0, "{what}: no merge");
            assert!(moved > 0, "{what}: no 8x8 block moved");
        }
    }
}

/// Lossy 0.4 (matrix presets 3/4, explicit tables, fixed and centroid
/// reconstruction bias): every decode route agrees, scalar equals SIMD, the
/// files list Lossy 0.4, a zero bias is byte-identical to no bias, and an
/// unbiased file keeps its old bytes.
#[test]
fn lossy_0_4_decodes_on_every_route() {
    let (w, h) = (137u32, 70u32);
    let px = test_image(w, h, 77);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let rb = info.row_bytes();
    #[cfg(feature = "lab")]
    let bias = ReconBias {
        k: 8,
        b: [[[16, 8], [4, -4]], [[12, 6], [3, -3]], [[12, 6], [3, -3]]],
    };
    let mut configs: Vec<(alloc::string::String, LossyConfig)> = Vec::new();
    for (name, color) in [("ycocg", LossyColor::YCoCg), ("xyb", LossyColor::Xyb)] {
        for m in [3u8, 4, 5] {
            let mut c = LossyConfig::new(60.0).with_color(color);
            c.matrix = m;
            configs.push((alloc::format!("matrix {m} {name}"), c));
        }
    }
    #[cfg(feature = "lab")]
    {
        // The tablemath study's probe presets encode and decode under the
        // same dequant_presets feature.
        for (name, color) in [("ycocg", LossyColor::YCoCg), ("xyb", LossyColor::Xyb)] {
            for m in [6u8, 7, 8, 9] {
                let mut c = LossyConfig::new(60.0).with_color(color);
                c.matrix = m;
                configs.push((alloc::format!("matrix {m} {name}"), c));
            }
        }
        for (name, color) in [("ycocg", LossyColor::YCoCg), ("xyb", LossyColor::Xyb)] {
            let mut c = LossyConfig::new(60.0).with_color(color);
            c.lab.recon_bias = Some(bias);
            configs.push((alloc::format!("bias {name}"), c));
        }
        let mut c = LossyConfig::new(60.0).with_color(LossyColor::YCoCg);
        let mut t = [[0.0f32; 64]; 3];
        for (ci, tc) in t.iter_mut().enumerate() {
            for (i, v) in tc.iter_mut().enumerate() {
                *v = 6.0 + (i % 8 + ci) as f32 * 1.5;
            }
        }
        c.lab.explicit_matrix = Some(alloc::boxed::Box::new(t));
        configs.push(("explicit tables".into(), c));
        for k in [8u8, 64] {
            let mut c = LossyConfig::new(60.0).with_color(LossyColor::YCoCg);
            c.lab.recon_bias_centroid = Some(k);
            configs.push((alloc::format!("centroid k{k}"), c));
        }
        // The bias reaches the sub-block reconstruction path too.
        let mut c = LossyConfig::new(60.0).with_color(LossyColor::YCoCg);
        c.channel_layouts = [
            LossyChannelLayout::new(crate::LossyTransform::Dct8, crate::ChannelSampling::Full),
            LossyChannelLayout::new(crate::LossyTransform::Dct4, crate::ChannelSampling::Full),
            LossyChannelLayout::new(crate::LossyTransform::Dct4, crate::ChannelSampling::Full),
        ];
        c.lab.recon_bias = Some(bias);
        configs.push(("bias subblocks".into(), c));
    }
    for (name, cfg) in configs {
        let enc = encode_lossy(&px, rb, &info, &Metadata::default(), &cfg).unwrap();
        let declared = crate::v1::parse_header_lossy(&enc).unwrap().3;
        assert!(
            declared.allows(crate::parse::header::Feature::DequantPresets)
                || declared.allows(crate::parse::header::Feature::ExplicitTables)
                || declared.allows(crate::parse::header::Feature::ReconBias)
                || declared.allows(crate::parse::header::Feature::ColorTransform),
            "{name}: no Lossy 0.4 feature declared"
        );
        let (a, b) = both(&enc);
        let full = a.unwrap();
        assert_eq!(b.unwrap(), full, "{name}: scalar differs");
        let mut out = vec![0u8; rb * h as usize];
        decode_test_pool(&enc, &mut out, rb, 4);
        assert_eq!(out, full, "{name}: threaded");
        let mut sd = crate::StreamDecoder::new(&enc).unwrap();
        let mut row = vec![0u8; rb];
        let mut y = 0;
        while sd.next_row(&mut row).unwrap() {
            assert_eq!(row, full[y * rb..(y + 1) * rb], "{name}: stream row {y}");
            y += 1;
        }
        assert_eq!(y, h as usize, "{name}: streamed rows");
        let rect = crate::planes::Rect::new(13, 9, 40, 25);
        let mut rw = vec![0u8; 40 * 3 * 25];
        crate::region::decode_region(&enc, rect, &mut rw, 40 * 3).unwrap();
        for y in 0..25usize {
            let src = &full[(9 + y) * rb + 13 * 3..][..40 * 3];
            assert_eq!(&rw[y * 40 * 3..][..40 * 3], src, "{name}: region row {y}");
        }
    }
    // A zero bias changes nothing: the same bytes as without the knob.
    #[cfg(feature = "lab")]
    {
        let plain = LossyConfig::new(60.0).with_color(LossyColor::YCoCg);
        let mut zero = plain.clone();
        zero.lab.recon_bias = Some(ReconBias {
            k: 64,
            b: [[[0; 2]; 2]; 3],
        });
        let a = encode_lossy(&px, rb, &info, &Metadata::default(), &plain).unwrap();
        let b = encode_lossy(&px, rb, &info, &Metadata::default(), &zero).unwrap();
        assert_eq!(a, b, "a zero bias must not change the file");
        // A centroid that rounds to zero takes the same path.
        let mut c = plain.clone();
        c.lab.recon_bias_centroid = Some(64);
        let d = encode_lossy(&px, rb, &info, &Metadata::default(), &c).unwrap();
        assert!(
            crate::v1::parse_header_lossy(&d)
                .unwrap()
                .3
                .allows(crate::parse::header::Feature::ReconBias)
        );
    }
    // An Annex K encode declares no Lossy 0.4 feature.
    let mut plain = LossyConfig::new(60.0).with_color(LossyColor::YCoCg);
    plain.matrix = 0;
    let enc = encode_lossy(&px, rb, &info, &Metadata::default(), &plain).unwrap();
    assert!(
        !crate::v1::parse_header_lossy(&enc)
            .unwrap()
            .3
            .contains(crate::parse::header::Feature::DequantPresets)
    );
}

/// The signalled linear color transform (Lossy 0.4): every preset and an
/// explicit matrix decode on every route, scalar equals SIMD, the YCoCg
/// preset is bit-exact against today's lifting, and unsupported combinations
/// are rejected.
#[test]
fn signalled_color_transform_decodes_on_every_route() {
    let (w, h) = (101u32, 53u32);
    let px = test_image(w, h, 31);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let rb = info.row_bytes();
    let base = LossyConfig::new(70.0).with_color(LossyColor::YCoCg);
    let plain = encode_lossy(&px, rb, &info, &Metadata::default(), &base).unwrap();
    let plain_px = crate::decode(&plain).unwrap().1;
    let configs: Vec<(alloc::string::String, LossyConfig)> = Vec::new();
    #[cfg(feature = "lab")]
    let configs = {
        let mut configs = configs;
        for (name, t) in [
            ("identity", ColorTransform::Identity),
            ("bt601", ColorTransform::Bt601),
            ("bt709", ColorTransform::Bt709),
            ("ycocg", ColorTransform::YCoCg),
            ("explicit", ColorTransform::Explicit(color::TRANSFORM_BT709)),
        ] {
            let mut c = base.clone();
            c.lab.color_transform = Some(t);
            configs.push((name.into(), c));
        }
        configs
    };
    for (name, cfg) in configs {
        let enc = encode_lossy(&px, rb, &info, &Metadata::default(), &cfg).unwrap();
        let declared = crate::v1::parse_header_lossy(&enc).unwrap().3;
        assert!(
            declared.allows(crate::parse::header::Feature::DequantPresets)
                || declared.allows(crate::parse::header::Feature::ExplicitTables)
                || declared.allows(crate::parse::header::Feature::ReconBias)
                || declared.allows(crate::parse::header::Feature::ColorTransform),
            "{name}: no Lossy 0.4 feature declared"
        );
        let (a, b) = both(&enc);
        let full = a.unwrap();
        assert_eq!(b.unwrap(), full, "{name}: scalar differs");
        let mut out = vec![0u8; rb * h as usize];
        decode_test_pool(&enc, &mut out, rb, 4);
        assert_eq!(out, full, "{name}: threaded");
        let mut sd = crate::StreamDecoder::new(&enc).unwrap();
        let mut row = vec![0u8; rb];
        let mut y = 0;
        while sd.next_row(&mut row).unwrap() {
            assert_eq!(row, full[y * rb..(y + 1) * rb], "{name}: stream row {y}");
            y += 1;
        }
        assert_eq!(y, h as usize, "{name}: streamed rows");
        let rect = crate::planes::Rect::new(11, 7, 37, 21);
        let mut rw = vec![0u8; 37 * 3 * 21];
        crate::region::decode_region(&enc, rect, &mut rw, 37 * 3).unwrap();
        for y in 0..21usize {
            let src = &full[(7 + y) * rb + 11 * 3..][..37 * 3];
            assert_eq!(&rw[y * 37 * 3..][..37 * 3], src, "{name}: region row {y}");
        }
        if name == "ycocg" {
            assert_eq!(full, plain_px, "the YCoCg preset must be bit-exact");
        }
    }
    #[cfg(feature = "lab")]
    {
        // The transform is defined for U8 YCoCg-family input only.
        let mut c = base.clone();
        c.lab.color_transform = Some(ColorTransform::Bt601);
        c.color = LossyColor::Xyb;
        assert!(matches!(
            encode_lossy(&px, rb, &info, &Metadata::default(), &c),
            Err(Error::Unsupported(_))
        ));
        let mut c = base.clone();
        c.lab.color_transform = Some(ColorTransform::Bt601);
        let info16 = ImageInfo::new(w, h, Channels::Rgb, SampleType::U16);
        let px16 = test_image16(w, h, 2);
        assert!(matches!(
            encode_lossy(&px16, info16.row_bytes(), &info16, &Metadata::default(), &c),
            Err(Error::Unsupported(_))
        ));
        let mut c = base.clone();
        c.lab.color_transform = Some(ColorTransform::Explicit([[0.0; 4]; 3]));
        assert!(matches!(
            encode_lossy(&px, rb, &info, &Metadata::default(), &c),
            Err(Error::InvalidInput(_))
        ));
    }
}

/// The generic signalled-transform SIMD kernel equals the scalar reference
/// bit for bit on random plane values (every tier; odd widths for tails).
#[test]
fn linear_rows_match_scalar() {
    let m = color::TRANSFORM_BT601;
    let mut s = 0x9e37_79b9u32;
    let mut rnd = |r: f32| {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        (s as f32 / u32::MAX as f32 * 2.0 - 1.0) * r
    };
    for w in [1usize, 7, 15, 16, 17, 31, 33, 255, 4099] {
        for _ in 0..(1 << 20) / w.max(64) {
            let p0: Vec<f32> = (0..w + 64).map(|_| rnd(400.0) + 120.0).collect();
            let p1: Vec<f32> = (0..w + 64).map(|_| rnd(400.0) + 120.0).collect();
            let p2: Vec<f32> = (0..w + 64).map(|_| rnd(400.0) + 120.0).collect();
            let mut out = vec![0u8; w * 3];
            simd::linear_row(&p0, &p1, &p2, [1.0; 3], &m, &mut out, w);
            for x in 0..w {
                let e = color::inverse_linear_px(&m, [1.0; 3], [p0[x], p1[x], p2[x]]);
                assert_eq!(out[x * 3..x * 3 + 3], e, "w {w} x {x}");
            }
        }
    }
}

/// Matrix 5 is matrix 4 at q90 (distance 1) and follows jpegli's nonlinear
/// distance law below it; its steps stay inside the format's bound across
/// the quality range (it costs no envelope bytes).
#[test]
fn matrix_5_follows_the_scale() {
    let q90 = QParams::from_quality(90.0, LossyColor::YCoCg, 5);
    let m4 = QParams::from_quality(90.0, LossyColor::YCoCg, 4);
    for c in 0..3 {
        assert_eq!(q90.base(c), m4.base(c), "channel {c} at q90");
    }
    let q40 = QParams::from_quality(40.0, LossyColor::YCoCg, 5);
    assert_ne!(
        q40.base(1),
        m4.base(1),
        "below the threshold the shape moves"
    );
    // q1 (scale 50) times the jpegli tables' coarsest chroma steps exceeds
    // the format's 65536 bound with the YCoCg multiplier 2, exactly as
    // matrix 4 does; the preset is usable from q2.
    for q in [2.0, 5.0, 20.0, 50.0, 80.0, 100.0] {
        let qp = QParams::from_quality(q, LossyColor::YCoCg, 5);
        assert!(qp.valid(), "q{q}");
        assert!(
            qp.steps().iter().flatten().all(|s| *s <= 65536.0),
            "q{q} exceeds the step bound"
        );
    }
}

/// Matrix 5 conformance vectors: the reference f32 values for the scales of
/// q90 (0.2), q50 (1.0) and q10 (5.0), pinned here and in SPEC 2.8.1. Every
/// tier computes these through the same deterministic ln/exp.
#[test]
fn matrix_5_conformance_vectors() {
    let vectors: [((f32, f32), [[u32; 4]; 3], [u32; 4]); 3] = [
        (
            (90.0, 0.2),
            [
                [0x4180_0000, 0x416f_c12c, 0x416f_c12c, 0x4218_51f5],
                [0x4188_0000, 0x4262_010c, 0x4262_010c, 0x4473_e70a],
                [0x4188_0000, 0x421c_7d1d, 0x421c_7d1d, 0x4479_d761],
            ],
            [0x3f4c_cccd, 0x403f_cdbd, 0x41b4_cda3, 0x43c7_df81],
        ),
        (
            (50.0, 1.0),
            [
                [0x4180_0000, 0x4104_e8db, 0x4104_e8db, 0x4218_51f5],
                [0x4188_0000, 0x41fa_92e7, 0x41fa_92e7, 0x4473_e70a],
                [0x4188_0000, 0x41ad_804f, 0x41ad_804f, 0x4479_d761],
            ],
            [0x4080_0000, 0x4104_e8db, 0x427a_92e7, 0x44f9_d761],
        ),
        (
            (10.0, 5.0),
            [
                [0x4180_0000, 0x40ef_c12c, 0x40ef_c12c, 0x4218_51f5],
                [0x4188_0000, 0x41e2_010c, 0x41e2_010c, 0x4473_e70a],
                [0x4188_0000, 0x419c_7d1d, 0x419c_7d1d, 0x4479_d761],
            ],
            [0x41a0_0000, 0x4215_d8bc, 0x438d_40a8, 0x461c_269d],
        ),
    ];
    for ((q, scale), bases, steps) in vectors {
        let qp = QParams::from_quality(q, LossyColor::YCoCg, 5);
        assert_eq!(qp.scale, scale);
        for (c, want) in bases.iter().enumerate() {
            let b = qp.base(c);
            assert_eq!(
                [
                    b[0].to_bits(),
                    b[1].to_bits(),
                    b[8].to_bits(),
                    b[63].to_bits()
                ],
                *want,
                "q{q} channel {c}"
            );
        }
        let s = qp.steps();
        assert_eq!(
            [
                s[0][0].to_bits(),
                s[0][1].to_bits(),
                s[1][1].to_bits(),
                s[2][63].to_bits()
            ],
            steps,
            "q{q} steps"
        );
    }
}

/// Small transforms (SPEC 2.11): forced 4x4 planes and the priced selector,
/// with and without merges, decode the same through the SIMD kernels and
/// the scalar reference, declare `small_transforms` (and `superblocks` only
/// when something merges), and stay close to the source.
#[cfg(feature = "lab")]
#[test]
fn small_transforms_decode_identically_on_every_kernel() {
    use crate::parse::header::Feature;
    let (w, h) = (200u32, 136u32);
    let px = test_image(w, h, 9);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let enc = |c: &LossyConfig| {
        encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), c).unwrap()
    };
    for (color, effort) in [
        (LossyColor::Xyb, 7.5f32),
        (LossyColor::YCoCg, 7.5),
        // The trellis (XYB, efforts 9 and 10) on 8x8 and 4x4 planes.
        (LossyColor::Xyb, 10.0),
    ] {
        for q in [10.0f32, 50.0, 90.0, 99.0] {
            let base = LossyConfig::new(q).with_color(color).with_effort(effort);
            let plain = enc(&base);
            let e_plain = {
                let d = both(&plain).0.unwrap();
                px.iter()
                    .zip(&d)
                    .map(|(&a, &b)| (a as f64 - b as f64).powi(2))
                    .sum::<f64>()
                    / px.len() as f64
            };
            for (force, merges) in [
                (Some(7u8), false),
                (Some(1), false),
                (Some(6), false),
                (None, false),
                (None, true),
            ] {
                let what =
                    alloc::format!("{color:?} e{effort} q{q} force {force:?} merges {merges}");
                let mut cfg = base.clone();
                cfg.lab.small = Some(alloc::boxed::Box::new(SmallLab {
                    split: force,
                    ..SmallLab::on()
                }));
                cfg.lab.superblocks = Some(merges);
                let file = enc(&cfg);
                let declared = crate::v1::parse_header_lossy(&file).unwrap().3;
                let p = parse_lossy(&file).unwrap();
                if force.is_some() {
                    assert!(declared.allows(Feature::SmallTransforms), "{what}");
                    assert_eq!(p.map, MapKind::Joint, "{what}");
                }
                if p.map == MapKind::Joint {
                    assert!(declared.allows(Feature::SmallTransforms), "{what}");
                }
                let (d, s) = both(&file);
                let (d, s) = (d.unwrap(), s.unwrap());
                assert!(d == s, "{what}: SIMD and scalar differ");
                let e = px
                    .iter()
                    .zip(&d)
                    .map(|(&a, &b)| (a as f64 - b as f64).powi(2))
                    .sum::<f64>()
                    / px.len() as f64;
                assert!(e <= 3.0 * e_plain + 2.0, "{what}: mse {e} vs {e_plain}");
            }
        }
    }
}

#[test]
fn freq_zero_bias_and_adaptive_partitions_roundtrip_all_modes_and_efforts() {
    let (w, h) = (160u32, 136u32);
    let px = test_image(w, h, 3);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    for ent in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
        for effort in [7.5f32, 10.0] {
            let cfg = LossyConfig::new(75.0)
                .with_effort(effort)
                .with_entropy(ent)
                .with_freq_zero_bias(true)
                .with_adaptive_partitions(true);
            let encoded =
                encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), &cfg).unwrap();
            let (d, s) = both(&encoded);
            let (d, s) = (d.unwrap(), s.unwrap());
            assert_eq!(d, s, "SIMD and scalar must match on {ent:?} e{effort}");
            let mut mt = vec![0u8; d.len()];
            crate::decode_into_threads(&encoded, &mut mt, info.row_bytes(), 4).unwrap();
            assert_eq!(mt, d, "4-thread decode must match on {ent:?} e{effort}");
        }
    }
}

#[cfg(feature = "lab")]
#[test]
fn split_huf_trellis_with_recon_bias_and_superblocks_does_not_panic() {
    let (w, h) = (160u32, 136u32);
    let px = test_image(w, h, 4);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let mut cfg = LossyConfig::new(70.0)
        .with_effort(10.0)
        .with_entropy(LossyEntropy::SplitHuf)
        .with_decode_speed(crate::DecodeSpeed::Fast)
        .with_freq_zero_bias(true)
        .with_adaptive_partitions(true);
    cfg.lab.recon_bias = Some(ReconBias {
        k: 16,
        b: [[[16, 8], [24, 12]], [[12, 6], [16, 8]], [[12, 6], [16, 8]]],
    });
    let encoded = encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), &cfg).unwrap();
    let (d, s) = both(&encoded);
    assert_eq!(d.unwrap(), s.unwrap());
}

#[test]
fn raw_channel_identity_lossy_and_near_lossless_raw_channels_roundtrip() {
    let (w, h) = (128u32, 96u32);
    let px = test_image(w, h, 2);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);

    // 1. Lossy DCT with ColorTransform::Identity (no color transform, raw RGB channels)
    let cfg = LossyConfig::new(80.0)
        .with_color_transform(Some(ColorTransform::Identity))
        .with_freq_zero_bias(true)
        .with_adaptive_partitions(true);
    let encoded = encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), &cfg).unwrap();
    let (d, s) = both(&encoded);
    let (d, s) = (d.unwrap(), s.unwrap());
    assert_eq!(
        d, s,
        "Identity raw-channel lossy SIMD and scalar must match"
    );

    // 2. Pixel-domain near-lossless with no DCT and no color transform (decor_mask = 0)
    let mut near_cfg = crate::EncodeConfig::default().with_max_error(2);
    near_cfg.decor_mask = 1; // bit 0 = identity (no cross-channel decorrelation)
    let near_bytes = crate::encode(
        &px,
        info.row_bytes(),
        &info,
        &Metadata::default(),
        &near_cfg,
    )
    .unwrap();
    let (_di, near_dec) = crate::decode(&near_bytes).unwrap();
    for (&a, &b) in px.iter().zip(near_dec.iter()) {
        assert!(
            a.abs_diff(b) <= 2,
            "near-lossless max_error=2 exceeded: {a} vs {b}"
        );
    }
}
