//! A short correctness-checked cost probe, not a corpus benchmark framework.
use std::{fmt::Write as _, fs, time::Instant};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn median(mut samples: Vec<u128>) -> u128 {
    samples.sort_unstable();
    samples[samples.len() / 2]
}
pub(crate) fn run(args: &[String]) -> Result<()> {
    if args.len() != 2 {
        return Err("--cost INPUT.jpg OUTPUT.tsv".into());
    }
    if std::path::Path::new(&args[1]).exists() {
        return Err("cost output exists; preserve it and choose a new path".into());
    }
    let input = fs::read(&args[0])?;
    let image = crate::importer::coefficient_image(&input)?;
    let metadata = crate::importer::metadata(&input)?;
    let info = image.image_info();
    let mut expected = vec![0; info.checked_buffer_len().ok_or("image too large")?];
    image.reconstruct_into(&mut expected, info.row_bytes())?;
    let mut array = Vec::new();
    let mut pixels = vec![0; expected.len()];
    for _ in 0..5 {
        let start = Instant::now();
        image.reconstruct_into(&mut pixels, info.row_bytes())?;
        array.push(start.elapsed().as_nanos());
        if pixels != expected {
            return Err("array cost check changed pixels".into());
        }
    }
    let array_ns = median(array);
    let mut config = zenjpeg::decoder::DecodeConfig::new();
    config.output_format = Some(zenjpeg::decoder::PixelFormat::Rgb);
    let source = config.decode(&input, enough::Unstoppable)?;
    let source_pixels = source.pixels_u8().ok_or("source has no RGB8 pixels")?;
    let mut jpeg_times = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        let decoded = config.decode(&input, enough::Unstoppable)?;
        jpeg_times.push(start.elapsed().as_nanos());
        if decoded.pixels_u8() != Some(source_pixels) {
            return Err("source JPEG decode changed pixels".into());
        }
    }
    let jpeg_ns = median(jpeg_times);
    let mut report = String::from(
        "width\theight\tmcu_rows\tentropy\tfile_bytes\tencode_ns\trequested_threads\tworkers\tdecode_median_ns\tarray_reconstruct_median_ns\tzenjpeg_source_decode_median_ns\tchecked\n",
    );
    for rows in [8u16, 256] {
        for (name, entropy) in [
            ("rl", imij::LossyEntropy::RunLevel),
            ("sh", imij::LossyEntropy::SplitHuf),
        ] {
            let start = Instant::now();
            let file = image.encode(&metadata, rows, entropy, 7.5)?;
            let encode_ns = start.elapsed().as_nanos();
            if imij::jpeg::decode_coefficients(&file)? != image {
                return Err("cost file changed coefficients/tables".into());
            }
            let bands = imij::jpeg::NativeArchive::open(&file)?.band_count();
            for threads in [1usize, 2, 4, 8] {
                imij::decode_into_threads(&file, &mut pixels, info.row_bytes(), threads)?;
                let mut times = Vec::new();
                for _ in 0..5 {
                    let start = Instant::now();
                    imij::decode_into_threads(&file, &mut pixels, info.row_bytes(), threads)?;
                    times.push(start.elapsed().as_nanos());
                    if pixels != expected {
                        return Err("cost decode changed pixels".into());
                    }
                }
                writeln!(
                    report,
                    "{}\t{}\t{rows}\t{name}\t{}\t{encode_ns}\t{threads}\t{}\t{}\t{array_ns}\t{jpeg_ns}\tok",
                    info.width,
                    info.height,
                    file.len(),
                    bands.div_ceil(bands.div_ceil(threads.min(bands))),
                    median(times)
                )?;
                fs::write(&args[1], &report)?;
            }
        }
    }
    Ok(())
}
