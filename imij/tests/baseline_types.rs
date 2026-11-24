//! Decoder build capabilities are independent of the frozen wire profiles.
use imij::{Channels, ImageInfo, Metadata, SampleType, Tier};

#[test]
fn sample_capabilities_match_the_approved_split() {
    for (sample, tier) in [
        (SampleType::U8, Tier::Baseline),
        (SampleType::U16, Tier::Baseline),
        (SampleType::F16, Tier::Baseline),
        (SampleType::F32, Tier::Extended),
        (SampleType::I8, Tier::Extended),
        (SampleType::I16, Tier::Extended),
        (SampleType::U32, Tier::Extended),
        (SampleType::I32, Tier::Extended),
        (SampleType::U64, Tier::Extended),
        (SampleType::I64, Tier::Extended),
        (SampleType::F64, Tier::Extended),
    ] {
        assert_eq!(sample.tier(), tier);
        let info = ImageInfo::new(9, 3, Channels::Gray, sample);
        let pixels = vec![0; info.row_bytes() * 3];
        // Lossless writing and structural inspection remain available.
        let file = imij::ImiEncoderConfig::default()
            .encode_samples(
                imij::ImageRef::new(&pixels, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        assert_eq!(imij::header(&file).unwrap().info, info);
        let mut out = vec![0xa5; pixels.len()];
        if tier == Tier::Baseline || cfg!(feature = "extended") {
            imij::decode_into_threads(&file, &mut out, info.row_bytes(), 4).unwrap();
            assert_eq!(out, pixels);
        } else {
            assert!(matches!(
                imij::decode(&file),
                Err(imij::Error::Unsupported(_))
            ));
            assert!(matches!(
                imij::decode_into_threads(&file, &mut out, info.row_bytes(), 4),
                Err(imij::Error::Unsupported(_))
            ));
            assert_eq!(out, vec![0xa5; pixels.len()]);
            assert!(matches!(
                imij::StreamDecoder::new(&file),
                Err(imij::Error::Unsupported(_))
            ));
            assert!(matches!(
                imij::region::RegionReader::open(imij::planes::SliceReader::new(&file)),
                Err(imij::Error::Unsupported(_))
            ));
            let mut callbacks = 0;
            assert!(matches!(
                imij::decode_reader(&file[..], &imij::Limits::default(), |_, _, _| {
                    callbacks += 1;
                    Ok(())
                }),
                Err(imij::Error::Unsupported(_))
            ));
            assert_eq!(callbacks, 0);
        }
    }
}

#[cfg(feature = "extended")]
#[test]
fn extended_f32_preserves_raw_bits_and_strided_padding_on_public_routes() {
    let bits = [
        0, 0x80000000, 1, 0x007fffff, 0x00800000, 0x3f800000, 0xbf800000, 0x7f800000, 0xff800000,
        0x7fc01234, 0xffc05678, 0x7f801234,
    ];
    for channels in [
        Channels::Gray,
        Channels::GrayAlpha,
        Channels::Rgb,
        Channels::Rgba,
    ] {
        let info = ImageInfo::new(37, 11, channels, SampleType::F32);
        let row_bytes = info.row_bytes();
        let stride = row_bytes + 12;
        let mut input = vec![0xa5; stride * 11];
        let mut packed = Vec::new();
        for y in 0..11 {
            for (x, sample) in input[y * stride..y * stride + row_bytes]
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .enumerate()
            {
                let raw: u32 = bits[(y * 7 + x) % bits.len()];
                sample.copy_from_slice(&raw.to_ne_bytes());
                packed.extend_from_slice(sample);
            }
        }
        let file = imij::ImiEncoderConfig::default()
            .encode_samples(
                imij::ImageRef::new(&input, stride, &info),
                &Metadata::default(),
            )
            .unwrap();
        assert_eq!(imij::decode(&file).unwrap().1, packed);
        for threads in [1, 4] {
            let mut out = vec![0x5a; stride * 11];
            imij::decode_into_threads(&file, &mut out, stride, threads).unwrap();
            for y in 0..11 {
                assert_eq!(
                    &out[y * stride..y * stride + row_bytes],
                    &packed[y * row_bytes..(y + 1) * row_bytes]
                );
                assert!(
                    out[y * stride + row_bytes..(y + 1) * stride]
                        .iter()
                        .all(|&v| v == 0x5a)
                );
            }
        }
        let mut reader_pixels = Vec::new();
        imij::decode_reader(&file[..], &imij::Limits::default(), |got, _, row| {
            assert_eq!(got, &info);
            reader_pixels.extend_from_slice(row);
            Ok(())
        })
        .unwrap();
        assert_eq!(reader_pixels, packed);
        let mut stream = imij::StreamDecoder::new(&file).unwrap();
        let mut row = vec![0; row_bytes];
        for expected in packed.chunks_exact(row_bytes) {
            assert!(stream.next_row(&mut row).unwrap());
            assert_eq!(row, expected);
        }
        assert!(!stream.next_row(&mut row).unwrap());
    }
}
