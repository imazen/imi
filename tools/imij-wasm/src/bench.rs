//! Deterministic browser timing streams: 256², 1024², 2048² and 4096², RGB8,
//! lossless and lossy q80 (plus explicit split and split-Huffman streams).
//! Written with the native encoder (the browser package decodes only) and
//! served to the Playwright timing harness
//! (`tools/imij-wasm/web/tests/benchmark.spec.ts`) — the same streams on every
//! host and browser, so the numbers compare.
//!
//! usage: imij-wasm bench-fixtures OUTDIR

use imij::{Channels, ImageInfo, Metadata, SampleType};
use std::{fs, path::Path};

fn pattern(info: &ImageInfo) -> Vec<u8> {
    // Smooth gradients (compression-realistic) plus a deterministic hash
    // texture; a low-amplitude third channel keeps the lossy path from
    // quantizing to flat blocks.
    let (w, h) = (info.width, info.height);
    let mut out = Vec::with_capacity(info.row_bytes() * info.height as usize);
    for y in 0..h {
        for x in 0..w {
            let r = (x * 255 / w.max(1) + y / 4) & 0xff;
            let g = (x / 3 + 255 - y * 255 / h.max(1)) & 0xff;
            let hash = (x.wrapping_mul(0x9e3779b9) ^ y.wrapping_mul(0x85ebca6b)) >> 24;
            let b = (((x + y) / 2 + hash) & 0x7f) & 0xff;
            out.extend_from_slice(&[r as u8, g as u8, b as u8]);
        }
    }
    out
}

fn threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(8)
}

pub fn emit(argv: &[String]) -> Result<(), String> {
    let dir = Path::new(
        argv.first()
            .ok_or("usage: imij-wasm bench-fixtures OUTDIR")?,
    );
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut manifest = String::from("name\twidth\theight\tmode\n");
    let t = threads();
    // The split-revisit study (2026-10-10): 256 px and 4096 px join the 1 MP / 4 MP pair
    // so the split/split-Huffman rows cover the evaluation sizes (256/1024/4096).
    for (label, w, h) in [
        ("256", 256u32, 256u32),
        ("1mp", 1024, 1024),
        ("4mp", 2048, 2048),
        ("4096", 4096, 4096),
    ] {
        let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
        let px = pattern(&info);
        let lossless = {
            let cfg = {
                let mut cfg = imij::ImiEncoderConfig::default();
                cfg = cfg.with_threads(t);
                cfg
            };
            cfg.encode_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .map_err(|e| format!("lossless {w}x{h}: {e}"))?
        };
        let lossy = {
            let mut cfg = imij::ImiEncoderConfig::lossy(80.0);
            cfg = cfg.with_threads(t);
            cfg.encode_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .map_err(|e| format!("lossy {w}x{h}: {e}"))?
        };
        // Entropy modes the wasm tier-parity kernels cover: the default
        // encode picks run-level or split-Huffman by a cost model, so an
        // explicit SplitHuffman stream is what the portable unpack kernels
        // (and their A/Bs) actually exercise.
        let explicit = |entropy: imij::LossyEntropy, what: &str| -> Result<Vec<u8>, String> {
            let mut cfg = imij::ImiEncoderConfig::lossy(80.0).tune_lossy(|t| t.entropy = entropy);
            cfg = cfg.with_threads(t);
            cfg.encode_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .map_err(|e| format!("{what} {w}x{h}: {e}"))
        };
        let lossy_huf = explicit(imij::LossyEntropy::SplitHuf, "lossy-splithuf")?;
        for (mode, bytes) in [
            ("lossless", lossless),
            ("lossy", lossy),
            ("lossy-splithuf", lossy_huf),
        ] {
            let name = format!("{label}-{mode}");
            fs::write(dir.join(format!("{name}.imi")), &bytes)
                .map_err(|e| format!("{name}.imi: {e}"))?;
            manifest.push_str(&format!("{name}\t{w}\t{h}\t{mode}\n"));
            eprintln!("{name}: {} bytes", bytes.len());
        }
    }
    fs::write(dir.join("manifest.tsv"), manifest).map_err(|e| format!("manifest.tsv: {e}"))?;
    Ok(())
}
