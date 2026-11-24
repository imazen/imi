//! Single-cell persisted RGB8 operations; rd.py owns resumability/provenance.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::Path, time::Instant};
pub(crate) type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub(crate) fn hash(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}
pub(crate) fn save(path: &Path, value: &Value) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}
pub(crate) fn check_rgb(p: &[u8], w: u32, h: u32) -> Result<()> {
    let n = (w as usize)
        .checked_mul(h as usize)
        .and_then(|n| n.checked_mul(3))
        .ok_or("RGB dimensions overflow")?;
    if w == 0 || h == 0 || p.len() != n {
        return Err("expected tight RGB8 width*height*3 bytes".into());
    }
    Ok(())
}

/// The raw-input layout of an IMI cell (`input` key; "rgb8" when absent).
/// The split-bound study (2026-10-10) added the channel/sample variants:
/// "gray" (1 byte per pixel), "rgba" (4), "u16" (RGB, native-endian, 2 bytes
/// per sample) and "f32" (RGB, native-endian, linear light, 4). The PNG
/// ladder path converts a decoded PNG into the cell's layout; the
/// `encode-rgb` raw path expects the bytes already in it.
pub(crate) fn input_key(cfg: &Value) -> Result<&str> {
    Ok(cfg.get("input").and_then(Value::as_str).unwrap_or("rgb8"))
}

/// `(channels, sample, bytes per sample)` of an `input` key.
pub(crate) fn input_layout(input: &str) -> Result<(imij::Channels, imij::SampleType, usize)> {
    Ok(match input {
        "rgb8" => (imij::Channels::Rgb, imij::SampleType::U8, 1),
        "gray" => (imij::Channels::Gray, imij::SampleType::U8, 1),
        "rgba" => (imij::Channels::Rgba, imij::SampleType::U8, 1),
        "u16" => (imij::Channels::Rgb, imij::SampleType::U16, 2),
        "f32" => (imij::Channels::Rgb, imij::SampleType::F32, 4),
        _ => return Err(format!("unknown input layout {input:?}").into()),
    })
}

/// The `ImageInfo` an IMI cell encodes: layout from `input`, colour
/// metadata from `transfer` ("srgb" default; "pq" = BT.2100 PQ). `transfer`
/// also names the source PNG's encoding for the ladder's `f32` conversion
/// (which requires "pq": the PQ EOTF). Linear f32 input is tagged linear
/// (transfer 8); the encoder codes it as linear-light XYB.
pub(crate) fn input_info(cfg: &Value, w: u32, h: u32) -> Result<imij::ImageInfo> {
    let input = input_key(cfg)?;
    let (channels, sample, _) = input_layout(input)?;
    let mut info = imij::ImageInfo::new(w, h, channels, sample);
    if input == "u16" && cfg.get("transfer").and_then(Value::as_str) == Some("pq") {
        info.cicp = imij::Cicp::BT2100_PQ;
    }
    if input == "f32" {
        info.cicp = imij::Cicp {
            color_primaries: 1,
            transfer_characteristics: 8,
            matrix_coefficients: 0,
            full_range: true,
        };
    }
    Ok(info)
}

/// Tight-length check for one cell's raw input bytes.
pub(crate) fn check_input(p: &[u8], w: u32, h: u32, cfg: &Value) -> Result<()> {
    let input = input_key(cfg)?;
    let (channels, _, bps) = input_layout(input)?;
    let n = (w as usize)
        .checked_mul(h as usize)
        .and_then(|n| n.checked_mul(channels.count()))
        .and_then(|n| n.checked_mul(bps))
        .ok_or("input dimensions overflow")?;
    if w == 0 || h == 0 || p.len() != n {
        return Err(format!(
            "expected tight {input} width*height*{}x{bps} bytes",
            channels.count()
        )
        .into());
    }
    Ok(())
}
pub(crate) fn command(a: &[String]) -> Result<()> {
    if a[1] == "score-rgb-batch" && a.len() == 3 {
        return score_batch(Path::new(&a[2]));
    }
    if a[1] == "eval-batch" && (a.len() == 7 || (a.len() == 8 && a[7] == "--maps")) {
        let source = Path::new(&a[2]);
        let w = a[3].parse()?;
        let h = a[4].parse()?;
        let jobs: Vec<Value> = serde_json::from_slice(&fs::read(&a[5])?)?;
        let scores_root = Path::new(&a[6]);
        fs::create_dir_all(scores_root)?;
        let mut scorer = crate::quality::WarmScorer::new(source, w, h)?;
        let mut failures = 0;
        for job in jobs {
            let result = (|| -> Result<()> {
                let output = Path::new(job["output"].as_str().ok_or("output missing")?);
                let config = Path::new(job["config"].as_str().ok_or("config missing")?);
                let cfg: Value = serde_json::from_slice(&fs::read(config)?)?;
                validate_config(&cfg)?;
                if !output.join("case.json").exists() {
                    encode(source, w, h, config, output)?;
                }
                let row: Value = serde_json::from_slice(&fs::read(output.join("case.json"))?)?;
                check_cached_case(&row, &cfg, w, h, scorer.source_sha256())?;
                let mut score_key = json!({"decoded":row["decoded_sha256"],"height":h,"source":row["source_sha256"],"width":w});
                // A scalar cache cannot satisfy a map request; keep it intact
                // and create a separate diagnostic cache with maps enabled.
                if a.len() == 8 {
                    score_key["maps"] = json!(true);
                }
                let score_id = hash(serde_json::to_string(&score_key)?.as_bytes());
                let score_dir = scores_root.join(score_id);
                if !score_dir.join("scores.json").exists() {
                    scorer.score(
                        source,
                        Path::new(row["decoded"].as_str().ok_or("decoded missing")?),
                        &score_dir,
                        a.len() == 8,
                    )?;
                }
                Ok(())
            })();
            if let Err(e) = result {
                eprintln!("failed {}: {e}", job["output"]);
                failures += 1;
            } else {
                eprintln!("complete {}", job["output"]);
            }
        }
        return if failures == 0 {
            Ok(())
        } else {
            Err(format!("{failures} batch failures").into())
        };
    }
    match a[1].as_str() {
        // The split-bound study (2026-10-10): print the AC entropy-table
        // section of one or more lossy IMI files (`mode<TAB>bytes<TAB>path`),
        // parsed from the file's own grammar.
        "table-bytes" if a.len() >= 3 => {
            for p in &a[2..] {
                let data = fs::read(p)?;
                let (mode, bytes) = imij::lab::lossy_ac_table_bytes(&data)?;
                println!("{mode}\t{bytes}\t{p}");
            }
            Ok(())
        }
        "source-rgb" if a.len() == 5 => source(Path::new(&a[2]), Path::new(&a[3]), Path::new(&a[4])),
        "redecode-case" if a.len() == 3 => redecode_case(Path::new(&a[2])),
        "decode-rgb" if a.len() == 7 => decode_file(&a[2], Path::new(&a[3]), a[4].parse()?, a[5].parse()?, Path::new(&a[6])),
        "encode-rgb" if a.len() == 7 => encode(Path::new(&a[2]), a[3].parse()?, a[4].parse()?, Path::new(&a[5]), Path::new(&a[6])),
        "score-rgb" if a.len() == 7 || (a.len() == 8 && a[7] == "--maps") => crate::quality::score(Path::new(&a[2]), Path::new(&a[3]), a[4].parse()?, a[5].parse()?, Path::new(&a[6]), a.len() == 8),
        _ => Err("source-rgb PNG CROP_JSON OUTDIR | encode-rgb SOURCE W H CONFIG_JSON OUTDIR | score-rgb SOURCE DECODED W H OUTDIR [--maps]".into())
    }
}
fn source(png: &Path, crop: &Path, output: &Path) -> Result<()> {
    if output.exists() {
        return Err("source output already exists".into());
    }
    let spec: Value = serde_json::from_slice(&fs::read(crop)?)?;
    let data = fs::read(png)?;
    if spec["sha256"].as_str() != Some(hash(&data).as_str()) {
        return Err("corpus PNG hash mismatch".into());
    }
    let image = zenpng::decode(
        &data,
        &zenpng::PngDecodeConfig::default(),
        &enough::Unstoppable,
    )?;
    if image.info.icc_profile.is_some() {
        return Err("source-rgb requires ICC-free sRGB input; create an explicitly color-managed supplemental rendition".into());
    }
    if image.info.cicp.is_some_and(|c| {
        c.color_primaries != 1
            || c.transfer_characteristics != 13
            || c.matrix_coefficients != 0
            || !c.full_range
    }) {
        return Err("source-rgb refuses non-sRGB CICP".into());
    }
    let pixels = image.pixels.copy_to_contiguous_bytes();
    let (sw, sh) = (image.info.width, image.info.height);
    if image.info.bit_depth != 8 {
        return Err("requires RGB8 PNG".into());
    }
    check_rgb(&pixels, sw, sh)?;
    let get = |k: &str| -> Result<u32> {
        Ok(u32::try_from(
            spec["crop"][k].as_u64().ok_or("missing crop coordinate")?,
        )?)
    };
    let (x, y, w, h) = (get("x")?, get("y")?, get("width")?, get("height")?);
    if x.checked_add(w).is_none_or(|v| v > sw) || y.checked_add(h).is_none_or(|v| v > sh) {
        return Err("crop outside source".into());
    }
    let mut raw = Vec::new();
    for row in y..y + h {
        let offset = (row as usize * sw as usize + x as usize) * 3;
        raw.extend_from_slice(&pixels[offset..offset + w as usize * 3]);
    }
    check_rgb(&raw, w, h)?;
    fs::create_dir_all(output)?;
    fs::write(output.join("source.rgb8"), &raw)?;
    save(
        &output.join("source.json"),
        &json!({"width":w,"height":h,"stride_bytes":w*3,"source":output.join("source.rgb8"),"source_sha256":hash(&raw),"png_sha256":hash(&data),"corpus_entry":spec}),
    )
}
fn channel_layouts(cfg: &Value) -> Result<[imij::LossyChannelLayout; 3]> {
    let values = cfg["channel_layouts"]
        .as_array()
        .ok_or("channel_layouts array required")?;
    if values.len() != 3 {
        return Err("three channel layouts required".into());
    }
    let mut out = [imij::LossyChannelLayout::default(); 3];
    for (dst, v) in out.iter_mut().zip(values) {
        let object = v.as_object().ok_or("channel layout must be an object")?;
        if object.len() != 2 {
            return Err("channel layout requires only transform and sampling".into());
        }
        let transform = match v["transform"].as_str() {
            Some("8") => imij::LossyTransform::Dct8,
            Some("4") => imij::LossyTransform::Dct4,
            Some("2") => imij::LossyTransform::Dct2,
            Some("adaptive4") => imij::LossyTransform::Adaptive4,
            _ => return Err("invalid transform".into()),
        };
        let sampling = match v["sampling"].as_str() {
            Some("full") => imij::ChannelSampling::Full,
            Some("half-x") => imij::ChannelSampling::HalfHorizontal,
            Some("half-y") => imij::ChannelSampling::HalfVertical,
            Some("half-xy") => imij::ChannelSampling::HalfBoth,
            _ => return Err("invalid sampling".into()),
        };
        *dst = imij::LossyChannelLayout::new(transform, sampling);
    }
    Ok(out)
}

// Pinned jxl-encoder 2b13266 supports an explicit EPF override. Its api.rs
// still forces Gaborish off at effective distance <= 0.5, even when explicitly
// enabled; validation therefore restricts both ablation arms to active distances.
const JXL_ABLATION_EPF_LEVEL: i8 = 2;

fn jxl_config(cfg: &Value, distance: f32) -> Result<jxl_encoder::LossyConfig> {
    let c = jxl_encoder::LossyConfig::new(distance).with_effort(7);
    if cfg.get("experiment").is_none() {
        // Preserve the historical builder and automatic EPF policy exactly.
        return Ok(c);
    }
    Ok(c.with_gaborish(
        cfg["gaborish"]
            .as_bool()
            .ok_or("gaborish boolean required")?,
    )
    .with_epf_level(JXL_ABLATION_EPF_LEVEL))
}

fn check_cached_case(row: &Value, cfg: &Value, w: u32, h: u32, source_hash: &str) -> Result<()> {
    if row["config"] != *cfg
        || row["width"].as_u64() != Some(u64::from(w))
        || row["height"].as_u64() != Some(u64::from(h))
        || row["source_sha256"].as_str() != Some(source_hash)
    {
        return Err("cached case configuration/source/geometry differs from requested job".into());
    }
    Ok(())
}

fn encode(source: &Path, w: u32, h: u32, config: &Path, output: &Path) -> Result<()> {
    if output.exists() {
        return Err("encode output already exists".into());
    }
    let mut raw = fs::read(source)?;
    let cfg: Value = serde_json::from_slice(&fs::read(config)?)?;
    validate_config(&cfg)?;
    let (mut w, mut h) = (w, h);
    // `crop` (the PNG ladder path applies it to the decoded PNG): a centered
    // n x n window of the raw input, in the input's own layout.
    if let Some(n) = cfg.get("crop").and_then(Value::as_u64) {
        let n = u32::try_from(n).map_err(|_| "crop out of range")?;
        let (channels, _, bps) = input_layout(input_key(&cfg)?)?;
        raw = center_crop(&raw, w, h, channels.count(), bps, n)?;
        (w, h) = (n, n);
    }
    check_input(&raw, w, h, &cfg)?;
    let codec = cfg["codec"].as_str().ok_or("codec missing")?;
    let q = cfg["quality"].as_f64().ok_or("quality missing")? as f32;
    if !q.is_finite() {
        return Err("nonfinite quality".into());
    }
    let start = Instant::now();
    // The split-revisit study (2026-10-10): sharded harnesses run one encode per process
    // with `IMIJ_ENCODE_THREADS=1`; bytes do not depend on the thread count.
    let enc_threads = std::env::var("IMIJ_ENCODE_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6);
    let (data, inner_jpeg_sha256) = encode_cell(&cfg, &raw, w, h, enc_threads)?;
    let enc_s = start.elapsed().as_secs_f64();
    let encoded_ext = if codec == "jpeg-jxl" { "jxl" } else { codec };
    fs::create_dir_all(output)?;
    let encoded = output.join(format!("encoded.{encoded_ext}"));
    fs::write(&encoded, &data)?;
    let decoded = decode_cell(&cfg, &data, w, h)?;
    let path = output.join(if input_key(&cfg)? == "rgb8" {
        "decoded.rgb8"
    } else {
        "decoded.bin"
    });
    fs::write(&path, &decoded)?;
    let (channels, _, bps) = input_layout(input_key(&cfg)?)?;
    let mut case = json!({"source":source,"decoded":path,"encoded":encoded,"encoded_bytes":data.len(),"source_sha256":hash(&raw),"decoded_sha256":hash(&decoded),"encoded_sha256":hash(&data),"width":w,"height":h,"stride_bytes":w as usize * channels.count() * bps,"quality":q,"config":cfg,"encode_wall_seconds_diagnostic":enc_s,"geometry":{"stripe_rows_pixels":cfg["stripe_block_rows"].as_u64().map(|n|n*8),"column_groups":"auto; none at corpus widths <=1024"}});
    if let Some(sha) = inner_jpeg_sha256 {
        case["inner_jpeg_sha256"] = json!(sha);
    }
    save(&output.join("case.json"), &case)
}

/// Centered `n x n` window of tight rows (`ch` channels, `bps` bytes per
/// sample); errors when `n` exceeds either dimension (split-bound study,
/// 2026-10-10: typed cells crop genuine high-bit sources instead of
/// resampling).
pub(crate) fn center_crop(
    p: &[u8],
    w: u32,
    h: u32,
    ch: usize,
    bps: usize,
    n: u32,
) -> Result<Vec<u8>> {
    if n == 0 || n > w || n > h {
        return Err(format!("crop {n} does not fit {w}x{h}").into());
    }
    let row = w as usize * ch * bps;
    let keep = n as usize * ch * bps;
    let x0 = (w as usize - n as usize) / 2 * ch * bps;
    let y0 = (h as usize - n as usize) / 2;
    let mut out = Vec::with_capacity(keep * n as usize);
    for r in 0..n as usize {
        let o = (y0 + r) * row + x0;
        out.extend_from_slice(&p[o..o + keep]);
    }
    Ok(out)
}

/// Decodes one cell's file for the pixel-identity check (`decoded_sha256`).
/// IMI cells of a non-RGB8 input decode to that layout; all other codecs
/// stay RGB8.
pub(crate) fn decode_cell(cfg: &Value, data: &[u8], w: u32, h: u32) -> Result<Vec<u8>> {
    if cfg["codec"].as_str() == Some("imij") && input_key(cfg)? != "rgb8" {
        let (header, p) = imij::decode(data)?;
        if header.width != w || header.height != h {
            return Err("IMIJ dimensions mismatch".into());
        }
        let (channels, _, bps) = input_layout(input_key(cfg)?)?;
        if p.len() != w as usize * h as usize * channels.count() * bps {
            return Err("IMIJ decoded layout mismatch".into());
        }
        return Ok(p);
    }
    decode_rgb(decode_codec(cfg)?, data, w, h)
}

/// Encodes one validated cell configuration. `imij_threads`: IMI encoder
/// threads (bytes do not depend on it); the study cells use 6.
pub(crate) fn encode_cell(
    cfg: &Value,
    raw: &[u8],
    w: u32,
    h: u32,
    imij_threads: usize,
) -> Result<(Vec<u8>, Option<String>)> {
    let codec = cfg["codec"].as_str().ok_or("codec missing")?;
    let q = cfg["quality"].as_f64().ok_or("quality missing")? as f32;
    if !q.is_finite() {
        return Err("nonfinite quality".into());
    }
    let mut inner_jpeg_sha256 = None;
    let data = match codec {
        "imij" => {
            // Lossy 0.4 / ycocgq study's `u16` knob codes the RGB8 source
            // widened x257 as U16 YCoCg (the 16-bit non-transfer path); the
            // split-bound typed inputs bring their own layout through
            // `input`, and validation excludes the two.
            let mut widened = None;
            let (info, stride) = if cfg["u16"].as_bool() == Some(true) {
                check_rgb(raw, w, h)?;
                widened = Some(
                    raw.iter()
                        .flat_map(|&v| (u16::from(v) * 257).to_ne_bytes())
                        .collect::<Vec<u8>>(),
                );
                (
                    imij::ImageInfo::new(w, h, imij::Channels::Rgb, imij::SampleType::U16),
                    w as usize * 6,
                )
            } else {
                let info = input_info(cfg, w, h)?;
                check_input(raw, w, h, cfg)?;
                let stride = w as usize * info.channels.count() * info.sample.bytes();
                (info, stride)
            };
            let pixels: &[u8] = widened.as_deref().unwrap_or(raw);
            let color = match cfg["color"].as_str() {
                Some("xyb") => imij::LossyColor::Xyb,
                Some("ycocg") => imij::LossyColor::YCoCg,
                _ => return Err("invalid color".into()),
            };
            let entropy = match cfg["entropy"].as_str() {
                Some("runlevel") => imij::LossyEntropy::RunLevel,
                Some("splithuf") => imij::LossyEntropy::SplitHuf,
                // The decode-speed tier picks the coding (encoder ablations).
                Some("tier") => imij::LossyEntropy::RunLevel,
                _ => return Err("invalid entropy".into()),
            };
            let effort = cfg["effort"].as_f64().ok_or("effort missing")? as f32;
            let mut c = LossyCell::new(
                imij::ImiEncoderConfig::lossy(q)
                    .with_effort(effort)
                    .with_threads(imij_threads),
            );
            c.tuning.color = color;
            c.tuning.entropy = entropy;
            c.tuning.stripe_block_rows = u32::try_from(
                cfg["stripe_block_rows"]
                    .as_u64()
                    .ok_or("stripe rows missing")?,
            )?;
            c.tuning.lab.rdoq_lambda = cfg["rdoq_lambda"].as_f64().ok_or("lambda missing")? as f32;
            // Lab switches travel with this encode (`LossyConfig::lab`), every
            // one set (at production behaviour unless the cell says otherwise),
            // so cells encoding on other threads at the same time never see
            // them. Never call `imij::lab::set_*` here: those are process-global
            // (benchmarks/raceaudit_2026-10-09; the `no_process_global_lab_switches`
            // test).
            c.tuning.lab.switches = imij::LossyLabSwitches::defaults();
            if cfg["experiment"] == "encoder-ablation" {
                let (dc_step, dering, dc_nearest) = ablation(cfg)?;
                c.tuning.lab.switches.dc_step = Some(dc_step);
                c.tuning.lab.switches.dering = Some(dering);
                c.tuning.lab.switches.dc_nearest = Some(dc_nearest);
                c.tuning.lab.trellis = trellis_knob(cfg)?;
                aq_knobs(cfg, &mut c)?;
            }
            if cfg["experiment"] == "subblocks-v2" {
                c.tuning.channel_layouts = channel_layouts(cfg)?;
                c.tuning.compact_streams = cfg["compact_streams"]
                    .as_bool()
                    .ok_or("compact_streams required")?;
            }
            let data = c.finish().encode_samples(
                imij::ImageRef::new(pixels, stride, &info),
                &imij::Metadata::default(),
            )?;
            // The split-revisit study (2026-10-10): `IMIJ_CELL_SIDE_STATS=1` prints the
            // encode's entropy-work counts (one JSON line per cell) for the
            // E17-form decode-cost model refit. The split-bound study
            // (2026-10-10) adds the written AC table bytes (`lossy_table_stats`;
            // consumed here: `ladder-batch`'s IMIJ_LADDER_TABLE_STATS must not
            // be combined with this env).
            if std::env::var_os("IMIJ_CELL_SIDE_STATS").is_some() {
                let [tokens, groups, coded_bc] = imij::lab::lossy_side_stats();
                let [table_mode, ac_table_bytes] = imij::lab::lossy_table_stats();
                eprintln!(
                    "side_stats {}",
                    serde_json::json!({
                        "tokens": tokens, "groups": groups, "coded_bc": coded_bc,
                        "table_mode": table_mode, "ac_table_bytes": ac_table_bytes,
                    })
                );
            }
            data
        }
        "jpeg" => zenjpeg_encode(cfg, q, w, h, raw)?,
        // Plain jpegli: jpegli tables and AQ, no trellis ("hybrid") at any q.
        "jpegli" => zenjpeg_encode(cfg, q, w, h, raw)?,
        // The same zenjpeg file, losslessly transcoded to a bare JPEG XL
        // codestream (no JPEG reconstruction box): JPEG's pixels at JPEG
        // XL's entropy coding.
        "jpeg-jxl" => {
            let jpeg = zenjpeg_encode(cfg, q, w, h, raw)?;
            inner_jpeg_sha256 = Some(hash(&jpeg));
            jxl_encoder::LosslessConfig::new()
                .with_effort(9)
                .encode_jpeg_transcode_codestream(&jpeg)?
        }
        "jxl" => jxl_config(cfg, q)?.encode(raw, w, h, jxl_encoder::PixelLayout::Rgb8)?,
        "avif" => {
            let sub = match cfg["chroma"].as_str() {
                Some("444") => zenavif::EncodeChromaSubsampling::Yuv444,
                Some("420") => zenavif::EncodeChromaSubsampling::Yuv420,
                _ => return Err("invalid chroma".into()),
            };
            let px = raw
                .as_chunks::<3>()
                .0
                .iter()
                .map(|p| rgb::RGB8::new(p[0], p[1], p[2]))
                .collect::<Vec<_>>();
            let c = zenavif::EncoderConfig::new()
                .backend(zenavif::Av1Backend::Zenav1Svt)
                .quality(q)
                .speed(6)
                .threads(Some(6))
                .chroma_subsampling(sub);
            zenavif::encode_rgb8(
                imgref::Img::new(px, w as usize, h as usize).as_ref(),
                &c,
                almost_enough::StopToken::new(enough::Unstoppable),
            )?
            .avif_file
        }
        _ => return Err("unknown codec".into()),
    };
    Ok((data, inner_jpeg_sha256))
}

/// zenjpeg as configured for the study: YCbCr, progressive, auto_optimize
/// (conditional AC trellis), default jpegli AQ, ApproxJpegli quality.
fn zenjpeg_encode(cfg: &Value, q: f32, w: u32, h: u32, raw: &[u8]) -> Result<Vec<u8>> {
    use zenjpeg::encoder::{ChromaSubsampling, EncoderConfig, PixelLayout, Unstoppable};
    let sub = match cfg["chroma"].as_str() {
        Some("444") => ChromaSubsampling::None,
        Some("420") => ChromaSubsampling::Quarter,
        _ => return Err("invalid chroma".into()),
    };
    let c = EncoderConfig::ycbcr(q, sub)
        .progressive(true)
        .auto_optimize(cfg["auto_optimize"] == true);
    let mut enc = c.encode_from_bytes(w, h, PixelLayout::Rgb8Srgb)?;
    enc.push_packed(raw, Unstoppable)?;
    Ok(enc.finish()?)
}

/// Encoder ablation knobs (lab): `dc_step` [luma, chroma] DC step multipliers
/// in 1/64 units (0 = default), `dering` and `dc_nearest` (defaults true).
fn ablation(cfg: &Value) -> Result<([u8; 2], bool, bool)> {
    let steps = cfg["dc_step"].as_array().ok_or("dc_step required")?;
    if steps.len() != 2 {
        return Err("dc_step must be [luma, chroma]".into());
    }
    let step = |v: &Value| -> Result<u8> {
        Ok(u8::try_from(v.as_u64().ok_or("dc_step must be integers")?)?)
    };
    Ok((
        [step(&steps[0])?, step(&steps[1])?],
        cfg["dering"].as_bool().ok_or("dering required")?,
        cfg["dc_nearest"].as_bool().ok_or("dc_nearest required")?,
    ))
}

/// AQ, tier, dequantization, exact-block and superblock knob keys
/// (encoder-ablation only).
const AQ_KEYS: [&str; 32] = [
    "trellis_rate",
    "trellis_channel_shift",
    "aq_field_strength",
    "aq_field_levels",
    "aq_field_cell",
    "aq_field_relative",
    "aq_field_fade",
    "trellis_aq_exp",
    "aq_gamma",
    "aq_r0",
    "decode_speed",
    "exact_blocks",
    "aq_note",
    "table_k_search",
    "table_k_mode",
    "matrix",
    "channel_mul",
    "explicit_matrix",
    "recon_bias",
    "quant_round",
    "u16",
    "color_transform",
    "superblocks",
    "superblock_shapes",
    "small_transforms",
    "small_split",
    "small_lambda",
    "small_edge",
    "high_quality",
    "r19_transform",
    "small_int",
    "seed_bank",
];

/// Encoder ablation AQ knobs (lab, 2026-10-09 AQ + trellis study):
/// `aq_field_strength` (0..=4; present = jpegli's field as AQ step map,
/// with `aq_field_levels` 0..=16, `aq_field_cell` 1..=8, `aq_field_relative`
/// and `aq_field_fade` required); `trellis_aq_exp` (-4..=4, the trellis's
/// lambda factor m^k per AQ multiplier, `LossyLabSwitches`);
/// `aq_gamma` / `aq_r0` (the activity rule, both or neither);
/// `decode_speed` (compact, balanced, fast, fastest; requires entropy
/// "tier" and the tier's color); `exact_blocks` (ratio > 0, or 0 = off);
/// `trellis_rate` (absent: the encoder's default, which since the
/// split-Huffman rate model's adoption runs the effort's trellis on XYB
/// split-Huffman files too; "huf": that, explicitly, requires entropy
/// "splithuf" or "tier"; "none": split-Huffman files without the trellis,
/// the default of builds before it; `LossyLabSwitches::trellis_huf_rate`);
/// `trellis_channel_shift` (three f32-exact numbers in -8..=8: per-channel
/// lambda log scale 1 offsets, `LossyLabSwitches::trellis_channel_shift`);
/// `superblocks` (bool; absent = the encoder's rule: the Compact tier from
/// 128 px, `LossyLabSwitches::superblocks`) and `superblock_shapes` (1..=7,
/// the shapes the selector may merge: bit 0 16x16, 1 16x8, 2 8x16);
/// `small_transforms` (bool; absent = the encoder's rule,
/// `SmallLab::on`) and `small_split` (0..=7, with
/// small transforms on: the planes coded 4x4 in every eligible block, bit c =
/// plane c; absent = the selector decides); `small_lambda` (0..=16) and
/// `small_edge` (0..=64), with small transforms on: the selector's
/// distortion and inner-edge weights for the 8x8 / 4x4 choice;
/// `seed_bank` (0: no seed tables, 1: bank 1, the default; `LossyConfig::
/// seed_bank`); `small_int` (true, with small transforms on: the lab integer 4x4
/// candidate, `SmallLab::int`); `r19_transform` ("2", "4", "adaptive4": every plane's record-19 lab
/// layout at full sampling, the small-transform study's 2x2 and record-19
/// control arms; the trellis is off with them); `high_quality` (true:
/// quality up
/// to 116, the near-lossless steps above 100, for the top of the RD range).
/// The switches go in the cell's own config (`LossyConfig::lab`).
/// Validates without a config when `c` is `None`.
fn aq_knobs_check(cfg: &Value, mut c: Option<&mut LossyCell>) -> Result<()> {
    let num = |k: &str, lo: f64, hi: f64| -> Result<Option<f64>> {
        match cfg.get(k) {
            None => Ok(None),
            Some(v) => {
                let x = v.as_f64().ok_or(format!("{k} must be a number"))?;
                if !(lo..=hi).contains(&x) || x as f32 as f64 != x {
                    return Err(format!("{k} out of range or not f32-exact").into());
                }
                Ok(Some(x))
            }
        }
    };
    if let Some(strength) = num("aq_field_strength", 0.0, 4.0)? {
        let levels = num("aq_field_levels", 0.0, 16.0)?.ok_or("aq_field_levels required")?;
        let cell = num("aq_field_cell", 1.0, 8.0)?.ok_or("aq_field_cell required")?;
        let relative = cfg["aq_field_relative"]
            .as_bool()
            .ok_or("aq_field_relative required")?;
        let fade = cfg["aq_field_fade"]
            .as_bool()
            .ok_or("aq_field_fade required")?;
        if levels.fract() != 0.0 || cell.fract() != 0.0 {
            return Err("aq_field_levels and aq_field_cell must be integers".into());
        }
        if let Some(c) = c.as_deref_mut() {
            c.tuning.lab.aq_field = Some(imij::LossyAqField {
                strength: strength as f32,
                levels: levels as u8,
                cell: cell as u8,
                relative,
                fade,
            });
        }
    } else if [
        "aq_field_levels",
        "aq_field_cell",
        "aq_field_relative",
        "aq_field_fade",
    ]
    .iter()
    .any(|k| cfg.get(*k).is_some())
    {
        return Err("aq_field_* options require aq_field_strength".into());
    }
    if let Some(k) = num("trellis_aq_exp", -4.0, 4.0)?
        && let Some(c) = c.as_deref_mut()
    {
        c.tuning.lab.switches.trellis_aq_lambda_exp = Some(k as f32);
    }
    match (num("aq_gamma", -4.0, 4.0)?, num("aq_r0", 0.0, 64.0)?) {
        (Some(g), Some(r0)) => {
            if cfg.get("aq_field_strength").is_some() {
                return Err("aq_gamma excludes aq_field".into());
            }
            if let Some(c) = c.as_deref_mut() {
                c.tuning.aq_gamma = g as f32;
                c.tuning.aq_r0 = r0 as f32;
            }
        }
        (None, None) => {}
        _ => return Err("aq_gamma and aq_r0 go together".into()),
    }
    let tier = match cfg.get("decode_speed").map(Value::as_str) {
        None => None,
        Some(Some("compact")) => Some(imij::DecodeSpeed::Compact),
        Some(Some("balanced")) => Some(imij::DecodeSpeed::Balanced),
        Some(Some("fast")) => Some(imij::DecodeSpeed::Fast),
        Some(Some("fastest")) => Some(imij::DecodeSpeed::Fastest),
        _ => return Err("invalid decode_speed".into()),
    };
    if tier.is_some() != (cfg["entropy"] == "tier") {
        return Err("entropy \"tier\" goes with decode_speed".into());
    }
    if let Some(t) = tier {
        let ycocg = t == imij::DecodeSpeed::Fastest;
        if (cfg["color"] == "ycocg") != ycocg {
            return Err("the color must be the tier's (YCoCg for fastest only)".into());
        }
        if let Some(c) = c.as_deref_mut() {
            c.config = c.config.clone().with_decode_speed(t);
        }
    }
    if let Some(r) = num("exact_blocks", 0.0, 64.0)?
        && let Some(c) = c.as_deref_mut()
    {
        c.tuning.exact_blocks = Some(r as f32);
    }
    match cfg.get("trellis_rate").map(Value::as_str) {
        None => {}
        Some(Some("none")) => {
            if let Some(c) = c.as_deref_mut() {
                c.tuning.lab.switches.trellis_huf_rate = Some(false);
            }
        }
        Some(Some("huf")) => {
            if cfg["entropy"] != "splithuf" && cfg["entropy"] != "tier" {
                return Err(
                    "trellis_rate \"huf\" requires entropy \"splithuf\" or \"tier\"".into(),
                );
            }
            if let Some(c) = c.as_deref_mut() {
                c.tuning.lab.switches.trellis_huf_rate = Some(true);
            }
        }
        _ => return Err("trellis_rate must be huf or none".into()),
    }
    // `table_k_search` (bool) / `table_k_mode` (0..=3), coefgap study
    // 2026-10-09: overrides how the encoder picks its table counts
    // (`LossyLabSwitches::table_k_mode`; true = mode 1, 0 = the effort's
    // budgets; absent = the production rule).
    let mode = match (cfg.get("table_k_search"), cfg.get("table_k_mode")) {
        (None, None) => None,
        (Some(v), None) => Some(v.as_bool().ok_or("table_k_search must be a bool")? as u8),
        (None, Some(v)) => Some(
            v.as_u64()
                .filter(|m| *m <= 3)
                .ok_or("table_k_mode must be 0..=3")? as u8,
        ),
        _ => return Err("table_k_search and table_k_mode exclude each other".into()),
    };
    if let (Some(m), Some(c)) = (mode, c.as_deref_mut()) {
        c.tuning.lab.switches.table_k_mode = Some(m);
    }
    if let Some(v) = cfg.get("superblocks") {
        let on = v.as_bool().ok_or("superblocks must be a bool")?;
        if let Some(c) = c.as_deref_mut() {
            c.tuning.lab.switches.superblocks = Some(on);
        }
    }
    if let Some(v) = cfg.get("superblock_shapes") {
        let m = v
            .as_u64()
            .filter(|m| (1..=7).contains(m))
            .ok_or("superblock_shapes must be 1..=7")? as u8;
        if let Some(c) = c.as_deref_mut() {
            c.tuning.lab.switches.superblock_shapes = Some(m);
        }
    }
    let mut small = imij::SmallLab::default();
    if let Some(v) = cfg.get("small_transforms") {
        small.on = Some(v.as_bool().ok_or("small_transforms must be a bool")?);
    }
    let on = small.on == Some(true);
    if let Some(v) = cfg.get("small_split") {
        small.split = Some(
            v.as_u64()
                .filter(|m| *m <= 7)
                .ok_or("small_split must be 0..=7")? as u8,
        );
        if !on {
            return Err("small_split requires small_transforms true".into());
        }
    }
    for (k, lo, hi) in [("small_lambda", 0.0, 16.0), ("small_edge", 0.0, 64.0)] {
        if let Some(x) = num(k, lo, hi)? {
            if !on {
                return Err(format!("{k} requires small_transforms true").into());
            }
            if k == "small_lambda" {
                small.lambda = Some(x as f32);
            } else {
                small.edge = Some(x as f32);
            }
        }
    }
    if let Some(v) = cfg.get("small_int") {
        v.as_bool()
            .filter(|&b| b)
            .ok_or("small_int must be true when present")?;
        if !on {
            return Err("small_int requires small_transforms true".into());
        }
        small.int = true;
    }
    if small != imij::SmallLab::default()
        && let Some(c) = c.as_deref_mut()
    {
        c.tuning.lab.switches.small = Some(Box::new(small));
    }
    if let Some(v) = cfg.get("r19_transform") {
        let t = match v.as_str() {
            Some("2") => imij::LossyTransform::Dct2,
            Some("4") => imij::LossyTransform::Dct4,
            Some("adaptive4") => imij::LossyTransform::Adaptive4,
            _ => return Err("r19_transform must be \"2\", \"4\" or \"adaptive4\"".into()),
        };
        if cfg.get("small_transforms") == Some(&Value::Bool(true)) {
            return Err("r19_transform excludes small_transforms".into());
        }
        if let Some(c) = c.as_deref_mut() {
            c.tuning.channel_layouts =
                [imij::LossyChannelLayout::new(t, imij::ChannelSampling::Full); 3];
        }
    }
    if let Some(v) = cfg.get("seed_bank") {
        let b = v
            .as_u64()
            .filter(|b| *b <= 1)
            .ok_or("seed_bank must be 0 (no seed tables) or 1")?;
        if let Some(c) = c.as_deref_mut() {
            c.tuning.lab.seed_bank = b;
        }
    }
    if let Some(v) = cfg.get("high_quality") {
        v.as_bool()
            .filter(|&b| b)
            .ok_or("high_quality must be true when present")?;
    }
    if let Some(v) = cfg.get("trellis_channel_shift") {
        let a = v
            .as_array()
            .filter(|a| a.len() == 3)
            .ok_or("trellis_channel_shift must be three numbers")?;
        let mut shift = [0f32; 3];
        for (d, x) in shift.iter_mut().zip(a) {
            let x = x
                .as_f64()
                .ok_or("trellis_channel_shift must be three numbers")?;
            if !(-8.0..=8.0).contains(&x) || x as f32 as f64 != x {
                return Err("trellis_channel_shift out of range or not f32-exact".into());
            }
            *d = x as f32;
        }
        if let Some(c) = c.as_deref_mut() {
            c.tuning.lab.switches.trellis_channel_shift = Some(shift);
        }
    }
    // Lossy 0.4 dequantization knobs (base-table and bias study).
    if let Some(m) = cfg.get("matrix") {
        let m = m
            .as_u64()
            .filter(|m| *m <= 255)
            .ok_or("matrix must be 0..=255")? as u8;
        if !matches!(m, 0..=9 | 63 | 255) {
            return Err("matrix must be 0..=9, 63 or 255".into());
        }
        if let Some(c) = c.as_deref_mut() {
            c.tuning.matrix = m;
        }
    }
    if let Some(v) = cfg.get("channel_mul") {
        let a = v
            .as_array()
            .filter(|a| a.len() == 3)
            .ok_or("channel_mul must be three numbers")?;
        let mut m = [0f32; 3];
        for (d, x) in m.iter_mut().zip(a) {
            let x = x.as_f64().ok_or("channel_mul must be three numbers")?;
            if !(x > 0.0 && x < 65536.0) || x as f32 as f64 != x {
                return Err("channel_mul out of range or not f32-exact".into());
            }
            *d = x as f32;
        }
        if let Some(c) = c.as_deref_mut() {
            c.tuning.channel_mul = Some(m);
        }
    }
    if let Some(v) = cfg.get("quant_round") {
        let x = v.as_f64().ok_or("quant_round must be a number")?;
        if !(0.0..=0.5).contains(&x) || x as f32 as f64 != x {
            return Err("quant_round out of range or not f32-exact".into());
        }
        if let Some(c) = c.as_deref_mut() {
            c.tuning.round = x as f32;
        }
    }
    if let Some(v) = cfg.get("explicit_matrix") {
        let name = v.as_str().ok_or("explicit_matrix must be a string")?;
        let q = cfg["quality"].as_f64().ok_or("quality missing")? as f32;
        let tables = explicit_catalog(name, q)?;
        if let Some(c) = c.as_deref_mut() {
            c.tuning.lab.switches.explicit_matrix = Some(Box::new(tables));
        }
    }
    if let Some(v) = cfg.get("recon_bias") {
        let (bias, centroid) = recon_bias_value(v)?;
        if let Some(c) = c.as_deref_mut() {
            c.tuning.lab.switches.recon_bias = bias;
            c.tuning.lab.switches.recon_bias_centroid = centroid;
        }
    }
    if let Some(v) = cfg.get("u16") {
        let on = v.as_bool().ok_or("u16 must be a boolean")?;
        if on && cfg["color"] != "ycocg" {
            return Err("u16 requires color ycocg".into());
        }
    }
    // Lossy 0.4 signalled color transform (R5 study): a preset name or
    // `{"explicit": [[4],[4],[4]]}` (the decode matrix, YCoCg family only).
    if let Some(v) = cfg.get("color_transform") {
        let t = match v {
            Value::String(s) => match s.as_str() {
                "identity" => imij::ColorTransform::Identity,
                "bt601" => imij::ColorTransform::Bt601,
                "bt709" => imij::ColorTransform::Bt709,
                "ycocg" => imij::ColorTransform::YCoCg,
                _ => return Err("unknown color_transform".into()),
            },
            Value::Object(o) => {
                let rows = o
                    .get("explicit")
                    .and_then(Value::as_array)
                    .filter(|a| a.len() == 3)
                    .ok_or("color_transform explicit needs 3 rows")?;
                let mut m = [[0f32; 4]; 3];
                for (dst, row) in m.iter_mut().zip(rows) {
                    let r = row
                        .as_array()
                        .filter(|a| a.len() == 4)
                        .ok_or("color_transform explicit rows need 4 numbers")?;
                    for (d, x) in dst.iter_mut().zip(r) {
                        let x = x.as_f64().ok_or("color_transform values must be numbers")?;
                        if !x.is_finite() || x.abs() > 65536.0 || x as f32 as f64 != x {
                            return Err(
                                "color_transform value out of range or not f32-exact".into()
                            );
                        }
                        *d = x as f32;
                    }
                }
                imij::ColorTransform::Explicit(m)
            }
            _ => return Err("color_transform must be a name or an object".into()),
        };
        if cfg["color"] != "ycocg" {
            return Err("color_transform requires color ycocg".into());
        }
        if cfg["u16"].as_bool() == Some(true) {
            return Err("color_transform excludes u16".into());
        }
        if let Some(c) = c {
            c.tuning.lab.switches.color_transform = Some(t);
        }
    }
    Ok(())
}

/// IMI's quality -> quantizer scale mapping (mirrors `QParams::from_quality`
/// for 1..=100; 0 above 100, where the floor takes over).
fn imij_scale(q: f64) -> f64 {
    let q = q.clamp(1.0, 116.0);
    if q < 50.0 {
        50.0 / q
    } else if q <= 100.0 {
        (200.0 - 2.0 * q) / 100.0
    } else {
        0.0
    }
}

/// jpegli's per-frequency exponents, raster order (zenjpeg
/// `src/quant/mod.rs:39`, `DIST_THRESHOLD` 1.5 at `:88`), copied so the
/// harness needs no hidden zenjpeg feature.
#[rustfmt::skip]
const JPEGLI_FREQUENCY_EXPONENT: [f32; 64] = [
    1.00, 0.51, 0.67, 0.74, 1.00, 1.00, 1.00, 1.00, 0.51, 0.66, 0.69, 0.87, 1.00, 1.00, 1.00, 1.00,
    0.67, 0.69, 0.84, 0.83, 0.96, 1.00, 1.00, 1.00, 0.74, 0.87, 0.83, 1.00, 1.00, 0.91, 0.91, 1.00,
    1.00, 1.00, 0.96, 1.00, 1.00, 1.00, 1.00, 1.00, 1.00, 1.00, 1.00, 0.91, 1.00, 1.00, 1.00, 1.00,
    1.00, 1.00, 1.00, 0.91, 1.00, 1.00, 1.00, 1.00, 1.00, 1.00, 1.00, 1.00, 1.00, 1.00, 1.00, 1.00,
];

/// jpegli's nonlinear distance-to-scale (zenjpeg `src/quant/mod.rs:485`):
/// linear below 1.5, then per-frequency exponents.
fn jpegli_distance_to_scale(distance: f32, freq_idx: usize) -> f32 {
    const DIST_THRESHOLD: f32 = 1.5;
    if distance < DIST_THRESHOLD {
        return distance;
    }
    let e = JPEGLI_FREQUENCY_EXPONENT[freq_idx];
    (0.5 * distance).max(DIST_THRESHOLD.powf(1.0 - e) * distance.powf(e))
}

/// Named explicit base tables for `explicit_matrix` (Lossy 0.4, matrix 63;
/// raster order, Y/Co/Cg). The catalog carries the base-table study's
/// candidates so a grid names a table instead of 192 numbers.
fn explicit_catalog(name: &str, q: f32) -> Result<[[f32; 64]; 3]> {
    let preset = |id: u8| -> Result<[[f32; 64]; 3]> {
        imij::lab_base_tables(id).ok_or_else(|| format!("bad preset {id}").into())
    };
    match name {
        // jpegli's YCbCr tables with the chroma channels exchanged (does Co
        // want Cb's shape and Cg Cr's?).
        "jpegli_ycbcr_swap" => {
            let t = preset(4)?;
            Ok([t[0], t[2], t[1]])
        }
        // Cr's shape for both chroma channels.
        "jpegli_ycbcr_cr_both" => {
            let t = preset(4)?;
            Ok([t[0], t[2], t[2]])
        }
        // jpegli's XYB shapes on YCoCg (the tables matrix 2 uses).
        "jpegli_xyb_on_ycocg" => preset(2),
        // jpegli's own nonlinear distance law on the matrix-4 tables: at
        // quality q the file's scale s corresponds to distance 5 s (matrix 4
        // is raw*GLOBAL*5s, so its linear step is jpegli's), and the
        // nonlinear correction `dts(d,i)/d` is what jpegli applies beyond it
        // (at q90, d = 1, this is matrix 4 exactly). The DC entry is kept:
        // the DC step is not a frequency shape.
        "jpegli_dist" => {
            // Above q95 the file's scale s shrinks toward 0 and the distance
            // law is linear; a small floor keeps q96..100 defined (their
            // steps sit on the floor and match matrix 4's).
            let s = imij_scale(q as f64) as f32;
            let d = (5.0 * s).max(0.1);
            let mut t = preset(4)?;
            for tc in &mut t {
                for (i, v) in tc.iter_mut().enumerate().skip(1) {
                    *v *= jpegli_distance_to_scale(d, i) / d;
                }
            }
            Ok(t)
        }
        // The same law on jpegli's XYB shapes (matrix 2), for XYB.
        "jpegli_xyb_dist" => {
            let s = imij_scale(q as f64) as f32;
            let d = (5.0 * s).max(0.1);
            let mut t = preset(2)?;
            for tc in &mut t {
                for (i, v) in tc.iter_mut().enumerate().skip(1) {
                    *v *= jpegli_distance_to_scale(d, i) / d;
                }
            }
            Ok(t)
        }
        _ => Err(format!("unknown explicit_matrix {name}").into()),
    }
}

/// `recon_bias`: "centroid", or an object: `k` (2..=63 or 64, default 64),
/// then either `b` (`[[[q1, q2]; 2]; 3]`, in 1/256 steps, i8) or
/// `"centroid": true` (the encoder computes the per-image centroid with the
/// band split `k`).
fn recon_bias_value(v: &Value) -> Result<(Option<imij::ReconBias>, Option<u8>)> {
    if v.as_str() == Some("centroid") {
        return Ok((None, Some(64)));
    }
    let o = v
        .as_object()
        .ok_or("recon_bias must be an object or \"centroid\"")?;
    let k = match o.get("k") {
        None => 64u64,
        Some(k) => k.as_u64().ok_or("recon_bias k must be an integer")?,
    };
    if !(k == 64 || (2..=63).contains(&k)) {
        return Err("recon_bias k must be 2..=63 or 64".into());
    }
    if let Some(c) = o.get("centroid") {
        if c.as_bool() != Some(true) {
            return Err("recon_bias centroid must be true".into());
        }
        if o.contains_key("b") {
            return Err("recon_bias centroid excludes b".into());
        }
        return Ok((None, Some(k as u8)));
    }
    let b = o.get("b").ok_or("recon_bias needs b or centroid")?;
    let rows = b
        .as_array()
        .filter(|a| a.len() == 3)
        .ok_or("recon_bias b must be 3x2x2")?;
    let mut out = [[[0i8; 2]; 2]; 3];
    for (dst, row) in out.iter_mut().zip(rows) {
        let bands = row
            .as_array()
            .filter(|a| a.len() == 2)
            .ok_or("recon_bias b must be 3x2x2")?;
        for (dst, band) in dst.iter_mut().zip(bands) {
            let qs = band
                .as_array()
                .filter(|a| a.len() == 2)
                .ok_or("recon_bias b must be 3x2x2")?;
            for (d, x) in dst.iter_mut().zip(qs) {
                *d = i8::try_from(x.as_i64().ok_or("recon_bias b must be integers")?)
                    .map_err(|_| "recon_bias b out of i8 range")?;
            }
        }
    }
    Ok((Some(imij::ReconBias { k: k as u8, b: out }), None))
}

/// A sweep cell's lossy encoder: the public configuration plus the expert
/// tuning the knobs write into, attached when the cell encodes.
#[derive(Clone)]
struct LossyCell {
    config: imij::ImiEncoderConfig,
    tuning: imij::LossyTuning,
}
impl LossyCell {
    fn new(config: imij::ImiEncoderConfig) -> Self {
        Self {
            config,
            tuning: imij::LossyTuning::default(),
        }
    }
    fn finish(self) -> imij::ImiEncoderConfig {
        self.config.with_lossy_tuning(self.tuning)
    }
}

fn aq_knobs(cfg: &Value, c: &mut LossyCell) -> Result<()> {
    aq_knobs_check(cfg, Some(c))
}

/// Trellis knob keys (encoder-ablation only).
const TRELLIS_KEYS: [&str; 7] = [
    "trellis",
    "trellis_lambda1",
    "trellis_lambda2",
    "trellis_passes",
    "trellis_min_quality",
    "trellis_thorough",
    "trellis_aq_coupling",
];

/// Typed-input keys (encoder-ablation only, split-bound study 2026-10-10).
const INPUT_KEYS: [&str; 3] = ["input", "transfer", "crop"];

/// Encoder ablation trellis knob (lab, `LossyConfig::trellis`): absent =
/// the trellis the cell's effort selects (none below effort 9; grids from
/// before 2026-10-08 ran a build without it, so their absent knob meant
/// off); "off" = no trellis at any effort; or "zenjpeg" (zenjpeg's AC
/// trellis priced with IMI's run-level tables) with
/// `trellis_lambda1` / `trellis_lambda2` (mozjpeg's lambda log scales,
/// f32-exact, 0..=40), `trellis_passes` (1..=4 rate-model passes) and
/// `trellis_min_quality` (0..=100; lower qualities keep the default
/// quantization); `trellis_thorough` (optional, default false) disables
/// zenjpeg's search limits on busy blocks; `trellis_aq_coupling` (optional,
/// default 0) is zenjpeg's AQ-to-lambda coupling scale, clamped to +-1 as
/// in zenjpeg's curated steps.
fn trellis_knob(cfg: &Value) -> Result<Option<imij::LossyTrellis>> {
    let plain = || {
        if TRELLIS_KEYS[1..].iter().any(|k| cfg.get(*k).is_some()) {
            return Err::<(), Box<dyn std::error::Error>>(
                "trellis_* options require trellis \"zenjpeg\"".into(),
            );
        }
        Ok(())
    };
    match cfg.get("trellis").map(Value::as_str) {
        None => plain().map(|_| None),
        Some(Some("off")) => plain().map(|_| {
            Some(imij::LossyTrellis {
                passes: 0,
                ..imij::LossyTrellis::default()
            })
        }),
        Some(Some("zenjpeg")) => {
            let exact = |key: &str, lo: f64, hi: f64| -> Result<f32> {
                let v = cfg[key].as_f64().ok_or_else(|| format!("{key} required"))?;
                if !(lo..=hi).contains(&v) || v as f32 as f64 != v {
                    return Err(format!("{key} must be an f32-exact value in {lo}..={hi}").into());
                }
                Ok(v as f32)
            };
            let passes = match cfg["trellis_passes"].as_u64() {
                Some(p @ 1..=4) => p as u8,
                _ => return Err("trellis_passes (1..=4) required".into()),
            };
            let thorough = match cfg.get("trellis_thorough") {
                None => false,
                Some(v) => v.as_bool().ok_or("trellis_thorough must be a boolean")?,
            };
            let aq_coupling = match cfg.get("trellis_aq_coupling") {
                None => 0.0,
                Some(_) => exact("trellis_aq_coupling", -16.0, 16.0)?,
            };
            Ok(Some(imij::LossyTrellis {
                lambda_log_scale1: exact("trellis_lambda1", 0.0, 40.0)?,
                lambda_log_scale2: exact("trellis_lambda2", 0.0, 40.0)?,
                passes,
                thorough,
                min_quality: exact("trellis_min_quality", 0.0, 100.0)?,
                aq_coupling,
                aq_clamp: 1.0,
            }))
        }
        _ => Err("trellis must be off or zenjpeg".into()),
    }
}

/// Decoder for a cell's encoded bytes: `jpegli` cells decode as JPEG, with
/// pixel-replicated chroma when `upsampling` is "nearest".
pub(crate) fn decode_codec(cfg: &Value) -> Result<&'static str> {
    Ok(match cfg["codec"].as_str().ok_or("codec missing")? {
        "imij" if cfg["u16"].as_bool() == Some(true) => "imij-u16",
        "imij" => "imij",
        "jpeg" => "jpeg",
        "jpegli" if cfg["upsampling"] == "nearest" => "jpeg-nn",
        "jpegli" => "jpeg",
        "jpeg-jxl" => "jpeg-jxl",
        "jxl" => "jxl",
        "avif" => "avif",
        _ => return Err("unknown codec".into()),
    })
}

/// The one decode path for RD cells; `decode-rgb` recovers pruned pixels with it.
pub(crate) fn decode_rgb(codec: &str, data: &[u8], w: u32, h: u32) -> Result<Vec<u8>> {
    let decoded = match codec {
        "imij" => {
            let (header, p) = imij::decode(data)?;
            if header.width != w || header.height != h {
                return Err("IMIJ dimensions mismatch".into());
            }
            p
        }
        // The `u16` arm: U16 YCoCg output of the x257 source, rounded back
        // to 8-bit codes for metrics.
        "imij-u16" => {
            let (header, p) = imij::decode(data)?;
            if header.width != w || header.height != h || header.sample != imij::SampleType::U16 {
                return Err("IMIJ u16 dimensions/sample mismatch".into());
            }
            p.as_chunks::<2>()
                .0
                .iter()
                .map(|s| ((u32::from(u16::from_ne_bytes([s[0], s[1]])) + 128) / 257) as u8)
                .collect()
        }
        "jpeg" | "jpeg-nn" => {
            use zenjpeg::decoder::{ChromaUpsampling, Decoder};
            let up = if codec == "jpeg-nn" {
                ChromaUpsampling::NearestNeighbor
            } else {
                ChromaUpsampling::Triangle
            };
            let result = Decoder::new()
                .chroma_upsampling(up)
                .decode(data, enough::Unstoppable)?;
            if result.width() != w || result.height() != h {
                return Err("JPEG dimensions mismatch".into());
            }
            result.pixels_u8().ok_or("JPEG not RGB8")?.to_vec()
        }
        "jxl" | "jpeg-jxl" => crate::cmp::jxl_decode_rgb8(data, false),
        "avif" => crate::cmp::avif_rgb8(&crate::cmp::avif_decode(data, 1)),
        _ => return Err("unknown codec".into()),
    };
    check_rgb(&decoded, w, h)?;
    Ok(decoded)
}

/// `redecode-case CELL`: restore a cell's pruned decoded.rgb8 from its encoded
/// file with the cell's own decoder, and require the recorded sha256.
fn redecode_case(cell: &Path) -> Result<()> {
    let case: Value = serde_json::from_slice(&fs::read(cell.join("case.json"))?)?;
    let decoded_path = Path::new(case["decoded"].as_str().ok_or("decoded missing")?);
    if decoded_path.exists() {
        return Err("decoded pixels already present".into());
    }
    let w = u32::try_from(case["width"].as_u64().ok_or("width missing")?)?;
    let h = u32::try_from(case["height"].as_u64().ok_or("height missing")?)?;
    let encoded = fs::read(case["encoded"].as_str().ok_or("encoded missing")?)?;
    if hash(&encoded) != case["encoded_sha256"] {
        return Err("encoded file hash differs from case.json".into());
    }
    let decoded = decode_rgb(decode_codec(&case["config"])?, &encoded, w, h)?;
    if hash(&decoded) != case["decoded_sha256"] {
        return Err("re-decoded pixels differ from case.json".into());
    }
    fs::write(decoded_path, &decoded)?;
    Ok(())
}

/// `decode-rgb CODEC ENCODED W H OUTPUT`: write packed RGB8, print its sha256.
fn decode_file(codec: &str, encoded: &Path, w: u32, h: u32, output: &Path) -> Result<()> {
    if output.exists() {
        return Err("decode output already exists".into());
    }
    let decoded = decode_rgb(codec, &fs::read(encoded)?, w, h)?;
    fs::write(output, &decoded)?;
    println!(
        "{}",
        json!({"decoded": output, "decoded_sha256": hash(&decoded)})
    );
    Ok(())
}

/// Persisted preset metadata must describe the settings actually used above.
/// This CLI deliberately supports only the declared study policies.
pub(crate) fn validate_config(cfg: &Value) -> Result<()> {
    let codec = cfg["codec"].as_str().ok_or("codec missing")?;
    let q = cfg["quality"].as_f64().ok_or("quality missing")?;
    let object = cfg.as_object().ok_or("config must be an object")?;
    let fixed = match codec {
        "imij" => json!({"threads":6}),
        "jpeg" => {
            json!({"progressive":true,"auto_optimize":true,"aq":true,"quality_axis":"ApproxJpegli"})
        }
        "jpeg-jxl" => {
            json!({"progressive":true,"auto_optimize":true,"aq":true,"quality_axis":"ApproxJpegli","transcode":"codestream","transcode_effort":9})
        }
        "jpegli" => {
            json!({"progressive":true,"auto_optimize":false,"aq":true,"quality_axis":"ApproxJpegli"})
        }
        "jxl" => json!({"effort":7,"quality_axis":"distance"}),
        "avif" => json!({"backend":"Zenav1Svt","speed":6,"threads":6}),
        _ => return Err("unknown codec".into()),
    };
    for (key, expected) in fixed.as_object().expect("fixed object") {
        if cfg[key] != *expected {
            return Err(format!("unsupported {codec} preset {key}: expected {expected}").into());
        }
    }
    if !q.is_finite()
        || (codec == "jxl" && !(0.0 < q && q <= 25.0))
        || (codec != "jxl"
            && !(0.0..=if codec == "imij"
                && (cfg["experiment"] == "subblocks-v2"
                    || (cfg["experiment"] == "encoder-ablation"
                        && cfg.get("high_quality") == Some(&Value::Bool(true))))
            {
                116.0
            } else {
                100.0
            })
                .contains(&q))
    {
        return Err("quality outside declared codec range".into());
    }
    let variable: &[&str] = match codec {
        "imij" => {
            if !matches!(cfg["color"].as_str(), Some("xyb" | "ycocg"))
                || !matches!(
                    cfg["entropy"].as_str(),
                    Some("runlevel" | "splithuf" | "tier")
                )
                || !matches!(cfg["stripe_block_rows"].as_u64(), Some(1 | 8))
                    && !(cfg["experiment"] == "encoder-ablation"
                        && cfg["stripe_block_rows"].as_u64() == Some(0))
            {
                return Err("unsupported IMIJ color/entropy/stripe policy".into());
            }
            if cfg["experiment"] != "subblocks-v2"
                && (cfg.get("channel_layouts").is_some() || cfg.get("compact_streams").is_some())
            {
                return Err("layout/compact options require subblocks-v2 experiment".into());
            }
            // Effort 10 everywhere except encoder ablations, which may vary
            // it (0..=10, f32-exact).
            let effort = cfg["effort"].as_f64().ok_or("effort missing")?;
            if cfg["experiment"] == "encoder-ablation" {
                if !(0.0..=10.0).contains(&effort) || effort as f32 as f64 != effort {
                    return Err("effort must be an f32-exact value in 0..=10".into());
                }
            } else if effort != 10.0 {
                return Err("unsupported imij preset effort: expected 10".into());
            }
            // The typed-input keys (split-bound study, 2026-10-10): `input`
            // the raw layout, `transfer` the colour metadata, `crop` a
            // centered window of the source.
            let input = input_key(cfg)?;
            input_layout(input)?;
            match (input, cfg.get("transfer").and_then(Value::as_str)) {
                (_, None) => {}
                ("u16" | "f32", Some("srgb" | "pq")) => {}
                _ => {
                    return Err(
                        "transfer is \"srgb\" or \"pq\" with input \"u16\"/\"f32\" only".into(),
                    );
                }
            }
            if input == "f32" && cfg.get("transfer").and_then(Value::as_str) != Some("pq") {
                return Err("input \"f32\" requires transfer \"pq\"".into());
            }
            if input != "rgb8" && cfg["u16"].as_bool() == Some(true) {
                return Err("u16 excludes the typed input key".into());
            }
            if let Some(n) = cfg.get("crop")
                && !matches!(n.as_u64(), Some(1..=16384))
            {
                return Err("crop must be an integer 1..=16384".into());
            }
            if cfg["experiment"] != "encoder-ablation"
                && ["dc_step", "dering", "dc_nearest"]
                    .iter()
                    .chain(&TRELLIS_KEYS)
                    .chain(&AQ_KEYS)
                    .chain(&INPUT_KEYS)
                    .any(|k| cfg.get(*k).is_some())
            {
                return Err("ablation options require encoder-ablation experiment".into());
            }
            match cfg["experiment"].as_str() {
                Some("subblocks-v2") if cfg["rdoq_lambda"].as_f64() == Some(0.0) => {
                    channel_layouts(cfg)?;
                    cfg["compact_streams"]
                        .as_bool()
                        .ok_or("compact_streams required")?;
                }
                Some("tested-envelope") if cfg["rdoq_lambda"].as_f64() == Some(0.0) => (),
                Some("encoder-ablation") if cfg["rdoq_lambda"].as_f64() == Some(0.0) => {
                    ablation(cfg)?;
                    aq_knobs_check(cfg, None)?;
                    if matches!(trellis_knob(cfg)?, Some(t) if t.passes > 0)
                        && cfg["entropy"] != "runlevel"
                        && cfg["trellis_rate"] != "huf"
                    {
                        return Err("the trellis prices run-level tokens only".into());
                    }
                }
                Some("rdoq-lambda-0.08")
                    if cfg["rdoq_lambda"].as_f64() == Some(0.08)
                        && cfg["entropy"] == "runlevel"
                        && cfg["stripe_block_rows"] == 1 => {}
                _ => return Err("unsupported IMIJ experiment/lambda policy".into()),
            }
            &[
                "effort",
                "color",
                "entropy",
                "stripe_block_rows",
                "rdoq_lambda",
                "experiment",
                "channel_layouts",
                "compact_streams",
                "dc_step",
                "dering",
                "dc_nearest",
                "trellis",
                "trellis_lambda1",
                "trellis_lambda2",
                "trellis_passes",
                "trellis_min_quality",
                "trellis_thorough",
                "trellis_aq_coupling",
                "aq_field_strength",
                "aq_field_levels",
                "aq_field_cell",
                "aq_field_relative",
                "aq_field_fade",
                "trellis_aq_exp",
                "aq_gamma",
                "aq_r0",
                "decode_speed",
                "exact_blocks",
                "aq_note",
                "trellis_rate",
                "trellis_channel_shift",
                "table_k_search",
                "table_k_mode",
                "matrix",
                "channel_mul",
                "explicit_matrix",
                "recon_bias",
                "quant_round",
                "u16",
                "color_transform",
                "input",
                "transfer",
                "crop",
                "superblocks",
                "superblock_shapes",
                "small_transforms",
                "small_split",
                "small_lambda",
                "small_edge",
                "high_quality",
                "r19_transform",
                "small_int",
                "seed_bank",
            ]
        }
        "jpeg" | "jpeg-jxl" | "avif" => {
            if !matches!(cfg["chroma"].as_str(), Some("444" | "420")) {
                return Err("unsupported chroma policy".into());
            }
            &["chroma"]
        }
        "jpegli" => {
            if !matches!(
                (cfg["chroma"].as_str(), cfg["upsampling"].as_str()),
                (Some("444"), Some("none")) | (Some("420"), Some("fancy" | "nearest"))
            ) {
                return Err("jpegli needs 444/none or 420/fancy|nearest".into());
            }
            &["chroma", "upsampling"]
        }
        "jxl" => {
            if cfg.get("experiment").is_none() {
                &[]
            } else {
                if cfg["experiment"] != "gaborish-ablation"
                    || cfg["gaborish"].as_bool().is_none()
                    || cfg["epf_level"].as_i64() != Some(i64::from(JXL_ABLATION_EPF_LEVEL))
                {
                    return Err(
                        "JXL gaborish-ablation requires gaborish boolean and epf_level=2".into(),
                    );
                }
                // Check the value passed to the encoder, not only its f64 JSON
                // representation: a distance just above 0.5 can round to 0.5.
                if q as f32 <= 0.5 {
                    return Err("Gaborish ablation requires distance >0.5 after f32 conversion; pinned encoder otherwise forces it off".into());
                }
                &["experiment", "gaborish", "epf_level"]
            }
        }
        _ => unreachable!(),
    };
    for key in object.keys() {
        if key != "codec"
            && key != "quality"
            && fixed.get(key).is_none()
            && !variable.contains(&key.as_str())
        {
            return Err(format!("unknown config field {key}").into());
        }
    }
    Ok(())
}

/// Score-only warm batch for filters: source identity is mandatory, dimensions
/// and every decoded buffer are validated before using cached reference state.
fn score_batch(spec_path: &Path) -> Result<()> {
    let spec_bytes = fs::read(spec_path)?;
    let spec: Value = serde_json::from_slice(&spec_bytes)?;
    let source = Path::new(spec["source"].as_str().ok_or("source path required")?);
    let w = u32::try_from(spec["width"].as_u64().ok_or("width required")?)?;
    let h = u32::try_from(spec["height"].as_u64().ok_or("height required")?)?;
    if spec["pixel_format"].as_str() != Some("RGB8_SRGB")
        || spec["stride_bytes"].as_u64() != Some(u64::from(w) * 3)
    {
        return Err("batch requires RGB8_SRGB with tight stride_bytes=width*3".into());
    }
    let expected_source = spec["source_sha256"]
        .as_str()
        .ok_or("source_sha256 required")?;
    let report = Path::new(spec["report"].as_str().ok_or("report path required")?);
    let jobs = spec["jobs"].as_array().ok_or("jobs array required")?;
    let mut scorer = crate::quality::WarmScorer::new(source, w, h)?;
    if scorer.source_sha256() != expected_source {
        return Err("source SHA-256 mismatch".into());
    }
    let binary_sha256 = hash(&fs::read(std::env::current_exe()?)?);
    if jobs.is_empty() {
        return Err("score batch jobs must be nonempty".into());
    }
    let prior = if report.exists() {
        let prior: Value = serde_json::from_slice(&fs::read(report)?)?;
        if prior["spec_sha256"].as_str() != Some(hash(&spec_bytes).as_str())
            || prior["binary_sha256"].as_str() != Some(binary_sha256.as_str())
        {
            return Err("resumed batch specification/binary differs".into());
        }
        prior
    } else {
        Value::Null
    };
    let mut rows = Vec::new();
    let mut failures = 0;
    for job in jobs {
        let result = (|| -> Result<Value> {
            let decoded = Path::new(job["decoded"].as_str().ok_or("decoded path required")?);
            let expected_decoded = job["decoded_sha256"]
                .as_str()
                .ok_or("decoded_sha256 required")?;
            let raw = fs::read(decoded)?;
            check_rgb(&raw, w, h)?;
            if hash(&raw) != expected_decoded {
                return Err("decoded SHA-256 mismatch".into());
            }
            let output = Path::new(job["output"].as_str().ok_or("output path required")?);
            let maps = job["maps"].as_bool().unwrap_or(false);
            let scores = output.join("scores.json");
            let prior_job = prior["jobs"].as_array().and_then(|rows| {
                rows.iter()
                    .find(|row| row["case_id"] == job["case_id"] && row["status"] == "complete")
            });
            if scores.exists() && prior_job.is_none() {
                return Err(
                    "existing score output has no matching completed producer report".into(),
                );
            }
            if !scores.exists() {
                scorer.score(source, decoded, output, maps)?;
            }
            let bytes = fs::read(&scores)?;
            if let Some(previous) = prior["jobs"].as_array().and_then(|rows| {
                rows.iter()
                    .find(|row| row["case_id"] == job["case_id"] && row["status"] == "complete")
            }) && previous["scores_sha256"].as_str() != Some(hash(&bytes).as_str())
            {
                return Err("resumed score SHA-256 differs".into());
            }
            let value: Value = serde_json::from_slice(&bytes)?;
            if value["source_sha256"].as_str() != Some(expected_source)
                || value["decoded_sha256"].as_str() != Some(expected_decoded)
                || value["width"].as_u64() != Some(u64::from(w))
                || value["height"].as_u64() != Some(u64::from(h))
            {
                return Err("saved score pair identity mismatch".into());
            }
            if maps {
                for key in ["cvvdp_fhd_diffmap", "butteraugli_diffmap"] {
                    let path = Path::new(value[key].as_str().ok_or("requested map absent")?);
                    if fs::metadata(path)?.len() != u64::from(w) * u64::from(h) * 4 {
                        return Err("map byte length mismatch".into());
                    }
                }
            }
            Ok(
                json!({"case_id":job["case_id"],"decoded":decoded,"decoded_sha256":expected_decoded,"scores":scores,"scores_sha256":hash(&bytes),"status":"complete"}),
            )
        })();
        let row = match result {
            Ok(row) => row,
            Err(error) => {
                failures += 1;
                eprintln!("score batch {}: {error}", job["case_id"]);
                json!({"case_id":job["case_id"],"decoded":job["decoded"],"status":"failed","error":error.to_string()})
            }
        };
        rows.push(row);
        save(
            report,
            &json!({"source":source,"source_sha256":expected_source,"width":w,"height":h,"spec_sha256":hash(&spec_bytes),"binary_sha256":binary_sha256,"jobs":rows,"failed":failures}),
        )?;
    }
    if failures != 0 {
        return Err(format!(
            "{failures} score-only batch failures; inspect {}",
            report.display()
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod preset_tests {
    use super::*;
    #[test]
    fn gaborish_ablation_pins_epf_and_rejects_inactive_or_mislabeled_settings() {
        let legacy = json!({"codec":"jxl","quality":1,"effort":7,"quality_axis":"distance"});
        validate_config(&legacy).unwrap();
        let old = jxl_config(&legacy, 1.0).unwrap();
        assert_eq!(old.distance(), 1.0);
        assert_eq!(old.effort(), 7);
        assert!(old.gaborish());
        assert_eq!(old.epf_level(), -1);

        for enabled in [false, true] {
            let mut cfg = legacy.clone();
            cfg["experiment"] = json!("gaborish-ablation");
            cfg["gaborish"] = json!(enabled);
            cfg["epf_level"] = json!(2);
            validate_config(&cfg).unwrap();
            let c = jxl_config(&cfg, 1.0).unwrap();
            assert_eq!(c.gaborish(), enabled);
            assert_eq!(c.epf_level(), 2);
            assert_eq!(c.effort(), 7);
            for (field, value) in [
                ("experiment", Value::Null),
                ("experiment", json!("unknown")),
                ("gaborish", Value::Null),
                ("gaborish", json!(1)),
                ("epf_level", Value::Null),
                ("epf_level", json!(-1)),
                ("epf_level", json!(0)),
                ("epf_level", json!(3)),
                ("effort", json!(8)),
                ("quality", json!(0.5)),
                ("quality", json!(0.5000000001)),
                ("quality", json!(26)),
                ("adaptive_gaborish", json!(true)),
            ] {
                let mut bad = cfg.clone();
                bad[field] = value;
                assert!(validate_config(&bad).is_err(), "{field}: {bad}");
            }
            for required in ["experiment", "gaborish", "epf_level"] {
                let mut bad = cfg.clone();
                bad.as_object_mut().unwrap().remove(required);
                assert!(validate_config(&bad).is_err(), "missing {required}");
            }
        }
        for field in ["gaborish", "epf_level"] {
            let mut bad = legacy.clone();
            bad[field] = json!(0);
            assert!(validate_config(&bad).is_err());
        }
    }

    #[test]
    fn cached_cases_cannot_silently_substitute_another_ablation_arm() {
        let cfg = json!({"codec":"jxl","quality":1,"effort":7,"quality_axis":"distance","experiment":"gaborish-ablation","gaborish":true,"epf_level":2});
        let row = json!({"config":cfg,"width":8,"height":9,"source_sha256":"source"});
        check_cached_case(&row, &cfg, 8, 9, "source").unwrap();
        let mut other_arm = cfg.clone();
        other_arm["gaborish"] = json!(false);
        assert!(check_cached_case(&row, &other_arm, 8, 9, "source").is_err());
        assert!(check_cached_case(&row, &cfg, 9, 9, "source").is_err());
        assert!(check_cached_case(&row, &cfg, 8, 8, "source").is_err());
        assert!(check_cached_case(&row, &cfg, 8, 9, "other source").is_err());
    }

    #[test]
    fn subblock_policy_is_explicit_and_strict() {
        let mut cfg = json!({"codec":"imij","quality":116,"color":"xyb","entropy":"runlevel","stripe_block_rows":1,"rdoq_lambda":0,"effort":10,"threads":6,"experiment":"subblocks-v2","compact_streams":true,"channel_layouts":[{"transform":"adaptive4","sampling":"full"},{"transform":"4","sampling":"half-x"},{"transform":"2","sampling":"half-xy"}]});
        validate_config(&cfg).unwrap();
        cfg["channel_layouts"][0]["sampling"] = json!("quarter");
        assert!(validate_config(&cfg).is_err());
        cfg["channel_layouts"][0]["sampling"] = json!("full");
        cfg["channel_layouts"][1]["unknown"] = json!(0);
        assert!(validate_config(&cfg).is_err());
        cfg["channel_layouts"][1]
            .as_object_mut()
            .unwrap()
            .remove("unknown");
        cfg["experiment"] = json!("tested-envelope");
        cfg["quality"] = json!(50);
        assert!(validate_config(&cfg).is_err());
    }
    #[test]
    fn encoder_ablation_requires_every_knob_and_only_in_its_experiment() {
        let cfg = json!({"codec":"imij","quality":30,"color":"xyb","entropy":"runlevel","stripe_block_rows":8,"rdoq_lambda":0,"effort":10,"threads":6,"experiment":"encoder-ablation","dc_step":[64,64],"dering":false,"dc_nearest":true});
        validate_config(&cfg).unwrap();
        assert_eq!(ablation(&cfg).unwrap(), ([64, 64], false, true));
        for (field, value) in [
            ("dc_step", json!([64])),
            ("dc_step", json!([64, 256])),
            ("dc_step", json!([64, -1])),
            ("dering", json!(1)),
            ("dc_nearest", Value::Null),
            ("rdoq_lambda", json!(0.08)),
        ] {
            let mut bad = cfg.clone();
            bad[field] = value;
            assert!(validate_config(&bad).is_err(), "{field}: {bad}");
        }
        let mut stray = cfg.clone();
        stray["experiment"] = json!("tested-envelope");
        assert!(validate_config(&stray).is_err());
    }

    #[test]
    fn aq_tier_and_exact_knobs_validate() {
        let base = json!({"codec":"imij","quality":30,"color":"xyb","entropy":"runlevel","stripe_block_rows":8,"rdoq_lambda":0,"effort":10,"threads":6,"experiment":"encoder-ablation","dc_step":[0,0],"dering":true,"dc_nearest":true});
        let with = |pairs: &[(&str, Value)]| {
            let mut c = base.clone();
            for (k, v) in pairs {
                c[*k] = v.clone();
            }
            c
        };
        let field = [
            ("aq_field_strength", json!(1.0)),
            ("aq_field_levels", json!(4)),
            ("aq_field_cell", json!(4)),
            ("aq_field_relative", json!(true)),
            ("aq_field_fade", json!(true)),
        ];
        validate_config(&with(&field)).unwrap();
        validate_config(&with(&[
            field[0].clone(),
            field[1].clone(),
            field[2].clone(),
            field[3].clone(),
            field[4].clone(),
            ("trellis_aq_exp", json!(2.0)),
        ]))
        .unwrap();
        assert!(validate_config(&with(&field[..4])).is_err(), "fade missing");
        assert!(
            validate_config(&with(&field[1..])).is_err(),
            "strength missing"
        );
        validate_config(&with(&[("aq_gamma", json!(0.5)), ("aq_r0", json!(1.0))])).unwrap();
        assert!(validate_config(&with(&[("aq_gamma", json!(0.5))])).is_err());
        validate_config(&with(&[
            ("decode_speed", json!("compact")),
            ("entropy", json!("tier")),
            ("stripe_block_rows", json!(0)),
        ]))
        .unwrap();
        assert!(
            validate_config(&with(&[("decode_speed", json!("compact"))])).is_err(),
            "entropy must be tier"
        );
        assert!(
            validate_config(&with(&[
                ("decode_speed", json!("fastest")),
                ("entropy", json!("tier"))
            ]))
            .is_err(),
            "fastest is YCoCg"
        );
        validate_config(&with(&[("exact_blocks", json!(0))])).unwrap();
        let huf = [
            ("trellis_rate", json!("huf")),
            ("entropy", json!("splithuf")),
        ];
        validate_config(&with(&huf)).unwrap();
        validate_config(&with(&[
            huf[0].clone(),
            huf[1].clone(),
            ("trellis", json!("zenjpeg")),
            ("trellis_lambda1", json!(14.5)),
            ("trellis_lambda2", json!(16.5)),
            ("trellis_passes", json!(2)),
            ("trellis_min_quality", json!(0.0)),
        ]))
        .unwrap();
        assert!(
            validate_config(&with(&huf[..1])).is_err(),
            "huf rate needs split-Huffman"
        );
        validate_config(&with(&[
            huf[0].clone(),
            ("decode_speed", json!("balanced")),
            ("entropy", json!("tier")),
            ("stripe_block_rows", json!(0)),
        ]))
        .unwrap();
        assert!(validate_config(&with(&[("trellis_rate", json!("rans"))])).is_err());
        assert!(validate_config(&with(&[("trellis_rate", json!("runlevel"))])).is_err());
        validate_config(&with(&[("trellis_rate", json!("none"))])).unwrap();
        validate_config(&with(&[("trellis_channel_shift", json!([0.0, 1.0, 1.0]))])).unwrap();
        assert!(validate_config(&with(&[("trellis_channel_shift", json!([0.0, 1.0]))])).is_err());
        assert!(
            validate_config(&with(&[("trellis_channel_shift", json!([0.0, 9.0, 1.0]))])).is_err()
        );
        let mut stray = with(&field);
        stray["experiment"] = json!("tested-envelope");
        assert!(validate_config(&stray).is_err());
    }

    #[test]
    fn trellis_knob_is_explicit_and_absent_follows_effort() {
        let base = json!({"codec":"imij","quality":30,"color":"xyb","entropy":"runlevel","stripe_block_rows":8,"rdoq_lambda":0,"effort":10,"threads":6,"experiment":"encoder-ablation","dc_step":[0,0],"dering":true,"dc_nearest":true});
        assert_eq!(trellis_knob(&base).unwrap(), None);
        let mut off = base.clone();
        off["trellis"] = json!("off");
        validate_config(&off).unwrap();
        assert_eq!(trellis_knob(&off).unwrap().unwrap().passes, 0);
        off["entropy"] = json!("splithuf");
        validate_config(&off).unwrap();
        // Encoder ablations may vary effort; other experiments pin 10.
        for (effort, ok) in [
            (json!(7.5), true),
            (json!(0), true),
            (json!(10.5), false),
            (json!(0.1), false),
        ] {
            let mut e = base.clone();
            e["effort"] = effort;
            assert_eq!(validate_config(&e).is_ok(), ok, "{e}");
        }
        let mut on = base.clone();
        on["trellis"] = json!("zenjpeg");
        on["trellis_lambda1"] = json!(14.5);
        on["trellis_lambda2"] = json!(16.5);
        on["trellis_passes"] = json!(1);
        on["trellis_min_quality"] = json!(0);
        validate_config(&on).unwrap();
        let t = trellis_knob(&on).unwrap().unwrap();
        assert_eq!(
            (
                t.lambda_log_scale1,
                t.lambda_log_scale2,
                t.passes,
                t.min_quality,
                t.thorough
            ),
            (14.5, 16.5, 1, 0.0, false)
        );
        assert_eq!((t.aq_coupling, t.aq_clamp), (0.0, 1.0));
        let mut coupled = on.clone();
        coupled["trellis_aq_coupling"] = json!(-4.0);
        assert_eq!(trellis_knob(&coupled).unwrap().unwrap().aq_coupling, -4.0);
        let mut thorough = on.clone();
        thorough["trellis_thorough"] = json!(true);
        assert!(trellis_knob(&thorough).unwrap().unwrap().thorough);
        for (field, value) in [
            ("trellis_lambda1", Value::Null),
            ("trellis_lambda1", json!(-1.0)),
            ("trellis_lambda1", json!(0.1)),
            ("trellis_lambda2", json!(41.0)),
            ("trellis_passes", json!(0)),
            ("trellis_passes", json!(5)),
            ("trellis_min_quality", json!(101)),
            ("trellis_thorough", json!(1)),
            ("trellis_aq_coupling", json!(17.0)),
            ("trellis_aq_coupling", json!(0.3)),
            ("trellis", json!("mozjpeg")),
            ("entropy", json!("splithuf")),
        ] {
            let mut bad = on.clone();
            bad[field] = value;
            assert!(validate_config(&bad).is_err(), "{field}: {bad}");
        }
        let mut stray = on.clone();
        stray["experiment"] = json!("tested-envelope");
        assert!(validate_config(&stray).is_err());
        let mut orphan = off.clone();
        orphan["trellis_lambda1"] = json!(14.5);
        assert!(validate_config(&orphan).is_err());
    }

    #[test]
    fn plain_jpegli_preset_and_decoder_are_explicit() {
        let cfg = json!({"codec":"jpegli","quality":30,"chroma":"420","upsampling":"nearest","progressive":true,"auto_optimize":false,"aq":true,"quality_axis":"ApproxJpegli"});
        validate_config(&cfg).unwrap();
        assert_eq!(decode_codec(&cfg).unwrap(), "jpeg-nn");
        let mut fancy = cfg.clone();
        fancy["upsampling"] = json!("fancy");
        assert_eq!(decode_codec(&fancy).unwrap(), "jpeg");
        for (field, value) in [
            ("auto_optimize", json!(true)),
            ("upsampling", json!("none")),
            ("chroma", json!("444")),
        ] {
            let mut bad = cfg.clone();
            bad[field] = value;
            assert!(validate_config(&bad).is_err(), "{field}: {bad}");
        }
        let full = json!({"codec":"jpegli","quality":30,"chroma":"444","upsampling":"none","progressive":true,"auto_optimize":false,"aq":true,"quality_axis":"ApproxJpegli"});
        validate_config(&full).unwrap();
    }

    #[test]
    fn jpeg_jxl_transcode_preset_is_pinned() {
        let cfg = json!({"codec":"jpeg-jxl","quality":30,"chroma":"420","progressive":true,"auto_optimize":true,"aq":true,"quality_axis":"ApproxJpegli","transcode":"codestream","transcode_effort":9});
        validate_config(&cfg).unwrap();
        for (field, value) in [
            ("transcode", json!("container")),
            ("transcode_effort", json!(7)),
            ("chroma", json!("422")),
            ("quality", json!(101)),
        ] {
            let mut bad = cfg.clone();
            bad[field] = value;
            assert!(validate_config(&bad).is_err(), "{field}: {bad}");
        }
    }

    #[test]
    fn declared_presets_accept_and_reject_mislabeled_policies() {
        let presets = [
            json!({"codec":"imij","quality":30,"color":"xyb","entropy":"runlevel","stripe_block_rows":1,"rdoq_lambda":0,"effort":10,"threads":6,"experiment":"tested-envelope"}),
            json!({"codec":"jpeg","quality":30,"chroma":"444","progressive":true,"auto_optimize":true,"aq":true,"quality_axis":"ApproxJpegli"}),
            json!({"codec":"jxl","quality":25,"effort":7,"quality_axis":"distance"}),
            json!({"codec":"avif","quality":30,"chroma":"420","backend":"Zenav1Svt","speed":6,"threads":6}),
        ];
        for (preset, key, bad) in [
            (&presets[0], "effort", json!(9)),
            (&presets[0], "threads", json!(1)),
            (&presets[1], "progressive", json!(false)),
            (&presets[1], "auto_optimize", json!(false)),
            (&presets[1], "aq", json!(false)),
            (&presets[2], "effort", json!(8)),
            (&presets[3], "speed", json!(7)),
            (&presets[3], "threads", json!(1)),
        ] {
            validate_config(preset).unwrap();
            let mut cfg = preset.clone();
            cfg[key] = bad;
            assert!(validate_config(&cfg).is_err(), "{key}");
            let mut cfg = preset.clone();
            cfg["unrecognized"] = json!(true);
            assert!(validate_config(&cfg).is_err());
        }
        let mut cfg = presets[0].clone();
        cfg["experiment"] = json!("rdoq-lambda-0.08");
        cfg["rdoq_lambda"] = json!(0.08);
        validate_config(&cfg).unwrap();
        cfg["entropy"] = json!("splithuf");
        assert!(validate_config(&cfg).is_err());
        let mut cfg = presets[2].clone();
        cfg["quality"] = json!(26);
        assert!(validate_config(&cfg).is_err());
    }

    /// Lossy 0.4 dequantization knobs: validation and a real encode/decode
    /// round trip for each (matrix ids, explicit catalog, bias, u16).
    #[test]
    fn dequant_knobs_validate_and_encode() {
        let base = json!({"codec":"imij","quality":60,"color":"ycocg","entropy":"runlevel","stripe_block_rows":8,"rdoq_lambda":0,"effort":7.5,"threads":6,"experiment":"encoder-ablation","dc_step":[0,0],"dering":true,"dc_nearest":true});
        let with = |pairs: &[(&str, Value)]| {
            let mut c = base.clone();
            for (k, v) in pairs {
                c[*k] = v.clone();
            }
            c
        };
        for ok in [
            with(&[("matrix", json!(3))]),
            with(&[("matrix", json!(4))]),
            with(&[("matrix", json!(5))]),
            with(&[("channel_mul", json!([1.0, 2.0, 2.5]))]),
            with(&[("quant_round", json!(0.5))]),
            with(&[("explicit_matrix", json!("jpegli_ycbcr_swap"))]),
            with(&[("explicit_matrix", json!("jpegli_ycbcr_cr_both"))]),
            with(&[("explicit_matrix", json!("jpegli_xyb_on_ycocg"))]),
            with(&[("explicit_matrix", json!("jpegli_dist"))]),
            with(&[(
                "recon_bias",
                json!({"k": 8, "b": [[[16,8],[4,-4]],[[12,6],[3,-3]],[[12,6],[3,-3]]]}),
            )]),
            with(&[("recon_bias", json!("centroid"))]),
            with(&[("recon_bias", json!({"centroid": true, "k": 64}))]),
            with(&[("u16", json!(true))]),
        ] {
            validate_config(&ok).unwrap();
        }
        for bad in [
            with(&[("matrix", json!(300))]),
            with(&[("channel_mul", json!([1.0, 2.0]))]),
            with(&[("channel_mul", json!([1.0, 0.0, 2.0]))]),
            with(&[("quant_round", json!(0.6))]),
            with(&[("explicit_matrix", json!("nope"))]),
            with(&[("recon_bias", json!({"k": 1}))]),
            with(&[("recon_bias", json!({"k": 8}))]),
            with(&[("recon_bias", json!({"centroid": true, "b": []}))]),
            with(&[("recon_bias", json!(3))]),
            with(&[("u16", json!(1))]),
        ] {
            assert!(validate_config(&bad).is_err(), "{bad}");
        }
        let mut xyb_u16 = base.clone();
        xyb_u16["u16"] = json!(true);
        xyb_u16["color"] = json!("xyb");
        assert!(validate_config(&xyb_u16).is_err(), "u16 needs ycocg");
        // A real encode/decode of each knob through the cell path.
        let (w, h) = (24u32, 18u32);
        let raw: Vec<u8> = (0..(w * h) as usize)
            .flat_map(|i| {
                let x = (i as u32 % w) * 255 / w;
                let y = (i as u32 / w) * 255 / h;
                [x as u8, y as u8, ((x + y) / 2) as u8]
            })
            .collect();
        for cfg in [
            with(&[("matrix", json!(3))]),
            with(&[("matrix", json!(4))]),
            with(&[("explicit_matrix", json!("jpegli_dist"))]),
            with(&[(
                "recon_bias",
                json!({"k": 64, "b": [[[16,8],[16,8]],[[12,6],[12,6]],[[12,6],[12,6]]]}),
            )]),
            with(&[("recon_bias", json!("centroid"))]),
            with(&[("u16", json!(true))]),
        ] {
            let (data, _) = encode_cell(&cfg, &raw, w, h, 1).unwrap();
            let out = decode_rgb(decode_codec(&cfg).unwrap(), &data, w, h).unwrap();
            assert_eq!(out.len(), raw.len(), "{cfg}");
        }
    }

    /// Typed-input keys (split-bound study): valid only in encoder
    /// ablations, values checked, and `f32` conversion needs `transfer`.
    #[test]
    fn typed_input_keys_are_ablation_only_and_validated() {
        let base = json!({"codec":"imij","quality":30,"color":"xyb","entropy":"runlevel","stripe_block_rows":8,"rdoq_lambda":0,"effort":7.5,"threads":6,"experiment":"encoder-ablation","dc_step":[0,0],"dering":true,"dc_nearest":true});
        validate_config(&base).unwrap();
        for (key, value) in [
            ("input", json!("gray")),
            ("input", json!("rgba")),
            ("input", json!("u16")),
        ] {
            let mut cfg = base.clone();
            cfg[key] = value;
            validate_config(&cfg).unwrap();
        }
        let mut cfg = base.clone();
        cfg["input"] = json!("u16");
        cfg["transfer"] = json!("pq");
        validate_config(&cfg).unwrap();
        let mut cfg = base.clone();
        cfg["input"] = json!("f32");
        cfg["transfer"] = json!("pq");
        cfg["crop"] = json!(256);
        validate_config(&cfg).unwrap();
        for bad in [
            json!({"input":"yuv"}),
            json!({"transfer":"pq"}),
            json!({"input":"f32"}),
            json!({"input":"f32","transfer":"srgb"}),
            json!({"input":"gray","transfer":"pq"}),
            json!({"crop":0}),
            json!({"crop":1.5}),
            json!({"crop":-8}),
        ] {
            let mut cfg = base.clone();
            for (k, v) in bad.as_object().unwrap() {
                cfg[k] = v.clone();
            }
            assert!(validate_config(&cfg).is_err(), "{bad}");
        }
        // The non-ablation experiment rejects them all.
        let mut cfg = base.clone();
        cfg["experiment"] = json!("tested-envelope");
        cfg["effort"] = json!(10);
        cfg["input"] = json!("gray");
        assert!(validate_config(&cfg).is_err());
        // `input_info` sets the colour metadata the encoder sees.
        let pq = json!({"input":"u16","transfer":"pq"});
        assert_eq!(input_info(&pq, 8, 8).unwrap().cicp, imij::Cicp::BT2100_PQ);
        let lin = json!({"input":"f32","transfer":"pq"});
        let info = input_info(&lin, 8, 8).unwrap();
        assert_eq!(info.sample, imij::SampleType::F32);
        assert_eq!(info.cicp.transfer_characteristics, 8);
        assert!(check_input(&[0u8; 8 * 8 * 12], 8, 8, &lin).is_ok());
        assert!(check_input(&[0u8; 8 * 8 * 3], 8, 8, &lin).is_err());
    }
}
