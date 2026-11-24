//! Standalone experimental raw filter and native zenbench harness.
#![forbid(unsafe_code)]
#[path = "../src/postfilter.rs"]
mod postfilter;

use postfilter::{Layout, Strength, apply};
use std::{error::Error, fs, hint::black_box, path::Path, sync::Arc, time::Duration};
type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn save(path: &str, value: &impl serde::Serialize) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

fn raw_apply(a: &[String]) -> Result<()> {
    let mut pixels = fs::read(&a[1])?;
    let l = Layout {
        width: a[3].parse()?,
        height: a[4].parse()?,
        channels: a[5].parse()?,
        stride_bytes: a[6].parse()?,
        origin: (a[7].parse()?, a[8].parse()?),
    };
    l.validate(pixels.len())?;
    let before = pixels.clone();
    let s = Strength::parse(&a[9])?;
    apply(&mut pixels, l, s)?;
    let mut change = Vec::with_capacity(l.width * l.height);
    let mut seam_jumps = [0u64; 2];
    let mut seam_count = 0u64;
    for y in 0..l.height {
        for x in 0..l.width {
            let i = y * l.stride_bytes + x * l.channels;
            change.push(
                (0..3)
                    .map(|c| before[i + c].abs_diff(pixels[i + c]))
                    .max()
                    .unwrap(),
            );
            for previous in [
                (x > 0 && (x % 8 + l.origin.0 % 8).is_multiple_of(8)).then(|| i - l.channels),
                (y > 0 && (y % 8 + l.origin.1 % 8).is_multiple_of(8)).then(|| i - l.stride_bytes),
            ]
            .into_iter()
            .flatten()
            {
                seam_count += 3;
                for c in 0..3 {
                    seam_jumps[0] += u64::from(before[i + c].abs_diff(before[previous + c]));
                    seam_jumps[1] += u64::from(pixels[i + c].abs_diff(pixels[previous + c]));
                }
            }
        }
    }
    fs::write(&a[2], &pixels)?;
    let change_path = format!("{}.change-max-rgb.u8", a[2]);
    fs::write(&change_path, &change)?;
    save(
        &format!("{}.json", a[2]),
        &serde_json::json!({
            "input":a[1],"output":a[2],"width":l.width,"height":l.height,
            "channels":l.channels,"stride_bytes":l.stride_bytes,"origin":l.origin,
            "strength":s.name(),"scratch_peak_bytes":0,"scratch_retained_bytes":0,
            "changed_pixels":change.iter().filter(|&&v|v!=0).count(),
            "maximum_rgb_delta":change.iter().max().copied().unwrap_or(0),
            "change_map":change_path,"map_format":"tight u8 maximum absolute RGB code-value change per pixel",
            "boundary_rgb_l1_before":seam_jumps[0],"boundary_rgb_l1_after":seam_jumps[1],
            "boundary_channel_count":seam_count,
            "boundary_warning":"jump magnitude is a diagnostic, not a quality score; real edges can align with blocks"
        }),
    )
}

fn load(encoded_path: &str) -> Result<(Vec<u8>, imij::ImageInfo, Vec<u8>, Layout)> {
    let encoded = fs::read(encoded_path)?;
    let (info, pixels) = imij::decode(&encoded)?;
    if info.sample != imij::SampleType::U8
        || ![imij::Channels::Rgb, imij::Channels::Rgba].contains(&info.channels)
    {
        return Err("timing requires RGB8/RGBA8".into());
    }
    if info.alpha_mode == imij::AlphaMode::Premultiplied {
        return Err("filter requires straight RGBA8".into());
    }
    let l = Layout {
        width: info.width as usize,
        height: info.height as usize,
        channels: info.channels.count(),
        stride_bytes: info.row_bytes(),
        origin: (0, 0),
    };
    l.validate(pixels.len())?;
    Ok((encoded, info, pixels, l))
}

fn time(a: &[String]) -> Result<()> {
    let threads: usize = a[3].parse()?;
    let rounds: usize = a[4].parse()?;
    if threads == 0 || rounds < 2 {
        return Err("positive threads and at least two rounds required".into());
    }
    let (encoded, info, pixels, l) = load(&a[1])?;
    let pool = Arc::new(
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()?,
    );
    let encoded = Arc::new(encoded);
    let pixels = Arc::new(pixels);
    // Verify each output outside timed loops, including threaded decode parity.
    for s in Strength::GRID {
        let mut expected = pixels.as_ref().clone();
        apply(&mut expected, l, s)?;
        let mut actual = vec![0; pixels.len()];
        pool.install(|| imij::decode_into_threads(&encoded, &mut actual, l.stride_bytes, threads))?;
        apply(&mut actual, l, s)?;
        if actual != expected {
            return Err("threaded decode/filter parity mismatch".into());
        }
        fs::write(format!("{}.verified-{}.raw", a[2], s.name()), &actual)?;
    }
    save(
        &format!("{}.metadata.json", a[2]),
        &serde_json::json!({
            "encoded":a[1],"width":info.width,"height":info.height,"channels":l.channels,
            "origin":[0,0],"decode_threads":threads,"filter_threads":1,"requested_rounds":rounds,
            "lab_instrumentation":cfg!(feature="lab"),"debug_assertions":cfg!(debug_assertions),
            "scratch_peak_bytes":0,"scratch_retained_bytes":0,"decoded_bytes":pixels.len(),
            "filter_only_setup":"fresh baseline pixels cloned outside timing; drop outside timing",
            "decode_operation":"decode into retained caller-owned output; decoder allocations/drop included",
            "filter_allocations":"none; fixed stack temporaries included in measured work",
            "decode_filter":"sequential filter after completed decode, zero crop origin"
        }),
    )?;
    let result = zenbench::run(|suite| {
        suite.group("postfilter", |g| {
            g.throughput(zenbench::Throughput::Bytes(pixels.len() as u64));
            let config = g.config();
            config.min_rounds = rounds;
            config.max_rounds = rounds;
            config.auto_rounds = false;
            config.max_time = Duration::from_secs(600);
            config.warmup_time = Duration::from_millis(100);
            for s in Strength::GRID {
                let bytes = encoded.clone();
                let pool = pool.clone();
                let mut out = vec![0; pixels.len()];
                // The off timing variant is pure baseline decode. Disabled-byte
                // identity is verified above, outside the measured loop.
                g.bench_tagged(
                    format!("decode-{}", s.name()),
                    &[("threads", &threads.to_string())],
                    move |b| {
                        b.iter(|| {
                            pool.install(|| {
                                imij::decode_into_threads(&bytes, &mut out, l.stride_bytes, threads)
                            })
                            .unwrap();
                            if s != Strength::Off {
                                apply(&mut out, l, s).unwrap();
                            }
                            black_box(&out);
                        });
                    },
                );
                let source = pixels.clone();
                g.bench_tagged(
                    format!("filter-{}", s.name()),
                    &[("threads", "1")],
                    move |b| {
                        let source = source.clone();
                        b.with_input(move || source.as_ref().clone())
                            .run(move |mut out| {
                                apply(&mut out, l, s).unwrap();
                                out
                            });
                    },
                );
            }
        });
    });
    result.save(Path::new(&a[2]))?;
    Ok(())
}

fn probe(a: &[String]) -> Result<()> {
    // Fresh-process memory probe: compressed input and output are reported
    // separately. Do not prepare or score images in this process.
    let bytes = fs::read(&a[1])?;
    let info = imij::header(&bytes)?.info;
    if info.sample != imij::SampleType::U8
        || ![imij::Channels::Rgb, imij::Channels::Rgba].contains(&info.channels)
    {
        return Err("probe requires RGB8/RGBA8".into());
    }
    let threads: usize = a[3].parse()?;
    let repetitions: usize = a[4].parse()?;
    if threads == 0 || repetitions == 0 {
        return Err("positive threads/repetitions required".into());
    }
    let l = Layout {
        width: info.width as usize,
        height: info.height as usize,
        channels: info.channels.count(),
        stride_bytes: info.row_bytes(),
        origin: (0, 0),
    };
    let s = Strength::parse(&a[2])?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()?;
    let mut out = vec![0; l.stride_bytes * l.height];
    for _ in 0..repetitions {
        pool.install(|| imij::decode_into_threads(&bytes, &mut out, l.stride_bytes, threads))?;
        apply(&mut out, l, s)?;
        black_box(&out);
    }
    println!(
        "{}",
        serde_json::json!({"compressed_bytes":bytes.len(),"output_capacity_bytes":out.capacity(),"filter_scratch_peak_bytes":0,"filter_scratch_retained_bytes":0,"threads":threads,"repetitions":repetitions,"strength":s.name()})
    );
    Ok(())
}

fn main() -> Result<()> {
    let a: Vec<_> = std::env::args()
        .skip(1)
        .filter(|v| v != "--bench")
        .collect();
    match a.first().map(String::as_str) {
        Some("apply") if a.len()==10=>raw_apply(&a),
        Some("batch") if a.len()==3=>batch(&a[1], &a[2]),
        Some("time") if a.len()==5=>time(&a),
        Some("probe") if a.len()==5=>probe(&a),
        _=>Err("usage: postfilter apply INPUT.raw OUTPUT.raw W H CHANNELS STRIDE_BYTES ORIGIN_X ORIGIN_Y off|mild|medium|strong; batch CASES.json NEW_OUTPUT_DIR; time INPUT.imi OUTPUT.json THREADS ROUNDS; probe INPUT.imi STRENGTH THREADS REPETITIONS".into())
    }
}

fn batch(input: &str, output: &str) -> Result<()> {
    let cases: Vec<serde_json::Value> = serde_json::from_slice(&fs::read(input)?)?;
    if Path::new(output).exists() {
        return Err("batch output directory already exists".into());
    }
    fs::create_dir_all(output)?;
    let mut records = Vec::new();
    for (index, case) in cases.into_iter().enumerate() {
        let decoded = case["decoded"]
            .as_str()
            .ok_or("case missing decoded path")?;
        let width = case["width"].as_u64().ok_or("case missing width")?;
        let height = case["height"].as_u64().ok_or("case missing height")?;
        let stride = case["stride_bytes"].as_u64().ok_or("case missing stride")?;
        if stride != width.checked_mul(3).ok_or("width overflow")?
            || fs::metadata(decoded)?.len()
                != stride.checked_mul(height).ok_or("extent overflow")?
        {
            return Err("batch requires exact tight RGB8 file extent".into());
        }
        for strength in Strength::GRID {
            let path = format!("{output}/{index:06}-{}.rgb8", strength.name());
            let args = [
                "apply".into(),
                decoded.into(),
                path.clone(),
                width.to_string(),
                height.to_string(),
                "3".into(),
                stride.to_string(),
                "0".into(),
                "0".into(),
                strength.name().into(),
            ];
            raw_apply(&args)?;
            let mut record = case.clone();
            record["baseline_decoded"] = decoded.into();
            // The output pixels have changed. Never carry a baseline hash under
            // the new decoded path; the run coordinator hashes every artifact.
            if let Some(hash) = record.as_object_mut().unwrap().remove("decoded_sha256") {
                record["baseline_decoded_sha256"] = hash;
            }
            record["decoded"] = path.into();
            record["filter_strength"] = strength.name().into();
            record["filter_origin"] = serde_json::json!([0, 0]);
            record["config_id"] = format!(
                "{}-postfilter-{}",
                case["config_id"].as_str().ok_or("missing config_id")?,
                strength.name()
            )
            .into();
            records.push(record);
        }
        save(&format!("{output}/cases.json"), &records)?;
        eprintln!("postfilter completed case {index}: {}", case["case_id"]);
    }
    Ok(())
}
