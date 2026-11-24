//! imij vs JPEG XL vs AVIF: size, quality and decode speed (see `cmp.rs`).
//!
//! ```text
//! imij-cmp enc MANIFEST OUTDIR lossless|lossy [class,filter]
//! imij-cmp dec OUTDIR THREADS(comma list) ITERS
//! imij-cmp steer-batch JOBS_JSON   (spatial steering, see steer.rs)
//! ```
//!
//! MANIFEST lines are `class<TAB>path.png`; only 8-bit RGB images are used.

mod chroma;
mod cmp;
mod coefjpeg;
mod huf;
mod jpeg_import;
mod jpegsize;
mod ladder;
mod quality;
#[cfg(test)]
mod race_tests;
mod rd;
mod ssim2map;
mod steer;

use std::path::Path;

/// An 8-bit RGB source image.
pub struct Img {
    pub name: String,
    pub info: imij::ImageInfo,
    pub pixels: Vec<u8>,
}

fn load_png(path: &Path) -> Option<Img> {
    let data = std::fs::read(path).unwrap();
    let out = zenpng::decode(
        &data,
        &zenpng::PngDecodeConfig::default(),
        &enough::Unstoppable,
    )
    .unwrap();
    let (w, h) = (out.info.width, out.info.height);
    let bytes = out.pixels.copy_to_contiguous_bytes();
    if out.info.bit_depth != 8 || bytes.len() != w as usize * h as usize * 3 {
        return None;
    }
    // IMIJ_CMP_CROP=N: centered N x N crop (sources smaller than N are skipped).
    if let Some(n) = std::env::var("IMIJ_CMP_CROP")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
    {
        if w < n || h < n {
            return None;
        }
        let (x0, y0) = ((w - n) / 2, (h - n) / 2);
        let mut c = Vec::with_capacity((n * n * 3) as usize);
        for y in y0..y0 + n {
            let o = ((y * w + x0) * 3) as usize;
            c.extend_from_slice(&bytes[o..o + (n * 3) as usize]);
        }
        return Some(Img {
            name: format!("{}_c{n}", path.file_stem().unwrap().to_string_lossy()),
            info: imij::ImageInfo::new(n, n, imij::Channels::Rgb, imij::SampleType::U8),
            pixels: c,
        });
    }
    Some(Img {
        name: path.file_stem().unwrap().to_string_lossy().into_owned(),
        info: imij::ImageInfo::new(w, h, imij::Channels::Rgb, imij::SampleType::U8),
        pixels: bytes,
    })
}

fn load_manifest(path: &str, filter: Option<&str>) -> Vec<Img> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| {
            let (class, p) = l.split_once('\t').unwrap();
            if filter.is_some_and(|f| !f.split(',').any(|x| x == class)) {
                return None;
            }
            load_png(Path::new(p))
        })
        .collect()
}

pub mod metrics {
    use crate::Img;

    fn px3(b: &[u8]) -> &[[u8; 3]] {
        b.as_chunks::<3>().0
    }

    pub fn ssim2_ref(img: &Img) -> fast_ssim2::Ssimulacra2Reference {
        let (w, h) = (img.info.width as usize, img.info.height as usize);
        fast_ssim2::Ssimulacra2Reference::new(
            &fast_ssim2::PixelSlice::new(
                &img.pixels,
                w as u32,
                h as u32,
                w * 3,
                fast_ssim2::PixelDescriptor::RGB8_SRGB,
            )
            .unwrap(),
        )
        .unwrap()
    }

    pub fn ssim2(r: &fast_ssim2::Ssimulacra2Reference, img: &Img, px: &[u8]) -> f64 {
        let (w, h) = (img.info.width as usize, img.info.height as usize);
        r.compare(
            &fast_ssim2::PixelSlice::new(
                px,
                w as u32,
                h as u32,
                w * 3,
                fast_ssim2::PixelDescriptor::RGB8_SRGB,
            )
            .unwrap(),
        )
        .unwrap()
    }

    /// zensim 0.2.7 (PreviewV0_2 profile, the published default).
    pub fn zensim(img: &Img, px: &[u8]) -> f64 {
        let (w, h) = (img.info.width as usize, img.info.height as usize);
        let z = zensim::Zensim::new(zensim::ZensimProfile::PreviewV0_2);
        z.compute(
            &zensim::RgbSlice::new(px3(&img.pixels), w, h),
            &zensim::RgbSlice::new(px3(px), w, h),
        )
        .unwrap()
        .score()
    }

    /// Butteraugli (3-norm, max-norm), default parameters.
    pub fn butteraugli(img: &Img, px: &[u8]) -> (f64, f64) {
        let (w, h) = (img.info.width as usize, img.info.height as usize);
        let to = |b: &[u8]| -> Vec<butteraugli::RGB8> {
            px3(b)
                .iter()
                .map(|c| butteraugli::RGB8::new(c[0], c[1], c[2]))
                .collect()
        };
        let (a, b) = (to(&img.pixels), to(px));
        let r = butteraugli::butteraugli(
            butteraugli::Img::new(a, w, h).as_ref(),
            butteraugli::Img::new(b, w, h).as_ref(),
            &butteraugli::ButteraugliParams::default(),
        )
        .unwrap();
        (r.pnorm_3, r.score)
    }

    pub fn imij_lossy(img: &Img, cfg: &imij::ImiEncoderConfig) -> Vec<u8> {
        cfg.encode_samples(
            imij::ImageRef::new(&img.pixels, img.info.row_bytes(), &img.info),
            &imij::Metadata::default(),
        )
        .unwrap()
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if matches!(
        a.get(1).map(String::as_str),
        Some(
            "source-rgb"
                | "decode-rgb"
                | "redecode-case"
                | "encode-rgb"
                | "score-rgb"
                | "eval-batch"
                | "score-rgb-batch"
                | "table-bytes"
        )
    ) {
        if let Err(e) = rd::command(&a) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    if a.get(1).map(String::as_str) == Some("ladder-batch") {
        if let Err(e) = ladder::command(&a) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    // `jxl-transcode FILE.jpg` prints the bytes of JPEG XL's lossless
    // transcode codestream (effort 9, as the jpeg-jxl comparator); with
    // `coef-jpeg` (coefjpeg.rs), the AQ + trellis study's coder comparison.
    if a.get(1).map(String::as_str) == Some("jpeg-import") {
        if let Err(e) = jpeg_import::import(&a[2..]) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    if a.get(1).map(String::as_str) == Some("jpeg-components-check") {
        if let Err(e) = jpeg_import::check(&a[2..]) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    if a.get(1).map(String::as_str) == Some("coef-jpeg") {
        if let Err(e) = coefjpeg::command(&a) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    if matches!(
        a.get(1).map(String::as_str),
        Some("jpegsize" | "jpegsize-scan" | "jpegsize-ppm" | "jpegsize-fallback-check")
    ) {
        if let Err(e) = jpegsize::command(&a) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    if a.get(1).map(String::as_str) == Some("jpegsize-zenenc") {
        if let Err(e) = chroma::zenenc(&a[2..]) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    if a.get(1).map(String::as_str) == Some("jpegsize-correction") {
        if let Err(e) = chroma::correction_worker(&a[2..]) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    if a.get(1).map(String::as_str) == Some("jpegsize-metrics") {
        if let Err(e) = chroma::metrics(&a[2..]) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    if a.get(1).map(String::as_str) == Some("jpegsize-crops") {
        if let Err(e) = chroma::crops_from_pixels(&a[2..]) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    if a.get(1).map(String::as_str) == Some("jpegsize-parity") {
        if let Err(e) = chroma::parity(&a[2..]) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    if a.get(1).map(String::as_str) == Some("jpegsize-source-pairs") {
        if let Err(e) = chroma::source_pairs(&a[2..]) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    if a.get(1).map(String::as_str) == Some("jpegsize-synthetic") {
        if let Err(e) = chroma::synthetic(&a[2..]) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    if a.get(1).map(String::as_str) == Some("jpegsize-chroma") {
        if let Err(e) = chroma::run(&a[2..]) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    if a.get(1).map(String::as_str) == Some("jxl-transcode") {
        let r = (|| -> rd::Result<usize> {
            let jpeg = std::fs::read(a.get(2).ok_or("jxl-transcode FILE.jpg")?)?;
            Ok(jxl_encoder::LosslessConfig::new()
                .with_effort(9)
                .encode_jpeg_transcode_codestream(&jpeg)?
                .len())
        })();
        match r {
            Ok(n) => println!("{n}"),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return;
    }
    if let Some(cmd @ ("steer-batch" | "aq-cost")) = a.get(1).map(String::as_str) {
        let r = if cmd == "aq-cost" {
            steer::aq_cost(&a)
        } else {
            steer::command(&a)
        };
        if let Err(e) = r {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    match a.get(1).map(|s| s.as_str()) {
        Some("enc") => cmp::cmd_enc(
            &load_manifest(&a[2], a.get(5).map(|s| s.as_str())),
            &a[3],
            &a[4],
        ),
        Some("dec") => cmp::cmd_dec(
            &a[2],
            &a[3]
                .split(',')
                .map(|v| v.parse().unwrap())
                .collect::<Vec<usize>>(),
            a[4].parse().unwrap(),
        ),
        // huf ITERS FILES...: interleaved Huffman decode prototype.
        Some("huf") => huf::cmd(&a[3..], a[2].parse().unwrap()),
        _ => {
            eprintln!(
                "imij-cmp enc MANIFEST OUTDIR lossless|lossy [classes]\n\
                 imij-cmp dec OUTDIR THREADS ITERS"
            );
            std::process::exit(2);
        }
    }
}
