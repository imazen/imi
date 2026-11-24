//! Shared prepare/verify/time/probe cases. See docs/BENCH_MATRIX.md.
#![forbid(unsafe_code)]

#[path = "../src/corpus.rs"]
#[allow(dead_code)]
mod corpus;
#[path = "matrix/quality.rs"]
mod quality;

use imij::{
    Channels, Cicp, ImageInfo, ImiEncoderConfig, LossyColor, LossyEntropy, Metadata, SampleType,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    error::Error,
    fs,
    hint::black_box,
    io::BufReader,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct Profile {
    name: String,
    mode: String,
    color: String,
    quality: f32,
    encode_threads: usize,
    decoder_memory_limit: Option<u64>,
    decoder_memory_threads: usize,
    stripe_rows: Option<u32>,
    group_cols: Option<u32>,
    effort: f32,
    fdct: u8,
    aq_gamma: f32,
    idct_fma: bool,
    rdoq_lambda: f32,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            name: "rl".into(),
            mode: "runlevel".into(),
            color: "xyb".into(),
            quality: 90.0,
            encode_threads: 1,
            decoder_memory_limit: None,
            decoder_memory_threads: 1,
            stripe_rows: None,
            group_cols: None,
            effort: 10.0,
            fdct: 0,
            aq_gamma: 0.0,
            idct_fma: false,
            rdoq_lambda: 0.0,
        }
    }
}

impl Profile {
    fn validate(&self) -> Result<()> {
        if !["lossless", "runlevel", "packed", "split", "huffman"].contains(&self.mode.as_str())
            || !["xyb", "ycocg"].contains(&self.color.as_str())
            || !self.quality.is_finite()
            || !(0.0..=116.0).contains(&self.quality)
            || self.encode_threads == 0
            || self.decoder_memory_threads == 0
            || !self.effort.is_finite()
            || !(0.0..=10.0).contains(&self.effort)
            || self.name.is_empty()
            || self.fdct > 2
            || !self.aq_gamma.is_finite()
            || !self.rdoq_lambda.is_finite()
            || !(0.0..=4.0).contains(&self.rdoq_lambda)
        {
            return Err("invalid profile".into());
        }
        if self.rdoq_lambda != 0.0 && (self.mode != "runlevel" || self.aq_gamma != 0.0) {
            return Err("RDOQ requires runlevel without AQ".into());
        }
        if (self.fdct != 0 || self.idct_fma || self.rdoq_lambda != 0.0) && !cfg!(feature = "lab") {
            return Err("experimental transform comparisons require a lab build".into());
        }
        if let Some(rows) = self.stripe_rows
            && (rows == 0 || (self.mode != "lossless" && (rows % 8 != 0 || rows / 8 > 255)))
        {
            return Err(
                "stripe_rows must be positive; lossy uses multiples of 8 up to 2040".into(),
            );
        }
        if self.mode != "lossless" && self.group_cols.is_some_and(|g| g % 8 != 0) {
            return Err("lossy group_cols must be a multiple of 8".into());
        }
        Ok(())
    }

    fn encode(&self, pixels: &[u8], info: &ImageInfo) -> Result<Vec<u8>> {
        let memory_limit = self.decoder_memory_limit.map(|bytes| {
            imij::DecoderMemoryLimit::new(bytes).with_threads(self.decoder_memory_threads)
        });
        #[cfg(feature = "lab")]
        {
            imij::lab::set_fdct_mode(self.fdct);
            imij::lab::set_idct_fma(self.idct_fma);
        }
        if self.mode == "lossless" {
            let cfg = {
                let mut cfg = imij::ImiEncoderConfig::default();
                cfg = cfg.with_threads(self.encode_threads);
                cfg = cfg.with_effort(self.effort);
                cfg = cfg.tune_lossless(|t| {
                    t.stripe_rows = self.stripe_rows;
                });
                cfg = cfg.with_column_groups(self.group_cols);
                cfg = cfg.with_decoder_memory_limit(memory_limit);
                cfg
            };
            Ok(cfg.encode_samples(
                imij::ImageRef::new(pixels, info.row_bytes(), info),
                &Metadata::default(),
            )?)
        } else {
            let mut cfg = imij::ImiEncoderConfig::lossy(self.quality)
                .with_threads(self.encode_threads)
                .with_effort(self.effort);
            cfg = cfg.tune_lossy(|t| { t.color = if self.color == "xyb" {
                LossyColor::Xyb
            } else {
                LossyColor::YCoCg
            }; });
            cfg = cfg.tune_lossy(|t| { t.entropy = match self.mode.as_str() {
                "runlevel" => LossyEntropy::RunLevel,
                "packed" => panic!("mode {}: packed is retired", self.mode),
                "split" => panic!("mode {}: split is retired", self.mode),
                "huffman" => LossyEntropy::SplitHuf,
                _ => unreachable!(),
            }; });
            cfg = cfg.tune_lossy(|t| {
                t.aq_gamma = self.aq_gamma;
            });
            #[cfg(feature = "lab")]
            {
                cfg = cfg.tune_lossy(|t| {
                    t.lab.rdoq_lambda = self.rdoq_lambda;
                });
            }
            if let Some(rows) = self.stripe_rows {
                cfg = cfg.tune_lossy(|t| {
                    t.stripe_block_rows = rows / 8;
                });
            }
            cfg = cfg.with_column_groups(self.group_cols);
            cfg = cfg.with_decoder_memory_limit(memory_limit);
            Ok(cfg.encode_samples(
                imij::ImageRef::new(pixels, info.row_bytes(), info),
                &Metadata::default(),
            )?)
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    corpus: String,
    classes: Option<String>,
    #[serde(default)]
    variants: bool,
    profiles: Vec<Profile>,
    operations: Vec<String>,
    decode_threads: Vec<usize>,
    pool_threads: usize,
    #[serde(default)]
    crop_sizes: Vec<u32>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: usize,
    comparison: String,
    name: String,
    profile: Profile,
    operation: String,
    threads: usize,
    pool_threads: usize,
    width: u32,
    height: u32,
    channels: u8,
    sample: u8,
    cicp: [u8; 4],
    encoded: PathBuf,
    pixels: PathBuf,
    encoded_bytes: usize,
    #[serde(default)]
    entropy_work: Option<[u64; 3]>,
}

impl Case {
    fn info(&self) -> Result<ImageInfo> {
        let channels = match self.channels {
            1 => Channels::Gray,
            2 => Channels::GrayAlpha,
            3 => Channels::Rgb,
            4 => Channels::Rgba,
            _ => return Err("bad channels".into()),
        };
        let sample = match self.sample {
            0 => SampleType::U8,
            1 => SampleType::U16,
            2 => SampleType::F16,
            3 => SampleType::U32,
            4 => SampleType::I32,
            5 => SampleType::F32,
            _ => return Err("bad sample".into()),
        };
        let mut info = ImageInfo::new(self.width, self.height, channels, sample);
        info.cicp = Cicp {
            color_primaries: self.cicp[0],
            transfer_characteristics: self.cicp[1],
            matrix_coefficients: self.cicp[2],
            full_range: self.cicp[3] != 0,
        };
        Ok(info)
    }
}

fn json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn save<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

fn verify(
    encoded: &[u8],
    original: &[u8],
    info: &ImageInfo,
    lossless: bool,
    threads: &[usize],
) -> Result<Vec<u8>> {
    let (got_info, expected) = imij::decode(encoded)?;
    if got_info != *info {
        return Err("header changed".into());
    }
    if lossless && expected != original {
        return Err("lossless pixels changed".into());
    }
    if info.channels.has_alpha() {
        let bps = info.sample.bytes();
        let bpp = info.bytes_per_pixel();
        for (a, b) in expected.chunks_exact(bpp).zip(original.chunks_exact(bpp)) {
            if a[bpp - bps..] != b[bpp - bps..] {
                return Err("alpha changed".into());
            }
        }
    }
    let rb = info.row_bytes();
    for &t in threads {
        let stride = rb + 13;
        let mut out = vec![0xA5; stride * info.height as usize];
        imij::decode_into_threads(encoded, &mut out, stride, t)?;
        for (a, b) in out.chunks_exact(stride).zip(expected.chunks_exact(rb)) {
            if &a[..rb] != b || a[rb..].iter().any(|&v| v != 0xA5) {
                return Err(format!("threaded pixels/padding changed at {t} threads").into());
            }
        }
    }
    let mut stream = imij::StreamDecoder::new(encoded)?;
    let mut row = vec![0; rb];
    for expected_row in expected.chunks_exact(rb) {
        if !stream.next_row(&mut row)? || row != expected_row {
            return Err("stream differs".into());
        }
    }
    if stream.next_row(&mut row)? {
        return Err("too many stream rows".into());
    }
    let mut rows = 0usize;
    imij::decode_reader(encoded, &imij::Limits::DEFAULT, |_, y, row| {
        if y as usize != rows || row != &expected[rows * rb..][..rb] {
            return Err(imij::Error::Corrupt("reader differs"));
        }
        rows += 1;
        Ok(())
    })?;
    if rows != info.height as usize {
        return Err("reader row count differs".into());
    }
    Ok(expected)
}

fn prepare(spec_path: &Path, out: &Path) -> Result<()> {
    let spec: Spec = json(spec_path)?;
    if out.exists() {
        return Err("output directory already exists; use a new run directory".into());
    }
    if spec.pool_threads == 0
        || spec.crop_sizes.contains(&0)
        || spec.decode_threads.is_empty()
        || spec.decode_threads.contains(&0)
        || spec.profiles.is_empty()
        || spec.operations.is_empty()
    {
        return Err("empty/zero grid axis".into());
    }
    let mut names = HashSet::new();
    for p in &spec.profiles {
        p.validate()?;
        if !names.insert(&p.name) {
            return Err("duplicate profile name".into());
        }
    }
    for op in &spec.operations {
        if !["encode", "into", "alloc", "stream", "reader", "reader-file"].contains(&op.as_str()) {
            return Err(format!("unknown operation: {op}").into());
        }
    }
    fs::create_dir_all(out.join("cases"))?;
    let out = fs::canonicalize(out)?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(spec.pool_threads)
        .build()?;
    let mut cases = Vec::new();
    let mut image_index = 0;
    let mut artifact_index = 0;
    for (class, path) in corpus::load_manifest(&spec.corpus, spec.classes.as_deref()) {
        let image = corpus::load_png(&class, Path::new(&path))?;
        let mut images = if spec.variants {
            corpus::variants(&image)
        } else {
            Vec::new()
        };
        images.insert(0, image);
        if !spec.crop_sizes.is_empty() {
            images = images
                .into_iter()
                .flat_map(|img| {
                    spec.crop_sizes
                        .iter()
                        .map(|&side| {
                            let mut info = img.info.clone();
                            info.width = info.width.min(side);
                            info.height = info.height.min(side);
                            let pixels = img
                                .pixels
                                .chunks_exact(img.info.row_bytes())
                                .take(info.height as usize)
                                .flat_map(|r| r[..info.row_bytes()].iter().copied())
                                .collect();
                            corpus::Img {
                                class: img.class.clone(),
                                name: format!("{}-crop{side}", img.name),
                                info,
                                pixels,
                                png_bytes: img.png_bytes,
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .collect();
        }
        for img in images {
            let raw_path = out.join(format!("{image_index}.raw"));
            fs::write(&raw_path, &img.pixels)?;
            let mut equal_pixels = HashMap::<String, Vec<u8>>::new();
            for profile in &spec.profiles {
                let encoded = pool.install(|| {
                    profile
                        .encode(&img.pixels, &img.info)
                        .map_err(|e| e.to_string())
                })?;
                #[cfg(feature = "lab")]
                let entropy_work = (profile.mode != "lossless").then(imij::lab::lossy_side_stats);
                #[cfg(not(feature = "lab"))]
                let entropy_work = None;
                // Keep failing outputs too: verification must not discard its reproducer.
                let encoded_path = out.join(format!("{artifact_index}.imi"));
                fs::write(&encoded_path, &encoded)?;
                artifact_index += 1;
                let expected = pool.install(|| {
                    verify(
                        &encoded,
                        &img.pixels,
                        &img.info,
                        profile.mode == "lossless",
                        &spec.decode_threads,
                    )
                    .map_err(|e| e.to_string())
                })?;
                if profile.mode != "lossless" {
                    let mut key = format!(
                        "{}:{}:{}:{}:{}:{}",
                        profile.color,
                        profile.quality,
                        profile.fdct,
                        profile.aq_gamma,
                        profile.idct_fma,
                        profile.rdoq_lambda
                    );
                    // RDOQ prices geometry/effort-dependent entropy models.
                    // Pixel invariance still applies to encoder thread count.
                    if profile.rdoq_lambda != 0.0 {
                        key.push_str(&format!(
                            ":{:?}:{:?}:{}",
                            profile.stripe_rows, profile.group_cols, profile.effort
                        ));
                    }
                    if equal_pixels.get(&key).is_some_and(|v| *v != expected) {
                        return Err(format!(
                            "effort/entropy/group/stripe/thread pixel mismatch: {} {}",
                            img.name, profile.name
                        )
                        .into());
                    }
                    equal_pixels.entry(key).or_insert(expected);
                }
                for op in &spec.operations {
                    let ts = if op == "into" {
                        spec.decode_threads.as_slice()
                    } else {
                        &[1]
                    };
                    for &t in ts {
                        let case = Case {
                            id: cases.len(),
                            comparison: format!("{image_index}:{}:{}:{op}", img.class, img.name),
                            name: format!("{}:{t}t", profile.name),
                            profile: profile.clone(),
                            operation: op.clone(),
                            threads: t,
                            pool_threads: spec.pool_threads,
                            width: img.info.width,
                            height: img.info.height,
                            channels: img.info.channels as u8,
                            sample: img.info.sample as u8,
                            cicp: [
                                img.info.cicp.color_primaries,
                                img.info.cicp.transfer_characteristics,
                                img.info.cicp.matrix_coefficients,
                                u8::from(img.info.cicp.full_range),
                            ],
                            encoded: encoded_path.clone(),
                            pixels: raw_path.clone(),
                            encoded_bytes: encoded.len(),
                            entropy_work,
                        };
                        save(&out.join("cases").join(format!("{}.json", case.id)), &case)?;
                        cases.push(case);
                    }
                }
            }
            image_index += 1;
            eprintln!(
                "verified image {image_index}: {} ({} cases)",
                img.name,
                cases.len()
            );
        }
    }
    if cases.is_empty() {
        return Err("no cases matched the corpus".into());
    }
    save(&out.join("index.json"), &cases)?;
    eprintln!(
        "prepared and verified {} cases / {image_index} images",
        cases.len()
    );
    Ok(())
}

struct State {
    case: Case,
    info: ImageInfo,
    encoded: Vec<u8>,
    pixels: Vec<u8>,
    output: Vec<u8>,
}

impl State {
    fn load(case: Case) -> Result<Self> {
        case.profile.validate()?;
        let info = case.info()?;
        let encoded = if matches!(case.operation.as_str(), "encode" | "reader-file") {
            Vec::new()
        } else {
            fs::read(&case.encoded)?
        };
        let pixels = if case.operation == "encode" {
            fs::read(&case.pixels)?
        } else {
            Vec::new()
        };
        let output = match case.operation.as_str() {
            "into" => vec![0; info.row_bytes() * info.height as usize],
            "stream" => vec![0; info.row_bytes()],
            _ => Vec::new(),
        };
        Ok(Self {
            case,
            info,
            encoded,
            pixels,
            output,
        })
    }

    fn run(&mut self) -> Result<usize> {
        #[cfg(feature = "lab")]
        imij::lab::set_idct_fma(self.case.profile.idct_fma);
        let result = match self.case.operation.as_str() {
            "encode" => black_box(self.case.profile.encode(&self.pixels, &self.info)?).len(),
            "into" => {
                imij::decode_into_threads(
                    &self.encoded,
                    &mut self.output,
                    self.info.row_bytes(),
                    self.case.threads,
                )?;
                black_box(&self.output);
                self.output.len()
            }
            "alloc" => black_box(imij::decode(&self.encoded)?.1).len(),
            "stream" => {
                let mut d = imij::StreamDecoder::new(&self.encoded)?;
                while d.next_row(&mut self.output)? {
                    black_box(&self.output);
                }
                d.rows_decoded() as usize
            }
            "reader" => {
                let mut n = 0;
                imij::decode_reader(
                    self.encoded.as_slice(),
                    &imij::Limits::DEFAULT,
                    |_, _, row| {
                        n += black_box(row).len();
                        Ok(())
                    },
                )?;
                n
            }
            "reader-file" => {
                let mut n = 0;
                imij::decode_reader(
                    BufReader::new(fs::File::open(&self.case.encoded)?),
                    &imij::Limits::DEFAULT,
                    |_, _, row| {
                        n += black_box(row).len();
                        Ok(())
                    },
                )?;
                n
            }
            _ => return Err("unknown operation".into()),
        };
        Ok(result)
    }
}

fn time(index: &Path, output: &Path, rounds: usize, filter: Option<&str>) -> Result<()> {
    if rounds < 2 {
        return Err("at least two rounds required".into());
    }
    let cases: Vec<Case> = json(index)?;
    let mut groups = BTreeMap::<String, Vec<Case>>::new();
    for case in cases {
        if filter.is_none_or(|s| case.comparison == s) {
            groups
                .entry(case.comparison.clone())
                .or_default()
                .push(case);
        }
    }
    if groups.is_empty() {
        return Err("no groups matched".into());
    }
    if groups.len() != 1 {
        return Err("time one comparison per process; use matrix.py time".into());
    }
    let pool_threads = groups.values().next().unwrap()[0].pool_threads;
    if groups
        .values()
        .flatten()
        .any(|c| c.pool_threads != pool_threads)
    {
        return Err("a timing run must use one pool size".into());
    }
    let pool = Arc::new(
        rayon::ThreadPoolBuilder::new()
            .num_threads(pool_threads)
            .build()?,
    );
    let mut loaded = Vec::new();
    for (name, cases) in groups {
        loaded.push((
            name,
            cases
                .into_iter()
                .map(State::load)
                .collect::<Result<Vec<_>>>()?,
        ));
    }
    let results = zenbench::run(|suite| {
        for (name, states) in loaded {
            suite.group(name, |group| {
                group.throughput(zenbench::Throughput::Bytes(
                    states[0].info.row_bytes() as u64 * states[0].info.height as u64,
                ));
                let config = group.config();
                config.min_rounds = rounds;
                config.max_rounds = rounds;
                config.max_time = Duration::from_secs(600);
                config.warmup_time = Duration::from_millis(100);
                config.auto_rounds = false;
                for mut state in states {
                    let pool = pool.clone();
                    let threads = if state.case.operation == "encode" {
                        state.case.profile.encode_threads
                    } else {
                        state.case.threads
                    }
                    .to_string();
                    group.bench_tagged(
                        state.case.name.clone(),
                        &[("threads", &threads)],
                        move |b| {
                            b.iter(|| {
                                pool.install(|| state.run().map_err(|e| e.to_string()))
                                    .unwrap()
                            })
                        },
                    );
                }
            });
        }
    });
    results.save(output)?;
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args()
        .skip(1)
        .filter(|a| a != "--bench")
        .collect();
    match args.first().map(String::as_str) {
        Some("build-info") if args.len() == 1 => {
            println!("{}", serde_json::json!({
                "lab_instrumentation": cfg!(feature = "lab"),
                "debug_assertions": cfg!(debug_assertions),
                "target_arch": std::env::consts::ARCH,
                "reported_lossless_tier": imij::simd_tier(),
            }));
            Ok(())
        }
        Some("prepare") if args.len() == 3 => prepare(Path::new(&args[1]), Path::new(&args[2])),
        Some("score") if args.len() == 3 => quality::score(Path::new(&args[1]), Path::new(&args[2])),
        Some("time") if (3..=5).contains(&args.len()) => time(Path::new(&args[1]), Path::new(&args[2]),
            args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(20), args.get(4).map(String::as_str)),
        Some("probe") if (2..=3).contains(&args.len()) => {
            let case: Case = json(Path::new(&args[1]))?;
            let repetitions: usize = args.get(2).map(|s| s.parse()).transpose()?.unwrap_or(1);
            if repetitions == 0 { return Err("repetitions must be positive".into()); }
            let pool = rayon::ThreadPoolBuilder::new().num_threads(case.pool_threads).build()?;
            let mut state = State::load(case)?;
            for _ in 0..repetitions {
                black_box(pool.install(|| state.run().map_err(|e| e.to_string()))?);
            }
            Ok(())
        }
        _ => Err("usage: matrix prepare SPEC.json NEW_DIR | time INDEX.json RESULT.json [ROUNDS] [GROUP_FILTER] | probe CASE.json [REPETITIONS]".into()),
    }
}
