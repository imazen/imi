//! Lab probe: bytes and one-thread encode time of the trellis against the
//! default quantization, on tight RGB8 rasters.
//!
//! `cargo run --release --features lab --example trellis_probe -- RAW:WxH...`
//! Prints one TSV row per (image, color, quality, arm): bytes, encode ms,
//! and the decoded image's MSE in 8-bit code units (a sanity signal only;
//! the RD study scores with perceptual metrics). `IMIJ_PROBE_OUT=DIR` also
//! writes each file as `DIR/IMAGE_COLOR_qQ_ARM.imi`.
//! `IMIJ_PROBE_SPEED=compact,balanced,fast,fastest` encodes under those
//! decode-speed tiers instead of the two colors (the tier picks the color,
//! entropy mode and stripe heights; files `DIR/IMAGE_TIER_qQ_ARM.imi`);
//! `IMIJ_PROBE_QS=10,30` sets the qualities.
use imij::{Channels, DecodeSpeed, ImageInfo, LossyColor, LossyTrellis, Metadata, SampleType};
use std::time::Instant;

/// (label, color, tier) settings to probe.
fn settings() -> Vec<(String, LossyColor, Option<DecodeSpeed>)> {
    let Ok(list) = std::env::var("IMIJ_PROBE_SPEED") else {
        return vec![
            ("Xyb".into(), LossyColor::Xyb, None),
            ("YCoCg".into(), LossyColor::YCoCg, None),
        ];
    };
    list.split(',')
        .map(|t| {
            let speed = match t {
                "compact" => DecodeSpeed::Compact,
                "balanced" => DecodeSpeed::Balanced,
                "fast" => DecodeSpeed::Fast,
                "fastest" => DecodeSpeed::Fastest,
                _ => panic!("unknown tier {t}"),
            };
            (t.to_string(), LossyColor::Xyb, Some(speed))
        })
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    println!(
        "image\tcolor\tq\tarm\tbytes\tencode_ms\tmse\ttokens\tcoded_block_channels\tblock_channels"
    );
    for spec in &args {
        let (path, dim) = spec.rsplit_once(':').expect("RAW:WxH");
        let (w, h) = dim.split_once('x').expect("WxH");
        let (w, h): (u32, u32) = (w.parse().unwrap(), h.parse().unwrap());
        let px = std::fs::read(path).unwrap();
        assert_eq!(px.len(), (w * h * 3) as usize);
        let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
        let name = std::path::Path::new(path)
            .parent()
            .and_then(|p| p.file_name())
            .map_or(path.to_string(), |n| n.to_string_lossy().into_owned());
        let qs: Vec<f32> = std::env::var("IMIJ_PROBE_QS").map_or_else(
            |_| vec![5.0, 10.0, 20.0, 30.0, 50.0, 70.0, 90.0, 95.0],
            |v| v.split(',').map(|q| q.parse().unwrap()).collect(),
        );
        for (label, color, speed) in settings() {
            for &q in &qs {
                for (arm, trellis) in [
                    // Effort 10 selects the trellis: "off" turns it off.
                    (
                        "off",
                        Some(LossyTrellis {
                            passes: 0,
                            ..LossyTrellis::default()
                        }),
                    ),
                    ("t14.5", Some(LossyTrellis::default())),
                    (
                        "t14.5p2",
                        Some(LossyTrellis {
                            passes: 2,
                            ..LossyTrellis::default()
                        }),
                    ),
                ] {
                    let mut cfg = imij::ImiEncoderConfig::lossy(q)
                        .tune_lossy(|t| t.color = color)
                        .with_effort(10.0)
                        .tune_lossy(|t| t.stripe_block_rows = 8);
                    if let Some(speed) = speed {
                        // The tier's own stripe heights.
                        cfg = cfg.with_decode_speed(speed);
                        cfg = cfg.tune_lossy(|t| {
                            t.stripe_block_rows = 0;
                        });
                    }
                    cfg = cfg.tune_lossy(|t| {
                        t.lab.trellis = trellis;
                    });
                    let t = Instant::now();
                    let file = cfg
                        .encode_samples(
                            imij::ImageRef::new(&px, w as usize * 3, &info),
                            &Metadata::default(),
                        )
                        .unwrap();
                    let ms = t.elapsed().as_secs_f64() * 1e3;
                    if let Ok(dir) = std::env::var("IMIJ_PROBE_OUT") {
                        let q = q as u32;
                        std::fs::write(format!("{dir}/{name}_{label}_q{q}_{arm}.imi"), &file)
                            .unwrap();
                    }
                    let (_, dec) = imij::decode(&file).unwrap();
                    let mse = px
                        .iter()
                        .zip(&dec)
                        .map(|(&a, &b)| (a as f64 - b as f64).powi(2))
                        .sum::<f64>()
                        / px.len() as f64;
                    // Counts of the coded coefficients (lab counters).
                    let [tokens, _, coded] = imij::lab::lossy_side_stats();
                    let blocks = (w as u64).div_ceil(8) * (h as u64).div_ceil(8) * 3;
                    println!(
                        "{name}\t{label}\t{q}\t{arm}\t{}\t{ms:.1}\t{mse:.3}\t{tokens}\t{coded}\t{blocks}",
                        file.len()
                    );
                }
            }
        }
    }
}
