use super::*;

#[test]
fn exact_blocks_with_production_trellis_preserve_pixels_and_decoder_parity() {
    // Exercise tier competition below 64K pixels and mandatory run-level above,
    // including clipped edge blocks, alpha and column groups.
    for (w, h, channels, groups) in [
        (100, 60, Channels::Rgb, None),
        (260, 260, Channels::Rgb, None),
        (300, 225, Channels::Rgba, Some(96)),
    ] {
        let info = ImageInfo::new(w as u32, h as u32, channels, SampleType::U8);
        let px = page(w, h, channels);
        for effort in [9.0, 10.0] {
            for quality in [5.0, 40.0, 90.0] {
                let mut cfg = imij::ImiEncoderConfig::lossy(quality)
                    .with_decode_speed(DecodeSpeed::Compact)
                    .with_effort(effort);
                cfg = cfg.tune_lossy(|t| {
                    t.exact_blocks = Some(3.0);
                });
                cfg = cfg.with_column_groups(groups);
                let file = enc(&px, &info, &cfg);
                assert_eq!(file, enc(&px, &info, &cfg.clone().with_threads(4)));
                let got = decode_all(&file, &info);
                #[cfg(feature = "lab")]
                assert_selected_pixels(&file, &info, &px, &got);
                // All feature sets require at least one complete source-exact block.
                assert!(source_exact_count(&px, &got, w, h, channels.count()) > 0);
            }
        }
    }
}

fn source_exact_count(src: &[u8], got: &[u8], w: usize, h: usize, n: usize) -> usize {
    (0..h.div_ceil(8))
        .flat_map(|by| (0..w.div_ceil(8)).map(move |bx| (bx, by)))
        .filter(|&(bx, by)| {
            (by * 8..(by * 8 + 8).min(h)).all(|y| {
                let row = (y * w + bx * 8) * n..(y * w + (bx * 8 + 8).min(w)) * n;
                src[row.clone()] == got[row]
            })
        })
        .count()
}

#[cfg(feature = "lab")]
fn selected(file: &[u8]) -> Vec<bool> {
    let (info, dc, _) = imij::lab::lossy_dc_image(file).unwrap();
    assert_eq!(info.channels, Channels::Rgba);
    let bytes = info.sample.bytes();
    dc.chunks_exact(4 * bytes)
        .map(|pixel| {
            let code = match info.sample {
                SampleType::U16 => u16::from_ne_bytes(pixel[6..8].try_into().unwrap()) as i32,
                SampleType::I32 => i32::from_ne_bytes(pixel[12..16].try_into().unwrap()),
                other => panic!("unexpected DC type {other:?}"),
            };
            assert!(
                code == 0 || code == 1,
                "invalid exact-block wire code {code}"
            );
            code == 1
        })
        .collect()
}

#[cfg(feature = "lab")]
fn assert_selected_pixels(file: &[u8], info: &ImageInfo, src: &[u8], got: &[u8]) {
    let mask = selected(file);
    assert!(mask.contains(&true));
    let (w, h, n) = (
        info.width as usize,
        info.height as usize,
        info.channels.count(),
    );
    assert_eq!(mask.len(), w.div_ceil(8) * h.div_ceil(8));
    for (b, chosen) in mask.into_iter().enumerate() {
        if chosen {
            let (bx, by) = (b % w.div_ceil(8), b / w.div_ceil(8));
            for y in by * 8..(by * 8 + 8).min(h) {
                let row = (y * w + bx * 8) * n..(y * w + (bx * 8 + 8).min(w)) * n;
                assert_eq!(&src[row.clone()], &got[row], "selected block {bx},{by}");
            }
        }
    }
}

#[cfg(feature = "lab")]
#[test]
fn non_exact_blocks_receive_trellis_search() {
    let (w, h) = (260, 260);
    let info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8);
    let mut px = page(w, h, Channels::Rgb);
    let texture = noise(w, h, Channels::Rgb);
    for y in 0..h {
        let right = (y * w + w / 2) * 3..(y + 1) * w * 3;
        px[right.clone()].copy_from_slice(&texture[right]);
    }
    // No tier competition: run-level coding with the same exact-block policy.
    let mut cfg = imij::ImiEncoderConfig::lossy(40.0).with_effort(9.0);
    cfg = cfg.tune_lossy(|t| {
        t.exact_blocks = Some(3.0);
    });
    let on = enc(&px, &info, &cfg);
    let on_px = decode_all(&on, &info);
    assert_selected_pixels(&on, &info, &px, &on_px);
    cfg = cfg.tune_lossy(|t| {
        t.lab.trellis = Some(imij::LossyTrellis {
            passes: 0,
            ..Default::default()
        });
    });
    let off = enc(&px, &info, &cfg);
    let off_px = decode_all(&off, &info);
    assert_selected_pixels(&off, &info, &px, &off_px);
    let (on_mask, off_mask) = (selected(&on), selected(&off));
    assert_eq!(on_mask, off_mask, "selection is fixed before the trellis");
    assert!(on_mask.contains(&false), "fixture needs non-exact blocks");
    let changed = on_mask.iter().enumerate().any(|(b, exact)| {
        if *exact {
            return false;
        }
        let (bx, by) = (b % w.div_ceil(8), b / w.div_ceil(8));
        (by * 8..(by * 8 + 8).min(h)).any(|y| {
            let row = (y * w + bx * 8) * 3..(y * w + (bx * 8 + 8).min(w)) * 3;
            on_px[row.clone()] != off_px[row]
        })
    });
    assert!(changed, "trellis skipped every non-exact block");
}
