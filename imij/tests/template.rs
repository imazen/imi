//! Template palette stripes (SPEC 2.14, feature `template_palettes`): the
//! encoder chooses them for low-color images where they are smaller, files
//! declare the feature and the Bilevel 0.2 profile, and every decode route
//! returns the source pixels.

mod common;

use common::{Rng, data_payload, decode_all, feature_mask, payload, with_feature_mask};
use imij::decode_resources::{DecodePolicy, DecodeSession};
use imij::planes::Rect;
use imij::region::decode_region_threads;
use imij::{
    Channels, Error, Feature, FrameConfig, FrameDecoder, FrameEncoder, ImageInfo, ImiEncoderConfig,
    Metadata, Profile, ProfileVersion, SampleType, decode, header,
};

use common::feature::TEMPLATE_PALETTES as TEMPLATE;

const BILEVEL_0_2: ProfileVersion = ProfileVersion {
    profile: Profile::Bilevel,
    major: 0,
    minor: 2,
};

/// Map-like indices: blocky regions with ragged edges, specks and repeated
/// rows; `n` colors. Rows from `two` on (when set) use only indices 1 and 2.
fn indices(w: usize, h: usize, n: usize, seed: u64, two: Option<usize>) -> Vec<usize> {
    let mut r = Rng(seed | 1);
    let mut v = vec![0usize; w * h];
    for y in 0..h {
        for x in 0..w {
            let s = r.next();
            let mut c = ((x + (s % 3) as usize) / 9 + 3 * ((y + (s >> 9) as usize % 2) / 7)) % n;
            if s.is_multiple_of(41) {
                c = (s >> 20) as usize % n;
            }
            if two.is_some_and(|t| y >= t) {
                c = 1 + (x / 5 + y / 3) % 2;
            }
            v[y * w + x] = c;
        }
        if y > 0 && r.next().is_multiple_of(4) {
            v.copy_within((y - 1) * w..y * w, y * w);
        }
    }
    v
}

fn gray8(idx: &[usize]) -> Vec<u8> {
    idx.iter().map(|&i| (i * 23 + 7) as u8).collect()
}

fn rgb8(idx: &[usize]) -> Vec<u8> {
    idx.iter()
        .flat_map(|&i| [(i * 6) as u8, (255 - i * 5) as u8, (i * i % 251) as u8])
        .collect()
}

fn enc(px: &[u8], info: &ImageInfo, cfg: &ImiEncoderConfig) -> Vec<u8> {
    // These tests inspect/mutate V3 record bytes. Keep that wire oracle and
    // independently require its positive files to retain V4 pixel parity.
    let file = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(px, info.row_bytes(), info),
            &Metadata::default(),
        )
        .unwrap();
    common::assert_current_migration_pixels(&file);
    file
}

/// The layout byte of every unit record of `file`.
fn layouts(file: &[u8], info: &ImageInfo) -> Vec<u8> {
    let d = data_payload(file);
    let p = payload(&file[d.clone()], info, false);
    p.records()
        .iter()
        .map(|r| file[d.start + r.start])
        .collect()
}

/// Template palette stripes chosen, declared and decoded through every
/// whole-image route; returns the file.
fn check(px: &[u8], info: &ImageInfo, cfg: &ImiEncoderConfig) -> Vec<u8> {
    let file = enc(px, info, cfg);
    let h = header(&file).unwrap();
    assert!(h.features.contains(Feature::TemplatePalettes), "{info:?}");
    assert!(h.profiles.contains(&BILEVEL_0_2), "{:?}", h.profiles);
    assert!(decode_all(&file, info) == px, "{info:?} {cfg:?}");
    let kinds = layouts(&file, info);
    assert!(kinds.iter().any(|k| [0x83, 0x84].contains(k)), "{kinds:?}");
    file
}

#[test]
fn shared_and_own_entries_decode_on_every_route() {
    let (w, h) = (300usize, 200usize);
    let info = ImageInfo::new(w as u32, h as u32, Channels::Gray, SampleType::U8);
    // Nine colors, the bottom stripes two: shared entries above, the
    // two-entry rule's own entries below.
    let px = gray8(&indices(w, h, 9, 1, Some(128)));
    let cfg = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some(32);
        });
        cfg
    };
    let file = check(&px, &info, &cfg);
    let kinds = layouts(&file, &info);
    assert!(kinds[..4].iter().all(|&k| k == 0x84), "{kinds:?}");
    assert!(kinds[4..].iter().all(|&k| k == 0x83), "{kinds:?}");
    // The tables are head record 35.
    let d = data_payload(&file);
    let p = payload(&file[d.clone()], &info, false);
    assert!(p.head_record(35).is_some());
    assert_ne!(feature_mask(&file) & TEMPLATE, 0);
}

#[test]
fn sample_types_channels_and_wide_entries() {
    let (w, h) = (230usize, 150usize);
    let idx = indices(w, h, 12, 2, None);
    let cases: Vec<(Channels, SampleType, Vec<u8>)> = vec![
        (Channels::Gray, SampleType::U8, gray8(&idx)),
        (Channels::Rgb, SampleType::U8, rgb8(&idx)),
        (
            Channels::Rgba,
            SampleType::U8,
            idx.iter()
                .flat_map(|&i| {
                    [
                        (i * 9) as u8,
                        40,
                        (200 - i) as u8,
                        if i == 0 { 0 } else { 255 },
                    ]
                })
                .collect(),
        ),
        (
            Channels::GrayAlpha,
            SampleType::U16,
            idx.iter()
                .flat_map(|&i| {
                    let g = (i as u16 * 5_003).to_ne_bytes();
                    let a = if i < 2 { 0u16 } else { 65_535 }.to_ne_bytes();
                    [g[0], g[1], a[0], a[1]]
                })
                .collect(),
        ),
        (
            Channels::Rgb,
            SampleType::U16,
            idx.iter()
                .flat_map(|&i| {
                    [i as u16 * 4_001, 65_535 - i as u16, 7]
                        .iter()
                        .flat_map(|c| c.to_ne_bytes())
                        .collect::<Vec<u8>>()
                })
                .collect(),
        ),
    ];
    for (ch, sample, px) in cases {
        let info = ImageInfo::new(w as u32, h as u32, ch, sample);
        check(&px, &info, &imij::ImiEncoderConfig::default());
    }
    // Declared significant bits: four bits of eight.
    let info = ImageInfo::new(w as u32, h as u32, Channels::Gray, SampleType::U8)
        .with_significant_bits(&[4])
        .unwrap();
    let px: Vec<u8> = idx.iter().map(|&i| i as u8).collect();
    check(&px, &info, &imij::ImiEncoderConfig::default());
}

#[test]
fn geometry_lanes_stripes_column_groups_and_regions() {
    for &(w, h) in &[(64usize, 300usize), (97, 220), (500, 90)] {
        let px = rgb8(&indices(w, h, 17, (w + 3 * h) as u64, None));
        let info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8);
        for (lanes, rows, groups) in [
            (Some(1), Some(5), None),
            (Some(3), Some(7), Some(40)),
            (Some(16), Some(1), None),
            (None, Some(2), Some(16)),
            (None, None, Some(33)),
        ] {
            let cfg = {
                let mut cfg = imij::ImiEncoderConfig::default();
                cfg = cfg.tune_lossless(|t| {
                    t.lanes = lanes;
                });
                cfg = cfg.tune_lossless(|t| {
                    t.stripe_rows = rows;
                });
                cfg = cfg.with_column_groups(groups);
                cfg
            };
            let file = check(&px, &info, &cfg);
            for (x, y, rw, rh) in [(0, 0, w, h), (5, 3, w / 2, h / 3), (w - 7, h - 2, 7, 2)] {
                let r = Rect::new(x as u32, y as u32, rw as u32, rh as u32);
                let mut got = vec![0u8; rw * rh * 3];
                decode_region_threads(&file, r, &mut got, rw * 3, 3).unwrap();
                for yy in 0..rh {
                    assert_eq!(
                        got[yy * rw * 3..(yy + 1) * rw * 3],
                        px[((y + yy) * w + x) * 3..((y + yy) * w + x + rw) * 3],
                        "{w}x{h} {cfg:?} region {r:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn more_than_256_colors_use_own_entries_per_unit() {
    let (w, h) = (230usize, 150usize);
    let info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8);
    let px: Vec<u8> = indices(w, h, 100, 4, None)
        .iter()
        .enumerate()
        .flat_map(|(p, &i)| [i as u8, (p / w / 64) as u8, (i * 3) as u8])
        .collect();
    let file = check(&px, &info, &imij::ImiEncoderConfig::default());
    assert!(!layouts(&file, &info).contains(&0x84));
    // Without shared entries in record 35, a shared-entry stripe is corrupt.
    let d = data_payload(&file);
    let u = payload(&file[d.clone()], &info, false).records()[0].clone();
    let mut f = file.clone();
    assert_eq!(f[d.start + u.start], 0x83);
    f[d.start + u.start] = 0x84;
    assert_eq!(
        decode(&f).map(|_| ()),
        Err(Error::Corrupt(
            "shared template palette stripe without shared entries"
        ))
    );
}

#[test]
fn pages_and_animation_frames_declare_the_feature() {
    let info = ImageInfo::new(160, 120, Channels::Rgb, SampleType::U8);
    let cfg = FrameConfig::new(imij::ImiEncoderConfig::default());
    let mut pages = FrameEncoder::pages(info.clone(), Metadata::default(), cfg.clone());
    let mut srcs = Vec::new();
    for s in 0..3u64 {
        let px = rgb8(&indices(160, 120, 11, s + 5, None));
        pages.push_page(&px, 160 * 3, &info).unwrap();
        srcs.push(px);
    }
    let file = pages.finish().unwrap();
    assert!(
        header(&file)
            .unwrap()
            .features
            .contains(Feature::TemplatePalettes)
    );
    let mut d = FrameDecoder::new(&file).unwrap();
    let mut out = vec![0u8; 160 * 120 * 3];
    for want in &srcs {
        assert!(d.next_frame().unwrap().is_some());
        d.decode_into(&mut out, 160 * 3, 1).unwrap();
        assert!(&out == want);
    }
    assert!(d.next_frame().unwrap().is_none());
}

#[test]
fn files_without_the_feature_are_corrupt() {
    let info = ImageInfo::new(300, 200, Channels::Gray, SampleType::U8);
    let file = check(
        &gray8(&indices(300, 200, 9, 7, None)),
        &info,
        &imij::ImiEncoderConfig::default(),
    );
    let mask = feature_mask(&file);
    for f in [
        with_feature_mask(&file, None),
        with_feature_mask(&file, Some(mask & !TEMPLATE)),
    ] {
        assert_eq!(
            decode(&f).map(|_| ()),
            Err(Error::Corrupt(
                "template tables without the feature template_palettes"
            ))
        );
    }
}

#[test]
fn stripe_mutations_are_corrupt() {
    let (w, h) = (300usize, 200usize);
    let info = ImageInfo::new(w as u32, h as u32, Channels::Gray, SampleType::U8);
    let cfg = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some(32);
        });
        cfg
    };
    let file = check(&gray8(&indices(w, h, 9, 1, Some(128))), &info, &cfg);
    let d = data_payload(&file);
    let p = payload(&file[d.clone()], &info, false);
    let units = p.records();
    let own = units
        .iter()
        .find(|u| file[d.start + u.start] == 0x83)
        .unwrap();
    // An unknown layout byte.
    let mut f = file.clone();
    f[d.start + own.start] = 0x85;
    assert_eq!(
        decode(&f).map(|_| ()),
        Err(Error::Corrupt("bad decorrelation mode"))
    );
    // The record's id made even (ignorable): stripes without their tables.
    let table = p.head_record(35).unwrap();
    let mut f = file.clone();
    let id_at = d.start + table.start - common::varint_bytes(table.len()).len() - 1;
    assert_eq!(f[id_at], 35);
    f[id_at] = 34;
    assert_eq!(
        decode(&f).map(|_| ()),
        Err(Error::Corrupt(
            "template palette stripe without template tables"
        ))
    );
}

#[test]
fn typical_row_bitmap_padding_is_corrupt() {
    // Stripes of 5 rows: each bitmap byte has three padding bits.
    let (w, h) = (120usize, 40usize);
    let info = ImageInfo::new(w as u32, h as u32, Channels::Gray, SampleType::U8);
    let cfg = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some(5);
        });
        cfg
    };
    let px = gray8(&indices(w, h, 9, 11, None));
    let file = check(&px, &info, &cfg);
    let d = data_payload(&file);
    let p = payload(&file[d.clone()], &info, false);
    let u = p
        .records()
        .into_iter()
        .find(|u| file[d.start + u.start] == 0x84)
        .unwrap();
    let mut f = file.clone();
    f[d.start + u.start + 1] |= 0x80;
    assert_eq!(
        decode(&f).map(|_| ()),
        Err(Error::Corrupt("typical-row bitmap padding"))
    );
}

#[test]
fn unsuitable_images_keep_their_coding() {
    // A photo-like gradient (more than 256 values per unit): no template.
    let (w, h) = (200usize, 100usize);
    let info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8);
    let px: Vec<u8> = (0..w * h * 3)
        .map(|i| ((i * 7 + i / 600) % 256) as u8)
        .collect();
    let file = enc(&px, &info, &imij::ImiEncoderConfig::default());
    assert_eq!(feature_mask(&file) & TEMPLATE, 0);
    // Palettes disallowed: no template stripes either.
    let px = rgb8(&indices(w, h, 9, 3, None));
    let cfg = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.palette = false;
        });
        cfg
    };
    let file = enc(&px, &info, &cfg);
    assert_eq!(feature_mask(&file) & TEMPLATE, 0);
    assert_eq!(decode(&file).unwrap().1, px);
}

/// Decode admission charges record 35 and the template rows before any
/// pixel is written: the planned working bytes and work units decode, one
/// less is refused with the output untouched, and the encoder's decoder
/// memory estimate for the configuration covers the plan.
#[test]
fn decode_budgets_cover_template_stripes() {
    let (w, h) = (230usize, 150usize);
    let info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8);
    let px = rgb8(&indices(w, h, 17, 9, Some(110)));
    let cfg = imij::ImiEncoderConfig::default();
    let file = check(&px, &info, &cfg);
    let mut probe = DecodeSession::new(DecodePolicy::default());
    assert!(probe.decode(&file).unwrap().1 == px);
    let usage = probe.usage();
    let (working, work) = (usage.peak_planned_working_bytes, usage.work_units);
    assert!(cfg.estimated_decoder_memory(&info).unwrap() >= working);
    for policy in [
        DecodePolicy::default().with_max_working_bytes(working - 1),
        DecodePolicy::default().with_max_work_units(work - 1),
    ] {
        let stride = info.row_bytes() + 13;
        let mut out = vec![0xA5; stride * h];
        let before = out.clone();
        assert!(matches!(
            DecodeSession::new(policy).decode_into(&file, &mut out, stride),
            Err(Error::TooLarge)
        ));
        assert!(out == before, "admission denial must precede pixel writes");
    }
    let policy = DecodePolicy::default()
        .with_max_working_bytes(working)
        .with_max_work_units(work);
    assert!(DecodeSession::new(policy).decode(&file).unwrap().1 == px);
}
