//! Historical carriers survive independently of active rendering metadata.
mod common;
use imij::{Channels, Chunk, Error, ImageInfo, Metadata, SampleType};
use std::borrow::Cow;
use zencodec::decode::{AnimationFrameDecoder, Decode, DecodeJob, DecoderConfig};
use zencodec::encode::{AnimationFrameEncoder, EncodeJob, Encoder, EncoderConfig};
use zenpixels::{Cicp, ColorAuthority, ColorOrigin, ColorProvenance, PixelDescriptor, PixelSlice};

fn origins() -> Vec<ColorOrigin> {
    vec![
        ColorOrigin::assumed(),
        ColorOrigin::from_gama_chrm(),
        ColorOrigin::from_icc(&b"historical profile"[..]),
        ColorOrigin::from_icc(&b""[..]),
        ColorOrigin::from_cicp(Cicp::BT2100_PQ),
        ColorOrigin::from_icc_and_cicp(&b"superseded profile"[..], Cicp::DISPLAY_P3),
        ColorOrigin::from_icc_and_cicp(&b"authoritative at source"[..], Cicp::SRGB)
            .with_color_authority(ColorAuthority::Icc),
    ]
}

#[test]
// Also checks the legacy job `with_policy`, deprecated in zencodec 0.2; imij enforces it.
#[allow(deprecated)]
fn historical_carriers_do_not_change_pixels_or_active_color() {
    let info = ImageInfo::new(7, 3, Channels::Rgb, SampleType::U8);
    let pixels: Vec<u8> = (0..63).map(|i| (i * 71) as u8).collect();
    let config = imij::ImiEncoderConfig::default().with_checksum(true);
    let plain = config
        .encode_samples(
            imij::ImageRef::new(&pixels, 21, &info),
            &Metadata::default(),
        )
        .unwrap();
    for origin in origins() {
        let mut metadata = Metadata {
            orientation: 1,
            ..Default::default()
        };
        metadata.set_color_origin(Some(&origin)).unwrap();
        assert_eq!(metadata.color_origin().unwrap().as_ref(), Some(&origin));
        let file = config
            .encode_samples(imij::ImageRef::new(&pixels, 21, &info), &metadata)
            .unwrap();
        let (actual, parsed) = imij::probe(&file).unwrap();
        assert_eq!(actual, info);
        assert_eq!(parsed, metadata);
        assert_eq!(imij::decode(&file).unwrap().1, pixels);
        let data = |file: &[u8]| {
            common::parts(file)
                .1
                .into_iter()
                .find(|c| c.0 == *b"DATA")
                .unwrap()
                .1
        };
        assert_eq!(data(&file), data(&plain));
        assert_eq!(
            imij::probe(&imij::compress_metadata(&file, 1 << 20).unwrap())
                .unwrap()
                .1,
            metadata
        );
        #[cfg(feature = "std")]
        {
            let mut rows = Vec::new();
            let (_, read) =
                imij::decode_reader(file.as_slice(), &imij::Limits::default(), |_, _, row| {
                    rows.extend_from_slice(row);
                    Ok(())
                })
                .unwrap();
            assert_eq!(rows, pixels);
            assert_eq!(read, metadata);
        }
        let out = imij::codec::DecodeConfig::default()
            .job()
            .decoder(Cow::Borrowed(&file), &[])
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!(
            out.info().source_color.color_authority,
            ColorAuthority::Cicp
        );
        assert!(out.pixels().color_context().unwrap().icc.is_none());
        assert_eq!(out.pixels().color_context().unwrap().cicp, Some(Cicp::SRGB));
        assert_eq!(
            out.extras::<imij::codec::DecodedMetadata>()
                .unwrap()
                .metadata
                .color_origin()
                .unwrap(),
            Some(origin)
        );
        let strict = imij::codec::DecodeConfig::default()
            .job()
            .with_policy(zencodec::decode::DecodePolicy::strict())
            .decoder(Cow::Borrowed(&file), &[])
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!(
            strict
                .extras::<imij::codec::DecodedMetadata>()
                .unwrap()
                .metadata
                .color_origin()
                .unwrap(),
            None
        );
    }
}

#[test]
fn typed_reader_validates_record_and_setter_replaces_only_origin() {
    for bytes in [
        vec![],
        vec![1],
        vec![1, 0, 0],
        vec![1, 0, 0, 2],
        vec![1, 0, 0, 2, 1, 13, 0, 2],
        vec![1, 0, 0, 0, 7],
    ] {
        let meta = Metadata {
            chunks: vec![Chunk {
                tag: *b"cORg",
                data: bytes,
            }],
            ..Default::default()
        };
        assert!(matches!(meta.color_origin(), Err(Error::Corrupt(_))));
    }
    for bytes in [
        vec![2],
        vec![1, 4, 0, 0],
        vec![1, 0, 2, 0],
        vec![1, 0, 0, 4],
    ] {
        let meta = Metadata {
            chunks: vec![Chunk {
                tag: *b"cORg",
                data: bytes,
            }],
            ..Default::default()
        };
        assert!(matches!(meta.color_origin(), Err(Error::Unsupported(_))));
    }
    let mut meta = Metadata {
        icc: Some(vec![9]),
        chunks: vec![Chunk {
            tag: *b"tEXt",
            data: vec![42],
        }],
        ..Default::default()
    };
    meta.set_color_origin(Some(&ColorOrigin::assumed()))
        .unwrap();
    meta.chunks.push(meta.chunks[1].clone());
    assert_eq!(
        meta.color_origin(),
        Err(Error::Corrupt("repeated color origin"))
    );
    meta.set_color_origin(Some(&ColorOrigin::from_gama_chrm()))
        .unwrap();
    assert_eq!(meta.chunks.len(), 2);
    assert_eq!(
        meta.color_origin().unwrap().unwrap().provenance,
        ColorProvenance::GamaChrm
    );
    meta.set_color_origin(None).unwrap();
    assert_eq!(
        meta.chunks,
        [Chunk {
            tag: *b"tEXt",
            data: vec![42]
        }]
    );
    assert_eq!(meta.icc, Some(vec![9]));
}

#[test]
// Also checks the legacy job `with_policy`, deprecated in zencodec 0.2; imij enforces it.
#[allow(deprecated)]
fn shared_whole_and_group_jobs_carry_origin_under_metadata_policy() {
    let pixels = [7u8; 7 * 3 * 3];
    let origin = ColorOrigin::from_icc_and_cicp(&b"old ICC"[..], Cicp::DISPLAY_P3);
    for config in [
        imij::ImiEncoderConfig::default(),
        imij::ImiEncoderConfig::default().with_row_groups(Some(imij::RowGroups::new(
            2,
            1,
            16 << 20,
        ))),
    ] {
        for retain in [true, false] {
            let mut policy = zencodec::encode::EncodePolicy::none();
            policy.embed_icc = Some(retain);
            let out = config
                .clone()
                .job()
                .with_color_origin(origin.clone())
                .with_policy(policy)
                .encoder()
                .unwrap()
                .encode(PixelSlice::new(&pixels, 7, 3, 21, PixelDescriptor::RGB8_SRGB).unwrap())
                .unwrap();
            let (info, meta) = imij::probe(out.data()).unwrap();
            assert_eq!(info.cicp, imij::Cicp::SRGB);
            assert!(meta.icc.is_none());
            assert_eq!(meta.color_origin().unwrap(), retain.then(|| origin.clone()));
            assert_eq!(imij::decode(out.data()).unwrap().1, pixels);
        }
    }
}

#[test]
// Also checks the legacy job `with_policy`, deprecated in zencodec 0.2; imij enforces it.
#[allow(deprecated)]
fn sequence_items_keep_their_own_origin_and_shared_animation_filters_it() {
    let info = ImageInfo::new(7, 3, Channels::Rgb, SampleType::U8);
    let pixels = [7u8; 63];
    let origins = origins();
    for kind in [imij::SequenceKind::Pages, imij::SequenceKind::Animation] {
        let mut encoder = imij::SequenceEncoder::new(
            kind,
            imij::FrameConfig::new(imij::ImiEncoderConfig::default()),
        );
        for origin in &origins {
            let mut metadata = Metadata::default();
            metadata.set_color_origin(Some(origin)).unwrap();
            encoder
                .push(imij::SequenceItem::new(&pixels, 21, &info).with_metadata(&metadata))
                .unwrap();
        }
        let file = encoder.finish().unwrap();
        let mut native = imij::FrameDecoder::new(&file).unwrap();
        for origin in &origins {
            native.next_frame().unwrap().unwrap();
            assert_eq!(
                native
                    .frame_metadata()
                    .unwrap()
                    .color_origin()
                    .unwrap()
                    .as_ref(),
                Some(origin)
            );
        }
        if kind == imij::SequenceKind::Animation {
            for retain in [true, false] {
                let job = imij::codec::DecodeConfig::default().job();
                let job = if retain {
                    job
                } else {
                    job.with_policy(zencodec::decode::DecodePolicy::strict())
                };
                let mut decoder = job
                    .animation_frame_decoder(Cow::Borrowed(&file), &[])
                    .unwrap();
                for origin in &origins {
                    let frame = decoder.render_next_frame_owned(None).unwrap().unwrap();
                    let meta = &frame
                        .extras::<imij::codec::DecodedFrameMetadata>()
                        .unwrap()
                        .metadata;
                    assert_eq!(
                        meta.color_origin().unwrap().as_ref(),
                        retain.then_some(origin)
                    );
                }
            }
        }
    }
}

#[test]
fn shared_animation_job_retains_historical_color_without_active_icc() {
    let origin = ColorOrigin::from_icc(&b"historical only"[..]);
    let raw = [19u8; 63];
    let mut encoder = imij::ImiEncoderConfig::default()
        .job()
        .with_color_origin(origin.clone())
        .animation_frame_encoder()
        .unwrap();
    for _ in 0..2 {
        encoder
            .push_frame(
                PixelSlice::new(&raw, 7, 3, 21, PixelDescriptor::RGB8_SRGB).unwrap(),
                20,
                None,
            )
            .unwrap();
    }
    let file = encoder.finish(None).unwrap();
    let (_, metadata) = imij::probe(file.data()).unwrap();
    assert_eq!(metadata.color_origin().unwrap(), Some(origin));
    assert!(metadata.icc.is_none());
    let mut native = imij::FrameDecoder::new(file.data()).unwrap();
    let mut canvas = vec![0; 63];
    for _ in 0..2 {
        native.next_frame().unwrap().unwrap();
        native.decode_into(&mut canvas, 21, 1).unwrap();
        assert_eq!(canvas, raw);
    }
}

#[test]
// Also checks the legacy job `with_policy`, deprecated in zencodec 0.2; imij enforces it.
#[allow(deprecated)]
fn historical_cicp_does_not_override_current_icc() {
    let info = ImageInfo::new(1, 1, Channels::Rgb, SampleType::U8);
    let mut metadata = Metadata {
        icc: Some(vec![7; 128]),
        ..Default::default()
    };
    metadata
        .set_color_origin(Some(&ColorOrigin::from_cicp(Cicp::BT2100_PQ)))
        .unwrap();
    let file = imij::ImiEncoderConfig::default()
        .encode_samples(imij::ImageRef::new(&[1, 2, 3], 3, &info), &metadata)
        .unwrap();
    let out = imij::codec::DecodeConfig::default()
        .job()
        .decoder(Cow::Borrowed(&file), &[])
        .unwrap()
        .decode()
        .unwrap();
    assert_eq!(out.info().source_color.color_authority, ColorAuthority::Icc);
    let pixels = out.pixels();
    let context = pixels.color_context().unwrap();
    assert_eq!(context.icc.as_deref(), Some(&[7; 128][..]));
    assert!(context.cicp.is_none());
    assert!(matches!(
        imij::codec::DecodeConfig::default()
            .job()
            .with_policy(zencodec::decode::DecodePolicy::strict())
            .probe_full(&file),
        Err(imij::codec::DecodeError::Codec(Error::Unsupported(_)))
    ));
}
