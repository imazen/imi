use how_far::Pulse;
use how_far_along::FnPulse;
use imij::ImiEncoderConfig;
use imij::{DecoderMemoryLimit, Error};
use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
use zencodec::{Metadata, ResourceLimits, ThreadingPolicy};
use zenpixels::{PixelDescriptor, PixelSlice};

#[test]
#[allow(deprecated)] // Explicit legacy metadata setter compatibility control.
fn denial_precedes_analysis_and_metadata_copy() {
    let input = [7; 24];
    let pixels = PixelSlice::new(&input, 4, 2, 12, PixelDescriptor::RGB8_SRGB).unwrap();
    for cap in [0, 1, 1024] {
        let pulse = FnPulse::new("encode", |_| Ok(()));
        let observer = pulse.observer();
        let result = imij::ImiEncoderConfig::default()
            .job()
            .with_metadata_policy(
                Metadata::none().with_exif(vec![0x42; 65536]),
                zencodec::MetadataPolicy::PreserveExact,
            )
            .with_limits(ResourceLimits::none().with_max_memory(cap))
            .with_progress(pulse.share().unwrap())
            .encoder()
            .unwrap()
            .encode(pixels.clone());
        assert_eq!(result.unwrap_err(), Error::TooLarge);
        assert!(observer.snapshot().children.is_empty());
    }
}

#[test]
#[allow(deprecated)] // Explicit legacy metadata setter compatibility control.
fn admitted_baseline_storage_and_strides_keep_identical_bytes() {
    for &desc in ImiEncoderConfig::supported_descriptors() {
        let row = 19 * desc.bytes_per_pixel();
        let stride = row + 2 * desc.bytes_per_pixel();
        let mut input = vec![0xee; stride * 9];
        let mut expected = Vec::new();
        for y in 0..9 {
            for x in 0..row {
                input[y * stride + x] = (x * 37 + y * 51) as u8;
            }
            expected.extend_from_slice(&input[y * stride..y * stride + row]);
        }
        let pixels = PixelSlice::new(&input, 19, 9, stride, desc).unwrap();
        for (effort, lanes, groups) in [(0.0, 1, 0), (7.5, 16, 7)] {
            let config = {
                let mut cfg = imij::ImiEncoderConfig::default();
                cfg = cfg.with_effort(effort);
                cfg = cfg.tune_lossless(|t| {
                    t.lanes = Some(lanes);
                });
                cfg = cfg.tune_lossless(|t| {
                    t.stripe_rows = Some(3);
                });
                cfg = cfg.with_column_groups(Some(groups));
                cfg = cfg.with_checksum(true);
                cfg
            };
            let metadata = Metadata::none()
                .with_exif(vec![1, 2, 3])
                .with_xmp(vec![4, 5, 6]);
            let limits = ResourceLimits::none().with_threading(ThreadingPolicy::Sequential);
            let ordinary = config
                .clone()
                .job()
                .with_limits(limits)
                .with_metadata_policy(metadata.clone(), zencodec::MetadataPolicy::PreserveExact)
                .encoder()
                .unwrap()
                .encode(pixels.clone())
                .unwrap();
            let admitted = config
                .job()
                .with_limits(limits.with_max_memory(256 << 20))
                .with_metadata_policy(metadata, zencodec::MetadataPolicy::PreserveExact)
                .encoder()
                .unwrap()
                .encode(pixels.clone())
                .unwrap();
            assert_eq!(admitted.data(), ordinary.data(), "{desc:?} effort {effort}");
            assert_eq!(imij::decode(admitted.data()).unwrap().1, expected);
        }
    }
}

#[test]
fn near_palette_and_decoder_target_keep_identical_bytes() {
    let input: Vec<u8> = (0..257 * 19)
        .flat_map(|i| {
            let v = if (i / 257 + i % 257 / 8) % 2 == 0 {
                0
            } else {
                255
            };
            [v, v, v, (i % 256) as u8]
        })
        .collect();
    let pixels = PixelSlice::new(&input, 257, 19, 257 * 4, PixelDescriptor::RGBA8_SRGB).unwrap();
    for max_error in [[0; 4], [3, 3, 3, 0]] {
        let native = {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.tune_lossless(|t| {
                t.max_error = max_error;
            });
            cfg = cfg.with_decoder_memory_limit(Some(DecoderMemoryLimit::new(16 << 20)));
            cfg = cfg.with_checksum(true);
            cfg
        };
        let config = native;
        let mut outputs = Vec::new();
        for cap in [None, Some(256 << 20)] {
            let mut limits = ResourceLimits::none().with_threading(ThreadingPolicy::Sequential);
            limits.max_memory_bytes = cap;
            let output = config
                .clone()
                .job()
                .with_limits(limits)
                .encoder()
                .unwrap()
                .encode(pixels.clone())
                .unwrap();
            let decoded = imij::decode(output.data()).unwrap().1;
            for (a, b) in input
                .as_chunks::<4>()
                .0
                .iter()
                .zip(decoded.as_chunks::<4>().0)
            {
                for c in 0..4 {
                    assert!(u32::from(a[c].abs_diff(b[c])) <= max_error[c]);
                }
            }
            outputs.push(output);
        }
        assert_eq!(outputs[0].data(), outputs[1].data());
    }
}

#[test]
fn uncovered_executors_still_refuse_working_caps() {
    let limits = ResourceLimits::none().with_max_memory(256 << 20);
    assert!(matches!(
        imij::ImiEncoderConfig::default()
            .with_lossless(false)
            .job()
            .with_limits(limits)
            .encoder(),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        imij::ImiEncoderConfig::default()
            .job()
            .with_limits(limits)
            .animation_frame_encoder(),
        Err(Error::Unsupported(_))
    ));
    assert!(!ImiEncoderConfig::capabilities().enforces_max_memory());
    #[cfg(feature = "lab")]
    for native in [
        {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.tune_lossless(|t| {
                t.seed_bank = 1;
            });
            cfg
        },
        {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.tune_lossless(|t| {
                t.guide = Some(std::sync::Arc::new(vec![]));
            });
            cfg
        },
    ] {
        assert!(matches!(
            native.job().with_limits(limits).encoder(),
            Err(Error::Unsupported(_))
        ));
    }
}
