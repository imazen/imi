//! Ordinary ROI output is compared with independently decoded native pixels.
use super::*;
use crate::planes::{ReadAt, Rect, SliceReader};
use crate::{Channels, Error, ImageInfo, ImiEncoderConfig, RowGroups, SampleType};
use alloc::{sync::Arc, vec, vec::Vec};
use core::ops::Range;
use std::sync::Mutex;

pub(crate) fn check(file: &[u8], expected: &[u8], info: &ImageInfo) {
    check_index(file, expected, info, true)
}
fn check_index(file: &[u8], expected: &[u8], info: &ImageInfo, index: bool) {
    for threads in [1, 2, 4, 8] {
        let mut reader = crate::region::RegionReader::open(SliceReader::new(file)).unwrap();
        assert_eq!(reader.info(), info);
        assert_eq!(reader.header().version, 4);
        assert_eq!(reader.has_unit_table(), index);
        for rect in [
            Rect::new(0, 0, info.width, info.height),
            Rect::new(info.width - 1, info.height - 1, 1, 1),
            Rect::new(3, 5, info.width - 6, info.height - 9),
        ] {
            let bpp = info.bytes_per_pixel();
            let row = rect.width() as usize * bpp;
            let stride = row + 19;
            let mut out = vec![0xD7; stride * rect.height() as usize];
            reader
                .decode_region_threads(rect, &mut out, stride, threads)
                .unwrap();
            for y in 0..rect.height() as usize {
                let at = (rect.y() as usize + y) * info.row_bytes() + rect.x() as usize * bpp;
                assert_eq!(
                    &out[y * stride..][..row],
                    &expected[at..][..row],
                    "{threads} workers, {rect:?}"
                );
                assert!(
                    out[y * stride + row..(y + 1) * stride]
                        .iter()
                        .all(|&b| b == 0xD7)
                );
            }
        }
    }
}

#[test]
fn ordinary_numeric_regions_cover_native_bits_and_morton_clipped_tiles() {
    for sample in [
        SampleType::U8,
        SampleType::U16,
        SampleType::U32,
        SampleType::U64,
        SampleType::I8,
        SampleType::I16,
        SampleType::I32,
        SampleType::I64,
        SampleType::F16,
        SampleType::F32,
        SampleType::F64,
    ] {
        for channels in [
            Channels::Gray,
            Channels::GrayAlpha,
            Channels::Rgb,
            Channels::Rgba,
            Channels::Cmyk,
            Channels::Oklab,
            Channels::OklabAlpha,
        ] {
            let info = ImageInfo::new(41, 31, channels, sample);
            let source: Vec<u8> = (0..info.checked_buffer_len().unwrap())
                .map(|i| (i * 73 + i / 7 * 41) as u8)
                .collect();
            for quick in [false, true] {
                let config = if quick {
                    ImiEncoderConfig::quick()
                } else {
                    ImiEncoderConfig::lossless()
                }
                .with_row_groups(RowGroups::new(9, 7, 8 << 20));
                let file = encode_numeric(
                    info.clone(),
                    &source,
                    info.row_bytes(),
                    &Default::default(),
                    &config,
                    SpatialConfig {
                        tile_columns: 3,
                        tile_rows: 2,
                        order: TileOrder::Morton,
                        integrity: ObjectIntegrity::ObjectsAndTiles,
                        limits: TileLimits {
                            max_object_bytes: 1 << 20,
                            max_tile_bytes: 16 << 20,
                        },
                    },
                )
                .unwrap();
                check(&file, &source, &info);
            }
        }
    }
}

struct Source {
    bytes: Arc<Vec<u8>>,
    reads: Arc<Mutex<Vec<Range<u64>>>>,
}
impl ReadAt for Source {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.bytes.len() as u64)
    }
    fn read_exact_at(&mut self, at: u64, out: &mut [u8]) -> Result<(), Error> {
        let end = at + out.len() as u64;
        self.reads.lock().unwrap().push(at..end);
        out.copy_from_slice(&self.bytes[at as usize..end as usize]);
        Ok(())
    }
}
#[test]
fn ordinary_region_fetches_one_complete_tile() {
    let info = ImageInfo::new(1009, 787, Channels::Rgba, SampleType::U16);
    let mut state = 1u64;
    let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect();
    for quick in [false, true] {
        let config = if quick {
            ImiEncoderConfig::quick()
        } else {
            ImiEncoderConfig::lossless()
        }
        .with_row_groups(RowGroups::new(128, 64, 64 << 20));
        let file = encode_numeric(
            info.clone(),
            &pixels,
            info.row_bytes(),
            &Default::default(),
            &config,
            SpatialConfig {
                tile_columns: 3,
                tile_rows: 2,
                order: TileOrder::Morton,
                integrity: ObjectIntegrity::ObjectsAndTiles,
                limits: TileLimits {
                    max_object_bytes: 1 << 20,
                    max_tile_bytes: 16 << 20,
                },
            },
        )
        .unwrap();
        for placement in [
            DirectoryPlacement::None,
            DirectoryPlacement::Tail,
            DirectoryPlacement::Front,
        ] {
            for open in [false, true] {
                let file = super::file_tests::rewrite_placement(&file, placement, open);
                let session = crate::decode_resources::DecodeSession::new(Default::default());
                let numeric = NumericFile::open(
                    &file,
                    TileLimits {
                        max_object_bytes: 1 << 20,
                        max_tile_bytes: 16 << 20,
                    },
                    &session,
                )
                .unwrap();
                let layout = numeric.layout();
                let rect = Rect::new(927, 705, 53, 57);
                let group = u64::from(rect.y() / 64) * u64::from(layout.group_grid().0)
                    + u64::from(rect.x() / 128);
                let tile = layout.tile_for_group(group).unwrap();
                let (_, physical) =
                    super::read_at_tests::physical_tiles(&file, layout.tile_count() as usize);
                let expected_range = physical
                    .into_iter()
                    .find(|(id, _)| *id == tile.id)
                    .unwrap()
                    .1;
                assert!(expected_range.start > 64 << 10);
                let reads = Arc::new(Mutex::new(Vec::new()));
                let source = Source {
                    bytes: Arc::new(file),
                    reads: Arc::clone(&reads),
                };
                let mut reader = crate::region::RegionReader::open(source).unwrap();
                assert_eq!(
                    reader.has_unit_table(),
                    placement != DirectoryPlacement::None
                );
                if placement == DirectoryPlacement::None {
                    let observed = reads.lock().unwrap();
                    assert_eq!(observed.first(), Some(&(0..(64 << 10))));
                    assert!(
                        observed[1..].iter().all(|r| r.end - r.start == 1),
                        "unindexed discovery must read identity/framing bytes, not entropy"
                    );
                }

                assert_eq!(
                    reader.read_stats().bytes,
                    reads
                        .lock()
                        .unwrap()
                        .iter()
                        .map(|r| r.end - r.start)
                        .sum::<u64>()
                );
                reader.reset_read_stats();
                reads.lock().unwrap().clear();
                assert_eq!(
                    reader.region_ranges(rect).unwrap(),
                    vec![expected_range.clone()]
                );
                let row = rect.width() as usize * info.bytes_per_pixel();
                let stride = row + 13;
                let mut output = vec![0xC1; stride * rect.height() as usize];
                reader
                    .decode_region_threads(rect, &mut output, stride, 8)
                    .unwrap();
                assert_eq!(*reads.lock().unwrap(), vec![expected_range.clone()]);
                assert_eq!(reader.read_stats().reads, 1);
                assert_eq!(
                    reader.read_stats().bytes,
                    expected_range.end - expected_range.start
                );
                for y in 0..rect.height() as usize {
                    let at = (rect.y() as usize + y) * info.row_bytes()
                        + rect.x() as usize * info.bytes_per_pixel();
                    assert_eq!(&output[y * stride..][..row], &pixels[at..][..row]);
                    assert!(
                        output[y * stride + row..(y + 1) * stride]
                            .iter()
                            .all(|&b| b == 0xC1)
                    );
                }
            }
        }
    }
}

#[test]
fn region_bitmap_checksums_crop_limits_and_late_failure_retain_their_scope() {
    let info = ImageInfo::new(59, 43, Channels::Rgba, SampleType::U8);
    let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 73 + i / 7 * 41) as u8)
        .collect();
    for lossy in [false, true] {
        for checksum in [false, true] {
            let config = if lossy {
                ImiEncoderConfig::lossy(73.0)
            } else {
                ImiEncoderConfig::lossless()
            }
            .with_column_groups(Some(16))
            .with_checksum(checksum);
            let native = config
                .encode_legacy_samples(crate::ImageRef::packed(&pixels, &info), &Default::default())
                .unwrap();
            let native = crate::pixelsum::add_pixel_checksum(native).unwrap();
            let file = upgrade_image(&native, &crate::UpgradeLimits::default()).unwrap();
            let expected = crate::decode(&native).unwrap().1;
            let limits = crate::Limits {
                max_pixels: 100,
                max_bytes: 64 << 20,
                verify_checksum: true,
            };
            let mut crop =
                crate::region::RegionReader::open_with_limits(SliceReader::new(&file), &limits)
                    .unwrap();
            assert_eq!(
                crop.pixel_checksum_status(),
                crate::PixelChecksumStatus::Pending
            );
            let mut out = vec![0xA3; 9 * 9 * 4];
            crop.decode_region_threads(Rect::new(49, 33, 9, 9), &mut out, 9 * 4, 4)
                .unwrap();
            assert_eq!(
                crop.pixel_checksum_status(),
                crate::PixelChecksumStatus::Pending
            );
            let mut denied = vec![0xA3; 11 * 11 * 4];
            let before = crop.read_stats();
            assert!(matches!(
                crop.decode_region(Rect::new(0, 0, 11, 11), &mut denied, 11 * 4),
                Err(Error::TooLarge)
            ));
            assert_eq!(crop.read_stats(), before);
            assert!(denied.iter().all(|&v| v == 0xA3));
            let mut full = crate::region::RegionReader::open(SliceReader::new(&file)).unwrap();
            let mut output = vec![0; expected.len()];
            full.decode_region_threads(
                Rect::new(0, 0, info.width, info.height),
                &mut output,
                info.row_bytes(),
                4,
            )
            .unwrap();
            assert_eq!(output, expected);
            assert_eq!(
                full.pixel_checksum_status(),
                crate::PixelChecksumStatus::Verified
            );
            // Corrupt only the bitmap digest, then rebuild the ordinary file seal.
            // Entropy and local object integrity remain valid. A partial crop must
            // remain provisional; full bitmap completion must reject late.
            let mut bad = file.clone();
            let mut r = crate::ByteReader::new(&file);
            crate::parse::header::read_header_v4(&mut r, 64 << 10).unwrap();
            loop {
                let (tag, len) = crate::parse::chunks::read_head(&mut r).unwrap();
                let len = len.unwrap() as usize;
                if tag == crate::pixelsum::PSUM {
                    bad[r.pos + len - 1] ^= 1;
                    break;
                }
                r.bytes(len).unwrap();
            }
            if checksum {
                let mut r = crate::ByteReader::new(&bad);
                crate::parse::header::read_header_v4(&mut r, 64 << 10).unwrap();
                let mut unsealed = bad[..r.pos].to_vec();
                while r.remaining() != 0 {
                    let at = r.pos;
                    let (tag, len) = crate::parse::chunks::read_head(&mut r).unwrap();
                    r.bytes(len.unwrap() as usize).unwrap();
                    if tag != *b"cSUM" {
                        unsealed.extend_from_slice(&bad[at..r.pos]);
                    }
                }
                bad = crate::checksum::refinish(unsealed).unwrap();
            }
            let mut reader = crate::region::RegionReader::open(SliceReader::new(&bad)).unwrap();
            reader
                .decode_region(Rect::new(49, 33, 9, 9), &mut out, 9 * 4)
                .unwrap();
            assert_eq!(
                reader.pixel_checksum_status(),
                crate::PixelChecksumStatus::Pending
            );
            assert!(matches!(
                reader.decode_region(
                    Rect::new(0, 0, info.width, info.height),
                    &mut output,
                    info.row_bytes()
                ),
                Err(Error::Corrupt("pixel checksum mismatch"))
            ));
            assert_eq!(
                reader.pixel_checksum_status(),
                crate::PixelChecksumStatus::Pending
            );
        }
    }
}

#[test]
fn observed_regions_stop_before_io_and_preserve_unwritten_output() {
    struct Stopped;
    impl how_far::Stop for Stopped {
        fn check(&self) -> Result<(), how_far::StopReason> {
            Err(how_far::StopReason::Cancelled)
        }
    }
    let info = ImageInfo::new(41, 31, Channels::Rgba, SampleType::U8);
    let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 73) as u8)
        .collect();
    for lossy in [false, true] {
        let native = if lossy {
            ImiEncoderConfig::lossy(73.0)
        } else {
            ImiEncoderConfig::lossless()
        }
        .encode_legacy_samples(crate::ImageRef::packed(&pixels, &info), &Default::default())
        .unwrap();
        let file = upgrade_image(&native, &crate::UpgradeLimits::default()).unwrap();
        let mut reader = crate::region::RegionReader::open(SliceReader::new(&file)).unwrap();
        let pulse = how_far::StopOnly::new(Stopped);
        let before = reader.read_stats();
        let mut output = vec![0xA7; pixels.len()];
        assert!(matches!(
            reader.decode_region_observed(
                Rect::new(0, 0, info.width, info.height),
                &mut output,
                info.row_bytes(),
                8,
                &pulse
            ),
            Err(Error::Cancelled)
        ));
        assert_eq!(reader.read_stats(), before);
        assert!(output.iter().all(|&b| b == 0xA7));
        // Per-region cancellation is not a poisoned file or a latched global
        // request. A new uncancelled request still reconstructs the same file.
        reader
            .decode_region(
                Rect::new(0, 0, info.width, info.height),
                &mut output,
                info.row_bytes(),
            )
            .unwrap();
        assert_eq!(output, crate::decode(&native).unwrap().1);
    }
}

#[test]
fn unindexed_and_indexed_regions_share_pixels_models_and_late_checksums() {
    let info = ImageInfo::new(41, 77, Channels::Rgba, SampleType::U8);
    let source: Vec<u8> = (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 73 + i / 7 * 41) as u8)
        .collect();
    for coding in 0..4 {
        for checksum in [false, true] {
            let config = match coding {
                0 => ImiEncoderConfig::lossless().with_row_groups(RowGroups::new(9, 7, 8 << 20)),
                1 => ImiEncoderConfig::quick().with_row_groups(RowGroups::new(9, 7, 8 << 20)),
                _ => ImiEncoderConfig::lossy(73.0)
                    .with_column_groups(Some(16))
                    .tune_lossy(|t| {
                        t.entropy = if coding == 2 {
                            crate::LossyEntropy::RunLevel
                        } else {
                            crate::LossyEntropy::SplitHuf
                        };
                        t.exact_blocks = Some(1000.0);
                        t.stripe_block_rows = 2;
                    }),
            }
            .with_checksum(checksum);
            let native = config
                .encode_legacy_samples(crate::ImageRef::packed(&source, &info), &Default::default())
                .unwrap();
            let native = crate::pixelsum::add_pixel_checksum(native).unwrap();
            let expected = crate::decode(&native).unwrap().1;
            let base = upgrade_image(&native, &crate::UpgradeLimits::default()).unwrap();
            for placement in [
                DirectoryPlacement::None,
                DirectoryPlacement::Front,
                DirectoryPlacement::Tail,
            ] {
                for open in [false, true] {
                    let file = super::file_tests::rewrite_placement(&base, placement, open);
                    assert_eq!(crate::decode(&file).unwrap().1, expected);
                    check_index(
                        &file,
                        &expected,
                        &info,
                        placement != DirectoryPlacement::None,
                    );
                    let mut reader =
                        crate::region::RegionReader::open(SliceReader::new(&file)).unwrap();
                    assert_eq!(
                        reader.pixel_checksum_status(),
                        crate::PixelChecksumStatus::Pending
                    );
                    let mut out = vec![0; expected.len()];
                    reader
                        .decode_region(
                            Rect::new(0, 0, info.width, info.height),
                            &mut out,
                            info.row_bytes(),
                        )
                        .unwrap();
                    assert_eq!(
                        reader.pixel_checksum_status(),
                        crate::PixelChecksumStatus::Verified
                    );
                    if placement == DirectoryPlacement::None {
                        let session =
                            crate::decode_resources::DecodeSession::new(Default::default());
                        assert!(matches!(
                            NumericTileReader::open(
                                SliceReader::new(&file),
                                TileLimits {
                                    max_object_bytes: 1 << 20,
                                    max_tile_bytes: 16 << 20
                                },
                                &session
                            ),
                            Err(Error::Unsupported("V4 sparse reader needs a directory"))
                        ));
                    }
                }
            }
        }
    }
}

#[test]
fn completed_numeric_images_keep_tiny_regions_in_one_native_unit() {
    let info = ImageInfo::new(1009, 787, Channels::Rgba, SampleType::U16);
    let mut state = 1u64;
    let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect();
    for quick in [false, true] {
        let config = if quick {
            ImiEncoderConfig::quick()
        } else {
            ImiEncoderConfig::lossless()
        }
        .with_threads(1)
        .with_checksum(false)
        .with_column_groups(Some(128))
        .tune_lossless(|t| t.stripe_rows = Some(64));
        let file = config
            .encode_samples(
                crate::ImageRef::new(&pixels, info.row_bytes(), &info),
                &Default::default(),
            )
            .unwrap();
        let session = crate::decode_resources::DecodeSession::new(Default::default());
        let numeric = NumericFile::open(
            &file,
            TileLimits {
                max_object_bytes: 1 << 20,
                max_tile_bytes: 16 << 20,
            },
            &session,
        )
        .unwrap();
        let layout = numeric.layout();
        assert_eq!(layout.tile_shape(), (1, 1));
        assert!(layout.tile_count() > 1);
        let rect = Rect::new(927, 705, 53, 57);
        let group =
            u64::from(rect.y() / 64) * u64::from(layout.group_grid().0) + u64::from(rect.x() / 128);
        let tile = layout.tile_for_group(group).unwrap();
        let physical = super::read_at_tests::physical_tiles(&file, layout.tile_count() as usize).1;
        let wanted = physical
            .into_iter()
            .find(|(id, _)| *id == tile.id)
            .unwrap()
            .1;
        // A viewport spanning many independently fetched tiles must remain
        // exact with every supported worker count, including padded output.
        check(&file, &pixels, &info);
        let reads = Arc::new(Mutex::new(Vec::new()));
        let source = Source {
            bytes: Arc::new(file),
            reads: reads.clone(),
        };
        let mut reader = crate::region::RegionReader::open(source).unwrap();
        reads.lock().unwrap().clear();
        let row = rect.width() as usize * info.bytes_per_pixel();
        let mut output = vec![0; row * rect.height() as usize];
        reader.decode_region(rect, &mut output, row).unwrap();
        assert_eq!(*reads.lock().unwrap(), vec![wanted]);
        for y in 0..rect.height() as usize {
            let at = (rect.y() as usize + y) * info.row_bytes()
                + rect.x() as usize * info.bytes_per_pixel();
            assert_eq!(&output[y * row..][..row], &pixels[at..][..row]);
        }
    }
}

#[test]
fn ordinary_numeric_region_coalesces_selected_neighbors_without_reading_gaps() {
    let info = ImageInfo::new(1025, 513, Channels::Rgb, SampleType::U8);
    let mut state = 7u64;
    let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect();
    let file = ImiEncoderConfig::lossless()
        .with_threads(1)
        .with_checksum(false)
        .with_column_groups(Some(128))
        .tune_lossless(|t| t.stripe_rows = Some(64))
        .encode_samples(crate::ImageRef::packed(&pixels, &info), &Default::default())
        .unwrap();
    let session = crate::decode_resources::DecodeSession::new(Default::default());
    let numeric = NumericFile::open(
        &file,
        TileLimits {
            max_object_bytes: 1 << 20,
            max_tile_bytes: 16 << 20,
        },
        &session,
    )
    .unwrap();
    let layout = numeric.layout();
    let (_, physical) = super::read_at_tests::physical_tiles(&file, layout.tile_count() as usize);
    for threads in [1, 2, 4, 8] {
        for rect in [
            Rect::new(128, 64, 256, 128),
            Rect::new(0, 0, info.width, info.height),
        ] {
            let reads = Arc::new(Mutex::new(Vec::new()));
            let mut reader = crate::region::RegionReader::open(Source {
                bytes: Arc::new(file.clone()),
                reads: reads.clone(),
            })
            .unwrap();
            reads.lock().unwrap().clear();
            let row = rect.width() as usize * info.bytes_per_pixel();
            let mut output = vec![0; row * rect.height() as usize];
            reader
                .decode_region_threads(rect, &mut output, row, threads)
                .unwrap();
            let observed = reads.lock().unwrap();
            assert!(observed.iter().all(|r| r.end - r.start <= 1 << 20));
            if rect.width() == 256 {
                let columns = u64::from(layout.group_grid().0);
                let spans: Vec<_> = [columns + 1, 2 * columns + 1]
                    .iter()
                    .map(|id| {
                        let first = physical
                            .iter()
                            .find(|(tile, _)| tile == id)
                            .unwrap()
                            .1
                            .clone();
                        let last = physical
                            .iter()
                            .find(|(tile, _)| *tile == id + 1)
                            .unwrap()
                            .1
                            .clone();
                        first.start..last.end
                    })
                    .collect();
                assert_eq!(*observed, spans, "unselected column gaps must not be read");
            } else {
                assert!(observed.len() < physical.len());
                assert_eq!(
                    observed.first().unwrap().start,
                    physical.first().unwrap().1.start
                );
                assert_eq!(observed.last().unwrap().end, physical.last().unwrap().1.end);
                assert!(observed.windows(2).all(|r| r[0].end == r[1].start));
            }
            for y in 0..rect.height() as usize {
                let at = (rect.y() as usize + y) * info.row_bytes()
                    + rect.x() as usize * info.bytes_per_pixel();
                assert_eq!(&output[y * row..][..row], &pixels[at..][..row]);
            }
        }
    }
}
