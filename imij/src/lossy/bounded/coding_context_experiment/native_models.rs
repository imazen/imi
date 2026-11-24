//! Same native encoder/decoder pipeline; only AC model placement varies.
use super::super::super::model_candidates::{Mode, Settings};
use super::*;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const ARMS: [&str; 11] = [
    "legacy",
    "global",
    "root-full",
    "root-fast",
    "inline-full",
    "inline-fast",
    "catalog-full",
    "catalog-fast",
    "mixed-full",
    "mixed-fast",
    "aa",
];

fn encode(
    info: &ImageInfo,
    pixels: &[u8],
    cfg: &LossyConfig,
    arm: &str,
    buckets: usize,
) -> Vec<u8> {
    let mut cfg = cfg.clone();
    cfg.catalog_lab = match arm {
        "legacy" | "aa" => None,
        "global" => Some(Settings {
            witness: false,
            mode: Mode::Global,
            buckets,
            fast_training: false,
        }),
        "root-full" | "root-fast" => Some(Settings {
            witness: false,
            mode: Mode::RootCandidates,
            buckets,
            fast_training: arm.ends_with("fast"),
        }),
        "inline-full" | "inline-fast" => Some(Settings {
            witness: false,
            mode: Mode::Inline,
            buckets,
            fast_training: arm.ends_with("fast"),
        }),
        "catalog-full" | "catalog-fast" => Some(Settings {
            witness: false,
            mode: Mode::Catalog,
            buckets,
            fast_training: arm.ends_with("fast"),
        }),
        "mixed-full" | "mixed-fast" => Some(Settings {
            witness: false,
            mode: Mode::Mixed,
            buckets,
            fast_training: arm.ends_with("fast"),
        }),
        _ => panic!("declared model-placement arm"),
    };
    super::super::super::encode_lossy(pixels, info.row_bytes(), info, &Metadata::default(), &cfg)
        .unwrap()
}

fn configuration_for(m: &Value) -> LossyConfig {
    let c = &m["config"];
    let color = match c["color"].as_str().unwrap() {
        "xyb" => LossyColor::Xyb,
        "ycocg" => LossyColor::YCoCg,
        _ => panic!("declared coding color"),
    };
    let entropy = match c["entropy"].as_str().unwrap() {
        "ans" => LossyEntropy::RunLevel,
        "huffman" => LossyEntropy::SplitHuf,
        _ => panic!("declared coder"),
    };
    let mut cfg = configuration(
        c["quality"].as_f64().unwrap() as f32,
        color,
        entropy,
        c["group_rows"].as_u64().unwrap() as usize,
    );
    cfg.effort = c["effort"].as_f64().unwrap() as f32;
    cfg.seed_bank = c["seed_bank"].as_u64().unwrap();
    cfg.aq_gamma = c["aq_gamma"].as_f64().unwrap() as f32;
    cfg.compact_streams = c["compact_states"].as_bool().unwrap();
    cfg.checksum = c["checksum"].as_bool().unwrap();
    cfg.unit_ordinals = c["unit_ordinals"].as_bool().unwrap();
    cfg.column_groups = c["column_groups"].as_u64().map(|n| n as u32);
    cfg.exact_blocks = Some(c["exact_ratio"].as_f64().unwrap() as f32);
    cfg.small_transforms = c["small_transforms"].as_bool().unwrap();
    cfg.adaptive_partitions = c["adaptive_partitions"].as_bool().unwrap();
    cfg.decode_speed = match c["tier"].as_str().unwrap() {
        "explicit" => None,
        "compact" => Some(crate::DecodeSpeed::Compact),
        "balanced" => Some(crate::DecodeSpeed::Balanced),
        "fast" => Some(crate::DecodeSpeed::Fast),
        "fastest" => Some(crate::DecodeSpeed::Fastest),
        _ => panic!("declared tier"),
    };
    cfg.lab = super::super::super::LossyLabSwitches::defaults();
    cfg
}

fn coefficients(file: &[u8]) -> String {
    let p = super::super::super::parse_lossy(file).unwrap();
    let mut hash = crate::Hasher::new(crate::HashAlgorithm::Xxh3_128);
    for (u, bytes) in p.stripes.iter().enumerate() {
        let g = p.group(u % p.ngroups()).1;
        let (q, _, _, _) = super::super::super::decode_stripe_raw_bytes(
            &p,
            &g,
            u / p.ngroups(),
            bytes,
            super::super::super::Group::ALL,
            false,
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
        let (a, b) = g.rows(u / p.ngroups());
        coefficient_digest(&mut hash, &q[..(b - a) * g.bw * 3 * 64]);
    }
    format!(
        "{:032x}",
        u128::from_le_bytes(hash.finish().as_bytes().try_into().unwrap())
    )
}

fn selected_roi(file: &[u8], info: &ImageInfo, reference: &[u8], threads: usize) {
    let x = info.width / 2;
    let y = info.height / 2;
    let rect = crate::planes::Rect::new(x, y, 1, 1);
    let mut pixel = vec![0; info.bytes_per_pixel()];
    crate::region::RegionReader::open(crate::planes::SliceReader::new(file))
        .unwrap()
        .decode_region_threads(rect, &mut pixel, info.bytes_per_pixel(), threads)
        .unwrap();
    let at = y as usize * info.row_bytes() + x as usize * info.bytes_per_pixel();
    assert_eq!(pixel, reference[at..at + pixel.len()]);
}

fn withheld_roi(file: &[u8], info: &ImageInfo, reference: &[u8]) {
    let p = super::super::super::parse_lossy(file).unwrap();
    let x = info.width as usize / 2;
    let y = info.height as usize / 2;
    let rows = p.g.rows(0).1;
    let column = x.checked_div(p.gcols).unwrap_or(0);
    let selected = (y / 8 / rows) * p.ngroups() + column;
    let mut withheld = file.to_vec();
    for (u, bytes) in p.stripes.iter().enumerate() {
        if u != selected {
            let at = bytes.as_ptr() as usize - file.as_ptr() as usize;
            withheld[at..at + bytes.len()].fill(0);
        }
    }
    selected_roi(&withheld, info, reference, 1);
}

fn model_stats(file: &[u8]) -> Value {
    let p = super::super::super::parse_lossy(file).unwrap();
    let mut counts = [0usize; 3];
    let mut inline_bytes = 0;
    if p.catalog_lab.is_some() {
        for (u, bytes) in p.stripes.iter().enumerate() {
            let g = p.group(u % p.ngroups()).1;
            let (_, body) = p.record(&g, u / p.ngroups(), bytes).unwrap();
            let control = ByteReader::new(body).varint().unwrap();
            let kind = if control & 2 != 0 {
                inline_bytes += (control >> 2) as usize;
                2
            } else if control >> 2 == 0 {
                0
            } else {
                1
            };
            counts[kind] += 1;
        }
    } else {
        counts[0] = p.stripes.len();
    }
    json!({"global_units":counts[0],"catalog_units":counts[1],"inline_units":counts[2],"inline_model_bytes":inline_bytes,"resident_catalog_models":p.catalog_lab.as_ref().map_or(0,|c|c.models.len()),"coder":format!("{:?}",p.tables.mode),"seed_bank":super::super::super::lab_entropy_seed(file).unwrap().1.bank,"seed_scope":super::super::super::lab_entropy_seed(file).unwrap().1.scope,"aq":p.aq,"exact":p.exact.is_some(),"map":format!("{:?}",p.map),"groups":p.ngroups(),"units":p.stripes.len()})
}

#[test]
fn native_model_comparison() {
    let info = ImageInfo::new(17, 33, Channels::Rgb, SampleType::U8);
    let pixels: Vec<_> = (0..17 * 33 * 3).map(|i| (i * 41 + i / 19) as u8).collect();
    let cfg = configuration(40.0, LossyColor::Xyb, LossyEntropy::RunLevel, 8);
    let legacy = encode(&info, &pixels, &cfg, "legacy", 2);
    let mixed = encode(&info, &pixels, &cfg, "mixed-fast", 2);
    assert_eq!(
        crate::decode(&legacy).unwrap(),
        crate::decode(&mixed).unwrap()
    );
    for arm in ["root-full", "root-fast"] {
        let root = encode(&info, &pixels, &cfg, arm, 2);
        assert!(
            super::super::super::parse_lossy(&root)
                .unwrap()
                .catalog_lab
                .is_none()
        );
        assert_eq!(
            crate::decode(&root).unwrap(),
            crate::decode(&legacy).unwrap()
        );
        assert_eq!(coefficients(&root), coefficients(&legacy));
    }
    if let Some(path) = std::env::var_os("IMI_NATIVE_MODEL_MANIFEST") {
        measure(Path::new(&path));
    }
}

fn measure(path: &Path) {
    let manifest_bytes = std::fs::read(path).unwrap();
    let m: Value = serde_json::from_slice(&manifest_bytes).unwrap();
    let source = &m["source"];
    let root = Path::new(m["output"].as_str().unwrap());
    std::fs::create_dir(root).unwrap();
    let pixels = Arc::new(std::fs::read(source["path"].as_str().unwrap()).unwrap());
    assert_eq!(digest(&pixels), source["xxh3_128"].as_str().unwrap());
    let channels = if source["channels"] == 4 {
        Channels::Rgba
    } else {
        Channels::Rgb
    };
    let mut info = ImageInfo::new(
        source["width"].as_u64().unwrap() as u32,
        source["height"].as_u64().unwrap() as u32,
        channels,
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
    assert_eq!(pixels.len(), info.row_bytes() * info.height as usize);
    let cfg = configuration_for(&m);
    let buckets = m["buckets"].as_u64().unwrap() as usize;
    if let Some(profile) = m.get("profile") {
        let mut cfg = cfg.clone();
        cfg.threads = profile["threads"].as_u64().unwrap() as usize;
        let arm = profile["arm"].as_str().unwrap();
        let f = encode(&info, &pixels, &cfg, arm, buckets);
        for _ in 0..profile["iterations"].as_u64().unwrap() {
            if profile["operation"] == "encode" {
                std::hint::black_box(encode(&info, &pixels, &cfg, arm, buckets));
            } else {
                std::hint::black_box(decode_native(&f, &info, cfg.threads));
            }
        }
        return;
    }
    let arms: Vec<&'static str> = m.get("arms").map_or_else(
        || ARMS.to_vec(),
        |v| {
            v.as_array()
                .unwrap()
                .iter()
                .map(|v| {
                    *ARMS
                        .iter()
                        .find(|&&arm| arm == v.as_str().unwrap())
                        .expect("declared arm")
                })
                .collect()
        },
    );
    assert_eq!(arms.first(), Some(&"legacy"));
    assert_eq!(arms.last(), Some(&"aa"));
    assert!(arms.iter().all(|a| ARMS.contains(a)));
    assert_eq!(
        arms.iter().collect::<std::collections::BTreeSet<_>>().len(),
        arms.len()
    );
    let mut files = Vec::new();
    let mut stats = Vec::new();
    let mut stored = 0u64;
    for arm in &arms[..arms.len() - 1] {
        let f = encode(&info, &pixels, &cfg, arm, buckets);
        stored += f.len() as u64;
        assert!(stored <= m["storage_budget_bytes"].as_u64().unwrap());
        let s = json!({"arm":arm,"bytes":f.len(),"xxh3_128":digest(&f),"ac_coefficients":coefficients(&f),"models":model_stats(&f)});
        std::fs::write(root.join(format!("{arm}.imi")), &f).unwrap();
        stats.push(s);
        files.push(Arc::new(f));
    }
    let reference = decode_native(&files[0], &info, 1);
    let coefficients = coefficients(&files[0]);
    for (i, file) in files.iter().enumerate() {
        assert_eq!(stats[i]["ac_coefficients"], coefficients);
        for threads in [1, 2, 4, 8] {
            assert_eq!(decode_native(file, &info, threads), reference);
            selected_roi(file, &info, &reference, threads);
        }
        // Encoded integrity intentionally remains enabled on the ordinary
        // control above. Withholding uses a separately encoded file without
        // checksums, so it never bypasses integrity verification on corrupt data.
        if cfg.checksum || cfg.unit_ordinals {
            let mut c = cfg.clone();
            c.checksum = false;
            c.unit_ordinals = false;
            let f = encode(&info, &pixels, &c, arms[i], buckets);
            withheld_roi(&f, &info, &reference);
        } else {
            withheld_roi(file, &info, &reference);
        }
    }
    if let Some(frozen) = m.get("frozen_legacy") {
        let f = std::fs::read(frozen.as_str().unwrap()).unwrap();
        assert_eq!(*files[0], f, "frozen native global bytes");
    }
    let correctness = json!({"source_revision":m["source_revision"],"binary_xxh3_128":digest(&std::fs::read(std::env::current_exe().unwrap()).unwrap()),"manifest_xxh3_128":digest(&manifest_bytes),"source":source,"config":m["config"],"buckets":buckets,"arms":stats,"pixels_xxh3_128":digest(&reference),"maps":false,"nas_writes":0,"timed_scope":"complete native encoding; parse+allocate+full pixel decode","withheld_scope":"all unrelated AC payloads zeroed; separate unchecksummed control when needed"});
    std::fs::write(
        root.join("correctness.json"),
        serde_json::to_vec_pretty(&correctness).unwrap(),
    )
    .unwrap();
    // The ordinary row API and padded output retain the same native samples.
    for file in &files {
        let mut row = vec![0; info.row_bytes()];
        let mut y_next = 0;
        crate::decode_resources::DecodeSession::new(
            crate::decode_resources::DecodePolicy::default(),
        )
        .decode_rows(file, &mut row, |_, y, bytes| {
            assert_eq!(y, y_next);
            let at = y as usize * info.row_bytes();
            assert_eq!(bytes, &reference[at..at + info.row_bytes()]);
            y_next += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(y_next, info.height);
        let stride = info.row_bytes() + 17;
        let mut padded = vec![0xa7; stride * info.height as usize];
        crate::decode_into_threads(file, &mut padded, stride, 4).unwrap();
        for y in 0..info.height as usize {
            assert_eq!(
                &padded[y * stride..y * stride + info.row_bytes()],
                &reference[y * info.row_bytes()..(y + 1) * info.row_bytes()]
            );
            assert!(
                padded[y * stride + info.row_bytes()..(y + 1) * stride]
                    .iter()
                    .all(|&v| v == 0xa7)
            );
        }
    }
    drop(reference);
    if m["timing"] == false {
        std::fs::write(
            root.join("COMPLETE"),
            b"size and native correctness only; no timing claim\n",
        )
        .unwrap();
        return;
    }
    let gate = zenbench::GateConfig::strict()
        .max_heavy_processes(4)
        .min_available_ram_mb(4096)
        .max_cpu_temp_c(Some(85.0))
        .max_wait(Duration::from_secs(30));
    let result = zenbench::run_gated(gate, |suite| {
        suite.plan().workload_id = Some(format!(
            "native-models-{}-{}",
            digest(&pixels),
            digest(&serde_json::to_vec(&m["config"]).unwrap())
        ));
        for operation in ["encode", "decode"] {
            for threads in [1, 2, 4, 8] {
                suite.group(format!("{operation}-{threads}t"), |g| {
                    g.config()
                        .min_rounds(24)
                        .max_rounds(24)
                        .auto_rounds(false)
                        .warmup_time(Duration::from_millis(200))
                        .max_wall_time(Duration::from_secs(600));
                    g.throughput(zenbench::Throughput::Elements(
                        u64::from(info.width) * u64::from(info.height),
                    ));
                    for (i, &arm) in arms.iter().enumerate() {
                        let pixels = pixels.clone();
                        let info = info.clone();
                        let mut cfg = cfg.clone();
                        cfg.threads = threads;
                        let file = files[if arm == "aa" { 0 } else { i }].clone();
                        g.bench(arm, move |b| {
                            if operation == "encode" {
                                b.iter(|| encode(&info, &pixels, &cfg, arm, buckets))
                            } else {
                                b.iter(|| decode_native(&file, &info, threads))
                            }
                        });
                    }
                });
            }
        }
    });
    result.save(root.join("timing.json")).unwrap();
    let t: Value =
        serde_json::from_slice(&std::fs::read(root.join("timing.json")).unwrap()).unwrap();
    assert_eq!(t["incomplete"], false);
    if m["cpu_policy"] != "unrestricted" {
        assert_eq!(t["unreliable"], false);
    } else {
        assert_eq!(
            std::env::var("ZENBENCH_CPU_POLICY").unwrap(),
            "unrestricted"
        );
        // The complete result retains UNVERIFIED_UNRESTRICTED placement.
        // These Mac observations cannot promote a no-regression baseline.
    }
    assert_eq!(t["comparisons"].as_array().unwrap().len(), 8);
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
    let marker = if m["cpu_policy"] == "unrestricted" {
        "EXPLORATORY_COMPLETE"
    } else {
        "COMPLETE"
    };
    std::fs::write(root.join(marker),b"native AC coefficients/pixels/full+ROI/withheld model access and 24 paired rounds; see timing.json for placement qualification\n").unwrap();
}

/// RSS driver loads only the retained compressed file and decoder output.
/// Encoder/source preparation is deliberately outside the measured process.
#[test]
fn native_model_decode_memory() {
    if let Some(path) = std::env::var_os("IMI_NATIVE_MODEL_MEMORY_MANIFEST") {
        let m: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let file = std::fs::read(m["file"].as_str().unwrap()).unwrap();
        assert_eq!(digest(&file), m["file_xxh3_128"].as_str().unwrap());
        let info = crate::header(&file).unwrap().info;
        let threads = m["threads"].as_u64().unwrap() as usize;
        for _ in 0..m["iterations"].as_u64().unwrap() {
            let output = decode_native(&file, &info, threads);
            assert_eq!(digest(&output), m["pixels_xxh3_128"].as_str().unwrap());
            std::hint::black_box(output);
        }
    } else {
        let info = ImageInfo::new(17, 33, Channels::Rgb, SampleType::U8);
        let pixels: Vec<u8> = (0..info.row_bytes() * info.height as usize)
            .map(|i| (i * 37) as u8)
            .collect();
        let cfg = configuration(40.0, LossyColor::Xyb, LossyEntropy::RunLevel, 8);
        let file = encode(&info, &pixels, &cfg, "mixed-full", 2);
        assert_eq!(decode_native(&file, &info, 1).len(), pixels.len());
    }
}
