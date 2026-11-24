//! Lossless 8x8 blocks over lossy: an estimate without a format change.
//!
//! For each RGB8 source of an imazen-26 split (center crops 256/512/1024,
//! never upscaled), XYB lossy at each quality: every block's run-level AC
//! bits (`imij::lab::encode_lossy_blocks`), its distortion (MSE, RGB 8-bit)
//! and a lossless-cost proxy; policies pick masks of blocks to code exactly.
//! Every mask is then measured with the real coders:
//!
//! * REPLACE: the lossy file with the masked blocks' AC dropped (DC kept),
//!   plus the masked blocks' source pixels as a lossless sub-image, plus the
//!   mask (a bw x bh lossless gray image).
//!
//! It decodes to the lossy reconstruction with the masked blocks exact. (A
//! RESIDUAL variant, the masked blocks' residual over the unchanged lossy
//! file, lost on every class on VALIDATE and was removed.) The sub-image packs each block row's masked blocks left to
//! right into an 8-pixel strip, strips stacked, zero padding: the layout an
//! overwrite pass after each lossy stripe could decode with the existing
//! lossless coder. Each sub-image and the mask are separate files here, so
//! their container headers are counted (not an upper bound: the lab
//! prototype's single file is measured separately, `proto_bytes`).
//!
//! Policies (`--policies`): `free` (proxy bits <= AC bits), `ratio<K>`
//! (proxy <= K * AC bits), `lam<L>` (proxy
//! <= AC + L * MSE), `region<L>` (4-connected components of blocks with at
//! most four source colors, at least 4 blocks, taken whole when the
//! component's proxy <= AC + L * MSE), plus the baseline `none`. `free`
//! and `ratio<K>` also encode the single-file format (`proto_bytes`; its
//! decode is scored by the estimate's mask, and `proto_mask_diff` counts the
//! blocks where the two differ). `default`: the encoder's own exact blocks
//! for the configuration (`--speed compact|balanced|fast|fastest`, else no
//! tier), measured as that file against the same configuration without
//! exact blocks; with `--speed`, `default` is the only policy (the per-block
//! estimate needs run-level coding without a tier).
//!
//! `cargo run --release --example lossless_over_lossy -- OUT.tsv SPLIT
//! [--sizes 256,512,1024] [--q 1,2,..] [--policies free,lam2,region0]
//! [--per-class N] [--classes screen,document,..] [--cvvdp] [--dump DIR]
//! [--diag] [--speed compact] [--encode-time]`
//! `--encode-time`: instead of the RD rows, one-thread encode time (best of
//! 3, images one after another) with exact blocks off and at the default,
//! for no tier and Compact, and the files' sizes.
//! with SPLIT `validate` or `test` (`imazen-26/manifests/split_map_family.tsv`).
use std::collections::HashMap;
use std::io::Write;
use std::sync::Mutex;

use imij::{Channels, ImageInfo, ImiEncoderConfig, Metadata, SampleType};
use rayon::prelude::*;

#[allow(dead_code)]
#[path = "../../benchmarks/perceptual.rs"]
mod perceptual;

/// The imazen-26 checkout: `IMAZEN26`, else imazen-26/ on the dev host.
fn root() -> String {
    std::env::var("IMAZEN26").unwrap_or_else(|_| "imazen-26/".into())
}

fn class_of(dir: &str) -> Option<&'static str> {
    Some(match &dir[..4] {
        "8000" | "8100" => "screen",
        "5000" | "5200" | "5300" => "document",
        "6000" | "6600" | "6800" => "scan",
        "7000" => "plot",
        "9000" | "9094" => "clipart",
        "1000" | "1200" | "1400" | "1600" | "2000" | "2200" | "2400" | "3000" | "3300" => "photo",
        _ => return None,
    })
}

struct Src {
    class: &'static str,
    id: String,
    w: usize,
    h: usize,
    rgb: Vec<u8>,
}

fn load(class: &'static str, id: &str, path: &str) -> Result<Src, String> {
    let data = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let lower = path.to_ascii_lowercase();
    let (w, h, rgb) = if lower.ends_with(".png") {
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
                rgb.push(((v * a + 255 * (255 - a) + 127) / 255) as u8);
            }
        }
        (w, h, rgb)
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        use zenjpeg::decoder::{Decoder, PixelFormat};
        let d = Decoder::new()
            .output_format(PixelFormat::Rgb)
            .decode(&data, enough::Unstoppable)
            .map_err(|e| format!("{path}: {e:?}"))?;
        let (w, h) = (d.width() as usize, d.height() as usize);
        (w, h, d.pixels_u8().ok_or("not RGB8")?.to_vec())
    } else {
        return Err(format!("{path}: unsupported format"));
    };
    Ok(Src {
        class,
        id: id.into(),
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

/// Per-block statistics of one lossy encode.
struct Blocks {
    bw: usize,
    bh: usize,
    ac_bits: Vec<f32>,
    mse: Vec<f64>,
    proxy: Vec<f64>,
    colors: Vec<u32>,
}

fn block_iter(w: usize, h: usize, bx: usize, by: usize) -> impl Iterator<Item = (usize, usize)> {
    (by * 8..(by * 8 + 8).min(h))
        .flat_map(move |y| (bx * 8..(bx * 8 + 8).min(w)).map(move |x| (x, y)))
}

/// Lossless proxy: G, R-G, B-G per pixel, MED prediction inside the block
/// (unavailable neighbours outside it, as in the packed sub-image), coded
/// at -log2 p under the image's residual histogram (add-one smoothing).
fn proxy_bits(src: &[u8], w: usize, h: usize, bw: usize, bh: usize) -> Vec<f64> {
    let chan = |x: usize, y: usize, c: usize| -> i32 {
        let p = &src[(y * w + x) * 3..][..3];
        let g = p[1] as i32;
        match c {
            0 => g,
            1 => p[0] as i32 - g,
            _ => p[2] as i32 - g,
        }
    };
    let mut res: Vec<Vec<i32>> = Vec::with_capacity(bw * bh);
    let mut hist = vec![[1u64; 1024]; 3];
    for by in 0..bh {
        for bx in 0..bw {
            let mut r = Vec::with_capacity(192);
            for (x, y) in block_iter(w, h, bx, by) {
                for (c, hc) in hist.iter_mut().enumerate() {
                    let (inl, inu) = (x > bx * 8, y > by * 8);
                    let a = if inl { chan(x - 1, y, c) } else { 0 };
                    let b = if inu { chan(x, y - 1, c) } else { 0 };
                    let cc = if inl && inu { chan(x - 1, y - 1, c) } else { 0 };
                    let pred = match (inl, inu) {
                        (true, true) => {
                            if cc >= a.max(b) {
                                a.min(b)
                            } else if cc <= a.min(b) {
                                a.max(b)
                            } else {
                                a + b - cc
                            }
                        }
                        (true, false) => a,
                        (false, true) => b,
                        _ => {
                            if c == 0 {
                                128
                            } else {
                                0
                            }
                        }
                    };
                    let e = chan(x, y, c) - pred;
                    hc[(e + 512) as usize] += 1;
                    r.push(e);
                }
            }
            res.push(r);
        }
    }
    let tot: Vec<f64> = hist.iter().map(|h| h.iter().sum::<u64>() as f64).collect();
    res.iter()
        .map(|r| {
            r.iter()
                .enumerate()
                .map(|(i, &e)| {
                    let c = i % 3;
                    (tot[c] / hist[c][(e + 512) as usize] as f64).log2()
                })
                .sum()
        })
        .collect()
}

fn blocks(src: &[u8], rec: &[u8], w: usize, h: usize, ac_bits: Vec<f32>) -> Blocks {
    let (bw, bh) = (w.div_ceil(8), h.div_ceil(8));
    let mut mse = vec![0.0; bw * bh];
    let mut colors = vec![0u32; bw * bh];
    for by in 0..bh {
        for bx in 0..bw {
            let (mut se, mut n) = (0f64, 0usize);
            let mut set: Vec<[u8; 3]> = Vec::new();
            for (x, y) in block_iter(w, h, bx, by) {
                let o = (y * w + x) * 3;
                for c in 0..3 {
                    let d = src[o + c] as f64 - rec[o + c] as f64;
                    se += d * d;
                }
                n += 3;
                let p = [src[o], src[o + 1], src[o + 2]];
                if set.len() <= 8 && !set.contains(&p) {
                    set.push(p);
                }
            }
            mse[by * bw + bx] = se / n as f64;
            colors[by * bw + bx] = set.len() as u32;
        }
    }
    let proxy = proxy_bits(src, w, h, bw, bh);
    Blocks {
        bw,
        bh,
        ac_bits,
        mse,
        proxy,
        colors,
    }
}

fn policy_mask(p: &str, b: &Blocks) -> Vec<bool> {
    let n = b.bw * b.bh;
    let gain = |i: usize, lam: f64| b.proxy[i] - b.ac_bits[i] as f64 - lam * b.mse[i];
    if p == "free" {
        return (0..n).map(|i| gain(i, 0.0) <= 0.0).collect();
    }
    if let Some(k) = p.strip_prefix("ratio") {
        // Quality-adaptive: lossless at most k times the block's lossy AC bits.
        let k: f64 = k.parse().unwrap();
        return (0..n)
            .map(|i| b.proxy[i] <= k * b.ac_bits[i] as f64)
            .collect();
    }
    if let Some(l) = p.strip_prefix("lam") {
        let lam: f64 = l.parse().unwrap();
        return (0..n).map(|i| gain(i, lam) <= 0.0).collect();
    }
    if let Some(l) = p.strip_prefix("region") {
        let lam: f64 = l.parse().unwrap();
        let cand: Vec<bool> = (0..n).map(|i| b.colors[i] <= 4).collect();
        let mut mask = vec![false; n];
        let mut seen = vec![false; n];
        for s in 0..n {
            if !cand[s] || seen[s] {
                continue;
            }
            let mut comp = vec![s];
            seen[s] = true;
            let mut k = 0;
            while k < comp.len() {
                let i = comp[k];
                k += 1;
                let (x, y) = (i % b.bw, i / b.bw);
                let nb = [
                    (x > 0).then(|| i - 1),
                    (x + 1 < b.bw).then(|| i + 1),
                    (y > 0).then(|| i - b.bw),
                    (y + 1 < b.bh).then(|| i + b.bw),
                ];
                for j in nb.into_iter().flatten() {
                    if cand[j] && !seen[j] {
                        seen[j] = true;
                        comp.push(j);
                    }
                }
            }
            if comp.len() >= 4 && comp.iter().map(|&i| gain(i, lam)).sum::<f64>() <= 0.0 {
                for &i in &comp {
                    mask[i] = true;
                }
            }
        }
        return mask;
    }
    panic!("unknown policy {p}");
}

/// Masked blocks packed per block row (8-pixel strips, zero padded).
/// `value(x, y, c)` gives the sample; returns (width, height, samples).
fn pack(
    mask: &[bool],
    bw: usize,
    bh: usize,
    w: usize,
    h: usize,
    value: impl Fn(usize, usize, usize) -> u16,
) -> (usize, usize, Vec<u16>) {
    let rows: Vec<Vec<usize>> = (0..bh)
        .map(|by| (0..bw).filter(|&bx| mask[by * bw + bx]).collect())
        .filter(|r: &Vec<usize>| !r.is_empty())
        .collect();
    let pw = 8 * rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let ph = 8 * rows.len();
    let mut out = vec![0u16; pw * ph * 3];
    let mut by_of = Vec::new();
    for by in 0..bh {
        if (0..bw).any(|bx| mask[by * bw + bx]) {
            by_of.push(by);
        }
    }
    for (ri, (r, &by)) in rows.iter().zip(&by_of).enumerate() {
        for (k, &bx) in r.iter().enumerate() {
            for dy in 0..8 {
                for dx in 0..8 {
                    let (x, y) = ((bx * 8 + dx).min(w - 1), (by * 8 + dy).min(h - 1));
                    for c in 0..3 {
                        out[((ri * 8 + dy) * pw + k * 8 + dx) * 3 + c] = value(x, y, c);
                    }
                }
            }
        }
    }
    (pw, ph, out)
}

/// The lab prototype's exact-block parts, measured separately (8 block rows
/// per lossy stripe, the default for these crop widths): per-stripe mask
/// records (varint run count + run lengths, plus each record's 1-byte size
/// varint), the nested packed image (all block rows of each stripe that has
/// an exact block, 64-row stripes), stripes with exact blocks, block rows
/// with exact blocks.
fn proto_parts(
    px: &[u8],
    w: usize,
    h: usize,
    mask: &[bool],
    bw: usize,
    bh: usize,
) -> (usize, usize, usize, usize) {
    let sb = 8;
    let varint = |v: usize| (usize::BITS - v.max(1).leading_zeros()).div_ceil(7) as usize;
    let count = |by: usize| (0..bw).filter(|&bx| mask[by * bw + bx]).count();
    let pw = 8 * (0..bh).map(count).max().unwrap_or(0).max(1);
    let nst = bh.div_ceil(sb);
    let (mut runs_bytes, mut used) = (0, Vec::new());
    for s in 0..nst {
        let (b0, b1) = (s * sb * bw, ((s + 1) * sb).min(bh) * bw);
        let mut runs = Vec::new();
        let (mut cur, mut len) = (false, 0);
        for &m in &mask[b0..b1] {
            if m != cur {
                runs.push(len);
                (cur, len) = (m, 0);
            }
            len += 1;
        }
        runs.push(len);
        if runs.len() > 1 {
            runs_bytes += 1 + varint(runs.len()) + runs.iter().map(|&r| varint(r)).sum::<usize>();
            used.push(s);
        } else {
            runs_bytes += 1;
        }
    }
    let ph = 8 * sb * used.len();
    let mut img = vec![0u8; pw * ph * 3];
    for (j, &s) in used.iter().enumerate() {
        for (r, by) in (s * sb..((s + 1) * sb).min(bh)).enumerate() {
            for (k, bx) in (0..bw).filter(|&bx| mask[by * bw + bx]).enumerate() {
                for (dy, dx) in (0..64).map(|i| (i / 8, i % 8)) {
                    let (x, y) = ((bx * 8 + dx).min(w - 1), (by * 8 + dy).min(h - 1));
                    let o = ((j * 8 * sb + r * 8 + dy) * pw + k * 8 + dx) * 3;
                    img[o..o + 3].copy_from_slice(&px[(y * w + x) * 3..][..3]);
                }
            }
        }
    }
    let info = ImageInfo::new(pw as u32, ph as u32, Channels::Rgb, SampleType::U8);
    let cfg = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.with_threads(1);
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some((8 * sb) as u32);
        });
        cfg = cfg.with_column_groups(Some(0));
        cfg
    };
    let ll = cfg
        .encode_samples(
            imij::ImageRef::new(&img, pw * 3, &info),
            &Metadata::default(),
        )
        .expect("lossless encode")
        .len();
    let rows_used = (0..bh).filter(|&by| count(by) > 0).count();
    (runs_bytes, ll, used.len(), rows_used)
}

fn lossless_bytes(w: usize, h: usize, ch: Channels, sample: SampleType, samples: &[u8]) -> usize {
    if w == 0 || h == 0 {
        return 0;
    }
    let info = ImageInfo::new(w as u32, h as u32, ch, sample);
    let cfg = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.with_threads(1);
        cfg
    };
    cfg.encode_samples(
        imij::ImageRef::new(samples, info.row_bytes(), &info),
        &Metadata::default(),
    )
    .expect("lossless encode")
    .len()
}

struct Metrics {
    zensim: f64,
    ssim2: f64,
    ba_p3: f64,
    ba_max: f64,
    cvvdp: f64,
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

fn metrics(src: &[u8], dec: &[u8], w: usize, h: usize, cv: Option<&mut cvvdp::Cvvdp>) -> Metrics {
    let px = |v: &[u8]| -> Vec<[u8; 3]> { v.as_chunks::<3>().0.to_vec() };
    let (a, b) = (px(src), px(dec));
    let zensim = zensim::Zensim::new(zensim::ZensimProfile::codec_target())
        .compute(
            &zensim::RgbSlice::new(&a, w, h),
            &zensim::RgbSlice::new(&b, w, h),
        )
        .map(|r| r.score())
        .unwrap_or(f64::NAN);
    let ssim2 = fast_ssim2::Ssimulacra2Reference::new(&slice(src, w, h))
        .ok()
        .and_then(|r| r.compare(&slice(dec, w, h)).ok())
        .unwrap_or(f64::NAN);
    let to = |v: &[u8]| -> Vec<butteraugli::RGB8> {
        v.as_chunks::<3>()
            .0
            .iter()
            .map(|c| butteraugli::RGB8::new(c[0], c[1], c[2]))
            .collect()
    };
    let (ia, ib) = (
        butteraugli::Img::new(to(src), w, h),
        butteraugli::Img::new(to(dec), w, h),
    );
    let (ba_p3, ba_max) = match butteraugli::butteraugli(
        ia.as_ref(),
        ib.as_ref(),
        &butteraugli::ButteraugliParams::default(),
    ) {
        Ok(r) => (r.pnorm_3, r.score),
        Err(_) => (f64::NAN, f64::NAN),
    };
    let cvvdp = cv
        .and_then(|c| c.score_with_warm_ref(dec).ok())
        .map_or(f64::NAN, |v| v as f64);
    Metrics {
        zensim,
        ssim2,
        ba_p3,
        ba_max,
        cvvdp,
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut opt = |name: &str| -> Option<String> {
        let i = args.iter().position(|a| a == name)?;
        let v = args.get(i + 1).cloned();
        args.drain(i..(i + 2).min(args.len()));
        v
    };
    let sizes: Vec<usize> = opt("--sizes")
        .unwrap_or("256,512,1024".into())
        .split(',')
        .map(|s| s.parse().unwrap())
        .collect();
    let qs: Vec<f32> = match opt("--q") {
        Some(s) => s.split(',').map(|s| s.parse().unwrap()).collect(),
        None => (1..=30)
            .chain((35..=100).step_by(5))
            .map(|q| q as f32)
            .collect(),
    };
    let policies: Vec<String> = opt("--policies")
        .unwrap_or("free,lam2,lam8,region0,region8".into())
        .split(',')
        .map(String::from)
        .collect();
    let per_class: Option<usize> = opt("--per-class").map(|s| s.parse().unwrap());
    let classes: Option<Vec<String>> =
        opt("--classes").map(|s| s.split(',').map(String::from).collect());
    // `--dump DIR`: write the plain lossy file and each prototype file
    // (`{class}_{image}_{w}x{h}_q{q}_{policy}.imi`, policy `none` = plain).
    let dump: Option<String> = opt("--dump");
    let speed = opt("--speed").map(|v| match v.as_str() {
        "compact" => imij::DecodeSpeed::Compact,
        "balanced" => imij::DecodeSpeed::Balanced,
        "fast" => imij::DecodeSpeed::Fast,
        "fastest" => imij::DecodeSpeed::Fastest,
        _ => panic!("unknown --speed {v}"),
    });
    assert!(
        speed.is_none() || policies.iter().all(|p| p == "default"),
        "--speed takes only the `default` policy"
    );
    // `--diag`: per prototype row, parts of the first prototype layout
    // measured separately (OUT.diag.tsv): the nested image in stripes
    // aligned to the lossy stripes and the mask as per-stripe varint runs
    // (both since replaced: block list, DC block codes).
    let diag = if let Some(i) = args.iter().position(|a| a == "--diag") {
        args.remove(i);
        true
    } else {
        false
    };
    let encode_time = if let Some(i) = args.iter().position(|a| a == "--encode-time") {
        args.remove(i);
        true
    } else {
        false
    };
    let use_cvvdp = if let Some(i) = args.iter().position(|a| a == "--cvvdp") {
        args.remove(i);
        true
    } else {
        false
    };
    let (out_path, split) = (args[0].clone(), args[1].clone());
    // Sources of the split, sorted by id, HEIC/DNG skipped; `--per-class N`
    // takes N at a fixed stride through each class.
    let manifest =
        std::fs::read_to_string(format!("{}/manifests/split_map_family.tsv", root())).unwrap();
    let mut by_class: HashMap<&'static str, Vec<(String, String)>> = HashMap::new();
    for line in manifest.lines().skip(1) {
        let f: Vec<&str> = line.split('\t').collect();
        let (id, sp, path) = (f[0], f[1], f[3]);
        let Some(class) = class_of(path) else {
            continue;
        };
        let lower = path.to_ascii_lowercase();
        if sp != split
            || !(lower.ends_with(".png") || lower.ends_with(".jpg") || lower.ends_with(".jpeg"))
        {
            continue;
        }
        if classes
            .as_ref()
            .is_some_and(|c| !c.iter().any(|c| c == class))
        {
            continue;
        }
        by_class
            .entry(class)
            .or_default()
            .push((id.into(), format!("{}/{path}", root())));
    }
    let mut picked = Vec::new();
    for (class, mut v) in by_class {
        v.sort();
        if let Some(n) = per_class.filter(|&n| n < v.len()) {
            let step = v.len() as f64 / n as f64;
            v = (0..n)
                .map(|i| v[(i as f64 * step) as usize].clone())
                .collect();
        }
        picked.extend(v.into_iter().map(|(id, p)| (class, id, p)));
    }
    picked.sort();
    let srcs: Vec<Src> = picked
        .par_iter()
        .filter_map(|(c, id, p)| load(c, id, p).map_err(|e| eprintln!("skip {e}")).ok())
        .collect();
    let jobs: Vec<(usize, usize)> = (0..srcs.len())
        .flat_map(|i| sizes.iter().map(move |&s| (i, s)))
        .collect();
    if encode_time {
        let mut out = std::fs::File::create(&out_path).unwrap();
        writeln!(
            out,
            "class\timage\tw\th\tq\tspeed\toff_ms\ton_ms\toff_bytes\ton_bytes"
        )
        .unwrap();
        for &(i, side) in &jobs {
            let s = &srcs[i];
            let (w, h, px) = crop(s, side);
            let smaller = sizes.iter().copied().filter(|&z| z < side).max();
            if smaller.is_some_and(|z| w <= z && h <= z) || w < 8 || h < 8 {
                continue;
            }
            let info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8);
            for &q in &qs {
                for sp in [None, Some(imij::DecodeSpeed::Compact)] {
                    let mut cfg = imij::ImiEncoderConfig::lossy(q);
                    // `IMIJ_ENC_THREADS`: encoder threads (default 1).
                    cfg = cfg.with_threads(std::env::var("IMIJ_ENC_THREADS")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(1));
                    if let Some(sp) = sp {
                        cfg = cfg.with_decode_speed(sp);
                    }
                    let time = |exact: Option<f32>| {
                        let mut c = cfg.clone();
                        c = c.tune_lossy(|t| { t.exact_blocks = exact; });
                        let mut best = f64::MAX;
                        let mut len = 0;
                        for _ in 0..3 {
                            let t = std::time::Instant::now();
                            let f = c
                                .encode_samples(
                                    imij::ImageRef::new(&px, w * 3, &info),
                                    &Metadata::default(),
                                )
                                .unwrap();
                            best = best.min(t.elapsed().as_secs_f64() * 1e3);
                            len = f.len();
                        }
                        (best, len)
                    };
                    let (off, on) = (time(Some(0.0)), time(None));
                    writeln!(
                        out,
                        "{}\t{}\t{w}\t{h}\t{q}\t{}\t{:.3}\t{:.3}\t{}\t{}",
                        s.class,
                        s.id,
                        if sp.is_some() { "compact" } else { "none" },
                        off.0,
                        on.0,
                        off.1,
                        on.1
                    )
                    .unwrap();
                }
            }
            eprintln!("{} {} {side}", s.class, s.id);
        }
        return;
    }
    let out = Mutex::new(std::fs::File::create(&out_path).unwrap());
    writeln!(
        out.lock().unwrap(),
        "class\timage\tw\th\tq\tpolicy\tblocks\tmasked\tlossy_bytes\tlossy_replace_bytes\tll_bytes\tmask_bytes\treplace_total\tac_bits_masked\tproxy_bits_masked\tzensim\tssim2\tba_p3\tba_max\tcvvdp\tproto_bytes\tproto_mask_diff"
    )
    .unwrap();
    let diag_out = diag.then(|| {
        let mut f = std::fs::File::create(format!("{out_path}.diag.tsv")).unwrap();
        writeln!(f, "class\timage\tw\th\tq\tpolicy\tmasked\tll_bytes\tmask_bytes\truns_bytes\tll_stripe_bytes\tstripes_used\tblock_rows_used").unwrap();
        Mutex::new(f)
    });
    let done = std::sync::atomic::AtomicUsize::new(0);
    jobs.par_iter().for_each(|&(i, side)| {
        let s = &srcs[i];
        let (w, h, px) = crop(s, side);
        let smaller = sizes.iter().copied().filter(|&z| z < side).max();
        if smaller.is_some_and(|z| w <= z && h <= z) || w < 8 || h < 8 {
            return;
        }
        let info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8);
        let mut cv = use_cvvdp.then(|| {
            let mut c = perceptual::cvvdp_fhd(w as u32, h as u32).unwrap();
            c.warm_reference(&px).unwrap();
            c
        });
        let mut rows = String::new();
        for &q in &qs {
            // The plain lossy file: the configuration without exact blocks.
            let mut cfg = imij::ImiEncoderConfig::lossy(q);
            if let Some(sp) = speed {
                cfg = cfg.with_decode_speed(sp);
            }
            cfg = cfg.tune_lossy(|t| { t.exact_blocks = Some(0.0); });
            let (file, bits) = match speed {
                None => imij::lab::encode_lossy_blocks(&px, w * 3, &info, &cfg, None).unwrap(),
                Some(_) => (
                    cfg.encode_samples(imij::ImageRef::new(&px, w * 3, &info), &Metadata::default()).unwrap(),
                    vec![0.0; w.div_ceil(8) * h.div_ceil(8)],
                ),
            };
            let rec = imij::decode(&file).unwrap().1;
            let b = blocks(&px, &rec, w, h, bits);
            let n = b.bw * b.bh;
            let base = metrics(&px, &rec, w, h, cv.as_mut());
            let row = |rows: &mut String, pol: &str, masked: usize, lr: usize, ll: usize, mk: usize, ac: f64, pr: f64, m: &Metrics, proto: Option<(usize, usize)>| {
                let (proto, diff) = proto.map_or(("NA".to_string(), "NA".to_string()), |(b, d)| (b.to_string(), d.to_string()));
                rows.push_str(&format!(
                    "{}\t{}\t{w}\t{h}\t{q}\t{pol}\t{n}\t{masked}\t{}\t{lr}\t{ll}\t{mk}\t{}\t{ac:.0}\t{pr:.0}\t{:.4}\t{:.4}\t{:.5}\t{:.5}\t{:.4}\t{proto}\t{diff}\n",
                    s.class, s.id, file.len(), lr + ll + mk, m.zensim, m.ssim2, m.ba_p3, m.ba_max, m.cvvdp
                ));
            };
            row(&mut rows, "none", 0, file.len(), 0, 0, 0.0, 0.0, &base, Some((file.len(), 0)));
            if let Some(d) = &dump {
                std::fs::write(format!("{d}/{}_{}_{w}x{h}_q{q}_none.imi", s.class, s.id), &file).unwrap();
            }
            // Blocks of `got` equal to the source and not to the plain decode.
            let exact_in = |got: &[u8]| -> usize {
                (0..n)
                    .filter(|&bi| {
                        let it = || block_iter(w, h, bi % b.bw, bi / b.bw);
                        let eq = |a: &[u8]| it().all(|(x, y)| a[(y * w + x) * 3..][..3] == got[(y * w + x) * 3..][..3]);
                        eq(&px) && !eq(&rec)
                    })
                    .count()
            };
            let mut seen: HashMap<Vec<bool>, String> = HashMap::new();
            for p in &policies {
                if p == "default" {
                    let mut c = cfg.clone();
                    c = c.tune_lossy(|t| { t.exact_blocks = None; });
                    let f = c.encode_samples(imij::ImageRef::new(&px, w * 3, &info), &Metadata::default()).unwrap();
                    let got = imij::decode(&f).unwrap().1;
                    let m = metrics(&px, &got, w, h, cv.as_mut());
                    if let Some(d) = &dump {
                        std::fs::write(format!("{d}/{}_{}_{w}x{h}_q{q}_{p}.imi", s.class, s.id), &f).unwrap();
                    }
                    row(&mut rows, p, exact_in(&got), f.len(), 0, 0, 0.0, 0.0, &m, Some((f.len(), 0)));
                    continue;
                }
                let mask = policy_mask(p, &b);
                let masked = mask.iter().filter(|&&m| m).count();
                if masked == 0 {
                    row(&mut rows, p, 0, file.len(), 0, 0, 0.0, 0.0, &base, Some((file.len(), 0)));
                    continue;
                }
                if let Some(prev) = seen.get(&mask) {
                    // Same mask as an earlier policy: copy its row.
                    let f: Vec<&str> = prev.split('\t').collect();
                    let mut f: Vec<String> = f.iter().map(|s| s.to_string()).collect();
                    f[5] = p.clone();
                    rows.push_str(&f.join("\t"));
                    continue;
                }
                let (rep, _) = imij::lab::encode_lossy_blocks(&px, w * 3, &info, &cfg, Some(&mask)).unwrap();
                let (pw, ph, orig) = pack(&mask, b.bw, b.bh, w, h, |x, y, c| px[(y * w + x) * 3 + c] as u16);
                let ll = lossless_bytes(pw, ph, Channels::Rgb, SampleType::U8, &orig.iter().map(|&v| v as u8).collect::<Vec<_>>());
                let mk = lossless_bytes(b.bw, b.bh, Channels::Gray, SampleType::U8, &mask.iter().map(|&m| m as u8).collect::<Vec<_>>());
                let mut comp = rec.clone();
                for (bi, _) in mask.iter().enumerate().filter(|(_, m)| **m) {
                    for (x, y) in block_iter(w, h, bi % b.bw, bi / b.bw) {
                        let o = (y * w + x) * 3;
                        comp[o..o + 3].copy_from_slice(&px[o..o + 3]);
                    }
                }
                // Ratio policies also encode the single-file format with the
                // encoder's own mask for the same ratio (the tier's nested
                // stripe height, 16 rows without one, unless IMIJ_EXACT_ROWS
                // says otherwise).
                let ratio = if p == "free" {
                    Some(1.0f32)
                } else {
                    p.strip_prefix("ratio").map(|k| k.parse::<f32>().unwrap())
                };
                let proto = ratio.map(|k| {
                    let f = imij::lab::encode_lossy_exact(&px, w * 3, &info, &cfg, k).unwrap();
                    let got = imij::decode(&f).unwrap().1;
                    let diff = (0..n)
                        .filter(|&bi| {
                            block_iter(w, h, bi % b.bw, bi / b.bw)
                                .any(|(x, y)| got[(y * w + x) * 3..][..3] != comp[(y * w + x) * 3..][..3])
                        })
                        .count();
                    if let Some(d) = &dump {
                        std::fs::write(format!("{d}/{}_{}_{w}x{h}_q{q}_{p}.imi", s.class, s.id), &f)
                            .unwrap();
                    }
                    (f.len(), diff)
                });
                if let (Some(d), Some(_)) = (&diag_out, proto) {
                    let (rb, lsb, su, ru) = proto_parts(&px, w, h, &mask, b.bw, b.bh);
                    writeln!(
                        d.lock().unwrap(),
                        "{}\t{}\t{w}\t{h}\t{q}\t{p}\t{masked}\t{ll}\t{mk}\t{rb}\t{lsb}\t{su}\t{ru}",
                        s.class, s.id
                    )
                    .unwrap();
                }
                let m = metrics(&px, &comp, w, h, cv.as_mut());
                let ac: f64 = (0..n).filter(|&i| mask[i]).map(|i| b.ac_bits[i] as f64).sum();
                let pr: f64 = (0..n).filter(|&i| mask[i]).map(|i| b.proxy[i]).sum();
                let mut one = String::new();
                row(&mut one, p, masked, rep.len(), ll, mk, ac, pr, &m, proto);
                seen.insert(mask, one.clone());
                rows.push_str(&one);
            }
        }
        out.lock().unwrap().write_all(rows.as_bytes()).unwrap();
        let k = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        eprintln!("[{k}/{}] {} {} {side}", jobs.len(), s.class, s.id);
    });
}
