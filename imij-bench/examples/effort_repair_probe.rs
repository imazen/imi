//! Fixed mask-policy lossless experiment. Root alone runs `time`; no default change.
#![forbid(unsafe_code)]
use imij::{Channels, ImageInfo, ImiEncoderConfig, Metadata, SampleType};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{error::Error, fs, path::Path, sync::Arc, time::Duration};
type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    version: u32,
    image: String,
    source: String,
    source_sha256: String,
    width: u32,
    height: u32,
}

#[derive(Clone)]
struct Policy {
    name: &'static str,
    config: ImiEncoderConfig,
}

fn policies() -> Vec<Policy> {
    let mut values: Vec<Policy> = [0., 2.5, 5., 7.5]
        .into_iter()
        .zip(["current-e0", "current-e2p5", "current-e5", "current-e7p5"])
        .map(|(effort, name)| Policy {
            name,
            config: imij::ImiEncoderConfig::default().with_effort(effort),
        })
        .collect();
    for (name, pred, decor) in [
        ("fixed-w-d1", 0x01, 0x02),
        ("fixed-w-med-d1", 0x09, 0x02),
        ("fixed-w-grad-d1", 0x11, 0x02),
        ("fixed-w-med-grad-d01", 0x19, 0x03),
    ] {
        // Effort is ten so public masks remain the exact effective set. This
        // emulates a future candidate policy; it is not literal effort 2.5.
        let mut config = imij::ImiEncoderConfig::default().with_effort(10.);
        config = config.tune_lossless(|t| {
            t.predictor_mask = pred;
        });
        config = config.tune_lossless(|t| {
            t.decor_mask = decor;
        });
        values.push(Policy { name, config });
    }
    values
}

fn description(policy: &Policy) -> serde_json::Value {
    let c = &policy.config;
    json!({"name":policy.name,"effort":c.effort,"predictor_mask":c.predictor_mask,
        "decor_mask":c.decor_mask,"threads":c.threads,"stripe_rows":c.stripe_rows,
        "lanes":c.lanes,"k_low":c.k_low,"k_high":c.k_high,"m":c.m,
        "run_min":c.run_min,"max_ctx":c.max_ctx,"ctx_cross":c.ctx_cross,
        "palette":c.palette,"column_groups":c.column_groups,"decoder_memory_limit":null,
        "effective_masks_explicit":policy.name.starts_with("fixed-"),"metadata":"empty"})
}

fn encode(pixels: &[u8], info: &ImageInfo, c: &ImiEncoderConfig) -> Result<Vec<u8>> {
    Ok(c.encode_samples(
        imij::ImageRef::new(pixels, info.row_bytes(), info),
        &Metadata::default(),
    )?)
}

fn verify(pixels: &[u8], info: &ImageInfo, policy: &Policy) -> Result<Vec<u8>> {
    let encoded = encode(pixels, info, &policy.config)?;
    let (actual_info, decoded) = imij::decode(&encoded)?;
    if actual_info != *info || decoded != pixels {
        return Err("lossless samples/header differ".into());
    }
    if encode(pixels, info, &policy.config)? != encoded {
        return Err("repeated encoded bytes differ".into());
    }
    let mut parallel = policy.config.clone();
    parallel = parallel.with_threads(4);
    if encode(pixels, info, &parallel)? != encoded {
        return Err("four-thread encoded bytes differ".into());
    }
    Ok(encoded)
}

struct Checked<'a> {
    pixels: Vec<u8>,
    expected: &'a [u8],
    info: Option<ImageInfo>,
    expected_info: &'a ImageInfo,
}
impl Drop for Checked<'_> {
    fn drop(&mut self) {
        assert_eq!(self.pixels, self.expected, "timed output differs");
        if let Some(i) = &self.info {
            assert_eq!(i, self.expected_info, "timed header differs");
        }
    }
}

#[derive(Clone)]
struct Case {
    policy: Policy,
    encoded: Arc<Vec<u8>>,
}

fn save(path: impl AsRef<Path>, value: &impl Serialize) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

fn synthetic(output: &Path) -> Result<()> {
    fs::create_dir(output)?;
    let mut cases = 0;
    for (width, height) in [(1, 1), (1, 17), (17, 1), (9, 11), (33, 65), (63, 129)] {
        let info = ImageInfo::new(width, height, Channels::Rgb, SampleType::U8);
        for pattern in 0..4u32 {
            let pixels: Vec<u8> = (0..width * height * 3)
                .map(|v| match pattern {
                    0 => 0,
                    1 => 255,
                    2 => ((v * 31 + v / 7) % 256) as u8,
                    _ => {
                        let mut x = v.wrapping_add(9_182_001);
                        x ^= x << 13;
                        x ^= x >> 17;
                        x ^= x << 5;
                        x as u8
                    }
                })
                .collect();
            for policy in policies() {
                verify(&pixels, &info, &policy)?;
                cases += 1;
            }
        }
    }
    save(
        output.join("synthetic.json"),
        &json!({"cases":cases,"policies":8,
        "patterns":4,"geometries":6,"exact_source_decode":true,"repeat_bytes":true,
        "four_thread_bytes":true,"native_timing":false}),
    )
}

fn run(mode: &str, spec_path: &Path, output: &Path) -> Result<()> {
    if !matches!(mode, "verify" | "time") {
        return Err("mode must be verify or time".into());
    }
    if fs::metadata(spec_path)?.len() > 16_384 {
        return Err("spec exceeds 16 KiB".into());
    }
    let spec: Spec = serde_json::from_slice(&fs::read(spec_path)?)?;
    let n = u64::from(spec.width) * u64::from(spec.height);
    if spec.version != 1
        || n == 0
        || n > 4_194_304
        || spec.image.is_empty()
        || spec.image.len() > 96
        || !spec
            .image
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        || spec.source_sha256.len() != 64
        || !spec
            .source_sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err("invalid version/geometry/name/source hash declaration".into());
    }
    let source = fs::canonicalize(spec_path.parent().ok_or("spec parent")?.join(&spec.source))?;
    if fs::metadata(&source)?.len() != n * 3 {
        return Err("source must be tight full-width RGB8".into());
    }
    let pixels = Arc::new(fs::read(source)?);
    if pixels.len() as u64 != n * 3 {
        return Err("source changed while reading".into());
    }
    fs::create_dir(output)?;
    let info = ImageInfo::new(spec.width, spec.height, Channels::Rgb, SampleType::U8);
    let mut cases = Vec::new();
    let mut rows = Vec::new();
    for policy in policies() {
        let encoded = Arc::new(verify(&pixels, &info, &policy)?);
        let filename = format!("{}.imi", policy.name);
        fs::write(output.join(&filename), encoded.as_slice())?;
        rows.push(
            json!({"name":policy.name,"config":description(&policy),"encoded":filename,
            "encoded_bytes":encoded.len(),"decoded_bytes":pixels.len(),"status":"complete",
            "exact_source_decode":true,"repeat_bytes":true,"four_thread_bytes":true}),
        );
        cases.push(Case { policy, encoded });
    }
    save(
        output.join("admission.json"),
        &json!({"schema":"effort-repair-admission-v1",
        "mode":mode,"spec":spec,"cases":rows,"rounds":12,"threads":1,
        "wall_timer":"allocating complete codec API; checks and output drop outside wall interval",
        "cpu_caveat":"zenbench input teardown included; use wall",
        "sha_contract":"external controller verifies spec/source/binary/output hashes; harness asserts no SHA computation",
        "source_policy":"exact retained RGB8; no transforms or decoded copies","production_change":false}),
    )?;
    if mode == "verify" {
        return Ok(());
    }
    // Authorization and host/controller admission are external. This command
    // is implemented for the native controller; never invoked by local run.py.
    let result = zenbench::run(|suite| {
        for encoding in [true, false] {
            suite.group(
                if encoding {
                    "lossless-encode-alloc"
                } else {
                    "lossless-decode-alloc"
                },
                |g| {
                    let cfg = g.config();
                    cfg.min_rounds = 12;
                    cfg.max_rounds = 12;
                    cfg.auto_rounds = false;
                    cfg.max_iterations = 16;
                    cfg.baseline_only = Some(false);
                    cfg.max_time = Duration::from_secs(600);
                    cfg.warmup_time = Duration::from_millis(100);
                    g.throughput(zenbench::Throughput::Bytes(pixels.len() as u64));
                    for case in &cases {
                        let case = case.clone();
                        let pixels = pixels.clone();
                        let info = info.clone();
                        g.bench_tagged(case.policy.name, &[("threads", "1")], move |b| {
                            b.with_input(|| ()).run(|()| {
                                if encoding {
                                    Checked {
                                        pixels: encode(&pixels, &info, &case.policy.config)
                                            .expect("encode failed"),
                                        expected: &case.encoded,
                                        info: None,
                                        expected_info: &info,
                                    }
                                } else {
                                    let (actual_info, decoded) =
                                        imij::decode(&case.encoded).expect("decode failed");
                                    Checked {
                                        pixels: decoded,
                                        expected: &pixels,
                                        info: Some(actual_info),
                                        expected_info: &info,
                                    }
                                }
                            });
                        });
                    }
                },
            );
        }
    });
    result.save(output.join("timing.json"))?;
    if result.unreliable
        || result.comparisons.len() != 2
        || result.comparisons.iter().any(|g| {
            g.completed_rounds != 12
                || g.samples.len() != 12
                || g.benchmarks.len() != 8
                || g.benchmarks.iter().any(|b| b.summary.n != 12)
                || g.samples.iter().any(|s| {
                    let mut order = s.execution_order.clone();
                    order.sort_unstable();
                    s.iterations == 0
                        || s.elapsed_ns.len() != 8
                        || order != (0..8).collect::<Vec<_>>()
                })
        })
    {
        return Err("unreliable/incomplete native timing retained for audit".into());
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [mode, output] if mode == "synthetic" => synthetic(Path::new(output)),
        [mode, spec, output] => run(mode, Path::new(spec), Path::new(output)),
        _ => Err("usage: synthetic NEW_DIR | verify|time SPEC NEW_DIR".into()),
    }
}
