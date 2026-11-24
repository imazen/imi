//! Native component identity survives every lossless route without RGB conversion.
mod common;
use imij::{Channels, Error, ImageInfo, ImiEncoderConfig, Limits, Metadata, SampleType};

fn raw(info: &ImageInfo) -> Vec<u8> {
    // Include negative zero, infinities, NaN payloads and nontrivial fourth channels.
    let words = [
        0u64,
        0x8000_0000,
        0x7fc0_1234,
        0xff80_0000,
        0x0123_4567_89ab_cdef,
    ];
    (0..info.width as usize * info.height as usize * info.channels.count())
        .flat_map(|i| words[i % words.len()].to_ne_bytes()[..info.sample.bytes()].to_vec())
        .collect()
}
fn config() -> ImiEncoderConfig {
    let mut cfg = imij::ImiEncoderConfig::default();
    cfg = cfg.with_effort(0.0);
    cfg = cfg.with_threads(1);
    cfg
}

#[test]
fn native_models_preserve_all_storage_bits_and_routes() {
    #[cfg_attr(not(feature = "extended"), allow(unused_mut))]
    let mut samples = vec![
        SampleType::U8,
        SampleType::U16,
        SampleType::F16,
        SampleType::F32,
    ];
    #[cfg(feature = "extended")]
    samples.extend([
        SampleType::U32,
        SampleType::I8,
        SampleType::I16,
        SampleType::I32,
        SampleType::U64,
        SampleType::I64,
        SampleType::F64,
    ]);
    for channels in [Channels::Cmyk, Channels::Oklab, Channels::OklabAlpha] {
        for &sample in &samples {
            let info = ImageInfo::new(19, 7, channels, sample);
            let pixels = raw(&info);
            let stride = info.row_bytes() + sample.bytes() * 3;
            let mut padded = vec![0xee; stride * info.height as usize];
            for (y, row) in pixels.chunks_exact(info.row_bytes()).enumerate() {
                padded[y * stride..y * stride + row.len()].copy_from_slice(row);
            }
            let file = config()
                .encode_samples(
                    imij::ImageRef::new(&padded, stride, &info),
                    &Metadata::default(),
                )
                .unwrap();
            assert_eq!(imij::probe(&file).unwrap().0, info);
            assert_ne!(
                common::feature_mask(&file) & (1 << imij::Feature::PixelModels.id()),
                0
            );
            let file = imij::add_pixel_checksum(file).unwrap();
            let case = common::matrix::Case {
                name: format!("{channels:?}-{sample:?}"),
                file: file.clone(),
                info: info.clone(),
                reference: pixels,
                frames: None,
                limits: Limits::default(),
            };
            for (route, outcome) in common::matrix::observe_all(&case) {
                assert_eq!(outcome, "ok", "{} {route}", case.name);
            }
            let packed = imij::compress_metadata(&file, 1 << 20).unwrap();
            assert_eq!(imij::probe(&packed).unwrap().0, info);
            assert_eq!(imij::decode(&packed).unwrap().1, case.reference);
        }
    }
}

#[test]
fn native_model_admission_precedes_output_and_lossy_transform() {
    for channels in [Channels::Cmyk, Channels::Oklab, Channels::OklabAlpha] {
        let info = ImageInfo::new(9, 5, channels, SampleType::U8);
        let pixels = raw(&info);
        let file = config()
            .encode_samples(
                imij::ImageRef::new(&pixels, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        let missing = common::with_feature_mask(
            &file,
            Some(common::feature_mask(&file) & !(1 << imij::Feature::PixelModels.id())),
        );
        let mut calls = 0;
        assert_eq!(
            imij::decode_reader(&missing[..], &Limits::default(), |_, _, _| {
                calls += 1;
                Ok(())
            }),
            Err(Error::Corrupt("pixel model needs its feature"))
        );
        assert_eq!(calls, 0);
        assert!(matches!(
            imij::ImiEncoderConfig::lossy(80.0).encode_samples(
                imij::ImageRef::new(&pixels, info.row_bytes(), &info),
                &Metadata::default()
            ),
            Err(Error::Unsupported(_))
        ));
    }
}

#[test]
fn cmyk_black_is_color_and_native_models_keep_grouped_plane_components() {
    use imij::planes::*;
    for channels in [Channels::Cmyk, Channels::Oklab, Channels::OklabAlpha] {
        let info = ImageInfo::new(13, 9, channels, SampleType::F32);
        let pixels = raw(&info);
        let primary = config()
            .encode_samples(
                imij::ImageRef::new(&pixels, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        let mut builder = PlaneSetBuilder::new(&primary).unwrap();
        let mut ids = Vec::new();
        for component in 0..channels.count() {
            ids.push(
                builder
                    .add_plane(
                        PlaneDescriptor::new(format!("c{component}"), info.sample)
                            .with_cicp(info.cicp),
                    )
                    .unwrap(),
            );
        }
        builder
            .add_color_level(
                &ids,
                LevelDescriptor::new(13, 9, 5, 4),
                &pixels,
                info.row_bytes(),
                channels,
                config(),
            )
            .unwrap();
        let file = builder.finish().unwrap();
        assert_eq!(imij::decode(&file).unwrap().1, pixels);
        let mut reader =
            PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
        for (component, id) in ids.into_iter().enumerate() {
            let mut output = vec![0xee; 9 * 4 * 7];
            reader
                .decode_region(id, 0, Rect::new(2, 1, 7, 7), &mut output, 9 * 4)
                .unwrap();
            for y in 0..7 {
                for x in 0..7 {
                    let at =
                        ((y + 1) * 13 * channels.count() + (x + 2) * channels.count() + component)
                            * 4;
                    assert_eq!(
                        &output[y * 36 + x * 4..y * 36 + (x + 1) * 4],
                        &pixels[at..at + 4]
                    );
                }
                assert!(output[y * 36 + 28..(y + 1) * 36].iter().all(|&b| b == 0xee));
            }
        }
    }
}

#[test]
fn native_sequence_references_and_mixed_pages_retain_model_identity() {
    use imij::{
        FrameConfig, FrameDecoder, FrameEncoder, ImageReference, SequenceEncoder, SequenceItem,
        SequenceKind,
    };
    // The existing writer offers inter prediction only below 32-bit storage.
    // Keep a real reference witness using F16; F32 storage routes are tested above.
    let cfg = FrameConfig::new(config().with_effort(4.0));
    let mut mixed = FrameEncoder::mixed_pages(cfg.clone());
    let mut originals = Vec::new();
    for channels in [Channels::Cmyk, Channels::Oklab, Channels::OklabAlpha] {
        let info = ImageInfo::new(129, 33, channels, SampleType::F16);
        let mut state = 711u32;
        let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        mixed.push_page(&pixels, info.row_bytes(), &info).unwrap();
        originals.push(pixels.clone());
        let mut changed = pixels.clone();
        changed[11] ^= 1;
        for kind in [SequenceKind::Pages, SequenceKind::Animation] {
            let mut seq = SequenceEncoder::new(kind, cfg.clone());
            seq.push(SequenceItem::new(&pixels, info.row_bytes(), &info))
                .unwrap();
            seq.push(
                SequenceItem::new(&changed, info.row_bytes(), &info).with_reference(
                    ImageReference {
                        index: 0,
                        pixels: &pixels,
                        stride: info.row_bytes(),
                    },
                ),
            )
            .unwrap();
            let file = imij::add_pixel_checksum(seq.finish().unwrap()).unwrap();
            let mut decoder = FrameDecoder::new(&file).unwrap();
            for (index, expected) in [&pixels, &changed].into_iter().enumerate() {
                decoder.next_frame().unwrap().unwrap();
                assert_eq!(decoder.frame_info().unwrap(), info);
                assert_eq!(decoder.reference_index(), (index == 1).then_some(0));
                let mut out = vec![0; expected.len()];
                if index == 0 {
                    decoder.decode_into(&mut out, info.row_bytes(), 2).unwrap();
                } else {
                    decoder
                        .decode_into_with_reference(
                            &pixels,
                            info.row_bytes(),
                            &mut out,
                            info.row_bytes(),
                            4,
                        )
                        .unwrap();
                }
                assert_eq!(out, *expected);
            }
            assert!(decoder.next_frame().unwrap().is_none());
        }
    }
    let file = mixed.finish().unwrap();
    let mut decoder = FrameDecoder::new(&file).unwrap();
    for (channels, original) in [Channels::Cmyk, Channels::Oklab, Channels::OklabAlpha]
        .into_iter()
        .zip(originals)
    {
        let info = ImageInfo::new(129, 33, channels, SampleType::F16);
        decoder.next_frame().unwrap().unwrap();
        assert_eq!(decoder.frame_info().unwrap(), info);
        let mut out = vec![0; info.checked_buffer_len().unwrap()];
        decoder.decode_into(&mut out, info.row_bytes(), 1).unwrap();
        assert_eq!(out, original);
    }
    assert!(decoder.next_frame().unwrap().is_none());
}

#[test]
#[allow(deprecated)] // Explicit legacy metadata setter compatibility control.
fn shared_models_keep_native_descriptors_and_icc_without_rgb_relabelling() {
    use std::borrow::Cow;
    use zencodec::decode::{Decode, DecodeJob, DecoderConfig};
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
    use zenpixels::{ColorPrimaries, PixelDescriptor, PixelSlice};
    for descriptor in [
        PixelDescriptor::OKLABF32,
        PixelDescriptor::OKLABAF32,
        PixelDescriptor::CMYK8.with_primaries(ColorPrimaries::Unknown),
    ] {
        for with_icc in [false, true] {
            let descriptor = if with_icc {
                descriptor.with_primaries(ColorPrimaries::Unknown)
            } else {
                descriptor
            };
            let mut metadata = zencodec::Metadata::none();
            if with_icc {
                metadata.icc_profile = Some(std::sync::Arc::from(&b"opaque source ICC bytes"[..]));
            }
            let bytes: Vec<u8> = (0..11 * 5 * descriptor.bytes_per_pixel())
                .map(|i| (i * 73) as u8)
                .collect();
            let encoded = imij::ImiEncoderConfig::default()
                .job()
                .with_metadata_policy(metadata, zencodec::MetadataPolicy::PreserveExact)
                .encoder()
                .unwrap()
                .encode(
                    PixelSlice::new(&bytes, 11, 5, 11 * descriptor.bytes_per_pixel(), descriptor)
                        .unwrap(),
                )
                .unwrap();
            let output = imij::codec::DecodeConfig::default()
                .job()
                .decoder(Cow::Borrowed(encoded.data()), &[])
                .unwrap()
                .decode()
                .unwrap();
            assert_eq!(output.pixels().descriptor(), descriptor);
            assert_eq!(
                output.info().has_alpha,
                descriptor.alpha().is_some_and(|mode| mode.has_alpha())
            );
            let color = &output.info().source_color;
            assert_eq!(color.cicp.unwrap().transfer_characteristics, 2);
            if descriptor.layout() == zenpixels::ChannelLayout::Cmyk {
                assert_eq!(color.cicp.unwrap().color_primaries, 2);
            }
            if with_icc {
                assert_eq!(color.color_authority, zencodec::ColorAuthority::Icc);
                assert_eq!(
                    color.icc_profile.as_deref(),
                    Some(&b"opaque source ICC bytes"[..])
                );
            }
            for y in 0..5 {
                let row = 11 * descriptor.bytes_per_pixel();
                assert_eq!(
                    output.pixels().row(y as u32),
                    &bytes[y * row..(y + 1) * row]
                );
            }
        }
    }
}

#[test]
fn native_descriptors_refuse_conflicting_rgb_color_labels() {
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
    use zenpixels::{ColorPrimaries, PixelDescriptor, PixelSlice, TransferFunction};
    for descriptor in [
        PixelDescriptor::CMYK8,
        PixelDescriptor::OKLABF32,
        PixelDescriptor::OKLABAF32,
    ] {
        for bad in [
            descriptor.with_primaries(ColorPrimaries::Bt2020),
            descriptor.with_transfer(TransferFunction::Srgb),
        ] {
            let bytes = vec![0; 3 * 2 * bad.bytes_per_pixel()];
            let result = imij::ImiEncoderConfig::default()
                .job()
                .encoder()
                .unwrap()
                .encode(PixelSlice::new(&bytes, 3, 2, 3 * bad.bytes_per_pixel(), bad).unwrap());
            assert!(matches!(
                result,
                Err(Error::InvalidInput(
                    "RGB color labels on native pixel model"
                ))
            ));
        }
    }
}
