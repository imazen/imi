//! Frozen quick-pixel units: every storage type, raw/packet choices and low bits.
use super::*;
pub(super) fn generate(dir: &Path) {
    let mut fixtures = Fixtures::new(dir);
    for sample in [
        SampleType::U8,
        SampleType::U16,
        SampleType::F16,
        SampleType::I8,
        SampleType::I16,
        SampleType::U32,
        SampleType::I32,
        SampleType::F32,
        SampleType::U64,
        SampleType::I64,
        SampleType::F64,
    ] {
        for channels in CHANNELS {
            let info = ImageInfo::new(67, 65, channels, sample);
            for kind in 0..3 {
                let mut state = 0x9152ea31u32;
                let raw: Vec<_> = (0..info.checked_buffer_len().unwrap())
                    .map(|i| {
                        state ^= state << 13;
                        state ^= state >> 17;
                        state ^= state << 5;
                        match kind {
                            0 => 0,
                            1 => (i / info.bytes_per_pixel() / 73) as u8,
                            _ => state as u8,
                        }
                    })
                    .collect();
                let mut cfg = imij::ImiEncoderConfig::quick();
                cfg = cfg.tune_lossless(|t| {
                    t.stripe_rows = Some(16);
                });
                cfg = cfg.with_column_groups(Some(31));
                cfg = cfg.with_threads(2);
                let file = imij::add_pixel_checksum(
                    cfg.encode_samples(
                        imij::ImageRef::new(&raw, info.row_bytes(), &info),
                        &Metadata::default(),
                    )
                    .unwrap(),
                )
                .unwrap();
                fixtures.save(
                    &format!("quick-{sample:?}-{channels:?}-{kind}"),
                    &info,
                    &raw,
                    file,
                    true,
                );
            }
        }
    }
    fixtures.finish();
}

/// Separate opt-in domain: exact masks, sparse values, repeated planes and fallback.
pub(super) fn generate_bits(dir: &Path) {
    let mut fixtures = Fixtures::new(dir);
    for policy in [imij::QuickBitSmarts::Packed, imij::QuickBitSmarts::Packets] {
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
            for channels in CHANNELS {
                let info = ImageInfo::new(67, 65, channels, sample);
                for kind in 0..3 {
                    let mut state = 0x9152ea31u32;
                    let raw: Vec<_> = (0..info.checked_buffer_len().unwrap())
                        .map(|i| {
                            state ^= state << 13;
                            state ^= state >> 17;
                            state ^= state << 5;
                            let ordinal = i / info.bytes_per_pixel();
                            match kind {
                                0 => {
                                    if i % info.bytes_per_pixel() % 4 == 3 {
                                        0xa5
                                    } else {
                                        ((ordinal / 3) % 16) as u8 * 8 + 2
                                    }
                                }
                                1 => ((ordinal / 7) % 3) as u8 * 85,
                                _ => state as u8,
                            }
                        })
                        .collect();
                    let mut cfg = imij::ImiEncoderConfig::quick();
                    cfg = cfg.with_quick(policy);
                    cfg = cfg.tune_lossless(|t| {
                        t.stripe_rows = Some(16);
                    });
                    cfg = cfg.with_column_groups(Some(31));
                    cfg = cfg.with_threads(2);
                    let file = imij::add_pixel_checksum(
                        cfg.encode_samples(
                            imij::ImageRef::new(&raw, info.row_bytes(), &info),
                            &Metadata::default(),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                    fixtures.save(
                        &format!("quick-bits-{policy:?}-{sample:?}-{channels:?}-{kind}"),
                        &info,
                        &raw,
                        file,
                        true,
                    );
                }
            }
        }
        for bits in 2..=7 {
            let info = ImageInfo::new(127, 65, Channels::Gray, SampleType::U16);
            let raw: Vec<u8> = (0..127 * 65)
                .flat_map(|i| {
                    (((i % (1 << bits)) as u32 * 65535 / ((1 << bits) - 1)) as u16).to_ne_bytes()
                })
                .collect();
            let mut cfg = imij::ImiEncoderConfig::quick();
            cfg = cfg.with_quick(policy);
            cfg = cfg.tune_lossless(|t| {
                t.stripe_rows = Some(32);
            });
            let file = cfg
                .encode_samples(
                    imij::ImageRef::new(&raw, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap();
            fixtures.save(
                &format!("quick-bits-{policy:?}-replicated-{bits}"),
                &info,
                &raw,
                file,
                true,
            );
        }
    }
    fixtures.finish();
}
