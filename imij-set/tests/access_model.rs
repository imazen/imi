#[path = "../examples/access_model/transport.rs"]
mod transport;
use imij::{
    Channels, ImageInfo, SampleType,
    planes::{ReadAt, Rect, SliceReader},
    region::RegionReader,
};
use imij_set::{IndexPlacement, Reader, Summary, Writer};
use std::{cell::RefCell, rc::Rc};
use transport::{Config, Ledger, Transport};
fn config() -> Config {
    Config {
        ranges: true,
        cache_bytes: 1024 * 1024,
        latency_ms: 40.0,
        bytes_per_ms: 1000.0,
        concurrency: 2,
        merge_gap: 0,
        max_range: 1024 * 1024,
        part_size: 0,
        shared_cache: false,
    }
}

#[test]
fn known_transport_cost_overlap_eviction_and_whole_file() {
    let bytes = vec![73; 1000];
    let l = Rc::new(RefCell::new(Ledger::default()));
    let mut t = Transport::new(
        SliceReader::new(&bytes),
        "input".into(),
        config(),
        l.clone(),
    )
    .unwrap();
    let (mut a, mut b) = ([0; 100], [0; 100]);
    t.read_many_at(&mut [(0, &mut a), (200, &mut b)]).unwrap();
    assert_eq!(a, [73; 100]);
    assert_eq!(b, [73; 100]);
    assert_eq!(l.borrow().now_ms, 40.2);
    assert_eq!(l.borrow().rounds, 1);
    t.read_exact_at(50, &mut a).unwrap();
    assert_eq!(l.borrow().transferred_bytes, 250);
    assert_eq!(l.borrow().cache_hit_bytes, 50);
    assert_eq!(l.borrow().events[1].ranges, vec![100..150]);
    let mut cfg = config();
    cfg.ranges = false;
    let l = Rc::new(RefCell::new(Ledger::default()));
    let mut t = Transport::new(SliceReader::new(&bytes), "full".into(), cfg, l.clone()).unwrap();
    t.read_exact_at(400, &mut a).unwrap();
    t.read_exact_at(600, &mut a).unwrap();
    assert_eq!(l.borrow().transferred_bytes, 1000);
    assert_eq!(l.borrow().now_ms, 41.0);
    assert_eq!(l.borrow().events[0].status, 200);
    cfg.cache_bytes = 0;
    let l = Rc::new(RefCell::new(Ledger::default()));
    let mut t =
        Transport::new(SliceReader::new(&bytes), "uncached".into(), cfg, l.clone()).unwrap();
    t.read_exact_at(400, &mut a).unwrap();
    t.read_exact_at(600, &mut a).unwrap();
    assert_eq!(l.borrow().transferred_bytes, 2000);
    assert_eq!(l.borrow().now_ms, 82.0);
    cfg.ranges = true;
    cfg.cache_bytes = 100;
    let mut t = Transport::new(SliceReader::new(&bytes), "small".into(), cfg, l.clone()).unwrap();
    t.read_exact_at(0, &mut a).unwrap();
    t.read_exact_at(200, &mut a).unwrap();
    assert_eq!(t.retained(), 100);
    let before = l.borrow().transferred_bytes;
    t.read_exact_at(0, &mut a).unwrap();
    assert_eq!(l.borrow().transferred_bytes - before, 100);
}
fn fixture() -> (Vec<u8>, Vec<u8>) {
    let info = ImageInfo::new(96, 64, Channels::Rgb, SampleType::U8);
    let raw = (0..96 * 64 * 3)
        .map(|i| ((i * 13 + i / 11) % 251) as u8)
        .collect::<Vec<_>>();
    let mut c = imij::ImiEncoderConfig::default();
    c = c.tune_lossless(|t| {
        t.stripe_rows = Some(8);
    });
    c = c.with_column_groups(Some(32));
    (
        c.encode_samples(imij::ImageRef::new(&raw, 288, &info), &Default::default())
            .unwrap(),
        raw,
    )
}
#[test]
fn real_region_decoder_pan_return_all_transport_modes() {
    let (file, raw) = fixture();
    for ranges in [false, true] {
        for budget in [0, 2048, 1024 * 1024] {
            for latency in [0.0, 100.0] {
                let l = Rc::new(RefCell::new(Ledger::default()));
                let mut cfg = config();
                cfg.ranges = ranges;
                cfg.cache_bytes = budget;
                cfg.latency_ms = latency;
                let source =
                    Transport::new(SliceReader::new(&file), "photo".into(), cfg, l.clone())
                        .unwrap();
                let mut reader = RegionReader::open(source).unwrap();
                for (x, y) in [(3, 2), (35, 9), (64, 32), (3, 2)] {
                    let rect = Rect::new(x, y, 24, 16);
                    let plan = reader.region_ranges(rect).unwrap();
                    assert!(!plan.is_empty());
                    let mut out = vec![0xa5; 80 * 16];
                    reader.decode_region(rect, &mut out, 80).unwrap();
                    for row in 0..16 {
                        assert_eq!(
                            &out[row * 80..row * 80 + 72],
                            &raw[((y as usize + row) * 96 + x as usize) * 3
                                ..((y as usize + row) * 96 + x as usize + 24) * 3]
                        );
                        assert_eq!(&out[row * 80 + 72..row * 80 + 80], &[0xa5; 8]);
                    }
                }
                let l = l.borrow();
                assert!(!l.events.is_empty());
                assert!(
                    l.events
                        .iter()
                        .all(|e| e.end_ms >= e.start_ms && e.object == "photo" && e.round > 0)
                );
                assert_eq!(
                    l.transferred_bytes,
                    l.events
                        .iter()
                        .flat_map(|e| &e.ranges)
                        .map(|r| r.end - r.start)
                        .sum()
                );
            }
        }
    }
}
#[test]
fn real_archive_index_and_pixels_survive_key_window_return_and_cache_policy() {
    let (file, raw) = fixture();
    for placement in [IndexPlacement::Interleaved, IndexPlacement::AtCommit] {
        let mut writer = Writer::new(Vec::new())
            .with_index_placement(placement)
            .unwrap();
        for key in 0..300 {
            writer
                .insert(
                    key,
                    file.len() as u64,
                    0,
                    Summary::default(),
                    &mut file.as_slice(),
                )
                .unwrap();
        }
        let archive = writer.finish().unwrap().0;
        for ranges in [false, true] {
            for budget in [0, 16384, archive.len() as u64] {
                let mut cfg = config();
                cfg.ranges = ranges;
                cfg.cache_bytes = budget;
                let l = Rc::new(RefCell::new(Ledger::default()));
                let source =
                    Transport::new(SliceReader::new(&archive), "pyramid".into(), cfg, l.clone())
                        .unwrap();
                let mut reader = Reader::open(source).unwrap();
                for keys in [
                    &[0, 1, 2][..],
                    &[128, 129][..],
                    &[297, 298, 299][..],
                    &[0, 1, 2][..],
                ] {
                    for entry in reader.get_many(keys).unwrap() {
                        let entry = entry.unwrap();
                        let mut rr =
                            RegionReader::open(reader.entry_source(&entry).unwrap()).unwrap();
                        let mut out = vec![0; raw.len()];
                        rr.decode_region(Rect::new(0, 0, 96, 64), &mut out, 288)
                            .unwrap();
                        assert_eq!(out, raw);
                    }
                }
                if !ranges && budget >= archive.len() as u64 {
                    assert_eq!(l.borrow().events.len(), 1);
                    assert_eq!(l.borrow().transferred_bytes, archive.len() as u64);
                }
            }
        }
    }
}
#[test]
fn rejects_invalid_models_and_ranges_before_transfer() {
    let mut cfg = config();
    cfg.latency_ms = f64::NAN;
    assert!(cfg.validate().is_err());
    let l = Rc::new(RefCell::new(Ledger::default()));
    let mut t = Transport::new(
        SliceReader::new(&[1, 2]),
        "short".into(),
        config(),
        l.clone(),
    )
    .unwrap();
    assert!(t.read_exact_at(u64::MAX, &mut [0; 2]).is_err());
    assert!(l.borrow().events.is_empty());
}

#[test]
fn dependency_rounds_parallel_waves_and_shared_bandwidth_are_distinct() {
    let bytes = vec![19; 1000];
    for concurrency in [1, 2, 8] {
        let mut cfg = config();
        cfg.cache_bytes = 0;
        cfg.concurrency = concurrency;
        let l = Rc::new(RefCell::new(Ledger::default()));
        let mut t =
            Transport::new(SliceReader::new(&bytes), "waves".into(), cfg, l.clone()).unwrap();
        let (mut a, mut b, mut c) = ([0; 100], [0; 100], [0; 100]);
        t.read_many_at(&mut [(0, &mut a), (200, &mut b), (400, &mut c)])
            .unwrap();
        let waves = 3_usize.div_ceil(concurrency);
        assert_eq!(l.borrow().rounds, 1);
        assert_eq!(l.borrow().events.len(), waves);
        assert!((l.borrow().now_ms - (waves as f64 * 40.0 + 0.3)).abs() < 1e-9);
        t.read_exact_at(800, &mut a).unwrap();
        assert_eq!(l.borrow().rounds, 2);
        assert_eq!(l.borrow().events.last().unwrap().round, 2);
    }
}

#[test]
fn overlapping_long_demands_are_unioned_before_range_splitting() {
    let bytes = vec![0; 1000];
    let mut cfg = config();
    cfg.max_range = 100;
    cfg.cache_bytes = 0;
    let l = Rc::new(RefCell::new(Ledger::default()));
    let mut t = Transport::new(SliceReader::new(&bytes), "overlap".into(), cfg, l.clone()).unwrap();
    let (mut a, mut b) = ([0; 250], [0; 250]);
    t.read_many_at(&mut [(0, &mut a), (100, &mut b)]).unwrap();
    assert_eq!(l.borrow().transferred_bytes, 350);
    assert_eq!(
        l.borrow()
            .events
            .iter()
            .flat_map(|e| &e.ranges)
            .cloned()
            .collect::<Vec<_>>(),
        vec![0..100, 100..200, 200..300, 300..350]
    );
}

#[test]
fn payload_batcher_and_reader_share_cache_without_duplicate_download() {
    let data = vec![44; 1024];
    let l = Rc::new(RefCell::new(Ledger::default()));
    let mut a = transport::SharedSource::new(
        Transport::new(
            SliceReader::new(&data),
            "shared".into(),
            config(),
            l.clone(),
        )
        .unwrap(),
    );
    let mut b = a.clone();
    let mut out = [0; 256];
    a.read_exact_at(64, &mut out).unwrap();
    b.read_exact_at(64, &mut out).unwrap();
    assert_eq!(out, [44; 256]);
    assert_eq!(l.borrow().transferred_bytes, 256);
    assert_eq!(l.borrow().cache_hit_bytes, 256);
}

#[path = "../examples/access_model/input.rs"]
mod input;
#[path = "../examples/access_model/layout.rs"]
mod layout;
#[path = "../examples/access_model/source.rs"]
mod source;

fn test_dir() -> std::path::PathBuf {
    static ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let base = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .expect("HOME or USERPROFILE for disk-backed test scratch");
    let path = std::path::PathBuf::from(base).join("tmp").join(format!(
        "imi-access-test-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn physical_boundaries_parallelism_short_tail_and_whole_part_cache() {
    let dir = test_dir();
    let bytes = (0..250).map(|i| i as u8).collect::<Vec<_>>();
    let path = dir.join("input.bin");
    std::fs::write(&path, &bytes).unwrap();
    let manifest = layout::split(&path, 100).unwrap();
    for ranges in [true, false] {
        for concurrency in [1, 2, 8] {
            let mut cfg = config();
            cfg.ranges = ranges;
            cfg.part_size = 100;
            cfg.concurrency = concurrency;
            cfg.merge_gap = 4096; // Even a large merge gap cannot cross a part boundary.
            let ledger = Rc::new(RefCell::new(Ledger::default()));
            let mut transport = Transport::new(
                input::Input::open(manifest.to_str().unwrap()).unwrap(),
                "parts".into(),
                cfg,
                ledger.clone(),
            )
            .unwrap();
            let mut out = [0; 160];
            transport.read_exact_at(90, &mut out).unwrap();
            assert_eq!(out.as_slice(), &bytes[90..]);
            let l = ledger.borrow();
            assert_eq!(l.rounds, 1);
            assert_eq!(l.events.iter().map(|e| e.ranges.len()).sum::<usize>(), 3);
            assert_eq!(
                l.events
                    .iter()
                    .map(|e| e.object.as_str())
                    .collect::<Vec<_>>(),
                vec![
                    "parts/part-000000",
                    "parts/part-000001",
                    "parts/part-000002"
                ]
            );
            assert_eq!(
                l.events[0].ranges,
                vec![if ranges { 90..100 } else { 0..100 }]
            );
            assert_eq!(l.events[1].ranges, vec![0..100]);
            assert_eq!(l.events[2].ranges, vec![0..50]);
            assert_eq!(l.events[2].logical_base, 200);
            assert_eq!(l.events[2].object_length, 50);
            let transferred = if ranges { 160 } else { 250 };
            assert_eq!(l.transferred_bytes, transferred);
            assert!(
                (l.now_ms
                    - (3usize.div_ceil(concurrency) as f64 * 40.0 + transferred as f64 / 1000.0))
                    .abs()
                    < 1e-9
            );
            drop(l);
            let before = ledger.borrow().transferred_bytes;
            transport.read_exact_at(90, &mut out).unwrap();
            assert_eq!(ledger.borrow().transferred_bytes, before);
            assert!(transport.read_exact_at(249, &mut [0; 2]).is_err());
            assert!(transport.read_exact_at(u64::MAX, &mut [0; 1]).is_err());
        }
    }
    // Adapter reads the physical parts, never a hidden concatenated original.
    std::fs::rename(&path, dir.join("original-retained.bin")).unwrap();
    let mut input = input::Input::open(manifest.to_str().unwrap()).unwrap();
    let mut all = vec![0; bytes.len()];
    input.read_exact_at(0, &mut all).unwrap();
    assert_eq!(all, bytes);
    std::fs::write(dir.join("input.bin.part-000001"), [0; 100]).unwrap();
    assert!(input.verify().is_err());
    assert!(input::Input::open(manifest.to_str().unwrap()).is_err());
}

#[test]
fn split_manifest_missing_parts_and_geometry_are_errors() {
    let dir = test_dir();
    let path = dir.join("input.bin");
    std::fs::write(&path, [7; 31]).unwrap();
    let manifest = layout::split(&path, 10).unwrap();
    let original = std::fs::read_to_string(&manifest).unwrap();
    for (i, changed) in [
        original.replacen("\t10\t31", "\t0\t31", 1),
        original.replacen("\t10\t31", "\t11\t31", 1),
        original.lines().take(4).collect::<Vec<_>>().join("\n"),
    ]
    .into_iter()
    .enumerate()
    {
        let invalid = dir.join(format!("invalid-{i}.parts.tsv"));
        std::fs::write(&invalid, changed).unwrap();
        assert!(input::Input::open(invalid.to_str().unwrap()).is_err());
    }
    std::fs::rename(
        dir.join("input.bin.part-000001"),
        dir.join("missing-part-retained"),
    )
    .unwrap();
    assert!(input::Input::open(manifest.to_str().unwrap()).is_err());
}

#[test]
fn flat_levels_preserve_sparse_keys_and_payload_identity() {
    let dir = test_dir();
    let mut paths = Vec::new();
    for level in 0..2u8 {
        let mut writer = Writer::new(Vec::new());
        for key in [0, 10] {
            let bytes = [level, key as u8];
            writer
                .insert(key, 2, 1, Summary::default(), &mut bytes.as_slice())
                .unwrap();
        }
        let path = dir.join(format!("flat-source-{level}.imiset"));
        std::fs::write(&path, writer.finish().unwrap().0).unwrap();
        paths.push(path);
    }
    let output = dir.join("flat.imiset");
    let bases = layout::flatten(&paths, &output, IndexPlacement::AtCommit).unwrap();
    assert_eq!(bases, [0, 11]);
    let mut reader = Reader::open(input::Input::open(output.to_str().unwrap()).unwrap()).unwrap();
    assert!(reader.get(1).unwrap().is_none());
    for (level, base) in bases.into_iter().enumerate() {
        for key in [0, 10] {
            let entry = reader.get(base + key).unwrap().unwrap();
            assert_eq!(entry.head_size(), 1);
            let mut bytes = Vec::new();
            reader.copy_to(&entry, &mut bytes).unwrap();
            assert_eq!(bytes, [level as u8, key as u8]);
        }
    }
}

#[test]
fn packed_pyramid_level_return_both_indexes_and_physical_parts_preserve_pixels() {
    let dir = test_dir();
    let (file, raw) = fixture();
    let small_info = ImageInfo::new(24, 16, Channels::Rgb, SampleType::U8);
    let small_raw = raw[..24 * 16 * 3].to_vec();
    let small = imij::ImiEncoderConfig::default()
        .encode_samples(
            imij::ImageRef::new(&small_raw, 72, &small_info),
            &Default::default(),
        )
        .unwrap();
    for (p, placement) in [IndexPlacement::Interleaved, IndexPlacement::AtCommit]
        .into_iter()
        .enumerate()
    {
        let mut outer = Writer::new(Vec::new())
            .with_index_placement(placement)
            .unwrap();
        for (level, encoded) in [&file, &small].into_iter().enumerate() {
            let mut inner = Writer::new(Vec::new())
                .with_index_placement(placement)
                .unwrap();
            for key in 0..300 {
                inner
                    .insert(
                        key,
                        encoded.len() as u64,
                        0,
                        Summary::default(),
                        &mut encoded.as_slice(),
                    )
                    .unwrap();
            }
            let level_bytes = inner.finish().unwrap().0;
            outer
                .insert(
                    level as u64,
                    level_bytes.len() as u64,
                    0,
                    Summary::default(),
                    &mut level_bytes.as_slice(),
                )
                .unwrap();
        }
        let pyramid = outer.finish().unwrap().0;
        let original = dir.join(format!("pyramid-{p}.imiset"));
        std::fs::write(&original, &pyramid).unwrap();
        let rebuilt = dir.join(format!("repacked-{p}.imiset"));
        layout::repack(
            original.to_str().unwrap(),
            &rebuilt,
            placement,
            &mut Vec::new(),
        )
        .unwrap();
        for part_size in [0, 4093] {
            // Deliberately misalign every header/index/payload family.
            let input_path = layout::split(&rebuilt, part_size).unwrap();
            for ranges in [false, true] {
                for cache in [0, 8192, pyramid.len() as u64 * 2] {
                    let input = input::Input::open(input_path.to_str().unwrap()).unwrap();
                    let mut cfg = config();
                    cfg.ranges = ranges;
                    cfg.cache_bytes = cache;
                    cfg.part_size = part_size;
                    let ledger = Rc::new(RefCell::new(Ledger::default()));
                    let shared = transport::SharedSource::new(
                        Transport::new(input, "pyramid".into(), cfg, ledger.clone()).unwrap(),
                    );
                    let mut tree = source::ArchiveTree::new(shared).unwrap();
                    for level in [0, 1, 0] {
                        let reader = tree.reader(&[level]).unwrap();
                        for entry in reader.get_many(&[0, 299]).unwrap() {
                            let entry = entry.unwrap();
                            let mut rr =
                                RegionReader::open(reader.entry_source(&entry).unwrap()).unwrap();
                            let (w, h, expected) = if level == 0 {
                                (96, 64, &raw)
                            } else {
                                (24, 16, &small_raw)
                            };
                            let mut out = vec![0; expected.len()];
                            rr.decode_region(Rect::new(0, 0, w, h), &mut out, w as usize * 3)
                                .unwrap();
                            assert_eq!(&out, expected);
                        }
                    }
                    let mut tile = tree.source(&[1, 299]).unwrap();
                    let mut encoded = vec![0; small.len()];
                    tile.read_exact_at(0, &mut encoded).unwrap();
                    assert_eq!(encoded, small);
                    let mut sentinel = [0xa5; 8];
                    let mut requests = [(small.len() as u64 - 1, sentinel.as_mut_slice())];
                    assert!(tile.read_many_at(&mut requests).is_err());
                    assert_eq!(requests[0].0, small.len() as u64 - 1);
                    assert_eq!(sentinel, [0xa5; 8]);
                    assert!(tree.source(&[77]).is_err());
                    assert!(tree.source(&[0; 9]).is_err());
                    let mut window = tree.source(&[1]).unwrap();
                    let length = window.len().unwrap();
                    let before = ledger.borrow().events.len();
                    assert!(window.read_exact_at(length, &mut [0; 1]).is_err());
                    assert_eq!(ledger.borrow().events.len(), before);
                }
            }
        }
    }
}

#[test]
fn shared_cache_budget_is_constant_across_objects_and_never_aliases_them() {
    let mut cfg = config();
    cfg.shared_cache = true;
    cfg.cache_bytes = 100;
    let ledger = Rc::new(RefCell::new(Ledger::default()));
    let mut a =
        Transport::new(SliceReader::new(&[1; 200]), "a".into(), cfg, ledger.clone()).unwrap();
    let mut b =
        Transport::new(SliceReader::new(&[2; 200]), "b".into(), cfg, ledger.clone()).unwrap();
    let mut out = [0; 60];
    a.read_exact_at(0, &mut out).unwrap();
    assert_eq!(out, [1; 60]);
    b.read_exact_at(0, &mut out).unwrap();
    assert_eq!(out, [2; 60]);
    assert_eq!(a.retained(), 60); // A was evicted from the shared 100-byte pool.
    assert_eq!(ledger.borrow().transferred_bytes, 120);
    b.read_exact_at(0, &mut out).unwrap();
    assert_eq!(ledger.borrow().transferred_bytes, 120);
    a.read_exact_at(0, &mut out).unwrap();
    assert_eq!(ledger.borrow().transferred_bytes, 180);
    cfg.cache_bytes = 200;
    assert!(Transport::new(SliceReader::new(&[3; 200]), "c".into(), cfg, ledger.clone()).is_err());
}

#[test]
fn provider_sized_parts_use_u64_offsets_without_giant_allocations() {
    struct Virtual(u64);
    impl ReadAt for Virtual {
        fn len(&mut self) -> Result<u64, imij::Error> {
            Ok(self.0)
        }
        fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), imij::Error> {
            if offset
                .checked_add(out.len() as u64)
                .is_none_or(|end| end > self.0)
            {
                return Err(imij::Error::Read);
            }
            for (i, b) in out.iter_mut().enumerate() {
                *b = (offset + i as u64) as u8;
            }
            Ok(())
        }
    }
    let size = 512 * 1024 * 1024;
    let length = 10 * 1024 * 1024 * 1024 + 37;
    let mut cfg = config();
    cfg.part_size = size;
    cfg.cache_bytes = 0;
    let ledger = Rc::new(RefCell::new(Ledger::default()));
    let mut t = Transport::new(Virtual(length), "large".into(), cfg, ledger.clone()).unwrap();
    let mut out = [0; 32];
    t.read_exact_at(9 * size - 16, &mut out).unwrap();
    assert_eq!(
        out,
        std::array::from_fn(|i| (9 * size - 16 + i as u64) as u8)
    );
    assert_eq!(ledger.borrow().events[0].object, "large/part-000008");
    assert_eq!(ledger.borrow().events[1].object, "large/part-000009");
    assert_eq!(ledger.borrow().transferred_bytes, 32);
    cfg.ranges = false;
    let mut t = Transport::new(Virtual(length), "large".into(), cfg, ledger.clone()).unwrap();
    t.read_exact_at(length - 1, &mut [0; 1]).unwrap();
    let l = ledger.borrow();
    assert_eq!(l.events.last().unwrap().ranges, vec![0..37]);
    assert_eq!(l.transferred_bytes, 69);
}

// Derived interaction classes, not a claim of representative browsing speed:
// pan across groups, reverse direction, zoom out, revisit, then move to an edge.
#[test]
fn recorded_interaction_classes_preserve_pixels_across_group_geometry() {
    let info = ImageInfo::new(769, 513, Channels::Rgb, SampleType::U8);
    let stride = info.width as usize * 3;
    let raw: Vec<u8> = (0..stride * info.height as usize)
        .map(|i| ((i * 13 + i / 997) % 251) as u8)
        .collect();
    let trajectory = [
        (220, 50, 90, 65),
        (350, 50, 90, 65),
        (220, 50, 90, 65),
        (100, 20, 500, 300),
        (220, 50, 90, 65),
        (700, 480, 69, 33),
    ];
    for (columns, rows) in [(128, 64), (256, 128), (512, 256)] {
        let mut encode = imij::ImiEncoderConfig::default();
        encode = encode.with_column_groups(Some(columns));
        encode = encode.tune_lossless(|t| {
            t.stripe_rows = Some(rows);
        });
        let file = encode
            .encode_samples(
                imij::ImageRef::new(&raw, stride, &info),
                &Default::default(),
            )
            .unwrap();
        for ranges in [false, true] {
            for budget in [0, 32 * 1024 * 1024] {
                let mut cfg = config();
                cfg.ranges = ranges;
                cfg.cache_bytes = budget;
                cfg.latency_ms = 160.0;
                let ledger = Rc::new(RefCell::new(Ledger::default()));
                let source = Transport::new(
                    SliceReader::new(&file),
                    "recorded-pattern".into(),
                    cfg,
                    ledger.clone(),
                )
                .unwrap();
                let mut decoder = RegionReader::open(source).unwrap();
                for (x, y, width, height) in trajectory {
                    let row = width as usize * 3;
                    let output_stride = row + 7;
                    let mut pixels = vec![0xcc; output_stride * height as usize];
                    decoder
                        .decode_region(Rect::new(x, y, width, height), &mut pixels, output_stride)
                        .unwrap();
                    for r in 0..height as usize {
                        let input = (y as usize + r) * stride + x as usize * 3;
                        assert_eq!(
                            &pixels[r * output_stride..r * output_stride + row],
                            &raw[input..input + row]
                        );
                        assert!(
                            pixels[r * output_stride + row..(r + 1) * output_stride]
                                .iter()
                                .all(|&b| b == 0xcc)
                        );
                    }
                }
                let l = ledger.borrow();
                assert!(l.transferred_bytes > 0);
                assert!(l.now_ms.is_finite());
                assert!(l.events.iter().all(|e| e.end_ms >= e.start_ms));
            }
        }
    }
}

#[test]
fn unit_ordinals_reduce_cold_region_open_requests_and_flat_morton_pyramid_catalog_roundtrips() {
    let info = ImageInfo::new(769, 513, Channels::Rgb, SampleType::U8);
    let stride = info.width as usize * 3;
    let raw: Vec<u8> = (0..stride * info.height as usize)
        .map(|i| ((i * 13 + i / 997) % 251) as u8)
        .collect();
    let mut base_cfg = imij::ImiEncoderConfig::default();
    base_cfg = base_cfg.with_column_groups(Some(128));
    base_cfg = base_cfg.tune_lossless(|t| {
        t.stripe_rows = Some(32);
    });
    let ord_cfg = base_cfg.clone().with_unit_ordinals(true);
    let file_base = base_cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&raw, stride, &info),
            &Default::default(),
        )
        .unwrap();
    let file_ord = ord_cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&raw, stride, &info),
            &Default::default(),
        )
        .unwrap();

    let run_cold = |file: &[u8]| -> (usize, usize, Vec<u8>) {
        let mut cfg = config();
        cfg.cache_bytes = 0;
        cfg.latency_ms = 160.0;
        cfg.concurrency = 8;
        cfg.merge_gap = 4096;
        let ledger = Rc::new(RefCell::new(Ledger::default()));
        let source =
            Transport::new(SliceReader::new(file), "cold".into(), cfg, ledger.clone()).unwrap();
        let mut reader = RegionReader::open(source).unwrap();
        let mut out = vec![0u8; 90 * 65 * 3];
        reader
            .decode_region(Rect::new(220, 50, 90, 65), &mut out, 90 * 3)
            .unwrap();
        let l = ledger.borrow();
        let requests: usize = l.events.iter().map(|e| e.ranges.len()).sum();
        (requests, l.rounds, out)
    };
    let (req_base, rounds_base, px_base) = run_cold(&file_base);
    let (req_ord, rounds_ord, px_ord) = run_cold(&file_ord);
    assert_eq!(px_base, px_ord);
    // unit_ordinals eliminates the open-time record-head verification round and its requests.
    assert!(
        rounds_ord < rounds_base && req_ord < req_base,
        "base=({req_base} reqs, {rounds_base} rounds) ord=({req_ord} reqs, {rounds_ord} rounds)"
    );

    // Verify Morton 2D bijection and flat in-archive PyramidCatalog at CATALOG_KEY (0).
    for ty in 0..19u32 {
        for tx in 0..23u32 {
            let z = imij_set::morton_encode_2d(tx, ty);
            assert_eq!(imij_set::morton_decode_2d(z), (tx, ty));
        }
    }
    let l0_morton =
        imij_set::PyramidLevel::new(0, 769, 513, 128, 128, imij_set::TileOrder::Morton, 1).unwrap();
    let l1_row =
        imij_set::PyramidLevel::new(1, 385, 257, 128, 128, imij_set::TileOrder::RowMajor, 1000)
            .unwrap();
    let catalog = imij_set::PyramidCatalog::new(vec![l0_morton.clone(), l1_row.clone()]).unwrap();
    let catalog_bytes = catalog.encode();
    assert_eq!(
        imij_set::PyramidCatalog::decode(&catalog_bytes).unwrap(),
        catalog
    );
    // Overlapping key domains or key_base == 0 are rejected.
    assert!(
        imij_set::PyramidLevel::new(0, 64, 64, 32, 32, imij_set::TileOrder::Morton, 0).is_err()
    );
    let overlap =
        imij_set::PyramidLevel::new(1, 64, 64, 32, 32, imij_set::TileOrder::RowMajor, 10).unwrap();
    assert!(imij_set::PyramidCatalog::new(vec![l0_morton.clone(), overlap]).is_err());

    // Build a real flat pyramid archive with key 0 = catalog and Morton-ordered tiles for level 0.
    let mut writer = Writer::new(Vec::new())
        .with_index_placement(IndexPlacement::AtCommit)
        .unwrap();
    writer
        .insert(
            imij_set::CATALOG_KEY,
            catalog_bytes.len() as u64,
            0,
            Summary::default(),
            &mut catalog_bytes.as_slice(),
        )
        .unwrap();
    let mut tiles = Vec::new();
    for ty in 0..l0_morton.rows() {
        for tx in 0..l0_morton.cols() {
            let x = tx * 128;
            let y = ty * 128;
            let tw = (769 - x).min(128);
            let th = (513 - y).min(128);
            let tinfo = ImageInfo::new(tw, th, Channels::Rgb, SampleType::U8);
            let mut tpx = vec![0u8; (tw * th * 3) as usize];
            for r in 0..th as usize {
                let src = ((y as usize + r) * 769 + x as usize) * 3;
                tpx[r * tw as usize * 3..(r + 1) * tw as usize * 3]
                    .copy_from_slice(&raw[src..src + tw as usize * 3]);
            }
            let encoded = imij::ImiEncoderConfig::default()
                .encode_samples(
                    imij::ImageRef::new(&tpx, tw as usize * 3, &tinfo),
                    &Default::default(),
                )
                .unwrap();
            tiles.push((l0_morton.tile_key(tx, ty).unwrap(), tw, th, encoded));
        }
    }
    tiles.sort_by_key(|t| t.0);
    for (key, tw, th, encoded) in &tiles {
        let mut s = Summary::default();
        s.width = *tw;
        s.height = *th;
        writer
            .insert(*key, encoded.len() as u64, 0, s, &mut encoded.as_slice())
            .unwrap();
    }
    let archive = writer.finish().unwrap().0;
    let mut reader = Reader::open(SliceReader::new(&archive)).unwrap();
    let cat_entry = reader.get(imij_set::CATALOG_KEY).unwrap().unwrap();
    let parsed_cat = imij_set::PyramidCatalog::decode(
        &reader.read_entry_bounded(&cat_entry, 64 * 1024).unwrap(),
    )
    .unwrap();
    let level0 = parsed_cat.level(0).unwrap();
    let keys = level0.keys_for_rect(100, 100, 200, 180).unwrap();
    assert_eq!(keys.len(), 9);
    for entry in reader.get_many(&keys).unwrap() {
        let entry = entry.unwrap();
        let bytes = reader
            .read_entry_bounded(&entry, entry.len() as usize)
            .unwrap();
        let (decoded_info, _) = imij::decode(&bytes).unwrap();
        assert_eq!(decoded_info.width, entry.summary().width);
        assert_eq!(decoded_info.height, entry.summary().height);
    }
}
