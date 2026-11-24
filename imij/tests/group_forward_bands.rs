#![cfg(feature = "std")]

use imij::planes::{Rect, SliceReader};
use imij::region::RegionReader;
use imij::stream_encode::{
    ForwardBandConfig, IndexedGroupEncoder, StreamEncodeError, encode_forward_bands_indexed,
};
use imij::{Channels, Error, ImageInfo, Metadata, SampleType};
use std::cell::Cell;
use std::io::{self, Cursor, Read};

const WORKING: usize = 32 << 20;

fn config(info: &ImageInfo, columns: u32, rows: u32, stride: u64) -> ForwardBandConfig {
    let group_bytes =
        columns.min(info.width) as usize * rows.min(info.height) as usize * info.bytes_per_pixel();
    ForwardBandConfig::new(
        imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(columns, rows, WORKING))),
        stride,
        group_bytes,
        stride * u64::from(info.height),
        u64::from(rows.min(info.height)) * info.row_bytes() as u64,
        100_000,
    )
}

#[test]
fn forward_bands_match_direct_groups_and_all_pixels() {
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
            let info = ImageInfo::new(19, 5, channels, sample);
            let row = info.row_bytes();
            let stride = row + 7;
            let raw: Vec<u8> = (0..info.checked_buffer_len().unwrap())
                .map(|i| ((i * 73 + i / 7 * 11 + 255) % 256) as u8)
                .collect();
            let mut physical = Vec::new();
            for (y, pixels) in raw.chunks_exact(row).enumerate() {
                physical.extend_from_slice(pixels);
                if y + 1 < info.height as usize {
                    physical.extend_from_slice(&[0xa5; 7]);
                }
            }
            let consumed = physical.len();
            physical.extend_from_slice(&[0xee; 9]);
            let mut source = Cursor::new(physical);
            let cfg = config(&info, 7, 3, stride as u64);
            let (file, index, band) = encode_forward_bands_indexed(
                &mut source,
                Vec::new(),
                Cursor::new(Vec::new()),
                Cursor::new(Vec::new()),
                info.clone(),
                &Metadata::default(),
                cfg.clone(),
            )
            .unwrap();
            assert_eq!(source.position() as usize, consumed);
            assert_eq!(band.get_ref().len(), row * 3);
            let mut direct = IndexedGroupEncoder::new(
                Vec::new(),
                Cursor::new(Vec::new()),
                info.clone(),
                &Metadata::default(),
                &cfg.encoder,
                cfg.max_index_bytes,
            )
            .unwrap();
            while let Some(rect) = direct.next_group() {
                let begin = rect.y as usize * row + rect.x as usize * info.bytes_per_pixel();
                direct.write_group(&raw[begin..], row).unwrap();
            }
            let (expected, expected_index) = direct.finish().unwrap();
            assert_eq!(file, expected, "{sample:?} {channels:?}");
            assert_eq!(index.into_inner(), expected_index.into_inner());
            assert_eq!(imij::decode(&file).unwrap(), (info.clone(), raw.clone()));

            let rect = Rect::new(3, 1, 11, 3);
            let region_row = rect.width() as usize * info.bytes_per_pixel();
            let region_stride = region_row + 5;
            let mut region = RegionReader::open(SliceReader::new(&file)).unwrap();
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
                            .all(|&byte| byte == 0xcd)
                    );
                }
            }
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
fn band_admission_precedes_source_and_output() {
    let info = ImageInfo::new(19, 5, Channels::Rgb, SampleType::U8);
    let row = info.row_bytes();
    let calls = Cell::new(0);
    let mut source = CountReads(&calls);
    let mut sink = Vec::new();
    let mut index = Cursor::new(Vec::new());
    let mut band = Cursor::new(Vec::new());
    let mut cfg = config(&info, 7, 3, row as u64);
    cfg.max_band_scratch_bytes -= 1;
    assert!(matches!(
        encode_forward_bands_indexed(
            &mut source,
            &mut sink,
            &mut index,
            &mut band,
            info.clone(),
            &Metadata::default(),
            cfg,
        ),
        Err(StreamEncodeError::Codec(Error::TooLarge))
    ));
    assert_eq!(calls.get(), 0);
    assert!(sink.is_empty());
    assert!(index.get_ref().is_empty());
    assert!(band.get_ref().is_empty());

    let mut cfg = config(&info, 7, 3, row as u64);
    cfg.max_source_buffer_bytes -= 1;
    assert!(matches!(
        encode_forward_bands_indexed(
            &mut source,
            &mut sink,
            &mut index,
            &mut band,
            info.clone(),
            &Metadata::default(),
            cfg,
        ),
        Err(StreamEncodeError::Codec(Error::TooLarge))
    ));
    assert_eq!(calls.get(), 0);
    assert!(sink.is_empty());

    let mut cfg = config(&info, 7, 3, row as u64);
    cfg.max_source_bytes = row as u64 * u64::from(info.height) - 1;
    assert!(matches!(
        encode_forward_bands_indexed(
            &mut source,
            &mut sink,
            &mut index,
            &mut band,
            info.clone(),
            &Metadata::default(),
            cfg,
        ),
        Err(StreamEncodeError::Codec(Error::TooLarge))
    ));
    assert_eq!(calls.get(), 0);
    assert!(sink.is_empty());

    let mut cfg = config(&info, 7, 3, row as u64);
    cfg.max_index_bytes = 0;
    assert!(matches!(
        encode_forward_bands_indexed(
            &mut source,
            &mut sink,
            &mut index,
            &mut band,
            info.clone(),
            &Metadata::default(),
            cfg,
        ),
        Err(StreamEncodeError::Codec(Error::TooLarge))
    ));
    assert_eq!(calls.get(), 0);
    assert!(sink.is_empty());
    assert!(index.get_ref().is_empty());

    let mut occupied = Cursor::new(vec![0xa5]);
    assert!(matches!(
        encode_forward_bands_indexed(
            &mut source,
            &mut sink,
            &mut index,
            &mut occupied,
            info.clone(),
            &Metadata::default(),
            config(&info, 7, 3, row as u64),
        ),
        Err(StreamEncodeError::Codec(Error::InvalidInput(
            "forward band scratch not empty"
        )))
    ));
    assert_eq!(occupied.into_inner(), vec![0xa5]);
    assert_eq!(calls.get(), 0);
    assert!(sink.is_empty());

    assert!(matches!(
        encode_forward_bands_indexed(
            &mut source,
            &mut sink,
            &mut index,
            &mut band,
            info.clone(),
            &Metadata::default(),
            config(&info, 7, 3, row as u64 - 1),
        ),
        Err(StreamEncodeError::Codec(Error::InvalidInput(
            "forward source stride"
        )))
    ));
    assert_eq!(calls.get(), 0);
    assert!(sink.is_empty());

    assert!(matches!(
        encode_forward_bands_indexed(
            &mut source,
            &mut sink,
            &mut index,
            &mut band,
            info.clone(),
            &Metadata::default(),
            config(&info, 7, 3, row as u64),
        ),
        Err(StreamEncodeError::Read(error)) if error.kind() == io::ErrorKind::UnexpectedEof
    ));
    assert!(calls.get() > 0);
    assert!(!sink.is_empty());
}
