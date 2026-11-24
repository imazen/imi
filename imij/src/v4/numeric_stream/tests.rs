use super::*;
use crate::decode_resources::DecodePolicy;
use crate::v4::{Directory, DirectoryWriter};
use crate::{Channels, Coder, ImiEncoderConfig, QuickBitSmarts, RowGroups, SampleType};
use alloc::vec;

fn spatial(order: TileOrder, integrity: ObjectIntegrity) -> SpatialConfig {
    SpatialConfig {
        tile_columns: 3,
        tile_rows: 3,
        order,
        integrity,
        limits: TileLimits {
            max_object_bytes: 1 << 20,
            max_tile_bytes: 16 << 20,
        },
    }
}

fn drain(encoder: &mut NumericGroupEncoder) {
    encoder.consume_output(encoder.output().len()).unwrap();
}

fn roundtrip(
    sample: SampleType,
    channels: Channels,
    coder: Coder,
    order: TileOrder,
    integrity: ObjectIntegrity,
) {
    let info = ImageInfo::new(41, 31, channels, sample);
    let stride = info.row_bytes() + 13;
    let pixels: Vec<u8> = (0..stride * info.height as usize)
        .map(|i| (i * 113 + i / 11) as u8)
        .collect();
    let cfg = ImiEncoderConfig::lossless()
        .with_coder(coder)
        .with_row_groups(RowGroups::new(9, 7, 8 << 20));
    let sp = spatial(order, integrity);
    let (mut encoder, setup) = NumericGroupEncoder::new(info.clone(), &cfg, sp).unwrap();
    let session = DecodeSession::new(DecodePolicy::default());
    let (mut decoder, consumed) = NumericGroupDecoder::new(
        info.clone(),
        crate::supported_features(),
        &setup,
        sp.limits,
        &session,
    )
    .unwrap();
    assert_eq!(consumed, setup.len());
    assert_eq!(decoder.info(), &info);
    assert_eq!(encoder.layout(), decoder.layout());
    let mut index = DirectoryWriter::new(encoder.layout(), integrity);
    let mut entries = Vec::new();
    let mut encode_entry = |entry: LocatedEntry| {
        let mut buffer = [0; 40];
        let n = index.push(entry.entry, &mut buffer).unwrap();
        entries.extend_from_slice(&buffer[..n]);
    };
    let mut records = Vec::new();
    let output_stride = info.row_bytes() + 17;
    let mut output = vec![0xa5; output_stride * info.height as usize];
    let mut ids = Vec::new();
    for _ in 0..encoder.layout().tile_count() {
        let begin = encoder.begin_tile().unwrap();
        encode_entry(begin);
        assert!(encoder.next_group().is_none());
        match decoder.read(encoder.output()).unwrap() {
            NumericEvent::Begin { entry, .. } => assert_eq!(entry, begin),
            _ => panic!("expected begin"),
        }
        records.extend_from_slice(encoder.output());
        drain(&mut encoder);
        while let Some(group) = encoder.next_group() {
            let at = group.y as usize * stride + group.x as usize * info.bytes_per_pixel();
            let entry = encoder.push_group(&pixels[at..], stride).unwrap();
            encode_entry(entry);
            let event = decoder.read(encoder.output()).unwrap();
            let NumericEvent::Group(work) = event else {
                panic!("expected numeric work")
            };
            assert_eq!(work.entry, entry);
            assert_eq!(work.integrity_verified, integrity != ObjectIntegrity::None);
            let rect = work.rect().unwrap();
            let dst =
                rect.y() as usize * output_stride + rect.x() as usize * info.bytes_per_pixel();
            work.decode_into(&mut output[dst..], output_stride, &session)
                .unwrap();
            // First pixels are ready here, before next input, close, directory or EOF.
            ids.push(entry.entry.id);
            records.extend_from_slice(encoder.output());
            drain(&mut encoder);
        }
        let (end, tile) = encoder.end_tile().unwrap();
        encode_entry(end);
        match decoder.read(encoder.output()).unwrap() {
            NumericEvent::End {
                entry,
                tile: observed,
                ..
            } => {
                assert_eq!(entry, end);
                assert_eq!(observed, tile);
            }
            _ => panic!("expected end"),
        }
        records.extend_from_slice(encoder.output());
        drain(&mut encoder);
    }
    encoder.finish().unwrap();
    decoder.finish_structure().unwrap();
    assert_ne!(ids, (0..25).collect::<Vec<_>>());
    ids.sort_unstable();
    assert_eq!(ids, (0..25).collect::<Vec<_>>());
    let (count, data_len) = index.finish().unwrap();
    assert_eq!(data_len as usize, records.len());
    let mut body = Vec::new();
    crate::put_varint(&mut body, count);
    body.extend_from_slice(&entries);
    body.extend_from_slice(&data_len.to_le_bytes());
    let directory =
        Directory::read(&body, encoder.layout(), integrity, data_len, &session).unwrap();
    for ordinal in 0..encoder.layout().tile_count() {
        let id = encoder.layout().tile_at(ordinal).unwrap().id;
        let tile = directory.tile(id, &session).unwrap();
        assert!(tile.offset + tile.len <= data_len);
    }
    for y in 0..info.height as usize {
        assert_eq!(
            &output[y * output_stride..][..info.row_bytes()],
            &pixels[y * stride..][..info.row_bytes()],
            "{sample:?}/{coder:?}/{order:?}/{integrity:?}"
        );
        assert!(
            output[y * output_stride + info.row_bytes()..(y + 1) * output_stride]
                .iter()
                .all(|&b| b == 0xa5)
        );
    }
}

#[test]
fn one_pass_native_lossless_and_quick_payloads_preserve_all_sample_bits() {
    for sample in [
        SampleType::U8,
        SampleType::U16,
        SampleType::U32,
        SampleType::U64,
        SampleType::I8,
        SampleType::I16,
        SampleType::I32,
        SampleType::I64,
        SampleType::F16,
        SampleType::F32,
        SampleType::F64,
    ] {
        for channels in [
            Channels::Gray,
            Channels::GrayAlpha,
            Channels::Rgb,
            Channels::Rgba,
            Channels::Cmyk,
            Channels::Oklab,
            Channels::OklabAlpha,
        ] {
            for coder in [Coder::Lossless, Coder::Quick(QuickBitSmarts::Off)] {
                roundtrip(
                    sample,
                    channels,
                    coder,
                    TileOrder::Morton,
                    ObjectIntegrity::ObjectsAndTiles,
                );
            }
        }
    }
}

#[test]
fn all_spatial_integrity_and_quick_preprocessing_choices_roundtrip() {
    for order in [TileOrder::Raster, TileOrder::Morton] {
        for integrity in [
            ObjectIntegrity::None,
            ObjectIntegrity::Objects,
            ObjectIntegrity::ObjectsAndTiles,
        ] {
            for bits in [
                QuickBitSmarts::Off,
                QuickBitSmarts::Packed,
                QuickBitSmarts::Packets,
            ] {
                roundtrip(
                    SampleType::U16,
                    Channels::Rgba,
                    Coder::Quick(bits),
                    order,
                    integrity,
                );
            }
        }
    }
}

#[test]
fn setup_binding_limits_and_incomplete_tiles_do_not_claim_success() {
    let info = ImageInfo::new(41, 31, Channels::Rgba, SampleType::U8);
    let cfg = ImiEncoderConfig::lossless().with_row_groups(RowGroups::new(9, 7, 8 << 20));
    let sp = spatial(TileOrder::Morton, ObjectIntegrity::ObjectsAndTiles);
    let (mut encoder, setup) = NumericGroupEncoder::new(info.clone(), &cfg, sp).unwrap();
    assert!(encoder.push_group(&[], 0).is_err());
    encoder.begin_tile().unwrap();
    assert!(encoder.end_tile().is_err());
    let session = DecodeSession::new(DecodePolicy::default());
    let mut wrong = info.clone();
    wrong.cicp.color_primaries = 9;
    let (mut decoder, _) = NumericGroupDecoder::new(
        wrong,
        crate::supported_features(),
        &setup,
        sp.limits,
        &session,
    )
    .unwrap();
    assert!(decoder.read(encoder.output()).is_err());
    assert!(decoder.finish_structure().is_err());
    for n in 0..setup.len() {
        let session = DecodeSession::new(DecodePolicy::default());
        assert!(
            NumericGroupDecoder::new(
                info.clone(),
                crate::supported_features(),
                &setup[..n],
                sp.limits,
                &session
            )
            .is_err()
        );
    }
    let session = DecodeSession::new(DecodePolicy::default().with_max_input_bytes(1));
    assert!(matches!(
        NumericGroupDecoder::new(
            info.clone(),
            crate::supported_features(),
            &setup,
            sp.limits,
            &session
        ),
        Err(Error::TooLarge)
    ));
    let cfg = ImiEncoderConfig::lossless().with_row_groups(RowGroups::new(0, 7, 8 << 20));
    assert!(NumericGroupEncoder::new(info, &cfg, sp).is_err());
}

#[test]
fn ancillary_objects_preserve_numeric_schedule_inside_and_outside_tiles() {
    let info = ImageInfo::new(41, 31, Channels::Rgba, SampleType::U8);
    let cfg = ImiEncoderConfig::lossless().with_row_groups(RowGroups::new(9, 7, 8 << 20));
    let sp = spatial(TileOrder::Morton, ObjectIntegrity::ObjectsAndTiles);
    let (mut encoder, setup) = NumericGroupEncoder::new(info.clone(), &cfg, sp).unwrap();
    let mut r = ByteReader::new(&setup);
    let n = r.var_usize().unwrap();
    let context = SetupContext::from_image_declaration(
        &info,
        crate::Coding::Lossless,
        None,
        r.bytes(n).unwrap(),
    )
    .unwrap();
    let mut frames = TileWriter::new(encoder.layout(), sp.integrity, context, sp.limits);
    let session = DecodeSession::new(DecodePolicy::default());
    let (mut decoder, _) = NumericGroupDecoder::new(
        info,
        crate::supported_features(),
        &setup,
        sp.limits,
        &session,
    )
    .unwrap();
    frames.append(222, b"outside").unwrap();
    assert!(matches!(
        decoder.read(frames.output()).unwrap(),
        NumericEvent::Ancillary {
            kind: 222,
            body: b"outside",
            entry: LocatedEntry {
                entry: Entry { id: 0, .. },
                tile_id: None,
                ..
            }
        }
    ));
    frames.consume_output(frames.output().len()).unwrap();
    frames.begin(&[]).unwrap();
    assert!(matches!(
        decoder.read(frames.output()).unwrap(),
        NumericEvent::Begin { .. }
    ));
    frames.consume_output(frames.output().len()).unwrap();
    frames.append(224, b"inside").unwrap();
    assert!(matches!(
        decoder.read(frames.output()).unwrap(),
        NumericEvent::Ancillary {
            kind: 224,
            body: b"inside",
            entry: LocatedEntry {
                entry: Entry { id: 1, .. },
                tile_id: Some(0),
                ..
            }
        }
    ));
    frames.consume_output(frames.output().len()).unwrap();
    encoder.begin_tile().unwrap();
    drain(&mut encoder);
    encoder.push_group(&[0; 9 * 7 * 4], 9 * 4).unwrap();
    let objects =
        crate::v4::ObjectReader::new(sp.integrity, context, &session, sp.limits.max_object_bytes);
    let object = objects.read(encoder.output()).unwrap();
    frames
        .append(crate::registry::UNIT_STRIPE, object.body)
        .unwrap();
    let NumericEvent::Group(work) = decoder.read(frames.output()).unwrap() else {
        panic!("expected group zero")
    };
    assert_eq!(work.entry.entry.id, 0);
    let mut decoded = [0xaa; 9 * 7 * 4];
    work.decode_into(&mut decoded, 9 * 4, &session).unwrap();
    assert_eq!(decoded, [0; 9 * 7 * 4]);
    assert!(decoder.finish_structure().is_err());
}

#[test]
fn standalone_research_bank_flag_is_not_a_v4_setup_field() {
    let info = ImageInfo::new(41, 31, Channels::Rgba, SampleType::U8);
    let cfg = ImiEncoderConfig::lossless().with_row_groups(RowGroups::new(9, 7, 8 << 20));
    let sp = spatial(TileOrder::Morton, ObjectIntegrity::ObjectsAndTiles);
    let (_, mut setup) = NumericGroupEncoder::new(info.clone(), &cfg, sp).unwrap();
    let mut prefix = ByteReader::new(&setup);
    prefix.varint().unwrap();
    for _ in 0..4 {
        prefix.varint().unwrap();
    }
    let present = prefix.pos;
    setup[present] |= crate::v1::H_LAB_BANK as u8;
    let session = DecodeSession::new(DecodePolicy::default());
    assert!(matches!(
        NumericGroupDecoder::new(
            info,
            crate::supported_features(),
            &setup,
            sp.limits,
            &session
        ),
        Err(Error::Unsupported("V4 standalone model bank"))
    ));
}
