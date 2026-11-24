//! The independent decoder preserves the stored final slot for all alpha modes.
// The independent decoder implements V2/V3; fixtures select that grammar explicitly.
#[test]
fn opaque_and_padding_lossless_oracle() {
    for channels in [imij::Channels::GrayAlpha, imij::Channels::Rgba] {
        for mode in [imij::AlphaMode::Opaque, imij::AlphaMode::Undefined] {
            let mut info = imij::ImageInfo::new(19, 11, channels, imij::SampleType::U8);
            info.alpha_mode = mode;
            let raw: Vec<u8> = (0..info.checked_buffer_len().unwrap())
                .map(|i| {
                    if mode == imij::AlphaMode::Opaque
                        && i % channels.count() == channels.count() - 1
                    {
                        255
                    } else {
                        (i * 71) as u8
                    }
                })
                .collect();
            let file = imij::ImiEncoderConfig::default()
                .encode_legacy_samples(
                    imij::ImageRef::new(&raw, info.row_bytes(), &info),
                    &imij::Metadata::default(),
                )
                .unwrap();
            assert_eq!(imij_oracle::decode(&file).unwrap().frames, [raw]);
            // Complete tiny header fits in one byte; flip the declared bit20 only.
            assert!(file[8] < 128);
            let mut bad = file.clone();
            // version, presence (two bytes), then feature mask (three bytes).
            assert_eq!(bad[9], 3);
            assert_eq!(bad[10] & 0x80, 0x80);
            assert_eq!(bad[11] & 0x80, 0);
            bad[14] &= !(1 << 6);
            // Keep a canonical nonzero high byte by additionally declaring an implemented bit16.
            bad[14] |= 1 << 2;
            assert!(matches!(
                imij_oracle::decode(&bad),
                Err(imij_oracle::Error::Corrupt(_))
            ));
        }
    }
}
