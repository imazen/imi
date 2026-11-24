//! `IndexPlacement::AtCommit`: the same archive content as the interleaved
//! writer, every index page after the last blob, and a view's lookup in one
//! tail read plus one range.
use imij::planes::{ReadAt, SliceReader};
use imij_set::{EntryMetadata, IndexPlacement, Reader, Summary, Writer};
use std::cell::RefCell;
use std::rc::Rc;

/// Each read call's ranges, (offset, length).
type Log = Rc<RefCell<Vec<Vec<(u64, usize)>>>>;
/// An entry without its offset, which differs between placements.
type Content = (
    u64,
    u64,
    imij_set::Digest,
    imij_set::Summary,
    imij_set::EntryMetadata,
);

/// Reads through a slice, logging each read call's ranges.
struct Logged<'a> {
    bytes: &'a [u8],
    log: Log,
}
impl ReadAt for Logged<'_> {
    fn len(&mut self) -> Result<u64, imij::Error> {
        Ok(self.bytes.len() as u64)
    }
    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), imij::Error> {
        self.log.borrow_mut().push(vec![(offset, out.len())]);
        SliceReader::new(self.bytes).read_exact_at(offset, out)
    }
    fn read_many_at(&mut self, reqs: &mut [(u64, &mut [u8])]) -> Result<(), imij::Error> {
        self.log
            .borrow_mut()
            .push(reqs.iter().map(|(o, b)| (*o, b.len())).collect());
        for (offset, out) in reqs.iter_mut() {
            SliceReader::new(self.bytes).read_exact_at(*offset, out)?;
        }
        Ok(())
    }
}

fn content(e: &imij_set::Entry) -> Content {
    (
        e.key(),
        e.len(),
        e.checksum(),
        e.summary(),
        e.metadata().clone(),
    )
}

fn contents(v: &[Option<imij_set::Entry>]) -> Vec<Option<Content>> {
    v.iter().map(|e| e.as_ref().map(content)).collect()
}

/// 100,000 entries with 200-byte blobs; every 97th carries a filename.
fn archive(placement: IndexPlacement) -> Vec<u8> {
    let mut w = Writer::new(Vec::new())
        .with_index_placement(placement)
        .unwrap();
    for k in 0..100_000u64 {
        let data = [k as u8; 200];
        let mut summary = Summary::default();
        summary.width = 512;
        summary.height = 256;
        let mut meta = EntryMetadata::default();
        if k % 97 == 0 {
            meta.filename = Some(format!("cell-{k}.imi"));
        }
        w.insert_with_metadata(k * 3, 200, 0, summary, &meta, &mut data.as_slice())
            .unwrap();
    }
    w.finish().unwrap().0
}

#[test]
fn at_commit_archives_read_the_same_as_interleaved_ones() {
    let a = archive(IndexPlacement::Interleaved);
    let b = archive(IndexPlacement::AtCommit);
    let mut ra = Reader::open(SliceReader::new(&a)).unwrap();
    let mut rb = Reader::open(SliceReader::new(&b)).unwrap();
    let la: Vec<_> = {
        let mut l = ra.listing().unwrap();
        std::iter::from_fn(|| l.next(&mut ra).unwrap()).collect()
    };
    let lb: Vec<_> = {
        let mut l = rb.listing().unwrap();
        std::iter::from_fn(|| l.next(&mut rb).unwrap()).collect()
    };
    assert_eq!(la.len(), 100_000);
    assert_eq!(
        la.iter().map(content).collect::<Vec<_>>(),
        lb.iter().map(content).collect::<Vec<_>>()
    );
    // Each placement's own offsets lead to its blob bytes.
    for e in lb.iter().step_by(997) {
        let bytes = rb.read_entry_bounded(e, 1 << 10).unwrap();
        assert_eq!(bytes, vec![(e.key() / 3) as u8; 200]);
    }
    let keys: Vec<u64> = (0..300_100u64).step_by(7).collect();
    // This exhaustive study explicitly admits more than a normal view-sized batch.
    let mut limits = imij_set::Limits::default();
    limits.max_batch_keys = keys.len();
    ra = ra.with_limits(limits);
    rb = rb.with_limits(limits);
    assert_eq!(
        contents(&ra.get_many(&keys).unwrap()),
        contents(&rb.get_many(&keys).unwrap())
    );
    for name in ["cell-0.imi", "cell-97.imi", "cell-96999.imi", "absent.imi"] {
        let fa: Vec<_> = {
            let mut l = ra.filename_listing(name).unwrap();
            std::iter::from_fn(|| l.next(&mut ra).unwrap()).collect()
        };
        let fb: Vec<_> = {
            let mut l = rb.filename_listing(name).unwrap();
            std::iter::from_fn(|| l.next(&mut rb).unwrap()).collect()
        };
        assert_eq!(
            fa.iter().map(content).collect::<Vec<_>>(),
            fb.iter().map(content).collect::<Vec<_>>(),
            "{name}"
        );
    }
    // Every index page follows the last blob: the blobs are contiguous from
    // byte zero, where the interleaved writer put leaves between them.
    let blobs: u64 = lb.iter().map(|e| e.len()).sum();
    let end = |l: &[imij_set::Entry]| l.iter().map(|e| e.offset() + e.len()).max().unwrap();
    assert_eq!(end(&lb), blobs);
    assert!(end(&la) > blobs);
}

#[test]
fn at_commit_view_lookup_is_one_tail_read_and_one_range() {
    let bytes = archive(IndexPlacement::AtCommit);
    let log = Rc::new(RefCell::new(Vec::new()));
    let source = Logged {
        bytes: &bytes,
        log: log.clone(),
    };
    let mut reader = Reader::open(source).unwrap();
    assert_eq!(log.borrow().len(), 1, "open: one tail read");
    // A view: four rows of five neighbouring cells, rows 1,000 keys apart.
    let keys: Vec<u64> = (0..4u64)
        .flat_map(|r| (0..5u64).map(move |c| 3 * (30_000 + r * 1_000 + c)))
        .collect();
    log.borrow_mut().clear();
    let found = reader.get_many(&keys).unwrap();
    assert!(found.iter().all(Option::is_some));
    let calls = log.borrow().clone();
    // The branch levels came with the tail; the leaves are one range.
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].len(), 1, "{calls:?}");
    // The interleaved layout needs a range per leaf (its leaves sit between
    // 51 KB of blobs each).
    let bytes = archive(IndexPlacement::Interleaved);
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut reader = Reader::open(Logged {
        bytes: &bytes,
        log: log.clone(),
    })
    .unwrap();
    log.borrow_mut().clear();
    assert_eq!(contents(&reader.get_many(&keys).unwrap()), contents(&found));
    let ranges: usize = log.borrow().iter().map(Vec::len).sum();
    assert!(ranges >= 4, "{:?}", log.borrow());
}

#[test]
fn placement_is_chosen_before_the_first_entry() {
    let mut w = Writer::new(Vec::new());
    w.insert(1, 1, 0, Summary::default(), &mut &b"x"[..])
        .unwrap();
    assert!(w.with_index_placement(IndexPlacement::AtCommit).is_err());
}

#[test]
fn an_at_commit_append_segment_reads_with_an_interleaved_base() {
    let base = archive(IndexPlacement::Interleaved);
    let mut bytes = base.clone();
    let snapshot = Reader::open(SliceReader::new(&base))
        .unwrap()
        .snapshot()
        .clone();
    let start = bytes.len() as u64;
    let mut w = Writer::append(&mut bytes, &snapshot, start)
        .unwrap()
        .with_index_placement(IndexPlacement::AtCommit)
        .unwrap();
    for k in [3u64, 150_000, 999_999] {
        w.insert(k, 3, 0, Summary::default(), &mut &b"new"[..])
            .unwrap();
    }
    w.finish().unwrap();
    let mut r = Reader::open(SliceReader::new(&bytes)).unwrap();
    let got = r.get_many(&[3, 6, 150_000, 999_999, 4]).unwrap();
    assert_eq!(got[0].as_ref().unwrap().len(), 3);
    assert_eq!(got[1].as_ref().unwrap().len(), 200);
    assert_eq!(got[2].as_ref().unwrap().len(), 3);
    assert_eq!(got[3].as_ref().unwrap().len(), 3);
    assert!(got[4].is_none());
}

#[test]
fn batched_lookup_admits_work_before_reads_and_caps_merged_ranges() {
    let bytes = archive(IndexPlacement::Interleaved);
    let keys = [0, 60_000, 150_000, 240_000, 0, 999_999];
    let mut reference = Reader::open(SliceReader::new(&bytes)).unwrap();
    let expected: Vec<_> = keys.iter().map(|&k| reference.get(k).unwrap()).collect();
    let log = Log::default();
    let mut reader = Reader::open(Logged {
        bytes: &bytes,
        log: log.clone(),
    })
    .unwrap();
    log.borrow_mut().clear();
    let mut limits = imij_set::Limits::default();
    limits.max_batch_keys = keys.len() - 1;
    reader = reader.with_limits(limits);
    assert!(
        reader
            .get_many(&keys)
            .unwrap_err()
            .to_string()
            .contains("key limit")
    );
    assert!(log.borrow().is_empty());
    limits.max_batch_keys = keys.len();
    limits.max_batch_pages = 0;
    reader = reader.with_limits(limits);
    assert!(
        reader
            .get_many(&keys)
            .unwrap_err()
            .to_string()
            .contains("page limit")
    );
    assert!(log.borrow().is_empty());
    limits.max_batch_pages = 1;
    reader = reader.with_limits(limits);
    assert!(
        reader
            .get_many(&keys)
            .unwrap_err()
            .to_string()
            .contains("page limit")
    );
    assert!(
        log.borrow().is_empty(),
        "root in tail consumes the one-page budget"
    );
    limits.max_batch_pages = 1024;
    limits.max_batch_read_bytes = 0;
    limits.page_gap = u64::MAX;
    limits.max_merged_range_bytes = u64::MAX;
    reader = reader.with_limits(limits);
    assert!(
        reader
            .get_many(&keys)
            .unwrap_err()
            .to_string()
            .contains("read limit")
    );
    assert!(
        log.borrow().is_empty(),
        "deny the entire range batch before reading"
    );
    limits.max_batch_read_bytes = 32 * 1024 * 1024;
    limits.max_merged_range_bytes = 64 * 1024;
    reader = reader.with_limits(limits);
    assert_eq!(reader.get_many(&keys).unwrap(), expected);
    assert!(!log.borrow().is_empty());
    assert!(log.borrow().iter().flatten().all(|(_, n)| *n <= 64 * 1024));
    assert!(
        log.borrow().iter().map(|batch| batch.len()).sum::<usize>() > 1,
        "unlimited gap must not merge the whole source"
    );
    limits.max_segments = 0;
    reader = reader.with_limits(limits);
    assert!(reader.get_many(&[]).unwrap().is_empty());
}

#[test]
fn batch_budgets_continue_across_append_segments_and_charge_footers() {
    let mut w = Writer::new(Vec::new());
    w.insert(1, 1, 0, Summary::default(), &mut &b"a"[..])
        .unwrap();
    let (mut bytes, _) = w.finish().unwrap();
    let snapshot = Reader::open(SliceReader::new(&bytes))
        .unwrap()
        .snapshot()
        .clone();
    let start = bytes.len() as u64;
    let mut w = Writer::append(&mut bytes, &snapshot, start).unwrap();
    w.insert(2, 1, 0, Summary::default(), &mut &b"b"[..])
        .unwrap();
    w.finish().unwrap();
    let log = Log::default();
    let mut reader = Reader::open(Logged {
        bytes: &bytes,
        log: log.clone(),
    })
    .unwrap();
    log.borrow_mut().clear();
    let mut limits = imij_set::Limits::default();
    limits.max_batch_read_bytes = 0;
    reader = reader.with_limits(limits);
    assert!(
        reader
            .get_many(&[1])
            .unwrap_err()
            .to_string()
            .contains("read limit")
    );
    assert!(log.borrow().is_empty());
    limits.max_batch_read_bytes = 1024;
    limits.max_batch_pages = 1;
    reader = reader.with_limits(limits);
    assert!(
        reader
            .get_many(&[1])
            .unwrap_err()
            .to_string()
            .contains("page limit")
    );
    let footer_bytes: u64 = log.borrow().iter().flatten().map(|(_, n)| *n as u64).sum();
    assert!(footer_bytes > 0);
    log.borrow_mut().clear();
    limits.max_batch_pages = 2;
    limits.max_batch_read_bytes = footer_bytes - 1;
    reader = reader.with_limits(limits);
    assert!(
        reader
            .get_many(&[1])
            .unwrap_err()
            .to_string()
            .contains("read limit")
    );
    assert!(log.borrow().is_empty());
    limits.max_batch_read_bytes = footer_bytes;
    reader = reader.with_limits(limits);
    let keys = [1, 2, 1, 3, 2, 3];
    let found = reader.get_many(&keys).unwrap();
    assert_eq!(
        found
            .iter()
            .map(|e| e.as_ref().map(|e| e.key()))
            .collect::<Vec<_>>(),
        [Some(1), Some(2), Some(1), None, Some(2), None]
    );
    assert_eq!(
        log.borrow()
            .iter()
            .flatten()
            .map(|(_, n)| *n as u64)
            .sum::<u64>(),
        footer_bytes
    );
}

#[test]
fn unaddressable_tail_is_rejected_without_reading() {
    struct Huge;
    impl ReadAt for Huge {
        fn len(&mut self) -> Result<u64, imij::Error> {
            Ok(u64::MAX)
        }
        fn read_exact_at(&mut self, _: u64, _: &mut [u8]) -> Result<(), imij::Error> {
            panic!("tail allocation must be admitted before a read")
        }
    }
    assert!(Reader::open_with_tail(Huge, u64::MAX).is_err());
}
