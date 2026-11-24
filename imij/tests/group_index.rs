#![cfg(feature = "std")]
use imij::planes::{Rect, SliceReader};
use imij::region::RegionReader;
use imij::stream_encode::{GroupEncoder, IndexedGroupEncoder, StreamEncodeError};
use imij::{Channels, Error, ImageInfo, Limits, Metadata, SampleType, StreamDecoder};
use std::cell::Cell;
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};

const BUDGET: usize = 32 << 20;

#[test]
fn indexed_groups_match_basic_records_and_every_pixel_route() {
    let mut samples = vec![
        SampleType::U8,
        SampleType::U16,
        SampleType::F16,
        SampleType::F32,
    ];
    if cfg!(feature = "extended") {
        samples.extend([
            SampleType::I8,
            SampleType::I16,
            SampleType::U32,
            SampleType::I32,
            SampleType::U64,
            SampleType::I64,
            SampleType::F64,
        ]);
    }
    for sample in samples {
        for channels in [
            Channels::Gray,
            Channels::GrayAlpha,
            Channels::Rgb,
            Channels::Rgba,
        ] {
            for (width, height, columns, rows) in [(1, 1, 1, 1), (19, 13, 7, 5), (35, 4, 64, 2)] {
                let info = ImageInfo::new(width, height, channels, sample);
                let raw: Vec<_> = (0..info.checked_buffer_len().unwrap())
                    .map(|i| ((i * 73 + i / 7 * 11 + 255) % 256) as u8)
                    .collect();
                let config = imij::ImiEncoderConfig::default()
                    .with_row_groups(Some(imij::RowGroups::new(columns, rows, BUDGET)));
                let mut basic =
                    GroupEncoder::new(Vec::new(), info.clone(), &Metadata::default(), &config)
                        .unwrap();
                let mut indexed = IndexedGroupEncoder::new(
                    Vec::new(),
                    Cursor::new(b"existing scratch".to_vec()),
                    info.clone(),
                    &Metadata::default(),
                    &config,
                    100_000,
                )
                .unwrap();
                while let Some(g) = indexed.next_group() {
                    assert_eq!(basic.next_group(), Some(g));
                    assert!(indexed.write_group(&[], info.row_bytes()).is_err());
                    assert_eq!(indexed.next_group(), Some(g));
                    let start =
                        g.y as usize * info.row_bytes() + g.x as usize * info.bytes_per_pixel();
                    indexed
                        .write_group(&raw[start..], info.row_bytes())
                        .unwrap();
                    basic.write_group(&raw[start..], info.row_bytes()).unwrap();
                }
                let current = basic.finish().unwrap();
                assert_eq!(imij::header(&current).unwrap().version, 4);
                assert_eq!(imij::decode(&current).unwrap(), (info.clone(), raw.clone()));
                // The indexed compatibility writer retains V3 framing. Compare
                // its entropy records to the explicit V3 group-core output.
                let plain = config
                    .encode_legacy_samples(
                        imij::ImageRef::packed(&raw, &info),
                        &Metadata::default(),
                    )
                    .unwrap();
                let (file, scratch) = indexed.finish().unwrap();
                assert!(scratch.get_ref().starts_with(b"existing scratch"));
                assert_eq!(scratch.position() as usize, scratch.get_ref().len());
                assert!(file.ends_with(b"uIDX"));
                // Both writers produce identical record bodies; only the head
                // reservation and the final index differ.
                let index_len =
                    u64::from_le_bytes(file[file.len() - 12..file.len() - 4].try_into().unwrap())
                        as usize;
                let index_start = file.len() - index_len;
                let extra_head = 4; // head record length, id 12, length 1, check-tail
                assert_eq!(index_start, plain.len() + extra_head);
                // Strip the two headers by finding the unchanged suffix. At
                // least the complete last record must match, even for 1x1.
                let spans = &scratch.get_ref()[b"existing scratch".len()..];
                let mut total = 0usize;
                let mut n = 0usize;
                let mut shift = 0;
                for &b in spans {
                    n |= ((b & 127) as usize) << shift;
                    if b < 128 {
                        total += n;
                        n = 0;
                        shift = 0;
                    } else {
                        shift += 7;
                    }
                }
                assert_eq!(
                    &file[index_start - total..index_start],
                    &plain[plain.len() - total..]
                );
                assert_eq!(imij::decode(&file).unwrap(), (info.clone(), raw.clone()));
                let mut stream = StreamDecoder::new(&file).unwrap();
                let mut row = vec![0; info.row_bytes()];
                for expected in raw.chunks(info.row_bytes()) {
                    assert!(stream.next_row(&mut row).unwrap());
                    assert_eq!(row, expected);
                }
                assert!(!stream.next_row(&mut row).unwrap());
                let mut read = Vec::new();
                imij::decode_reader(file.as_slice(), &Limits::DEFAULT, |_, _, row| {
                    read.extend_from_slice(row);
                    Ok(())
                })
                .unwrap();
                assert_eq!(read, raw);
                let rect = Rect::new(
                    width / 3,
                    height / 3,
                    width - width / 3,
                    height - height / 3,
                );
                let stride = rect.width() as usize * info.bytes_per_pixel() + 7;
                let mut reader = RegionReader::open(SliceReader::new(&file)).unwrap();
                assert!(reader.has_unit_table());
                for threads in [1, 2, 4, 8] {
                    let mut out = vec![0xCD; stride * rect.height() as usize];
                    reader
                        .decode_region_threads(rect, &mut out, stride, threads)
                        .unwrap();
                    for y in 0..rect.height() as usize {
                        let start = (height / 3) as usize * info.row_bytes()
                            + y * info.row_bytes()
                            + (width / 3) as usize * info.bytes_per_pixel();
                        let len = rect.width() as usize * info.bytes_per_pixel();
                        assert_eq!(&out[y * stride..y * stride + len], &raw[start..start + len]);
                        assert!(
                            out[y * stride + len..(y + 1) * stride]
                                .iter()
                                .all(|&b| b == 0xCD)
                        );
                    }
                }
            }
        }
    }
}

struct Scratch<'a> {
    data: Cursor<Vec<u8>>,
    fail: &'a Cell<u8>,
    max_read: usize,
}
impl Write for Scratch<'_> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        if self.fail.get() == 1 {
            return Err(io::Error::other("scratch write"));
        }
        self.data.write(b)
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
        self.max_read = self.max_read.max(b.len());
        match self.fail.get() {
            4 => Err(io::Error::other("scratch read")),
            5 => Ok(0),
            6 => {
                let n = self.data.read(b)?;
                if n != 0 {
                    b[0] ^= 1;
                }
                Ok(n)
            }
            _ => self.data.read(b),
        }
    }
}

#[test]
fn scratch_failures_are_sticky_or_fail_finish_without_valid_footer() {
    for mode in 1..=6 {
        let fail = Cell::new(0);
        let mut output = Vec::new();
        let mut enc = IndexedGroupEncoder::new(
            &mut output,
            Scratch {
                data: Cursor::new(Vec::new()),
                fail: &fail,
                max_read: 0,
            },
            ImageInfo::new(2, 2, Channels::Gray, SampleType::U8),
            &Metadata::default(),
            &imij::ImiEncoderConfig::default()
                .with_row_groups(Some(imij::RowGroups::new(2, 2, BUDGET))),
            10,
        )
        .unwrap();
        if mode == 1 {
            fail.set(mode);
            assert!(matches!(
                enc.write_group(&[0, 255, 3, 4], 2),
                Err(StreamEncodeError::Write(_))
            ));
            fail.set(0);
            assert!(enc.next_group().is_none());
            assert!(enc.write_group(&[0, 255, 3, 4], 2).is_err());
        } else {
            enc.write_group(&[0, 255, 3, 4], 2).unwrap();
            fail.set(mode);
        }
        let result = enc.finish();
        if mode == 6 {
            assert!(matches!(
                result,
                Err(StreamEncodeError::Codec(Error::InvalidInput(
                    "group index scratch changed"
                )))
            ));
        } else {
            assert!(result.is_err());
        }
        assert!(!output.ends_with(b"uIDX"));
    }
}

#[test]
fn scratch_budget_is_admitted_before_either_destination_and_replay_is_chunked() {
    let mut output = Vec::new();
    let mut scratch = Cursor::new(b"keep".to_vec());
    let info = ImageInfo::new(1, 9000, Channels::Gray, SampleType::U8);
    let cfg =
        imij::ImiEncoderConfig::default().with_row_groups(Some(imij::RowGroups::new(1, 1, BUDGET)));
    assert!(matches!(
        IndexedGroupEncoder::new(
            &mut output,
            &mut scratch,
            info.clone(),
            &Metadata::default(),
            &cfg,
            89999
        ),
        Err(StreamEncodeError::Codec(Error::TooLarge))
    ));
    assert!(output.is_empty());
    assert_eq!(scratch.get_ref(), b"keep");
    assert_eq!(scratch.position(), 0);
    let fail = Cell::new(0);
    let scratch = Scratch {
        data: Cursor::new(Vec::new()),
        fail: &fail,
        max_read: 0,
    };
    let mut enc = IndexedGroupEncoder::new(
        Vec::new(),
        scratch,
        info.clone(),
        &Metadata::default(),
        &cfg,
        90000,
    )
    .unwrap();
    while let Some(g) = enc.next_group() {
        enc.write_group(&[g.y as u8], 1).unwrap();
    }
    let (file, scratch) = enc.finish().unwrap();
    assert!(scratch.data.get_ref().len() > 8192);
    assert_eq!(scratch.max_read, 8192);
    assert_eq!(
        imij::decode(&file).unwrap().1,
        (0..9000).map(|y| y as u8).collect::<Vec<_>>()
    );
    assert!(
        RegionReader::open(SliceReader::new(&file))
            .unwrap()
            .has_unit_table()
    );
}

#[test]
fn output_failures_remain_sticky_and_finish_requires_all_groups() {
    struct Sink<'a>(&'a Cell<bool>);
    impl Write for Sink<'_> {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            if self.0.get() {
                Err(io::Error::other("sink write"))
            } else {
                Ok(b.len())
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            if self.0.get() {
                Err(io::Error::other("sink flush"))
            } else {
                Ok(())
            }
        }
    }
    for fail_at in [0, 1, 2] {
        let fail = Cell::new(false);
        let mut enc = IndexedGroupEncoder::new(
            Sink(&fail),
            Cursor::new(Vec::new()),
            ImageInfo::new(2, 2, Channels::Gray, SampleType::U8),
            &Metadata::default(),
            &imij::ImiEncoderConfig::default()
                .with_row_groups(Some(imij::RowGroups::new(2, 2, BUDGET))),
            10,
        )
        .unwrap();
        if fail_at == 0 {
            assert!(enc.finish().is_err());
            continue;
        }
        if fail_at == 1 {
            fail.set(true);
            assert!(enc.write_group(&[1, 2, 3, 4], 2).is_err());
            fail.set(false);
            assert!(enc.next_group().is_none());
            assert!(enc.write_group(&[1, 2, 3, 4], 2).is_err());
        } else {
            enc.write_group(&[1, 2, 3, 4], 2).unwrap();
            fail.set(true);
        }
        assert!(enc.finish().is_err());
    }
}
