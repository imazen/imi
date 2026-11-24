//! Build capabilities and header recognition are separate contracts.

use imij::{
    Channels, Feature, Features, ImageInfo, ImiEncoderConfig, Metadata, Profile, ProfileVersion,
    SampleType, decode, header, supported_features, supported_profiles,
};

#[test]
fn extended_header_is_inspectable_in_every_build() {
    let info = ImageInfo::new(3, 2, Channels::Gray, SampleType::U32);
    let pixels: Vec<u8> = [0u32, 1, u32::MAX, 1 << 31, 0x1234_5678, 42]
        .into_iter()
        .flat_map(u32::to_ne_bytes)
        .collect();
    let file = imij::ImiEncoderConfig::default()
        .encode_samples(
            imij::ImageRef::new(&pixels, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let parsed = header(&file).unwrap();
    assert_eq!(parsed.info, info);
    assert!(
        parsed
            .profiles
            .iter()
            .any(|p| p.profile == Profile::Extended)
    );
    assert_eq!(
        supported_profiles()
            .iter()
            .any(|p| p.profile == Profile::Extended),
        cfg!(feature = "extended")
    );
    #[cfg(feature = "extended")]
    assert_eq!(decode(&file).unwrap().1, pixels);
    #[cfg(not(feature = "extended"))]
    assert_eq!(
        decode(&file),
        Err(imij::Error::Unsupported("32-bit samples (extended tier)"))
    );
}

#[test]
fn signed_and_wide_samples_report_their_required_capabilities() {
    let wide_bits = [0u64, u64::MAX, 1 << 63, 1, 0x7ff8_1234_5678_9abc, 42];
    let wide: Vec<u8> = wide_bits.into_iter().flat_map(u64::to_ne_bytes).collect();
    let signed16 = [i16::MIN, -1, 0, 1, i16::MAX, -42]
        .into_iter()
        .flat_map(i16::to_ne_bytes)
        .collect();
    for (sample, pixels, profile) in [
        (
            SampleType::I8,
            vec![0x80, 0xff, 0, 1, 0x7f, 42],
            Profile::Lossless,
        ),
        (SampleType::I16, signed16, Profile::Lossless),
        (SampleType::U64, wide.clone(), Profile::Extended),
        (SampleType::I64, wide.clone(), Profile::Extended),
        (SampleType::F64, wide, Profile::Extended),
    ] {
        let info = ImageInfo::new(3, 2, Channels::Gray, sample);
        // Regular maps: the sample type alone decides the profiles here
        // (six distinct values would otherwise code as value tables).
        let cfg = {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.tune_lossless(|t| {
                t.value_tables = false;
            });
            cfg
        };
        let file = cfg
            .encode_samples(
                imij::ImageRef::new(&pixels, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        let required = ProfileVersion {
            profile,
            major: 0,
            minor: 2,
        };
        let mut expected = Vec::new();
        if profile == Profile::Extended {
            expected.push(ProfileVersion {
                profile: Profile::Lossless,
                major: 0,
                minor: 1,
            });
        }
        expected.push(required);
        assert_eq!(header(&file).unwrap().profiles, expected, "{sample:?}");
        let enabled = cfg!(feature = "extended");
        assert_eq!(
            supported_profiles().contains(&required),
            enabled,
            "{sample:?}"
        );
        if enabled {
            assert_eq!(decode(&file).unwrap().1, pixels, "{sample:?}");
        } else {
            assert_eq!(
                decode(&file),
                Err(imij::Error::Unsupported(if sample.bytes() == 8 {
                    "64-bit samples (extended tier)"
                } else {
                    "signed samples (extended tier)"
                }))
            );
        }
    }
}

#[test]
fn header_reports_the_declared_features() {
    let gray = |sample| ImageInfo::new(3, 2, Channels::Gray, sample);
    let enc = |info: &ImageInfo, pixels: &[u8], cfg: &ImiEncoderConfig| {
        cfg.encode_samples(
            imij::ImageRef::new(pixels, info.row_bytes(), info),
            &Metadata::default(),
        )
        .unwrap()
    };
    // Implied by the fields: nothing declared.
    let plain = enc(
        &gray(SampleType::U8),
        &[1, 2, 3, 4, 5, 6],
        &imij::ImiEncoderConfig::default(),
    );
    assert_eq!(header(&plain).unwrap().features, Features::EMPTY);
    let u32s: Vec<u8> = (0..6u32).flat_map(u32::to_ne_bytes).collect();
    let wide = enc(
        &gray(SampleType::U32),
        &u32s,
        &imij::ImiEncoderConfig::default(),
    );
    // (The encoder may pick the offset map, a declared feature, for them.)
    let f = header(&wide).unwrap().features;
    assert!(!f.contains(Feature::WideSamples) && !f.contains(Feature::SignedSamples));
    // Declared: signed samples, near-lossless bounds.
    let signed = enc(
        &gray(SampleType::I8),
        &[0x80, 0xff, 0, 1, 0x7f, 42],
        &imij::ImiEncoderConfig::default(),
    );
    let f = header(&signed).unwrap().features;
    assert_eq!(f.iter().collect::<Vec<_>>(), [Feature::SignedSamples]);
    assert_eq!(f.bits(), 1 << Feature::SignedSamples.id());
    let ramp: Vec<u8> = (0..64u32 * 16).map(|i| (i * 7 % 251) as u8).collect();
    let near = enc(
        &ImageInfo::new(64, 16, Channels::Gray, SampleType::U8),
        &ramp,
        &{
            let mut cfg = imij::ImiEncoderConfig::default().tune_lossless(|t| t.max_error = [2; 4]);
            cfg = cfg.tune_lossless(|t| {
                t.palette = false;
            });
            cfg
        },
    );
    let f = header(&near).unwrap().features;
    assert!(f.contains(Feature::NearLossless), "{f:?}");
    assert_eq!(Feature::NearLossless.name(), "near_lossless");
    // This build decodes what it reports.
    for file in [&plain, &signed, &near] {
        if supported_features().covers(header(file).unwrap().features) {
            decode(file).unwrap();
        } else {
            assert_eq!(
                decode(file),
                Err(imij::Error::Unsupported("signed samples (extended tier)"))
            );
        }
    }
    // The Lossy 0.4 dequantization features and superblocks (Compact) are
    // implemented.
    for f in [
        Feature::DequantPresets,
        Feature::ExplicitTables,
        Feature::ReconBias,
        Feature::ColorTransform,
        Feature::Superblocks,
        Feature::SmallTransforms,
    ] {
        assert!(supported_features().contains(f), "{f:?}");
    }
    assert_eq!(
        supported_features().contains(Feature::WideSamples),
        cfg!(feature = "extended")
    );
    // Assigned bits are contiguous from 0; the first free bit and the top
    // bit are unknown to this decoder.
    let free = (0..64)
        .map(|k| 1u64 << k)
        .find(|&b| Features::from_bits(b).is_none())
        .expect("a free feature bit");
    assert!(Features::from_bits(free - 1).is_some());
    assert_eq!(Features::from_bits(1 << 63), None);
}
