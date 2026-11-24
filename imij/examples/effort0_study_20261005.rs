//! `effort0_study_20261005` — effort-0 AC-budget regression probe.
//!
//! Encodes the frozen 36-source RGB8 set for the requested efforts and
//! prints `id  effort  bytes  wall_ns` rows. Writes each .imi under
//! --out for stream_walk byte decomposition. Wall time is DIAGNOSTIC
//! ONLY (mechanism check, not a speed claim).
//!
//! Usage: effort0_study_20261005 [--out DIR] [--src ROOT] [--repeat N]
//!        [--quality Q] [--entropy rl|sh] [--efforts a,b,c]
//!        [--time] [--threads T] [--sizes TSV] [--no-dering] [SRC_DIR ...]
//!
//! --time emits `<dir>#e<E>: <X> Mpx/s bytes=<N>` lines consumable by
//! tools/env_ab_paired.py (trailing byte count rides on the Mpx/s line)
//! and appends `arm\tsrc\tq\te\tmode\tbytes` rows to --sizes TSV
//! (arm label from $IMIJ_ARM, default "arm"). Positional args are source
//! dirs (or direct paths to a `source.rgb8` file); default = all of
//! --src ROOT.

use imij::{Channels, ImageInfo, LossyEntropy, Metadata, SampleType};
use std::fs;
use std::path::Path;
use std::time::Instant;

const SRC_ROOT: &str = "local-scratch/zqi-openjph-expanded-20261004/sources";

fn main() {
    let mut out = String::new();
    let mut root = SRC_ROOT.to_string();
    let mut repeat = 1usize;
    let mut quality = 110.0f32;
    let mut entropy = LossyEntropy::RunLevel;
    let mut efforts = vec![0.0f32, 2.5, 5.0, 7.5, 10.0];
    let mut time_mode = false;
    let mut threads = 1usize;
    let mut sizes = String::new();
    let mut file_args: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--out" => out = args.next().expect("--out DIR"),
            "--src" => root = args.next().expect("--src ROOT"),
            "--repeat" => repeat = args.next().expect("--repeat N").parse().unwrap(),
            "--quality" => quality = args.next().expect("--quality Q").parse().unwrap(),
            "--entropy" => {
                entropy = match args.next().expect("--entropy MODE").as_str() {
                    "sh" => LossyEntropy::SplitHuf,
                    _ => LossyEntropy::RunLevel,
                }
            }
            "--efforts" => {
                efforts = args
                    .next()
                    .expect("--efforts LIST")
                    .split(',')
                    .map(|s| s.parse().unwrap())
                    .collect()
            }
            "--time" => time_mode = true,
            // Encoder-only change landed after this study: compare like with like.
            "--no-dering" => {
                #[cfg(feature = "lab")]
                imij::lab::set_lossy_dering(false);
            }
            "--threads" => threads = args.next().expect("--threads T").parse().unwrap(),
            "--sizes" => sizes = args.next().expect("--sizes TSV"),
            other if other.starts_with("--") => panic!("unknown arg {other}"),
            other => file_args.push(other.to_string()),
        }
    }
    if !out.is_empty() {
        fs::create_dir_all(&out).unwrap();
    }
    let mut ids: Vec<String> = fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| {
            let n = e.unwrap().file_name().into_string().unwrap();
            Path::new(&root)
                .join(&n)
                .join("source.rgb8")
                .exists()
                .then_some(n)
        })
        .collect();
    ids.sort();
    // (label, pixel-path): positional dirs/files override the default sweep.
    let mut items: Vec<(String, String)> = if file_args.is_empty() {
        ids.iter()
            .map(|id| (format!("{root}/{id}"), format!("{root}/{id}/source.rgb8")))
            .collect()
    } else {
        file_args
            .iter()
            .map(|f| {
                let d = f.trim_end_matches('/');
                let (label, p) = if Path::new(d).is_dir() {
                    (d.to_string(), format!("{d}/source.rgb8"))
                } else {
                    let dir = Path::new(d)
                        .parent()
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_else(|| d.to_string());
                    (dir, d.to_string())
                };
                (label, p)
            })
            .collect()
    };
    items.sort();
    eprintln!("sources: {}", items.len());
    let arm = std::env::var("IMIJ_ARM").unwrap_or_else(|_| "arm".into());
    let ent_name = match entropy {
        LossyEntropy::SplitHuf => "sh",
        _ => "rl",
    };
    for (label, path) in &items {
        let px = fs::read(path).unwrap();
        let info = ImageInfo::new(
            1024,
            (px.len() / 3072) as u32,
            Channels::Rgb,
            SampleType::U8,
        );
        assert_eq!(px.len(), info.row_bytes() * info.height as usize);
        for &e in &efforts {
            let mut cfg = imij::ImiEncoderConfig::lossy(quality);
            cfg = cfg.with_effort(e);
            cfg = cfg.tune_lossy(|t| {
                t.entropy = entropy;
            });
            cfg = cfg.with_threads(threads);
            // Warm up once so the timed encode is not first-touch.
            let enc = cfg
                .encode_samples(
                    imij::ImageRef::new(&px, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap();
            let mut best = u64::MAX;
            for _ in 0..repeat {
                let t = Instant::now();
                let enc2 = cfg
                    .encode_samples(
                        imij::ImageRef::new(&px, info.row_bytes(), &info),
                        &Metadata::default(),
                    )
                    .unwrap();
                best = best.min(t.elapsed().as_nanos() as u64);
                assert_eq!(enc, enc2, "nondeterministic encode {label} e{e}");
            }
            if !out.is_empty() {
                // Labels can be absolute paths: name the output by the source
                // directory alone so it lands under --out.
                let base = Path::new(&label).file_name().unwrap().to_string_lossy();
                fs::write(Path::new(&out).join(format!("{base}-e{e}.imi")), &enc).unwrap();
            }
            if time_mode {
                let mpx = info.width as f64 * info.height as f64 * 1e3 / best as f64;
                println!("{label}#e{e}: {mpx:.3} Mpx/s bytes={}", enc.len());
            } else {
                println!("{label}\t{e}\t{}\t{best}", enc.len());
            }
            if !sizes.is_empty() {
                use std::io::Write;
                let mut f = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&sizes)
                    .unwrap();
                writeln!(
                    f,
                    "{arm}\t{label}\t{quality}\t{e}\t{ent_name}\t{}",
                    enc.len()
                )
                .unwrap();
            }
        }
    }
}
