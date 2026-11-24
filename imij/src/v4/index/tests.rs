use super::*;
use crate::decode_resources::DecodePolicy;
use crate::v4::{
    ObjectReader, SetupContext, TileEvent, TileLimits, TileOrder, TileReader, TileWriter,
};
use alloc::vec::Vec;

fn layout() -> TileLayout {
    TileLayout::new(5, 4, 2, 3, TileOrder::Morton).unwrap()
}

fn fixture() -> (Vec<u8>, Vec<u8>, Vec<TileExtent>) {
    let context = SetupContext::from_declaration(b"setup");
    let limits = TileLimits {
        max_object_bytes: 4096,
        max_tile_bytes: 32768,
    };
    let mode = ObjectIntegrity::ObjectsAndTiles;
    let mut frames = TileWriter::new(layout(), mode, context, limits);
    let mut index = DirectoryWriter::new(layout(), mode);
    let mut records = Vec::new();
    let mut entries = Vec::new();
    let mut tiles = Vec::new();
    let mut each = |frame: &mut TileWriter, kind: EntryKind, id: u64, len: u64| {
        let mut buffer = [0; 40];
        let n = index
            .push(
                Entry {
                    kind,
                    role: 0,
                    id,
                    record_len: len,
                },
                &mut buffer,
            )
            .unwrap();
        entries.extend_from_slice(&buffer[..n]);
        records.extend_from_slice(frame.output());
        frame.consume_output(frame.output().len()).unwrap();
    };
    for ordinal in 0..layout().tile_count() {
        let tile = layout().tile_at(ordinal).unwrap();
        let begin = frames.begin(&[]).unwrap();
        each(&mut frames, EntryKind::TileBegin, tile.id, begin.len);
        for group in layout().groups(tile.id).unwrap() {
            let mut body = Vec::new();
            crate::put_varint(&mut body, group.id);
            body.extend_from_slice(b"independent entropy placeholder");
            let extent = frames.append(51, &body).unwrap();
            each(&mut frames, EntryKind::Unit, group.id, extent.len);
        }
        let (extent, tile) = frames.end().unwrap();
        each(&mut frames, EntryKind::TileEnd, tile.tile_id, extent.len);
        tiles.push(tile);
    }
    frames.finish().unwrap();
    let (count, len) = index.finish().unwrap();
    assert_eq!(len as usize, records.len());
    let mut body = Vec::new();
    crate::put_varint(&mut body, count);
    body.extend_from_slice(&entries);
    body.extend_from_slice(&len.to_le_bytes());
    (records, body, tiles)
}

#[test]
fn directory_extents_match_real_tile_frames_and_each_fetched_identity() {
    let (records, body, tiles) = fixture();
    let session = DecodeSession::new(DecodePolicy::default());
    let mode = ObjectIntegrity::ObjectsAndTiles;
    let directory = Directory::read(&body, layout(), mode, records.len() as u64, &session).unwrap();
    assert_eq!(directory.entry_count(), 20 + 2 * layout().tile_count());
    assert_eq!(directory.objects_len(), records.len() as u64);
    let context = SetupContext::from_declaration(b"setup");
    let object_reader = ObjectReader::new(mode, context, &session, 4096);
    directory
        .visit(&session, |entry, _| {
            let object = object_reader
                .read(&records[entry.offset as usize..])
                .unwrap();
            let rest = entry.bind(object).unwrap();
            if entry.entry.kind == EntryKind::Unit {
                assert_eq!(rest, b"independent entropy placeholder");
            }
            Ok(true)
        })
        .unwrap();
    for (ordinal, expected) in tiles.into_iter().enumerate() {
        let tile = directory.tile(expected.tile_id, &session).unwrap();
        assert_eq!(tile, expected);
        // One contiguous range supplies all of this tile's framed children.
        let bytes = &records[tile.offset as usize..(tile.offset + tile.len) as usize];
        let limits = TileLimits {
            max_object_bytes: 4096,
            max_tile_bytes: 32768,
        };
        let mut reader = TileReader::new(layout(), mode, context, limits, &session)
            .at_tile(ordinal as u64, tile.offset)
            .unwrap();
        let mut at = 0;
        while at < bytes.len() {
            at += match reader.read(&bytes[at..]).unwrap() {
                TileEvent::Begin { extent, .. } | TileEvent::Object { extent, .. } => {
                    extent.len as usize
                }
                TileEvent::End {
                    extent,
                    tile: observed,
                    ..
                } => {
                    assert_eq!(observed, tile);
                    extent.len as usize
                }
            };
        }
        for group in layout().groups(tile.tile_id).unwrap() {
            let entry = directory
                .find(EntryKind::Unit, 0, group.id, Some(tile.tile_id), &session)
                .unwrap();
            assert!(entry.offset >= tile.offset);
            assert!(entry.offset + entry.entry.record_len < tile.offset + tile.len);
        }
    }
}

#[test]
fn four_field_entry_encoding_is_canonical_and_full_width() {
    let entry = Entry {
        kind: EntryKind::Model,
        role: 3,
        id: 16_384,
        record_len: 128,
    };
    let mut buffer = [0; 40];
    let n = entry.encode(&mut buffer);
    assert_eq!(&buffer[..n], &[1, 3, 0x80, 0x80, 1, 0x80, 1]);
    let entry = Entry {
        kind: EntryKind::Model,
        role: 3,
        id: u64::MAX,
        record_len: u64::MAX,
    };
    let n = entry.encode(&mut buffer);
    assert_eq!(n, 22);
    assert_eq!(
        Entry::read(&mut ByteReader::new(&buffer[..n])).unwrap(),
        entry
    );
}

fn bare(entries: &[Entry], total: u64) -> Vec<u8> {
    let mut out = Vec::new();
    crate::put_varint(&mut out, entries.len() as u64);
    for &entry in entries {
        let mut buffer = [0; 40];
        let n = entry.encode(&mut buffer);
        out.extend_from_slice(&buffer[..n]);
    }
    out.extend_from_slice(&total.to_le_bytes());
    out
}

#[test]
fn missing_duplicate_cross_tile_and_overflowing_entries_fail() {
    let l = TileLayout::new(2, 1, 1, 1, TileOrder::Raster).unwrap();
    let begin = Entry {
        kind: EntryKind::TileBegin,
        role: 0,
        id: 0,
        record_len: 3,
    };
    let unit = Entry {
        kind: EntryKind::Unit,
        role: 0,
        id: 0,
        record_len: 7,
    };
    let end = Entry {
        kind: EntryKind::TileEnd,
        role: 0,
        id: 0,
        record_len: 5,
    };
    let valid = [
        begin,
        unit,
        end,
        Entry { id: 1, ..begin },
        Entry { id: 1, ..unit },
        Entry { id: 1, ..end },
    ];
    let session = DecodeSession::new(DecodePolicy::default());
    assert!(Directory::read(&bare(&valid, 30), l, ObjectIntegrity::None, 30, &session).is_ok());
    for variant in 0..8 {
        let mut entries = valid.to_vec();
        match variant {
            0 => {
                entries.remove(2);
            } // missing end
            1 => {
                entries.insert(2, unit);
            } // duplicate
            2 => entries[1].id = 1,   // unit belongs to other tile
            3 => entries[3].id = 0,   // repeated tile
            4 => entries[2].role = 1, // marker role
            5 => entries[0].record_len = u64::MAX, // prefix sum overflow
            6 => entries[1].record_len = 1, // impossible frame
            7 => entries[1].role = 9, // invalid plane
            _ => unreachable!(),
        }
        let bytes = bare(&entries, 30);
        let session = DecodeSession::new(DecodePolicy::default());
        assert!(
            Directory::read(&bytes, l, ObjectIntegrity::None, 30, &session).is_err(),
            "variant {variant}"
        );
    }
}

#[test]
fn model_ids_are_sparse_immutable_and_lookup_is_directly_bound() {
    let l = TileLayout::new(1, 1, 1, 1, TileOrder::Raster).unwrap();
    let model = Entry {
        kind: EntryKind::Model,
        role: 1,
        id: 0,
        record_len: 10,
    };
    let entries = [
        model,
        Entry {
            id: u64::MAX,
            ..model
        },
        Entry {
            kind: EntryKind::TileBegin,
            role: 0,
            id: 0,
            record_len: 3,
        },
        Entry {
            kind: EntryKind::Unit,
            role: 1,
            id: 0,
            record_len: 7,
        },
        Entry {
            kind: EntryKind::TileEnd,
            role: 0,
            id: 0,
            record_len: 5,
        },
    ];
    let body = bare(&entries, 35);
    let session = DecodeSession::new(DecodePolicy::default());
    let d = Directory::read(&body, l, ObjectIntegrity::None, 35, &session).unwrap();
    let found = d
        .find(EntryKind::Model, 1, u64::MAX, None, &session)
        .unwrap();
    assert_eq!(found.offset, 10);
    // No allocation or dense table indexed by u64::MAX.
    let object = Object {
        kind: 63,
        body: &[1, 0, 0],
        encoded_len: 10,
        integrity_verified: false,
    };
    assert!(found.bind(object).is_err()); // body model ID disagrees
    assert!(d.find(EntryKind::Model, 1, 1, None, &session).is_err());
    let mut repeated = entries;
    repeated[1].id = 0;
    assert!(Directory::read(&bare(&repeated, 35), l, ObjectIntegrity::None, 35, &session).is_err());
}

#[test]
fn next_model_lifetime_is_bound_without_an_extra_directory_field() {
    let layout = TileLayout::new(1, 1, 1, 1, TileOrder::Raster).unwrap();
    let root = Entry {
        kind: EntryKind::Model,
        role: 1,
        id: 0,
        record_len: 10,
    };
    let next = Entry {
        kind: EntryKind::NextModel,
        id: u64::MAX,
        record_len: 20,
        ..root
    };
    let entries = [
        root,
        Entry {
            kind: EntryKind::TileBegin,
            role: 0,
            id: 0,
            record_len: 3,
        },
        next,
        Entry {
            kind: EntryKind::Ignorable,
            role: 0,
            id: 0,
            record_len: 2,
        },
        Entry {
            kind: EntryKind::Unit,
            role: 1,
            id: 0,
            record_len: 7,
        },
        Entry {
            kind: EntryKind::TileEnd,
            role: 0,
            id: 0,
            record_len: 5,
        },
    ];
    let body = bare(&entries, 47);
    let session = DecodeSession::new(DecodePolicy::default());
    let directory = Directory::read(&body, layout, ObjectIntegrity::None, 47, &session).unwrap();
    let found = directory
        .find(EntryKind::Model, 1, u64::MAX, None, &session)
        .unwrap();
    assert_eq!(found.entry.kind, EntryKind::NextModel);
    let mut bound = 0;
    directory
        .visit_bindings(&session, |entry, _, _, model| {
            if let Some(model) = model {
                assert_eq!(model, next);
                assert_eq!(entry.entry, entries[4]);
                bound += 1;
            }
            Ok(true)
        })
        .unwrap();
    assert_eq!(bound, 1);
    let mut model_body = alloc::vec![1];
    crate::put_varint(&mut model_body, u64::MAX);
    let lifetime_at = model_body.len();
    model_body.extend_from_slice(&[1, 0, 0, 3, b'r', b'a', b'w']); // next main 0, opaque model
    fn object(body: &[u8]) -> Object<'_> {
        Object {
            kind: 63,
            body,
            encoded_len: 20,
            integrity_verified: false,
        }
    }
    assert_eq!(
        found.bind(object(&model_body)).unwrap(),
        &[1, 0, 0, 3, b'r', b'a', b'w']
    );
    model_body[lifetime_at] = 0;
    assert_eq!(
        found.bind(object(&model_body)),
        Err(Error::Corrupt("V4 directory/frame identity mismatch"))
    );

    for id in [0, 127, 128, 16383, 16384, u32::MAX as u64 + 1, u64::MAX] {
        let mut a = [0; 40];
        let mut b = [0; 40];
        let payload = Entry { id, ..root };
        let transient = Entry {
            kind: EntryKind::NextModel,
            ..payload
        };
        let count = payload.encode(&mut a);
        assert_eq!(count, transient.encode(&mut b));
        assert_eq!(a[0], 1);
        assert_eq!(b[0], 5);
        assert_eq!(&a[1..count], &b[1..count]);
    }
    let mut wrong = entries;
    wrong[2].id = 0;
    assert!(
        Directory::read(
            &bare(&wrong, 47),
            layout,
            ObjectIntegrity::None,
            47,
            &session
        )
        .is_err()
    );
    let mut wrong = entries;
    wrong[4].role = 0;
    assert!(
        Directory::read(
            &bare(&wrong, 47),
            layout,
            ObjectIntegrity::None,
            47,
            &session
        )
        .is_err()
    );
    let mut wrong = entries.to_vec();
    wrong.insert(2, Entry { id: 1, ..next });
    assert!(
        Directory::read(
            &bare(&wrong, 67),
            layout,
            ObjectIntegrity::None,
            67,
            &session
        )
        .is_err()
    );
    let mut wrong = entries.to_vec();
    wrong.remove(4);
    assert!(
        Directory::read(
            &bare(&wrong, 40),
            layout,
            ObjectIntegrity::None,
            40,
            &session
        )
        .is_err()
    );
    let mut wrong = entries.to_vec();
    wrong.insert(5, Entry { role: 2, ..root });
    assert!(
        Directory::read(
            &bare(&wrong, 57),
            layout,
            ObjectIntegrity::None,
            57,
            &session
        )
        .is_err()
    );
}

#[test]
fn truncation_counts_limits_cancellation_and_failed_writer_are_explicit() {
    let (records, body, _) = fixture();
    for n in 0..body.len() {
        let session = DecodeSession::new(DecodePolicy::default());
        assert!(
            Directory::read(
                &body[..n],
                layout(),
                ObjectIntegrity::ObjectsAndTiles,
                records.len() as u64,
                &session
            )
            .is_err()
        );
    }
    let session =
        DecodeSession::new(DecodePolicy::default().with_max_input_bytes(body.len() as u64 - 1));
    assert!(matches!(
        Directory::read(
            &body,
            layout(),
            ObjectIntegrity::ObjectsAndTiles,
            records.len() as u64,
            &session
        ),
        Err(Error::TooLarge)
    ));
    let session = DecodeSession::with_cancel(DecodePolicy::default(), || true);
    assert!(matches!(
        Directory::read(
            &body,
            layout(),
            ObjectIntegrity::ObjectsAndTiles,
            records.len() as u64,
            &session
        ),
        Err(Error::Cancelled)
    ));
    let session = DecodeSession::new(DecodePolicy::default().with_max_work_units(5));
    assert!(matches!(
        Directory::read(
            &body,
            layout(),
            ObjectIntegrity::ObjectsAndTiles,
            records.len() as u64,
            &session
        ),
        Err(Error::TooLarge)
    ));
    let mut w = DirectoryWriter::new(layout(), ObjectIntegrity::None);
    let mut buffer = [0; 40];
    assert!(
        w.push(
            Entry {
                kind: EntryKind::TileEnd,
                role: 0,
                id: 0,
                record_len: 3
            },
            &mut buffer
        )
        .is_err()
    );
    assert!(w.finish().is_err());
    assert!(
        w.push(
            Entry {
                kind: EntryKind::TileBegin,
                role: 0,
                id: 0,
                record_len: 3
            },
            &mut buffer
        )
        .is_err()
    );
}

#[test]
fn every_registry_unit_kind_binds_to_the_specified_plane_role() {
    // Explicit V3 registry assignments; role ordering is DC, AC, alpha, exact.
    for (kind, role) in [(51, 0), (53, 2), (55, 1), (57, 3), (59, 0)] {
        let body = [7, 0xaa, 0xbb];
        let object = Object {
            kind,
            body: &body,
            encoded_len: 5,
            integrity_verified: false,
        };
        let mut entry = LocatedEntry {
            entry: Entry {
                kind: EntryKind::Unit,
                role,
                id: 7,
                record_len: 5,
            },
            offset: 0,
            tile_id: Some(0),
        };
        assert_eq!(entry.bind(object).unwrap(), [0xaa, 0xbb]);
        entry.entry.role = (role + 1) % 4;
        assert!(entry.bind(object).is_err());
    }
}
