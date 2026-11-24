use super::*;
use crate::decode_resources::DecodePolicy;
use crate::parse::limits::Structure;
use crate::stream_encode::{GroupEncodeConfig, NativeGroupEncodeSession as GroupEncodeSession};
use crate::{Channels, SampleType};
use alloc::vec;

#[test]
fn coverage_rectangles_preserve_native_bits_between_uniform_boundaries() {
    use crate::stream_encode::GroupRect;
    for sample in [
        SampleType::U8,
        SampleType::U16,
        SampleType::I8,
        SampleType::I16,
        SampleType::U32,
        SampleType::I32,
        SampleType::U64,
        SampleType::I64,
        SampleType::F16,
        SampleType::F32,
        SampleType::F64,
    ] {
        for channels in [Channels::Gray, Channels::Rgba] {
            for quick in [false, true] {
                let info = ImageInfo::new(23, 17, channels, sample);
                let stride = info.row_bytes() + 13;
                let pixels: Vec<u8> = (0..stride * 17)
                    .map(|i| (i * 47 + i / 7 * 13) as u8)
                    .collect();
                let cfg = if quick {
                    GroupEncodeConfig::new(9, 7, 8 << 20)
                        .with_quick_bit_smarts(crate::QuickBitSmarts::Off)
                } else {
                    GroupEncodeConfig::new(9, 7, 8 << 20)
                };
                let (_, head) = GroupEncodeSession::nested(info.clone(), cfg).unwrap();
                let session = DecodeSession::new(DecodePolicy::default());
                let model =
                    NumericModel::read_root(info.clone(), &head, Features::ALL, &session).unwrap();
                for rect in [
                    crate::planes::Rect::new(0, 3, 9, 5),
                    crate::planes::Rect::new(9, 8, 9, 4),
                    crate::planes::Rect::new(18, 12, 5, 5),
                ] {
                    let mut local = info.clone();
                    local.height = rect.height();
                    let mut local_cfg = cfg;
                    local_cfg.rows = rect.height();
                    let (mut encoder, _) = GroupEncodeSession::nested(local, local_cfg).unwrap();
                    let at =
                        rect.y() as usize * stride + rect.x() as usize * info.bytes_per_pixel();
                    let (encoded, start) = encoder
                        .encode_rectangle(
                            GroupRect {
                                x: rect.x(),
                                y: 0,
                                width: rect.width(),
                                height: rect.height(),
                            },
                            &pixels[at..],
                            stride,
                        )
                        .unwrap();
                    let mut record = ByteReader::new(&encoded[start..]);
                    assert_eq!(record.varint().unwrap(), crate::registry::UNIT_STRIPE);
                    let length = record.var_usize().unwrap();
                    let entropy = record.bytes(length).unwrap();
                    let row = rect.width() as usize * info.bytes_per_pixel();
                    let out_stride = row + 17;
                    let mut output = vec![0xd5; out_stride * rect.height() as usize];
                    model
                        .decode_rectangle(rect, entropy, &mut output, out_stride, &session)
                        .unwrap();
                    for y in 0..rect.height() as usize {
                        assert_eq!(
                            &output[y * out_stride..y * out_stride + row],
                            &pixels[at + y * stride..at + y * stride + row]
                        );
                        assert!(
                            output[y * out_stride + row..(y + 1) * out_stride]
                                .iter()
                                .all(|&b| b == 0xd5)
                        );
                    }
                }
                let mut output = vec![0x93; 1024];
                for rect in [
                    crate::planes::Rect::new(1, 0, 9, 5),
                    crate::planes::Rect::new(0, 0, 8, 5),
                    crate::planes::Rect::new(0, 0, 9, 8),
                    crate::planes::Rect::new(0, 16, 9, 2),
                ] {
                    assert!(
                        model
                            .decode_rectangle(rect, &[], &mut output, 256, &session)
                            .is_err()
                    );
                    assert!(output.iter().all(|&b| b == 0x93));
                }
            }
        }
    }
}

#[test]
fn large_canvas_tail_group_identity_does_not_narrow_to_pointer_width() {
    use crate::stream_encode::GroupRect;
    for quick in [false, true] {
        let info = ImageInfo::new(1_000_000, 1_000_000, Channels::Gray, SampleType::U16);
        let cfg = if quick {
            GroupEncodeConfig::new(9, 7, 8 << 20).with_quick_bit_smarts(crate::QuickBitSmarts::Off)
        } else {
            GroupEncodeConfig::new(9, 7, 8 << 20)
        };
        let (mut encoder, head) = GroupEncodeSession::nested(info.clone(), cfg).unwrap();
        let session = DecodeSession::new(DecodePolicy::default());
        let model = NumericModel::read_root(info, &head, Features::ALL, &session).unwrap();
        let id = model.group_count() - 1;
        assert!(id > u64::from(u32::MAX));
        let rect = model.group_rect(id).unwrap();
        assert_eq!(rect, crate::planes::Rect::new(999_999, 999_999, 1, 1));
        let pixels = [0x2f, 0x91];
        let (encoded, start) = encoder
            .encode_rectangle(
                GroupRect {
                    x: 999_999,
                    y: 999_999,
                    width: 1,
                    height: 1,
                },
                &pixels,
                2,
            )
            .unwrap();
        let mut record = ByteReader::new(&encoded[start..]);
        record.varint().unwrap();
        let length = record.var_usize().unwrap();
        let entropy = record.bytes(length).unwrap();
        let mut output = [0; 2];
        assert_eq!(
            model
                .decode_group(id, entropy, &mut output, 2, &session)
                .unwrap(),
            rect
        );
        assert_eq!(output, pixels);
    }
}

fn definition(head: &[u8]) -> (NumericGeometry, Vec<u8>) {
    let mut r = ByteReader::new(head);
    let present = r.varint().unwrap();
    let lanes = r.u8().unwrap();
    let rows = r.var_u32().unwrap();
    let columns = r.var_u32().unwrap();
    let mut out = Vec::new();
    put_varint(&mut out, present);
    out.extend_from_slice(&head[r.pos..]);
    // Pixel interpretation is replaced by each witness's actual plane info.
    (
        NumericGeometry::new(
            ImageInfo::new(23, 17, Channels::Gray, SampleType::U8),
            lanes,
            rows,
            columns,
        )
        .unwrap(),
        out,
    )
}

fn witness(info: &ImageInfo) -> (Vec<u8>, Vec<Vec<u8>>, Vec<u8>, usize) {
    let stride = info.row_bytes() + 13;
    let pixels: Vec<u8> = (0..stride * info.height as usize)
        .map(|i| ((i * 47 + i / 7 * 13) & 255) as u8)
        .collect();
    let cfg = GroupEncodeConfig::new(9, 7, 8 << 20);
    let (mut encoder, head) = GroupEncodeSession::nested(info.clone(), cfg).unwrap();
    let mut groups = Vec::new();
    while let Some(group) = encoder.next_group() {
        let start = group.y as usize * stride + group.x as usize * info.bytes_per_pixel();
        encoder.push_group(&pixels[start..], stride).unwrap();
        let mut r = ByteReader::new(encoder.output());
        assert_eq!(r.varint().unwrap(), crate::registry::UNIT_STRIPE);
        let len = r.var_usize().unwrap();
        groups.push(r.bytes(len).unwrap().to_vec());
        assert_eq!(r.remaining(), 0);
        encoder.consume_output(encoder.output().len()).unwrap();
    }
    encoder.finish().unwrap();
    (head, groups, pixels, stride)
}

#[test]
fn native_numeric_planes_decode_exact_bits_with_strided_edge_groups() {
    for sample in [
        SampleType::U8,
        SampleType::U16,
        SampleType::I8,
        SampleType::I16,
        SampleType::U32,
        SampleType::I32,
        SampleType::U64,
        SampleType::I64,
        SampleType::F16,
        SampleType::F32,
        SampleType::F64,
    ] {
        for channels in [Channels::Gray, Channels::Rgba] {
            let info = ImageInfo::new(23, 17, channels, sample);
            let (head, groups, pixels, stride) = witness(&info);
            let (mut geometry, definition) = definition(&head);
            geometry.info = info.clone();
            let session = DecodeSession::new(DecodePolicy::default());
            let model = NumericModel::read(
                geometry,
                &definition,
                0,
                crate::supported_features(),
                &session,
            )
            .unwrap();
            assert_eq!(model.group_count() as usize, groups.len());
            assert_eq!(model.info(), &info);
            let mut reconstructed = vec![0xa5; stride * info.height as usize];
            // Deliberately decode in reverse geometry order; no inherited rows.
            for id in (0..model.group_count()).rev() {
                let rect = model.group_rect(id).unwrap();
                let row_bytes = rect.width() as usize * info.bytes_per_pixel();
                let dst_stride = row_bytes + 11;
                let mut decoded = vec![0xcc; dst_stride * rect.height() as usize];
                model
                    .decode_group(id, &groups[id as usize], &mut decoded, dst_stride, &session)
                    .unwrap();
                for y in 0..rect.height() as usize {
                    let at = (rect.y() as usize + y) * stride
                        + rect.x() as usize * info.bytes_per_pixel();
                    assert_eq!(
                        &decoded[y * dst_stride..][..row_bytes],
                        &pixels[at..][..row_bytes],
                        "{sample:?}/{channels:?}/{id}/{y}"
                    );
                    assert!(
                        decoded[y * dst_stride + row_bytes..(y + 1) * dst_stride]
                            .iter()
                            .all(|&v| v == 0xcc)
                    );
                    reconstructed[at..at + row_bytes]
                        .copy_from_slice(&decoded[y * dst_stride..][..row_bytes]);
                }
            }
            let peak = session.usage().peak_planned_working_bytes;
            let rect = model.group_rect(0).unwrap();
            let row_bytes = rect.width() as usize * info.bytes_per_pixel();
            let mut again = vec![0; row_bytes * rect.height() as usize];
            model
                .decode_group(0, &groups[0], &mut again, row_bytes, &session)
                .unwrap();
            assert_eq!(session.usage().peak_planned_working_bytes, peak);
            for y in 0..info.height as usize {
                assert_eq!(
                    &reconstructed[y * stride..][..info.row_bytes()],
                    &pixels[y * stride..][..info.row_bytes()]
                );
            }
        }
    }
}

#[test]
fn geometry_is_not_serialized_in_definitions_and_large_canvas_needs_no_unit_vector() {
    let info = ImageInfo::new(23, 17, Channels::Gray, SampleType::U8);
    let (head, _, _, _) = witness(&info);
    let (geometry, definition) = definition(&head);
    let small = DecodeSession::new(DecodePolicy::default());
    NumericModel::read(
        geometry,
        &definition,
        0,
        crate::supported_features(),
        &small,
    )
    .unwrap();
    let huge_geometry = NumericGeometry::new(
        ImageInfo::new(1_000_000, 1_000_000, Channels::Gray, SampleType::U8),
        16,
        7,
        9,
    )
    .unwrap();
    let huge = DecodeSession::new(DecodePolicy::default());
    let model = NumericModel::read(
        huge_geometry,
        &definition,
        0,
        crate::supported_features(),
        &huge,
    )
    .unwrap();
    assert!(model.group_count() > 1_000_000_000);
    assert_eq!(
        small.usage().peak_planned_working_bytes,
        huge.usage().peak_planned_working_bytes
    );
    assert_eq!(small.usage().work_units, huge.usage().work_units);
    assert_eq!(huge.usage().units, 0); // No units decoded or reserved by ID/count.
    assert!(model.group_rect(model.group_count()).is_err());
}

#[test]
fn malformed_definitions_limits_and_cancel_refuse_before_output() {
    let info = ImageInfo::new(23, 17, Channels::Gray, SampleType::U8);
    let (head, groups, _, _) = witness(&info);
    let (geometry, definition) = definition(&head);
    // Independently confirm this is a complete old head before extracting fields.
    let mut old = ByteReader::new(&head);
    crate::v1::parse_head_checked(info, &mut old, &Structure, crate::supported_features()).unwrap();
    assert_eq!(old.remaining(), 0);
    for n in 0..definition.len() {
        let session = DecodeSession::new(DecodePolicy::default());
        assert!(
            NumericModel::read(
                geometry.clone(),
                &definition[..n],
                0,
                crate::supported_features(),
                &session
            )
            .is_err()
        );
    }
    let session = DecodeSession::new(DecodePolicy::default());
    assert!(
        NumericModel::read(
            geometry.clone(),
            &definition,
            u64::MAX,
            crate::supported_features(),
            &session
        )
        .is_err()
    );
    let session = DecodeSession::new(DecodePolicy::default().with_max_working_bytes(1024));
    assert!(matches!(
        NumericModel::read(
            geometry.clone(),
            &definition,
            0,
            crate::supported_features(),
            &session
        ),
        Err(Error::TooLarge)
    ));
    let session = DecodeSession::new(DecodePolicy::default());
    let model = NumericModel::read(
        geometry,
        &definition,
        0,
        crate::supported_features(),
        &session,
    )
    .unwrap();
    let cancelled = DecodeSession::with_cancel(DecodePolicy::default(), || true);
    let mut out = vec![0x77; 9 * 7];
    assert!(matches!(
        model.decode_group(0, &groups[0], &mut out, 9, &cancelled),
        Err(Error::Cancelled)
    ));
    assert!(out.iter().all(|&v| v == 0x77));
    assert!(
        model
            .decode_group(0, &groups[0], &mut out, 8, &session)
            .is_err()
    );
    assert!(out.iter().all(|&v| v == 0x77));
}

#[cfg(feature = "std")]
#[test]
fn one_fetched_tile_supplies_numeric_groups_at_one_two_four_eight_workers() {
    use crate::v4::{
        ObjectIntegrity, SetupContext, TileEvent, TileLayout, TileLimits, TileOrder, TileReader,
        TileWriter,
    };
    let info = ImageInfo::new(41, 31, Channels::Rgba, SampleType::U16);
    let (head, groups, source, source_stride) = witness(&info);
    let (mut geometry, definition) = definition(&head);
    geometry.info = info.clone();
    let session = DecodeSession::new(DecodePolicy::default());
    let model = NumericModel::read(
        geometry,
        &definition,
        0,
        crate::supported_features(),
        &session,
    )
    .unwrap();
    let layout = TileLayout::new(5, 5, 3, 3, TileOrder::Morton).unwrap();
    let mode = ObjectIntegrity::ObjectsAndTiles;
    let context = SetupContext::from_declaration(b"numeric test setup");
    let limits = TileLimits {
        max_object_bytes: 1 << 20,
        max_tile_bytes: 8 << 20,
    };
    let mut frames = TileWriter::new(layout, mode, context, limits);
    let mut bytes = Vec::new();
    let mut tiles = Vec::new();
    let drain = |frames: &mut TileWriter, bytes: &mut Vec<u8>| {
        bytes.extend_from_slice(frames.output());
        frames.consume_output(frames.output().len()).unwrap();
    };
    for ordinal in 0..layout.tile_count() {
        let tile = layout.tile_at(ordinal).unwrap();
        frames.begin(&[]).unwrap();
        drain(&mut frames, &mut bytes);
        for group in layout.groups(tile.id).unwrap() {
            let mut body = Vec::new();
            put_varint(&mut body, group.id);
            put_varint(&mut body, 0); // explicit root numeric model ID
            body.extend_from_slice(&groups[group.id as usize]);
            frames.append(51, &body).unwrap();
            drain(&mut frames, &mut bytes);
        }
        tiles.push(frames.end().unwrap().1);
        drain(&mut frames, &mut bytes);
    }
    frames.finish().unwrap();
    let tile = tiles[0];
    // Exactly one contiguous payload acquisition. Setup/model are already open.
    let fetched = bytes[tile.offset as usize..(tile.offset + tile.len) as usize].to_vec();
    let mut reader = TileReader::new(layout, mode, context, limits, &session)
        .at_tile(0, tile.offset)
        .unwrap();
    let mut jobs = Vec::new();
    let mut offset = 0;
    while offset < fetched.len() {
        offset += match reader.read(&fetched[offset..]).unwrap() {
            TileEvent::Begin { extent, .. } | TileEvent::End { extent, .. } => extent.len as usize,
            TileEvent::Object { extent, object } => {
                let mut body = ByteReader::new(object.body);
                let id = body.varint().unwrap();
                assert_eq!(body.varint().unwrap(), 0);
                jobs.push((id, &object.body[body.pos..]));
                extent.len as usize
            }
        };
    }
    assert_eq!(jobs.len(), 9);
    for workers in [1, 2, 4, 8] {
        let decoded = std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for worker in 0..workers {
                let jobs = &jobs;
                let model = &model;
                handles.push(scope.spawn(move || {
                    let session = DecodeSession::new(DecodePolicy::default());
                    let mut results = Vec::new();
                    for &(id, entropy) in jobs.iter().skip(worker).step_by(workers) {
                        let rect = model.group_rect(id).unwrap();
                        let stride = rect.width() as usize * model.info().bytes_per_pixel() + 7;
                        let mut output = vec![0xa5; stride * rect.height() as usize];
                        model
                            .decode_group(id, entropy, &mut output, stride, &session)
                            .unwrap();
                        results.push((id, rect, stride, output));
                    }
                    results
                }));
            }
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap())
                .collect::<Vec<_>>()
        });
        let mut ids = Vec::new();
        for (id, rect, stride, output) in decoded {
            ids.push(id);
            let row_bytes = rect.width() as usize * info.bytes_per_pixel();
            for y in 0..rect.height() as usize {
                let at = (rect.y() as usize + y) * source_stride
                    + rect.x() as usize * info.bytes_per_pixel();
                assert_eq!(
                    &output[y * stride..][..row_bytes],
                    &source[at..][..row_bytes]
                );
                assert!(
                    output[y * stride + row_bytes..(y + 1) * stride]
                        .iter()
                        .all(|&b| b == 0xa5)
                );
            }
        }
        ids.sort_unstable();
        assert_eq!(
            ids,
            layout
                .groups(tile.tile_id)
                .unwrap()
                .map(|g| g.id)
                .collect::<Vec<_>>()
        );
    }
}
