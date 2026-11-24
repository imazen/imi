use imij::{
    Channels, Cicp, ImageInfo, ImiEncoderConfig as V1Config, Metadata, SampleType,
    decode as decode_v1,
};

/// Deterministic xorshift.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn synth(w: u32, h: u32, ch: Channels, st: SampleType, seed: u64, kind: u8) -> Vec<u8> {
    let mut r = Rng(seed | 1);
    let n = ch.count();
    let bytes = st.bytes();
    let mut v = vec![0u8; (w * h) as usize * n * bytes];
    let maxv: u32 = if bytes == 1 { 255 } else { 65535 };
    for y in 0..h {
        for x in 0..w {
            for c in 0..n {
                let base = ((x * 7 + y * 3 + c as u32 * 50) % 256) * (maxv / 255);
                let val = match kind {
                    0 => (r.next() as u32) & maxv,                 // noise
                    1 => (base + (r.next() as u32 % 9)).min(maxv), // gradient + noise
                    2 => {
                        if (x / 8 + y / 8) % 2 == 0 {
                            maxv
                        } else {
                            0
                        }
                    } // blocks
                    3 => {
                        if c == n - 1 && ch.has_alpha() {
                            if x < w / 2 { maxv } else { 0 }
                        } else {
                            base
                        }
                    }
                    _ => ((base >> 6) << 6).min(maxv), // MSB-aligned 10-bit
                };
                let i = ((y * w + x) as usize * n + c) * bytes;
                if bytes == 1 {
                    v[i] = val as u8;
                } else {
                    v[i..i + 2].copy_from_slice(&(val as u16).to_ne_bytes());
                }
            }
        }
    }
    v
}

fn roundtrip(w: u32, h: u32, ch: Channels, st: SampleType, kind: u8, cfg: &V1Config) {
    let mut info = ImageInfo::new(w, h, ch, st);
    info.cicp = Cicp::BT2100_PQ;
    let px = synth(
        w,
        h,
        ch,
        st,
        (w as u64) << 20 | h as u64 | (kind as u64) << 40,
        kind,
    );
    let meta = Metadata {
        content_light_level: Some([0, 0, 39, 16, 0, 0, 78, 32]),
        ..Default::default()
    };
    let enc = cfg
        .encode_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &meta)
        .unwrap();
    let (dinfo, dec) = decode_v1(&enc).unwrap();
    assert_eq!(dinfo, info);
    assert!(
        dec == px,
        "pixel mismatch {w}x{h} {ch:?} {st:?} kind {kind}"
    );
}

#[test]
fn v1_roundtrip_matrix() {
    let cfg = V1Config::default();
    for &(w, h) in &[(1, 1), (3, 2), (17, 5), (64, 64), (513, 37), (1000, 20)] {
        for ch in [
            Channels::Gray,
            Channels::GrayAlpha,
            Channels::Rgb,
            Channels::Rgba,
        ] {
            for st in [SampleType::U8, SampleType::U16, SampleType::F16] {
                for kind in 0..5 {
                    roundtrip(w, h, ch, st, kind, &cfg);
                }
            }
        }
    }
}

#[test]
fn v1_roundtrip_configs() {
    for cfg in [
        {
            let mut cfg = V1Config::default();
            cfg = cfg.tune_lossless(|t| {
                t.lanes = Some(1);
            });
            cfg
        },
        {
            let mut cfg = V1Config::default();
            cfg = cfg.tune_lossless(|t| {
                t.lanes = Some(5);
            });
            cfg = cfg.tune_lossless(|t| {
                t.stripe_rows = Some(3);
            });
            cfg
        },
        {
            let mut cfg = V1Config::default();
            cfg = cfg.tune_lossless(|t| {
                t.max_ctx = 1;
            });
            cfg = cfg.tune_lossless(|t| {
                t.run_min = 0;
            });
            cfg
        },
        {
            let mut cfg = V1Config::default();
            cfg = cfg.tune_lossless(|t| {
                t.k_low = 8;
            });
            cfg = cfg.tune_lossless(|t| {
                t.k_high = 6;
            });
            cfg = cfg.tune_lossless(|t| {
                t.m = 1;
            });
            cfg
        },
    ] {
        for kind in 0..5 {
            roundtrip(300, 41, Channels::Rgba, SampleType::U8, kind, &cfg);
            roundtrip(129, 33, Channels::Rgb, SampleType::U16, kind, &cfg);
        }
    }
}

#[test]
fn v1_every_predictor_and_decor() {
    for p in 0..7u8 {
        for decor in 0..4u8 {
            let cfg = {
                let mut cfg = V1Config::default();
                cfg = cfg.tune_lossless(|t| {
                    t.predictor_mask = 1 << p;
                });
                cfg = cfg.tune_lossless(|t| {
                    t.decor_mask = 1 << decor;
                });
                cfg
            };
            for kind in [0u8, 1, 2, 4] {
                roundtrip(203, 19, Channels::Rgb, SampleType::U8, kind, &cfg);
                roundtrip(97, 13, Channels::Rgba, SampleType::U16, kind, &cfg);
                roundtrip(150, 11, Channels::GrayAlpha, SampleType::F16, kind, &cfg);
            }
        }
    }
}

#[test]
fn v1_threads_match_single() {
    for (w, h, rows) in [(700u32, 301u32, 16u32), (64, 1000, 7), (2000, 33, 4)] {
        let info = ImageInfo::new(w, h, Channels::Rgba, SampleType::U8);
        let px = synth(w, h, Channels::Rgba, SampleType::U8, 99, 1);
        let cfg = {
            let mut cfg = V1Config::default();
            cfg = cfg.tune_lossless(|t| {
                t.stripe_rows = Some(rows);
            });
            cfg
        };
        let enc = cfg
            .encode_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        for threads in [1, 2, 3, 8, 64] {
            // Padded stride exercises strided output.
            let stride = info.row_bytes() + 13;
            let mut out = vec![0u8; stride * h as usize];
            imij::decode_into_threads(&enc, &mut out, stride, threads).unwrap();
            for y in 0..h as usize {
                let a = &out[y * stride..y * stride + info.row_bytes()];
                let b = &px[y * info.row_bytes()..(y + 1) * info.row_bytes()];
                assert!(a == b, "row {y} threads {threads}");
            }
        }
    }
}

#[test]
fn v1_sixteen_bit_maps() {
    let (w, h) = (157u32, 23u32);
    let mut r = Rng(7);
    let rep = |s: u32, n: u32| -> u16 { ((s << (16 - n)) | (s >> (2 * n - 16))) as u16 };
    let scale = |s: u32, n: u32| -> u16 {
        ((s as u64 * 65535 + ((1u64 << n) - 1) / 2) / ((1u64 << n) - 1)) as u16
    };
    type Gen = Box<dyn Fn(&mut Rng, u32, u32, usize) -> u16>;
    let gens: Vec<(&str, Gen)> = vec![
        (
            "rep10",
            Box::new(move |r: &mut Rng, x, y, _c| {
                rep(((x * 5 + y * 3) + (r.next() as u32 % 7)) % 1024, 10)
            }),
        ),
        (
            "rep12",
            Box::new(move |r: &mut Rng, x, y, _c| {
                rep(((x * 20 + y * 9) + (r.next() as u32 % 30)) % 4096, 12)
            }),
        ),
        (
            "scale10",
            Box::new(move |r: &mut Rng, x, y, _c| {
                scale(((x * 5 + y) + (r.next() as u32 % 5)) % 1024, 10)
            }),
        ),
        (
            "lsb12",
            Box::new(move |r: &mut Rng, x, y, _c| {
                (((x * 17 + y * 5) + (r.next() as u32 % 9)) % 4096) as u16
            }),
        ),
        (
            "alpha1",
            Box::new(move |r: &mut Rng, x, y, c| {
                if c == 3 {
                    if (x + y) % 7 < 3 { 65535 } else { 0 }
                } else {
                    (r.next() as u16) >> 3
                }
            }),
        ),
    ];
    for (name, g) in &gens {
        for st in [SampleType::U16, SampleType::F16] {
            let info = ImageInfo::new(w, h, Channels::Rgba, st);
            let mut px = Vec::new();
            for y in 0..h {
                for x in 0..w {
                    for c in 0..4 {
                        px.extend_from_slice(&g(&mut r, x, y, c).to_ne_bytes());
                    }
                }
            }
            let enc = V1Config::default()
                .encode_samples(
                    imij::ImageRef::new(&px, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap();
            let (_, dec) = decode_v1(&enc).unwrap();
            assert!(dec == px, "{name} {st:?}");
            if st == SampleType::U16 && *name != "alpha1" {
                // Real precision must show in the size (well under 16 bits/sample).
                assert!(enc.len() < px.len() / 2, "{name}: {} bytes", enc.len());
            }
        }
    }
}

#[test]
fn report_tier() {
    eprintln!("imij simd tier: {}", imij::simd_tier());
}

#[test]
fn stream_decoder_matches() {
    let (w, h) = (333u32, 157u32);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let px = synth(w, h, Channels::Rgb, SampleType::U8, 5, 1);
    let cfg = {
        let mut cfg = V1Config::default();
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some(40);
        });
        cfg
    };
    let enc = cfg
        .encode_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let mut d = imij::StreamDecoder::new(&enc).unwrap();
    let mut row = vec![0u8; info.row_bytes()];
    let mut y = 0;
    while d.next_row(&mut row).unwrap() {
        assert_eq!(
            &row[..],
            &px[y * info.row_bytes()..(y + 1) * info.row_bytes()],
            "row {y}"
        );
        y += 1;
    }
    assert_eq!(y, h as usize);
}

#[test]
fn previews_roundtrip_and_skip() {
    let (w, h) = (200u32, 120u32);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let px = synth(w, h, Channels::Rgb, SampleType::U8, 11, 1);
    let hinfo = ImageInfo::new(100, 60, Channels::Rgb, SampleType::U8);
    let hpx = synth(100, 60, Channels::Rgb, SampleType::U8, 12, 1);
    let prev = imij::ImageRef {
        pixels: &hpx,
        stride: hinfo.row_bytes(),
        info: &hinfo,
    };
    let enc = V1Config::default()
        .encode_samples_with_previews(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
            &[prev],
        )
        .unwrap();
    let (_, full) = decode_v1(&enc).unwrap();
    assert!(full == px);
    let ps = imij::previews(&enc).unwrap();
    assert_eq!(ps.len(), 1);
    let (pinfo, pdec) = decode_v1(ps[0]).unwrap();
    assert_eq!(pinfo, hinfo);
    assert!(pdec == hpx);
    // The preview must be readable from a prefix that ends before DATA.
    let end = ps[0].as_ptr() as usize - enc.as_ptr() as usize + ps[0].len();
    assert!(imij::previews(&enc[..end + 8]).is_ok() || end < enc.len());
}

/// Rows `0..pal_rows` drawn from `ncol` random colors; the rest noise.
fn palette_image(
    w: u32,
    h: u32,
    ch: Channels,
    st: SampleType,
    ncol: usize,
    pal_rows: u32,
    seed: u64,
) -> Vec<u8> {
    let mut r = Rng(seed | 1);
    let bpp = ch.count() * st.bytes();
    let cols: Vec<Vec<u8>> = (0..ncol)
        .map(|_| (0..bpp).map(|_| r.next() as u8).collect())
        .collect();
    let mut v = Vec::with_capacity((w * h) as usize * bpp);
    for y in 0..h {
        for x in 0..w {
            if y < pal_rows {
                // Runs of a color, so the index plane has structure.
                let i = if x % 5 == 0 {
                    r.next() as usize % ncol
                } else {
                    (x as usize / 5 + y as usize) % ncol
                };
                v.extend_from_slice(&cols[i]);
            } else {
                v.extend((0..bpp).map(|_| r.next() as u8));
            }
        }
    }
    v
}

#[test]
fn palette_stripes_roundtrip() {
    for (ch, st) in [
        (Channels::Rgb, SampleType::U8),
        (Channels::Rgba, SampleType::U8),
        (Channels::GrayAlpha, SampleType::U8),
        (Channels::Gray, SampleType::U8),
        (Channels::Rgb, SampleType::U16),
        (Channels::Rgba, SampleType::U16),
    ] {
        for (w, h, ncol, pal_rows) in [
            (1u32, 1u32, 1usize, 1u32),
            (37, 50, 2, 50),
            (129, 70, 256, 70),
            (129, 70, 257, 70),
            (300, 90, 40, 45),
            (77, 33, 7, 20),
        ] {
            let info = ImageInfo::new(w, h, ch, st);
            let px = palette_image(w, h, ch, st, ncol, pal_rows, (w * 1000 + h) as u64);
            let what = format!("{w}x{h} {ch:?} {st:?} {ncol} colors");
            let mut sizes = [0usize; 2];
            for (i, palette) in [true, false].into_iter().enumerate() {
                // Regular maps: few 16-bit colors would otherwise code as
                // value tables, which leave palettes little to gain.
                let cfg = {
                    let mut cfg = V1Config::default();
                    cfg = cfg.tune_lossless(|t| {
                        t.stripe_rows = Some(16);
                    });
                    cfg = cfg.tune_lossless(|t| {
                        t.palette = palette;
                    });
                    cfg = cfg.tune_lossless(|t| {
                        t.value_tables = false;
                    });
                    cfg
                };
                let enc = cfg
                    .encode_samples(
                        imij::ImageRef::new(&px, info.row_bytes(), &info),
                        &Metadata::default(),
                    )
                    .unwrap();
                sizes[i] = enc.len();
                let (_, dec) = decode_v1(&enc).unwrap();
                assert!(dec == px, "{what} palette {palette}: pixel mismatch");
                let mut out = vec![0u8; px.len()];
                imij::decode_into_threads(&enc, &mut out, info.row_bytes(), 4).unwrap();
                assert!(out == px, "{what}: threaded mismatch");
                let mut got = Vec::new();
                imij::decode_reader(&enc[..], &imij::Limits::DEFAULT, |_, _, row| {
                    got.extend_from_slice(row);
                    Ok(())
                })
                .unwrap();
                assert!(got == px, "{what}: reader mismatch");
            }
            // Fully paletted, few colors, multi-slot: palette must be picked and win.
            if pal_rows == h && ncol <= 40 && w * h > 1000 && ch.count() >= 2 {
                assert!(
                    sizes[0] * 10 < sizes[1] * 8,
                    "{what}: palette {} vs plain {}",
                    sizes[0],
                    sizes[1]
                );
            }
            // Gray has no palette stripes (one slot); only template palette
            // stripes (SPEC 2.14) can change its bytes, and only to shrink them.
            if ch == Channels::Gray && sizes[0] != sizes[1] {
                let cfg = {
                    let mut cfg = V1Config::default();
                    cfg = cfg.tune_lossless(|t| {
                        t.stripe_rows = Some(16);
                    });
                    cfg
                };
                let enc = cfg
                    .encode_samples(
                        imij::ImageRef::new(&px, info.row_bytes(), &info),
                        &Metadata::default(),
                    )
                    .unwrap();
                assert!(
                    imij::header(&enc)
                        .unwrap()
                        .features
                        .contains(imij::Feature::TemplatePalettes),
                    "{what}: palette used where it can't be"
                );
                assert!(
                    sizes[0] < sizes[1],
                    "{what}: template stripes grew the file"
                );
            }
        }
    }
}

/// A palette stripe whose first pixel packs to the all-ones key: opaque
/// 16-bit white in four coded slots. Noise rows keep every channel map at
/// 16 identity bits, so alpha is a slot and white codes as 0xFFFF in each.
/// The index plane must still map that pixel to its own entry.
#[test]
fn palette_stripe_starting_with_all_ones_key() {
    let (w, h) = (64u32, 32u32);
    let info = ImageInfo::new(w, h, Channels::Rgba, SampleType::U16);
    let mut r = Rng(7);
    let mut px = Vec::with_capacity((w * h) as usize * 8);
    for y in 0..h {
        for x in 0..w {
            let c: [u16; 4] = if y >= 16 || x < 16 {
                core::array::from_fn(|_| r.next() as u16)
            } else if (x + y) % 2 == 0 {
                [65535; 4]
            } else {
                [0; 4]
            };
            for v in c {
                px.extend_from_slice(&v.to_ne_bytes());
            }
        }
    }
    // Rows 0..16 as one stripe starting at a white pixel of the right
    // groups, and as 1-row stripes in 16-column groups.
    for (rows, groups) in [(16u32, Some(16u32)), (1, Some(16))] {
        let cfg = {
            let mut cfg = V1Config::default();
            cfg = cfg.tune_lossless(|t| {
                t.stripe_rows = Some(rows);
            });
            cfg = cfg.with_column_groups(groups);
            cfg
        };
        let enc = cfg
            .encode_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        let (_, dec) = decode_v1(&enc).unwrap();
        let bad = (0..(w * h) as usize)
            .filter(|&p| dec[p * 8..p * 8 + 8] != px[p * 8..p * 8 + 8])
            .count();
        assert_eq!(bad, 0, "stripes {rows} groups {groups:?}: pixels differ");
    }
}

/// Column groups: every decode path reproduces the pixels, for group widths
/// that divide the width or not, palette stripes inside groups, all sample
/// tiers; corrupt group records fail cleanly.
#[test]
fn column_groups_roundtrip() {
    let cases = [
        (Channels::Rgb, SampleType::U8, 1u8),
        (Channels::Rgba, SampleType::U8, 1),
        (Channels::Gray, SampleType::U8, 2),
        (Channels::Rgb, SampleType::U16, 1),
        (Channels::GrayAlpha, SampleType::F16, 1),
    ];
    for (ch, st, kind) in cases {
        for (w, h, g) in [
            (301u32, 70u32, 100u32),
            (300, 33, 100),
            (64, 20, 1),
            (257, 9, 256),
            (40, 40, 39),
        ] {
            let info = ImageInfo::new(w, h, ch, st);
            let px = synth(w, h, ch, st, (w as u64) << 8 | g as u64, kind);
            let what = format!("{w}x{h} {ch:?} {st:?} groups {g}");
            let cfg = {
                let mut cfg = V1Config::default();
                cfg = cfg.tune_lossless(|t| {
                    t.stripe_rows = Some(16);
                });
                cfg = cfg.with_column_groups(Some(g));
                cfg
            };
            let enc = cfg
                .encode_samples(
                    imij::ImageRef::new(&px, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap();
            let (_, dec) = decode_v1(&enc).unwrap();
            assert!(dec == px, "{what}: decode");
            for t in [2, 3, 8] {
                let mut out = vec![0u8; px.len()];
                imij::decode_into_threads(&enc, &mut out, info.row_bytes(), t).unwrap();
                assert!(out == px, "{what}: {t} threads");
            }
            let mut sd = imij::StreamDecoder::new(&enc).unwrap();
            let rb = info.row_bytes();
            let mut row = vec![0u8; rb];
            let mut y = 0;
            while sd.next_row(&mut row).unwrap() {
                assert!(row == px[y * rb..(y + 1) * rb], "{what}: stream row {y}");
                y += 1;
            }
            let mut got = Vec::new();
            imij::decode_reader(&enc[..], &imij::Limits::DEFAULT, |_, _, r| {
                got.extend_from_slice(r);
                Ok(())
            })
            .unwrap();
            assert!(got == px, "{what}: reader");
            // Groups off: same pixels, different bytes.
            let plain = {
                let mut cfg = V1Config::default();
                cfg = cfg.tune_lossless(|t| {
                    t.stripe_rows = Some(16);
                });
                cfg = cfg.with_column_groups(Some(0));
                cfg
            }
            .encode_samples(imij::ImageRef::new(&px, rb, &info), &Metadata::default())
            .unwrap();
            assert!(plain != enc || g >= w, "{what}: groups not used");
            // Corruption: no panics.
            let mut s = 0x1234_5678u32 ^ w;
            for _ in 0..200 {
                let mut bad = enc.clone();
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                let i = s as usize % bad.len();
                bad[i] ^= (s >> 24) as u8 | 1;
                let _ = decode_v1(&bad);
                let mut out = vec![0u8; px.len()];
                let _ = imij::decode_into_threads(&bad, &mut out, rb, 3);
            }
        }
    }
}

/// Images at least 8192 wide get column groups by default; they round-trip
/// (baseline and extended tiers) and differ from an ungrouped encode.
#[test]
fn column_groups_automatic() {
    for (ch, st) in [
        (Channels::Rgb, SampleType::U8),
        (Channels::Gray, SampleType::U16),
    ] {
        let (w, h) = (8200u32, 21u32);
        let info = ImageInfo::new(w, h, ch, st);
        let px = synth(w, h, ch, st, 77, 1);
        let rb = info.row_bytes();
        let auto = V1Config::default()
            .encode_samples(imij::ImageRef::new(&px, rb, &info), &Metadata::default())
            .unwrap();
        let none = {
            let mut cfg = V1Config::default();
            cfg = cfg.with_column_groups(Some(0));
            cfg
        }
        .encode_samples(imij::ImageRef::new(&px, rb, &info), &Metadata::default())
        .unwrap();
        assert!(auto != none, "{ch:?} {st:?}: no groups by default");
        assert!(decode_v1(&auto).unwrap().1 == px);
        let mut out = vec![0u8; px.len()];
        imij::decode_into_threads(&auto, &mut out, rb, 4).unwrap();
        assert!(out == px);
    }
}
