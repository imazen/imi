//! Native harness for decoder-only noise; no encoder or fitted parameters.
#![forbid(unsafe_code)]
#[path = "../src/noise.rs"]
mod noise;

use noise::{Kernel, Layout, Parameters, Variant, apply, apply_kernel, apply_parallel_kernel};
use rayon::prelude::*;
use std::{error::Error, fs, hint::black_box, path::Path, sync::Arc, time::Duration};
type Result<T> = std::result::Result<T, Box<dyn Error>>;
const STRIPE_ROWS: usize = 8;

fn save(path: &str, value: &impl serde::Serialize) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

fn load(path: &str) -> Result<(Vec<u8>, Vec<u8>, Layout)> {
    let encoded = fs::read(path)?;
    let (info, pixels) = imij::decode(&encoded)?;
    if info.sample != imij::SampleType::U8
        || ![imij::Channels::Rgb, imij::Channels::Rgba].contains(&info.channels)
        || (info.alpha_mode == imij::AlphaMode::Premultiplied)
    {
        return Err("requires SDR RGB8 or straight RGBA8".into());
    }
    let l = Layout {
        width: info.width as usize,
        height: info.height as usize,
        channels: info.channels.count(),
        stride_bytes: info.row_bytes(),
        origin: (0, 0),
    };
    l.validate(pixels.len())?;
    Ok((encoded, pixels, l))
}

fn synth(out: &mut [u8], l: Layout, p: &Parameters, v: Variant, parallel: bool, kernel: Kernel) {
    if parallel {
        apply_parallel_kernel(out, l, p, v, STRIPE_ROWS, kernel).unwrap();
    } else {
        apply_kernel(out, l, p, v, kernel).unwrap();
    }
}

fn verify(
    encoded: &[u8],
    pixels: &[u8],
    l: Layout,
    pool: &rayon::ThreadPool,
    threads: usize,
    output: &str,
) -> Result<serde_json::Value> {
    let p = Parameters::fixed();
    let mut records = Vec::new();
    let mut decoded = vec![0; pixels.len()];
    pool.install(|| imij::decode_into_threads(encoded, &mut decoded, l.stride_bytes, threads))?;
    if decoded != pixels {
        return Err("threaded decoder baseline mismatch".into());
    }
    fs::write(format!("{output}.verified-off.raw"), &decoded)?;
    for v in Variant::ALL {
        let mut expected = pixels.to_vec();
        apply(&mut expected, l, &p, v)?;
        let mut actual = decoded.clone();
        for kernel in Kernel::available() {
            for parallel in [false, true] {
                actual.copy_from_slice(&decoded);
                pool.install(|| synth(&mut actual, l, &p, v, parallel, kernel));
                if actual != expected {
                    return Err("scalar/SIMD serial/parallel parity mismatch".into());
                }
                fs::write(
                    format!(
                        "{output}.verified-{}-{}-{}.raw",
                        v.name(),
                        kernel.name(),
                        if parallel { "parallel" } else { "serial" }
                    ),
                    &actual,
                )?;
            }
        }
        let mut changed = 0usize;
        let mut endpoint_changes = 0usize;
        let mut max_delta = 0u8;
        let mut rgb_delta_sum = 0u64;
        for (before, after) in pixels
            .chunks_exact(l.channels)
            .zip(actual.chunks_exact(l.channels))
        {
            changed += usize::from(before[..3] != after[..3]);
            for c in 0..3 {
                let d = before[c].abs_diff(after[c]);
                max_delta = max_delta.max(d);
                rgb_delta_sum += u64::from(d);
                endpoint_changes += usize::from((before[c] == 0 || before[c] == 255) && d != 0);
            }
            if l.channels == 4 && before[3] != after[3] {
                return Err("alpha changed".into());
            }
        }
        if changed == 0 || max_delta > 12 {
            return Err("neutral or unbounded workload".into());
        }
        let path = format!("{output}.verified-{}.raw", v.name());
        fs::write(&path, &actual)?;
        records.push(serde_json::json!({"variant":v.name(),"verified_output":path,"changed_pixels":changed,"endpoint_channel_changes":endpoint_changes,"maximum_rgb_delta":max_delta,"rgb_absolute_delta_sum":rgb_delta_sum}));
    }
    Ok(serde_json::json!(records))
}

fn time(a: &[String], measure: bool) -> Result<()> {
    let threads: usize = a[3].parse()?;
    if ![1, 4].contains(&threads) {
        return Err("native grid requires threads 1 or 4".into());
    }
    let (encoded, pixels, l) = load(&a[1])?;
    let pool = Arc::new(
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()?,
    );
    let verification = verify(&encoded, &pixels, l, &pool, threads, &a[2])?;
    let metadata = serde_json::json!({
        "encoded":a[1],"width":l.width,"height":l.height,"channels":l.channels,"stride_bytes":l.stride_bytes,"origin":l.origin,
        "threads":threads,"requested_rounds":12,"noise_tiers":Kernel::available().iter().map(|k|k.name()).collect::<Vec<_>>(),"maximum_iterations_per_sample":16,"stripe_rows":STRIPE_ROWS,"seed":0x6d2b79f5u32,"amplitude_low":4,"amplitude_high":12,
        "verification":verification,"lab_instrumentation":cfg!(feature="lab"),"debug_assertions":cfg!(debug_assertions),
        "allocation_ownership":"each decode variant owns retained output; decoder scratch allocation/drop is timed; one prebuilt caller pool shared by decode and noise",
        "noise_only_setup":"fresh decoded pixels cloned before timed call; input/output allocation and drop outside timing; clone warms input",
        "copy_setup":"fresh same-size output allocated before timed call; copy and pool scheduling timed",
        "noise_scratch":"256-byte LUT retained in parameters; fixed stack temporaries; no full-image scratch; Rayon task bookkeeping may allocate",
        "fusion":"none: full decode completes before separate noise pass; serial and same-pool parallel are distinguished",
        "gate":"zenbench auto-disables groups tagged threads > 1; controller must verify idle host",
        "scope":"fixed synthetic noise cost only; no encoder, noise estimation, RD gain, universal ceiling or assumed 5% result"
    });
    save(&format!("{}.metadata.json", a[2]), &metadata)?;
    if !measure {
        return Ok(());
    }
    let encoded = Arc::new(encoded);
    let pixels = Arc::new(pixels);
    let p = Parameters::fixed();
    let result = zenbench::run(|suite| {
        // Separate groups pair decode variants against off, and noise variants
        // against their preparation/pool no-op control. All orders are shuffled
        // by zenbench within each of the twelve rounds.
        suite.group("decode-plus-noise", |g| {
            g.throughput(zenbench::Throughput::Bytes(pixels.len() as u64));
            let c = g.config();
            c.min_rounds = 12;
            c.max_rounds = 12;
            c.auto_rounds = false;
            c.max_iterations = 16;
            c.baseline_only = Some(false);
            c.max_time = Duration::from_secs(600);
            c.warmup_time = Duration::from_millis(100);
            let mut settings = vec![None];
            for v in Variant::ALL {
                for kernel in Kernel::available() {
                    for parallel in [false, true] {
                        settings.push(Some((v, parallel, kernel)));
                    }
                }
            }
            for setting in settings {
                let bytes = encoded.clone();
                let pool = pool.clone();
                let mut out = vec![0; pixels.len()];
                let name = setting.map_or("decode-off".into(), |(v, parallel, kernel)| {
                    format!(
                        "decode-{}-{}-{}",
                        v.name(),
                        kernel.name(),
                        if parallel { "parallel" } else { "serial" }
                    )
                });
                g.bench_tagged(name, &[("threads", &threads.to_string())], move |b| {
                    b.iter(|| {
                        pool.install(|| {
                            imij::decode_into_threads(&bytes, &mut out, l.stride_bytes, threads)
                                .unwrap();
                            if let Some((v, parallel, kernel)) = setting {
                                synth(&mut out, l, &p, v, parallel, kernel);
                            }
                            black_box(&out);
                        });
                    });
                });
            }
        });
        suite.group("noise-only", |g| {
            g.throughput(zenbench::Throughput::Bytes(pixels.len() as u64));
            let c = g.config();
            c.min_rounds = 12;
            c.max_rounds = 12;
            c.auto_rounds = false;
            c.max_iterations = 16;
            c.baseline_only = Some(false);
            c.max_time = Duration::from_secs(600);
            c.warmup_time = Duration::from_millis(100);
            for parallel in [false, true] {
                let label = if parallel { "parallel" } else { "serial" };
                let noop_pool = pool.clone();
                let source = pixels.clone();
                g.bench_tagged(
                    format!("noop-{label}"),
                    &[("threads", &threads.to_string())],
                    move |b| {
                        let source = source.clone();
                        let pool = noop_pool.clone();
                        b.with_input(move || source.as_ref().clone())
                            .run(move |mut out| {
                                pool.install(|| {
                                    if parallel {
                                        out.par_chunks_mut(l.stride_bytes * STRIPE_ROWS).for_each(
                                            |chunk| {
                                                black_box(chunk);
                                            },
                                        );
                                    } else {
                                        black_box(&mut out);
                                    }
                                });
                                out
                            });
                    },
                );
                let copy_pool = pool.clone();
                let source = pixels.clone();
                let len = pixels.len();
                g.bench_tagged(
                    format!("copy-{label}"),
                    &[("threads", &threads.to_string())],
                    move |b| {
                        let pool = copy_pool.clone();
                        let source = source.clone();
                        b.with_input(move || vec![0; len]).run(move |mut out| {
                            pool.install(|| {
                                if parallel {
                                    let chunk = l.stride_bytes * STRIPE_ROWS;
                                    out.par_chunks_mut(chunk).enumerate().for_each(|(i, dst)| {
                                        dst.copy_from_slice(
                                            &source[i * chunk..i * chunk + dst.len()],
                                        )
                                    });
                                } else {
                                    out.copy_from_slice(&source);
                                }
                                black_box(&out);
                            });
                            out
                        });
                    },
                );
                for (v, kernel) in Variant::ALL
                    .into_iter()
                    .flat_map(|v| Kernel::available().into_iter().map(move |k| (v, k)))
                {
                    let pool = pool.clone();
                    let source = pixels.clone();
                    g.bench_tagged(
                        format!("noise-{}-{}-{label}", v.name(), kernel.name()),
                        &[("threads", &threads.to_string())],
                        move |b| {
                            let source = source.clone();
                            let pool = pool.clone();
                            b.with_input(move || source.as_ref().clone())
                                .run(move |mut out| {
                                    pool.install(|| {
                                        synth(&mut out, l, &p, v, parallel, kernel);
                                        black_box(&out);
                                    });
                                    out
                                });
                        },
                    );
                }
            }
        });
    });
    result.save(Path::new(&a[2]))?;
    let inspection:Vec<_>=result.comparisons.iter().map(|g| serde_json::json!({
        "group":g.group_name,"completed_rounds":g.completed_rounds,"retained_samples":g.samples.len(),
        "round_iterations":g.samples.iter().map(|s| s.iterations).collect::<Vec<_>>(),
        "execution_orders":g.samples.iter().map(|s| &s.execution_order).collect::<Vec<_>>(),
        "gate_clean":g.samples.iter().map(|s| s.gate_clean).collect::<Vec<_>>()
    })).collect();
    save(
        &format!("{}.inspection.json", a[2]),
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
                    || s.compensated_ns.len() != g.benchmarks.len()
                    || order != (0..g.benchmarks.len()).collect::<Vec<_>>()
                    || (threads > 1 && s.gate_clean.is_some())
            })
            || g.samples
                .iter()
                .all(|s| s.execution_order == g.samples[0].execution_order)
        {
            return Err(
                "incomplete native rounds or retained samples; inspect saved output".into(),
            );
        }
    }
    Ok(())
}

fn raw(a: &[String]) -> Result<()> {
    let mut pixels = fs::read(&a[1])?;
    let l = Layout {
        width: a[3].parse()?,
        height: a[4].parse()?,
        channels: a[5].parse()?,
        stride_bytes: a[6].parse()?,
        origin: (a[7].parse()?, a[8].parse()?),
    };
    let p = Parameters::new(a[10].parse()?, a[11].parse()?, a[12].parse()?)?;
    apply(&mut pixels, l, &p, Variant::parse(&a[9])?)?;
    fs::write(&a[2], &pixels)?;
    Ok(())
}

fn main() -> Result<()> {
    let a: Vec<_> = std::env::args().skip(1).collect();
    match a.first().map(String::as_str) {
        Some("time") if a.len()==4 => time(&a,true),
        Some("verify") if a.len()==4 => time(&a,false),
        Some("apply") if a.len()==13 => raw(&a),
        _ => Err("usage: noise_probe time|verify INPUT.imi OUTPUT.json THREADS(1|4); apply INPUT.raw OUTPUT.raw W H CHANNELS STRIDE_BYTES ORIGIN_X ORIGIN_Y shared|rgb|template SEED AMP_LOW AMP_HIGH".into()),
    }
}
