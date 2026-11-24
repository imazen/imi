//! Staged against an exact native-component candidate, not the released API.
#![forbid(unsafe_code)]
#[allow(dead_code)]
mod importer;
use enough::Unstoppable;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{fs, hint::black_box, io::Write, path::Path, time::Instant};
const W: usize = 1024;
fn sha(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}
fn save(path: &Path, bytes: &[u8]) {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}
fn encode(codec: &str, q: f32, effort: f32, raw: &[u8], jpeg: &[u8], band_rows: u16) -> Vec<u8> {
    match codec {
        "jpeg" => {
            let cfg = zenjpeg::encoder::EncoderConfig::ycbcr(
                q,
                zenjpeg::encoder::ChromaSubsampling::Quarter,
            )
            .progressive(false)
            .restart_mcu_rows(4);
            let mut enc = cfg
                .encode_from_bytes(W as u32, W as u32, zenjpeg::encoder::PixelLayout::Rgb8Srgb)
                .unwrap();
            enc.push_packed(raw, Unstoppable).unwrap();
            enc.finish().unwrap()
        }
        "direct" => imij::ImiEncoderConfig::lossy(q).with_effort(effort).encode_samples(imij::ImageRef::new(raw, W * 3, &imij::ImageInfo::new(
                W as u32,
                W as u32,
                imij::Channels::Rgb,
                imij::SampleType::U8,
            )), &imij::Metadata::default())
        .unwrap(),
        "native-rl" | "native-sh" => {
            let image = importer::coefficient_image(jpeg).unwrap();
            let meta = importer::metadata(jpeg).unwrap();
            let entropy = if codec == "native-rl" {
                imij::LossyEntropy::RunLevel
            } else {
                imij::LossyEntropy::SplitHuf
            };
            image.encode(&meta, band_rows, entropy, effort).unwrap()
        }
        _ => panic!("codec"),
    }
}
fn decode(codec: &str, file: &[u8], threads: usize, pool: &rayon::ThreadPool) -> Vec<u8> {
    if codec == "jpeg" {
        pool.install(|| {
            zenjpeg::decoder::Decoder::new()
                .num_threads(threads)
                .decode(file, Unstoppable)
        })
        .unwrap()
        .into_pixels_u8()
        .unwrap()
    } else {
        let mut out = vec![0; W * W * 3];
        pool.install(|| imij::decode_into_threads(file, &mut out, W * 3, threads))
            .unwrap();
        out
    }
}
fn main() {
    let a: Vec<_> = std::env::args().collect();
    assert!(
        (10..=11).contains(&a.len()),
        "ACTION CODEC Q EFFORT THREADS RAW JPEG ENCODED REFERENCE [BAND_ROWS=8]"
    );
    let band_rows = a.get(10).map(|s| s.parse::<u16>().unwrap()).unwrap_or(8);
    let (action, codec) = (&a[1], &a[2]);
    let q: f32 = a[3].parse().unwrap();
    let effort: f32 = a[4].parse().unwrap();
    let threads: usize = a[5].parse().unwrap();
    let raw = fs::read(&a[6]).unwrap();
    assert_eq!(raw.len(), W * W * 3);
    let jpeg = if codec == "jpeg" {
        Vec::new()
    } else {
        fs::read(&a[7]).unwrap()
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(8)
        .build()
        .unwrap();
    if action == "prepare" {
        let t = Instant::now();
        let file = encode(codec, q, effort, &raw, &jpeg, band_rows);
        let ms = t.elapsed().as_secs_f64() * 1000.;
        let pixels = decode(codec, &file, 1, &pool);
        assert_eq!(pixels, decode(codec, &file, 8, &pool), "thread pixels");
        if codec.starts_with("native-") {
            let image = importer::coefficient_image(&jpeg).unwrap();
            assert_eq!(
                imij::jpeg::decode_coefficients(&file).unwrap(),
                image,
                "exact coefficients/tables"
            );
            let mut expected = vec![0; raw.len()];
            image.reconstruct_into(&mut expected, W * 3).unwrap();
            assert_eq!(pixels, expected, "native reconstruction");
        }
        let source = fast_ssim2::PixelSlice::new(
            &raw,
            W as u32,
            W as u32,
            W * 3,
            fast_ssim2::PixelDescriptor::RGB8_SRGB,
        )
        .unwrap();
        let candidate = fast_ssim2::PixelSlice::new(
            &pixels,
            W as u32,
            W as u32,
            W * 3,
            fast_ssim2::PixelDescriptor::RGB8_SRGB,
        )
        .unwrap();
        let score = fast_ssim2::Ssimulacra2Reference::new(&source)
            .unwrap()
            .compare(&candidate)
            .unwrap();
        assert!(score.is_finite());
        let jpg_pixels = if codec == "jpeg" {
            pixels.clone()
        } else {
            decode("jpeg", &jpeg, 1, &pool)
        };
        let max_diff = pixels
            .iter()
            .zip(&jpg_pixels)
            .map(|(&x, &y)| x.abs_diff(y))
            .max()
            .unwrap();
        save(Path::new(&a[8]), &file);
        save(Path::new(&a[9]), &pixels);
        println!(
            "{}",
            json!({"codec":codec,"q":q,"effort":effort,"bytes":file.len(),"bpp":file.len() as f64*8./(W*W) as f64,"ssim2":score,"prepare_encode_ms":ms,"source_sha256":sha(&raw),"encoded_sha256":sha(&file),"decoded_sha256":sha(&pixels),"max_rgb_diff_from_jpeg":max_diff})
        );
        return;
    }
    let file = fs::read(&a[8]).unwrap();
    let expected = fs::read(&a[9]).unwrap();
    assert_eq!(expected.len(), raw.len());
    assert_eq!(
        decode(codec, &file, threads, &pool),
        expected,
        "reference pixels"
    );
    if action == "encode" {
        assert_eq!(threads, 1);
        let start = Instant::now();
        let warm = encode(codec, q, effort, &raw, &jpeg, band_rows);
        let warm_ms = start.elapsed().as_secs_f64() * 1000.;
        assert_eq!(warm, file);
        drop(warm);
        let start = Instant::now();
        let actual = encode(codec, q, effort, &raw, &jpeg, band_rows);
        let ms = start.elapsed().as_secs_f64() * 1000.;
        assert_eq!(actual, file);
        println!("elapsed_ms={ms}\nencode_total_ms={}", warm_ms + ms);
    } else {
        assert_eq!(action, "decode");
        for _ in 0..2 {
            black_box(decode(codec, &file, threads, &pool));
        }
        let start = Instant::now();
        for _ in 0..3 {
            black_box(decode(codec, &file, threads, &pool));
        }
        println!("elapsed_ms={}", start.elapsed().as_secs_f64() * 1000. / 3.);
    }
    println!(
        "check={}",
        json!({"source_sha256":sha(&raw),"reference_ok":true,"pixels":W*W,"decoded_sha256":sha(&expected)})
    );
}
