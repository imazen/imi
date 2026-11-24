//! The writer/upgrade surface is independent of optional decode sample kernels.
use imij::{Channels, ImageInfo, ImageRef, ImiEncoderConfig, Metadata, SampleType};
#[test]
fn all_native_numeric_types_encode_and_migrate_without_extended_decoder() {
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
        let info = ImageInfo::new(17, 19, Channels::Rgba, sample);
        let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
            .map(|i| (i * 73 + i / 7 * 41) as u8)
            .collect();
        for config in [ImiEncoderConfig::lossless(), ImiEncoderConfig::quick()] {
            let image = ImageRef::packed(&pixels, &info);
            let legacy = config
                .encode_legacy_samples(image, &Metadata::default())
                .unwrap();
            assert_eq!(imij::header(&legacy).unwrap().version, 3);
            let output = config.encode_samples(image, &Metadata::default()).unwrap();
            assert_eq!(imij::header(&output).unwrap().version, 4);
            assert_eq!(imij::header(&output).unwrap().info, info);
            let migrated = imij::upgrade_image(&legacy, &Default::default()).unwrap();
            assert_eq!(output, migrated);
            assert_eq!(
                imij::upgrade_image(&output, &Default::default()).unwrap(),
                output
            );
        }
    }
}
