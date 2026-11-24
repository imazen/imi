//! Superblock port probe (2026-10-10, `benchmarks/sbport_2026-10-10`):
//! encodes an RGB8 image in the Compact tier with superblocks off and on
//! (lab switch), writes the files and reports bytes, merged parents and the
//! best encode time of `--reps` encodes on `--threads` encoder threads.
//!
//! `sb_probe_20261010 IMAGE.rgb8 W H [--q 5,20,50,90] [--effort 7.5]
//! [--threads 1] [--reps 3] [--write DIR]` prints one line per (quality,
//! arm): `q arm bytes merged_parents parents encode_best_ms`. Arms: `b`
//! (Compact, superblocks off: main's Compact file) and `s` (the production
//! rule, which merges from 128 px). Files go to `DIR/q<q>_<arm>.imi`.
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("IMAGE.rgb8");
    let w: u32 = args.next().expect("W").parse().unwrap();
    let h: u32 = args.next().expect("H").parse().unwrap();
    let (mut qs, mut effort, mut threads, mut reps, mut dir) =
        (vec![5.0f32, 20.0, 50.0, 90.0], 7.5f32, 1usize, 3usize, None);
    while let Some(a) = args.next() {
        let v = args.next().expect("value");
        match a.as_str() {
            "--q" => qs = v.split(',').map(|s| s.parse().unwrap()).collect(),
            "--effort" => effort = v.parse().unwrap(),
            "--threads" => threads = v.parse().unwrap(),
            "--reps" => reps = v.parse().unwrap(),
            "--write" => dir = Some(std::path::PathBuf::from(v)),
            _ => panic!("unknown option {a}"),
        }
    }
    let px = std::fs::read(&path).unwrap();
    let info = imij::ImageInfo::new(w, h, imij::Channels::Rgb, imij::SampleType::U8);
    assert_eq!(px.len(), info.row_bytes() * h as usize, "raw size");
    if let Some(d) = &dir {
        std::fs::create_dir_all(d).unwrap();
    }
    for &q in &qs {
        for (arm, sb) in [("b", Some(false)), ("s", None)] {
            let mut cfg = imij::ImiEncoderConfig::lossy(q)
                .with_decode_speed(imij::DecodeSpeed::Compact)
                .with_effort(effort)
                .with_threads(threads);
            cfg = cfg.tune_lossy(|t| {
                t.lab.switches = imij::LossyLabSwitches::defaults();
            });
            cfg = cfg.tune_lossy(|t| t.lab.switches.superblocks = sb);
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
            let st = imij::lab::lossy_superblock_stats(&file).unwrap();
            println!(
                "{q}\t{arm}\t{}\t{}\t{}\t{best:.2}",
                file.len(),
                st[1] + st[2] + st[3],
                st[0]
            );
            if let Some(d) = &dir {
                std::fs::write(d.join(format!("q{q}_{arm}.imi")), &file).unwrap();
            }
        }
    }
}
