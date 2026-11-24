//! The retired experiment is v2-only and never admitted by v3 or converters.
mod common;
use imij::{Channels, Error, ImageInfo, SampleType};

#[test]
fn v3_rejects_retired_native_capability_even_without_native_payload() {
    let info = ImageInfo::new(9, 17, Channels::Rgb, SampleType::U8);
    let pixels = vec![93; info.checked_buffer_len().unwrap()];
    let lossless = imij::ImiEncoderConfig::default()
        .encode_samples(
            imij::ImageRef::new(&pixels, info.row_bytes(), &info),
            &imij::Metadata::default(),
        )
        .unwrap();
    let lossy = imij::ImiEncoderConfig::lossy(90.0)
        .encode_samples(
            imij::ImageRef::new(&pixels, info.row_bytes(), &info),
            &imij::Metadata::default(),
        )
        .unwrap();
    for file in [&lossless, &lossy] {
        let mask = common::feature_mask(file) | (1 << 18);
        let over = common::with_feature_mask(file, Some(mask));
        assert_eq!(
            imij::decode(&over).err(),
            Some(Error::Unsupported("native components retired in v3"))
        );
        assert_eq!(
            imij::upgrade_image(&over, &imij::UpgradeLimits::default()).err(),
            Some(Error::Unsupported("native components retired in v3"))
        );
    }
}

#[test]
#[cfg(feature = "native-jpeg")]
fn native_syntax_requires_its_feature_on_every_route() {
    let image = imij::jpeg::CoefficientImage {
        width: 1,
        height: 9,
        color: imij::jpeg::ComponentColor::Gray,
        siting: imij::jpeg::ChromaSiting::Unspecified,
        tables: [Some([1; 64]), None, None, None],
        components: vec![imij::jpeg::QuantizedComponent {
            info: imij::jpeg::ComponentInfo {
                id: 3,
                horizontal_sampling: 1,
                vertical_sampling: 1,
                quant_table: 0,
                blocks_wide: 1,
                blocks_high: 2,
            },
            coefficients: vec![0; 128],
        }],
    };
    let mut file = image
        .encode(
            &imij::Metadata::default(),
            1,
            imij::LossyEntropy::RunLevel,
            7.5,
        )
        .unwrap();
    // Reuse the current-header mutation helper without changing its v3 contract.
    set_version(&mut file, 3);
    assert_v3_rejected(&file);
    let mut bad = common::with_feature_mask(&file, Some(common::feature_mask(&file) & !(1 << 18)));
    set_version(&mut bad, 2);
    assert_eq!(
        imij::decode(&bad).err(),
        Some(Error::Corrupt("bad stripe height"))
    );
    let expected = Err(Error::Corrupt("bad stripe height"));
    let info = image.image_info();
    let mut out = vec![0; info.checked_buffer_len().unwrap()];
    assert_eq!(
        imij::decode_into(&bad, &mut out, info.row_bytes()).map(|_| ()),
        expected
    );
    for threads in [1, 2, 8] {
        assert_eq!(
            imij::decode_into_threads(&bad, &mut out, info.row_bytes(), threads).map(|_| ()),
            expected
        );
    }
    assert_eq!(imij::StreamDecoder::new(&bad).map(|_| ()), expected);
    assert_eq!(
        imij::region::RegionReader::open(imij::planes::SliceReader::new(&bad)).map(|_| ()),
        expected
    );
    assert_eq!(
        imij::decode_reader(
            std::io::Cursor::new(&bad),
            &imij::Limits::DEFAULT,
            |_, _, _| panic!("undeclared native syntax emitted pixels")
        )
        .map(|_| ()),
        expected
    );
    let mut session =
        imij::decode_resources::DecodeSession::new(imij::decode_resources::DecodePolicy::default());
    assert_eq!(
        session
            .decode_rows(&bad, &mut out[..info.row_bytes()], |_, _, _| panic!(
                "undeclared native syntax emitted session pixels"
            ))
            .map(|_| ()),
        expected
    );
}

fn set_version(file: &mut [u8], version: u8) {
    let mut pos = 8;
    while file[pos] & 0x80 != 0 {
        pos += 1;
    }
    file[pos + 1] = version;
}

fn assert_v3_rejected(file: &[u8]) {
    let expected = Error::Unsupported("native components retired in v3");
    assert_eq!(imij::header(file).unwrap_err(), expected);
    let mut scratch = std::io::Cursor::new(Vec::new());
    assert_eq!(
        imij::validation::validate_v3(
            &mut imij::planes::SliceReader::new(file),
            &mut scratch,
            &imij::validation::ValidationLimits::default()
        )
        .unwrap_err(),
        expected
    );
    assert!(scratch.into_inner().is_empty());
    assert_eq!(imij::decode(file).unwrap_err(), expected);
    let mut out = [0xa5; 64];
    assert_eq!(imij::decode_into(file, &mut out, 8).unwrap_err(), expected);
    assert_eq!(out, [0xa5; 64]);
    assert_eq!(imij::StreamDecoder::new(file).err(), Some(expected.clone()));
    assert_eq!(
        imij::region::RegionReader::open(imij::planes::SliceReader::new(file)).err(),
        Some(expected.clone())
    );
    assert_eq!(
        imij::decode_reader(
            std::io::Cursor::new(file),
            &imij::Limits::DEFAULT,
            |_, _, _| panic!("retired native pixels emitted")
        )
        .unwrap_err(),
        expected
    );
    assert_eq!(
        imij::upgrade_image(file, &imij::UpgradeLimits::default()).unwrap_err(),
        expected
    );
}

#[test]
fn native_declaration_is_rejected_from_v3_before_payload_in_every_build() {
    // Minimal one-pixel Gray lossy header, feature 18, followed by no DATA.
    // Header admission must reject without needing any coefficient bytes.
    let mut file = b"IMIJ\r\n\x87\n".to_vec();
    file.extend_from_slice(&[8, 3, 5, 0x80, 0x80, 0x10, 2, 1, 1]);
    assert_v3_rejected(&file);
    set_version(&mut file, 2);
    assert_eq!(
        imij::upgrade_image(&file, &imij::UpgradeLimits::default()).unwrap_err(),
        Error::Unsupported("native components cannot be upgraded")
    );
    assert_eq!(
        imij::supported_features().contains(imij::Feature::NativeComponents),
        cfg!(feature = "native-jpeg")
    );
    #[cfg(not(feature = "native-jpeg"))]
    assert_eq!(
        imij::decode(&file).unwrap_err(),
        Error::Unsupported("feature native_components")
    );
}
