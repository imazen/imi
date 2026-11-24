#![cfg(feature = "lab")]

#[path = "../src/gaborish_lab.rs"]
mod gaborish_lab;
#[allow(dead_code)]
#[path = "../src/restoration.rs"]
mod restoration;

use gaborish_lab::{HEADER_BYTES, decode, encode};
use imij::{Channels, ImageInfo, ImiEncoderConfig, LossyColor, LossyEntropy, Metadata, SampleType};

fn fixture(w: usize, h: usize, stride: usize, ox: usize, oy: usize) -> Vec<u8> {
    let mut pixels = vec![0xcd; stride * h];
    for y in 0..h {
        for x in 0..w {
            let i = y * stride + x * 3;
            pixels[i] = ((x + ox) * 37 + (y + oy) * 71) as u8;
            pixels[i + 1] = ((x + ox) * 101 + (y + oy) * 23) as u8;
            pixels[i + 2] = if (x + ox + y + oy).is_multiple_of(2) {
                255
            } else {
                0
            };
        }
    }
    pixels
}

fn production(rgb: &[u8], stride: usize, w: usize, h: usize, q: f32, color: LossyColor) -> Vec<u8> {
    let mut cfg = imij::ImiEncoderConfig::lossy(90.0);
    cfg = cfg.with_quality(q);
    cfg = cfg.tune_lossy(|t| {
        t.color = color;
    });
    cfg = cfg.with_threads(6);
    cfg = cfg.tune_lossy(|t| {
        t.stripe_block_rows = 1;
    });
    cfg = cfg.tune_lossy(|t| {
        t.entropy = LossyEntropy::RunLevel;
    });
    cfg = cfg.with_column_groups(Some(0));
    cfg.encode_samples(
        imij::ImageRef::new(
            rgb,
            stride,
            &ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8),
        ),
        &Metadata::default(),
    )
    .unwrap()
}

#[test]
fn off_control_preserves_native_file_and_rgb_exactly() {
    for (w, h) in [(1, 1), (7, 9), (16, 16), (17, 15)] {
        let stride = w * 3 + 5;
        let rgb = fixture(w, h, stride, 0, 0);
        for color in [LossyColor::Xyb, LossyColor::YCoCg] {
            for q in [0.0, 30.0, 80.0, 116.0] {
                let native = production(&rgb, stride, w, h, q, color);
                let wrapper = encode(&rgb, stride, w, h, q, color, false).unwrap();
                assert_eq!(&wrapper[HEADER_BYTES..], native);
                assert_eq!(wrapper.len(), native.len() + HEADER_BYTES);
                let decoded = decode(&wrapper).unwrap();
                assert_eq!(decoded.width, w);
                assert_eq!(decoded.height, h);
                assert_eq!(decoded.stride, w * 3);
                assert_eq!(decoded.pixels, imij::decode(&native).unwrap().1);
            }
        }
    }
}

#[test]
fn enabled_pair_changes_both_coded_coefficients_and_decoded_pixels() {
    let (w, h, stride) = (19, 17, 61);
    let rgb = fixture(w, h, stride, 3, 5);
    for color in [LossyColor::Xyb, LossyColor::YCoCg] {
        let on = encode(&rgb, stride, w, h, 80.0, color, true).unwrap();
        let off = encode(&rgb, stride, w, h, 80.0, color, false).unwrap();
        let native_on = &on[HEADER_BYTES..];
        let native_off = &off[HEADER_BYTES..];
        assert_ne!(
            imij::chroma_lab::unpack(native_on).unwrap(),
            imij::chroma_lab::unpack(native_off).unwrap(),
            "encoder inverse filter must change actual serialized coefficients"
        );
        let reconstructed = decode(&on).unwrap();
        assert_ne!(
            reconstructed.pixels,
            imij::decode(native_on).unwrap().1,
            "decoder filter must affect reconstructed native planes"
        );
        assert_ne!(reconstructed.pixels, decode(&off).unwrap().pixels);
        assert_eq!(reconstructed, decode(&on).unwrap());
        assert_eq!(on, encode(&rgb, stride, w, h, 80.0, color, true).unwrap());
        assert_eq!(
            imij::chroma_lab::unpack_parameters(native_on).unwrap().0,
            color
        );
    }
}

#[test]
fn all_crop_phases_odd_dimensions_stride_and_single_pixel() {
    for oy in 0..8 {
        for ox in 0..8 {
            let (w, h, stride) = (9, 11, 32);
            let source = fixture(w, h, stride, ox, oy);
            let mut compact = Vec::new();
            for row in source.chunks_exact(stride) {
                compact.extend_from_slice(&row[..w * 3]);
            }
            let padded = encode(&source, stride, w, h, 60.0, LossyColor::Xyb, true).unwrap();
            let tight = encode(&compact, w * 3, w, h, 60.0, LossyColor::Xyb, true).unwrap();
            assert_eq!(
                padded, tight,
                "padding must not enter image math at {ox},{oy}"
            );
            assert_eq!(decode(&padded).unwrap().pixels.len(), w * h * 3);
            assert!(
                source
                    .chunks_exact(stride)
                    .all(|row| row[w * 3..].iter().all(|&b| b == 0xcd))
            );
        }
    }
    for (w, h) in [(1, 1), (1, 17), (17, 1)] {
        let rgb = fixture(w, h, w * 3, 0, 0);
        let wrapper = encode(&rgb, w * 3, w, h, 116.0, LossyColor::Xyb, true).unwrap();
        assert_eq!(decode(&wrapper).unwrap().pixels.len(), w * h * 3);
    }
}

#[test]
fn rejects_bad_wrapper_and_input_before_image_allocation() {
    let source = fixture(3, 5, 9, 0, 0);
    let wrapper = encode(&source, 9, 3, 5, 30.0, LossyColor::Xyb, true).unwrap();
    for len in 0..HEADER_BYTES + 8 {
        assert!(decode(&wrapper[..len]).is_err());
    }
    for (at, value) in [(0, 0), (4, 0), (4, 2), (5, 2), (6, 1), (7, 1)] {
        let mut bad = wrapper.clone();
        bad[at] = value;
        assert!(decode(&bad).is_err());
    }
    assert!(decode(&wrapper[..wrapper.len() - 1]).is_err());
    let mut recursive = wrapper[..HEADER_BYTES].to_vec();
    recursive.extend_from_slice(&wrapper);
    assert!(decode(&recursive).is_err());
    for q in [f32::NAN, f32::INFINITY, -1.0, 117.0] {
        assert!(encode(&source, 9, 3, 5, q, LossyColor::Xyb, true).is_err());
    }
    for enabled in [false, true] {
        assert!(encode(&[], 0, 0, 1, 30.0, LossyColor::Xyb, enabled).is_err());
        assert!(encode(&source, 8, 3, 5, 30.0, LossyColor::Xyb, enabled).is_err());
        assert!(encode(&source[..44], 9, 3, 5, 30.0, LossyColor::Xyb, enabled).is_err());
        assert!(
            encode(
                &[],
                usize::MAX,
                usize::MAX,
                1,
                30.0,
                LossyColor::Xyb,
                enabled
            )
            .is_err()
        );
        assert!(encode(&[], 3, 1, usize::MAX, 30.0, LossyColor::Xyb, enabled).is_err());
    }
}

fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 128 {
        out.push(value as u8 | 128);
        value >>= 7;
    }
    out.push(value as u8);
}

#[test]
fn native_geometry_cap_precedes_nested_parse_for_both_modes() {
    let mut head = Vec::new();
    varint(&mut head, 4_194_305);
    varint(&mut head, 8);
    head.extend_from_slice(&[3, 0, 0, 1, 13, 0, 1, 1, 1, 1]);
    for enabled in [0, 1] {
        let mut wrapper = b"ZQGB\x01\x00\x00\x00IMIJ\r\n\x87\n".to_vec();
        wrapper[5] = enabled;
        varint(&mut wrapper, head.len() as u64);
        wrapper.extend_from_slice(&head);
        // No DATA follows: the geometry error must win before native entropy
        // parsing notices the missing payload.
        assert_eq!(
            decode(&wrapper).unwrap_err(),
            imij::Error::InvalidInput("chroma lab geometry/working-set cap")
        );
    }
}

#[test]
fn exported_padded_planes_reproduce_decoder_filter_stage() {
    let (w, h, stride) = (13, 9, 43);
    let rgb = fixture(w, h, stride, 5, 7);
    for color in [LossyColor::Xyb, LossyColor::YCoCg] {
        let wrapper = encode(&rgb, stride, w, h, 50.0, color, true).unwrap();
        let exported = gaborish_lab::pre_gaborish_planes(&wrapper).unwrap();
        assert_eq!((exported.width, exported.height), (w, h));
        assert_eq!(
            (
                exported.padded_width,
                exported.padded_height,
                exported.stride
            ),
            (16, 16, 16)
        );
        assert_eq!(exported.color, color);
        let (_, steps) = imij::chroma_lab::unpack_parameters(&wrapper[HEADER_BYTES..]).unwrap();
        let coefs = imij::chroma_lab::unpack(&wrapper[HEADER_BYTES..]).unwrap();
        for channel in 0..3 {
            let first = imij::chroma_lab::inverse8(
                &coefs[channel * 64..(channel + 1) * 64],
                &steps[channel],
            );
            for (i, expected) in first.iter().enumerate() {
                assert_eq!(
                    exported.planes[channel][i / 8 * 16 + i % 8].to_bits(),
                    expected.to_bits()
                );
            }
        }
        let mut filtered: [Vec<f32>; 3] = core::array::from_fn(|_| vec![0.0; 256]);
        for (src, dst) in exported.planes.iter().zip(&mut filtered) {
            restoration::plane(
                src,
                dst,
                16,
                16,
                16,
                restoration::FloatFilter::Gaborish,
                restoration::Kernel::Scalar,
                false,
            )
            .unwrap();
        }
        let mut expected = Vec::new();
        for y in 0..h {
            for x in 0..w {
                expected.extend_from_slice(&imij::chroma_lab::inverse_color(
                    color,
                    core::array::from_fn(|c| filtered[c][y * 16 + x]),
                ));
            }
        }
        assert_eq!(decode(&wrapper).unwrap().pixels, expected);
        let mut off = wrapper.clone();
        off[5] = 0;
        // Export denotes the prefilter stage independent of whether the
        // outer wrapper asks for postfilter application.
        assert_eq!(exported, gaborish_lab::pre_gaborish_planes(&off).unwrap());
    }
}
