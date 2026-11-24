//! LAB: the AC trellis's process-global switches
//! (`lab::set_trellis_aq_lambda_exp`, `lab::set_trellis_huf_rate`,
//! `lab::set_trellis_channel_shift`). One test, so no other test of this
//! binary encodes while a switch is set; each switch ends at its default.
#![cfg(feature = "lab")]
use imij::{
    Channels, DecodeSpeed, ImageInfo, ImiEncoderConfig, LossyAqField, LossyColor, LossyEntropy,
    Metadata, SampleType,
};

/// Smooth ramps, texture and hard edges (as `tests/trellis.rs`).
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
                let ramp = (x * 3 + y * 2 + c * 40) as f32;
                let tex = 25.0 * ((x as f32 * 0.9 + c as f32).sin() * (y as f32 * 0.7).cos());
                let edge = if (x / 11 + y / 7) % 3 == 0 { 60.0 } else { 0.0 };
                px[(y * w + x) * n + c] = ((ramp + tex + edge) % 256.0).clamp(0.0, 255.0) as u8;
            }
        }
    }
    px
}

fn encode(px: &[u8], info: &ImageInfo, cfg: &ImiEncoderConfig) -> Vec<u8> {
    cfg.encode_samples(
        imij::ImageRef::new(px, info.row_bytes(), info),
        &Metadata::default(),
    )
    .unwrap()
}

/// Decodes through the whole-image, threaded and streaming entry points
/// (same pixels) and returns the pixels.
fn decoded(file: &[u8], info: &ImageInfo, what: &str) -> Vec<u8> {
    let (_, dec) = imij::decode(file).unwrap();
    let r = info.row_bytes();
    let mut out = vec![0u8; r * info.height as usize];
    imij::decode_into_threads(file, &mut out, r, 4).unwrap();
    assert_eq!(out, dec, "{what}: threaded decode");
    let mut streamed = Vec::with_capacity(dec.len());
    imij::decode_reader(file, &imij::Limits::DEFAULT, |_, _, row| {
        streamed.extend_from_slice(row);
        Ok(())
    })
    .unwrap();
    assert_eq!(streamed, dec, "{what}: streamed decode");
    dec
}

#[test]
fn trellis_lab_switches() {
    let info = ImageInfo::new(170, 122, Channels::Rgb, SampleType::U8);
    let px = image(&info);

    // The lambda exponent per AQ multiplier moves AQ files and only them.
    let mut aq = imij::ImiEncoderConfig::lossy(50.0).with_effort(10.0);
    aq = aq.tune_lossy(|t| {
        t.lab.aq_field = Some(LossyAqField {
            strength: 1.0,
            levels: 4,
            cell: 1,
            relative: true,
            fade: false,
        });
    });
    let plain = encode(&px, &info, &aq);
    imij::lab::set_trellis_aq_lambda_exp(2.0);
    let exp = encode(&px, &info, &aq);
    let no_aq = imij::ImiEncoderConfig::lossy(50.0).with_effort(10.0);
    let off_a = encode(&px, &info, &no_aq);
    imij::lab::set_trellis_aq_lambda_exp(0.0);
    let off_b = encode(&px, &info, &no_aq);
    assert_ne!(plain, exp, "exponent ignored");
    assert_eq!(off_a, off_b, "exponent moved a file without AQ");
    decoded(&exp, &info, "AQ exponent");

    // The same exponent moves split-Huffman AQ files.
    let aq_sh = aq
        .clone()
        .tune_lossy(|t| t.entropy = LossyEntropy::SplitHuf);
    let plain_sh = encode(&px, &info, &aq_sh);
    imij::lab::set_trellis_aq_lambda_exp(2.0);
    let exp_sh = encode(&px, &info, &aq_sh);
    imij::lab::set_trellis_aq_lambda_exp(0.0);
    assert_ne!(plain_sh, exp_sh, "split-Huffman exponent ignored");
    decoded(&exp_sh, &info, "split-Huffman AQ exponent");

    // The split-Huffman rate model (on by default): off, split-Huffman keeps
    // the default quantization (the pixels of effort 8.99); on, the trellis
    // runs at efforts 9 and 10; run-level files ignore the switch.
    for color in [LossyColor::Xyb, LossyColor::YCoCg] {
        for q in [5.0, 30.0, 70.0, 95.0] {
            for effort in [9.0, 10.0] {
                let what = format!("{color:?} q{q} e{effort}");
                let sh = imij::ImiEncoderConfig::lossy(q)
                    .tune_lossy(|t| t.color = color)
                    .tune_lossy(|t| t.entropy = LossyEntropy::SplitHuf)
                    .with_effort(effort);
                let rl = sh
                    .clone()
                    .tune_lossy(|t| t.entropy = LossyEntropy::RunLevel);
                let on = encode(&px, &info, &sh);
                let on4 = encode(&px, &info, &sh.clone().with_threads(4));
                let rl_on = encode(&px, &info, &rl);
                imij::lab::set_trellis_huf_rate(false);
                let below = encode(&px, &info, &sh.clone().with_effort(8.99));
                let off = encode(&px, &info, &sh);
                let rl_off = encode(&px, &info, &rl);
                imij::lab::set_trellis_huf_rate(true);
                assert_eq!(on, on4, "{what}: bytes depend on threads");
                assert_eq!(rl_on, rl_off, "{what}: run-level moved");
                assert_eq!(
                    decoded(&off, &info, &what),
                    decoded(&below, &info, &what),
                    "{what}: split-Huffman trellis without the switch"
                );
                let p_on = decoded(&on, &info, &what);
                // The effort trellis runs on XYB only; YCoCg is untouched.
                if color == LossyColor::Xyb {
                    assert_ne!(p_on, decoded(&off, &info, &what), "{what}: no trellis");
                } else {
                    assert_eq!(on, off, "{what}: YCoCg moved");
                }
            }
        }
    }

    // Tiers: split-Huffman tier files carry the split-Huffman trellis's
    // pixels with the switch on (both the cost model's large images and
    // small images' split-Huffman candidate) and decode everywhere; with it
    // off they carry the effort-8.99 pixels (the rule before 2026-10-09).
    let mut moved = 0;
    for (w, h) in [(16, 16), (64, 64), (139, 25), (255, 255), (512, 384)] {
        let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
        let px = image(&info);
        for speed in [
            DecodeSpeed::Compact,
            DecodeSpeed::Balanced,
            DecodeSpeed::Fast,
        ] {
            for q in [5.0, 20.0, 50.0, 90.0] {
                let what = format!("{w}x{h} {speed:?} q{q}");
                let cfg = imij::ImiEncoderConfig::lossy(q)
                    .with_decode_speed(speed)
                    .with_effort(10.0);
                let on = encode(&px, &info, &cfg);
                imij::lab::set_trellis_huf_rate(false);
                let off = encode(&px, &info, &cfg);
                // The rule before 2026-10-09 predates superblocks (Compact
                // merges below effort 9 from 128 px): the reference without.
                let mut below_cfg = cfg.clone().with_effort(8.99);
                below_cfg = below_cfg.tune_lossy(|t| t.lab.switches.superblocks = Some(false));
                let below = encode(&px, &info, &below_cfg);
                imij::lab::set_trellis_huf_rate(true);
                let (mode, ..) = imij::seeds::lossy_entropy_seed(&on).unwrap();
                let p_on = decoded(&on, &info, &what);
                if mode == LossyEntropy::SplitHuf as u8 && p_on != decoded(&off, &info, &what) {
                    moved += 1;
                }
                if mode == LossyEntropy::RunLevel as u8 {
                    assert_eq!(on, off, "{what}: run-level tier file moved");
                }
                let (mode_off, ..) = imij::seeds::lossy_entropy_seed(&off).unwrap();
                if mode_off == LossyEntropy::SplitHuf as u8 {
                    assert_eq!(
                        decoded(&off, &info, &what),
                        decoded(&below, &info, &what),
                        "{what}: split-Huffman tier file with trellis pixels, switch off"
                    );
                }
            }
        }
    }
    assert!(moved > 0, "no split-Huffman tier file took the trellis");

    // Per-channel lambda offsets: zero is the default; chroma offsets move
    // XYB run-level effort-10 files.
    let rl = imij::ImiEncoderConfig::lossy(40.0).with_effort(10.0);
    let base = encode(&px, &info, &rl);
    imij::lab::set_trellis_channel_shift([0.0; 3]);
    assert_eq!(encode(&px, &info, &rl), base, "zero shift moved a file");
    imij::lab::set_trellis_channel_shift([0.0, 4.0, 4.0]);
    let shifted = encode(&px, &info, &rl);
    imij::lab::set_trellis_channel_shift([0.0; 3]);
    assert_ne!(shifted, base, "channel shift ignored");
    decoded(&shifted, &info, "channel shift");
    assert_eq!(encode(&px, &info, &rl), base, "shift reset");
}
