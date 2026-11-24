//! Native wire follow-up: complete encode/decode against framewise and one-pass
//! controls. The caller requests exactly one frozen source/quality per process.
use super::*;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

fn encode_arm(info: &ImageInfo, pixels: &[u8], cfg: &LossyConfig, arm: &str) -> Vec<u8> {
    match arm {
        "frame" | "aa" => super::super::super::encode_lossy(
            pixels,
            info.row_bytes(),
            info,
            &Metadata::default(),
            cfg,
        )
        .unwrap(),
        "original" => encode_native(info, pixels, cfg, Preparation::OnePass),
        "global" => encode_native(info, pixels, cfg, Preparation::StudyFirstBandDc),
        "local" => {
            let mut cfg = cfg.clone();
            cfg.local_ans_contexts = true;
            encode_native(info, pixels, &cfg, Preparation::OnePass)
        }
        _ => panic!("declared native arm"),
    }
}

#[test]
fn native_ans_encode_comparison() {
    let info = ImageInfo::new(65, 73, Channels::Rgb, SampleType::U8);
    let pixels: Vec<u8> = (0..info.row_bytes() * 73).map(|v| (v * 37) as u8).collect();
    let cfg = configuration(40.0, LossyColor::Xyb, LossyEntropy::RunLevel, 64);
    let reference = encode_arm(&info, &pixels, &cfg, "frame");
    for arm in ["original", "global", "local"] {
        let encoded = encode_arm(&info, &pixels, &cfg, arm);
        assert_eq!(
            native_stats(&encoded).coefficients,
            native_stats(&reference).coefficients
        );
        assert_eq!(
            decode_native(&encoded, &info, 1),
            decode_native(&reference, &info, 1)
        );
    }
    if let Some(path) = std::env::var_os("IMI_NATIVE_ANS_MANIFEST") {
        measure(Path::new(&path));
    }
}

fn measure(path: &Path) {
    let manifest_bytes = std::fs::read(path).unwrap();
    let manifest: Value = serde_json::from_slice(&manifest_bytes).unwrap();
    let source = &manifest["source"];
    let root = Path::new(manifest["output"].as_str().unwrap());
    std::fs::create_dir(root).unwrap();
    let pixels = Arc::new(std::fs::read(source["path"].as_str().unwrap()).unwrap());
    assert_eq!(digest(&pixels), source["xxh3_128"].as_str().unwrap());
    let mut info = ImageInfo::new(
        source["width"].as_u64().unwrap() as u32,
        source["height"].as_u64().unwrap() as u32,
        Channels::Rgb,
        if source["bits"].as_u64().unwrap() == 8 {
            SampleType::U8
        } else {
            SampleType::U16
        },
    );
    let c = &source["cicp"];
    info.cicp = crate::Cicp {
        color_primaries: c[0].as_u64().unwrap() as u8,
        transfer_characteristics: c[1].as_u64().unwrap() as u8,
        matrix_coefficients: c[2].as_u64().unwrap() as u8,
        full_range: c[3].as_bool().unwrap(),
    };
    let color = if source["color"] == "xyb" {
        LossyColor::Xyb
    } else {
        LossyColor::YCoCg
    };
    let q = manifest["quality"].as_f64().unwrap() as f32;
    let cfg = configuration(q, color, LossyEntropy::RunLevel, 64);
    let mut files = Vec::new();
    let mut stats = Vec::new();
    for arm in ["frame", "original", "global", "local"] {
        let encoded = encode_arm(&info, &pixels, &cfg, arm);
        let s = native_stats(&encoded);
        stats.push(
            json!({"arm":arm,"bytes":encoded.len(),"xxh3_128":digest(&encoded),
            "coefficient_xxh3_128":s.coefficients,"dc_record_bytes":s.dc_bytes,
            "ac_record_bytes_including_local_models":s.ac_bytes}),
        );
        std::fs::write(root.join(format!("{arm}.imi")), &encoded).unwrap();
        files.push(Arc::new(encoded));
    }
    let reference = decode_native(&files[0], &info, 1);
    for f in &files {
        assert_eq!(
            native_stats(f).coefficients,
            stats[0]["coefficient_xxh3_128"].as_str().unwrap()
        );
        for threads in [1, 2, 4, 8] {
            let decoded = decode_native(f, &info, threads);
            assert_eq!(decoded.len(), reference.len());
            assert_eq!(
                decoded.iter().zip(&reference).position(|(a, b)| a != b),
                None,
                "native pixel mismatch at worker budget {threads}"
            );
        }
    }
    // The original and compatible controls must retain the previously frozen
    // native bytes; a later code change cannot silently move these baselines.
    let name = format!("{}-q{q}-g64-ans", source["name"].as_str().unwrap());
    let roots = manifest["frozen_roots"].as_array().unwrap();
    let frozen = roots
        .iter()
        .map(|r| Path::new(r.as_str().unwrap()).join(&name))
        .find(|p| p.join("COMPLETE").exists())
        .expect("retained complete baseline cell");
    for (name, actual) in [("a.imi", &files[1]), ("b.imi", &files[2])] {
        let old = std::fs::read(frozen.join(name)).unwrap();
        assert!(
            old == **actual,
            "frozen {name} changed: old={} actual={}",
            digest(&old),
            digest(actual)
        );
    }
    let parsed = super::super::super::parse_lossy(&files[3]).unwrap();
    let (mut local, mut global, mut table_bytes, mut compact) = (0, 0, 0, 0);
    for (u, bytes) in parsed.stripes.iter().enumerate() {
        let g = parsed.group(u % parsed.ngroups()).1;
        let (_, bytes) = parsed.record(&g, u / parsed.ngroups(), bytes).unwrap();
        let (table, packed, _) = super::super::super::local_ans::split(bytes).unwrap();
        if let Some(t) = table {
            local += 1;
            table_bytes += t.len();
        } else {
            global += 1;
        }
        compact += usize::from(packed);
    }
    let identity = json!({"source_revision":manifest["source_revision"],"binary_xxh3_128":digest(&std::fs::read(std::env::current_exe().unwrap()).unwrap()),
        "manifest_xxh3_128":digest(&manifest_bytes),"source":source,"quality":q,"group_rows":64,
        "arms":stats,"local_units":local,"global_units":global,"local_table_bytes":table_bytes,"compact_units":compact,
        "pixels_xxh3_128":digest(&reference),"old_native_bytes":"exact for original/global controls",
        "maps":false,"nas_writes":0});
    std::fs::write(
        root.join("correctness.json"),
        serde_json::to_vec_pretty(&identity).unwrap(),
    )
    .unwrap();
    drop(parsed);
    drop(reference);
    let gate = zenbench::GateConfig::strict()
        .max_heavy_processes(4)
        .min_available_ram_mb(4096)
        .max_cpu_temp_c(Some(85.0))
        .max_wait(Duration::from_secs(30));
    let measured = zenbench::run_gated(gate, |suite| {
        suite.plan().workload_id = Some(format!("native-ans-{}-q{q}", digest(&pixels)));
        for operation in ["encode", "decode"] {
            for threads in [1, 2, 4, 8] {
                suite.group(format!("{operation}-{threads}t"), |group| {
                    group
                        .config()
                        .min_rounds(24)
                        .max_rounds(24)
                        .auto_rounds(false)
                        .max_wall_time(Duration::from_secs(300))
                        .warmup_time(Duration::from_millis(200));
                    group.throughput(zenbench::Throughput::Elements(
                        u64::from(info.width) * u64::from(info.height),
                    ));
                    for (i, arm) in ["frame", "original", "global", "local", "aa"]
                        .into_iter()
                        .enumerate()
                    {
                        let pixels = pixels.clone();
                        let info = info.clone();
                        let mut cfg = cfg.clone();
                        cfg.threads = threads;
                        let encoded = files[i.min(3)].clone();
                        let encoded = if arm == "aa" {
                            files[0].clone()
                        } else {
                            encoded
                        };
                        group.bench(arm, move |b| {
                            if operation == "encode" {
                                b.iter(|| encode_arm(&info, &pixels, &cfg, arm))
                            } else {
                                b.iter(|| decode_native(&encoded, &info, threads))
                            }
                        });
                    }
                });
            }
        }
    });
    measured.save(root.join("timing.json")).unwrap();
    let timing: Value =
        serde_json::from_slice(&std::fs::read(root.join("timing.json")).unwrap()).unwrap();
    assert_eq!(timing["incomplete"], false);
    assert_eq!(timing["unreliable"], false);
    for group in timing["comparisons"].as_array().unwrap() {
        assert_eq!(group["completed_rounds"], 24);
        assert!(
            group["samples"]
                .as_array()
                .unwrap()
                .iter()
                .all(|s| s["gate_clean"] == true)
        );
    }
    std::fs::write(
        root.join("COMPLETE"),
        b"exact source-bound bytes, coefficients, pixels; all timing gates pass\n",
    )
    .unwrap();
}
