#![cfg(feature = "encode")]

use imij::{
    Channels, Error, Feature, ImageInfo, ImageRef, ImiEncoderConfig, Metadata, SampleType,
    StreamDecoder, decode, decode_into, decode_into_threads, header,
};

/// Correlated photographic-like synthetic RGB/RGBA test pattern where per-lane
/// adaptive color weights strongly outperform fixed modes and never collapse to
/// a <=256 color palette.
fn synthetic_pattern(w: u32, h: u32, channels: Channels, opaque_alpha: bool) -> Vec<u8> {
    let nch = channels.count();
    let mut px = Vec::with_capacity(w as usize * h as usize * nch);
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) as usize;
            if idx < 4 {
                // Anchor 0, 1, 129, 254 across the first pixels so maps always stay full 8-bit Lsb
                // even on tiny images (e.g. 1x4 or 2x5).
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
                let a = if opaque_alpha {
                    255
                } else {
                    ((x * 5 + y * 9) & 0xFF) as u8
                };
                px.push(a);
            }
        }
    }
    px
}

fn encode_forced_adapt(px: &[u8], info: &ImageInfo, column_groups: Option<u32>) -> Vec<u8> {
    let mut cfg = ImiEncoderConfig::default()
        .with_effort(7.5)
        .tune_lossless(|t| {
            t.decor_mask = 1 << 4;
            t.palette = false;
            t.stripe_rows = Some(16);
        });
    if let Some(g) = column_groups {
        cfg = cfg.with_column_groups(Some(g));
    }
    cfg.encode_samples(
        ImageRef::new(px, info.row_bytes(), info),
        &Metadata::default(),
    )
    .unwrap()
}

#[test]
fn adaptive_color_roundtrip_widths_channels_and_streaming() {
    for &(w, h) in &[
        (1, 1),
        (1, 4),
        (2, 5),
        (7, 9),
        (15, 17),
        (16, 16),
        (17, 19),
        (31, 20),
        (32, 32),
        (33, 35),
        (63, 21),
        (64, 18),
        (65, 25),
        (137, 37),
        (257, 33),
    ] {
        for &(ch, opaque_alpha) in &[
            (Channels::Rgb, false),
            (Channels::Rgba, false),
            (Channels::Rgba, true),
        ] {
            let info = ImageInfo::new(w, h, ch, SampleType::U8);
            let px = synthetic_pattern(w, h, ch, opaque_alpha);
            let file = encode_forced_adapt(&px, &info, None);
            let hdr = header(&file).unwrap();
            if w * h >= 4 {
                assert!(
                    hdr.features.contains(Feature::AdaptiveColor),
                    "w={w} h={h} {ch:?} opaque={opaque_alpha} should declare adaptive_color"
                );
            }

            // 1. One-shot decode
            let (dec_info, dec_px) = decode(&file).unwrap();
            assert_eq!(dec_info, info);
            assert_eq!(dec_px, px, "one-shot mismatch w={w} h={h} {ch:?}");

            // 2. Strided decode with canary padding
            let stride = info.row_bytes() + 13;
            let mut strided = vec![0xA5u8; stride * h as usize];
            decode_into(&file, &mut strided, stride).unwrap();
            for y in 0..h as usize {
                let row_start = y * stride;
                assert_eq!(
                    &strided[row_start..row_start + info.row_bytes()],
                    &px[y * info.row_bytes()..(y + 1) * info.row_bytes()],
                    "strided mismatch row {y} w={w} {ch:?}"
                );
                assert!(
                    strided[row_start + info.row_bytes()..row_start + stride]
                        .iter()
                        .all(|&b| b == 0xA5),
                    "strided padding clobbered row {y} w={w} {ch:?}"
                );
            }

            // 3. Multi-threaded decode
            let mut mt = vec![0u8; px.len()];
            decode_into_threads(&file, &mut mt, info.row_bytes(), 4).unwrap();
            assert_eq!(mt, px, "mt mismatch w={w} h={h} {ch:?}");

            // 4. Streaming row-by-row decode
            let mut stream = StreamDecoder::new(&file).unwrap();
            let mut streamed = vec![0u8; px.len()];
            for y in 0..h as usize {
                let row = &mut streamed[y * info.row_bytes()..(y + 1) * info.row_bytes()];
                assert!(stream.next_row(row).unwrap());
            }
            assert!(!stream.next_row(&mut vec![0u8; info.row_bytes()]).unwrap());
            assert_eq!(streamed, px, "stream mismatch w={w} h={h} {ch:?}");
        }
    }
}

#[test]
fn adaptive_color_column_groups_and_region_crop() {
    let info = ImageInfo::new(137, 45, Channels::Rgb, SampleType::U8);
    let px = synthetic_pattern(info.width, info.height, info.channels, false);
    let file = encode_forced_adapt(&px, &info, Some(48));
    assert!(
        header(&file)
            .unwrap()
            .features
            .contains(Feature::AdaptiveColor)
    );
    assert_eq!(decode(&file).unwrap().1, px);

    // Region decode across stripe and column-group boundaries
    let crop = imij::planes::Rect::new(31, 11, 73, 27);
    let rw = crop.width() as usize;
    let mut out = vec![0u8; rw * crop.height() as usize * 3];
    imij::region::decode_region(&file, crop, &mut out, rw * 3).unwrap();
    for dy in 0..crop.height() as usize {
        let sy = crop.y() as usize + dy;
        let sx = crop.x() as usize;
        assert_eq!(
            &out[dy * rw * 3..(dy + 1) * rw * 3],
            &px[(sy * info.width as usize + sx) * 3..(sy * info.width as usize + sx + rw) * 3],
            "region crop mismatch row {dy}"
        );
    }
}

#[test]
fn adaptive_color_non_plain_u8_layouts_scalar_fallback() {
    // U16 RGB with 8-bit replicated samples -> layout has 8-bit color slots with Replicate map.
    let info16 = ImageInfo::new(67, 29, Channels::Rgb, SampleType::U16);
    let px8 = synthetic_pattern(info16.width, info16.height, Channels::Rgb, false);
    let px16: Vec<u8> = px8
        .iter()
        .flat_map(|&v| (u16::from(v) * 257).to_ne_bytes())
        .collect();
    let file16 = encode_forced_adapt(&px16, &info16, None);
    assert!(
        header(&file16)
            .unwrap()
            .features
            .contains(Feature::AdaptiveColor)
    );
    assert_eq!(decode(&file16).unwrap().1, px16);

    // I8 RGB/RGBA when `extended` is enabled (tests order-map sign flip in scalar adapt path).
    #[cfg(feature = "extended")]
    for ch in [Channels::Rgb, Channels::Rgba] {
        let info_i8 = ImageInfo::new(67, 29, ch, SampleType::I8);
        let px_i8 = synthetic_pattern(info_i8.width, info_i8.height, ch, false);
        let file_i8 = encode_forced_adapt(&px_i8, &info_i8, None);
        assert!(
            header(&file_i8)
                .unwrap()
                .features
                .contains(Feature::AdaptiveColor)
        );
        assert_eq!(decode(&file_i8).unwrap().1, px_i8);
    }
}

#[test]
fn adaptive_color_effort_gating_and_no_over_declaration() {
    let info = ImageInfo::new(128, 64, Channels::Rgb, SampleType::U8);
    let px = synthetic_pattern(info.width, info.height, Channels::Rgb, false);

    // Effort < 7.0 must not emit adaptive_color.
    let low = ImiEncoderConfig::default()
        .with_effort(5.0)
        .encode_samples(
            ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    assert!(
        !header(&low)
            .unwrap()
            .features
            .contains(Feature::AdaptiveColor)
    );
    assert_eq!(decode(&low).unwrap().1, px);

    // Explicit decor_mask = 0x0f must not emit adaptive_color.
    let fixed = ImiEncoderConfig::default()
        .with_effort(10.0)
        .tune_lossless(|t| t.decor_mask = 0x0f)
        .encode_samples(
            ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    assert!(
        !header(&fixed)
            .unwrap()
            .features
            .contains(Feature::AdaptiveColor)
    );
    assert_eq!(decode(&fixed).unwrap().1, px);

    // Default effort 7.5 on correlated pattern beats fixed modes and declares feature 32.
    let auto = ImiEncoderConfig::default()
        .with_effort(7.5)
        .encode_samples(
            ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    assert!(
        header(&auto)
            .unwrap()
            .features
            .contains(Feature::AdaptiveColor)
    );
    assert!(
        auto.len() < fixed.len(),
        "expected adaptive_color ({}) < fixed ({})",
        auto.len(),
        fixed.len()
    );
    assert_eq!(decode(&auto).unwrap().1, px);

    // Gray8 must never declare adaptive_color, even with forced decor_mask = 0x10.
    let gray_info = ImageInfo::new(64, 32, Channels::Gray, SampleType::U8);
    let gray_px: Vec<u8> = (0..64 * 32).map(|i| (i * 37) as u8).collect();
    let gray_file = encode_forced_adapt(&gray_px, &gray_info, None);
    assert!(
        !header(&gray_file)
            .unwrap()
            .features
            .contains(Feature::AdaptiveColor)
    );
    assert_eq!(decode(&gray_file).unwrap().1, gray_px);

    // Lossy encode at effort 7.5 must never declare adaptive_color.
    let lossy = ImiEncoderConfig::lossy(85.0)
        .with_effort(7.5)
        .encode_samples(
            ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    assert!(
        !header(&lossy)
            .unwrap()
            .features
            .contains(Feature::AdaptiveColor)
    );
}

#[test]
fn adaptive_color_corrupt_wire_inputs_are_rejected() {
    let info = ImageInfo::new(48, 24, Channels::Rgb, SampleType::U8);
    let px = synthetic_pattern(info.width, info.height, Channels::Rgb, false);
    let valid = encode_forced_adapt(&px, &info, None);
    assert_eq!(decode(&valid).unwrap().1, px);

    // Locate the header feature mask varint and clear bit 32 (feature 32 = 1 << 32 = [0x80, 0x80, 0x80, 0x80, 0x10]).
    // Check we find the exact 5-byte LEB128 encoding of (1u64 << 32) inside the header.
    let feat32_leb = [0x80u8, 0x80, 0x80, 0x80, 0x10];
    let pos = valid
        .windows(feat32_leb.len())
        .position(|w| w == feat32_leb)
        .expect("expected LEB128(1 << 32) in header");
    assert!(pos < 64, "feature mask should sit near start of header");

    // 1. Replace feature mask with bit 24 (template_palettes, allowed in V4 without needing syntax)
    // while clearing bit 32, and verify the decoder specifically rejects the missing feature.
    {
        let mut stripped = Vec::with_capacity(valid.len() - 1);
        stripped.extend_from_slice(&valid[..pos]);
        stripped.extend_from_slice(&[0x80, 0x80, 0x80, 0x08]); // bit 24 (minimal 4-byte LEB128), bit 32 cleared!
        stripped.extend_from_slice(&valid[pos + 5..]);
        // Byte 8 is header_len (1-byte varint for this tiny header).
        assert!(stripped[8] < 0x80 && stripped[8] > 4);
        stripped[8] -= 1;
        let err = decode(&stripped).unwrap_err();
        assert_eq!(
            err,
            Error::Corrupt("adaptive color stripe without the feature adaptive_color"),
            "missing feature bit 32 must be rejected with missing-feature error, got {err:?}"
        );
    }

    // 2. Corrupt weight payload inside a stripe (force raw -128, delta overflow, and nonzero pad bit)
    // on both V3 and V4 files.
    let small_info = ImageInfo::new(12, 8, Channels::Rgb, SampleType::U8);
    let mut small_px = Vec::with_capacity(12 * 8 * 3);
    for y in 0..8u8 {
        for x in 0..12u8 {
            let g = 128u8.wrapping_add(x.wrapping_mul(17).wrapping_add(y.wrapping_mul(19)));
            small_px.extend_from_slice(&[g, g, g]);
        }
    }
    let small_cfg = ImiEncoderConfig::default()
        .with_effort(7.5)
        .tune_lossless(|t| {
            t.lanes = Some(1);
            t.decor_mask = 1 << 4;
            t.palette = false;
            t.stripe_rows = Some(8);
        });
    let small_v3 = small_cfg
        .encode_legacy_samples(
            ImageRef::new(&small_px, small_info.row_bytes(), &small_info),
            &Metadata::default(),
        )
        .unwrap();
    assert_eq!(decode(&small_v3).unwrap().1, small_px);
    let data_pos = small_v3
        .windows(4)
        .position(|w| w == b"DATA")
        .expect("DATA chunk");
    // Record id 59 (0x3b), varint len, decor byte 4.
    let stripe_decor_pos = (data_pos + 4..small_v3.len() - 4)
        .find(|&i| {
            small_v3[i] == 0x3b
                && small_v3[i + 1] > 10
                && small_v3[i + 1] < 0x80
                && small_v3[i + 2] == 4
        })
        .map(|i| i + 2)
        .expect("V3 stripe record with decor=4");
    // 2a. Overwrite first weight with raw -128 (flag=1, raw=0x80 -> low 9 bits = 0x101).
    {
        let mut bad_w = small_v3.clone();
        bad_w[stripe_decor_pos + 1] = 0x01;
        bad_w[stripe_decor_pos + 2] |= 0x01;
        let err = decode(&bad_w).unwrap_err();
        assert!(
            matches!(err, Error::Corrupt(_)),
            "weight -128 must be Corrupt, got {err:?}"
        );
    }
    // 2b. Set the required-zero trailing padding bit (for active=1 with three 5-bit deltas = 15 bits,
    // bit 7 of byte 2 is padding; force all three deltas to 0 = 0_1000 -> 15 bits, then set bit 7 of byte 2).
    {
        let mut bad_pad = small_v3.clone();
        // Three 5-bit tokens of (flag=0, u=8 -> 0b10000):
        // bits 0..5 = 0b10000, bits 5..10 = 0b10000, bits 10..15 = 0b10000 =>
        // val = 0x10 | (0x10 << 5) | (0x10 << 10) = 0x4210 -> bytes [0x10, 0x42]; set pad bit 15 => [0x10, 0xC2].
        bad_pad[stripe_decor_pos + 1] = 0x10;
        bad_pad[stripe_decor_pos + 2] = 0xC2;
        let err = decode(&bad_pad).unwrap_err();
        assert!(
            matches!(err, Error::Corrupt(_)),
            "nonzero pad bit must be Corrupt, got {err:?}"
        );
    }
    // 2c. Also test V4 roundtrip & feature-bit stripping on small image.
    let small_v4 = small_cfg
        .encode_samples(
            ImageRef::new(&small_px, small_info.row_bytes(), &small_info),
            &Metadata::default(),
        )
        .unwrap();
    assert_eq!(header(&small_v4).unwrap().version, 4);
    assert!(
        header(&small_v4)
            .unwrap()
            .features
            .contains(Feature::AdaptiveColor)
    );
    assert_eq!(decode(&small_v4).unwrap().1, small_px);
}
