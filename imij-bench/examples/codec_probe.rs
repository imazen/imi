//! Fair native, single-thread allocating encode/decode measurement.
//! `verify|time SPEC.json NEWDIR`; `decode INPUT.imi OUTPUT.raw` for process RSS.
//! SPEC v1: {"version":1,"source":{"path":"rgb.raw","width":W,"height":H},
//! "qualities":[{"quality":30,"legacy_encoded":"old.imi",
//! "legacy_decoded":"old.raw"}]}. All fields are required; unknown fields fail.
//! Paths are relative to SPEC. Every legacy reference must come from the
//! controller's independently hashed frozen full8/XYB/runlevel/sb1/e10 baseline.
//! This driver checks exact bytes, not the provenance of those reference files.
#![forbid(unsafe_code)]

use imij::{
    ChannelSampling, Channels, ImageInfo, ImiEncoderConfig, LossyChannelLayout, LossyColor,
    LossyEntropy, LossyTransform, Metadata, SampleType,
};
use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    version: u32,
    source: Source,
    qualities: Vec<Quality>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Source {
    path: PathBuf,
    width: u32,
    height: u32,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Quality {
    quality: f32,
    legacy_encoded: PathBuf,
    legacy_decoded: PathBuf,
}

#[derive(Clone)]
enum Config {
    Lossless(ImiEncoderConfig),
    Lossy(ImiEncoderConfig),
}
impl Config {
    fn encode(&self, pixels: &[u8], info: &ImageInfo) -> Vec<u8> {
        let stride = info.width as usize * 3;
        match self {
            Self::Lossless(c) => c.encode_samples(
                imij::ImageRef::new(pixels, stride, info),
                &Metadata::default(),
            ),
            Self::Lossy(c) => c.encode_samples(
                imij::ImageRef::new(pixels, stride, info),
                &Metadata::default(),
            ),
        }
        .expect("admitted encode failed")
    }
    fn json(&self) -> serde_json::Value {
        match self {
            Self::Lossless(c) => serde_json::json!({
                "coding":"lossless", "aliases":["default","effort10"],
                "stripe_rows":c.stripe_rows,"lanes":c.lanes,"k_low":c.k_low,
                "k_high":c.k_high,"m":c.m,"run_min":c.run_min,"max_ctx":c.max_ctx,
                "ctx_cross":c.ctx_cross,"decor_mask":c.decor_mask,
                "predictor_mask":c.predictor_mask,"threads":c.threads,
                "effort":c.effort,"palette":c.palette,"column_groups":c.column_groups,
                "decoder_memory_limit":null,"lab_guide":null
            }),
            Self::Lossy(c) => serde_json::json!({
                "coding":"lossy","quality":c.quality,"color":"xyb",
                "channel_layouts":["full8","full8","full8"],
                "compact_streams":c.compact_streams,"stripe_block_rows":c.stripe_block_rows,
                "threads":c.threads,"effort":c.effort,"round":c.round,
                "max_ac_tables":c.max_ac_tables,"channel_mul":c.channel_mul,
                "aq_gamma":c.aq_gamma,"aq_r0":c.aq_r0,"entropy":"runlevel",
                "decode_speed":null,"matrix":c.matrix,"column_groups":c.column_groups,
                "decoder_memory_limit":null,"lab_lf_pred":0,"lab_rdoq_lambda":0
            }),
        }
    }
}
struct Case {
    name: String,
    config: Config,
    encoded: Vec<u8>,
    decoded: Arc<Vec<u8>>,
}
struct Admitted {
    pixels: Arc<Vec<u8>>,
    info: ImageInfo,
    cases: Vec<Arc<Case>>,
}

// InputBencher::run stops wall timing before dropping output. Check every
// measured output here, then release it; do not accumulate multiple rasters.
struct Checked<'a> {
    bytes: Vec<u8>,
    expected: &'a [u8],
    info: Option<ImageInfo>,
    expected_info: &'a ImageInfo,
}
impl Drop for Checked<'_> {
    fn drop(&mut self) {
        assert!(
            self.bytes.as_slice() == self.expected,
            "timed output changed"
        );
        if let Some(info) = &self.info {
            assert_eq!(info, self.expected_info);
        }
    }
}

fn save(path: impl AsRef<Path>, value: &impl Serialize) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}
fn resolve(base: &Path, path: &Path) -> Result<PathBuf> {
    Ok(fs::canonicalize(base.join(path))?)
}
fn read_exact_file(path: &Path, length: usize) -> Result<Vec<u8>> {
    if fs::metadata(path)?.len() != length as u64 {
        return Err("raw RGB/reference byte count differs from geometry".into());
    }
    let bytes = fs::read(path)?;
    if bytes.len() != length {
        return Err("file changed while reading".into());
    }
    Ok(bytes)
}
fn validate_spec(spec: &Spec) -> Result<usize> {
    if spec.version != 1
        || spec.source.width == 0
        || spec.source.height == 0
        || spec.qualities.is_empty()
        || spec.qualities.len() > 16
    {
        return Err("expected spec v1, nonzero RGB geometry and 1..16 qualities".into());
    }
    let pixels = u64::from(spec.source.width) * u64::from(spec.source.height);
    if pixels > 4_194_304 {
        return Err("probe source exceeds 4,194,304 pixels".into());
    }
    for (i, q) in spec.qualities.iter().enumerate() {
        if !q.quality.is_finite()
            || !(0.0..=116.0).contains(&q.quality)
            || spec.qualities[..i].iter().any(|p| p.quality == q.quality)
        {
            return Err("qualities must be unique finite values in 0..116".into());
        }
    }
    Ok(usize::try_from(pixels * 3)?)
}

fn admit(spec: &mut Spec, base: &Path, out: &Path) -> Result<Admitted> {
    let length = validate_spec(spec)?;
    spec.source.path = resolve(base, &spec.source.path)?;
    let pixels = Arc::new(read_exact_file(&spec.source.path, length)?);
    let info = ImageInfo::new(
        spec.source.width,
        spec.source.height,
        Channels::Rgb,
        SampleType::U8,
    );
    let mut cases = Vec::new();
    for (index, q) in spec.qualities.iter_mut().enumerate() {
        q.legacy_encoded = resolve(base, &q.legacy_encoded)?;
        q.legacy_decoded = resolve(base, &q.legacy_decoded)?;
        if fs::metadata(&q.legacy_encoded)?.len() > 128 << 20 {
            return Err("reference file exceeds 128 MiB".into());
        }
        let old = fs::read(&q.legacy_encoded)?;
        let expected = Arc::new(read_exact_file(&q.legacy_decoded, length)?);
        for compact in [false, true] {
            let cfg = imij::ImiEncoderConfig::lossy(q.quality)
                .tune_lossy(|t| t.color = LossyColor::Xyb)
                .tune_lossy(|t| t.entropy = LossyEntropy::RunLevel)
                .tune_lossy(|t| t.stripe_block_rows = 1)
                .with_threads(1)
                .tune_lossy(|t| {
                    t.channel_layouts =
                        [LossyChannelLayout::new(LossyTransform::Dct8, ChannelSampling::Full); 3]
                })
                .with_effort(10.0)
                .tune_lossy(|t| t.compact_streams = compact);
            let config = Config::Lossy(cfg);
            let encoded = config.encode(&pixels, &info);
            if !compact && encoded != old {
                return Err(format!("q{} legacy encoded parity failed", q.quality).into());
            }
            let (decoded_info, decoded) = imij::decode(&encoded)?;
            if decoded_info != info || decoded.as_slice() != expected.as_slice() {
                return Err(
                    format!("q{} decoded parity failed (compact={compact})", q.quality).into(),
                );
            }
            cases.push(Arc::new(Case {
                name: format!("q{index:02}-compact{}", u8::from(compact)),
                config,
                encoded,
                decoded: expected.clone(),
            }));
        }
    }
    let default = imij::ImiEncoderConfig::default();
    if default.threads != 1 || default.effort != 10.0 {
        return Err("lossless default profile changed".into());
    }
    let config = Config::Lossless(default.clone());
    let encoded = config.encode(&pixels, &info);
    if encoded != Config::Lossless(default.with_effort(10.0)).encode(&pixels, &info) {
        return Err("lossless default/effort10 alias parity failed".into());
    }
    let (decoded_info, decoded) = imij::decode(&encoded)?;
    if decoded_info != info || decoded.as_slice() != pixels.as_slice() {
        return Err("lossless roundtrip failed".into());
    }
    cases.push(Arc::new(Case {
        name: "lossless-default-effort10".into(),
        config,
        encoded,
        decoded: pixels.clone(),
    }));
    // Repeated exact correctness before benchmark registration; I/O is outside
    // timing. Every later timed output is additionally checked during teardown.
    for c in &cases {
        if c.config.encode(&pixels, &info) != c.encoded {
            return Err("repeated encoder output changed".into());
        }
        fs::write(out.join(format!("{}.imi", c.name)), &c.encoded)?;
        fs::write(out.join(format!("{}.raw", c.name)), c.decoded.as_slice())?;
    }
    Ok(Admitted {
        pixels,
        info,
        cases,
    })
}

fn standalone_decode(input: &Path, output: &Path) -> Result<()> {
    if fs::metadata(input)?.len() > 128 << 20 {
        return Err("input exceeds 128 MiB".into());
    }
    let input = fs::read(input)?;
    let (info, pixels) = imij::decode_with_limits(&input, &{
        let mut limits = imij::Limits::DEFAULT;
        limits.max_pixels = 4_194_304;
        limits.max_bytes = 16_777_216;
        limits
    })?;
    if info.channels != Channels::Rgb || info.sample != SampleType::U8 {
        return Err("RSS command requires RGB8".into());
    }
    fs::write(output, &pixels)?;
    save(
        output.with_extension("raw.json"),
        &serde_json::json!({
            "width":info.width,"height":info.height,"stride_bytes":info.width as usize*3,
            "decoded_bytes":pixels.len(),"threads":1,
            "memory_scope":"whole process: runtime, compressed input, allocated output and codec scratch; not a codec working-memory cap"
        }),
    )
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err(
            "usage: codec_probe verify|time SPEC.json NEWDIR | decode INPUT.imi OUTPUT.raw".into(),
        );
    }
    if args[0] == "decode" {
        return standalone_decode(Path::new(&args[1]), Path::new(&args[2]));
    }
    if args[0] != "verify" && args[0] != "time" {
        return Err("unknown command".into());
    }
    let spec_path = fs::canonicalize(&args[1])?;
    if fs::metadata(&spec_path)?.len() > 64 * 1024 {
        return Err("spec exceeds 64 KiB".into());
    }
    let mut spec: Spec = serde_json::from_slice(&fs::read(&spec_path)?)?;
    validate_spec(&spec)?;
    let out = Path::new(&args[2]);
    fs::create_dir(out)?;
    let Admitted {
        pixels,
        info,
        cases,
    } = admit(&mut spec, spec_path.parent().unwrap(), out)?;
    save(
        out.join("admission.json"),
        &serde_json::json!({
            "spec":spec,"spec_path":spec_path,"mode":args[0],"threads":1,
            "source_format":"tight native RGB8 sRGB, stride=width*3",
            "requested_rounds":12,"requested_max_iterations":16,
            "timer":"zenbench with_input unit setup: allocating API closure timed, output check and drop outside wall interval",
            "cpu_time_caveat":"zenbench InputBencher CPU-time interval includes teardown; use wall-clock for cross-codec comparison",
            "encode_scope":"color transform, quantization when lossy, entropy and codec/output allocations",
            "decode_scope":"native allocating decode including output allocation/zeroing, header/entropy/color and codec scratch",
            "verification":"exact frozen legacy bytes/RGB, compact pixel parity, lossless exact input, repeated encode; every timed output byte-checked after timer",
            "hashes":"controller must hash source, frozen references, executable and emitted .imi/.raw files; this driver provides no SHA claims",
            "cases":cases.iter().map(|c|serde_json::json!({"name":c.name,"config":c.config.json(),"encoded_bytes":c.encoded.len(),"decoded_bytes":c.decoded.len(),"encoded":format!("{}.imi",c.name),"decoded":format!("{}.raw",c.name)})).collect::<Vec<_>>()
        }),
    )?;
    if args[0] == "verify" {
        return Ok(());
    }
    let result = zenbench::run(|suite| {
        for encode in [true, false] {
            suite.group(
                if encode {
                    "encode-alloc"
                } else {
                    "decode-alloc"
                },
                |g| {
                    let c = g.config();
                    c.min_rounds = 12;
                    c.max_rounds = 12;
                    c.auto_rounds = false;
                    c.max_iterations = 16;
                    c.baseline_only = Some(false);
                    c.max_time = Duration::from_secs(600);
                    c.warmup_time = Duration::from_millis(100);
                    g.throughput(zenbench::Throughput::Bytes(pixels.len() as u64));
                    for case in &cases {
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
    });
    result.save(out.join("timing.json"))?;
    let groups:Vec<_>=result.comparisons.iter().map(|g|serde_json::json!({
        "group":g.group_name,"completed_rounds":g.completed_rounds,"samples":g.samples.len(),
        "benchmark_count":g.benchmarks.len(),"summary_n":g.benchmarks.iter().map(|b|b.summary.n).collect::<Vec<_>>(),
        "orders":g.samples.iter().map(|s|&s.execution_order).collect::<Vec<_>>(),
        "iterations":g.samples.iter().map(|s|s.iterations).collect::<Vec<_>>(),
        "gate_clean":g.samples.iter().map(|s|s.gate_clean).collect::<Vec<_>>()
    })).collect();
    save(
        out.join("inspection.json"),
        &serde_json::json!({"unreliable":result.unreliable,"gate_waits":result.gate_waits,"groups":groups}),
    )?;
    if result.comparisons.len() != 2
        || result.comparisons.iter().any(|g| {
            g.completed_rounds != 12
                || g.samples.len() != 12
                || g.benchmarks.len() != cases.len()
                || g.benchmarks.iter().any(|b| b.summary.n != 12)
                || g.samples.iter().any(|s| {
                    let mut order = s.execution_order.clone();
                    order.sort_unstable();
                    s.iterations == 0
                        || s.elapsed_ns.len() != cases.len()
                        || order != (0..cases.len()).collect::<Vec<_>>()
                })
        })
    {
        return Err(
            "incomplete rounds, summaries or permutations; retained results require rejection"
                .into(),
        );
    }
    Ok(())
}
