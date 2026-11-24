use imij::codec::FORMAT;
use imij::{Error, ImiEncoderConfig, LossyQuality};
use std::borrow::Cow;
use zencodec::decode::{Decode, DecodeJob, DecoderConfig};
use zencodec::encode::{AnimationFrameEncoder, EncodeJob, Encoder, EncoderConfig, Fidelity};
use zencodec::{ImageFormatRegistry, Metadata, MetadataPolicy, ResourceLimits, ThreadingPolicy};
use zenpixels::{AlphaMode, PixelDescriptor, PixelSlice};

#[test]
fn native_strides_and_all_baseline_storage_roundtrip() {
    for &desc in ImiEncoderConfig::supported_descriptors() {
        let row = 13 * desc.bytes_per_pixel();
        let stride = row + 2 * desc.bytes_per_pixel();
        let mut data = vec![0; stride * 7];
        let mut expected = Vec::new();
        for y in 0..7 {
            for x in 0..row {
                data[y * stride + x] = (x * 37 + y * 31) as u8;
            }
            expected.extend_from_slice(&data[y * stride..y * stride + row]);
            data[y * stride + row..(y + 1) * stride].fill(0xee);
        }
        let pixels = PixelSlice::new(&data, 13, 7, stride, desc).unwrap();
        let config = imij::ImiEncoderConfig::default().with_generic_effort(0);
        let output = config
            .clone()
            .job()
            .with_limits(ResourceLimits::none().with_threading(ThreadingPolicy::Sequential))
            .encoder()
            .unwrap()
            .encode(pixels.clone())
            .unwrap();
        assert_eq!(imij::decode(output.data()).unwrap().1, expected, "{desc:?}");
        let registry = ImageFormatRegistry::from_vec(vec![&FORMAT]);
        assert_eq!(
            registry.detect(output.data()),
            Some(ImiEncoderConfig::format())
        );
        assert_eq!(output.format(), ImiEncoderConfig::format());
        // Reusing a config never carries metadata/limits from a previous job.
        let other = config
            .job()
            .encoder()
            .unwrap()
            .encode(pixels.clone())
            .unwrap();
        assert_eq!(imij::decode(other.data()).unwrap().1, expected);
    }
}

#[test]
fn generic_quality_and_fidelity_use_native_calibrations() {
    let desc = PixelDescriptor::RGB8_SRGB;
    let data: Vec<u8> = (0..32 * 19 * 3).map(|i| (i * 29) as u8).collect();
    let pixels = PixelSlice::new(&data, 32, 19, 96, desc).unwrap();
    let info = imij::ImageInfo::new(32, 19, imij::Channels::Rgb, imij::SampleType::U8);
    for (target, quality) in [
        (Fidelity::ssim2(55.0), LossyQuality::Ssim2(55.0)),
        (
            Fidelity::butteraugli(2.0),
            LossyQuality::Butteraugli {
                distance: 2.0,
                norm: imij::ButteraugliNorm::Max,
            },
        ),
        (Fidelity::codec_quality(35.0), LossyQuality::Scale(35.0)),
    ] {
        let config = imij::ImiEncoderConfig::default()
            .with_fidelity(target)
            .with_generic_effort(0);
        assert_eq!(config.resolved_target_fidelity(), Some(target));
        assert_eq!(config.generic_effort(), Some(0));
        let out = config
            .job()
            .with_limits(ResourceLimits::none().with_threading(ThreadingPolicy::Sequential))
            .encoder()
            .unwrap()
            .encode(pixels.clone())
            .unwrap();
        let direct = imij::ImiEncoderConfig::lossy(80.0)
            .with_quality(quality)
            .with_effort(0.0)
            .encode_samples(
                imij::ImageRef::new(&data, 96, &info),
                &imij::Metadata::default(),
            )
            .unwrap();
        assert_eq!(out.data(), direct);
    }
    let config = imij::ImiEncoderConfig::default()
        .with_generic_quality(62.0)
        .with_lossless(false);
    assert_eq!(config.generic_quality(), Some(62.0));
    assert_eq!(
        config.resolved_target_fidelity(),
        Some(Fidelity::ssim2(62.0))
    );
    assert_eq!(
        config
            .clone()
            .with_fidelity(Fidelity::zensim_b(70.0))
            .resolved_target_fidelity(),
        config.resolved_target_fidelity()
    );
    assert_eq!(config.with_lossless(true).is_lossless(), Some(true));
    assert_eq!(
        imij::ImiEncoderConfig::lossy(80.0)
            .with_quality(LossyQuality::JpegQuality(77.0))
            .resolved_target_fidelity(),
        None
    );
    let near = imij::ImiEncoderConfig::default().tune_lossless(|t| t.max_error = [2; 4]);
    assert_eq!(near.is_lossless(), Some(false));
    assert_eq!(near.resolved_target_fidelity(), None);
}

#[test]
#[allow(deprecated)] // Explicit legacy metadata setter compatibility control.
fn metadata_policy_and_color_checks_preserve_meaning() {
    let data = [8u8; 18];
    let pixels = PixelSlice::new(&data, 3, 2, 9, PixelDescriptor::RGB8_SRGB).unwrap();
    let meta = Metadata::none()
        .with_xmp(&b"xmp"[..])
        .with_content_light_level(zencodec::ContentLightLevel::new(50_000, 2000))
        .with_orientation(zencodec::Orientation::Rotate90);
    let out = imij::ImiEncoderConfig::default()
        .job()
        .with_metadata_policy(meta.clone(), MetadataPolicy::PreserveExact)
        .encoder()
        .unwrap()
        .encode(pixels.clone())
        .unwrap();
    let metadata = imij::probe(out.data()).unwrap().1;
    assert_eq!(metadata.xmp.as_deref(), Some(&b"xmp"[..]));
    assert_eq!(metadata.orientation, 6);
    assert_eq!(
        metadata.content_light_level.unwrap(),
        [500_000_000u32.to_be_bytes(), 20_000_000u32.to_be_bytes()]
            .concat()
            .as_slice()
    );
    let wrong = meta.with_cicp(zencodec::Cicp::new(9, 16, 0, true));
    assert!(matches!(
        imij::ImiEncoderConfig::default()
            .job()
            .with_metadata_policy(wrong, MetadataPolicy::PreserveExact)
            .encoder()
            .unwrap()
            .encode(pixels.clone()),
        Err(Error::InvalidInput(_))
    ));
    // The original RGB8 + Undefined declaration has no physical fourth slot.
    // The paired pixel API now rejects it before an encoder can receive it.
    let invalid = pixels.descriptor().with_alpha(Some(AlphaMode::Undefined));
    let error = PixelSlice::new(&data, 3, 2, 9, invalid).unwrap_err();
    assert!(matches!(
        error.error(),
        zenpixels::BufferError::InvalidDescriptor(_)
    ));
    let white = pixels.with_diffuse_white(zenpixels::hdr::DiffuseWhite::BT2408);
    let encoded = imij::ImiEncoderConfig::default()
        .job()
        .encoder()
        .unwrap()
        .encode(white)
        .unwrap();
    assert_eq!(
        imij::probe(encoded.data()).unwrap().1.diffuse_white,
        Some(zenpixels::hdr::DiffuseWhite::BT2408)
    );
}

fn strided_undefined(x: u8, row_padding: u8) -> [u8; 32] {
    let mut bytes = [row_padding; 32];
    for row in bytes.as_chunks_mut::<16>().0 {
        for pixel in row[..12].as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&[8, 17, 29, x]);
        }
    }
    bytes
}

#[test]
fn native_undefined_slot_preserves_differing_padding() {
    let mut info = imij::ImageInfo::new(3, 2, imij::Channels::Rgba, imij::SampleType::U8);
    info.alpha_mode = AlphaMode::Undefined;
    let config = imij::ImiEncoderConfig::default().with_generic_effort(0);
    let mut files = Vec::new();
    for x in [0x00, 0xa5] {
        let bytes = strided_undefined(x, 0xee);
        let file = config
            .encode_samples(
                imij::ImageRef::new(&bytes, 16, &info),
                &imij::Metadata::default(),
            )
            .unwrap();
        let (decoded_info, decoded) = imij::decode(&file).unwrap();
        assert_eq!(decoded_info.alpha_mode, AlphaMode::Undefined);
        assert!(!decoded_info.alpha_mode.has_alpha());
        let tight: Vec<u8> = bytes
            .as_chunks::<16>()
            .0
            .iter()
            .flat_map(|r| r[..12].iter().copied())
            .collect();
        assert_eq!(decoded, tight);
        // Physical row padding must not be serialized; in-pixel X is preserved.
        let different_row_padding = strided_undefined(x, 0x31);
        let same = config
            .encode_samples(
                imij::ImageRef::new(&different_row_padding, 16, &info),
                &imij::Metadata::default(),
            )
            .unwrap();
        assert_eq!(file, same);
        files.push(file);
    }
    // Same RGB and declaration, different losslessly retained X bytes.
    assert_ne!(files[0], files[1]);
}

#[test]
fn shared_undefined_slots_preserve_bytes_and_ignore_row_padding() {
    for desc in [
        PixelDescriptor::RGBX8_SRGB,
        PixelDescriptor::RGBA8_SRGB.with_alpha(Some(AlphaMode::Undefined)),
    ] {
        for config in [
            ImiEncoderConfig::default().with_generic_effort(0),
            ImiEncoderConfig::default().with_row_groups(Some(imij::RowGroups::new(5, 1, 16 << 20))),
        ] {
            let encode = |bytes: &[u8]| {
                let view = PixelSlice::new(bytes, 3, 2, 16, desc).unwrap();
                assert_eq!(view.descriptor().alpha(), Some(AlphaMode::Undefined));
                assert!(!view.descriptor().has_alpha());
                config
                    .clone()
                    .job()
                    .encoder()
                    .unwrap()
                    .encode(view)
                    .unwrap()
            };
            let mut files = Vec::new();
            for x in [0x00, 0xa5] {
                let bytes = strided_undefined(x, 0xee);
                let file = encode(&bytes);
                let (native_info, decoded) = imij::decode(file.data()).unwrap();
                assert_eq!(native_info.alpha_mode, AlphaMode::Undefined);
                assert!(!native_info.alpha_mode.has_alpha());
                let tight: Vec<u8> = bytes
                    .as_chunks::<16>()
                    .0
                    .iter()
                    .flat_map(|row| row[..12].iter().copied())
                    .collect();
                assert_eq!(decoded, tight);
                let shared = imij::codec::DecodeConfig::default()
                    .job()
                    .decoder(Cow::Borrowed(file.data()), &[])
                    .unwrap()
                    .decode()
                    .unwrap();
                assert_eq!(
                    shared.pixels().descriptor().alpha(),
                    Some(AlphaMode::Undefined)
                );
                assert!(!shared.info().has_alpha);
                for y in 0..2 {
                    assert_eq!(
                        shared.pixels().row(y),
                        &tight[y as usize * 12..(y as usize + 1) * 12]
                    );
                }
                // Only the per-pixel X slots survive; inter-row padding is ignored.
                let different_row_padding = strided_undefined(x, 0x31);
                assert_eq!(file.data(), encode(&different_row_padding).data());
                files.push(file);
            }
            // The RGB samples match; opaque stored X bytes distinguish the files.
            assert_ne!(files[0].data(), files[1].data());
        }
    }
}

#[test]
fn limits_and_unavailable_operations_fail_explicitly() {
    let data = [0u8; 24];
    let pixels = PixelSlice::new(&data, 4, 2, 12, PixelDescriptor::RGB8_SRGB).unwrap();
    for limits in [
        ResourceLimits::none().with_max_pixels(7),
        ResourceLimits::none().with_max_width(3),
        ResourceLimits::none().with_max_input_bytes(23),
        ResourceLimits::none().with_max_output(1),
    ] {
        assert_eq!(
            imij::ImiEncoderConfig::default()
                .job()
                .with_limits(limits)
                .encoder()
                .unwrap()
                .encode(pixels.clone())
                .unwrap_err(),
            Error::TooLarge
        );
    }
    assert_eq!(
        imij::ImiEncoderConfig::default()
            .job()
            .with_limits(ResourceLimits::none().with_max_memory(1))
            .encoder()
            .unwrap()
            .encode(pixels.clone())
            .unwrap_err(),
        Error::TooLarge
    );
    let mut encoder = imij::ImiEncoderConfig::default().job().encoder().unwrap();
    assert!(matches!(
        encoder.push_rows(pixels.clone()),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(encoder.finish(), Err(Error::Unsupported(_))));
    let mut animation = imij::ImiEncoderConfig::default()
        .job()
        .animation_frame_encoder()
        .unwrap();
    animation.push_frame(pixels.clone(), 10, None).unwrap();
    let output = animation.finish(None).unwrap();
    let (_, reconstructed) = imij::decode(output.data()).unwrap();
    assert_eq!(reconstructed, data);
    assert!(ImiEncoderConfig::capabilities().animation());
    assert!(!ImiEncoderConfig::capabilities().stop());
    assert!(!ImiEncoderConfig::capabilities().enforces_max_memory());
    let bad = imij::ImiEncoderConfig::default()
        .with_lossless(false)
        .with_generic_quality(f32::NAN);
    assert!(matches!(
        bad.job().encoder().unwrap().encode(pixels.clone()),
        Err(Error::InvalidInput(_))
    ));
    let bad = imij::ImiEncoderConfig::default().with_generic_effort(11);
    assert!(matches!(
        bad.job().encoder().unwrap().encode(pixels.clone()),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
// Also checks the legacy job `with_stop`, deprecated in zencodec 0.2; imij keeps it.
#[allow(deprecated)]
fn cancellation_before_and_after_codec_never_returns_output() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use zencodec::enough::{Stop, StopReason};
    #[derive(Clone)]
    struct After {
        count: Arc<AtomicUsize>,
        limit: usize,
    }
    impl Stop for After {
        fn check(&self) -> Result<(), StopReason> {
            if self.count.fetch_add(1, Ordering::SeqCst) >= self.limit {
                Err(StopReason::Cancelled)
            } else {
                Ok(())
            }
        }
    }
    let data = [3u8; 24];
    let pixels = PixelSlice::new(&data, 4, 2, 12, PixelDescriptor::RGB8_SRGB).unwrap();
    for limit in 0..3 {
        let count = Arc::new(AtomicUsize::new(0));
        let job = imij::ImiEncoderConfig::default()
            .job()
            .with_stop(zencodec::StopToken::new(After {
                count: count.clone(),
                limit,
            }));
        let result = job.encoder().and_then(|enc| enc.encode(pixels.clone()));
        assert!(matches!(result, Err(Error::Cancelled)));
        assert_eq!(count.load(Ordering::SeqCst), limit + 1);
    }
}

#[test]
#[allow(deprecated)] // Explicit legacy metadata setter compatibility control.
fn mastering_metadata_and_premultiplied_alpha_survive() {
    let data = [7u8; 24];
    let desc = PixelDescriptor::RGBA8_SRGB.with_alpha(Some(AlphaMode::Premultiplied));
    let pixels = PixelSlice::new(&data, 3, 2, 12, desc).unwrap();
    let mastering = zencodec::MasteringDisplay::new(
        [[0.64, 0.33], [0.3, 0.6], [0.15, 0.06]],
        [0.3127, 0.329],
        1000.0,
        0.0,
    );
    let meta = Metadata::none().with_mastering_display(mastering);
    let out = imij::ImiEncoderConfig::default()
        .job()
        .with_metadata_policy(meta, MetadataPolicy::PreserveExact)
        .encoder()
        .unwrap()
        .encode(pixels.clone())
        .unwrap();
    let (info, meta) = imij::probe(out.data()).unwrap();
    assert_eq!(info.alpha_mode, imij::AlphaMode::Premultiplied);
    assert_eq!(imij::decode(out.data()).unwrap().1, data);
    let values: [u16; 8] = [32000, 16500, 15000, 30000, 7500, 3000, 15635, 16450];
    let mut expected: Vec<u8> = values.into_iter().flat_map(u16::to_be_bytes).collect();
    expected.extend_from_slice(&10_000_000u32.to_be_bytes());
    expected.extend_from_slice(&0u32.to_be_bytes());
    assert_eq!(meta.mastering_display.unwrap().as_slice(), expected);
    let bad = zencodec::MasteringDisplay::new([[f32::NAN, 0.0]; 3], [0.0; 2], 0.0, 0.0);
    assert!(matches!(
        imij::ImiEncoderConfig::default()
            .job()
            .with_metadata_policy(
                Metadata::none().with_mastering_display(bad),
                MetadataPolicy::PreserveExact
            )
            .encoder()
            .unwrap()
            .encode(pixels),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
#[allow(deprecated)] // Explicit legacy metadata setter compatibility control.
fn diffuse_white_context_and_job_agree_or_fail() {
    let data = [8u8; 18];
    let pixels = PixelSlice::new(&data, 3, 2, 9, PixelDescriptor::RGB8_SRGB).unwrap();
    let white = zenpixels::hdr::DiffuseWhite::new(187.25);
    for (context, metadata) in [
        (None, Some(white)),
        (Some(white), None),
        (Some(white), Some(white)),
    ] {
        let pixels =
            context.map_or_else(|| pixels.clone(), |w| pixels.clone().with_diffuse_white(w));
        let mut meta = Metadata::none();
        meta.diffuse_white = metadata;
        let out = imij::ImiEncoderConfig::default()
            .job()
            .with_metadata_policy(meta, MetadataPolicy::PreserveExact)
            .encoder()
            .unwrap()
            .encode(pixels)
            .unwrap();
        assert_eq!(
            imij::probe(out.data()).unwrap().1.diffuse_white,
            Some(white)
        );
        assert_eq!(imij::decode(out.data()).unwrap().1, data);
    }
    let result = imij::ImiEncoderConfig::default()
        .job()
        .with_metadata_policy(
            Metadata::none().with_diffuse_white(white),
            MetadataPolicy::PreserveExact,
        )
        .encoder()
        .unwrap()
        .encode(pixels.with_diffuse_white(zenpixels::hdr::DiffuseWhite::BT2408));
    assert_eq!(
        result.unwrap_err(),
        Error::InvalidInput("metadata and pixel diffuse white disagree")
    );
}
