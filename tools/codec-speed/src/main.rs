//! Small real-image comparison: one image at a time, no file I/O in timers.
#![forbid(unsafe_code)]
use enough::Unstoppable;
use std::{fs, io::Write, path::Path, time::Instant};

const SIDE: usize = 1024;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let out = Path::new(
        args.get(1)
            .ok_or("usage: codec-speed NEW-OUTPUT photo.png ...")?,
    );
    fs::create_dir(out)?;
    let pool = rayon::ThreadPoolBuilder::new().num_threads(8).build()?;
    let mut log = fs::File::create_new(out.join("preparation.tsv"))?;
    writeln!(
        log,
        "image\tcodec\tsetting\twidth\theight\tbytes\tencode_ms"
    )?;
    let mut encode_total = 0.0;
    for source in &args[2..] {
        let file = fs::read(source)?;
        let src = zenpng::decode(
            &file,
            &zenpng::PngDecodeConfig::strict().with_max_threads(1),
            &Unstoppable,
        )?;
        let w = src.info.width as usize;
        let h = src.info.height as usize;
        if w < SIDE || h < SIDE || src.info.bit_depth != 8 || src.info.color_type != 2 {
            return Err("expected an RGB8 photograph at least 1024 square".into());
        }
        let full = src.pixels.copy_to_contiguous_bytes();
        assert_eq!(full.len(), w * h * 3);
        let x = (w - SIDE) / 2;
        let y = (h - SIDE) / 2;
        let raw: Vec<u8> = (y..y + SIDE)
            .flat_map(|row| {
                full[(row * w + x) * 3..(row * w + x + SIDE) * 3]
                    .iter()
                    .copied()
            })
            .collect();
        let id = Path::new(source).file_stem().unwrap().to_str().unwrap();
        fs::write(out.join(format!("{id}.rgb")), &raw)?;
        fs::write(
            out.join(format!("{id}.crop.txt")),
            format!(
                "source={source}\nsource_width={w}\nsource_height={h}\nx={x}\ny={y}\nwidth={SIDE}\nheight={SIDE}\nresized=false\n"
            ),
        )?;
        let rgb: Vec<rgb::Rgb<u8>> = raw
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| rgb::Rgb::new(p[0], p[1], p[2]))
            .collect();
        let info = imij::ImageInfo::new(
            SIDE as u32,
            SIDE as u32,
            imij::Channels::Rgb,
            imij::SampleType::U8,
        );
        for (codec, setting) in [
            ("imij", 0),
            ("imij", 50),
            ("imij", 80),
            ("zenjpeg", 50),
            ("zenjpeg", 80),
            ("zenpng", 1),
            ("zenpng", 2),
            ("zenpng", 7),
        ] {
            // Bound this smoke test: do not start another encode after 100 s.
            // Each moderate-effort encode is measured once; no quality sweep.
            if encode_total >= 100.0 {
                return Err("encoding budget exhausted; completed rows retained".into());
            }
            let start = Instant::now();
            let encoded = match codec {
                "imij" if setting == 0 => imij::ImiEncoderConfig::default().encode_samples(
                    imij::ImageRef::new(&raw, SIDE * 3, &info),
                    &imij::Metadata::default(),
                )?,
                "imij" => imij::ImiEncoderConfig::lossy(setting as f32).encode_samples(
                    imij::ImageRef::new(&raw, SIDE * 3, &info),
                    &imij::Metadata::default(),
                )?,
                "zenjpeg" => {
                    let cfg = zenjpeg::encoder::EncoderConfig::ycbcr(
                        setting as f32,
                        zenjpeg::encoder::ChromaSubsampling::Quarter,
                    )
                    .progressive(false)
                    .restart_mcu_rows(4);
                    let mut enc = cfg.encode_from_bytes(
                        SIDE as u32,
                        SIDE as u32,
                        zenjpeg::encoder::PixelLayout::Rgb8Srgb,
                    )?;
                    enc.push_packed(&raw, Unstoppable)?;
                    enc.finish()?
                }
                "zenpng" => {
                    let mut cfg = zenpng::EncodeConfig::default()
                        .with_compression(zenpng::Compression::Effort(setting));
                    cfg.max_threads = 1;
                    cfg.parallel = false;
                    cfg.decode_segments = 0;
                    zenpng::encode_rgb8(
                        imgref::ImgRef::new(&rgb, SIDE, SIDE),
                        None,
                        &cfg,
                        &Unstoppable,
                        &Unstoppable,
                    )?
                }
                _ => unreachable!(),
            };
            let encode_secs = start.elapsed().as_secs_f64();
            encode_total += encode_secs;
            fs::write(out.join(format!("{id}-{codec}-{setting}.bin")), &encoded)?;
            let mut reference = None;
            for threads in if codec == "zenpng" { [1, 2] } else { [1, 8] } {
                let pixels = match codec {
                    "imij" => {
                        let mut p = vec![0; raw.len()];
                        pool.install(|| {
                            imij::decode_into_threads(&encoded, &mut p, SIDE * 3, threads)
                        })?;
                        p
                    }
                    "zenjpeg" => pool
                        .install(|| {
                            zenjpeg::decoder::DecodeConfig::new()
                                .num_threads(threads)
                                .decode(&encoded, Unstoppable)
                        })?
                        .into_pixels_u8()
                        .ok_or("expected RGB8 JPEG")?,
                    "zenpng" => zenpng::decode(
                        &encoded,
                        &zenpng::PngDecodeConfig::default().with_max_threads(threads),
                        &Unstoppable,
                    )?
                    .pixels
                    .copy_to_contiguous_bytes(),
                    _ => unreachable!(),
                };
                assert_eq!(pixels.len(), raw.len());
                if codec == "zenpng" || (codec == "imij" && setting == 0) {
                    assert_eq!(pixels, raw, "lossless mismatch {id}/{codec}/{setting}");
                }
                if let Some(ref r) = reference {
                    assert_eq!(&pixels, r, "thread mismatch {id}/{codec}/{setting}");
                } else {
                    reference = Some(pixels);
                }
            }
            writeln!(
                log,
                "{id}\t{codec}\t{setting}\t{SIDE}\t{SIDE}\t{}\t{}",
                encoded.len(),
                encode_secs * 1000.0
            )?;
            log.flush()?;
            println!(
                "{id} {codec}/{setting}: {} bytes; encode {:.3} ms; cumulative encode {:.3} s",
                encoded.len(),
                encode_secs * 1000.0,
                encode_total
            );
        }
    }
    fs::write(
        out.join("complete.txt"),
        format!("encode_seconds={encode_total}\n"),
    )?;
    Ok(())
}
