//! Independent wire admission and unchanged reconstructed samples.
// The independent decoder implements V2/V3; fixtures select that grammar explicitly.
#[test]
fn diffuse_white_wire_values_and_duplicates() {
    let info = imij::ImageInfo::new(1, 1, imij::Channels::Gray, imij::SampleType::U8);
    let file = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(
            imij::ImageRef::new(&[173], 1, &info),
            &imij::Metadata::default(),
        )
        .unwrap();
    // The complete header length is one byte for this tiny default file.
    assert!(file[8] < 128);
    let end = 9 + file[8] as usize;
    for bits in [
        1u32,
        203f32.to_bits(),
        187.25f32.to_bits(),
        f32::MAX.to_bits(),
        0,
        0x8000_0000,
        0xbf80_0000,
        0x7f80_0000,
        0xff80_0000,
        0x7fc0_0001,
    ] {
        let mut changed = file.clone();
        let chunk: Vec<u8> = b"dWHI"
            .iter()
            .copied()
            .chain([5])
            .chain(bits.to_be_bytes())
            .collect();
        changed.splice(end..end, chunk.iter().copied());
        let result = imij_oracle::decode(&changed);
        if bits > 0 && bits < 0x7f80_0000 {
            assert_eq!(result.unwrap().frames, [vec![173]]);
            changed.splice(end..end, chunk);
            assert!(matches!(
                imij_oracle::decode(&changed),
                Err(imij_oracle::Error::Corrupt(_))
            ));
        } else {
            assert!(matches!(result, Err(imij_oracle::Error::Corrupt(_))));
        }
    }
}
