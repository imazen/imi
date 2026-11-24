//! LAB block hooks for the lossless-over-lossy estimate
//! (`imij::lab::encode_lossy_blocks`).
#![cfg(all(feature = "lab", feature = "std"))]

use imij::{Channels, ImageInfo, Metadata, SampleType, decode};

fn scene(w: usize, h: usize) -> Vec<u8> {
    (0..w * h * 3)
        .map(|i| {
            let (p, c) = (i / 3, i % 3);
            let (x, y) = (p % w, p / w);
            if (x / 13 + y / 9) % 3 == 0 {
                250
            } else {
                ((x * 7 + y * 3 + c * 40) % 256) as u8
            }
        })
        .collect()
}

/// `set_lossy_exact_blocks` is process-global; tests here run serially.
static SWITCH: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn bits_mask_and_pixels() {
    let _g = SWITCH.lock().unwrap_or_else(|e| e.into_inner());
    let (w, h) = (100, 60);
    let px = scene(w, h);
    let info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8);
    let (bw, bh) = (w.div_ceil(8), h.div_ceil(8));
    for q in [20.0, 70.0] {
        let mut cfg = imij::ImiEncoderConfig::lossy(q);
        cfg = cfg.tune_lossy(|t| {
            t.exact_blocks = Some(0.0);
        });
        let plain = cfg
            .encode_samples(imij::ImageRef::new(&px, w * 3, &info), &Metadata::default())
            .unwrap();
        let (file, bits) = imij::lab::encode_lossy_blocks(&px, w * 3, &info, &cfg, None).unwrap();
        assert_eq!(file, plain);
        assert_eq!(bits.len(), bw * bh);
        let none = vec![false; bw * bh];
        let (same, _) =
            imij::lab::encode_lossy_blocks(&px, w * 3, &info, &cfg, Some(&none)).unwrap();
        assert_eq!(same, plain);
        // Block bits fit inside the file and cover most of its AC payload.
        let total = bits.iter().map(|&b| b as f64).sum::<f64>() / 8.0;
        assert!(
            total > 0.0 && total < file.len() as f64,
            "q{q}: {total} of {}",
            file.len()
        );
        // Masking every other block: unmasked blocks decode unchanged,
        // masked ones decode flat (DC only), and the file shrinks.
        let mask: Vec<bool> = (0..bw * bh).map(|b| (b % bw + b / bw) % 2 == 0).collect();
        let (masked, mbits) =
            imij::lab::encode_lossy_blocks(&px, w * 3, &info, &cfg, Some(&mask)).unwrap();
        assert!(masked.len() < file.len());
        let (a, b) = (decode(&file).unwrap().1, decode(&masked).unwrap().1);
        for by in 0..bh {
            for bx in 0..bw {
                let blk = by * bw + bx;
                let (x0, y0) = (bx * 8, by * 8);
                let pix = |img: &[u8], x: usize, y: usize| img[(y * w + x) * 3..][..3].to_vec();
                let mut first = None;
                for y in y0..(y0 + 8).min(h) {
                    for x in x0..(x0 + 8).min(w) {
                        if mask[blk] {
                            let p = pix(&b, x, y);
                            assert_eq!(
                                first.get_or_insert(p.clone()),
                                &p,
                                "masked block {bx},{by} not flat"
                            );
                        } else {
                            assert_eq!(pix(&a, x, y), pix(&b, x, y), "unmasked block {bx},{by}");
                        }
                    }
                }
                if mask[blk] {
                    assert!(mbits[blk] < 20.0, "masked block costs {} bits", mbits[blk]);
                }
            }
        }
    }
}

/// Text on a flat page next to a textured patch: text blocks are expensive
/// lossy and cheap lossless.
fn page(w: usize, h: usize) -> Vec<u8> {
    let mut v = vec![255u8; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let o = (y * w + x) * 3;
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
        }
    }
    v
}

#[test]
fn exact_blocks_are_exact_and_the_rest_is_the_lossy_decode() {
    let _g = SWITCH.lock().unwrap_or_else(|e| e.into_inner());
    // 600x530: over 512 exact blocks, so several nested stripes, packed
    // rows narrower than the image (64 of 75 blocks), stripes sharing them.
    for (w, h) in [(100usize, 60usize), (64, 64), (203, 37), (600, 530)] {
        let px = page(w, h);
        let info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8);
        for q in [30.0, 70.0, 95.0] {
            let mut cfg = imij::ImiEncoderConfig::lossy(q);
            cfg = cfg.tune_lossy(|t| {
                t.exact_blocks = Some(0.0);
            });
            let plain = cfg
                .encode_samples(imij::ImageRef::new(&px, w * 3, &info), &Metadata::default())
                .unwrap();
            imij::lab::set_lossy_exact_blocks(1.0);
            let exact =
                cfg.encode_samples(imij::ImageRef::new(&px, w * 3, &info), &Metadata::default());
            imij::lab::set_lossy_exact_blocks(0.0);
            let exact = exact.unwrap();
            let lossy = decode(&plain).unwrap().1;
            let (got_info, got) = decode(&exact).unwrap();
            assert_eq!(got_info, info);
            let mut out4 = vec![0u8; got.len()];
            imij::decode_into_threads(&exact, &mut out4, w * 3, 4).unwrap();
            assert_eq!(out4, got, "{w}x{h} q{q}: threads");
            let (bw, bh) = (w.div_ceil(8), h.div_ceil(8));
            let mut n_exact = 0;
            for by in 0..bh {
                for bx in 0..bw {
                    let rows = by * 8..(by * 8 + 8).min(h);
                    let cols = bx * 8..(bx * 8 + 8).min(w);
                    let same = |a: &[u8]| {
                        rows.clone().all(|y| {
                            let r = (y * w + cols.start) * 3..(y * w + cols.end) * 3;
                            a[r.clone()] == got[r]
                        })
                    };
                    let is_exact = same(&px);
                    assert!(is_exact || same(&lossy), "{w}x{h} q{q} block {bx},{by}");
                    n_exact += usize::from(is_exact && !same(&lossy));
                }
            }
            assert!(n_exact > 0, "{w}x{h} q{q}: no exact block");
            if w == 600 {
                assert!(n_exact > 512, "{w}x{h} q{q}: {n_exact} exact blocks");
            }
            // The streaming reader decodes them too.
            let mut read = vec![0u8; got.len()];
            imij::decode_reader(
                std::io::Cursor::new(&exact),
                &imij::Limits::DEFAULT,
                |_, y, r| {
                    read[y as usize * w * 3..][..w * 3].copy_from_slice(r);
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(read, got, "{w}x{h} q{q}: decode_reader");
        }
    }
}

/// Exact blocks under every shared-DC schedule and DC split: images whose
/// one DC stripe spans several lossy stripes (Compact, up to 65,536 pixels).
#[test]
fn exact_blocks_under_every_dc_schedule() {
    let _g = SWITCH.lock().unwrap_or_else(|e| e.into_inner());
    for (w, h) in [(256usize, 200usize), (480, 120)] {
        let px = page(w, h);
        let info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8);
        let mut cfg =
            imij::ImiEncoderConfig::lossy(80.0).with_decode_speed(imij::DecodeSpeed::Compact);
        let file = cfg
            .encode_samples(imij::ImageRef::new(&px, w * 3, &info), &Metadata::default())
            .unwrap();
        cfg = cfg.tune_lossy(|t| {
            t.exact_blocks = Some(0.0);
        });
        let plain = cfg
            .encode_samples(imij::ImageRef::new(&px, w * 3, &info), &Metadata::default())
            .unwrap();
        assert_ne!(file, plain, "{w}x{h}: no exact block");
        let (_, want) = decode(&file).unwrap();
        let lossy = decode(&plain).unwrap().1;
        // Exact blocks are the source, the rest the plain lossy decode.
        for (i, (g, (s, l))) in want.iter().zip(px.iter().zip(&lossy)).enumerate() {
            assert!(g == s || g == l, "{w}x{h} byte {i}");
        }
        for mode in [0u8, 1, 2] {
            for split in [0usize, 1, 2] {
                imij::lab::set_shared_dc(mode);
                imij::lab::set_dc_split(split);
                for t in [2, 3, 8] {
                    let mut out = vec![0u8; want.len()];
                    imij::decode_into_threads(&file, &mut out, w * 3, t).unwrap();
                    assert_eq!(out, want, "{w}x{h} mode {mode} split {split} threads {t}");
                }
            }
        }
        imij::lab::set_shared_dc(2);
        imij::lab::set_dc_split(0);
    }
}
