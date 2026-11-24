//! PNG-compatible HDR precision and legacy whole-nit conversion.
mod common;

use imij::{Channels, ImageInfo, Limits, Metadata, SampleType};

fn clli(max: u32, average: u32) -> [u8; 8] {
    let mut out = [0; 8];
    out[..4].copy_from_slice(&max.to_be_bytes());
    out[4..].copy_from_slice(&average.to_be_bytes());
    out
}

#[test]
fn content_light_precision_and_legacy_routes() {
    let info = ImageInfo::new(5, 3, Channels::Rgb, SampleType::U8);
    let pixels: Vec<u8> = (0..45).map(|x| x * 5).collect();
    let plain = imij::ImiEncoderConfig::default()
        .encode_samples(
            imij::ImageRef::new(&pixels, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let (header, original) = common::parts(&plain);
    // Fractional nits, the PNG example (1000 / 250 nits), unknown, and u32 max.
    for values in [
        (1, 1),
        (10_000_000, 2_500_000),
        (0, 0),
        (u32::MAX, 655_350_001),
    ] {
        let expected = clli(values.0, values.1);
        let metadata = Metadata {
            content_light_level: Some(expected),
            orientation: 1,
            ..Default::default()
        };
        let file = imij::ImiEncoderConfig::default()
            .with_checksum(true)
            .encode_samples(
                imij::ImageRef::new(&pixels, info.row_bytes(), &info),
                &metadata,
            )
            .unwrap();
        assert_eq!(imij::probe(&file).unwrap().1, metadata);
        let (_, chunks) = common::parts(&file);
        assert_eq!(
            chunks.iter().find(|(t, _)| t == b"cLLI").unwrap().1,
            expected
        );
        assert_eq!(
            chunks.iter().find(|(t, _)| t == b"DATA"),
            original.iter().find(|(t, _)| t == b"DATA")
        );
        check_routes(&file, &info, &pixels, expected);
        let remux = imij::compress_metadata(&file, 1 << 20).unwrap();
        assert_eq!(imij::probe(&remux).unwrap().1, metadata);
    }
    for (old, expected) in [
        ([0, 100, 0, 50], clli(1_000_000, 500_000)),
        ([255, 255, 0, 0], clli(655_350_000, 0)),
    ] {
        let mut chunks = original.clone();
        chunks.insert(0, (*b"cLLI", old.to_vec()));
        let file = common::join(&header, &chunks, false);
        check_routes(&file, &info, &pixels, expected);
        let metadata = imij::probe(&file).unwrap().1;
        let rewrite = imij::ImiEncoderConfig::default()
            .encode_samples(
                imij::ImageRef::new(&pixels, info.row_bytes(), &info),
                &metadata,
            )
            .unwrap();
        assert_eq!(
            common::parts(&rewrite)
                .1
                .iter()
                .find(|(t, _)| t == b"cLLI")
                .unwrap()
                .1,
            expected
        );
    }
    for len in [0, 1, 3, 5, 7, 9] {
        let mut chunks = original.clone();
        chunks.insert(0, (*b"cLLI", vec![0; len]));
        let file = common::join(&header, &chunks, false);
        assert_eq!(imij::probe(&file), Err(imij::Error::Corrupt("bad cLLI")));
    }
}

fn check_routes(file: &[u8], info: &ImageInfo, pixels: &[u8], expected: [u8; 8]) {
    assert_eq!(
        imij::probe(file).unwrap().1.content_light_level,
        Some(expected)
    );
    #[cfg(feature = "std")]
    {
        let mut rows = Vec::new();
        let (_, metadata) = imij::decode_reader(file, &Limits::default(), |_, _, row| {
            rows.extend_from_slice(row);
            Ok(())
        })
        .unwrap();
        assert_eq!(rows, pixels);
        assert_eq!(metadata.content_light_level, Some(expected));
    }
    let case = common::matrix::Case {
        name: "content-light".into(),
        file: file.to_vec(),
        info: info.clone(),
        reference: pixels.to_vec(),
        frames: None,
        limits: Limits::default(),
    };
    for (route, outcome) in common::matrix::observe_all(&case) {
        assert_eq!(outcome, "ok", "{route}");
    }
}

#[test]
fn diffuse_white_preserves_bits_pixels_and_remux() {
    let info = ImageInfo::new(5, 3, Channels::Rgb, SampleType::U8);
    let pixels: Vec<u8> = (0..45).map(|i| (i * 71) as u8).collect();
    let plain = imij::ImiEncoderConfig::default()
        .encode_samples(
            imij::ImageRef::new(&pixels, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let original = common::parts(&plain).1;
    assert_eq!(imij::probe(&plain).unwrap().1.diffuse_white, None);
    for value in [f32::from_bits(1), 100.0, 203.0, 203.25, f32::MAX] {
        let white = zenpixels::hdr::DiffuseWhite::new(value);
        let meta = Metadata {
            diffuse_white: Some(white),
            orientation: 1,
            ..Default::default()
        };
        let file = imij::ImiEncoderConfig::default()
            .with_checksum(true)
            .encode_samples(imij::ImageRef::new(&pixels, info.row_bytes(), &info), &meta)
            .unwrap();
        assert_eq!(imij::probe(&file).unwrap().1, meta);
        let chunks = common::parts(&file).1;
        assert_eq!(
            chunks.iter().find(|(t, _)| t == b"dWHI").unwrap().1,
            value.to_be_bytes()
        );
        assert_eq!(
            chunks.iter().find(|(t, _)| t == b"DATA"),
            original.iter().find(|(t, _)| t == b"DATA")
        );
        let remux = imij::compress_metadata(&file, 1 << 20).unwrap();
        assert_eq!(imij::probe(&remux).unwrap().1, meta);
        #[cfg(feature = "std")]
        {
            let mut rows = Vec::new();
            let (_, read_meta) =
                imij::decode_reader(file.as_slice(), &Limits::default(), |_, _, row| {
                    rows.extend_from_slice(row);
                    Ok(())
                })
                .unwrap();
            assert_eq!(read_meta, meta);
            assert_eq!(rows, pixels);
        }
        let case = common::matrix::Case {
            name: "diffuse-white".into(),
            file,
            info: info.clone(),
            reference: pixels.clone(),
            frames: None,
            limits: Limits::default(),
        };
        for (route, outcome) in common::matrix::observe_all(&case) {
            assert_eq!(outcome, "ok", "{route}");
        }
    }
}

#[test]
fn invalid_or_duplicate_diffuse_white_is_rejected() {
    let info = ImageInfo::new(1, 1, Channels::Gray, SampleType::U8);
    let file = imij::ImiEncoderConfig::default()
        .encode_samples(imij::ImageRef::new(&[7], 1, &info), &Metadata::default())
        .unwrap();
    let (head, original) = common::parts(&file);
    let mut invalid: Vec<Vec<u8>> = [0.0, -0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY]
        .map(|v| v.to_be_bytes().to_vec())
        .into_iter()
        .collect();
    invalid.extend([0, 1, 3, 5, 8].map(|n| vec![0; n]));
    for payload in invalid {
        let mut chunks = original.clone();
        chunks.insert(0, (*b"dWHI", payload));
        let file = common::join(&head, &chunks, false);
        assert_eq!(imij::probe(&file), Err(imij::Error::Corrupt("bad dWHI")));
        assert_eq!(imij::decode(&file), Err(imij::Error::Corrupt("bad dWHI")));
        #[cfg(feature = "std")]
        {
            let mut calls = 0;
            let result = imij::decode_reader(file.as_slice(), &Limits::default(), |_, _, _| {
                calls += 1;
                Ok(())
            });
            assert_eq!(result, Err(imij::Error::Corrupt("bad dWHI")));
            assert_eq!(calls, 0);
        }
    }
    let mut chunks = original;
    chunks.insert(0, (*b"dWHI", 203f32.to_be_bytes().to_vec()));
    chunks.insert(0, (*b"dWHI", 100f32.to_be_bytes().to_vec()));
    assert!(matches!(
        imij::probe(&common::join(&head, &chunks, false)),
        Err(imij::Error::Corrupt(_))
    ));
    let illegal = Metadata {
        chunks: vec![imij::Chunk {
            tag: *b"dWHI",
            data: 203f32.to_be_bytes().to_vec(),
        }],
        ..Default::default()
    };
    assert!(matches!(
        imij::ImiEncoderConfig::default()
            .encode_samples(imij::ImageRef::new(&[7], 1, &info), &illegal),
        Err(imij::Error::InvalidInput(_))
    ));
}
