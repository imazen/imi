//! Opt-in bitmap-checksum fixtures; existing sealed generations stay unchanged.
use super::*;
use imij::{FrameConfig, FrameDuration, FrameEncoder};

fn raw_bits(info: &ImageInfo) -> Vec<u8> {
    let mut out = Vec::new();
    let bits = [
        0u64,
        1,
        u64::MAX,
        0x8000_0000_0000_0000,
        0x7ff8_0000_0000_1234,
        0xfff0_0000_0000_0000,
        0x7fc0_1234,
        0x8000_0000,
        0x7c01,
        0x8000,
    ];
    for i in 0..info.width as usize * info.height as usize * info.channels.count() {
        let value = bits[i % bits.len()].rotate_left((i % 7) as u32);
        match info.sample.bytes() {
            1 => out.push(value as u8),
            2 => out.extend_from_slice(&(value as u16).to_ne_bytes()),
            4 => out.extend_from_slice(&(value as u32).to_ne_bytes()),
            8 => out.extend_from_slice(&value.to_ne_bytes()),
            _ => unreachable!(),
        }
    }
    out
}

fn near_config(channels: Channels) -> ImiEncoderConfig {
    let mut max_error = [5; 4];
    if channels.has_alpha() {
        max_error[channels.count() - 1] = 0;
    }
    {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.max_error = max_error;
        });
        cfg = cfg.with_threads(1);
        cfg
    }
}

pub(super) fn generate(dir: &Path) {
    let mut fixtures = Fixtures::new(dir);
    for sample in [
        SampleType::U8,
        SampleType::I8,
        SampleType::U16,
        SampleType::I16,
        SampleType::F16,
        SampleType::U32,
        SampleType::I32,
        SampleType::F32,
        SampleType::U64,
        SampleType::I64,
        SampleType::F64,
    ] {
        for channels in CHANNELS {
            for (w, h) in [(1, 1), (17, 19), (65, 67)] {
                let info = ImageInfo::new(w, h, channels, sample);
                let source = raw_bits(&info);
                let cfg = {
                    let mut cfg = imij::ImiEncoderConfig::default();
                    cfg = cfg.tune_lossless(|t| {
                        t.stripe_rows = Some(16);
                    });
                    cfg = cfg.with_column_groups(Some(32));
                    cfg = cfg.with_threads(1);
                    cfg
                };
                let file = cfg
                    .encode_samples(
                        imij::ImageRef::new(&source, info.row_bytes(), &info),
                        &Metadata::default(),
                    )
                    .unwrap();
                let file = imij::add_pixel_checksum(file).unwrap();
                fixtures.save(
                    &format!("psum-{sample:?}-{channels:?}-{w}x{h}"),
                    &info,
                    &source,
                    file,
                    true,
                );
            }
        }
    }
    for sample in [
        SampleType::U8,
        SampleType::U16,
        SampleType::F16,
        SampleType::U32,
        SampleType::F32,
    ] {
        for channels in CHANNELS {
            for color in [LossyColor::Xyb, LossyColor::YCoCg] {
                if sample.is_float() && color == LossyColor::YCoCg {
                    continue;
                } // Undefined lossy input combination.
                for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                    let info = ImageInfo::new(65, 67, channels, sample);
                    let source = pixels(&info, 6, true);
                    let mut cfg = imij::ImiEncoderConfig::lossy(20.0);
                    cfg = cfg.tune_lossy(|t| {
                        t.color = color;
                    });
                    cfg = cfg.tune_lossy(|t| {
                        t.entropy = entropy;
                    });
                    cfg = cfg.with_threads(1);
                    cfg = cfg.with_checksum(true);
                    let file = cfg
                        .encode_samples(
                            imij::ImageRef::new(&source, info.row_bytes(), &info),
                            &Metadata::default(),
                        )
                        .unwrap();
                    let file = imij::add_pixel_checksum(file).unwrap();
                    fixtures.save(
                        &format!("psum-lossy-{sample:?}-{channels:?}-{color:?}-{entropy:?}"),
                        &info,
                        &source,
                        file,
                        false,
                    );
                }
            }
        }
    }
    // Check low and high quality, production trellis, compact exact blocks,
    // and both entropy coders on the same native reconstruction contract.
    for quality in [5.0, 20.0, 50.0, 90.0] {
        for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
            for effort in [9.0, 10.0] {
                let info = ImageInfo::new(65, 67, Channels::Rgba, SampleType::U8);
                let source = pixels(&info, 6, false);
                let mut cfg = imij::ImiEncoderConfig::lossy(quality)
                    .with_threads(1)
                    .with_decode_speed(imij::DecodeSpeed::Compact);
                cfg = cfg.tune_lossy(|t| {
                    t.entropy = entropy;
                });
                cfg = cfg.with_effort(effort);
                cfg = cfg.tune_lossy(|t| {
                    t.exact_blocks = Some(10.0);
                });
                let file = cfg
                    .encode_samples(
                        imij::ImageRef::new(&source, info.row_bytes(), &info),
                        &Metadata::default(),
                    )
                    .unwrap();
                fixtures.save(
                    &format!("psum-compact-q{quality}-e{effort}-{entropy:?}"),
                    &info,
                    &source,
                    imij::add_pixel_checksum(file).unwrap(),
                    false,
                );
            }
        }
    }
    for sample in [SampleType::U8, SampleType::U16, SampleType::U32] {
        for channels in CHANNELS {
            let info = ImageInfo::new(65, 67, channels, sample);
            let source = pixels(&info, 6, false);
            let cfg = near_config(channels);
            let file = cfg
                .encode_samples(
                    imij::ImageRef::new(&source, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap();
            let file = imij::add_pixel_checksum(file).unwrap();
            fixtures.save(
                &format!("psum-near-{sample:?}-{channels:?}"),
                &info,
                &source,
                file,
                false,
            );
        }
    }
    for channels in [Channels::Gray, Channels::Rgba] {
        for coding in [
            {
                let mut cfg = imij::ImiEncoderConfig::default();
                cfg = cfg.with_threads(1);
                cfg
            },
            imij::ImiEncoderConfig::lossy(20.0).with_threads(1),
        ] {
            let lossless = coding.coder().is_lossless();
            let info = ImageInfo::new(65, 67, channels, SampleType::U8);
            let first = pixels(&info, 6, false);
            let mut changed = first.clone();
            changed[info.row_bytes() * 21 + info.bytes_per_pixel() * 25] ^= 0x7f;
            let originals = [first, changed.clone(), changed];
            let mut encoder = FrameEncoder::animation(
                info.clone(),
                Metadata::default(),
                FrameConfig::new(coding.clone()),
            );
            for source in &originals {
                encoder
                    .push_frame(source, info.row_bytes(), FrameDuration::from_millis(10))
                    .unwrap();
            }
            let file = imij::add_pixel_checksum(encoder.finish().unwrap()).unwrap();
            extra::save_frames(
                &mut fixtures,
                &format!("psum-animation-{channels:?}-{lossless}"),
                &info,
                file,
                &originals,
                lossless,
            );
            let mut encoder =
                FrameEncoder::pages(info.clone(), Metadata::default(), FrameConfig::new(coding));
            let mut pages = Vec::new();
            for (w, h) in [(65, 67), (17, 19), (1, 1)] {
                let page = ImageInfo::new(w, h, channels, SampleType::U8);
                let source = pixels(&page, 6, false);
                encoder.push_page(&source, page.row_bytes(), &page).unwrap();
                pages.push(source);
            }
            let file = imij::add_pixel_checksum(encoder.finish().unwrap()).unwrap();
            extra::save_frames(
                &mut fixtures,
                &format!("psum-pages-{channels:?}-{lossless}"),
                &info,
                file,
                &pages,
                lossless,
            );
        }
    }
    let coding = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.with_threads(1);
        cfg
    };
    let mut encoder = FrameEncoder::mixed_pages(FrameConfig::new(coding.clone()));
    let first = ImageInfo::new(17, 19, Channels::Gray, SampleType::U8);
    let mut originals = Vec::new();
    for info in [
        first.clone(),
        ImageInfo::new(65, 67, Channels::Rgba, SampleType::F64),
        ImageInfo::new(1, 1, Channels::GrayAlpha, SampleType::U16),
    ] {
        let source = raw_bits(&info);
        encoder
            .push_page_with(
                &source,
                info.row_bytes(),
                &info,
                &Metadata::default(),
                &coding,
            )
            .unwrap();
        originals.push(source);
    }
    let file = imij::add_pixel_checksum(encoder.finish().unwrap()).unwrap();
    extra::save_frames(
        &mut fixtures,
        "psum-mixed-pages",
        &first,
        file,
        &originals,
        true,
    );
    fixtures.finish();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_configuration_preserves_actual_alpha() {
        for (channels, bounds) in [
            (Channels::Gray, [5; 4]),
            (Channels::Rgb, [5; 4]),
            (Channels::GrayAlpha, [5, 0, 5, 5]),
            (Channels::Rgba, [5, 5, 5, 0]),
        ] {
            let cfg = near_config(channels);
            cfg.clone()
                .tune_lossless(|tuning| assert_eq!(tuning.max_error, bounds));
            let info = ImageInfo::new(17, 19, channels, SampleType::U8);
            let source = pixels(&info, 6, false);
            let encoded = cfg
                .encode_samples(
                    imij::ImageRef::new(&source, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap();
            let sealed = imij::add_pixel_checksum(encoded).unwrap();
            let (_, decoded) = imij::decode(&sealed).unwrap();
            for (expected, actual) in source
                .chunks(channels.count())
                .zip(decoded.chunks(channels.count()))
            {
                for c in 0..channels.count() {
                    assert!(u32::from(expected[c].abs_diff(actual[c])) <= bounds[c]);
                }
            }
        }
    }
}
