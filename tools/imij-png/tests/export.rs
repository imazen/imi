use imij::{Channels, FrameConfig, FrameDuration, FrameEncoder, ImageInfo, Metadata, SampleType};
use imij_png::ExportConfig;
use std::io::Cursor;

fn encode(info: &ImageInfo, meta: &Metadata, raw: &[u8]) -> Vec<u8> {
    imij::ImiEncoderConfig::default()
        .with_effort(0.0)
        .with_threads(1)
        .encode_samples(imij::ImageRef::new(raw, info.row_bytes(), info), meta)
        .unwrap()
}
fn export(file: &[u8]) -> Vec<u8> {
    ExportConfig::default()
        .job()
        .exporter(file)
        .unwrap()
        .export()
        .unwrap()
}
fn codes(info: &ImageInfo, seed: u8) -> Vec<u8> {
    (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 73 + usize::from(seed)) as u8)
        .collect()
}
fn png_endian(raw: &[u8], sample: SampleType) -> Vec<u8> {
    if sample == SampleType::U16 {
        raw.as_chunks::<2>()
            .0
            .iter()
            .flat_map(|b| u16::from_ne_bytes(*b).to_be_bytes())
            .collect()
    } else {
        raw.to_vec()
    }
}
#[test]
fn native_sample_models_export_without_precision_or_alpha_loss() {
    for sample in [SampleType::U8, SampleType::U16] {
        for channels in [
            Channels::Gray,
            Channels::GrayAlpha,
            Channels::Rgb,
            Channels::Rgba,
        ] {
            let info = ImageInfo::new(19, 7, channels, sample);
            let mut raw = codes(&info, 7);
            if channels == Channels::Rgba {
                raw[..4 * sample.bytes()].fill(0);
                raw[0] = 123;
            }
            let file = encode(&info, &Metadata::default(), &raw);
            let bytes = export(&file);
            let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
            let mut decoded = vec![0; reader.output_buffer_size().unwrap()];
            let output = reader.next_frame(&mut decoded).unwrap();
            assert_eq!(output.width, info.width);
            assert_eq!(output.height, info.height);
            assert_eq!(output.bit_depth as u8, (sample.bytes() * 8) as u8);
            assert_eq!(&decoded[..output.buffer_size()], png_endian(&raw, sample));
        }
    }
}

#[test]
fn animation_exact_delays_loops_and_hidden_samples_survive() {
    for sample in [SampleType::U8, SampleType::U16] {
        let info = ImageInfo::new(11, 5, Channels::Rgba, sample);
        let mut cfg = FrameConfig::new({
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.with_effort(0.0);
            cfg = cfg.with_threads(1);
            cfg
        });
        cfg.loop_count = 3;
        let mut encoder = FrameEncoder::animation(info.clone(), Metadata::default(), cfg);
        let mut first = codes(&info, 13);
        first[..info.bytes_per_pixel()].fill(0);
        first[0] = 87;
        let mut second = first.clone();
        second[0] = 31;
        let sources = [first, second.clone(), second];
        let delays = [(1, 2000), (1001, 30000), (0, 1)];
        for (raw, &(num, den)) in sources.iter().zip(&delays) {
            encoder
                .push_frame(raw, info.row_bytes(), FrameDuration { num, den })
                .unwrap();
        }
        let bytes = export(&encoder.finish().unwrap());
        let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
        assert_eq!(reader.info().animation_control.unwrap().num_plays, 3);
        assert_eq!(reader.info().animation_control.unwrap().num_frames, 3);
        let mut canvas = vec![0; info.checked_buffer_len().unwrap()];
        for (raw, &(num, den)) in sources.iter().zip(&delays) {
            let mut frame = vec![0; reader.output_buffer_size().unwrap()];
            reader.next_frame(&mut frame).unwrap();
            let control = reader.info().frame_control.unwrap();
            assert_eq!(
                u64::from(control.delay_num) * u64::from(den),
                num * u64::from(control.delay_den)
            );
            assert_eq!(control.blend_op, png::BlendOp::Source);
            assert_eq!(control.dispose_op, png::DisposeOp::None);
            let bpp = info.bytes_per_pixel();
            for y in 0..control.height as usize {
                let dst = ((control.y_offset as usize + y) * info.width as usize
                    + control.x_offset as usize)
                    * bpp;
                let src = y * control.width as usize * bpp;
                canvas[dst..dst + control.width as usize * bpp]
                    .copy_from_slice(&frame[src..src + control.width as usize * bpp]);
            }
            assert_eq!(canvas, png_endian(raw, sample));
        }
    }
}

#[test]
fn exact_hdr_and_orientation_are_preserved() {
    let mut info = ImageInfo::new(2, 3, Channels::Rgb, SampleType::U16);
    info.cicp.color_primaries = 9;
    info.cicp.transfer_characteristics = 16;
    let mut master = [0u8; 24];
    for (i, item) in master[..16].as_chunks_mut::<2>().0.iter_mut().enumerate() {
        item.copy_from_slice(&(123u16 + i as u16 * 153).to_be_bytes());
    }
    master[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
    master[23] = 1;
    let mut light = [0; 8];
    light[..4].copy_from_slice(&u32::MAX.to_be_bytes());
    light[7] = 1;
    let meta = Metadata {
        mastering_display: Some(master),
        content_light_level: Some(light),
        orientation: 6,
        xmp: Some(b"<x:xmpmeta/>".to_vec()),
        ..Default::default()
    };
    let bytes = export(&encode(&info, &meta, &codes(&info, 17)));
    let oracle = png::Decoder::new(Cursor::new(&bytes)).read_info().unwrap();
    assert_eq!(
        oracle
            .info()
            .coding_independent_code_points
            .as_ref()
            .unwrap()
            .color_primaries,
        9
    );
    let clli = oracle.info().content_light_level.unwrap();
    assert_eq!(clli.max_content_light_level, u32::MAX);
    assert_eq!(clli.max_frame_average_light_level, 1);
    let native = zenpng::probe(&bytes).unwrap();
    assert_eq!(native.mastering_display_raw, Some(master));
    assert_eq!(native.content_light_level_raw, Some([u32::MAX, 1]));
    assert_eq!(native.xmp, meta.xmp);
    assert_eq!(
        zencodec::exif::Exif::parse(oracle.info().exif_metadata.as_ref().unwrap())
            .unwrap()
            .orientation()
            .unwrap()
            .to_exif(),
        6
    );
}

#[test]
fn unsupported_meanings_and_job_limits_are_explicit() {
    let info = ImageInfo::new(2, 2, Channels::Rgba, SampleType::U8);
    let raw = codes(&info, 17);
    let file = encode(&info, &Metadata::default(), &raw);
    let result = ExportConfig::default()
        .job()
        .with_limits(zencodec::ResourceLimits::none().with_max_pixels(3))
        .exporter(&file)
        .unwrap()
        .export();
    assert!(result.is_err());
    let result = ExportConfig::default()
        .job()
        .with_limits(zencodec::ResourceLimits::none().with_max_memory(1))
        .exporter(&file);
    assert!(result.is_err());
    for mutate in [0, 1, 2, 3, 4] {
        let mut changed = info.clone();
        let mut meta = Metadata::default();
        match mutate {
            0 => changed.alpha_mode = imij::AlphaMode::Premultiplied,
            1 => changed.significant_bits = [7; 4],
            2 => meta.xmp = Some(vec![255]),
            3 => changed.alpha_mode = imij::AlphaMode::Opaque,
            _ => changed.alpha_mode = imij::AlphaMode::Undefined,
        }
        let file = encode(&changed, &meta, &raw);
        assert!(
            ExportConfig::default()
                .job()
                .exporter(&file)
                .unwrap()
                .export()
                .is_err()
        );
    }
    let mut encoder = FrameEncoder::animation(
        info.clone(),
        Metadata::default(),
        FrameConfig::new(imij::ImiEncoderConfig::default()),
    );
    encoder
        .push_frame(
            &raw,
            info.row_bytes(),
            FrameDuration {
                num: u64::MAX,
                den: 1,
            },
        )
        .unwrap();
    let file = encoder.finish().unwrap();
    assert!(
        ExportConfig::default()
            .job()
            .with_limits(zencodec::ResourceLimits::none())
            .exporter(&file)
            .unwrap()
            .export()
            .is_err()
    );
}

#[test]
fn explicit_icc_is_authoritative_in_still_and_animation() {
    let info = ImageInfo::new(3, 2, Channels::Rgba, SampleType::U8);
    let icc = zenpixels_convert::icc_profiles::DISPLAY_P3_V4;
    let meta = Metadata {
        icc: Some(icc.to_vec()),
        ..Default::default()
    };
    let raw = codes(&info, 29);
    let mut encoder = FrameEncoder::animation(
        info.clone(),
        meta.clone(),
        FrameConfig::new(imij::ImiEncoderConfig::default()),
    );
    for _ in 0..2 {
        encoder
            .push_frame(&raw, info.row_bytes(), FrameDuration::from_millis(17))
            .unwrap();
    }
    for file in [encode(&info, &meta, &raw), encoder.finish().unwrap()] {
        let bytes = export(&file);
        let reader = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
        assert_eq!(reader.info().icc_profile.as_deref(), Some(icc));
        assert!(reader.info().coding_independent_code_points.is_none());
    }
}

#[test]
fn lossy_reconstructed_samples_export_exactly() {
    let info = ImageInfo::new(33, 17, Channels::Rgb, SampleType::U8);
    let mut cfg = imij::ImiEncoderConfig::lossy(60.0).with_effort(0.0);
    cfg = cfg.with_threads(1);
    let raw = codes(&info, 23);
    let file = cfg
        .encode_samples(
            imij::ImageRef::new(&raw, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let decoded = imij::decode(&file).unwrap();
    assert_ne!(decoded.1, raw);
    let bytes = export(&file);
    let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let output = reader.next_frame(&mut pixels).unwrap();
    assert_eq!(&pixels[..output.buffer_size()], decoded.1);
}

#[test]
fn per_frame_metadata_and_cancellation_are_not_silently_lost() {
    use imij::{SequenceEncoder, SequenceItem, SequenceKind};
    let info = ImageInfo::new(3, 2, Channels::Rgba, SampleType::U8);
    let raw = codes(&info, 19);
    let mut encoder = SequenceEncoder::new(
        SequenceKind::Animation,
        FrameConfig::new(imij::ImiEncoderConfig::default()),
    );
    for orientation in [1, 6] {
        encoder
            .push(
                SequenceItem::new(&raw, info.row_bytes(), &info).with_metadata(&Metadata {
                    orientation,
                    ..Default::default()
                }),
            )
            .unwrap();
    }
    let file = encoder.finish().unwrap();
    assert!(
        ExportConfig::default()
            .job()
            .exporter(&file)
            .unwrap()
            .export()
            .is_err()
    );
    struct Cancel;
    impl enough::Stop for Cancel {
        fn check(&self) -> Result<(), enough::StopReason> {
            Err(enough::StopReason::Cancelled)
        }
    }
    assert!(matches!(
        ExportConfig::default()
            .job()
            .with_stop(zencodec::StopToken::new(Cancel))
            .exporter(&file),
        Err(imij_png::TranscodeError::Stopped(
            enough::StopReason::Cancelled
        ))
    ));
}

#[test]
fn explicit_diffuse_white_is_not_silently_dropped() {
    let info = ImageInfo::new(2, 2, Channels::Rgb, SampleType::U8);
    let meta = Metadata {
        diffuse_white: Some(zenpixels::DiffuseWhite::BT2408),
        ..Default::default()
    };
    let file = encode(&info, &meta, &codes(&info, 3));
    let error = ExportConfig::default()
        .job()
        .exporter(&file)
        .unwrap()
        .export()
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("explicit diffuse white has no PNG carrier")
    );
}

#[test]
fn native_models_cannot_be_relabelled_as_png_rgb_or_rgba() {
    for channels in [Channels::Cmyk, Channels::Oklab, Channels::OklabAlpha] {
        let sample = if channels == Channels::Cmyk {
            SampleType::U8
        } else {
            SampleType::F32
        };
        let info = ImageInfo::new(3, 2, channels, sample);
        let file = encode(&info, &Metadata::default(), &codes(&info, 9));
        let result = ExportConfig::default()
            .job()
            .exporter(&file)
            .and_then(|job| job.export());
        assert!(matches!(
            result,
            Err(imij_png::TranscodeError::Imij(imij::Error::Unsupported(_)))
        ));
    }
}

#[test]
fn historical_icc_never_reenters_png_active_color() {
    let info = ImageInfo::new(7, 3, Channels::Rgb, SampleType::U8);
    let raw = codes(&info, 19);
    let mut metadata = Metadata::default();
    metadata
        .set_color_origin(Some(&zenpixels::ColorOrigin::from_icc(
            zenpixels_convert::icc_profiles::DISPLAY_P3_V4,
        )))
        .unwrap();
    let file = encode(&info, &metadata, &raw);
    let bytes = export(&file);
    let mut reader = png::Decoder::new(Cursor::new(&bytes)).read_info().unwrap();
    assert!(reader.info().icc_profile.is_none());
    let parsed = zenpng::probe(&bytes).unwrap();
    assert_eq!(parsed.cicp, Some(zenpixels::Cicp::SRGB));
    let mut decoded = vec![0; reader.output_buffer_size().unwrap()];
    let out = reader.next_frame(&mut decoded).unwrap();
    assert_eq!(&decoded[..out.buffer_size()], raw);
}
