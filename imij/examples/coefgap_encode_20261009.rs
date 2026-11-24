//! Lossy encodes for the coefgap study's decode timing (lab; 2026-10-09).
//!
//! `coefgap_encode_20261009 RAW W H OUT Q EFFORT DC_LANES [STRIPES]` writes
//! the XYB run-level file of raw RGB8 at quality Q and effort EFFORT, its
//! nested DC image with DC_LANES lanes (0 = the lossless default), lossy
//! stripes of STRIPES block rows (0 or absent = automatic), production
//! defaults otherwise, and prints the file size. COEFGAP_KMODE=0|1|2|3 sets
//! `lab::set_table_k_mode` (0 the effort's table budgets; unset: production,
//! which chooses table counts by coded size from effort 7.5; the study's
//! timing scripts run with 0 unless an arm sets another mode).
//! COEFGAP_ENC_ITERS=N repeats the encode N times and also prints the fastest
//! encode in ms; COEFGAP_ENC_THREADS sets the encoder threads (default 4);
//! COEFGAP_TIER=compact|balanced|fast|fastest encodes with that decode-speed
//! tier (its coder choice, geometry and color) instead; COEFGAP_SEEDS=0
//! disables seed-bank tables.

fn main() {
    let a: Vec<String> = std::env::args().collect();
    assert!(
        a.len() == 8 || a.len() == 9,
        "RAW W H OUT Q EFFORT DC_LANES [STRIPES]"
    );
    let raw = std::fs::read(&a[1]).unwrap();
    let (w, h): (u32, u32) = (a[2].parse().unwrap(), a[3].parse().unwrap());
    let q: f32 = a[5].parse().unwrap();
    let effort: f32 = a[6].parse().unwrap();
    imij::lab::set_lossy_dc_lanes(a[7].parse().unwrap());
    if let Ok(m) = std::env::var("COEFGAP_KMODE") {
        imij::lab::set_table_k_mode(m.parse().unwrap());
    }
    let info = imij::ImageInfo::new(w, h, imij::Channels::Rgb, imij::SampleType::U8);
    let mut c = imij::ImiEncoderConfig::lossy(q)
        .tune_lossy(|t| t.color = imij::LossyColor::Xyb)
        .tune_lossy(|t| t.entropy = imij::LossyEntropy::RunLevel)
        .with_effort(effort)
        .with_threads(env_or("COEFGAP_ENC_THREADS", 4));
    c = c.tune_lossy(|t| {
        t.stripe_block_rows = a.get(8).map_or(0, |v| v.parse().unwrap());
    });
    if env_or("COEFGAP_SEEDS", 1) == 0 {
        c = c.tune_lossy(|t| {
            t.lab.seed_bank = 0;
        });
    }
    if let Ok(t) = std::env::var("COEFGAP_TIER") {
        c = c.with_decode_speed(match t.as_str() {
            "compact" => imij::DecodeSpeed::Compact,
            "balanced" => imij::DecodeSpeed::Balanced,
            "fast" => imij::DecodeSpeed::Fast,
            "fastest" => imij::DecodeSpeed::Fastest,
            _ => panic!("COEFGAP_TIER: compact, balanced, fast or fastest"),
        });
    }
    let iters = env_or("COEFGAP_ENC_ITERS", 0);
    let encode = || {
        c.encode_samples(
            imij::ImageRef::new(&raw, w as usize * 3, &info),
            &imij::Metadata::default(),
        )
        .unwrap()
    };
    let mut best = f64::INFINITY;
    let mut data = Vec::new();
    for _ in 0..iters.max(1) {
        let t = std::time::Instant::now();
        data = encode();
        best = best.min(t.elapsed().as_secs_f64() * 1e3);
    }
    std::fs::write(&a[4], &data).unwrap();
    if iters > 0 {
        println!("{} {best:.3}", data.len());
    } else {
        println!("{}", data.len());
    }
}

fn env_or(name: &str, default: usize) -> usize {
    std::env::var(name).map_or(default, |v| v.parse().unwrap())
}
