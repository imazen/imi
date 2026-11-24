use crate::{ByteReader, Error, Header};

/// Read a V4 stored-sample declaration within a serialized header-byte ceiling.
/// This does not validate chunks, models or pixels. Ordinary header inspection
/// dispatches V4 and the separate V2/V3 compatibility parsers.
pub fn header(data: &[u8], max_header_bytes: u64) -> Result<Header, Error> {
    Ok(crate::parse::header::read_header_v4(&mut ByteReader::new(data), max_header_bytes)?.header)
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::{Channels, Coding, DecodeSpeed, Features, ImageInfo, Metadata, SampleType};
    use alloc::vec::Vec;

    #[test]
    fn v4_is_explicit_and_preserves_native_sample_declarations() {
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
            for channels in [
                Channels::Gray,
                Channels::GrayAlpha,
                Channels::Rgb,
                Channels::Rgba,
                Channels::Cmyk,
                Channels::Oklab,
                Channels::OklabAlpha,
            ] {
                let info = ImageInfo::new(41, 31, channels, sample);
                let mut encoded = Vec::new();
                crate::container::write_group_head_features(
                    &mut encoded,
                    &info,
                    &Metadata::default(),
                    Coding::Lossless,
                    None,
                    DecodeSpeed::Balanced,
                    Features::EMPTY,
                    4,
                )
                .unwrap();
                let parsed = header(&encoded, 64 << 10).unwrap();
                assert_eq!(parsed.version, 4);
                assert_eq!(parsed.info, info);
                assert_eq!(crate::header(&encoded).unwrap().version, 4);
                assert!(matches!(header(&encoded, 1), Err(Error::TooLarge)));
                // The bounded V3 path must retain the exact old header bytes.
                let mut old = Vec::new();
                crate::container::write_head_features(
                    &mut old,
                    &info,
                    &Metadata::default(),
                    Coding::Lossless,
                    None,
                    DecodeSpeed::Balanced,
                    Features::EMPTY,
                )
                .unwrap();
                let mut bounded = Vec::new();
                crate::container::write_group_head_features(
                    &mut bounded,
                    &info,
                    &Metadata::default(),
                    Coding::Lossless,
                    None,
                    DecodeSpeed::Balanced,
                    Features::EMPTY,
                    3,
                )
                .unwrap();
                assert_eq!(old, bounded);
            }
        }
    }

    #[test]
    fn former_optional_syntax_feature_bits_are_reserved_in_v4() {
        let info = ImageInfo::new(41, 31, Channels::Rgb, SampleType::U8);
        for feature in [
            crate::Feature::UnitOrdinals,
            crate::Feature::LocalAnsContexts,
            crate::Feature::SeedBankIds,
            crate::Feature::CodingDefaults,
            crate::Feature::DcCoverage,
            crate::Feature::ModelReferences,
        ] {
            let mut encoded = Vec::new();
            crate::container::write_head_features(
                &mut encoded,
                &info,
                &Metadata::default(),
                Coding::Lossless,
                None,
                DecodeSpeed::Balanced,
                Features::EMPTY.with(feature),
            )
            .unwrap();
            let mut r = ByteReader::new(&encoded);
            r.bytes(8).unwrap();
            r.varint().unwrap();
            let version_at = r.pos;
            encoded[version_at] = 4;
            assert!(matches!(
                header(&encoded, 64 << 10),
                Err(Error::Corrupt("reserved V4 syntax features"))
            ));
            let mut out = Vec::new();
            assert!(
                crate::container::write_group_head_features(
                    &mut out,
                    &info,
                    &Metadata::default(),
                    Coding::Lossless,
                    None,
                    DecodeSpeed::Balanced,
                    Features::EMPTY.with(feature),
                    4
                )
                .is_err()
            );
            assert!(out.is_empty());
        }
    }

    #[test]
    fn bounded_root_setup_does_not_require_a_resident_whole_raster() {
        let info = ImageInfo::new(1_000_000, 1_000_000, Channels::Gray, SampleType::U16);
        let mut encoded = Vec::new();
        crate::container::write_group_head_features(
            &mut encoded,
            &info,
            &Metadata::default(),
            Coding::Lossless,
            None,
            DecodeSpeed::Balanced,
            Features::EMPTY,
            4,
        )
        .unwrap();
        assert_eq!(header(&encoded, 64 << 10).unwrap().info, info);
        let cfg = crate::ImiEncoderConfig::lossless().with_row_groups(crate::RowGroups::new(
            9,
            7,
            8 << 20,
        ));
        let spatial = crate::v4::SpatialConfig {
            tile_columns: 3,
            tile_rows: 3,
            order: crate::v4::TileOrder::Morton,
            integrity: crate::v4::ObjectIntegrity::Objects,
            limits: crate::v4::TileLimits {
                max_object_bytes: 1 << 20,
                max_tile_bytes: 16 << 20,
            },
        };
        let (mut encoder, setup) =
            crate::v4::NumericGroupEncoder::new(info.clone(), &cfg, spatial).unwrap();
        let session = crate::decode_resources::DecodeSession::new(
            crate::decode_resources::DecodePolicy::default(),
        );
        let (mut decoder, _) = crate::v4::NumericGroupDecoder::new(
            info,
            crate::supported_features(),
            &setup,
            spatial.limits,
            &session,
        )
        .unwrap();
        encoder.begin_tile().unwrap();
        assert!(matches!(
            decoder.read(encoder.output()).unwrap(),
            crate::v4::NumericEvent::Begin { .. }
        ));
        encoder.consume_output(encoder.output().len()).unwrap();
        encoder.push_group(&[0; 9 * 7 * 2], 9 * 2).unwrap();
        let crate::v4::NumericEvent::Group(work) = decoder.read(encoder.output()).unwrap() else {
            panic!("expected first group")
        };
        let mut decoded = [0xff; 9 * 7 * 2];
        work.decode_into(&mut decoded, 9 * 2, &session).unwrap();
        assert_eq!(decoded, [0; 9 * 7 * 2]);
    }
}
