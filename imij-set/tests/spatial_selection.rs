#[path = "../examples/access_model/selection.rs"]
mod selection;
#[path = "../examples/access_model/source.rs"]
mod source;
#[path = "../examples/access_model/transport.rs"]
mod transport;

use imij::{
    Channels, ImageInfo, SampleType,
    planes::{ReadAt, Rect, SliceReader},
};
use imij_set::{IndexPlacement, PyramidCatalog, PyramidLevel, Reader, Summary, TileOrder, Writer};
use selection::{Delivery, Limits, PyramidReader, Selection};
use std::{
    cell::{Cell, RefCell},
    io,
    rc::Rc,
};
use transport::{Config, Ledger, SharedSource, Transport};

fn config(cache: u64, ranges: bool) -> Config {
    Config {
        ranges,
        cache_bytes: cache,
        latency_ms: 40.0,
        bytes_per_ms: 1250.0,
        concurrency: 4,
        merge_gap: 0,
        max_range: 1024 * 1024,
        part_size: 0,
        shared_cache: false,
    }
}
fn fixture(
    channels: Channels,
    sample: SampleType,
    bad: bool,
) -> (Vec<u8>, Vec<(ImageInfo, Vec<u8>)>) {
    fixture_options(channels, sample, bad, false, false)
}

// Retained V3 ordinal-comparison controls; ordinary fixtures above use V4.
fn fixture_with_ordinals(
    channels: Channels,
    sample: SampleType,
    bad: bool,
    ordinals: bool,
) -> (Vec<u8>, Vec<(ImageInfo, Vec<u8>)>) {
    fixture_options(channels, sample, bad, ordinals, true)
}
fn fixture_options(
    channels: Channels,
    sample: SampleType,
    bad: bool,
    ordinals: bool,
    legacy: bool,
) -> (Vec<u8>, Vec<(ImageInfo, Vec<u8>)>) {
    let levels = vec![
        PyramidLevel::new(0, 513, 257, 256, 128, TileOrder::Morton, 1).unwrap(),
        PyramidLevel::new(1, 257, 129, 128, 64, TileOrder::RowMajor, 1000).unwrap(),
    ];
    let catalog = PyramidCatalog::new(levels.clone()).unwrap().encode();
    let mut payloads = vec![(0, catalog)];
    let mut references = Vec::new();
    let bpp = channels.count() * sample.bytes();
    for level in &levels {
        let info = ImageInfo::new(level.width, level.height, channels, sample);
        let raw: Vec<u8> = (0..level.width as usize * level.height as usize * bpp)
            .map(|i| {
                ((i.wrapping_mul(997) ^ (i >> 4) ^ (level.level_id as usize * 79)) % 251) as u8
            })
            .collect();
        for ty in 0..level.rows() {
            for tx in 0..level.cols() {
                let (x, y) = (tx * level.tile_width, ty * level.tile_height);
                let (w, h) = (
                    (level.width - x).min(level.tile_width),
                    (level.height - y).min(level.tile_height),
                );
                let mut tile_info = info.clone();
                tile_info.width = w;
                tile_info.height = h;
                if bad && level.level_id == 0 && tx == 1 && ty == 0 {
                    tile_info.cicp = imij::Cicp::BT2100_PQ;
                }
                let stride = w as usize * bpp + 7;
                let mut pixels = vec![0xa5; stride * h as usize];
                for row in 0..h as usize {
                    let start = ((y as usize + row) * level.width as usize + x as usize) * bpp;
                    pixels[row * stride..row * stride + w as usize * bpp]
                        .copy_from_slice(&raw[start..start + w as usize * bpp]);
                }
                let encoder = imij::ImiEncoderConfig::default()
                    .with_unit_ordinals(ordinals)
                    .with_column_groups(Some(32))
                    .tune_lossless(|t| t.stripe_rows = Some(8));
                let image = imij::ImageRef::new(&pixels, stride, &tile_info);
                let bytes = if legacy {
                    encoder.encode_legacy_samples(image, &Default::default())
                } else {
                    encoder.encode_samples(image, &Default::default())
                }
                .unwrap();
                payloads.push((level.tile_key(tx, ty).unwrap(), bytes));
            }
        }
        references.push((info, raw));
    }
    payloads.sort_unstable_by_key(|p| p.0);
    let mut writer = Writer::new(Vec::new())
        .with_index_placement(IndexPlacement::AtCommit)
        .unwrap();
    for (key, bytes) in payloads {
        writer
            .insert(
                key,
                bytes.len() as u64,
                0,
                Summary::default(),
                &mut bytes.as_slice(),
            )
            .unwrap();
    }
    (writer.finish().unwrap().0, references)
}
fn crop(info: &ImageInfo, bytes: &[u8], r: Rect) -> Vec<u8> {
    let bpp = info.channels.count() * info.sample.bytes();
    (0..r.height() as usize)
        .flat_map(|y| {
            let start = ((r.y() as usize + y) * info.width as usize + r.x() as usize) * bpp;
            bytes[start..start + r.width() as usize * bpp]
                .iter()
                .copied()
        })
        .collect()
}

#[test]
fn selected_level_regions_match_full_level_samples_across_delivery_and_cache_modes() {
    for (channels, sample) in [
        (Channels::Rgb, SampleType::U8),
        (Channels::Rgba, SampleType::U16),
        (Channels::Gray, SampleType::F16),
        (Channels::Rgb, SampleType::F32),
    ] {
        let (archive, references) = fixture(channels, sample, false);
        for ranges in [false, true] {
            for cache in [0, 32768, archive.len() as u64] {
                for delivery in [Delivery::NativeRanges, Delivery::WholeEntries] {
                    let ledger = Rc::new(RefCell::new(Ledger::default()));
                    let source = SharedSource::new(
                        Transport::new(
                            SliceReader::new(&archive),
                            "immutable-v1".into(),
                            config(cache, ranges),
                            ledger.clone(),
                        )
                        .unwrap(),
                    );
                    let mut reader = PyramidReader::open(source, Limits::default()).unwrap();
                    for selection in [
                        Selection {
                            level: 0,
                            rect: Rect::new(250, 120, 175, 99),
                        },
                        Selection {
                            level: 1,
                            rect: Rect::new(120, 60, 137, 69),
                        },
                        Selection {
                            level: 0,
                            rect: Rect::new(250, 120, 175, 99),
                        },
                    ] {
                        let output = reader.decode(selection, delivery, || Ok(())).unwrap();
                        let (info, raw) = &references[selection.level as usize];
                        assert_eq!(output.bytes, crop(info, raw, selection.rect));
                        assert_eq!(
                            output.stride,
                            selection.rect.width() as usize
                                * info.channels.count()
                                * info.sample.bytes()
                        );
                        assert_eq!(output.info.sample, sample);
                    }
                    let l = ledger.borrow();
                    assert_eq!(
                        l.transferred_bytes,
                        l.events
                            .iter()
                            .flat_map(|e| &e.ranges)
                            .map(|r| r.end - r.start)
                            .sum()
                    );
                    assert!(l.events.iter().all(|e| {
                        e.end_ms >= e.start_ms
                            && e.status == if ranges { 206 } else { 200 }
                            && e.logical_base == 0
                            && e.object_length == archive.len() as u64
                    }));
                    println!(
                        "spatial_model\t{channels:?}\t{sample:?}\t{ranges}\t{cache}\t{}\t{}\t{}\t{}\t{}\t{:.6}",
                        match delivery {
                            Delivery::NativeRanges => "native_ranges",
                            Delivery::WholeEntries => "whole_entries",
                        },
                        l.events.iter().map(|e| e.ranges.len()).sum::<usize>(),
                        l.rounds,
                        l.transferred_bytes,
                        l.cache_hit_bytes,
                        l.now_ms,
                    );
                }
            }
        }
    }
}

#[test]
fn selected_nested_item_does_not_confuse_tile_keys_with_document_item_identity() {
    let (archive, references) = fixture(Channels::Rgb, SampleType::U8, false);
    let mut outer = Writer::new(Vec::new());
    for key in [7, 42] {
        outer
            .insert(
                key,
                archive.len() as u64,
                0,
                Summary::default(),
                &mut archive.as_slice(),
            )
            .unwrap();
    }
    let bytes = outer.finish().unwrap().0;
    let ledger = Rc::new(RefCell::new(Ledger::default()));
    let source = SharedSource::new(
        Transport::new(
            SliceReader::new(&bytes),
            "nested-v1".into(),
            config(32768, true),
            ledger,
        )
        .unwrap(),
    );
    let mut tree = source::ArchiveTree::new(source).unwrap();
    for item in [7, 42] {
        let mut pyramid =
            PyramidReader::open(tree.source(&[item]).unwrap(), Limits::default()).unwrap();
        let r = Rect::new(120, 60, 130, 60);
        let output = pyramid
            .decode(
                Selection { level: 1, rect: r },
                Delivery::WholeEntries,
                || Ok(()),
            )
            .unwrap();
        assert_eq!(output.bytes, crop(&references[1].0, &references[1].1, r));
    }
}

#[test]
fn invalid_selection_and_admission_refuse_without_data_requests() {
    let (archive, _) = fixture(Channels::Rgb, SampleType::U8, false);
    for (selection, limits) in [
        (
            Selection {
                level: 99,
                rect: Rect::new(0, 0, 1, 1),
            },
            Limits::default(),
        ),
        (
            Selection {
                level: 0,
                rect: Rect::new(u32::MAX, 0, 10, 1),
            },
            Limits::default(),
        ),
        (
            Selection {
                level: 0,
                rect: Rect::new(0, 0, 0, 1),
            },
            Limits::default(),
        ),
        (
            Selection {
                level: 0,
                rect: Rect::new(250, 120, 175, 99),
            },
            Limits {
                tiles: 1,
                ..Limits::default()
            },
        ),
        (
            Selection {
                level: 0,
                rect: Rect::new(0, 0, 100, 100),
            },
            Limits {
                output_bytes: 1,
                ..Limits::default()
            },
        ),
    ] {
        let ledger = Rc::new(RefCell::new(Ledger::default()));
        let source = SharedSource::new(
            Transport::new(
                SliceReader::new(&archive),
                "immutable-v1".into(),
                config(0, true),
                ledger.clone(),
            )
            .unwrap(),
        );
        let mut reader = PyramidReader::open(source, limits).unwrap();
        let before = ledger.borrow().transferred_bytes;
        assert!(
            reader
                .decode(selection, Delivery::NativeRanges, || Ok(()))
                .is_err()
        );
        assert_eq!(ledger.borrow().transferred_bytes, before);
    }
}

#[test]
fn final_view_cancellation_publishes_nothing_and_preserves_immutable_cache() {
    let (archive, references) = fixture(Channels::Rgb, SampleType::U8, false);
    let ledger = Rc::new(RefCell::new(Ledger::default()));
    let source = SharedSource::new(
        Transport::new(
            SliceReader::new(&archive),
            "immutable-v1".into(),
            config(archive.len() as u64, true),
            ledger.clone(),
        )
        .unwrap(),
    );
    let mut reader = PyramidReader::open(source, Limits::default()).unwrap();
    let selection = Selection {
        level: 0,
        rect: Rect::new(250, 120, 175, 99),
    };
    let calls = Cell::new(0);
    let output = reader
        .decode(selection, Delivery::NativeRanges, || {
            calls.set(calls.get() + 1);
            Ok(())
        })
        .unwrap();
    let count = calls.get();
    calls.set(0);
    let error = reader
        .decode(selection, Delivery::NativeRanges, || {
            calls.set(calls.get() + 1);
            if calls.get() == count {
                Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "obsolete viewport",
                ))
            } else {
                Ok(())
            }
        })
        .err()
        .expect("stale output must not be returned");
    assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    let before = ledger.borrow().transferred_bytes;
    let current = reader
        .decode(selection, Delivery::NativeRanges, || Ok(()))
        .unwrap();
    assert_eq!(
        ledger.borrow().transferred_bytes,
        before,
        "cancellation must not invalidate immutable bytes"
    );
    assert_eq!(current.bytes, output.bytes);
    assert_eq!(
        current.bytes,
        crop(&references[0].0, &references[0].1, selection.rect)
    );
}

#[test]
fn mixed_tile_interpretation_never_returns_a_partially_stitched_region() {
    let (archive, _) = fixture(Channels::Rgb, SampleType::U8, true);
    for delivery in [Delivery::NativeRanges, Delivery::WholeEntries] {
        let ledger = Rc::new(RefCell::new(Ledger::default()));
        let source = SharedSource::new(
            Transport::new(
                SliceReader::new(&archive),
                "mixed".into(),
                config(0, true),
                ledger,
            )
            .unwrap(),
        );
        let mut reader = PyramidReader::open(source, Limits::default()).unwrap();
        let result = reader.decode(
            Selection {
                level: 0,
                rect: Rect::new(250, 20, 30, 20),
            },
            delivery,
            || Ok(()),
        );
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("inconsistent tile sample interpretation")
        );
    }
}

#[test]
fn encoded_staging_refuses_before_fetching_tile_payloads() {
    let (archive, _) = fixture(Channels::Rgb, SampleType::U8, false);
    let mut native = Reader::open(SliceReader::new(&archive)).unwrap();
    let payloads: Vec<_> = [1, 2, 3, 4]
        .into_iter()
        .map(|key| {
            let e = native.get(key).unwrap().unwrap();
            e.offset()..e.offset() + e.len()
        })
        .collect();
    let ledger = Rc::new(RefCell::new(Ledger::default()));
    let source = SharedSource::new(
        Transport::new(
            SliceReader::new(&archive),
            "bounded".into(),
            config(0, true),
            ledger.clone(),
        )
        .unwrap(),
    );
    let mut reader = PyramidReader::open(
        source,
        Limits {
            staged_encoded_bytes: 1,
            ..Limits::default()
        },
    )
    .unwrap();
    let before = ledger.borrow().events.len();
    let error = reader
        .decode(
            Selection {
                level: 0,
                rect: Rect::new(250, 120, 175, 99),
            },
            Delivery::WholeEntries,
            || Ok(()),
        )
        .err()
        .unwrap();
    assert!(error.to_string().contains("encoded staging allowance"));
    assert!(
        ledger.borrow().events[before..]
            .iter()
            .flat_map(|e| &e.ranges)
            .all(|read| payloads
                .iter()
                .all(|p| read.end <= p.start || read.start >= p.end))
    );
}

/// A version-bound source contract, not a mock decoder. Partial failures mutate
/// caller buffers before returning an error, as ordinary I/O is permitted to do.
#[derive(Clone)]
struct FaultSource {
    bytes: Rc<Vec<u8>>,
    reads: Rc<Cell<usize>>,
    fail_at: Rc<Cell<Option<usize>>>,
    version: Rc<Cell<u64>>,
    invalidated: Rc<Cell<bool>>,
}
impl FaultSource {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes: Rc::new(bytes),
            reads: Rc::new(Cell::new(0)),
            fail_at: Rc::new(Cell::new(None)),
            version: Rc::new(Cell::new(1)),
            invalidated: Rc::new(Cell::new(false)),
        }
    }
    fn validate(&self) -> Result<(), imij::Error> {
        if self.version.get() != 1 {
            self.invalidated.set(true);
        }
        if self.invalidated.get() {
            Err(imij::Error::Corrupt("source version changed"))
        } else {
            Ok(())
        }
    }
}
impl ReadAt for FaultSource {
    fn len(&mut self) -> Result<u64, imij::Error> {
        self.validate()?;
        Ok(self.bytes.len() as u64)
    }
    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), imij::Error> {
        self.validate()?;
        self.reads.set(self.reads.get() + 1);
        if self.fail_at.get() == Some(self.reads.get()) {
            if let Some(byte) = out.first_mut() {
                *byte = 0xff;
            }
            return Err(imij::Error::Read);
        }
        SliceReader::new(&self.bytes).read_exact_at(offset, out)
    }
}

#[test]
fn partial_read_failure_and_changed_source_never_publish_a_region() {
    let (archive, references) = fixture(Channels::Rgb, SampleType::U8, false);
    let selected = Selection {
        level: 0,
        rect: Rect::new(250, 120, 175, 99),
    };
    for delivery in [Delivery::NativeRanges, Delivery::WholeEntries] {
        let reference = FaultSource::new(archive.clone());
        let mut reader = PyramidReader::open(reference.clone(), Limits::default()).unwrap();
        let opened = reference.reads.get();
        let retained = reader.decode(selected, delivery, || Ok(())).unwrap();
        let finished = reference.reads.get();
        assert!(finished > opened + 2);
        for fail_at in [opened + 1, (opened + finished) / 2, finished] {
            let source = FaultSource::new(archive.clone());
            source.fail_at.set(Some(fail_at));
            let mut reader = PyramidReader::open(source, Limits::default()).unwrap();
            let error = reader.decode(selected, delivery, || Ok(())).err().unwrap();
            assert!(error.to_string().contains("read error"), "{error}");
        }
        let source = FaultSource::new(archive.clone());
        let mut reader = PyramidReader::open(source.clone(), Limits::default()).unwrap();
        let checks = Cell::new(0);
        let result = reader.decode(selected, delivery, || {
            checks.set(checks.get() + 1);
            // Native ranges: the second tile. Whole entries: just before the
            // batched payload read. Both follow successful catalog/index reads.
            if checks.get()
                == if matches!(delivery, Delivery::NativeRanges) {
                    3
                } else {
                    2
                }
            {
                source.version.set(2);
            }
            Ok(())
        });
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("source version changed")
        );
        source.version.set(1);
        assert!(
            reader.decode(selected, delivery, || Ok(())).is_err(),
            "observed version replacement cannot revive the old source binding"
        );
        assert_eq!(
            retained.bytes,
            crop(&references[0].0, &references[0].1, selected.rect)
        );
    }
}

#[test]
fn corrupted_tile_header_is_not_published_by_either_delivery_path() {
    let (mut archive, _) = fixture(Channels::Rgb, SampleType::U8, false);
    let offset = Reader::open(SliceReader::new(&archive))
        .unwrap()
        .get(1)
        .unwrap()
        .unwrap()
        .offset() as usize;
    archive[offset] ^= 0xff;
    for delivery in [Delivery::NativeRanges, Delivery::WholeEntries] {
        let mut reader =
            PyramidReader::open(FaultSource::new(archive.clone()), Limits::default()).unwrap();
        assert!(
            reader
                .decode(
                    Selection {
                        level: 0,
                        rect: Rect::new(0, 0, 8, 8)
                    },
                    delivery,
                    || Ok(())
                )
                .is_err()
        );
    }
}

#[test]
fn ordinal_enabled_regions_keep_pixels_and_reduce_dependent_discovery() {
    let mut baseline_rounds = std::collections::BTreeMap::new();
    for ordinals in [false, true] {
        let (archive, references) =
            fixture_with_ordinals(Channels::Rgb, SampleType::U8, false, ordinals);
        for latency in [0.0, 40.0, 160.0] {
            for concurrency in [1, 4] {
                for delivery in [Delivery::NativeRanges, Delivery::WholeEntries] {
                    let ledger = Rc::new(RefCell::new(Ledger::default()));
                    let mut cfg = config(0, true);
                    cfg.latency_ms = latency;
                    cfg.concurrency = concurrency;
                    let source = SharedSource::new(
                        Transport::new(
                            SliceReader::new(&archive),
                            "ordinal-control".into(),
                            cfg,
                            ledger.clone(),
                        )
                        .unwrap(),
                    );
                    let mut reader = PyramidReader::open(source, Limits::default()).unwrap();
                    let r = Rect::new(250, 120, 175, 99);
                    let output = reader
                        .decode(Selection { level: 0, rect: r }, delivery, || Ok(()))
                        .unwrap();
                    assert_eq!(output.bytes, crop(&references[0].0, &references[0].1, r));
                    let l = ledger.borrow();
                    // These fixtures require one catalog lookup plus four tiles.
                    // The format's native ordinal validation removes the separate
                    // all-unit-head discovery round for each of those tiles.
                    if matches!(delivery, Delivery::NativeRanges) {
                        let key = (latency as u32, concurrency);
                        if ordinals {
                            assert_eq!(l.rounds + 4, baseline_rounds[&key]);
                        } else {
                            baseline_rounds.insert(key, l.rounds);
                        }
                    }
                    println!(
                        "spatial_ordinals\t{ordinals}\t{latency}\t{concurrency}\t{}\t{}\t{}\t{}\t{:.6}",
                        match delivery {
                            Delivery::NativeRanges => "native_ranges",
                            Delivery::WholeEntries => "whole_entries",
                        },
                        l.events.iter().map(|e| e.ranges.len()).sum::<usize>(),
                        l.rounds,
                        l.transferred_bytes,
                        l.now_ms
                    );
                }
            }
        }
    }
}
