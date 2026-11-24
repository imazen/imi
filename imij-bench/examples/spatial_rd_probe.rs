//! Synthetic correctness and pre-change classic byte oracle. No RD scoring/timing.
#![forbid(unsafe_code)]
#[cfg(feature = "lab")]
mod probe {
    use imij::{Channels, ImageInfo, ImiEncoderConfig, LossyColor, Metadata, SampleType};
    use serde_json::{Value, json};
    use std::{
        error::Error,
        fs,
        io::Write,
        path::Path,
        process::{Command, Stdio},
    };
    type Result<T> = std::result::Result<T, Box<dyn Error>>;
    fn need(ok: bool, message: &str) -> Result<()> {
        if ok { Ok(()) } else { Err(message.into()) }
    }
    fn hash(raw: &[u8]) -> Result<String> {
        let mut child = Command::new("sha256sum")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .ok_or("hash stdin absent")?
            .write_all(raw)?;
        let out = child.wait_with_output()?;
        need(out.status.success(), "hash failed")?;
        Ok(String::from_utf8(out.stdout)?
            .split_whitespace()
            .next()
            .ok_or("digest absent")?
            .into())
    }
    fn set_policy(cfg: &mut ImiEncoderConfig, policy: u8) -> Result<()> {
        need(policy <= 4, "invalid probe policy")?;
        cfg = cfg.tune_lossy(|t| {
            t.lab.rdoq_policy = policy;
        });
        Ok(())
    }
    fn input(info: &ImageInfo, pattern: usize) -> Vec<u8> {
        (0..info.row_bytes() * info.height as usize)
            .map(|i| match pattern {
                0 => 0,
                1 => 255,
                2 => ((i * 37 + i / 17 * 91) % 256) as u8,
                3 => ((i / info.bytes_per_pixel() * 3) % 256) as u8,
                _ => {
                    if (i / info.bytes_per_pixel() + i / info.row_bytes()).is_multiple_of(2) {
                        0
                    } else {
                        255
                    }
                }
            })
            .collect()
    }
    fn encode(raw: &[u8], info: &ImageInfo, cfg: &ImiEncoderConfig) -> Result<Vec<u8>> {
        Ok(cfg.encode_samples(
            imij::ImageRef::new(raw, info.row_bytes(), info),
            &Metadata::default(),
        )?)
    }
    fn golden(dest: &Path, reference: Option<&Path>) -> Result<Vec<Value>> {
        need(!dest.exists(), "output directory must be new")?;
        fs::create_dir_all(dest)?;
        let mut rows = Vec::new();
        for channels in [
            Channels::Rgb,
            Channels::Rgba,
            Channels::Gray,
            Channels::GrayAlpha,
        ] {
            let info = ImageInfo::new(139, 25, channels, SampleType::U8);
            let raw = input(&info, 2);
            for color in [LossyColor::Xyb, LossyColor::YCoCg] {
                for quality in [30.0, 90.0] {
                    for lambda in [0.0, 0.08] {
                        for compact in [false, true] {
                            let mut cfg = imij::ImiEncoderConfig::lossy(quality)
                                .tune_lossy(|t| t.color = color)
                                .with_effort(7.5)
                                .tune_lossy(|t| t.compact_streams = compact)
                                .tune_lossy(|t| t.stripe_block_rows = 3);
                            cfg = cfg.with_column_groups(Some(16));
                            cfg = cfg.tune_lossy(|t| {
                                t.lab.rdoq_lambda = lambda;
                            });
                            set_policy(&mut cfg, 0)?;
                            let file = encode(&raw, &info, &cfg)?;
                            let id = format!(
                                "{:?}-{:?}-q{quality}-l{lambda}-c{compact}",
                                channels, color
                            );
                            let path = dest.join(format!("{id}.imi"));
                            if let Some(old) = reference {
                                need(
                                    fs::read(old.join(format!("{id}.imi")))? == file,
                                    "classic/disabled bytes differ from pre-change producer",
                                )?;
                            }
                            fs::write(&path, &file)?;
                            rows.push(json!({"id":id,"encoded":path,"encoded_bytes":file.len(),"sha256":hash(&file)?,"classic":true}));
                        }
                    }
                }
            }
        }
        need(rows.len() == 64, "golden grid differs")?;
        Ok(rows)
    }
    fn apis(file: &[u8], info: &ImageInfo) -> Result<()> {
        let (actual, expected) = imij::decode(file)?;
        need(actual == *info, "image info differs")?;
        let stride = info.row_bytes() + 11;
        for threads in [1, 4] {
            let mut padded = vec![0xa5; stride * info.height as usize];
            imij::decode_into_threads(file, &mut padded, stride, threads)?;
            for (a, b) in padded
                .chunks_exact(stride)
                .zip(expected.chunks_exact(info.row_bytes()))
            {
                need(
                    &a[..info.row_bytes()] == b && a[info.row_bytes()..].iter().all(|&v| v == 0xa5),
                    "padded/threaded decode differs",
                )?;
            }
        }
        let mut stream = imij::StreamDecoder::new(file)?;
        let mut row = vec![0; info.row_bytes()];
        for actual in expected.chunks_exact(info.row_bytes()) {
            need(
                stream.next_row(&mut row)? && row == actual,
                "stream differs",
            )?;
        }
        need(!stream.next_row(&mut row)?, "stream has extra row")?;
        let mut read = Vec::new();
        imij::decode_reader(file, &imij::Limits::DEFAULT, |_, _, row| {
            read.extend_from_slice(row);
            Ok(())
        })?;
        need(read == expected, "reader differs")
    }
    fn bounded() -> Result<usize> {
        let mut count = 0;
        for fdct in [0, 1, 2] {
            imij::lab::set_fdct_mode(fdct);
            for (w, h, pattern) in [
                (1, 1, 0),
                (3, 5, 1),
                (67, 35, 2),
                (139, 25, 3),
                (257, 129, 4),
            ] {
                let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
                let raw = input(&info, pattern);
                for group in [0, 16, 136] {
                    for policy in 1..=4 {
                        for quality in [30.0, 80.0, 81.0, 100.0, 116.0] {
                            let mut cfg = imij::ImiEncoderConfig::lossy(quality)
                                .with_effort(7.5)
                                .tune_lossy(|t| t.stripe_block_rows = 3);
                            cfg = cfg.with_column_groups(Some(group));
                            cfg = cfg.tune_lossy(|t| {
                                t.lab.rdoq_lambda = 0.08;
                            });
                            set_policy(&mut cfg, policy)?;
                            let file = encode(&raw, &info, &cfg)?;
                            apis(&file, &info)?;
                            cfg = cfg.with_threads(4);
                            need(encode(&raw, &info, &cfg)? == file, "encoder threads differ")?;
                            cfg = cfg.with_threads(1);
                            cfg = cfg.tune_lossy(|t| {
                                t.lab.rdoq_lambda = 0.0;
                            });
                            let disabled = encode(&raw, &info, &cfg)?;
                            if quality > 80.0 {
                                need(file == disabled, "high quality bypass differs")?;
                            }
                            set_policy(&mut cfg, 0)?;
                            need(
                                encode(&raw, &info, &cfg)? == disabled,
                                "lambda0 disabled bytes differ",
                            )?;
                            count += 1;
                        }
                    }
                }
            }
        }
        imij::lab::set_fdct_mode(0);
        Ok(count)
    }
    fn restrictions() -> Result<usize> {
        let rgb = ImageInfo::new(3, 5, Channels::Rgb, SampleType::U8);
        let raw = input(&rgb, 2);
        let mut count = 0;
        for policy in [5, 255] {
            for lambda in [0.0, 0.08] {
                let mut cfg = imij::ImiEncoderConfig::lossy(30.0);
                cfg = cfg.tune_lossy(|t| {
                    t.lab.rdoq_policy = policy;
                });
                cfg = cfg.tune_lossy(|t| {
                    t.lab.rdoq_lambda = lambda;
                });
                need(encode(&raw, &rgb, &cfg).is_err(), "invalid policy accepted")?;
                count += 1;
            }
        }
        for policy in 1..=4 {
            for quality in [30.0, 90.0] {
                let mut cfg = imij::ImiEncoderConfig::lossy(quality);
                cfg = cfg.tune_lossy(|t| {
                    t.lab.rdoq_policy = policy;
                });
                cfg = cfg.tune_lossy(|t| {
                    t.lab.rdoq_lambda = 0.08;
                });
                for channels in [Channels::Rgba, Channels::Gray, Channels::GrayAlpha] {
                    let info = ImageInfo::new(3, 5, channels, SampleType::U8);
                    need(
                        encode(&input(&info, 2), &info, &cfg).is_err(),
                        "bounded alpha/gray accepted",
                    )?;
                    count += 1;
                }
                let mut unsupported = cfg.clone();
                unsupported = unsupported.tune_lossy(|t| { t.color = LossyColor::YCoCg; });
                need(
                    encode(&raw, &rgb, &unsupported).is_err(),
                    "bounded non-XYB accepted",
                )?;
                count += 1;
                let mut non_srgb = rgb.clone();
                non_srgb.cicp = imij::Cicp::BT2100_PQ;
                need(
                    encode(&raw, &non_srgb, &cfg).is_err(),
                    "bounded non-sRGB accepted",
                )?;
                count += 1;
                let mut unsupported = cfg.clone();
                unsupported = unsupported.tune_lossy(|t| { t.aq_gamma = 1.0; });
                need(encode(&raw, &rgb, &unsupported).is_err(), "AQ accepted")?;
                count += 1;
                let mut unsupported = cfg.clone();
                unsupported.lf_pred = 1;
                need(encode(&raw, &rgb, &unsupported).is_err(), "LF accepted")?;
                count += 1;
                let mut unsupported = cfg.clone();
                unsupported = unsupported.tune_lossy(|t| { t.entropy = imij::LossyEntropy::SplitHuf; });
                need(
                    encode(&raw, &rgb, &unsupported).is_err(),
                    "SplitHuf accepted",
                )?;
                count += 1;
                let high = ImageInfo::new(3, 5, Channels::Rgb, SampleType::U16);
                need(
                    encode(&input(&high, 2), &high, &cfg).is_err(),
                    "high-depth accepted",
                )?;
                count += 1;
            }
        }
        Ok(count)
    }
    fn blob(base: &Path, kind: &str, suffix: &str, raw: &[u8]) -> Result<(String, String)> {
        let digest = hash(raw)?;
        let folder = base.join(kind);
        fs::create_dir_all(&folder)?;
        let path = folder.join(format!("{digest}.{suffix}"));
        if path.exists() {
            need(
                fs::read(&path)? == raw,
                "existing content-addressed blob differs",
            )?;
        } else {
            fs::write(&path, raw)?;
        }
        Ok((path.to_string_lossy().into_owned(), digest))
    }
    fn free_space(path: &Path) -> Result<u64> {
        let out = Command::new("df")
            .args(["-B1", "--output=avail"])
            .arg(path)
            .output()?;
        need(out.status.success(), "free-space check failed")?;
        Ok(String::from_utf8(out.stdout)?
            .split_whitespace()
            .last()
            .ok_or("free space absent")?
            .parse()?)
    }
    fn exposed(spec_path: &Path, dest: &Path, blobs: &Path) -> Result<()> {
        need(
            fs::metadata(spec_path)?.len() <= 16384,
            "input spec exceeds bound",
        )?;
        let spec: Value = serde_json::from_slice(&fs::read(spec_path)?)?;
        need(
            spec["version"] == 1 && spec["exposure"] == "EXPOSED-TRAIN-EXPLORATORY",
            "spec version/exposure differs",
        )?;
        let text = |key: &str| -> Result<&str> {
            spec[key]
                .as_str()
                .ok_or_else(|| format!("spec {key} absent").into())
        };
        let number = |key: &str| -> Result<u32> {
            Ok(spec[key]
                .as_u64()
                .ok_or_else(|| format!("spec {key} absent"))?
                .try_into()?)
        };
        let width = number("width")?;
        let height = number("height")?;
        need(
            (1..=1024).contains(&width) && (1..=1024).contains(&height),
            "source dimensions exceed pilot bound",
        )?;
        let info = ImageInfo::new(width, height, Channels::Rgb, SampleType::U8);
        need(
            number("stride_bytes")? as usize == info.row_bytes()
                && spec["cicp"] == json!([1, 13, 0, 1]),
            "source format/stride differs",
        )?;
        need(
            [
                "1000", "1210", "1400", "2014", "2024", "8100", "7000", "9094",
            ]
            .contains(&text("family")?),
            "family outside frozen training roster",
        )?;
        let source = Path::new(text("source")?);
        need(
            fs::metadata(source)?.len() as usize == info.row_bytes() * height as usize,
            "source extent differs",
        )?;
        let raw = fs::read(source)?;
        need(
            hash(&raw)? == text("source_sha256")?,
            "source identity differs",
        )?;
        need(!dest.exists(), "case output must be new")?;
        fs::create_dir_all(dest)?;
        fs::create_dir_all(blobs)?;
        imij::lab::set_fdct_mode(0);
        let profiles: Vec<(u8, f32)> = core::iter::once((0, 0.0))
            .chain((1..=4).flat_map(|p| [(p, 0.04), (p, 0.08)]))
            .collect();
        let mut rows = Vec::new();
        for quality in [1, 3, 5, 10, 15, 20, 30, 40, 50, 60, 80, 100] {
            let mut baseline = None;
            for &(policy, lambda) in &profiles {
                // Preserve 2 GiB for the host and later maps, plus a cell-write allowance.
                need(
                    free_space(dest)? >= (2u64 << 30) + (32 << 20),
                    "storage reserve reached; stop before filling disk",
                )?;
                let case_id = format!("q{quality:03}-p{policy}-l{lambda:.2}");
                let config = json!({"codec":"imij","experiment":"spatial-rd-exposed-pilot-v1",
                    "quality":quality,"color":"xyb","entropy":"runlevel","compact_streams":true,
                    "channel_layouts":vec![json!({"transform":"dct8","sampling":"full"});3],
                    "matrix":255,"effective_matrix":2,"round":0.4,"effort":7.5,"threads":1,
                    "stripe_block_rows":1,"column_groups":null,"aq_gamma":0.0,"lf_pred":0,
                    "rdoq_policy":policy,"rdoq_lambda":lambda,"fdct_mode":0,
                    "decoder_memory_limit":{"max_bytes":33554432,"threads":1},"metadata":"empty"});
                let curve = if policy == 0 {
                    "imij-spatial-rd-pilot-control".into()
                } else {
                    format!("imij-spatial-rd-pilot-p{policy}-l{lambda:.2}")
                };
                let mut row = json!({"case_id":case_id,"image":text("image")?,"family":text("family")?,
                    "content_class":text("content_class")?,"source":source,"source_sha256":text("source_sha256")?,
                    "width":width,"height":height,"stride_bytes":info.row_bytes(),"cicp":[1,13,0,1],
                    "exposure":"EXPOSED-TRAIN-EXPLORATORY","codec":"imij","lossless":false,
                    "quality":quality,"curve":curve,"config":config,"metrics_scored":false,"native_timing":false});
                let result = (|| -> Result<()> {
                    let mut cfg = imij::ImiEncoderConfig::lossy(quality as f32).with_effort(7.5);
                    cfg = cfg
                        .with_decoder_memory_limit(Some(imij::DecoderMemoryLimit::new(32 << 20)));
                    cfg = cfg.tune_lossy(|t| {
                        t.lab.rdoq_lambda = lambda;
                    });
                    cfg = cfg.tune_lossy(|t| {
                        t.lab.rdoq_policy = policy;
                    });
                    let file = encode(&raw, &info, &cfg)?;
                    let (encoded, encoded_hash) = blob(blobs, "encoded", "imij", &file)?;
                    row["encoded"] = json!(encoded);
                    row["encoded_sha256"] = json!(encoded_hash);
                    row["encoded_bytes"] = json!(file.len());
                    row["bytes"] = json!(file.len());
                    if policy == 0 {
                        baseline = Some(encoded_hash.clone());
                    } else if quality > 80 {
                        need(
                            baseline.as_ref() == Some(&encoded_hash),
                            "high-q bypass differs from control",
                        )?;
                    }
                    let decode_policy = imij::decode_resources::DecodePolicy::default()
                        .with_max_working_bytes(32 << 20);
                    let mut session = imij::decode_resources::DecodeSession::new(decode_policy);
                    let (decoded_info, decoded) = session.decode(&file)?;
                    need(decoded_info == info, "budgeted decoded info differs")?;
                    need(decoded.len() == raw.len(), "decoded extent differs")?;
                    apis(&file, &info)?;
                    let (_, ordinary) = imij::decode(&file)?;
                    need(ordinary == decoded, "budgeted/ordinary decode differs")?;
                    need(
                        session.usage().peak_planned_working_bytes <= 32 << 20,
                        "runtime decode budget exceeded",
                    )?;
                    let (decoded_path, decoded_hash) = blob(blobs, "decoded", "rgb8", &decoded)?;
                    row["decoded"] = json!(decoded_path);
                    row["decoded_sha256"] = json!(decoded_hash);
                    row["decoded_bytes"] = json!(decoded.len());
                    row["decode_usage"] = serde_json::json!({
                        "peak_planned_working_bytes":session.usage().peak_planned_working_bytes,
                        "input_bytes":session.usage().input_bytes,"units":session.usage().units,
                        "work_units":session.usage().work_units});
                    row["parity"] = json!({"allocating":true,"padded_threads_1_4":true,
                        "stream":true,"reader":true,"serial_budget32MiB":true,
                        "q100_control_byte_identity":if quality>80 {Some(true)}else{None}});
                    Ok(())
                })();
                match result {
                    Ok(()) => row["status"] = json!("ok"),
                    Err(error) => {
                        row["status"] = json!("failed");
                        row["error"] = json!(error.to_string());
                    }
                }
                fs::write(
                    dest.join(format!("{case_id}.json")),
                    serde_json::to_vec_pretty(&row)?,
                )?;
                rows.push(row);
            }
            fs::write(dest.join("rows.json"), serde_json::to_vec_pretty(&rows)?)?;
            println!(
                "{} q{quality}: {} retained cells",
                text("family")?,
                rows.len()
            );
        }
        need(rows.len() == 108, "source cell grid differs")?;
        need(
            rows.iter().all(|r| r["status"] == "ok"),
            "source has retained failed cells",
        )
    }
    pub fn run() -> Result<()> {
        let args: Vec<_> = std::env::args().collect();
        need(
            args.len() >= 3,
            "spatial_rd_probe legacy NEW_DIRECTORY | verify PRECHANGE_DIRECTORY NEW_DIRECTORY",
        )?;
        imij::lab::set_fdct_mode(0);
        if args[1] == "encode-exposed" {
            need(
                args.len() == 5,
                "encode-exposed SPEC NEW_CASE_DIRECTORY OWNED_BLOB_DIRECTORY",
            )?;
            return exposed(
                Path::new(&args[2]),
                Path::new(&args[3]),
                Path::new(&args[4]),
            );
        }
        let (rows, count, rejected) = match args[1].as_str() {
            "legacy" => {
                need(args.len() == 3, "legacy arguments differ")?;
                (golden(Path::new(&args[2]), None)?, 0, 0)
            }
            "verify" => {
                need(args.len() == 4, "verify arguments differ")?;
                let rows = golden(Path::new(&args[3]), Some(Path::new(&args[2])))?;
                (rows, bounded()?, restrictions()?)
            }
            _ => return Err("unknown action; corpus encoding/scoring not admitted".into()),
        };
        let dest = Path::new(args.last().unwrap());
        fs::write(
            dest.join("validation.json"),
            serde_json::to_vec_pretty(
                &json!({"golden":rows,"bounded_cases":count,"rejected_cases":rejected,"timing":false,"corpus_scoring":false}),
            )?,
        )?;
        Ok(())
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "lab")]
    {
        probe::run()
    }
    #[cfg(not(feature = "lab"))]
    {
        Err("spatial_rd_probe requires lab feature".into())
    }
}
