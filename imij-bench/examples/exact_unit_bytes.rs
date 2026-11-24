//! Compact-tier files of an imazen-26 split: bytes, whether the file has
//! exact blocks, and a hash of its decoded pixels, one row per (image, crop,
//! quality). Two builds of this example compare an exact-block layout
//! change at identical pixels (benchmarks/exact_blocks_per_unit_2026-10-11).
//!
//! Sources and crops follow `lossless_over_lossy`: RGB8 (alpha composited
//! on white, 16-bit PNGs reduced to 8 bits), sorted by id, HEIC/DNG
//! skipped, `--per-class N` at a fixed stride, center crops never
//! upscaled, a crop skipped when the source already fits the next smaller
//! size. Encodes are the default `imij::ImiEncoderConfig::lossy(q)` with the `Compact`
//! tier, one thread.
//!
//! `cargo run --release -p imij-bench --example exact_unit_bytes -- OUT.tsv
//! SPLIT [--sizes 256,512,1024] [--q 1,2,..] [--per-class N] [--dump DIR]`
//! with the corpus root in `IMAZEN26` and SPLIT `validate` or `test`.
//! `--dump DIR` writes every file with exact blocks
//! (`{class}_{image}_{w}x{h}_q{q}.imi`).
use std::collections::HashMap;
use std::io::Write;

use imij::{Channels, ImageInfo, ImiEncoderConfig, Metadata, SampleType};
use rayon::prelude::*;

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
    } else {
        use zenjpeg::decoder::{Decoder, PixelFormat};
        let d = Decoder::new()
            .output_format(PixelFormat::Rgb)
            .decode(&data, enough::Unstoppable)
            .map_err(|e| format!("{path}: {e:?}"))?;
        let (w, h) = (d.width() as usize, d.height() as usize);
        (w, h, d.pixels_u8().ok_or("not RGB8")?.to_vec())
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

/// FNV-1a, 64 bits.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)
    })
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
    let qs: Vec<f32> = opt("--q")
        .unwrap_or("1,2,3,5,7,10,13,16,20,25,30,40,50,60,70,80,90,100".into())
        .split(',')
        .map(|s| s.parse().unwrap())
        .collect();
    let per_class: Option<usize> = opt("--per-class").map(|s| s.parse().unwrap());
    let dump: Option<String> = opt("--dump");
    let (out_path, split) = (args[0].clone(), args[1].clone());
    let root = std::env::var("IMAZEN26").expect("IMAZEN26: the imazen-26 checkout");
    let manifest =
        std::fs::read_to_string(format!("{root}/manifests/split_map_family.tsv")).unwrap();
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
        by_class
            .entry(class)
            .or_default()
            .push((id.into(), format!("{root}/{path}")));
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
    let mut jobs: Vec<(usize, usize, f32)> = Vec::new();
    for i in 0..srcs.len() {
        for &s in &sizes {
            for &q in &qs {
                jobs.push((i, s, q));
            }
        }
    }
    if let Some(d) = &dump {
        std::fs::create_dir_all(d).unwrap();
    }
    let rows: Vec<String> = jobs
        .par_iter()
        .filter_map(|&(i, side, q)| {
            let s = &srcs[i];
            let (w, h, px) = crop(s, side);
            let smaller = sizes.iter().copied().filter(|&z| z < side).max();
            if smaller.is_some_and(|z| w <= z && h <= z) || w < 8 || h < 8 {
                return None;
            }
            let info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8);
            let mut cfg =
                imij::ImiEncoderConfig::lossy(q).with_decode_speed(imij::DecodeSpeed::Compact);
            cfg = cfg.with_threads(1);
            let file = cfg
                .encode_samples(imij::ImageRef::new(&px, w * 3, &info), &Metadata::default())
                .unwrap();
            let exact = imij::header(&file)
                .unwrap()
                .features
                .contains(imij::Feature::ExactBlocks);
            let (_, dec) = imij::decode(&file).unwrap();
            if exact && let Some(d) = &dump {
                let name = format!("{d}/{}_{}_{w}x{h}_q{q}.imi", s.class, s.id);
                std::fs::write(name, &file).unwrap();
            }
            Some(format!(
                "{}\t{}\t{w}\t{h}\t{q}\t{}\t{}\t{:016x}",
                s.class,
                s.id,
                file.len(),
                exact as u8,
                fnv(&dec)
            ))
        })
        .collect();
    let mut out = std::fs::File::create(&out_path).unwrap();
    writeln!(out, "class\timage\tw\th\tq\tbytes\texact\tpixels_fnv").unwrap();
    for r in rows {
        writeln!(out, "{r}").unwrap();
    }
}
