use super::*;
use crate::decode_resources::{DecodePolicy, DecodeSession};
use crate::planes::ReadAt;
use crate::{Channels, ImageInfo, ImiEncoderConfig, RowGroups, SampleType};
use alloc::{vec, vec::Vec};
use core::ops::Range;
use std::sync::{Arc, Mutex, mpsc};

mod checksum;

struct Source {
    bytes: Arc<Vec<u8>>,
    reads: Arc<Mutex<Vec<Range<u64>>>>,
    allowed: Arc<Mutex<Option<Range<u64>>>>,
}
impl Source {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes: Arc::new(bytes),
            reads: Arc::default(),
            allowed: Arc::default(),
        }
    }
}
impl ReadAt for Source {
    fn len(&mut self) -> Result<u64, crate::Error> {
        Ok(self.bytes.len() as u64)
    }
    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), crate::Error> {
        let range = offset..offset + out.len() as u64;
        if let Some(allowed) = self.allowed.lock().unwrap().as_ref() {
            assert!(
                range.start >= allowed.start && range.end <= allowed.end,
                "unexpected read {range:?}, allowed {allowed:?}"
            );
        }
        self.reads.lock().unwrap().push(range.clone());
        out.copy_from_slice(&self.bytes[range.start as usize..range.end as usize]);
        Ok(())
    }
}

fn spatial(mode: ObjectIntegrity, order: TileOrder) -> SpatialConfig {
    SpatialConfig {
        tile_columns: 3,
        tile_rows: 3,
        order,
        integrity: mode,
        limits: TileLimits {
            max_object_bytes: 1 << 20,
            max_tile_bytes: 16 << 20,
        },
    }
}

// Independent physical framing walk: no production directory lookup is used
// to decide which byte range must be fetched.
pub(super) fn physical_tiles(bytes: &[u8], count: usize) -> (Range<u64>, Vec<(u64, Range<u64>)>) {
    let mut r = crate::ByteReader::new(bytes);
    crate::parse::header::read_header_v4(&mut r, 64 << 10).unwrap();
    loop {
        let (tag, len) = crate::parse::chunks::read_head(&mut r).unwrap();
        if tag == *b"DATA" {
            break;
        }
        r.bytes(len.unwrap() as usize).unwrap();
    }
    let length = r.var_usize().unwrap();
    r.bytes(length).unwrap();
    let placement = r.varint().unwrap();
    if placement == 2 {
        let capacity = r.var_usize().unwrap();
        r.bytes(capacity).unwrap();
    }
    let objects_at = r.pos as u64;
    let mut tiles = Vec::new();
    let mut start = None;
    while tiles.len() < count {
        let at = r.pos as u64;
        let kind = r.varint().unwrap();
        let length = r.var_usize().unwrap();
        let body = r.bytes(length).unwrap();
        if kind == 65 {
            start = Some((crate::ByteReader::new(body).varint().unwrap(), at));
        } else if kind == 67 {
            let (id, at) = start.take().unwrap();
            assert_eq!(id, crate::ByteReader::new(body).varint().unwrap());
            tiles.push((id, at..r.pos as u64));
        }
    }
    (objects_at..r.pos as u64, tiles)
}

#[test]
fn one_physical_tile_read_supplies_parallel_jobs_and_parser_can_advance() {
    let info = ImageInfo::new(79, 53, Channels::Rgba, SampleType::U16);
    let stride = info.row_bytes() + 13;
    let pixels: Vec<u8> = (0..stride * info.height as usize)
        .map(|i| (i * 73 + i / 7 * 41) as u8)
        .collect();
    for quick in [false, true] {
        let config = if quick {
            ImiEncoderConfig::quick()
        } else {
            ImiEncoderConfig::lossless()
        }
        .with_row_groups(RowGroups::new(9, 7, 8 << 20));
        for mode in [
            ObjectIntegrity::None,
            ObjectIntegrity::Objects,
            ObjectIntegrity::ObjectsAndTiles,
        ] {
            for order in [TileOrder::Raster, TileOrder::Morton] {
                let spatial = spatial(mode, order);
                let tail = encode_numeric(
                    info.clone(),
                    &pixels,
                    stride,
                    &Default::default(),
                    &config,
                    spatial,
                )
                .unwrap();
                for (placement, open) in [
                    (DirectoryPlacement::Tail, false),
                    (DirectoryPlacement::Tail, true),
                    (DirectoryPlacement::Front, false),
                    (DirectoryPlacement::Front, true),
                ] {
                    let bytes = super::file_tests::rewrite_placement(&tail, placement, open);
                    let (objects, physical) = physical_tiles(&bytes, 9);
                    let source = Source::new(bytes);
                    let reads = Arc::clone(&source.reads);
                    let allowed = Arc::clone(&source.allowed);
                    let session = DecodeSession::new(DecodePolicy::default());
                    let mut reader =
                        NumericTileReader::open(source, spatial.limits, &session).unwrap();
                    assert_eq!(reader.header().info, info);
                    let opening_bytes: u64 =
                        reads.lock().unwrap().iter().map(|r| r.end - r.start).sum();
                    assert!(
                        reads
                            .lock()
                            .unwrap()
                            .iter()
                            .all(|r| r.end <= objects.start || r.start >= objects.end),
                        "open read pixel objects"
                    );
                    let mut cumulative = opening_bytes;
                    for threads in [1, 2, 4, 8] {
                        let expected = physical.iter().find(|(id, _)| *id == 4).unwrap().1.clone();
                        assert_eq!(reader.tile_range(4).unwrap(), expected);
                        reads.lock().unwrap().clear();
                        *allowed.lock().unwrap() = Some(expected.clone());
                        let mut compressed = Vec::new();
                        let mut jobs = Vec::new();
                        reader
                            .read_tile(4, &mut compressed, |work| {
                                assert_eq!(work.integrity_verified, mode != ObjectIntegrity::None);
                                jobs.push(work);
                                Ok(())
                            })
                            .unwrap();
                        assert_eq!(*reads.lock().unwrap(), vec![expected.clone()]);
                        cumulative += expected.end - expected.start;
                        assert_eq!(jobs.len(), 9);
                        let queue = Mutex::new(jobs.into_iter());
                        std::thread::scope(|scope| {
                            // Sender drop releases workers even when an
                            // assertion fails before the second fetch finishes.
                            let (release, start) = mpsc::channel::<()>();
                            let start = Arc::new(Mutex::new(start));
                            for _ in 0..threads {
                                let (queue, start, pixels, info) =
                                    (&queue, Arc::clone(&start), &pixels, &info);
                                scope.spawn(move || {
                                    let _ = start.lock().unwrap().recv();
                                    let worker = DecodeSession::new(
                                        DecodePolicy::default().with_max_working_bytes(8 << 20),
                                    );
                                    loop {
                                        let Some(work) = queue.lock().unwrap().next() else {
                                            break;
                                        };
                                        let rect = work.rect().unwrap();
                                        let row = rect.width() as usize * info.bytes_per_pixel();
                                        let out_stride = row + 11;
                                        let mut output =
                                            vec![0xc3; out_stride * rect.height() as usize];
                                        work.decode_into(&mut output, out_stride, &worker).unwrap();
                                        for y in 0..rect.height() as usize {
                                            let at = (rect.y() as usize + y) * stride
                                                + rect.x() as usize * info.bytes_per_pixel();
                                            assert_eq!(
                                                &output[y * out_stride..y * out_stride + row],
                                                &pixels[at..at + row]
                                            );
                                            assert!(
                                                output[y * out_stride + row..(y + 1) * out_stride]
                                                    .iter()
                                                    .all(|&b| b == 0xc3)
                                            );
                                        }
                                    }
                                });
                            }
                            // Earlier jobs remain live while the same source/parser
                            // fetches and validates a different tile.
                            let next = physical.iter().find(|(id, _)| *id == 0).unwrap().1.clone();
                            reads.lock().unwrap().clear();
                            *allowed.lock().unwrap() = Some(next.clone());
                            let mut second = Vec::new();
                            reader.read_tile(0, &mut second, |_| Ok(())).unwrap();
                            assert_eq!(*reads.lock().unwrap(), vec![next.clone()]);
                            cumulative += next.end - next.start;
                            drop(release);
                        });
                        assert_eq!(session.usage().input_bytes, cumulative);
                    }
                }
            }
        }
    }
}

#[test]
fn sparse_limits_corruption_and_metadata_keep_their_scope() {
    let info = ImageInfo::new(79, 53, Channels::Gray, SampleType::U8);
    let cfg = ImiEncoderConfig::lossless().with_row_groups(RowGroups::new(9, 7, 8 << 20));
    let spatial = spatial(ObjectIntegrity::ObjectsAndTiles, TileOrder::Morton);
    let pixels = vec![137; info.row_bytes() * info.height as usize];
    let metadata = crate::Metadata {
        orientation: 6,
        xmp: Some(b"<metadata/>".to_vec()),
        ..Default::default()
    };
    let bytes = encode_numeric(
        info.clone(),
        &pixels,
        info.row_bytes(),
        &metadata,
        &cfg,
        spatial,
    )
    .unwrap();
    let (_, tiles) = physical_tiles(&bytes, 9);
    let expected = tiles.iter().find(|(id, _)| *id == 4).unwrap().1.clone();
    let source = Source::new(bytes.clone());
    let requests = Arc::clone(&source.reads);
    let session = DecodeSession::new(DecodePolicy::default().with_max_input_bytes(0));
    assert!(NumericTileReader::open(source, spatial.limits, &session).is_err());
    assert!(requests.lock().unwrap().is_empty());
    let session = DecodeSession::new(DecodePolicy::default());
    let mut reader =
        NumericTileReader::open(Source::new(bytes.clone()), spatial.limits, &session).unwrap();
    assert_eq!(reader.header().orientation, 6);
    let mut compressed = Vec::new();
    let mut groups = 0;
    reader
        .read_tile(4, &mut compressed, |work| {
            let rect = work.rect()?;
            let row = rect.width() as usize;
            let mut out = vec![0; row * rect.height() as usize];
            let worker = DecodeSession::new(DecodePolicy::default());
            work.decode_into(&mut out, row, &worker)?;
            assert!(out.iter().all(|&b| b == 137));
            groups += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(groups, 9);
    let mut corrupted = bytes.clone();
    corrupted[expected.start as usize + 5] ^= 1;
    let session = DecodeSession::new(DecodePolicy::default());
    let mut reader =
        NumericTileReader::open(Source::new(corrupted), spatial.limits, &session).unwrap();
    assert!(reader.read_tile(4, &mut Vec::new(), |_| Ok(())).is_err());
    let session = DecodeSession::new(DecodePolicy::default());
    let mut tiny = spatial.limits;
    tiny.max_tile_bytes = 1;
    let source = Source::new(bytes.clone());
    let requests = Arc::clone(&source.reads);
    let mut reader = NumericTileReader::open(source, tiny, &session).unwrap();
    requests.lock().unwrap().clear();
    assert!(reader.read_tile(4, &mut Vec::new(), |_| Ok(())).is_err());
    assert!(requests.lock().unwrap().is_empty());
    for cut in [0, 8, bytes.len() - 1, bytes.len() - 12] {
        let session = DecodeSession::new(DecodePolicy::default());
        assert!(
            NumericTileReader::open(Source::new(bytes[..cut].to_vec()), spatial.limits, &session)
                .is_err()
        );
    }
    let session = DecodeSession::with_cancel(DecodePolicy::default(), || true);
    assert!(NumericTileReader::open(Source::new(bytes), spatial.limits, &session).is_err());
}
