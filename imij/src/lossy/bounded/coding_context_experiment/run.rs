//! Mechanical pixel/coefficient oracles and explicit, caller-requested timing.
use super::*;
use crate::planes::{ReadAt, Rect};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

pub(super) fn small_regression() {
    for bits in [8, 16] {
        let info = ImageInfo::new(
            35,
            33,
            Channels::Rgb,
            if bits == 8 {
                SampleType::U8
            } else {
                SampleType::U16
            },
        );
        let mut source = Vec::new();
        for y in 0..33 {
            for x in 0..35 * 3 {
                let value = if y < 8 {
                    0
                } else {
                    ((x * 353 + y * 7907 + x * y * 11) & ((1 << bits) - 1)) as u16
                };
                if bits == 8 {
                    source.push(value as u8);
                } else {
                    source.extend_from_slice(&value.to_ne_bytes());
                }
            }
        }
        for color in [LossyColor::Xyb, LossyColor::YCoCg] {
            for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                for rows in [8, 32] {
                    let cfg = configuration(40.0, color, entropy, rows);
                    let a = encode_native(&info, &source, &cfg, Preparation::OnePass);
                    let b = encode_native(&info, &source, &cfg, Preparation::StudyFirstBandDc);
                    let (c, c_stats) = encode_contexts(&info, &source, &cfg, rows, &a);
                    let reference = decode_native(&a, &info, 1);
                    // A's check-tail reservation cannot be silently accepted
                    // after removing its ordinary trailing index.
                    let parsed_header =
                        crate::parse::header::read_header(&mut ByteReader::new(&a)).unwrap();
                    let (_, _, a_body, a_features) = crate::v1::lossy_body(&a).unwrap().unwrap();
                    let mut bad_output = vec![0; info.row_bytes() * info.height as usize];
                    assert!(
                        decode_group(
                            info.clone(),
                            &parsed_header,
                            a_features,
                            a_body,
                            &mut bad_output,
                            info.row_bytes()
                        )
                        .is_err()
                    );
                    assert_eq!(decode_native(&b, &info, 2), reference);
                    let candidate = ContextFile::parse(&c).unwrap();
                    assert_eq!(candidate.decode(1).unwrap(), reference);
                    assert_eq!(candidate.decode(2).unwrap(), reference);
                    let a_stats = native_stats(&a);
                    assert_eq!(native_stats(&b).coefficients, a_stats.coefficients);
                    assert_eq!(c_stats.coefficients, a_stats.coefficients);
                    let at = info.height as usize / 2 * info.row_bytes()
                        + info.width as usize / 2 * info.bytes_per_pixel();
                    assert_eq!(
                        candidate.center_pixel().unwrap().0,
                        reference[at..at + info.bytes_per_pixel()]
                    );
                    // No earlier/later numeric payload is needed for the selected
                    // group. Keep directory/record heads, withhold all other DATA.
                    let selected = info.height as usize / 2 / rows;
                    let mut isolated = TrackedSource {
                        bytes: &c,
                        ranges: Vec::new(),
                        forbidden: candidate
                            .groups
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| *i != selected)
                            .map(|(_, g)| {
                                let start = g.body.as_ptr() as usize - c.as_ptr() as usize;
                                start..start + g.body.len()
                            })
                            .collect(),
                    };
                    assert_eq!(
                        super::indexed::center_pixel(&mut isolated).unwrap(),
                        reference[at..at + info.bytes_per_pixel()]
                    );
                    let mut withheld = c.clone();
                    for (ordinal, group) in candidate.groups.iter().enumerate() {
                        if ordinal != selected {
                            let start = group.body.as_ptr() as usize - c.as_ptr() as usize;
                            withheld[start..start + group.body.len()].fill(0);
                        }
                    }
                    let withheld = ContextFile::parse(&withheld).unwrap();
                    assert_eq!(
                        withheld.center_pixel().unwrap().0,
                        reference[at..at + info.bytes_per_pixel()]
                    );
                    assert!(withheld.decode(1).is_err());
                    let mut broken = c.clone();
                    broken[candidate.index_start] ^= 1;
                    assert!(ContextFile::parse(&broken).is_err());
                    assert!(ContextFile::parse(&c[..c.len() - 1]).is_err());
                    let mut broken = c.clone();
                    broken[candidate.header_bytes] = 1;
                    assert!(ContextFile::parse(&broken).is_err());
                }
            }
        }
    }
}

struct TrackedSource<'a> {
    bytes: &'a [u8],
    ranges: Vec<(usize, usize)>,
    forbidden: Vec<core::ops::Range<usize>>,
}
impl ReadAt for TrackedSource<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.bytes.len() as u64)
    }
    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
        let start = usize::try_from(offset).map_err(|_| Error::TooLarge)?;
        let end = start.checked_add(out.len()).ok_or(Error::TooLarge)?;
        if self
            .forbidden
            .iter()
            .any(|range| start < range.end && end > range.start)
        {
            return Err(Error::InvalidInput(
                "study read touched a withheld numeric payload",
            ));
        }
        out.copy_from_slice(
            self.bytes
                .get(start..end)
                .ok_or(Error::Corrupt("study tracked source"))?,
        );
        self.ranges.push((start, end));
        Ok(())
    }
}
fn native_roi(data: &[u8], info: &ImageInfo, reference: &[u8]) -> Value {
    let source = TrackedSource {
        bytes: data,
        ranges: Vec::new(),
        forbidden: Vec::new(),
    };
    let mut reader = crate::region::RegionReader::open(source).unwrap();
    let rect = Rect::new(info.width / 2, info.height / 2, 1, 1);
    let mut pixel = vec![0; info.bytes_per_pixel()];
    reader
        .decode_region(rect, &mut pixel, info.bytes_per_pixel())
        .unwrap();
    let at = rect.y() as usize * info.row_bytes() + rect.x() as usize * info.bytes_per_pixel();
    assert_eq!(pixel, reference[at..at + pixel.len()]);
    let source = reader.into_inner();
    let total: usize = source.ranges.iter().map(|(a, b)| b - a).sum();
    let mut ranges = source.ranges.clone();
    ranges.sort_unstable();
    let (mut union, mut end) = (0, 0);
    for (a, b) in ranges {
        union += b.saturating_sub(a.max(end));
        end = end.max(b);
    }
    json!({"requested_bytes":total,"unique_bytes":union,"read_calls":source.ranges.len(),"pixel_xxh3_128":digest(&pixel),"verification":"fresh native ReadAt reader; checksums off for all arms"})
}

fn statistics(stats: &CodingStats) -> Value {
    json!({"ac_payload_bytes":stats.ac_bytes,"dc_payload_bytes":stats.dc_bytes,"numeric_head_and_record_framing_bytes":stats.numeric_other_bytes,"outer_header_framing_index_bytes":stats.outer_bytes,"coefficient_xxh3_128":stats.coefficients,"dc_u16_contexts":stats.dc_u16_groups,"dc_i32_contexts":stats.dc_i32_groups})
}
fn save(path: &Path, bytes: &[u8], used: &mut u64, budget: u64) {
    use std::io::Write;
    *used = used.checked_add(bytes.len() as u64).unwrap();
    assert!(
        *used <= budget,
        "declared encoded/result storage budget exhausted"
    );
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}
fn progress(root: &Path, stage: &str, cell: &str, completed: usize, required: usize) {
    let value = json!({"stage":stage,"cell":cell,"completed":completed,"required":required,"pid":std::process::id(),"observed_unix_seconds":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()});
    std::fs::write(
        root.join("PROGRESS.pending.json"),
        serde_json::to_vec_pretty(&value).unwrap(),
    )
    .unwrap();
    std::fs::rename(
        root.join("PROGRESS.pending.json"),
        root.join("PROGRESS.json"),
    )
    .unwrap();
    println!("{value}");
}

pub(super) fn measure(manifest_path: &Path) {
    let native_byte_order = u16::from_ne_bytes([1, 0]);
    assert_eq!(native_byte_order, 1, "frozen raw sources are little endian");
    let manifest_bytes = std::fs::read(manifest_path).unwrap();
    let manifest: Value = serde_json::from_slice(&manifest_bytes).unwrap();
    let root = Path::new(manifest["output"].as_str().unwrap());
    assert!(!root.exists());
    std::fs::create_dir(root).unwrap();
    let budget = manifest["storage_budget_bytes"].as_u64().unwrap();
    let mut used = 0;
    let binary = std::env::current_exe().unwrap();
    let identity = json!({"source_revision":manifest["source_revision"],"test_binary":binary,"binary_xxh3_128":digest(&std::fs::read(&binary).unwrap()),"manifest_xxh3_128":digest(&manifest_bytes),"maps":false,"new_nas_copies":0,"endpoint":"fresh allocated complete-frame decode, parsing included; no IO/CMS/network"});
    save(
        &root.join("IDENTITY.json"),
        &serde_json::to_vec_pretty(&identity).unwrap(),
        &mut used,
        budget,
    );
    let sources = manifest["sources"].as_array().unwrap();
    let qualities = manifest["qualities"].as_array().unwrap();
    let rows = manifest["group_rows"].as_array().unwrap();
    let required = sources.len() * qualities.len() * rows.len() * 2;
    let mut completed = 0;
    let mut results = Vec::new();
    for source in sources {
        let bytes = std::fs::read(source["path"].as_str().unwrap()).unwrap();
        assert_eq!(digest(&bytes), source["xxh3_128"].as_str().unwrap());
        let bits = source["bits"].as_u64().unwrap();
        let mut info = ImageInfo::new(
            source["width"].as_u64().unwrap() as u32,
            source["height"].as_u64().unwrap() as u32,
            Channels::Rgb,
            if bits == 8 {
                SampleType::U8
            } else {
                SampleType::U16
            },
        );
        let cicp = &source["cicp"];
        info.cicp = crate::Cicp {
            color_primaries: cicp[0].as_u64().unwrap() as u8,
            transfer_characteristics: cicp[1].as_u64().unwrap() as u8,
            matrix_coefficients: cicp[2].as_u64().unwrap() as u8,
            full_range: cicp[3].as_bool().unwrap(),
        };
        assert_eq!(bytes.len(), info.row_bytes() * info.height as usize);
        let color = match source["color"].as_str().unwrap() {
            "xyb" => LossyColor::Xyb,
            "ycocg" => LossyColor::YCoCg,
            _ => panic!("explicit supported color"),
        };
        for quality in qualities {
            for group_rows in rows {
                for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                    let q = quality.as_f64().unwrap() as f32;
                    let height = group_rows.as_u64().unwrap() as usize;
                    let case = format!(
                        "{}-q{q}-g{height}-{}",
                        source["name"].as_str().unwrap(),
                        if entropy == LossyEntropy::RunLevel {
                            "ans"
                        } else {
                            "huf"
                        }
                    );
                    let out = root.join(&case);
                    std::fs::create_dir(&out).unwrap();
                    let cfg = configuration(q, color, entropy, height);
                    progress(root, "encode-a", &case, completed, required);
                    let a = Arc::new(encode_native(&info, &bytes, &cfg, Preparation::OnePass));
                    progress(root, "encode-b", &case, completed, required);
                    let b = Arc::new(encode_native(
                        &info,
                        &bytes,
                        &cfg,
                        Preparation::StudyFirstBandDc,
                    ));
                    progress(root, "encode-c", &case, completed, required);
                    let (c, c_stats) = encode_contexts(&info, &bytes, &cfg, height, &a);
                    let c = Arc::new(c);
                    let a_stats = native_stats(&a);
                    let b_stats = native_stats(&b);
                    assert_eq!(a_stats.coefficients, b_stats.coefficients);
                    assert_eq!(a_stats.coefficients, c_stats.coefficients);
                    let reference = decode_native(&a, &info, 1);
                    assert_eq!(decode_native(&b, &info, 1), reference);
                    let candidate = ContextFile::parse(&c).unwrap();
                    assert_eq!(candidate.decode(1).unwrap(), reference);
                    for threads in [2, 4, 8] {
                        assert_eq!(decode_native(&a, &info, threads), reference);
                        assert_eq!(decode_native(&b, &info, threads), reference);
                        assert_eq!(candidate.decode(threads).unwrap(), reference);
                    }
                    let roi_a = native_roi(&a, &info, &reference);
                    let roi_b = native_roi(&b, &info, &reference);
                    let mut roi_source = TrackedSource {
                        bytes: &c,
                        ranges: Vec::new(),
                        forbidden: Vec::new(),
                    };
                    let roi = super::indexed::center_pixel(&mut roi_source).unwrap();
                    let required_bytes: usize = roi_source.ranges.iter().map(|(a, b)| b - a).sum();
                    assert_eq!(digest(&roi), roi_a["pixel_xxh3_128"].as_str().unwrap());
                    let correct = json!({"cell":case,"source":source,"quality":q,"group_rows":height,"entropy":format!("{entropy:?}"),"a_bytes":a.len(),"b_bytes":b.len(),"c_bytes":c.len(),"a_xxh3_128":digest(&a),"b_xxh3_128":digest(&b),"c_xxh3_128":digest(&c),"pixels_xxh3_128":digest(&reference),"a_coding":statistics(&a_stats),"b_coding":statistics(&b_stats),"c_coding":statistics(&c_stats),"a_roi":roi_a,"b_roi":roi_b,"c_roi":{"requested_bytes":required_bytes,"read_calls":roi_source.ranges.len(),"pixel_xxh3_128":digest(&roi),"scope":"actual fresh ReadAt directory and selected context; no previous payload"},"checksums":"off in every arm","c_scope":"complete existing numeric payload per local context; includes repeated quantization fields; no whole child files or child indexes"});
                    for (name, data) in [
                        ("a.imi", a.as_slice()),
                        ("b.imi", b.as_slice()),
                        ("c.ctx-study", c.as_slice()),
                    ] {
                        save(&out.join(name), data, &mut used, budget);
                    }
                    save(
                        &out.join("correctness.json"),
                        &serde_json::to_vec_pretty(&correct).unwrap(),
                        &mut used,
                        budget,
                    );
                    drop(candidate);
                    drop(reference);
                    progress(root, "decode-benchmark", &case, completed, required);
                    let gate = zenbench::GateConfig::strict()
                        .max_heavy_processes(4)
                        .min_available_ram_mb(4096)
                        .max_cpu_temp_c(Some(85.0))
                        .max_wait(Duration::from_secs(30));
                    let measured = zenbench::run_gated(gate, |suite| {
                        suite.plan().workload_id =
                            Some(format!("coding-context-{}-{}", digest(&bytes), case));
                        for threads in [1, 2, 4, 8] {
                            suite.group(format!("decode-{threads}t"), |group| {
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
                                for (label, data, local) in [
                                    ("a", a.clone(), false),
                                    ("b", b.clone(), false),
                                    ("c", c.clone(), true),
                                    ("aa", a.clone(), false),
                                ] {
                                    let info = info.clone();
                                    group.bench(label, move |bench| {
                                        bench.iter(|| {
                                            if local {
                                                ContextFile::parse(&data)
                                                    .unwrap()
                                                    .decode(threads)
                                                    .unwrap()
                                            } else {
                                                decode_native(&data, &info, threads)
                                            }
                                        })
                                    });
                                }
                            });
                        }
                    });
                    measured.save(out.join("decode.json")).unwrap();
                    let timing: Value =
                        serde_json::from_slice(&std::fs::read(out.join("decode.json")).unwrap())
                            .unwrap();
                    assert_eq!(timing["incomplete"], false);
                    assert_eq!(timing["unreliable"], false);
                    for group in timing["comparisons"].as_array().unwrap() {
                        assert_eq!(group["completed_rounds"], 24);
                        assert!(
                            group["samples"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .all(|sample| sample["gate_clean"] == true)
                        );
                    }
                    results.push(correct);
                    completed += 1;
                    save(
                        &out.join("COMPLETE"),
                        b"exact coefficient/pixel and 24-clean-round decode gates pass\n",
                        &mut used,
                        budget,
                    );
                    progress(root, "cell-complete", &case, completed, required);
                }
            }
        }
    }
    save(&root.join("SUMMARY.json"),&serde_json::to_vec_pretty(&json!({"identity":identity,"completed":completed,"required":required,"cells":results,"stored_bytes_before_summary":used})).unwrap(),&mut used,budget);
    progress(root, "complete", "all", completed, required);
}
