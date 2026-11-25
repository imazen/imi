//! Superblocks (feature 12, SPEC 2.9): merged 16x16 / 16x8 / 8x16 DCTs in
//! the Compact tier. Merged files decode identically through every route,
//! thread count and kernel tier (the scalar reference included), regions
//! equal crops of the full decode, exact blocks share a file with merges,
//! the encoder merges only where its rule says, and malformed maps or
//! records fail with SPEC's error classes.

mod common;

use common::feature::{EXACT_BLOCKS, SUPERBLOCKS};
use common::{
    H_SUPERBLOCKS, data_payload, feature_mask, payload, put_varint, read_varint,
    superblock_map_lengths as map_lengths, superblock_units as merges,
};
use imij::planes::Rect;
use imij::region::decode_region_threads;
use imij::{
    Channels, DecodeSpeed, Error, ImageInfo, ImiEncoderConfig, Metadata, SampleType, decode,
};

/// Smooth ramps (merge territory), a textured band and hard edges.
fn image(info: &ImageInfo) -> Vec<u8> {
    let (w, h, n) = (
        info.width as usize,
        info.height as usize,
        info.channels.count(),
    );
    let mut px = vec![0u8; info.row_bytes() * h];
    for y in 0..h {
        for x in 0..w {
            for c in 0..n {
                let ramp = (x as f32 * 0.7 + y as f32 * 0.4 + c as f32 * 40.0) % 200.0 + 20.0;
                let tex = if x > w / 2 {
                    18.0 * ((x as f32 * 0.9 + c as f32).sin() * (y as f32 * 0.7).cos())
                } else {
                    0.0
                };
                let edge = if (x / 23 + y / 17) % 5 == 0 {
                    40.0
                } else {
                    0.0
                };
                px[(y * w + x) * n + c] = (ramp + tex + edge).clamp(0.0, 255.0) as u8;
            }
        }
    }
    px
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

fn compact(q: f32) -> ImiEncoderConfig {
    imij::ImiEncoderConfig::lossy(q).with_decode_speed(DecodeSpeed::Compact)
}

fn mse(a: &[u8], b: &[u8]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(&x, &y)| (x as f64 - y as f64).powi(2))
        .sum::<f64>()
        / a.len() as f64
}

#[cfg(feature = "lab")]
fn fnv(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| {
        (h ^ x as u64).wrapping_mul(0x100_0000_01b3)
    })
}

#[test]
fn compact_merges_and_every_route_agrees() {
    for (w, h) in [(256u32, 160u32), (200, 136), (131, 257)] {
        for channels in [Channels::Rgb, Channels::Rgba, Channels::Gray] {
            let info = ImageInfo::new(w, h, channels, SampleType::U8);
            let px = image(&info);
            for q in [5.0f32, 20.0, 50.0, 90.0] {
                let what = format!("{w}x{h} {channels:?} q{q}");
                let mut cfg = compact(q);
                // Exact blocks off: the region reader refuses them.
                cfg = cfg.tune_lossy(|t| {
                    t.exact_blocks = Some(0.0);
                });
                let file = enc(&px, &info, &cfg);
                assert_eq!(feature_mask(&file) & SUPERBLOCKS, SUPERBLOCKS, "{what}");
                assert!(merges(&file, &info) > 0, "{what}");
                assert_eq!(
                    file,
                    enc(&px, &info, &cfg.clone().with_threads(4)),
                    "{what}"
                );
                let dec = common::decode_all(&file, &info);
                // Pixels stay close to the source: no worse than three times
                // the error of the same encode in the Balanced tier.
                let plain =
                    imij::ImiEncoderConfig::lossy(q).with_decode_speed(DecodeSpeed::Balanced);
                let off = decode(&enc(&px, &info, &plain)).unwrap().1;
                let (e_on, e_off) = (mse(&px, &dec), mse(&px, &off));
                assert!(e_on <= 3.0 * e_off + 3.0, "{what}: mse {e_on} vs {e_off}");
                if channels.has_alpha() {
                    for (s, d) in px.as_chunks::<4>().0.iter().zip(dec.as_chunks::<4>().0) {
                        assert_eq!(s[3], d[3], "{what}: alpha");
                    }
                }
                // Every region equals the same crop of the full decode.
                let bpp = info.row_bytes() / w as usize;
                for r in [
                    Rect::new(0, 0, w, h),
                    Rect::new(3, 5, w / 2, h / 3),
                    Rect::new(w / 2 - 1, h / 2 - 9, 17, 23),
                    Rect::new(w - 1, h - 1, 1, 1),
                    Rect::new(8, 16, 16, 16),
                ] {
                    let rb = r.width() as usize * bpp;
                    let mut want = Vec::new();
                    for y in r.y()..r.y() + r.height() {
                        let a = y as usize * info.row_bytes() + r.x() as usize * bpp;
                        want.extend_from_slice(&dec[a..a + rb]);
                    }
                    for threads in [1, 4] {
                        let mut got = vec![0xA5u8; want.len()];
                        decode_region_threads(&file, r, &mut got, rb, threads).unwrap();
                        assert!(got == want, "{what}: region {r:?}, {threads} threads");
                    }
                }
            }
        }
    }
}

#[test]
fn only_compact_merges_and_not_below_128_px() {
    let info = ImageInfo::new(256, 192, Channels::Rgb, SampleType::U8);
    let px = image(&info);
    for speed in [
        DecodeSpeed::Balanced,
        DecodeSpeed::Fast,
        DecodeSpeed::Fastest,
    ] {
        let file = enc(
            &px,
            &info,
            &imij::ImiEncoderConfig::lossy(30.0).with_decode_speed(speed),
        );
        assert_eq!(feature_mask(&file) & SUPERBLOCKS, 0, "{speed:?}");
    }
    let file = enc(&px, &info, &imij::ImiEncoderConfig::lossy(30.0));
    assert_eq!(feature_mask(&file) & SUPERBLOCKS, 0, "no tier");
    // Compact retains superblocks across trellis efforts (9.0, 9.5, 10.0).
    for effort in [9.0f32, 9.5, 10.0] {
        let file = enc(&px, &info, &compact(30.0).with_effort(effort));
        assert_ne!(
            feature_mask(&file) & SUPERBLOCKS,
            0,
            "Compact should retain superblocks at effort {effort}"
        );
        let _ = common::decode_all(&file, &info);
    }
    // The longer side decides: 127 px never merges, 128 may.
    for (w, h, on) in [
        (127u32, 127u32, false),
        (127, 64, false),
        (128, 40, true),
        (40, 128, true),
    ] {
        let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
        let px = image(&info);
        let file = enc(&px, &info, &compact(20.0));
        assert_eq!(
            feature_mask(&file) & SUPERBLOCKS != 0,
            on,
            "{w}x{h}: {}",
            merges(&file, &info)
        );
    }
    // 16-bit input keeps 8x8 blocks.
    let info16 = ImageInfo::new(256, 192, Channels::Rgb, SampleType::U16);
    let px16: Vec<u8> = px
        .iter()
        .flat_map(|&v| (v as u16 * 257).to_ne_bytes())
        .collect();
    let file = enc(&px16, &info16, &compact(30.0));
    assert_eq!(feature_mask(&file) & SUPERBLOCKS, 0, "16-bit");
}

/// Sharp-edged ink lines on paper in the top half from column `ink_x0`
/// (exact-block territory: every 8x8 block there holds both), a smooth ramp
/// elsewhere (merge territory).
fn ink_and_ramp(info: &ImageInfo, ink_x0: usize) -> Vec<u8> {
    let (w, h) = (info.width as usize, info.height as usize);
    let mut px = vec![0u8; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let o = (y * w + x) * 3;
            if y < h / 2 && x >= ink_x0 {
                let ink = (x % 9 < 2 && y % 13 > 2) || (y % 13 == 7 && x % 17 < 11);
                px[o..o + 3].copy_from_slice(if ink { &[20, 30, 90] } else { &[250, 250, 245] });
            } else {
                for c in 0..3 {
                    px[o + c] = (30.0 + x as f32 * 0.6 + y as f32 * 0.3 + c as f32 * 25.0) as u8;
                }
            }
        }
    }
    px
}

#[test]
fn exact_blocks_and_superblocks_share_a_file() {
    // Compact with exact blocks on.
    let info = ImageInfo::new(192, 160, Channels::Rgb, SampleType::U8);
    let px = ink_and_ramp(&info, 0);
    for q in [10.0f32, 40.0, 80.0] {
        let file = enc(&px, &info, &compact(q));
        let mask = feature_mask(&file);
        assert_eq!(
            mask & (EXACT_BLOCKS | SUPERBLOCKS),
            EXACT_BLOCKS | SUPERBLOCKS,
            "q{q}"
        );
        common::decode_all(&file, &info);
    }
}

/// Files of many configurations (lab switch: merges on any tier, color,
/// entropy coder, column groups, sample type): (name, file).
#[cfg(feature = "lab")]
fn lab_files() -> Vec<(String, Vec<u8>)> {
    use imij::{LossyColor, LossyEntropy};
    let mut out = Vec::new();
    for (w, h, ch, sample) in [
        (141u32, 53u32, Channels::Rgb, SampleType::U8),
        (96, 160, Channels::Rgba, SampleType::U8),
        (77, 90, Channels::Gray, SampleType::U8),
        (88, 72, Channels::Rgb, SampleType::U16),
        (64, 48, Channels::Rgb, SampleType::F32),
    ] {
        let info = ImageInfo::new(w, h, ch, sample);
        let base = image(&ImageInfo::new(w, h, ch, SampleType::U8));
        let px: Vec<u8> = match sample {
            SampleType::U16 => base
                .iter()
                .flat_map(|&v| (v as u16 * 257).to_ne_bytes())
                .collect(),
            SampleType::F32 => base
                .iter()
                .flat_map(|&v| (v as f32 / 255.0).to_ne_bytes())
                .collect(),
            _ => base,
        };
        for color in [LossyColor::Xyb, LossyColor::YCoCg] {
            for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                for (rows, group) in [(2u32, 0u32), (3, 0), (8, 0), (2, 16), (4, 40), (1, 24)] {
                    for q in [5.0f32, 30.0, 90.0] {
                        let mut cfg = imij::ImiEncoderConfig::lossy(q)
                            .tune_lossy(|t| t.color = color)
                            .tune_lossy(|t| t.entropy = entropy)
                            .tune_lossy(|t| t.stripe_block_rows = rows);
                        cfg = cfg.with_column_groups(Some(group));
                        cfg = cfg.tune_lossy(|t| t.lab.switches.superblocks = Some(true));
                        let file = enc(&px, &info, &cfg);
                        let name = format!(
                            "{w}x{h}-{ch:?}-{sample:?}-{color:?}-{entropy:?}-r{rows}-g{group}-q{q}"
                        );
                        out.push((name, file));
                    }
                }
            }
        }
    }
    out
}

#[cfg(feature = "lab")]
#[test]
fn lab_switch_merges_everywhere_and_routes_agree() {
    let mut merged = 0;
    for (name, file) in lab_files() {
        let (info, _) = decode(&file).unwrap();
        if feature_mask(&file) & SUPERBLOCKS != 0 {
            merged += 1;
        }
        let _ = name;
        common::decode_all(&file, &info);
    }
    assert!(merged > 300, "{merged} merged files");
}

#[cfg(feature = "lab")]
#[test]
fn superblock_files_decode_identically_with_scalar_kernels() {
    // The parent decodes the files with the dispatched kernels; a child
    // process of this test binary decodes the same files with
    // `IMIJ_SCALAR=1` (every decoder kernel scalar) and prints hashes.
    let files = lab_files();
    if std::env::var_os("SB_SCALAR_CHILD").is_some() {
        for (name, file) in &files {
            println!("HASH {name} {:016x}", fnv(&decode(file).unwrap().1));
        }
        return;
    }
    let want: std::collections::BTreeMap<String, String> = files
        .iter()
        .map(|(n, f)| (n.clone(), format!("{:016x}", fnv(&decode(f).unwrap().1))))
        .collect();
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "superblock_files_decode_identically_with_scalar_kernels",
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .env("SB_SCALAR_CHILD", "1")
        .env("IMIJ_SCALAR", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let got: std::collections::BTreeMap<String, String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.find("HASH ").map(|i| &l[i + 5..]))
        .map(|l| {
            let (a, b) = l.rsplit_once(' ').unwrap();
            (a.to_string(), b.to_string())
        })
        .collect();
    assert_eq!(got.len(), want.len());
    assert_eq!(got, want);
}

/// `file` (lossy, `info`) with its payload rebuilt by `edit`.
fn with_payload(file: &[u8], edit: impl FnOnce(&[u8]) -> Vec<u8>) -> Vec<u8> {
    let r = data_payload(file);
    let new = edit(&file[r.clone()]);
    // DATA's length varint precedes its payload.
    let mut start = r.start - 1;
    while file[start - 1] & 0x80 != 0 {
        start -= 1;
    }
    let mut out = file[..start].to_vec();
    put_varint(&mut out, new.len() as u64 + 1);
    out.extend_from_slice(&new);
    out.extend_from_slice(&file[r.end..]);
    out
}

#[test]
fn malformed_superblock_files_are_rejected() {
    let info = ImageInfo::new(160, 128, Channels::Rgb, SampleType::U8);
    let px = image(&info);
    let mut cfg = compact(20.0);
    cfg = cfg.tune_lossy(|t| {
        t.exact_blocks = Some(0.0);
    });
    let file = enc(&px, &info, &cfg);
    assert_ne!(feature_mask(&file) & SUPERBLOCKS, 0);
    // Without the feature bit: the record is gated syntax.
    let no_bit = common::with_feature_mask(&file, Some(feature_mask(&file) & !SUPERBLOCKS));
    assert_eq!(
        decode(&no_bit).err(),
        Some(Error::Corrupt(
            "superblocks without the feature superblocks"
        ))
    );
    // The record carries no payload.
    let long = with_payload(&file, |d| {
        let p = payload(d, &info, true);
        common::with_head_record(d, &p, H_SUPERBLOCKS, Some(&[0]))
    });
    assert_eq!(
        decode(&long).err(),
        Some(Error::Corrupt("lossy head record length"))
    );
    // A merged unit's map with bytes its decoding never reads (past the
    // zeros a decoder reads beyond the end).
    let lens = map_lengths(&file, &info);
    let u = lens.iter().position(|&n| n > 0).unwrap();
    let grown = with_payload(&file, |d| {
        let p = payload(d, &info, true);
        let rec = p.units.iter().filter(|r| r.0 == 55).nth(u).unwrap().clone();
        let mut at = rec.2.start;
        let n = read_varint(d, &mut at) as usize;
        let mut body = Vec::new();
        put_varint(&mut body, n as u64 + 12);
        body.extend_from_slice(&d[at..at + n]);
        body.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0x5a, 0x5a, 0x5a, 0x5a]);
        body.extend_from_slice(&d[at + n..rec.2.end]);
        rebuild_record(d, &p, &rec, &body)
    });
    assert_eq!(
        decode(&grown).err(),
        Some(Error::Corrupt("superblock map has unread bytes"))
    );
    // Record 21 with record 27 (channel layouts, declared; even the default
    // layouts' bytes) or record 9 (keep blocks).
    for (id, rec) in [(27u64, &[8u8, 8, 8][..]), (9, &[][..])] {
        let mut bad = common::with_lossy_head_record(&file, id, Some(rec));
        if id == 27 {
            let mask = feature_mask(&bad) | common::feature::CHANNEL_LAYOUTS;
            bad = common::with_feature_mask(&bad, Some(mask));
        }
        assert_eq!(
            decode(&bad).err(),
            Some(Error::Corrupt(
                "superblocks with keep blocks or channel layouts"
            )),
            "record {id}"
        );
    }
    // Tree value 7 in the first group of a unit.
    let seven = common::superblock_map(2, 2, |_, _| 8);
    assert_eq!(
        decode(&common::with_superblock_map(&file, u, &seven)).err(),
        Some(Error::Corrupt("superblock map symbol"))
    );
    // A map that merges nothing (a writer writes no map then).
    assert_eq!(
        decode(&common::with_superblock_map(&file, u, &[0])).err(),
        Some(Error::Corrupt("superblock map without a merge"))
    );
}

#[test]
fn merges_past_the_unit_are_rejected() {
    // Stripes of three block rows, one column group: each unit is 3 x 20
    // parents, so its second group row has no lower parents.
    let info = ImageInfo::new(160, 128, Channels::Rgb, SampleType::U8);
    let px = image(&info);
    let mut cfg = compact(20.0).tune_lossy(|t| t.stripe_block_rows = 3);
    cfg = cfg.with_column_groups(Some(0));
    cfg = cfg.tune_lossy(|t| {
        t.exact_blocks = Some(0.0);
    });
    let file = enc(&px, &info, &cfg);
    assert!(merges(&file, &info) > 0);
    assert_eq!(map_lengths(&file, &info).len(), 6);
    // Valid: a 16x8 merge in the lower group row (symbol 2) and a 16x16 in
    // the upper one, on top of what the unit coded.
    let ok = common::superblock_map(3, 20, |r, x| match (r, x) {
        (0, 4) => 1,
        (2, 6) => 2,
        _ => 0,
    });
    decode(&common::with_superblock_map(&file, 0, &ok)).unwrap();
    // A 16x16 (symbol 1) or the lower 16x8 (symbol 3) there needs parents
    // the unit does not have.
    for s in [1, 3, 4] {
        let past = common::superblock_map(3, 20, |r, x| if (r, x) == (2, 6) { s } else { 0 });
        assert_eq!(
            decode(&common::with_superblock_map(&file, 0, &past)).err(),
            Some(Error::Corrupt("superblock past its unit")),
            "symbol {s}"
        );
    }
}

#[test]
fn merges_over_exact_blocks_or_mixed_block_codes_are_rejected() {
    // Compact with exact blocks on, stripes of two block rows and one column
    // group: unit `by / 2` holds block rows by and by + 1. Ink from block
    // column 13: the groups at column 12 pair a ramp block (left) with an
    // ink block, those at 14 two ink blocks.
    let info = ImageInfo::new(192, 160, Channels::Rgb, SampleType::U8);
    let (w, bw, ink_rows) = (192usize, 24usize, 10usize);
    let px = ink_and_ramp(&info, 13 * 8);
    let mut cfg = compact(20.0).tune_lossy(|t| t.stripe_block_rows = 2);
    cfg = cfg.with_column_groups(Some(0));
    let file = enc(&px, &info, &cfg);
    assert_eq!(
        feature_mask(&file) & (EXACT_BLOCKS | SUPERBLOCKS),
        EXACT_BLOCKS | SUPERBLOCKS
    );
    assert_eq!(map_lengths(&file, &info).len(), 10);
    let dec = decode(&file).unwrap().1;
    // An ink block decodes to its source exactly only as an exact block.
    let exact = |by: usize, bx: usize| {
        (0..8).all(|y| {
            let a = ((by * 8 + y) * w + bx * 8) * 3;
            dec[a..a + 24] == px[a..a + 24]
        })
    };
    let (mut over, mut mixed) = (None, None);
    for by in (0..ink_rows).step_by(2) {
        for bx in (0..bw).step_by(2) {
            let e = [
                exact(by, bx),
                exact(by, bx + 1),
                exact(by + 1, bx),
                exact(by + 1, bx + 1),
            ];
            if e[0] {
                over.get_or_insert((by, bx));
            } else if e[1..].contains(&true) {
                mixed.get_or_insert((by, bx));
            }
        }
    }
    for (at, want) in [
        (over, "superblock over an exact or kept block"),
        (mixed, "superblock parents with different block codes"),
    ] {
        let (by, bx) = at.expect(want);
        let map = common::superblock_map(2, bw, |_, x| u8::from(x == bx));
        let bad = common::with_superblock_map(&file, by / 2, &map);
        assert_eq!(
            decode(&bad).err(),
            Some(Error::Corrupt(want)),
            "group at block ({by}, {bx})"
        );
    }
}

/// With superblocks on and nothing merged, the file is the plain file: no
/// record 21, no map bytes, and no size estimate charged for maps never
/// written (it picks restart states and seed tables).
#[cfg(feature = "lab")]
#[test]
fn files_without_merges_are_the_plain_files() {
    let mut plain = 0;
    for (w, h) in [
        (128u32, 16u32),
        (160, 40),
        (256, 64),
        (136, 200),
        (320, 136),
    ] {
        let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
        let mut s = w ^ h << 9 | 1;
        let px: Vec<u8> = (0..info.row_bytes() * h as usize)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                s as u8
            })
            .collect();
        for q in [60.0f32, 75.0, 85.0, 90.0, 95.0, 97.0, 99.0] {
            let on = enc(&px, &info, &compact(q));
            if feature_mask(&on) & SUPERBLOCKS != 0 {
                continue;
            }
            plain += 1;
            let mut off = compact(q);
            off = off.tune_lossy(|t| t.lab.switches.superblocks = Some(false));
            assert!(on == enc(&px, &info, &off), "{w}x{h} q{q}");
        }
    }
    assert!(plain >= 10, "{plain} files without merges");
}

/// Superblocks combine with entropy 0 and 3, the only entropy values: a
/// split-Huffman superblock file whose entropy byte is forged to the retired
/// 1 or 2 is `Unsupported` (SPEC 2.8), with no superblock decode for them.
#[cfg(feature = "lab")]
#[test]
fn superblocks_with_retired_entropy_values_are_unsupported() {
    let info = ImageInfo::new(96, 64, Channels::Rgb, SampleType::U8);
    let px = image(&info);
    let mut cfg = imij::ImiEncoderConfig::lossy(30.0)
        .tune_lossy(|t| t.entropy = imij::LossyEntropy::SplitHuf);
    cfg = cfg.tune_lossy(|t| t.lab.switches.superblocks = Some(true));
    let file = enc(&px, &info, &cfg);
    assert!(merges(&file, &info) > 0);
    decode(&file).unwrap();
    let data = data_payload(&file).start;
    let p = payload(&file[data..], &info, true);
    let entropy_at = data
        + p.present_at
        + common::varint(&file[data + p.present_at..]).1
        + 4
        + usize::from(p.present & 1 != 0);
    assert_eq!(file[entropy_at], imij::LossyEntropy::SplitHuf as u8);
    for v in [1, 2] {
        let mut bad = file.clone();
        bad[entropy_at] = v;
        assert_eq!(
            decode(&bad).err(),
            Some(Error::Unsupported("retired entropy mode")),
            "entropy {v}"
        );
    }
}

/// The payload `d` (walked as `p`) with unit record `rec`'s payload
/// replaced by `body` (its length and the unit table hint entry follow).
fn rebuild_record(
    d: &[u8],
    p: &common::Payload,
    rec: &(u64, std::ops::Range<usize>, std::ops::Range<usize>),
    body: &[u8],
) -> Vec<u8> {
    // Rebuild the hint with the record's new span.
    let mains: Vec<_> = p.units.iter().filter(|u| u.0 & 1 == 1).collect();
    let i = mains.iter().position(|u| u.1 == rec.1).unwrap();
    let mut framed = Vec::new();
    put_varint(&mut framed, rec.0);
    put_varint(&mut framed, body.len() as u64);
    framed.extend_from_slice(body);
    let mut d2 = d[..rec.1.start].to_vec();
    d2.extend_from_slice(&framed);
    d2.extend_from_slice(&d[rec.1.end..]);
    // The head (and so `p`'s offsets into it) is unchanged.
    match p.table() {
        Some(_) => {
            let mut hint = Vec::new();
            for (k, u) in mains.iter().enumerate() {
                let n = if k == i { framed.len() } else { u.1.len() };
                put_varint(&mut hint, n as u64);
            }
            common::with_head_record(&d2, p, common::H_UNIT_TABLE, Some(&hint))
        }
        None => d2,
    }
}
