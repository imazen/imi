//! Diagnostic only: records the historical RGB8 calibration witness under the
//! production effort policy, without changing the existing test expectation.
use imij::{Channels, ImageInfo, ImiEncoderConfig, LossyColor, LossyQuality, Metadata, SampleType};

fn main() {
    let info = ImageInfo::new(61, 43, Channels::Rgb, SampleType::U8);
    let px: Vec<u8> = (0..61 * 43)
        .flat_map(|i| {
            let (x, y) = (i % 61, i / 61);
            let n = (x * 7 + y * 3 + (x * y) % 11) as u8;
            [n, n.wrapping_mul(3).wrapping_add(y as u8), 255 - n / 2]
        })
        .collect();
    let encode = |cfg: &ImiEncoderConfig| {
        cfg.encode_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap()
    };
    let decode = |cfg: &ImiEncoderConfig| imij::decode(&encode(cfg)).unwrap().1;
    println!("color\tquality\teffort\tdifferent_samples\ttotal_samples\tmode");
    for color in [LossyColor::Xyb, LossyColor::YCoCg] {
        for q in [1.0, 5.0, 12.0, 30.0, 70.0, 100.0] {
            let base = decode(
                &imij::ImiEncoderConfig::lossy(q)
                    .tune_lossy(|t| t.color = color)
                    .with_effort(8.99),
            );
            for effort in [0.0, 5.0, 7.5, 8.99, 9.0, 9.9, 10.0] {
                let cfg = imij::ImiEncoderConfig::lossy(q)
                    .tune_lossy(|t| t.color = color)
                    .with_effort(effort);
                let got = decode(&cfg);
                let different = got.iter().zip(&base).filter(|(a, b)| a != b).count();
                println!(
                    "{color:?}\t{q}\t{effort}\t{different}\t{}\tproduction",
                    got.len()
                );
                #[cfg(feature = "lab")]
                {
                    let mut cfg = cfg.clone();
                    cfg = cfg.tune_lossy(|t| {
                        t.lab.trellis = Some(imij::LossyTrellis {
                            passes: 0,
                            ..Default::default()
                        });
                    });
                    let got = decode(&cfg);
                    let different = got.iter().zip(&base).filter(|(a, b)| a != b).count();
                    println!(
                        "{color:?}\t{q}\t{effort}\t{different}\t{}\tlab-off",
                        got.len()
                    );
                    assert_eq!(got, base);
                }
                // Both public request forms must still resolve to identical bytes.
                let target = LossyQuality::Ssim2(70.0);
                let a = imij::ImiEncoderConfig::lossy(90.0)
                    .with_quality(target)
                    .tune_lossy(|t| t.color = color)
                    .with_effort(effort);
                let b =
                    imij::ImiEncoderConfig::lossy(target.to_scale(color, info.width, info.height))
                        .tune_lossy(|t| t.color = color)
                        .with_effort(effort);
                assert_eq!(encode(&a), encode(&b));
            }
        }
    }
    println!(
        "geometry_color\tquality\teffort\tstripe_block_rows\tdifferent_samples\ttotal_samples"
    );
    for color in [LossyColor::Xyb, LossyColor::YCoCg] {
        for q in [1.0, 5.0, 12.0, 30.0, 70.0, 100.0] {
            for effort in [7.5, 9.0, 9.9, 10.0] {
                let cfg = imij::ImiEncoderConfig::lossy(q)
                    .tune_lossy(|t| t.color = color)
                    .with_effort(effort);
                let base = decode(&cfg.clone().tune_lossy(|t| t.stripe_block_rows = 8));
                for rows in [1, 2, 8] {
                    let got = decode(&cfg.clone().tune_lossy(|t| t.stripe_block_rows = rows));
                    let different = got.iter().zip(&base).filter(|(a, b)| a != b).count();
                    println!(
                        "{color:?}\t{q}\t{effort}\t{rows}\t{different}\t{}",
                        got.len()
                    );
                }
            }
        }
    }
}
