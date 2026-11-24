//! Codec comparison: imij against JPEG XL (jxl-encoder + zenjxl-decoder) and
//! AVIF (zenav1-svt encode, rav1d-safe decode via zenavif).
//!
//! Two steps, so the timing step can run alone on an idle machine:
//!
//! - `cmpenc MANIFEST OUTDIR lossless|lossy` encodes every image with every
//!   setting, checks lossless roundtrips are exact (decoding with the same
//!   decoder the timing step uses), scores lossy decodes (SSIMULACRA2,
//!   zensim, butteraugli 3-norm and max) and appends rows to
//!   `OUTDIR/index.tsv`.
//! - `cmpdec OUTDIR THREADS ITERS` decodes every file in the index to an
//!   RGB8 buffer at each thread count (comma list), best of ITERS, and
//!   writes `OUTDIR/speed.tsv`. Every decode allocates its output, as a
//!   caller would.

use crate::{Img, metrics};
use std::io::Write;

/// One encoded file.
pub struct Row {
    pub kind: String,
    pub img: String,
    pub w: u32,
    pub h: u32,
    pub codec: String,
    pub setting: String,
    pub file: String,
    pub bytes: usize,
    pub enc_s: f64,
    pub ssim2: f64,
    pub zensim: f64,
    pub ba3: f64,
    pub bamax: f64,
}

const HEAD: &str = "kind\timg\tw\th\tcodec\tsetting\tfile\tbytes\tenc_s\tssim2\tzensim\tba3\tbamax";

impl Row {
    fn line(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.4}\t{:.3}\t{:.3}\t{:.4}\t{:.4}",
            self.kind,
            self.img,
            self.w,
            self.h,
            self.codec,
            self.setting,
            self.file,
            self.bytes,
            self.enc_s,
            self.ssim2,
            self.zensim,
            self.ba3,
            self.bamax
        )
    }

    fn parse(l: &str) -> Row {
        let f: Vec<&str> = l.split('\t').collect();
        Row {
            kind: f[0].into(),
            img: f[1].into(),
            w: f[2].parse().unwrap(),
            h: f[3].parse().unwrap(),
            codec: f[4].into(),
            setting: f[5].into(),
            file: f[6].into(),
            bytes: f[7].parse().unwrap(),
            enc_s: f[8].parse().unwrap(),
            ssim2: f[9].parse().unwrap(),
            zensim: f[10].parse().unwrap(),
            ba3: f[11].parse().unwrap(),
            bamax: f[12].parse().unwrap(),
        }
    }
}

pub fn read_index(dir: &str) -> Vec<Row> {
    std::fs::read_to_string(format!("{dir}/index.tsv"))
        .unwrap()
        .lines()
        .skip(1)
        .filter(|l| !l.is_empty())
        .map(Row::parse)
        .collect()
}

/// Codec family of a codec label (picks the decoder).
fn family(codec: &str) -> &'static str {
    if codec.starts_with("imij") {
        "imij"
    } else if codec.starts_with("jxl") {
        "jxl"
    } else if codec.starts_with("avif") {
        "avif"
    } else {
        panic!("unknown codec {codec}")
    }
}

/// zenjxl-decoder to RGB8 (no alpha), the caller's rayon pool when `parallel`.
pub fn jxl_decode_rgb8(data: &[u8], parallel: bool) -> Vec<u8> {
    use zenjxl_decoder::api::*;
    let mut opts = JxlDecoderOptions::default();
    opts.parallel = parallel;
    let mut input = data;
    let dec = JxlDecoder::<states::Initialized>::new(opts);
    let mut dec = match dec.process(&mut input).unwrap() {
        ProcessingResult::Complete { result } => result,
        ProcessingResult::NeedsMoreInput { .. } => panic!("jxl: truncated header"),
    };
    let (w, h) = dec.basic_info().size;
    dec.set_pixel_format(JxlPixelFormat {
        color_type: JxlColorType::Rgb,
        color_data_format: Some(JxlDataFormat::U8 { bit_depth: 8 }),
        extra_channel_format: vec![],
    });
    let dec = match dec.process(&mut input).unwrap() {
        ProcessingResult::Complete { result } => result,
        ProcessingResult::NeedsMoreInput { .. } => panic!("jxl: truncated frame header"),
    };
    let mut out = vec![0u8; w * h * 3];
    {
        let mut bufs = [JxlOutputBuffer::new(&mut out, h, w * 3)];
        match dec.process(&mut input, &mut bufs).unwrap() {
            ProcessingResult::Complete { .. } => {}
            ProcessingResult::NeedsMoreInput { .. } => panic!("jxl: truncated frame"),
        }
    }
    out
}

/// zenavif (rav1d-safe) to an 8-bit pixel buffer.
pub(crate) fn avif_decode(data: &[u8], threads: u32) -> zenavif::PixelBuffer {
    let cfg = zenavif::DecoderConfig::new()
        .threads(threads)
        .prefer_8bit(true);
    zenavif::decode_with(data, &cfg, &enough::Unstoppable).unwrap()
}

/// RGB8 bytes of a decoded AVIF (alpha dropped).
pub(crate) fn avif_rgb8(pb: &zenavif::PixelBuffer) -> Vec<u8> {
    let b = pb.copy_to_contiguous_bytes();
    let n = pb.width() as usize * pb.height() as usize;
    match b.len() / n {
        3 => b,
        4 => b
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|c| [c[0], c[1], c[2]])
            .collect(),
        c => panic!("avif: {c} bytes per pixel"),
    }
}

/// Decodes `data` to RGB8 with `threads` threads (`pool` holds that many).
fn decode_rgb8(codec: &str, data: &[u8], threads: usize, pool: &rayon::ThreadPool) -> Vec<u8> {
    match family(codec) {
        "imij" => imij::decode(data).unwrap().1,
        "jxl" => pool.install(|| jxl_decode_rgb8(data, threads > 1)),
        _ => avif_rgb8(&avif_decode(data, threads as u32)),
    }
}

/// The timed decode: input bytes to a fresh RGB8 buffer.
fn decode_timed(
    codec: &str,
    data: &[u8],
    w: usize,
    h: usize,
    threads: usize,
    pool: &rayon::ThreadPool,
) {
    match family(codec) {
        "imij" => {
            let mut buf = vec![0u8; w * h * 3];
            imij::decode_into_threads(data, &mut buf, w * 3, threads).unwrap();
            std::hint::black_box(&buf);
        }
        "jxl" => {
            std::hint::black_box(pool.install(|| jxl_decode_rgb8(data, threads > 1)));
        }
        _ => {
            std::hint::black_box(avif_decode(data, threads as u32));
        }
    }
}

fn jxl_lossless(img: &Img, effort: u8, fast: bool) -> Vec<u8> {
    let mut c = jxl_encoder::LosslessConfig::new().with_effort(effort);
    if fast {
        c = c.with_fast_decode();
    }
    c.encode(
        &img.pixels,
        img.info.width,
        img.info.height,
        jxl_encoder::PixelLayout::Rgb8,
    )
    .unwrap()
}

fn jxl_lossy(img: &Img, distance: f32) -> Vec<u8> {
    jxl_encoder::LossyConfig::new(distance)
        .encode(
            &img.pixels,
            img.info.width,
            img.info.height,
            jxl_encoder::PixelLayout::Rgb8,
        )
        .unwrap()
}

fn avif_svt(img: &Img, quality: f32, speed: u8) -> Vec<u8> {
    let px: Vec<rgb::RGB8> = img
        .pixels
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| rgb::RGB8::new(c[0], c[1], c[2]))
        .collect();
    let im = imgref::Img::new(px, img.info.width as usize, img.info.height as usize);
    let cfg = zenavif::EncoderConfig::new()
        .backend(zenavif::Av1Backend::Zenav1Svt)
        .chroma_subsampling(zenavif::EncodeChromaSubsampling::Yuv420)
        .quality(quality)
        .speed(speed)
        .threads(Some(8));
    zenavif::encode_rgb8(
        im.as_ref(),
        &cfg,
        almost_enough::StopToken::new(enough::Unstoppable),
    )
    .unwrap()
    .avif_file
}

fn imij_lossless(img: &Img, effort: f32) -> Vec<u8> {
    {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.with_effort(effort);
        cfg = cfg.with_threads(8);
        cfg
    }
    .encode_samples(
        imij::ImageRef::new(&img.pixels, img.info.row_bytes(), &img.info),
        &imij::Metadata::default(),
    )
    .unwrap()
}

/// (codec, setting) pairs and their encoders.
type Enc = Box<dyn Fn(&Img) -> Vec<u8> + Sync>;

/// The settings of `kind`; `IMIJ_CMP_CODECS` (comma list of families: imij,
/// jxl, avif) restricts them.
fn settings(kind: &str) -> Vec<(String, String, &'static str, Enc)> {
    let keep = std::env::var("IMIJ_CMP_CODECS").ok();
    let mut v = settings_all(kind);
    if let Some(k) = keep {
        v.retain(|s| k.split(',').any(|f| f == s.2));
    }
    // IMIJ_CMP_NAMES: comma list of codec labels to keep.
    if let Ok(k) = std::env::var("IMIJ_CMP_NAMES") {
        v.retain(|s| k.split(',').any(|f| f == s.0));
    }
    v
}

fn settings_all(kind: &str) -> Vec<(String, String, &'static str, Enc)> {
    let mut v: Vec<(String, String, &'static str, Enc)> = Vec::new();
    if kind == "lossless" {
        // Same searches as the historical 0/1 switch, on the 0–10 scale.
        for e in [5.0, 10.0] {
            v.push((
                "imij".into(),
                format!("effort{e}"),
                "imij",
                Box::new(move |i| imij_lossless(i, e)),
            ));
        }
        v.push((
            "jxl-fast".into(),
            "e1".into(),
            "jxl",
            Box::new(|i| jxl_lossless(i, 1, true)),
        ));
        v.push((
            "jxl-fast".into(),
            "e3".into(),
            "jxl",
            Box::new(|i| jxl_lossless(i, 3, true)),
        ));
        v.push((
            "jxl".into(),
            "e1".into(),
            "jxl",
            Box::new(|i| jxl_lossless(i, 1, false)),
        ));
        v.push((
            "jxl".into(),
            "e3".into(),
            "jxl",
            Box::new(|i| jxl_lossless(i, 3, false)),
        ));
        v.push((
            "jxl".into(),
            "e7".into(),
            "jxl",
            Box::new(|i| jxl_lossless(i, 7, false)),
        ));
        return v;
    }
    let qs: Vec<f32> = match std::env::var("IMIJ_CMP_QS") {
        Ok(v) => v.split(',').map(|x| x.parse().unwrap()).collect(),
        Err(_) => vec![
            10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 85.0, 90.0, 95.0,
        ],
    };
    for q in qs {
        // Fixed split-Huffman XYB (cost-model validation).
        v.push((
            "imij-sh".into(),
            format!("q{q}"),
            "imij",
            Box::new(move |i| {
                metrics::imij_lossy(
                    i,
                    &imij::ImiEncoderConfig::lossy(q)
                        .tune_lossy(|t| t.entropy = imij::LossyEntropy::SplitHuf)
                        .with_threads(8),
                )
            }),
        ));
        // imij by decode-speed tier (ImiEncoderConfig::with_decode_speed).
        for (name, speed) in [
            ("imij-compact", imij::DecodeSpeed::Compact),
            ("imij-balanced", imij::DecodeSpeed::Balanced),
            ("imij-fast", imij::DecodeSpeed::Fast),
            ("imij-fastest", imij::DecodeSpeed::Fastest),
        ] {
            v.push((
                name.into(),
                format!("q{q}"),
                "imij",
                Box::new(move |i| {
                    metrics::imij_lossy(
                        i,
                        &imij::ImiEncoderConfig::lossy(q)
                            .with_decode_speed(speed)
                            .with_threads(8),
                    )
                }),
            ));
        }
    }
    for d in [0.5f32, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0] {
        v.push((
            "jxl-lossy".into(),
            format!("d{d}"),
            "jxl",
            Box::new(move |i| jxl_lossy(i, d)),
        ));
    }
    for q in [20.0f32, 35.0, 42.0, 50.0, 60.0, 70.0, 80.0, 88.0, 94.0] {
        v.push((
            "avif-svt".into(),
            format!("q{q}"),
            "avif",
            Box::new(move |i| avif_svt(i, q, 6)),
        ));
    }
    v
}

pub fn cmd_enc(imgs: &[Img], dir: &str, kind: &str) {
    std::fs::create_dir_all(format!("{dir}/files")).unwrap();
    let idx = format!("{dir}/index.tsv");
    let fresh = !std::path::Path::new(&idx).exists();
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&idx)
        .unwrap();
    if fresh {
        writeln!(out, "{HEAD}").unwrap();
    }
    let pool1 = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    for img in imgs {
        let (w, h) = (img.info.width, img.info.height);
        let s2ref = if kind == "lossy" {
            Some(metrics::ssim2_ref(img))
        } else {
            None
        };
        // Encode (timed) and decode in order; score in parallel, 8 at a time.
        let all = settings(kind);
        // Scoring memory: fewer at a time for huge images.
        let per = if img.pixels.len() > 48 << 20 { 2 } else { 8 };
        let mut stats = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(format!("{dir}/stats.tsv"))
            .unwrap();
        for batch in all.chunks(per) {
            let mut done: Vec<(Row, Vec<u8>)> = Vec::new();
            for (codec, setting, ext, enc) in batch {
                let t = std::time::Instant::now();
                let data = enc(img);
                let enc_s = t.elapsed().as_secs_f64();
                if codec == "imij-compact" {
                    // Entropy-work counts of this (run-level) encode.
                    let [tk, gr, bc] = imij::lab::lossy_side_stats();
                    writeln!(stats, "{}\t{setting}\t{tk}\t{gr}\t{bc}", img.name).unwrap();
                }
                let file = format!("files/{}.{codec}.{setting}.{ext}", img.name);
                std::fs::write(format!("{dir}/{file}"), &data).unwrap();
                let dec = decode_rgb8(codec, &data, 1, &pool1);
                assert_eq!(dec.len(), img.pixels.len(), "{file}: decoded size");
                if s2ref.is_none() {
                    assert!(dec == img.pixels, "{file}: lossless roundtrip mismatch");
                }
                done.push((
                    Row {
                        kind: kind.into(),
                        img: img.name.clone(),
                        w,
                        h,
                        codec: codec.clone(),
                        setting: setting.clone(),
                        file,
                        bytes: data.len(),
                        enc_s,
                        ssim2: f64::NAN,
                        zensim: f64::NAN,
                        ba3: f64::NAN,
                        bamax: f64::NAN,
                    },
                    dec,
                ));
            }
            if let Some(s2) = &s2ref {
                use rayon::prelude::*;
                done.par_iter_mut().for_each(|(r, dec)| {
                    r.ssim2 = metrics::ssim2(s2, img, dec);
                    r.zensim = metrics::zensim(img, dec);
                    (r.ba3, r.bamax) = metrics::butteraugli(img, dec);
                });
            }
            for (r, _) in &done {
                eprintln!(
                    "{:<10} {:<16} {:<8} {:>7.3} bpp  ssim2 {:>6.2}  enc {:>6.2}s",
                    img.name,
                    r.codec,
                    r.setting,
                    r.bytes as f64 * 8.0 / (w as f64 * h as f64),
                    r.ssim2,
                    r.enc_s
                );
                writeln!(out, "{}", r.line()).unwrap();
            }
            out.flush().unwrap();
        }
    }
}

pub fn cmd_dec(dir: &str, threads: &[usize], iters: usize) {
    let rows = read_index(dir);
    let pools: Vec<rayon::ThreadPool> = threads
        .iter()
        .map(|&t| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(t)
                .build()
                .unwrap()
        })
        .collect();
    let mut out = std::fs::File::create(format!("{dir}/speed.tsv")).unwrap();
    writeln!(out, "file\tthreads\tbest_s").unwrap();
    for r in &rows {
        let data = std::fs::read(format!("{dir}/{}", r.file)).unwrap();
        let (w, h) = (r.w as usize, r.h as usize);
        let mut line = format!("{:<44}", r.file);
        for (&t, pool) in threads.iter().zip(&pools) {
            decode_timed(&r.codec, &data, w, h, t, pool);
            let best = (0..iters)
                .map(|_| {
                    let s = std::time::Instant::now();
                    decode_timed(&r.codec, &data, w, h, t, pool);
                    s.elapsed().as_secs_f64()
                })
                .fold(f64::MAX, f64::min);
            writeln!(out, "{}\t{t}\t{best:.6}", r.file).unwrap();
            line += &format!(" {t}T {:>7.1}", (w * h) as f64 / best / 1e6);
        }
        eprintln!("{line} Mpx/s");
        out.flush().unwrap();
    }
}
