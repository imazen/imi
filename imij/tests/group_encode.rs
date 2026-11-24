#![cfg(feature = "std")]
use imij::stream_encode::{GroupEncoder, StreamEncodeError};
use imij::{Channels, Error, ImageInfo, Limits, Metadata, SampleType, StreamDecoder};
use std::io::{self, Write};

const BUDGET: usize = 32 << 20;

#[test]
fn strided_groups_preserve_full_storage_and_all_decode_rows() {
    for sample in [SampleType::U8, SampleType::U16] {
        for channels in [
            Channels::Gray,
            Channels::GrayAlpha,
            Channels::Rgb,
            Channels::Rgba,
        ] {
            for (width, height, columns, rows) in
                [(1, 1, 1, 1), (19, 13, 7, 5), (35, 4, 64, 2), (17, 9, 1, 1)]
            {
                let mut info = ImageInfo::new(width, height, channels, sample);
                // Padding bits are deliberately nonzero, even with a declared
                // narrow code range. Lossless means retaining every storage bit.
                if sample == SampleType::U16 {
                    info = info
                        .with_significant_bits(&vec![10; channels.count()])
                        .unwrap();
                }
                let raw: Vec<_> = (0..info.checked_buffer_len().unwrap())
                    .map(|i| ((i * 73 + i / 7 * 11 + 255) % 256) as u8)
                    .collect();
                let meta = Metadata {
                    orientation: 6,
                    content_light_level: Some([1, 2, 3, 4, 5, 6, 7, 8]),
                    ..Default::default()
                };
                let mut encoder = GroupEncoder::new(
                    Vec::new(),
                    info.clone(),
                    &meta,
                    &imij::ImiEncoderConfig::default()
                        .with_row_groups(Some(imij::RowGroups::new(columns, rows, BUDGET))),
                )
                .unwrap();
                while let Some(g) = encoder.next_group() {
                    let row = g.width as usize * info.bytes_per_pixel();
                    let stride = row + 7;
                    let mut group = vec![0xA5; stride * (g.height as usize - 1) + row];
                    for y in 0..g.height as usize {
                        let start = (g.y as usize + y) * info.row_bytes()
                            + g.x as usize * info.bytes_per_pixel();
                        group[y * stride..y * stride + row]
                            .copy_from_slice(&raw[start..start + row]);
                    }
                    // Bad input must leave the next rectangle unchanged.
                    assert!(encoder.write_group(&[], stride).is_err());
                    assert_eq!(encoder.next_group(), Some(g));
                    encoder.write_group(&group, stride).unwrap();
                }
                let file = encoder.finish().unwrap();
                let (got, pixels) = imij::decode(&file).unwrap();
                assert_eq!(got, info);
                assert_eq!(pixels, raw, "{sample:?} {channels:?} {width}x{height}");
                assert_eq!(imij::probe(&file).unwrap().1, meta);
                let output_stride = info.row_bytes() + 11;
                for threads in [1, 2, 4, 8] {
                    let mut padded = vec![0xCD; output_stride * height as usize];
                    imij::decode_into_threads(&file, &mut padded, output_stride, threads).unwrap();
                    for (actual, want) in padded
                        .chunks(output_stride)
                        .zip(raw.chunks(info.row_bytes()))
                    {
                        assert_eq!(&actual[..info.row_bytes()], want);
                        assert!(actual[info.row_bytes()..].iter().all(|&v| v == 0xCD));
                    }
                }
                let mut stream = StreamDecoder::new(&file).unwrap();
                let mut row = vec![0; info.row_bytes()];
                for want in raw.chunks(info.row_bytes()) {
                    assert!(stream.next_row(&mut row).unwrap());
                    assert_eq!(row, want);
                }
                assert!(!stream.next_row(&mut row).unwrap());
                let mut read = Vec::new();
                imij::decode_reader(file.as_slice(), &Limits::DEFAULT, |_, _, r| {
                    read.extend_from_slice(r);
                    Ok(())
                })
                .unwrap();
                assert_eq!(read, raw);
            }
        }
    }
}

#[derive(Default)]
struct FailWriter {
    written: usize,
    fail: bool,
}
impl Write for FailWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.fail {
            return Err(io::Error::other("expected sink failure"));
        }
        self.written += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn admission_precedes_output_and_does_not_scale_with_image_extent() {
    let mut sink = FailWriter::default();
    let small = ImageInfo::new(16, 16, Channels::Rgba, SampleType::U16);
    assert!(matches!(
        GroupEncoder::new(
            &mut sink,
            small.clone(),
            &Metadata::default(),
            &imij::ImiEncoderConfig::default()
                .with_row_groups(Some(imij::RowGroups::new(16, 16, 1)))
        ),
        Err(StreamEncodeError::Codec(Error::TooLarge))
    ));
    assert_eq!(sink.written, 0);
    let huge = ImageInfo::new(1_000_000, 1_000_000, Channels::Rgba, SampleType::U16);
    #[cfg(target_pointer_width = "64")]
    {
        let e = GroupEncoder::new(
            &mut sink,
            huge,
            &Metadata::default(),
            &imij::ImiEncoderConfig::default()
                .with_row_groups(Some(imij::RowGroups::new(16, 16, BUDGET))),
        )
        .unwrap();
        assert_eq!(e.next_group().unwrap().width, 16);
        assert!(matches!(
            e.finish(),
            Err(StreamEncodeError::Codec(Error::InvalidInput(_)))
        ));
        assert!(sink.written < 4096);
    }
    #[cfg(not(target_pointer_width = "64"))]
    let _ = huge;
    let e = GroupEncoder::new(
        Vec::new(),
        small,
        &Metadata::default(),
        &imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(16, 16, BUDGET))),
    )
    .unwrap();
    assert!(e.finish().is_err());
}

#[test]
fn sink_failure_poisoning_prevents_record_replay() {
    use std::cell::Cell;
    struct Sink<'a>(&'a Cell<bool>);
    impl Write for Sink<'_> {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            if self.0.get() {
                Err(io::Error::other("fail"))
            } else {
                Ok(b.len())
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let fail = Cell::new(false);
    let mut e = GroupEncoder::new(
        Sink(&fail),
        ImageInfo::new(2, 2, Channels::Gray, SampleType::U8),
        &Metadata::default(),
        &imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(2, 2, BUDGET))),
    )
    .unwrap();
    fail.set(true);
    assert!(matches!(
        e.write_group(&[0, 255, 3, 4], 2),
        Err(StreamEncodeError::Write(_))
    ));
    fail.set(false);
    assert_eq!(e.next_group(), None);
    assert!(e.write_group(&[0, 255, 3, 4], 2).is_err());
    assert!(e.finish().is_err());
}
