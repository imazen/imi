//! Memory policies must preserve samples, alpha and all decoder entry points.
use imij::{
    Channels, DecoderMemoryLimit, Error, ImageInfo, LossyEntropy, Metadata, SampleType,
    StreamDecoder, decode,
};

fn pixels(info: &ImageInfo) -> Vec<u8> {
    (0..info.width as usize * info.height as usize * info.channels.count())
        .flat_map(|i| {
            let n = (i * 37 + (i / 71) * 19) as u8;
            match info.sample {
                SampleType::U8 => vec![n],
                SampleType::U16 => (u16::from(n) * 257).to_ne_bytes().to_vec(),
                SampleType::F16 => (0x3000 + u16::from(n) * 12).to_ne_bytes().to_vec(),
                SampleType::F32 => (f32::from(n) / 255.0).to_ne_bytes().to_vec(),
                SampleType::U32 => (u32::from(n) * 0x01010101).to_ne_bytes().to_vec(),
                SampleType::I32 => ((i32::from(n) - 128) * 1_000_003).to_ne_bytes().to_vec(),
                SampleType::I8 => vec![n.wrapping_sub(128)],
                SampleType::I16 => ((i16::from(n) - 128) * 101).to_ne_bytes().to_vec(),
                SampleType::U64 => (u64::from(n) * 0x0101_0101_0101_0101)
                    .to_ne_bytes()
                    .to_vec(),
                SampleType::I64 => ((i64::from(n) - 128) * 1_000_000_007)
                    .to_ne_bytes()
                    .to_vec(),
                SampleType::F64 => (f64::from(n) / 255.0).to_ne_bytes().to_vec(),
                s => unreachable!("no pattern for {s:?}"),
            }
        })
        .collect()
}

fn parity(file: &[u8], info: &ImageInfo, expected: &[u8]) {
    assert_eq!(decode(file).unwrap().1, expected);
    let mut stream = StreamDecoder::new(file).unwrap();
    let mut row = vec![0; info.row_bytes()];
    for expected in expected.chunks_exact(info.row_bytes()) {
        assert!(stream.next_row(&mut row).unwrap());
        assert_eq!(row, expected);
    }
    assert!(!stream.next_row(&mut row).unwrap());
    #[cfg(feature = "std")]
    {
        let stride = info.row_bytes() + 11;
        let mut out = vec![0xA5; stride * info.height as usize];
        imij::decode_into_threads(file, &mut out, stride, 4).unwrap();
        for (r, expected) in out
            .chunks_exact(stride)
            .zip(expected.chunks_exact(info.row_bytes()))
        {
            assert_eq!(&r[..info.row_bytes()], expected);
            assert!(r[info.row_bytes()..].iter().all(|&v| v == 0xA5));
        }
        let mut y = 0;
        imij::decode_reader(file, &imij::Limits::DEFAULT, |_, _, row| {
            assert_eq!(row, &expected[y..y + row.len()]);
            y += row.len();
            Ok(())
        })
        .unwrap();
        assert_eq!(y, expected.len());
    }
}

#[test]
fn roomy_budget_preserves_existing_bytes() {
    let info = ImageInfo::new(129, 35, Channels::Rgba, SampleType::U8);
    let px = pixels(&info);
    let meta = Metadata::default();
    let limit = DecoderMemoryLimit::new(u64::MAX).with_threads(4);
    let cfg = imij::ImiEncoderConfig::default();
    let original = cfg
        .encode_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &meta)
        .unwrap();
    assert_eq!(
        original,
        cfg.with_decoder_memory_limit(limit)
            .encode_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &meta)
            .unwrap()
    );
    let cfg = imij::ImiEncoderConfig::lossy(30.0);
    let original = cfg
        .encode_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &meta)
        .unwrap();
    assert_eq!(
        original,
        cfg.with_decoder_memory_limit(limit)
            .encode_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &meta)
            .unwrap()
    );
}

#[test]
fn lossy_budget_preserves_pixels_and_nested_alpha() {
    for sample in [
        SampleType::U8,
        SampleType::U16,
        SampleType::F16,
        SampleType::F32,
    ] {
        for channels in [Channels::GrayAlpha, Channels::Rgba] {
            let info = ImageInfo::new(65, 73, channels, sample);
            let px = pixels(&info);
            for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                // One-block-row stripes as the low-memory baseline.
                let cfg = imij::ImiEncoderConfig::lossy(30.0)
                    .tune_lossy(|t| t.entropy = entropy)
                    .tune_lossy(|t| t.stripe_block_rows = 1);
                let bytes = cfg.estimated_decoder_memory(&info).unwrap();
                let tall = cfg.tune_lossy(|t| t.stripe_block_rows = 8);
                assert!(tall.estimated_decoder_memory(&info).unwrap() > bytes);
                let original = tall
                    .encode_samples(
                        imij::ImageRef::new(&px, info.row_bytes(), &info),
                        &Metadata::default(),
                    )
                    .unwrap();
                let expected = decode(&original).unwrap().1;
                let budget = tall.with_decoder_memory_limit(DecoderMemoryLimit::new(bytes));
                assert!(budget.estimated_decoder_memory(&info).unwrap() <= bytes);
                let file = budget
                    .encode_samples(
                        imij::ImageRef::new(&px, info.row_bytes(), &info),
                        &Metadata::default(),
                    )
                    .unwrap();
                assert_ne!(file, original, "test must exercise changed geometry");
                parity(&file, &info, &expected);
                let bps = sample.bytes();
                for (a, b) in px
                    .chunks_exact(info.bytes_per_pixel())
                    .zip(expected.chunks_exact(info.bytes_per_pixel()))
                {
                    assert_eq!(&a[a.len() - bps..], &b[b.len() - bps..]);
                }
            }
        }
    }
}

#[test]
fn lossless_budget_preserves_samples_when_lanes_change() {
    for sample in [
        SampleType::U8,
        SampleType::U16,
        SampleType::F16,
        SampleType::F32,
        SampleType::U32,
        SampleType::I32,
        SampleType::I8,
        SampleType::I16,
        SampleType::U64,
        SampleType::I64,
        SampleType::F64,
    ] {
        if sample.tier() == imij::Tier::Extended && !cfg!(feature = "extended") {
            continue;
        }
        let info = ImageInfo::new(513, 17, Channels::Rgba, sample);
        let px = pixels(&info);
        let bytes = imij::ImiEncoderConfig::default()
            .estimated_decoder_memory(&info)
            .unwrap();
        let cfg = {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.tune_lossless(|t| {
                t.lanes = Some(1);
            });
            cfg
        }
        .with_decoder_memory_limit(DecoderMemoryLimit::new(bytes));
        let file = cfg
            .encode_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        parity(&file, &info, &px);
    }
}

#[test]
fn budgets_can_select_groups_for_parallel_decode() {
    let info = ImageInfo::new(1025, 129, Channels::Rgba, SampleType::U8);
    let px = pixels(&info);
    let target = DecoderMemoryLimit::new(u64::MAX).with_threads(4);
    let grouped = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.with_column_groups(Some(256));
        cfg = cfg.tune_lossless(|t| {
            t.palette = false;
        });
        cfg = cfg.with_decoder_memory_limit(Some(target));
        cfg
    };
    let bytes = grouped.estimated_decoder_memory(&info).unwrap();
    let cfg = imij::ImiEncoderConfig::default()
        .with_decoder_memory_limit(DecoderMemoryLimit::new(bytes).with_threads(4));
    let file = cfg
        .encode_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    parity(&file, &info, &px);

    let mut grouped = imij::ImiEncoderConfig::lossy(15.0).with_decoder_memory_limit(target);
    grouped = grouped.with_column_groups(Some(256));
    let bytes = grouped.estimated_decoder_memory(&info).unwrap();
    let cfg = imij::ImiEncoderConfig::lossy(15.0)
        .with_decoder_memory_limit(DecoderMemoryLimit::new(bytes).with_threads(4));
    let reference = imij::ImiEncoderConfig::lossy(15.0)
        .encode_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let file = cfg
        .encode_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    assert_ne!(file, reference);
    parity(&file, &info, &decode(&reference).unwrap().1);
}

#[test]
fn invalid_budgets_fail_before_touching_pixels() {
    let info = ImageInfo::new(1024, 1024, Channels::Rgb, SampleType::U8);
    for limit in [
        DecoderMemoryLimit::new(0),
        DecoderMemoryLimit::new(u64::MAX).with_threads(0),
    ] {
        let cfg = imij::ImiEncoderConfig::default().with_decoder_memory_limit(limit);
        assert!(matches!(
            cfg.estimated_decoder_memory(&info),
            Err(Error::InvalidInput(_))
        ));
        assert_eq!(
            cfg.encode_samples(
                imij::ImageRef::new(&[], info.row_bytes(), &info),
                &Metadata::default()
            ),
            cfg.estimated_decoder_memory(&info).map(|_| Vec::new())
        );
        let cfg = imij::ImiEncoderConfig::lossy(90.0).with_decoder_memory_limit(limit);
        assert_eq!(
            cfg.encode_samples(
                imij::ImageRef::new(&[], info.row_bytes(), &info),
                &Metadata::default()
            ),
            cfg.estimated_decoder_memory(&info).map(|_| Vec::new())
        );
    }
}
