//! Exact blocks (lossless 8x8 blocks over lossy; docs/LOSSY.md "Exact
//! blocks"): exact blocks decode to the source pixels, every other block to
//! the plain lossy decode, all decoders agree, and images where nothing
//! qualifies encode byte-identically to the plain lossy file.

mod common;

use common::feature::{EXACT_BLOCKS as EXACT, SEED_BANK_IDS as SEEDS, SUPERBLOCKS};
use common::{SIGNATURE, feature_mask as features, with_feature_mask};
use imij::decode_resources::{DecodePolicy, DecodeSession};
use imij::{
    Channels, DecodeSpeed, ImageInfo, ImiEncoderConfig, Limits, Metadata, SampleType,
    StreamDecoder, decode, decode_into_threads, decode_reader,
};

/// Text-like ink on a flat page (left half: cheap lossless, costly lossy)
/// next to a texture (right half), with alpha when `ch` has it.
fn page(w: usize, h: usize, ch: Channels) -> Vec<u8> {
    let n = ch.count();
    let mut v = vec![255u8; w * h * n];
    for y in 0..h {
        for x in 0..w {
            let o = (y * w + x) * n;
            if x < w / 2 {
                let ink = (x % 9 < 2 && y % 13 > 2) || (y % 13 == 7 && x % 17 < 11);
                if ink {
                    v[o..o + 3].copy_from_slice(&[20, 30, 90]);
                }
            } else {
                for c in 0..3 {
                    v[o + c] = ((x * 31 + y * 17 + c * 50) % 200) as u8 + 20;
                }
            }
            if n == 4 {
                v[o + 3] = ((x + 2 * y) % 256) as u8;
            }
        }
    }
    v
}

/// Pseudo-random pixels: no block is cheaper lossless than lossy.
fn noise(w: usize, h: usize, ch: Channels) -> Vec<u8> {
    let mut s = 0x9E37_79B9_7F4A_7C15u64;
    (0..w * h * ch.count())
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 24) as u8
        })
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

fn off(cfg: &ImiEncoderConfig) -> ImiEncoderConfig {
    let mut c = cfg.clone();
    c = c.tune_lossy(|t| {
        t.exact_blocks = Some(0.0);
    });
    c
}

/// Every decoder's output of `file`, which must agree.
fn decode_all(file: &[u8], info: &ImageInfo) -> Vec<u8> {
    let (got_info, px) = decode(file).unwrap();
    assert_eq!(&got_info, info);
    let rb = info.row_bytes();
    for t in [1, 3, 8] {
        let mut out = vec![0u8; px.len()];
        decode_into_threads(file, &mut out, rb, t).unwrap();
        assert_eq!(out, px, "{t} threads");
    }
    // Strided output: padding bytes stay as they were.
    let stride = rb + 5;
    let mut out = vec![0xA5u8; stride * info.height as usize];
    decode_into_threads(file, &mut out, stride, 2).unwrap();
    for (y, row) in out.chunks(stride).enumerate() {
        assert_eq!(&row[..rb], &px[y * rb..][..rb], "strided row {y}");
        assert!(row[rb..].iter().all(|&b| b == 0xA5));
    }
    let mut sd = StreamDecoder::new(file).unwrap();
    let mut row = vec![0u8; rb];
    let mut streamed = Vec::new();
    while sd.next_row(&mut row).unwrap() {
        streamed.extend_from_slice(&row);
    }
    assert_eq!(streamed, px, "StreamDecoder");
    let mut read = vec![0u8; px.len()];
    decode_reader(std::io::Cursor::new(file), &Limits::DEFAULT, |_, y, r| {
        read[y as usize * rb..][..rb].copy_from_slice(r);
        Ok(())
    })
    .unwrap();
    assert_eq!(read, px, "decode_reader");
    let (_, session) = DecodeSession::new(DecodePolicy::default())
        .decode(file)
        .unwrap();
    assert_eq!(session, px, "DecodeSession");
    px
}

/// Blocks of `got` equal to the source and different from `lossy`; every
/// block must equal one of the two.
/// `strict` false skips the per-block check and only counts (see the Compact
/// case of `exact_blocks_are_exact_and_decoders_agree`).
fn exact_blocks(
    src: &[u8],
    lossy: &[u8],
    got: &[u8],
    (w, h, n): (usize, usize, usize),
    strict: bool,
) -> usize {
    let mut exact = 0;
    for by in 0..h.div_ceil(8) {
        for bx in 0..w.div_ceil(8) {
            let rows = by * 8..(by * 8 + 8).min(h);
            let (x0, x1) = (bx * 8, (bx * 8 + 8).min(w));
            // Color channels only: alpha is lossless everywhere.
            let same = |a: &[u8]| {
                rows.clone().all(|y| {
                    (x0..x1).all(|x| {
                        let o = (y * w + x) * n;
                        a[o..o + 3] == got[o..o + 3] && a[o + n - 1] == got[o + n - 1]
                    })
                })
            };
            let is_exact = same(src);
            assert!(!strict || is_exact || same(lossy), "block {bx},{by}");
            exact += usize::from(is_exact && !same(lossy));
        }
    }
    exact
}

#[test]
fn exact_blocks_are_exact_and_decoders_agree() {
    let cases: &[(usize, usize, Channels, Option<u32>)] = &[
        (100, 60, Channels::Rgb, None),
        (203, 37, Channels::Rgb, None),
        (600, 530, Channels::Rgb, None),
        // Compact codes images up to 65,536 pixels with one DC stripe over
        // several lossy stripes: the shared-DC decode path.
        (256, 200, Channels::Rgb, None),
        (300, 120, Channels::Rgba, None),
        // Column groups of 64 pixels: the block list follows unit order.
        (300, 90, Channels::Rgb, Some(64)),
        (200, 70, Channels::Rgba, Some(96)),
    ];
    for &(w, h, ch, groups) in cases {
        let info = ImageInfo::new(w as u32, h as u32, ch, SampleType::U8);
        let px = page(w, h, ch);
        for q in [30.0, 70.0, 95.0] {
            for (name, speed, ratio) in [
                ("compact", Some(DecodeSpeed::Compact), None),
                ("balanced1.5", Some(DecodeSpeed::Balanced), Some(1.5)),
                ("ratio3", None, Some(3.0)),
            ] {
                let mut cfg = imij::ImiEncoderConfig::lossy(q);
                if let Some(s) = speed {
                    cfg = cfg.with_decode_speed(s);
                }
                cfg = cfg.tune_lossy(|t| {
                    t.exact_blocks = ratio;
                });
                cfg = cfg.with_column_groups(groups);
                // Compact merges 8-bit blocks from 128 px (superblocks), and
                // parents of a merge are never exact blocks, so the blocks
                // around exact blocks merge differently than in the plain
                // encode: "every block equals the source or the plain
                // decode" is false there by design. Lab builds check it
                // with superblocks off; default builds (no switch) keep the
                // decoder agreement and the exact-block counts. Exact blocks
                // beside merges: `tests/superblocks.rs`
                // (`exact_blocks_and_superblocks_share_a_file`) and the
                // superblock parity set's `sb-exact-*` fixtures.
                #[cfg(feature = "lab")]
                if name == "compact" {
                    cfg = cfg.tune_lossy(|t| t.lab.switches.superblocks = Some(false));
                }
                let strict = cfg!(feature = "lab") || name != "compact";
                let file = enc(&px, &info, &cfg);
                let plain = enc(&px, &info, &off(&cfg));
                let lossy = decode_all(&plain, &info);
                let got = decode_all(&file, &info);
                let n = exact_blocks(&px, &lossy, &got, (w, h, ch.count()), strict);
                let tag = format!("{w}x{h} {ch:?} groups {groups:?} q{q} {name}");
                assert!(n > 0, "{tag}: no exact block");
                if w == 600 && name != "balanced1.5" {
                    // Several nested stripes, shared across stripes.
                    assert!(n > 512, "{tag}: {n} exact blocks");
                }
            }
        }
    }
}

#[test]
fn files_without_qualifying_blocks_are_unchanged() {
    for ch in [Channels::Rgb, Channels::Rgba] {
        let (w, h) = (96, 72);
        let info = ImageInfo::new(w, h, ch, SampleType::U8);
        let px = noise(w as usize, h as usize, ch);
        for q in [20.0, 60.0, 90.0] {
            for speed in [
                None,
                Some(DecodeSpeed::Compact),
                Some(DecodeSpeed::Balanced),
            ] {
                let mut cfg = imij::ImiEncoderConfig::lossy(q);
                if let Some(s) = speed {
                    cfg = cfg.with_decode_speed(s);
                }
                assert_eq!(
                    enc(&px, &info, &cfg),
                    enc(&px, &info, &off(&cfg)),
                    "{ch:?} q{q} {speed:?}"
                );
            }
        }
    }
}

#[test]
fn tiers_and_inputs_without_exact_blocks_are_unchanged() {
    let (w, h) = (120, 64);
    // Only Compact uses exact blocks by default.
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let px = page(w as usize, h as usize, Channels::Rgb);
    let tiers = [
        None,
        Some(DecodeSpeed::Balanced),
        Some(DecodeSpeed::Fast),
        Some(DecodeSpeed::Fastest),
    ];
    for speed in tiers {
        let mut cfg = imij::ImiEncoderConfig::lossy(70.0);
        if let Some(s) = speed {
            cfg = cfg.with_decode_speed(s);
        }
        assert_eq!(
            enc(&px, &info, &cfg),
            enc(&px, &info, &off(&cfg)),
            "{speed:?}"
        );
    }
    // Gray and 16-bit inputs, and AQ, never use them.
    let gray = ImageInfo::new(w, h, Channels::Gray, SampleType::U8);
    let gpx: Vec<u8> = px.chunks(3).map(|p| p[1]).collect();
    let cfg = imij::ImiEncoderConfig::lossy(70.0).with_decode_speed(DecodeSpeed::Compact);
    assert_eq!(enc(&gpx, &gray, &cfg), enc(&gpx, &gray, &off(&cfg)));
    let wide = ImageInfo::new(w, h, Channels::Rgb, SampleType::U16);
    let wpx: Vec<u8> = px
        .iter()
        .flat_map(|&v| (v as u16 * 257).to_ne_bytes())
        .collect();
    assert_eq!(enc(&wpx, &wide, &cfg), enc(&wpx, &wide, &off(&cfg)));
    let mut aq = cfg.clone();
    aq = aq.tune_lossy(|t| {
        t.aq_gamma = 0.5;
    });
    assert_eq!(enc(&px, &info, &aq), enc(&px, &info, &off(&aq)));
}

#[test]
fn mutated_exact_files_do_not_panic() {
    let mut files = Vec::new();
    for (w, h, ch, groups) in [
        (64usize, 48usize, Channels::Rgb, None),
        (90, 40, Channels::Rgba, None),
        (120, 30, Channels::Rgb, Some(48)),
    ] {
        let info = ImageInfo::new(w as u32, h as u32, ch, SampleType::U8);
        let mut cfg = imij::ImiEncoderConfig::lossy(80.0).with_decode_speed(DecodeSpeed::Compact);
        cfg = cfg.with_column_groups(groups);
        cfg = cfg.tune_lossy(|t| {
            t.exact_blocks = Some(3.0);
        });
        let file = enc(&page(w, h, ch), &info, &cfg);
        assert_ne!(file, enc(&page(w, h, ch), &info, &off(&cfg)));
        files.push(file);
    }
    let mut s = 0x2545_F491_4F6C_DD1Du64;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    let iters = std::env::var("IMIJ_FUZZ_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1500);
    let mut rejected = 0;
    for it in 0..iters {
        let mut f = files[it % files.len()].clone();
        match next() % 3 {
            0 => {
                for _ in 0..1 + next() % 4 {
                    let i = next() as usize % f.len();
                    f[i] ^= 1 << (next() % 8);
                }
            }
            1 => {
                let i = next() as usize % f.len();
                f[i] = next() as u8;
            }
            _ => f.truncate(next() as usize % f.len()),
        }
        // Every decoder: an error or some pixels, never a panic.
        let r = decode(&f);
        rejected += usize::from(r.is_err());
        if let Ok((info, px)) = r {
            let mut out = vec![0u8; px.len()];
            let _ = decode_into_threads(&f, &mut out, info.row_bytes(), 4);
        }
        if let Ok(mut sd) = StreamDecoder::new(&f) {
            let mut row = vec![0u8; sd.info().row_bytes()];
            while let Ok(true) = sd.next_row(&mut row) {}
        }
        let _ = decode_reader(std::io::Cursor::new(&f), &Limits::DEFAULT, |_, _, _| Ok(()));
        let _ = DecodeSession::new(DecodePolicy::default()).decode(&f);
    }
    eprintln!("{rejected}/{iters} mutated exact files rejected");
}

#[test]
fn every_byte_flip_and_truncation_of_a_small_exact_file_is_safe() {
    for ch in [Channels::Rgb, Channels::Rgba] {
        let (w, h) = (40, 24);
        let info = ImageInfo::new(w as u32, h as u32, ch, SampleType::U8);
        let mut cfg = imij::ImiEncoderConfig::lossy(80.0).with_decode_speed(DecodeSpeed::Compact);
        cfg = cfg.tune_lossy(|t| {
            t.exact_blocks = Some(3.0);
        });
        let file = enc(&page(w, h, ch), &info, &cfg);
        assert_ne!(file, enc(&page(w, h, ch), &info, &off(&cfg)));
        let check = |f: &[u8]| {
            if let Ok((i, px)) = decode(f) {
                let mut out = vec![0u8; px.len()];
                let _ = decode_into_threads(f, &mut out, i.row_bytes(), 3);
            }
            if let Ok(mut sd) = StreamDecoder::new(f) {
                let mut row = vec![0u8; sd.info().row_bytes()];
                while let Ok(true) = sd.next_row(&mut row) {}
            }
            let _ = decode_reader(std::io::Cursor::new(f), &Limits::DEFAULT, |_, _, _| Ok(()));
            let _ = DecodeSession::new(DecodePolicy::default()).decode(f);
        };
        for i in 0..file.len() {
            for bit in [0x01u8, 0x80] {
                let mut f = file.clone();
                f[i] ^= bit;
                check(&f);
            }
            check(&file[..i]);
        }
    }
}

/// `file`'s feature mask set to `mask`, the field removed for 0.
fn with_features(file: &[u8], mask: u64) -> Vec<u8> {
    with_feature_mask(file, (mask != 0).then_some(mask))
}

#[test]
fn exact_files_declare_exact_blocks() {
    // Seed tables are used when they make the file smaller, so a file
    // declares `exact_blocks` alone or with `seed_bank_1`, in the feature mask.
    let mut seen = Vec::new();
    for (w, h, q) in [(64usize, 48usize, 80.0), (256, 192, 80.0), (256, 192, 40.0)] {
        let info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8);
        let cfg = imij::ImiEncoderConfig::lossy(q).with_decode_speed(DecodeSpeed::Compact);
        let file = enc(&page(w, h, Channels::Rgb), &info, &cfg);
        let plain = enc(&page(w, h, Channels::Rgb), &info, &off(&cfg));
        assert_ne!(file, plain);
        let (fp, pp) = (features(&file), features(&plain));
        assert_eq!(fp & EXACT, EXACT, "{fp:#x}");
        assert_eq!(pp & EXACT, 0, "{pp:#x}");
        // Compact merges blocks from 128 px (feature 12, `superblocks`): a
        // file declares it exactly when one of its units merges.
        assert_eq!(
            (fp | pp) & !(EXACT | SEEDS | SUPERBLOCKS),
            0,
            "{fp:#x} {pp:#x}"
        );
        for (f, mask) in [(&file, fp), (&plain, pp)] {
            let merged = common::superblock_units(f, &info) > 0;
            assert_eq!(mask & SUPERBLOCKS != 0, merged, "{w}x{h} q{q}: {mask:#x}");
        }
        // Without the mask the headers agree: the mask is the only header
        // cost, its field and one byte. (The DATA locator, record 4,
        // differs with the payload's head length.)
        let bare = |f: &[u8]| common::header_without(&with_feature_mask(f, None), &[4]);
        assert_eq!(bare(&file), bare(&plain));
        seen.push(fp);
    }
    assert!(seen.contains(&(SEEDS | EXACT)), "{seen:?}");
}

/// Every decode entry point fails on `file` with `want`.
fn rejected_everywhere(file: &[u8], info: &ImageInfo, want: &imij::Error) {
    let rb = info.row_bytes();
    let mut out = vec![0u8; rb * info.height as usize];
    let w = Some(want);
    assert_eq!(decode(file).err().as_ref(), w, "decode");
    assert_eq!(
        imij::decode_with_limits(file, &Limits::DEFAULT)
            .err()
            .as_ref(),
        w,
        "decode_with_limits"
    );
    assert_eq!(
        imij::decode_into(file, &mut out, rb).err().as_ref(),
        w,
        "decode_into"
    );
    for t in [1, 4] {
        assert_eq!(
            decode_into_threads(file, &mut out, rb, t).err().as_ref(),
            w,
            "decode_into_threads {t}"
        );
    }
    let streamed = StreamDecoder::new(file).and_then(|mut sd| {
        let mut row = vec![0u8; rb];
        while sd.next_row(&mut row)? {}
        Ok(())
    });
    assert_eq!(streamed.err().as_ref(), w, "StreamDecoder");
    let read = decode_reader(std::io::Cursor::new(file), &Limits::DEFAULT, |_, _, _| {
        Ok(())
    });
    assert_eq!(read.err().as_ref(), w, "decode_reader");
    let session = || DecodeSession::new(DecodePolicy::default());
    assert_eq!(session().decode(file).err().as_ref(), w, "session decode");
    assert_eq!(
        session().decode_into(file, &mut out, rb).err().as_ref(),
        w,
        "session decode_into"
    );
    let mut row = vec![0u8; rb];
    assert_eq!(
        session()
            .decode_rows(file, &mut row, |_, _, _| Ok(()))
            .err()
            .as_ref(),
        w,
        "session decode_rows"
    );
    assert_eq!(
        session()
            .decode_reader(std::io::Cursor::new(file), |_, _, _| Ok(()))
            .err()
            .as_ref(),
        w,
        "session decode_reader"
    );
    // Header and metadata calls read no closed payload (as for any payload
    // fault): they succeed, and decoding the frame fails.
    imij::header(file).unwrap();
    imij::probe(file).unwrap();
    for t in [1, 4] {
        let mut fd = imij::FrameDecoder::new(file).unwrap();
        fd.next_frame().unwrap().unwrap();
        assert_eq!(
            fd.decode_into(&mut out, rb, t).err().as_ref(),
            w,
            "FrameDecoder {t} threads"
        );
    }
}

/// In the `skip..skip + take`-th complete files nested in `file` (signature
/// order: the file itself, then frames, plane tiles, ...), clears the
/// `exact_blocks` bit of the feature mask. The mask is one byte here and
/// keeps its length, so chunk lengths and plane indexes stay valid. Returns
/// the number of masks changed.
fn unlist_exact(file: &mut [u8], skip: usize, take: usize) -> usize {
    let (mut changed, mut p, mut k) = (0, 0, 0);
    while let Some(off) = file[p..].windows(8).position(|w| w == SIGNATURE) {
        let s = p + off;
        p = s + 8;
        k += 1;
        if k <= skip || k > skip.saturating_add(take) {
            continue;
        }
        let h = common::header_record(&file[s..]);
        let l = common::header_layout(&file[s + h.start..s + h.end]);
        if l.features.is_empty() {
            continue;
        }
        let at = s + h.start + l.features.start;
        if file[at] & EXACT as u8 == 0 {
            continue;
        }
        file[at] &= !(EXACT as u8);
        changed += 1;
    }
    changed
}

/// (width, height, channels, column groups, tier, exact ratio, quality).
type Case = (
    usize,
    usize,
    Channels,
    Option<u32>,
    Option<DecodeSpeed>,
    Option<f32>,
    f32,
);

#[test]
fn exact_files_without_the_feature_are_corrupt() {
    let want = imij::Error::Corrupt("exact blocks without the feature exact_blocks");
    // (size, channels, column groups, tier, exact ratio, quality): Compact
    // seeded and unseeded, alpha, column groups, a non-Compact tier opted in.
    let cases: &[Case] = &[
        (
            64,
            48,
            Channels::Rgb,
            None,
            Some(DecodeSpeed::Compact),
            None,
            80.0,
        ),
        (
            256,
            192,
            Channels::Rgb,
            None,
            Some(DecodeSpeed::Compact),
            None,
            40.0,
        ),
        (
            90,
            40,
            Channels::Rgba,
            None,
            Some(DecodeSpeed::Compact),
            Some(3.0),
            80.0,
        ),
        (
            120,
            30,
            Channels::Rgb,
            Some(48),
            Some(DecodeSpeed::Compact),
            Some(3.0),
            80.0,
        ),
        (
            100,
            60,
            Channels::Rgb,
            None,
            Some(DecodeSpeed::Balanced),
            Some(1.5),
            80.0,
        ),
        (100, 60, Channels::Rgb, None, None, Some(1.5), 80.0),
    ];
    let mut shapes = Vec::new();
    for &(w, h, ch, groups, speed, ratio, q) in cases {
        let info = ImageInfo::new(w as u32, h as u32, ch, SampleType::U8);
        let mut cfg = imij::ImiEncoderConfig::lossy(q);
        cfg = cfg.with_decode_speed(speed);
        cfg = cfg.with_column_groups(groups);
        cfg = cfg.tune_lossy(|t| {
            t.exact_blocks = ratio;
        });
        let file = enc(&page(w, h, ch), &info, &cfg);
        assert_ne!(file, enc(&page(w, h, ch), &info, &off(&cfg)), "{w}x{h}");
        let listed = features(&file);
        assert_eq!(listed & EXACT, EXACT, "{listed:#x}");
        // The rewrite itself changes nothing when it keeps the mask.
        assert_eq!(with_features(&file, listed), file);
        rejected_everywhere(&with_features(&file, listed & !EXACT), &info, &want);
        shapes.push(listed);
    }
    // The feature was cleared from the mask this encoder writes by default
    // (with seed tables).
    assert!(shapes.contains(&(SEEDS | EXACT)), "{shapes:#x?}");
    // `exact_blocks` alone needs a file without seed tables (the lab
    // switch; the encoder uses seeds whenever they pay), and clearing it
    // removes the mask field.
    #[cfg(feature = "lab")]
    {
        let (w, h) = (64usize, 48usize);
        let info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8);
        let mut cfg = imij::ImiEncoderConfig::lossy(80.0).with_decode_speed(DecodeSpeed::Compact);
        cfg = cfg.tune_lossy(|t| {
            t.lab.seed_bank = 0;
        });
        let file = enc(&page(w, h, Channels::Rgb), &info, &cfg);
        assert_eq!(features(&file), EXACT);
        rejected_everywhere(&with_features(&file, 0), &info, &want);
    }
}

#[test]
fn files_may_declare_exact_blocks_without_using_them() {
    // `exact_blocks` declared but unused (like `seed_bank_1` without seed
    // tables; SPEC 0.1, over-declaring): the same pixels from every decoder.
    let (w, h) = (100usize, 60usize);
    let info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8);
    let cfg = imij::ImiEncoderConfig::lossy(80.0).with_decode_speed(DecodeSpeed::Compact);
    let plain = enc(&page(w, h, Channels::Rgb), &info, &off(&cfg));
    let listed = features(&plain);
    assert_eq!(listed & EXACT, 0);
    let listed_compact = with_features(&plain, listed | EXACT);
    assert_ne!(listed_compact, plain);
    assert_eq!(
        decode_all(&listed_compact, &info),
        decode_all(&plain, &info)
    );
}

/// Every frame of an animation or page file, decoded with `threads`.
fn all_frames(file: &[u8], info: &ImageInfo, threads: usize) -> Result<Vec<Vec<u8>>, imij::Error> {
    let rb = info.row_bytes();
    let mut fd = imij::FrameDecoder::new(file)?;
    let mut canvas = vec![0u8; rb * info.height as usize];
    let mut out = Vec::new();
    while fd.next_frame()?.is_some() {
        fd.decode_into(&mut canvas, rb, threads)?;
        out.push(canvas.clone());
    }
    Ok(out)
}

#[test]
fn frames_and_pages_with_exact_blocks_declare_the_feature() {
    use imij::{FrameConfig, FrameDuration, FrameEncoder};
    let want = imij::Error::Corrupt("exact blocks without the feature exact_blocks");
    let (w, h) = (96usize, 64usize);
    let info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8);
    let mut cfg = imij::ImiEncoderConfig::lossy(80.0).with_decode_speed(DecodeSpeed::Compact);
    cfg = cfg.tune_lossy(|t| {
        t.exact_blocks = Some(3.0);
    });
    let a = page(w, h, Channels::Rgb);
    // Inverted: no block equals the previous frame's, so nothing is kept.
    let b: Vec<u8> = a.iter().map(|v| 255 - v).collect();
    let d = FrameDuration { num: 1, den: 10 };
    let mut anim = FrameEncoder::animation(
        info.clone(),
        Metadata::default(),
        FrameConfig::new(cfg.clone()),
    );
    anim.push_frame(&a, w * 3, d).unwrap();
    anim.push_frame(&b, w * 3, d).unwrap();
    let mut pages = FrameEncoder::pages(info.clone(), Metadata::default(), FrameConfig::new(cfg));
    pages.push_page(&a, w * 3, &info).unwrap();
    pages.push_page(&b, w * 3, &info).unwrap();
    for file in [
        anim.finish_legacy().unwrap(),
        pages.finish_legacy().unwrap(),
    ] {
        let good = all_frames(&file, &info, 1).unwrap();
        assert_eq!(good.len(), 2);
        assert_eq!(all_frames(&file, &info, 4).unwrap(), good);
        // Both the first frame (the file's own header) and the fRAM frame
        // declare `exact_blocks`. With both unlisted, every entry point
        // rejects the first frame's payload.
        let mut probe = file.clone();
        assert_eq!(unlist_exact(&mut probe, 0, usize::MAX), 2);
        rejected_everywhere(&probe, &info, &want);
        assert_eq!(all_frames(&probe, &info, 2).err().as_ref(), Some(&want));
        // Later frame unlisted: the first frame and every single-image
        // decoder are unaffected; the second frame fails at any thread count.
        let mut later = file.clone();
        assert_eq!(unlist_exact(&mut later, 1, usize::MAX), 1);
        assert_eq!(decode_all(&later, &info), good[0]);
        for t in [1, 4] {
            let mut fd = imij::FrameDecoder::new(&later).unwrap();
            let mut canvas = vec![0u8; w * h * 3];
            fd.next_frame().unwrap().unwrap();
            fd.decode_into(&mut canvas, w * 3, t).unwrap();
            assert_eq!(canvas, good[0]);
            fd.next_frame().unwrap().unwrap();
            assert_eq!(
                fd.decode_into(&mut canvas, w * 3, t).err().as_ref(),
                Some(&want)
            );
        }
        // Only the outer header unlisted: single-image decoders reject the
        // first frame's payload, and a frame decoder rejects the document
        // when it opens it, since the later frame needs a feature the outer
        // feature set lacks (SPEC 0.1).
        let mut first = file.clone();
        assert_eq!(unlist_exact(&mut first, 0, 1), 1);
        assert_eq!(decode(&first).err().as_ref(), Some(&want));
        assert_eq!(
            all_frames(&first, &info, 2).err(),
            Some(imij::Error::Corrupt(
                "frame needs a feature the file does not declare"
            ))
        );
    }
}

#[test]
fn plane_tiles_with_exact_blocks_declare_the_feature() {
    use imij::planes::{
        LevelDescriptor, PlaneDescriptor, PlaneSetBuilder, PlaneSetLimits, PlaneSetReader, Rect,
        SliceReader,
    };
    let (w, h) = (96usize, 64usize);
    let primary = imij::ImiEncoderConfig::default()
        .encode_samples(
            imij::ImageRef::new(
                &[1, 2, 3, 4],
                2,
                &ImageInfo::new(2, 2, Channels::Gray, SampleType::U8),
            ),
            &Metadata::default(),
        )
        .unwrap();
    let mut b = PlaneSetBuilder::new(&primary).unwrap();
    let ids: Vec<_> = ["red", "green", "blue"]
        .into_iter()
        .map(|n| {
            b.add_plane(PlaneDescriptor::new(n, SampleType::U8))
                .unwrap()
        })
        .collect();
    let mut cfg = imij::ImiEncoderConfig::lossy(80.0).with_decode_speed(DecodeSpeed::Compact);
    cfg = cfg.tune_lossy(|t| {
        t.exact_blocks = Some(3.0);
    });
    // Two 48x64 tiles.
    b.add_color_level(
        &ids,
        LevelDescriptor::new(w as u32, h as u32, 48, h as u32),
        &page(w, h, Channels::Rgb),
        w * 3,
        Channels::Rgb,
        cfg,
    )
    .unwrap();
    let file = b.finish().unwrap();
    let decode = |f: &[u8]| -> Result<Vec<u8>, imij::Error> {
        let mut r = PlaneSetReader::open(SliceReader::new(f), PlaneSetLimits::default())?;
        let mut color = vec![0u8; w * h * 3];
        r.decode_color_region(
            &ids,
            0,
            Rect::new(0, 0, w as u32, h as u32),
            &mut color,
            w * 3,
        )?;
        let mut red = vec![0u8; w * h];
        r.decode_region(ids[0], 0, Rect::new(0, 0, w as u32, h as u32), &mut red, w)?;
        assert!(red.iter().zip(color.chunks(3)).all(|(r, c)| *r == c[0]));
        Ok(color)
    };
    decode(&file).unwrap();
    // Both tiles (after the primary image) declare `exact_blocks`;
    // unlisted, the plane reader rejects them.
    let mut tiles = file.clone();
    assert_eq!(unlist_exact(&mut tiles, 1, usize::MAX), 2);
    assert_eq!(
        decode(&tiles).err(),
        Some(imij::Error::Corrupt(
            "V4 exact blocks need declared RGB(A)8"
        ))
    );
}

#[test]
fn seed_and_exact_feature_rules_are_checked_alike() {
    // A file with seed tables and exact blocks: without `seed_bank_ids` the
    // seed rule fails first (the envelope precedes the nested images), at
    // the same entry points as the exact-block rule.
    let (w, h) = (64usize, 48usize);
    let info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8);
    let cfg = imij::ImiEncoderConfig::lossy(80.0).with_decode_speed(DecodeSpeed::Compact);
    let file = enc(&page(w, h, Channels::Rgb), &info, &cfg);
    assert_eq!(features(&file), SEEDS | EXACT);
    let seeds = imij::Error::Corrupt("seed bank id without its feature");
    rejected_everywhere(&with_features(&file, EXACT), &info, &seeds);
    rejected_everywhere(&with_features(&file, 0), &info, &seeds);
    rejected_everywhere(
        &with_features(&file, SEEDS),
        &info,
        &imij::Error::Corrupt("exact blocks without the feature exact_blocks"),
    );
    assert_eq!(
        decode_all(&with_features(&file, SEEDS | EXACT), &info),
        decode(&file).unwrap().1
    );
}

#[path = "exact_blocks/trellis.rs"]
mod trellis_interaction;
