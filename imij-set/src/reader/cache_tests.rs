use super::*;
use imij::planes::SliceReader;
use std::{cell::Cell, rc::Rc};

struct Counted<'a> {
    bytes: &'a [u8],
    reads: Rc<Cell<usize>>,
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
fn archive() -> Vec<u8> {
    let mut writer = crate::Writer::new(Vec::new());
    for key in 0..20_000 {
        writer
            .insert(
                key,
                64,
                0,
                crate::Summary::default(),
                &mut &[key as u8; 64][..],
            )
            .unwrap();
    }
    writer.finish().unwrap().0
}
fn reader(bytes: &[u8]) -> (Reader<Counted<'_>>, Rc<Cell<usize>>) {
    let reads = Rc::new(Cell::new(0));
    let source = Counted {
        bytes,
        reads: reads.clone(),
    };
    (Reader::open(source).unwrap(), reads)
}

#[test]
fn nearby_point_and_batch_lookups_reuse_validated_index_bytes() {
    let bytes = archive();
    for batch_first in [false, true] {
        let (mut reader, reads) = reader(&bytes);
        let opened = reads.get();
        let expected = if batch_first {
            reader.get_many(&[5_000, 5_001]).unwrap()[0]
                .clone()
                .unwrap()
        } else {
            reader.get(5_000).unwrap().unwrap()
        };
        assert!(
            reads.get() > opened,
            "fixture must fetch an index outside the tail"
        );
        let cold = reads.get();
        for _ in 0..3 {
            assert_eq!(reader.get(5_000).unwrap(), Some(expected.clone()));
            let entries = reader.get_many(&[5_001, 5_000, 5_001]).unwrap();
            assert_eq!(entries[1], Some(expected.clone()));
            assert_eq!(entries[0].as_ref().unwrap().key(), 5_001);
            assert_eq!(entries[0], entries[2]);
        }
        assert_eq!(
            reads.get(),
            cold,
            "warm index lookup must do zero source IO"
        );
        assert_eq!(
            reader.read_entry_bounded(&expected, 64).unwrap(),
            [5_000u64 as u8; 64]
        );
    }
}

#[test]
fn listings_reuse_tail_and_verify_entry_count_when_the_whole_tree_was_read() {
    let mut writer = crate::Writer::new(Vec::new());
    for key in [20, 22] {
        writer
            .insert(key, 1, 0, crate::Summary::default(), &mut &[9][..])
            .unwrap();
    }
    let (bytes, _) = writer.finish().unwrap();
    let (mut reader, reads) = reader(&bytes);
    let opened = reads.get();
    let mut all = reader.listing().unwrap();
    assert_eq!(all.next(&mut reader).unwrap().unwrap().key(), 20);
    assert_eq!(all.next(&mut reader).unwrap().unwrap().key(), 22);
    assert!(all.next(&mut reader).unwrap().is_none());
    let mut partial = reader.listing_from(21).unwrap();
    assert_eq!(partial.next(&mut reader).unwrap().unwrap().key(), 22);
    assert!(partial.next(&mut reader).unwrap().is_none());
    assert_eq!(reads.get(), opened);

    reader.snapshot.entries += 1;
    let mut from_first = reader.listing_from(0).unwrap();
    assert_eq!(from_first.next(&mut reader).unwrap().unwrap().key(), 20);
    assert_eq!(
        from_first.next(&mut reader).unwrap_err().to_string(),
        "segment entry count"
    );
}

#[test]
fn cached_bytes_still_check_each_requested_node_binding() {
    let bytes = archive();
    let (mut reader, reads) = reader(&bytes);
    let snapshot = reader.snapshot.clone();
    let mut node = snapshot.root;
    let mut level = snapshot.level;
    while level != 0 {
        let Page::Branch(children) = reader.page(&snapshot, node, level).unwrap() else {
            panic!()
        };
        node = *children.iter().find(|n| n.max >= Some(5_000)).unwrap();
        level -= 1;
    }
    reader.page(&snapshot, node, level).unwrap();
    assert!(reader.cached_index(node).is_some());
    let fetched = reads.get();
    let mut bad = node;
    bad.hash = snapshot.algo.digest(b"different page").into();
    assert!(reader.page(&snapshot, bad, level).is_err());
    bad = node;
    bad.max = Some(u64::MAX);
    assert!(reader.page(&snapshot, bad, level).is_err());
    assert_eq!(reads.get(), fetched);
    assert!(reader.page(&snapshot, node, level).is_ok());
}

#[test]
fn retained_index_capacity_and_range_count_remain_bounded() {
    let bytes = archive();
    let (mut reader, _) = reader(&bytes);
    for i in 0..64 {
        reader.retain_index(i * 1024, vec![0; 1024]).unwrap();
        assert!(reader.index_cache.len() <= INDEX_CACHE_RANGES);
    }
    for i in 0..8 {
        reader.retain_index(i * 262_144, vec![0; 262_144]).unwrap();
        assert!(reader.index_cache_bytes <= INDEX_CACHE_BYTES);
        assert_eq!(
            reader.index_cache_bytes,
            reader.index_cache.iter().map(|r| r.bytes.capacity()).sum()
        );
    }
    let before = reader.index_cache_bytes;
    reader
        .retain_index(0, vec![0; INDEX_CACHE_BYTES + 1])
        .unwrap();
    assert_eq!(reader.index_cache_bytes, before);
}
