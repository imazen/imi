//! The encode-side AC table-bytes counter (`lab::lossy_table_stats`) against
//! the file's own grammar (`lab::lossy_ac_table_bytes`), for the surviving
//! entropy modes (table-count search, 2026-10-10).
#![cfg(feature = "lab")]

use imij::{Channels, ImageInfo, LossyColor, LossyEntropy, Metadata, SampleType};

fn image(info: &ImageInfo) -> Vec<u8> {
    let (w, h) = (info.width as usize, info.height as usize);
    let mut px = vec![0u8; info.row_bytes() * h];
    for y in 0..h {
        for x in 0..w {
            for c in 0..3 {
                let ramp = (x * 3 + y * 2 + c * 40) as f32;
                let tex = 30.0 * ((x as f32 * 0.9 + c as f32).sin() * (y as f32 * 0.7).cos());
                let edge = if (x / 11 + y / 7) % 3 == 0 { 60.0 } else { 0.0 };
                px[(y * w + x) * 3 + c] = (ramp + tex + edge).rem_euclid(256.0) as u8;
            }
        }
    }
    px
}

#[test]
fn table_stats_match_the_written_files() {
    let info = ImageInfo::new(96, 72, Channels::Rgb, SampleType::U8);
    let px = image(&info);
    for (color, entropy, k) in [
        (LossyColor::Xyb, LossyEntropy::RunLevel, None),
        (LossyColor::Xyb, LossyEntropy::RunLevel, Some(0)),
        (LossyColor::Xyb, LossyEntropy::SplitHuf, Some(3)),
        (LossyColor::Xyb, LossyEntropy::SplitHuf, None),
        (LossyColor::YCoCg, LossyEntropy::RunLevel, None),
        (LossyColor::YCoCg, LossyEntropy::SplitHuf, Some(3)),
        (LossyColor::YCoCg, LossyEntropy::SplitHuf, None),
    ] {
        let mut cfg = imij::ImiEncoderConfig::lossy(30.0)
            .tune_lossy(|t| t.color = color)
            .tune_lossy(|t| t.entropy = entropy)
            .with_effort(7.5)
            .with_threads(1);
        if let Some(k) = k {
            cfg = cfg.tune_lossy(|t| t.lab.switches.table_k_mode = Some(k));
        }
        let file = cfg
            .encode_samples(
                imij::ImageRef::new(&px, info.width as usize * 3, &info),
                &Metadata::default(),
            )
            .expect("encode");
        let stats = imij::lab::lossy_table_stats();
        // The counter names the file's mode and its table bytes; the reading
        // is consumed.
        assert_eq!(stats[0], entropy as u64, "{color:?} {entropy:?} k={k:?}");
        assert!(stats[1] > 0, "{color:?} {entropy:?} k={k:?}: no tables");
        assert_eq!(imij::lab::lossy_table_stats(), [255, 0], "consumed");
        let (mode, bytes) =
            imij::lab::lossy_ac_table_bytes(&file).expect("parse the file's table section");
        assert_eq!(mode, entropy as u8, "{color:?} {entropy:?} k={k:?}");
        assert_eq!(
            bytes, stats[1],
            "{color:?} {entropy:?} k={k:?}: counter vs file"
        );
    }
}

/// Without a lossy encode on the thread, the counter is the `no encode`
/// reading; a second read is too.
#[test]
fn table_stats_empty_without_an_encode() {
    assert_eq!(imij::lab::lossy_table_stats(), [255, 0]);
    assert_eq!(imij::lab::lossy_table_stats(), [255, 0]);
}
