use imij::planes::{ReadAt, Rect, SliceReader};
use imij::{
    Channels, FrameConfig, FrameDecoder, FrameDuration, FrameEncoder, ImageInfo, ImageReference,
    IndexedReference, Limits, Metadata, SampleType, SequenceEncoder, SequenceItem, SequenceKind,
    decode_indexed_nested,
};
use imij_set::{
    EntryMetadata, HashAlgorithm, Reader, SequenceIndex, SequenceIndexRef, Summary, Value, Writer,
};
use std::cell::RefCell;
use std::rc::Rc;

fn image() -> Vec<u8> {
    image_with_fillers(0)
}

fn image_with_fillers(fillers: u8) -> Vec<u8> {
    imij::add_pixel_checksum(image_with_fillers_raw(fillers)).unwrap()
}

fn image_with_fillers_raw(fillers: u8) -> Vec<u8> {
    image_with_reference_hash(fillers, HashAlgorithm::DEFAULT)
}

fn image_with_reference_hash(fillers: u8, algorithm: HashAlgorithm) -> Vec<u8> {
    let info = ImageInfo::new(129, 33, Channels::Rgb, SampleType::U8);
    let mut state = 711u32;
    let first: Vec<u8> = (0..info.checked_buffer_len().unwrap())
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect();
    let mut changed = first.clone();
    changed[20] ^= 1;
    let mut encoder = SequenceEncoder::new(
        SequenceKind::Pages,
        FrameConfig::new({
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.with_effort(4.0);
            cfg = cfg.with_threads(1);
            cfg.with_hash_algorithm(algorithm)
        }),
    );
    for (index, pixels) in [&first, &first, &changed].into_iter().enumerate() {
        let mut item = SequenceItem::new(pixels, info.row_bytes(), &info);
        if index == 2 {
            item = item.with_reference(ImageReference {
                index: 0,
                pixels: &first,
                stride: info.row_bytes(),
            });
        }
        encoder.push(item).unwrap();
    }
    for index in 0..fillers {
        let pixels: Vec<u8> = first
            .iter()
            .enumerate()
            .map(|(at, &value)| {
                value
                    .wrapping_add(index.wrapping_mul(29))
                    .rotate_left((at % 7) as u32)
            })
            .collect();
        encoder
            .push(SequenceItem::new(&pixels, info.row_bytes(), &info))
            .unwrap();
    }
    encoder.finish().unwrap()
}
fn archive(image: &[u8]) -> (Vec<u8>, SequenceIndexRef) {
    archive_with_hash(image, HashAlgorithm::DEFAULT)
}

fn archive_with_hash(image: &[u8], algorithm: HashAlgorithm) -> (Vec<u8>, SequenceIndexRef) {
    let mut writer = Writer::new(Vec::new())
        .with_hash_algorithm(algorithm)
        .unwrap();
    let reference = writer
        .insert_sequence_index(10, image, &Limits::DEFAULT)
        .unwrap();
    let mut meta = EntryMetadata::default();
    meta.filename = Some("variants.imij".into());
    meta.attributes
        .insert("group".into(), Value::Text("original".into()));
    reference.write_metadata(&mut meta).unwrap();
    writer
        .insert_with_metadata(
            20,
            image.len() as u64,
            0,
            Summary::default(),
            &meta,
            &mut &image[..],
        )
        .unwrap();
    (writer.finish().unwrap().0, reference)
}

#[cfg(any(feature = "blake3", feature = "sha2"))]
fn check_wide_sequence(algorithm: HashAlgorithm) {
    let image = image_with_reference_hash(0, algorithm);
    let mut native = FrameDecoder::new(&image).unwrap();
    let full_digest = native.frame_seek_info(2).unwrap().reference_hash.unwrap();
    assert_eq!(full_digest.algorithm(), algorithm);
    let info = native.info().clone();
    let mut pristine = vec![0; info.checked_buffer_len().unwrap()];
    native.seek_frame(0).unwrap();
    native
        .decode_into(&mut pristine, info.row_bytes(), 1)
        .unwrap();
    let mut expected = vec![0; pristine.len()];
    native.seek_frame(2).unwrap();
    native
        .decode_into_with_reference(
            &pristine,
            info.row_bytes(),
            &mut expected,
            info.row_bytes(),
            1,
        )
        .unwrap();

    // Wide reference hashes also work with the ordinary archive checksum.
    for archive_algorithm in [HashAlgorithm::DEFAULT, algorithm] {
        let (bytes, binding) = archive_with_hash(&image, archive_algorithm);
        let index = extracted(&bytes);
        assert_eq!(&index[..8], b"IMIJSEQ\x05");
        let suffix = algorithm.width() - 32;
        assert_eq!(usize::from(index[18]), suffix);
        assert_eq!(index.len(), 96 + 3 * (96 + suffix));
        assert_eq!(binding.checksum(), archive_algorithm.digest(&index));

        let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
        let (upgraded, _) =
            imij_set::upgrade(&mut reader, Vec::new(), &Default::default()).unwrap();
        for file in [&bytes, &upgraded] {
            let mut reader = Reader::open(SliceReader::new(file)).unwrap();
            let image_entry = reader.get(20).unwrap().unwrap();
            let index_entry = reader.get(10).unwrap().unwrap();
            let binding = SequenceIndexRef::from_metadata(image_entry.metadata())
                .unwrap()
                .unwrap();
            assert_eq!(binding.checksum(), index_entry.checksum());
            assert_eq!(
                binding.checksum().as_bytes().len(),
                archive_algorithm.width()
            );
            {
                let mut index = binding.open(&mut reader, &image_entry).unwrap();
                assert!(index.get(1).unwrap().reference_hash.is_none());
                assert_eq!(index.get(2).unwrap().reference_hash, Some(full_digest));
            }
            assert_eq!(
                reader
                    .decode_indexed_item_verified(&image_entry, 2, 2, 1 << 20, &Limits::DEFAULT, 1)
                    .unwrap(),
                (info.clone(), expected.clone())
            );
        }

        // The 32-byte check covers the full record, including its suffix.
        let mut damaged = index;
        damaged[96 + 2 * (96 + suffix) + 64] ^= 1;
        let mut reader = SequenceIndex::open(
            SliceReader::new(&damaged),
            image.len() as u64,
            archive_algorithm.digest(&image),
        )
        .unwrap();
        assert!(reader.get(1).is_ok());
        assert!(reader.get(2).is_err());
    }
}

#[cfg(feature = "blake3")]
#[test]
fn indexed_sequences_preserve_blake3_reference_suffixes() {
    check_wide_sequence(HashAlgorithm::Blake3_512);
    check_wide_sequence(HashAlgorithm::Blake3_1024);
}

#[cfg(feature = "sha2")]
#[test]
fn indexed_sequences_preserve_sha512_reference_suffixes() {
    check_wide_sequence(HashAlgorithm::Sha512);
}
fn extracted(bytes: &[u8]) -> Vec<u8> {
    let mut reader = Reader::open(SliceReader::new(bytes)).unwrap();
    let entry = reader.get(10).unwrap().unwrap();
    let mut out = Vec::new();
    reader.copy_to(&entry, &mut out).unwrap();
    out
}

#[test]
fn optional_index_matches_native_locations_and_survives_compaction() {
    let image = image();
    let (bytes, reference) = archive(&image);
    let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
    let (flat, _) = imij_set::compact(&mut reader, Vec::new()).unwrap();
    for bytes in [&bytes, &flat] {
        let mut reader = Reader::open(SliceReader::new(bytes)).unwrap();
        let entry = reader.get(20).unwrap().unwrap();
        assert_eq!(entry.metadata().filename.as_deref(), Some("variants.imij"));
        assert_eq!(
            SequenceIndexRef::from_metadata(entry.metadata()).unwrap(),
            Some(reference)
        );
        let index_entry = reader.get(reference.key()).unwrap().unwrap();
        assert_eq!(index_entry.len(), reference.encoded_len());
        assert_eq!(index_entry.checksum(), reference.checksum());
        let mut recovered = Vec::new();
        reader.copy_to(&entry, &mut recovered).unwrap();
        assert_eq!(recovered, image);
        let native = FrameDecoder::new(&image).unwrap();
        let mut index = reference.open(&mut reader, &entry).unwrap();
        assert_eq!(index.image_count(), 3);
        for ordinal in [2, 0, 1, 2] {
            let got = index.get(ordinal).unwrap();
            let expected = native.frame_seek_info(ordinal as usize).unwrap();
            assert_eq!(got.encoded, expected.encoded);
            assert_eq!(
                got.reference_index,
                expected.reference_index.map(|n| n as u64)
            );
            assert_eq!(got.reference_hash, expected.reference_hash);
        }
        assert_eq!(index.get(2).unwrap().reference_index, Some(0));
        assert!(index.get(3).is_err());
    }
}

#[test]
fn identity_binding_rejects_wrong_image_and_latest_replacement() {
    let image = image();
    let (mut bytes, reference) = archive(&image);
    let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
    let entry = reader.get(20).unwrap().unwrap();
    let snapshot = reader.snapshot().clone();
    let blob = extracted(&bytes);
    assert!(
        SequenceIndex::open(
            SliceReader::new(&blob),
            image.len() as u64 + 1,
            entry.checksum()
        )
        .is_err()
    );
    assert!(
        SequenceIndex::open(
            SliceReader::new(&blob),
            image.len() as u64,
            flipped(entry.checksum())
        )
        .is_err()
    );
    let start = bytes.len() as u64;
    let mut writer = Writer::append(&mut bytes, &snapshot, start).unwrap();
    let mut changed = blob;
    changed[40] ^= 1;
    writer
        .insert(
            10,
            changed.len() as u64,
            0,
            Summary::default(),
            &mut changed.as_slice(),
        )
        .unwrap();
    writer.finish().unwrap();
    let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
    let entry = reader.get(20).unwrap().unwrap();
    assert!(reference.open(&mut reader, &entry).is_err());
}

struct Counted<'a> {
    bytes: &'a [u8],
    reads: Rc<RefCell<Vec<(u64, usize)>>>,
}
impl ReadAt for Counted<'_> {
    fn len(&mut self) -> Result<u64, imij::Error> {
        Ok(self.bytes.len() as u64)
    }
    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), imij::Error> {
        self.reads.borrow_mut().push((offset, out.len()));
        SliceReader::new(self.bytes).read_exact_at(offset, out)
    }
}
#[test]
fn direct_seek_reads_only_the_header_and_requested_record() {
    let image = image();
    let (bytes, _) = archive(&image);
    let blob = extracted(&bytes);
    let reads = Rc::new(RefCell::new(Vec::new()));
    let source = Counted {
        bytes: &blob,
        reads: reads.clone(),
    };
    let mut index = SequenceIndex::open(
        source,
        image.len() as u64,
        imij::HashAlgorithm::DEFAULT.digest(&image),
    )
    .unwrap();
    assert_eq!(index.get(2).unwrap().index, 2);
    assert_eq!(&*reads.borrow(), &[(0, 96), (288, 96)]);
}

#[test]
fn indexed_nested_file_decodes_with_exact_reference_pixels() {
    let image = image_with_fillers(10);
    let (archive, binding) = archive(&image);
    let mut reader = Reader::open(SliceReader::new(&archive)).unwrap();
    let entry = reader.get(20).unwrap().unwrap();
    let location = {
        let mut index = binding.open(&mut reader, &entry).unwrap();
        index.get(2).unwrap()
    };
    let range = location.encoded.unwrap();
    let mut nested = vec![0; (range.end - range.start) as usize];
    reader
        .entry_source(&entry)
        .unwrap()
        .read_exact_at(range.start, &mut nested)
        .unwrap();
    let mut full = FrameDecoder::new(&image).unwrap();
    let info = full.info().clone();
    let mut pristine = vec![0; info.checked_buffer_len().unwrap()];
    full.seek_frame(0).unwrap();
    full.decode_into(&mut pristine, info.row_bytes(), 1)
        .unwrap();
    let mut primary =
        imij::region::RegionReader::open(reader.entry_source(&entry).unwrap()).unwrap();
    let mut ranged_pristine = vec![0; pristine.len()];
    primary
        .decode_region_threads(
            Rect::new(0, 0, info.width, info.height),
            &mut ranged_pristine,
            info.row_bytes(),
            1,
        )
        .unwrap();
    assert_eq!(ranged_pristine, pristine);
    assert!(primary.read_stats().bytes < image.len() as u64);
    let mut expected = vec![0; pristine.len()];
    full.seek_frame(2).unwrap();
    full.decode_into_with_reference(
        &pristine,
        info.row_bytes(),
        &mut expected,
        info.row_bytes(),
        1,
    )
    .unwrap();
    let mut actual = vec![0; pristine.len()];
    let reference = IndexedReference {
        index: location.reference_index.unwrap(),
        hash: location.reference_hash.unwrap(),
        info: &info,
        pixels: &pristine,
        stride: info.row_bytes(),
    };
    assert_eq!(
        decode_indexed_nested(
            &nested,
            &imij::header(&image).unwrap(),
            Some(reference),
            &mut actual,
            info.row_bytes(),
            1,
        )
        .unwrap(),
        info
    );
    assert_eq!(actual, expected);
    let mut wrong = vec![0; pristine.len()];
    assert!(
        decode_indexed_nested(
            &nested,
            &imij::header(&image).unwrap(),
            Some(IndexedReference {
                index: 0,
                hash: flipped(location.reference_hash.unwrap()),
                info: &info,
                pixels: &pristine,
                stride: info.row_bytes(),
            }),
            &mut wrong,
            info.row_bytes(),
            1,
        )
        .is_err()
    );
}

#[test]
fn indexed_item_fetches_and_decodes_only_its_dependency_chain() {
    let image = image_with_fillers_raw(10);
    let (archive_bytes, _) = archive(&image);
    let reads = Rc::new(RefCell::new(Vec::new()));
    let mut reader = Reader::open(Counted {
        bytes: &archive_bytes,
        reads: reads.clone(),
    })
    .unwrap();
    let entry = reader.get(20).unwrap().unwrap();
    let mut full = FrameDecoder::new(&image).unwrap();
    let info = full.info().clone();
    let mut pristine = vec![0; info.checked_buffer_len().unwrap()];
    full.seek_frame(0).unwrap();
    full.decode_into(&mut pristine, info.row_bytes(), 1)
        .unwrap();
    let mut expected = vec![0; pristine.len()];
    full.seek_frame(2).unwrap();
    full.decode_into_with_reference(
        &pristine,
        info.row_bytes(),
        &mut expected,
        info.row_bytes(),
        1,
    )
    .unwrap();
    let before = reads.borrow().len();
    let (got_info, got) = reader
        .decode_indexed_item(&entry, 2, 2, 1 << 20, &Limits::DEFAULT, 1)
        .unwrap();
    assert_eq!(got_info, info);
    assert_eq!(got, expected);
    let fetched: usize = reads.borrow()[before..].iter().map(|(_, len)| *len).sum();
    assert!(
        fetched < image.len(),
        "selected fetch must not copy the whole group"
    );
    let (_, independent) = reader
        .decode_indexed_item(&entry, 1, 1, 1 << 20, &Limits::DEFAULT, 1)
        .unwrap();
    assert_eq!(independent, pristine);
    assert!(
        reader
            .decode_indexed_item(&entry, 2, 1, 1 << 20, &Limits::DEFAULT, 1)
            .is_err(),
        "reference chain bound is enforced before image fetches"
    );
    let summed = image_with_fillers(0);
    let (archive_bytes, _) = archive(&summed);
    let mut reader = Reader::open(SliceReader::new(&archive_bytes)).unwrap();
    let entry = reader.get(20).unwrap().unwrap();
    assert!(
        reader
            .decode_indexed_item(&entry, 2, 2, 1 << 20, &Limits::DEFAULT, 1)
            .is_err(),
        "supported outer pixel checksums require complete verification"
    );
    assert_eq!(
        reader
            .decode_indexed_item_verified(&entry, 2, 2, 1 << 20, &Limits::DEFAULT, 1)
            .unwrap()
            .1,
        expected,
    );
    let mut damaged = archive_bytes.clone();
    let tag = summed.windows(4).position(|tag| tag == b"pSUM").unwrap();
    let length = usize::from(summed[tag + 4]) - 1;
    assert!(length < 127);
    damaged[entry.offset() as usize + tag + 4 + length] ^= 1;
    let mut reader = Reader::open(SliceReader::new(&damaged)).unwrap();
    let entry = reader.get(20).unwrap().unwrap();
    assert!(
        reader
            .decode_indexed_item_verified(&entry, 2, 2, 1 << 20, &Limits::DEFAULT, 1)
            .unwrap_err()
            .to_string()
            .contains("pixel checksum mismatch")
    );

    let info = ImageInfo::new(16, 16, Channels::Rgb, SampleType::U8);
    let pixels = vec![7; info.checked_buffer_len().unwrap()];
    let mut checksummed = SequenceEncoder::new(
        SequenceKind::Pages,
        FrameConfig::new({
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.with_checksum(true);
            cfg
        }),
    );
    for _ in 0..2 {
        checksummed
            .push(SequenceItem::new(&pixels, info.row_bytes(), &info))
            .unwrap();
    }
    let checksummed = checksummed.finish().unwrap();
    let (archive_bytes, _) = archive(&checksummed);
    let mut reader = Reader::open(SliceReader::new(&archive_bytes)).unwrap();
    let entry = reader.get(20).unwrap().unwrap();
    let primary = imij::region::RegionReader::open_with_limits(
        reader.entry_source(&entry).unwrap(),
        &Limits::DEFAULT.with_checksum_verification(false),
    )
    .unwrap();
    assert!(primary.is_unified_sequence());
    assert!(primary.has_file_checksum());
    drop(primary);
    assert!(
        reader
            .decode_indexed_item(
                &entry,
                1,
                1,
                1 << 20,
                &Limits::DEFAULT.with_checksum_verification(false),
                1,
            )
            .is_err(),
        "an outer cSUM declaration still needs complete verification when disabled by limits"
    );
    assert_eq!(
        reader
            .decode_indexed_item_verified(&entry, 1, 1, 1 << 20, &Limits::DEFAULT, 1)
            .unwrap()
            .1,
        pixels,
    );
    let point = FrameDecoder::new(&checksummed)
        .unwrap()
        .frame_seek_info(1)
        .unwrap();
    let mut damaged = archive_bytes.clone();
    damaged[(entry.offset() + point.encoded.unwrap().end - 1) as usize] ^= 1;
    let mut reader = Reader::open(SliceReader::new(&damaged)).unwrap();
    let entry = reader.get(20).unwrap().unwrap();
    assert!(
        reader
            .decode_indexed_item_verified(&entry, 1, 1, 1 << 20, &Limits::DEFAULT, 1)
            .unwrap_err()
            .to_string()
            .contains("checksum mismatch")
    );

    let (mut altered, _) = archive(&image);
    let mut reader = Reader::open(SliceReader::new(&altered)).unwrap();
    let entry = reader.get(20).unwrap().unwrap();
    let control_end = {
        let binding = SequenceIndexRef::from_metadata(entry.metadata())
            .unwrap()
            .unwrap();
        let mut index = binding.open(&mut reader, &entry).unwrap();
        index.get(2).unwrap().encoded.unwrap().start
    };
    altered[entry.offset() as usize + control_end as usize - 1] ^= 1;
    let mut reader = Reader::open(SliceReader::new(&altered)).unwrap();
    let entry = reader.get(20).unwrap().unwrap();
    assert!(
        reader
            .decode_indexed_item(&entry, 2, 2, 1 << 20, &Limits::DEFAULT, 1)
            .is_err(),
        "changed on-wire fRAM controls must not be overridden by the index"
    );
}

#[test]
fn indexed_item_uses_explicit_fram_offset_after_ancillary_chunk() {
    let mut image = image_with_fillers_raw(2);
    let point = FrameDecoder::new(&image)
        .unwrap()
        .frame_seek_info(2)
        .unwrap();
    let at = point.controls.unwrap().start as usize;
    let gap = b"tEXt\x02x";
    image.splice(at..at, gap.iter().copied());
    let native = FrameDecoder::new(&image).unwrap();
    let shifted = native.frame_seek_info(2).unwrap();
    assert_eq!(
        shifted.controls.as_ref().unwrap().start,
        (at + gap.len()) as u64
    );
    let (archive, binding) = archive(&image);
    let mut reader = Reader::open(SliceReader::new(&archive)).unwrap();
    let image_entry = reader.get(20).unwrap().unwrap();
    let mut index = binding.open(&mut reader, &image_entry).unwrap();
    let point = index.get(2).unwrap();
    assert_eq!(point.controls, shifted.controls);
    assert_eq!(point.encoded, shifted.encoded);
    let (_, decoded) = reader
        .decode_indexed_item(&image_entry, 2, 2, 1 << 20, &Limits::DEFAULT, 1)
        .unwrap();
    let (_, primary) = imij::decode(&image).unwrap();
    let mut expected = primary;
    expected[20] ^= 1;
    assert_eq!(decoded, expected);
}

#[test]
fn version_three_sequence_index_retains_its_nested_coordinates() {
    let image = image();
    let decoder = FrameDecoder::new(&image).unwrap();
    let (bytes, _) = archive(&image);
    let mut index = extracted(&bytes);
    index[7] = 3;
    repair_header(&mut index);
    for ordinal in 1..decoder.frame_count() {
        let start = (ordinal + 1) * 96;
        let span = decoder.frame_seek_info(ordinal).unwrap().encoded.unwrap();
        index[start..start + 8].copy_from_slice(&span.start.to_le_bytes());
        index[start + 58..start + 64].fill(0);
        repair_record(&mut index, ordinal);
    }
    let mut reader = SequenceIndex::open(
        SliceReader::new(&index),
        image.len() as u64,
        imij::HashAlgorithm::DEFAULT.digest(&image),
    )
    .unwrap();
    for ordinal in 0..decoder.frame_count() {
        let point = reader.get(ordinal as u64).unwrap();
        assert_eq!(point.controls, None);
        assert_eq!(
            point.encoded,
            decoder.frame_seek_info(ordinal).unwrap().encoded
        );
    }
}

#[test]
fn verified_indexed_reads_support_v3_file_checksums() {
    let info = ImageInfo::new(16, 16, Channels::Gray, SampleType::U8);
    let mut encoder = SequenceEncoder::new(
        SequenceKind::Pages,
        FrameConfig::new(imij::ImiEncoderConfig::lossless().with_checksum(true)),
    );
    encoder
        .push(SequenceItem::new(&[3; 256], 16, &info))
        .unwrap();
    encoder
        .push(SequenceItem::new(&[19; 256], 16, &info))
        .unwrap();
    let image = encoder.finish_legacy().unwrap();
    let (bytes, _) = archive(&image);
    let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
    let entry = reader.get(20).unwrap().unwrap();
    assert_eq!(
        reader
            .decode_indexed_item_verified(&entry, 1, 1, 1 << 20, &Limits::DEFAULT, 1)
            .unwrap(),
        (info, vec![19; 256]),
    );
}

#[test]
fn v4_primary_open_does_not_visit_unselected_sequence_headers() {
    let image = image_with_fillers_raw(100);
    assert_eq!(imij::header(&image).unwrap().version, 4);
    let (bytes, _) = archive(&image);
    let reads = Rc::new(RefCell::new(Vec::new()));
    let mut reader = Reader::open(Counted {
        bytes: &bytes,
        reads: reads.clone(),
    })
    .unwrap();
    let entry = reader.get(20).unwrap().unwrap();
    let before = reads.borrow().len();
    let (info, selected) = reader
        .decode_indexed_item(&entry, 2, 2, 1 << 20, &Limits::DEFAULT, 1)
        .unwrap();
    let mut full = FrameDecoder::new(&image).unwrap();
    let mut pristine = vec![0; info.checked_buffer_len().unwrap()];
    full.seek_frame(0).unwrap();
    full.decode_into(&mut pristine, info.row_bytes(), 1)
        .unwrap();
    let mut expected = vec![0; pristine.len()];
    full.seek_frame(2).unwrap();
    full.decode_into_with_reference(
        &pristine,
        info.row_bytes(),
        &mut expected,
        info.row_bytes(),
        1,
    )
    .unwrap();
    assert_eq!(selected, expected);
    // The first 64 KiB region window may include later items. The root's
    // directory is also needed. Newly completed sequences keep it at the
    // front; retained tail-layout sequences need their final directory too.
    let tail_len = if &image[image.len() - 4..] == b"uIDX" {
        u64::from_le_bytes(image[image.len() - 12..image.len() - 4].try_into().unwrap())
    } else {
        0
    };
    let tail_at = entry.offset() + entry.len() - tail_len;
    assert!(
        reads.borrow()[before..]
            .iter()
            .all(|&(at, _)| at < entry.offset() + 65536 || at >= tail_at),
        "primary open walked unrelated later sequence headers: {:?}, image at {} length {}",
        &reads.borrow()[before..],
        entry.offset(),
        entry.len()
    );
}

#[test]
fn indexed_partial_decode_rejects_legacy_animation_semantics() {
    let info = ImageInfo::new(16, 16, Channels::Rgb, SampleType::U8);
    let mut encoder = FrameEncoder::animation(
        info.clone(),
        Metadata::default(),
        FrameConfig::new(imij::ImiEncoderConfig::default()),
    );
    let first = vec![12; info.checked_buffer_len().unwrap()];
    let second = vec![34; first.len()];
    for pixels in [&first, &second] {
        encoder
            .push_frame(pixels, info.row_bytes(), FrameDuration { num: 1, den: 30 })
            .unwrap();
    }
    let image = encoder.finish().unwrap();
    let (archive, _) = archive(&image);
    let mut reader = Reader::open(SliceReader::new(&archive)).unwrap();
    let entry = reader.get(20).unwrap().unwrap();
    assert!(
        reader
            .decode_indexed_item(&entry, 1, 1, 1 << 20, &Limits::DEFAULT, 1)
            .is_err(),
        "legacy animation may inherit color or use canvas-update semantics"
    );
}

/// The index's own algorithm (header byte 16).
fn index_algo(bytes: &[u8]) -> imij::HashAlgorithm {
    imij::HashAlgorithm::from_wire(bytes[16]).unwrap()
}
fn repair_header(bytes: &mut [u8]) {
    let digest = index_algo(bytes).digest(&bytes[..64]);
    bytes[64..96].copy_from_slice(&digest.to_slot());
}
fn repair_record(bytes: &mut [u8], ordinal: usize) {
    let start = 96 + ordinal * 96;
    let mut data = (ordinal as u64).to_le_bytes().to_vec();
    data.extend_from_slice(&bytes[start..start + 64]);
    let digest = index_algo(bytes).digest(&data);
    bytes[start + 64..start + 96].copy_from_slice(&digest.to_slot());
}
fn flipped(d: imij::Digest) -> imij::Digest {
    let mut b = d.as_bytes().to_vec();
    b[0] ^= 1;
    imij::Digest::from_bytes(d.algorithm(), &b).unwrap()
}
#[test]
fn malformed_indices_reject_before_using_coordinates() {
    let image = image();
    let (bytes, _) = archive(&image);
    let original = extracted(&bytes);
    let digest = imij::HashAlgorithm::DEFAULT.digest(&image);
    fn open(
        bytes: &[u8],
        length: u64,
        digest: imij::Digest,
    ) -> std::io::Result<SequenceIndex<SliceReader<'_>>> {
        SequenceIndex::open(SliceReader::new(bytes), length, digest)
    }
    for length in [0, 95, 96, original.len() - 1] {
        assert!(open(&original[..length], image.len() as u64, digest).is_err());
    }
    for count in [0u64, u64::MAX] {
        let mut changed = original.clone();
        changed[56..64].copy_from_slice(&count.to_le_bytes());
        repair_header(&mut changed);
        assert!(open(&changed, image.len() as u64, digest).is_err());
    }
    let mut trailing = original.clone();
    trailing.push(0);
    assert!(open(&trailing, image.len() as u64, digest).is_err());
    let mut swapped = original.clone();
    let (left, right) = swapped[192..].split_at_mut(96);
    left.swap_with_slice(right);
    assert!(
        open(&swapped, image.len() as u64, digest)
            .unwrap()
            .get(1)
            .is_err()
    );
    // Correct checksums cannot legalize forward dependencies, overflowed ranges,
    // reserved flags, malformed empty entries or a partial primary-file view.
    for (ordinal, field, value) in [
        (2, 16, 2u64),
        (2, 0, u64::MAX - 1),
        (2, 56, 4),
        (2, 0, u64::MAX),
        (0, 8, 1),
    ] {
        let mut changed = original.clone();
        let start = 96 + ordinal * 96 + field;
        changed[start..start + 8].copy_from_slice(&value.to_le_bytes());
        repair_record(&mut changed, ordinal);
        assert!(
            open(&changed, image.len() as u64, digest)
                .unwrap()
                .get(ordinal as u64)
                .is_err()
        );
    }
    let mut meta = EntryMetadata::default();
    assert!(SequenceIndexRef::from_metadata(&meta).unwrap().is_none());
    for value in [
        Value::Unsigned(10),
        Value::Bytes(vec![1; 24]),
        Value::Bytes(vec![2; 25]),
    ] {
        meta.attributes.insert("imij.sequence-index".into(), value);
        assert!(SequenceIndexRef::from_metadata(&meta).is_err());
    }
}

#[test]
fn container_upgrade_preserves_sequence_bindings_across_digest_generations() {
    let image = image();
    let mut writer = Writer::new(Vec::new())
        .with_hash_algorithm(imij::HashAlgorithm::Xxh3_64)
        .unwrap();
    let reference = writer
        .insert_sequence_index(10, &image, &Limits::DEFAULT)
        .unwrap();
    let mut metadata = EntryMetadata::default();
    reference.write_metadata(&mut metadata).unwrap();
    writer
        .insert_with_metadata(
            20,
            image.len() as u64,
            0,
            Summary::default(),
            &metadata,
            &mut image.as_slice(),
        )
        .unwrap();
    let (mut bytes, snapshot) = writer.finish().unwrap();
    let start = bytes.len() as u64;
    let mut writer = Writer::append(&mut bytes, &snapshot, start).unwrap();
    writer
        .insert(30, 6, 0, Summary::default(), &mut &b"opaque"[..])
        .unwrap();
    writer.finish().unwrap();
    let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
    let (upgraded, _) = imij_set::upgrade_container(&mut reader, Vec::new()).unwrap();
    let mut reader = Reader::open(SliceReader::new(&upgraded)).unwrap();
    let entry = reader.get(20).unwrap().unwrap();
    assert_eq!(
        SequenceIndexRef::from_metadata(entry.metadata()).unwrap(),
        Some(reference)
    );
    assert_eq!(
        reader.read_entry_bounded(&entry, image.len()).unwrap(),
        image
    );
    let native = FrameDecoder::new(&image).unwrap();
    let mut index = reference.open(&mut reader, &entry).unwrap();
    for ordinal in 0..3 {
        let cached = index.get(ordinal).unwrap();
        let expected = native.frame_seek_info(ordinal as usize).unwrap();
        assert_eq!(cached.encoded, expected.encoded);
        assert_eq!(
            cached.reference_index,
            expected.reference_index.map(|n| n as u64)
        );
        assert_eq!(cached.reference_hash, expected.reference_hash);
    }
}

#[test]
fn compaction_preserves_sequence_digest_identities_or_refuses_before_writing() {
    let image = image();
    let mut writer = Writer::new(Vec::new())
        .with_hash_algorithm(imij::HashAlgorithm::Xxh3_64)
        .unwrap();
    let reference = writer
        .insert_sequence_index(10, &image, &Limits::DEFAULT)
        .unwrap();
    let mut metadata = EntryMetadata::default();
    reference.write_metadata(&mut metadata).unwrap();
    writer
        .insert_with_metadata(
            20,
            image.len() as u64,
            0,
            Summary::default(),
            &metadata,
            &mut image.as_slice(),
        )
        .unwrap();
    let (mut bytes, mut snapshot) = writer.finish().unwrap();
    for mixed in [false, true] {
        if mixed {
            let start = bytes.len() as u64;
            let mut writer = Writer::append(&mut bytes, &snapshot, start).unwrap();
            writer
                .insert(25, 3, 0, Summary::default(), &mut &b"mix"[..])
                .unwrap();
            snapshot = writer.finish().unwrap().1;
        }
        let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
        let (compacted, _) = imij_set::compact(&mut reader, Vec::new()).unwrap();
        let mut reader = Reader::open(SliceReader::new(&compacted)).unwrap();
        let entry = reader.get(20).unwrap().unwrap();
        assert_eq!(
            reference.open(&mut reader, &entry).unwrap().image_count(),
            3
        );
    }
    let start = bytes.len() as u64;
    let mut writer = Writer::append(&mut bytes, &snapshot, start).unwrap();
    let other = writer
        .insert_sequence_index(30, &image, &Limits::DEFAULT)
        .unwrap();
    other.write_metadata(&mut metadata).unwrap();
    writer
        .insert_with_metadata(
            40,
            image.len() as u64,
            0,
            Summary::default(),
            &metadata,
            &mut image.as_slice(),
        )
        .unwrap();
    writer.finish().unwrap();
    let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
    let mut output = Vec::new();
    let error = imij_set::compact(&mut reader, &mut output).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
    assert!(output.is_empty());
    // A history-preserving upgrade has no single-algorithm restriction.
    let (upgraded, _) = imij_set::upgrade_container(&mut reader, Vec::new()).unwrap();
    let mut reader = Reader::open(SliceReader::new(&upgraded)).unwrap();
    for (key, reference) in [(20, reference), (40, other)] {
        let entry = reader.get(key).unwrap().unwrap();
        assert_eq!(
            reference.open(&mut reader, &entry).unwrap().image_count(),
            3
        );
    }
}

#[test]
fn full_upgrade_rebuilds_indexes_for_explicit_reference_images() {
    let image = image_with_fillers_raw(0);
    let (bytes, _) = archive(&image);
    let mut old = Reader::open(SliceReader::new(&bytes)).unwrap();
    let entry = old.get(20).unwrap().unwrap();
    let expected = old
        .decode_indexed_item(&entry, 2, 2, 1 << 20, &Limits::DEFAULT, 1)
        .unwrap();
    let (new, _) = imij_set::upgrade(&mut old, Vec::new(), &Default::default()).unwrap();
    let mut reader = Reader::open(SliceReader::new(&new)).unwrap();
    let entry = reader.get(20).unwrap().unwrap();
    let actual = reader
        .decode_indexed_item(&entry, 2, 2, 1 << 20, &Limits::DEFAULT, 1)
        .unwrap();
    assert_eq!(actual, expected);
    let reference = SequenceIndexRef::from_metadata(entry.metadata())
        .unwrap()
        .unwrap();
    let mut index = reference.open(&mut reader, &entry).unwrap();
    let location = index.get(2).unwrap();
    assert_eq!(location.reference_index, Some(0));
    assert_eq!(
        location.reference_hash,
        FrameDecoder::new(&image)
            .unwrap()
            .frame_seek_info(2)
            .unwrap()
            .reference_hash
    );
}
