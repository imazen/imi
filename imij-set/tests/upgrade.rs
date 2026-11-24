use imij::planes::SliceReader;
use imij::{FrameDecoder, HashAlgorithm};
use imij_set::{
    EntryMetadata, Reader, SequenceIndexRef, Summary, UpgradeOptions, Value, Writer, upgrade,
};

const OLD: &[u8] = include_bytes!("../../fuzz/seeds/30-seed-csum-mixed-pages.imi");
const OTHER: &[u8] = include_bytes!("../../fuzz/seeds/21-seed-pages-u8.imi");
fn legacy_index(image: &[u8], key: u64) -> (Vec<u8>, EntryMetadata) {
    legacy_index_version(image, key, 1)
}
fn legacy_index_version(image: &[u8], key: u64, version: u8) -> (Vec<u8>, EntryMetadata) {
    let native = FrameDecoder::new(image).unwrap();
    let hash = xxhash_rust::xxh3::xxh3_64;
    let mut blob = b"IMIJSEQ\x01".to_vec();
    blob[7] = version;
    for value in [image.len() as u64, hash(image), native.frame_count() as u64] {
        blob.extend_from_slice(&value.to_le_bytes());
    }
    blob.extend_from_slice(&hash(&blob).to_le_bytes());
    for index in 0..native.frame_count() {
        let point = native.frame_seek_info(index).unwrap();
        let mut record = Vec::new();
        let (at, len) = point
            .encoded
            .map_or((u64::MAX, 0), |r| (r.start, r.end - r.start));
        assert!(point.reference_hash.is_none());
        for value in [
            at,
            len,
            point.reference_index.map_or(u64::MAX, |n| n as u64),
            0,
            0,
        ] {
            record.extend_from_slice(&value.to_le_bytes());
        }
        if version == 2 {
            record.splice(32..32, [0; 8]);
        }
        let mut checked = (index as u64).to_le_bytes().to_vec();
        checked.extend_from_slice(&record);
        record.extend_from_slice(&hash(&checked).to_le_bytes());
        blob.extend_from_slice(&record);
    }
    let mut reference = vec![1];
    for value in [key, blob.len() as u64, hash(&blob)] {
        reference.extend_from_slice(&value.to_le_bytes());
    }
    let mut metadata = EntryMetadata::default();
    metadata.filename = Some("preserved.imi".into());
    metadata
        .attributes
        .insert("opaque.attribute".into(), Value::Bytes(vec![0, 255]));
    metadata
        .attributes
        .insert("imij.sequence-index".into(), Value::Bytes(reference));
    (blob, metadata)
}
fn archive(forward: bool) -> Vec<u8> {
    let index_key = if forward { 30 } else { 10 };
    let (index, metadata) = legacy_index(OLD, index_key);
    let mut writer = Writer::new(Vec::new())
        .with_hash_algorithm(HashAlgorithm::Xxh3_64)
        .unwrap();
    if !forward {
        writer
            .insert(
                index_key,
                index.len() as u64,
                0,
                Summary::default(),
                &mut index.as_slice(),
            )
            .unwrap();
    }
    writer
        .insert_with_metadata(
            20,
            OLD.len() as u64,
            5,
            Summary::default(),
            &metadata,
            &mut &OLD[..],
        )
        .unwrap();
    if forward {
        writer
            .insert(
                index_key,
                index.len() as u64,
                0,
                Summary::default(),
                &mut index.as_slice(),
            )
            .unwrap();
    }
    writer.finish().unwrap().0
}
fn check(bytes: &[u8], original: &[u8]) {
    let mut reader = Reader::open(SliceReader::new(bytes)).unwrap();
    let entry = reader.get(20).unwrap().unwrap();
    assert_eq!(entry.head_size(), 0);
    assert_eq!(entry.metadata().filename.as_deref(), Some("preserved.imi"));
    assert_eq!(
        entry.metadata().attributes["opaque.attribute"],
        Value::Bytes(vec![0, 255])
    );
    let image = reader.read_entry_bounded(&entry, 1 << 20).unwrap();
    assert_eq!(imij::header(&image).unwrap().version, 4);
    assert_eq!(
        imij::decode(&image).unwrap(),
        imij::decode(original).unwrap()
    );
    let reference = SequenceIndexRef::from_metadata(entry.metadata())
        .unwrap()
        .unwrap();
    let mut index = reference.open(&mut reader, &entry).unwrap();
    let mut native = FrameDecoder::new(&image).unwrap();
    let mut old = FrameDecoder::new(original).unwrap();
    assert_eq!(index.image_count(), old.frame_count() as u64);
    while let Some(frame) = native.next_frame().unwrap() {
        let old_frame = old.next_frame().unwrap().unwrap();
        assert_eq!(format!("{frame:?}"), format!("{old_frame:?}"));
        let info = native.frame_info().unwrap();
        let mut a = vec![0; info.checked_buffer_len().unwrap()];
        let mut b = a.clone();
        native.decode_into(&mut a, info.row_bytes(), 1).unwrap();
        old.decode_into(&mut b, info.row_bytes(), 1).unwrap();
        assert_eq!(a, b);
        let point = native.frame_seek_info(frame.index).unwrap();
        let cached = index.get(frame.index as u64).unwrap();
        assert_eq!(point.encoded, cached.encoded);
        assert_eq!(point.reference_hash, cached.reference_hash);
    }
}

#[test]
fn legacy_indexes_and_forward_references_rebind_to_upgraded_images() {
    for forward in [false, true] {
        let source = archive(forward);
        let mut reader = Reader::open(SliceReader::new(&source)).unwrap();
        let (output, _) = upgrade(&mut reader, Vec::new(), &UpgradeOptions::default()).unwrap();
        check(&output, OLD);
    }
}

#[test]
fn every_generation_keeps_its_own_images_and_sequence_bindings() {
    let mut source = archive(false);
    let snapshot = Reader::open(SliceReader::new(&source))
        .unwrap()
        .snapshot()
        .clone();
    let start = source.len() as u64;
    let mut writer = Writer::append(&mut source, &snapshot, start).unwrap();
    let reference = writer
        .insert_sequence_index(10, OTHER, &imij::Limits::DEFAULT)
        .unwrap();
    let mut meta = legacy_index(OTHER, 10).1;
    reference.write_metadata(&mut meta).unwrap();
    writer
        .insert_with_metadata(
            20,
            OTHER.len() as u64,
            0,
            Summary::default(),
            &meta,
            &mut &OTHER[..],
        )
        .unwrap();
    writer.finish().unwrap();
    let mut reader = Reader::open(SliceReader::new(&source)).unwrap();
    let (output, snapshot) = upgrade(&mut reader, Vec::new(), &UpgradeOptions::default()).unwrap();
    assert_eq!(snapshot.generation(), 2);
    check(&output, OTHER);
    let previous = u64::from_le_bytes(
        output[output.len() - 120..output.len() - 112]
            .try_into()
            .unwrap(),
    ) as usize;
    check(&output[..previous], OLD);
}

#[test]
fn limits_and_damaged_bindings_fail_without_committing_an_upgrade() {
    let source = archive(false);
    for entry_limit in [true, false] {
        let mut options = UpgradeOptions::default();
        if entry_limit {
            options.image_limits.max_encoded_bytes = OLD.len() - 1;
        } else {
            options.max_sequence_indexes = 0;
        }
        let mut reader = Reader::open(SliceReader::new(&source)).unwrap();
        let mut output = Vec::new();
        assert!(upgrade(&mut reader, &mut output, &options).is_err());
        assert!(output.is_empty());
    }
    let mut corrupt = source.clone();
    corrupt[0] ^= 1;
    let mut reader = Reader::open(SliceReader::new(&corrupt)).unwrap();
    let mut output = Vec::new();
    assert!(upgrade(&mut reader, &mut output, &UpgradeOptions::default()).is_err());
    assert!(output.is_empty());
}

#[test]
fn nested_archives_upgrade_images_and_keep_opaque_blobs() {
    let inner = archive(false);
    let mut writer = Writer::new(Vec::new());
    writer
        .insert(
            1,
            inner.len() as u64,
            0,
            Summary::default(),
            &mut inner.as_slice(),
        )
        .unwrap();
    writer
        .insert(2, 5, 2, Summary::default(), &mut &b"other"[..])
        .unwrap();
    let (source, _) = writer.finish().unwrap();
    let mut reader = Reader::open(SliceReader::new(&source)).unwrap();
    let (output, _) = upgrade(&mut reader, Vec::new(), &UpgradeOptions::default()).unwrap();
    let mut reader = Reader::open(SliceReader::new(&output)).unwrap();
    let entry = reader.get(1).unwrap().unwrap();
    check(&reader.read_entry_bounded(&entry, 1 << 20).unwrap(), OLD);
    let entry = reader.get(2).unwrap().unwrap();
    assert_eq!(entry.head_size(), 2);
    assert_eq!(reader.read_entry_bounded(&entry, 5).unwrap(), b"other");
    let mut options = UpgradeOptions::default();
    options.image_limits.max_depth = 0;
    let mut reader = Reader::open(SliceReader::new(&source)).unwrap();
    assert!(upgrade(&mut reader, Vec::new(), &options).is_err());
}

#[test]
fn archive_upgrade_keeps_opaque_blobs_with_wrong_footer_markers() {
    let mut writer = Writer::new(Vec::new());
    let mut payloads = Vec::new();
    for (length, version) in [(136, 99), (80, 99), (136, 1), (80, 2)] {
        let mut payload = [0x1a; 180];
        let marker = payload.len() - length;
        payload[marker..marker + 7].copy_from_slice(b"IMIJSET");
        payload[marker + 7] = version;
        writer
            .insert(
                payloads.len() as u64,
                payload.len() as u64,
                0,
                Summary::default(),
                &mut payload.as_slice(),
            )
            .unwrap();
        payloads.push(payload);
    }
    let (source, _) = writer.finish().unwrap();
    let mut reader = Reader::open(SliceReader::new(&source)).unwrap();
    let (output, _) = upgrade(&mut reader, Vec::new(), &UpgradeOptions::default()).unwrap();
    let mut reader = Reader::open(SliceReader::new(&output)).unwrap();
    for (index, expected) in payloads.into_iter().enumerate() {
        let entry = reader.get(index as u64).unwrap().unwrap();
        assert_eq!(
            reader.read_entry_bounded(&entry, expected.len()).unwrap(),
            expected
        );
    }
}

#[test]
fn transitional_v2_sequence_indexes_migrate() {
    let (index, metadata) = legacy_index_version(OLD, 10, 2);
    let mut writer = Writer::new(Vec::new())
        .with_hash_algorithm(HashAlgorithm::Xxh3_64)
        .unwrap();
    writer
        .insert(
            10,
            index.len() as u64,
            0,
            Summary::default(),
            &mut index.as_slice(),
        )
        .unwrap();
    writer
        .insert_with_metadata(
            20,
            OLD.len() as u64,
            0,
            Summary::default(),
            &metadata,
            &mut &OLD[..],
        )
        .unwrap();
    let (bytes, _) = writer.finish().unwrap();
    let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
    let (new, _) = upgrade(&mut reader, Vec::new(), &Default::default()).unwrap();
    check(&new, OLD);
}

#[test]
fn upgrade_rejects_retired_native_images_instead_of_copying_them() {
    for version in [2, 3] {
        let mut image = b"IMIJ\r\n\x87\n".to_vec();
        image.extend_from_slice(&[8, version, 5, 0x80, 0x80, 0x10, 2, 1, 1]);
        let mut writer = Writer::new(Vec::new());
        writer
            .insert(
                1,
                image.len() as u64,
                0,
                Summary::default(),
                &mut image.as_slice(),
            )
            .unwrap();
        let bytes = writer.finish().unwrap().0;
        let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
        let error = upgrade(&mut reader, Vec::new(), &UpgradeOptions::default()).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("native components"), "{error}");
    }
}
