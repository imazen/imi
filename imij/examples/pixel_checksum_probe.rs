//! Optional bitmap-checksum cost: one input per timing process, no native CPU flags.
use imij::{Channels, ImageInfo, Metadata, SampleType};
use std::{fs, hint::black_box, path::Path, time::Instant};

fn source(info: &ImageInfo) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(info.checked_buffer_len().unwrap());
    for y in 0..info.height {
        for x in 0..info.width {
            pixels.extend_from_slice(&[
                (x.wrapping_mul(7).wrapping_add(y * 3)) as u8,
                (x.wrapping_mul(3).wrapping_add(y * 11)) as u8,
                ((x / 7 + y / 13) * 17) as u8,
                (x + y * 3) as u8,
            ]);
        }
    }
    pixels
}
fn encode(kind: &str, pixels: &[u8], info: &ImageInfo, threads: usize) -> Vec<u8> {
    match kind {
        "lossless" => {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.with_threads(threads);
            cfg
        }
        .encode_samples(
            imij::ImageRef::new(pixels, info.row_bytes(), info),
            &Metadata::default(),
        )
        .unwrap(),
        "near" => {
            let mut cfg = imij::ImiEncoderConfig::default().tune_lossless(|t| t.max_error = [5; 4]);
            cfg = cfg.with_threads(threads);
            cfg
        }
        .encode_samples(
            imij::ImageRef::new(pixels, info.row_bytes(), info),
            &Metadata::default(),
        )
        .unwrap(),
        "lossy" => imij::ImiEncoderConfig::lossy(50.0)
            .with_threads(threads)
            .encode_samples(
                imij::ImageRef::new(pixels, info.row_bytes(), info),
                &Metadata::default(),
            )
            .unwrap(),
        _ => panic!("unknown kind"),
    }
}
fn prepare(dir: &Path) {
    fs::create_dir(dir).expect("output directory must be new");
    let mut manifest = String::from("name\twidth\theight\tkind\tplain_bytes\tsummed_bytes\n");
    for size in [1, 64, 1024, 4096] {
        let info = ImageInfo::new(size, size, Channels::Rgba, SampleType::U8);
        let pixels = source(&info);
        for kind in ["lossless", "near", "lossy"] {
            let plain = encode(kind, &pixels, &info, 1);
            let summed = imij::add_pixel_checksum(plain.clone()).unwrap();
            assert_eq!(
                imij::decode(&plain).unwrap(),
                imij::decode(&summed).unwrap()
            );
            let name = format!("{kind}-{size}");
            fs::write(dir.join(format!("{name}-off.imi")), &plain).unwrap();
            fs::write(dir.join(format!("{name}-on.imi")), &summed).unwrap();
            manifest.push_str(&format!(
                "{name}\t{size}\t{size}\t{kind}\t{}\t{}\n",
                plain.len(),
                summed.len()
            ));
        }
    }
    fs::write(dir.join("files.tsv"), manifest).unwrap();
}
fn time(file: &Path, threads: usize, rounds: usize) {
    let encoded = fs::read(file).unwrap();
    let info = imij::header(&encoded).unwrap().info;
    let mut pixels = vec![0; info.checked_buffer_len().unwrap()];
    let stride = info.row_bytes();
    for _ in 0..3 {
        imij::decode_into_threads(&encoded, &mut pixels, stride, threads).unwrap();
    }
    println!("round\tthreads\tdecode_ns\tstream_first_row_ns\tstream_total_ns");
    let mut row = vec![0; stride];
    for round in 0..rounds {
        let start = Instant::now();
        imij::decode_into_threads(black_box(&encoded), black_box(&mut pixels), stride, threads)
            .unwrap();
        black_box(&pixels);
        let elapsed = start.elapsed().as_nanos();
        let start = Instant::now();
        let mut decoder = imij::StreamDecoder::new(black_box(&encoded)).unwrap();
        assert!(decoder.next_row(black_box(&mut row)).unwrap());
        let first = start.elapsed().as_nanos();
        while decoder.next_row(black_box(&mut row)).unwrap() {
            black_box(&row);
        }
        let total = start.elapsed().as_nanos();
        println!("{round}\t{threads}\t{elapsed}\t{first}\t{total}");
    }
}
fn encode_time(kind: &str, size: u32, threads: usize, rounds: usize) {
    let info = ImageInfo::new(size, size, Channels::Rgba, SampleType::U8);
    let pixels = source(&info);
    println!("round\tthreads\tencode_ns\tfinalizer_ns\twhole_opt_in_ns\tbytes");
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap();
    pool.install(|| {
        for round in 0..rounds {
            let start = Instant::now();
            let plain = encode(kind, black_box(&pixels), &info, threads);
            let encoded = start.elapsed().as_nanos();
            let start = Instant::now();
            let summed = imij::add_pixel_checksum(plain).unwrap();
            let sealed = start.elapsed().as_nanos();
            black_box(&summed);
            println!(
                "{round}\t{threads}\t{encoded}\t{sealed}\t{}\t{}",
                encoded + sealed,
                summed.len()
            );
        }
    });
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    match args[1].as_str() {
        "prepare" => prepare(Path::new(&args[2])),
        "time" => time(
            Path::new(&args[2]),
            args[3].parse().unwrap(),
            args[4].parse().unwrap(),
        ),
        "encode-time" => encode_time(
            &args[2],
            args[3].parse().unwrap(),
            args[4].parse().unwrap(),
            args[5].parse().unwrap(),
        ),
        _ => {
            panic!("prepare DIR | time FILE THREADS ROUNDS | encode-time KIND SIZE THREADS ROUNDS")
        }
    }
}
