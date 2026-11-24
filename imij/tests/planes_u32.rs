//! U32 named planes: full storage lossless, declared-range lossy and seekable ROI.
#![cfg(feature = "extended")]

use imij::planes::*;
use imij::{Channels, Cicp, ImageInfo, LossyColor, Metadata, SampleType};

fn primary() -> Vec<u8> {
    imij::ImiEncoderConfig::default()
        .with_effort(0.0)
        .encode_samples(
            imij::ImageRef::new(
                &[17],
                1,
                &ImageInfo::new(1, 1, Channels::Gray, SampleType::U8),
            ),
            &Metadata::default(),
        )
        .unwrap()
}

fn max_code(bits: u8) -> u32 {
    ((1u64 << bits) - 1) as u32
}

fn values(bits: [u8; 4], width: usize, height: usize, stride: usize) -> Vec<u8> {
    let mut pixels = vec![0xbd; stride * height];
    for y in 0..height {
        for x in 0..width {
            for (c, b) in bits.into_iter().enumerate() {
                let m = max_code(b);
                let v = match (x + y + c) % 8 {
                    0 => 0,
                    1 => m,
                    2 => 1,
                    3 => m - 1,
                    4 => m / 2,
                    5 => 0x0100_0001 & m,
                    6 => 0x8000_0001 & m,
                    _ => (x as u32).wrapping_mul(0x9e37_79b9) & m,
                };
                let start = y * stride + (x * 4 + c) * 4;
                pixels[start..start + 4].copy_from_slice(&v.to_ne_bytes());
            }
        }
    }
    pixels
}

#[test]
fn named_u32_shared_color_and_scalar_roi_preserve_ranges_and_alpha() {
    let (w, h) = (19usize, 11usize);
    let stride = w * 16 + 7;
    for bits in [[17, 24, 32, 1], [32, 31, 25, 32]] {
        let pixels = values(bits, w, h, stride);
        for color in [LossyColor::YCoCg, LossyColor::Xyb] {
            for cicp in [Cicp::SRGB, Cicp::BT2100_PQ, Cicp::BT2100_HLG] {
                let mut builder = PlaneSetBuilder::new(&primary()).unwrap();
                let ids: Vec<_> = bits
                    .into_iter()
                    .enumerate()
                    .map(|(c, b)| {
                        builder
                            .add_plane(
                                PlaneDescriptor::new(format!("capture-{c}"), SampleType::U32)
                                    .with_precision(b)
                                    .with_cicp(cicp)
                                    .with_origin(-5, 9),
                            )
                            .unwrap()
                    })
                    .collect();
                let geometry = LevelDescriptor::new(w as u32, h as u32, 9, 5);
                builder
                    .add_color_level(
                        &ids,
                        geometry.clone(),
                        &pixels,
                        stride,
                        Channels::Rgba,
                        imij::ImiEncoderConfig::default().with_effort(0.0),
                    )
                    .unwrap();
                builder
                    .add_color_level(
                        &ids,
                        geometry,
                        &pixels,
                        stride,
                        Channels::Rgba,
                        imij::ImiEncoderConfig::lossy(75.0).tune_lossy(|t| t.color = color),
                    )
                    .unwrap();
                let file = builder.finish().unwrap();
                let mut reader =
                    PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default())
                        .unwrap();
                let row = w * 16;
                for level in [0, 1] {
                    let mut full = vec![0; row * h];
                    reader
                        .decode_color_region(
                            &ids,
                            level,
                            Rect::new(0, 0, w as u32, h as u32),
                            &mut full,
                            row,
                        )
                        .unwrap();
                    for y in 0..h {
                        for x in 0..w {
                            for (c, b) in bits.into_iter().enumerate() {
                                let at = y * row + (x * 4 + c) * 4;
                                let source = y * stride + (x * 4 + c) * 4;
                                let value =
                                    u32::from_ne_bytes(full[at..at + 4].try_into().unwrap());
                                assert!(value <= max_code(b));
                                if level == 0 || c == 3 {
                                    assert_eq!(&full[at..at + 4], &pixels[source..source + 4]);
                                }
                            }
                        }
                    }
                    // Cross both horizontal and vertical tile boundaries.
                    for (c, &id) in ids.iter().enumerate() {
                        let crop_stride = 11 * 4 + 3;
                        let mut crop = vec![0xa5; crop_stride * 7 + 13];
                        reader
                            .decode_region(
                                id,
                                level,
                                Rect::new(4, 2, 11, 7),
                                &mut crop,
                                crop_stride,
                            )
                            .unwrap();
                        for y in 0..7 {
                            for x in 0..11 {
                                let at = (y + 2) * row + ((x + 4) * 4 + c) * 4;
                                assert_eq!(
                                    &crop[y * crop_stride + x * 4..][..4],
                                    &full[at..at + 4]
                                );
                            }
                            assert!(
                                crop[y * crop_stride + 44..(y + 1) * crop_stride]
                                    .iter()
                                    .all(|&v| v == 0xa5)
                            );
                        }
                        assert!(crop[7 * crop_stride..].iter().all(|&v| v == 0xa5));
                    }
                }
            }
        }
    }
}

#[test]
fn lossy_u32_rejection_keeps_builder_usable() {
    let mut builder = PlaneSetBuilder::new(&primary()).unwrap();
    let id = builder
        .add_plane(PlaneDescriptor::new("depth", SampleType::U32).with_precision(17))
        .unwrap();
    let geometry = LevelDescriptor::new(2, 1, 1, 1);
    // First tile can encode; failure in the second must roll back both records.
    let invalid: Vec<_> = [0u32, 1 << 17]
        .into_iter()
        .flat_map(u32::to_ne_bytes)
        .collect();
    assert!(
        builder
            .add_level(
                id,
                geometry.clone(),
                &invalid,
                8,
                imij::ImiEncoderConfig::lossy(60.0)
            )
            .is_err()
    );
    let valid: Vec<_> = [0u32, (1 << 17) - 1]
        .into_iter()
        .flat_map(u32::to_ne_bytes)
        .collect();
    assert_eq!(
        builder
            .add_level(id, geometry, &valid, 8, imij::ImiEncoderConfig::lossy(60.0))
            .unwrap(),
        0
    );
    let file = builder.finish().unwrap();
    let mut reader =
        PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
    let mut out = [0; 8];
    reader
        .decode_region(id, 0, Rect::new(0, 0, 2, 1), &mut out, 8)
        .unwrap();
    for &code in out.as_chunks::<4>().0 {
        assert!(u32::from_ne_bytes(code) < 1 << 17);
    }
}
