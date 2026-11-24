//! Native DC grouping and scheduler controls. AC bytes stay frozen.
use super::*;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

fn frame(info: &ImageInfo, pixels: &[u8], cfg: &LossyConfig, k: u32) -> Vec<u8> {
    let mut cfg = cfg.clone();
    cfg.lab.dc_stripes = Some(k);
    super::super::super::encode_lossy(pixels, info.row_bytes(), info, &Metadata::default(), &cfg)
        .unwrap()
}

fn check(info: &ImageInfo, pixels: &[u8], cfg: &LossyConfig) -> Vec<Vec<u8>> {
    let files: Vec<_> = [1, 4, 16]
        .into_iter()
        .map(|k| frame(info, pixels, cfg, k))
        .collect();
    let parsed = super::super::super::parse_lossy(&files[0]).unwrap();
    let coefficients = native_stats(&files[0]).coefficients;
    let pixels = decode_native(&files[0], info, 1);
    for file in &files {
        let p = super::super::super::parse_lossy(file).unwrap();
        assert_eq!(
            p.stripes, parsed.stripes,
            "DC geometry must retain every AC payload"
        );
        let mut a = BitWriter::new();
        let mut b = BitWriter::new();
        p.tables.write(&mut a);
        parsed.tables.write(&mut b);
        assert_eq!(a.finish(), b.finish(), "AC model must stay frozen");
        assert_eq!(native_stats(file).coefficients, coefficients);
        for threads in [1, 2, 4, 8] {
            assert_eq!(decode_native(file, info, threads), pixels);
        }
    }
    files
}

#[test]
fn dc_grouping_oracle() {
    let info = ImageInfo::new(129, 1025, Channels::Rgb, SampleType::U8);
    let pixels: Vec<_> = (0..info.row_bytes() * info.height as usize)
        .map(|v| (v * 37 + v / 31) as u8)
        .collect();
    let cfg = configuration(40.0, LossyColor::Xyb, LossyEntropy::RunLevel, 64);
    check(&info, &pixels, &cfg);
    if let Some(path) = std::env::var_os("IMI_DC_GROUP_MANIFEST") {
        measure(Path::new(&path));
    }
}

fn decode_mode(file: &[u8], info: &ImageInfo, threads: usize, mode: u8, split: usize) -> Vec<u8> {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            super::super::super::set_shared_dc(2);
            super::super::super::set_dc_split(0);
        }
    }
    let _reset = Reset;
    super::super::super::set_shared_dc(mode);
    super::super::super::set_dc_split(split);
    decode_native(file, info, threads)
}

fn measure(path: &Path) {
    let bytes = std::fs::read(path).unwrap();
    let manifest: Value = serde_json::from_slice(&bytes).unwrap();
    let source = &manifest["source"];
    let pixels = Arc::new(std::fs::read(source["path"].as_str().unwrap()).unwrap());
    assert_eq!(digest(&pixels), source["xxh3_128"].as_str().unwrap());
    let mut info = ImageInfo::new(
        source["width"].as_u64().unwrap() as u32,
        source["height"].as_u64().unwrap() as u32,
        Channels::Rgb,
        if source["bits"] == 8 {
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
    let cfg = configuration(
        manifest["quality"].as_f64().unwrap() as f32,
        color,
        LossyEntropy::RunLevel,
        64,
    );
    let root = Path::new(manifest["output"].as_str().unwrap());
    std::fs::create_dir(root).unwrap();
    let files: Vec<_> = check(&info, &pixels, &cfg)
        .into_iter()
        .map(Arc::new)
        .collect();
    let reference = decode_native(&files[0], &info, 1);
    for mode in [0, 1, 2] {
        for split in [0, 1, 2] {
            for threads in [1, 2, 4, 8] {
                assert_eq!(
                    decode_mode(&files[2], &info, threads, mode, split),
                    reference
                );
            }
        }
    }
    let mut arms = Vec::new();
    for (&k, file) in [1, 4, 16].iter().zip(&files) {
        std::fs::write(root.join(format!("dc-k{k}.imi")), &**file).unwrap();
        let p = super::super::super::parse_lossy(file).unwrap();
        let stats = native_stats(file);
        assert_eq!(p.dc_k(), k as usize);
        arms.push(json!({"dc_k":k,"dc_units":p.dc.as_ref().unwrap().stripes.len(),"ac_units":p.stripes.len(),"bytes":file.len(),"dc_bytes":stats.dc_bytes,"ac_bytes":stats.ac_bytes,"coefficient_xxh3_128":stats.coefficients,"xxh3_128":digest(file)}));
    }
    std::fs::write(root.join("correctness.json"),serde_json::to_vec_pretty(&json!({
        "source_revision":manifest["source_revision"],"binary_xxh3_128":digest(&std::fs::read(std::env::current_exe().unwrap()).unwrap()),
        "source":source,"quality":manifest["quality"],"group_rows":64,"arms":arms,"ac_tables_and_payloads":"exact across DC geometries","pixels_xxh3_128":digest(&reference),"maps":false,"nas_writes":0
    })).unwrap()).unwrap();
    drop(reference);
    let gate = zenbench::GateConfig::strict()
        .max_heavy_processes(4)
        .min_available_ram_mb(4096)
        .max_cpu_temp_c(Some(85.0))
        .max_wait(Duration::from_secs(30));
    let result = zenbench::run_gated(gate, |suite| {
        suite.plan().workload_id = Some(format!(
            "dc-group-{}-q{}",
            source["name"], manifest["quality"]
        ));
        for threads in [1, 8] {
            suite.group(format!("encode-{threads}t"), |g| {
                g.config()
                    .min_rounds(24)
                    .max_rounds(24)
                    .auto_rounds(false)
                    .warmup_time(Duration::from_millis(200))
                    .max_wall_time(Duration::from_secs(180));
                g.throughput(zenbench::Throughput::Elements(
                    u64::from(info.width) * u64::from(info.height),
                ));
                for (name, k) in [("k1", 1), ("k4", 4), ("k16", 16), ("aa-k1", 1)] {
                    let info = info.clone();
                    let pixels = pixels.clone();
                    let mut cfg = cfg.clone();
                    cfg.threads = threads;
                    g.bench(name, move |b| b.iter(|| frame(&info, &pixels, &cfg, k)));
                }
            });
        }
        for threads in [1, 2, 4, 8] {
            suite.group(format!("decode-{threads}t"), |g| {
                g.config()
                    .min_rounds(24)
                    .max_rounds(24)
                    .auto_rounds(false)
                    .warmup_time(Duration::from_millis(200))
                    .max_wall_time(Duration::from_secs(180));
                g.throughput(zenbench::Throughput::Elements(
                    u64::from(info.width) * u64::from(info.height),
                ));
                for (name, i, mode, split) in [
                    ("k1", 0, 2, 0),
                    ("k4", 1, 2, 0),
                    ("k16", 2, 2, 0),
                    ("k16-own-dc", 2, 0, 0),
                    ("k16-first-dc", 2, 1, 0),
                    ("k16-split1", 2, 2, 1),
                    ("k16-split2", 2, 2, 2),
                    ("aa-k1", 0, 2, 0),
                ] {
                    let info = info.clone();
                    let file = files[i].clone();
                    g.bench(name, move |b| {
                        b.iter(|| decode_mode(&file, &info, threads, mode, split))
                    });
                }
            });
        }
    });
    result.save(root.join("timing.json")).unwrap();
    let t: Value =
        serde_json::from_slice(&std::fs::read(root.join("timing.json")).unwrap()).unwrap();
    assert_eq!(t["incomplete"], false);
    assert_eq!(t["unreliable"], false);
    for g in t["comparisons"].as_array().unwrap() {
        assert_eq!(g["completed_rounds"], 24);
        assert!(
            g["samples"]
                .as_array()
                .unwrap()
                .iter()
                .all(|s| s["gate_clean"] == true)
        );
    }
    std::fs::write(
        root.join("COMPLETE"),
        b"native DC grouping and scheduler oracles and paired gates pass\n",
    )
    .unwrap();
}
