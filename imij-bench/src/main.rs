mod ansmodel;
mod corpus;
mod cost;
mod lossy;
pub use ansmodel::{normalize, tans_cost};

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use imij::{AnalysisConfig as EncodeConfig, Channels, Metadata, SampleType};
use rayon::prelude::*;

use corpus::Img;

fn qoi_size(img: &Img) -> Option<usize> {
    if img.info.sample != SampleType::U8 {
        return None;
    }
    let layout = match img.info.channels {
        Channels::Rgb => zenbitmaps::PixelLayout::Rgb8,
        Channels::Rgba => zenbitmaps::PixelLayout::Rgba8,
        Channels::Gray => {
            // QOI has no gray: expand to RGB like a real user would.
            let rgb: Vec<u8> = img.pixels.iter().flat_map(|&v| [v, v, v]).collect();
            return zenbitmaps::encode_qoi(
                &rgb,
                img.info.width,
                img.info.height,
                zenbitmaps::PixelLayout::Rgb8,
                enough::Unstoppable,
            )
            .ok()
            .map(|v| v.len());
        }
        Channels::GrayAlpha => {
            let rgba: Vec<u8> = img
                .pixels
                .chunks_exact(2)
                .flat_map(|p| [p[0], p[0], p[0], p[1]])
                .collect();
            return zenbitmaps::encode_qoi(
                &rgba,
                img.info.width,
                img.info.height,
                zenbitmaps::PixelLayout::Rgba8,
                enough::Unstoppable,
            )
            .ok()
            .map(|v| v.len());
        }
    };
    zenbitmaps::encode_qoi(
        &img.pixels,
        img.info.width,
        img.info.height,
        layout,
        enough::Unstoppable,
    )
    .ok()
    .map(|v| v.len())
}

fn configs() -> Vec<(&'static str, EncodeConfig)> {
    let d = EncodeConfig::default;
    vec![
        ("base", d()),
        ("ctx1", EncodeConfig { max_ctx: 1, ..d() }),
        ("ctx2", EncodeConfig { max_ctx: 2, ..d() }),
        ("ctx8", EncodeConfig { max_ctx: 8, ..d() }),
        (
            "nocross",
            EncodeConfig {
                ctx_cross: false,
                ..d()
            },
        ),
        ("norun", EncodeConfig { run_min: 0, ..d() }),
        ("run2", EncodeConfig { run_min: 2, ..d() }),
        ("run8", EncodeConfig { run_min: 8, ..d() }),
        (
            "decor01",
            EncodeConfig {
                decor_mask: 0b11,
                ..d()
            },
        ),
        (
            "pred_wn_avg_grad",
            EncodeConfig {
                predictor_mask: 0b10111,
                ..d()
            },
        ),
        (
            "pred_med_only",
            EncodeConfig {
                predictor_mask: 0b1001,
                ..d()
            },
        ),
        (
            "pred_no_paeth_ne",
            EncodeConfig {
                predictor_mask: 0b11111,
                ..d()
            },
        ),
        (
            "len11",
            EncodeConfig {
                max_code_len: 11,
                ..d()
            },
        ),
        (
            "len10",
            EncodeConfig {
                max_code_len: 10,
                ..d()
            },
        ),
        (
            "stripe16",
            EncodeConfig {
                stripe_rows: Some(16),
                ..d()
            },
        ),
        (
            "stripe64",
            EncodeConfig {
                stripe_rows: Some(64),
                ..d()
            },
        ),
        (
            "stripe256",
            EncodeConfig {
                stripe_rows: Some(256),
                ..d()
            },
        ),
    ]
}

fn cmd_sizes(manifest: &str, filter: Option<&str>, out_path: &str) {
    let list = corpus::load_manifest(manifest, filter);
    let cfgs = configs();
    let rows: Vec<(
        String,
        String,
        usize,
        usize,
        Option<usize>,
        Vec<usize>,
        String,
    )> = list
        .par_iter()
        .flat_map_iter(|(class, path)| {
            let img = match corpus::load_png(class, Path::new(path)) {
                Ok(i) => i,
                Err(e) => {
                    eprintln!("skip {e}");
                    return Vec::new();
                }
            };
            let mut imgs = corpus::variants(&img);
            imgs.insert(0, img);
            imgs.into_iter()
                .map(|im| {
                    let qoi = qoi_size(&im);
                    let sizes: Vec<usize> = cfgs
                        .iter()
                        .map(|(_, c)| {
                            imij::encode_huffman(
                                &im.pixels,
                                im.info.row_bytes(),
                                &im.info,
                                &Metadata::default(),
                                c,
                            )
                            .unwrap()
                            .len()
                        })
                        .collect();
                    let a = imij::analyze(
                        &im.pixels,
                        im.info.row_bytes(),
                        &im.info,
                        &EncodeConfig::default(),
                    )
                    .unwrap();
                    let maps: Vec<String> = a
                        .maps()
                        .iter()
                        .map(|m| format!("{:?}{}", m.mode, m.bits))
                        .collect();
                    eprintln!(
                        "{} {} {}x{} {:?}",
                        im.class, im.name, im.info.width, im.info.height, maps
                    );
                    (
                        im.class.clone(),
                        im.name.clone(),
                        im.pixels_count(),
                        im.png_bytes,
                        qoi,
                        sizes,
                        maps.join(","),
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect();

    let mut f = std::fs::File::create(out_path).unwrap();
    write!(f, "class\timage\tpixels\tpng\tqoi").unwrap();
    for (n, _) in &cfgs {
        write!(f, "\t{n}").unwrap();
    }
    writeln!(f, "\tmaps").unwrap();
    for r in &rows {
        write!(
            f,
            "{}\t{}\t{}\t{}\t{}",
            r.0,
            r.1,
            r.2,
            r.3,
            r.4.map(|v| v.to_string()).unwrap_or_default()
        )
        .unwrap();
        for s in &r.5 {
            write!(f, "\t{s}").unwrap();
        }
        writeln!(f, "\t{}", r.6).unwrap();
    }

    // Per-class summary: bits per pixel.
    let mut by: BTreeMap<String, (usize, usize, usize, Option<usize>, Vec<usize>)> =
        BTreeMap::new();
    for r in &rows {
        let e = by
            .entry(r.0.clone())
            .or_insert((0, 0, 0, Some(0), vec![0; cfgs.len()]));
        e.0 += 1;
        e.1 += r.2;
        e.2 += r.3;
        e.3 = match (e.3, r.4) {
            (Some(a), Some(b)) => Some(a + b),
            _ => None,
        };
        for (i, s) in r.5.iter().enumerate() {
            e.4[i] += s;
        }
    }
    print!("{:<10} {:>3} {:>7} {:>7}", "class", "n", "png", "qoi");
    for (n, _) in &cfgs {
        print!(" {:>8}", &n[..n.len().min(8)]);
    }
    println!();
    for (c, (n, px, png, qoi, s)) in &by {
        let bpp = |b: usize| b as f64 * 8.0 / *px as f64;
        print!(
            "{c:<10} {n:>3} {:>7.3} {:>7}",
            bpp(*png),
            qoi.map(|q| format!("{:.3}", bpp(q))).unwrap_or("-".into())
        );
        for v in s {
            print!(" {:>8.3}", bpp(*v));
        }
        println!();
    }
}

fn cmd_cost(manifest: &str, filter: Option<&str>, out_path: &str) {
    let list = corpus::load_manifest(manifest, filter);
    let rows: Vec<(String, usize, Vec<f64>)> = list
        .par_iter()
        .flat_map_iter(|(class, path)| {
            let img = match corpus::load_png(class, Path::new(path)) {
                Ok(i) => i,
                Err(e) => {
                    eprintln!("skip {e}");
                    return Vec::new();
                }
            };
            let mut imgs = corpus::variants(&img);
            imgs.insert(0, img);
            imgs.into_iter()
                .map(|im| {
                    let cfg = EncodeConfig::default();
                    let huff = imij::encode_huffman(
                        &im.pixels,
                        im.info.row_bytes(),
                        &im.info,
                        &Metadata::default(),
                        &cfg,
                    )
                    .unwrap()
                    .len();
                    let a = imij::analyze(&im.pixels, im.info.row_bytes(), &im.info, &cfg).unwrap();
                    let c1 = cost::evaluate(&a, 1);
                    let c16 = cost::evaluate(&a, 16);
                    let qoi = qoi_size(&im).map(|q| q as f64 * 8.0).unwrap_or(f64::NAN);
                    eprintln!("{} {}", im.class, im.name);
                    (
                        im.class.clone(),
                        im.pixels_count(),
                        vec![
                            qoi,
                            huff as f64 * 8.0,
                            c1.rice_bits as f64,
                            c1.rice_norun_bits as f64,
                            c1.entropy_bits,
                            c16.rice_bits as f64,
                            c16.entropy_bits,
                        ],
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let names = [
        "qoi",
        "huff4ctx",
        "rice",
        "rice_norun",
        "ent18",
        "rice_seg16",
        "ent18_seg16",
    ];
    let mut f = std::fs::File::create(out_path).unwrap();
    writeln!(f, "class\tpixels\t{}", names.join("\t")).unwrap();
    let mut by: BTreeMap<String, (usize, usize, Vec<f64>)> = BTreeMap::new();
    for r in &rows {
        writeln!(
            f,
            "{}\t{}\t{}",
            r.0,
            r.1,
            r.2.iter()
                .map(|v| format!("{v:.0}"))
                .collect::<Vec<_>>()
                .join("\t")
        )
        .unwrap();
        let e = by
            .entry(r.0.clone())
            .or_insert((0, 0, vec![0.0; names.len()]));
        e.0 += 1;
        e.1 += r.1;
        for (i, v) in r.2.iter().enumerate() {
            e.2[i] += v;
        }
    }
    print!("{:<10} {:>3}", "class", "n");
    for n in names {
        print!(" {n:>11}");
    }
    println!();
    for (c, (n, px, v)) in &by {
        print!("{c:<10} {n:>3}");
        for x in v {
            print!(" {:>11.3}", x / *px as f64);
        }
        println!();
    }
}

fn entropy_configs() -> Vec<(&'static str, EncodeConfig)> {
    let d = EncodeConfig::default;
    let l16 = || EncodeConfig { lanes: 16, ..d() };
    vec![
        ("row_k8", d()),
        ("l16_k8", l16()),
        (
            "l16_k4m2",
            EncodeConfig {
                k_low: 4,
                msb_bits: 2,
                k_high: 4,
                ..l16()
            },
        ),
        (
            "l16_k3m1",
            EncodeConfig {
                k_low: 3,
                msb_bits: 1,
                k_high: 3,
                ..l16()
            },
        ),
        (
            "l16_k5m2",
            EncodeConfig {
                k_low: 5,
                msb_bits: 2,
                k_high: 5,
                ..l16()
            },
        ),
        (
            "l16_k4m2_c2",
            EncodeConfig {
                k_low: 4,
                msb_bits: 2,
                k_high: 4,
                max_ctx: 2,
                ..l16()
            },
        ),
        (
            "l16_k4m2_c8",
            EncodeConfig {
                k_low: 4,
                msb_bits: 2,
                k_high: 4,
                max_ctx: 8,
                ..l16()
            },
        ),
        (
            "l16_k4m2_L7",
            EncodeConfig {
                k_low: 4,
                msb_bits: 2,
                k_high: 4,
                max_code_len: 7,
                ..l16()
            },
        ),
        (
            "l16_k4m2_r2",
            EncodeConfig {
                k_low: 4,
                msb_bits: 2,
                k_high: 4,
                run_min: 2,
                ..l16()
            },
        ),
    ]
}

fn cmd_entropy(manifest: &str, filter: Option<&str>, out_path: &str) {
    let list = corpus::load_manifest(manifest, filter);
    let cfgs = entropy_configs();
    // per image: class, pixels, per config: [huff+extra, tans6.., tans12, max_used]
    let rows: Vec<(String, String, usize, Vec<Vec<f64>>)> = list
        .par_iter()
        .flat_map_iter(|(class, path)| {
            let img = match corpus::load_png(class, Path::new(path)) {
                Ok(i) => i,
                Err(e) => {
                    eprintln!("skip {e}");
                    return Vec::new();
                }
            };
            let mut imgs = corpus::variants(&img);
            imgs.insert(0, img);
            imgs.into_iter()
                .map(|im| {
                    let per: Vec<Vec<f64>> = cfgs
                        .iter()
                        .map(|(_, c)| {
                            let (_, r) = imij::encode_report(
                                &im.pixels,
                                im.info.row_bytes(),
                                &im.info,
                                &Metadata::default(),
                                c,
                            )
                            .unwrap();
                            let mut v = vec![(r.huff_bits + r.extra_bits) as f64];
                            v.extend(r.tans_bits.iter().map(|t| t + r.extra_bits as f64));
                            v.push(r.max_used_symbols as f64);
                            v
                        })
                        .collect();
                    eprintln!("{} {}", im.class, im.name);
                    (im.class.clone(), im.name.clone(), im.pixels_count(), per)
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let cols = [
        "huff", "tans6", "tans7", "tans8", "tans10", "tans12", "maxsym",
    ];
    let mut f = std::fs::File::create(out_path).unwrap();
    write!(f, "class\timage\tpixels").unwrap();
    for (n, _) in &cfgs {
        for c in cols {
            write!(f, "\t{n}.{c}").unwrap();
        }
    }
    writeln!(f).unwrap();
    for r in &rows {
        write!(f, "{}\t{}\t{}", r.0, r.1, r.2).unwrap();
        for v in &r.3 {
            for x in v {
                write!(f, "\t{x:.0}").unwrap();
            }
        }
        writeln!(f).unwrap();
    }
    let classes: std::collections::BTreeSet<String> = rows.iter().map(|r| r.0.clone()).collect();
    for (ci, (n, _)) in cfgs.iter().enumerate() {
        println!("== {n}  (payload bits/pixel; maxsym = most used symbols in any table)");
        print!("{:<10} {:>3}", "class", "n");
        for c in cols {
            print!(" {c:>8}");
        }
        println!();
        for cl in &classes {
            let sel: Vec<_> = rows.iter().filter(|r| &r.0 == cl).collect();
            let px: usize = sel.iter().map(|r| r.2).sum();
            print!("{cl:<10} {:>3}", sel.len());
            for k in 0..cols.len() {
                if k == cols.len() - 1 {
                    let m = sel.iter().map(|r| r.3[ci][k]).fold(0.0, f64::max);
                    print!(" {m:>8.0}");
                } else {
                    let t: f64 = sel.iter().map(|r| r.3[ci][k]).sum();
                    print!(" {:>8.3}", t / px as f64);
                }
            }
            println!();
        }
    }
}

fn cmd_labprof(path: &str, coder: &str, iters: usize) {
    use imij::lab::{self, Coder, LabCfg};
    let im = corpus::load_png("x", Path::new(path)).unwrap();
    let a = imij::analyze(
        &im.pixels,
        im.info.row_bytes(),
        &im.info,
        &EncodeConfig {
            lanes: 16,
            ..Default::default()
        },
    )
    .unwrap();
    let st = lab::lane_stripes(&a);
    let coder = match coder {
        "huff" => Coder::Huffman { max_len: 12 },
        c => Coder::Tans {
            log_m: c.trim_start_matches("tans").parse().unwrap(),
        },
    };
    let cfg = LabCfg {
        coder,
        k: 4,
        m: 2,
        run_min: 4,
        max_ctx: 4,
        cross: true,
        color_slots: im.info.channels.color_count(),
    };
    let enc = lab::encode(&st, a.slot_bits(), cfg);
    let mut outs: Vec<Vec<Vec<u16>>> = st
        .iter()
        .map(|s| s.u.iter().map(|v| vec![0u16; v.len()]).collect())
        .collect();
    // QOI reference on the same image.
    let qoi = zenbitmaps::encode_qoi(
        &im.pixels,
        im.info.width,
        im.info.height,
        zenbitmaps::PixelLayout::Rgb8,
        enough::Unstoppable,
    )
    .unwrap();
    let t = std::time::Instant::now();
    let mut best_q = f64::MAX;
    for _ in 0..iters {
        let t1 = std::time::Instant::now();
        let d = zenbitmaps::decode_qoi(&qoi, enough::Unstoppable).unwrap();
        std::hint::black_box(d.pixels()[0]);
        best_q = best_q.min(t1.elapsed().as_secs_f64());
    }
    let _ = t;
    let mut best = f64::MAX;
    for _ in 0..iters {
        let t1 = std::time::Instant::now();
        for (si, s) in st.iter().enumerate() {
            match (s.u.len(), enc.is_tans()) {
                (3, false) => lab::decode_fast::<3, false>(&enc, si, s, &mut outs[si]),
                (3, true) => lab::decode_fast::<3, true>(&enc, si, s, &mut outs[si]),
                _ => lab::decode_lane_seq(&enc, si, s, &mut outs[si]),
            }
        }
        best = best.min(t1.elapsed().as_secs_f64());
    }
    let px = im.pixels_count() as f64;
    println!(
        "entropy {:.1} Mpx/s ({:.1} Msym/s)  qoi {:.1} Mpx/s",
        px / best / 1e6,
        enc.tokens as f64 / best / 1e6,
        px / best_q / 1e6
    );
}

fn cmd_scaled(manifest: &str, filter: Option<&str>) {
    let list = corpus::load_manifest(manifest, filter);
    let logms = [6u32, 7, 8, 12];
    let variants: [(u32, u32); 4] = [(4, 2), (2, 1), (3, 1), (2, 0)];
    let rows: Vec<(String, usize, Vec<f64>, f64)> = list
        .par_iter()
        .flat_map_iter(|(class, path)| {
            let img = match corpus::load_png(class, Path::new(path)) {
                Ok(i) => i,
                Err(_) => return Vec::new(),
            };
            let mut imgs = corpus::variants(&img);
            imgs.insert(0, img);
            imgs.into_iter()
                .map(|im| {
                    let cfg = EncodeConfig {
                        lanes: 16,
                        k_low: 4,
                        msb_bits: 2,
                        k_high: 4,
                        ..Default::default()
                    };
                    let (_, rep) = imij::encode_report(
                        &im.pixels,
                        im.info.row_bytes(),
                        &im.info,
                        &Metadata::default(),
                        &cfg,
                    )
                    .unwrap();
                    let a = imij::analyze(&im.pixels, im.info.row_bytes(), &im.info, &cfg).unwrap();
                    let mut v = Vec::new();
                    for (k, m) in variants {
                        v.extend(cost::eval_scaled(&a, k, m, &logms, 3));
                    }
                    eprintln!("{} {}", im.class, im.name);
                    (
                        im.class.clone(),
                        im.pixels_count(),
                        v,
                        (rep.huff_bits + rep.extra_bits) as f64,
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let classes: std::collections::BTreeSet<String> = rows.iter().map(|r| r.0.clone()).collect();
    print!("{:<10} {:>8}", "class", "huff4ctx");
    for (k, m) in variants {
        for lm in logms {
            print!(" {:>9}", format!("k{k}m{m}R{lm}"));
        }
    }
    println!();
    for cl in &classes {
        let sel: Vec<_> = rows.iter().filter(|r| &r.0 == cl).collect();
        let px: usize = sel.iter().map(|r| r.1).sum::<usize>();
        print!(
            "{cl:<10} {:>8.3}",
            sel.iter().map(|r| r.3).sum::<f64>() / px as f64
        );
        for i in 0..variants.len() * logms.len() {
            print!(
                " {:>9.3}",
                sel.iter().map(|r| r.2[i]).sum::<f64>() / px as f64
            );
        }
        println!();
    }
}

fn cmd_ransprof(path: &str, iters: usize) {
    use imij::lab;
    use imij::lab_rans;
    let im = corpus::load_png("x", Path::new(path)).unwrap();
    let a = imij::analyze(
        &im.pixels,
        im.info.row_bytes(),
        &im.info,
        &EncodeConfig {
            lanes: 16,
            ..Default::default()
        },
    )
    .unwrap();
    let st: Vec<_> = lab::lane_stripes(&a)
        .iter()
        .map(lab_rans::interleave)
        .collect();
    let enc = lab_rans::encode(
        &st,
        a.slot_bits(),
        im.info.channels.color_count(),
        10,
        4,
        2,
        4,
    );
    let flats: Vec<_> = enc
        .models
        .iter()
        .map(|m| m.flat::<{ lab_rans::NCTX << 12 }>())
        .collect();
    let flats2: Vec<_> = enc.models.iter().map(|m| m.flat_v2()).collect();
    let mut outs: Vec<Vec<Vec<u16>>> = st
        .iter()
        .map(|s| s.u.iter().map(|v| vec![0u16; v.len()]).collect())
        .collect();
    for (si, s) in st.iter().enumerate() {
        lab_rans::decode_v3(&enc, si, s, &mut outs[si], &flats2);
        assert!(outs[si] == s.u, "v3 mismatch stripe {si}");
    }
    let px = im.pixels_count() as f64;
    let mode = std::env::var("IMIJ_MODE").unwrap_or_default();
    if mode == "rice" {
        let renc = imij::lab_rice::encode(&st, a.slot_bits(), im.info.channels.color_count());
        let mut o: Vec<Vec<Vec<u16>>> = st
            .iter()
            .map(|s| s.u.iter().map(|v| vec![0u16; v.len()]).collect())
            .collect();
        for (si, s) in st.iter().enumerate() {
            imij::lab_rice::decode_scalar(&renc, si, s, &mut o[si]);
            assert!(o[si] == s.u, "rice scalar mismatch stripe {si}");
            imij::lab_rice::decode_simd(&renc, si, s, &mut o[si]);
            assert!(o[si] == s.u, "rice simd mismatch stripe {si}");
        }
        let mut best = f64::MAX;
        for _ in 0..iters {
            let t1 = std::time::Instant::now();
            for (si, s) in st.iter().enumerate() {
                imij::lab_rice::decode_simd(&renc, si, s, &mut o[si]);
            }
            best = best.min(t1.elapsed().as_secs_f64());
        }
        println!(
            "rice simd {:.1} Mpx/s, {:.4} bpp (rans10 {:.4} bpp)",
            px / best / 1e6,
            renc.payload_bytes as f64 * 8.0 / px,
            enc.payload_bytes as f64 * 8.0 / px
        );
        return;
    }
    if mode == "qoi" {
        let qoi = zenbitmaps::encode_qoi(
            &im.pixels,
            im.info.width,
            im.info.height,
            zenbitmaps::PixelLayout::Rgb8,
            enough::Unstoppable,
        )
        .unwrap();
        let mut best = f64::MAX;
        for _ in 0..iters {
            let t1 = std::time::Instant::now();
            let d = zenbitmaps::decode_qoi(&qoi, enough::Unstoppable).unwrap();
            std::hint::black_box(d.pixels()[0]);
            best = best.min(t1.elapsed().as_secs_f64());
        }
        println!("qoi {:.1} Mpx/s", px / best / 1e6);
        return;
    }
    for (name, which) in [("fused", 0), ("v3", 1)] {
        if !mode.is_empty() && mode != name {
            continue;
        }
        let mut best = f64::MAX;
        for _ in 0..iters {
            let t1 = std::time::Instant::now();
            for (si, s) in st.iter().enumerate() {
                if which == 0 {
                    lab_rans::decode_fused(&enc, si, s, &mut outs[si], &flats);
                } else {
                    lab_rans::decode_v3(&enc, si, s, &mut outs[si], &flats2);
                }
            }
            best = best.min(t1.elapsed().as_secs_f64());
        }
        println!("rans {name} {:.1} Mpx/s", px / best / 1e6);
    }
}

fn cmd_rice(manifest: &str, filter: Option<&str>) {
    let list = corpus::load_manifest(manifest, filter);
    let rows: Vec<(String, usize, Vec<f64>)> = list
        .par_iter()
        .flat_map_iter(|(class, path)| {
            let img = match corpus::load_png(class, Path::new(path)) {
                Ok(i) => i,
                Err(_) => return Vec::new(),
            };
            let mut imgs = corpus::variants(&img);
            imgs.insert(0, img);
            imgs.into_iter()
                .map(|im| {
                    let cfg = EncodeConfig {
                        lanes: 16,
                        k_low: 4,
                        msb_bits: 2,
                        k_high: 4,
                        ..Default::default()
                    };
                    let (_, rep) = imij::encode_report(
                        &im.pixels,
                        im.info.row_bytes(),
                        &im.info,
                        &Metadata::default(),
                        &cfg,
                    )
                    .unwrap();
                    let a = imij::analyze(&im.pixels, im.info.row_bytes(), &im.info, &cfg).unwrap();
                    let v = vec![
                        (rep.huff_bits + rep.extra_bits) as f64,
                        rep.tans_bits[3] + rep.extra_bits as f64,
                        cost::eval_rice_lanes(&a, false, false) as f64,
                        cost::eval_rice_lanes(&a, true, false) as f64,
                        cost::eval_rice_lanes(&a, true, true) as f64,
                    ];
                    eprintln!("{} {}", im.class, im.name);
                    (im.class.clone(), im.pixels_count(), v)
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let names = [
        "huff4ctx",
        "rans10",
        "rice",
        "rice_fine",
        "rice_fine_runany",
    ];
    let classes: std::collections::BTreeSet<String> = rows.iter().map(|r| r.0.clone()).collect();
    print!("{:<10}", "class");
    for n in names {
        print!(" {n:>16}");
    }
    println!();
    for cl in &classes {
        let sel: Vec<_> = rows.iter().filter(|r| &r.0 == cl).collect();
        let px: usize = sel.iter().map(|r| r.1).sum();
        print!("{cl:<10}");
        for i in 0..names.len() {
            print!(
                " {:>16.3}",
                sel.iter().map(|r| r.2[i]).sum::<f64>() / px as f64
            );
        }
        println!();
    }
}

fn time_best<F: FnMut()>(iters: usize, mut f: F) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..iters {
        let t = std::time::Instant::now();
        f();
        best = best.min(t.elapsed().as_secs_f64());
    }
    best
}

fn cmd_v1(manifest: &str, filter: Option<&str>, iters: usize) {
    let list = corpus::load_manifest(manifest, filter);
    let mut by: BTreeMap<String, (usize, usize, f64, f64, usize, usize, f64, f64)> =
        BTreeMap::new();
    for (class, path) in &list {
        let img = match corpus::load_png(class, Path::new(path)) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("skip {e}");
                continue;
            }
        };
        let mut imgs = corpus::variants(&img);
        imgs.insert(0, img);
        for im in imgs {
            let mut vcfg = imij::ImiEncoderConfig::default();
            if let Ok(k) = std::env::var("IMIJ_K") {
                vcfg = vcfg.tune_lossless(|t| { t.k_low = k.parse().unwrap(); });
                vcfg = vcfg.tune_lossless(|t| { t.k_high = t.k_low; });
            }
            if std::env::var_os("IMIJ_NOCROSS").is_some() {
                vcfg = vcfg.tune_lossless(|t| { t.ctx_cross = false; });
            }
            let enc = vcfg.encode_samples(imij::ImageRef::new(&im.pixels, im.info.row_bytes(), &im.info), &Metadata::default())
            .unwrap();
            let (_, dec) = imij::decode(&enc).unwrap();
            assert!(dec == im.pixels, "roundtrip mismatch {}", im.name);
            let mut buf = vec![0u8; im.pixels.len()];
            let t_z = time_best(iters, || {
                imij::decode_into(&enc, &mut buf, im.info.row_bytes()).unwrap();
            });
            let (qsz, t_q) = match qoi_size(&im) {
                Some(_)
                    if im.info.channels == Channels::Rgb || im.info.channels == Channels::Rgba =>
                {
                    let layout = if im.info.channels == Channels::Rgb {
                        zenbitmaps::PixelLayout::Rgb8
                    } else {
                        zenbitmaps::PixelLayout::Rgba8
                    };
                    let q = zenbitmaps::encode_qoi(
                        &im.pixels,
                        im.info.width,
                        im.info.height,
                        layout,
                        enough::Unstoppable,
                    )
                    .unwrap();
                    let t = time_best(iters, || {
                        let d = zenbitmaps::decode_qoi(&q, enough::Unstoppable).unwrap();
                        std::hint::black_box(d.pixels()[0]);
                    });
                    (q.len(), t)
                }
                _ => (0, 0.0),
            };
            let px = im.pixels_count();
            println!(
                "{:<9} {:<40} {:>6.3} bpp (qoi {:>6.3})  imij {:>6.1} MB/s  qoi {:>6.1} MB/s",
                im.class,
                &im.name[..im.name.len().min(40)],
                enc.len() as f64 * 8.0 / px as f64,
                qsz as f64 * 8.0 / px as f64,
                im.pixels.len() as f64 / t_z / 1e6,
                if t_q > 0.0 {
                    im.pixels.len() as f64 / t_q / 1e6
                } else {
                    0.0
                }
            );
            let e = by
                .entry(im.class.clone())
                .or_insert((0, 0, 0.0, 0.0, 0, 0, 0.0, 0.0));
            e.0 += px;
            e.1 += enc.len();
            e.2 += t_z;
            e.3 += t_q;
            e.4 += qsz;
            e.5 += im.pixels.len();
            if t_q > 0.0 {
                e.6 += t_z;
                e.7 += im.pixels.len() as f64;
            }
        }
    }
    println!(
        "\n{:<9} {:>8} {:>8} {:>10} {:>10}",
        "class", "imij bpp", "qoi bpp", "imij MB/s", "qoi MB/s"
    );
    for (c, e) in &by {
        println!(
            "{c:<9} {:>8.3} {:>8} {:>10.1} {:>10}",
            e.1 as f64 * 8.0 / e.0 as f64,
            if e.4 > 0 {
                format!("{:.3}", e.4 as f64 * 8.0 / e.0 as f64)
            } else {
                "-".into()
            },
            e.5 as f64 / e.2 / 1e6,
            if e.3 > 0.0 {
                format!("{:.1}", e.7 / e.3 / 1e6)
            } else {
                "-".into()
            }
        );
    }
}

/// Size sweep: center crops at several sizes, imij vs QOI bytes.
fn cmd_tiny(manifest: &str, filter: Option<&str>) {
    let list = corpus::load_manifest(manifest, filter);
    let sizes = [16usize, 32, 64, 128, 256, 512];
    let mut tot: BTreeMap<(String, usize), (usize, usize, usize, usize)> = BTreeMap::new();
    for (class, path) in &list {
        let Ok(img) = corpus::load_png(class, Path::new(path)) else {
            continue;
        };
        if img.info.sample != SampleType::U8 || img.info.channels.color_count() != 3 {
            continue;
        }
        let (w, h) = (img.info.width as usize, img.info.height as usize);
        let bpp = img.info.bytes_per_pixel();
        for &s in &sizes {
            if s > w || s > h {
                continue;
            }
            let (x0, y0) = ((w - s) / 2, (h - s) / 2);
            let mut px = Vec::with_capacity(s * s * bpp);
            for y in y0..y0 + s {
                px.extend_from_slice(&img.pixels[(y * w + x0) * bpp..(y * w + x0 + s) * bpp]);
            }
            let mut info = img.info.clone();
            info.width = s as u32;
            info.height = s as u32;
            let z = imij::ImiEncoderConfig::default().encode_samples(imij::ImageRef::new(&px, s * bpp, &info), &Metadata::default())
            .unwrap();
            let layout = if bpp == 3 {
                zenbitmaps::PixelLayout::Rgb8
            } else {
                zenbitmaps::PixelLayout::Rgba8
            };
            let q = zenbitmaps::encode_qoi(&px, s as u32, s as u32, layout, enough::Unstoppable)
                .unwrap();
            let e = tot.entry((class.clone(), s)).or_insert((0, 0, 0, 0));
            e.0 += 1;
            e.1 += z.len();
            e.2 += q.len();
            e.3 += s * s;
        }
    }
    println!(
        "{:<9} {:>5} {:>4} {:>10} {:>10} {:>8}",
        "class", "size", "n", "imij B/img", "qoi B/img", "imij/qoi"
    );
    for ((c, s), (n, z, q, _)) in &tot {
        println!(
            "{c:<9} {s:>5} {n:>4} {:>10.0} {:>10.0} {:>8.3}",
            *z as f64 / *n as f64,
            *q as f64 / *n as f64,
            *z as f64 / *q as f64
        );
    }
}

/// Encodes the image (and its variants) to `<dir>/<name>.imi`.
fn cmd_dump(path: &str, class: &str, dir: &str) {
    let img = corpus::load_png(class, Path::new(path)).unwrap();
    let mut imgs = corpus::variants(&img);
    imgs.insert(0, img);
    std::fs::create_dir_all(dir).unwrap();
    let threads: usize = std::env::var("IMIJ_ENC_THREADS")
        .ok()
        .map(|v| v.parse().unwrap())
        .unwrap_or(1);
    for im in imgs {
        let enc = imij::ImiEncoderConfig::default().with_threads(threads).encode_samples(imij::ImageRef::new(&im.pixels, im.info.row_bytes(), &im.info), &Metadata::default())
        .unwrap();
        let out = format!("{dir}/{}.imi", im.name);
        std::fs::write(&out, &enc).unwrap();
        println!("{out} {} bytes", enc.len());
    }
}

/// Decode-only loop for profiling.
fn cmd_decprof(path: &str, iters: usize, threads: usize) {
    let data = std::fs::read(path).unwrap();
    let (info, _) = imij::decode(&data).unwrap();
    let mut buf = vec![0u8; info.row_bytes() * info.height as usize];
    let t = time_best(iters, || {
        imij::decode_into_threads(&data, &mut buf, info.row_bytes(), threads).unwrap();
    });
    println!(
        "{path}: {:.1} MB/s ({} threads, tier {})",
        buf.len() as f64 / t / 1e6,
        threads,
        imij::simd_tier()
    );
}

fn cmd_encspeed(manifest: &str, filter: Option<&str>) {
    let list = corpus::load_manifest(manifest, filter);
    for (class, path) in list.iter().take(6) {
        let Ok(im) = corpus::load_png(class, Path::new(path)) else {
            continue;
        };
        let threads: usize = std::env::var("IMIJ_ENC_THREADS")
            .ok()
            .map(|v| v.parse().unwrap())
            .unwrap_or(1);
        let effort: f32 = std::env::var("IMIJ_EFFORT")
            .ok()
            .map(|v| v.parse().unwrap())
            .unwrap_or(10.0);
        let cfg = imij::ImiEncoderConfig::default().with_threads(threads).with_effort(effort);
        let size = cfg.encode_samples(imij::ImageRef::new(&im.pixels, im.info.row_bytes(), &im.info), &Metadata::default())
        .unwrap()
        .len();
        print!("{:.3} bpp  ", size as f64 * 8.0 / im.pixels_count() as f64);
        let t = time_best(2, || {
            std::hint::black_box(
                cfg.encode_samples(imij::ImageRef::new(&im.pixels, im.info.row_bytes(), &im.info), &Metadata::default())
                .unwrap(),
            );
        });
        let layout = if im.info.channels == Channels::Rgb {
            zenbitmaps::PixelLayout::Rgb8
        } else {
            zenbitmaps::PixelLayout::Rgba8
        };
        let tq = time_best(2, || {
            std::hint::black_box(
                zenbitmaps::encode_qoi(
                    &im.pixels,
                    im.info.width,
                    im.info.height,
                    layout,
                    enough::Unstoppable,
                )
                .ok(),
            );
        });
        println!(
            "{class:<8} {:>6.1} MB/s encode (qoi {:>7.1} MB/s)",
            im.pixels.len() as f64 / t / 1e6,
            im.pixels.len() as f64 / tq / 1e6
        );
    }
}

/// Integer 2x bilinear upsample (center-aligned, 3/4-1/4 taps) of an RGB8
/// base to `w x h`, returned as per-channel u16 planes.
fn upsample2x(base: &[u8], bw: usize, bh: usize, w: usize, h: usize) -> Vec<Vec<u16>> {
    let at = |x: isize, y: isize, c: usize| -> u32 {
        let x = x.clamp(0, bw as isize - 1) as usize;
        let y = y.clamp(0, bh as isize - 1) as usize;
        base[(y * bw + x) * 3 + c] as u32
    };
    let mut out = vec![vec![0u16; w * h]; 3];
    for y in 0..h {
        let by = (y / 2) as isize;
        let oy = if y % 2 == 0 { by - 1 } else { by + 1 };
        for x in 0..w {
            let bx = (x / 2) as isize;
            let ox = if x % 2 == 0 { bx - 1 } else { bx + 1 };
            for c in 0..3 {
                let v = 9 * at(bx, by, c) + 3 * at(ox, by, c) + 3 * at(bx, oy, c) + at(ox, oy, c);
                out[c][y * w + x] = ((v + 8) >> 4) as u16;
            }
        }
    }
    out
}

fn cmd_pyramid(manifest: &str, filter: Option<&str>) {
    let list = corpus::load_manifest(manifest, filter);
    let rows: Vec<(String, usize, [usize; 4])> = list
        .par_iter()
        .filter_map(|(class, path)| {
            let im = corpus::load_png(class, Path::new(path)).ok()?;
            if im.info.sample != SampleType::U8 || im.info.channels != Channels::Rgb {
                return None;
            }
            let (w, h) = (im.info.width as usize, im.info.height as usize);
            let (bw, bh) = (w.div_ceil(2), h.div_ceil(2));
            let rc = zenresize::ResizeConfig::builder(w as u32, h as u32, bw as u32, bh as u32)
                .filter(zenresize::Filter::Mitchell)
                .format(zenresize::PixelDescriptor::RGB8_SRGB)
                .build();
            let base = zenresize::Resizer::new(&rc).resize(&im.pixels);
            let mut binfo = im.info.clone();
            binfo.width = bw as u32;
            binfo.height = bh as u32;
            let cfg = imij::ImiEncoderConfig::default();
            let plain = cfg.encode_samples(imij::ImageRef::new(&im.pixels, im.info.row_bytes(), &im.info), &Metadata::default())
            .unwrap()
            .len();
            let base_sz =
                cfg.encode_samples(imij::ImageRef::new(&base, binfo.row_bytes(), &binfo), &Metadata::default())
                    .unwrap()
                    .len();
            let guide = upsample2x(&base, bw, bh, w, h);
            let gcfg = imij::ImiEncoderConfig::default().tune_lossless(|t| { t.guide = Some(std::sync::Arc::new(guide)); });
            let guided = gcfg.encode_samples(imij::ImageRef::new(&im.pixels, im.info.row_bytes(), &im.info), &Metadata::default())
            .unwrap()
            .len();
            eprintln!(
                "{} {} plain {} base {} guided {}",
                class, im.name, plain, base_sz, guided
            );
            Some((
                class.clone(),
                w * h,
                [plain, base_sz, guided, base_sz + guided],
            ))
        })
        .collect();
    let classes: std::collections::BTreeSet<String> = rows.iter().map(|r| r.0.clone()).collect();
    println!(
        "{:<9} {:>3} {:>8} {:>8} {:>8} {:>9} {:>7}",
        "class", "n", "plain", "base", "guided", "base+gd", "ratio"
    );
    for c in &classes {
        let sel: Vec<_> = rows.iter().filter(|r| &r.0 == c).collect();
        let px: usize = sel.iter().map(|r| r.1).sum();
        let t = |i: usize| sel.iter().map(|r| r.2[i]).sum::<usize>() as f64 * 8.0 / px as f64;
        println!(
            "{c:<9} {:>3} {:>8.3} {:>8.3} {:>8.3} {:>9.3} {:>7.3}",
            sel.len(),
            t(0),
            t(1),
            t(2),
            t(3),
            t(3) / t(0)
        );
    }
}

/// One-level 2D S-transform (reversible Haar lifting) per channel.
/// Returns (LL as RGB8, [HL, LH, HH] as RGB u16 with +offset).
fn squeeze(px: &[u8], w: usize, h: usize) -> (Vec<u8>, [Vec<u8>; 3], usize, usize) {
    let (bw, bh) = (w.div_ceil(2), h.div_ceil(2));
    let get = |x: usize, y: usize, c: usize| -> i32 {
        px[(y.min(h - 1) * w + x.min(w - 1)) * 3 + c] as i32
    };
    let mut ll = vec![0u8; bw * bh * 3];
    let mut bands: [Vec<u8>; 3] = [
        vec![0u8; bw * bh * 6],
        vec![0u8; bw * bh * 6],
        vec![0u8; bw * bh * 6],
    ];
    for by in 0..bh {
        for bx in 0..bw {
            for c in 0..3 {
                let (a, b) = (get(2 * bx, 2 * by, c), get(2 * bx + 1, 2 * by, c));
                let (e, f) = (get(2 * bx, 2 * by + 1, c), get(2 * bx + 1, 2 * by + 1, c));
                // Horizontal.
                let (l0, d0) = ((a + b) >> 1, a - b);
                let (l1, d1) = ((e + f) >> 1, e - f);
                // Vertical.
                let (lll, lh) = ((l0 + l1) >> 1, l0 - l1);
                let (hl, hh) = ((d0 + d1) >> 1, d0 - d1);
                let i = (by * bw + bx) * 3 + c;
                ll[i] = lll as u8;
                for (k, v) in [(0, hl + 512), (1, lh + 512), (2, hh + 1024)] {
                    bands[k][2 * i..2 * i + 2].copy_from_slice(&(v as u16).to_ne_bytes());
                }
            }
        }
    }
    (ll, bands, bw, bh)
}

fn cmd_squeeze(manifest: &str, filter: Option<&str>) {
    let list = corpus::load_manifest(manifest, filter);
    let rows: Vec<(String, usize, [usize; 3])> = list
        .par_iter()
        .filter_map(|(class, path)| {
            let im = corpus::load_png(class, Path::new(path)).ok()?;
            if im.info.sample != SampleType::U8 || im.info.channels != Channels::Rgb {
                return None;
            }
            let (w, h) = (im.info.width as usize, im.info.height as usize);
            let cfg = imij::ImiEncoderConfig::default();
            let plain = cfg.encode_samples(imij::ImageRef::new(&im.pixels, im.info.row_bytes(), &im.info), &Metadata::default())
            .unwrap()
            .len();
            let (ll, bands, bw, bh) = squeeze(&im.pixels, w, h);
            let mut binfo = im.info.clone();
            binfo.width = bw as u32;
            binfo.height = bh as u32;
            let ll_sz = cfg.encode_samples(imij::ImageRef::new(&ll, binfo.row_bytes(), &binfo), &Metadata::default())
                .unwrap()
                .len();
            let mut dinfo = binfo.clone();
            dinfo.sample = SampleType::U16;
            let det: usize = bands
                .iter()
                .map(|b| {
                    cfg.encode_samples(imij::ImageRef::new(b, dinfo.row_bytes(), &dinfo), &Metadata::default())
                        .unwrap()
                        .len()
                })
                .sum();
            Some((class.clone(), w * h, [plain, ll_sz, det]))
        })
        .collect();
    let classes: std::collections::BTreeSet<String> = rows.iter().map(|r| r.0.clone()).collect();
    println!(
        "{:<9} {:>3} {:>8} {:>8} {:>8} {:>8} {:>7}",
        "class", "n", "plain", "LL", "details", "total", "ratio"
    );
    for c in &classes {
        let sel: Vec<_> = rows.iter().filter(|r| &r.0 == c).collect();
        let px: usize = sel.iter().map(|r| r.1).sum();
        let t = |i: usize| sel.iter().map(|r| r.2[i]).sum::<usize>() as f64 * 8.0 / px as f64;
        println!(
            "{c:<9} {:>3} {:>8.3} {:>8.3} {:>8.3} {:>8.3} {:>7.3}",
            sel.len(),
            t(0),
            t(1),
            t(2),
            t(1) + t(2),
            (t(1) + t(2)) / t(0)
        );
    }
}

fn mitchell_half(px: &[u8], w: u32, h: u32) -> (Vec<u8>, u32, u32) {
    let (bw, bh) = (w.div_ceil(2), h.div_ceil(2));
    let rc = zenresize::ResizeConfig::builder(w, h, bw, bh)
        .filter(zenresize::Filter::Mitchell)
        .format(zenresize::PixelDescriptor::RGB8_SRGB)
        .build();
    (zenresize::Resizer::new(&rc).resize(px), bw, bh)
}

/// Writes <dir>/<stem>.{full.imi,half.imi,qoi} for an RGB8 PNG.
fn cmd_serveprep(path: &str, dir: &str) {
    let im = corpus::load_png("x", Path::new(path)).unwrap();
    assert_eq!(im.info.channels, Channels::Rgb);
    std::fs::create_dir_all(dir).unwrap();
    let cfg = imij::ImiEncoderConfig::default().with_threads(8);
    let full = cfg.encode_samples(imij::ImageRef::new(&im.pixels, im.info.row_bytes(), &im.info), &Metadata::default())
    .unwrap();
    let (half, bw, bh) = mitchell_half(&im.pixels, im.info.width, im.info.height);
    let mut hi = im.info.clone();
    hi.width = bw;
    hi.height = bh;
    let halfz = cfg.encode_samples(imij::ImageRef::new(&half, hi.row_bytes(), &hi), &Metadata::default()).unwrap();
    let q = zenbitmaps::encode_qoi(
        &im.pixels,
        im.info.width,
        im.info.height,
        zenbitmaps::PixelLayout::Rgb8,
        enough::Unstoppable,
    )
    .unwrap();
    let prev = imij::Preview {
        pixels: &half,
        stride: hi.row_bytes(),
        info: &hi,
    };
    let withprev = cfg.encode_samples_with_previews(imij::ImageRef::new(&im.pixels, im.info.row_bytes(), &im.info), &Metadata::default(), &[prev])
    .unwrap();
    let stem = format!("{dir}/{}", im.name);
    std::fs::write(format!("{stem}.withprev.imi"), &withprev).unwrap();
    std::fs::write(format!("{stem}.full.imi"), &full).unwrap();
    std::fs::write(format!("{stem}.half.imi"), &halfz).unwrap();
    std::fs::write(format!("{stem}.qoi"), &q).unwrap();
    println!(
        "{stem}: {}x{} full {} B, half {} B (+{:.1}%), qoi {} B",
        im.info.width,
        im.info.height,
        full.len(),
        halfz.len(),
        halfz.len() as f64 * 100.0 / full.len() as f64,
        q.len()
    );
}

/// One serving pipeline producing the 50% image. Returns a checksum.
fn serve_once(mode: &str, stem: &str, full: &[u8], half: &[u8], qoi: &[u8]) -> u64 {
    let _ = stem;
    match mode {
        "full" => {
            let (_, px) = imij::decode(full).unwrap();
            px[px.len() / 2] as u64
        }
        "full_resize" => {
            let (info, px) = imij::decode(full).unwrap();
            let (o, _, _) = mitchell_half(&px, info.width, info.height);
            o[o.len() / 2] as u64
        }
        "qoi_full_resize" => {
            let d = zenbitmaps::decode_qoi(qoi, enough::Unstoppable).unwrap();
            let (o, _, _) = mitchell_half(d.pixels(), d.width, d.height);
            o[o.len() / 2] as u64
        }
        "embedded_preview" => {
            let p = imij::previews(half).unwrap();
            let (_, px) = imij::decode(p[0]).unwrap();
            px[px.len() / 2] as u64
        }
        "half_file" => {
            let (_, px) = imij::decode(half).unwrap();
            px[px.len() / 2] as u64
        }
        "stream_resize" => {
            let mut d = imij::StreamDecoder::new(full).unwrap();
            let info = d.info().clone();
            let (bw, bh) = (info.width.div_ceil(2), info.height.div_ceil(2));
            let rc = zenresize::ResizeConfig::builder(info.width, info.height, bw, bh)
                .filter(zenresize::Filter::Mitchell)
                .format(zenresize::PixelDescriptor::RGB8_SRGB)
                .build();
            let mut rs = zenresize::StreamingResize::new(&rc);
            let ol = rs.output_row_len();
            let mut out = vec![0u8; ol * bh as usize];
            let mut row = vec![0u8; info.row_bytes()];
            let mut oy = 0usize;
            while d.next_row(&mut row).unwrap() {
                rs.push_row(&row).unwrap();
                while oy < bh as usize && rs.next_output_row_into(&mut out[oy * ol..(oy + 1) * ol])
                {
                    oy += 1;
                }
            }
            rs.finish();
            while oy < bh as usize && rs.next_output_row_into(&mut out[oy * ol..(oy + 1) * ol]) {
                oy += 1;
            }
            assert_eq!(oy, bh as usize);
            out[out.len() / 2] as u64
        }
        _ => panic!("mode"),
    }
}

fn cmd_serve(mode: &str, stem: &str, iters: usize) {
    let rd = |ext: &str, need: bool| {
        if need {
            std::fs::read(format!("{stem}.{ext}")).unwrap()
        } else {
            Vec::new()
        }
    };
    let full = rd(
        "full.imi",
        matches!(mode, "full" | "full_resize" | "stream_resize"),
    );
    let half = if mode == "embedded_preview" {
        // Read only the prefix a server would fetch: walk chunk headers with
        // seeks until DATA, then read [0, DATA).
        use std::io::{Read, Seek, SeekFrom};
        let mut f = std::fs::File::open(format!("{stem}.withprev.imi")).unwrap();
        let mut pos = 19u64;
        loop {
            f.seek(SeekFrom::Start(pos)).unwrap();
            let mut hdr = [0u8; 8];
            f.read_exact(&mut hdr).unwrap();
            if &hdr[..4] == b"DATA" {
                break;
            }
            pos += 8 + u32::from_le_bytes(hdr[4..].try_into().unwrap()) as u64;
        }
        let mut buf = vec![0u8; pos as usize];
        f.seek(SeekFrom::Start(0)).unwrap();
        f.read_exact(&mut buf).unwrap();
        buf
    } else {
        rd("half.imi", mode == "half_file")
    };
    let qoi = rd("qoi", mode == "qoi_full_resize");
    let mut ck = 0;
    let t = time_best(iters, || {
        ck ^= serve_once(mode, stem, &full, &half, &qoi);
    });
    println!("{mode:<16} {:>8.2} ms  (ck {ck})", t * 1e3);
}

fn cmd_util(manifest: &str, filter: Option<&str>) {
    for (class, path) in corpus::load_manifest(manifest, filter).iter().take(8) {
        let Ok(im) = corpus::load_png(class, Path::new(path)) else {
            continue;
        };
        let u = imij::lane_utilization(&im.pixels, im.info.row_bytes(), &im.info).unwrap();
        let parts: Vec<String> = u
            .iter()
            .map(|(steps, busy, syms)| {
                format!(
                    "busy {:>5.1}% lanes/busy {:>4.1}",
                    *busy as f64 * 100.0 / *steps as f64,
                    *syms as f64 / (*busy).max(1) as f64
                )
            })
            .collect();
        println!(
            "{class:<8} {:<30} {}",
            &im.name[..im.name.len().min(30)],
            parts.join(" | ")
        );
    }
}

fn load_rgb8(manifest: &str, filter: Option<&str>) -> Vec<Img> {
    corpus::load_manifest(manifest, filter)
        .iter()
        .filter_map(|(class, path)| corpus::load_png(class, Path::new(path)).ok())
        .filter(|i| i.info.channels == Channels::Rgb && i.info.sample == SampleType::U8)
        .collect()
}

/// E1/E4: lifting vs float DCT vs zenjpeg 4:4:4, rate-quality curves.
fn cmd_lossy(manifest: &str, filter: Option<&str>, which: &str) {
    use lossy::{LossyCfg, Quant, Xform};
    let imgs = load_rgb8(manifest, filter);
    eprintln!("{} images", imgs.len());
    let jq: Vec<u32> = vec![70, 75, 80, 85, 88, 90, 92, 94, 95, 96, 97, 98, 99, 100];
    let mk = |xform: Xform, nb: bool, q: Quant| LossyCfg {
        xform,
        quant: q,
        round: 0.4,
        nb_ctx: nb,
        color: lossy::Color::YCoCgR,
    };
    let mut variants: Vec<(String, Vec<(String, LossyCfg)>)> = Vec::new();
    let jt = |x: Xform, nb: bool, cm: f64| -> Vec<(String, LossyCfg)> {
        jq.iter()
            .map(|&q| (format!("q{q}"), mk(x, nb, Quant::jpeg(q, cm))))
            .collect()
    };
    match which {
        "e1" => {
            variants.push(("float_jt".into(), jt(Xform::Float, true, 2.0)));
            for f in [0, 1, 2] {
                variants.push((format!("lift{f}_jt"), jt(Xform::Lift { f }, true, 2.0)));
            }
            variants.push(("lift1_jt_nonb".into(), jt(Xform::Lift { f: 1 }, false, 2.0)));
        }
        "e1b" => {
            variants.push(("float_jt".into(), jt(Xform::Float, true, 2.0)));
            variants.push(("liftnr_jt".into(), jt(Xform::LiftNoRound, true, 2.0)));
            for f in [3, 4] {
                variants.push((format!("lift{f}_jt"), jt(Xform::Lift { f }, true, 2.0)));
            }
            for (name, r) in [("float_jt_r40", 0.40), ("float_jt_r30", 0.30)] {
                variants.push((
                    name.into(),
                    jq.iter()
                        .map(|&q| {
                            let mut c = mk(Xform::Float, true, Quant::jpeg(q, 2.0));
                            c = c.tune_lossy(|t| { t.round = r; });
                            (format!("q{q}"), c)
                        })
                        .collect(),
                ));
            }
            for cm in [1.5, 3.0] {
                variants.push((format!("float_jt_cm{cm}"), jt(Xform::Float, true, cm)));
            }
            let fq = [0.5, 0.7, 1.0, 1.4, 2.0, 2.8, 4.0, 5.6, 8.0, 11.0, 16.0];
            for slope in [0.0, 1.0, 3.0] {
                variants.push((
                    format!("float_flat_s{slope}"),
                    fq.iter()
                        .map(|&q| {
                            (
                                format!("Q{q}"),
                                mk(Xform::Float, true, Quant::flat(q, 2.0, slope)),
                            )
                        })
                        .collect(),
                ));
            }
        }
        "rgb" => {
            // Color transform vs none: YCoCg, XYB, RGB planes (luma steps on
            // all channels; or R/B steps x1.5), lab coder, same sweep.
            let qs = [20u32, 30, 40, 50, 60, 70, 80, 85, 90, 93, 95, 97, 99];
            let mut v = |name: &str, color: lossy::Color, quant: &dyn Fn(u32) -> Quant| {
                variants.push((
                    name.into(),
                    qs.iter()
                        .map(|&q| {
                            let mut c = mk(Xform::Float, true, quant(q));
                            c = c.tune_lossy(|t| { t.color = color; });
                            (format!("q{q}"), c)
                        })
                        .collect(),
                ));
            };
            v("ycocg", lossy::Color::YCoCgR, &|q| Quant::jpeg(q, 2.0));
            v("xyb8", lossy::Color::Xyb { x_gain: 8.0 }, &|q| {
                Quant::jpeg(q, 2.0)
            });
            v("rgb_l", lossy::Color::Rgb, &|q| {
                Quant::luma3(q, [1.0, 1.0, 1.0])
            });
            v("rgb_l15", lossy::Color::Rgb, &|q| {
                Quant::luma3(q, [1.5, 1.0, 1.5])
            });
        }
        "e2" => {
            variants.push(("ycocg_jt".into(), jt(Xform::Float, true, 2.0)));
            variants.push(("lift4_jt".into(), jt(Xform::Lift { f: 4 }, true, 2.0)));
            for (xg, mx, mb) in [
                (16.0, 1.0, 1.0),
                (16.0, 2.0, 1.0),
                (16.0, 1.0, 2.0),
                (32.0, 1.0, 1.0),
                (8.0, 1.0, 1.0),
                (16.0, 2.0, 2.0),
            ] {
                variants.push((
                    format!("xyb_g{xg}_x{mx}_b{mb}"),
                    jq.iter()
                        .map(|&q| {
                            let mut c = mk(Xform::Float, true, Quant::jpeg3(q, [1.0, mx, mb]));
                            c = c.tune_lossy(|t| { t.color = lossy::Color::Xyb { x_gain: xg }; });
                            (format!("q{q}"), c)
                        })
                        .collect(),
                ));
            }
            variants.push((
                "lift0_lossless".into(),
                vec![(
                    "ll".into(),
                    mk(Xform::Lift { f: 0 }, true, Quant::flat(1e-9, 1.0, 0.0)),
                )],
            ));
        }
        "e2b" => {
            variants.push(("ycocg_jt".into(), jt(Xform::Float, true, 2.0)));
            for (xg, mx, mb) in [
                (8.0, 1.0, 1.0),
                (4.0, 1.0, 1.0),
                (2.0, 1.0, 1.0),
                (4.0, 1.0, 0.7),
                (4.0, 1.0, 1.4),
            ] {
                variants.push((
                    format!("xyb_g{xg}_b{mb}"),
                    jq.iter()
                        .map(|&q| {
                            let mut c = mk(Xform::Float, true, Quant::jpeg3(q, [1.0, mx, mb]));
                            c = c.tune_lossy(|t| { t.color = lossy::Color::Xyb { x_gain: xg }; });
                            (format!("q{q}"), c)
                        })
                        .collect(),
                ));
            }
        }
        "ctx" => {
            let ccs = [
                (
                    "none",
                    lossy::CtxCfg {
                        nb: 0,
                        dc_act: false,
                        luma_eob: false,
                        eob_mode: 0,
                    },
                ),
                (
                    "above",
                    lossy::CtxCfg {
                        nb: 1,
                        dc_act: false,
                        luma_eob: false,
                        eob_mode: 0,
                    },
                ),
                (
                    "left+above",
                    lossy::CtxCfg {
                        nb: 2,
                        dc_act: false,
                        luma_eob: false,
                        eob_mode: 0,
                    },
                ),
                (
                    "above+act",
                    lossy::CtxCfg {
                        nb: 1,
                        dc_act: true,
                        luma_eob: false,
                        eob_mode: 0,
                    },
                ),
                (
                    "above+act+leob",
                    lossy::CtxCfg {
                        nb: 1,
                        dc_act: true,
                        luma_eob: true,
                        eob_mode: 0,
                    },
                ),
                (
                    "l+a+act+leob",
                    lossy::CtxCfg {
                        nb: 2,
                        dc_act: true,
                        luma_eob: true,
                        eob_mode: 0,
                    },
                ),
                (
                    "rem,eob:l+a",
                    lossy::CtxCfg {
                        nb: 5,
                        dc_act: false,
                        luma_eob: false,
                        eob_mode: 0,
                    },
                ),
                (
                    "rem,eob:above",
                    lossy::CtxCfg {
                        nb: 5,
                        dc_act: false,
                        luma_eob: false,
                        eob_mode: 4,
                    },
                ),
                (
                    "rem,eob:none",
                    lossy::CtxCfg {
                        nb: 5,
                        dc_act: false,
                        luma_eob: false,
                        eob_mode: 1,
                    },
                ),
                (
                    "rem,eob:laneprev",
                    lossy::CtxCfg {
                        nb: 5,
                        dc_act: false,
                        luma_eob: false,
                        eob_mode: 2,
                    },
                ),
                (
                    "rem,eob:lp+luma",
                    lossy::CtxCfg {
                        nb: 5,
                        dc_act: false,
                        luma_eob: false,
                        eob_mode: 3,
                    },
                ),
            ];
            for (name, color) in [
                ("ycocg", lossy::Color::YCoCgR),
                ("xyb8", lossy::Color::Xyb { x_gain: 8.0 }),
            ] {
                for q in [80u32, 90, 95, 98, 100] {
                    let mut c = mk(Xform::Float, true, Quant::jpeg(q, 2.0));
                    c = c.tune_lossy(|t| { t.color = color; });
                    let rows: Vec<Vec<f64>> = imgs
                        .par_iter()
                        .map(|im| {
                            let r = lossy::run(im, &c);
                            let npx = im.pixels_count() as f64;
                            ccs.iter()
                                .map(|(_, cc)| {
                                    let (a, e, d) = lossy::model_bits(&r, *cc);
                                    (a + e + d) / npx
                                })
                                .collect()
                        })
                        .collect();
                    let mut line = format!("{name:<6} q{q:<3}");
                    let dcs: f64 = imgs
                        .par_iter()
                        .map(|im| {
                            let r = lossy::run(im, &c);
                            let (a, e, d) = lossy::model_bits(&r, ccs[1].1);
                            d / (a + e + d)
                        })
                        .sum::<f64>()
                        / imgs.len() as f64;
                    line += &format!(" dc-share {:.1}%", dcs * 100.0);
                    let base: f64 = rows.iter().map(|r| r[0]).sum::<f64>();
                    for (i, (cn, _)) in ccs.iter().enumerate() {
                        let t: f64 = rows.iter().map(|r| r[i]).sum::<f64>();
                        line += &format!(
                            "  {cn} {:.3} ({:+.1}%)",
                            t / rows.len() as f64,
                            (t / base - 1.0) * 100.0
                        );
                    }
                    println!("{line}");
                }
            }
            return;
        }
        "e4" => {
            let qs = [
                50.0f32, 60.0, 70.0, 75.0, 80.0, 85.0, 88.0, 90.0, 92.0, 94.0, 95.0, 96.0, 97.0,
                98.0, 99.0, 100.0, 102.0, 104.0, 106.0, 108.0,
            ];
            let jq = [
                50.0f32, 60.0, 70.0, 75.0, 80.0, 85.0, 88.0, 90.0, 92.0, 94.0, 95.0, 96.0, 97.0,
                98.0, 99.0, 100.0,
            ];
            let t = std::time::Instant::now();
            // IMIJ_E4_COLORS=xyb (or ycocg, none) narrows the imij variants.
            let all = [
                ("imij_xyb", imij::LossyColor::Xyb),
                ("imij_ycocg", imij::LossyColor::YCoCg),
            ];
            let sel = std::env::var("IMIJ_E4_COLORS").unwrap_or("xyb,ycocg".into());
            let colors: Vec<_> = all
                .into_iter()
                .filter(|(n, _)| sel.split(',').any(|s| n.ends_with(s)))
                .collect();
            let pts = lossy::sweep_real(&imgs, &qs, &colors, &jq);
            eprintln!("sweep {:.1}s", t.elapsed().as_secs_f64());
            lossy_report(&imgs, &pts, which);
            return;
        }
        "e4speed" => {
            println!(
                "{:<10} {:>5} {:>7} {:>9} {:>9}",
                "codec", "q", "bpp", "Mpx/s", "MB/s"
            );
            for (name, q) in [
                ("zenjpeg", 90.0f32),
                ("zenjpeg", 95.0),
                ("zenjpeg", 99.0),
                ("xyb", 90.0),
                ("xyb", 95.0),
                ("xyb", 99.0),
                ("ycocg", 95.0),
            ] {
                let mut bytes = 0usize;
                let mut px = 0usize;
                let mut secs = 0.0;
                for img in &imgs {
                    let enc = match name {
                        "zenjpeg" => lossy::jpeg_encode(img, q),
                        "xyb" => lossy::imij_lossy(img, q, imij::LossyColor::Xyb),
                        _ => lossy::imij_lossy(img, q, imij::LossyColor::YCoCg),
                    };
                    let best = (0..5)
                        .map(|_| {
                            let t = std::time::Instant::now();
                            if name == "zenjpeg" {
                                std::hint::black_box(lossy::jpeg_decode(&enc));
                            } else {
                                std::hint::black_box(imij::decode(&enc).unwrap());
                            }
                            t.elapsed().as_secs_f64()
                        })
                        .fold(f64::MAX, f64::min);
                    secs += best;
                    bytes += enc.len();
                    px += img.pixels_count();
                }
                println!(
                    "{name:<10} {q:>5} {:>7.3} {:>9.1} {:>9.1}",
                    bytes as f64 * 8.0 / px as f64,
                    px as f64 / secs / 1e6,
                    px as f64 * 3.0 / secs / 1e6
                );
            }
            return;
        }
        "packed" => {
            // Dense packed (group widths + raw bits) vs run-level, same coefficients.
            for (name, color) in [
                ("ycocg", lossy::Color::YCoCgR),
                ("xyb8", lossy::Color::Xyb { x_gain: 8.0 }),
            ] {
                for q in [70u32, 80, 90, 95, 98] {
                    let mut c = mk(Xform::Float, true, Quant::jpeg(q, 2.0));
                    c = c.tune_lossy(|t| { t.color = color; });
                    let rows: Vec<Vec<f64>> = imgs
                        .par_iter()
                        .map(|im| {
                            let r = lossy::run(im, &c);
                            let npx = im.pixels_count() as f64;
                            let (b3, t3, _, _) = lossy::model_runlevel(&r, 3, true);
                            let mut v = vec![b3 / npx, t3 as f64 / npx];
                            for gs in [
                                lossy::shapes_zigzag4(),
                                lossy::shapes_halfrow(),
                                lossy::shapes_square(),
                            ] {
                                let (b, t) = lossy::model_packed_shapes(&r, &gs);
                                v.push(b / npx);
                                v.push(t as f64 / npx);
                            }
                            let (b, t) = lossy::model_packed_dec(&r, false, true);
                            v.push(b / npx);
                            v.push(t as f64 / npx);
                            v.push(0.0);
                            v.push(0.0);
                            v
                        })
                        .collect();
                    let n = rows.len() as f64;
                    let m = |i: usize| rows.iter().map(|r| r[i]).sum::<f64>() / n;
                    let mut line = format!(
                        "{name:<6} q{q:<3} runlevel {:.3} bpp {:.3} tok/px",
                        m(0),
                        m(1)
                    );
                    for (j, lab) in ["zz4", "halfrow", "square", "ngrp+nb(ref)", "-"]
                        .iter()
                        .enumerate()
                    {
                        let (b, t) = (m(2 + 2 * j), m(3 + 2 * j));
                        line += &format!(
                            " | {lab} {b:.3} ({:+.1}%) {t:.3} tok",
                            (b / m(0) - 1.0) * 100.0
                        );
                    }
                    println!("{line}");
                }
            }
            return;
        }
        "fastent" => {
            // Entropy layouts that trade size for SIMD decode, same
            // coefficients: run-level (current), packed with entropy-coded
            // counts/widths (as built; its raw bits can live in a separate
            // bit-packed stream at no size cost), and no entropy coder at all
            // (fixed 5-bit counts and widths, groups of 4/8/16).
            for (name, color) in [
                ("ycocg", lossy::Color::YCoCgR),
                ("xyb8", lossy::Color::Xyb { x_gain: 8.0 }),
            ] {
                for q in [50u32, 70, 80, 90, 95, 98] {
                    let mut c = mk(Xform::Float, true, Quant::jpeg(q, 2.0));
                    c = c.tune_lossy(|t| { t.color = color; });
                    let rows: Vec<Vec<f64>> = imgs
                        .par_iter()
                        .map(|im| {
                            let r = lossy::run(im, &c);
                            let npx = im.pixels_count() as f64;
                            let (b3, t3, _, _) = lossy::model_runlevel(&r, 3, true);
                            let mut v = vec![b3 / npx, t3 as f64 / npx];
                            let (b, t) = lossy::model_packed_dec(&r, false, true);
                            v.extend([b / npx, t as f64 / npx]);
                            for g in [4, 8, 16] {
                                let (b, t) = lossy::model_packed_raw(&r, g);
                                v.extend([b / npx, t as f64 / npx]);
                            }
                            v
                        })
                        .collect();
                    let n = rows.len() as f64;
                    let m = |i: usize| rows.iter().map(|r| r[i]).sum::<f64>() / n;
                    let mut line = format!(
                        "{name:<6} q{q:<3} runlevel {:.3} bpp {:.3} tok/px",
                        m(0),
                        m(1)
                    );
                    for (j, lab) in ["packed(rANS w)", "raw g4", "raw g8", "raw g16"]
                        .iter()
                        .enumerate()
                    {
                        let (b, t) = (m(2 + 2 * j), m(3 + 2 * j));
                        line += &format!(
                            " | {lab} {b:.3} ({:+.1}%) {t:.3}/px",
                            (b / m(0) - 1.0) * 100.0
                        );
                    }
                    println!("{line}");
                }
            }
            return;
        }
        "runlevel" => {
            for (name, color) in [
                ("ycocg", lossy::Color::YCoCgR),
                ("xyb8", lossy::Color::Xyb { x_gain: 8.0 }),
            ] {
                for q in [80u32, 90, 95, 98, 100] {
                    let mut c = mk(Xform::Float, true, Quant::jpeg(q, 2.0));
                    c = c.tune_lossy(|t| { t.color = color; });
                    let rows: Vec<[f64; 6]> = imgs
                        .par_iter()
                        .map(|im| {
                            let r = lossy::run(im, &c);
                            let npx = im.pixels_count() as f64;
                            let (b15, t15, bd, td) = lossy::model_runlevel(&r, 15, true);
                            let (b3, t3, _, _) = lossy::model_runlevel(&r, 3, true);
                            [
                                bd / npx,
                                td as f64 / npx,
                                b15 / npx,
                                t15 as f64 / npx,
                                b3 / npx,
                                t3 as f64 / npx,
                            ]
                        })
                        .collect();
                    let n = rows.len() as f64;
                    let m = |i: usize| rows.iter().map(|r| r[i]).sum::<f64>() / n;
                    println!(
                        "{name:<6} q{q:<3} dense {:.3} bpp {:.3} tok/px | runlevel15 {:.3} bpp ({:+.1}%) {:.3} tok/px ({:+.0}%) | runlevel3 {:.3} ({:+.1}%) {:.3} tok/px ({:+.0}%)",
                        m(0),
                        m(1),
                        m(2),
                        (m(2) / m(0) - 1.0) * 100.0,
                        m(3),
                        (m(3) / m(1) - 1.0) * 100.0,
                        m(4),
                        (m(4) / m(0) - 1.0) * 100.0,
                        m(5),
                        (m(5) / m(1) - 1.0) * 100.0
                    );
                }
            }
            return;
        }
        "rlexact" => {
            let variants: [(&str, usize, u32, u32); 6] = [
                ("codec(run3,d1,m0)", 3, 1, 0),
                ("run3,d2,m0", 3, 2, 0),
                ("run3,d4,m1", 3, 4, 1),
                ("run3,d8,m1", 3, 8, 1),
                ("run2,d8,m1", 2, 8, 1),
                ("run3,d4,m2", 3, 4, 2),
            ];
            for (name, color) in [("xyb8", lossy::Color::Xyb { x_gain: 8.0 })] {
                for q in [80u32, 90, 95, 98] {
                    let mut c = mk(Xform::Float, true, Quant::jpeg(q, 1.0));
                    c = c.tune_lossy(|t| { t.color = color; });
                    let rows: Vec<Vec<(f64, f64)>> = imgs
                        .par_iter()
                        .map(|im| {
                            let r = lossy::run(im, &c);
                            let npx = im.pixels_count() as f64;
                            let (a, e, _) = lossy::model_bits(
                                &r,
                                lossy::CtxCfg {
                                    nb: 5,
                                    dc_act: false,
                                    luma_eob: false,
                                    eob_mode: 3,
                                },
                            );
                            let mut v = vec![((a + e) / npx, r.tokens_per_px)];
                            for (_, mr, d, m) in variants {
                                let (b, t) = lossy::model_rl_exact(&r, mr, d, m);
                                v.push((b / npx, t as f64 / npx));
                            }
                            v
                        })
                        .collect();
                    let n = rows.len() as f64;
                    let m = |i: usize| {
                        (
                            rows.iter().map(|r| r[i].0).sum::<f64>() / n,
                            rows.iter().map(|r| r[i].1).sum::<f64>() / n,
                        )
                    };
                    let (db, dt) = m(0);
                    let mut line = format!("{name} q{q}: dense {db:.3}b {dt:.3}t");
                    for (i, (vn, ..)) in variants.iter().enumerate() {
                        let (b, t) = m(i + 1);
                        line +=
                            &format!(" | {vn} {b:.3}b({:+.1}%) {t:.3}t", (b / db - 1.0) * 100.0);
                    }
                    println!("{line}");
                }
            }
            return;
        }
        "sb" => {
            println!("{:<4} {:>3} {:>8} {:>9}", "q", "sb", "bpp", "Mpx/s");
            for q in [90.0f32, 95.0] {
                for sb in [1u32, 2, 4, 8, 16] {
                    let mut bytes = 0usize;
                    let mut px = 0usize;
                    let mut secs = 0.0;
                    for img in &imgs {
                        let c = imij::ImiEncoderConfig::lossy(q).tune_lossy(|t| t.stripe_block_rows = sb);
                        let enc = c.encode_samples(imij::ImageRef::new(&img.pixels, img.info.width as usize * 3, &img.info), &imij::Metadata::default())
                        .unwrap();
                        secs += time_best(5, || {
                            std::hint::black_box(imij::decode(&enc).unwrap());
                        });
                        bytes += enc.len();
                        px += img.pixels_count();
                    }
                    println!(
                        "{q:<4} {sb:>3} {:>8.4} {:>9.1}",
                        bytes as f64 * 8.0 / px as f64,
                        px as f64 / secs / 1e6
                    );
                }
            }
            return;
        }
        "hdr16" => {
            let imgs: Vec<Img> = corpus::load_manifest(manifest, filter)
                .iter()
                .filter_map(|(class, path)| corpus::load_png(class, Path::new(path)).ok())
                .filter(|i| i.info.channels == Channels::Rgb && i.info.sample == SampleType::U16)
                .collect();
            println!("{} images (16-bit RGB)", imgs.len());
            let px: usize = imgs.iter().map(|i| i.pixels_count()).sum();
            let ll: usize = imgs
                .iter()
                .map(|im| {
                    imij::ImiEncoderConfig::default().encode_samples(imij::ImageRef::new(&im.pixels, im.info.row_bytes(), &im.info), &imij::Metadata::default())
                    .unwrap()
                    .len()
                })
                .sum();
            println!("lossless {:.3} bpp", ll as f64 * 8.0 / px as f64);
            for q in [70.0f32, 80.0, 90.0, 95.0, 100.0, 104.0, 108.0, 112.0, 116.0] {
                let mut bytes = 0usize;
                let mut secs = 0.0;
                for im in &imgs {
                    let enc = imij::ImiEncoderConfig::lossy(q).encode_samples(imij::ImageRef::new(&im.pixels, im.info.row_bytes(), &im.info), &imij::Metadata::default())
                    .unwrap();
                    secs += time_best(3, || {
                        std::hint::black_box(imij::decode(&enc).unwrap());
                    });
                    bytes += enc.len();
                }
                // Size and speed only; quality is in `hdrpu` / `hdrpq`.
                println!(
                    "q{q:<5} {:>7.3} bpp  {:>6.1} Mpx/s",
                    bytes as f64 * 8.0 / px as f64,
                    px as f64 / secs / 1e6
                );
            }
            return;
        }
        "hdrlin" => {
            let imgs: Vec<Img> = corpus::load_manifest(manifest, filter)
                .iter()
                .filter_map(|(class, path)| corpus::load_png(class, Path::new(path)).ok())
                .filter(|i| i.info.channels == Channels::Rgb && i.info.sample == SampleType::U16)
                .collect();
            let pq_eotf = |e: f64| -> f64 {
                let (m1, m2, c1, c2, c3) =
                    (0.1593017578125, 78.84375, 0.8359375, 18.8515625, 18.6875);
                let p = e.max(0.0).powf(1.0 / m2);
                ((p - c1).max(0.0) / (c2 - c3 * p)).powf(1.0 / m1)
            };
            let px: usize = imgs.iter().map(|i| i.pixels_count()).sum();
            for q in [70.0f32, 80.0, 90.0, 95.0, 100.0, 104.0, 108.0, 112.0, 116.0] {
                let mut bytes = 0usize;
                let mut secs = 0.0;
                for im in &imgs {
                    let codes: Vec<u16> = im
                        .pixels
                        .chunks_exact(2)
                        .map(|c| u16::from_ne_bytes([c[0], c[1]]))
                        .collect();
                    let lin: Vec<u8> = codes
                        .iter()
                        .flat_map(|&v| {
                            ((pq_eotf(v as f64 / 65535.0) * 10000.0 / 203.0) as f32).to_ne_bytes()
                        })
                        .collect();
                    let mut info = im.info.clone();
                    info.sample = SampleType::F32;
                    let enc = imij::ImiEncoderConfig::lossy(q).encode_samples(imij::ImageRef::new(&lin, info.row_bytes(), &info), &imij::Metadata::default())
                    .unwrap();
                    secs += time_best(3, || {
                        std::hint::black_box(imij::decode(&enc).unwrap());
                    });
                    bytes += enc.len();
                }
                println!(
                    "q{q:<5} {:>7.3} bpp  {:>6.1} Mpx/s",
                    bytes as f64 * 8.0 / px as f64,
                    px as f64 / secs / 1e6
                );
            }
            return;
        }
        "hdrpu" => {
            // Perceptual HDR comparison: zensim PU21 path (absolute nits) for
            // 16-bit PQ (YCoCg) vs linear f32 (XYB) coding of the same images.
            let imgs: Vec<Img> = corpus::load_manifest(manifest, filter)
                .iter()
                .filter_map(|(class, path)| corpus::load_png(class, Path::new(path)).ok())
                .filter(|i| i.info.channels == Channels::Rgb && i.info.sample == SampleType::U16)
                .collect();
            let pq_eotf = |e: f64| -> f64 {
                let (m1, m2, c1, c2, c3) =
                    (0.1593017578125, 78.84375, 0.8359375, 18.8515625, 18.6875);
                let p = e.max(0.0).powf(1.0 / m2);
                ((p - c1).max(0.0) / (c2 - c3 * p)).powf(1.0 / m1) * 10000.0
            };
            let z = zensim::Zensim::new(zensim::ZensimProfile::codec_target());
            let px: usize = imgs.iter().map(|i| i.pixels_count()).sum();
            println!(
                "{:<6} {:>9} {:>8} {:>9} {:>8}",
                "q", "pq16 bpp", "zensim", "lin bpp", "zensim"
            );
            for q in [60.0f32, 70.0, 80.0, 90.0, 95.0, 100.0, 104.0, 108.0] {
                let (mut b16, mut s16, mut bl, mut sl) = (0usize, 0.0, 0usize, 0.0);
                for im in &imgs {
                    let (w, h) = (im.info.width as usize, im.info.height as usize);
                    let codes: Vec<u16> = im
                        .pixels
                        .chunks_exact(2)
                        .map(|c| u16::from_ne_bytes([c[0], c[1]]))
                        .collect();
                    let nits: Vec<f32> = codes
                        .iter()
                        .map(|&v| pq_eotf(v as f64 / 65535.0) as f32)
                        .collect();
                    // 16-bit PQ path
                    let e16 = imij::ImiEncoderConfig::lossy(q).encode_samples(imij::ImageRef::new(&im.pixels, im.info.row_bytes(), &im.info), &imij::Metadata::default())
                    .unwrap();
                    let (_, d16) = imij::decode(&e16).unwrap();
                    let n16: Vec<f32> = d16
                        .chunks_exact(2)
                        .map(|c| pq_eotf(u16::from_ne_bytes([c[0], c[1]]) as f64 / 65535.0) as f32)
                        .collect();
                    s16 += z
                        .compute_pu_linear(&nits, &n16, w, h, 3 * w, 3 * w)
                        .unwrap()
                        .score();
                    b16 += e16.len();
                    // linear path (1.0 = 203 nits)
                    let mut info = im.info.clone();
                    info.sample = SampleType::F32;
                    let lin: Vec<u8> = nits
                        .iter()
                        .flat_map(|&n| (n / 203.0).to_ne_bytes())
                        .collect();
                    let el = imij::ImiEncoderConfig::lossy(q).encode_samples(imij::ImageRef::new(&lin, info.row_bytes(), &info), &imij::Metadata::default())
                    .unwrap();
                    let (_, dl) = imij::decode(&el).unwrap();
                    let nl: Vec<f32> = dl
                        .chunks_exact(4)
                        .map(|c| f32::from_ne_bytes(c.try_into().unwrap()).max(0.0) * 203.0)
                        .collect();
                    sl += z
                        .compute_pu_linear(&nits, &nl, w, h, 3 * w, 3 * w)
                        .unwrap()
                        .score();
                    bl += el.len();
                }
                let n = imgs.len() as f64;
                println!(
                    "q{q:<5} {:>9.3} {:>8.2} {:>9.3} {:>8.2}",
                    b16 as f64 * 8.0 / px as f64,
                    s16 / n,
                    bl as f64 * 8.0 / px as f64,
                    sl / n
                );
            }
            return;
        }
        "hdrpq" => {
            // 16-bit PQ input coded as YCoCg vs XYB (PQ transfer), scored with
            // SSIMULACRA2-PU21 (fast-ssim2 hdr-pu) and zensim PU21 on nits.
            let imgs: Vec<Img> = corpus::load_manifest(manifest, filter)
                .iter()
                .filter_map(|(class, path)| corpus::load_png(class, Path::new(path)).ok())
                .filter(|i| i.info.channels == Channels::Rgb && i.info.sample == SampleType::U16)
                .collect();
            let pq_nits = |e: f64| -> f64 {
                let (m1, m2, c1, c2, c3) =
                    (0.1593017578125, 78.84375, 0.8359375, 18.8515625, 18.6875);
                let p = e.max(0.0).powf(1.0 / m2);
                ((p - c1).max(0.0) / (c2 - c3 * p)).powf(1.0 / m1) * 10000.0
            };
            let z = zensim::Zensim::new(zensim::ZensimProfile::codec_target());
            let px: usize = imgs.iter().map(|i| i.pixels_count()).sum();
            println!(
                "{:<6} {:>10} {:>8} {:>8} {:>8} | {:>9} {:>8} {:>8} {:>8}",
                "q",
                "ycocg bpp",
                "ssim2pu",
                "zensim",
                "Mpx/s",
                "xyb bpp",
                "ssim2pu",
                "zensim",
                "Mpx/s"
            );
            for q in [50.0f32, 60.0, 70.0, 80.0, 90.0, 95.0, 100.0, 104.0, 108.0] {
                let mut line = format!("q{q:<5}");
                for color in [imij::LossyColor::YCoCg, imij::LossyColor::Xyb] {
                    let (mut bytes, mut s2, mut zs, mut secs) = (0usize, 0.0, 0.0, 0.0);
                    for im in &imgs {
                        let (w, h) = (im.info.width, im.info.height);
                        let mut info = im.info.clone();
                        info.cicp = imij::Cicp::BT2100_PQ;
                        let enc = imij::ImiEncoderConfig::lossy(q).tune_lossy(|t| t.color = color).encode_samples(imij::ImageRef::new(&im.pixels, info.row_bytes(), &info), &imij::Metadata::default())
                        .unwrap();
                        let (_, dec) = imij::decode(&enc).unwrap();
                        secs += time_best(3, || {
                            std::hint::black_box(imij::decode(&enc).unwrap());
                        });
                        let d = fast_ssim2::PixelDescriptor::RGB16_BT2100_PQ;
                        let a = fast_ssim2::PixelSlice::new(&im.pixels, w, h, w as usize * 6, d)
                            .unwrap();
                        let b = fast_ssim2::PixelSlice::new(&dec, w, h, w as usize * 6, d).unwrap();
                        s2 += fast_ssim2::compute_ssimulacra2_pu(&a, &b).unwrap();
                        let nits = |p: &[u8]| -> Vec<f32> {
                            p.chunks_exact(2)
                                .map(|c| {
                                    pq_nits(u16::from_ne_bytes([c[0], c[1]]) as f64 / 65535.0)
                                        as f32
                                })
                                .collect()
                        };
                        zs += z
                            .compute_pu_linear(
                                &nits(&im.pixels),
                                &nits(&dec),
                                w as usize,
                                h as usize,
                                3 * w as usize,
                                3 * w as usize,
                            )
                            .unwrap()
                            .score();
                        bytes += enc.len();
                    }
                    let n = imgs.len() as f64;
                    line += &format!(
                        " {:>10.3} {:>8.2} {:>8.2} {:>8.1} |",
                        bytes as f64 * 8.0 / px as f64,
                        s2 / n,
                        zs / n,
                        px as f64 / secs / 1e6
                    );
                }
                println!("{line}");
            }
            return;
        }
        "encspeed" => {
            let px: usize = imgs.iter().map(|i| i.pixels_count()).sum();
            for (q, threads) in [(90.0f32, 1usize), (90.0, 8)] {
                let t = std::time::Instant::now();
                for im in &imgs {
                    let c = imij::ImiEncoderConfig::lossy(q).with_threads(threads);
                    std::hint::black_box(
                        c.encode_samples(imij::ImageRef::new(&im.pixels, im.info.width as usize * 3, &im.info), &imij::Metadata::default())
                        .unwrap(),
                    );
                }
                let s = t.elapsed().as_secs_f64();
                let t2 = std::time::Instant::now();
                for im in &imgs {
                    std::hint::black_box(lossy::jpeg_encode(im, q));
                }
                let sj = t2.elapsed().as_secs_f64();
                println!(
                    "q{q} threads {threads}: imij encode {:.1} Mpx/s, zenjpeg encode (1 thread) {:.1} Mpx/s",
                    px as f64 / s / 1e6,
                    px as f64 / sj / 1e6
                );
            }
            return;
        }
        "parspeed" => {
            // Decode speed vs zenjpeg baseline + restart markers (parallel
            // segment decode on a rayon pool of the same size), imij XYB at the
            // quality matching zenjpeg's SSIMULACRA2.
            let threads: Vec<usize> = std::env::var("IMIJ_THREADS")
                .unwrap_or("1,4,8,12".into())
                .split(',')
                .map(|v| v.parse().unwrap())
                .collect();
            let iters: usize = std::env::var("IMIJ_ITERS")
                .ok()
                .map(|v| v.parse().unwrap())
                .unwrap_or(10);
            let best = |n: usize, f: &mut dyn FnMut()| {
                let mut b = f64::MAX;
                for _ in 0..n {
                    let t = std::time::Instant::now();
                    f();
                    b = b.min(t.elapsed().as_secs_f64());
                }
                b
            };
            let mut head = format!(
                "{:<8} {:>3} {:>3} {:>6} {:>5} {:>6} {:>6} {:>4}",
                "img", "sub", "jq", "jbpp", "zq", "zbpp", "ssim2", "rst"
            );
            for t in &threads {
                head += &format!(" | {t:>2}T imij/jpeg Mpx/s  x");
            }
            println!("{head}");
            for img in &imgs {
                let (w, h) = (img.info.width as usize, img.info.height as usize);
                let npx = (w * h) as f64;
                let refslice = fast_ssim2::PixelSlice::new(
                    &img.pixels,
                    img.info.width,
                    img.info.height,
                    w * 3,
                    fast_ssim2::PixelDescriptor::RGB8_SRGB,
                )
                .unwrap();
                let reference = fast_ssim2::Ssimulacra2Reference::new(&refslice).unwrap();
                for quarter in [false, true] {
                    for jq in [85.0f32, 90.0, 95.0] {
                        let je = lossy::jpeg_encode_sub(img, jq, quarter);
                        let rst = je
                            .windows(2)
                            .filter(|p| p[0] == 0xFF && (0xD0..=0xD7).contains(&p[1]))
                            .count();
                        let js = lossy::ssim2(&reference, img, &lossy::jpeg_decode(&je));
                        let color = if std::env::var("IMIJ_PAR_COLOR").as_deref() == Ok("ycocg") {
                            imij::LossyColor::YCoCg
                        } else {
                            imij::LossyColor::Xyb
                        };
                        let (mut lo, mut hi) = (20.0f32, 104.0f32);
                        for _ in 0..8 {
                            let mid = (lo + hi) * 0.5;
                            let e = lossy::imij_lossy(img, mid, color);
                            let (_, d) = imij::decode(&e).unwrap();
                            if lossy::ssim2(&reference, img, &d) >= js {
                                hi = mid;
                            } else {
                                lo = mid;
                            }
                        }
                        let ze = lossy::imij_lossy(img, hi, color);
                        if let Ok(dir) = std::env::var("IMIJ_SPEED_OUT") {
                            let stem = format!(
                                "{dir}/{}_{}_q{jq}",
                                img.name,
                                if quarter { 420 } else { 444 }
                            );
                            std::fs::write(format!("{stem}.jpg"), &je).unwrap();
                            std::fs::write(format!("{stem}.imi"), &ze).unwrap();
                        }
                        let mut line = format!(
                            "{:<8} {:>3} {:>3} {:>6.3} {:>5.1} {:>6.3} {:>6.2} {:>4}",
                            img.name,
                            if quarter { 420 } else { 444 },
                            jq,
                            je.len() as f64 * 8.0 / npx,
                            hi,
                            ze.len() as f64 * 8.0 / npx,
                            js,
                            rst
                        );
                        let mut buf = vec![0u8; w * h * 3];
                        for &t in &threads {
                            let tz = best(iters, &mut || {
                                imij::decode_into_threads(&ze, &mut buf, w * 3, t).unwrap();
                            });
                            let pool = rayon::ThreadPoolBuilder::new()
                                .num_threads(t)
                                .build()
                                .unwrap();
                            let dec = zenjpeg::decoder::Decoder::new().num_threads(if t == 1 {
                                1
                            } else {
                                0
                            });
                            let tj = pool.install(|| {
                                let a = best(iters, &mut || {
                                    std::hint::black_box(
                                        dec.decode(&je, zenjpeg::encoder::Unstoppable).unwrap(),
                                    );
                                });
                                // decode_into fails on some baseline 4:2:0 RST files
                                // (zenjpeg InternalError); fall back to decode().
                                let ok = dec
                                    .decode_into(
                                        &je,
                                        zenjpeg::decoder::PixelFormat::Rgb,
                                        &mut buf,
                                        zenjpeg::encoder::Unstoppable,
                                    )
                                    .is_ok();
                                if ok {
                                    let b = best(iters, &mut || {
                                        dec.decode_into(
                                            &je,
                                            zenjpeg::decoder::PixelFormat::Rgb,
                                            &mut buf,
                                            zenjpeg::encoder::Unstoppable,
                                        )
                                        .unwrap();
                                    });
                                    a.min(b)
                                } else {
                                    a
                                }
                            });
                            line += &format!(
                                " | {:>6.0} {:>6.0} {:>4.2}x",
                                npx / tz / 1e6,
                                npx / tj / 1e6,
                                tj / tz
                            );
                        }
                        println!("{line}");
                    }
                }
            }
            return;
        }
        "bamax" => {
            // Where butteraugli's errors land: imij XYB vs zenjpeg 4:4:4 at
            // matched size, per 8x8 block bucketed by local luma / saturation.
            let zq: f32 = std::env::var("IMIJ_BA_Q")
                .ok()
                .map(|v| v.parse().unwrap())
                .unwrap_or(90.0);
            let jq: Vec<f32> = (50..=100).map(|q| q as f32).collect();
            // buckets: luma (BT.601 of sRGB codes) <32, <64, <128, <192, >=192;
            // saturation (max-min) <16, <48, <96, >=96
            let rows: Vec<String> = imgs
                .par_iter()
                .map(|img| {
                    let (w, h) = (img.info.width as usize, img.info.height as usize);
                    let npx = (w * h) as f64;
                    let ze = lossy::imij_lossy(img, zq, imij::LossyColor::Xyb);
                    let zb = ze.len() as f64 * 8.0 / npx;
                    let (_, zd) = imij::decode(&ze).unwrap();
                    let je = jq
                        .iter()
                        .map(|&q| lossy::jpeg_encode(img, q))
                        .min_by(|a, b| {
                            let da = (a.len() as f64 * 8.0 / npx - zb).abs();
                            let db = (b.len() as f64 * 8.0 / npx - zb).abs();
                            da.partial_cmp(&db).unwrap()
                        })
                        .unwrap();
                    let jb = je.len() as f64 * 8.0 / npx;
                    let jd = lossy::jpeg_decode(&je);
                    let dm = |px: &[u8]| {
                        let to = |b: &[u8]| -> Vec<butteraugli::RGB8> {
                            b.chunks_exact(3)
                                .map(|c| butteraugli::RGB8::new(c[0], c[1], c[2]))
                                .collect()
                        };
                        let a = butteraugli::Img::new(to(&img.pixels), w, h);
                        let b = butteraugli::Img::new(to(px), w, h);
                        let r = butteraugli::butteraugli(
                            a.as_ref(),
                            b.as_ref(),
                            &butteraugli::ButteraugliParams::default().with_compute_diffmap(true),
                        )
                        .unwrap();
                        (r.score, r.pnorm_3, r.diffmap.unwrap())
                    };
                    let (zs, z3, zm) = dm(&zd);
                    let (js, j3, jm) = dm(&jd);
                    let px = |x: usize, y: usize| &img.pixels[(y * w + x) * 3..][..3];
                    let luma = |p: &[u8]| 0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64;
                    let lb = |l: f64| [32.0, 64.0, 128.0, 192.0].iter().filter(|&&t| l >= t).count();
                    let sb = |p: &[u8]| {
                        let s = *p.iter().max().unwrap() as f64 - *p.iter().min().unwrap() as f64;
                        [16.0, 48.0, 96.0].iter().filter(|&&t| s >= t).count()
                    };
                    // per-bucket sums of diff^3 and pixel counts
                    let mut acc = [[0.0f64; 3]; 9];
                    let mut edge = [0.0f64; 2];
                    let mut tot = [0.0f64; 2];
                    for y in 0..h {
                        for x in 0..w {
                            let p = px(x, y);
                            let (l, sa) = (lb(luma(p)), sb(p));
                            let z = zm.buf()[y * zm.stride() + x] as f64;
                            let j = jm.buf()[y * jm.stride() + x] as f64;
                            for (k, v) in [(l, 0usize), (5 + sa, 0)] {
                                let _ = v;
                                acc[k][0] += z * z * z;
                                acc[k][1] += j * j * j;
                                acc[k][2] += 1.0;
                            }
                            let on_edge = x % 8 == 0 || x % 8 == 7 || y % 8 == 0 || y % 8 == 7;
                            if on_edge {
                                edge[0] += z * z * z;
                                edge[1] += j * j * j;
                            }
                            tot[0] += z * z * z;
                            tot[1] += j * j * j;
                        }
                    }
                    let argmax = |m: &butteraugli::ImgVec<f32>| {
                        let mut best = (0.0f32, 0usize, 0usize);
                        for y in 0..h {
                            for x in 0..w {
                                let v = m.buf()[y * m.stride() + x];
                                if v > best.0 {
                                    best = (v, x, y);
                                }
                            }
                        }
                        best
                    };
                    let (zv, zx, zy) = argmax(&zm);
                    let (jv, jx, jy) = argmax(&jm);
                    let mut line = format!(
                        "{:<10} imij {zb:.2}b max {zs:5.2} p3 {z3:4.2} @({zx},{zy})%8=({},{}) luma {:3.0} | jpeg {jb:.2}b max {js:5.2} p3 {j3:4.2} @({jx},{jy})%8=({},{}) luma {:3.0} | edge share z {:.2} j {:.2} |",
                        img.name,
                        zx % 8,
                        zy % 8,
                        luma(px(zx, zy)),
                        jx % 8,
                        jy % 8,
                        luma(px(jx, jy)),
                        edge[0] / tot[0],
                        edge[1] / tot[1],
                    );
                    let _ = (zv, jv);
                    for (k, a) in acc.iter().enumerate() {
                        let n = a[2].max(1.0);
                        line += &format!(
                            " {}{}:{:.3}/{:.3}",
                            if k < 5 { "L" } else { "S" },
                            if k < 5 { k } else { k - 5 },
                            a[0] / n,
                            a[1] / n
                        );
                    }
                    line
                })
                .collect();
            println!(
                "# imij XYB q{zq} vs zenjpeg444 at nearest size; buckets: mean diff^3 imij/jpeg"
            );
            println!(
                "# L0..L4: luma <32,<64,<128,<192,>=192   S0..S3: sat (max-min) <16,<48,<96,>=96"
            );
            for r in rows {
                println!("{r}");
            }
            return;
        }
        "matrix" => {
            // Real-codec sweep of base quantization matrices: size vs zenjpeg at
            // equal SSIMULACRA2 and zensim.
            let variants: Vec<(String, u8)> = std::env::var("IMIJ_MATRIX")
                .unwrap_or("0,1,2".into())
                .split(',')
                .map(|v| (format!("matrix{v}"), v.parse().unwrap()))
                .collect();
            let qs: Vec<f32> = std::env::var("IMIJ_QS")
                .unwrap_or("30,40,50,60,70,80,85,90,93,95,97,98,99,100".into())
                .split(',')
                .map(|v| v.parse().unwrap())
                .collect();
            let jq = [
                50.0f32, 60.0, 70.0, 75.0, 80.0, 85.0, 88.0, 90.0, 92.0, 94.0, 95.0, 96.0, 97.0,
                98.0, 99.0, 100.0,
            ];
            let pts: Vec<lossy::Point> = imgs
                .par_iter()
                .enumerate()
                .flat_map(|(ii, img)| {
                    let refslice = fast_ssim2::PixelSlice::new(
                        &img.pixels,
                        img.info.width,
                        img.info.height,
                        img.info.width as usize * 3,
                        fast_ssim2::PixelDescriptor::RGB8_SRGB,
                    )
                    .unwrap();
                    let reference = fast_ssim2::Ssimulacra2Reference::new(&refslice).unwrap();
                    let npx = img.pixels_count() as f64;
                    let mut pts = Vec::new();
                    let mut add = |variant: &str, setting: String, n: usize, dec: &[u8]| {
                        let ba = lossy::butteraugli_scores(img, dec);
                        pts.push(lossy::Point {
                            img: ii,
                            variant: variant.into(),
                            setting,
                            bpp: n as f64 * 8.0 / npx,
                            ssim2: lossy::ssim2(&reference, img, dec),
                            tok: 0.0,
                            zensim: lossy::zensim_score(img, dec),
                            ba3: ba.0,
                            bamax: ba.1,
                            jod: f64::NAN,
                        });
                    };
                    for &q in &jq {
                        let enc = lossy::jpeg_encode(img, q);
                        add(
                            "zenjpeg444",
                            format!("q{q}"),
                            enc.len(),
                            &lossy::jpeg_decode(&enc),
                        );
                    }
                    for (name, m) in &variants {
                        for &q in qs.iter() {
                            let mut c = imij::ImiEncoderConfig::lossy(q);
                            c = c.tune_lossy(|t| { t.matrix = *m; });
                            let enc = lossy::imij_lossy_cfg(img, &c);
                            let (_, dec) = imij::decode(&enc).unwrap();
                            add(name, format!("q{q}"), enc.len(), &dec);
                        }
                    }
                    pts
                })
                .collect();
            print!("{}", lossy::metric_tables(&pts, imgs.len(), "zenjpeg444"));
            return;
        }
        "edges" => {
            // Edge artifacts from cvvdp (FHD) distortion maps (imij 4:4:4, zenjpeg
            // 4:4:4 and 4:2:0): stripe-seam
            // and 8-px block-edge ratios (lossy::edge_ratios), the mean
            // distortion profile over row % 64, and heat-map PNGs.
            let qs: Vec<f32> = std::env::var("IMIJ_QS")
                .unwrap_or("30,60,85,95".into())
                .split(',')
                .map(|v| v.parse().unwrap())
                .collect();
            let out = std::env::var("IMIJ_MAPS").unwrap_or("edges".into());
            let nmaps: usize = std::env::var("IMIJ_NMAPS")
                .map(|v| v.parse().unwrap())
                .unwrap_or(3);
            std::fs::create_dir_all(&out).unwrap();
            // (variant, q) -> per image (jod, seam ratio, block ratio, row profile)
            type Row = (String, f32, f64, f64, f64, Vec<f64>);
            let rows: Vec<(usize, Row)> = imgs
                .par_iter()
                .enumerate()
                .flat_map(|(ii, img)| {
                    let w = img.info.width as usize;
                    let mut res = Vec::new();
                    for &q in &qs {
                        let mut dec: Vec<(String, Vec<u8>)> = Vec::new();
                        for quarter in [false, true] {
                            let e = lossy::jpeg_encode_chroma(img, q, quarter);
                            let n = format!("zenjpeg{}", if quarter { 420 } else { 444 });
                            dec.push((n, lossy::jpeg_decode(&e)));
                        }
                        for (cn, color) in [
                            ("xyb", imij::LossyColor::Xyb),
                            ("ycocg", imij::LossyColor::YCoCg),
                        ] {
                            let c = imij::ImiEncoderConfig::lossy(q).tune_lossy(|t| t.color = color);
                            let e = lossy::imij_lossy_cfg(img, &c);
                            dec.push((format!("imij-{cn}444"), imij::decode(&e).unwrap().1));
                        }
                        let mut maps = Vec::new();
                        for (n, d) in &dec {
                            let mut map = Vec::new();
                            let jod = lossy::cvvdp_jod_map(img, d, &mut map);
                            let (seam, block) = lossy::edge_ratios(&map, w, 64, 2);
                            let mut prof = vec![0.0f64; 64];
                            let mut cnt = vec![0usize; 64];
                            for (y, row) in map.chunks_exact(w).enumerate() {
                                prof[y % 64] += row.iter().map(|&v| v as f64).sum::<f64>();
                                cnt[y % 64] += w;
                            }
                            let mean =
                                map.iter().map(|&v| v as f64).sum::<f64>() / map.len() as f64;
                            for (p, c) in prof.iter_mut().zip(&cnt) {
                                *p /= *c as f64 * mean.max(1e-30);
                            }
                            res.push((ii, (n.clone(), q, jod, seam, block, prof)));
                            maps.push(map);
                        }
                        if ii < nmaps {
                            // One scale per (image, q) so the variants compare.
                            let mut all: Vec<f32> = maps.iter().flatten().copied().collect();
                            all.sort_by(|a, b| a.total_cmp(b));
                            let scale = all[all.len() * 995 / 1000].max(1e-12);
                            for ((n, d), map) in dec.iter().zip(&maps) {
                                let base = format!("{out}/{}_q{q}_{n}", img.name);
                                lossy::write_heatmap(&format!("{base}_map.png"), map, w, scale);
                                let px: Vec<rgb::RGB8> = d
                                    .chunks_exact(3)
                                    .map(|c| rgb::RGB8::new(c[0], c[1], c[2]))
                                    .collect();
                                lossy::write_png(&format!("{base}.png"), &px, w);
                            }
                        }
                    }
                    res
                })
                .collect();
            let mut keys: Vec<(String, f32)> =
                rows.iter().map(|(_, r)| (r.0.clone(), r.1)).collect();
            keys.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
            keys.dedup();
            println!(
                "{:<14} {:>4} {:>6} {:>7} {:>7} {:>7}  profile over row%64 (mean = 1): rows 60..63 | 0..3 || 30..33",
                "variant", "q", "JOD", "seam", "seamMax", "block"
            );
            for (v, q) in &keys {
                let sel: Vec<&Row> = rows
                    .iter()
                    .map(|(_, r)| r)
                    .filter(|r| &r.0 == v && r.1 == *q)
                    .collect();
                let n = sel.len() as f64;
                let mean = |f: &dyn Fn(&Row) -> f64| sel.iter().map(|r| f(r)).sum::<f64>() / n;
                let mut prof = vec![0.0; 64];
                for r in &sel {
                    for (p, x) in prof.iter_mut().zip(&r.5) {
                        *p += x / n;
                    }
                }
                let f = |ys: &[usize]| {
                    ys.iter()
                        .map(|&y| format!("{:.2}", prof[y]))
                        .collect::<Vec<_>>()
                        .join(" ")
                };
                println!(
                    "{v:<14} {q:>4} {:>6.3} {:>7.3} {:>7.3} {:>7.3}  {} | {} || {}",
                    mean(&|r| r.2),
                    mean(&|r| r.3),
                    sel.iter().map(|r| r.3).fold(0.0, f64::max),
                    mean(&|r| r.4),
                    f(&[60, 61, 62, 63]),
                    f(&[0, 1, 2, 3]),
                    f(&[30, 31, 32, 33]),
                );
            }
            return;
        }
        "mtime" => {
            // Per-call metric cost, one thread, best of 3, on an IMI q60 decode.
            println!(
                "{:<10} {:>6} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}",
                "img", "MP", "ssim2", "ssim2-wr", "zensim", "butter", "cvvdp-wr", "cvvdp-map"
            );
            for img in &imgs {
                let enc = lossy::imij_lossy_cfg(img, &imij::ImiEncoderConfig::lossy(60.0));
                let dec = imij::decode(&enc).unwrap().1;
                let (w, h) = (img.info.width, img.info.height);
                fn slice(p: &[u8], w: u32, h: u32) -> fast_ssim2::PixelSlice<'_> {
                    let d = fast_ssim2::PixelDescriptor::RGB8_SRGB;
                    fast_ssim2::PixelSlice::new(p, w, h, w as usize * 3, d).unwrap()
                }
                let t_s2 = time_best(3, || {
                    let r =
                        fast_ssim2::Ssimulacra2Reference::new(&slice(&img.pixels, w, h)).unwrap();
                    std::hint::black_box(r.compare(&slice(&dec, w, h)).unwrap());
                });
                let r = fast_ssim2::Ssimulacra2Reference::new(&slice(&img.pixels, w, h)).unwrap();
                let t_s2w = time_best(3, || {
                    std::hint::black_box(r.compare(&slice(&dec, w, h)).unwrap());
                });
                let t_z = time_best(3, || {
                    std::hint::black_box(lossy::zensim_score(img, &dec));
                });
                let t_ba = time_best(3, || {
                    std::hint::black_box(lossy::butteraugli_scores(img, &dec));
                });
                let mut cv = lossy::cvvdp_fhd(w, h);
                cv.warm_reference(&img.pixels).unwrap();
                let t_cw = time_best(3, || {
                    std::hint::black_box(cv.score_with_warm_ref(&dec).unwrap());
                });
                let mut map = Vec::new();
                let t_cm = time_best(3, || {
                    std::hint::black_box(cv.score_with_warm_ref_diffmap(&dec, &mut map).unwrap());
                });
                println!(
                    "{:<10} {:>6.2} {:>8.0}ms {:>8.0}ms {:>8.0}ms {:>8.0}ms {:>8.0}ms {:>8.0}ms",
                    img.name,
                    (w * h) as f64 / 1e6,
                    t_s2 * 1e3,
                    t_s2w * 1e3,
                    t_z * 1e3,
                    t_ba * 1e3,
                    t_cw * 1e3,
                    t_cm * 1e3
                );
            }
            return;
        }
        "qmul" => {
            // Real-codec sweep of XYB channel multipliers: size vs zenjpeg at
            // equal SSIMULACRA2 and zensim.
            let variants: Vec<(String, [f32; 3])> = std::env::var("IMIJ_QMUL")
                .unwrap_or("1,1,1;1,1,1.4;1,1.4,1;1,0.7,1;1,1,0.7;1,1.4,1.4".into())
                .split(';')
                .map(|v| {
                    let m: Vec<f32> = v.split(',').map(|x| x.parse().unwrap()).collect();
                    (format!("m{}_{}_{}", m[0], m[1], m[2]), [m[0], m[1], m[2]])
                })
                .collect();
            // IMIJ_QS overrides the imij quality grid (comma-separated).
            let qs: Vec<f32> = std::env::var("IMIJ_QS")
                .unwrap_or("60,70,80,85,90,93,95,97,98,99,100".into())
                .split(',')
                .map(|v| v.parse().unwrap())
                .collect();
            let base_matrix: u8 = std::env::var("IMIJ_QMUL_MATRIX")
                .ok()
                .map(|v| v.parse().unwrap())
                .unwrap_or(0);
            let jq = [
                50.0f32, 60.0, 70.0, 75.0, 80.0, 85.0, 88.0, 90.0, 92.0, 94.0, 95.0, 96.0, 97.0,
                98.0, 99.0, 100.0,
            ];
            let pts: Vec<lossy::Point> = imgs
                .par_iter()
                .enumerate()
                .flat_map(|(ii, img)| {
                    let refslice = fast_ssim2::PixelSlice::new(
                        &img.pixels,
                        img.info.width,
                        img.info.height,
                        img.info.width as usize * 3,
                        fast_ssim2::PixelDescriptor::RGB8_SRGB,
                    )
                    .unwrap();
                    let reference = fast_ssim2::Ssimulacra2Reference::new(&refslice).unwrap();
                    let npx = img.pixels_count() as f64;
                    let mut pts = Vec::new();
                    let mut add = |variant: &str, setting: String, n: usize, dec: &[u8]| {
                        let ba = lossy::butteraugli_scores(img, dec);
                        pts.push(lossy::Point {
                            img: ii,
                            variant: variant.into(),
                            setting,
                            bpp: n as f64 * 8.0 / npx,
                            ssim2: lossy::ssim2(&reference, img, dec),
                            tok: 0.0,
                            zensim: lossy::zensim_score(img, dec),
                            ba3: ba.0,
                            bamax: ba.1,
                            jod: f64::NAN,
                        });
                    };
                    for &q in &jq {
                        let enc = lossy::jpeg_encode(img, q);
                        add(
                            "zenjpeg444",
                            format!("q{q}"),
                            enc.len(),
                            &lossy::jpeg_decode(&enc),
                        );
                    }
                    for (name, m) in &variants {
                        for &q in qs.iter() {
                            let mut c = imij::ImiEncoderConfig::lossy(q);
                            c = c.tune_lossy(|t| { t.channel_mul = Some(*m); });
                            c = c.tune_lossy(|t| { t.matrix = base_matrix; });
                            let enc = lossy::imij_lossy_cfg(img, &c);
                            let (_, dec) = imij::decode(&enc).unwrap();
                            add(name, format!("q{q}"), enc.len(), &dec);
                        }
                    }
                    pts
                })
                .collect();
            print!("{}", lossy::metric_tables(&pts, imgs.len(), "zenjpeg444"));
            return;
        }
        "aqsweep" => {
            // Real-codec sweep of XYB channel multipliers: size vs zenjpeg at
            // equal SSIMULACRA2 and zensim.
            let variants: Vec<(String, [f32; 3])> = std::env::var("IMIJ_AQ")
                .unwrap_or("0,1;0.2,1;0.35,1;0.5,1;0.35,3;0.5,3".into())
                .split(';')
                .map(|v| {
                    let m: Vec<f32> = v.split(',').map(|x| x.parse().unwrap()).collect();
                    (format!("aq_g{}_r{}", m[0], m[1]), [m[0], m[1], 0.0])
                })
                .collect();
            let qs = [
                60.0f32, 70.0, 80.0, 85.0, 90.0, 93.0, 95.0, 97.0, 98.0, 99.0, 100.0,
            ];
            let jq = [
                50.0f32, 60.0, 70.0, 75.0, 80.0, 85.0, 88.0, 90.0, 92.0, 94.0, 95.0, 96.0, 97.0,
                98.0, 99.0, 100.0,
            ];
            let pts: Vec<lossy::Point> = imgs
                .par_iter()
                .enumerate()
                .flat_map(|(ii, img)| {
                    let refslice = fast_ssim2::PixelSlice::new(
                        &img.pixels,
                        img.info.width,
                        img.info.height,
                        img.info.width as usize * 3,
                        fast_ssim2::PixelDescriptor::RGB8_SRGB,
                    )
                    .unwrap();
                    let reference = fast_ssim2::Ssimulacra2Reference::new(&refslice).unwrap();
                    let npx = img.pixels_count() as f64;
                    let mut pts = Vec::new();
                    let mut add = |variant: &str, setting: String, n: usize, dec: &[u8]| {
                        let ba = lossy::butteraugli_scores(img, dec);
                        pts.push(lossy::Point {
                            img: ii,
                            variant: variant.into(),
                            setting,
                            bpp: n as f64 * 8.0 / npx,
                            ssim2: lossy::ssim2(&reference, img, dec),
                            tok: 0.0,
                            zensim: lossy::zensim_score(img, dec),
                            ba3: ba.0,
                            bamax: ba.1,
                            jod: f64::NAN,
                        });
                    };
                    for &q in &jq {
                        let enc = lossy::jpeg_encode(img, q);
                        add(
                            "zenjpeg444",
                            format!("q{q}"),
                            enc.len(),
                            &lossy::jpeg_decode(&enc),
                        );
                    }
                    for (name, m) in &variants {
                        for &q in &qs {
                            let mut c = imij::ImiEncoderConfig::lossy(q);
                            c = c.tune_lossy(|t| { t.aq_gamma = m[0]; });
                            c = c.tune_lossy(|t| { t.aq_r0 = m[1]; });
                            let enc = lossy::imij_lossy_cfg(img, &c);
                            let (_, dec) = imij::decode(&enc).unwrap();
                            add(name, format!("q{q}"), enc.len(), &dec);
                        }
                    }
                    pts
                })
                .collect();
            print!("{}", lossy::metric_tables(&pts, imgs.len(), "zenjpeg444"));
            return;
        }
        "tables" => {
            for q in [80.0f32, 90.0, 95.0] {
                let mut line = format!("q{q}");
                for nt in [8usize, 12, 16, 24] {
                    let bytes: usize = imgs
                        .par_iter()
                        .map(|im| {
                            let mut c = imij::ImiEncoderConfig::lossy(q);
                            c = c.tune_lossy(|t| { t.max_ac_tables = nt; });
                            c.encode_samples(imij::ImageRef::new(&im.pixels, im.info.width as usize * 3, &im.info), &imij::Metadata::default())
                            .unwrap()
                            .len()
                        })
                        .sum();
                    let px: usize = imgs.iter().map(|i| i.pixels_count()).sum();
                    line += &format!("  ac{nt}: {:.4} bpp", bytes as f64 * 8.0 / px as f64);
                }
                println!("{line}");
            }
            return;
        }
        "stats" => {
            for (name, color) in [
                ("ycocg", lossy::Color::YCoCgR),
                ("xyb8", lossy::Color::Xyb { x_gain: 8.0 }),
            ] {
                for q in [80u32, 90, 95, 98, 100] {
                    let mut c = mk(Xform::Float, true, Quant::jpeg(q, 2.0));
                    c = c.tune_lossy(|t| { t.color = color; });
                    let r: Vec<_> = imgs.par_iter().map(|im| lossy::run(im, &c)).collect();
                    let n = r.len() as f64;
                    let mut line = format!(
                        "{name:<6} q{q:<3} tok/px {:.2}",
                        r.iter().map(|x| x.tokens_per_px).sum::<f64>() / n
                    );
                    for ch in 0..3 {
                        line += &format!(
                            "  ch{ch} eob {:5.1} nz {:5.1}",
                            r.iter().map(|x| x.eob_nz[ch].0).sum::<f64>() / n,
                            r.iter().map(|x| x.eob_nz[ch].1).sum::<f64>() / n
                        );
                    }
                    println!("{line}");
                }
            }
            return;
        }
        _ => panic!("unknown experiment {which}"),
    }
    let t0 = std::time::Instant::now();
    let jpeg_q: Vec<f32> = vec![
        70.0, 75.0, 80.0, 85.0, 88.0, 90.0, 92.0, 94.0, 95.0, 96.0, 97.0, 98.0, 99.0, 100.0,
    ];
    let pts = lossy::sweep(&imgs, &variants, &jpeg_q);
    eprintln!("sweep {:.1}s", t0.elapsed().as_secs_f64());
    lossy_report(&imgs, &pts, which);
}

fn lossy_report(imgs: &[Img], pts: &[lossy::Point], which: &str) {
    // raw points
    let mut raw =
        String::from("img\tvariant\tsetting\tbpp\tssim2\tzensim\tba3\tbamax\ttok_per_px\n");
    for p in pts {
        raw += &format!(
            "{}\t{}\t{}\t{:.4}\t{:.3}\t{:.3}\t{:.4}\t{:.4}\t{:.3}\n",
            imgs[p.img].name, p.variant, p.setting, p.bpp, p.ssim2, p.zensim, p.ba3, p.bamax, p.tok
        );
    }
    let out = format!(
        "{}/lossy_{which}.tsv",
        std::env::var("IMIJ_OUT").unwrap_or(".".into())
    );
    std::fs::write(&out, raw).unwrap();
    // mean per variant/setting
    let mut agg: BTreeMap<(String, String), (f64, f64, f64, f64, usize)> = BTreeMap::new();
    for p in pts {
        let e = agg
            .entry((p.variant.clone(), p.setting.clone()))
            .or_default();
        e.0 += p.bpp;
        e.1 += p.ssim2;
        e.2 += p.zensim;
        e.3 += p.tok;
        e.4 += 1;
    }
    println!(
        "{:<18} {:>6} {:>7} {:>7} {:>7} {:>6}",
        "variant", "set", "bpp", "ssim2", "zensim", "tok/px"
    );
    for ((v, s), (b, q, ps, t, n)) in &agg {
        let n = *n as f64;
        println!(
            "{v:<18} {s:>6} {:>7.3} {:>7.2} {:>7.2} {:>6.2}",
            b / n,
            q / n,
            ps / n,
            t / n
        );
    }
    println!();
    print!("{}", lossy::metric_tables(pts, imgs.len(), "zenjpeg444"));
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("entropy") => cmd_entropy(&args[2], args.get(4).map(|s| s.as_str()), &args[3]),
        Some("labprof") => cmd_labprof(
            &args[2],
            &args[3],
            args.get(4).map(|v| v.parse().unwrap()).unwrap_or(20),
        ),
        Some("scaled") => cmd_scaled(&args[2], args.get(3).map(|s| s.as_str())),
        Some("ransprof") => cmd_ransprof(
            &args[2],
            args.get(3).map(|v| v.parse().unwrap()).unwrap_or(20),
        ),
        Some("rice") => cmd_rice(&args[2], args.get(3).map(|s| s.as_str())),
        Some("v1") => cmd_v1(
            &args[2],
            args.get(3).map(|s| s.as_str()),
            args.get(4).map(|v| v.parse().unwrap()).unwrap_or(5),
        ),
        Some("tiny") => cmd_tiny(&args[2], args.get(3).map(|s| s.as_str())),
        Some("dump") => cmd_dump(&args[2], &args[3], &args[4]),
        Some("lossyenc16pq") => {
            // 16-bit PNG input tagged PQ, encoded lossy (XYB through the PQ transfer).
            let mut img = corpus::load_png("x", Path::new(&args[2])).unwrap();
            img.info.cicp = imij::Cicp::BT2100_PQ;
            let q: f32 = args[3].parse().unwrap();
            std::fs::write(
                &args[4],
                lossy::imij_lossy_cfg(&img, &imij::ImiEncoderConfig::lossy(q)),
            )
            .unwrap();
        }
        Some("jpegenc") => {
            // Baseline zenjpeg with restart markers: FILE Q 444|420 OUT
            let img = corpus::load_png("x", Path::new(&args[2])).unwrap();
            let q: f32 = args[3].parse().unwrap();
            std::fs::write(&args[5], lossy::jpeg_encode_sub(&img, q, args[4] == "420")).unwrap();
        }
        Some("lossyenc") => {
            let img = corpus::load_png("x", Path::new(&args[2])).unwrap();
            let q: f32 = args[3].parse().unwrap();
            let color = if args.get(5).map(|s| s.as_str()) == Some("ycocg") {
                imij::LossyColor::YCoCg
            } else {
                imij::LossyColor::Xyb
            };
            imij::lab::lossy_token_stats();
            let enc = lossy::imij_lossy(&img, q, color);
            let [tok, slots] = imij::lab::lossy_token_stats();
            let [st, sg, sb] = imij::lab::lossy_side_stats();
            eprintln!("side_stats tokens {st} groups {sg} coded_bc {sb}");
            let px = img.pixels_count() as f64;
            eprintln!(
                "{:.3} bpp, {:.3} tokens/px, lane utilization {:.1}%",
                enc.len() as f64 * 8.0 / px,
                tok as f64 / px,
                tok as f64 * 100.0 / slots.max(1) as f64
            );
            std::fs::write(&args[4], enc).unwrap();
        }
        Some("palstat") => {
            // Palette potential: palstat MANIFEST — global colors and the
            // share of 64-row stripes with <= 256 colors (RGB8 images).
            for (class, path) in corpus::load_manifest(&args[2], None) {
                let Ok(img) = corpus::load_png(&class, Path::new(&path)) else {
                    continue;
                };
                if img.info.channels != Channels::Rgb || img.info.sample != SampleType::U8 {
                    continue;
                }
                let (w, h) = (img.info.width as usize, img.info.height as usize);
                let key = |i: usize| {
                    let p = &img.pixels[i * 3..i * 3 + 3];
                    (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32
                };
                let mut all = std::collections::HashSet::new();
                let (mut ok, mut n) = (0, 0);
                let mut worst = 0;
                for y0 in (0..h).step_by(64) {
                    let mut set = std::collections::HashSet::new();
                    for i in y0 * w..(y0 + 64).min(h) * w {
                        set.insert(key(i));
                        all.insert(key(i));
                    }
                    n += 1;
                    worst = worst.max(set.len());
                    if set.len() <= 256 {
                        ok += 1;
                    }
                }
                println!(
                    "{class}\t{}\t{w}x{h}\tcolors {}\tstripes<=256 {ok}/{n}\tmax/stripe {worst}",
                    img.name,
                    all.len()
                );
            }
        }
        Some("palab") => {
            // palab MANIFEST [FILTER]: lossless bytes, palette stripes and
            // 1-thread decode Mpx/s (best of 7) with palette stripes off/on;
            // IMIJ_PALAB_OUT=DIR also writes DIR/{off,on}/NAME.imi.
            println!(
                "class\tname\tpx\tbytes_off\tbytes_on\tpal_stripes\tstripes\tmpxs_off\tmpxs_on"
            );
            for (class, path) in corpus::load_manifest(&args[2], args.get(3).map(|s| s.as_str())) {
                let Ok(img) = corpus::load_png(&class, Path::new(&path)) else {
                    continue;
                };
                let px = img.info.width as f64 * img.info.height as f64;
                let mut row = format!("{class}\t{}\t{px}", img.name);
                let mut cols = Vec::new();
                for palette in [false, true] {
                    let cfg = imij::ImiEncoderConfig::default().with_threads(8).tune_lossless(|t| { t.palette = palette; });
                    let enc = cfg.encode_samples(imij::ImageRef::new(&img.pixels, img.info.row_bytes(), &img.info), &Metadata::default())
                    .unwrap();
                    let mut out = vec![0u8; img.pixels.len()];
                    let mut best = f64::MAX;
                    for _ in 0..7 {
                        let t = std::time::Instant::now();
                        imij::decode_into_threads(&enc, &mut out, img.info.row_bytes(), 1).unwrap();
                        best = best.min(t.elapsed().as_secs_f64());
                    }
                    assert!(out == img.pixels, "{}: mismatch", img.name);
                    let (ps, ns) = imij::lab::palette_stripes(&enc).unwrap();
                    cols.push((enc.len(), ps, ns, px / best / 1e6));
                    if let Ok(dir) = std::env::var("IMIJ_PALAB_OUT") {
                        // Files of images with palette stripes, for timing elsewhere.
                        let sub = if palette { "on" } else { "off" };
                        std::fs::create_dir_all(format!("{dir}/{sub}")).unwrap();
                        std::fs::write(format!("{dir}/{sub}/{}.imi", img.name), &enc).unwrap();
                    }
                }
                row += &format!(
                    "\t{}\t{}\t{}\t{}\t{:.1}\t{:.1}",
                    cols[0].0, cols[1].0, cols[1].1, cols[1].2, cols[0].3, cols[1].3
                );
                println!("{row}");
            }
        }
        Some("palenc") => {
            // palenc PNG OUT.imi: the image as a luminance-ordered index plane
            // (global palette, <= 256 colors) coded as lossless Gray8; prints
            // index-plane bytes and palette bytes.
            let img = corpus::load_png("x", Path::new(&args[2])).unwrap();
            let (w, h) = (img.info.width as usize, img.info.height as usize);
            let mut cols: Vec<[u8; 3]> = img.pixels.as_chunks::<3>().0.to_vec();
            cols.sort_unstable();
            cols.dedup();
            assert!(cols.len() <= 256, "{} colors", cols.len());
            cols.sort_by_key(|c| 299 * c[0] as u32 + 587 * c[1] as u32 + 114 * c[2] as u32);
            let idx: std::collections::HashMap<[u8; 3], u8> = cols
                .iter()
                .enumerate()
                .map(|(i, &c)| (c, i as u8))
                .collect();
            let plane: Vec<u8> = img
                .pixels
                .as_chunks::<3>()
                .0
                .iter()
                .map(|c| idx[c])
                .collect();
            let info = imij::ImageInfo::new(w as u32, h as u32, Channels::Gray, SampleType::U8);
            let enc = imij::ImiEncoderConfig::default().with_threads(8).encode_samples(imij::ImageRef::new(&plane, w, &info), &Metadata::default())
            .unwrap();
            std::fs::write(&args[3], &enc).unwrap();
            println!("{}\t{}\t{}", args[2], enc.len(), cols.len() * 3);
        }
        Some("anim") => {
            // anim OUTDIR: animation sequences built from corpus images, each
            // encoded lossless / lossy q85 with and without inter coding
            // (OUTDIR/SEQ_{ll,ly}_{intra,inter}.imi); prints bytes and encode
            // time.
            let dir = &args[2];
            std::fs::create_dir_all(dir).unwrap();
            let rgb = |path: &str| -> (usize, usize, Vec<u8>) {
                let img = corpus::load_png("x", Path::new(path)).unwrap();
                let n = img.info.channels.count();
                assert!(img.info.sample == SampleType::U8 && n >= 3);
                let px: Vec<u8> = img
                    .pixels
                    .chunks_exact(n)
                    .flat_map(|p| {
                        if n == 4 {
                            // Over white.
                            let a = p[3] as u32;
                            [0, 1, 2]
                                .map(|c| ((p[c] as u32 * a + 255 * (255 - a) + 127) / 255) as u8)
                        } else {
                            [p[0], p[1], p[2]]
                        }
                    })
                    .collect();
                (img.info.width as usize, img.info.height as usize, px)
            };
            let crop = |src: &(usize, usize, Vec<u8>),
                        x0: usize,
                        y0: usize,
                        w: usize,
                        h: usize|
             -> Vec<u8> {
                let mut v = Vec::with_capacity(w * h * 3);
                for y in y0..y0 + h {
                    v.extend_from_slice(&src.2[(y * src.0 + x0) * 3..(y * src.0 + x0 + w) * 3]);
                }
                v
            };
            let paste = |dst: &mut [u8],
                         dw: usize,
                         src: &[u8],
                         sw: usize,
                         sh: usize,
                         x0: usize,
                         y0: usize| {
                for y in 0..sh {
                    dst[((y0 + y) * dw + x0) * 3..((y0 + y) * dw + x0 + sw) * 3]
                        .copy_from_slice(&src[y * sw * 3..(y + 1) * sw * 3]);
                }
            };
            let photo = rgb("local-scratch/llb-data/2048/png-high/1407.png");
            let clip = rgb(
                "imazen-26/9000-lilith-ai-clipart/9007_gen_clipart_boba-tea-cup_1024x1024.png",
            );
            let shot = rgb(
                "imazen-26/8100-lilith-web-screenshots/1440x900/8100_web-screenshots_archive-wayback-search_dpr1_page1_1440x900.png",
            );
            let shot2 = rgb(
                "imazen-26/8100-lilith-web-screenshots/1440x900/8130_web-screenshots_nih-recipes-keep-the-beat_dpr1_page1_1440x900.png",
            );
            let tall = rgb(
                "imazen-26/8000-lilith-mobile-screenshots/8004_mobile-screenshots_mobile-app-dashboard_screenshot-20260526-065758-github_1080x2520.png",
            );
            let photos: Vec<_> = ["2000", "2005", "2011", "2014", "1407"]
                .iter()
                .map(|n| rgb(&format!("local-scratch/llb-data/2048/png-high/{n}.png")))
                .collect();
            let mut seqs: Vec<(&str, usize, usize, Vec<Vec<u8>>)> = Vec::new();
            {
                // A 160x160 sprite (clip-art crop) crossing a 1024x768 photo.
                let (w, h) = (1024, 768);
                let bg = crop(&photo, 400, 300, w, h);
                let sprite = crop(&clip, 432, 432, 160, 160);
                let f = (0..30)
                    .map(|t| {
                        let mut c = bg.clone();
                        paste(
                            &mut c,
                            w,
                            &sprite,
                            160,
                            160,
                            20 + t * 27,
                            100 + (t * 13) % 400,
                        );
                        c
                    })
                    .collect();
                seqs.push(("sprite", w, h, f));
            }
            {
                // Typing: rows of another screenshot revealed 16 px per frame
                // into a 640x40 box, plus a blinking 2x18 cursor.
                let (w, h) = (1440, 900);
                let text = crop(&shot2, 100, 200, 640, 40);
                let f = (0..30)
                    .map(|t| {
                        let mut c = shot.2.clone();
                        let n = (t * 16 + 16).min(640);
                        for y in 0..40 {
                            let o = ((420 + y) * w + 300) * 3;
                            c[o..o + n * 3].copy_from_slice(&text[y * 640 * 3..(y * 640 + n) * 3]);
                        }
                        if t % 2 == 0 {
                            for y in 0..18 {
                                for x in 0..2 {
                                    let o = ((431 + y) * w + 300 + n + x) * 3;
                                    c[o..o + 3].copy_from_slice(&[0, 0, 0]);
                                }
                            }
                        }
                        c
                    })
                    .collect();
                seqs.push(("typing", w, h, f));
            }
            {
                // Crossfade of a 400x300 region between two photo crops.
                let (w, h) = (1440, 900);
                let (a, b) = (
                    crop(&photos[0], 0, 0, 400, 300),
                    crop(&photos[1], 0, 0, 400, 300),
                );
                let f = (0..20)
                    .map(|t| {
                        let mut c = shot.2.clone();
                        let m: Vec<u8> = a
                            .iter()
                            .zip(&b)
                            .map(|(&p, &q)| ((p as u32 * (19 - t) + q as u32 * t + 9) / 19) as u8)
                            .collect();
                        paste(&mut c, w, &m, 400, 300, 900, 400);
                        c
                    })
                    .collect();
                seqs.push(("crossfade", w, h, f));
            }
            {
                // Scrolling a 1080x2520 screenshot through a 1080x1200 view.
                let f = (0..30)
                    .map(|t| crop(&tall, 0, t as usize * 12, 1080, 1200))
                    .collect();
                seqs.push(("scroll", 1080, 1200, f));
            }
            {
                // Slideshow: 5 photos, 6 frames each, hard cuts.
                let f = (0..30)
                    .map(|t| crop(&photos[t / 6], 0, 0, 1024, 768))
                    .collect();
                seqs.push(("slideshow", 1024, 768, f));
            }
            println!("seq\tcanvas\tframes\tmode\tbytes\tencode_ms");
            for (name, w, h, frames) in &seqs {
                let info =
                    imij::ImageInfo::new(*w as u32, *h as u32, Channels::Rgb, SampleType::U8);
                for (cname, coding) in [
                    (
                        "ll",
                        imij::ImiEncoderConfig::default().with_threads(8),
                    ),
                    (
                        "ly",
                        imij::ImiEncoderConfig::lossy(85.0).with_threads(8),
                    ),
                ] {
                    for inter in [false, true] {
                        let mut cfg = imij::FrameConfig::new(coding.clone());
                        cfg.inter = inter;
                        let t0 = std::time::Instant::now();
                        let mut e =
                            imij::FrameEncoder::animation(info.clone(), Metadata::default(), cfg);
                        for f in frames {
                            e.push_frame(f, w * 3, imij::FrameDuration::from_millis(40))
                                .unwrap();
                        }
                        let file = e.finish().unwrap();
                        let ms = t0.elapsed().as_secs_f64() * 1e3;
                        let mode = if inter { "inter" } else { "intra" };
                        std::fs::write(format!("{dir}/{name}_{cname}_{mode}.imi"), &file).unwrap();
                        println!(
                            "{name}\t{w}x{h}\t{}\t{cname}_{mode}\t{}\t{ms:.0}",
                            frames.len(),
                            file.len()
                        );
                    }
                }
            }
        }
        Some("mosaic") => {
            // mosaic OUTPREFIX ROWS PNG...: RGB8 images side by side (first
            // ROWS rows of each; all must have them), written as
            // OUTPREFIX_ly.imi (lossy default) and OUTPREFIX_ll_gG.imi per
            // column group width in IMIJ_GROUPS (default 0 = none).
            let rows: usize = args[3].parse().unwrap();
            let imgs: Vec<_> = args[4..]
                .iter()
                .map(|f| corpus::load_png("x", Path::new(f)).unwrap())
                .collect();
            let w: usize = imgs.iter().map(|i| i.info.width as usize).sum();
            let mut px = Vec::with_capacity(w * rows * 3);
            for y in 0..rows {
                for i in &imgs {
                    assert!(i.info.channels == Channels::Rgb && i.info.sample == SampleType::U8);
                    let rb = i.info.row_bytes();
                    px.extend_from_slice(&i.pixels[y * rb..(y + 1) * rb]);
                }
            }
            let info = imij::ImageInfo::new(w as u32, rows as u32, Channels::Rgb, SampleType::U8);
            // IMIJ_LYGROUPS=G,...: lossy with column groups of G pixels
            // (0 = none), OUTPREFIX_ly_gG.imi.
            if let Ok(list) = std::env::var("IMIJ_LYGROUPS") {
                for g in list.split(',') {
                    let g: u32 = g.parse().unwrap();
                    let mut cfg = imij::ImiEncoderConfig::lossy(85.0).with_threads(8);
                    cfg = cfg.with_column_groups(Some(g));
                    let t = std::time::Instant::now();
                    let e =
                        cfg.encode_samples(imij::ImageRef::new(&px, w * 3, &info), &Metadata::default()).unwrap();
                    let ms = t.elapsed().as_secs_f64() * 1e3;
                    std::fs::write(format!("{}_ly_g{g}.imi", args[2]), &e).unwrap();
                    println!(
                        "{w}x{rows}: lossy groups {g}: {} B, encode {ms:.0} ms",
                        e.len()
                    );
                }
            }
            // IMIJ_STRIPS=G,...: also the lossy image cut into G-column strips,
            // each a complete file (OUTPREFIX_ly_sG_NNN.imi): independent
            // column groups simulated (decspeed --as-one).
            if let Ok(list) = std::env::var("IMIJ_STRIPS") {
                for g in list.split(',') {
                    let g: usize = g.parse().unwrap();
                    let mut total = 0;
                    // IMIJ_TILE_ROWS=H: strips also cut into H-row tiles.
                    let th: usize =
                        std::env::var("IMIJ_TILE_ROWS").map_or(rows, |v| v.parse().unwrap());
                    let tiles: Vec<(usize, usize)> = (0..rows)
                        .step_by(th)
                        .flat_map(|y0| (0..w).step_by(g).map(move |x0| (x0, y0)))
                        .collect();
                    for (i, (x0, y0)) in tiles.into_iter().enumerate() {
                        let gw = g.min(w - x0);
                        let tr = th.min(rows - y0);
                        let mut sp = Vec::with_capacity(gw * tr * 3);
                        for y in y0..y0 + tr {
                            sp.extend_from_slice(&px[(y * w + x0) * 3..(y * w + x0 + gw) * 3]);
                        }
                        let si = imij::ImageInfo::new(
                            gw as u32,
                            tr as u32,
                            Channels::Rgb,
                            SampleType::U8,
                        );
                        let e = imij::ImiEncoderConfig::lossy(85.0).with_threads(8).encode_samples(imij::ImageRef::new(&sp, gw * 3, &si), &Metadata::default())
                        .unwrap();
                        total += e.len();
                        std::fs::write(format!("{}_ly_s{g}_{i:03}.imi", args[2]), &e).unwrap();
                    }
                    println!("{w}x{rows}: lossy strips {g}: {total} B");
                }
            }
            let ly = imij::ImiEncoderConfig::lossy(85.0).with_threads(8).encode_samples(imij::ImageRef::new(&px, w * 3, &info), &Metadata::default())
            .unwrap();
            std::fs::write(format!("{}_ly.imi", args[2]), &ly).unwrap();
            println!("{w}x{rows}: lossy {} B", ly.len());
            for g in std::env::var("IMIJ_GROUPS")
                .unwrap_or("0".into())
                .split(',')
            {
                let g: u32 = g.parse().unwrap();
                let t = std::time::Instant::now();
                let ll = imij::ImiEncoderConfig::default().with_threads(8).with_column_groups(Some(g)).encode_samples(imij::ImageRef::new(&px, w * 3, &info), &Metadata::default())
                .unwrap();
                let ms = t.elapsed().as_secs_f64() * 1e3;
                std::fs::write(format!("{}_ll_g{g}.imi", args[2]), &ll).unwrap();
                println!(
                    "{w}x{rows} groups {g}: {} B, encode {ms:.0} ms (8 threads)",
                    ll.len()
                );
            }
        }
        Some("lfpred") => {
            // lfpred MANIFEST [FILTER]: lossy bytes with low-frequency AC
            // prediction from neighbouring DCs (lab, encoder-only; identical
            // pixels by construction) for modes 0 / 1 / 3 / 5 / 7, q 30..95.
            let modes: Vec<u8> = std::env::var("IMIJ_LFMODES")
                .unwrap_or("0,1,3,5,7".into())
                .split(',')
                .map(|v| v.parse().unwrap())
                .collect();
            let head: Vec<String> = modes.iter().map(|m| format!("m{m}")).collect();
            println!("class\tname\tq\t{}", head.join("\t"));
            for (class, path) in corpus::load_manifest(&args[2], args.get(3).map(|s| s.as_str())) {
                let Ok(img) = corpus::load_png(&class, Path::new(&path)) else {
                    continue;
                };
                if img.info.channels != Channels::Rgb || img.info.sample != SampleType::U8 {
                    continue;
                }
                for q in [30.0f32, 50.0, 70.0, 85.0, 95.0] {
                    let mut row = format!("{class}\t{}\t{q}", img.name);
                    for &m in &modes {
                        let mut cfg = imij::ImiEncoderConfig::lossy(q).with_threads(8);
                        cfg = cfg.tune_lossy(|t| { t.lab.lf_pred = m; });
                        let e = cfg.encode_samples(imij::ImageRef::new(&img.pixels, img.info.row_bytes(), &img.info), &Metadata::default())
                        .unwrap();
                        row += &format!("\t{}", e.len());
                    }
                    println!("{row}");
                }
            }
        }
        Some("widegen") => {
            // widegen PNG N OUTPREFIX [ROWS]: the image tiled N times
            // horizontally (first ROWS rows), written as OUTPREFIX_ll.imi
            // (lossless) and OUTPREFIX_ly.imi (lossy, default config).
            let img = corpus::load_png("x", Path::new(&args[2])).unwrap();
            let n: usize = args[3].parse().unwrap();
            let rb = img.info.row_bytes();
            let h = args.get(5).map_or(img.info.height as usize, |r| {
                r.parse::<usize>().unwrap().min(img.info.height as usize)
            });
            let mut px = Vec::with_capacity(rb * n * h);
            for y in 0..h {
                for _ in 0..n {
                    px.extend_from_slice(&img.pixels[y * rb..(y + 1) * rb]);
                }
            }
            let info = imij::ImageInfo::new(
                img.info.width * n as u32,
                h as u32,
                img.info.channels,
                img.info.sample,
            );
            // IMIJ_GROUPS=0,4096,...: lossless only, one file per column
            // group width (OUTPREFIX_ll_gG.imi; 0 = no groups).
            if let Ok(list) = std::env::var("IMIJ_GROUPS") {
                for g in list.split(',') {
                    let g: u32 = g.parse().unwrap();
                    let t = std::time::Instant::now();
                    let ll = imij::ImiEncoderConfig::default().with_threads(8).with_column_groups(Some(g)).encode_samples(imij::ImageRef::new(&px, rb * n, &info), &Metadata::default())
                    .unwrap();
                    let ms = t.elapsed().as_secs_f64() * 1e3;
                    std::fs::write(format!("{}_ll_g{g}.imi", args[4]), &ll).unwrap();
                    println!(
                        "{}x{} groups {g}: {} B, encode {ms:.0} ms (8 threads)",
                        info.width,
                        info.height,
                        ll.len()
                    );
                }
                return;
            }
            let ll = imij::ImiEncoderConfig::default().with_threads(8).encode_samples(imij::ImageRef::new(&px, rb * n, &info), &Metadata::default())
            .unwrap();
            std::fs::write(format!("{}_ll.imi", args[4]), &ll).unwrap();
            let ly = imij::ImiEncoderConfig::lossy(85.0).with_threads(8).encode_samples(imij::ImageRef::new(&px, rb * n, &info), &Metadata::default())
            .unwrap();
            std::fs::write(format!("{}_ly.imi", args[4]), &ly).unwrap();
            println!(
                "{}x{}: lossless {} B, lossy {} B",
                info.width,
                info.height,
                ll.len(),
                ly.len()
            );
        }
        Some("maps") => {
            // Channel maps of lossless files: maps FILE...
            for f in &args[2..] {
                let d = std::fs::read(f).unwrap();
                println!("{f}: {:?}", imij::lab::channel_maps(&d).unwrap());
            }
        }
        Some("llenc") => {
            // Lossless with a given stripe height: llenc FILE ROWS OUT
            // (IMIJ_PALETTE=0: no palette stripes).
            let img = corpus::load_png("x", Path::new(&args[2])).unwrap();
            let enc = imij::ImiEncoderConfig::default().with_threads(8).tune_lossless(|t| { t.stripe_rows = Some(args[3].parse().unwrap()); t.palette = std::env::var("IMIJ_PALETTE").map_or(true, |v| v != "0"); }).encode_samples(imij::ImageRef::new(&img.pixels, img.info.row_bytes(), &img.info), &Metadata::default())
            .unwrap();
            std::fs::write(&args[4], enc).unwrap();
        }
        Some("memdec") => {
            // Decode once for a heap profile: memdec MODE FILE, MODE =
            // imij-stream | imij-full | jpeg-stream | jpeg-full. Streaming
            // modes keep one 16-row output buffer; full modes the image.
            if args[2] == "imij-reader" {
                // From the file through a BufReader: the file is never held whole.
                let f = std::io::BufReader::new(std::fs::File::open(&args[3]).unwrap());
                let mut sum = 0u64;
                imij::decode_reader(f, &imij::Limits::DEFAULT, |_, _, row| {
                    sum += row[0] as u64;
                    Ok(())
                })
                .unwrap();
                std::hint::black_box(sum);
                return;
            }
            let data = std::fs::read(&args[3]).unwrap();
            let mut sum = 0u64;
            match args[2].as_str() {
                "imij-stream" => {
                    let mut sd = imij::StreamDecoder::new(&data).unwrap();
                    let mut row = vec![0u8; sd.info().row_bytes()];
                    while sd.next_row(&mut row).unwrap() {
                        sum += row[0] as u64;
                    }
                }
                "imij-full" => {
                    let (_, px) = imij::decode(&data).unwrap();
                    sum = px[0] as u64;
                }
                "jpeg-stream" => {
                    let mut r = zenjpeg::decoder::Decoder::new()
                        .scanline_reader(&data)
                        .unwrap();
                    let w = r.width() as usize;
                    let mut buf = vec![0u8; w * 3 * 16];
                    while !r.is_finished() {
                        let out = imgref::ImgRefMut::new(&mut buf[..], w * 3, 16);
                        let n = r.read_rows_rgb8(out).unwrap();
                        sum += buf[0] as u64 + n as u64;
                    }
                }
                "jpeg-full" => sum = lossy::jpeg_decode(&data)[0] as u64,
                m => panic!("mode {m}"),
            }
            std::hint::black_box(sum);
        }
        Some("streamrows") => {
            // Decode a file row by row (or fully with "full"), report time.
            let data = std::fs::read(&args[2]).unwrap();
            let t = std::time::Instant::now();
            let mut sum = 0u64;
            if args.get(3).map(|s| s.as_str()) == Some("full") {
                let (_, px) = imij::decode(&data).unwrap();
                sum = px.iter().map(|&b| b as u64).sum();
            } else {
                let mut sd = imij::StreamDecoder::new(&data).unwrap();
                let mut row = vec![0u8; sd.info().row_bytes()];
                while sd.next_row(&mut row).unwrap() {
                    sum += row[0] as u64;
                }
            }
            println!("{:.1} ms (checksum {sum})", t.elapsed().as_secs_f64() * 1e3);
        }
        Some("encprof") => {
            let img = corpus::load_png("x", Path::new(&args[2])).unwrap();
            let threads: usize = args.get(3).map(|v| v.parse().unwrap()).unwrap_or(1);
            let t = std::time::Instant::now();
            for _ in 0..5 {
                std::hint::black_box(lossy::imij_lossy_cfg(
                    &img,
                    &imij::ImiEncoderConfig::lossy(90.0).with_threads(threads),
                ));
            }
            println!(
                "{:.1} Mpx/s",
                img.pixels_count() as f64 * 5.0 / t.elapsed().as_secs_f64() / 1e6
            );
        }
        Some("decprof") => cmd_decprof(
            &args[2],
            args.get(3).map(|v| v.parse().unwrap()).unwrap_or(10),
            args.get(4).map(|v| v.parse().unwrap()).unwrap_or(1),
        ),
        Some("encspeed") => cmd_encspeed(&args[2], args.get(3).map(|s| s.as_str())),
        Some("pyramid") => cmd_pyramid(&args[2], args.get(3).map(|s| s.as_str())),
        Some("squeeze") => cmd_squeeze(&args[2], args.get(3).map(|s| s.as_str())),
        Some("serveprep") => cmd_serveprep(&args[2], &args[3]),
        Some("serve") => cmd_serve(
            &args[2],
            &args[3],
            args.get(4).map(|v| v.parse().unwrap()).unwrap_or(10),
        ),
        Some("util") => cmd_util(&args[2], args.get(3).map(|s| s.as_str())),
        Some("cost") => cmd_cost(&args[2], args.get(4).map(|s| s.as_str()), &args[3]),
        Some("lossy") => cmd_lossy(&args[2], args.get(4).map(|s| s.as_str()), &args[3]),
        Some("sizes") => cmd_sizes(&args[2], args.get(4).map(|s| s.as_str()), &args[3]),
        _ => eprintln!("usage: imij-bench sizes <manifest.tsv> <out.tsv> [class,class]"),
    }
}
