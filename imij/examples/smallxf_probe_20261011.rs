//! Small-transform study probe (2026-10-11, `benchmarks/smallxf_2026-10-11`):
//! encodes an RGB8 image per decode-speed tier with small transforms off,
//! priced and forced on every plane with AC (the worst case), writes the
//! files and reports bytes, 4x4 planes, merged parents and the best encode
//! time of `--reps` encodes on `--threads` encoder threads.
//!
//! `smallxf_probe_20261011 IMAGE.rgb8 W H [--q 5,20,50,90] [--effort 7.5]
//! [--tiers compact,balanced,fast,fastest] [--threads 1] [--reps 3]
//! [--write DIR]` prints one line per (tier, quality, arm): `tier q arm bytes
//! split0 split1 split2 merged parents encode_best_ms`. Arms: `b` (the tier,
//! small transforms off: main's file), `s` (small transforms priced, merges
//! by the tier's rule), `w` (every plane with AC 4x4, no merges). Files go to
//! `DIR/<tier>_q<q>_<arm>.imi`.
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("IMAGE.rgb8");
    let w: u32 = args.next().expect("W").parse().unwrap();
    let h: u32 = args.next().expect("H").parse().unwrap();
    let (mut qs, mut effort, mut threads, mut reps, mut dir) =
        (vec![5.0f32, 20.0, 50.0, 90.0], 7.5f32, 1usize, 3usize, None);
    let mut tiers: Vec<String> = ["compact", "balanced", "fast", "fastest"]
        .map(String::from)
        .to_vec();
    while let Some(a) = args.next() {
        let v = args.next().expect("value");
        match a.as_str() {
            "--q" => qs = v.split(',').map(|s| s.parse().unwrap()).collect(),
            "--effort" => effort = v.parse().unwrap(),
            "--threads" => threads = v.parse().unwrap(),
            "--reps" => reps = v.parse().unwrap(),
            "--write" => dir = Some(std::path::PathBuf::from(v)),
            "--tiers" => tiers = v.split(',').map(String::from).collect(),
            _ => panic!("unknown option {a}"),
        }
    }
    let px = std::fs::read(&path).unwrap();
    let info = imij::ImageInfo::new(w, h, imij::Channels::Rgb, imij::SampleType::U8);
    assert_eq!(px.len(), info.row_bytes() * h as usize, "raw size");
    if let Some(d) = &dir {
        std::fs::create_dir_all(d).unwrap();
    }
    for tier in &tiers {
        let speed = match tier.as_str() {
            "compact" => imij::DecodeSpeed::Compact,
            "balanced" => imij::DecodeSpeed::Balanced,
            "fast" => imij::DecodeSpeed::Fast,
            "fastest" => imij::DecodeSpeed::Fastest,
            _ => panic!("unknown tier {tier}"),
        };
        for &q in &qs {
            for arm in ["b", "s", "w"] {
                let mut cfg = imij::ImiEncoderConfig::lossy(q)
                    .with_decode_speed(speed)
                    .with_effort(effort)
                    .with_threads(threads);
                cfg = cfg.tune_lossy(|t| {
                    t.lab.switches = imij::LossyLabSwitches::defaults();
                });
                match arm {
                    "b" => {}
                    "s" => {
                        cfg = cfg.tune_lossy(|t| {
                            t.lab.switches.small = Some(Box::new(imij::SmallLab::on()))
                        });
                    }
                    _ => {
                        cfg = cfg.tune_lossy(|t| t.lab.switches.superblocks = Some(false));
                        cfg = cfg.tune_lossy(|t| {
                            t.lab.switches.small = Some(Box::new(imij::SmallLab {
                                split: Some(7),
                                ..imij::SmallLab::on()
                            }))
                        });
                    }
                }
                let mut best = f64::INFINITY;
                let mut file = Vec::new();
                for _ in 0..reps {
                    let t = Instant::now();
                    file = cfg
                        .encode_samples(
                            imij::ImageRef::new(&px, info.row_bytes(), &info),
                            &imij::Metadata::default(),
                        )
                        .unwrap();
                    best = best.min(t.elapsed().as_secs_f64() * 1e3);
                }
                let st = imij::lab::lossy_small_stats(&file).unwrap();
                println!(
                    "{tier}\t{q}\t{arm}\t{}\t{}\t{}\t{}\t{}\t{}\t{best:.2}",
                    file.len(),
                    st[2],
                    st[3],
                    st[4],
                    st[1],
                    st[0]
                );
                if let Some(d) = &dir {
                    std::fs::write(d.join(format!("{tier}_q{q}_{arm}.imi")), &file).unwrap();
                }
            }
        }
    }
}
