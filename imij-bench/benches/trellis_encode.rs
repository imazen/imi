//! Lossy encode time across efforts, with and without the AC trellis
//! (zenbench, interleaved): `cargo bench --bench trellis_encode`.
//!
//! Inputs: RGB8 PNGs named by `IMIJ_TRELLIS_PNGS` (`path[,...]`), center
//! crops of each size in `IMIJ_TRELLIS_SIZES` (default `64,256,1024,4096`;
//! a crop never exceeds the image), or tight RGB8 rasters named by
//! `IMIJ_TRELLIS_RAWS` (`path:WIDTHxHEIGHT[,...]`). Groups: each input x
//! quality (XYB, run-level, 8-row stripes); arms: efforts 5, 7.5 (the
//! default, baseline), 8.5, 9 and 10 with the trellis each selects, and
//! effort 10 without it (the lab override). `IMIJ_TRELLIS_THREADS` sets
//! encoder threads (default 1). Throughput: input pixels.
use imij::{Channels, ImageInfo, ImiEncoderConfig, LossyColor, LossyTrellis, Metadata, SampleType};
use std::sync::Arc;
use zenbench::prelude::*;

type Input = (String, Arc<Vec<u8>>, u32, u32);

fn raws(spec: &str) -> Vec<Input> {
    spec.split(',')
        .map(|item| {
            let (path, dim) = item.rsplit_once(':').expect("path:WxH");
            let (w, h) = dim.split_once('x').expect("WxH");
            let (w, h): (u32, u32) = (w.parse().unwrap(), h.parse().unwrap());
            let px = std::fs::read(path).unwrap();
            assert_eq!(px.len(), (w * h * 3) as usize, "{path}");
            let name = std::path::Path::new(path)
                .parent()
                .and_then(|p| p.file_name())
                .map_or(path.to_string(), |n| n.to_string_lossy().into_owned());
            (name, Arc::new(px), w, h)
        })
        .collect()
}

fn pngs(spec: &str, sizes: &[u32]) -> Vec<Input> {
    let mut out = Vec::new();
    for path in spec.split(',') {
        let data = std::fs::read(path).unwrap();
        let image = zenpng::decode(
            &data,
            &zenpng::PngDecodeConfig::default(),
            &enough::Unstoppable,
        )
        .unwrap();
        let (w, h) = (image.info.width, image.info.height);
        let px = image.pixels.copy_to_contiguous_bytes();
        assert_eq!(px.len(), (w * h * 3) as usize, "{path}: not RGB8");
        let stem = std::path::Path::new(path)
            .file_name()
            .unwrap()
            .to_string_lossy();
        let id = stem.split('_').next().unwrap().to_string();
        for &size in sizes {
            let (cw, ch) = (size.min(w), size.min(h));
            let (x0, y0) = ((w - cw) / 2, (h - ch) / 2);
            let mut crop = Vec::with_capacity((cw * ch * 3) as usize);
            for y in y0..y0 + ch {
                let row = (y * w + x0) as usize * 3;
                crop.extend_from_slice(&px[row..row + cw as usize * 3]);
            }
            out.push((format!("{id} {cw}x{ch}"), Arc::new(crop), cw, ch));
        }
    }
    out
}

fn inputs() -> Vec<Input> {
    if let Ok(spec) = std::env::var("IMIJ_TRELLIS_PNGS") {
        let sizes: Vec<u32> = std::env::var("IMIJ_TRELLIS_SIZES")
            .unwrap_or_else(|_| "64,256,1024,4096".into())
            .split(',')
            .map(|s| s.parse().unwrap())
            .collect();
        return pngs(&spec, &sizes);
    }
    let spec = std::env::var("IMIJ_TRELLIS_RAWS")
        .expect("IMIJ_TRELLIS_PNGS=path[,...] or IMIJ_TRELLIS_RAWS=path:WxH[,...]");
    raws(&spec)
}

fn bench(suite: &mut Suite) {
    let threads: usize = std::env::var("IMIJ_TRELLIS_THREADS")
        .ok()
        .map_or(1, |v| v.parse().unwrap());
    for (name, px, w, h) in inputs() {
        for q in [10.0f32, 30.0, 60.0, 90.0] {
            suite.group(format!("{name} q{q} t{threads}"), |g| {
                g.throughput(Throughput::Elements(u64::from(w) * u64::from(h)));
                g.throughput_unit("px");
                g.config().max_time(std::time::Duration::from_secs(20));
                g.baseline("e7.5");
                let off = LossyTrellis {
                    passes: 0,
                    ..LossyTrellis::default()
                };
                let arms: [(&str, f32, Option<LossyTrellis>); 6] = [
                    ("e7.5", 7.5, None),
                    ("e5", 5.0, None),
                    ("e8.5", 8.5, None),
                    ("e9", 9.0, None),
                    ("e10", 10.0, None),
                    ("e10-off", 10.0, Some(off)),
                ];
                for (arm, effort, trellis) in arms {
                    let px = px.clone();
                    g.bench(arm, move |b| {
                        let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
                        let mut cfg = imij::ImiEncoderConfig::lossy(q)
                            .tune_lossy(|t| t.color = LossyColor::Xyb)
                            .with_effort(effort)
                            .with_threads(threads)
                            .tune_lossy(|t| t.stripe_block_rows = 8);
                        cfg = cfg.tune_lossy(|t| {
                            t.lab.trellis = trellis;
                        });
                        b.iter(|| {
                            cfg.encode_samples(
                                imij::ImageRef::new(&px, w as usize * 3, &info),
                                &Metadata::default(),
                            )
                            .unwrap()
                        })
                    });
                }
            });
        }
    }
}

zenbench::main!(bench);
