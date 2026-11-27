//! Caller-driven, bounded output, with the existing independent decode paths.
use imij::stream_encode::GroupEncodeSession;
use imij::{Channels, Error, ImageInfo, Metadata, SampleType};

fn encode(info: &ImageInfo, pixels: &[u8], chunk: usize) -> Vec<u8> {
    let mut encoder = GroupEncodeSession::new(
        info.clone(),
        &Metadata::default(),
        &imij::ImiEncoderConfig::default().with_row_groups(Some(imij::RowGroups::new(
            7,
            3,
            32 << 20,
        ))),
    )
    .unwrap();
    let mut file = Vec::new();
    while !encoder.is_finished() {
        if !encoder.output().is_empty() {
            let before = encoder.output().to_vec();
            assert_eq!(encoder.next_group(), None);
            assert!(encoder.push_group(&[], 0).is_err());
            assert!(encoder.consume_output(before.len() + 1).is_err());
            assert_eq!(encoder.output(), before);
            encoder.consume_output(0).unwrap();
            let n = chunk.min(before.len());
            file.extend_from_slice(&encoder.output()[..n]);
            encoder.consume_output(n).unwrap();
            assert_eq!(encoder.output(), &before[n..]);
        } else {
            let group = encoder.next_group().unwrap();
            let row = group.width as usize * info.bytes_per_pixel();
            let stride = row + 5;
            let mut buffer = vec![0x5a; stride * group.height as usize];
            for y in 0..group.height as usize {
                let start = (group.y as usize + y) * info.row_bytes()
                    + group.x as usize * info.bytes_per_pixel();
                buffer[y * stride..y * stride + row].copy_from_slice(&pixels[start..start + row]);
            }
            assert!(encoder.push_group(&[], stride).is_err());
            assert_eq!(encoder.next_group(), Some(group));
            encoder.push_group(&buffer, stride).unwrap();
        }
    }
    assert!(encoder.push_group(&[], 0).is_err());
    assert_eq!(encoder.next_group(), None);
    encoder.finish().unwrap();
    file
}

#[test]
fn every_sample_type_preserves_bits_with_output_backpressure() {
    for sample in [
        SampleType::U8,
        SampleType::U16,
        SampleType::F16,
        SampleType::F32,
        SampleType::I8,
        SampleType::I16,
        SampleType::U32,
        SampleType::I32,
        SampleType::U64,
        SampleType::I64,
        SampleType::F64,
    ] {
        for channels in [
            Channels::Gray,
            Channels::GrayAlpha,
            Channels::Rgb,
            Channels::Rgba,
        ] {
            let info = ImageInfo::new(19, 11, channels, sample);
            // All-ones, zero, signed zero, NaN/infinity/subnormal bits and noisy
            // words, including low words of 64-bit samples and alpha.
            let patterns: [u64; 8] = [
                0,
                u64::MAX,
                1,
                1 << 63,
                0x7ff8_1234_7fc0_1234,
                0xfff0_0000_7f80_0000,
                0x8000_0000_8000_0000,
                0x1234_5678_9abc_def0,
            ];
            let mut pixels = Vec::new();
            for i in 0..19 * 11 * channels.count() {
                let value = patterns[i % patterns.len()];
                match sample.bytes() {
                    1 => pixels.push(value as u8),
                    2 => pixels.extend_from_slice(&(value as u16).to_ne_bytes()),
                    4 => pixels.extend_from_slice(&(value as u32).to_ne_bytes()),
                    _ => pixels.extend_from_slice(&value.to_ne_bytes()),
                }
            }
            let file = encode(&info, &pixels, 7);
            assert_eq!(imij::header(&file).unwrap().info, info);
            if sample.tier() == imij::Tier::Extended && !cfg!(feature = "extended") {
                assert!(matches!(imij::decode(&file), Err(Error::Unsupported(_))));
                continue;
            }
            assert_eq!(
                imij::decode(&file).unwrap().1,
                pixels,
                "{sample:?} {channels:?}"
            );
            let mut stream = imij::StreamDecoder::new(&file).unwrap();
            let mut row = vec![0; info.row_bytes()];
            for expected in pixels.chunks_exact(info.row_bytes()) {
                assert!(stream.next_row(&mut row).unwrap());
                assert_eq!(row, expected, "{sample:?} {channels:?}");
            }
            assert!(!stream.next_row(&mut row).unwrap());
            #[cfg(feature = "std")]
            {
                let stride = info.row_bytes() + 9;
                let mut output = vec![0xa5; stride * 11];
                imij::decode_into_threads(&file, &mut output, stride, 4).unwrap();
                for (row, expected) in output
                    .chunks_exact(stride)
                    .zip(pixels.chunks_exact(info.row_bytes()))
                {
                    assert_eq!(&row[..info.row_bytes()], expected);
                    assert!(row[info.row_bytes()..].iter().all(|&v| v == 0xa5));
                }
                let mut output = Vec::new();
                imij::decode_reader(file.as_slice(), &imij::Limits::DEFAULT, |_, _, row| {
                    output.extend_from_slice(row);
                    Ok(())
                })
                .unwrap();
                assert_eq!(output, pixels);
            }
        }
    }
}

#[test]
fn output_chunking_does_not_change_wire_and_finish_requires_drain() {
    let info = ImageInfo::new(8, 5, Channels::Rgb, SampleType::F32);
    let pixels: Vec<u8> = (0..8 * 5 * 3)
        .flat_map(|i| (i as f32 * 0.125 - 7.0).to_ne_bytes())
        .collect();
    let one = encode(&info, &pixels, 1);
    assert_eq!(one, encode(&info, &pixels, 13));
    assert_eq!(one, encode(&info, &pixels, usize::MAX));
    let tiny = ImageInfo::new(1, 1, Channels::Gray, SampleType::U8);
    let mut encoder = GroupEncodeSession::new(
        tiny.clone(),
        &Metadata::default(),
        &imij::ImiEncoderConfig::default().with_row_groups(Some(imij::RowGroups::new(
            1,
            1,
            2 << 20,
        ))),
    )
    .unwrap();
    let header = encoder.output().len();
    encoder.consume_output(header).unwrap();
    encoder.push_group(&[255], 1).unwrap();
    assert!(!encoder.is_finished());
    assert!(matches!(encoder.finish(), Err(Error::InvalidInput(_))));
    assert!(matches!(
        GroupEncodeSession::new(
            tiny,
            &Metadata::default(),
            &imij::ImiEncoderConfig::default().with_row_groups(Some(imij::RowGroups::new(1, 1, 1)))
        ),
        Err(Error::TooLarge)
    ));
}

#[test]
fn invalid_group_can_be_retried_after_symbol_scratch_has_been_reused() {
    let info = ImageInfo::new(19, 11, Channels::Rgb, SampleType::U16);
    let pixels: Vec<u8> = (0..19 * 11 * 3)
        .flat_map(|i| (i as u16 * 79).to_ne_bytes())
        .collect();
    let expected = encode(&info, &pixels, 7);
    let mut encoder = GroupEncodeSession::new(
        info.clone(),
        &Metadata::default(),
        &imij::ImiEncoderConfig::default().with_row_groups(imij::RowGroups::new(7, 3, 32 << 20)),
    )
    .unwrap();
    let mut file = Vec::new();
    let mut retried = false;
    while !encoder.is_finished() {
        let count = encoder.output().len();
        file.extend_from_slice(encoder.output());
        encoder.consume_output(count).unwrap();
        if let Some(group) = encoder.next_group() {
            if group.y > 0 && !retried {
                assert!(encoder.push_group(&[], info.row_bytes()).is_err());
                assert_eq!(encoder.next_group(), Some(group));
                assert!(encoder.output().is_empty());
                retried = true;
            }
            let start =
                group.y as usize * info.row_bytes() + group.x as usize * info.bytes_per_pixel();
            encoder
                .push_group(&pixels[start..], info.row_bytes())
                .unwrap();
        }
    }
    encoder.finish().unwrap();
    assert!(retried);
    assert_eq!(file, expected);
    assert_eq!(imij::decode(&file).unwrap().1, pixels);
}

#[test]
fn bounded_groups_support_near_lossless_within_exact_bounds() {
    for (sample, max_err) in [
        (SampleType::U8, 4u32),
        (SampleType::U16, 4u32),
        (SampleType::U32, 8u32),
        (SampleType::I32, 8u32),
    ] {
        if sample.tier() == imij::Tier::Extended && !cfg!(feature = "extended") {
            continue;
        }
        for channels in [Channels::Gray, Channels::Rgb, Channels::Rgba] {
            let info = ImageInfo::new(41, 19, channels, sample);
            let nch = channels.count();
            let mut pixels = Vec::new();
            for y in 0..19u32 {
                for x in 0..41u32 {
                    for c in 0..nch as u32 {
                        let base = (x * 13 + y * 29 + c * 7) % 251;
                        match sample {
                            SampleType::U8 => pixels.push(base as u8),
                            SampleType::U16 => {
                                pixels.extend_from_slice(&((base * 197) as u16).to_ne_bytes())
                            }
                            SampleType::U32 => {
                                pixels.extend_from_slice(&(base * 100_003).to_ne_bytes())
                            }
                            SampleType::I32 => {
                                let v = (base as i32 - 125) * 10_007;
                                pixels.extend_from_slice(&v.to_ne_bytes());
                            }
                            _ => unreachable!(),
                        }
                    }
                }
            }
            let cfg = imij::ImiEncoderConfig::lossless()
                .tune_lossless(|t| t.max_error = [max_err; 4])
                .with_row_groups(imij::RowGroups::new(16, 8, 32 << 20));
            let mut sess =
                GroupEncodeSession::new(info.clone(), &Metadata::default(), &cfg).unwrap();
            let mut file = Vec::new();
            let stride = info.row_bytes();
            let bpp = info.bytes_per_pixel();
            while !sess.is_finished() {
                if !sess.output().is_empty() {
                    let n = sess.output().len();
                    file.extend_from_slice(sess.output());
                    sess.consume_output(n).unwrap();
                } else {
                    let r = sess.next_group().unwrap();
                    let at = r.y as usize * stride + r.x as usize * bpp;
                    sess.push_group(&pixels[at..], stride).unwrap();
                }
            }
            sess.finish().unwrap();
            let (dec_info, dec) = imij::decode(&file).unwrap();
            assert_eq!(dec_info, info);
            assert_eq!(dec.len(), pixels.len());
            match sample {
                SampleType::U8 => {
                    for (&a, &b) in pixels.iter().zip(&dec) {
                        assert!(a.abs_diff(b) as u32 <= max_err);
                    }
                }
                SampleType::U16 => {
                    for (&a, &b) in pixels.as_chunks::<2>().0.iter().zip(dec.as_chunks::<2>().0) {
                        let va = u16::from_ne_bytes(a);
                        let vb = u16::from_ne_bytes(b);
                        assert!(va.abs_diff(vb) as u32 <= max_err);
                    }
                }
                SampleType::U32 => {
                    for (&a, &b) in pixels.as_chunks::<4>().0.iter().zip(dec.as_chunks::<4>().0) {
                        let va = u32::from_ne_bytes(a);
                        let vb = u32::from_ne_bytes(b);
                        assert!(va.abs_diff(vb) <= max_err);
                    }
                }
                SampleType::I32 => {
                    for (&a, &b) in pixels.as_chunks::<4>().0.iter().zip(dec.as_chunks::<4>().0) {
                        let va = i32::from_ne_bytes(a);
                        let vb = i32::from_ne_bytes(b);
                        assert!(va.abs_diff(vb) <= max_err);
                    }
                }
                _ => unreachable!(),
            }
        }
    }
    // Float samples and quick groups reject near-lossless cleanly.
    let f32_info = ImageInfo::new(8, 8, Channels::Gray, SampleType::F32);
    let near_cfg = imij::ImiEncoderConfig::lossless()
        .tune_lossless(|t| t.max_error = [2; 4])
        .with_row_groups(imij::RowGroups::new(8, 8, 16 << 20));
    assert!(matches!(
        GroupEncodeSession::new(f32_info, &Metadata::default(), &near_cfg),
        Err(Error::InvalidInput(_))
    ));
    let u8_info = ImageInfo::new(8, 8, Channels::Gray, SampleType::U8);
    let quick_near = imij::ImiEncoderConfig::quick()
        .tune_lossless(|t| t.max_error = [2; 4])
        .with_row_groups(imij::RowGroups::new(8, 8, 16 << 20));
    assert!(matches!(
        GroupEncodeSession::new(u8_info, &Metadata::default(), &quick_near),
        Err(Error::Unsupported(_))
    ));
}

#[test]
fn check_working_admits_1024x64_rgb8_in_20mib_and_bounds_worst_case_peak_allocs() {
    // Tightened per-buffer working estimate admits 1024x64 RGB8 under 20 MiB
    // (previously required >100 MiB at 512 B/sample, ~118 MiB with V4 framing).
    let wide_info = ImageInfo::new(1024, 64, Channels::Rgb, SampleType::U8);
    let wide_cfg = imij::ImiEncoderConfig::lossless().with_row_groups(imij::RowGroups::new(
        1024,
        64,
        20 << 20,
    ));
    GroupEncodeSession::new(wide_info, &Metadata::default(), &wide_cfg).unwrap();

    // Measure live peak allocations on worst-case incompressible random groups
    // (including wide 1-row Gray/U32 groups with alternating 0 / 2^30 residuals)
    // across sample depths, channel counts, and near-lossless mode, verifying
    // peak heap stays strictly below the admitted working budget.
    let cases = [
        (Channels::Gray, SampleType::U8, 256u32, 32u32, 0u32, false),
        (Channels::Rgb, SampleType::U8, 256, 32, 0, false),
        (Channels::Rgb, SampleType::U8, 256, 32, 4, false),
        (Channels::Rgba, SampleType::U16, 128, 32, 0, false),
        (Channels::Gray, SampleType::F32, 256, 32, 0, false),
        (Channels::Rgba, SampleType::F32, 128, 16, 0, false),
        (Channels::Gray, SampleType::F64, 128, 32, 0, false),
        (Channels::Gray, SampleType::U32, 262_160, 1, 0, true),
        (Channels::Gray, SampleType::U32, 65_536, 1, 4, true),
    ];
    for (channels, sample, cols, rows, near, wide_u32_spike) in cases {
        let info = ImageInfo::new(cols, rows * 2, channels, sample);
        let bpp = info.bytes_per_pixel();
        let stride = cols as usize * bpp;
        let mut pixels = vec![0u8; stride * rows as usize * 2];
        if wide_u32_spike {
            for (i, chunk) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let v: u32 = if i & 1 == 0 { 0 } else { 1 << 30 };
                *chunk = v.to_ne_bytes();
            }
        } else {
            let mut s = 0x9e37_79b9_7f4a_7c15u64 ^ (cols as u64) ^ ((near as u64) << 8);
            for b in &mut pixels {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                *b = s as u8;
            }
        }
        // Binary-search the exact admitted floor for this geometry and config.
        let make_cfg = |budget: usize| {
            imij::ImiEncoderConfig::lossless()
                .tune_lossless(|t| t.max_error = [near; 4])
                .with_row_groups(imij::RowGroups::new(cols, rows, budget))
        };
        let (mut lo, mut hi) = (0usize, 128 << 20);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if GroupEncodeSession::new(info.clone(), &Metadata::default(), &make_cfg(mid)).is_ok() {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        let admitted = lo;
        assert!(
            GroupEncodeSession::new(info.clone(), &Metadata::default(), &make_cfg(admitted - 1))
                .is_err()
        );
        let cfg = make_cfg(admitted);
        let stats = allocation_counter::measure(|| {
            let mut sess =
                GroupEncodeSession::new(info.clone(), &Metadata::default(), &cfg).unwrap();
            while !sess.is_finished() {
                if !sess.output().is_empty() {
                    let n = sess.output().len();
                    sess.consume_output(n).unwrap();
                } else {
                    let r = sess.next_group().unwrap();
                    let at = r.y as usize * stride + r.x as usize * bpp;
                    sess.push_group(&pixels[at..], stride).unwrap();
                }
            }
            sess.finish().unwrap();
        });
        assert!(
            stats.bytes_max > 0 && stats.bytes_max <= admitted as u64,
            "{channels:?} {sample:?} {cols}x{rows} near={near}: peak {} vs admitted {}",
            stats.bytes_max,
            admitted
        );
    }
}
