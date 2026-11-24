//! Small transforms (feature `small_transforms`, SPEC 2.11): 4x4 planes chosen per block and
//! plane by a joint partition map that also carries superblock merges.
//! Files decode identically through every route, thread count and kernel
//! tier (the scalar reference included), regions equal crops of the full
//! decode, malformed maps and records fail with SPEC's error classes, and
//! (lab builds) the encoder's 4x4 planes combine with every other tool.

mod common;

#[cfg(feature = "lab")]
use common::feature::EXACT_BLOCKS;
use common::feature::{SMALL_TRANSFORMS, SUPERBLOCKS};
use common::{H_PARTITIONS, H_SUPERBLOCKS, feature_mask};
use imij::DecodeSpeed;
#[cfg(feature = "lab")]
use imij::LossyEntropy;
use imij::planes::Rect;
use imij::region::decode_region_threads;
use imij::{Channels, Error, ImageInfo, ImiEncoderConfig, Metadata, SampleType, decode};

/// Smooth ramps, a textured band and hard edges.
fn image(info: &ImageInfo) -> Vec<u8> {
    let (w, h, n) = (
        info.width as usize,
        info.height as usize,
        info.channels.count(),
    );
    let bps = info.sample.bytes();
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
                let v = (ramp + tex + edge).clamp(0.0, 255.0);
                let o = ((y * w + x) * n + c) * bps;
                match info.sample {
                    SampleType::U8 => px[o] = v as u8,
                    SampleType::U16 => px[o..o + 2]
                        .copy_from_slice(&((v * 257.0) as u16 + (x % 7) as u16).to_ne_bytes()),
                    SampleType::F32 => {
                        px[o..o + 4].copy_from_slice(&(v / 255.0).powf(2.2).to_ne_bytes())
                    }
                    _ => unreachable!(),
                }
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

#[cfg(feature = "lab")]
fn fnv(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| {
        (h ^ x as u64).wrapping_mul(0x100_0000_01b3)
    })
}

#[cfg(feature = "lab")]
fn xorshift(s: &mut u32) -> u32 {
    *s ^= *s << 13;
    *s ^= *s >> 17;
    *s ^= *s << 5;
    *s
}

/// `file` (a plain lossy file without maps) made a small-transform file:
/// record 23 and the feature bit added, and every AC record given the joint
/// map `map(unit, rows, cols, ac)` (SPEC 2.11.1), `ac(r, x)` the planes of
/// parent (r, x) with AC (lab builds read them from the file; others assume
/// every plane has AC, which only maps without flags may rely on).
type AcFn<'a> = &'a dyn Fn(usize, usize) -> u8;
fn forge(file: &[u8], map: impl Fn(usize, usize, usize, AcFn<'_>) -> Vec<u8>) -> Vec<u8> {
    #[cfg(feature = "lab")]
    let bits = imij::lab::lossy_unit_ac_bits(file).unwrap();
    let f = common::with_lossy_head_record(file, H_PARTITIONS, Some(&[]));
    let f = common::with_feature_mask(&f, Some(feature_mask(&f) | SMALL_TRANSFORMS));
    let geo = common::lossy_units(&f);
    let mut out = f;
    for (u, &(rows, cols)) in geo.iter().enumerate() {
        #[cfg(feature = "lab")]
        let ac = |r: usize, x: usize| bits[u][r * cols + x];
        #[cfg(not(feature = "lab"))]
        let ac = |_: usize, _: usize| 7u8;
        let m = map(u, rows, cols, &ac);
        out = common::with_inserted_map(&out, u, &m);
    }
    out
}

/// Every route decodes `file` to the same pixels as `decode`; regions equal
/// crops of the full decode.
fn every_route(file: &[u8], info: &ImageInfo) -> Vec<u8> {
    let dec = common::decode_all(file, info);
    let (w, h) = (info.width, info.height);
    let bpp = info.row_bytes() / w as usize;
    let fit = |x: u32, y: u32, rw: u32, rh: u32| {
        let (x, y) = (x.min(w - 1), y.min(h - 1));
        Rect::new(x, y, rw.clamp(1, w - x), rh.clamp(1, h - y))
    };
    for r in [
        fit(0, 0, w, h),
        fit(3, 5, w / 2, h / 3),
        fit((w / 2).saturating_sub(1), (h / 2).saturating_sub(9), 17, 23),
        fit(w - 1, h - 1, 1, 1),
    ] {
        let rb = r.width() as usize * bpp;
        let mut want = Vec::new();
        for y in r.y()..r.y() + r.height() {
            let a = y as usize * info.row_bytes() + r.x() as usize * bpp;
            want.extend_from_slice(&dec[a..a + rb]);
        }
        for threads in [1, 4] {
            let mut got = vec![0xA5u8; want.len()];
            decode_region_threads(file, r, &mut got, rb, threads).unwrap();
            assert!(got == want, "region {r:?} threads {threads}");
        }
    }
    dec
}

/// Plain files of several geometries and their forged small-transform
/// versions: random split flags on planes with AC (about one in three;
/// sometimes every such plane), no merges.
#[cfg(feature = "lab")]
fn forged_files() -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut seed = 0x51ed_270bu32;
    for (w, h) in [(256u32, 160u32), (200, 136), (131, 257), (64, 8), (9, 70)] {
        for channels in [Channels::Rgb, Channels::Rgba, Channels::Gray] {
            let info = ImageInfo::new(w, h, channels, SampleType::U8);
            let px = image(&info);
            for (entropy, rows, groups) in [
                (LossyEntropy::RunLevel, 2u32, 0u32),
                (LossyEntropy::SplitHuf, 3, 0),
                (LossyEntropy::RunLevel, 8, 16),
                (LossyEntropy::SplitHuf, 1, 24),
            ] {
                if groups != 0 && groups * 8 >= w {
                    continue;
                }
                for q in [10.0f32, 60.0, 95.0] {
                    let mut cfg = imij::ImiEncoderConfig::lossy(q)
                        .tune_lossy(|t| t.entropy = entropy)
                        .tune_lossy(|t| t.stripe_block_rows = rows);
                    cfg = cfg.with_column_groups(Some(groups));
                    let plain = enc(&px, &info, &cfg);
                    let all = xorshift(&mut seed).is_multiple_of(4);
                    let s0 = xorshift(&mut seed);
                    let file = forge(&plain, |u, r, c, ac| {
                        let mut s = s0 ^ (u as u32).wrapping_mul(0x9e37_79b9) | 1;
                        let flags: Vec<u8> = (0..r * c)
                            .map(|i| {
                                let f = if all {
                                    7
                                } else {
                                    (xorshift(&mut s) % 8) as u8 & (xorshift(&mut s) % 8) as u8
                                };
                                f & ac(i / c, i % c)
                            })
                            .collect();
                        if flags.iter().all(|&f| f == 0) {
                            return Vec::new();
                        }
                        common::joint_map(r, c, |_, _| 0, |y, x| Some(flags[y * c + x]), ac)
                    });
                    out.push((
                        format!("{w}x{h}-{channels:?}-{entropy:?}-r{rows}-g{groups}-q{q}"),
                        file,
                    ));
                }
            }
        }
    }
    out
}

#[cfg(feature = "lab")]
#[test]
fn forged_small_transform_files_decode_on_every_route() {
    let mut n = 0;
    for (name, file) in forged_files() {
        let (info, _) = decode(&file).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        every_route(&file, &info);
        n += 1;
    }
    assert!(n > 100, "{n} files");
}

#[cfg(feature = "lab")]
#[test]
fn small_transform_files_decode_identically_with_scalar_kernels() {
    // The parent decodes the files with the dispatched kernels; a child
    // process of this test binary decodes the same files with
    // `IMIJ_SCALAR=1` (every decoder kernel scalar) and prints hashes.
    let files = [forged_files(), lab_files()].concat();
    if std::env::var_os("SMALL_SCALAR_CHILD").is_some() {
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
            "small_transform_files_decode_identically_with_scalar_kernels",
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .env("SMALL_SCALAR_CHILD", "1")
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

/// Header-level rules of record 23 need no flags: a file with the record
/// and empty maps (nothing splits) decodes as the plain file.
#[test]
fn record_23_header_rules() {
    let info = ImageInfo::new(160, 128, Channels::Rgb, SampleType::U8);
    let px = image(&info);
    let plain = enc(&px, &info, &imij::ImiEncoderConfig::lossy(30.0));
    let file = forge(&plain, |_, _, _, _| Vec::new());
    assert_eq!(decode(&file).unwrap().1, decode(&plain).unwrap().1);
    // Without the feature bit: the record is gated syntax.
    let no_bit = common::with_feature_mask(&file, Some(feature_mask(&file) & !SMALL_TRANSFORMS));
    assert_eq!(
        decode(&no_bit).err(),
        Some(Error::Corrupt(
            "small transforms without the feature small_transforms"
        ))
    );
    // The record carries no payload.
    let long = common::with_lossy_head_record(&file, H_PARTITIONS, Some(&[0]));
    assert_eq!(
        decode(&long).err(),
        Some(Error::Corrupt("lossy head record length"))
    );
    // With record 21 or current channel-layout record 27, both declared.
    let both = common::with_feature_mask(
        &common::with_lossy_head_record(&file, H_SUPERBLOCKS, Some(&[])),
        Some(feature_mask(&file) | SUPERBLOCKS),
    );
    assert_eq!(
        decode(&both).err(),
        Some(Error::Corrupt(
            "small transforms with superblocks or channel layouts"
        ))
    );
    let layouts = common::with_feature_mask(
        &common::with_lossy_head_record(&file, common::H_LAYOUTS, Some(&[8, 8, 8])),
        Some(feature_mask(&file) | common::feature::CHANNEL_LAYOUTS),
    );
    assert_eq!(
        decode(&layouts).err(),
        Some(Error::Corrupt(
            "small transforms with superblocks or channel layouts"
        ))
    );
    // A map that neither merges nor splits (a writer writes no map then):
    // one zero byte, read as the zeros past the end are.
    let mut zero = file.clone();
    for u in 0..common::lossy_units(&file).len() {
        zero = common::with_superblock_map(&zero, u, &[0]);
    }
    assert_eq!(
        decode(&zero).err(),
        Some(Error::Corrupt("partition map without a merge or split"))
    );
}

#[cfg(feature = "lab")]
#[test]
fn malformed_small_transform_maps_are_rejected() {
    let info = ImageInfo::new(160, 128, Channels::Rgb, SampleType::U8);
    let px = image(&info);
    let mut cfg = imij::ImiEncoderConfig::lossy(30.0).tune_lossy(|t| t.stripe_block_rows = 2);
    cfg = cfg.with_column_groups(Some(0));
    let plain = enc(&px, &info, &cfg);
    let bits = imij::lab::lossy_unit_ac_bits(&plain).unwrap();
    // Unit 0 (2 x 20 parents): luma of its first parent with luma AC split.
    let at = bits[0]
        .iter()
        .position(|&a| a & 1 != 0)
        .expect("a parent with luma AC");
    let ac0 = |y: usize, x: usize| bits[0][y * 20 + x];
    let split_one = || {
        common::joint_map(
            2,
            20,
            |_, _| 0,
            |y, x| Some(u8::from(y * 20 + x == at)),
            ac0,
        )
    };
    let file = forge(
        &plain,
        |u, _, _, _| if u == 0 { split_one() } else { Vec::new() },
    );
    decode(&file).unwrap();
    // A map with bytes its decoding never reads.
    let mut grown = split_one();
    grown.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0x5a]);
    assert_eq!(
        decode(&common::with_superblock_map(&file, 0, &grown)).err(),
        Some(Error::Corrupt("partition map has unread bytes"))
    );
    // A flag coded for a plane without AC: the decoder reads one bit fewer
    // than the map holds (the map ends in a 1 bit it never reads).
    if let Some(no_luma) = bits[0].iter().position(|&a| a & 1 == 0) {
        let extra = common::joint_map(
            2,
            20,
            |_, _| 0,
            |y, x| Some(u8::from(y * 20 + x == at || y * 20 + x == no_luma)),
            |y, x| ac0(y, x) | u8::from(y * 20 + x == no_luma),
        );
        assert!(decode(&common::with_superblock_map(&file, 0, &extra)).is_err());
    }
    // A merge in a file that does not declare superblocks.
    let merge = common::joint_map(
        2,
        20,
        |r, x| u8::from((r, x) == (0, 4)),
        |y, x| (!(y < 2 && (4..6).contains(&x))).then_some(0),
        ac0,
    );
    let mut merge = merge;
    if merge.is_empty() {
        merge.push(0);
    }
    assert_eq!(
        decode(&common::with_superblock_map(&file, 0, &merge)).err(),
        Some(Error::Corrupt(
            "partition map merges without the feature superblocks"
        ))
    );
    // With the feature the same map decodes: a 16x16 merge whose parents
    // carry no flags (a valid map: it merges).
    let with_sb = common::with_feature_mask(
        &common::with_superblock_map(&file, 0, &merge),
        Some(feature_mask(&file) | SUPERBLOCKS),
    );
    decode(&with_sb).unwrap();
    // A header announcing a section its bits do not fill: merges with
    // every symbol 0, splits with every flag 0.
    let empty_merges = common::joint_map_with_header(
        2,
        20,
        |_, _| 0,
        |y, x| Some(u8::from(y * 20 + x == at)),
        ac0,
        Some((true, true)),
    );
    assert_eq!(
        decode(&common::with_superblock_map(&with_sb, 0, &empty_merges)).err(),
        Some(Error::Corrupt(
            "partition map merges section without a merge"
        ))
    );
    let mut empty_splits =
        common::joint_map_with_header(2, 20, |_, _| 0, |_, _| Some(0), ac0, Some((false, true)));
    if empty_splits.is_empty() {
        empty_splits.push(0);
    }
    assert_eq!(
        decode(&common::with_superblock_map(&file, 0, &empty_splits)).err(),
        Some(Error::Corrupt(
            "partition map splits section without a split"
        ))
    );
    // Tree value 7.
    let seven = common::joint_map(2, 20, |_, _| 8, |_, _| None, ac0);
    assert_eq!(
        decode(&common::with_superblock_map(&with_sb, 0, &seven)).err(),
        Some(Error::Corrupt("superblock map symbol"))
    );
}

#[cfg(feature = "lab")]
#[test]
fn merges_past_the_unit_in_a_joint_map_are_rejected() {
    let info = ImageInfo::new(160, 128, Channels::Rgb, SampleType::U8);
    let px = image(&info);
    let mut cfg = imij::ImiEncoderConfig::lossy(20.0).tune_lossy(|t| t.stripe_block_rows = 3);
    cfg = cfg.with_column_groups(Some(0));
    let plain = enc(&px, &info, &cfg);
    let file = forge(&plain, |_, r, c, ac| {
        common::joint_map(r, c, |_, _| 0, |_, _| Some(1), ac)
    });
    let file = common::with_feature_mask(&file, Some(feature_mask(&file) | SUPERBLOCKS));
    decode(&file).unwrap();
    for s in [1u8, 3, 4] {
        let past = common::joint_map(
            3,
            20,
            |r, x| if (r, x) == (2, 6) { s } else { 0 },
            |_, _| Some(0),
            |_, _| 7,
        );
        assert_eq!(
            decode(&common::with_superblock_map(&file, 0, &past)).err(),
            Some(Error::Corrupt("superblock past its unit")),
            "symbol {s}"
        );
    }
}

/// Exact parents carry no flags (SPEC 2.11.1): a joint map forged onto a
/// Compact file with exact blocks, flags for every other parent, decodes;
/// exact blocks keep their source pixels.
#[cfg(feature = "lab")]
#[test]
fn exact_parents_are_bypassed() {
    let info = ImageInfo::new(192, 160, Channels::Rgb, SampleType::U8);
    let (w, bw) = (192usize, 24usize);
    let mut px = image(&info);
    // Ink on white in the top half: exact-block territory.
    for y in 0..80 {
        for x in 104..w {
            let ink = (x % 9 < 2 && y % 13 > 2) || (y % 13 == 7 && x % 17 < 11);
            let o = (y * w + x) * 3;
            px[o..o + 3].copy_from_slice(if ink { &[20, 30, 90] } else { &[250, 250, 245] });
        }
    }
    let mut cfg = imij::ImiEncoderConfig::lossy(20.0)
        .with_decode_speed(DecodeSpeed::Compact)
        .tune_lossy(|t| t.stripe_block_rows = 2);
    cfg = cfg.with_column_groups(Some(0));
    cfg = cfg.tune_lossy(|t| t.lab.switches.superblocks = Some(false));
    let plain = enc(&px, &info, &cfg);
    assert_eq!(feature_mask(&plain) & EXACT_BLOCKS, EXACT_BLOCKS);
    let dec = decode(&plain).unwrap().1;
    let exact = |by: usize, bx: usize| {
        (0..8).all(|y| {
            let a = ((by * 8 + y) * w + bx * 8) * 3;
            dec[a..a + 24] == px[a..a + 24]
        })
    };
    let file = forge(&plain, |u, r, c, ac| {
        common::joint_map(
            r,
            c,
            |_, _| 0,
            |y, x| (!exact(u * 2 + y, x)).then_some(7),
            ac,
        )
    });
    let got = decode(&file).unwrap().1;
    for by in 0..10 {
        for bx in 0..bw {
            if exact(by, bx) {
                for y in 0..8 {
                    let a = ((by * 8 + y) * w + bx * 8) * 3;
                    assert_eq!(got[a..a + 24], px[a..a + 24], "exact block ({by}, {bx})");
                }
            }
        }
    }
    // Exact blocks also participate in the common whole/stream/reader checks.
    common::decode_all(&file, &info);
}

// ---- inherited encoder study arms (lab builds) ----

#[cfg(feature = "lab")]
fn small_cfg(cfg: ImiEncoderConfig, force: Option<u8>, merges: bool) -> ImiEncoderConfig {
    let mut c = cfg;
    c = c.tune_lossy(|t| {
        t.lab.switches.small = Some(Box::new(imij::SmallLab {
            split: force,
            ..imij::SmallLab::on()
        }))
    });
    c = c.tune_lossy(|t| t.lab.switches.superblocks = Some(merges));
    c
}

#[cfg(feature = "lab")]
fn lab_files() -> Vec<(String, Vec<u8>)> {
    use imij::LossyColor;
    let mut out = Vec::new();
    for (w, h, ch, sample) in [
        (200u32, 136u32, Channels::Rgb, SampleType::U8),
        (131, 70, Channels::Rgba, SampleType::U8),
        (96, 80, Channels::Gray, SampleType::U8),
        (120, 64, Channels::Rgb, SampleType::U16),
        (72, 56, Channels::Rgb, SampleType::F32),
    ] {
        let info = ImageInfo::new(w, h, ch, sample);
        let px = image(&info);
        for color in [LossyColor::Xyb, LossyColor::YCoCg] {
            for (entropy, rows, groups) in [
                (LossyEntropy::RunLevel, 2u32, 0u32),
                (LossyEntropy::SplitHuf, 3, 0),
                (LossyEntropy::RunLevel, 4, 40),
            ] {
                for (force, merges) in [(None, false), (Some(7u8), false), (None, true)] {
                    for q in [8.0f32, 50.0, 97.0] {
                        let mut cfg = imij::ImiEncoderConfig::lossy(q)
                            .tune_lossy(|t| t.color = color)
                            .tune_lossy(|t| t.entropy = entropy)
                            .tune_lossy(|t| t.stripe_block_rows = rows);
                        cfg = cfg.with_column_groups(Some(groups));
                        let cfg = small_cfg(cfg, force, merges);
                        let file = enc(&px, &info, &cfg);
                        out.push((
                            format!(
                                "{w}x{h}-{ch:?}-{sample:?}-{color:?}-{entropy:?}-r{rows}-g{groups}-f{force:?}-m{merges}-q{q}"
                            ),
                            file,
                        ));
                    }
                }
            }
        }
    }
    out
}

#[cfg(feature = "lab")]
#[test]
fn encoder_small_transforms_combine_and_routes_agree() {
    let mut split = 0;
    for (name, file) in lab_files() {
        let (info, _) = decode(&file).unwrap();
        if feature_mask(&file) & SMALL_TRANSFORMS != 0 {
            split += 1;
        }
        let _ = name;
        every_route(&file, &info);
    }
    assert!(split > 150, "{split} files with 4x4 planes");
}

/// With small transforms on and nothing split the file is the plain file.
#[cfg(feature = "lab")]
#[test]
fn files_without_splits_are_the_plain_files() {
    let info = ImageInfo::new(96, 64, Channels::Rgb, SampleType::U8);
    // Flat blocks: nothing to gain from 4x4 planes.
    let px: Vec<u8> = (0..96 * 64 * 3)
        .map(|i| {
            let (x, y) = ((i / 3) % 96, (i / 3) / 96);
            ((x / 8) * 19 + (y / 8) * 31 + (i % 3) * 50) as u8
        })
        .collect();
    for q in [20.0f32, 60.0, 90.0] {
        let on = enc(
            &px,
            &info,
            &small_cfg(imij::ImiEncoderConfig::lossy(q), None, false),
        );
        assert_eq!(feature_mask(&on) & SMALL_TRANSFORMS, 0, "q{q}");
        let mut off = imij::ImiEncoderConfig::lossy(q);
        off = off.tune_lossy(|t| {
            t.lab.switches.small = Some(Box::new(imij::SmallLab {
                on: Some(false),
                ..imij::SmallLab::default()
            }))
        });
        assert!(on == enc(&px, &info, &off), "q{q}");
    }
}

/// Outside merges, below the trellis efforts, the 4x4 choice prices with
/// one effort's tables, so the pixels are the same at every such effort
/// (`ImiEncoderConfig::effort`): the explicit coders and the non-merging tiers.
#[cfg(feature = "lab")]
#[test]
fn small_transform_pixels_do_not_depend_on_effort() {
    use imij::LossyColor;
    let info = ImageInfo::new(200, 136, Channels::Rgb, SampleType::U8);
    let px = image(&info);
    let bases = [
        (
            "xyb-rl",
            imij::ImiEncoderConfig::lossy(60.0).tune_lossy(|t| t.entropy = LossyEntropy::RunLevel),
        ),
        (
            "ycocg-huf",
            imij::ImiEncoderConfig::lossy(60.0)
                .tune_lossy(|t| t.color = LossyColor::YCoCg)
                .tune_lossy(|t| t.entropy = LossyEntropy::SplitHuf),
        ),
        (
            "balanced",
            imij::ImiEncoderConfig::lossy(60.0).with_decode_speed(DecodeSpeed::Balanced),
        ),
        (
            "fastest",
            imij::ImiEncoderConfig::lossy(60.0).with_decode_speed(DecodeSpeed::Fastest),
        ),
    ];
    for (what, base) in bases {
        for q in [10.0f32, 60.0, 95.0] {
            let cfg = |effort: f32| {
                let mut c = base.clone().with_effort(effort);
                c = c.with_quality(q);
                small_cfg(c, None, false)
            };
            let reference = enc(&px, &info, &cfg(7.5));
            assert_ne!(
                feature_mask(&reference) & SMALL_TRANSFORMS,
                0,
                "{what} q{q}: no 4x4 planes"
            );
            let want = decode(&reference).unwrap().1;
            for effort in [0.0f32, 2.5, 5.0, 8.99] {
                let got = decode(&enc(&px, &info, &cfg(effort))).unwrap().1;
                assert!(got == want, "{what} q{q} effort {effort}");
            }
        }
    }
}

/// AQ, exact blocks (Compact) and the effort-10 trellis with 4x4 planes.
#[cfg(feature = "lab")]
#[test]
fn small_transforms_with_aq_exact_blocks_and_the_trellis() {
    let info = ImageInfo::new(192, 160, Channels::Rgb, SampleType::U8);
    let px = image(&info);
    for (what, base) in [
        ("aq", {
            let mut c = imij::ImiEncoderConfig::lossy(40.0);
            c = c.tune_lossy(|t| {
                t.aq_gamma = 0.5;
            });
            c = c.tune_lossy(|t| {
                t.aq_r0 = 1.0;
            });
            c
        }),
        (
            "compact",
            imij::ImiEncoderConfig::lossy(40.0).with_decode_speed(DecodeSpeed::Compact),
        ),
        ("e10", imij::ImiEncoderConfig::lossy(40.0).with_effort(10.0)),
        (
            "e10-huf",
            imij::ImiEncoderConfig::lossy(40.0)
                .with_effort(10.0)
                .tune_lossy(|t| t.entropy = LossyEntropy::SplitHuf),
        ),
    ] {
        for force in [None, Some(7u8)] {
            let file = enc(
                &px,
                &info,
                &small_cfg(base.clone(), force, what == "compact"),
            );
            assert_ne!(
                feature_mask(&file) & SMALL_TRANSFORMS,
                0,
                "{what} {force:?}"
            );
            every_route(&file, &info);
        }
    }
}

/// A production opt-in reaches the existing selector with no lab switches,
/// keeps strided source bytes identical, and preserves every decode route.
#[test]
fn production_compact_opt_in_selects_4x4_and_supports_stride() {
    let info = ImageInfo::new(160, 136, Channels::Rgba, SampleType::U8);
    let px = image(&info);
    let mut plain_cfg = imij::ImiEncoderConfig::lossy(95.0).with_decode_speed(DecodeSpeed::Compact);
    plain_cfg = plain_cfg.tune_lossy(|t| {
        t.exact_blocks = Some(0.0);
    });
    assert!(
        !plain_cfg
            .lossy_tuning()
            .cloned()
            .unwrap_or_default()
            .small_transforms
    );
    let plain = enc(&px, &info, &plain_cfg);
    assert_eq!(feature_mask(&plain) & SMALL_TRANSFORMS, 0);
    let cfg = plain_cfg.clone().tune_lossy(|t| t.small_transforms = true);
    let file = enc(&px, &info, &cfg);
    assert_ne!(feature_mask(&file) & SMALL_TRANSFORMS, 0);
    assert!(
        common::payload(&file[common::data_payload(&file)], &info, true)
            .head_record(H_PARTITIONS)
            .is_some()
    );
    let reference = every_route(&file, &info);
    // The selected color transforms leave alpha exact.
    for (got, src) in reference
        .as_chunks::<4>()
        .0
        .iter()
        .zip(px.as_chunks::<4>().0.iter())
    {
        assert_eq!(got[3], src[3]);
    }
    let rb = info.row_bytes();
    let stride = rb + 19;
    let mut padded = vec![0xa5; stride * info.height as usize];
    for (row, src) in padded.chunks_exact_mut(stride).zip(px.chunks_exact(rb)) {
        row[..rb].copy_from_slice(src);
    }
    for threads in [1, 2, 4, 8] {
        let mut threaded = cfg.clone();
        threaded = threaded.with_threads(threads);
        let got = threaded
            .encode_legacy_samples(
                imij::ImageRef::new(&padded, stride, &info),
                &Metadata::default(),
            )
            .unwrap();
        assert_eq!(got, file, "strided encoding with {threads} threads");
        let mut pixels = vec![0; reference.len()];
        imij::decode_into_threads(&file, &mut pixels, rb, threads).unwrap();
        assert_eq!(pixels, reference, "decode with {threads} threads");
    }
    assert_eq!(
        enc(
            &px,
            &info,
            &cfg.clone().tune_lossy(|t| t.small_transforms = false)
        ),
        plain
    );
    #[cfg(feature = "lab")]
    {
        let mut lab_cfg = plain_cfg;
        lab_cfg =
            lab_cfg.tune_lossy(|t| t.lab.switches.small = Some(Box::new(imij::SmallLab::on())));
        assert_eq!(enc(&px, &info, &lab_cfg), file, "same inherited selector");
    }
}

/// Intra animation keyframes and pages match the same opted-in image coded
/// independently, including exact alpha and padded multi-thread decoding.
#[test]
fn production_intra_frames_and_pages_opt_in() {
    use imij::{FrameConfig, FrameDecoder, FrameDuration, FrameEncoder, FrameKind};

    let info = ImageInfo::new(160, 136, Channels::Rgba, SampleType::U8);
    let first = image(&info);
    let mut second = first.clone();
    for pixel in second.as_chunks_mut::<4>().0 {
        pixel[0] = pixel[0].saturating_add(3);
        pixel[3] = pixel[3].wrapping_add(37);
    }
    let sources = [first, second];
    let mut cfg = imij::ImiEncoderConfig::lossy(95.0)
        .with_decode_speed(DecodeSpeed::Compact)
        .tune_lossy(|t| t.small_transforms = true);
    cfg = cfg.tune_lossy(|t| {
        t.exact_blocks = Some(0.0);
    });
    let references: Vec<_> = sources
        .iter()
        .map(|src| {
            let file = enc(src, &info, &cfg);
            assert_ne!(feature_mask(&file) & SMALL_TRANSFORMS, 0);
            every_route(&file, &info)
        })
        .collect();
    for (kind, inter) in [
        (FrameKind::Animation, false),
        (FrameKind::Animation, true),
        (FrameKind::Pages, false),
        (FrameKind::MixedPages, false),
    ] {
        let coding = cfg.clone();
        let mut frames = FrameConfig::new(coding.clone());
        frames.inter = inter;
        // Even with inter enabled, every changed frame is an intra keyframe.
        frames.keyframe_interval = 1;
        let mut encoder = match kind {
            FrameKind::Animation => {
                FrameEncoder::animation(info.clone(), Metadata::default(), frames)
            }
            FrameKind::Pages => FrameEncoder::pages(info.clone(), Metadata::default(), frames),
            FrameKind::MixedPages => FrameEncoder::mixed_pages(frames),
            _ => unreachable!(),
        };
        for src in &sources {
            match kind {
                FrameKind::Animation => encoder
                    .push_frame(src, info.row_bytes(), FrameDuration::from_millis(10))
                    .unwrap(),
                FrameKind::Pages => encoder.push_page(src, info.row_bytes(), &info).unwrap(),
                FrameKind::MixedPages => encoder
                    .push_page_with(src, info.row_bytes(), &info, &Metadata::default(), &coding)
                    .unwrap(),
                _ => unreachable!(),
            }
        }
        let file = encoder.finish().unwrap();
        assert_ne!(feature_mask(&file) & SMALL_TRANSFORMS, 0);
        assert_eq!(decode(&file).unwrap().1, references[0]);
        for threads in [1, 4] {
            let mut decoder = FrameDecoder::new(&file).unwrap();
            assert_eq!(decoder.kind(), kind);
            let rb = info.row_bytes();
            let stride = rb + 19;
            for (index, (src, want)) in sources.iter().zip(&references).enumerate() {
                let header = decoder.next_frame().unwrap().unwrap();
                assert_eq!(header.index, index);
                assert!(!header.uses_previous, "{kind:?} inter={inter}");
                let mut got = vec![0xa5; stride * info.height as usize];
                decoder.decode_into(&mut got, stride, threads).unwrap();
                for ((row, expected), original) in got
                    .chunks_exact(stride)
                    .zip(want.chunks_exact(rb))
                    .zip(src.chunks_exact(rb))
                {
                    assert_eq!(&row[..rb], expected);
                    assert!(row[rb..].iter().all(|&p| p == 0xa5));
                    for (pixel, source) in row[..rb]
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .zip(original.as_chunks::<4>().0.iter())
                    {
                        assert_eq!(pixel[3], source[3]);
                    }
                }
            }
            assert!(decoder.next_frame().unwrap().is_none());
        }
    }
    // A real reference-predicted encode remains outside production admission.
    let mut frames = FrameConfig::new(cfg);
    frames.inter = true;
    frames.keyframe_interval = 0;
    let mut encoder = FrameEncoder::animation(info.clone(), Metadata::default(), frames);
    encoder
        .push_frame(
            &sources[0],
            info.row_bytes(),
            FrameDuration::from_millis(10),
        )
        .unwrap();
    assert_eq!(
        encoder
            .push_frame(
                &sources[1],
                info.row_bytes(),
                FrameDuration::from_millis(10)
            )
            .err(),
        Some(Error::Unsupported(
            "small transforms require Compact U8 stills with default layouts, effort below 9 and no trellis"
        ))
    );
}

#[test]
fn production_opt_in_rejects_unsupported_configuration() {
    let info = ImageInfo::new(16, 16, Channels::Rgb, SampleType::U8);
    let px = image(&info);
    let compact = imij::ImiEncoderConfig::lossy(90.0)
        .with_decode_speed(DecodeSpeed::Compact)
        .tune_lossy(|t| t.small_transforms = true);
    for cfg in [
        imij::ImiEncoderConfig::lossy(90.0).tune_lossy(|t| t.small_transforms = true),
        compact.clone().with_decode_speed(DecodeSpeed::Balanced),
        compact.clone().with_decode_speed(DecodeSpeed::Fast),
        compact.clone().with_decode_speed(DecodeSpeed::Fastest),
        {
            let mut cfg = compact.clone();
            cfg = cfg.with_effort(9.0);
            cfg
        },
        {
            let mut cfg = compact.clone();
            cfg = cfg.with_effort(10.0);
            cfg
        },
        {
            let mut cfg = compact.clone();
            cfg = cfg.tune_lossy(|t| {
                t.channel_layouts[0] = imij::LossyChannelLayout::new(
                    imij::LossyTransform::Dct4,
                    imij::ChannelSampling::Full,
                )
            });
            cfg
        },
    ] {
        assert_eq!(
            cfg.encode_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default()
            )
            .err(),
            Some(Error::Unsupported(
                "small transforms require Compact U8 stills with default layouts, effort below 9 and no trellis"
            ))
        );
    }
    let wide = ImageInfo::new(16, 16, Channels::Rgb, SampleType::U16);
    let pixels = image(&wide);
    assert!(matches!(
        compact.encode_samples(
            imij::ImageRef::new(&pixels, wide.row_bytes(), &wide),
            &Metadata::default()
        ),
        Err(Error::Unsupported(_))
    ));
}
