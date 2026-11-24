//! Production ordinary companions; configuration pattern shared with the
//! source-isolated wavelet encode comparison. No experimental reconstruction.
use super::*;

struct Case {
    name: String,
    color: LossyColor,
    entropy: LossyEntropy,
    effort: f32,
    seed: u8,
    tier: Option<crate::DecodeSpeed>,
    small: bool,
}

fn cases() -> Vec<Case> {
    let mut result = Vec::new();
    // All color/coder/effort triples, with both seed levels for every pair.
    for (ci, color) in [LossyColor::Xyb, LossyColor::YCoCg].into_iter().enumerate() {
        for (ei, entropy) in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf]
            .into_iter()
            .enumerate()
        {
            for (fi, effort) in [7.5, 9.0, 10.0].into_iter().enumerate() {
                let seed = (ci ^ ei ^ (fi % 2)) as u8;
                result.push(Case {
                    name: format!("auto_{color:?}_{entropy:?}_e{effort}_s{seed}"),
                    color,
                    entropy,
                    effort,
                    seed,
                    tier: None,
                    small: false,
                });
            }
        }
    }
    for (name, tier, small) in [
        ("compact", crate::DecodeSpeed::Compact, false),
        ("small4", crate::DecodeSpeed::Compact, true),
        ("balanced", crate::DecodeSpeed::Balanced, false),
        ("fast", crate::DecodeSpeed::Fast, false),
        ("fastest", crate::DecodeSpeed::Fastest, false),
    ] {
        let profiles = if small {
            vec![
                (LossyEntropy::RunLevel, 7.5, 0),
                (LossyEntropy::RunLevel, 7.5, 1),
                (LossyEntropy::SplitHuf, 7.5, 0),
                (LossyEntropy::SplitHuf, 7.5, 1),
            ]
        } else {
            vec![
                (LossyEntropy::RunLevel, 7.5, 0),
                (LossyEntropy::SplitHuf, 7.5, 1),
                (LossyEntropy::RunLevel, 9.0, 1),
                (LossyEntropy::SplitHuf, 9.0, 0),
                (LossyEntropy::RunLevel, 10.0, 0),
                (LossyEntropy::SplitHuf, 10.0, 1),
            ]
        };
        for (entropy, effort, seed) in profiles {
            let color = if tier == crate::DecodeSpeed::Fastest {
                LossyColor::YCoCg
            } else {
                LossyColor::Xyb
            };
            result.push(Case {
                name: format!("{name}_{color:?}_{entropy:?}_e{effort}_s{seed}"),
                color,
                entropy,
                effort,
                seed,
                tier: Some(tier),
                small,
            });
        }
    }
    assert_eq!(result.len(), 40);
    result
}

struct Recorder<'a> {
    source: &'a [u8],
    id: &'a str,
    w: usize,
    h: usize,
    source_path: String,
    source_hash: String,
    output: &'a Path,
    cas: &'a Path,
    table: std::fs::File,
    completed: usize,
    required: usize,
    evidence: Vec<serde_json::Value>,
}
impl Recorder<'_> {
    fn record(&mut self, file: Vec<u8>, case: &Case, quality: Option<f32>) {
        let declared = crate::header(&file).unwrap().info;
        assert_eq!(
            (declared.width as usize, declared.height as usize),
            (self.w, self.h)
        );
        assert_eq!(declared.channels, Channels::Rgb);
        assert_eq!(declared.sample, SampleType::U8);
        assert_eq!(declared.cicp, crate::Cicp::SRGB);
        let mut pixels = vec![0; self.source.len()];
        crate::decode_into_threads(&file, &mut pixels, self.w * 3, 1).unwrap();
        for threads in [2, 8] {
            let mut other = vec![0; pixels.len()];
            crate::decode_into_threads(&file, &mut other, self.w * 3, threads).unwrap();
            assert_eq!(other, pixels, "native full decoder thread parity");
        }
        let stride = self.w * 3 + 13;
        let mut padded = vec![0xCC; stride * self.h];
        crate::decode_into_threads(&file, &mut padded, stride, 4).unwrap();
        for (expected, row) in pixels
            .chunks_exact(self.w * 3)
            .zip(padded.chunks_exact(stride))
        {
            assert_eq!(&row[..self.w * 3], expected);
            assert!(row[self.w * 3..].iter().all(|&v| v == 0xCC));
        }
        let mut stream = crate::StreamDecoder::new(&file).unwrap();
        let mut row = vec![0; self.w * 3];
        for expected in pixels.chunks_exact(self.w * 3) {
            assert!(stream.next_row(&mut row).unwrap());
            assert_eq!(row, expected);
        }
        assert!(!stream.next_row(&mut row).unwrap());
        if quality.is_none() {
            assert_eq!(pixels, self.source, "exact lossless source");
        }
        let (color, entropy, details) = if quality.is_some() {
            let p = parse_lossy(&file).unwrap();
            assert_eq!(
                (p.info.width as usize, p.info.height as usize),
                (self.w, self.h)
            );
            assert_eq!(p.info.cicp, crate::Cicp::SRGB);
            let entropy = if p.huf.is_some() {
                LossyEntropy::SplitHuf
            } else {
                LossyEntropy::RunLevel
            };
            (
                format!("{:?}", p.color),
                format!("{entropy:?}"),
                serde_json::json!({
                    "layouts":format!("{:?}",p.layouts), "partition_map":format!("{:?}",p.map),
                    "exact_blocks":p.exact.is_some(), "column_groups":p.ngroups(),
                    "column_width":p.gcols, "stripe_block_rows":p.g.sb, "aq_present":p.aq,
                    "requested_color":format!("{:?}",case.color), "requested_entropy":format!("{:?}",case.entropy)
                }),
            )
        } else {
            (
                "Native".into(),
                "Native".into(),
                serde_json::json!({"codec":"native-lossless"}),
            )
        };
        let score = fast_ssim2::compute_ssimulacra2(
            imgref::Img::new(self.source.as_chunks::<3>().0, self.w, self.h),
            imgref::Img::new(pixels.as_chunks::<3>().0, self.w, self.h),
        )
        .unwrap();
        let (encoded, encoded_hash) = store(self.cas, &file, "imi");
        let (decoded, decoded_hash) = store(self.cas, &pixels, "rgb");
        let q = quality.map(|v| v.to_string()).unwrap_or_default();
        let mode = format!("base_stock_{}", case.name);
        writeln!(self.table,"{}\t{}\t{}\t{mode}\t{color}\t{entropy}\t{q}\t{}\tfalse\t{}\t{score}\t{encoded}\t{decoded}\t{encoded_hash}\t{decoded_hash}\t{}\t{}\t{}\t{}",
                 self.id,self.w,self.h,case.effort,file.len(),self.source_hash,self.source_path,case.seed,pixels==self.source).unwrap();
        self.table.flush().unwrap();
        self.completed += 1;
        self.evidence.push(serde_json::json!({"mode":mode,"quality":quality,"encoded_sha256":encoded_hash,
            "decoded_sha256":decoded_hash,"parsed":details,"threads":"1/2/4/8 exact;4t padded rows;public row output exact"}));
        let progress = serde_json::json!({"pid":std::process::id(),"source":self.id,"completed":self.completed,
            "required":self.required,"last_mode":mode,"last_quality":quality,
            "last_check_epoch":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()});
        let stage = self.output.join("PROGRESS.stage");
        std::fs::write(&stage, serde_json::to_vec_pretty(&progress).unwrap()).unwrap();
        std::fs::rename(stage, self.output.join("PROGRESS.json")).unwrap();
        println!("ordinary {} / {}", self.completed, self.required);
    }
}

pub(super) fn run() {
    let input = std::env::var("OVERLAP7_INPUT").expect("source JSON required");
    let source: serde_json::Value = serde_json::from_slice(&std::fs::read(input).unwrap()).unwrap();
    let id = source["source"].as_str().unwrap();
    let (w, h, stride) = (
        source["width"].as_u64().unwrap() as usize,
        source["height"].as_u64().unwrap() as usize,
        source["stride"].as_u64().unwrap() as usize,
    );
    let raw = std::fs::read(source["pixels"].as_str().unwrap()).unwrap();
    assert!(w > 0 && h > 0 && stride >= w * 3 && raw.len() >= (h - 1) * stride + w * 3);
    let packed = if stride == w * 3 {
        std::borrow::Cow::Borrowed(&raw[..w * h * 3])
    } else {
        let mut v = Vec::with_capacity(w * h * 3);
        for row in raw.chunks(stride).take(h) {
            v.extend_from_slice(&row[..w * 3]);
        }
        std::borrow::Cow::Owned(v)
    };
    let expected_hash = source["source_sha256"].as_str().unwrap();
    assert_eq!(sha256(&packed), expected_hash);
    let output_name = std::env::var("OVERLAP7_OUTPUT").unwrap();
    let output = Path::new(&output_name);
    std::fs::create_dir(output).unwrap();
    let cas_name = std::env::var("OVERLAP7_CAS").unwrap();
    let cas = Path::new(&cas_name);
    let (source_path, source_hash) = store(cas, &packed, "rgb");
    let qualities = std::env::var("OVERLAP7_QUALITIES")
        .ok()
        .map(|v| {
            v.split(',')
                .map(|q| q.parse::<f32>().unwrap())
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            (1..=30)
                .chain((35..=100).step_by(5))
                .chain((102..=116).step_by(2))
                .map(|q| q as f32)
                .collect()
        });
    assert!(
        !qualities.is_empty()
            && qualities
                .iter()
                .all(|q| q.is_finite() && (1.0..=116.0).contains(q))
    );
    for (i, q) in qualities.iter().enumerate() {
        assert!(!qualities[..i].contains(q), "duplicate quality");
    }
    let cases = cases();
    let required = cases.len() * qualities.len() + 2;
    let mut table = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(output.join("quality.tsv"))
        .unwrap();
    writeln!(table,"source\twidth\theight\tmode\tcolor\tentropy\tquality\teffort\taq\tbytes\tssim2\tencoded\tdecoded\tencoded_sha256\tdecoded_sha256\tsource_sha256\tsource_pixels\tseed\tsource_exact").unwrap();
    let mut recorder = Recorder {
        source: &packed,
        id,
        w,
        h,
        source_path,
        source_hash,
        output,
        cas,
        table,
        completed: 0,
        required,
        evidence: Vec::new(),
    };
    let info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8);
    assert_eq!(info.cicp, crate::Cicp::SRGB);
    let mut refusals = Vec::new();
    for effort in [9.0, 10.0] {
        for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
            let cfg = crate::ImiEncoderConfig::lossy(35.0)
                .with_effort(effort)
                .with_threads(1)
                .with_decode_speed(crate::DecodeSpeed::Compact)
                .tune_lossy(|t| {
                    t.entropy = entropy;
                    t.small_transforms = true;
                });
            let error = cfg
                .encode_samples(
                    crate::ImageRef::new(&raw, stride, &info),
                    &Metadata::default(),
                )
                .unwrap_err();
            let reason = error.to_string();
            assert!(reason.contains("small transforms require Compact U8 stills with default layouts, effort below 9 and no trellis"));
            refusals.push(serde_json::json!({"quality":35,"effort":effort,"entropy":format!("{entropy:?}"),"small4":true,"result":"Unsupported","reason":reason}));
        }
    }
    std::fs::write(
        output.join("UNSUPPORTED.json"),
        serde_json::to_vec_pretty(&refusals).unwrap(),
    )
    .unwrap();
    for case in cases {
        for &q in &qualities {
            let cfg = crate::ImiEncoderConfig::lossy(q)
                .with_effort(case.effort)
                .with_threads(1)
                .with_decode_speed(case.tier)
                .tune_lossy(|t| {
                    t.color = case.color;
                    t.entropy = case.entropy;
                    t.small_transforms = case.small;
                    t.lab.seed_bank = u64::from(case.seed);
                });
            assert_eq!(
                cfg.lossy_config().aq_gamma,
                0.0,
                "stock AQ default recorded explicitly"
            );
            let file = cfg
                .encode_samples(
                    crate::ImageRef::new(&raw, stride, &info),
                    &Metadata::default(),
                )
                .unwrap();
            recorder.record(file, &case, Some(q));
        }
    }
    for effort in [7.5, 10.0] {
        let cfg = crate::ImiEncoderConfig::lossless()
            .with_effort(effort)
            .with_threads(1);
        let file = cfg
            .encode_samples(
                crate::ImageRef::new(&raw, stride, &info),
                &Metadata::default(),
            )
            .unwrap();
        let case = Case {
            name: format!("lossless_e{effort}"),
            effort,
            color: LossyColor::Xyb,
            entropy: LossyEntropy::RunLevel,
            seed: 0,
            tier: None,
            small: false,
        };
        recorder.record(file, &case, None);
    }
    assert_eq!(recorder.completed, required);
    std::fs::write(
        output.join("PARSED.json"),
        serde_json::to_vec_pretty(&recorder.evidence).unwrap(),
    )
    .unwrap();
    std::fs::write(output.join("COMPLETE"), format!("{required}\n")).unwrap();
}
