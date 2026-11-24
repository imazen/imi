#![cfg(feature = "std")]

use imij::planes::{Rect, SliceReader};
use imij::region::RegionReader;
use imij::stream_encode::{GrowingHeightGroupEncoder, IndexedGroupEncoder};
use imij::{Channels, ImageInfo, Metadata, SampleType};
use std::io::Cursor;

const BUDGET: usize = 32 << 20;

#[test]
fn growing_admits_full_group_geometry_before_scratch_or_output() {
    let meta = Metadata::default();
    for (columns, rows) in [(32, 64), (2, u32::MAX)] {
        let info = ImageInfo::new(columns, 0, Channels::Gray, SampleType::U8);
        let mut output = Vec::new();
        let mut data = Cursor::new(b"data".to_vec());
        let mut index = Cursor::new(b"index".to_vec());
        let result = GrowingHeightGroupEncoder::new(
            &mut output,
            &mut data,
            &mut index,
            info,
            &meta,
            &imij::ImiEncoderConfig::default().with_row_groups(Some(imij::RowGroups::new(
                columns,
                rows,
                2 << 20,
            ))),
            100_000,
            100_000,
        );
        assert!(matches!(
            result,
            Err(imij::stream_encode::StreamEncodeError::Codec(
                imij::Error::TooLarge
            ))
        ));
        assert!(output.is_empty());
        assert_eq!(data.position(), 0);
        assert_eq!(index.position(), 0);
        assert_eq!(data.get_ref(), b"data");
        assert_eq!(index.get_ref(), b"index");
    }
    // The same budget admits a small group; unknown height itself is not costly.
    GrowingHeightGroupEncoder::new(
        Vec::new(),
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        ImageInfo::new(32, 0, Channels::Gray, SampleType::U8),
        &meta,
        &imij::ImiEncoderConfig::default().with_row_groups(Some(imij::RowGroups::new(
            32,
            1,
            2 << 20,
        ))),
        100_000,
        100_000,
    )
    .unwrap();
}

#[test]
fn discovered_height_matches_fixed_height_bytes_and_region_pixels() {
    for (width, height, columns, rows, channels, sample) in [
        (1, 1, 1, 5, Channels::GrayAlpha, SampleType::U16),
        (19, 13, 7, 5, Channels::Gray, SampleType::U8),
        (35, 4, 64, 2, Channels::Rgba, SampleType::F32),
        (65, 29, 16, 7, Channels::Gray, SampleType::F32),
    ] {
        let mut info = ImageInfo::new(width, height, channels, sample);
        let mut meta = Metadata::default();
        if sample == SampleType::U16 {
            info = info.with_significant_bits(&[12, 10]).unwrap();
            meta.orientation = 6;
        }
        let raw: Vec<_> = (0..info.checked_buffer_len().unwrap())
            .map(|i| ((i * 73 + i / 7 * 11 + 255) % 256) as u8)
            .collect();
        let cfg = imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(columns, rows, BUDGET)));
        let mut fixed = IndexedGroupEncoder::new(
            Vec::new(),
            Cursor::new(Vec::new()),
            info.clone(),
            &meta,
            &cfg,
            100_000,
        )
        .unwrap();
        let mut unknown = info.clone();
        unknown.height = 0;
        let mut growing = GrowingHeightGroupEncoder::new(
            Vec::new(),
            Cursor::new(b"data prefix".to_vec()),
            Cursor::new(b"index prefix".to_vec()),
            unknown,
            &meta,
            &cfg,
            1_000_000,
            100_000,
        )
        .unwrap();
        let final_stripe = height / rows * rows;
        while let Some(rect) = fixed.next_group() {
            if height % rows != 0 && rect.y == final_stripe && rect.x == 0 {
                growing.set_final_height(height).unwrap();
            }
            assert_eq!(growing.next_group(), Some(rect));
            let start =
                rect.y as usize * info.row_bytes() + rect.x as usize * info.bytes_per_pixel();
            growing
                .write_group(&raw[start..], info.row_bytes())
                .unwrap();
            fixed.write_group(&raw[start..], info.row_bytes()).unwrap();
        }
        if height % rows == 0 {
            growing.set_final_height(height).unwrap();
        }
        assert_eq!(growing.next_group(), None);
        let (fixed_file, _) = fixed.finish().unwrap();
        let (file, data_scratch, index_scratch) = growing.finish().unwrap();
        assert_eq!(file, fixed_file);
        assert!(data_scratch.get_ref().starts_with(b"data prefix"));
        assert!(index_scratch.get_ref().starts_with(b"index prefix"));
        assert_eq!(imij::decode(&file).unwrap(), (info.clone(), raw.clone()));
        let rect = Rect::new(
            width / 3,
            height / 3,
            width - width / 3,
            height - height / 3,
        );
        let mut pixels =
            vec![0; rect.width() as usize * rect.height() as usize * info.bytes_per_pixel()];
        let mut reader = RegionReader::open(SliceReader::new(&file)).unwrap();
        assert!(reader.has_unit_table());
        reader
            .decode_region(
                rect,
                &mut pixels,
                rect.width() as usize * info.bytes_per_pixel(),
            )
            .unwrap();
        for row in 0..rect.height() as usize {
            let src = (rect.y() as usize + row) * info.row_bytes()
                + rect.x() as usize * info.bytes_per_pixel();
            let len = rect.width() as usize * info.bytes_per_pixel();
            assert_eq!(&pixels[row * len..(row + 1) * len], &raw[src..src + len]);
        }
    }
}

#[test]
fn final_height_requires_boundary_and_scratch_cap_is_sticky() {
    let info = ImageInfo::new(4, 0, Channels::Gray, SampleType::U8);
    let cfg =
        imij::ImiEncoderConfig::default().with_row_groups(Some(imij::RowGroups::new(2, 2, BUDGET)));
    let meta = Metadata::default();
    let mut growing = GrowingHeightGroupEncoder::new(
        Vec::new(),
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        info.clone(),
        &meta,
        &cfg,
        100_000,
        100_000,
    )
    .unwrap();
    assert!(growing.set_final_height(0).is_err());
    growing.write_group(&[1, 2, 3, 4], 2).unwrap();
    assert!(growing.set_final_height(1).is_err());
    assert_eq!(growing.next_group().unwrap().x, 2);
    growing.write_group(&[5, 6, 7, 8], 2).unwrap();
    assert!(growing.set_final_height(1).is_err());
    growing.set_final_height(3).unwrap();
    assert_eq!(growing.next_group().unwrap().height, 1);
    growing.write_group(&[9, 10], 2).unwrap();
    growing.write_group(&[11, 12], 2).unwrap();
    let (file, _, _) = growing.finish().unwrap();
    assert_eq!(
        imij::decode(&file).unwrap().1,
        vec![1, 2, 5, 6, 3, 4, 7, 8, 9, 10, 11, 12]
    );

    let mut capped = GrowingHeightGroupEncoder::new(
        Vec::new(),
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        info,
        &meta,
        &cfg,
        1,
        10,
    )
    .unwrap();
    assert!(capped.write_group(&[1, 2, 3, 4], 2).is_err());
    assert!(capped.next_group().is_none());
    assert!(capped.set_final_height(2).is_err());
}

#[cfg(target_pointer_width = "64")]
#[test]
fn unknown_height_does_not_admit_an_imaginary_maximum_image() {
    let info = ImageInfo::new(u32::MAX, 0, Channels::Gray, SampleType::F32);
    let encoder = GrowingHeightGroupEncoder::new(
        Vec::new(),
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        info,
        &Metadata::default(),
        &imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(1, 1, BUDGET))),
        100,
        100,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(encoder.next_group().unwrap().height, 1);
}
