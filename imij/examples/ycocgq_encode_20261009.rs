//! Lossy encodes for the ycocgq study's decode timing (lab; 2026-10-10).
//!
//! `ycocgq_encode_20261009 RAW W H OUT Q EFFORT [STRIPES]` writes one file
//! from raw RGB8, production defaults otherwise, and prints its size.
//!
//! Environment:
//! * `YCOCGQ_COLOR=xyb|ycocg` (default ycocg)
//! * `YCOCGQ_MATRIX=0..4|63|255` (`ImiEncoderConfig::matrix`; 255 = automatic)
//! * `YCOCGQ_BIAS=K:B1:B2` (fixed bias, 1/256 steps, all channels) or
//!   `K:centroid` (per-image centroid, band split K)
//! * `YCOCGQ_TRANSFORM=identity|bt601|bt709|ycocg` (signalled color
//!   transform; requires YCoCg color)
//! * `YCOCGQ_U16=1`: encode the RGB8 source widened x257 as U16 YCoCg
//! * `YCOCGQ_ROUND=F`: quantization rounding offset (default 0.4)
//! * `YCOCGQ_ENTROPY=runlevel|splithuf` (default runlevel)
//! * `YCOCGQ_TIER=compact|balanced|fast|fastest`
//! * `YCOCGQ_ENC_THREADS=N` (default 4), `YCOCGQ_ENC_ITERS=N` (also prints
//!   the fastest encode in ms)
//! * `YCOCGQ_SEED=0` disables seed tables

fn main() {
    let a: Vec<String> = std::env::args().collect();
    assert!(
        a.len() == 7 || a.len() == 8,
        "RAW W H OUT Q EFFORT [STRIPES]"
    );
    let raw = std::fs::read(&a[1]).unwrap();
    let (w, h): (u32, u32) = (a[2].parse().unwrap(), a[3].parse().unwrap());
    let q: f32 = a[5].parse().unwrap();
    let effort: f32 = a[6].parse().unwrap();
    let u16_mode = std::env::var("YCOCGQ_U16").is_ok_and(|v| v == "1");
    let (pixels, sample, bpp) = if u16_mode {
        let px: Vec<u8> = raw
            .iter()
            .flat_map(|&v| (u16::from(v) * 257).to_ne_bytes())
            .collect();
        (px, imij::SampleType::U16, 6usize)
    } else {
        (raw.clone(), imij::SampleType::U8, 3usize)
    };
    let color = match std::env::var("YCOCGQ_COLOR").as_deref() {
        Ok("xyb") => imij::LossyColor::Xyb,
        Ok("ycocg") | Err(_) => imij::LossyColor::YCoCg,
        Ok(other) => panic!("YCOCGQ_COLOR: {other}"),
    };
    let info = imij::ImageInfo::new(w, h, imij::Channels::Rgb, sample);
    let mut c = imij::ImiEncoderConfig::lossy(q)
        .tune_lossy(|t| t.color = color)
        .with_effort(effort)
        .with_threads(env_or("YCOCGQ_ENC_THREADS", 4));
    if let Ok(e) = std::env::var("YCOCGQ_ENTROPY") {
        c = c.tune_lossy(|t| {
            t.entropy = match e.as_str() {
                "runlevel" => imij::LossyEntropy::RunLevel,
                "splithuf" => imij::LossyEntropy::SplitHuf,
                _ => panic!("YCOCGQ_ENTROPY: {e}"),
            }
        });
    }
    if let Ok(t) = std::env::var("YCOCGQ_TIER") {
        c = c.with_decode_speed(match t.as_str() {
            "compact" => imij::DecodeSpeed::Compact,
            "balanced" => imij::DecodeSpeed::Balanced,
            "fast" => imij::DecodeSpeed::Fast,
            "fastest" => imij::DecodeSpeed::Fastest,
            _ => panic!("YCOCGQ_TIER: {t}"),
        });
    }
    if let Ok(m) = std::env::var("YCOCGQ_MATRIX") {
        c = c.tune_lossy(|t| {
            t.matrix = m.parse().unwrap();
        });
    }
    if let Ok(r) = std::env::var("YCOCGQ_ROUND") {
        c = c.tune_lossy(|t| {
            t.round = r.parse().unwrap();
        });
    }
    c = c.tune_lossy(|t| {
        t.lab.switches = imij::LossyLabSwitches::defaults();
    });
    if std::env::var("YCOCGQ_SEED").is_ok_and(|v| v == "0") {
        c = c.tune_lossy(|t| {
            t.lab.seed_bank = 0;
        });
    }
    if let Ok(b) = std::env::var("YCOCGQ_BIAS") {
        let parts: Vec<&str> = b.split(':').collect();
        assert!(parts.len() == 3, "YCOCGQ_BIAS=K:B1:B2 or K:centroid");
        let k: u8 = parts[0].parse().unwrap();
        if parts[1] == "centroid" {
            c = c.tune_lossy(|t| t.lab.switches.recon_bias_centroid = Some(k));
        } else {
            let b1: i8 = parts[1].parse().unwrap();
            let b2: i8 = parts[2].parse().unwrap();
            c = c.tune_lossy(|t| {
                t.lab.switches.recon_bias = Some(imij::ReconBias {
                    k,
                    b: [[[b1, b2], [b1, b2]]; 3],
                })
            });
        }
    }
    if let Ok(t) = std::env::var("YCOCGQ_TRANSFORM") {
        c = c.tune_lossy(|tuning| {
            tuning.lab.switches.color_transform = Some(match t.as_str() {
                "identity" => imij::ColorTransform::Identity,
                "bt601" => imij::ColorTransform::Bt601,
                "bt709" => imij::ColorTransform::Bt709,
                "ycocg" => imij::ColorTransform::YCoCg,
                _ => panic!("YCOCGQ_TRANSFORM: {t}"),
            })
        });
    }
    c = c.tune_lossy(|t| {
        t.stripe_block_rows = a.get(7).map_or(0, |v| v.parse().unwrap());
    });
    let iters = env_or("YCOCGQ_ENC_ITERS", 0);
    let encode = || {
        c.encode_samples(
            imij::ImageRef::new(&pixels, w as usize * bpp, &info),
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
