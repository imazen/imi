//! Seed bank integrity and seeded-file exactness (lab).

use super::*;
use crate::lossy::tests::{both, test_image};
use crate::test_sha256::sha256;
use crate::{Channels, DecodeSpeed, ImageInfo, LossyColor, LossyConfig, LossyEntropy, Metadata};
use crate::{SampleType, encode_lossy};
use alloc::string::String;
use alloc::vec::Vec;

/// The generator's bank 1 (benchmarks/entropy_seeds_2026-10-07/).
const BANK1_SHA256: &str = "79048930e6eedc048be0dfd752456119bee344dc81dcc863be329b33e293e332";

// Legacy selector notation in these retained tests; production inspection
// returns the full bank ID and independent low-three-bit scope mask.
fn lossy_entropy_seed(file: &[u8]) -> Result<(u8, u8), Error> {
    let (e, id, scope) = crate::seeds::lossy_entropy_seed(file)?;
    Ok((
        e,
        if scope == 0 {
            0
        } else {
            u8::try_from(id).unwrap() | (scope << 4)
        },
    ))
}

#[test]
fn sha256_matches_known_vectors() {
    let hex = |d: [u8; 32]| {
        d.iter()
            .map(|b| alloc::format!("{b:02x}"))
            .collect::<String>()
    };
    assert_eq!(
        hex(sha256(b"")),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        hex(sha256(b"abc")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn bank_blob_is_pinned() {
    let hex: String = sha256(bank_blob(1).unwrap())
        .iter()
        .map(|b| alloc::format!("{b:02x}"))
        .collect();
    assert_eq!(hex, BANK1_SHA256, "imij/src/seeds/bank1.bin changed");
}

/// The loader bounds each directory entry and seed once: a bad count,
/// misaligned or out-of-blob section, or seed offsets past the data give no
/// seeds or a corrupt-layout error, never a panic.
#[test]
fn bank_loader_checks_entries_and_offsets() {
    let blob = bank_blob(1).unwrap();
    let u32at = |b: &[u8], i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap()) as usize;
    // Entry 0 is the run-level EOB kind.
    assert_eq!(blob[8], Kind::RlEob as u8);
    let open = |patch: &dyn Fn(&mut Vec<u8>)| {
        let mut b = blob.to_vec();
        patch(&mut b);
        let words = Vec::leak(b).as_chunks::<4>().0;
        Bank {
            id: 1,
            header_id: 1,
            words,
        }
    };
    let set = |b: &mut Vec<u8>, at: usize, v: usize| {
        b[at..at + 4].copy_from_slice(&(v as u32).to_le_bytes())
    };
    // Entry 1 (run-level AC): count at byte 24, section offset at 28.
    assert_eq!(blob[20], Kind::RlAc as u8);
    let at1 = u32at(blob, 28);
    for (what, patch) in [
        (
            "no seeds",
            &(|b: &mut Vec<u8>| set(b, 24, 0)) as &dyn Fn(&mut Vec<u8>),
        ),
        ("257 seeds", &|b: &mut Vec<u8>| set(b, 24, 257)),
        ("misaligned", &|b: &mut Vec<u8>| set(b, 28, at1 + 2)),
        ("past the blob", &|b: &mut Vec<u8>| set(b, 28, b.len())),
        ("offsets past the blob", &|b: &mut Vec<u8>| {
            set(b, 28, b.len() - 4)
        }),
        ("bad magic", &|b: &mut Vec<u8>| b[0] = b'X'),
        ("too many kinds", &|b: &mut Vec<u8>| b[6] = 255),
        ("too few kinds", &|b: &mut Vec<u8>| b[6] = 1),
    ] {
        let bank = open(patch);
        let found = bank.valid() && bank.kind(Kind::RlAc, [0; 3]).is_some();
        assert!(!found, "{what}");
    }
    let at0 = u32at(blob, 16);
    // Last seed's end offset past the data: that seed is corrupt, the others decode.
    let n = u32at(blob, 12);
    let bank = open(&|b: &mut Vec<u8>| set(b, at0 + 4 * n, 1 << 30));
    let ks = bank.kind(Kind::RlEob, [0; 3]).unwrap();
    assert!(ks.weights(0, crate::lossy::coef::ALPHA).is_ok());
    assert!(matches!(
        ks.weights(n - 1, crate::lossy::coef::ALPHA),
        Err(Error::Corrupt("seed bank layout"))
    ));
    assert!(matches!(
        ks.weights(n, crate::lossy::coef::ALPHA),
        Err(Error::Corrupt("seed bank layout"))
    ));
}

#[test]
fn bank_layout_and_seeds_are_valid() {
    let blob = bank_blob(1).unwrap();
    let b = bank(1).unwrap();
    let u32at = |i: usize| u32::from_le_bytes(blob[i..i + 4].try_into().unwrap()) as usize;
    assert_eq!(&blob[..4], b"IMSB");
    assert_eq!((blob[4], blob[5], blob[7]), (1, 1, 0));
    assert_eq!(blob.len() % 4, 0);
    let k = blob[6] as usize;
    let mut expect_at = 8 + 12 * k;
    let mut prev = None;
    for e in 0..k {
        let d = 8 + 12 * e;
        let key = [blob[d], blob[d + 1], blob[d + 2], blob[d + 3]];
        assert!(prev < Some(key), "directory order");
        prev = Some(key);
        let (n, at) = (u32at(d + 4), u32at(d + 8));
        assert!((1..=256).contains(&n));
        assert_eq!(at, expect_at, "sections in directory order, contiguous");
        let offs: Vec<usize> = (0..=n).map(|i| u32at(at + 4 * i)).collect();
        assert_eq!(offs[0], 0);
        assert!(offs.windows(2).all(|w| w[0] < w[1]), "offsets increase");
        let end = at + 4 * (n + 1) + offs[n];
        let padded = end.next_multiple_of(4);
        assert!(blob[end..padded].iter().all(|&v| v == 0), "zero padding");
        expect_at = padded;
        let kind = match key[0] {
            0 => Kind::RlEob,
            1 => Kind::RlAc,
            4 => Kind::HufCount,
            5 => Kind::HufWidth,
            6 => Kind::Lossless,
            v => panic!("kind {v}"),
        };
        let tc = [key[1], key[2], key[3]];
        if kind != Kind::Lossless {
            assert_eq!(tc, [0; 3]);
        }
        let ks = b.kind(kind, tc).unwrap();
        assert_eq!(ks.len(), n);
        for i in 0..n {
            match kind {
                Kind::HufCount | Kind::HufWidth => {
                    let alpha = if kind == Kind::HufCount {
                        crate::lossy::huf::CNT_ALPHA
                    } else {
                        crate::lossy::huf::WID_ALPHA
                    };
                    let l = ks.lengths(i, alpha).unwrap();
                    let log = crate::lossy::huf::LOG;
                    assert!(l.iter().all(|&v| v as u32 <= log));
                    let kraft: u64 = l
                        .iter()
                        .filter(|&&v| v > 0)
                        .map(|&v| 1u64 << (log - v as u32))
                        .sum();
                    assert!(kraft > 0 && kraft <= 1 << log, "seed {i} of {kind:?}");
                }
                _ => {
                    let alpha = codec::alphabet(kind as u8, tc);
                    let (w, _) = ks.weights(i, alpha).unwrap();
                    assert!(crate::rans::weights_to_freq(&w).is_some());
                    for (s, &v) in w.iter().enumerate() {
                        if v == 0 {
                            continue;
                        }
                        match kind {
                            Kind::RlEob => assert!(s < 64),
                            Kind::RlAc => assert!(crate::lossy::coef::valid_ac_symbol(s)),
                            // No run token past the run cap (SPEC 3.2): a
                            // file that selects the seed stays valid.
                            Kind::Lossless => {
                                let [k, m, d] = tc.map(u32::from);
                                let scheme = crate::tokens::TokenCfg::v1(k, m, d);
                                assert!(s < crate::rans::run_cap_end(&scheme), "seed {i}");
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    assert_eq!(expect_at, blob.len(), "no trailing bytes");
}

#[test]
fn cost16_matches_float() {
    for f in 1..=crate::rans::M {
        let want = (16.0 * (10.0 - crate::mathx::log2(f as f64))).round() as u8;
        assert_eq!(costs::cost16(f), want, "f = {f}");
    }
}

/// The compile-time cost table holds, for every seed and symbol, the cost of
/// the table the decoder builds from that seed.
#[test]
fn cost_table_matches_runtime_tables() {
    let b = bank(1).unwrap();
    let blob = bank_blob(1).unwrap();
    for e in 0..blob[6] as usize {
        let d = 8 + 12 * e;
        let (kind_id, tc) = (blob[d], [blob[d + 1], blob[d + 2], blob[d + 3]]);
        let kind = match kind_id {
            0 => Kind::RlEob,
            1 => Kind::RlAc,
            4 => Kind::HufCount,
            5 => Kind::HufWidth,
            _ => Kind::Lossless,
        };
        let alpha = codec::alphabet(kind_id, tc);
        let set = kind_set(b, kind, tc, alpha).unwrap();
        let width = costs::width(kind_id, tc);
        for i in 0..set.seeds.len() {
            let row = &set.cost[i * width..(i + 1) * width];
            let t = set.table(i);
            for (sym, &c) in row.iter().enumerate() {
                let want = if t.lens.is_empty() {
                    match t.freq[sym] {
                        0 => costs::ABSENT,
                        f => (16.0 * (10.0 - crate::mathx::log2(f as f64))).round() as u8,
                    }
                } else {
                    match t.lens[sym] {
                        0 => costs::ABSENT,
                        l => l * 16,
                    }
                };
                assert_eq!(c, want, "kind {kind_id} {tc:?} seed {i} symbol {sym}");
            }
            // EOB seeds code nothing past the cost table's width.
            if t.lens.is_empty() {
                assert!(t.freq[width..].iter().all(|&f| f == 0));
            }
        }
    }
}

/// Encodes `px` with and without bank 1; both must decode (SIMD and scalar)
/// to the same pixels. Returns (unseeded bytes, seeded bytes, seed byte).
fn ab(px: &[u8], info: &ImageInfo, cfg: &LossyConfig) -> (usize, usize, u8) {
    let stride = info.row_bytes();
    let mut off = cfg.clone();
    off.seed_bank = 0;
    // The declaration below is checked without the unit table hint, then
    // with it at the end.
    let enc = |c: &LossyConfig| encode_lossy(px, stride, info, &Metadata::default(), c).unwrap();
    let a = crate::v1::without_hint(|| enc(&off));
    // The default seeds from bank 1.
    assert_eq!(cfg.seed_bank, 1);
    let b = crate::v1::without_hint(|| enc(cfg));
    let (_, seed) = lossy_entropy_seed(&b).unwrap();
    let da = crate::decode(&a).unwrap().1;
    let db = crate::decode(&b).unwrap().1;
    assert_eq!(da, db, "seeded file decodes differently ({cfg:?})");
    let (s, r) = both(&b);
    assert_eq!(s.unwrap(), r.unwrap());
    if seed == 0 {
        assert_eq!(a, b, "no seeds used, yet the file changed");
    }
    // A seeded file declares `seed_bank_1` (and is still no larger, its
    // feature mask included).
    let seeds = |f: &[u8]| {
        crate::v1::parse_header_lossy(f)
            .unwrap()
            .3
            .contains(crate::parse::header::Feature::SeedBankIds)
    };
    assert!(!seeds(&a));
    assert_eq!(seeds(&b), seed != 0);
    assert!(b.len() <= a.len(), "seeds made the file larger ({cfg:?})");
    // With a unit table hint (two or more units) the declaration is the same;
    // seeds still never make the file larger.
    let (ta, tb) = (enc(&off), enc(cfg));
    assert_eq!(crate::decode(&ta).unwrap().1, da);
    assert_eq!(crate::decode(&tb).unwrap().1, db);
    if ta == a {
        assert_eq!(tb, b, "one unit: no table either way");
    }
    assert!(!seeds(&ta));
    assert_eq!(seeds(&tb), seed != 0);
    assert!(tb.len() <= ta.len(), "seeds made the file larger ({cfg:?})");
    (a.len(), b.len(), seed)
}

#[test]
fn seeded_files_decode_to_the_same_pixels() {
    let mut used = [0u32; 3];
    for &(w, h) in &[(16, 16), (64, 40), (96, 200)] {
        for rgba in [false, true] {
            let ch = if rgba { Channels::Rgba } else { Channels::Rgb };
            let info = ImageInfo::new(w, h, ch, SampleType::U8);
            let rgb = test_image(w, h, w + 7 * h);
            // Alpha: noise at 16 x 16, a gradient, a 4 x 4 checker mask.
            let alpha = |i: u32| {
                let (x, y) = (i % w, i / w);
                match w {
                    16 => (i * 37 % 251) as u8,
                    64 => (x * 255 / w) as u8,
                    _ => 255 * ((x / 4 + y / 4) % 2) as u8,
                }
            };
            let px: Vec<u8> = if rgba {
                rgb.chunks(3)
                    .enumerate()
                    .flat_map(|(i, p)| [p[0], p[1], p[2], alpha(i as u32)])
                    .collect()
            } else {
                rgb
            };
            for q in [3.0, 20.0, 60.0, 95.0] {
                for color in [LossyColor::Xyb, LossyColor::YCoCg] {
                    for mode in 0..3 {
                        let cfg = LossyConfig::new(q).with_color(color);
                        let cfg = match mode {
                            0 => cfg,
                            1 => cfg.with_entropy(LossyEntropy::SplitHuf),
                            _ => cfg.with_decode_speed(DecodeSpeed::Balanced),
                        };
                        let (_, _, seed) = ab(&px, &info, &cfg);
                        for (j, bit) in [SCOPE_AC, SCOPE_DC, SCOPE_ALPHA].iter().enumerate() {
                            used[j] += (seed & bit != 0) as u32;
                        }
                        assert!(seed == 0 || seed & 15 == 1);
                        assert!(rgba || seed & SCOPE_ALPHA == 0);
                    }
                }
            }
        }
    }
    // Small images must actually exercise the AC and DC scopes (alpha seeds
    // need binary masks of some sizes: `stream_tests::scope_coverage`).
    assert!(used[0] > 0 && used[1] > 0, "seed scopes used: {used:?}");
}

/// Selector grammar: an index past the kind's last seed, or a seed for a
/// kind / token scheme the bank has none of, is corrupt.
#[test]
fn selectors_reject_bad_indices() {
    let b = bank(1).unwrap();
    let read = |seeds: Option<KindSeeds>, fields: &[(u32, u32)]| {
        let mut w = BitWriter::new();
        for &(v, n) in fields {
            w.put(v, n);
        }
        let buf = w.finish();
        read_selector(&mut BitReader::new(&buf), seeds)
    };
    let ac = b.kind(Kind::RlAc, [0; 3]).unwrap();
    assert_eq!((ac.len(), ac.index_bits()), (255, 8));
    assert!(matches!(read(Some(ac), &[(0, 1)]), Ok(None)));
    assert!(matches!(read(Some(ac), &[(1, 1), (254, 8)]), Ok(Some(254))));
    assert!(matches!(
        read(Some(ac), &[(1, 1), (255, 8)]),
        Err(Error::Corrupt("seed index out of range"))
    ));
    // Token schemes the encoder meets but the bank has no seeds for.
    for tc in [[4, 2, 14], [4, 2, 8], [1, 2, 2]] {
        assert!(b.kind(Kind::Lossless, tc).is_none(), "{tc:?}");
    }
    assert!(matches!(
        read(None, &[(1, 1), (0, 8)]),
        Err(Error::Corrupt("seed for a table kind without seeds"))
    ));
    // Split-Huffman's rANS-shaped kinds have no seed kind (its tables are
    // the Huffman kinds).
    assert!(Kind::of_lossy(LossyEntropy::SplitHuf, false).is_none());
    // Every in-range index of every kind reads back.
    for (kind, tc) in [
        (Kind::RlEob, [0; 3]),
        (Kind::HufCount, [0; 3]),
        (Kind::Lossless, [5, 2, 8]),
    ] {
        let ks = b.kind(kind, tc).unwrap();
        for i in 0..ks.len() {
            let got = read(Some(ks), &[(1, 1), (i as u32, ks.index_bits())]);
            assert!(matches!(got, Ok(Some(j)) if j == i));
        }
    }
}

#[test]
fn seeded_corrupt_streams_do_not_panic() {
    let (w, h) = (48, 40);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let px = test_image(w, h, 5);
    let mut found = 0;
    for (q, mode) in [
        (10.0, LossyEntropy::RunLevel),
        (10.0, LossyEntropy::SplitHuf),
    ] {
        let mut cfg = LossyConfig::new(q).with_entropy(mode);
        cfg.seed_bank = 1;
        let enc = encode_lossy(&px, w as usize * 3, &info, &Metadata::default(), &cfg).unwrap();
        if lossy_entropy_seed(&enc).unwrap().1 == 0 {
            continue;
        }
        found += 1;
        let mut s = 777u32;
        for i in 0..3000 {
            let mut d = enc.clone();
            for _ in 0..1 + i % 4 {
                s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
                // Bias toward the head, where selectors live.
                let at = (s >> 8) as usize % d.len().min(if i % 2 == 0 { 400 } else { d.len() });
                d[at] ^= 1 << (s % 8);
            }
            if i % 7 == 0 {
                d.truncate((s as usize >> 3) % d.len());
            }
            let _ = crate::decode(&d);
            let _ = both(&d);
        }
    }
    assert!(found > 0, "no seeded file to corrupt");
}

#[test]
fn unknown_bank_is_corrupt_and_scopes_are_checked() {
    let (w, h) = (32, 24);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let px = test_image(w, h, 11);
    let mut cfg = LossyConfig::new(5.0);
    cfg.seed_bank = 1;
    let enc = encode_lossy(&px, w as usize * 3, &info, &Metadata::default(), &cfg).unwrap();
    let enc = crate::lossy::legacy_seed_fixture(&enc);
    let (_, seed) = lossy_entropy_seed(&enc).unwrap();
    assert_ne!(seed, 0, "tiny q5 image should use seeds");
    // The seed byte is the only byte equal to `seed` right after the
    // envelope's fixed fields; find it by re-reading with each candidate.
    // A bank with no feature in this version is gated syntax (SPEC 0.3).
    let at = (0..enc.len())
        .filter(|&i| enc[i] == seed)
        .find(|&i| {
            let mut d = enc.clone();
            d[i] = (seed & 0xF0) | 2;
            crate::decode(&d).err() == Some(crate::Error::Corrupt("seed bank without its feature"))
        })
        .expect("seed byte position");
    let mut d = enc.clone();
    d[at] = seed & 0x0F;
    assert!(crate::decode(&d).is_err(), "no scope bits");
    let mut d = enc.clone();
    d[at] = seed | 0x80;
    assert!(crate::decode(&d).is_err(), "reserved bit");
    let mut d = enc.clone();
    d[at] = seed | SCOPE_ALPHA;
    assert!(crate::decode(&d).is_err(), "alpha scope without alpha");
}

/// Gray pages and low-contrast noise give DC slots token schemes the bank
/// has no seeds for (e.g. (4, 2, 14)) next to seeded ones; those tables stay
/// explicit (a 2026-10-07 encoder panicked here).
#[test]
fn slots_without_bank_seeds_stay_explicit() {
    let (w, h) = (96u32, 64u32);
    let mut s = 0x2545_f491u32;
    let mut noise = || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        (s % 7) as i32 - 3
    };
    let lowcon: Vec<u8> = (0..w * h)
        .flat_map(|_| {
            let g = (120 + noise()) as u8;
            [g, g + 1, g]
        })
        .collect();
    let text: Vec<u8> = (0..h)
        .flat_map(|y| {
            (0..w).flat_map(move |x| {
                let g = if (x / 3 + y / 5) % 7 == 0 && y % 8 < 5 {
                    30
                } else {
                    235
                };
                [g, g, g]
            })
        })
        .collect();
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let mut seeded = 0;
    for px in [&lowcon, &text] {
        for q in [1.0, 5.0, 12.0, 30.0, 60.0, 90.0, 100.0] {
            for color in [LossyColor::Xyb, LossyColor::YCoCg] {
                for sh in [false, true] {
                    let mut cfg = LossyConfig::new(q).with_color(color);
                    if sh {
                        cfg = cfg.with_entropy(LossyEntropy::SplitHuf);
                    }
                    seeded += (ab(px, &info, &cfg).2 != 0) as u32;
                }
            }
        }
    }
    assert!(seeded > 0);
}

/// The registration check accepts bank 1 as built and agrees with its
/// compile-time cost table.
#[test]
fn check_blob_accepts_bank1() {
    let blob = bank_blob(1).unwrap();
    let words = blob.as_chunks::<4>().0;
    assert_eq!(check_blob(words, 1), Ok(BANK1_COSTS.len()));
    assert!(check_blob(words, 2).is_err(), "header id must match");
}

/// Lab bank registration, on a registry of its own (the process registry
/// stays untouched, so other tests still see bank 13 as unknown): a
/// malformed blob is refused before anything is kept; an id registers once.
/// The process registry and a seeded encode are tested in
/// `tests/lab_seed_bank.rs`, a process of its own.
#[test]
fn register_bank_rejects_malformed_and_duplicate_ids() {
    const ID: u8 = 13;
    let reg = lab_banks::Registry::new();
    let mut good = bank_blob(1).unwrap().to_vec();
    good[5] = ID;
    let u32at = |b: &[u8], i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap()) as usize;
    let set = |b: &mut Vec<u8>, at: usize, v: usize| {
        b[at..at + 4].copy_from_slice(&(v as u32).to_le_bytes())
    };
    let (n0, at0) = (u32at(&good, 12), u32at(&good, 16));
    // First byte of zero padding after some section.
    let pad = (0..good[6] as usize)
        .map(|e| {
            let (n, at) = (u32at(&good, 8 + 12 * e + 4), u32at(&good, 8 + 12 * e + 8));
            at + 4 * (n + 1) + u32at(&good, at + 4 * n)
        })
        .find(|end| end % 4 != 0)
        .expect("a padded section");
    let bad = Error::InvalidInput("bad seed bank blob");
    let cases: Vec<(&str, Box<dyn Fn(&mut Vec<u8>)>)> = vec![
        ("empty", Box::new(|b| b.clear())),
        ("bad magic", Box::new(|b| b[0] = b'X')),
        ("layout 2", Box::new(|b| b[4] = 2)),
        ("bank id 1", Box::new(|b| b[5] = 1)),
        ("bank id 16", Box::new(|b| b[5] = 16)),
        ("reserved byte", Box::new(|b| b[7] = 1)),
        ("no kinds", Box::new(|b| b[6] = 0)),
        ("too many kinds", Box::new(|b| b[6] = 255)),
        ("too few kinds", Box::new(|b| b[6] = 1)),
        ("length not a multiple of 4", Box::new(|b| b.push(0))),
        ("truncated", Box::new(|b| b.truncate(b.len() - 4))),
        ("trailing word", Box::new(|b| b.extend_from_slice(&[0; 4]))),
        ("unknown kind", Box::new(|b| b[8] = 2)),
        ("token scheme on a lossy kind", Box::new(|b| b[9] = 1)),
        ("kinds out of order", Box::new(|b| b[20] = 0)),
        ("no seeds", Box::new(move |b| set(b, 12, 0))),
        ("257 seeds", Box::new(move |b| set(b, 12, 257))),
        ("section misplaced", Box::new(move |b| set(b, 16, at0 + 4))),
        (
            "section past the blob",
            Box::new(move |b| set(b, 28, b.len())),
        ),
        ("first offset not 0", Box::new(move |b| set(b, at0, 1))),
        (
            "offsets not increasing",
            Box::new(move |b| set(b, at0 + 8, 0)),
        ),
        (
            "last offset past the section",
            Box::new(move |b| set(b, at0 + 4 * n0, 1 << 20)),
        ),
        ("padding not zero", Box::new(move |b| b[pad] = 0xFF)),
    ];
    for (what, patch) in &cases {
        let mut b = good.clone();
        patch(&mut b);
        assert_eq!(reg.register(b), Err(bad.clone()), "{what}");
        assert!(reg.get(u64::from(ID)).is_none(), "{what} registered a bank");
    }
    // A bad lossless token scheme: depth 64 would overflow the alphabet.
    let lossless = (0..good[6] as usize)
        .map(|e| 8 + 12 * e)
        .find(|&d| good[d] == 6)
        .unwrap();
    for tc in [[5, 2, 64], [5, 5, 8], [17, 2, 16], [5, 2, 0]] {
        let mut b = good.clone();
        b[lossless + 1..lossless + 4].copy_from_slice(&tc);
        assert_eq!(reg.register(b), Err(bad.clone()), "{tc:?}");
    }
    assert!(reg.get(u64::from(ID)).is_none());
    // Accepted once; the same id again is refused and keeps the first bank.
    assert_eq!(reg.register(good.clone()), Ok(u64::from(ID)));
    let (first, costs_of, _) = reg.get(u64::from(ID)).unwrap();
    assert_eq!(
        reg.register(good.clone()),
        Err(Error::InvalidInput("seed bank id already registered"))
    );
    assert!(core::ptr::eq(reg.get(u64::from(ID)).unwrap().0, first));
    // Same content as bank 1, same encoder costs.
    assert_eq!(costs_of, &BANK1_COSTS[..]);
    // The process registry never saw it.
    assert!(bank(u64::from(ID)).is_none());
}

/// A structurally valid blob whose seed payloads are arbitrary bytes passes
/// the registration check, and neither the encoder cost table nor the
/// decoder's seed reading panics on it (checked before anything is leaked).
#[test]
fn cost_fill_and_seed_reads_survive_arbitrary_payloads() {
    let blob = bank_blob(1).unwrap().to_vec();
    let u32at = |b: &[u8], i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap()) as usize;
    // Byte ranges of every seed's payload and each kind's alphabet.
    let mut seeds = Vec::new();
    for e in 0..blob[6] as usize {
        let d = 8 + 12 * e;
        let (kind, tc) = (blob[d], [blob[d + 1], blob[d + 2], blob[d + 3]]);
        let (n, at) = (u32at(&blob, d + 4), u32at(&blob, d + 8));
        let data = at + 4 * (n + 1);
        for i in 0..n {
            let (a, b) = (u32at(&blob, at + 4 * i), u32at(&blob, at + 4 * (i + 1)));
            seeds.push((kind, codec::alphabet(kind, tc), data + a, data + b));
        }
    }
    let mut x = 0x9E37_79B9u32;
    for round in 0..64 {
        let mut b = blob.clone();
        for &(_, _, s, e) in &seeds {
            for v in &mut b[s..e] {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                // Some rounds all ones / zeros, the rest random.
                *v = match round % 8 {
                    0 => 0xFF,
                    1 => 0,
                    _ => x as u8,
                };
            }
        }
        let words = b.as_chunks::<4>().0;
        let len = check_blob(words, 1).expect("layout unchanged");
        assert_eq!(len, BANK1_COSTS.len());
        let mut c = std::vec![costs::ABSENT; len];
        costs::fill(&b, &mut c);
        for &(kind, alpha, s, e) in &seeds {
            if kind == 4 || kind == 5 {
                continue; // code lengths: any 4-bit value reads
            }
            let _ = crate::rans::read_weights(&mut crate::bits::BitReader::new(&b[s..e]), alpha);
        }
    }
}

/// Cost table size of a lab bank of `n` seeds of the standalone lossless
/// token scheme (5, 2, 8): the bound `LAB_MAX_COSTS` leaves room for it.
#[test]
fn lossless_bank_cost_width() {
    let w = costs::width(6, [5, 2, 8]);
    assert_eq!(w, crate::tokens::TokenCfg::v1(5, 2, 8).alphabet() as usize);
    assert!(255 * w <= LAB_MAX_COSTS);
    std::eprintln!("(5,2,8) cost table: {} bytes for 255 seeds", 255 * w);
}
