//! LAB encoder for the 2026-10-09 AQ + trellis study's decode timing
//! (`benchmarks/aq_trellis_2026-10-09`): one raw RGB8 image to one file.
//!
//! `aq_trellis_encode_20261009 RAW.rgb8 W H OUT.imij QUALITY EFFORT [FIELD] [K] [ENTROPY] [THREADS]`
//! FIELD: `off` or `<strength>,<levels>,<cell>,<r|a>,<f|u>` (jpegli-field AQ
//! map, `ImiEncoderConfig::aq_field`); K: the trellis's lambda exponent per AQ
//! multiplier (`lab::set_trellis_aq_lambda_exp`); ENTROPY: `rl` (run-level,
//! default), `sh` (split-Huffman) or `sh-huf` (split-Huffman with the
//! split-Huffman rate model, `lab::set_trellis_huf_rate`); THREADS: encoder
//! threads (default 1). XYB, automatic stripes (the encoder's defaults
//! otherwise). Prints the encode seconds.
use imij::{Channels, ImageInfo, Metadata, SampleType};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 7 {
        eprintln!("aq_trellis_encode_20261009 RAW W H OUT QUALITY EFFORT [FIELD] [K] [ENTROPY]");
        std::process::exit(2);
    }
    let (w, h): (u32, u32) = (a[2].parse().unwrap(), a[3].parse().unwrap());
    let px = std::fs::read(&a[1]).expect("read raw");
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    assert_eq!(px.len(), info.row_bytes() * h as usize, "raw size");
    let mut cfg = imij::ImiEncoderConfig::lossy(a[5].parse::<f32>().unwrap())
        .with_effort(a[6].parse().unwrap());
    if let Some(f) = a.get(7).filter(|f| *f != "off") {
        let p: Vec<&str> = f.split(',').collect();
        cfg = cfg.tune_lossy(|t| {
            t.lab.aq_field = Some(imij::LossyAqField {
                strength: p[0].parse().unwrap(),
                levels: p[1].parse().unwrap(),
                cell: p[2].parse().unwrap(),
                relative: p[3] == "r",
                fade: p[4] == "f",
            });
        });
    }
    if let Some(k) = a.get(8) {
        imij::lab::set_trellis_aq_lambda_exp(k.parse().unwrap());
    }
    match a.get(9).map(String::as_str) {
        None | Some("rl") => {}
        Some("sh") => cfg = cfg.tune_lossy(|t| t.entropy = imij::LossyEntropy::SplitHuf),
        Some("sh-huf") => {
            cfg = cfg.tune_lossy(|t| t.entropy = imij::LossyEntropy::SplitHuf);
            imij::lab::set_trellis_huf_rate(true);
        }
        Some(e) => panic!("entropy {e}"),
    }
    // One encoder thread by default (the bytes do not depend on it): timing.
    cfg = cfg.with_threads(a.get(10).map_or(1, |t| t.parse().unwrap()));
    let t = std::time::Instant::now();
    let out = cfg
        .encode_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    println!("{:.6}", t.elapsed().as_secs_f64());
    std::fs::write(&a[4], out).unwrap();
}
