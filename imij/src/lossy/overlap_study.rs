//! Experimental driver, compiled only into lab unit-test executables.
//! See docs/EVAL_OVERLAP7.md. No production syntax or public API additions.
use super::*;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::Path;
mod framing;
mod ghost;
mod memory;
mod optimized;
mod ordinary;
mod packer;
mod phase;
#[path = "../../../imij-bench/src/postfilter.rs"]
mod postfilter;
mod pruned;
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
mod pruned_v4;
mod rate;
mod rdo;

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn store(cas: &Path, bytes: &[u8], extension: &str) -> (String, String) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
    let hash = sha256(bytes);
    let directory = cas.join(&hash[..2]);
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("{hash}.{extension}"));
    if !path.exists() {
        let stage = directory.join(format!(
            ".{hash}.{}.{}.stage",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut f = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&stage)
            .unwrap();
        f.write_all(bytes).unwrap();
        drop(f);
        match std::fs::hard_link(&stage, &path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => panic!("CAS publication: {e}"),
        }
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "CAS content must be complete and identical"
        );
        std::fs::remove_file(stage).unwrap();
    } else {
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "CAS digest collision or altered artifact"
        );
    }
    (path.to_string_lossy().into_owned(), hash)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Base,
    Own,
    Blend,
}
impl Mode {
    fn step(self) -> usize {
        if self == Self::Base { 8 } else { 7 }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Own => "own",
            Self::Blend => "blend",
        }
    }
}

fn patches(src: &[u8], w: usize, h: usize, stride: usize, mode: Mode) -> (ImageInfo, Vec<u8>) {
    assert!(w > 0 && h > 0 && stride >= w * 3 && src.len() >= (h - 1) * stride + w * 3);
    if mode == Mode::Base {
        let mut info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8);
        info.cicp = crate::Cicp::SRGB;
        let mut packed = vec![0; w * h * 3];
        for y in 0..h {
            packed[y * w * 3..][..w * 3].copy_from_slice(&src[y * stride..][..w * 3]);
        }
        return (info, packed);
    }
    let step = mode.step();
    let (bw, bh) = (w.div_ceil(step), h.div_ceil(step));
    let mut out = vec![0; bw * bh * 64 * 3];
    for by in 0..bh {
        for y in 0..8 {
            for bx in 0..bw {
                for x in 0..8 {
                    let a = (by * step + y).min(h - 1) * stride + (bx * step + x).min(w - 1) * 3;
                    let b = ((by * 8 + y) * bw * 8 + bx * 8 + x) * 3;
                    out[b..b + 3].copy_from_slice(&src[a..a + 3]);
                }
            }
        }
    }
    let mut info = ImageInfo::new(
        (bw * 8) as u32,
        (bh * 8) as u32,
        Channels::Rgb,
        SampleType::U8,
    );
    info.cicp = crate::Cicp::SRGB;
    (info, out)
}

fn admitted(p: &LossyParsed<'_>) {
    assert_eq!(p.info.channels, Channels::Rgb, "study carrier is RGB");
    assert_eq!(
        p.info.sample,
        SampleType::U8,
        "study carrier uses U8 storage"
    );
    assert_eq!(
        p.ngroups(),
        1,
        "study decoder currently requires one column group"
    );
    assert_eq!(
        p.map,
        MapKind::None,
        "partitioned transforms need a separate geometry rule"
    );
    assert!(p.exact.is_none() && p.alpha.is_none() && p.transform.is_none());
    assert_eq!(p.layouts, [LossyChannelLayout::default(); 3]);
}

fn dc_all(p: &LossyParsed<'_>) -> Result<(Vec<i32>, Vec<f32>), Error> {
    let d = p.dc.as_ref().unwrap();
    let om = crate::v1::OutMap::new(&d.info, &d.layout);
    let mut px = vec![0; d.info.row_bytes() * d.info.height as usize];
    let rb = d.info.row_bytes();
    for s in 0..d.stripes.len() {
        let mut rows = crate::v1::UnitRows::new(d, s)?;
        let first = s * d.hs;
        for y in first..(first + d.hs).min(d.info.height as usize) {
            rows.next_row(d, &om, &mut px[y * rb..][..rb])?;
        }
    }
    let (mut values, mut scales) = (Vec::new(), Vec::new());
    dc_values(d.info.sample, p.aq, false, &px, &mut values, &mut scales)?;
    Ok((values, scales))
}

/// Deliberately direct, frozen scalar contributor oracle.
fn reference(file: &[u8], w: usize, h: usize, mode: Mode) -> Vec<u8> {
    let p = parse_lossy(file).unwrap();
    admitted(&p);
    let (_, _, q, _, _) = quantized_lab(file).unwrap();
    let (_, aq) = dc_all(&p).unwrap();
    let mut planes = vec![[0.0f32; 3]; w * h];
    let mut counts = vec![0u8; w * h];
    let step = mode.step();
    for by in 0..p.g.bh {
        for bx in 0..p.g.bw {
            let b = by * p.g.bw + bx;
            let mut blocks = [[0.0f32; 64]; 3];
            for ch in 0..3 {
                let dq = aq_dequant(&p.dq[ch], aq.get(b).copied());
                for k in 0..64 {
                    let v = q[(b * 3 + ch) * 64 + k];
                    let sg = v.clamp(-1, 1);
                    let bias = p
                        .bias
                        .as_ref()
                        .map_or(0.0, |t| t[ch][usize::from(v != sg)][k]);
                    blocks[ch][k] = (v as f32 - sg as f32 * bias) * dq[k];
                }
                dct::idct8x8(&mut blocks[ch]);
            }
            let extent = if mode == Mode::Blend { 8 } else { step };
            for y in 0..extent.min(h.saturating_sub(by * step)) {
                for x in 0..extent.min(w.saturating_sub(bx * step)) {
                    let dst = (by * step + y) * w + bx * step + x;
                    for c in 0..3 {
                        planes[dst][c] += blocks[c][y * 8 + x];
                    }
                    counts[dst] += 1;
                }
            }
        }
    }
    let mut out = vec![0; w * h * 3];
    for (i, px) in planes.iter().enumerate() {
        assert!(counts[i] > 0);
        let v = px.map(|v| v / counts[i] as f32);
        out[i * 3..][..3].copy_from_slice(&color::inverse_px(p.color, p.inv_scale, v));
    }
    out
}

/// SIMD IDCT followed by owned output packing. Blend is a fixed gather from
/// at most four block predictions, with precisely the oracle's sum order.
/// Each work item owns disjoint output rows, so there are no concurrent stores
/// to shared pixels. A blend work item decodes a preceding block-row halo.
fn decode(
    file: &[u8],
    w: usize,
    h: usize,
    stride: usize,
    out: &mut [u8],
    mode: Mode,
    threads: usize,
    scalar: bool,
) -> Result<(), Error> {
    assert!(stride >= w * 3 && out.len() >= (h - 1) * stride + w * 3);
    let p = parse_lossy(file)?;
    admitted(&p);
    let (dc, aq) = dc_all(&p)?;
    let step = mode.step();
    let bw = p.g.bw;
    let work_rows = p.g.sb * step;
    let results: Vec<Result<(), Error>> = crate::par::par_map(
        out.chunks_mut(stride * work_rows).enumerate().collect(),
        threads,
        |_, (si, dst)| {
            let first = si * p.g.sb;
            let last = (first + p.g.sb).min(p.g.bh);
            let start = if mode == Mode::Blend {
                first.saturating_sub(1)
            } else {
                first
            };
            let mut sc = Scratch::default();
            let mut previous = vec![0.0f32; 3 * 8 * bw * 8];
            let mut current = vec![0.0f32; 3 * 8 * bw * 8];
            let ps = w.div_ceil(8) * 8;
            let mut packed = vec![0.0f32; 3 * ps];
            let mut loaded = usize::MAX;
            let pw = bw * 8;
            for by in start..last {
                let s = by / p.g.sb;
                if loaded != s {
                    sc.q.fill(0);
                    decode_stripe_coefs(
                        &p,
                        &p.g,
                        s,
                        s,
                        &dc[s * p.g.sb * bw * 3..((s + 1) * p.g.sb).min(p.g.bh) * bw * 3],
                        !scalar,
                        &mut sc,
                    )?;
                    loaded = s;
                }
                let r = by % p.g.sb;
                if scalar {
                    for bx in 0..bw {
                        for ch in 0..3 {
                            let b = r * bw + bx;
                            let factors = aq_dequant(&p.dq[ch], aq.get(by * bw + bx).copied());
                            let mut block = [0.0; 64];
                            for k in 0..64 {
                                let v = sc.q[(b * 3 + ch) * 64 + k];
                                let sg = v.clamp(-1, 1);
                                let bias = p
                                    .bias
                                    .as_ref()
                                    .map_or(0.0, |t| t[ch][usize::from(v != sg)][k]);
                                block[k] = (v as f32 - sg as f32 * bias) * factors[k];
                            }
                            dct::idct8x8(&mut block);
                            for y in 0..8 {
                                current[(ch * 8 + y) * pw + bx * 8..][..8]
                                    .copy_from_slice(&block[y * 8..][..8]);
                            }
                            sc.q[(b * 3 + ch) * 64..][..64].fill(0);
                        }
                    }
                } else {
                    let aqr = if aq.is_empty() {
                        &[][..]
                    } else {
                        &aq[by * bw..][..bw]
                    };
                    simd::recon_block_row(
                        &mut sc.q,
                        &sc.eobs,
                        r,
                        bw,
                        0,
                        bw,
                        &p.dq,
                        aqr,
                        p.bias.as_deref(),
                        &mut current,
                        pw,
                        simd::Small::NONE,
                        true,
                    );
                }
                if by >= first {
                    for y in 0..step.min(h.saturating_sub(by * step)) {
                        for ch in 0..3 {
                            let cur = &current[(ch * 8 + y) * pw..][..pw];
                            let prev = &previous[(ch * 8 + 7) * pw..][..pw];
                            let row = &mut packed[ch * ps..][..w];
                            for bx in 0..bw {
                                let x0 = bx * step;
                                let n = step.min(w.saturating_sub(x0));
                                if mode != Mode::Blend || (y != 0 || by == 0) {
                                    row[x0..x0 + n].copy_from_slice(&cur[bx * 8..bx * 8 + n]);
                                    if mode == Mode::Blend && bx > 0 && n > 0 {
                                        row[x0] = (cur[(bx - 1) * 8 + 7] + cur[bx * 8]) * 0.5;
                                    }
                                } else {
                                    for x in 0..n {
                                        row[x0 + x] = if x == 0 && bx > 0 {
                                            ((prev[(bx - 1) * 8 + 7] + prev[bx * 8])
                                                + cur[(bx - 1) * 8 + 7]
                                                + cur[bx * 8])
                                                * 0.25
                                        } else {
                                            (prev[bx * 8 + x] + cur[bx * 8 + x]) * 0.5
                                        };
                                    }
                                }
                            }
                        }
                        let dy = (by - first) * step + y;
                        let rgb = &mut dst[dy * stride..][..w * 3];
                        if scalar {
                            for x in 0..w {
                                let v = [packed[x], packed[ps + x], packed[2 * ps + x]];
                                rgb[x * 3..][..3].copy_from_slice(&color::inverse_px(
                                    p.color,
                                    p.inv_scale,
                                    v,
                                ));
                            }
                        } else {
                            simd::color_row(
                                p.color,
                                &packed[..ps],
                                &packed[ps..2 * ps],
                                &packed[2 * ps..],
                                p.inv_scale,
                                rgb,
                                w,
                            );
                        }
                    }
                }
                core::mem::swap(&mut previous, &mut current);
            }
            Ok(())
        },
    );
    for r in results {
        r?;
    }
    Ok(())
}

#[test]
fn scalar_and_simd_match_contributor_oracle() {
    let pools = [1, 2, 4, 8].map(|threads| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
    });
    for (w, h, mixed_flat) in [
        (1, 1, false),
        (7, 7, false),
        (8, 8, false),
        (14, 15, false),
        (33, 62, false),
        (127, 73, false),
        (511, 85, false),
        (511, 85, true),
        (8199, 9, true),
    ] {
        let stride = w * 3 + 5;
        let mut src = vec![0xCC; stride * h];
        let mut seed = 17u32;
        for y in 0..h {
            for x in 0..w * 3 {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                src[y * stride + x] = if mixed_flat && (x / 3 >= 224 || y >= 42) {
                    [41, 163, 97][x % 3]
                } else {
                    seed as u8
                };
            }
        }
        for mode in [Mode::Base, Mode::Own, Mode::Blend] {
            for color in [LossyColor::YCoCg, LossyColor::Xyb] {
                for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                    let (info, atlas) = patches(&src, w, h, stride, mode);
                    let mut cfg = LossyConfig::new(50.0)
                        .with_color(color)
                        .with_entropy(entropy)
                        .with_stripe_block_rows(2);
                    cfg.lab.superblocks = Some(false);
                    cfg.column_groups = Some(0);
                    let file =
                        encode_lossy(&atlas, info.row_bytes(), &info, &Metadata::default(), &cfg)
                            .unwrap();
                    let expected = reference(&file, w, h, mode);
                    if mode == Mode::Base {
                        assert_eq!(crate::decode(&file).unwrap().1, expected);
                    }
                    for scalar in [true, false] {
                        for threads in [1usize, 2, 4, 8] {
                            let mut out = vec![0xCC; stride * h];
                            pools[threads.ilog2() as usize]
                                .install(|| {
                                    decode(&file, w, h, stride, &mut out, mode, threads, scalar)
                                })
                                .unwrap();
                            for y in 0..h {
                                assert_eq!(
                                    &out[y * stride..][..w * 3],
                                    &expected[y * w * 3..][..w * 3],
                                    "{mode:?} {color:?} {entropy:?} {w}x{h} scalar={scalar} t={threads} y={y}"
                                );
                                assert!(
                                    out[y * stride + w * 3..(y + 1) * stride]
                                        .iter()
                                        .all(|&v| v == 0xCC)
                                );
                            }
                        }
                    }
                    for chunk in [8, 16, 32, 64] {
                        let mut seen = 0;
                        optimized::decode_stream(&file, w, h, mode, chunk, |y, pixels| {
                            assert_eq!(y, seen);
                            assert_eq!(pixels, &expected[y * w * 3..][..w * 3]);
                            seen += 1;
                            true
                        })
                        .unwrap();
                        assert_eq!(seen, h);
                        let mut first = Vec::new();
                        optimized::decode_stream(&file, w, h, mode, chunk, |y, pixels| {
                            assert_eq!(y, 0);
                            first.extend_from_slice(pixels);
                            false
                        })
                        .unwrap();
                        assert_eq!(first, expected[..w * 3]);
                    }
                    for prune in [false, true] {
                        for chunk in [8, 16, 32, 64] {
                            for threads in [1usize, 2, 4, 8] {
                                let mut out = vec![0xCC; stride * h];
                                pools[threads.ilog2() as usize]
                                    .install(|| {
                                        (if prune {
                                            optimized::decode_pruned
                                        } else {
                                            optimized::decode_chunked
                                        })(
                                            &file, w, h, stride, &mut out, mode, threads, chunk
                                        )
                                    })
                                    .unwrap();
                                for y in 0..h {
                                    assert_eq!(
                                        &out[y * stride..][..w * 3],
                                        &expected[y * w * 3..][..w * 3],
                                        "chunked {chunk} {mode:?} {color:?} {entropy:?} {w}x{h} t{threads} row{y}"
                                    );
                                    assert!(
                                        out[y * stride + w * 3..(y + 1) * stride]
                                            .iter()
                                            .all(|&v| v == 0xCC)
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Explicit study entry point, not a regression test. Regular regression
/// tests above always run; this driver requires a caller-provided manifest.
#[test]
#[ignore = "explicit experiment driver: just overlap7-study MANIFEST OUTPUT"]
fn study_driver() {
    if std::env::var("OVERLAP7_ACTION").as_deref() == Ok("ordinary-controls") {
        ordinary::run();
        return;
    }
    if std::env::var("OVERLAP7_ACTION").as_deref() == Ok("phase-source") {
        phase::run();
        return;
    }
    if std::env::var("OVERLAP7_ACTION").as_deref() == Ok("finalize-carriers") {
        framing::run();
        return;
    }
    if std::env::var("OVERLAP7_ACTION").as_deref() == Ok("memory") {
        memory::run();
        return;
    }
    if std::env::var("OVERLAP7_ACTION").as_deref() == Ok("check-carriers") {
        carrier_check();
        return;
    }
    if std::env::var("OVERLAP7_ACTION").as_deref() == Ok("filter") {
        filter_driver();
        return;
    }
    if matches!(
        std::env::var("OVERLAP7_ACTION").as_deref(),
        Ok("time" | "profile")
    ) {
        timing_driver();
        return;
    }
    let manifest = std::env::var("OVERLAP7_MANIFEST").expect("OVERLAP7_MANIFEST required");
    let output = std::env::var("OVERLAP7_OUTPUT").expect("OVERLAP7_OUTPUT required");
    let output = Path::new(&output);
    std::fs::create_dir_all(output).unwrap();
    let mut results = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(output.join("quality.tsv"))
        .unwrap();
    writeln!(results,"source\twidth\theight\tmode\tcolor\tentropy\tquality\teffort\taq\tbytes\tssim2\tencoded\tdecoded\trdo_rho\trdo_edge\trdo_passes\tseed\tghost_iterations\tencoded_sha256\tdecoded_sha256\tsource_sha256\tsource_pixels\tghost_domain\trate_model").unwrap();
    let rate_model = std::env::var("OVERLAP7_RATE_MODEL").unwrap_or_else(|_| "proxy".into());
    assert!(matches!(rate_model.as_str(), "proxy" | "tokens"));
    let cas = std::env::var("OVERLAP7_CAS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| output.join("cas"));
    let ghost_domain = std::env::var("OVERLAP7_GHOST_DOMAIN").unwrap_or_else(|_| "source".into());
    assert!(matches!(ghost_domain.as_str(), "source" | "reconstruction"));
    let ghost_iterations = std::env::var("OVERLAP7_GHOST_ITERS")
        .unwrap_or_else(|_| "0".into())
        .parse::<usize>()
        .unwrap();
    let effort = std::env::var("OVERLAP7_EFFORT")
        .unwrap_or_else(|_| "7.5".into())
        .parse::<f32>()
        .unwrap();
    let aq_gamma = std::env::var("OVERLAP7_AQ")
        .unwrap_or_else(|_| "0".into())
        .parse::<f32>()
        .unwrap();
    let seed = std::env::var("OVERLAP7_SEED")
        .unwrap_or_else(|_| "1".into())
        .parse::<u8>()
        .unwrap();
    let rho = std::env::var("OVERLAP7_RHO")
        .unwrap_or_else(|_| "0.25".into())
        .parse::<f64>()
        .unwrap();
    let edge = std::env::var("OVERLAP7_EDGE")
        .unwrap_or_else(|_| "1".into())
        .parse::<f64>()
        .unwrap();
    let passes = std::env::var("OVERLAP7_PASSES")
        .unwrap_or_else(|_| "0".into())
        .parse::<usize>()
        .unwrap();
    let qualities = std::env::var("OVERLAP7_QUALITIES")
        .unwrap_or_else(|_| "5,10,15,20,25,30,35,40,45,50,55,60,65,70,75,80,85,90,95,100".into());
    for line in std::fs::read_to_string(manifest)
        .unwrap()
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let fields: Vec<_> = line.split('\t').collect();
        let name = fields[0];
        let path = fields[1];
        let source_file = std::fs::read(path).unwrap();
        let upgraded =
            crate::upgrade_image(&source_file, &crate::UpgradeLimits::default()).unwrap();
        let (info, stored_src) = crate::decode(&upgraded).unwrap();
        assert_eq!(info.sample, SampleType::U8);
        assert_eq!(info.cicp, crate::Cicp::SRGB, "source must be declared sRGB");
        // Explicit lossless sample expansion. Never flatten nonopaque alpha or
        // silently truncate higher precision at this RGB8 study boundary.
        let full_src: Vec<u8> = match info.channels {
            Channels::Rgb => stored_src,
            Channels::Gray => stored_src.into_iter().flat_map(|v| [v; 3]).collect(),
            Channels::Rgba => stored_src
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| {
                    assert_eq!(p[3], 255, "study requires opaque alpha");
                    [p[0], p[1], p[2]]
                })
                .collect(),
            Channels::GrayAlpha => stored_src
                .as_chunks::<2>()
                .0
                .iter()
                .flat_map(|p| {
                    assert_eq!(p[1], 255, "study requires opaque alpha");
                    [p[0]; 3]
                })
                .collect(),
            _ => panic!("study input requires declared sRGB RGB or grayscale samples"),
        };
        let (full_w, full_h) = (info.width as usize, info.height as usize);
        let cap = fields
            .get(2)
            .map_or(usize::MAX, |s| s.parse::<usize>().unwrap());
        let height_cap = fields.get(3).map_or(cap, |s| s.parse::<usize>().unwrap());
        let (w, h) = (full_w.min(cap), full_h.min(height_cap));
        let (x0, y0) = ((full_w - w) / 2, (full_h - h) / 2);
        let mut src = vec![0; w * h * 3];
        for y in 0..h {
            src[y * w * 3..][..w * 3]
                .copy_from_slice(&full_src[((y + y0) * full_w + x0) * 3..][..w * 3]);
        }
        std::fs::write(output.join(format!("{name}.source.rgb")), &src).unwrap();
        let (source_pixels, source_sha256) = store(&cas, &src, "rgb");
        for mode in [Mode::Base, Mode::Own, Mode::Blend] {
            let (atlas_info, atlas) = patches(&src, w, h, w * 3, mode);
            for color in [LossyColor::Xyb, LossyColor::YCoCg] {
                for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                    for quality in qualities.split(',').map(|s| s.parse::<f32>().unwrap()) {
                        let mut cfg = LossyConfig::new(quality)
                            .with_color(color)
                            .with_entropy(entropy)
                            .with_effort(effort)
                            .with_stripe_block_rows(4);
                        cfg.lab.superblocks = Some(false);
                        cfg.column_groups = Some(0);
                        cfg.aq_gamma = aq_gamma;
                        cfg.seed_bank = u64::from(seed);
                        let mut file = encode_lossy(
                            &atlas,
                            atlas_info.row_bytes(),
                            &atlas_info,
                            &Metadata::default(),
                            &cfg,
                        )
                        .unwrap();
                        if ghost_iterations > 0 && mode == Mode::Own {
                            file = ghost::refine(
                                &file,
                                &src,
                                w,
                                h,
                                w * 3,
                                &cfg,
                                ghost_iterations,
                                ghost_domain == "reconstruction",
                            );
                        }
                        if passes > 0 {
                            file = rdo::refine(
                                &file,
                                &src,
                                w,
                                h,
                                w * 3,
                                mode,
                                &cfg,
                                rho,
                                edge,
                                passes,
                                rate_model == "tokens",
                            );
                        }
                        let mut decoded = vec![0; w * h * 3];
                        decode(&file, w, h, w * 3, &mut decoded, mode, 1, false).unwrap();
                        let score = fast_ssim2::compute_ssimulacra2(
                            imgref::Img::new(src.as_chunks::<3>().0, w, h),
                            imgref::Img::new(decoded.as_chunks::<3>().0, w, h),
                        )
                        .unwrap();
                        let tag =
                            format!("{name}_{}_{color:?}_{entropy:?}_q{quality}", mode.name());
                        let mut wrapped = Vec::new();
                        if mode != Mode::Base {
                            wrapped.extend_from_slice(b"OV7LAB01");
                            wrapped.extend_from_slice(&(w as u32).to_le_bytes());
                            wrapped.extend_from_slice(&(h as u32).to_le_bytes());
                            wrapped.extend_from_slice(
                                &(if mode == Mode::Own { 1u32 } else { 2u32 }).to_le_bytes(),
                            );
                            wrapped.extend_from_slice(&0u32.to_le_bytes());
                        }
                        wrapped.extend_from_slice(&file);
                        let bytes = wrapped.len();
                        let (enc, encoded_sha256) = store(&cas, &wrapped, "ov7");
                        let (dec, decoded_sha256) = store(&cas, &decoded, "rgb");
                        let ghost_actual = if mode == Mode::Own {
                            ghost_iterations
                        } else {
                            0
                        };
                        writeln!(results,"{name}\t{w}\t{h}\t{}\t{color:?}\t{entropy:?}\t{quality}\t{effort}\t{}\t{bytes}\t{score}\t{enc}\t{dec}\t{rho}\t{edge}\t{passes}\t{seed}\t{ghost_actual}\t{encoded_sha256}\t{decoded_sha256}\t{source_sha256}\t{source_pixels}\t{ghost_domain}\t{rate_model}",mode.name(),aq_gamma!=0.0).unwrap();
                        results.flush().unwrap();
                        println!("complete {tag}: {} bytes, SSIMULACRA2 {score}", bytes);
                    }
                }
            }
        }
    }
}

fn timing_driver() {
    use std::sync::Arc;
    use std::time::Duration;
    let dir = std::env::var("OVERLAP7_INPUT").expect("OVERLAP7_INPUT required");
    let source = std::env::var("OVERLAP7_TIME_SOURCE").expect("OVERLAP7_TIME_SOURCE required");
    let quality = std::env::var("OVERLAP7_TIME_Q").unwrap_or_else(|_| "50".into());
    let per_mode: std::collections::BTreeMap<String, String> =
        std::env::var("OVERLAP7_TIME_QUALITIES")
            .map(|value| serde_json::from_str(&value).expect("mode quality JSON object"))
            .unwrap_or_default();
    let comparison = if per_mode.is_empty() {
        format!("q{quality}")
    } else {
        assert_eq!(
            per_mode.keys().map(String::as_str).collect::<Vec<_>>(),
            ["base", "blend", "own"]
        );
        std::env::var("OVERLAP7_COMPARISON_TAG")
            .expect("explicit comparison label required for different per-arm qualities")
    };
    let color = std::env::var("OVERLAP7_TIME_COLOR").unwrap_or_else(|_| "Xyb".into());
    let entropy = std::env::var("OVERLAP7_TIME_ENTROPY").unwrap_or_else(|_| "RunLevel".into());
    let threads: usize = std::env::var("OVERLAP7_TIME_THREADS")
        .expect("thread count required")
        .parse()
        .unwrap();
    let kernels: Vec<Option<(usize, bool)>> = std::env::var("OVERLAP7_KERNELS")
        .unwrap_or_else(|_| "legacy,8,16,32,64,p8,p16,p32,p64".into())
        .split(',')
        .map(|v| {
            if v == "legacy" {
                None
            } else {
                Some((
                    v.strip_prefix('p').unwrap_or(v).parse().unwrap(),
                    v.starts_with('p'),
                ))
            }
        })
        .collect();
    let mut arms = Vec::new();
    for line in std::fs::read_to_string(Path::new(&dir).join("quality.tsv"))
        .unwrap()
        .lines()
        .skip(1)
    {
        let f: Vec<_> = line.split('\t').collect();
        let selected_quality = per_mode.get(f[3]).unwrap_or(&quality);
        if f[0] != source || f[4] != color || f[5] != entropy || f[6] != selected_quality {
            continue;
        }
        let mode = match f[3] {
            "base" => Mode::Base,
            "own" => Mode::Own,
            "blend" => Mode::Blend,
            _ => panic!("unknown mode"),
        };
        assert!(
            matches!(f.len(), 13 | 17 | 18 | 22 | 23 | 24),
            "quality table schema"
        );
        let wrapped = std::fs::read(Path::new(&dir).join(f[11])).unwrap();
        assert_eq!(
            wrapped.len(),
            f[9].parse::<usize>().unwrap(),
            "encoded artifact length"
        );
        let bytes = if mode == Mode::Base {
            wrapped
        } else {
            assert_eq!(&wrapped[..8], b"OV7LAB01");
            wrapped[24..].to_vec()
        };
        let (w, h) = (
            f[1].parse::<usize>().unwrap(),
            f[2].parse::<usize>().unwrap(),
        );
        let expected = std::fs::read(Path::new(&dir).join(f[12])).unwrap();
        let mut out = vec![0; w * h * 3];
        decode(&bytes, w, h, w * 3, &mut out, mode, threads, false).unwrap();
        assert_eq!(out, expected);
        arms.push((mode, Arc::new(bytes), w, h));
    }
    assert_eq!(arms.len(), 3, "need complete measured base/own/blend cells");
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap();
    if std::env::var("OVERLAP7_ACTION").as_deref() == Ok("profile") {
        let selected = std::env::var("OVERLAP7_PROFILE_MODE").expect("profile mode required");
        let (mode, file, w, h) = arms
            .into_iter()
            .find(|(mode, _, _, _)| {
                mode.name() == selected || selected == "production" && *mode == Mode::Base
            })
            .expect("profile arm not found");
        let mut out = vec![0; w * h * 3];
        pool.install(|| {
            for _ in 0..1000 {
                if selected == "production" {
                    crate::decode_into_threads(
                        std::hint::black_box(&file),
                        &mut out,
                        w * 3,
                        threads,
                    )
                    .unwrap();
                } else {
                    if let Some((chunk, prune)) = kernels[0] {
                        (if prune {
                            optimized::decode_pruned
                        } else {
                            optimized::decode_chunked
                        })(
                            &file, w, h, w * 3, &mut out, mode, threads, chunk
                        )
                        .unwrap();
                    } else {
                        decode(
                            std::hint::black_box(&file),
                            w,
                            h,
                            w * 3,
                            &mut out,
                            mode,
                            threads,
                            false,
                        )
                        .unwrap();
                    }
                }
                std::hint::black_box(&out);
            }
        });
        return;
    }
    let result = pool.install(|| {
        zenbench::run(|suite| {
            suite.group(
                format!("{source}/{color}/{entropy}/{comparison}/{threads}t"),
                |g| {
                    g.config()
                        .max_rounds(24)
                        .min_rounds(24)
                        .auto_rounds(false)
                        .warmup_time(Duration::from_millis(100));
                    let geometry = (arms[0].2, arms[0].3);
                    assert!(arms.iter().all(|(_, _, w, h)| (*w, *h) == geometry));
                    // Reuse the identical caller buffer across whole-image arms.
                    // This prevents heap placement from distinguishing A/A.
                    let shared_out =
                        Arc::new(std::sync::Mutex::new(vec![0; geometry.0 * geometry.1 * 3]));
                    for (mode, file, w, h) in arms {
                        if mode == Mode::Base {
                            for name in ["production", "production_AA"] {
                                let file = file.clone();
                                let out = shared_out.clone();
                                g.bench(name, move |b| {
                                    b.iter(|| {
                                        let mut out = out.lock().unwrap();
                                        crate::decode_into_threads(
                                            std::hint::black_box(&file),
                                            &mut out,
                                            w * 3,
                                            threads,
                                        )
                                        .unwrap();
                                        std::hint::black_box(&out[..]);
                                    })
                                });
                            }
                        }
                        if mode == Mode::Base {
                            for strength in postfilter::Strength::GRID {
                                if strength == postfilter::Strength::Off {
                                    continue;
                                }
                                let out = shared_out.clone();
                                let file = file.clone();
                                let name = format!("production_seam_{}", strength.name());
                                g.bench(name, move |b| {
                                    b.iter(|| {
                                        let mut out = out.lock().unwrap();
                                        crate::decode_into_threads(
                                            std::hint::black_box(&file),
                                            &mut out,
                                            w * 3,
                                            threads,
                                        )
                                        .unwrap();
                                        postfilter::apply(
                                            &mut out,
                                            postfilter::Layout {
                                                width: w,
                                                height: h,
                                                stride_bytes: w * 3,
                                                channels: 3,
                                                origin: (0, 0),
                                            },
                                            strength,
                                        )
                                        .unwrap();
                                        std::hint::black_box(&out[..]);
                                    })
                                });
                            }
                        }
                        if threads == 1 {
                            if mode == Mode::Base {
                                for prefix in [false, true] {
                                    let file = file.clone();
                                    let mut row = vec![0; w * 3];
                                    let name = if prefix {
                                        "production_stream_first_row"
                                    } else {
                                        "production_stream_full"
                                    };
                                    g.bench(name, move |b| {
                                        b.iter(|| {
                                            let mut stream = crate::StreamDecoder::new(
                                                std::hint::black_box(&file),
                                            )
                                            .unwrap();
                                            let mut delivered = 0;
                                            while stream.next_row(&mut row).unwrap() {
                                                delivered += 1;
                                                std::hint::black_box(&row);
                                                if prefix {
                                                    break;
                                                }
                                            }
                                            assert_eq!(delivered, if prefix { 1 } else { h });
                                        })
                                    });
                                }
                            }
                            for (chunk, _) in kernels
                                .iter()
                                .flatten()
                                .copied()
                                .filter(|(_, prune)| *prune)
                            {
                                for prefix in [false, true] {
                                    let file = file.clone();
                                    let name = format!(
                                        "{}_stream_p{chunk}_{}",
                                        mode.name(),
                                        if prefix { "first_row" } else { "full" }
                                    );
                                    g.bench(name, move |b| {
                                        b.iter(|| {
                                            let mut delivered = 0;
                                            optimized::decode_stream(
                                                std::hint::black_box(&file),
                                                w,
                                                h,
                                                mode,
                                                chunk,
                                                |_, row| {
                                                    delivered += 1;
                                                    std::hint::black_box(row);
                                                    !prefix
                                                },
                                            )
                                            .unwrap();
                                            assert_eq!(delivered, if prefix { 1 } else { h });
                                        })
                                    });
                                }
                            }
                        }
                        for kernel in kernels.iter().copied() {
                            let out = shared_out.clone();
                            let file = file.clone();
                            let name = match kernel {
                                None => format!("{}_legacy", mode.name()),
                                Some((n, false)) => format!("{}_c{n}", mode.name()),
                                Some((n, true)) => format!("{}_p{n}", mode.name()),
                            };
                            g.bench(name, move |b| {
                                b.iter(|| {
                                    let mut out = out.lock().unwrap();
                                    if let Some((chunk, prune)) = kernel {
                                        (if prune {
                                            optimized::decode_pruned
                                        } else {
                                            optimized::decode_chunked
                                        })(
                                            std::hint::black_box(&file),
                                            w,
                                            h,
                                            w * 3,
                                            &mut out,
                                            mode,
                                            threads,
                                            chunk,
                                        )
                                        .unwrap();
                                    } else {
                                        decode(
                                            std::hint::black_box(&file),
                                            w,
                                            h,
                                            w * 3,
                                            &mut out,
                                            mode,
                                            threads,
                                            false,
                                        )
                                        .unwrap();
                                    }
                                    std::hint::black_box(&out[..]);
                                })
                            });
                        }
                    }
                },
            );
        })
    });
    let output = std::env::var("OVERLAP7_OUTPUT").expect("OVERLAP7_OUTPUT required");
    let bytes = serde_json::to_vec_pretty(&result).unwrap();
    let mut f = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(output)
        .unwrap();
    f.write_all(&bytes).unwrap();
    zenbench::postprocess_result(&result);
}

/// Strong baseline control: existing fixed rendering filter, with no per-file
/// signalling charge. This favours the ordinary encoder in rate comparisons.
fn filter_driver() {
    let input = std::env::var("OVERLAP7_INPUT").expect("OVERLAP7_INPUT required");
    let dir = Path::new(&input);
    let output = std::env::var("OVERLAP7_OUTPUT").expect("OVERLAP7_OUTPUT required");
    let output = Path::new(&output);
    std::fs::create_dir(output).unwrap();
    let cas = std::env::var("OVERLAP7_CAS").expect("shared CAS required");
    let cas = Path::new(&cas);
    let table = std::fs::read_to_string(dir.join("quality.tsv")).unwrap();
    let mut result = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(output.join("quality.tsv"))
        .unwrap();
    writeln!(result,"source\twidth\theight\tmode\tcolor\tentropy\tquality\teffort\taq\tbytes\tssim2\tencoded\tdecoded\trdo_rho\trdo_edge\trdo_passes\tseed\tghost_iterations\tencoded_sha256\tdecoded_sha256\tsource_sha256\tsource_pixels").unwrap();
    for line in table.lines().skip(1) {
        let row: Vec<_> = line.split('\t').collect();
        assert!(matches!(row.len(), 13 | 17 | 18 | 22 | 23 | 24));
        if row[3] != "base" {
            continue;
        }
        let (w, h) = (
            row[1].parse::<usize>().unwrap(),
            row[2].parse::<usize>().unwrap(),
        );
        let encoded = dir.join(row[11]);
        let source = if row.len() >= 22 {
            Path::new(row[21]).to_path_buf()
        } else {
            encoded
                .parent()
                .unwrap()
                .join(format!("{}.source.rgb", row[0]))
        };
        let original = std::fs::read(&source).unwrap();
        let decoded = std::fs::read(dir.join(row[12])).unwrap();
        assert_eq!(original.len(), w * h * 3);
        assert_eq!(decoded.len(), original.len());
        let encoded_bytes = std::fs::read(&encoded).unwrap();
        assert_eq!(encoded_bytes.len(), row[9].parse::<usize>().unwrap());
        let (enc, enc_hash) = store(cas, &encoded_bytes, "ov7");
        let (source_path, source_hash) = store(cas, &original, "rgb");
        for strength in postfilter::Strength::GRID {
            if strength == postfilter::Strength::Off {
                continue;
            }
            let strength = postfilter::Strength::parse(strength.name()).unwrap();
            let mut pixels = decoded.clone();
            postfilter::apply(
                &mut pixels,
                postfilter::Layout {
                    width: w,
                    height: h,
                    stride_bytes: w * 3,
                    channels: 3,
                    origin: (0, 0),
                },
                strength,
            )
            .unwrap();
            let score = fast_ssim2::compute_ssimulacra2(
                imgref::Img::new(original.as_chunks::<3>().0, w, h),
                imgref::Img::new(pixels.as_chunks::<3>().0, w, h),
            )
            .unwrap();
            let (dec, dec_hash) = store(cas, &pixels, "rgb");
            let mode = format!("base_{}", strength.name());
            writeln!(result,"{}\t{}\t{}\t{mode}\t{}\t{}\t{}\t{}\t{}\t{}\t{score}\t{enc}\t{dec}\t0\t0\t0\t1\t0\t{enc_hash}\t{dec_hash}\t{source_hash}\t{source_path}",row[0],w,h,row[4],row[5],row[6],row[7],row[8],row[9]).unwrap();
            result.flush().unwrap();
        }
    }
}

/// Check retained, real entropy-coded carriers before admitting timing inputs.
fn carrier_check() {
    let input = std::env::var("OVERLAP7_INPUT").expect("OVERLAP7_INPUT required");
    let dir = Path::new(&input);
    let output = std::env::var("OVERLAP7_OUTPUT").expect("progress output required");
    let pools = [1, 2, 4, 8].map(|n| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build()
            .unwrap()
    });
    let table = std::fs::read_to_string(dir.join("quality.tsv")).unwrap();
    let mut completed = 0;
    for line in table.lines().skip(1) {
        let row: Vec<_> = line.split('\t').collect();
        assert!(matches!(row.len(), 13 | 17 | 18 | 22 | 23 | 24));
        let mode = match row[3] {
            "base" => Mode::Base,
            "own" => Mode::Own,
            "blend" => Mode::Blend,
            _ => panic!("unsupported carrier mode"),
        };
        let (w, h) = (
            row[1].parse::<usize>().unwrap(),
            row[2].parse::<usize>().unwrap(),
        );
        let wrapped = std::fs::read(dir.join(row[11])).unwrap();
        assert_eq!(wrapped.len(), row[9].parse::<usize>().unwrap());
        if row.len() >= 22 {
            assert_eq!(sha256(&wrapped), row[18]);
        }
        let file = if mode == Mode::Base {
            &wrapped[..]
        } else {
            assert_eq!(&wrapped[..8], b"OV7LAB01");
            let value =
                |offset| u32::from_le_bytes(wrapped[offset..offset + 4].try_into().unwrap());
            assert_eq!((value(8), value(12)), (w as u32, h as u32));
            assert_eq!(value(16), if mode == Mode::Own { 1 } else { 2 });
            assert_eq!(value(20), 0);
            &wrapped[24..]
        };
        let expected = std::fs::read(dir.join(row[12])).unwrap();
        assert_eq!(expected.len(), w * h * 3);
        if row.len() >= 22 {
            assert_eq!(sha256(&expected), row[19]);
        }
        for chunk in [8, 16, 32, 64] {
            for prune in [false, true] {
                for (index, threads) in [1, 2, 4, 8].into_iter().enumerate() {
                    let stride = w * 3 + 5;
                    let mut pixels = vec![0xCC; stride * h];
                    pools[index]
                        .install(|| {
                            (if prune {
                                optimized::decode_pruned
                            } else {
                                optimized::decode_chunked
                            })(
                                file, w, h, stride, &mut pixels, mode, threads, chunk
                            )
                        })
                        .unwrap();
                    for y in 0..h {
                        assert_eq!(
                            &pixels[y * stride..][..w * 3],
                            &expected[y * w * 3..][..w * 3],
                            "carrier={completed} chunk={chunk} prune={prune} threads={threads} row={y}"
                        );
                        assert!(
                            pixels[y * stride + w * 3..(y + 1) * stride]
                                .iter()
                                .all(|&v| v == 0xCC)
                        );
                    }
                }
            }
            let mut seen = 0;
            optimized::decode_stream(file, w, h, mode, chunk, |y, pixels| {
                assert_eq!(y, seen);
                assert_eq!(pixels, &expected[y * w * 3..][..w * 3]);
                seen += 1;
                true
            })
            .unwrap();
            assert_eq!(seen, h);
        }
        completed += 1;
        std::fs::write(&output,serde_json::to_vec_pretty(&serde_json::json!({"pid":std::process::id(),"completed":completed,"source":row[0],"mode":row[3],"quality":row[6],"last_check_epoch":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()})).unwrap()).unwrap();
    }
    println!("exact retained carriers: {completed}");
}
