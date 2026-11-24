//! Exact one-step resize references and mixed coding in indexed levels.
use imij::planes::*;
use imij::{Channels, Error, ImageInfo, ImiEncoderConfig, LossyColor, Metadata, SampleType};
use zenresize::{Filter, PixelDescriptor, ResizeConfig, Resizer};

fn primary() -> Vec<u8> {
    imij::ImiEncoderConfig::default()
        .with_effort(0.0)
        .encode_samples(
            imij::ImageRef::new(
                &[0],
                1,
                &ImageInfo::new(1, 1, Channels::Gray, SampleType::U8),
            ),
            &Metadata::default(),
        )
        .unwrap()
}
fn resize(input: &[u8], w: u32, h: u32, tw: u32, th: u32, format: PixelDescriptor) -> Vec<u8> {
    let config = ResizeConfig::builder(w, h, tw, th)
        .filter(Filter::Lanczos)
        .format(format)
        .build();
    Resizer::new(&config).resize(input)
}

#[test]
fn independent_lossless_levels_equal_one_step_original_resize() {
    let (w, h) = (193, 129);
    let pixels: Vec<u8> = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            if (x / 3 + y / 5) % 2 == 0 {
                ((x * 7 + y * 13) % 256) as u8
            } else {
                255
            }
        })
        .collect();
    let original = OriginalPlane::new(&pixels, w, h, w as usize, SampleType::U8).unwrap();
    let mut builder = PlaneSetBuilder::new(&primary()).unwrap();
    let plane = builder
        .add_plane(PlaneDescriptor::new("luma", SampleType::U8).with_role(PlaneRole::Luma))
        .unwrap();
    let mut expected = Vec::new();
    for (tw, th) in [(80, 53), (31, 21), (17, 11)] {
        let reference = resize(&pixels, w, h, tw, th, PixelDescriptor::GRAY8_SRGB);
        builder
            .add_level_from_original(
                plane,
                &original,
                LevelDescriptor::new(tw, th, 16, 16),
                imij::ImiEncoderConfig::default().with_effort(0.0),
                |source, rw, rh, out, stride| {
                    assert_eq!((source.width(), source.height()), (w, h));
                    assert_eq!(source.pixels().as_ptr(), pixels.as_ptr());
                    assert_eq!(stride, rw as usize);
                    out.copy_from_slice(&resize(
                        source.pixels(),
                        w,
                        h,
                        rw,
                        rh,
                        PixelDescriptor::GRAY8_SRGB,
                    ));
                    Ok(())
                },
            )
            .unwrap();
        expected.push((tw, th, reference));
    }
    // The fixtures expose cascade drift, so this test would catch accidentally
    // using the previous level as the source of a later resize.
    let cascade = resize(&expected[0].2, 80, 53, 31, 21, PixelDescriptor::GRAY8_SRGB);
    assert_ne!(cascade, expected[1].2);
    let encoded = builder.finish().unwrap();
    let mut reader =
        PlaneSetReader::open(SliceReader::new(&encoded), PlaneSetLimits::default()).unwrap();
    for (i, (tw, th, reference)) in expected.into_iter().enumerate() {
        assert_eq!(
            reader.planes()[0].levels()[i].source(),
            LevelSource::DirectFromOriginal
        );
        let mut out = vec![0; reference.len()];
        reader
            .decode_region(
                plane,
                i as u32,
                Rect::new(0, 0, tw, th),
                &mut out,
                tw as usize,
            )
            .unwrap();
        assert_eq!(out, reference, "level {i}");
    }
}

#[test]
fn packed_color_levels_mix_lossless_and_lossy_with_exact_alpha() {
    let (w, h) = (193, 129);
    let rgba: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let (x, y) = (i % w, i / w);
            [
                (x * 7 + y * 3) as u8,
                (x ^ y) as u8,
                if (x + y) % 3 == 0 { 255 } else { 0 },
                (x * 5 + y * 11) as u8,
            ]
        })
        .collect();
    let mut builder = PlaneSetBuilder::new(&primary()).unwrap();
    let roles = [
        PlaneRole::Red,
        PlaneRole::Green,
        PlaneRole::Blue,
        PlaneRole::Alpha,
    ];
    let ids: Vec<_> = roles
        .into_iter()
        .enumerate()
        .map(|(i, role)| {
            builder
                .add_plane(
                    PlaneDescriptor::new(format!("rgba.{i}"), SampleType::U8)
                        .with_role(role)
                        .with_group(7),
                )
                .unwrap()
        })
        .collect();
    let mut references = Vec::new();
    for (i, (tw, th)) in [(80, 53), (31, 21)].into_iter().enumerate() {
        // Color and alpha filtering happen together, directly from the original.
        let reference = resize(&rgba, w, h, tw, th, PixelDescriptor::RGBA8_SRGB);
        let coding = if i == 0 {
            imij::ImiEncoderConfig::default().with_effort(0.0)
        } else {
            let mut cfg = imij::ImiEncoderConfig::lossy(80.0);
            cfg = cfg.tune_lossy(|t| {
                t.color = LossyColor::YCoCg;
            });
            cfg
        };
        builder
            .add_color_level(
                &ids,
                LevelDescriptor::new(tw, th, 16, 16).with_source(LevelSource::DirectFromOriginal),
                &reference,
                tw as usize * 4,
                Channels::Rgba,
                coding,
            )
            .unwrap();
        references.push((tw, th, reference));
    }
    let encoded = builder.finish().unwrap();
    let mut reader =
        PlaneSetReader::open(SliceReader::new(&encoded), PlaneSetLimits::default()).unwrap();
    for (i, (tw, th, reference)) in references.into_iter().enumerate() {
        let mut output = vec![0; reference.len()];
        reader
            .decode_color_region(
                &ids,
                i as u32,
                Rect::new(0, 0, tw, th),
                &mut output,
                tw as usize * 4,
            )
            .unwrap();
        if i == 0 {
            assert_eq!(output, reference);
        } else {
            assert_eq!(
                output
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|p| p[3])
                    .collect::<Vec<_>>(),
                reference
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|p| p[3])
                    .collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn rejected_generated_level_never_calls_resizer() {
    let original = OriginalPlane::new(&[1, 2, 3, 4], 2, 2, 2, SampleType::U8).unwrap();
    let mut builder = PlaneSetBuilder::new(&primary())
        .unwrap()
        .with_limits(PlaneSetLimits::default().with_levels(0))
        .unwrap();
    let id = builder
        .add_plane(PlaneDescriptor::new("test", SampleType::U8))
        .unwrap();
    let result = builder.add_level_from_original(
        id,
        &original,
        LevelDescriptor::new(2, 2, 2, 2),
        imij::ImiEncoderConfig::default(),
        |_, _, _, _, _| panic!("preflight must precede callback"),
    );
    assert!(matches!(result, Err(Error::TooLarge)));
}
