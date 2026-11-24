//! Overshoot deringing A/B (encoder-only change, lossy): every source at
//! 64², 256² and 1024² center crops (or smaller when the source is), lossy
//! q5..=100 step 5, XYB and YCoCg, deringing off and on. One TSV row per
//! cell: complete-file bytes, zensim, SSIMULACRA2, butteraugli (3-norm and
//! max), and white fidelity (source pixels at 255,255,255 that do not
//! decode to 255,255,255, and their worst channel error).
//!
//! `cargo run --release --example dering_ab -- OUT.tsv ARM CLASS=FILE.png...`
//! with ARM `off`/`on` (deringing) or `dc-off`/`dc-on` (DC rounding): the
//! switches are process-global, so each arm runs in its own process with
//! every cell in parallel; concatenate the two TSVs.
//!
//! Sources are converted to RGB8: alpha is composited over white, 16-bit
//! samples keep their high byte, gray is replicated. Cells run in parallel
//! over (source, size); rows stream to OUT.tsv as they finish.
use std::io::Write;
use std::sync::Mutex;

use imij::{Channels, ImageInfo, ImiEncoderConfig, LossyColor, Metadata, SampleType};
use rayon::prelude::*;

struct Src {
    class: String,
    name: String,
    w: usize,
    h: usize,
    rgb: Vec<u8>,
}

fn load(class: &str, path: &str) -> Result<Src, String> {
    let data = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let out = zenpng::decode(
        &data,
        &zenpng::PngDecodeConfig::default(),
        &enough::Unstoppable,
    )
    .map_err(|e| format!("{path}: {e:?}"))?;
    let (w, h) = (out.info.width as usize, out.info.height as usize);
    let bytes = out.pixels.copy_to_contiguous_bytes();
    let wide = out.info.bit_depth == 16;
    let bpp = bytes.len() / (w * h);
    let nch = if wide { bpp / 2 } else { bpp };
    let sample = |i: usize| -> u32 {
        if wide {
            u16::from_ne_bytes([bytes[2 * i], bytes[2 * i + 1]]) as u32 >> 8
        } else {
            bytes[i] as u32
        }
    };
    let mut rgb = Vec::with_capacity(w * h * 3);
    for p in 0..w * h {
        let px: Vec<u32> = (0..nch).map(|c| sample(p * nch + c)).collect();
        let (c3, a) = match nch {
            1 => ([px[0]; 3], 255),
            2 => ([px[0]; 3], px[1]),
            3 => ([px[0], px[1], px[2]], 255),
            4 => ([px[0], px[1], px[2]], px[3]),
            _ => return Err(format!("{path}: {nch} channels")),
        };
        for v in c3 {
            // Over white: v * a + 255 * (255 - a), rounded.
            rgb.push(((v * a + 255 * (255 - a) + 127) / 255) as u8);
        }
    }
    let name = std::path::Path::new(path)
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .chars()
        .take(48)
        .collect();
    Ok(Src {
        class: class.into(),
        name,
        w,
        h,
        rgb,
    })
}

fn crop(s: &Src, side: usize) -> (usize, usize, Vec<u8>) {
    let (w, h) = (side.min(s.w), side.min(s.h));
    let (x0, y0) = ((s.w - w) / 2, (s.h - h) / 2);
    let mut v = Vec::with_capacity(w * h * 3);
    for y in y0..y0 + h {
        v.extend_from_slice(&s.rgb[(y * s.w + x0) * 3..][..w * 3]);
    }
    (w, h, v)
}

fn zensim(a: &[u8], b: &[u8], w: usize, h: usize) -> f64 {
    let px = |v: &[u8]| -> Vec<[u8; 3]> { v.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect() };
    let (a, b) = (px(a), px(b));
    zensim::Zensim::new(zensim::ZensimProfile::codec_target())
        .compute(
            &zensim::RgbSlice::new(&a, w, h),
            &zensim::RgbSlice::new(&b, w, h),
        )
        .map(|r| r.score())
        .unwrap_or(f64::NAN)
}

fn slice(v: &[u8], w: usize, h: usize) -> fast_ssim2::PixelSlice<'_> {
    fast_ssim2::PixelSlice::new(
        v,
        w as u32,
        h as u32,
        w * 3,
        fast_ssim2::PixelDescriptor::RGB8_SRGB,
    )
    .unwrap()
}

fn ssim2(a: &[u8], b: &[u8], w: usize, h: usize) -> f64 {
    match fast_ssim2::Ssimulacra2Reference::new(&slice(a, w, h)) {
        Ok(r) => r.compare(&slice(b, w, h)).unwrap_or(f64::NAN),
        Err(_) => f64::NAN,
    }
}

fn butteraugli(a: &[u8], b: &[u8], w: usize, h: usize) -> (f64, f64) {
    let to = |v: &[u8]| -> Vec<butteraugli::RGB8> {
        v.chunks_exact(3)
            .map(|c| butteraugli::RGB8::new(c[0], c[1], c[2]))
            .collect()
    };
    let (ia, ib) = (to(a), to(b));
    let (ia, ib) = (
        butteraugli::Img::new(ia, w, h),
        butteraugli::Img::new(ib, w, h),
    );
    match butteraugli::butteraugli(
        ia.as_ref(),
        ib.as_ref(),
        &butteraugli::ButteraugliParams::default(),
    ) {
        Ok(r) => (r.pnorm_3, r.score),
        Err(_) => (f64::NAN, f64::NAN),
    }
}

fn png(path: &str, w: usize, h: usize, rgb: &[u8]) {
    let px: Vec<rgb::RGB8> = rgb
        .chunks_exact(3)
        .map(|c| rgb::RGB8::new(c[0], c[1], c[2]))
        .collect();
    let bytes = zenpng::encode_rgb8(
        imgref::Img::new(px.as_slice(), w, h),
        None,
        &zenpng::EncodeConfig::default(),
        &enough::Unstoppable,
        &enough::Unstoppable,
    )
    .unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// `gallery OUTDIR CLASS=FILE.png SIZE COLOR Q NAME`: one case, both arms.
/// Writes NAME-{src,off,on}.png, NAME-diff.png (red: deringing raised the
/// pixel's worst-channel error, green: lowered it; 8x gain) and
/// NAME-zoom.png (source | off | on, 4x nearest, the 64x64 window where
/// deringing raised the error most), and prints both arms' bytes.
fn gallery(a: &[String]) {
    let (out, spec, side, color, q, name) = (&a[0], &a[1], &a[2], &a[3], &a[4], &a[5]);
    let (class, path) = spec.split_once('=').unwrap();
    let s = load(class, path).unwrap();
    let (w, h, px) = crop(&s, side.parse().unwrap());
    let info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8);
    let mut cfg = imij::ImiEncoderConfig::lossy(q.parse().unwrap());
    cfg.color = if color == "YCoCg" {
        LossyColor::YCoCg
    } else {
        LossyColor::Xyb
    };
    let mut dec = Vec::new();
    for on in [false, true] {
        imij::lab::set_lossy_dering(on);
        let file = cfg
            .encode_samples(imij::ImageRef::new(&px, w * 3, &info), &Metadata::default())
            .unwrap();
        println!("{name}\t{}\t{}", u8::from(on), file.len());
        dec.push(imij::decode(&file).unwrap().1);
    }
    let err = |d: &[u8], p: usize| {
        (0..3)
            .map(|c| px[p * 3 + c].abs_diff(d[p * 3 + c]))
            .max()
            .unwrap() as i32
    };
    let delta: Vec<i32> = (0..w * h)
        .map(|p| err(&dec[1], p) - err(&dec[0], p))
        .collect();
    let win = 64.min(w).min(h);
    let (mut best, mut at) = (i64::MIN, (0, 0));
    for y in (0..=h - win).step_by(8) {
        for x in (0..=w - win).step_by(8) {
            let sum: i64 = (y..y + win)
                .map(|yy| {
                    delta[yy * w + x..yy * w + x + win]
                        .iter()
                        .map(|&d| d as i64)
                        .sum::<i64>()
                })
                .sum();
            if sum > best {
                (best, at) = (sum, (x, y));
            }
        }
    }
    // A yellow outline marks the zoomed window on every map.
    let outline = |map: &mut Vec<u8>| {
        for i in 0..win {
            for (x, y) in [
                (at.0 + i, at.1),
                (at.0 + i, at.1 + win - 1),
                (at.0, at.1 + i),
                (at.0 + win - 1, at.1 + i),
            ] {
                map[(y * w + x) * 3..][..3].copy_from_slice(&[255, 220, 0]);
            }
        }
    };
    png(&format!("{out}/{name}-src.png"), w, h, &px);
    png(&format!("{out}/{name}-off.png"), w, h, &dec[0]);
    png(&format!("{out}/{name}-on.png"), w, h, &dec[1]);
    // Maps scale to each image's own range (square root, so errors of one
    // or two codes stay visible next to large ones); the maxima go to stdout.
    let gain =
        |m: i32| move |v: i32| ((v.unsigned_abs() as f32 / m.max(1) as f32).sqrt() * 255.0) as u8;
    let dmax = delta.iter().map(|d| d.abs()).max().unwrap_or(0);
    let g = gain(dmax);
    let diff: Vec<u8> = delta
        .iter()
        .flat_map(|&d| if d > 0 { [g(d), 0, 0] } else { [0, g(d), 0] })
        .collect();
    let mut diff = diff;
    outline(&mut diff);
    png(&format!("{out}/{name}-diff.png"), w, h, &diff);
    for (k, arm) in ["off", "on"].iter().enumerate() {
        let e: Vec<i32> = (0..w * h).map(|p| err(&dec[k], p)).collect();
        let emax = *e.iter().max().unwrap_or(&0);
        let g = gain(emax);
        let map: Vec<u8> = e.iter().flat_map(|&v| [g(v); 3]).collect();
        let mut map = map;
        outline(&mut map);
        png(&format!("{out}/{name}-err-{arm}.png"), w, h, &map);
        // White fidelity: source-white pixels that decode non-white, magenta
        // over a faint copy of the source.
        let wf: Vec<u8> = (0..w * h)
            .flat_map(|p| {
                let s = &px[p * 3..p * 3 + 3];
                if s == [255, 255, 255] && dec[k][p * 3..p * 3 + 3] != [255, 255, 255] {
                    [255, 0, 255]
                } else {
                    let l = (s.iter().map(|&v| v as u32).sum::<u32>() / 3 / 2 + 128) as u8;
                    [l, l, l]
                }
            })
            .collect();
        let mut wf = wf;
        outline(&mut wf);
        png(&format!("{out}/{name}-white-{arm}.png"), w, h, &wf);
        println!("{name}\temax_{arm}\t{emax}");
        // White fidelity: source-white pixels that decode non-white, split by
        // whether their 8x8 block is entirely white in the source (the DC
        // rounding fix makes those exact) or holds an edge (ringing). Dip =
        // 255 minus the lowest channel; chroma-dominated when the channels
        // differ by at least half the dip (a tint, not a luma undershoot).
        let is_white = |p: usize| px[p * 3..p * 3 + 3] == [255, 255, 255];
        let full_block = |p: usize| {
            let (x0, y0) = ((p % w) / 8 * 8, (p / w) / 8 * 8);
            x0 + 8 <= w
                && y0 + 8 <= h
                && (y0..y0 + 8).all(|y| (x0..x0 + 8).all(|x| is_white(y * w + x)))
        };
        let (mut white, mut in_full, mut in_edge, mut dip2, mut dip16, mut tint) =
            (0, 0, 0, 0, 0, 0);
        let mut lowest = 255u8;
        for p in 0..w * h {
            if !is_white(p) {
                continue;
            }
            white += 1;
            let d = &dec[k][p * 3..p * 3 + 3];
            if d == [255, 255, 255] {
                continue;
            }
            if full_block(p) {
                in_full += 1;
            } else {
                in_edge += 1;
            }
            let (lo, hi) = (*d.iter().min().unwrap(), *d.iter().max().unwrap());
            let dip = 255 - lo;
            lowest = lowest.min(lo);
            dip2 += usize::from(dip <= 2);
            dip16 += usize::from(dip <= 16);
            tint += usize::from(2 * (hi - lo) >= dip);
        }
        println!(
            "{name}\twhite_{arm}\twhite={white};full={in_full};edge={in_edge};dip2={dip2};dip16={dip16};tint={tint};lowest={lowest}"
        );
    }
    println!("{name}\tdmax\t{dmax}");
    let z = 4;
    let zw = win * z * 3 + 2 * 4;
    let zh = win * z;
    let mut zoom = vec![255u8; zw * zh * 3];
    for (k, img) in [&px, &dec[0], &dec[1]].into_iter().enumerate() {
        for y in 0..zh {
            for x in 0..win * z {
                let sp = ((at.1 + y / z) * w + at.0 + x / z) * 3;
                let dp = (y * zw + k * (win * z + 4) + x) * 3;
                zoom[dp..dp + 3].copy_from_slice(&img[sp..sp + 3]);
            }
        }
    }
    png(&format!("{out}/{name}-zoom.png"), zw, zh, &zoom);
    println!("{name}\tzoom_at\t{},{},{win}", at.0, at.1);
}

/// `colors OUT.tsv ARM CLASS=FILE.png...`: do an image's dominant colors
/// survive? For each source (1024² crop), q5..100 step 5, XYB and YCoCg,
/// and each color covering at least 1% of the crop (up to 8): the mean
/// decoded color of that color's pixels, as Oklab distance x100 from the
/// source color, over pixels in blocks made only of that color (`flat`:
/// the block mean, i.e. DC, alone) and over all its pixels (`all`).
fn colors(a: &[String]) {
    let out_path = &a[0];
    match a[1].as_str() {
        "dc-on" | "dc-off" => imij::lab::set_lossy_dc_nearest(a[1] == "dc-on"),
        "default" => {}
        x if x.starts_with("dcmul=") => {
            let v: Vec<f32> = x[6..].split(',').map(|x| x.parse().unwrap()).collect();
            imij::lab::set_lossy_dc_step((v[0] * 64.0).round() as u8, (v[1] * 64.0).round() as u8);
        }
        x => panic!("ARM must be dc-on, dc-off or default, not {x}"),
    }
    let srcs: Vec<Src> = a[2..]
        .iter()
        .filter_map(|s| {
            let (class, path) = s.split_once('=').unwrap();
            load(class, path).map_err(|e| eprintln!("skip {e}")).ok()
        })
        .collect();
    let m1 = zenpixels_convert::oklab::rgb_to_lms_matrix(zenpixels::ColorPrimaries::Bt709).unwrap();
    let lab = |c: [f32; 3]| {
        let l = c.map(linear_srgb::default::srgb_to_linear);
        zenpixels_convert::oklab::rgb_to_oklab(l[0], l[1], l[2], &m1)
    };
    let de = |a: [f32; 3], b: [f32; 3]| {
        let (x, y) = (lab(a), lab(b));
        100.0 * ((x[0] - y[0]).powi(2) + (x[1] - y[1]).powi(2) + (x[2] - y[2]).powi(2)).sqrt()
    };
    let out = Mutex::new(std::fs::File::create(out_path).unwrap());
    writeln!(
        out.lock().unwrap(),
        "class\timage\tw\th\tcolor\tq\trank\tsrc_rgb\tshare\tflat_px\tde_flat\tde_all\tdec_flat_rgb\tbytes"
    )
    .unwrap();
    srcs.par_iter().for_each(|s| {
        let (w, h, px) = crop(s, 1024);
        let mut freq = std::collections::HashMap::<[u8; 3], usize>::new();
        for p in px.chunks_exact(3) {
            *freq.entry([p[0], p[1], p[2]]).or_default() += 1;
        }
        let mut top: Vec<([u8; 3], usize)> =
            freq.into_iter().filter(|&(_, n)| n * 100 >= w * h).collect();
        top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        top.truncate(8);
        // Pixels of blocks made of one color only.
        let mut flat = vec![false; w * h];
        for by in 0..h / 8 {
            for bx in 0..w / 8 {
                let c0 = &px[((by * 8) * w + bx * 8) * 3..][..3];
                let uniform = (0..8).all(|y| {
                    (0..8).all(|x| &px[((by * 8 + y) * w + bx * 8 + x) * 3..][..3] == c0)
                });
                if uniform {
                    for y in 0..8 {
                        for x in 0..8 {
                            flat[(by * 8 + y) * w + bx * 8 + x] = true;
                        }
                    }
                }
            }
        }
        let info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8);
        let mut rows = String::new();
        for color in [LossyColor::Xyb, LossyColor::YCoCg] {
            for q in qualities() {
                let mut cfg = imij::ImiEncoderConfig::lossy(q);
                cfg = cfg.tune_lossy(|t| { t.color = color; });
                let file = cfg.encode_samples(imij::ImageRef::new(&px, w * 3, &info), &Metadata::default()).unwrap();
                let (_, dec) = imij::decode(&file).unwrap();
                for (rank, (c, n)) in top.iter().enumerate() {
                    let (mut sf, mut nf, mut sa) = ([0f64; 3], 0usize, [0f64; 3]);
                    for p in 0..w * h {
                        if px[p * 3..p * 3 + 3] != c[..] {
                            continue;
                        }
                        for k in 0..3 {
                            sa[k] += dec[p * 3 + k] as f64;
                            if flat[p] {
                                sf[k] += dec[p * 3 + k] as f64;
                            }
                        }
                        nf += usize::from(flat[p]);
                    }
                    let src = c.map(|v| v as f32 / 255.0);
                    let mean = |s: [f64; 3], m: usize| s.map(|v| (v / m.max(1) as f64 / 255.0) as f32);
                    let (mf, ma) = (mean(sf, nf), mean(sa, *n));
                    let def = if nf > 0 { de(src, mf) } else { f32::NAN };
                    rows.push_str(&format!(
                        "{}\t{}\t{w}\t{h}\t{color:?}\t{q}\t{rank}\t{},{},{}\t{:.4}\t{nf}\t{def:.3}\t{:.3}\t{:.1},{:.1},{:.1}\t{}\n",
                        s.class,
                        s.name,
                        c[0],
                        c[1],
                        c[2],
                        *n as f64 / (w * h) as f64,
                        de(src, ma),
                        mf[0] * 255.0,
                        mf[1] * 255.0,
                        mf[2] * 255.0,
                        file.len()
                    ));
                }
            }
        }
        out.lock().unwrap().write_all(rows.as_bytes()).unwrap();
        eprintln!("{} {}", s.class, s.name);
    });
}

/// `--fast` (first argument): screening grid. Sizes 256² and 1024², q5..30
/// step 5 plus q40..90 step 10, and no butteraugli (written as NaN).
static FAST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// `--with-ba` keeps butteraugli in `--fast`; `--only-1024` drops 256².
static WITH_BA: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static ONLY_1024: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn qualities() -> Vec<f32> {
    if FAST.load(std::sync::atomic::Ordering::Relaxed) {
        vec![
            5.0, 10.0, 15.0, 20.0, 25.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0,
        ]
    } else {
        (1..=20).map(|i| i as f32 * 5.0).collect()
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    while let Some(flag) = args.first().filter(|a| a.starts_with("--")).cloned() {
        match flag.as_str() {
            "--fast" => FAST.store(true, std::sync::atomic::Ordering::Relaxed),
            "--with-ba" => WITH_BA.store(true, std::sync::atomic::Ordering::Relaxed),
            "--only-1024" => ONLY_1024.store(true, std::sync::atomic::Ordering::Relaxed),
            f => panic!("unknown flag {f}"),
        }
        args.remove(0);
    }
    if args.first().map(String::as_str) == Some("gallery") {
        return gallery(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("colors") {
        return colors(&args[1..]);
    }
    let out_path = &args[0];
    // ARM: `off`/`on` toggles deringing (DC rounding at its default);
    // `dc-off`/`dc-on` toggles nearest + clamp-aware DC rounding (deringing
    // on). The `dering` column records which side of the toggle ran.
    let on = match args[1].as_str() {
        "on" => true,
        "off" => false,
        "dc-on" | "dc-off" => {
            imij::lab::set_lossy_dc_nearest(args[1] == "dc-on");
            args[1] == "dc-on"
        }
        // `dcmul=L,C`: lab DC step multipliers (luma, chroma); the column
        // records 1 (a candidate arm; compare against a `dc-on` baseline).
        a if a.starts_with("dcmul=") => {
            let v: Vec<f32> = a[6..].split(',').map(|x| x.parse().unwrap()).collect();
            imij::lab::set_lossy_dc_step((v[0] * 64.0).round() as u8, (v[1] * 64.0).round() as u8);
            true
        }
        a => panic!("ARM must be on, off, dc-on, dc-off or dcmul=L,C, not {a}"),
    };
    if !args[1].starts_with("dc-") && !args[1].starts_with("dcmul=") {
        imij::lab::set_lossy_dering(on);
    }
    let srcs: Vec<Src> = args[2..]
        .iter()
        .filter_map(|a| {
            let (class, path) = a.split_once('=').expect("CLASS=FILE.png");
            load(class, path).map_err(|e| eprintln!("skip {e}")).ok()
        })
        .collect();
    let sizes: &[usize] = if ONLY_1024.load(std::sync::atomic::Ordering::Relaxed) {
        &[1024]
    } else if FAST.load(std::sync::atomic::Ordering::Relaxed) {
        &[256, 1024]
    } else {
        &[64, 256, 1024]
    };
    let jobs: Vec<(usize, usize)> = (0..srcs.len())
        .flat_map(|i| sizes.iter().map(move |&s| (i, s)))
        .collect();
    let out = Mutex::new(std::fs::File::create(out_path).unwrap());
    writeln!(
        out.lock().unwrap(),
        "class\timage\tw\th\tcolor\tq\tdering\tbytes\tzensim\tssim2\tba_p3\tba_max\twhite_px\twhite_bad\twhite_maxerr"
    )
    .unwrap();
    let done = std::sync::atomic::AtomicUsize::new(0);
    jobs.par_iter().for_each(|&(i, side)| {
        let s = &srcs[i];
        let (w, h, px) = crop(s, side);
        let smaller = sizes.iter().copied().filter(|&z| z < side).max();
        if smaller.is_some_and(|z| w <= z && h <= z) {
            return; // the same crop as a smaller size
        }
        let info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8);
        let white: Vec<usize> = (0..w * h)
            .filter(|&p| px[p * 3..p * 3 + 3] == [255, 255, 255])
            .collect();
        let mut rows = String::new();
        for color in [LossyColor::Xyb, LossyColor::YCoCg] {
            for q in qualities() {
                {
                    let mut cfg = imij::ImiEncoderConfig::lossy(q);
                    cfg = cfg.tune_lossy(|t| { t.color = color; });
                    let file = cfg.encode_samples(imij::ImageRef::new(&px, w * 3, &info), &Metadata::default());
                    let file = match file {
                        Ok(f) => f,
                        Err(e) => {
                            eprintln!("{} {side} {color:?} q{q}: {e:?}", s.name);
                            continue;
                        }
                    };
                    let (_, dec) = imij::decode(&file).unwrap();
                    let (bad, maxerr) = white.iter().fold((0usize, 0u8), |(n, m), &p| {
                        let e = dec[p * 3..p * 3 + 3].iter().map(|&v| 255 - v).max().unwrap();
                        (n + usize::from(e != 0), m.max(e))
                    });
                    let z = zensim(&px, &dec, w, h);
                    let s2 = ssim2(&px, &dec, w, h);
                    let (b3, bmax) = if FAST.load(std::sync::atomic::Ordering::Relaxed)
                        && !WITH_BA.load(std::sync::atomic::Ordering::Relaxed)
                    {
                        (f64::NAN, f64::NAN)
                    } else {
                        butteraugli(&px, &dec, w, h)
                    };
                    rows.push_str(&format!(
                        "{}\t{}\t{w}\t{h}\t{color:?}\t{q}\t{}\t{}\t{z:.4}\t{s2:.4}\t{b3:.5}\t{bmax:.5}\t{}\t{bad}\t{maxerr}\n",
                        s.class,
                        s.name,
                        u8::from(on),
                        file.len(),
                        white.len()
                    ));
                }
            }
        }
        out.lock().unwrap().write_all(rows.as_bytes()).unwrap();
        let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        eprintln!("[{n}/{}] {} {} {side}", jobs.len(), s.class, s.name);
    });
}
