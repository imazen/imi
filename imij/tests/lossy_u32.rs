//! U32 is a color carrier; lossy f32 reconstruction does not promise 32-bit accuracy.
#![cfg(feature = "extended")]

use imij::decode_resources::{DecodePolicy, DecodeSession};
use imij::{Channels, Cicp, ImageInfo, LossyColor, LossyEntropy, Metadata, SampleType};

fn max_code(bits: u8) -> u32 {
    ((1u64 << bits) - 1) as u32
}

fn words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| u32::from_ne_bytes(*b))
        .collect()
}

fn fixture(info: &ImageInfo, stride: usize) -> Vec<u8> {
    let boundary = [
        0,
        1,
        (1 << 24) - 1,
        1 << 24,
        (1 << 24) + 1,
        (1 << 31) - 1,
        1 << 31,
        (1 << 31) + 1,
        u32::MAX - 1,
        u32::MAX,
    ];
    let mut pixels = vec![0xbd; stride * info.height as usize];
    for y in 0..info.height as usize {
        for x in 0..info.width as usize {
            for c in 0..info.channels.count() {
                let v = boundary[(x + 3 * y + c) % boundary.len()]
                    & max_code(info.channel_bits(c).unwrap());
                pixels[y * stride + (x * info.channels.count() + c) * 4..][..4]
                    .copy_from_slice(&v.to_ne_bytes());
            }
        }
    }
    pixels
}

fn assert_paths(file: &[u8], info: &ImageInfo, source: &[u8], source_stride: usize) {
    let (actual_info, decoded) = imij::decode(file).unwrap();
    assert_eq!(&actual_info, info);
    let nc = info.channels.count();
    let values = words(&decoded);
    for (i, value) in values.into_iter().enumerate() {
        assert!(value <= max_code(info.channel_bits(i % nc).unwrap()));
        if info.channels.has_alpha() && i % nc == nc - 1 {
            let pixel = i / nc;
            let y = pixel / info.width as usize;
            let x = pixel % info.width as usize;
            let off = y * source_stride + (x * nc + nc - 1) * 4;
            assert_eq!(
                value,
                u32::from_ne_bytes(source[off..off + 4].try_into().unwrap())
            );
        }
    }
    let stride = info.row_bytes() + 13;
    for threads in [1, 4] {
        let mut output = vec![0xac; stride * info.height as usize];
        #[cfg(feature = "std")]
        imij::decode_into_threads(file, &mut output, stride, threads).unwrap();
        #[cfg(not(feature = "std"))]
        {
            let _ = threads;
            imij::decode_into(file, &mut output, stride).unwrap();
        }
        for (row, reference) in output
            .chunks_exact(stride)
            .zip(decoded.chunks_exact(info.row_bytes()))
        {
            assert_eq!(&row[..info.row_bytes()], reference);
            assert!(row[info.row_bytes()..].iter().all(|&b| b == 0xac));
        }
    }
    let mut stream = imij::StreamDecoder::new(file).unwrap();
    let mut row = vec![0; info.row_bytes()];
    for reference in decoded.chunks_exact(info.row_bytes()) {
        assert!(stream.next_row(&mut row).unwrap());
        assert_eq!(row, reference);
    }
    assert!(!stream.next_row(&mut row).unwrap());
    let mut session = DecodeSession::new(DecodePolicy::default().with_max_working_bytes(32 << 20));
    let mut output = vec![0; decoded.len()];
    session
        .decode_into(file, &mut output, info.row_bytes())
        .unwrap();
    assert_eq!(output, decoded);
    #[cfg(feature = "std")]
    {
        let mut rows = Vec::new();
        let (reader_info, _) = imij::decode_reader(
            std::io::Cursor::new(file),
            &imij::Limits::default(),
            |actual, y, row| {
                assert_eq!(actual, info);
                assert_eq!(y as usize * info.row_bytes(), rows.len());
                rows.extend_from_slice(row);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(reader_info, *info);
        assert_eq!(rows, decoded);
    }
}

#[test]
fn boundaries_unequal_ranges_hdr_cropped_strides_and_all_decode_paths() {
    for (channels, bits) in [
        (Channels::Gray, vec![32]),
        (Channels::GrayAlpha, vec![24, 32]),
        (Channels::Rgb, vec![17, 24, 32]),
        (Channels::Rgba, vec![32, 31, 25, 32]),
        (Channels::Rgba, vec![17, 24, 32, 1]),
    ] {
        let mut info = ImageInfo::new(19, 17, channels, SampleType::U32)
            .with_significant_bits(&bits)
            .unwrap();
        // An offset view retains extra pixels and padding beyond the requested
        // rectangle. Encoding must use only the given odd-sized extent.
        let stride = (19 + 5) * channels.count() * 4 + 7;
        let prefix = stride + 3 * channels.count() * 4;
        let mut backing = vec![0xe9; prefix];
        backing.extend(fixture(&info, stride));
        let pixels = &backing[prefix..];
        for cicp in [Cicp::SRGB, Cicp::BT2100_PQ, Cicp::BT2100_HLG] {
            info.cicp = cicp;
            for color in [LossyColor::YCoCg, LossyColor::Xyb] {
                let mut cfg = imij::ImiEncoderConfig::lossy(100.0).tune_lossy(|t| t.color = color);
                cfg = cfg.with_effort(0.0);
                cfg = cfg.with_column_groups(Some(16));
                cfg = cfg.tune_lossy(|t| {
                    t.stripe_block_rows = 3;
                });
                let file = cfg
                    .encode_samples(
                        imij::ImageRef::new(pixels, stride, &info),
                        &Metadata::default(),
                    )
                    .unwrap();
                cfg = cfg.with_threads(4);
                assert_eq!(
                    file,
                    cfg.encode_samples(
                        imij::ImageRef::new(pixels, stride, &info),
                        &Metadata::default()
                    )
                    .unwrap()
                );
                assert_paths(&file, &info, pixels, stride);
            }
        }
    }
}

#[test]
fn all_declared_depths_normalize_white_without_claiming_code_exactness() {
    for bits in 1..=32 {
        let info = ImageInfo::new(9, 7, Channels::Gray, SampleType::U32)
            .with_significant_bits(&[bits])
            .unwrap();
        let max = max_code(bits);
        for color in [LossyColor::YCoCg, LossyColor::Xyb] {
            let mut cfg = imij::ImiEncoderConfig::lossy(116.0).tune_lossy(|t| t.color = color);
            cfg = cfg.with_effort(0.0);
            let pixels: Vec<_> = (0..63).flat_map(|_| max.to_ne_bytes()).collect();
            let file = cfg
                .encode_samples(
                    imij::ImageRef::new(&pixels, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap();
            let decoded = imij::decode(&file).unwrap().1;
            let tolerance = max / 1000 + 1;
            assert!(
                words(&decoded)
                    .iter()
                    .all(|&v| v <= max && max - v <= tolerance),
                "{bits} {color:?}: {:?}",
                &words(&decoded)[..3]
            );
        }
    }
}

#[test]
fn full_storage_and_entropy_envelopes_preserve_u32_alpha() {
    let info = ImageInfo::new(17, 9, Channels::Rgba, SampleType::U32);
    let pixels = fixture(&info, info.row_bytes());
    for compact in [false, true] {
        for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
            let mut cfg = imij::ImiEncoderConfig::lossy(75.0);
            cfg = cfg.with_effort(0.0);
            cfg = cfg.tune_lossy(|t| {
                t.compact_streams = compact;
            });
            cfg = cfg.tune_lossy(|t| {
                t.entropy = entropy;
            });
            let file = cfg
                .encode_samples(
                    imij::ImageRef::new(&pixels, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap();
            assert_paths(&file, &info, &pixels, info.row_bytes());
            let explicit = info.clone().with_significant_bits(&[32; 4]).unwrap();
            assert_eq!(
                file,
                cfg.encode_samples(
                    imij::ImageRef::new(&pixels, info.row_bytes(), &explicit),
                    &Metadata::default()
                )
                .unwrap()
            );
        }
    }
}

#[test]
fn invalid_ranges_cicp_and_unknown_xyb_transfer_are_rejected() {
    let color_info = ImageInfo::new(1, 1, Channels::Rgb, SampleType::U32)
        .with_significant_bits(&[17, 24, 32])
        .unwrap();
    let overflow: Vec<_> = [1u32 << 17, 0, 0]
        .into_iter()
        .flat_map(u32::to_ne_bytes)
        .collect();
    assert!(
        imij::ImiEncoderConfig::lossy(90.0)
            .encode_samples(
                imij::ImageRef::new(&overflow, 12, &color_info),
                &Metadata::default()
            )
            .is_err()
    );
    let info = ImageInfo::new(1, 1, Channels::Rgba, SampleType::U32)
        .with_significant_bits(&[32, 32, 32, 1])
        .unwrap();
    let pixels: Vec<_> = [0u32, 0, 0, 2]
        .into_iter()
        .flat_map(u32::to_ne_bytes)
        .collect();
    assert!(
        imij::ImiEncoderConfig::lossy(90.0)
            .encode_samples(
                imij::ImageRef::new(&pixels, 16, &info),
                &Metadata::default()
            )
            .is_err()
    );
    let pixels = [0; 12];
    let info = ImageInfo::new(1, 1, Channels::Rgb, SampleType::U32);
    for matrix in [0, 1] {
        let mut bad = info.clone();
        bad.cicp.matrix_coefficients = matrix;
        bad.cicp.full_range = matrix != 0;
        assert!(
            imij::ImiEncoderConfig::lossy(90.0)
                .encode_samples(imij::ImageRef::new(&pixels, 12, &bad), &Metadata::default())
                .is_err()
        );
    }
    let mut unknown = info;
    unknown.cicp.transfer_characteristics = 2;
    assert!(
        imij::ImiEncoderConfig::lossy(90.0)
            .encode_samples(
                imij::ImageRef::new(&pixels, 12, &unknown),
                &Metadata::default()
            )
            .is_err()
    );
    assert!(
        imij::ImiEncoderConfig::lossy(90.0)
            .tune_lossy(|t| t.color = LossyColor::YCoCg)
            .encode_samples(
                imij::ImageRef::new(&pixels, 12, &unknown),
                &Metadata::default()
            )
            .is_ok()
    );
}
