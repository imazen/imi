//! The reference encoder writes; only the independent oracle reconstructs.
use imij::{Channels, ImageInfo, Metadata, SampleType};

#[test]
fn all_group_sample_types_match_source_bits_in_independent_decoder() {
    for sample in [
        SampleType::U8,
        SampleType::U16,
        SampleType::F16,
        SampleType::F32,
        SampleType::I8,
        SampleType::I16,
        SampleType::U32,
        SampleType::I32,
        SampleType::U64,
        SampleType::I64,
        SampleType::F64,
    ] {
        for channels in [
            Channels::Gray,
            Channels::GrayAlpha,
            Channels::Rgb,
            Channels::Rgba,
        ] {
            let info = ImageInfo::new(23, 17, channels, sample);
            let values = [
                0u64,
                u64::MAX,
                1,
                1 << 63,
                0x7ff8_1234_7fc0_1234,
                0xfff0_0000_7f80_0000,
                0x8000_0000_8000_0000,
                0x1234_5678_9abc_def0,
            ];
            let mut source = Vec::new();
            let mut expected = Vec::new();
            for i in 0..23 * 17 * channels.count() {
                let value = values[i % values.len()];
                match sample.bytes() {
                    1 => {
                        source.push(value as u8);
                        expected.push(value as u8);
                    }
                    2 => {
                        source.extend_from_slice(&(value as u16).to_ne_bytes());
                        expected.extend_from_slice(&(value as u16).to_le_bytes());
                    }
                    4 => {
                        source.extend_from_slice(&(value as u32).to_ne_bytes());
                        expected.extend_from_slice(&(value as u32).to_le_bytes());
                    }
                    _ => {
                        source.extend_from_slice(&value.to_ne_bytes());
                        expected.extend_from_slice(&value.to_le_bytes());
                    }
                }
            }
            // The independent decoder covers V2/V3. This explicit compatibility
            // encode uses the same retained group core as the indexed writer.
            let file = imij::ImiEncoderConfig::default()
                .with_row_groups(Some(imij::RowGroups::new(9, 5, 16 << 20)))
                .encode_legacy_samples(
                    imij::ImageRef::new(&source, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap();
            let decoded = imij_oracle::decode(&file).unwrap();
            assert_eq!(
                (
                    decoded.width,
                    decoded.height,
                    decoded.channels,
                    decoded.sample
                ),
                (23, 17, channels.count() as u8, sample as u8)
            );
            assert_eq!(
                decoded.frames,
                [expected.clone()],
                "{sample:?} {channels:?}"
            );
            let mut indexed = imij::stream_encode::IndexedGroupEncoder::new(
                Vec::new(),
                std::io::Cursor::new(Vec::new()),
                info.clone(),
                &Metadata::default(),
                &imij::ImiEncoderConfig::default().with_row_groups(Some(imij::RowGroups::new(
                    9,
                    5,
                    16 << 20,
                ))),
                1000,
            )
            .unwrap();
            while let Some(g) = indexed.next_group() {
                let start = g.y as usize * info.row_bytes() + g.x as usize * info.bytes_per_pixel();
                indexed
                    .write_group(&source[start..], info.row_bytes())
                    .unwrap();
            }
            let (indexed, _) = indexed.finish().unwrap();
            let decoded = imij_oracle::decode(&indexed).unwrap();
            assert_eq!(
                decoded.frames,
                [expected],
                "indexed {sample:?} {channels:?}"
            );
        }
    }
}
