//! Native 16K raster: bounded preparation and finite per-cohort region studies.
use super::*;
use imij::stream_encode::{BoundedLossyConfig, LossyUnitIndex, encode_lossy_bounded};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Read};

fn limits() -> imij::Limits {
    let mut limits = imij::Limits::DEFAULT;
    limits.max_pixels = 16384 * 16384;
    limits.max_bytes = 2 << 30;
    limits
}

pub(super) fn run(manifest: &Path, root: &Path, mode: &str) {
    let raw = std::fs::read(manifest).unwrap();
    let m: Value = serde_json::from_slice(&raw).unwrap();
    std::fs::create_dir(root).unwrap();
    if mode == "encode-large" {
        encode(&m, root);
    } else {
        assert!(["probe-large", "access-large"].contains(&mode));
        probe(&m, &raw, root, mode == "probe-large");
    }
}

fn encode(m: &Value, root: &Path) {
    let source = Path::new(m["source"]["path"].as_str().unwrap());
    let mut source_file = File::open(source).unwrap();
    let mut hasher = imij::Hasher::new(imij::HashAlgorithm::Xxh3_128);
    let mut buffer = vec![0; 1 << 20];
    loop {
        let n = source_file.read(&mut buffer).unwrap();
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    let source_hash = u128::from_le_bytes(hasher.finish().as_bytes().try_into().unwrap());
    assert_eq!(
        format!("{source_hash:032x}"),
        m["source"]["xxh3_128"].as_str().unwrap()
    );
    assert_eq!(source_file.metadata().unwrap().len(), 16384 * 16384 * 3);
    let info = imij::ImageInfo::new(16384, 16384, imij::Channels::Rgb, imij::SampleType::U8);
    let mut expected = BTreeMap::new();
    let mut cases = Vec::new();
    for (i, case) in m["cases"].as_array().unwrap().iter().enumerate() {
        std::fs::write(root.join("progress.json"), json!({"stage":"encode","completed":i,"required":28,"case":case["name"],"pid":std::process::id()}).to_string()).unwrap();
        let cfg = imij::ImiEncoderConfig::lossy(case["quality"].as_u64().unwrap() as f32)
            .with_effort(0.0)
            .with_threads(1)
            .with_checksum(false)
            .with_unit_ordinals(true)
            .with_column_groups(Some(case["group_width"].as_u64().unwrap() as u32))
            .tune_lossy(|t| {
                t.color = imij::LossyColor::Xyb;
                t.entropy = if case["coder"] == "ans" {
                    imij::LossyEntropy::RunLevel
                } else {
                    imij::LossyEntropy::SplitHuf
                };
                t.stripe_block_rows = case["group_height"].as_u64().unwrap() as u32 / 8;
                t.compact_streams = false;
                t.aq_gamma = 0.0;
                t.small_transforms = false;
                t.adaptive_partitions = false;
                t.exact_blocks = Some(0.0);
            });
        let path = root.join(format!("{}.imi", case["name"].as_str().unwrap()));
        let scratch = root.join(format!("{}.index-scratch", case["name"].as_str().unwrap()));
        let writer = OpenOptions::new()
            .write(true)
            .read(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let index = OpenOptions::new()
            .write(true)
            .read(true)
            .create_new(true)
            .open(&scratch)
            .unwrap();
        let mut raster = File::open(source).unwrap();
        let pulse = PulseTree::new(Phase::new("encode", Total::Unknown), Unstoppable);
        let (mut writer, _, _) = encode_lossy_bounded(
            &mut raster,
            BufWriter::new(writer),
            index,
            None,
            info.clone(),
            &imij::Metadata::default(),
            &cfg,
            BoundedLossyConfig::new(
                info.row_bytes() as u64,
                LossyUnitIndex::Reserved(4),
                64 << 20,
            ),
            &pulse,
        )
        .unwrap();
        writer.flush().unwrap();
        pulse.finish(Outcome::Succeeded).unwrap();
        drop(writer);
        let file = std::fs::read(&path).unwrap();
        let (decoded_info, pixels) = imij::decode_with_limits(&file, &limits()).unwrap();
        assert_eq!(decoded_info, info);
        let pixel_hash = hash(&pixels);
        let quality = case["quality"].as_u64().unwrap();
        assert_eq!(
            expected
                .entry(quality)
                .or_insert_with(|| pixel_hash.clone()),
            &pixel_hash
        );
        let mut item = case.clone();
        item["file"] = path.to_str().unwrap().into();
        item["file_bytes"] = file.len().into();
        item["file_xxh3_128"] = hash(&file).into();
        item["pixels_xxh3_128"] = pixel_hash.into();
        save(
            &root.join(format!("{}.json", case["name"].as_str().unwrap())),
            &item,
        );
        cases.push(item);
    }
    save(
        &root.join("cases.json"),
        &json!({"cases":cases,"pixels_by_quality":expected}),
    );
    std::fs::write(
        root.join("progress.json"),
        json!({"stage":"complete","completed":28,"required":28,"pid":std::process::id()})
            .to_string(),
    )
    .unwrap();
}

fn load_cases(m: &Value) -> Vec<Input> {
    let mut inputs = Vec::new();
    let mut reference: Option<Arc<Vec<u8>>> = None;
    for case in m["cases"].as_array().unwrap() {
        let file = std::fs::read(case["file"].as_str().unwrap()).unwrap();
        assert_eq!(hash(&file), case["file_xxh3_128"].as_str().unwrap());
        let (info, pixels) = imij::decode_with_limits(&file, &limits()).unwrap();
        assert_eq!(
            (info.width, info.height, info.bytes_per_pixel()),
            (16384, 16384, 3)
        );
        assert_eq!(hash(&pixels), case["pixels_xxh3_128"].as_str().unwrap());
        let pixels = match &reference {
            Some(r) => {
                assert_eq!(pixels.as_slice(), r.as_slice());
                r.clone()
            }
            None => {
                let r = Arc::new(pixels);
                reference = Some(r.clone());
                r
            }
        };
        inputs.push(Input {
            case: case.clone(),
            file: Arc::new(file),
            info,
            pixels,
        });
    }
    assert_eq!(inputs.len(), 4);
    inputs
}

fn probe(m: &Value, raw: &[u8], root: &Path, timing: bool) {
    let inputs = load_cases(m);
    let mut receipts = Vec::new();
    for input in &inputs {
        for threads in m["access_threads"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as usize)
        {
            for workload in m["workloads"].as_array().unwrap() {
                receipts.push(access(input, workload, threads));
            }
        }
    }
    save(
        &root.join("access.json"),
        &json!({"manifest_xxh3_128":hash(raw),
         "binary_xxh3_128":hash(&std::fs::read(std::env::current_exe().unwrap()).unwrap()),"receipts":receipts}),
    );
    if !timing {
        save(
            &root.join("COMPLETE.json"),
            &json!({"groups":0,"timing":false,"access_receipts":receipts.len()}),
        );
        return;
    }
    let timed: Vec<Rect> = m["timed_phase_indices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            let name = format!("phase-{:02}", i.as_u64().unwrap());
            rect(
                m["workloads"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|w| w["name"] == name)
                    .unwrap(),
            )
        })
        .collect();
    assert_eq!(
        timed.len(),
        m["timed_viewports_per_call"].as_u64().unwrap() as usize
    );
    assert!(!timed.is_empty());
    let gate = zenbench::GateConfig::strict()
        .min_available_ram_mb(4096)
        .max_cpu_temp_c(Some(85.0))
        .max_wait(Duration::from_secs(30));
    let result = zenbench::run_gated(gate, |suite| {
        suite.plan().workload_id = Some(format!("portrait-16k-{}", hash(raw)));
        for threads in [1, 2, 4, 8] {
            let mut operations: Vec<(&str, Vec<Rect>, bool)> =
                ["middle", "reference-viewport", "phone-viewport"]
                    .into_iter()
                    .map(|name| {
                        let w = m["workloads"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|w| w["name"] == name)
                            .unwrap();
                        (name, vec![rect(w)], false)
                    })
                    .collect();
            operations.push(("phases-cold", timed.clone(), false));
            operations.push(("phases-warm", timed.clone(), true));
            for (operation, rects, warm) in operations {
                suite.group(
                    format!("{}-{operation}-{threads}t", m["cohort"].as_str().unwrap()),
                    |g| {
                        g.config()
                            .min_rounds(24)
                            .max_rounds(24)
                            .auto_rounds(false)
                            .warmup_time(Duration::from_millis(200))
                            .max_wall_time(Duration::from_secs(600));
                        g.throughput(zenbench::Throughput::Elements(
                            rects
                                .iter()
                                .map(|r| u64::from(r.width()) * u64::from(r.height()))
                                .sum(),
                        ));
                        for arm in 0..5 {
                            let input = inputs[if arm == 4 { 0 } else { arm }].clone();
                            let label = if arm == 4 {
                                "aa".to_string()
                            } else {
                                input.case["label"].as_str().unwrap().to_string()
                            };
                            let rects = rects.clone();
                            g.bench(label, move |b| {
                                let mut outputs: Vec<Vec<u8>> = rects
                                    .iter()
                                    .map(|r| vec![0; r.width() as usize * r.height() as usize * 3])
                                    .collect();
                                if warm {
                                    let mut reader = RegionReader::open_with_limits(
                                        SliceReader::new(&input.file),
                                        &limits(),
                                    )
                                    .unwrap();
                                    for (r, out) in rects.iter().zip(&mut outputs) {
                                        reader
                                            .decode_region_threads(
                                                *r,
                                                out,
                                                r.width() as usize * 3,
                                                threads,
                                            )
                                            .unwrap();
                                    }
                                    b.iter(|| {
                                        for (r, out) in rects.iter().zip(&mut outputs) {
                                            reader
                                                .decode_region_threads(
                                                    *r,
                                                    out,
                                                    r.width() as usize * 3,
                                                    threads,
                                                )
                                                .unwrap();
                                        }
                                        std::hint::black_box(&outputs);
                                    });
                                } else {
                                    b.iter(|| {
                                        for (r, out) in rects.iter().zip(&mut outputs) {
                                            RegionReader::open_with_limits(
                                                SliceReader::new(&input.file),
                                                &limits(),
                                            )
                                            .unwrap()
                                            .decode_region_threads(
                                                *r,
                                                out,
                                                r.width() as usize * 3,
                                                threads,
                                            )
                                            .unwrap();
                                        }
                                        std::hint::black_box(&outputs);
                                    });
                                }
                            });
                        }
                    },
                );
            }
        }
    });
    result.save(root.join("timing.json")).unwrap();
    zenbench::postprocess_result(&result);
    assert!(!result.incomplete);
    assert_eq!(result.comparisons.len(), 20);
    assert!(
        result
            .comparisons
            .iter()
            .all(|g| g.completed_rounds == 24 && g.samples.len() == 24)
    );
    save(
        &root.join("COMPLETE.json"),
        &json!({"groups":20,"rounds":24,"resource_qualification":"See all flags in timing.json; completion is not speed qualification."}),
    );
}
