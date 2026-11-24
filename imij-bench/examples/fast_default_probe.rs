//! Pixel-preserving default search; root controls native execution.
//! `verify|time SPEC.json NEW_DIRECTORY`; no implicit grid or source conversion.
#![forbid(unsafe_code)]
use imij::{
    ChannelSampling, Channels, DecodeSpeed, ImageInfo, ImiEncoderConfig, LossyChannelLayout,
    LossyColor, LossyEntropy, LossyTransform, Metadata, SampleType,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
type Result<T> = std::result::Result<T, Box<dyn Error>>;
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Runlevel,
    Huffman,
    Balanced,
    Fast,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    name: String,
    effort: f32,
    mode: Mode,
    compact_streams: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Source {
    path: PathBuf,
    width: u32,
    height: u32,
    lossless_encoded: PathBuf,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Reference {
    quality: f32,
    legacy_encoded: PathBuf,
    compact_encoded: PathBuf,
    decoded: PathBuf,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    version: u32,
    source: Source,
    references: Vec<Reference>,
    policies: Vec<Policy>,
    lossless_efforts: Vec<f32>,
}
#[derive(Clone)]
enum Config {
    Lossy(ImiEncoderConfig, Policy),
    Lossless(ImiEncoderConfig),
}
impl Config {
    fn encode(&self, pixels: &[u8], info: &ImageInfo) -> Vec<u8> {
        match self {
            Self::Lossy(c, _) => c.encode_samples(
                imij::ImageRef::new(pixels, info.row_bytes(), info),
                &Metadata::default(),
            ),
            Self::Lossless(c) => c.encode_samples(
                imij::ImageRef::new(pixels, info.row_bytes(), info),
                &Metadata::default(),
            ),
        }
        .expect("admitted encode failed")
    }
    fn description(&self) -> serde_json::Value {
        match self {
            Self::Lossy(c, p) => json!({"coding":"lossy","policy":p,"quality":c.quality,
                "color":"xyb","channel_layouts":["full8","full8","full8"],
                "compact_streams":c.compact_streams,"effort":c.effort,"threads":c.threads,
                "stripe_block_rows":c.stripe_block_rows,"max_ac_tables":c.max_ac_tables,
                "round":c.round,"matrix":c.matrix,"channel_mul":c.channel_mul,
                "aq_gamma":c.aq_gamma,"aq_r0":c.aq_r0,"entropy_configured":format!("{:?}",c.entropy),
                "decode_speed":c.decode_speed.map(|v|format!("{v:?}")),
                "column_groups":c.column_groups,"decoder_memory_limit":null,"rdoq":0}),
            Self::Lossless(c) => json!({"coding":"lossless","effort":c.effort,"threads":c.threads,
                "stripe_rows":c.stripe_rows,"lanes":c.lanes,"k_low":c.k_low,"k_high":c.k_high,
                "m":c.m,"run_min":c.run_min,"max_ctx":c.max_ctx,"ctx_cross":c.ctx_cross,
                "decor_mask":c.decor_mask,"predictor_mask":c.predictor_mask,"palette":c.palette,
                "column_groups":c.column_groups,"decoder_memory_limit":null}),
        }
    }
}
fn lossy(policy: &Policy, quality: f32) -> Config {
    let mut c = imij::ImiEncoderConfig::lossy(quality)
        .tune_lossy(|t| t.color = LossyColor::Xyb)
        .with_threads(1)
        .with_effort(policy.effort)
        .tune_lossy(|t| t.compact_streams = policy.compact_streams)
        .tune_lossy(|t| t.stripe_block_rows = 1)
        .tune_lossy(|t| t.entropy = LossyEntropy::RunLevel)
        .tune_lossy(|t| {
            t.channel_layouts =
                [LossyChannelLayout::new(LossyTransform::Dct8, ChannelSampling::Full); 3]
        });
    c = c.tune_lossy(|t| {
        t.round = 0.4;
    });
    c = c.tune_lossy(|t| {
        t.max_ac_tables = 16;
    });
    c = c.tune_lossy(|t| {
        t.channel_mul = None;
    });
    c = c.tune_lossy(|t| {
        t.matrix = 255;
    });
    c = c.tune_lossy(|t| {
        t.aq_gamma = 0.;
    });
    c = c.tune_lossy(|t| {
        t.aq_r0 = 1.;
    });
    match policy.mode {
        Mode::Runlevel => {}
        Mode::Huffman => c = c.tune_lossy(|t| t.entropy = LossyEntropy::SplitHuf),
        Mode::Balanced => c = c.with_decode_speed(DecodeSpeed::Balanced),
        Mode::Fast => c = c.with_decode_speed(DecodeSpeed::Fast),
    }
    Config::Lossy(c, policy.clone())
}
fn effort(v: f32) -> bool {
    v.is_finite() && (0.0..=10.0).contains(&v)
}
fn validate(spec: &Spec) -> Result<usize> {
    let n = u64::from(spec.source.width) * u64::from(spec.source.height);
    if spec.version != 1
        || n == 0
        || n > 4_194_304
        || spec.references.is_empty()
        || spec.references.len() > 8
        || spec.policies.is_empty()
        || spec.policies.len() > 16
        || spec.lossless_efforts.is_empty()
        || spec.lossless_efforts.len() > 8
    {
        return Err("spec count/geometry/version limit".into());
    }
    let first = &spec.policies[0];
    if first.effort != 10.
        || first.mode != Mode::Runlevel
        || first.compact_streams
        || spec.lossless_efforts[0] != 10.
    {
        return Err(
            "first policies must be current effort10 runlevel legacy and lossless10".into(),
        );
    }
    for (i, p) in spec.policies.iter().enumerate() {
        if !effort(p.effort)
            || p.name.is_empty()
            || p.name.len() > 64
            || !p
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || spec.policies[..i].iter().any(|q| {
                q.name == p.name
                    || (q.effort == p.effort
                        && q.mode == p.mode
                        && q.compact_streams == p.compact_streams)
            })
        {
            return Err("policy effort/name/duplicate configuration".into());
        }
    }
    for (i, r) in spec.references.iter().enumerate() {
        if !r.quality.is_finite()
            || !(1.0..=116.).contains(&r.quality)
            || spec.references[..i].iter().any(|p| p.quality == r.quality)
        {
            return Err("reference quality invalid/duplicate; q0 alias excluded".into());
        }
    }
    for (i, &v) in spec.lossless_efforts.iter().enumerate() {
        if !effort(v) || spec.lossless_efforts[..i].contains(&v) {
            return Err("lossless effort invalid/duplicate".into());
        }
    }
    Ok(usize::try_from(n * 3)?)
}
fn read(path: &Path, max: usize, exact: bool) -> Result<Vec<u8>> {
    let n = usize::try_from(fs::metadata(path)?.len())?;
    if n > max || (exact && n != max) {
        return Err("file length outside declared bound".into());
    }
    let value = fs::read(path)?;
    if value.len() != n {
        return Err("file changed during read".into());
    }
    Ok(value)
}
fn resolve(base: &Path, p: &mut PathBuf) -> Result<()> {
    *p = fs::canonicalize(base.join(&*p))?;
    Ok(())
}
fn save(path: impl AsRef<Path>, value: &impl Serialize) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}
struct Case {
    name: String,
    config: Config,
    encoded: Vec<u8>,
    decoded: Arc<Vec<u8>>,
}
struct Group {
    name: String,
    cases: Vec<Arc<Case>>,
}
struct Checked<'a> {
    bytes: Vec<u8>,
    expected: &'a [u8],
    info: Option<ImageInfo>,
    expected_info: &'a ImageInfo,
}
impl Drop for Checked<'_> {
    fn drop(&mut self) {
        assert_eq!(self.bytes.as_slice(), self.expected, "timed output changed");
        if let Some(info) = &self.info {
            assert_eq!(info, self.expected_info);
        }
    }
}
fn main() -> Result<()> {
    if cfg!(feature = "lab") {
        return Err("build --no-default-features; lab timing prohibited".into());
    }
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["self-test"] {
        return small_cases();
    }
    if args.len() != 3 || !matches!(args[0].as_str(), "verify" | "time") {
        return Err("usage: fast_default_probe verify|time SPEC.json NEWDIR".into());
    }
    let path = fs::canonicalize(&args[1])?;
    let base = path.parent().ok_or("spec has no parent")?;
    let mut spec: Spec = serde_json::from_slice(&read(&path, 64 << 10, false)?)?;
    let length = validate(&spec)?;
    let out = Path::new(&args[2]);
    fs::create_dir(out)?;
    resolve(base, &mut spec.source.path)?;
    let pixels = Arc::new(read(&spec.source.path, length, true)?);
    let info = ImageInfo::new(
        spec.source.width,
        spec.source.height,
        Channels::Rgb,
        SampleType::U8,
    );
    let mut groups = Vec::new();
    let mut cells = Vec::new();
    for (index, r) in spec.references.iter_mut().enumerate() {
        println!("reference {index}: q{}", r.quality);
        resolve(base, &mut r.legacy_encoded)?;
        resolve(base, &mut r.compact_encoded)?;
        resolve(base, &mut r.decoded)?;
        let legacy = read(&r.legacy_encoded, 128 << 20, false)?;
        let compact = read(&r.compact_encoded, 128 << 20, false)?;
        let decoded = Arc::new(read(&r.decoded, length, true)?);
        for original in [&legacy, &compact] {
            let (actual_info, raw) = imij::decode(original)?;
            if actual_info != info || raw.as_slice() != decoded.as_slice() {
                return Err("frozen baseline decode differs".into());
            }
        }
        let name = format!("q{index:02}");
        let mut cases = Vec::new();
        for policy in &spec.policies {
            let config = lossy(policy, r.quality);
            let encoded = config.encode(&pixels, &info);
            if policy.mode == Mode::Runlevel
                && policy.effort == 10.
                && encoded
                    != *(if policy.compact_streams {
                        &compact
                    } else {
                        &legacy
                    })
            {
                return Err("effort10 baseline encoded bytes differ from frozen reference".into());
            }
            let (actual_info, raw) = imij::decode(&encoded)?;
            if actual_info != info || raw.as_slice() != decoded.as_slice() {
                return Err("policy changed decoded pixels".into());
            }
            if config.encode(&pixels, &info) != encoded {
                return Err("repeated encode differs".into());
            }
            let case_name = format!("{name}-{}", policy.name);
            let file = format!("{case_name}.imi");
            fs::write(out.join(&file), &encoded)?;
            cells.push(json!({"name":case_name,"group":name,"config":config.description(),"encoded":file,
                "encoded_bytes":encoded.len(),"decoded_reference":r.decoded,"decoded_bytes":decoded.len()}));
            cases.push(Arc::new(Case {
                name: case_name,
                config,
                encoded,
                decoded: decoded.clone(),
            }));
        }
        groups.push(Group { name, cases });
    }
    resolve(base, &mut spec.source.lossless_encoded)?;
    let lossless_reference = read(&spec.source.lossless_encoded, 128 << 20, false)?;
    let mut cases = Vec::new();
    for &value in &spec.lossless_efforts {
        let mut c = imij::ImiEncoderConfig::default().with_effort(value);
        c = c.with_threads(1);
        let config = Config::Lossless(c);
        let encoded = config.encode(&pixels, &info);
        if value == 10. && encoded != lossless_reference {
            return Err("lossless e10 bytes differ from frozen baseline".into());
        }
        let (actual_info, raw) = imij::decode(&encoded)?;
        if actual_info != info || raw.as_slice() != pixels.as_slice() {
            return Err("lossless differs from source".into());
        }
        if config.encode(&pixels, &info) != encoded {
            return Err("lossless repeated encode differs".into());
        }
        let name = format!("lossless-e{value}");
        let file = format!("{name}.imi");
        fs::write(out.join(&file), &encoded)?;
        cells.push(json!({"name":name,"group":"lossless","config":config.description(),"encoded":file,
            "encoded_bytes":encoded.len(),"decoded_reference":spec.source.path,"decoded_bytes":pixels.len()}));
        cases.push(Arc::new(Case {
            name,
            config,
            encoded,
            decoded: pixels.clone(),
        }));
    }
    groups.push(Group {
        name: "lossless".into(),
        cases,
    });
    save(
        out.join("admission.json"),
        &json!({"mode":args[0],"spec":spec,"cases":cells,
        "lab":false,"threads":1,"rounds":20,"max_iterations":16,
        "wall_timer":"allocating API; every output check and drop outside wall interval",
        "cpu_caveat":"zenbench input teardown included; use wall",
        "pixel_parity":"every lossy policy equals frozen e10 RGB; lossless equals exact source",
        "hash_policy":"controller hashes source/references/binary/spec/output; harness makes no SHA claims"}),
    )?;
    if args[0] == "verify" {
        return Ok(());
    }
    let result = zenbench::run(|suite| {
        for group in &groups {
            for encode in [true, false] {
                println!(
                    "timing {} {}",
                    group.name,
                    if encode { "encode" } else { "decode" }
                );
                suite.group(
                    format!(
                        "{}-{}-alloc",
                        group.name,
                        if encode { "encode" } else { "decode" }
                    ),
                    |g| {
                        let c = g.config();
                        c.min_rounds = 20;
                        c.max_rounds = 20;
                        c.auto_rounds = false;
                        c.max_iterations = 16;
                        c.baseline_only = Some(false);
                        c.max_time = Duration::from_secs(600);
                        c.warmup_time = Duration::from_millis(100);
                        g.throughput(zenbench::Throughput::Bytes(pixels.len() as u64));
                        for case in &group.cases {
                            let case = case.clone();
                            let pixels = pixels.clone();
                            let info = info.clone();
                            g.bench_tagged(case.name.clone(), &[("threads", "1")], move |b| {
                                b.with_input(|| ()).run(|()| {
                                    if encode {
                                        Checked {
                                            bytes: case.config.encode(&pixels, &info),
                                            expected: &case.encoded,
                                            info: None,
                                            expected_info: &info,
                                        }
                                    } else {
                                        let (actual_info, bytes) = imij::decode(&case.encoded)
                                            .expect("admitted decode failed");
                                        Checked {
                                            bytes,
                                            expected: &case.decoded,
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
        }
    });
    result.save(out.join("timing.json"))?;
    if result.unreliable
        || result.comparisons.len() != groups.len() * 2
        || result.comparisons.iter().any(|g| {
            let count = groups
                .iter()
                .find(|p| g.group_name.starts_with(&format!("{}-", p.name)))
                .unwrap()
                .cases
                .len();
            g.completed_rounds != 20
                || g.samples.len() != 20
                || g.benchmarks.len() != count
                || g.benchmarks.iter().any(|b| b.summary.n != 20)
                || g.samples.iter().any(|s| {
                    let mut order = s.execution_order.clone();
                    order.sort_unstable();
                    s.iterations == 0
                        || s.elapsed_ns.len() != count
                        || order != (0..count).collect::<Vec<_>>()
                })
        })
    {
        return Err(
            "unreliable/incomplete measurement retained, rejected for default choice".into(),
        );
    }
    Ok(())
}

fn small_cases() -> Result<()> {
    // Dispatch boundary cases, not a resized photo corpus or timing cohort.
    for (w, h) in [(3, 5), (65, 67), (256, 256), (257, 256)] {
        let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
        let pixels: Vec<u8> = (0..u64::from(w) * u64::from(h) * 3)
            .map(|v| ((v * 37 + (v / 17) * 91) % 256) as u8)
            .collect();
        for quality in [5., 90.] {
            let baseline = Policy {
                name: "current".into(),
                effort: 10.,
                mode: Mode::Runlevel,
                compact_streams: false,
            };
            let reference = imij::decode(&lossy(&baseline, quality).encode(&pixels, &info))?.1;
            for mode in [Mode::Runlevel, Mode::Huffman, Mode::Balanced, Mode::Fast] {
                for e in [0., 5., 10.] {
                    let policy = Policy {
                        mode: mode.clone(),
                        effort: e,
                        compact_streams: true,
                        ..baseline.clone()
                    };
                    let cfg = lossy(&policy, quality);
                    let encoded = cfg.encode(&pixels, &info);
                    let (got, decoded) = imij::decode(&encoded)?;
                    if got != info || decoded != reference || cfg.encode(&pixels, &info) != encoded
                    {
                        return Err(
                            "small/boundary tier pixels or deterministic bytes differ".into()
                        );
                    }
                }
            }
        }
        println!("correctness-only {w}x{h}: four modes, efforts0/5/10, q5/90 pass");
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tiny_and_64k_tier_boundaries_preserve_pixels() {
        small_cases().unwrap();
    }
    #[test]
    fn rejects_unknown_schema_and_duplicate_policies() {
        assert!(
            serde_json::from_str::<Policy>(
                r#"{"name":"a","effort":5,"mode":"fastest","compact_streams":true}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<Policy>(
                r#"{"name":"a","effort":5,"mode":"fast","compact_streams":true,"unused":1}"#
            )
            .is_err()
        );
        let mut s = Spec {
            version: 1,
            source: Source {
                path: "raw".into(),
                width: 1,
                height: 1,
                lossless_encoded: "ll".into(),
            },
            references: vec![Reference {
                quality: 5.,
                legacy_encoded: "a".into(),
                compact_encoded: "b".into(),
                decoded: "c".into(),
            }],
            policies: vec![Policy {
                name: "current".into(),
                effort: 10.,
                mode: Mode::Runlevel,
                compact_streams: false,
            }],
            lossless_efforts: vec![10.],
        };
        assert!(validate(&s).is_ok());
        s.policies.push(s.policies[0].clone());
        assert!(validate(&s).is_err());
        s.policies.pop();
        s.lossless_efforts.push(f32::NAN);
        assert!(validate(&s).is_err());
    }
}
