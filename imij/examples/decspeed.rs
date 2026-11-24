//! Decode throughput of .imi files, best of N runs.
//!
//! Usage: decspeed [--iters N] [--threads T] [--stream] FILE...
//! `--stream`: row-at-a-time `StreamDecoder` into one row buffer (1 thread).
//! `--frames`: every frame of an animation (in place into the canvas),
//! reported per canvas pixel decoded.
//! `--as-one`: the files are strips of one image (simulated column groups):
//! decoded together, in parallel over files (one thread each) when
//! `--threads` > 1, reported as one image.
//! `--first-row`: `StreamDecoder` from the file bytes into one row buffer, no
//! whole-image decode or buffer: best time from `StreamDecoder::new` to the
//! first row, and best whole-image streaming time. `--iters 1` makes it a
//! single-decode heap probe.
//! Kernel selection follows the decoder's switches: `IMIJ_NO_AVX512` caps the
//! entropy kernels at AVX2, `IMIJ_SCALAR` forces the scalar reference.

use std::time::Instant;

fn main() {
    let mut iters = 20usize;
    let mut threads = 1usize;
    let mut stream = false;
    let mut as_one = false;
    let mut frames = false;
    let mut first_row = false;
    let mut files = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--iters" => iters = args.next().unwrap().parse().unwrap(),
            "--threads" => threads = args.next().unwrap().parse().unwrap(),
            "--stream" => stream = true,
            "--as-one" => as_one = true,
            "--frames" => frames = true,
            "--first-row" => first_row = true,
            _ => files.push(a),
        }
    }
    // IMIJ_ROWPAIRS=0/1/2 (lab): lossless rows one slot at a time / slots
    // 1 + 2 paired / also slot 0 of the next row (default).
    #[cfg(feature = "lab")]
    if let Ok(v) = std::env::var("IMIJ_ROWPAIRS") {
        imij::lab::set_lossless_row_pairs(v.parse().unwrap());
    }
    // COEFGAP_EOB_PROXY=1 (lab): run-level decodes do the extra work of EOB
    // contexts split by a per-block bucket (coefgap study decode-cost proxy).
    #[cfg(feature = "lab")]
    if let Ok(v) = std::env::var("COEFGAP_EOB_PROXY") {
        imij::lab::set_eob_bucket_proxy(true);
        // COEFGAP_EOB_PROXY=probe: every bucket offset 1 (decodes go wrong);
        // =pass / =loads: only the bucket pass / only the kernel loads.
        imij::lab::set_eob_bucket_probe((v == "probe") as u32);
        imij::lab::set_eob_bucket_proxy_mode(match v.as_str() {
            "pass" => 2,
            "loads" => 3,
            _ => 1,
        });
    }
    // IMIJ_KEEP_SCRATCH=1 (lab): lossy scratch kept across decodes.
    #[cfg(feature = "lab")]
    if std::env::var_os("IMIJ_KEEP_SCRATCH").is_some() {
        imij::lab::set_keep_scratch(true);
    }
    // Decode inside a rayon pool of the requested size (as a caller would).
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap();
    if frames {
        pool.install(|| run_frames(&files, iters, threads));
        return;
    }
    if as_one {
        pool.install(|| run_as_one(&files, iters, threads));
        return;
    }
    if first_row {
        pool.install(|| run_first_row(&files, iters));
        return;
    }
    pool.install(|| run(&files, iters, threads, stream));
}

fn run_frames(files: &[String], iters: usize, threads: usize) {
    for path in files {
        let data = std::fs::read(path).unwrap();
        let d = imij::FrameDecoder::new(&data).unwrap();
        let info = d.info().clone();
        let rb = info.row_bytes();
        let mut canvas = vec![0u8; rb * info.height as usize];
        let n = d.frame_count();
        let mut best = f64::MAX;
        for _ in 0..iters {
            let t = Instant::now();
            let mut d = imij::FrameDecoder::new(&data).unwrap();
            while d.next_frame().unwrap().is_some() {
                d.decode_into(&mut canvas, rb, threads).unwrap();
            }
            best = best.min(t.elapsed().as_secs_f64());
        }
        let px = info.width as f64 * info.height as f64 * n as f64;
        println!(
            "{path}: {n} frames, {:.1} frames/s, {:.1} Mpx/s (canvas pixels)",
            n as f64 / best,
            px / best / 1e6
        );
    }
}

fn run_first_row(files: &[String], iters: usize) {
    for path in files {
        let data = std::fs::read(path).unwrap();
        let (info, _) = imij::probe(&data).unwrap();
        let mut row = vec![0u8; info.row_bytes()];
        let (mut first, mut all) = (f64::MAX, f64::MAX);
        for _ in 0..iters {
            let t = Instant::now();
            let mut sd = imij::StreamDecoder::new(&data).unwrap();
            assert!(sd.next_row(&mut row).unwrap());
            first = first.min(t.elapsed().as_secs_f64());
            while sd.next_row(&mut row).unwrap() {}
            all = all.min(t.elapsed().as_secs_f64());
        }
        let px = info.width as f64 * info.height as f64;
        println!(
            "{path}: first row {:.1} us, stream {:.1} Mpx/s",
            first * 1e6,
            px / all / 1e6
        );
    }
}

fn run_as_one(files: &[String], iters: usize, threads: usize) {
    use rayon::prelude::*;
    let data: Vec<Vec<u8>> = files.iter().map(|f| std::fs::read(f).unwrap()).collect();
    let mut bufs: Vec<(imij::ImageInfo, Vec<u8>)> = data
        .iter()
        .map(|d| {
            let (info, _) = imij::decode(d).unwrap();
            let n = info.row_bytes() * info.height as usize;
            (info, vec![0u8; n])
        })
        .collect();
    let px: f64 = bufs
        .iter()
        .map(|(i, _)| i.width as f64 * i.height as f64)
        .sum();
    let mut best = f64::MAX;
    for _ in 0..iters {
        let t = Instant::now();
        if threads > 1 {
            bufs.par_iter_mut()
                .zip(data.par_iter())
                .for_each(|((i, b), d)| {
                    imij::decode_into_threads(d, b, i.row_bytes(), 1).unwrap();
                });
        } else {
            for ((i, b), d) in bufs.iter_mut().zip(&data) {
                imij::decode_into_threads(d, b, i.row_bytes(), 1).unwrap();
            }
        }
        best = best.min(t.elapsed().as_secs_f64());
    }
    let bytes: usize = data.iter().map(|d| d.len()).sum();
    println!(
        "total: {:.1} Mpx/s ({} strips as one image, {} threads, {} B)",
        px / best / 1e6,
        files.len(),
        threads,
        bytes
    );
}

fn run(files: &[String], iters: usize, threads: usize, stream: bool) {
    let mut total_px = 0f64;
    let mut total_t = 0f64;
    for path in files {
        let data = std::fs::read(path).unwrap();
        let (info, _) = imij::decode(&data).unwrap();
        let mut buf = vec![0u8; info.row_bytes() * info.height as usize];
        let mut best = f64::MAX;
        #[cfg(feature = "lab")]
        imij::lab::lossy_stage_ns();
        // IMIJ_DUMP=dir (lab): symbol streams for external coder benchmarks.
        #[cfg(feature = "lab")]
        if let Ok(dir) = std::env::var("IMIJ_DUMP") {
            let tag = std::path::Path::new(path)
                .file_stem()
                .unwrap()
                .to_string_lossy();
            print!("{}", imij::lab::lossy_dump_streams(&data, &dir, &tag));
            continue;
        }
        // IMIJ_LANES=1 (lab): per-lane scalar entropy decode vs the kernel.
        #[cfg(feature = "lab")]
        if std::env::var_os("IMIJ_LANES").is_some() {
            print!("{path}: {}", imij::lab::lossy_token_mix(&data));
            print!("{}", imij::lab::lossy_lane_bench(&data, iters));
            continue;
        }
        // IMIJ_AB_PAIRS=2,1 (lab): alternate stripe pairing (0 auto, 1 always,
        // 2 never) every iteration, report each best.
        #[cfg(feature = "lab")]
        if let Ok(list) = std::env::var("IMIJ_AB_PAIRS") {
            let set: Vec<u8> = list.split(',').map(|v| v.parse().unwrap()).collect();
            let mut b = vec![f64::MAX; set.len()];
            imij::lab::set_stage_timers(false);
            for _ in 0..iters {
                for (k, &v) in set.iter().enumerate() {
                    imij::lab::set_stripe_pairs(v);
                    let t = Instant::now();
                    imij::decode_into_threads(&data, &mut buf, info.row_bytes(), threads).unwrap();
                    b[k] = b[k].min(t.elapsed().as_secs_f64());
                }
            }
            imij::lab::set_stripe_pairs(0);
            imij::lab::set_stage_timers(true);
            let px = info.width as f64 * info.height as f64;
            let parts: Vec<String> = set
                .iter()
                .zip(&b)
                .map(|(v, t)| format!("pairs {v}: {:.1} Mpx/s", px / t / 1e6))
                .collect();
            println!("{path}: {}", parts.join(" | "));
            continue;
        }
        // IMIJ_AB_SHAREDC=2,1,0 (lab): alternate DC scheduling modes (2 DC
        // work items, 1 shared on demand, 0 per-item DC decodes) every
        // iteration, report each best.
        #[cfg(feature = "lab")]
        if let Ok(list) = std::env::var("IMIJ_AB_SHAREDC") {
            let set: Vec<u8> = list.split(',').map(|v| v.parse().unwrap()).collect();
            let mut b = vec![f64::MAX; set.len()];
            imij::lab::set_stage_timers(false);
            for _ in 0..iters {
                for (k, &mode) in set.iter().enumerate() {
                    imij::lab::set_shared_dc(mode);
                    let t = Instant::now();
                    imij::decode_into_threads(&data, &mut buf, info.row_bytes(), threads).unwrap();
                    b[k] = b[k].min(t.elapsed().as_secs_f64());
                }
            }
            imij::lab::set_shared_dc(2);
            imij::lab::set_stage_timers(true);
            let px = info.width as f64 * info.height as f64;
            let parts: Vec<String> = set
                .iter()
                .zip(&b)
                .map(|(m, t)| format!("dcmode {m}: {:.1} Mpx/s", px / t / 1e6))
                .collect();
            println!("{path}: {}", parts.join(" | "));
            continue;
        }
        // IMIJ_AB_DCSPLIT=8,0 (lab): alternate lossy stripes per work item
        // within a DC stripe (0 automatic) every iteration.
        #[cfg(feature = "lab")]
        if let Ok(list) = std::env::var("IMIJ_AB_DCSPLIT") {
            let set: Vec<usize> = list.split(',').map(|v| v.parse().unwrap()).collect();
            let mut b = vec![f64::MAX; set.len()];
            imij::lab::set_stage_timers(false);
            for _ in 0..iters {
                for (k, &v) in set.iter().enumerate() {
                    imij::lab::set_dc_split(v);
                    let t = Instant::now();
                    imij::decode_into_threads(&data, &mut buf, info.row_bytes(), threads).unwrap();
                    b[k] = b[k].min(t.elapsed().as_secs_f64());
                }
            }
            imij::lab::set_dc_split(0);
            imij::lab::set_stage_timers(true);
            let px = info.width as f64 * info.height as f64;
            let parts: Vec<String> = set
                .iter()
                .zip(&b)
                .map(|(v, t)| format!("split {v}: {:.1} Mpx/s", px / t / 1e6))
                .collect();
            println!("{path}: {}", parts.join(" | "));
            continue;
        }
        // IMIJ_AB_CHUNK=max,8,16 (lab): alternate reconstruction chunk widths
        // (blocks; max = whole block rows) every iteration, report each best.
        #[cfg(feature = "lab")]
        if let Ok(list) = std::env::var("IMIJ_AB_CHUNK") {
            let set: Vec<(String, usize)> = list
                .split(',')
                .map(|v| {
                    (
                        v.to_string(),
                        if v == "max" {
                            usize::MAX
                        } else {
                            v.parse().unwrap()
                        },
                    )
                })
                .collect();
            let mut b = vec![f64::MAX; set.len()];
            imij::lab::set_stage_timers(false);
            for _ in 0..iters {
                for (k, (_, n)) in set.iter().enumerate() {
                    imij::lab::set_recon_chunk(*n);
                    let t = Instant::now();
                    imij::decode_into_threads(&data, &mut buf, info.row_bytes(), threads).unwrap();
                    b[k] = b[k].min(t.elapsed().as_secs_f64());
                }
            }
            imij::lab::set_recon_chunk(0);
            imij::lab::set_stage_timers(true);
            let px = info.width as f64 * info.height as f64;
            let parts: Vec<String> = set
                .iter()
                .zip(&b)
                .map(|((name, _), t)| format!("chunk {name}: {:.1} Mpx/s", px / t / 1e6))
                .collect();
            println!("{path}: {}", parts.join(" | "));
            continue;
        }
        // IMIJ_AB=1 (lab): alternate the AVX-512 gather / no-gather entropy
        // kernels every iteration and report both bests.
        #[cfg(all(feature = "lab", target_arch = "x86_64"))]
        if std::env::var_os("IMIJ_AB").is_some() {
            let mut b = [f64::MAX; 2];
            for _ in 0..iters {
                for (k, on) in [true, false].into_iter().enumerate() {
                    imij::lab::set_avx512_gather(on);
                    let t = Instant::now();
                    imij::decode_into_threads(&data, &mut buf, info.row_bytes(), threads).unwrap();
                    b[k] = b[k].min(t.elapsed().as_secs_f64());
                }
            }
            imij::lab::set_avx512_gather(true);
            let px = info.width as f64 * info.height as f64;
            println!(
                "{path}: gather {:.1} Mpx/s | no-gather {:.1} Mpx/s",
                px / b[0] / 1e6,
                px / b[1] / 1e6
            );
            continue;
        }
        for _ in 0..iters {
            let t = Instant::now();
            if stream {
                let mut sd = imij::StreamDecoder::new(&data).unwrap();
                let row = &mut buf[..info.row_bytes()];
                while sd.next_row(row).unwrap() {}
            } else {
                imij::decode_into_threads(&data, &mut buf, info.row_bytes(), threads).unwrap();
            }
            best = best.min(t.elapsed().as_secs_f64());
        }
        let px = info.width as f64 * info.height as f64;
        total_px += px;
        total_t += best;
        println!(
            "{path}: {:.1} Mpx/s {:.1} MB/s {:.3} bpp",
            px / best / 1e6,
            buf.len() as f64 / best / 1e6,
            data.len() as f64 * 8.0 / px
        );
        #[cfg(feature = "lab")]
        {
            let ns = imij::lab::lossy_stage_ns();
            let all: u64 = ns.iter().sum();
            if all > 0 {
                let ms = |v: u64| v as f64 / iters as f64 / 1e6;
                println!(
                    "  stages ms/iter: dc {:.2} entropy {:.2} idct {:.2} color {:.2}",
                    ms(ns[0]),
                    ms(ns[1]),
                    ms(ns[2]),
                    ms(ns[3])
                );
                if ns[4] + ns[5] > 0 {
                    println!(
                        "  split-Huffman: side info {:.2} unpack {:.2}",
                        ms(ns[4]),
                        ms(ns[5])
                    );
                }
            }
        }
    }
    #[cfg(all(feature = "lab", target_arch = "x86_64"))]
    println!("entropy kernels: {:?}", imij::lab::avx512_kernel_choice());
    println!(
        "total: {:.1} Mpx/s ({} files, {} threads, tier {})",
        total_px / total_t / 1e6,
        files.len(),
        threads,
        imij::simd_tier()
    );
}
