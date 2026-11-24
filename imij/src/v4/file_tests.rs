use super::*;
use crate::decode_resources::{DecodePolicy, DecodeSession};
use crate::{Channels, ImageInfo, ImiEncoderConfig, RowGroups, SampleType};
use alloc::{vec, vec::Vec};

fn spatial(mode: ObjectIntegrity, order: TileOrder) -> SpatialConfig {
    SpatialConfig {
        tile_columns: 3,
        tile_rows: 2,
        order,
        integrity: mode,
        limits: TileLimits {
            max_object_bytes: 1 << 20,
            max_tile_bytes: 16 << 20,
        },
    }
}

fn source(info: &ImageInfo, stride: usize) -> Vec<u8> {
    (0..stride * info.height as usize)
        .map(|i| ((i * 73 + i / 7 * 41) & 255) as u8)
        .collect()
}

#[test]
fn numeric_files_preserve_native_bits_metadata_and_strides() {
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
            let info = ImageInfo::new(41, 31, channels, sample);
            let row = info.row_bytes();
            let stride = row + 13;
            let pixels = source(&info, stride);
            for quick in [false, true] {
                for mode in [
                    ObjectIntegrity::None,
                    ObjectIntegrity::Objects,
                    ObjectIntegrity::ObjectsAndTiles,
                ] {
                    for order in [TileOrder::Raster, TileOrder::Morton] {
                        let cfg = if quick {
                            ImiEncoderConfig::quick()
                        } else {
                            ImiEncoderConfig::lossless()
                        }
                        .with_row_groups(RowGroups::new(9, 7, 8 << 20));
                        let meta = crate::Metadata {
                            orientation: 6,
                            xmp: Some(b"<test/>".to_vec()),
                            ..Default::default()
                        };
                        let spatial = spatial(mode, order);
                        let bytes =
                            encode_numeric(info.clone(), &pixels, stride, &meta, &cfg, spatial)
                                .unwrap();
                        assert_eq!(header(&bytes, 64 << 10).unwrap().version, 4);
                        assert_eq!(crate::header(&bytes).unwrap().version, 4);
                        let session = DecodeSession::new(DecodePolicy::default());
                        let file = NumericFile::open(&bytes, spatial.limits, &session).unwrap();
                        let out_stride = row + 17;
                        let mut output = vec![0xdb; out_stride * info.height as usize];
                        let (head, got_meta) = file.decode_into(&mut output, out_stride).unwrap();
                        assert_eq!(head.info, info);
                        assert_eq!(got_meta.xmp, meta.xmp);
                        assert_eq!(got_meta.orientation, 6);
                        for y in 0..info.height as usize {
                            assert_eq!(
                                &output[y * out_stride..y * out_stride + row],
                                &pixels[y * stride..y * stride + row],
                                "{sample:?} {channels:?} quick={quick} {mode:?} {order:?}"
                            );
                            assert!(
                                output[y * out_stride + row..(y + 1) * out_stride]
                                    .iter()
                                    .all(|&b| b == 0xdb)
                            );
                        }
                        assert_eq!(session.usage().input_bytes, bytes.len() as u64);
                    }
                }
            }
        }
    }
}

pub(crate) fn rewrite_placement(
    bytes: &[u8],
    placement: DirectoryPlacement,
    open: bool,
) -> Vec<u8> {
    let mut r = crate::ByteReader::new(bytes);
    crate::parse::header::read_header_v4(&mut r, 64 << 10).unwrap();
    let before_data = r.pos;
    let mut chunks = Vec::new();
    while r.remaining() != 0 {
        let (tag, len) = crate::parse::chunks::read_head(&mut r).unwrap();
        let body = crate::parse::chunks::payload(&mut r, len.unwrap()).unwrap();
        chunks.push((tag, body));
    }
    let payload = chunks.iter().find(|(tag, _)| tag == b"DATA").unwrap().1;
    let mut setup = crate::ByteReader::new(payload);
    let length = setup.var_usize().unwrap();
    setup.bytes(length).unwrap();
    let immutable_end = setup.pos;
    let session = DecodeSession::new(DecodePolicy::default());
    let prefix = DirectoryPrefix::read(&payload[immutable_end..], &session).unwrap();
    let index = match prefix.placement {
        DirectoryPlacement::Front => prefix.front.unwrap(),
        DirectoryPlacement::Tail => {
            let body = chunks.iter().find(|(tag, _)| tag == b"uIDX").unwrap().1;
            &body[..body.len() - 12]
        }
        DirectoryPlacement::None => panic!("placement fixture needs an indexed source"),
    };
    setup.bytes(prefix.encoded_len).unwrap();
    let mut index_payload = index.to_vec();
    let chunk_len = crate::parse::records::chunk_len_of(index.len() as u64 + 12);
    index_payload.extend_from_slice(&chunk_len.to_le_bytes());
    index_payload.extend_from_slice(b"uIDX");
    let mut new_payload = payload[..immutable_end].to_vec();
    append_directory_prefix(
        &mut new_payload,
        placement,
        if placement == DirectoryPlacement::Front {
            index.len() as u64 + 20
        } else {
            0
        },
        (placement == DirectoryPlacement::Front).then_some(index),
        1 << 20,
    )
    .unwrap();
    new_payload.extend_from_slice(&payload[setup.pos..]);
    let mut file = bytes[..before_data].to_vec();
    let mut checksum = false;
    for (tag, body) in chunks {
        match &tag {
            b"cSUM" => checksum = true,
            b"uIDX" => {}
            b"DATA" => {
                if open {
                    file.extend_from_slice(b"DATA\0");
                    file.extend_from_slice(&new_payload);
                } else {
                    crate::container::put_data(&mut file, &new_payload);
                }
            }
            _ => crate::container::put_chunk(&mut file, &tag, body),
        }
    }
    if placement == DirectoryPlacement::Tail {
        crate::container::put_chunk(&mut file, b"uIDX", &index_payload);
    }
    if checksum {
        file = crate::checksum::refinish(file).unwrap();
    }
    file
}

#[test]
fn same_objects_decode_with_front_tail_none_and_open_indexed_data() {
    let info = ImageInfo::new(41, 31, Channels::Rgba, SampleType::U16);
    let spatial = spatial(ObjectIntegrity::ObjectsAndTiles, TileOrder::Morton);
    let cfg = ImiEncoderConfig::lossless().with_row_groups(RowGroups::new(9, 7, 8 << 20));
    let pixels = source(&info, info.row_bytes());
    let original = encode_numeric(
        info.clone(),
        &pixels,
        info.row_bytes(),
        &Default::default(),
        &cfg,
        spatial,
    )
    .unwrap();
    for (placement, open) in [
        (DirectoryPlacement::Tail, false),
        (DirectoryPlacement::Tail, true),
        (DirectoryPlacement::Front, false),
        (DirectoryPlacement::Front, true),
        (DirectoryPlacement::None, false),
        (DirectoryPlacement::None, true),
    ] {
        let bytes = rewrite_placement(&original, placement, open);
        let session = DecodeSession::new(DecodePolicy::default());
        let file = NumericFile::open(&bytes, spatial.limits, &session).unwrap();
        let mut output = vec![0; pixels.len()];
        file.decode_into(&mut output, info.row_bytes()).unwrap();
        assert_eq!(output, pixels);
        assert_eq!(
            session.usage().input_bytes,
            bytes.len() as u64,
            "{placement:?}"
        );
    }
}

#[test]
fn physical_identity_extent_and_finalization_errors_are_not_success() {
    let info = ImageInfo::new(41, 31, Channels::Gray, SampleType::U8);
    let spatial = spatial(ObjectIntegrity::None, TileOrder::Morton);
    let cfg = ImiEncoderConfig::lossless().with_row_groups(RowGroups::new(9, 7, 8 << 20));
    let pixels = source(&info, info.row_bytes());
    let original = encode_numeric(
        info.clone(),
        &pixels,
        info.row_bytes(),
        &Default::default(),
        &cfg,
        spatial,
    )
    .unwrap();
    let attempt = |bytes: &[u8]| {
        let session = DecodeSession::new(DecodePolicy::default());
        NumericFile::open(bytes, spatial.limits, &session)
            .and_then(|f| f.decode_into(&mut vec![0; pixels.len()], info.row_bytes()))
            .map(|_| ())
    };
    let mut forged = original.clone();
    *forged.last_mut().unwrap() ^= 1;
    assert!(attempt(&forged).is_err());
    let mut forged = original.clone();
    crate::container::put_chunk(&mut forged, b"tEST", b"after directory");
    assert!(attempt(&forged).is_err());
    for cut in [
        0,
        8,
        15,
        original.len() / 2,
        original.len() - 1,
        original.len() - 12,
    ] {
        assert!(attempt(&original[..cut]).is_err());
    }
    let mut r = crate::ByteReader::new(&original);
    crate::parse::header::read_header_v4(&mut r, 64 << 10).unwrap();
    let (_, len) = crate::parse::chunks::read_head(&mut r).unwrap();
    let payload_at = r.pos;
    let payload = crate::parse::chunks::payload(&mut r, len.unwrap()).unwrap();
    let session = DecodeSession::new(DecodePolicy::default());
    let (mut decoder, start) = NumericGroupDecoder::new(
        info.clone(),
        crate::supported_features(),
        payload,
        spatial.limits,
        &session,
    )
    .unwrap();
    let begin = decoder.read(&payload[start..]).unwrap().entry();
    let unit_at = payload_at + start + begin.entry.record_len as usize;
    let mut unit = crate::ByteReader::new(&original[unit_at..]);
    assert_eq!(unit.varint().unwrap(), 59);
    unit.varint().unwrap();
    let identity_at = unit_at + unit.pos;
    let mut forged = original.clone();
    forged[identity_at] = 1; // Valid small identity, wrong scheduled group.
    assert!(attempt(&forged).is_err());
    let session = DecodeSession::new(DecodePolicy::default());
    let file = NumericFile::open(&original, spatial.limits, &session).unwrap();
    let mut output = vec![0xd7; pixels.len()];
    assert!(file.decode_into(&mut output, info.row_bytes() - 1).is_err());
    assert!(output.iter().all(|&b| b == 0xd7));
    let session = DecodeSession::new(DecodePolicy::default().with_max_work_units(0));
    assert!(NumericFile::open(&original, spatial.limits, &session).is_err());
    let session = DecodeSession::with_cancel(DecodePolicy::default(), || true);
    assert!(NumericFile::open(&original, spatial.limits, &session).is_err());
}
