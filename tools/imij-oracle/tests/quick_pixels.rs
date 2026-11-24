//! Production writers checked by the separately implemented SPEC 2.13 reader.
// The independent decoder implements V2/V3; fixtures select that grammar explicitly.
use imij::{Channels, ImageInfo, Metadata, SampleType};
#[test]
fn independent_packet_and_raw_units_preserve_every_storage_width() {
    for sample in [
        SampleType::U8,
        SampleType::I8,
        SampleType::U16,
        SampleType::I16,
        SampleType::F16,
        SampleType::U32,
        SampleType::I32,
        SampleType::F32,
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
            let info = ImageInfo::new(67, 9, channels, sample);
            for pattern in 0..3 {
                let mut state = 0x72a59314u32;
                let raw: Vec<u8> = (0..info.checked_buffer_len().unwrap())
                    .map(|i| {
                        state ^= state << 13;
                        state ^= state >> 17;
                        state ^= state << 5;
                        match pattern {
                            0 => 0,
                            1 => (i / 73) as u8,
                            _ => state as u8,
                        }
                    })
                    .collect();
                let mut cfg = imij::ImiEncoderConfig::quick();
                cfg = cfg.tune_lossless(|t| {
                    t.stripe_rows = Some(4);
                });
                cfg = cfg.with_column_groups(Some(31));
                let file = cfg
                    .encode_legacy_samples(
                        imij::ImageRef::new(&raw, info.row_bytes(), &info),
                        &Metadata::default(),
                    )
                    .unwrap();
                let decoded = imij_oracle::decode(&file).unwrap();
                let expected: Vec<u8> = raw
                    .chunks_exact(sample.bytes())
                    .flat_map(|word| {
                        let mut word = word.to_vec();
                        if cfg!(target_endian = "big") {
                            word.reverse();
                        }
                        word
                    })
                    .collect();
                assert_eq!(
                    decoded.frames,
                    [expected],
                    "{sample:?} {channels:?} {pattern}"
                );
            }
        }
    }
}
