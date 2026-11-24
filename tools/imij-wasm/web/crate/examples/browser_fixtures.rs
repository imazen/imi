//! Current grammar supplement: source bytes are the pixel oracle, including
//! native color, significance/alpha declarations, checksums and earlier references.
use imij::{
    AlphaMode, Channels, FrameConfig, ImageInfo, ImageReference, Metadata, SampleType,
    SequenceEncoder, SequenceItem, SequenceKind,
};
use std::{fs, path::Path};

fn raw(info: &ImageInfo) -> Vec<u8> {
    (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 149 + i / 97) as u8)
        .collect()
}

fn save(
    dir: &Path,
    rows: &mut Vec<String>,
    name: &str,
    info: &ImageInfo,
    file: &[u8],
    expected: &[u8],
    frames: bool,
) {
    fs::write(dir.join(format!("{name}.imi")), file).unwrap();
    fs::write(dir.join(format!("{name}.raw")), expected).unwrap();
    rows.push(format!(r#"{{"name":"{name}","width":{},"height":{},"channels":"{:?}","sample":"{:?}","frames":{frames}}}"#,
        info.width, info.height, info.channels, info.sample));
}

fn main() {
    let dir = std::env::args()
        .nth(1)
        .expect("new output directory required");
    generate(Path::new(&dir));
}

pub fn generate(dir: &Path) {
    fs::create_dir(dir).unwrap();
    let config = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.with_effort(0.0);
        cfg = cfg.with_threads(1);
        cfg
    };
    let mut rows = Vec::new();
    for (channels, sample) in [
        (Channels::Rgb, SampleType::U8),
        (Channels::Rgba, SampleType::U16),
        (Channels::GrayAlpha, SampleType::F16),
        (Channels::Rgb, SampleType::F32),
        (Channels::GrayAlpha, SampleType::I8),
        (Channels::Rgb, SampleType::I16),
        (Channels::Rgba, SampleType::I32),
        (Channels::Rgb, SampleType::U32),
        (Channels::GrayAlpha, SampleType::I64),
        (Channels::Rgb, SampleType::F64),
        (Channels::Rgba, SampleType::U64),
        (Channels::Cmyk, SampleType::U8),
        (Channels::Cmyk, SampleType::F32),
        (Channels::Oklab, SampleType::F32),
        (Channels::OklabAlpha, SampleType::F32),
    ] {
        let info = ImageInfo::new(33, 17, channels, sample);
        let pixels = raw(&info);
        let file = imij::add_pixel_checksum(
            config
                .encode_samples(
                    imij::ImageRef::new(&pixels, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(imij::decode(&file).unwrap().1, pixels);
        save(
            dir,
            &mut rows,
            &format!("native-{channels:?}-{sample:?}"),
            &info,
            &file,
            &pixels,
            false,
        );
    }
    for (channels, sample) in [
        (Channels::Rgb, SampleType::U8),
        (Channels::GrayAlpha, SampleType::U16),
        (Channels::Rgba, SampleType::F32),
        (Channels::Rgb, SampleType::U64),
    ] {
        let info = ImageInfo::new(33, 17, channels, sample);
        for policy in [
            imij::QuickBitSmarts::Off,
            imij::QuickBitSmarts::Packed,
            imij::QuickBitSmarts::Packets,
        ] {
            let pixels = if policy == imij::QuickBitSmarts::Off {
                raw(&info)
            } else {
                (0..info.checked_buffer_len().unwrap())
                    .map(|i| {
                        if i % info.bytes_per_pixel() % 4 == 3 {
                            0xa5
                        } else {
                            ((i / info.bytes_per_pixel() / 3) % 16) as u8 * 8 + 2
                        }
                    })
                    .collect()
            };
            let mut quick = imij::ImiEncoderConfig::quick();
            quick = quick.with_quick(policy);
            quick = quick.tune_lossless(|t| {
                t.stripe_rows = Some(5);
            });
            quick = quick.with_column_groups(Some(13));
            let file = imij::add_pixel_checksum(
                quick
                    .encode_samples(
                        imij::ImageRef::new(&pixels, info.row_bytes(), &info),
                        &Metadata::default(),
                    )
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(imij::decode(&file).unwrap().1, pixels);
            save(
                dir,
                &mut rows,
                &format!("quick-{policy:?}-{channels:?}-{sample:?}"),
                &info,
                &file,
                &pixels,
                false,
            );
        }
    }
    for mode in [
        AlphaMode::Straight,
        AlphaMode::Premultiplied,
        AlphaMode::Opaque,
        AlphaMode::Undefined,
    ] {
        let mut info = ImageInfo::new(33, 17, Channels::Rgba, SampleType::U8);
        info.alpha_mode = mode;
        let mut pixels = raw(&info);
        if mode == AlphaMode::Opaque {
            for pixel in pixels.as_chunks_mut::<4>().0 {
                pixel[3] = 255;
            }
        }
        let file = config
            .encode_samples(
                imij::ImageRef::new(&pixels, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        assert_eq!(imij::decode(&file).unwrap(), (info.clone(), pixels.clone()));
        save(
            dir,
            &mut rows,
            &format!("alpha-{mode:?}"),
            &info,
            &file,
            &pixels,
            false,
        );
    }
    let info = ImageInfo::new(33, 17, Channels::Rgb, SampleType::U16)
        .with_significant_bits(&[10; 3])
        .unwrap();
    let pixels: Vec<u8> = (0..33 * 17 * 3)
        .flat_map(|i| ((i as u16 * 29) & 1023).to_ne_bytes())
        .collect();
    let file = config
        .encode_samples(
            imij::ImageRef::new(&pixels, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    assert_eq!(imij::decode(&file).unwrap(), (info.clone(), pixels.clone()));
    save(
        dir,
        &mut rows,
        "significant-10",
        &info,
        &file,
        &pixels,
        false,
    );
    for kind in [SequenceKind::Pages, SequenceKind::Animation] {
        for (channels, sample) in [
            (Channels::Rgba, SampleType::U8),
            (Channels::Rgb, SampleType::F32),
            (Channels::Cmyk, SampleType::F32),
            (Channels::Oklab, SampleType::F32),
        ] {
            let info = ImageInfo::new(33, 17, channels, sample);
            let first = raw(&info);
            let second: Vec<u8> = first.iter().map(|b| b ^ 0x69).collect();
            let mut third = first.clone();
            third[41] ^= 3;
            let frames = [first, second, third];
            let mut encoder = SequenceEncoder::new(kind, FrameConfig::new(config.clone()));
            for (i, pixels) in frames.iter().enumerate() {
                let mut item = SequenceItem::new(pixels, info.row_bytes(), &info);
                if i == 2 {
                    item = item.with_reference(ImageReference {
                        index: 0,
                        pixels: &frames[0],
                        stride: info.row_bytes(),
                    });
                }
                encoder.push(item).unwrap();
            }
            let file = imij::add_pixel_checksum(encoder.finish().unwrap()).unwrap();
            let mut decoder = imij::FrameDecoder::new(&file).unwrap();
            for (i, expected) in frames.iter().enumerate() {
                decoder.next_frame().unwrap().unwrap();
                assert_eq!(decoder.reference_index(), (i == 2).then_some(0));
                let mut out = if i == 2 {
                    frames[0].clone()
                } else {
                    vec![0; expected.len()]
                };
                decoder.decode_into(&mut out, info.row_bytes(), 1).unwrap();
                assert_eq!(&out, expected);
            }
            assert!(decoder.next_frame().unwrap().is_none());
            save(
                dir,
                &mut rows,
                &format!("sequence-{kind:?}-{channels:?}-{sample:?}"),
                &info,
                &file,
                &frames.concat(),
                true,
            );
        }
    }
    fs::write(
        dir.join("manifest.json"),
        format!("[{}]\n", rows.join(",\n")),
    )
    .unwrap();
    println!("{} source-exact current-grammar fixtures", rows.len());
}
