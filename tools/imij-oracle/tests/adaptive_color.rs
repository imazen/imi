//! Independent oracle conformance tests for feature 32 (`adaptive_color`, SPEC 2.4.1).
use imij::{Channels, Feature, ImageInfo, ImageRef, ImiEncoderConfig, Metadata, SampleType};

fn pattern(w: u32, h: u32, channels: Channels, opaque_alpha: bool) -> Vec<u8> {
    let nch = channels.count();
    let mut px = Vec::with_capacity(w as usize * h as usize * nch);
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) as usize;
            if idx < 4 {
                const ANCHORS: [[u8; 3]; 4] =
                    [[201, 173, 149], [1, 2, 3], [254, 253, 252], [129, 130, 131]];
                px.extend_from_slice(&ANCHORS[idx]);
                if nch == 4 {
                    px.push(if opaque_alpha { 255 } else { ANCHORS[idx][0] });
                }
                continue;
            }
            let lx = (x * 16) / w.max(1);
            let g = (((x * 3 + y * 5 + ((x ^ y) & 15)) & 0xFF) as u8)
                .wrapping_add(((x.wrapping_mul(17) ^ y.wrapping_mul(31)) & 7) as u8);
            let (gr, gb, rb): (i32, i32, i32) = match lx % 4 {
                0 => (48, 8, 24),
                1 => (16, 40, -12),
                2 => (-24, 12, 36),
                _ => (36, 28, 12),
            };
            let gs = g as i32 - 128;
            let nr = ((x.wrapping_mul(13).wrapping_add(y.wrapping_mul(7))) & 3) as i32 - 1;
            let nb = ((x.wrapping_mul(19).wrapping_add(y.wrapping_mul(11))) & 3) as i32 - 1;
            let r = (128 + ((gr * gs) >> 5) + nr).clamp(0, 255) as u8;
            let rs = r as i32 - 128;
            let b = (128 + ((gb * gs + rb * rs) >> 5) + nb).clamp(0, 255) as u8;
            px.push(r);
            px.push(g);
            px.push(b);
            if nch == 4 {
                px.push(if opaque_alpha {
                    255
                } else {
                    ((x * 5 + y * 9) & 0xFF) as u8
                });
            }
        }
    }
    px
}

#[test]
fn adaptive_color_matches_independent_oracle_across_layouts() {
    for &(w, h, lanes, col_groups) in &[
        (1, 4, None, None),
        (7, 9, None, None),
        (16, 12, None, None),
        (37, 25, Some(5), None),
        (137, 35, None, Some(48)),
    ] {
        for &(ch, opaque_alpha) in &[
            (Channels::Rgb, false),
            (Channels::Rgba, false),
            (Channels::Rgba, true),
        ] {
            let info = ImageInfo::new(w, h, ch, SampleType::U8);
            let src = pattern(w, h, ch, opaque_alpha);
            let mut cfg = ImiEncoderConfig::default()
                .with_effort(7.5)
                .tune_lossless(|t| {
                    t.decor_mask = 1 << 4;
                    t.palette = false;
                    t.stripe_rows = Some(16);
                    t.lanes = lanes;
                });
            if let Some(g) = col_groups {
                cfg = cfg.with_column_groups(Some(g));
            }
            let file = cfg
                .encode_legacy_samples(
                    ImageRef::new(&src, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap();
            assert!(
                imij::header(&file)
                    .unwrap()
                    .features
                    .contains(Feature::AdaptiveColor),
                "w={w} h={h} {ch:?} opaque={opaque_alpha}"
            );
            let decoded = imij_oracle::decode(&file).unwrap();
            assert_eq!(
                decoded.frames,
                [src],
                "oracle mismatch w={w} h={h} {ch:?} opaque={opaque_alpha}"
            );
        }
    }

    // Also verify I8 and 8-bit-replicated U16 through the independent oracle.
    let info_i8 = ImageInfo::new(67, 29, Channels::Rgb, SampleType::I8);
    let src_i8 = pattern(67, 29, Channels::Rgb, false);
    let file_i8 = ImiEncoderConfig::default()
        .with_effort(7.5)
        .tune_lossless(|t| {
            t.decor_mask = 1 << 4;
            t.palette = false;
            t.stripe_rows = Some(16);
        })
        .encode_legacy_samples(
            ImageRef::new(&src_i8, info_i8.row_bytes(), &info_i8),
            &Metadata::default(),
        )
        .unwrap();
    assert!(
        imij::header(&file_i8)
            .unwrap()
            .features
            .contains(Feature::AdaptiveColor)
    );
    assert_eq!(imij_oracle::decode(&file_i8).unwrap().frames, [src_i8]);

    let info_u16 = ImageInfo::new(67, 29, Channels::Rgb, SampleType::U16);
    let src_u16: Vec<u8> = pattern(67, 29, Channels::Rgb, false)
        .iter()
        .flat_map(|&v| (u16::from(v) * 257).to_le_bytes())
        .collect();
    let file_u16 = ImiEncoderConfig::default()
        .with_effort(7.5)
        .tune_lossless(|t| {
            t.decor_mask = 1 << 4;
            t.palette = false;
            t.stripe_rows = Some(16);
        })
        .encode_legacy_samples(
            ImageRef::new(&src_u16, info_u16.row_bytes(), &info_u16),
            &Metadata::default(),
        )
        .unwrap();
    assert!(
        imij::header(&file_u16)
            .unwrap()
            .features
            .contains(Feature::AdaptiveColor)
    );
    assert_eq!(imij_oracle::decode(&file_u16).unwrap().frames, [src_u16]);
}

#[test]
fn adaptive_color_oracle_rejects_missing_feature_and_corrupt_weights() {
    let info = ImageInfo::new(12, 8, Channels::Rgb, SampleType::U8);
    let mut src = Vec::with_capacity(12 * 8 * 3);
    for y in 0..8u8 {
        for x in 0..12u8 {
            let g = x * 17 + y * 19;
            src.extend_from_slice(&[g, g, g]);
        }
    }
    let valid = ImiEncoderConfig::default()
        .with_effort(7.5)
        .tune_lossless(|t| {
            t.lanes = Some(1);
            t.decor_mask = 1 << 4;
            t.palette = false;
            t.stripe_rows = Some(8);
        })
        .encode_legacy_samples(
            ImageRef::new(&src, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    assert_eq!(imij_oracle::decode(&valid).unwrap().frames, [src]);

    // Strip feature 32 from V3 header -> oracle must reject as Corrupt.
    let feat32_leb = [0x80u8, 0x80, 0x80, 0x80, 0x10];
    let pos = valid
        .windows(feat32_leb.len())
        .position(|w| w == feat32_leb)
        .expect("LEB128(1 << 32)");
    let mut stripped = Vec::with_capacity(valid.len() - 1);
    stripped.extend_from_slice(&valid[..pos]);
    stripped.extend_from_slice(&[0x80, 0x80, 0x80, 0x08]); // bit 24 instead of bit 32
    stripped.extend_from_slice(&valid[pos + 5..]);
    stripped[8] -= 1;
    assert!(matches!(
        imij_oracle::decode(&stripped),
        Err(imij_oracle::Error::Corrupt(_))
    ));

    // Corrupt weight byte to raw -128 -> oracle must reject as Corrupt.
    let data_pos = valid.windows(4).position(|w| w == b"DATA").unwrap();
    let decor_pos = (data_pos + 4..valid.len() - 4)
        .find(|&i| {
            valid[i] == 0x3b && valid[i + 1] > 10 && valid[i + 1] < 0x80 && valid[i + 2] == 4
        })
        .map(|i| i + 2)
        .unwrap();
    let mut bad_w = valid.clone();
    bad_w[decor_pos + 1] = 0x01;
    bad_w[decor_pos + 2] |= 0x01;
    assert!(matches!(
        imij_oracle::decode(&bad_w),
        Err(imij_oracle::Error::Corrupt(_))
    ));
}
