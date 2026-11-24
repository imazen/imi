#![cfg(feature = "std")]
mod common;
use imij::planes::{Rect, SliceReader};
use imij::region::RegionReader;
use imij::stream_encode::{ChecksummedGroupConfig, ChecksummedGroupEncoder, StreamEncodeError};
use imij::{Channels, Error, ImageInfo, Metadata, SampleType};
use std::cell::Cell;
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};

fn config(k: u8, limit: u64) -> ChecksummedGroupConfig {
    ChecksummedGroupConfig::new(
        imij::ImiEncoderConfig::default().with_row_groups(Some(imij::RowGroups::new(
            9,
            5,
            32 << 20,
        ))),
        100000,
        k,
        limit,
    )
}

// Find trailing cSUM independently through the index's DATA length, rather
// than searching encoded bytes for a tag that could also occur in pixels.
fn checksum(file: &[u8]) -> (usize, usize, usize) {
    assert!(file.ends_with(b"uIDX"));
    let data_len =
        u64::from_le_bytes(file[file.len() - 20..file.len() - 12].try_into().unwrap()) as usize;
    let chunk_len =
        u64::from_le_bytes(file[file.len() - 12..file.len() - 4].try_into().unwrap()) as usize;
    let index = file.len() - chunk_len;
    let (_, data) = common::checksum::find(file, b"DATA").unwrap();
    let at = data.start + data_len;
    assert_eq!(&file[at..at + 4], b"cSUM");
    let (n, w) = common::varint(&file[at + 4..]);
    let payload = at + 4 + w;
    assert_eq!(payload + n as usize - 1, index);
    (at, payload, index)
}

#[test]
fn independent_hashes_cover_header_groups_and_trailing_index() {
    for k in [10, 16, 32, 40] {
        for sample in [
            SampleType::U8,
            SampleType::U16,
            SampleType::F16,
            SampleType::F32,
        ] {
            for channels in [
                Channels::Gray,
                Channels::GrayAlpha,
                Channels::Rgb,
                Channels::Rgba,
            ] {
                let info = ImageInfo::new(43, 29, channels, sample);
                let raw: Vec<_> = (0..info.checked_buffer_len().unwrap())
                    .map(|i| ((i * 117 + i / 3) & 255) as u8)
                    .collect();
                let meta = common::checksum::meta();
                let mut enc = ChecksummedGroupEncoder::new(
                    Vec::new(),
                    Cursor::new(b"index prefix".to_vec()),
                    Cursor::new(b"checksum prefix".to_vec()),
                    info.clone(),
                    &meta,
                    config(k, 1 << 20),
                )
                .unwrap();
                while let Some(g) = enc.next_group() {
                    assert!(enc.write_group(&[], info.row_bytes()).is_err());
                    assert_eq!(enc.next_group(), Some(g));
                    let start =
                        g.y as usize * info.row_bytes() + g.x as usize * info.bytes_per_pixel();
                    enc.write_group(&raw[start..], info.row_bytes()).unwrap();
                }
                let (file, index_scratch, digest_scratch) = enc.finish().unwrap();
                assert!(index_scratch.get_ref().starts_with(b"index prefix"));
                assert!(digest_scratch.get_ref().starts_with(b"checksum prefix"));
                let (at, payload, end) = checksum(&file);
                assert_eq!(&file[payload..payload + 3], &[1, 16, k]);
                let (covered_len, w) = common::varint(&file[payload + 3..]);
                let covered: Vec<_> = file[..at].iter().chain(&file[end..]).copied().collect();
                assert_eq!(covered_len, covered.len() as u64);
                let table = &file[payload + 3 + w..end];
                let pieces = (covered.len() as u64).div_ceil(1u64 << k);
                assert_eq!(table.len() as u64, pieces * 16);
                assert_eq!(table, &digest_scratch.get_ref()[b"checksum prefix".len()..]);
                for i in 0..pieces {
                    let start = (i << k) as usize;
                    let stop = ((i + 1) << k).min(covered.len() as u64) as usize;
                    assert_eq!(
                        &table[i as usize * 16..][..16],
                        &xxhash_rust::xxh3::xxh3_128(&covered[start..stop]).to_le_bytes()
                    );
                }
                assert_eq!(common::decode_all(&file, &info), raw);
                let mut reader = RegionReader::open(SliceReader::new(&file)).unwrap();
                assert!(reader.has_unit_table());
                for threads in [1, 2, 4, 8] {
                    let r = Rect::new(7, 4, 15, 17);
                    let bytes = 15 * info.bytes_per_pixel();
                    let stride = bytes + 5;
                    let mut output = vec![0xCD; stride * 17];
                    reader
                        .decode_region_threads(r, &mut output, stride, threads)
                        .unwrap();
                    for y in 0..17 {
                        let src = (y + 4) * info.row_bytes() + 7 * info.bytes_per_pixel();
                        assert_eq!(
                            &output[y * stride..y * stride + bytes],
                            &raw[src..src + bytes]
                        );
                        assert!(
                            output[y * stride + bytes..(y + 1) * stride]
                                .iter()
                                .all(|&v| v == 0xCD)
                        );
                    }
                }
                // A checksum-table mutation must fail every whole-image route.
                let mut bad = file.clone();
                bad[payload + 3 + w] ^= 1;
                assert_eq!(
                    imij::decode(&bad).err(),
                    Some(Error::Corrupt("file checksum mismatch"))
                );
                assert_eq!(
                    common::checksum::via_reader(bad.as_slice()).err(),
                    Some(Error::Corrupt("file checksum mismatch"))
                );
                // The final index bytes are covered too (even when their
                // structural error is also detectable without a checksum).
                let mut bad_index = file.clone();
                let n = bad_index.len();
                bad_index[n - 20] ^= 1;
                assert!(imij::decode(&bad_index).is_err());
                assert!(imij::decode(&file[..at]).is_err());
            }
        }
    }
}

#[test]
fn checksum_budget_and_configuration_fail_without_silent_completion() {
    let info = ImageInfo::new(43, 29, Channels::Rgb, SampleType::U8);
    for (k, limit) in [(9, 100), (41, 100), (16, 7)] {
        let mut out = Vec::new();
        let mut index = Cursor::new(b"index".to_vec());
        let mut checksum = Cursor::new(b"checksum".to_vec());
        assert!(
            ChecksummedGroupEncoder::new(
                &mut out,
                &mut index,
                &mut checksum,
                info.clone(),
                &Metadata::default(),
                config(k, limit)
            )
            .is_err()
        );
        assert!(out.is_empty());
        assert_eq!(index.get_ref(), b"index");
        assert_eq!(checksum.get_ref(), b"checksum");
    }
    let mut out = Vec::new();
    let mut scratch = Cursor::new(Vec::new());
    let mut enc = ChecksummedGroupEncoder::new(
        &mut out,
        Cursor::new(Vec::new()),
        &mut scratch,
        info.clone(),
        &Metadata::default(),
        config(10, 16),
    )
    .unwrap();
    let mut failed = false;
    while let Some(g) = enc.next_group() {
        let pixels: Vec<_> = (0..g.width * g.height * 3)
            .map(|i| (i * 73) as u8)
            .collect();
        if let Err(error) = enc.write_group(&pixels, g.width as usize * 3) {
            assert!(matches!(error, StreamEncodeError::Codec(Error::TooLarge)));
            assert!(enc.next_group().is_none());
            assert!(enc.write_group(&pixels, g.width as usize * 3).is_err());
            failed = true;
            break;
        }
    }
    assert!(failed, "budget must fail before all groups");
    assert!(enc.finish().is_err());
    assert_eq!(scratch.get_ref().len(), 16);
    assert!(imij::decode(&out).is_err());
}

struct Scratch<'a> {
    data: Cursor<Vec<u8>>,
    fail: &'a Cell<u8>,
}
impl Write for Scratch<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.fail.get() == 1 {
            Err(io::Error::other("scratch write"))
        } else {
            self.data.write(bytes)
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        if self.fail.get() == 2 {
            Err(io::Error::other("scratch flush"))
        } else {
            Ok(())
        }
    }
}
impl Seek for Scratch<'_> {
    fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
        if self.fail.get() == 3 {
            Err(io::Error::other("scratch seek"))
        } else {
            self.data.seek(p)
        }
    }
}
impl Read for Scratch<'_> {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        if self.fail.get() == 4 {
            return Err(io::Error::other("scratch read"));
        }
        if self.fail.get() == 5 {
            return Ok(0);
        }
        let n = self.data.read(b)?;
        if self.fail.get() == 6 && n != 0 {
            b[0] ^= 1;
        }
        Ok(n)
    }
}

#[test]
fn checksum_scratch_failure_never_completes_and_preserves_prefix() {
    for mode in 1..=6 {
        let fail = Cell::new(0);
        let mut out = Vec::new();
        let mut scratch = Scratch {
            data: Cursor::new(b"keep".to_vec()),
            fail: &fail,
        };
        let mut enc = ChecksummedGroupEncoder::new(
            &mut out,
            Cursor::new(Vec::new()),
            &mut scratch,
            ImageInfo::new(2, 2, Channels::Gray, SampleType::U8),
            &Metadata::default(),
            config(40, 16),
        )
        .unwrap();
        enc.write_group(&[0, 255, 3, 4], 2).unwrap();
        fail.set(mode);
        let result = enc.finish();
        if mode == 6 {
            assert!(matches!(
                result,
                Err(StreamEncodeError::Codec(Error::InvalidInput(
                    "group checksum scratch changed"
                )))
            ));
        } else {
            assert!(result.is_err());
        }
        assert!(scratch.data.get_ref().starts_with(b"keep"));
        assert!(!out.ends_with(b"uIDX"));
    }
}

#[test]
fn final_index_hashing_also_obeys_checksum_scratch_budget() {
    fn encode(meta: &Metadata, budget: u64) -> Result<Vec<u8>, StreamEncodeError> {
        let mut e = ChecksummedGroupEncoder::new(
            Vec::new(),
            Cursor::new(Vec::new()),
            Cursor::new(Vec::new()),
            ImageInfo::new(1, 1, Channels::Gray, SampleType::U8),
            meta,
            config(10, budget),
        )?;
        e.write_group(&[7], 1)?;
        e.finish().map(|(v, _, _)| v)
    }
    let mut meta = Metadata {
        chunks: vec![imij::Chunk {
            tag: *b"vnDr",
            data: vec![0; 500],
        }],
        ..Default::default()
    };
    let first = encode(&meta, 1000).unwrap();
    let (before, _, _) = checksum(&first);
    assert!(before < 2044);
    meta.chunks[0].data.resize(500 + 2044 - before, 0);
    let full = encode(&meta, 1000).unwrap();
    assert_eq!(checksum(&full).0, 2044);
    // Only one complete 1024-byte piece precedes finish. Hashing the index
    // completes the second, which cannot fit in this eight-byte scratch cap.
    assert!(matches!(
        encode(&meta, 8),
        Err(StreamEncodeError::Codec(Error::TooLarge))
    ));
}
