//! Alpha interpretation is separate from the bytes in a stored final slot.
mod common;
use imij::{AlphaMode, Channels, Error, ImageInfo, Limits, Metadata, SampleType};
use std::borrow::Cow;
use zencodec::decode::{Decode, DecodeJob, DecoderConfig};
use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

#[test]
fn every_shared_alpha_mode_preserves_strided_baseline_samples_and_descriptors() {
    for &desc in imij::ImiEncoderConfig::supported_descriptors() {
        if desc.alpha().is_none() {
            continue;
        }
        for mode in [
            AlphaMode::Straight,
            AlphaMode::Premultiplied,
            AlphaMode::Opaque,
            AlphaMode::Undefined,
        ] {
            let desc = desc.with_alpha(Some(mode));
            let row = 13 * desc.bytes_per_pixel();
            let stride = row + desc.bytes_per_pixel() * 2;
            let mut input = vec![0xee; stride * 7];
            let mut tight = Vec::new();
            for y in 0..7 {
                for x in 0..row {
                    input[y * stride + x] = (x * 37 + y * 71) as u8;
                }
                // Opaque is a source promise; construct a valid promise for every storage type.
                if mode == AlphaMode::Opaque {
                    let size = desc.bytes_per_channel();
                    let one = match desc.channel_type() {
                        zenpixels::ChannelType::U8 => vec![255],
                        zenpixels::ChannelType::U16 => u16::MAX.to_ne_bytes().to_vec(),
                        zenpixels::ChannelType::F16 => 0x3c00u16.to_ne_bytes().to_vec(),
                        zenpixels::ChannelType::F32 => 1f32.to_ne_bytes().to_vec(),
                        _ => unreachable!(),
                    };
                    for pixel in
                        input[y * stride..y * stride + row].chunks_exact_mut(desc.bytes_per_pixel())
                    {
                        let end = pixel.len();
                        pixel[end - size..].copy_from_slice(&one);
                    }
                }
                tight.extend_from_slice(&input[y * stride..y * stride + row]);
            }
            for config in [
                imij::ImiEncoderConfig::default().with_generic_effort(0),
                imij::ImiEncoderConfig::default().with_row_groups(Some(imij::RowGroups::new(
                    5,
                    1,
                    16 << 20,
                ))),
            ] {
                let file = config
                    .job()
                    .encoder()
                    .unwrap()
                    .encode(zenpixels::PixelSlice::new(&input, 13, 7, stride, desc).unwrap())
                    .unwrap();
                let info = imij::probe(file.data()).unwrap().0;
                assert_eq!(info.alpha_mode, mode);
                assert_eq!(imij::decode(file.data()).unwrap().1, tight);
                let out = imij::codec::DecodeConfig::default()
                    .job()
                    .decoder(Cow::Borrowed(file.data()), &[])
                    .unwrap()
                    .decode()
                    .unwrap();
                assert_eq!(out.pixels().descriptor().alpha(), Some(mode));
                assert_eq!(out.info().has_alpha, mode.has_alpha());
                assert_eq!(
                    imij::codec::DecodeConfig::default()
                        .job()
                        .probe(file.data())
                        .unwrap()
                        .has_alpha,
                    mode.has_alpha()
                );
                for y in 0..7 {
                    assert_eq!(
                        out.pixels().row(y),
                        &tight[y as usize * row..(y as usize + 1) * row]
                    );
                }
                let remux = imij::compress_metadata(file.data(), 1 << 20).unwrap();
                assert_eq!(imij::probe(&remux).unwrap().0, info);
            }
        }
    }
}

#[test]
fn new_alpha_modes_keep_payloads_and_all_decode_routes() {
    // V3 DATA is identical across alpha interpretations. V4's scoped local
    // digests also bind the image declaration, so its framing changes.
    for channels in [Channels::GrayAlpha, Channels::Rgba] {
        let mut info = ImageInfo::new(19, 11, channels, SampleType::U8);
        let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
            .map(|i| {
                if i % channels.count() == channels.count() - 1 {
                    255
                } else {
                    (i * 37) as u8
                }
            })
            .collect();
        let plain = imij::ImiEncoderConfig::default()
            .encode_legacy_samples(
                imij::ImageRef::new(&pixels, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        let payload = common::parts(&plain)
            .1
            .into_iter()
            .find(|(tag, _)| tag == b"DATA")
            .unwrap();
        for mode in [AlphaMode::Opaque, AlphaMode::Undefined] {
            info.alpha_mode = mode;
            let file = imij::ImiEncoderConfig::default()
                .encode_legacy_samples(
                    imij::ImageRef::new(&pixels, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap();
            assert_eq!(
                common::parts(&file)
                    .1
                    .into_iter()
                    .find(|(tag, _)| tag == b"DATA")
                    .unwrap(),
                payload
            );
            assert_ne!(
                common::feature_mask(&file) & (1 << imij::Feature::AlphaInterpretation.id()),
                0
            );
            let file = imij::add_pixel_checksum(file).unwrap();
            let case = common::matrix::Case {
                name: "alpha-mode".into(),
                file: file.clone(),
                info: info.clone(),
                reference: pixels.clone(),
                frames: None,
                limits: Limits::default(),
            };
            for (route, outcome) in common::matrix::observe_all(&case) {
                assert_eq!(outcome, "ok", "{mode:?} {route}");
            }
            let undeclared = common::with_feature_mask(
                &file,
                Some(common::feature_mask(&file) & !(1 << imij::Feature::AlphaInterpretation.id())),
            );
            assert_eq!(
                imij::probe(&undeclared),
                Err(Error::Corrupt("alpha mode needs its feature"))
            );
            let mut callbacks = 0;
            assert_eq!(
                imij::decode_reader(&undeclared[..], &Limits::default(), |_, _, _| {
                    callbacks += 1;
                    Ok(())
                }),
                Err(Error::Corrupt("alpha mode needs its feature"))
            );
            assert_eq!(callbacks, 0);
            let (mut header, chunks) = common::parts(&file);
            let layout = common::header_layout(&header);
            let mut no_slot = header.clone();
            assert_eq!(no_slot[layout.features.end], channels as u8);
            no_slot[layout.features.end] = Channels::Rgb as u8;
            assert_eq!(
                imij::probe(&common::join(&no_slot, &chunks, false)),
                Err(Error::Corrupt(
                    "alpha interpretation needs a stored alpha slot"
                ))
            );
            let at = layout.dims - 1;
            for value in [4, 255] {
                header[at] = value;
                assert_eq!(
                    imij::probe(&common::join(&header, &chunks, false)),
                    Err(Error::Corrupt("reserved header bits"))
                );
            }
        }
    }
    for mode in [AlphaMode::Opaque, AlphaMode::Undefined] {
        let mut info = ImageInfo::new(1, 1, Channels::Rgb, SampleType::U8);
        info.alpha_mode = mode;
        assert_eq!(
            imij::ImiEncoderConfig::default().encode_legacy_samples(
                imij::ImageRef::new(&[1, 2, 3], 3, &info),
                &Metadata::default()
            ),
            Err(Error::InvalidInput(
                "alpha interpretation needs a stored alpha slot"
            ))
        );
    }
}

#[test]
fn lossy_reconstruction_and_animation_keep_alpha_interpretation() {
    use imij::{FrameConfig, FrameDecoder, FrameDuration, FrameEncoder};
    for mode in [AlphaMode::Opaque, AlphaMode::Undefined] {
        let mut info = ImageInfo::new(19, 11, Channels::Rgba, SampleType::U8);
        info.alpha_mode = mode;
        let raw: Vec<u8> = (0..info.checked_buffer_len().unwrap())
            .map(|i| {
                if mode == AlphaMode::Opaque && i % 4 == 3 {
                    255
                } else {
                    (i * 37) as u8
                }
            })
            .collect();
        let config = imij::ImiEncoderConfig::lossy(80.0).with_effort(0.0);
        let file = config
            .encode_samples(
                imij::ImageRef::new(&raw, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        let (got, expected) = imij::decode(&file).unwrap();
        assert_eq!(got, info);
        for (a, b) in expected
            .as_chunks::<4>()
            .0
            .iter()
            .zip(raw.as_chunks::<4>().0)
        {
            assert_eq!(a[3], b[3]);
        }
        let case = common::matrix::Case {
            name: "lossy-alpha-mode".into(),
            file,
            info: info.clone(),
            reference: expected,
            frames: None,
            limits: Limits::default(),
        };
        for (route, outcome) in common::matrix::observe_all(&case) {
            assert_eq!(outcome, "ok", "{mode:?} {route}");
        }
        let mut animation = FrameEncoder::animation(
            info.clone(),
            Metadata::default(),
            FrameConfig::new(imij::ImiEncoderConfig::default()),
        );
        for _ in 0..3 {
            animation
                .push_frame(&raw, info.row_bytes(), FrameDuration { num: 1, den: 10 })
                .unwrap();
        }
        let file = animation.finish().unwrap();
        let mut decoder = FrameDecoder::new(&file).unwrap();
        let mut out = vec![0; raw.len()];
        for _ in 0..3 {
            assert!(decoder.next_frame().unwrap().is_some());
            decoder.decode_into(&mut out, info.row_bytes(), 2).unwrap();
            assert_eq!(out, raw);
        }
        assert!(decoder.next_frame().unwrap().is_none());
        assert_eq!(imij::probe(&file).unwrap().0.alpha_mode, mode);
    }
}

#[test]
fn mixed_page_alpha_modes_are_declared_and_uniform_pages_refuse_relabeling() {
    use imij::{FrameConfig, FrameDecoder, FrameEncoder};
    let info = ImageInfo::new(7, 3, Channels::Rgba, SampleType::U8);
    let raw = vec![255; info.checked_buffer_len().unwrap()];
    let mut padded = info.clone();
    padded.alpha_mode = AlphaMode::Undefined;
    let config = FrameConfig::new(imij::ImiEncoderConfig::default());
    let mut uniform = FrameEncoder::pages(info.clone(), Metadata::default(), config.clone());
    uniform.push_page(&raw, info.row_bytes(), &info).unwrap();
    assert_eq!(
        uniform.push_page(&raw, padded.row_bytes(), &padded),
        Err(Error::InvalidInput("page pixel format differs"))
    );
    let mut mixed = FrameEncoder::mixed_pages(config);
    for page in [&info, &padded] {
        mixed.push_page(&raw, page.row_bytes(), page).unwrap();
    }
    let file = mixed.finish().unwrap();
    assert_ne!(
        common::feature_mask(&file) & (1 << imij::Feature::AlphaInterpretation.id()),
        0
    );
    let mut frames = FrameDecoder::new(&file).unwrap();
    for page in [&info, &padded] {
        frames.next_frame().unwrap().unwrap();
        assert_eq!(frames.frame_info().unwrap(), *page);
        let mut out = vec![0; raw.len()];
        frames.decode_into(&mut out, page.row_bytes(), 1).unwrap();
        assert_eq!(out, raw);
    }
    assert!(frames.next_frame().unwrap().is_none());
    let bad = common::with_feature_mask(
        &file,
        Some(common::feature_mask(&file) & !(1 << imij::Feature::AlphaInterpretation.id())),
    );
    assert!(matches!(
        FrameDecoder::new(&bad),
        Err(Error::Corrupt(
            "page needs a feature the file does not declare"
        ))
    ));
}
