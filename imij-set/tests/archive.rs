use imij::planes::{ReadAt, SliceReader};
use imij_set::{Reader, Summary, Writer};
use std::cell::Cell;
use std::io::{self, Cursor, Write};
use std::rc::Rc;

#[test]
fn append_latest_listing_compaction_and_byte_identity() {
    let mut w = Writer::new(Vec::new());
    for k in 0..2000u64 {
        let data = k.to_le_bytes();
        w.insert(k, 8, 2, Summary::default(), &mut data.as_slice())
            .unwrap();
    }
    let (mut bytes, snapshot) = w.finish().unwrap();
    let start = bytes.len() as u64;
    let mut w = Writer::append(&mut bytes, &snapshot, start).unwrap();
    w.insert(10, 7, 0, Summary::default(), &mut &b"changed"[..])
        .unwrap();
    w.insert_stream(5000, 0, Summary::default(), |out| {
        out.write_all(b"streamed")
    })
    .unwrap();
    w.finish().unwrap();
    let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
    let mut list = reader.listing().unwrap();
    let mut count = 0;
    while let Some(e) = list.next(&mut reader).unwrap() {
        let mut out = Vec::new();
        reader.copy_to(&e, &mut out).unwrap();
        assert_eq!(e.checksum(), imij::HashAlgorithm::DEFAULT.digest(&out));
        match e.key() {
            10 => assert_eq!(out, b"changed"),
            5000 => assert_eq!(out, b"streamed"),
            k => assert_eq!(out, k.to_le_bytes()),
        }
        count += 1;
    }
    assert_eq!(count, 2001);
    let mut suffix = reader.listing_from(1998).unwrap();
    for expected in [1998, 1999, 5000] {
        assert_eq!(suffix.next(&mut reader).unwrap().unwrap().key(), expected);
    }
    assert!(suffix.next(&mut reader).unwrap().is_none());
    let mut replacement = reader.listing_from(10).unwrap();
    let entry = replacement.next(&mut reader).unwrap().unwrap();
    assert_eq!(entry.key(), 10);
    assert_eq!(reader.read_entry_bounded(&entry, 7).unwrap(), b"changed");
    assert!(
        reader
            .listing_from(u64::MAX)
            .unwrap()
            .next(&mut reader)
            .unwrap()
            .is_none()
    );
    assert!(reader.get(2000).unwrap().is_none());
    let (flat, snapshot) = imij_set::compact(&mut reader, Vec::new()).unwrap();
    assert_eq!(snapshot.generation(), 1);
    let mut flat = Reader::open(SliceReader::new(&flat)).unwrap();
    for k in [0, 10, 255, 256, 1999, 5000] {
        let e = flat.get(k).unwrap().unwrap();
        let mut a = Vec::new();
        flat.copy_to(&e, &mut a).unwrap();
        let e = reader.get(k).unwrap().unwrap();
        let mut b = Vec::new();
        reader.copy_to(&e, &mut b).unwrap();
        assert_eq!(a, b);
    }
}

#[test]
fn recovery_torn_append_and_failed_writer() {
    let mut w = Writer::new(Vec::new());
    w.insert(7, 3, 0, Summary::default(), &mut &b"old"[..])
        .unwrap();
    let (original, snapshot) = w.finish().unwrap();
    let mut complete = original.clone();
    let start = complete.len() as u64;
    let mut w = Writer::append(&mut complete, &snapshot, start).unwrap();
    w.insert(9, 6, 0, Summary::default(), &mut &b"newest"[..])
        .unwrap();
    w.finish().unwrap();
    for end in original.len() + 1..complete.len() {
        let data = &complete[..end];
        let mut r = Reader::recover(SliceReader::new(data), data.len() as u64).unwrap();
        assert_eq!(r.snapshot().end(), original.len() as u64);
        assert!(r.get(7).unwrap().is_some());
        assert!(r.get(9).unwrap().is_none());
    }
    assert!(Reader::recover(SliceReader::new(&complete[..complete.len() - 1]), 1).is_err());
    let mut failed = original.clone();
    let start = failed.len() as u64;
    let mut w = Writer::append(&mut failed, &snapshot, start).unwrap();
    assert!(
        w.insert(8, 20, 0, Summary::default(), &mut &b"short"[..])
            .is_err()
    );
    assert!(
        w.insert(9, 0, 0, Summary::default(), &mut &b""[..])
            .is_err()
    );
    assert!(w.finish().is_err());
    assert!(Reader::recover(SliceReader::new(&failed), failed.len() as u64).is_ok());
}

#[test]
fn bounded_read_returns_only_verified_entry_bytes() {
    let mut writer = Writer::new(Vec::new());
    writer
        .insert(1, 4, 0, Summary::default(), &mut &b"data"[..])
        .unwrap();
    let (mut archive, _) = writer.finish().unwrap();
    let mut reader = Reader::open(SliceReader::new(&archive)).unwrap();
    let entry = reader.get(1).unwrap().unwrap();
    assert_eq!(reader.read_entry_bounded(&entry, 4).unwrap(), b"data");
    assert_eq!(
        reader.read_entry_bounded(&entry, 3).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );

    archive[entry.offset() as usize] ^= 1;
    let mut reader = Reader::open(SliceReader::new(&archive)).unwrap();
    assert!(reader.read_entry_bounded(&entry, 4).is_err());
}

#[test]
fn bounded_read_fetches_large_entry_in_one_positional_read() {
    let data = vec![0x5a; 150_000];
    let mut writer = Writer::new(Vec::new());
    writer
        .insert(
            1,
            data.len() as u64,
            0,
            Summary::default(),
            &mut data.as_slice(),
        )
        .unwrap();
    let (archive, _) = writer.finish().unwrap();
    let reads = Rc::new(Cell::new(0));
    let mut reader = Reader::open(Counted {
        bytes: &archive,
        reads: reads.clone(),
    })
    .unwrap();
    let entry = reader.get(1).unwrap().unwrap();
    let before = reads.get();
    assert_eq!(reader.read_entry_bounded(&entry, data.len()).unwrap(), data);
    assert_eq!(reads.get() - before, 1);
}

#[test]
fn empty_zero_length_and_corruption() {
    let (empty, _) = Writer::new(Vec::new()).finish().unwrap();
    let mut reader = Reader::open(SliceReader::new(&empty)).unwrap();
    assert!(reader.get(0).unwrap().is_none());
    assert!(
        reader
            .listing()
            .unwrap()
            .next(&mut reader)
            .unwrap()
            .is_none()
    );
    let mut w = Writer::new(Vec::new());
    w.insert(0, 0, 0, Summary::default(), &mut &b""[..])
        .unwrap();
    w.insert(1, 4, 0, Summary::default(), &mut &b"data"[..])
        .unwrap();
    assert!(
        w.insert(1, 0, 0, Summary::default(), &mut &b""[..])
            .is_err()
    );
    let (mut bytes, _) = w.finish().unwrap();
    let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
    let empty_entry = reader.get(0).unwrap().unwrap();
    assert!(empty_entry.is_empty());
    assert!(
        reader
            .read_entry_bounded(&empty_entry, 0)
            .unwrap()
            .is_empty()
    );
    let e = reader.get(1).unwrap().unwrap();
    bytes[e.offset() as usize] ^= 1;
    let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
    assert!(reader.copy_to(&e, &mut Vec::new()).is_err());
    let end = bytes.len();
    bytes[end - 1] ^= 1;
    assert!(Reader::open(SliceReader::new(&bytes)).is_err());
}

struct Counted<'a> {
    bytes: &'a [u8],
    reads: Rc<Cell<u64>>,
}
impl ReadAt for Counted<'_> {
    fn len(&mut self) -> Result<u64, imij::Error> {
        Ok(self.bytes.len() as u64)
    }
    fn read_exact_at(&mut self, at: u64, out: &mut [u8]) -> Result<(), imij::Error> {
        self.reads.set(self.reads.get() + 1);
        SliceReader::new(self.bytes).read_exact_at(at, out)
    }
}
#[test]
fn multiple_index_levels_and_bounded_lookup_reads() {
    let mut w = Writer::new(Vec::new());
    for k in 0..100_000 {
        w.insert(k, 1, 0, Summary::default(), &mut &[42][..])
            .unwrap();
    }
    let (bytes, _) = w.finish().unwrap();
    let reads = Rc::new(Cell::new(0));
    let mut r = Reader::open(Counted {
        bytes: &bytes,
        reads: reads.clone(),
    })
    .unwrap();
    assert_eq!(reads.get(), 1, "tail includes small root");
    for k in [0, 255, 256, 32767, 32768, 65535, 99999] {
        let before = reads.get();
        assert_eq!(r.get(k).unwrap().unwrap().key(), k);
        assert!(reads.get() - before <= 2, "one internal + one leaf fetch");
    }
    let mut list = r.listing().unwrap();
    let mut count = 0;
    while let Some(e) = list.next(&mut r).unwrap() {
        assert_eq!(e.key(), count);
        count += 1;
    }
    assert_eq!(count, 100_000);
}
#[test]
fn standalone_imij_roi_and_batched_offsets() {
    let info = imij::ImageInfo::new(7, 3, imij::Channels::Rgb, imij::SampleType::U8);
    let pixels = (0..63).map(|i| i * 3).collect::<Vec<u8>>();
    let file = imij::ImiEncoderConfig::default()
        .encode_samples(
            imij::ImageRef::new(&pixels, 21, &info),
            &imij::Metadata::default(),
        )
        .unwrap();
    let mut w = Writer::new(Vec::new());
    w.insert(1, 9, 0, Summary::default(), &mut &b"unrelated"[..])
        .unwrap();
    w.insert(
        2,
        file.len() as u64,
        0,
        Summary::default(),
        &mut file.as_slice(),
    )
    .unwrap();
    let (bytes, _) = w.finish().unwrap();
    let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
    let e = reader.get(2).unwrap().unwrap();
    let mut copy = Vec::new();
    reader.copy_to(&e, &mut copy).unwrap();
    assert_eq!(copy, file);
    let mut source = reader.entry_source(&e).unwrap();
    let mut a = [0; 3];
    let mut b = [0; 4];
    let mut reqs = [(0, a.as_mut_slice()), (3, b.as_mut_slice())];
    source.read_many_at(&mut reqs).unwrap();
    assert_eq!((reqs[0].0, reqs[1].0), (0, 3));
    assert_eq!(a, file[..3]);
    assert_eq!(b, file[3..7]);
    assert!(source.read_exact_at(file.len() as u64, &mut a).is_err());
    let mut region = imij::region::RegionReader::open(source).unwrap();
    let mut out = vec![0; 63];
    region
        .decode_region(imij::planes::Rect::new(0, 0, 7, 3), &mut out, 21)
        .unwrap();
    assert_eq!(out, pixels);
}

struct Failing {
    bytes: Vec<u8>,
    remaining: usize,
}
impl Write for Failing {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Err(io::Error::other("simulated disk full"));
        }
        let n = b.len().min(self.remaining);
        self.bytes.extend_from_slice(&b[..n]);
        self.remaining -= n;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[test]
fn failed_sink_does_not_commit() {
    let mut sink = Failing {
        bytes: Vec::new(),
        remaining: 3,
    };
    let mut w = Writer::new(&mut sink);
    assert!(
        w.insert(1, 6, 0, Summary::default(), &mut Cursor::new(b"abcdef"))
            .is_err()
    );
    assert!(w.finish().is_err());
    assert_eq!(sink.bytes, b"abc");
}

#[test]
fn corrupt_latest_root_recovers_previous_commit() {
    let mut writer = Writer::new(Vec::new());
    writer
        .insert(1, 3, 0, Summary::default(), &mut &b"old"[..])
        .unwrap();
    let (mut bytes, old) = writer.finish().unwrap();
    let start = bytes.len() as u64;
    let mut writer = Writer::append(&mut bytes, &old, start).unwrap();
    writer
        .insert(2, 3, 0, Summary::default(), &mut &b"new"[..])
        .unwrap();
    writer.finish().unwrap();
    let footer = bytes.len() - 136;
    let root = u64::from_le_bytes(bytes[footer + 24..footer + 32].try_into().unwrap()) as usize;
    bytes[root] ^= 1;
    assert!(Reader::open(SliceReader::new(&bytes)).is_err());
    let mut reader = Reader::recover(SliceReader::new(&bytes), bytes.len() as u64).unwrap();
    assert_eq!(reader.snapshot().end(), old.end());
    assert!(reader.get(1).unwrap().is_some());
    assert!(reader.get(2).unwrap().is_none());
}

#[test]
fn late_leaf_failure_is_sticky() {
    let mut writer = Writer::new(Vec::new());
    for key in 0..5000 {
        writer
            .insert(key, 1, 0, Summary::default(), &mut &[42][..])
            .unwrap();
    }
    let (mut bytes, _) = writer.finish().unwrap();
    let footer = bytes.len() - 136;
    let root = u64::from_le_bytes(bytes[footer + 24..footer + 32].try_into().unwrap()) as usize;
    assert_eq!(&bytes[root..root + 2], &[6, 1]);
    let child = root + 4 + 16 * 96;
    let leaf = u64::from_le_bytes(bytes[child + 8..child + 16].try_into().unwrap()) as usize;
    bytes[leaf] ^= 1;
    let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
    let mut list = reader.listing().unwrap();
    let mut completed = 0;
    loop {
        match list.next(&mut reader) {
            Ok(Some(_)) => completed += 1,
            Err(_) => break,
            Ok(None) => panic!("corruption was silently skipped"),
        }
    }
    assert_eq!(completed, 4095);
    assert!(list.next(&mut reader).is_err());
    assert!(list.next(&mut reader).is_err());
}

#[test]
fn dimension_pages_prune_and_latest_replacements_hide_old_matches() {
    let mut writer = Writer::new(Vec::new());
    for key in 0..1024u64 {
        let mut summary = Summary::default();
        summary.width = if key == 800 || key == 801 {
            40
        } else if key < 256 {
            10
        } else if key < 512 {
            20
        } else {
            30
        };
        summary.height = 7;
        writer.insert(key, 1, 0, summary, &mut &b"x"[..]).unwrap();
    }
    let (mut archive, snapshot) = writer.finish().unwrap();
    let old_footer = archive.len() - 136;
    let old_root = u64::from_le_bytes(
        archive[old_footer + 24..old_footer + 32]
            .try_into()
            .unwrap(),
    ) as usize;
    assert_eq!(&archive[old_root..old_root + 2], &[6, 1]);
    let first_leaf =
        u64::from_le_bytes(archive[old_root + 12..old_root + 20].try_into().unwrap()) as usize;
    let start = archive.len() as u64;
    let mut writer = Writer::append(&mut archive, &snapshot, start).unwrap();
    let mut nonmatching = Summary::default();
    nonmatching.width = 2;
    nonmatching.height = 7;
    writer
        .insert(800, 1, 0, nonmatching, &mut &b"y"[..])
        .unwrap();
    let mut matching = Summary::default();
    matching.width = 40;
    matching.height = 7;
    writer.insert(1024, 1, 0, matching, &mut &b"z"[..]).unwrap();
    writer.finish().unwrap();

    // A disjoint old page is not requested. Its corruption still fails a
    // complete listing, whereas the selective query touches only candidates.
    archive[first_leaf] ^= 1;
    let mut reader = Reader::open(SliceReader::new(&archive)).unwrap();
    let mut query = reader.dimension_listing(40..=40, 7..=7).unwrap();
    assert_eq!(query.next(&mut reader).unwrap().unwrap().key(), 1024);
    assert_eq!(query.next(&mut reader).unwrap().unwrap().key(), 801);
    assert!(query.next(&mut reader).unwrap().is_none());
    let empty_min = 40;
    let empty_max = 39;
    assert!(
        reader
            .dimension_listing(empty_min..=empty_max, 7..=7)
            .is_err()
    );
    assert!(reader.listing().is_err());
}

#[test]
fn selective_dimension_query_reads_fewer_index_pages_than_a_full_listing() {
    let mut writer = Writer::new(Vec::new());
    for key in 0..100_000u64 {
        let mut summary = Summary::default();
        summary.width = if key < 256 { 512 } else { 840 };
        summary.height = 940;
        writer.insert(key, 1, 0, summary, &mut &b"x"[..]).unwrap();
    }
    let (archive, _) = writer.finish().unwrap();
    let reads = Rc::new(Cell::new(0));
    let mut reader = Reader::open(Counted {
        bytes: &archive,
        reads: reads.clone(),
    })
    .unwrap();
    let before = reads.get();
    let mut query = reader.dimension_listing(512..=512, 940..=940).unwrap();
    let mut query_count = 0;
    while let Some(entry) = query.next(&mut reader).unwrap() {
        assert_eq!(entry.key(), query_count);
        query_count += 1;
    }
    assert_eq!(query_count, 256);
    let query_reads = reads.get() - before;
    let before = reads.get();
    let mut listing = reader.listing().unwrap();
    let mut full_count = 0;
    while listing.next(&mut reader).unwrap().is_some() {
        full_count += 1;
    }
    assert_eq!(full_count, 100_000);
    assert!(query_reads < reads.get() - before);
}

#[test]
fn selective_cicp_query_prunes_and_latest_replacements_hide_old_matches() {
    let mut writer = Writer::new(Vec::new());
    for key in 0..100_000u64 {
        let mut summary = Summary::default();
        summary.cicp = if key < 256 {
            Some(imij::Cicp::SRGB)
        } else if key < 512 {
            None
        } else {
            Some(imij::Cicp::BT2100_PQ)
        };
        writer.insert(key, 1, 0, summary, &mut &b"x"[..]).unwrap();
    }
    let (mut archive, snapshot) = writer.finish().unwrap();
    let reads = Rc::new(Cell::new(0));
    let mut reader = Reader::open(Counted {
        bytes: &archive,
        reads: reads.clone(),
    })
    .unwrap();
    let before = reads.get();
    let mut query = reader.cicp_listing(Some(imij::Cicp::SRGB)).unwrap();
    for key in 0..256 {
        assert_eq!(query.next(&mut reader).unwrap().unwrap().key(), key);
    }
    assert!(query.next(&mut reader).unwrap().is_none());
    let query_reads = reads.get() - before;
    let before = reads.get();
    let mut listing = reader.listing().unwrap();
    let mut count = 0;
    while listing.next(&mut reader).unwrap().is_some() {
        count += 1;
    }
    assert_eq!(count, 100_000);
    assert!(query_reads < reads.get() - before);

    let start = archive.len() as u64;
    let mut writer = Writer::append(&mut archive, &snapshot, start).unwrap();
    let mut pq = Summary::default();
    pq.cicp = Some(imij::Cicp::BT2100_PQ);
    writer.insert(10, 1, 0, pq, &mut &b"p"[..]).unwrap();
    writer.insert(300, 1, 0, pq, &mut &b"p"[..]).unwrap();
    let mut srgb = Summary::default();
    srgb.cicp = Some(imij::Cicp::SRGB);
    writer.insert(100_000, 1, 0, srgb, &mut &b"s"[..]).unwrap();
    writer.finish().unwrap();
    let mut reader = Reader::open(SliceReader::new(&archive)).unwrap();
    let mut query = reader.cicp_listing(Some(imij::Cicp::SRGB)).unwrap();
    assert_eq!(query.next(&mut reader).unwrap().unwrap().key(), 100_000);
    for key in (0..256).filter(|&key| key != 10) {
        assert_eq!(query.next(&mut reader).unwrap().unwrap().key(), key);
    }
    assert!(query.next(&mut reader).unwrap().is_none());
    let mut query = reader.cicp_listing(None).unwrap();
    for key in (256..512).filter(|&key| key != 300) {
        assert_eq!(query.next(&mut reader).unwrap().unwrap().key(), key);
    }
    assert!(query.next(&mut reader).unwrap().is_none());
}

#[test]
fn exact_filename_query_prunes_and_checks_colliding_prefixes_and_replacements() {
    use imij_set::EntryMetadata;
    let target = "variants/longname-A.imij";
    let collision = "variants/longname-B.imij";
    assert_eq!(&target.as_bytes()[..16], &collision.as_bytes()[..16]);
    let mut writer = Writer::new(Vec::new());
    for key in 0..100_000u64 {
        let mut metadata = EntryMetadata::default();
        metadata.filename = Some(
            if key < 256 {
                target
            } else if key < 512 {
                collision
            } else {
                "other/image.imij"
            }
            .into(),
        );
        writer
            .insert_with_metadata(key, 1, 0, Summary::default(), &metadata, &mut &b"x"[..])
            .unwrap();
    }
    let (mut archive, snapshot) = writer.finish().unwrap();
    let reads = Rc::new(Cell::new(0));
    let mut reader = Reader::open(Counted {
        bytes: &archive,
        reads: reads.clone(),
    })
    .unwrap();
    let before = reads.get();
    let mut query = reader.filename_listing(target).unwrap();
    for key in 0..256 {
        assert_eq!(query.next(&mut reader).unwrap().unwrap().key(), key);
    }
    assert!(query.next(&mut reader).unwrap().is_none());
    let query_reads = reads.get() - before;
    let before = reads.get();
    let mut listing = reader.listing().unwrap();
    let mut count = 0;
    while listing.next(&mut reader).unwrap().is_some() {
        count += 1;
    }
    assert_eq!(count, 100_000);
    assert!(query_reads < reads.get() - before);
    for invalid in ["", "bad\0name", &"x".repeat(4097)] {
        assert_eq!(
            reader.filename_listing(invalid).err().unwrap().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    let start = archive.len() as u64;
    let mut writer = Writer::append(&mut archive, &snapshot, start).unwrap();
    let mut metadata = EntryMetadata::default();
    metadata.filename = Some(collision.into());
    writer
        .insert_with_metadata(10, 1, 0, Summary::default(), &metadata, &mut &b"y"[..])
        .unwrap();
    metadata.filename = Some(target.into());
    writer
        .insert_with_metadata(100_000, 1, 0, Summary::default(), &metadata, &mut &b"z"[..])
        .unwrap();
    writer.finish().unwrap();
    let mut reader = Reader::open(SliceReader::new(&archive)).unwrap();
    let mut query = reader.filename_listing(target).unwrap();
    assert_eq!(query.next(&mut reader).unwrap().unwrap().key(), 100_000);
    for key in (0..256).filter(|&key| key != 10) {
        assert_eq!(query.next(&mut reader).unwrap().unwrap().key(), key);
    }
    assert!(query.next(&mut reader).unwrap().is_none());
}

#[test]
fn indexed_metadata_roundtrips_replaces_filters_and_compacts() {
    use imij_set::{EntryMetadata, Value};
    let mut metadata = EntryMetadata::default();
    metadata.filename = Some("photos/猫.imij".into());
    metadata
        .attributes
        .insert("codec".into(), Value::Text("zenjpeg".into()));
    metadata
        .attributes
        .insert("quality".into(), Value::Float(72.5));
    metadata
        .attributes
        .insert("source".into(), Value::Bytes(vec![0, 255, 42]));
    metadata
        .attributes
        .insert("attempt".into(), Value::Unsigned(u64::MAX));
    metadata
        .attributes
        .insert("offset".into(), Value::Integer(i64::MIN));
    metadata
        .attributes
        .insert("selected".into(), Value::Bool(true));
    let mut summary = Summary::default();
    summary.width = 840;
    summary.height = 940;
    summary.cicp = Some(imij::Cicp::BT2100_PQ);
    summary.image_count = Some(1000);
    let mut w = Writer::new(Vec::new());
    w.insert_with_metadata(7, 4, 0, summary, &metadata, &mut &b"data"[..])
        .unwrap();
    w.insert_with_metadata(9, 4, 0, summary, &metadata, &mut &b"same"[..])
        .unwrap();
    let (mut bytes, snapshot) = w.finish().unwrap();
    let start = bytes.len() as u64;
    let mut w = Writer::append(&mut bytes, &snapshot, start).unwrap();
    // Replacement removes old metadata; a filter must not resurrect the old match.
    w.insert(7, 3, 0, Summary::default(), &mut &b"new"[..])
        .unwrap();
    w.finish().unwrap();
    let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
    let mut list = reader.listing().unwrap();
    let e = list
        .next_matching(&mut reader, |e| {
            e.metadata().filename.as_deref() == Some("photos/猫.imij")
        })
        .unwrap()
        .unwrap();
    assert_eq!(e.key(), 9);
    assert_eq!(e.summary(), summary);
    assert_eq!(e.metadata(), &metadata);
    let mut filenames = reader.filename_listing("photos/猫.imij").unwrap();
    assert_eq!(filenames.next(&mut reader).unwrap().unwrap().key(), 9);
    assert!(filenames.next(&mut reader).unwrap().is_none());
    assert!(
        list.next_matching(&mut reader, |e| e.metadata().filename.is_some())
            .unwrap()
            .is_none()
    );
    let (flat, _) = imij_set::compact(&mut reader, Vec::new()).unwrap();
    let mut flat = Reader::open(SliceReader::new(&flat)).unwrap();
    let mut filenames = flat.filename_listing("photos/猫.imij").unwrap();
    assert_eq!(filenames.next(&mut flat).unwrap().unwrap().key(), 9);
    assert!(filenames.next(&mut flat).unwrap().is_none());
    let e = flat.get(9).unwrap().unwrap();
    assert_eq!(e.summary(), summary);
    assert_eq!(e.metadata(), &metadata);
    let mut blob = Vec::new();
    flat.copy_to(&e, &mut blob).unwrap();
    assert_eq!(blob, b"same");
    assert_eq!(
        flat.get(7).unwrap().unwrap().metadata(),
        &EntryMetadata::default()
    );
}

#[test]
fn metadata_admission_precedes_output_and_pages_stay_bounded() {
    use imij_set::{EntryMetadata, Value};
    let mut metadata = EntryMetadata::default();
    metadata
        .attributes
        .insert("large".into(), Value::Bytes(vec![37; 15_000]));
    let mut w = Writer::new(Vec::new());
    let mut invalid = metadata.clone();
    invalid.filename = Some("x".repeat(4097));
    assert!(
        w.insert_stream_with_metadata(0, 0, Summary::default(), &invalid, |_| panic!(
            "invalid metadata reached callback"
        ))
        .is_err()
    );
    for k in 0..100u64 {
        w.insert_with_metadata(
            k,
            8,
            0,
            Summary::default(),
            &metadata,
            &mut k.to_le_bytes().as_slice(),
        )
        .unwrap();
    }
    let (bytes, _) = w.finish().unwrap();
    let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
    let mut list = reader.listing().unwrap();
    let mut ranges = Vec::new();
    for k in 0..100 {
        let e = list.next(&mut reader).unwrap().unwrap();
        assert_eq!(e.key(), k);
        assert_eq!(e.metadata(), &metadata);
        ranges.push((e.offset(), e.offset() + e.len()));
    }
    assert!(list.next(&mut reader).unwrap().is_none());
    // After the bounded tail read at open, refuse every read intersecting a blob.
    struct IndexOnly<'a> {
        bytes: &'a [u8],
        ranges: Vec<(u64, u64)>,
        armed: Rc<Cell<bool>>,
    }
    impl ReadAt for IndexOnly<'_> {
        fn len(&mut self) -> Result<u64, imij::Error> {
            Ok(self.bytes.len() as u64)
        }
        fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), imij::Error> {
            assert!(out.len() <= 65536);
            if self.armed.get() {
                assert!(
                    !self
                        .ranges
                        .iter()
                        .any(|&(start, end)| offset < end && offset + out.len() as u64 > start),
                    "listing fetched blob bytes"
                );
            }
            SliceReader::new(self.bytes).read_exact_at(offset, out)
        }
    }
    let armed = Rc::new(Cell::new(false));
    let mut r = Reader::open(IndexOnly {
        bytes: &bytes,
        ranges,
        armed: armed.clone(),
    })
    .unwrap();
    armed.set(true);
    let mut list = r.listing().unwrap();
    for k in 0..100 {
        assert_eq!(list.next(&mut r).unwrap().unwrap().key(), k);
    }
    assert!(list.next(&mut r).unwrap().is_none());
}

/// `get_many` equals per-key `get` across append segments (latest wins,
/// missing keys, duplicates) and reads each index level in one batch.
#[test]
fn get_many_matches_get_with_one_batch_per_level() {
    struct Counting<'a> {
        bytes: &'a [u8],
        batches: Rc<Cell<u32>>,
        reads: Rc<Cell<u32>>,
    }
    impl ReadAt for Counting<'_> {
        fn len(&mut self) -> Result<u64, imij::Error> {
            Ok(self.bytes.len() as u64)
        }
        fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), imij::Error> {
            self.reads.set(self.reads.get() + 1);
            SliceReader::new(self.bytes).read_exact_at(offset, out)
        }
        fn read_many_at(&mut self, reqs: &mut [(u64, &mut [u8])]) -> Result<(), imij::Error> {
            self.batches.set(self.batches.get() + 1);
            for (offset, out) in reqs.iter_mut() {
                SliceReader::new(self.bytes).read_exact_at(*offset, out)?;
            }
            Ok(())
        }
    }
    let mut w = Writer::new(Vec::new());
    for k in 0..100_000u64 {
        let data = [k as u8; 64];
        w.insert(k, 64, 0, Summary::default(), &mut data.as_slice())
            .unwrap();
    }
    let (mut bytes, snapshot) = w.finish().unwrap();
    let start = bytes.len() as u64;
    let mut w = Writer::append(&mut bytes, &snapshot, start).unwrap();
    for k in [10u64, 50_001, 99_999, 200_000] {
        w.insert(k, 3, 0, Summary::default(), &mut &b"new"[..])
            .unwrap();
    }
    w.finish().unwrap();
    // Four "view rows" of five neighbouring keys, replaced keys, keys absent
    // from every segment, and a duplicate.
    let mut keys: Vec<u64> = (0..4u64)
        .flat_map(|r| (0..5u64).map(move |c| 30_000 + r * 1_000 + c))
        .collect();
    keys.extend([10, 50_001, 99_999, 200_000, 150_000, u64::MAX, 31_002]);
    let (batches, reads) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
    let source = Counting {
        bytes: &bytes,
        batches: batches.clone(),
        reads: reads.clone(),
    };
    let mut reader = Reader::open(source).unwrap();
    let (b0, r0) = (batches.get(), reads.get());
    let many = reader.get_many(&keys).unwrap();
    let (many_batches, many_reads) = (batches.get() - b0, reads.get() - r0);
    let (b1, r1) = (batches.get(), reads.get());
    let one: Vec<_> = keys.iter().map(|&k| reader.get(k).unwrap()).collect();
    let one_ops = (batches.get() - b1) + (reads.get() - r1);
    assert_eq!(many, one);
    assert_eq!(
        many[keys.len() - 7].as_ref().unwrap().len(),
        3,
        "replaced key 10"
    );
    assert!(many[keys.len() - 3].is_none() && many[keys.len() - 2].is_none());
    // Two segments; each level of each one is at most one batch. The only
    // single read is the older segment's footer (`get` reads it too).
    assert_eq!(many_reads, 1, "{many_reads}");
    assert!(many_batches <= 8, "{many_batches}");
    assert!(many_batches < one_ops, "{many_batches} vs {one_ops}");
}

// Write a v1 footer around a current XXH3-64 page tree. Page grammar is
// selected by its kind, while v1 differs in footer fields and digest slots.
fn legacy_footer(mut bytes: Vec<u8>) -> Vec<u8> {
    let at = bytes.len() - 136;
    let footer = bytes.split_off(at);
    let mut old = b"IMIJSET\x01".to_vec();
    old.extend_from_slice(&((at + 80) as u64).to_le_bytes());
    old.extend_from_slice(&footer[16..40]);
    old.extend_from_slice(&footer[40..48]);
    old.extend_from_slice(&footer[72..96]);
    old.extend_from_slice(imij::HashAlgorithm::Xxh3_64.digest(&old).as_bytes());
    assert_eq!(old.len(), 80);
    bytes.extend_from_slice(&old);
    bytes
}

#[test]
fn mixed_digest_generations_and_legacy_footer_use_their_own_format() {
    for legacy in [false, true] {
        let mut writer = Writer::new(Vec::new())
            .with_hash_algorithm(imij::HashAlgorithm::Xxh3_64)
            .unwrap();
        // More than one leaf exercises branch and prefetched-leaf parsing.
        for key in 0..300 {
            let mut metadata = imij_set::EntryMetadata::default();
            metadata.filename = Some(format!("{key}.imi"));
            writer
                .insert_with_metadata(key, 1, 0, Summary::default(), &metadata, &mut &[7][..])
                .unwrap();
        }
        let (bytes, _) = writer.finish().unwrap();
        let mut bytes = if legacy { legacy_footer(bytes) } else { bytes };
        let reader = Reader::open(SliceReader::new(&bytes)).unwrap();
        let snapshot = reader.snapshot().clone();
        let start = bytes.len() as u64;
        let mut writer = Writer::append(&mut bytes, &snapshot, start).unwrap();
        writer
            .insert(299, 70000, 0, Summary::default(), &mut &[9; 70000][..])
            .unwrap();
        writer.finish().unwrap();
        for tail in [136, 1024 * 1024] {
            let mut reader = Reader::open_with_tail(SliceReader::new(&bytes), tail).unwrap();
            assert_eq!(
                reader.get(0).unwrap().unwrap().checksum().algorithm(),
                imij::HashAlgorithm::Xxh3_64
            );
            assert!(reader.get(301).unwrap().is_none());
            let entries = reader.get_many(&[0, 299, 301]).unwrap();
            assert_eq!(entries[0].as_ref().unwrap().key(), 0);
            assert_eq!(
                entries[1].as_ref().unwrap().checksum().algorithm(),
                imij::HashAlgorithm::DEFAULT
            );
            assert!(entries[2].is_none());
            let mut list = reader.listing().unwrap();
            for key in 0..300 {
                assert_eq!(list.next(&mut reader).unwrap().unwrap().key(), key);
            }
            assert!(list.next(&mut reader).unwrap().is_none());
            let mut filtered = reader.filename_listing("0.imi").unwrap();
            assert_eq!(filtered.next(&mut reader).unwrap().unwrap().key(), 0);
            assert!(filtered.next(&mut reader).unwrap().is_none());
            let mut filtered = reader.dimension_listing(0..=0, 0..=0).unwrap();
            let mut count = 0;
            while filtered.next(&mut reader).unwrap().is_some() {
                count += 1;
            }
            assert_eq!(count, 300);
            let mut filtered = reader.cicp_listing(None).unwrap();
            let mut count = 0;
            while filtered.next(&mut reader).unwrap().is_some() {
                count += 1;
            }
            assert_eq!(count, 300);
        }
    }
}

fn assert_same_generation(source: &[u8], upgraded: &[u8]) {
    let mut a = Reader::open(SliceReader::new(source)).unwrap();
    let mut b = Reader::open(SliceReader::new(upgraded)).unwrap();
    assert_eq!(a.snapshot().generation(), b.snapshot().generation());
    assert_eq!(a.snapshot().algorithm(), b.snapshot().algorithm());
    assert_eq!(b.snapshot().footer_len(), 136);
    let mut la = a.listing().unwrap();
    let mut lb = b.listing().unwrap();
    while let Some(ea) = la.next(&mut a).unwrap() {
        let eb = lb.next(&mut b).unwrap().unwrap();
        assert_eq!(ea.key(), eb.key());
        assert_eq!(ea.head_size(), eb.head_size());
        assert_eq!(ea.summary(), eb.summary());
        assert_eq!(ea.metadata(), eb.metadata());
        assert_eq!(ea.checksum(), eb.checksum());
        assert_eq!(
            a.read_entry_bounded(&ea, 1024).unwrap(),
            b.read_entry_bounded(&eb, 1024).unwrap()
        );
    }
    assert!(lb.next(&mut b).unwrap().is_none());
}

#[test]
fn upgrade_preserves_generation_history_metadata_and_digest_identities() {
    let mut writer = Writer::new(Vec::new())
        .with_hash_algorithm(imij::HashAlgorithm::Xxh3_64)
        .unwrap();
    let mut meta = imij_set::EntryMetadata::default();
    meta.filename = Some("old-name".into());
    meta.attributes.insert(
        "opaque.future.attribute".into(),
        imij_set::Value::Bytes(vec![0, 255]),
    );
    writer
        .insert_with_metadata(7, 3, 2, Summary::default(), &meta, &mut &b"old"[..])
        .unwrap();
    let (bytes, _) = writer.finish().unwrap();
    let mut bytes = legacy_footer(bytes);
    let mut source_ends = vec![bytes.len()];
    for algorithm in [imij::HashAlgorithm::DEFAULT, imij::HashAlgorithm::Xxh3_64] {
        let reader = Reader::open(SliceReader::new(&bytes)).unwrap();
        let snapshot = reader.snapshot().clone();
        drop(reader);
        let start = bytes.len() as u64;
        let mut writer = Writer::append(&mut bytes, &snapshot, start)
            .unwrap()
            .with_hash_algorithm(algorithm)
            .unwrap();
        writer
            .insert(7, 3, 0, Summary::default(), &mut &b"new"[..])
            .unwrap();
        writer
            .insert_with_metadata(9, 4, 1, Summary::default(), &meta, &mut &b"blob"[..])
            .unwrap();
        writer.finish().unwrap();
        source_ends.push(bytes.len());
    }
    let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
    let (upgraded, snapshot) = imij_set::upgrade_container(&mut reader, Vec::new()).unwrap();
    assert_eq!(snapshot.generation(), 3);
    let mut end = upgraded.len();
    for source_end in source_ends.into_iter().rev() {
        assert_same_generation(&bytes[..source_end], &upgraded[..end]);
        let previous = u64::from_le_bytes(upgraded[end - 120..end - 112].try_into().unwrap());
        if previous == u64::MAX {
            break;
        }
        end = previous as usize;
    }
    // Rewriting current output is deterministic, not another generation append.
    let mut reader = Reader::open(SliceReader::new(&upgraded)).unwrap();
    let (again, _) = imij_set::upgrade_container(&mut reader, Vec::new()).unwrap();
    assert_eq!(again, upgraded);
}

#[test]
fn upgrade_empty_v1_generation_and_admission_before_writes() {
    let (bytes, _) = Writer::new(Vec::new())
        .with_hash_algorithm(imij::HashAlgorithm::Xxh3_64)
        .unwrap()
        .finish()
        .unwrap();
    let mut bytes = legacy_footer(bytes);
    assert!(bytes.len() < 136);
    let reader = Reader::open(SliceReader::new(&bytes)).unwrap();
    let snapshot = reader.snapshot().clone();
    drop(reader);
    let start = bytes.len() as u64;
    Writer::append(&mut bytes, &snapshot, start)
        .unwrap()
        .finish()
        .unwrap();
    let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
    let (upgraded, snapshot) = imij_set::upgrade_container(&mut reader, Vec::new()).unwrap();
    assert_eq!(snapshot.generation(), 2);
    assert_same_generation(&bytes, &upgraded);
    let mut limits = imij_set::Limits::default();
    limits.max_segments = 1;
    let mut reader = Reader::open(SliceReader::new(&bytes))
        .unwrap()
        .with_limits(limits);
    let mut output = Vec::new();
    assert!(imij_set::upgrade_container(&mut reader, &mut output).is_err());
    assert!(output.is_empty());
}

#[test]
fn upgrade_rejects_corrupt_superseded_blob() {
    let mut writer = Writer::new(Vec::new());
    writer
        .insert(1, 3, 0, Summary::default(), &mut &b"old"[..])
        .unwrap();
    let (mut bytes, snapshot) = writer.finish().unwrap();
    let start = bytes.len() as u64;
    let mut writer = Writer::append(&mut bytes, &snapshot, start).unwrap();
    writer
        .insert(1, 3, 0, Summary::default(), &mut &b"new"[..])
        .unwrap();
    writer.finish().unwrap();
    bytes[0] ^= 1;
    let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
    let mut output = Vec::new();
    assert!(imij_set::upgrade_container(&mut reader, &mut output).is_err());
    assert!(Reader::open(SliceReader::new(&output)).is_err());
}

// An encoded archive backed by a virtual uniform blob and only its index/footer
// bytes. Neither the source nor destination stores the large payload in memory.
struct VirtualBlob {
    blob_len: u64,
    position: u64,
    tail: Vec<u8>,
    maximum_request: usize,
}
impl VirtualBlob {
    fn new(blob_len: u64) -> Self {
        Self {
            blob_len,
            position: 0,
            tail: Vec::new(),
            maximum_request: 0,
        }
    }
}
impl Write for VirtualBlob {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        assert!(bytes.len() <= 65536, "whole-blob output request");
        let payload =
            (self.blob_len.saturating_sub(self.position)).min(bytes.len() as u64) as usize;
        assert!(bytes[..payload].iter().all(|&byte| byte == 0x5a));
        self.tail.extend_from_slice(&bytes[payload..]);
        self.position += bytes.len() as u64;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl ReadAt for VirtualBlob {
    fn len(&mut self) -> Result<u64, imij::Error> {
        Ok(self.position)
    }
    fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), imij::Error> {
        assert!(output.len() <= 65536, "whole-blob input request");
        assert!(offset + output.len() as u64 <= self.position);
        self.maximum_request = self.maximum_request.max(output.len());
        let payload = self
            .blob_len
            .saturating_sub(offset)
            .min(output.len() as u64) as usize;
        output[..payload].fill(0x5a);
        if payload < output.len() {
            let start = (offset + payload as u64 - self.blob_len) as usize;
            let remaining = output.len() - payload;
            output[payload..].copy_from_slice(&self.tail[start..start + remaining]);
        }
        Ok(())
    }
}

#[test]
fn upgrade_streams_large_blob_between_bounded_io_adapters() {
    // 64 MiB on the wire, generated/counted in chunks. This enforces I/O request
    // bounds; it is not a process-RSS measurement.
    let length = 64 * 1024 * 1024;
    let mut writer = Writer::new(VirtualBlob::new(length))
        .with_hash_algorithm(imij::HashAlgorithm::Xxh3_64)
        .unwrap();
    writer
        .insert(1, length, 4, Summary::default(), &mut io::repeat(0x5a))
        .unwrap();
    let (source, original) = writer.finish().unwrap();
    let expected_tail = source.tail.clone();
    let mut reader = Reader::open(source).unwrap();
    let (output, snapshot) =
        imij_set::upgrade_container(&mut reader, VirtualBlob::new(length)).unwrap();
    assert_eq!(output.position, original.end());
    assert_eq!(snapshot.end(), original.end());
    assert_eq!(output.tail, expected_tail);
    assert_eq!(reader.into_inner().maximum_request, 65536);
    let mut reader = Reader::open(output).unwrap();
    let entry = reader.get(1).unwrap().unwrap();
    assert_eq!(entry.len(), length);
    reader.copy_to(&entry, &mut io::sink()).unwrap();
}

#[test]
fn upgrade_propagates_write_and_flush_failures() {
    let mut writer = Writer::new(Vec::new());
    writer
        .insert(1, 6, 0, Summary::default(), &mut &b"abcdef"[..])
        .unwrap();
    let (bytes, _) = writer.finish().unwrap();
    for remaining in [0, 3, bytes.len() - 1] {
        let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
        let mut sink = Failing {
            bytes: Vec::new(),
            remaining,
        };
        assert!(imij_set::upgrade_container(&mut reader, &mut sink).is_err());
        assert!(Reader::open(SliceReader::new(&sink.bytes)).is_err());
    }
    struct FlushFailure;
    impl Write for FlushFailure {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("flush failed"))
        }
    }
    let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
    assert!(imij_set::upgrade_container(&mut reader, FlushFailure).is_err());
}

#[test]
fn upgrade_read_failure_does_not_report_partial_history_as_success() {
    struct Fault<'a> {
        bytes: &'a [u8],
        offset: u64,
    }
    impl ReadAt for Fault<'_> {
        fn len(&mut self) -> Result<u64, imij::Error> {
            Ok(self.bytes.len() as u64)
        }
        fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), imij::Error> {
            // Fail only a blob read, after open's tail read and index validation.
            if offset == self.offset && out.len() == 3 {
                return Err(imij::Error::Read);
            }
            SliceReader::new(self.bytes).read_exact_at(offset, out)
        }
    }
    let mut writer = Writer::new(Vec::new());
    writer
        .insert(1, 3, 0, Summary::default(), &mut &b"old"[..])
        .unwrap();
    let (mut bytes, first) = writer.finish().unwrap();
    let start = bytes.len() as u64;
    let mut writer = Writer::append(&mut bytes, &first, start).unwrap();
    writer
        .insert(1, 3, 0, Summary::default(), &mut &b"new"[..])
        .unwrap();
    writer.finish().unwrap();
    let mut reader = Reader::open(Fault {
        bytes: &bytes,
        offset: start,
    })
    .unwrap();
    let mut output = Vec::new();
    assert!(imij_set::upgrade_container(&mut reader, &mut output).is_err());
    let mut partial = Reader::open(SliceReader::new(&output)).unwrap();
    assert_eq!(partial.snapshot().generation(), 1);
    let entry = partial.get(1).unwrap().unwrap();
    assert_eq!(partial.read_entry_bounded(&entry, 3).unwrap(), b"old");
}

#[test]
fn legacy_footer_is_not_masked_by_a_false_newer_magic_in_prior_bytes() {
    let (mut bytes, _) = Writer::new(Vec::new())
        .with_hash_algorithm(imij::HashAlgorithm::Xxh3_64)
        .unwrap()
        .finish()
        .unwrap();
    let at = bytes.len() - 136;
    // Legal unreferenced bytes before a v1 footer can look like a v2 prefix.
    let mut gap = [0u8; 56];
    gap[..8].copy_from_slice(b"IMIJSET\x02");
    bytes.splice(at..at, gap);
    let mut bytes = legacy_footer(bytes);
    let reader = Reader::open(SliceReader::new(&bytes)).unwrap();
    assert_eq!(reader.snapshot().footer_len(), 80);
    let snapshot = reader.snapshot().clone();
    drop(reader);
    let start = bytes.len() as u64;
    Writer::append(&mut bytes, &snapshot, start)
        .unwrap()
        .finish()
        .unwrap();
    let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
    assert!(
        reader
            .listing()
            .unwrap()
            .next(&mut reader)
            .unwrap()
            .is_none()
    );
    let (_, snapshot) = imij_set::upgrade_container(&mut reader, io::sink()).unwrap();
    assert_eq!(snapshot.generation(), 2);
}

#[test]
fn full_upgrade_streams_large_blob_between_bounded_io_adapters() {
    // 64 MiB on the wire, generated/counted in chunks. This enforces I/O request
    // bounds; it is not a process-RSS measurement.
    let length = 64 * 1024 * 1024;
    let mut writer = Writer::new(VirtualBlob::new(length))
        .with_hash_algorithm(imij::HashAlgorithm::Xxh3_64)
        .unwrap();
    writer
        .insert(1, length, 4, Summary::default(), &mut io::repeat(0x5a))
        .unwrap();
    let (source, original) = writer.finish().unwrap();
    let expected_tail = source.tail.clone();
    let mut reader = Reader::open(source).unwrap();
    let (output, snapshot) = imij_set::upgrade(&mut reader, VirtualBlob::new(length), &{
        let mut options = imij_set::UpgradeOptions::default();
        options.image_limits.max_encoded_bytes = 8;
        options
    })
    .unwrap();
    assert_eq!(output.position, original.end());
    assert_eq!(snapshot.end(), original.end());
    assert_eq!(output.tail, expected_tail);
    assert_eq!(reader.into_inner().maximum_request, 65536);
    let mut reader = Reader::open(output).unwrap();
    let entry = reader.get(1).unwrap().unwrap();
    assert_eq!(entry.len(), length);
    reader.copy_to(&entry, &mut io::sink()).unwrap();
}

#[test]
fn full_upgrade_propagates_write_and_flush_failures() {
    let mut writer = Writer::new(Vec::new());
    writer
        .insert(1, 6, 0, Summary::default(), &mut &b"abcdef"[..])
        .unwrap();
    let (bytes, _) = writer.finish().unwrap();
    for remaining in [0, 3, bytes.len() - 1] {
        let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
        let mut sink = Failing {
            bytes: Vec::new(),
            remaining,
        };
        assert!(imij_set::upgrade(&mut reader, &mut sink, &Default::default()).is_err());
        assert!(Reader::open(SliceReader::new(&sink.bytes)).is_err());
    }
    struct FlushFailure;
    impl Write for FlushFailure {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("flush failed"))
        }
    }
    let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
    assert!(imij_set::upgrade(&mut reader, FlushFailure, &Default::default()).is_err());
}

#[test]
fn full_upgrade_read_failure_does_not_report_partial_history_as_success() {
    struct Fault<'a> {
        bytes: &'a [u8],
        offset: u64,
    }
    impl ReadAt for Fault<'_> {
        fn len(&mut self) -> Result<u64, imij::Error> {
            Ok(self.bytes.len() as u64)
        }
        fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), imij::Error> {
            // Fail only a blob read, after open's tail read and index validation.
            if offset == self.offset && out.len() == 3 {
                return Err(imij::Error::Read);
            }
            SliceReader::new(self.bytes).read_exact_at(offset, out)
        }
    }
    let mut writer = Writer::new(Vec::new());
    writer
        .insert(1, 3, 0, Summary::default(), &mut &b"old"[..])
        .unwrap();
    let (mut bytes, first) = writer.finish().unwrap();
    let start = bytes.len() as u64;
    let mut writer = Writer::append(&mut bytes, &first, start).unwrap();
    writer
        .insert(1, 3, 0, Summary::default(), &mut &b"new"[..])
        .unwrap();
    writer.finish().unwrap();
    let mut reader = Reader::open(Fault {
        bytes: &bytes,
        offset: start,
    })
    .unwrap();
    let mut output = Vec::new();
    assert!(imij_set::upgrade(&mut reader, &mut output, &Default::default()).is_err());
    let mut partial = Reader::open(SliceReader::new(&output)).unwrap();
    assert_eq!(partial.snapshot().generation(), 1);
    let entry = partial.get(1).unwrap().unwrap();
    assert_eq!(partial.read_entry_bounded(&entry, 3).unwrap(), b"old");
}

#[test]
fn full_upgrade_migrates_an_image_inside_a_v1_archive() {
    let image = include_bytes!("../../fuzz/seeds/24-seed-csum-lossless.imi");
    let mut writer = Writer::new(Vec::new())
        .with_hash_algorithm(imij::HashAlgorithm::Xxh3_64)
        .unwrap();
    writer
        .insert(
            1,
            image.len() as u64,
            0,
            Summary::default(),
            &mut &image[..],
        )
        .unwrap();
    let bytes = legacy_footer(writer.finish().unwrap().0);
    let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
    assert_eq!(reader.snapshot().footer_len(), 80);
    let (new, snapshot) = imij_set::upgrade(&mut reader, Vec::new(), &Default::default()).unwrap();
    assert_eq!(snapshot.footer_len(), 136);
    let mut reader = Reader::open(SliceReader::new(&new)).unwrap();
    let entry = reader.get(1).unwrap().unwrap();
    let migrated = reader.read_entry_bounded(&entry, 1 << 20).unwrap();
    assert_eq!(imij::header(&migrated).unwrap().version, 4);
    assert_eq!(
        imij::decode(&migrated).unwrap(),
        imij::decode(image).unwrap()
    );
}
