#![cfg(feature = "std")]

use imij::planes::{Rect, SliceReader};
use imij::region::RegionReader;
use imij::stream_encode::{
    ForwardRowConfig, IndexedGroupEncoder, StreamEncodeError, encode_forward_rows_indexed,
};
use imij::{Channels, Error, ImageInfo, Metadata, SampleType};
use std::cell::Cell;
use std::io::{self, Cursor, Read};

const WORKING: usize = 32 << 20;

fn config(info: &ImageInfo, columns: u32, stride: u64, source_cap: usize) -> ForwardRowConfig {
    ForwardRowConfig::new(
        imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(columns, 1, WORKING))),
        stride,
        source_cap,
        stride * u64::from(info.height),
        100_000,
    )
}

#[test]
fn forward_strided_rows_match_direct_indexed_groups_and_decode_exactly() {
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
            let mut info = ImageInfo::new(19, 5, channels, sample);
            if sample == SampleType::U16 {
                info = info
                    .with_significant_bits(&vec![10; channels.count()])
                    .unwrap();
            }
            let row = info.row_bytes();
            let stride = row + 7;
            let raw: Vec<u8> = (0..info.checked_buffer_len().unwrap())
                .map(|i| ((i * 73 + i / 7 * 11 + 255) % 256) as u8)
                .collect();
            let mut source = Vec::new();
            for (y, pixels) in raw.chunks_exact(row).enumerate() {
                source.extend_from_slice(pixels);
                if y + 1 < info.height as usize {
                    source.extend_from_slice(&[0xa5; 7]);
                }
            }
            let consumed = source.len();
            source.extend_from_slice(&[0xee; 9]); // final-row padding is not required or read
            let mut input = Cursor::new(source);
            let metadata = Metadata::default();
            let cfg = config(&info, 7, stride as u64, 7 * info.bytes_per_pixel());
            let (file, scratch) = encode_forward_rows_indexed(
                &mut input,
                Vec::new(),
                Cursor::new(Vec::new()),
                info.clone(),
                &metadata,
                cfg.clone(),
            )
            .unwrap();
            assert_eq!(input.position() as usize, consumed);
            let mut direct = IndexedGroupEncoder::new(
                Vec::new(),
                Cursor::new(Vec::new()),
                info.clone(),
                &metadata,
                &cfg.encoder,
                cfg.max_index_bytes,
            )
            .unwrap();
            while let Some(rect) = direct.next_group() {
                let begin = rect.y as usize * row + rect.x as usize * info.bytes_per_pixel();
                let len = rect.width as usize * info.bytes_per_pixel();
                direct.write_group(&raw[begin..begin + len], len).unwrap();
            }
            let (expected, expected_scratch) = direct.finish().unwrap();
            assert_eq!(file, expected, "{sample:?} {channels:?}");
            assert_eq!(scratch.into_inner(), expected_scratch.into_inner());
            let rect = Rect::new(3, 1, 11, 3);
            let mut region = RegionReader::open(SliceReader::new(&file)).unwrap();
            assert!(region.has_unit_table());
            let region_row = rect.width() as usize * info.bytes_per_pixel();
            let region_stride = region_row + 5;
            for threads in [1, 2, 4, 8] {
                let mut output = vec![0xcd; region_stride * rect.height() as usize];
                region
                    .decode_region_threads(rect, &mut output, region_stride, threads)
                    .unwrap();
                for y in 0..rect.height() as usize {
                    let begin =
                        (rect.y() as usize + y) * row + rect.x() as usize * info.bytes_per_pixel();
                    assert_eq!(
                        &output[y * region_stride..y * region_stride + region_row],
                        &raw[begin..begin + region_row]
                    );
                    assert!(
                        output[y * region_stride + region_row..(y + 1) * region_stride]
                            .iter()
                            .all(|&value| value == 0xcd)
                    );
                }
            }
            assert_eq!(imij::decode(&file).unwrap(), (info, raw));
        }
    }
}

struct CountReads<'a>(&'a Cell<usize>);
impl Read for CountReads<'_> {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        self.0.set(self.0.get() + 1);
        Ok(0)
    }
}

#[test]
fn admission_precedes_source_and_output_and_truncation_is_a_read_error() {
    let info = ImageInfo::new(19, 5, Channels::Rgb, SampleType::U8);
    let calls = Cell::new(0);
    let mut source = CountReads(&calls);
    let mut sink = Vec::new();
    let mut scratch = Cursor::new(Vec::new());
    let row = info.row_bytes();
    let cfg = config(&info, 7, row as u64, 7 * info.bytes_per_pixel() - 1);
    assert!(matches!(
        encode_forward_rows_indexed(
            &mut source,
            &mut sink,
            &mut scratch,
            info.clone(),
            &Metadata::default(),
            cfg,
        ),
        Err(StreamEncodeError::Codec(Error::TooLarge))
    ));
    assert_eq!(calls.get(), 0);
    assert!(sink.is_empty());
    assert!(scratch.get_ref().is_empty());

    let cfg = config(&info, 7, row as u64 - 1, row);
    assert!(matches!(
        encode_forward_rows_indexed(
            &mut source,
            &mut sink,
            &mut scratch,
            info.clone(),
            &Metadata::default(),
            cfg,
        ),
        Err(StreamEncodeError::Codec(Error::InvalidInput(
            "forward source stride"
        )))
    ));
    assert_eq!(calls.get(), 0);
    assert!(sink.is_empty());

    let mut cfg = config(&info, 7, row as u64, row);
    cfg.max_source_bytes -= 1;
    assert!(matches!(
        encode_forward_rows_indexed(
            &mut source,
            &mut sink,
            &mut scratch,
            info.clone(),
            &Metadata::default(),
            cfg,
        ),
        Err(StreamEncodeError::Codec(Error::TooLarge))
    ));
    assert_eq!(calls.get(), 0);
    assert!(sink.is_empty());

    let mut cfg = config(&info, 7, row as u64, row);
    cfg.encoder = cfg.encoder.clone().with_row_groups(imij::RowGroups {
        rows: 2,
        ..cfg.encoder.row_groups().unwrap()
    });
    assert!(matches!(
        encode_forward_rows_indexed(
            &mut source,
            &mut sink,
            &mut scratch,
            info.clone(),
            &Metadata::default(),
            cfg,
        ),
        Err(StreamEncodeError::Codec(Error::InvalidInput(
            "forward row group geometry"
        )))
    ));
    assert_eq!(calls.get(), 0);
    assert!(sink.is_empty());

    let cfg = config(&info, 7, row as u64, row);
    assert!(matches!(
        encode_forward_rows_indexed(
            &mut source,
            &mut sink,
            &mut scratch,
            info,
            &Metadata::default(),
            cfg,
        ),
        Err(StreamEncodeError::Read(error)) if error.kind() == io::ErrorKind::UnexpectedEof
    ));
    assert!(calls.get() > 0);
    assert!(!sink.is_empty()); // the admitted header may already be written
}
