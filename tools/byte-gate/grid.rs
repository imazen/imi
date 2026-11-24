// The byte-gate grid, included by main.rs and presence.rs (`include!`):
// lossy and lossless encodes of one image per configuration.
use imij::{ChannelSampling, Channels, DecodeSpeed, ImiEncoderConfig, ImageInfo, LossyChannelLayout, LossyColor, LossyEntropy, LossyTransform, Metadata, SampleType};

/// The gate's grid (shared by `main.rs` and `presence.rs` through
/// `include!`): every encode of one image, in a fixed order, with its key
/// (`name, effort, quality or "lossless", configuration`, tab-separated).
fn grid(path: &str, mut each: impl FnMut(String, Vec<u8>)) {
    {
        let stem = std::path::Path::new(path)
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let mut parts = stem.rsplitn(3, '_');
        let h: u32 = parts.next().unwrap().parse().unwrap();
        let w: u32 = parts.next().unwrap().parse().unwrap();
        let name = parts.next().unwrap().to_string();
        let rgb = std::fs::read(path).unwrap();
        assert_eq!(rgb.len(), (w * h * 3) as usize);
        let rgba: Vec<u8> = rgb
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], p[1] / 2 + 64])
            .collect();
        let sub = LossyChannelLayout::new(LossyTransform::Adaptive4, ChannelSampling::Full);
        let half = LossyChannelLayout::new(LossyTransform::Dct8, ChannelSampling::HalfBoth);
        for effort in [0.0f32, 5.0, 7.5, 8.99, 9.0, 10.0] {
            for q in [3.0f32, 10.0, 30.0, 60.0, 90.0] {
                let base = imij::ImiEncoderConfig::lossy(q).with_effort(effort);
                let mut cfgs: Vec<(&str, ImiEncoderConfig, bool)> = vec![
                    ("xyb-rl", base.clone(), false),
                    (
                        "ycocg-rl",
                        base.clone().tune_lossy(|t| t.color = LossyColor::YCoCg),
                        false,
                    ),
                    (
                        "xyb-shuf",
                        base.clone().tune_lossy(|t| t.entropy = LossyEntropy::SplitHuf),
                        false,
                    ),
                    (
                        "ycocg-shuf",
                        base.clone()
                            .tune_lossy(|t| t.color = LossyColor::YCoCg)
                            .tune_lossy(|t| t.entropy = LossyEntropy::SplitHuf),
                        false,
                    ),
                    (
                        "compact",
                        base.clone().with_decode_speed(DecodeSpeed::Compact),
                        false,
                    ),
                    (
                        "balanced",
                        base.clone().with_decode_speed(DecodeSpeed::Balanced),
                        false,
                    ),
                    (
                        "fast",
                        base.clone().with_decode_speed(DecodeSpeed::Fast),
                        false,
                    ),
                    (
                        "fastest",
                        base.clone().with_decode_speed(DecodeSpeed::Fastest),
                        false,
                    ),
                    (
                        "layouts",
                        base.clone().tune_lossy(|t| t.channel_layouts = [sub, half, half]),
                        false,
                    ),
                    ("rgba-t4", base.clone().with_threads(4), true),
                ];
                let aq = base.clone().tune_lossy(|t| t.aq_gamma = 0.5);
                cfgs.push(("aq", aq, false));
                let exact = base.clone().tune_lossy(|t| t.exact_blocks = Some(1.5));
                cfgs.push(("exact", exact, false));
                for (label, cfg, alpha) in cfgs {
                    let (px, ch) = if alpha {
                        (&rgba, Channels::Rgba)
                    } else {
                        (&rgb, Channels::Rgb)
                    };
                    let info = ImageInfo::new(w, h, ch, SampleType::U8);
                    let out =
                        cfg.encode_samples(imij::ImageRef::new(px, info.row_bytes(), &info), &Metadata::default())
                            .unwrap();
                    each(format!("{name}\te{effort}\tq{q}\t{label}"), out);
                }
            }
        }
        // Lossless: the efforts' search, palettes, column groups, checksums,
        // near-lossless and encoder threads, on RGB and RGBA.
        for effort in [0.0f32, 2.5, 5.0, 7.5, 10.0] {
            let base = imij::ImiEncoderConfig::default().with_effort(effort);
            let mut cfgs: Vec<(&str, ImiEncoderConfig, bool)> = vec![
                ("ll", base.clone(), false),
                ("ll-rgba-t4", base.clone().with_threads(4), true),
                (
                    "ll-nopal-cg64",
                    base.clone()
                        .with_column_groups(64)
                        .tune_lossless(|t| t.palette = false),
                    false,
                ),
                ("ll-csum", base.clone().with_checksum(true), false),
                ("ll-near2", base.clone().tune_lossless(|t| t.max_error = [2; 4]), false),
            ];
            if effort == 7.5 {
                cfgs.push((
                    "ll-stripes16",
                    base.clone().tune_lossless(|t| t.stripe_rows = Some(16)),
                    false,
                ));
            }
            for (label, cfg, alpha) in cfgs {
                let (px, ch) = if alpha {
                    (&rgba, Channels::Rgba)
                } else {
                    (&rgb, Channels::Rgb)
                };
                let info = ImageInfo::new(w, h, ch, SampleType::U8);
                let out =
                    cfg.encode_samples(imij::ImageRef::new(px, info.row_bytes(), &info), &Metadata::default()).unwrap();
                each(format!("{name}\te{effort}\tlossless\t{label}"), out);
            }
        }
    }
}
