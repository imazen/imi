//! Native-only measurement driver; the controller owns host-idle admission.
//! `time INPUT.imi INPUT.xyb.f32 OUT.json THREADS` or `verify` with same args.
//! XYB input is three tightly packed f32 LE planes, in codec Y/X/B−Y order,
//! extracted before RGB conversion. Its dimensions must match INPUT.imi.
//! With `lab`, also supports paired Gaborish `encode`, `decode`, and `export`.
#![forbid(unsafe_code)]
#[cfg(feature = "lab")]
#[path = "../src/gaborish_lab.rs"]
mod gaborish_lab;
#[path = "../src/restoration.rs"]
#[allow(dead_code)]
mod restoration;
use restoration::{FloatFilter, Kernel, Layout, Strength};
use std::{error::Error, fs, hint::black_box, path::Path, sync::Arc, time::Duration};
type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn save(path: &str, value: &impl serde::Serialize) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}
fn float_bytes(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}
fn float_planes(
    input: &[f32],
    out: &mut [f32],
    l: Layout,
    f: FloatFilter,
    k: Kernel,
    parallel: bool,
) {
    let n = l.width * l.height;
    for (src, dst) in input.chunks_exact(n).zip(out.chunks_exact_mut(n)) {
        restoration::plane(src, dst, l.width, l.height, l.width, f, k, parallel).unwrap();
    }
}

#[cfg(feature = "lab")]
fn paired_command(args: &[String]) -> Result<bool> {
    use std::io::Write;
    match args.first().map(String::as_str) {
        Some("encode") => {
            if args.len() != 9 {
                return Err("usage: restoration_probe encode RGB.raw WIDTH HEIGHT STRIDE_BYTES QUALITY xyb|ycocg off|on OUTPUT.zqgb".into());
            }
            let width = args[2].parse()?;
            let height = args[3].parse()?;
            let stride = args[4].parse()?;
            imij::chroma_lab::checked_geometry(width, height)?;
            let quality = args[5].parse()?;
            let color = match args[6].as_str() {
                "xyb" => imij::LossyColor::Xyb,
                "ycocg" => imij::LossyColor::YCoCg,
                _ => return Err("color must be xyb or ycocg".into()),
            };
            let enabled = match args[7].as_str() {
                "off" => false,
                "on" => true,
                _ => return Err("filter must be off or on".into()),
            };
            if fs::metadata(&args[1])?.len() > 512 * 1024 * 1024 {
                return Err("raw input exceeds lab byte cap".into());
            }
            let rgb = fs::read(&args[1])?;
            let encoded =
                gaborish_lab::encode(&rgb, stride, width, height, quality, color, enabled)?;
            fs::write(&args[8], &encoded)?;
            save(
                &format!("{}.json", args[8]),
                &serde_json::json!({
                    "input":args[1], "width":width, "height":height,
                    "input_stride_bytes":stride, "quality":quality, "color":args[6],
                    "filter_enabled":enabled, "encoded_bytes":encoded.len(),
                    "wrapper_bytes":gaborish_lab::HEADER_BYTES,
                    "profile":"paired approximate inverse5x5 then native8x8; decoder Gaborish3x3 before inverse color",
                    "boundaries":"replicate source to padded8x8 grid, mirror filter at padded boundary, crop RGB output"
                }),
            )?;
        }
        Some("decode") => {
            if args.len() != 3 {
                return Err("usage: restoration_probe decode INPUT.zqgb OUTPUT.rgb".into());
            }
            let encoded = fs::read(&args[1])?;
            let decoded = gaborish_lab::decode(&encoded)?;
            fs::write(&args[2], &decoded.pixels)?;
            save(
                &format!("{}.json", args[2]),
                &serde_json::json!({
                    "input":args[1], "width":decoded.width, "height":decoded.height,
                    "stride_bytes":decoded.stride, "pixel_format":"RGB8 sRGB",
                    "encoded_bytes":encoded.len(), "decoded_bytes":decoded.pixels.len()
                }),
            )?;
        }
        Some("export") => {
            if args.len() != 3 {
                return Err("usage: restoration_probe export INPUT.zqgb PREFIX".into());
            }
            let encoded = fs::read(&args[1])?;
            let image = gaborish_lab::pre_gaborish_planes(&encoded)?;
            let padded_path = format!("{}.padded.f32", args[2]);
            let cropped_path = format!("{}.cropped.f32", args[2]);
            let native_path = format!("{}.native.imi", args[2]);
            let mut padded = std::io::BufWriter::new(fs::File::create(&padded_path)?);
            let mut cropped = std::io::BufWriter::new(fs::File::create(&cropped_path)?);
            for plane in &image.planes {
                for value in plane {
                    padded.write_all(&value.to_le_bytes())?;
                }
                for row in plane.chunks_exact(image.stride).take(image.height) {
                    for value in &row[..image.width] {
                        cropped.write_all(&value.to_le_bytes())?;
                    }
                }
            }
            padded.flush()?;
            cropped.flush()?;
            fs::write(&native_path, &encoded[gaborish_lab::HEADER_BYTES..])?;
            let color = match image.color {
                imij::LossyColor::Xyb => "xyb",
                imij::LossyColor::YCoCg => "ycocg",
                _ => unreachable!("validated lab profile"),
            };
            save(
                &format!("{}.export.json", args[2]),
                &serde_json::json!({
                    "input":args[1], "encoded_bytes":encoded.len(), "color":color,
                    "plane_order":if color == "xyb" { "Y,X,B-Y" } else { "Y,Co,Cg" },
                    "representation":"three sequential f32 little-endian planes before decoder Gaborish and inverse color",
                    "padded":{"path":padded_path,"width":image.padded_width,"height":image.padded_height,"stride_samples":image.stride},
                    "cropped":{"path":cropped_path,"width":image.width,"height":image.height,"stride_samples":image.width,"origin":[0,0]},
                    "native_inner":native_path,
                    "boundary_note":"paired wrapper mirrors at padded bounds; native timing of cropped planes mirrors at original bounds and is isolated kernel cost",
                    "hashes":"coordinator must hash generated files together with frozen executable/source inventory"
                }),
            )?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    #[cfg(feature = "lab")]
    if paired_command(&args)? {
        return Ok(());
    }
    if args.len() != 5 || !matches!(args[0].as_str(), "time" | "verify") {
        return Err(
            "usage: restoration_probe time|verify INPUT.imi INPUT.xyb.f32 OUT.json THREADS".into(),
        );
    }
    let threads: usize = args[4].parse()?;
    if ![1, 4].contains(&threads) {
        return Err("threads must be1 or4".into());
    }
    let bytes = fs::read(&args[1])?;
    let (info, pixels) = imij::decode(&bytes)?;
    if info.sample != imij::SampleType::U8
        || !matches!(info.channels, imij::Channels::Rgb | imij::Channels::Rgba)
        || (info.alpha_mode == imij::AlphaMode::Premultiplied)
    {
        return Err("requires SDR RGB8/straightRGBA8".into());
    }
    let l = Layout {
        width: info.width as usize,
        height: info.height as usize,
        stride_bytes: info.row_bytes(),
        channels: info.channels.count(),
        origin: (0, 0),
    };
    let raw = fs::read(&args[2])?;
    let count = l
        .width
        .checked_mul(l.height)
        .and_then(|n| n.checked_mul(3))
        .ok_or("planar extent overflow")?;
    if raw.len() != count.checked_mul(4).ok_or("planar byte overflow")? {
        return Err("XYB file length does not match image".into());
    }
    let planes: Vec<f32> = raw
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    if planes.iter().any(|v| !v.is_finite()) {
        return Err("nonfinite XYB input".into());
    }
    let pool = Arc::new(
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()?,
    );
    let mut verification = Vec::new();
    for strength in Strength::GRID {
        let mut expected = pixels.clone();
        restoration::seam_filter(&mut expected, l, strength, Kernel::Scalar, false)?;
        for kernel in Kernel::available() {
            for parallel in [false, true] {
                let mut actual = vec![0; pixels.len()];
                pool.install(|| {
                    imij::decode_into_threads(&bytes, &mut actual, l.stride_bytes, threads)
                })?;
                pool.install(|| {
                    restoration::seam_filter(&mut actual, l, strength, kernel, parallel)
                })?;
                if actual != expected {
                    return Err("seam tier/schedule parity mismatch".into());
                }
                let path = format!(
                    "{}.verified-seam-{}-{}-{}.raw",
                    args[3],
                    strength.name(),
                    kernel.name(),
                    if parallel { "parallel" } else { "serial" }
                );
                fs::write(&path, &actual)?;
                verification.push(path);
            }
        }
    }
    for f in [FloatFilter::Gaborish, FloatFilter::Inverse] {
        let mut expected = vec![0.0; count];
        float_planes(&planes, &mut expected, l, f, Kernel::Scalar, false);
        for kernel in Kernel::float_available() {
            for parallel in [false, true] {
                let mut actual = vec![0.0; count];
                pool.install(|| float_planes(&planes, &mut actual, l, f, kernel, parallel));
                if actual
                    .iter()
                    .zip(&expected)
                    .any(|(a, b)| a.to_bits() != b.to_bits())
                {
                    return Err("float tier/schedule parity mismatch".into());
                }
                let path = format!(
                    "{}.verified-{}-{}-{}.f32",
                    args[3],
                    f.name(),
                    kernel.name(),
                    if parallel { "parallel" } else { "serial" }
                );
                fs::write(&path, float_bytes(&actual))?;
                verification.push(path);
            }
        }
    }
    save(
        &format!("{}.metadata.json", args[3]),
        &serde_json::json!({
            "encoded":args[1],"xyb_planes":args[2],"xyb_order":"Y,X,B-Y, planar f32LE before RGB conversion","width":l.width,"height":l.height,"threads":threads,"requested_rounds":12,"requested_max_iterations":16,
            "kernels":Kernel::available().iter().map(|k|k.name()).collect::<Vec<_>>(),"verification":verification,
            "float_kernels":Kernel::float_available().iter().map(|k|k.name()).collect::<Vec<_>>(),
            "lab_instrumentation":cfg!(feature="lab"),"debug_assertions":cfg!(debug_assertions),
            "float_arithmetic":"canonical f32 fused multiply-add, mirrored image boundaries",
            "filter_only_setup":"fresh pixel/output allocation outside timing; input/output drop outside timing",
            "scratch":"seam zero heap; planar caller-owned output and bounded stack edges; Rayon bookkeeping may allocate",
            "fusion":"seam follows complete RGB decode; Gaborish is isolated planar kernel, not integrated decode timing",
            "gate":"four-thread groups have no automatic gate; controller must admit idle host",
            "scope":"kernel costs and exact tier parity; no RD, default adoption or universal optimized ceiling claim"
        }),
    )?;
    if args[0] == "verify" {
        return Ok(());
    }
    let bytes = Arc::new(bytes);
    let pixels = Arc::new(pixels);
    let planes = Arc::new(planes);
    let result = zenbench::run(|suite| {
        suite.group("decode-plus-seam", |g| {
            let c = g.config();
            c.min_rounds = 12;
            c.max_rounds = 12;
            c.auto_rounds = false;
            c.max_iterations = 16;
            c.baseline_only = Some(false);
            c.max_time = Duration::from_secs(600);
            c.warmup_time = Duration::from_millis(100);
            g.throughput(zenbench::Throughput::Bytes(pixels.len() as u64));
            let mut settings = vec![(Strength::Off, Kernel::Scalar, false)];
            for s in [Strength::Mild, Strength::Medium, Strength::Strong] {
                for k in Kernel::available() {
                    for parallel in [false, true] {
                        settings.push((s, k, parallel));
                    }
                }
            }
            for (s, k, parallel) in settings {
                let bytes = bytes.clone();
                let pool = pool.clone();
                let mut out = vec![0; pixels.len()];
                let name = if s == Strength::Off {
                    "decode-off".into()
                } else {
                    format!(
                        "decode-{}-{}-{}",
                        s.name(),
                        k.name(),
                        if parallel { "parallel" } else { "serial" }
                    )
                };
                g.bench_tagged(name, &[("threads", &threads.to_string())], move |b| {
                    b.iter(|| {
                        pool.install(|| {
                            imij::decode_into_threads(&bytes, &mut out, l.stride_bytes, threads)
                                .unwrap();
                            restoration::seam_filter(&mut out, l, s, k, parallel).unwrap();
                            black_box(&out);
                        });
                    });
                });
            }
        });
        suite.group("seam-only", |g| {
            let c = g.config();
            c.min_rounds = 12;
            c.max_rounds = 12;
            c.auto_rounds = false;
            c.max_iterations = 16;
            c.baseline_only = Some(false);
            c.max_time = Duration::from_secs(600);
            c.warmup_time = Duration::from_millis(100);
            g.throughput(zenbench::Throughput::Bytes(pixels.len() as u64));
            for s in Strength::GRID {
                for k in Kernel::available() {
                    for parallel in [false, true] {
                        if s == Strength::Off && (parallel || k.name() != "scalar") {
                            continue;
                        }
                        let source = pixels.clone();
                        let pool = pool.clone();
                        g.bench_tagged(
                            format!(
                                "seam-{}-{}-{}",
                                s.name(),
                                k.name(),
                                if parallel { "parallel" } else { "serial" }
                            ),
                            &[("threads", &threads.to_string())],
                            move |b| {
                                let source = source.clone();
                                let pool = pool.clone();
                                b.with_input(move || source.as_ref().clone()).run(
                                    move |mut out| {
                                        pool.install(|| {
                                            restoration::seam_filter(&mut out, l, s, k, parallel)
                                        })
                                        .unwrap();
                                        black_box(&out);
                                        out
                                    },
                                );
                            },
                        );
                    }
                }
            }
        });
        suite.group("planar-float", |g| {
            let c = g.config();
            c.min_rounds = 12;
            c.max_rounds = 12;
            c.auto_rounds = false;
            c.max_iterations = 16;
            c.baseline_only = Some(false);
            c.max_time = Duration::from_secs(600);
            c.warmup_time = Duration::from_millis(100);
            g.throughput(zenbench::Throughput::Bytes((count * 4) as u64));
            let source = planes.clone();
            let copy_pool = pool.clone();
            g.bench_tagged("copy", &[("threads", &threads.to_string())], move |b| {
                let source = source.clone();
                let pool = copy_pool.clone();
                b.with_input(move || vec![0.0; count]).run(move |mut out| {
                    pool.install(|| out.copy_from_slice(&source));
                    black_box(&out);
                    out
                });
            });
            for f in [FloatFilter::Gaborish, FloatFilter::Inverse] {
                for k in Kernel::float_available() {
                    for parallel in [false, true] {
                        let source = planes.clone();
                        let pool = pool.clone();
                        g.bench_tagged(
                            format!(
                                "{}-{}-{}",
                                f.name(),
                                k.name(),
                                if parallel { "parallel" } else { "serial" }
                            ),
                            &[("threads", &threads.to_string())],
                            move |b| {
                                let source = source.clone();
                                let pool = pool.clone();
                                b.with_input(move || vec![0.0; count]).run(move |mut out| {
                                    pool.install(|| {
                                        float_planes(&source, &mut out, l, f, k, parallel)
                                    });
                                    black_box(&out);
                                    out
                                });
                            },
                        );
                    }
                }
            }
        });
    });
    result.save(Path::new(&args[3]))?;
    let inspection:Vec<_>=result.comparisons.iter().map(|g|serde_json::json!({"group":g.group_name,"completed_rounds":g.completed_rounds,"samples":g.samples.len(),"orders":g.samples.iter().map(|s|&s.execution_order).collect::<Vec<_>>(),"iterations":g.samples.iter().map(|s|s.iterations).collect::<Vec<_>>(),"gate_clean":g.samples.iter().map(|s|s.gate_clean).collect::<Vec<_>>() })).collect();
    save(
        &format!("{}.inspection.json", args[3]),
        &serde_json::json!({"unreliable":result.unreliable,"gate_waits":result.gate_waits,"groups":inspection}),
    )?;
    for g in &result.comparisons {
        if g.completed_rounds != 12
            || g.samples.len() != 12
            || g.samples.iter().any(|s| {
                let mut order = s.execution_order.clone();
                order.sort_unstable();
                s.iterations == 0
                    || s.elapsed_ns.len() != g.benchmarks.len()
                    || order != (0..g.benchmarks.len()).collect::<Vec<_>>()
            })
        {
            return Err("incomplete rounds/samples/permutations; inspect saved output".into());
        }
    }
    Ok(())
}
