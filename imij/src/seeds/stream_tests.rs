//! Seeded-stream robustness and coverage (from the 2026-10-07 independent
//! review, adapted to seed tables being on by default).

use crate::lossy::tests::{both, test_image};
use crate::{ByteReader, Channels, ImageInfo, LossyConfig, LossyEntropy, Metadata, SampleType};
use crate::{DecodeSpeed, LossyColor, encode_lossy};
use alloc::vec::Vec;

fn selector(file: &[u8]) -> Result<(u8, u8), crate::Error> {
    let (e, id, scope) = crate::seeds::lossy_entropy_seed(file)?;
    Ok((
        e,
        if scope == 0 {
            0
        } else {
            u8::try_from(id).unwrap() | scope << 4
        },
    ))
}

/// Offsets in `file` (SPEC 2.8) of: the parameters' start (`present`),
/// the seed byte (where it is, or would go), the parameters' end, table blob
/// start, table blob end, dc head start, dc head end, alpha head (start,
/// end).
fn layout(file: &[u8]) -> [usize; 9] {
    let (info, _, body) = crate::v1::parse_header(file).unwrap();
    let base = file.len() - body.len();
    let mut r = ByteReader::new(body);
    let pos = |r: &ByteReader<'_>| base + body.len() - r.remaining();
    r.u8().unwrap();
    r.varint().unwrap();
    let env0 = pos(&r);
    let present = r.varint().unwrap();
    r.bytes(4).unwrap();
    let mut seed_at = pos(&r);
    if present & 1 != 0 {
        r.u8().unwrap();
    }
    for (bit, n) in [(1, 1), (2, 1), (3, 3), (6, 4), (7, 12), (8, 12)] {
        if present >> bit & 1 != 0 {
            r.bytes(n).unwrap();
        }
    }
    if present & (1 << 9) != 0 {
        seed_at = pos(&r);
        r.varint().unwrap();
        r.varint().unwrap();
    }
    let env1 = pos(&r);
    let tl = r.var_usize().unwrap();
    let t0 = pos(&r);
    r.bytes(tl).unwrap();
    let t1 = pos(&r);
    if present & 16 != 0 {
        let n = r.var_usize().unwrap();
        r.bytes(n).unwrap();
    }
    let dl = r.var_usize().unwrap();
    let d0 = pos(&r);
    r.bytes(dl).unwrap();
    let d1 = pos(&r);
    let (mut a0, mut a1) = (d1, d1);
    if info.channels.has_alpha() {
        let al = r.var_usize().unwrap();
        a0 = pos(&r);
        r.bytes(al).unwrap();
        a1 = pos(&r);
    }
    [env0, seed_at, env1, t0, t1, d0, d1, a0, a1]
}

fn decode_all(d: &[u8]) -> Result<Vec<u8>, crate::Error> {
    let a = crate::decode(d).map(|x| x.1);
    let (s, r) = both(d);
    // SIMD and scalar must agree on success.
    assert_eq!(s.is_ok(), r.is_ok());
    if let (Ok(s), Ok(r)) = (&s, &r) {
        assert_eq!(s, r);
    }
    a
}

/// Inserts a seed byte (presence bit 0) into an unseeded file's parameters
/// and rebuilds the file; `seeds` declares `seed_bank_1` in the new header.
fn with_seed_byte(file: &[u8], seed: u8, seeds: bool) -> Vec<u8> {
    let l = layout(file);
    let mut p = ByteReader::new(&file[l[0]..]);
    let present = p.varint().unwrap();
    assert_eq!(present & 1, 0);
    let header = crate::v1::header(file).unwrap();
    let (info, meta, body) = crate::v1::parse_header(file).unwrap();
    let base = file.len() - body.len();
    let mut nb = body[..l[0] - base].to_vec();
    crate::put_varint(&mut nb, present | 1);
    nb.extend_from_slice(&file[l[0] + p.pos..l[1]]);
    nb.push(seed);
    nb.extend_from_slice(&body[l[1] - base..]);
    rewrap(&header, &info, &meta, &nb, seeds)
}

/// A lossy file from `header` and `body`; `seeds` declares `seed_bank_1`.
fn rewrap(
    header: &crate::Header,
    info: &ImageInfo,
    meta: &Metadata,
    body: &[u8],
    seeds: bool,
) -> Vec<u8> {
    use crate::parse::header::{Feature, Features};
    let features = if seeds {
        Features::EMPTY.with(Feature::SeedBank1)
    } else {
        Features::EMPTY
    };
    let mut out = Vec::new();
    crate::container::write_head_features(
        &mut out,
        info,
        meta,
        crate::Coding::Lossy,
        header.lossy_color,
        header.decode_speed,
        features,
    )
    .unwrap();
    crate::container::put_data(&mut out, body);
    out
}

fn files() -> Vec<(Vec<u8>, &'static str)> {
    let mut v = Vec::new();
    for &(w, h, rgba) in &[(32u32, 24u32, false), (48, 40, true), (64, 64, false)] {
        let ch = if rgba { Channels::Rgba } else { Channels::Rgb };
        let info = ImageInfo::new(w, h, ch, SampleType::U8);
        let rgb = test_image(w, h, 3 * w + h);
        let px: Vec<u8> = if rgba {
            rgb.chunks(3)
                .enumerate()
                .flat_map(|(i, p)| [p[0], p[1], p[2], (i * 37 % 251) as u8])
                .collect()
        } else {
            rgb
        };
        for (e, name) in [
            (LossyEntropy::RunLevel, "rl"),
            (LossyEntropy::SplitHuf, "sh"),
        ] {
            for q in [5.0, 40.0] {
                let cfg = LossyConfig::new(q).with_entropy(e);
                let f =
                    encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), &cfg).unwrap();
                v.push((crate::lossy::legacy_seed_fixture(&f), name));
            }
        }
    }
    v
}

/// Every single-bit flip in the parameters, AC tables, head records and
/// nested heads, and
/// every value of the seed byte: no panics, SIMD == scalar.
#[test]
fn exhaustive_head_bitflips() {
    let mut seeded = 0;
    for (f, name) in files() {
        let (_, seed) = selector(&f).unwrap();
        if seed == 0 {
            continue;
        }
        seeded += 1;
        let l = layout(&f);
        let mut flips = 0;
        let mut ok_after_flip = 0;
        for at in l[0]..l[8] {
            for b in 0..8 {
                let mut d = f.clone();
                d[at] ^= 1 << b;
                flips += 1;
                if decode_all(&d).is_ok() {
                    ok_after_flip += 1;
                }
            }
        }
        // All 256 seed byte values.
        let sb = l[1];
        assert_eq!(f[sb], seed);
        for s in 0..=255u8 {
            let mut d = f.clone();
            d[sb] = s;
            let r = decode_all(&d);
            if s & 0x80 != 0 || s & 0x70 == 0 || s & 15 == 0 {
                assert!(matches!(r, Err(crate::Error::Corrupt(_))), "{s:#x} {r:?}");
            } else if s & 15 != 1 && crate::seeds::bank(u64::from(s & 15)).is_none() {
                // A bank without a feature in this version (SPEC 0.3).
                assert_eq!(
                    r,
                    Err(crate::Error::Corrupt("seed bank without its feature")),
                    "{s:#x}"
                );
            }
        }
        std::eprintln!(
            "{name} len {} seed {seed:#x} env {}..{} tables {}..{} dc {}..{} alpha {}..{}: {flips} flips, {ok_after_flip} decoded",
            f.len(),
            l[1],
            l[2],
            l[3],
            l[4],
            l[5],
            l[6],
            l[7],
            l[8]
        );
    }
    assert!(seeded > 0);
}

/// Truncating a seeded file anywhere never panics.
#[test]
fn truncations() {
    for (f, _) in files() {
        for n in 0..f.len() {
            let _ = decode_all(&f[..n]);
        }
    }
}

/// A seed byte inserted into a file whose tables carry no selectors: never a
/// panic; without `seed_bank_1` in the header it is corrupt; with it, bank 0
/// is corrupt and an unknown bank unsupported.
#[test]
fn inserted_seed_byte() {
    let (w, h) = (40u32, 32u32);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let px = test_image(w, h, 9);
    for e in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
        let mut cfg = LossyConfig::new(30.0).with_entropy(e);
        cfg.seed_bank = 0;
        let f = encode_lossy(&px, w as usize * 3, &info, &Metadata::default(), &cfg).unwrap();
        assert!(decode_all(&f).is_ok());
        // Rebuilding the header changes nothing; declaring `seed_bank_1`
        // without seed tables is valid and costs the one-byte mask (its
        // presence bit shares the `present` byte with the coding's).
        let header = crate::v1::header(&f).unwrap();
        let (i, m, body) = crate::v1::parse_header(&f).unwrap();
        assert_eq!(rewrap(&header, &i, &m, body, false), f);
        let listed = rewrap(&header, &i, &m, body, true);
        assert_eq!(listed.len(), f.len() + 1);
        assert_eq!(decode_all(&listed).unwrap(), decode_all(&f).unwrap());
        for scope in [0x10u8, 0x20, 0x40, 0x70] {
            let r = decode_all(&with_seed_byte(&f, scope | 1, false));
            assert_eq!(
                r.unwrap_err(),
                crate::Error::Corrupt("seed tables without the feature seed_bank_1")
            );
            let d = with_seed_byte(&f, scope | 1, true);
            let r = decode_all(&d);
            // Alpha scope on an image without alpha.
            if scope & 0x40 != 0 {
                assert!(matches!(r, Err(crate::Error::Corrupt(_))), "{e:?} {r:?}");
            }
        }
        // Bank 2 (absent: no feature in this version, SPEC 0.3) and bank 0:
        // corrupt.
        let r = decode_all(&with_seed_byte(&f, 0x10 | 2, true));
        assert_eq!(
            r.unwrap_err(),
            crate::Error::Corrupt("seed bank without its feature")
        );
        let r = decode_all(&with_seed_byte(&f, 0x10, true));
        assert!(matches!(r, Err(crate::Error::Corrupt(_))), "{r:?}");
    }
}

/// Tiers: seeds never make a file larger than the encode without them, and
/// decode to the same pixels.
#[test]
fn tiers_never_larger() {
    let mut grown = Vec::new();
    let mut cells = 0;
    for &(w, h) in &[(24u32, 24u32), (40, 32), (64, 48), (80, 80), (128, 96)] {
        for seed in 0..4u32 {
            let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
            let px = test_image(w, h, seed * 101 + w);
            for q in [2.0, 8.0, 15.0, 25.0, 45.0, 70.0, 90.0] {
                for color in [LossyColor::Xyb, LossyColor::YCoCg] {
                    for speed in [
                        DecodeSpeed::Compact,
                        DecodeSpeed::Balanced,
                        DecodeSpeed::Fast,
                    ] {
                        let cfg = LossyConfig::new(q)
                            .with_color(color)
                            .with_decode_speed(speed);
                        let mut off = cfg.clone();
                        off.seed_bank = 0;
                        let a =
                            encode_lossy(&px, w as usize * 3, &info, &Metadata::default(), &off)
                                .unwrap();
                        let b =
                            encode_lossy(&px, w as usize * 3, &info, &Metadata::default(), &cfg)
                                .unwrap();
                        cells += 1;
                        assert_eq!(crate::decode(&a).unwrap().1, crate::decode(&b).unwrap().1);
                        if b.len() > a.len() {
                            grown.push((w, h, seed, q, color, speed, a.len(), b.len()));
                        }
                    }
                }
            }
        }
    }
    std::eprintln!("cells {cells}, grown {grown:?}");
    assert!(grown.is_empty());
}

/// Seeds are used in every scope and for every lossless token scheme the
/// bank has: alpha tables, DC palettes (5,2,8) and 15-bit DC (4,2,15).
#[test]
fn scope_coverage() {
    use alloc::collections::BTreeMap;
    let mut used: BTreeMap<(u8, u8, [u8; 3]), u32> = BTreeMap::new();
    let mut alpha_scope = 0;
    for &(w, h) in &[(24u32, 24u32), (48, 40), (96, 64)] {
        for amode in 0..4 {
            let info = ImageInfo::new(w, h, Channels::Rgba, SampleType::U8);
            let rgb = if amode == 3 {
                // Few flat colors: palette-friendly DC.
                (0..w * h)
                    .flat_map(|i| {
                        let c = ((i % w) / 8 + (i / w) / 8) % 3;
                        [[200u8, 30, 30], [30, 200, 30], [250, 250, 250]][c as usize]
                    })
                    .collect::<Vec<u8>>()
            } else {
                test_image(w, h, w + h)
            };
            let px: Vec<u8> = rgb
                .chunks(3)
                .enumerate()
                .flat_map(|(i, p)| {
                    let (x, y) = (i as u32 % w, i as u32 / w);
                    let a = match amode {
                        0 => (x * 255 / w) as u8,
                        1 => {
                            if (x / 4 + y / 4) % 2 == 0 {
                                255
                            } else {
                                0
                            }
                        }
                        2 => {
                            if x * x + y * y < w * h / 2 {
                                255
                            } else {
                                0
                            }
                        }
                        _ => 255,
                    };
                    [p[0], p[1], p[2], a]
                })
                .collect();
            for q in [3.0, 20.0, 60.0, 90.0] {
                for e in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                    let mut cfg = LossyConfig::new(q).with_entropy(e);
                    cfg.threads = 1;
                    let (f, cap) = crate::seeds::capture(|| {
                        encode_lossy(&px, w as usize * 4, &info, &Metadata::default(), &cfg)
                            .unwrap()
                    });
                    let mut off = cfg.clone();
                    off.seed_bank = 0;
                    let g = encode_lossy(&px, w as usize * 4, &info, &Metadata::default(), &off)
                        .unwrap();
                    assert!(f.len() <= g.len());
                    assert_eq!(crate::decode(&f).unwrap().1, crate::decode(&g).unwrap().1);
                    let (s, r) = both(&f);
                    assert_eq!(s.unwrap(), r.unwrap(), "SIMD and scalar decodes differ");
                    let (_, sb) = selector(&f).unwrap();
                    alpha_scope += (sb & 0x40 != 0) as u32;
                    for s in cap.seeds {
                        *used.entry((s.role, s.kind, s.tc)).or_default() += 1;
                    }
                }
            }
        }
    }
    // Gray pages and low-contrast noise: DC planes of 14-15 significant bits.
    let mut s = 0x2545_f491u32;
    let lowcon: Vec<u8> = (0..96 * 64)
        .flat_map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let g = (120 + (s % 7) as i32 - 3) as u8;
            [g, g + 1, g, 255]
        })
        .collect();
    let info = ImageInfo::new(96, 64, Channels::Rgba, SampleType::U8);
    for q in [1.0, 2.0, 5.0, 12.0, 30.0, 60.0, 90.0] {
        for color in [LossyColor::Xyb, LossyColor::YCoCg] {
            let mut cfg = LossyConfig::new(q).with_color(color);
            cfg.threads = 1;
            let (f, cap) = crate::seeds::capture(|| {
                encode_lossy(&lowcon, 96 * 4, &info, &Metadata::default(), &cfg).unwrap()
            });
            let mut off = cfg.clone();
            off.seed_bank = 0;
            let g = encode_lossy(&lowcon, 96 * 4, &info, &Metadata::default(), &off).unwrap();
            assert!(f.len() <= g.len());
            assert_eq!(crate::decode(&f).unwrap().1, crate::decode(&g).unwrap().1);
            for s in cap.seeds {
                *used.entry((s.role, s.kind, s.tc)).or_default() += 1;
            }
        }
    }
    // Three flat colors in 24 x 16 blocks: palette DC stripes.
    let flat: Vec<u8> = (0..64u32)
        .flat_map(|y| {
            (0..96u32).flat_map(move |x| {
                [[250u8, 250, 250], [20, 40, 200], [220, 30, 30]][((x / 24 + y / 16) % 3) as usize]
            })
        })
        .collect();
    let info = ImageInfo::new(96, 64, Channels::Rgb, SampleType::U8);
    for q in [5.0, 50.0, 90.0] {
        for e in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
            let mut cfg = LossyConfig::new(q).with_entropy(e);
            cfg.threads = 1;
            let (f, cap) = crate::seeds::capture(|| {
                encode_lossy(&flat, 96 * 3, &info, &Metadata::default(), &cfg).unwrap()
            });
            let mut off = cfg.clone();
            off.seed_bank = 0;
            let g = encode_lossy(&flat, 96 * 3, &info, &Metadata::default(), &off).unwrap();
            assert!(f.len() <= g.len());
            assert_eq!(crate::decode(&f).unwrap().1, crate::decode(&g).unwrap().1);
            for s in cap.seeds {
                *used.entry((s.role, s.kind, s.tc)).or_default() += 1;
            }
        }
    }
    std::eprintln!("alpha scope files {alpha_scope}; seeds chosen (role, kind, tc): {used:?}");
    let any = |pred: &dyn Fn(&(u8, u8, [u8; 3])) -> bool| used.keys().any(pred);
    assert!(alpha_scope > 0, "alpha tables never seeded");
    assert!(any(&|k| k.0 == 2), "no alpha seed recorded");
    assert!(
        any(&|k| k.2 == [5, 2, 8]),
        "palette (5,2,8) seeds never used"
    );
    assert!(any(&|k| k.2 == [4, 2, 15]), "(4,2,15) seeds never used");
    assert!(any(&|k| k.2 == [4, 2, 16]), "(4,2,16) seeds never used");
}

/// U16 input with seeds: same pixels (SIMD and scalar), never larger.
#[test]
fn u16_seeds() {
    use alloc::collections::BTreeMap;
    let mut used: BTreeMap<(u8, u8, [u8; 3]), u32> = BTreeMap::new();
    let mut n = 0;
    for &(w, h) in &[(24u32, 24u32), (64, 48)] {
        for rgba in [false, true] {
            let ch = if rgba { 4 } else { 3 };
            let info = ImageInfo::new(
                w,
                h,
                if rgba { Channels::Rgba } else { Channels::Rgb },
                SampleType::U16,
            );
            let rgb = test_image(w, h, w * 3 + h);
            let px: Vec<u8> = rgb
                .chunks(3)
                .enumerate()
                .flat_map(|(i, p)| {
                    // Wrapping, as release builds always computed it (near
                    // 65535 the offset wraps to dark samples).
                    let mut v: Vec<u16> = p
                        .iter()
                        .map(|&x| (x as u16 * 257).wrapping_add(i as u16 % 13))
                        .collect();
                    if ch == 4 {
                        v.push(((i as u32 % w) * 65535 / w) as u16);
                    }
                    v.into_iter().flat_map(|x| x.to_ne_bytes())
                })
                .collect();
            for q in [3.0, 30.0, 80.0] {
                for e in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                    let mut cfg = LossyConfig::new(q).with_entropy(e);
                    cfg.threads = 1;
                    let mut off = cfg.clone();
                    off.seed_bank = 0;
                    let a = encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), &off)
                        .unwrap();
                    let (b, cap) = crate::seeds::capture(|| {
                        encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), &cfg)
                            .unwrap()
                    });
                    assert!(b.len() <= a.len());
                    assert_eq!(crate::decode(&a).unwrap().1, crate::decode(&b).unwrap().1);
                    let (s, r) = both(&b);
                    assert_eq!(s.unwrap(), r.unwrap());
                    n += 1;
                    for s in cap.seeds {
                        *used.entry((s.role, s.kind, s.tc)).or_default() += 1;
                    }
                }
            }
        }
    }
    std::eprintln!("u16 cells {n}; seeds {used:?}");
    assert!(!used.is_empty(), "no seeds on U16 input");
}
