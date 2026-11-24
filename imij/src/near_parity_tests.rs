//! Near-lossless reconstruction: SIMD kernels against the scalar reference,
//! and the encoder's closed loop against the decoder, value for value; and
//! the head parsers that admit the Scientific features' syntax only when
//! declared.
use super::*;
use crate::Channels;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

#[cfg(target_arch = "x86_64")]
const CHANNELS: [Channels; 4] = [
    Channels::Gray,
    Channels::GrayAlpha,
    Channels::Rgb,
    Channels::Rgba,
];

/// Slot depths and bounds: equal color depths (decorrelation allowed) or
/// mixed ones; some slots exact.
#[cfg(target_arch = "x86_64")]
fn layout_for(ch: Channels, bits: &[u32], near: &[u32]) -> Layout {
    let maps = bits
        .iter()
        .map(|&b| ChannelMap {
            mode: MapMode::Lsb,
            bits: b as u8,
            value: 0,
        })
        .collect();
    Layout::new(ch, maps).with_near(near)
}

#[cfg(target_arch = "x86_64")]
fn parity_case<S: crate::px::Px>(
    r: &mut Rng,
    ch: Channels,
    sample: SampleType,
    max_bits: u32,
) -> usize {
    let nch = ch.count();
    let same = r.below(2) == 0;
    let b0 = 1 + r.below(max_bits as u64) as u32;
    let bits: Vec<u32> = (0..nch)
        .map(|_| {
            if same {
                b0
            } else {
                1 + r.below(max_bits as u64) as u32
            }
        })
        .collect();
    let near: Vec<u32> = bits
        .iter()
        .map(|&b| match r.below(4) {
            0 => 0,
            1 => crate::near::delta_cap(b),
            2 => crate::near::delta_cap(b).min(1),
            _ => (r.next() as u32) % (crate::near::delta_cap(b) + 1),
        })
        .collect();
    let layout = layout_for(ch, &bits, &near);
    if !layout.is_near() {
        return 0;
    }
    let mut compared = 0;
    let ns = layout.slot_channel.len();
    let w = 1 + r.below(150) as usize;
    let lanes = 1 + r.below(16) as usize;
    let lw = w.div_ceil(lanes);
    let widths = lane_widths(w, lw);
    let step_row = lw * NL;
    let info = ImageInfo::new(w as u32, 1, ch, sample);
    let om = OutMap::new(&info, &layout);
    let rand_rows = |r: &mut Rng| -> Vec<Vec<S>> {
        (0..ns)
            .map(|s| {
                let mask = crate::mask_of(layout.slot_bits[s]) as u64;
                (0..step_row)
                    .map(|_| {
                        // Bias toward the range ends to reach the clamps.
                        let v = match r.below(4) {
                            0 => 0,
                            1 => mask,
                            _ => r.next() & mask,
                        };
                        S::put(v as u32)
                    })
                    .collect()
            })
            .collect()
    };
    let res = rand_rows(r);
    let prev = rand_rows(r);
    let decors: &[u8] = if layout.decor_ok { &[0, 1, 2, 3] } else { &[0] };
    for &decor in decors {
        for pred in 0..predict::NUM_PREDICTORS {
            for row in [0usize, 1] {
                if row == 0 && pred != predict::P_W {
                    continue;
                }
                let mut scalar = vec![vec![S::default(); step_row]; ns];
                let mut out = vec![0u8; info.row_bytes()];
                recon_row_scalar(
                    &layout,
                    &res,
                    &prev,
                    &mut scalar,
                    &widths,
                    lw,
                    row,
                    pred,
                    decor,
                    &om,
                    &mut out,
                );
                let mut simd = vec![vec![S::default(); step_row]; ns];
                let ran = match (S::rows16(&res), S::rows16(&prev)) {
                    (Some(rs), Some(pv)) => recon_row_simd(
                        &layout,
                        rs,
                        pv,
                        S::rows16_mut(&mut simd).unwrap(),
                        &widths,
                        lw,
                        row,
                        pred,
                        decor,
                    ),
                    _ => recon_row_simd32(
                        &layout,
                        S::rows32(&res).unwrap(),
                        S::rows32(&prev).unwrap(),
                        S::rows32_mut(&mut simd).unwrap(),
                        &widths,
                        lw,
                        row,
                        pred,
                        decor,
                    ),
                };
                if !ran {
                    // No AVX2 on this CPU (or IMIJ_SCALAR): nothing to compare.
                    return 0;
                }
                compared += 1;
                for s in 0..ns {
                    for (l, &wl) in widths.iter().enumerate() {
                        for x in 0..wl as usize {
                            let i = x * NL + l;
                            assert_eq!(
                                simd[s][i].get(),
                                scalar[s][i].get(),
                                "{ch:?} bits {bits:?} near {near:?} pred {pred} decor {decor} \
                                 row {row} slot {s} lane {l} x {x}"
                            );
                        }
                    }
                }
            }
        }
    }
    compared
}

#[cfg(target_arch = "x86_64")]
#[test]
fn near_recon_simd_matches_scalar() {
    use archmage::SimdToken;
    let mut r = Rng(0x5eed_1234_9876);
    let (mut n16, mut n32) = (0, 0);
    for _ in 0..60 {
        for ch in CHANNELS {
            n16 += parity_case::<u16>(&mut r, ch, SampleType::U16, 16);
            n32 += parity_case::<u32>(&mut r, ch, SampleType::U32, 32);
        }
    }
    #[cfg(feature = "std")]
    std::eprintln!("near recon parity: compared {n16} u16 and {n32} u32 rows");
    if archmage::X64V3Token::summon().is_some() && !force_scalar() {
        assert!(
            n16 > 500 && n32 > 500,
            "compared {n16} u16 and {n32} u32 rows"
        );
    }
}

/// Decoded coded values (per slot, image order) of a lossless file.
fn decoded_coded(file: &[u8], layout: &Layout, info: &ImageInfo) -> Vec<Vec<u32>> {
    let (_, px) = crate::decode(file).unwrap();
    let n = info.channels.count();
    let bps = info.sample.bytes();
    let depth = info.sample.depth();
    layout
        .slot_channel
        .iter()
        .map(|&c| {
            (0..(info.width * info.height) as usize)
                .map(|i| {
                    let raw = read_sample(&px, i * n + c, bps);
                    layout.maps[c].reduce(info.sample.order(raw), depth)
                })
                .collect()
        })
        .collect()
}

#[test]
fn encoder_closed_loop_equals_decoder_output() {
    let mut r = Rng(77);
    let cases = [
        (Channels::Rgb, SampleType::U8, 2u32),
        (Channels::Gray, SampleType::U16, 9),
        (Channels::Rgba, SampleType::U16, 3),
        (Channels::GrayAlpha, SampleType::U32, 1000),
        (Channels::Rgb, SampleType::I32, 70_000),
    ];
    for (ch, st, delta) in cases {
        // Without the `extended` feature the decoder is baseline-only.
        if st.tier() == crate::Tier::Extended && !cfg!(feature = "extended") {
            continue;
        }
        let (w, h) = (73usize, 29usize);
        let info = ImageInfo::new(w as u32, h as u32, ch, st);
        let n = ch.count();
        let bps = st.bytes();
        let mut px = vec![0u8; info.row_bytes() * h];
        let top: u64 = if bps == 4 {
            u32::MAX as u64
        } else {
            (1u64 << (8 * bps)) - 1
        };
        for y in 0..h {
            for x in 0..w {
                for c in 0..n {
                    let t = ((x * 3 + y * 5 + c * 17) % 97) as u64 * top / 97;
                    let v = (t + r.below(top / 50 + 1)).min(top);
                    let o = ((y * w + x) * n + c) * bps;
                    match bps {
                        1 => px[o] = v as u8,
                        2 => px[o..o + 2].copy_from_slice(&(v as u16).to_ne_bytes()),
                        _ => px[o..o + 4].copy_from_slice(&(v as u32).to_ne_bytes()),
                    }
                }
            }
        }
        let (hs, lanes) = (11usize, 5u32);
        let cfg = EncodeConfig {
            stripe_rows: Some(hs as u32),
            lanes: Some(lanes),
            palette: false,
            effort: 10.0,
            ..Default::default()
        }
        .with_max_error(delta);
        let file = crate::encode(&px, info.row_bytes(), &info, &Metadata::default(), &cfg).unwrap();
        let acfg = AnalysisConfig {
            stripe_rows: Some(hs as u32),
            lanes,
            max_error: cfg.max_error,
            ..Default::default()
        };
        let a = analyze(&px, info.row_bytes(), &info, &acfg).unwrap();
        assert!(a.layout.is_near(), "{ch:?} {st:?}");
        let slot_planes: Vec<&[u32]> = a
            .layout
            .slot_channel
            .iter()
            .map(|&c| a.planes[c].as_slice())
            .collect();
        let dec = decoded_coded(&file, &a.layout, &info);
        for y0 in (0..h).step_by(hs) {
            let y1 = (y0 + hs).min(h);
            let mut res = vec![vec![0u32; (y1 - y0) * w]; slot_planes.len()];
            let mut views: Vec<&mut [u32]> = res.iter_mut().map(|v| v.as_mut_slice()).collect();
            let rows: Vec<&[u32]> = slot_planes.iter().map(|p| &p[y0 * w..y1 * w]).collect();
            let (_, rec) = crate::near::analyze_stripe_near_rec(
                &a.layout,
                &rows,
                w,
                a.sw,
                0,
                y1 - y0,
                &acfg,
                &mut views,
            );
            for (s, rs) in rec.iter().enumerate() {
                assert_eq!(
                    &dec[s][y0 * w..y1 * w],
                    &rs[..],
                    "{ch:?} {st:?} slot {s} stripe at {y0}"
                );
            }
        }
    }
}

#[test]
fn scientific_heads_need_the_declaration_and_nested_heads_never_take_it() {
    let unrestricted = &crate::parse::limits::Unrestricted;
    let gray = ImageInfo::new(64, 16, Channels::Gray, SampleType::U8);
    let px: Vec<u8> = (0..64 * 16).map(|i| (i * 37 % 251) as u8).collect();
    let near_cfg = EncodeConfig {
        palette: false,
        ..EncodeConfig::default().with_max_error(2)
    };
    let near = crate::encode(&px, 64, &gray, &Metadata::default(), &near_cfg).unwrap();
    let half = ImageInfo::new(64, 16, Channels::Gray, SampleType::F16);
    let fpx: Vec<u8> = (0u16..64 * 16)
        .flat_map(|x| ((x % 512) * 128).to_ne_bytes())
        .collect();
    let float = crate::encode(
        &fpx,
        128,
        &half,
        &Metadata::default(),
        &EncodeConfig::default(),
    )
    .unwrap();
    use crate::parse::header::{Feature, Features};
    for (file, feature, why) in [
        (
            &near,
            Feature::NearLossless,
            "near-lossless payload without the feature near_lossless",
        ),
        (
            &float,
            Feature::FloatShift,
            "float map without the feature float_shift",
        ),
    ] {
        let f = parse_file(file, unrestricted, crate::checksum::Verify::Now(1)).unwrap();
        assert_eq!(f.declared, Features::EMPTY.with(feature));
        let (info, body) = (f.header.info, f.data);
        let head = |d| {
            parse_head_checked(info.clone(), &mut ByteReader::new(body), unrestricted, d)
                .map(|_| ())
        };
        assert_eq!(head(Features::EMPTY.with(feature)), Ok(()));
        assert_eq!(head(Features::EMPTY), Err(Error::Corrupt(why)));
        // Nested images of lossy payloads never take them, whatever the
        // outer file declares (SPEC 2.8).
        let nested = parse_head_nested(
            info.clone(),
            &mut ByteReader::new(body),
            unrestricted,
            None,
            Features::ALL,
        );
        assert_eq!(nested.map(|_| ()), Err(Error::Corrupt(why)));
    }
}

#[test]
fn version_two_heads_need_a_sample_type_with_bounds() {
    // Near-lossless heads of u8 and u16 files read as i8, i16 or 64-bit
    // samples: Corrupt, never a misread or a panic.
    let unrestricted = &crate::parse::limits::Unrestricted;
    let cfg = EncodeConfig {
        palette: false,
        ..EncodeConfig::default().with_max_error(3)
    };
    let cases = [
        (SampleType::U8, SampleType::I8),
        (SampleType::U16, SampleType::I16),
    ];
    for (st, other) in cases {
        let info = ImageInfo::new(32, 8, Channels::Gray, st);
        let px: Vec<u8> = (0..info.row_bytes() * 8)
            .map(|i| (i * 37 % 251) as u8)
            .collect();
        let file = crate::encode(&px, info.row_bytes(), &info, &Metadata::default(), &cfg).unwrap();
        let f = parse_file(&file, unrestricted, crate::checksum::Verify::Now(1)).unwrap();
        // A record list carries the bounds (SPEC 2).
        assert_eq!(f.data[0] as u64 & H_RECORDS, H_RECORDS);
        let read = |sample, decoding| {
            let info = ImageInfo::new(32, 8, Channels::Gray, sample);
            let sci = crate::parse::header::Features::EMPTY
                .with(crate::parse::header::Feature::NearLossless);
            if decoding {
                parse_head_checked(info, &mut ByteReader::new(f.data), unrestricted, sci)
            } else {
                parse_head_checked(
                    info,
                    &mut ByteReader::new(f.data),
                    &crate::parse::limits::Structure,
                    sci,
                )
            }
            .map(|_| ())
        };
        assert_eq!(read(st, true), Ok(()));
        assert_eq!(
            read(other, false),
            Err(Error::Corrupt(
                "near-lossless payload for a sample type without bounds"
            ))
        );
        assert_eq!(
            read(other, true),
            if cfg!(feature = "extended") {
                Err(Error::Corrupt(
                    "near-lossless payload for a sample type without bounds",
                ))
            } else {
                Err(Error::Unsupported("signed samples (extended tier)"))
            }
        );
        for wide in [SampleType::U64, SampleType::I64, SampleType::F64] {
            assert!(read(wide, true).is_err(), "{st:?} as {wide:?}");
        }
    }
}
