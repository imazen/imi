//! Independent SPEC reconstruction, including non-RGB slot order and wide words.
// The independent decoder implements V2/V3; fixtures select that grammar explicitly.
#[test]
fn model_storage_and_referenced_items_match_raw_samples() {
    use imij::{
        Channels, FrameConfig, ImageInfo, ImageReference, Metadata, SampleType, SequenceEncoder,
        SequenceItem, SequenceKind,
    };
    for channels in [Channels::Cmyk, Channels::Oklab, Channels::OklabAlpha] {
        for sample in [
            SampleType::U8,
            SampleType::U16,
            SampleType::F16,
            SampleType::F32,
            SampleType::U32,
            SampleType::I32,
            SampleType::U64,
            SampleType::I64,
            SampleType::F64,
        ] {
            let info = ImageInfo::new(129, 33, channels, sample);
            let mut state = 983u32;
            let raw: Vec<u8> = (0..info.checked_buffer_len().unwrap())
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    state as u8
                })
                .collect();
            let config = {
                let mut cfg = imij::ImiEncoderConfig::default();
                cfg = cfg.with_effort(4.0);
                cfg = cfg.with_threads(1);
                cfg
            };
            let file = config
                .encode_legacy_samples(
                    imij::ImageRef::new(&raw, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap();
            let decoded = imij_oracle::decode(&file).unwrap();
            assert_eq!(decoded.channels as usize, channels.count());
            assert_eq!(decoded.frames.as_slice(), std::slice::from_ref(&raw));
            // Inter prediction uses one coded word; wide HI/LO storage stays intra.
            if sample.bytes() <= 4 {
                let mut seq = SequenceEncoder::new(SequenceKind::Pages, FrameConfig::new(config));
                seq.push(SequenceItem::new(&raw, info.row_bytes(), &info))
                    .unwrap();
                let mut changed = raw.clone();
                changed[13] ^= 17;
                seq.push(
                    SequenceItem::new(&changed, info.row_bytes(), &info).with_reference(
                        ImageReference {
                            index: 0,
                            pixels: &raw,
                            stride: info.row_bytes(),
                        },
                    ),
                )
                .unwrap();
                let file = seq.finish_legacy().unwrap();
                let mut admission = imij::FrameDecoder::new(&file).unwrap();
                admission.next_frame().unwrap().unwrap();
                admission.next_frame().unwrap().unwrap();
                assert_eq!(admission.reference_index(), Some(0));
                assert_eq!(imij_oracle::decode(&file).unwrap().frames, [raw, changed]);
            }
        }
    }
}

#[test]
fn shifted_f32_references_keep_the_raw_float_map() {
    use imij::{
        Channels, Feature, FrameConfig, FrameDecoder, ImageInfo, ImageReference, SampleType,
        SequenceEncoder, SequenceItem, SequenceKind,
    };
    for channels in [Channels::Gray, Channels::Rgba, Channels::OklabAlpha] {
        let info = ImageInfo::new(129, 33, channels, SampleType::F32);
        let mut state = 983u32;
        let raw: Vec<u8> = (0..info.checked_buffer_len().unwrap() / 4)
            .flat_map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state & 0xffff_ff00).to_ne_bytes()
            })
            .collect();
        let mut changed = raw.clone();
        // Change a retained mantissa bit while keeping eight zero low raw bits.
        let bits = u32::from_ne_bytes(changed[12..16].try_into().unwrap()) ^ 0x100;
        changed[12..16].copy_from_slice(&bits.to_ne_bytes());
        let config = FrameConfig::new({
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.with_effort(4.0);
            cfg = cfg.with_threads(1);
            cfg = cfg.tune_lossless(|t| {
                t.palette = false;
            });
            cfg = cfg.with_column_groups(Some(32));
            cfg = cfg.tune_lossless(|t| {
                t.stripe_rows = Some(4);
            });
            cfg
        });
        let mut encoder = SequenceEncoder::new(SequenceKind::Pages, config);
        encoder
            .push(SequenceItem::new(&raw, info.row_bytes(), &info))
            .unwrap();
        encoder
            .push(
                SequenceItem::new(&changed, info.row_bytes(), &info).with_reference(
                    ImageReference {
                        index: 0,
                        pixels: &raw,
                        stride: info.row_bytes(),
                    },
                ),
            )
            .unwrap();
        let file = imij::add_pixel_checksum(encoder.finish_legacy().unwrap()).unwrap();
        assert!(
            imij::header(&file)
                .unwrap()
                .features
                .contains(Feature::FloatShift)
        );
        for threads in [1, 2, 4, 8] {
            let mut decoder = FrameDecoder::new(&file).unwrap();
            let mut out = vec![0; raw.len()];
            decoder.next_frame().unwrap().unwrap();
            decoder
                .decode_into(&mut out, info.row_bytes(), threads)
                .unwrap();
            assert_eq!(out, raw);
            decoder.next_frame().unwrap().unwrap();
            assert_eq!(decoder.reference_index(), Some(0));
            decoder
                .decode_into_with_reference(
                    &raw,
                    info.row_bytes(),
                    &mut out,
                    info.row_bytes(),
                    threads,
                )
                .unwrap();
            assert_eq!(out, changed);
        }
        let expected: Vec<Vec<u8>> = [raw, changed]
            .into_iter()
            .map(|bytes| {
                bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|v| u32::from_ne_bytes(*v).to_le_bytes())
                    .collect()
            })
            .collect();
        assert_eq!(imij_oracle::decode(&file).unwrap().frames, expected);
    }
}

#[test]
fn historical_origin_is_ignorable_without_changing_reconstruction() {
    let info = imij::ImageInfo::new(7, 3, imij::Channels::Rgb, imij::SampleType::U8);
    let raw: Vec<u8> = (0..63).map(|i| (i * 71) as u8).collect();
    // Version 1: CICP-origin, CICP authority, both carriers, P3 CICP,
    // followed by opaque superseded ICC bytes. This is not active iCCP.
    let metadata = imij::Metadata {
        chunks: vec![imij::Chunk {
            tag: *b"cORg",
            data: vec![1, 2, 1, 3, 12, 13, 0, 1, 9, 8, 7],
        }],
        ..Default::default()
    };
    let file = imij::ImiEncoderConfig::default()
        .with_checksum(true)
        .encode_legacy_samples(imij::ImageRef::new(&raw, 21, &info), &metadata)
        .unwrap();
    assert_eq!(imij_oracle::decode(&file).unwrap().frames, [raw]);
    let origin = imij::probe(&file)
        .unwrap()
        .1
        .color_origin()
        .unwrap()
        .unwrap();
    assert_eq!(origin.icc.as_deref(), Some(&[9, 8, 7][..]));
    assert_eq!(origin.cicp.unwrap().color_primaries, 12);
}
