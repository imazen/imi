//! Equal-area geometry study using retained files, real region reads and zenbench.
//! Usage: portrait_decode_bench MANIFEST OUTPUT access|timing
//! Timing excludes file I/O and output allocation. Cold ROI includes reader open;
//! warm ROI retains its reader. Access receipts use an independently counted source.
#![forbid(unsafe_code)]

use how_far::{Outcome, Unstoppable};
use how_far_along::{Phase, PulseTree, Status, Total};
use imij::planes::{ReadAt, Rect, SliceReader};
use imij::region::{ReadStats, RegionReader};
use serde_json::{Value, json};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[path = "portrait_decode_bench/large.rs"]
mod large;

fn hash(bytes: &[u8]) -> String {
    format!("{:032x}", xxhash_rust::xxh3::xxh3_128(bytes))
}

fn save(path: &Path, value: &Value) {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap();
    serde_json::to_writer_pretty(&mut f, value).unwrap();
    writeln!(f).unwrap();
}

#[derive(Clone)]
struct Input {
    case: Value,
    file: Arc<Vec<u8>>,
    info: imij::ImageInfo,
    pixels: Arc<Vec<u8>>,
}

#[derive(Clone, Default)]
struct Ledger {
    batches: Vec<Vec<(u64, usize)>>,
}

struct Counted {
    bytes: Arc<Vec<u8>>,
    ledger: Arc<Mutex<Ledger>>,
}

impl ReadAt for Counted {
    fn len(&mut self) -> Result<u64, imij::Error> {
        Ok(self.bytes.len() as u64)
    }
    fn read_exact_at(&mut self, at: u64, out: &mut [u8]) -> Result<(), imij::Error> {
        self.read_many_at(&mut [(at, out)])
    }
    fn read_many_at(&mut self, requests: &mut [(u64, &mut [u8])]) -> Result<(), imij::Error> {
        let mut batch = Vec::new();
        for (at, out) in requests.iter_mut() {
            let a = usize::try_from(*at).map_err(|_| imij::Error::TooLarge)?;
            let end = a.checked_add(out.len()).ok_or(imij::Error::TooLarge)?;
            out.copy_from_slice(self.bytes.get(a..end).ok_or(imij::Error::Read)?);
            batch.push((*at, out.len()));
        }
        self.ledger.lock().unwrap().batches.push(batch);
        Ok(())
    }
}

fn stats(value: ReadStats, ledger: &Ledger) -> Value {
    assert_eq!(
        value.bytes,
        ledger
            .batches
            .iter()
            .flatten()
            .map(|(_, n)| *n as u64)
            .sum::<u64>()
    );
    assert_eq!(
        value.reads,
        ledger.batches.iter().map(|b| b.len() as u64).sum::<u64>()
    );
    assert_eq!(value.batches, ledger.batches.len() as u64);
    json!({"bytes":value.bytes,"ranges":value.reads,"dependent_batches":value.batches,
           "requested_ranges_by_batch":ledger.batches})
}

fn crop(input: &Input, rect: Rect) -> Vec<u8> {
    let stride = rect.width() as usize * input.info.bytes_per_pixel();
    (0..rect.height() as usize)
        .flat_map(|y| {
            let a = (rect.y() as usize + y) * input.info.row_bytes()
                + rect.x() as usize * input.info.bytes_per_pixel();
            input.pixels[a..a + stride].iter().copied()
        })
        .collect()
}

fn rect(value: &Value) -> Rect {
    let v = value["rect"].as_array().unwrap();
    Rect::new(
        v[0].as_u64().unwrap() as u32,
        v[1].as_u64().unwrap() as u32,
        v[2].as_u64().unwrap() as u32,
        v[3].as_u64().unwrap() as u32,
    )
}

fn access(input: &Input, workload: &Value, threads: usize) -> Value {
    let rect = rect(workload);
    let stride = rect.width() as usize * input.info.bytes_per_pixel();
    let expected = crop(input, rect);
    let mut output = vec![0; expected.len()];
    let ledger = Arc::new(Mutex::new(Ledger::default()));
    let mut limits = imij::Limits::DEFAULT;
    if let Some(explicit) = input.case.get("limits") {
        limits.max_pixels = explicit["max_pixels"].as_u64().unwrap();
        limits.max_bytes = explicit["max_bytes"].as_u64().unwrap();
    }
    let mut reader = RegionReader::open_with_limits(
        Counted {
            bytes: input.file.clone(),
            ledger: ledger.clone(),
        },
        &limits,
    )
    .unwrap();
    assert!(reader.has_unit_table());
    let open = stats(reader.read_stats(), &ledger.lock().unwrap());
    reader.reset_read_stats();
    *ledger.lock().unwrap() = Ledger::default();
    let pulse = PulseTree::new(Phase::new("region", Total::Unknown), Unstoppable);
    let observer = pulse.observer();
    reader
        .decode_region_observed(rect, &mut output, stride, threads, &pulse)
        .unwrap();
    pulse.finish(Outcome::Succeeded).unwrap();
    assert_eq!(output, expected);
    let first = stats(reader.read_stats(), &ledger.lock().unwrap());
    let snapshot = observer.snapshot();
    let mut units = Vec::new();
    let mut reconstructed = 0u64;
    let gw = input.case["group_width"].as_u64().unwrap();
    let gh = input.case["group_height"].as_u64().unwrap();
    for u in &snapshot.children {
        if !u.name.starts_with("stripe ") {
            continue;
        }
        assert_eq!(u.status, Status::Finished(Outcome::Succeeded));
        let words: Vec<_> = u.name.split_whitespace().collect();
        let s: u64 = words[1].parse().unwrap();
        let g: u64 = words[3].parse().unwrap();
        let width = gw.min(u64::from(input.info.width) - g * gw);
        let height = gh.min(u64::from(input.info.height) - s * gh);
        reconstructed += width * height;
        units.push(json!({"name":u.name,"stripe":s,"group":g,"width":width,"height":height}));
    }
    assert!(!units.is_empty());
    reader.reset_read_stats();
    *ledger.lock().unwrap() = Ledger::default();
    reader
        .decode_region_threads(rect, &mut output, stride, threads)
        .unwrap();
    assert_eq!(output, expected);
    let warm = stats(reader.read_stats(), &ledger.lock().unwrap());
    json!({"case":input.case,"workload":workload,"threads":threads,"file_bytes":input.file.len(),
           "open":open,"first":first,"warm":warm,"units":units,
           "reconstructed_pixels":reconstructed,"returned_pixels":u64::from(rect.width())*u64::from(rect.height()),
           "pixels_xxh3_128":hash(&output),"scope":"Actual ReadAt ranges, no simulated RTT. Reconstructed pixels sum observed completed AC units with clipped geometry; excludes extra DC prediction work."})
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.len() == 2 && args[1] == "identity" {
        println!(
            "{}",
            hash(&std::fs::read(std::env::current_exe().unwrap()).unwrap())
        );
        return;
    }
    assert_eq!(args.len(), 4, "MANIFEST OUTPUT access|timing");
    if ["encode-large", "probe-large", "access-large"].contains(&args[3].as_str()) {
        large::run(Path::new(&args[1]), Path::new(&args[2]), &args[3]);
        return;
    }
    let raw = std::fs::read(&args[1]).unwrap();
    let manifest: Value = serde_json::from_slice(&raw).unwrap();
    let root = Path::new(&args[2]);
    std::fs::create_dir(root).unwrap();
    let mut inputs = Vec::new();
    for case in manifest["cases"].as_array().unwrap() {
        let file = std::fs::read(case["file"].as_str().unwrap()).unwrap();
        assert_eq!(hash(&file), case["file_xxh3_128"].as_str().unwrap());
        let (info, pixels) = imij::decode(&file).unwrap();
        assert_eq!((info.width, info.height), (3072, 3072));
        assert_eq!(info.bytes_per_pixel(), 3);
        assert_eq!(hash(&pixels), case["pixels_xxh3_128"].as_str().unwrap());
        inputs.push(Input {
            case: case.clone(),
            file: Arc::new(file),
            info,
            pixels: Arc::new(pixels),
        });
    }
    let mut receipts = Vec::new();
    for input in &inputs {
        for threads in [1, 2, 4, 8] {
            let mut whole = vec![0; input.pixels.len()];
            imij::decode_into_threads(&input.file, &mut whole, input.info.row_bytes(), threads)
                .unwrap();
            assert_eq!(whole.as_slice(), input.pixels.as_slice());
            for workload in manifest["workloads"].as_array().unwrap() {
                receipts.push(access(input, workload, threads));
            }
        }
    }
    save(
        &root.join("access.json"),
        &json!({"manifest_xxh3_128":hash(&raw),
         "binary_xxh3_128":hash(&std::fs::read(std::env::current_exe().unwrap()).unwrap()),"receipts":receipts}),
    );
    if args[3] == "access" {
        return;
    }
    assert_eq!(args[3], "timing");
    let gate = zenbench::GateConfig::strict()
        .min_available_ram_mb(4096)
        .max_cpu_temp_c(Some(85.0))
        .max_wait(Duration::from_secs(30));
    let mut registered = 0;
    let result = zenbench::run_gated(gate, |suite| {
        suite.plan().workload_id = Some(format!("portrait-access-{}", hash(&raw)));
        for quality in [20, 90] {
            for coder in ["ans", "huffman"] {
                for set in [0, 1] {
                    let baseline = if set == 0 {
                        "square"
                    } else {
                        "near-square-taller"
                    };
                    let labels = if set == 0 {
                        [baseline, "phone", "wide", baseline]
                    } else {
                        [baseline, "phone-taller", "wide-taller", baseline]
                    };
                    for threads in [1, 2, 4, 8] {
                        let mut operations = vec![("whole".to_string(), None, false)];
                        for w in manifest["workloads"].as_array().unwrap() {
                            operations.push((
                                format!("cold-{}", w["name"].as_str().unwrap()),
                                Some(rect(w)),
                                false,
                            ));
                            if w["warm_timing"] == true {
                                operations.push((
                                    format!("warm-{}", w["name"].as_str().unwrap()),
                                    Some(rect(w)),
                                    true,
                                ));
                            }
                        }
                        for (operation, roi, warm) in operations {
                            let name =
                                format!("q{quality}-{coder}-set{set}-{operation}-{threads}t");
                            std::fs::write(root.join("registration.json"), json!({"stage":"registration","current_group":name,"registered":registered,"required":320,"pid":std::process::id()}).to_string()).unwrap();
                            suite.group(name, |g| {
                                g.config()
                                    .min_rounds(24)
                                    .max_rounds(24)
                                    .auto_rounds(false)
                                    .warmup_time(Duration::from_millis(200))
                                    .max_wall_time(Duration::from_secs(600));
                                let pixels = roi.map_or(3072 * 3072, |r| {
                                    u64::from(r.width()) * u64::from(r.height())
                                });
                                g.throughput(zenbench::Throughput::Elements(pixels));
                                for (index, label) in labels.into_iter().enumerate() {
                                    let input = inputs
                                        .iter()
                                        .find(|i| {
                                            i.case["label"] == label
                                                && i.case["quality"] == quality
                                                && i.case["coder"] == coder
                                        })
                                        .unwrap()
                                        .clone();
                                    let arm = if index == 3 { "aa" } else { label };
                                    g.bench(arm, move |b| {
                                        let stride = roi.map_or(input.info.row_bytes(), |r| {
                                            r.width() as usize * 3
                                        });
                                        let size = roi.map_or(input.pixels.len(), |r| {
                                            stride * r.height() as usize
                                        });
                                        let mut out = vec![0; size];
                                        if let Some(r) = roi {
                                            if warm {
                                                let mut reader = RegionReader::open(
                                                    SliceReader::new(&input.file),
                                                )
                                                .unwrap();
                                                reader
                                                    .decode_region_threads(
                                                        r, &mut out, stride, threads,
                                                    )
                                                    .unwrap();
                                                b.iter(|| {
                                                    reader
                                                        .decode_region_threads(
                                                            r, &mut out, stride, threads,
                                                        )
                                                        .unwrap();
                                                    std::hint::black_box(&out);
                                                });
                                            } else {
                                                b.iter(|| {
                                                    RegionReader::open(SliceReader::new(
                                                        &input.file,
                                                    ))
                                                    .unwrap()
                                                    .decode_region_threads(
                                                        r, &mut out, stride, threads,
                                                    )
                                                    .unwrap();
                                                    std::hint::black_box(&out);
                                                });
                                            }
                                        } else {
                                            b.iter(|| {
                                                imij::decode_into_threads(
                                                    &input.file,
                                                    &mut out,
                                                    stride,
                                                    threads,
                                                )
                                                .unwrap();
                                                std::hint::black_box(&out);
                                            });
                                        }
                                    });
                                }
                            });
                            registered += 1;
                        }
                    }
                }
            }
        }
    });
    result.save(root.join("timing.json")).unwrap();
    std::fs::write(
        root.join("progress.json"),
        json!({"stage":"finished","registered":registered,"returned_groups":result.comparisons.len(),"incomplete":result.incomplete,"required":320,"pid":std::process::id()})
            .to_string(),
    )
    .unwrap();
    zenbench::postprocess_result(&result);
}
