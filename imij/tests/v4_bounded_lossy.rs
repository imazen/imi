//! V4 serialization uses the native bounded encoder as its pixel oracle.
use imij::stream_encode::{BoundedLossyConfig, LossyUnitIndex};
use imij::{
    Channels, DecodeSpeed, ImageInfo, ImiEncoderConfig, LossyEntropy, Metadata, SampleType,
};
use std::io::Cursor;

fn raster(info: &ImageInfo, stride: usize) -> Vec<u8> {
    let mut source = vec![0xee; stride * info.height as usize];
    for y in 0..info.height as usize {
        for x in 0..info.width as usize {
            for c in 0..info.channels.count() {
                let v = match (x / 23 + y / 17) % 4 {
                    0 => 240,
                    1 if (x + y) % 3 == 0 => 20,
                    1 => 235,
                    2 => ((x * 7 + y * 13 + c * 91) % 251) as u8,
                    _ => ((x * 3 + y * 2 + c * 50) % 256) as u8,
                };
                let at = y * stride + (x * info.channels.count() + c) * info.sample.bytes();
                match info.sample {
                    SampleType::U8 => source[at] = v,
                    SampleType::U16 => {
                        source[at..at + 2].copy_from_slice(&(u16::from(v) * 257).to_ne_bytes())
                    }
                    SampleType::U32 => source[at..at + 4]
                        .copy_from_slice(&(u32::from(v) * 16_843_009).to_ne_bytes()),
                    SampleType::F16 => source[at..at + 2]
                        .copy_from_slice(&(0x3000 + u16::from(v) * 4).to_ne_bytes()),
                    SampleType::F32 => source[at..at + 4]
                        .copy_from_slice(&(0.125 + f32::from(v) / 512.0).to_ne_bytes()),
                    _ => panic!("unsupported fixture sample"),
                }
            }
        }
    }
    source
}
fn check(info: &ImageInfo, config: &ImiEncoderConfig) -> Vec<u8> {
    let stride = info.row_bytes() + 19;
    let source = raster(info, stride);
    check_source(info, config, &source, stride)
}
/// Returns `(setup_start, setup_end, data_end, present_offset, integrity)`
/// for the V4 lossy `DATA` chunk (SPEC_V4.md).
fn v4_data_setup_layout(file: &[u8]) -> (usize, usize, usize, usize, u64) {
    fn varint(b: &[u8], pos: &mut usize) -> u64 {
        let mut v = 0u64;
        let mut shift = 0u32;
        loop {
            let byte = b[*pos];
            *pos += 1;
            v |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return v;
            }
            shift += 7;
        }
    }
    assert_eq!(&file[..8], b"IMIJ\r\n\x87\n");
    let mut pos = 8;
    let hlen = varint(file, &mut pos) as usize;
    pos += hlen;
    while pos + 4 <= file.len() {
        let tag = &file[pos..pos + 4];
        pos += 4;
        let len_plus1 = varint(file, &mut pos);
        if tag == b"DATA" {
            let data_end = if len_plus1 == 0 {
                file.len()
            } else {
                pos + (len_plus1 - 1) as usize
            };
            let setup_len = varint(file, &mut pos) as usize;
            let setup_start = pos;
            let setup_end = setup_start + setup_len;
            // 7 header varints: rows, columns, tile_cols, tile_rows, order, integrity, entropy.
            let mut integrity = 0;
            for i in 0..7 {
                let v = varint(file, &mut pos);
                if i == 5 {
                    integrity = v;
                }
            }
            let _q_len = varint(file, &mut pos);
            assert!(
                file[pos] < 0x80,
                "expected 1-byte quantizer.present varint, got {:#x}",
                file[pos]
            );
            let present_off = pos;
            return (setup_start, setup_end, data_end, present_off, integrity);
        }
        assert!(len_plus1 > 0, "open-ended non-DATA chunk");
        pos += (len_plus1 - 1) as usize;
    }
    panic!("DATA chunk not found in V4 file");
}

/// Reads the V4 lossy setup's `quantizer.present` bit 5 (`1 << 5`, sRGB XYB power transfer)
/// directly from the encoded file's `DATA` chunk (SPEC_V4.md).
fn v4_quantizer_power_bit(file: &[u8]) -> bool {
    let (_, _, _, present_off, _) = v4_data_setup_layout(file);
    file[present_off] & (1 << 5) != 0
}

/// Returns a copy of an upgraded V4 lossy file (`ObjectIntegrity::Objects`) with
/// `quantizer.present` bit 5 enabled and every framed object re-bound to the
/// updated `SetupContext`.
fn v4_enable_power_bit(info: &ImageInfo, color: imij::LossyColor, upgraded: &[u8]) -> Vec<u8> {
    use imij::decode_resources::{DecodePolicy, DecodeSession};
    use imij::v4::{DirectoryPrefix, ObjectIntegrity, ObjectReader, SetupContext, append_object};

    let (setup_start, setup_end, data_end, present_off, integrity) = v4_data_setup_layout(upgraded);
    let mode = ObjectIntegrity::try_from(integrity).unwrap();
    assert_eq!(mode, ObjectIntegrity::Objects);
    let old_ctx = SetupContext::from_image_declaration(
        info,
        imij::Coding::Lossy,
        Some(color),
        &upgraded[setup_start..setup_end],
    )
    .unwrap();
    let mut out = upgraded.to_vec();
    assert!(out[present_off] < 0x40);
    out[present_off] |= 1 << 5;
    let new_ctx = SetupContext::from_image_declaration(
        info,
        imij::Coding::Lossy,
        Some(color),
        &out[setup_start..setup_end],
    )
    .unwrap();
    let session = DecodeSession::new(
        DecodePolicy::default()
            .with_output_limits(imij::Limits::NONE)
            .with_max_input_bytes(u64::MAX)
            .with_max_work_units(u64::MAX),
    );
    let prefix = DirectoryPrefix::read(&upgraded[setup_end..data_end], &session).unwrap();
    let objects_start = setup_end + prefix.encoded_len;
    let reader = ObjectReader::new(mode, old_ctx, &session, u64::MAX);
    let mut reframed = Vec::with_capacity(data_end - objects_start);
    let mut cursor = objects_start;
    while cursor < data_end {
        let obj = reader.read(&upgraded[cursor..data_end]).unwrap();
        append_object(&mut reframed, obj.kind, obj.body, mode, new_ctx, u64::MAX).unwrap();
        cursor += obj.encoded_len;
    }
    assert_eq!(reframed.len(), data_end - objects_start);
    out[objects_start..data_end].copy_from_slice(&reframed);
    out
}

fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&(data.len() as u64 * 8).to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for t in 0..16 {
            w[t] = u32::from_be_bytes(chunk[4 * t..4 * t + 4].try_into().unwrap());
        }
        for t in 16..64 {
            let s0 = w[t - 15].rotate_right(7) ^ w[t - 15].rotate_right(18) ^ (w[t - 15] >> 3);
            let s1 = w[t - 2].rotate_right(17) ^ w[t - 2].rotate_right(19) ^ (w[t - 2] >> 10);
            w[t] = w[t - 16]
                .wrapping_add(s0)
                .wrapping_add(w[t - 7])
                .wrapping_add(s1);
        }
        let mut v = h;
        for t in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[t])
                .wrapping_add(w[t]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [
                t1.wrapping_add(t2),
                v[0],
                v[1],
                v[2],
                v[3].wrapping_add(t1),
                v[4],
                v[5],
                v[6],
            ];
        }
        for (a, b) in h.iter_mut().zip(v) {
            *a = a.wrapping_add(b);
        }
    }
    let mut hex = String::with_capacity(64);
    for word in h {
        use std::fmt::Write;
        write!(&mut hex, "{word:08x}").unwrap();
    }
    hex
}

fn check_source(
    info: &ImageInfo,
    config: &ImiEncoderConfig,
    source: &[u8],
    stride: usize,
) -> Vec<u8> {
    let mut prefixed = vec![7; 11];
    prefixed.extend_from_slice(source);
    let mut a = Cursor::new(prefixed.clone());
    let mut b = Cursor::new(prefixed.clone());
    a.set_position(11);
    b.set_position(11);
    let cfg = BoundedLossyConfig::new(stride as u64, LossyUnitIndex::Trailing, 8 << 20);
    let (native, _, _) = imij::stream_encode::encode_lossy_bounded(
        &mut a,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        config.checksum().then(|| Cursor::new(Vec::new())),
        info.clone(),
        &Metadata::default(),
        config,
        cfg,
        &how_far::NoPulse,
    )
    .unwrap();
    let expected = imij::decode(native.get_ref()).unwrap();
    let (file, index, checksum) = imij::v4::encode_lossy_bounded(
        &mut b,
        Vec::new(),
        Cursor::new(vec![11, 13]),
        config.checksum().then(|| Cursor::new(vec![17, 19])),
        info.clone(),
        &Metadata::default(),
        config,
        imij::v4::BoundedLossyConfig::new(stride as u64, 8 << 20),
        &how_far::NoPulse,
    )
    .unwrap();
    let hdr = imij::header(&file).unwrap();
    assert_eq!(hdr.version, 4);
    let actual = imij::decode(&file).unwrap();
    assert_eq!(actual.0, expected.0);
    // Verify the emitted V4 quantizer bit 5 matches SPEC_V4.md eligibility:
    // set iff full-precision sRGB U8/U16 XYB; clear on YCoCg, U32/F16/F32, or non-sRGB.
    let expected_power = hdr.lossy_color == Some(imij::LossyColor::Xyb)
        && matches!(info.sample, SampleType::U8 | SampleType::U16)
        && info.cicp == imij::Cicp::SRGB
        && info.significant_bits == [0; 4];
    let wire_power = v4_quantizer_power_bit(&file);
    assert_eq!(
        wire_power, expected_power,
        "V4 quantizer bit 5 mismatch for {:?} {:?} {:?}",
        hdr.lossy_color, info.sample, info.cicp
    );
    if !wire_power {
        assert_eq!(actual.1, expected.1);
    } else {
        // Bit-exact oracle for power-enabled V4 streams: upgrade the V3 native
        // codestream to V4 (which preserves legacy reconstruction bit-for-bit)
        // and enable quantizer bit 5 on the upgraded setup. The resulting
        // decoded raster must match `imij::v4::encode_lossy_bounded` 0-bit exact.
        let native_no_csum: Vec<u8> = if !config.checksum() {
            native.into_inner()
        } else {
            let mut c = Cursor::new(prefixed);
            c.set_position(11);
            let nocsum_cfg = config.clone().with_checksum(false);
            let (plain, _, _) = imij::stream_encode::encode_lossy_bounded(
                &mut c,
                Cursor::new(Vec::new()),
                Cursor::new(Vec::new()),
                None,
                info.clone(),
                &Metadata::default(),
                &nocsum_cfg,
                cfg,
                &how_far::NoPulse,
            )
            .unwrap();
            let plain = plain.into_inner();
            assert_eq!(imij::decode(&plain).unwrap().1, expected.1);
            plain
        };
        let upgraded =
            imij::v4::upgrade_image(&native_no_csum, &imij::UpgradeLimits::default()).unwrap();
        assert!(!v4_quantizer_power_bit(&upgraded));
        assert_eq!(imij::decode(&upgraded).unwrap().1, expected.1);
        let upgraded_power = v4_enable_power_bit(info, hdr.lossy_color.unwrap(), &upgraded);
        let power_oracle = imij::decode(&upgraded_power).unwrap();
        assert_eq!(actual.1, power_oracle.1);
    }
    assert_eq!(&index.get_ref()[..2], &[11, 13]);
    assert_eq!(checksum.is_some(), config.checksum());
    if let Some(checksum) = checksum {
        assert_eq!(&checksum.get_ref()[..2], &[17, 19]);
    }
    for threads in [1, 2, 4, 8] {
        let stride = info.row_bytes() + 13;
        let mut out = vec![0xa7; stride * info.height as usize];
        imij::decode_into_threads(&file, &mut out, stride, threads).unwrap();
        for y in 0..info.height as usize {
            assert_eq!(
                &out[y * stride..][..info.row_bytes()],
                &actual.1[y * info.row_bytes()..][..info.row_bytes()]
            );
            assert!(
                out[y * stride + info.row_bytes()..(y + 1) * stride]
                    .iter()
                    .all(|&b| b == 0xa7)
            );
        }
    }
    file
}
#[test]
fn bounded_v4_matches_native_samples_coders_alpha_and_checksums() {
    for sample in [
        SampleType::U8,
        SampleType::U16,
        SampleType::U32,
        SampleType::F16,
        SampleType::F32,
    ] {
        for channels in [
            Channels::Gray,
            Channels::GrayAlpha,
            Channels::Rgb,
            Channels::Rgba,
        ] {
            let info = ImageInfo::new(67, 51, channels, sample);
            for coder in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                for checksum in [false, true] {
                    let config = ImiEncoderConfig::lossy(73.0)
                        .with_effort(0.0)
                        .with_column_groups(Some(32))
                        .with_checksum(checksum)
                        .tune_lossy(|t| {
                            t.entropy = coder;
                            t.stripe_block_rows = 2;
                            t.exact_blocks = Some(0.0);
                        });
                    check(&info, &config);
                }
            }
        }
    }
    // YCoCg U8/U16 must leave V4 quantizer bit 5 clear and match V3 bit-for-bit.
    for sample in [SampleType::U8, SampleType::U16] {
        let info = ImageInfo::new(67, 51, Channels::Rgba, sample);
        let config = ImiEncoderConfig::lossy(73.0)
            .with_effort(0.0)
            .with_column_groups(Some(32))
            .tune_lossy(|t| {
                t.color = imij::LossyColor::YCoCg;
                t.stripe_block_rows = 2;
                t.exact_blocks = Some(0.0);
            });
        check(&info, &config);
    }
}
#[test]
fn bounded_v4_preserves_tiers_exact_small_layouts_and_local_models() {
    let info = ImageInfo::new(137, 99, Channels::Rgba, SampleType::U8);
    for coder in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
        for speed in [
            DecodeSpeed::Fastest,
            DecodeSpeed::Fast,
            DecodeSpeed::Balanced,
            DecodeSpeed::Compact,
        ] {
            let config = ImiEncoderConfig::lossy(80.0)
                .with_decode_speed(speed)
                .with_column_groups(Some(32))
                .with_checksum(true)
                .tune_lossy(|t| t.entropy = coder);
            check(&info, &config);
        }
        for mode in 0..4 {
            let config = ImiEncoderConfig::lossy(80.0)
                .with_effort(7.5)
                .with_column_groups(Some(32))
                .with_local_ans_contexts(mode == 1 && coder == LossyEntropy::RunLevel)
                .with_model_references(mode == 2)
                .with_coding_defaults(mode == 3)
                .tune_lossy(|t| {
                    t.entropy = coder;
                    t.exact_blocks = Some(1000.0);
                    t.stripe_block_rows = 2;
                });
            check(&info, &config);
        }
    }
}

#[test]
fn bounded_v4_preserves_fixed_adaptive_and_sampled_plane_layouts() {
    use imij::{
        ChannelSampling as Sampling, LossyChannelLayout as Layout, LossyTransform as Transform,
    };
    for sample in [
        SampleType::U8,
        SampleType::U16,
        SampleType::U32,
        SampleType::F16,
        SampleType::F32,
    ] {
        let info = ImageInfo::new(41, 29, Channels::Rgba, sample);
        for transform in [
            Transform::Dct8,
            Transform::Dct4,
            Transform::Dct2,
            Transform::Adaptive4,
        ] {
            for sampling in [
                Sampling::Full,
                Sampling::HalfHorizontal,
                Sampling::HalfVertical,
                Sampling::HalfBoth,
            ] {
                for coder in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                    let config = ImiEncoderConfig::lossy(73.0)
                        .with_effort(0.0)
                        .with_column_groups(Some(16))
                        .tune_lossy(|t| {
                            t.entropy = coder;
                            t.exact_blocks = Some(0.0);
                            t.stripe_block_rows = 2;
                            t.channel_layouts = [Layout::new(transform, sampling); 3];
                        });
                    check(&info, &config);
                }
            }
        }
    }
}

#[cfg(feature = "lab")]
#[test]
fn bounded_v4_dc_bands_are_distinct_contiguous_spatial_tiles() {
    let info = ImageInfo::new(137, 259, Channels::Rgba, SampleType::U8);
    let config = ImiEncoderConfig::lossy(73.0)
        .with_decode_speed(DecodeSpeed::Fastest)
        .with_column_groups(Some(32))
        .tune_lossy(|t| {
            t.exact_blocks = Some(0.0);
            t.stripe_block_rows = 2;
        });
    let file = check(&info, &config);
    let trace: serde_json::Value =
        serde_json::from_str(&imij::lab::codestream_trace(&file)).unwrap();
    assert_eq!(trace["decode"], "ok");
    let tiles = trace["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["kind"] == "tile")
        .count();
    assert!(tiles > 1);
    let expected = imij::decode(&file).unwrap().1;
    let rect = imij::planes::Rect::new(3, 5, 17, 19);
    let stride = rect.width() as usize * info.bytes_per_pixel() + 13;
    let mut output = vec![0xa7; stride * rect.height() as usize];
    imij::region::decode_region(&file, rect, &mut output, stride).unwrap();
    for y in 0..rect.height() as usize {
        let at =
            (rect.y() as usize + y) * info.row_bytes() + rect.x() as usize * info.bytes_per_pixel();
        assert_eq!(
            &output[y * stride..][..rect.width() as usize * info.bytes_per_pixel()],
            &expected[at..][..rect.width() as usize * info.bytes_per_pixel()]
        );
    }
}

#[test]
fn bounded_v4_front_tail_and_none_preserve_pixels_rows_regions_and_scratch() {
    use imij::v4::{BoundedLossyConfig as Config, DirectoryPlacement as Placement};
    let info = ImageInfo::new(137, 99, Channels::Rgba, SampleType::U8);
    let stride = info.row_bytes() + 19;
    let source = raster(&info, stride);
    for coder in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
        for checksum in [false, true] {
            let coding = ImiEncoderConfig::lossy(73.0)
                .with_decode_speed(DecodeSpeed::Compact)
                .with_column_groups(Some(32))
                .with_checksum(checksum)
                .tune_lossy(|t| t.entropy = coder);
            let baseline = check(&info, &coding);
            let expected = imij::decode(&baseline).unwrap().1;
            let cfg = Config::new(stride as u64, 8 << 20);
            for cfg in [
                cfg,
                cfg.without_directory(),
                cfg.with_front_directory(16 << 10),
            ] {
                let mut src = Cursor::new(source.clone());
                let (out, index, sums) = imij::v4::encode_lossy_bounded_seekable(
                    &mut src,
                    Cursor::new(vec![7, 11, 13]),
                    Cursor::new(vec![17, 19]),
                    checksum.then(|| Cursor::new(vec![23, 29])),
                    info.clone(),
                    &Metadata::default(),
                    &coding,
                    cfg,
                    &how_far::NoPulse,
                )
                .unwrap();
                // Cursor starts at zero above, so the output owns its whole file.
                let file = out.into_inner();
                assert_eq!(imij::decode(&file).unwrap().1, expected);
                assert_eq!(&index.get_ref()[..2], &[17, 19]);
                if let Some(sums) = sums {
                    assert_eq!(&sums.get_ref()[..2], &[23, 29]);
                }
                let mut rows = Vec::new();
                imij::decode_resources::DecodeSession::new(Default::default())
                    .decode_reader(file.as_slice(), |_, _, row| {
                        rows.extend_from_slice(row);
                        Ok(())
                    })
                    .unwrap();
                assert_eq!(rows, expected);
                let mut region =
                    imij::region::RegionReader::open(imij::planes::SliceReader::new(&file))
                        .unwrap();
                assert_eq!(region.has_unit_table(), cfg.directory != Placement::None);
                let rect = imij::planes::Rect::new(5, 7, 21, 19);
                let stride = rect.width() as usize * info.bytes_per_pixel() + 13;
                let mut out = vec![0xa7; stride * rect.height() as usize];
                region
                    .decode_region_threads(rect, &mut out, stride, 4)
                    .unwrap();
                for y in 0..rect.height() as usize {
                    let at = (rect.y() as usize + y) * info.row_bytes()
                        + rect.x() as usize * info.bytes_per_pixel();
                    assert_eq!(
                        &out[y * stride..][..rect.width() as usize * info.bytes_per_pixel()],
                        &expected[at..][..rect.width() as usize * info.bytes_per_pixel()]
                    );
                    assert!(
                        out[y * stride + rect.width() as usize * info.bytes_per_pixel()
                            ..(y + 1) * stride]
                            .iter()
                            .all(|&b| b == 0xa7)
                    );
                }
            }
        }
    }
}

#[test]
fn bounded_v4_front_admission_and_short_reservation_never_finalize() {
    let info = ImageInfo::new(41, 31, Channels::Rgb, SampleType::U8);
    let source = raster(&info, info.row_bytes());
    let coding = ImiEncoderConfig::lossy(73.0).with_column_groups(Some(16));
    let cfg = imij::v4::BoundedLossyConfig::new(info.row_bytes() as u64, 1 << 20);
    let mut output = Vec::new();
    assert!(
        imij::v4::encode_lossy_bounded(
            &mut Cursor::new(&source),
            &mut output,
            Cursor::new(Vec::new()),
            None,
            info.clone(),
            &Metadata::default(),
            &coding,
            cfg.with_front_directory(4096),
            &how_far::NoPulse
        )
        .is_err()
    );
    assert!(output.is_empty());
    let mut output = Cursor::new(Vec::new());
    let result = imij::v4::encode_lossy_bounded_seekable(
        &mut Cursor::new(&source),
        &mut output,
        Cursor::new(Vec::new()),
        None,
        info,
        &Metadata::default(),
        &coding,
        cfg.with_front_directory(1),
        &how_far::NoPulse,
    );
    assert!(matches!(
        result,
        Err(imij::stream_encode::StreamEncodeError::Codec(
            imij::Error::TooLarge
        ))
    ));
    assert!(imij::decode(output.get_ref()).is_err());
}

#[cfg(feature = "lab")]
#[test]
fn bounded_v4_exercises_inline_models_on_changing_band_statistics() {
    let info = ImageInfo::new(1025, 131, Channels::Rgba, SampleType::U8);
    let stride = info.row_bytes() + 13;
    let mut source = vec![0xa7; stride * info.height as usize];
    for y in 0..info.height as usize {
        for x in 0..info.row_bytes() {
            let raw = (x * 997 + y * 7907 + x * y * 11) & 255;
            source[y * stride + x] = if y < 64 { 63 + raw / 16 } else { raw } as u8;
        }
    }
    for references in [false, true] {
        let mut selected = false;
        for quality in [5.0, 40.0, 95.0] {
            let config = ImiEncoderConfig::lossy(quality)
                .with_effort(0.0)
                .with_column_groups(Some(512))
                .with_local_ans_contexts(!references)
                .with_model_references(references)
                .tune_lossy(|t| {
                    t.stripe_block_rows = 8;
                    t.exact_blocks = Some(0.0);
                });
            let file = check_source(&info, &config, &source, stride);
            let trace: serde_json::Value =
                serde_json::from_str(&imij::lab::codestream_trace(&file)).unwrap();
            assert_eq!(trace["decode"], "ok");
            selected |= trace["nodes"].as_array().unwrap().iter().any(|node| {
                node["label"] == "model ID"
                    && node["value"]
                        .as_str()
                        .and_then(|id| id.parse::<u64>().ok())
                        .is_some_and(|id| id != 0)
            });
        }
        assert!(
            selected,
            "fixture must actually select an inline model, references={references}"
        );
    }
}

#[test]
fn bounded_v4_xyb_power_matches_exact_frozen_pixels() {
    // Freeze exact SHA-256 digests of U8 and U16 sRGB XYB power-on rasters so
    // any 1-code regression in the V4 power reconstruction path fails directly,
    // not just relative to an upgraded V3 stream.
    let cases = [
        (
            SampleType::U8,
            LossyEntropy::RunLevel,
            "05232f6b22d3c96e18902a287059d2f02f271eb305ecd95bd97db02fe0579d35",
        ),
        (
            SampleType::U8,
            LossyEntropy::SplitHuf,
            "05232f6b22d3c96e18902a287059d2f02f271eb305ecd95bd97db02fe0579d35",
        ),
        (
            SampleType::U16,
            LossyEntropy::RunLevel,
            "0de70f0bee08f8ab5ff1e7b69536033d5f9e4e3245c720cd41cbb5ee347d3269",
        ),
        (
            SampleType::U16,
            LossyEntropy::SplitHuf,
            "0de70f0bee08f8ab5ff1e7b69536033d5f9e4e3245c720cd41cbb5ee347d3269",
        ),
    ];
    for (sample, coder, expected_sha) in cases {
        let info = ImageInfo::new(67, 51, Channels::Rgba, sample);
        let stride = info.row_bytes() + 19;
        let source = raster(&info, stride);
        let config = ImiEncoderConfig::lossy(73.0)
            .with_effort(0.0)
            .with_column_groups(Some(32))
            .tune_lossy(|t| {
                t.entropy = coder;
                t.stripe_block_rows = 2;
                t.exact_blocks = Some(0.0);
            });
        let file = check_source(&info, &config, &source, stride);
        assert!(v4_quantizer_power_bit(&file));
        let (native, _, _) = imij::stream_encode::encode_lossy_bounded(
            &mut Cursor::new(&source),
            Cursor::new(Vec::new()),
            Cursor::new(Vec::new()),
            None,
            info.clone(),
            &Metadata::default(),
            &config,
            BoundedLossyConfig::new(stride as u64, LossyUnitIndex::Trailing, 8 << 20),
            &how_far::NoPulse,
        )
        .unwrap();
        let legacy = imij::decode(native.get_ref()).unwrap().1;
        let actual = imij::decode(&file).unwrap().1;
        assert_ne!(
            actual, legacy,
            "fixture must exercise pixels where V4 power differs from V3 legacy"
        );
        // Canonicalize U16 samples to little-endian so the digest is host-endian neutral.
        let digest = if sample == SampleType::U16 {
            let mut le = Vec::with_capacity(actual.len());
            for &px in actual.as_chunks::<2>().0 {
                let v = u16::from_ne_bytes(px);
                le.extend_from_slice(&v.to_le_bytes());
            }
            sha256_hex(&le)
        } else {
            sha256_hex(&actual)
        };
        assert_eq!(
            digest, expected_sha,
            "V4 power pixel digest mismatch for {sample:?} {coder:?}"
        );
    }
}

#[test]
fn bounded_v4_adaptive_partitions_and_compact_match_whole_and_regions() {
    // > 64 Ki pixels so bounded trellis efforts 9.0, 9.5, 10.0 run; odd width
    // (293 -> 37 blocks) exercises trailing partial superblocks too.
    let info = ImageInfo::new(293, 244, Channels::Rgb, SampleType::U8);
    let stride = info.row_bytes() + 19;
    let source = raster(&info, stride);
    let packed: Vec<u8> = (0..info.height as usize)
        .flat_map(|y| source[y * stride..][..info.row_bytes()].iter().copied())
        .collect();

    for &(label, compact, adaptive, coder) in &[
        ("compact", true, false, LossyEntropy::RunLevel),
        ("adaptive-rl", false, true, LossyEntropy::RunLevel),
        ("adaptive-sh", false, true, LossyEntropy::SplitHuf),
    ] {
        for effort in [7.5f32, 9.0, 9.5, 10.0] {
            let mut config = ImiEncoderConfig::lossy(60.0)
                .with_effort(effort)
                .with_column_groups(Some(128))
                .with_checksum(true)
                .tune_lossy(|t| {
                    t.entropy = coder;
                    t.adaptive_partitions = adaptive;
                });
            if compact {
                config = config.with_decode_speed(DecodeSpeed::Compact);
            }
            // Checks V4 vs bit-exact upgraded V3 power oracle + 1/2/4/8T decode parity.
            let v4_file = check_source(&info, &config, &source, stride);

            // V4 bounded must match whole-image V4 `encode_samples` 0-bit exact.
            let whole = config
                .encode_samples(
                    imij::ImageRef::new(&packed, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap();
            let expected_v4 = imij::decode(&v4_file).unwrap().1;
            assert_eq!(
                expected_v4,
                imij::decode(&whole).unwrap().1,
                "{label} e{effort}: V4 bounded vs whole-image V4 pixel mismatch"
            );

            // Region decode parity on the V4 file.
            let rect = imij::planes::Rect::new(11, 13, 47, 39);
            let rstride = rect.width() as usize * info.bytes_per_pixel() + 11;
            let mut out = vec![0x5au8; rstride * rect.height() as usize];
            imij::region::decode_region(&v4_file, rect, &mut out, rstride).unwrap();
            for y in 0..rect.height() as usize {
                let at = (rect.y() as usize + y) * info.row_bytes()
                    + rect.x() as usize * info.bytes_per_pixel();
                assert_eq!(
                    &out[y * rstride..][..rect.width() as usize * info.bytes_per_pixel()],
                    &expected_v4[at..][..rect.width() as usize * info.bytes_per_pixel()],
                    "{label} e{effort}: V4 region decode mismatch at row {y}"
                );
            }
        }
    }
}
