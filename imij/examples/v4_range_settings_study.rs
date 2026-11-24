//! Matched sparse/range V4 files and public-route pixel and timing oracles.
use imij::codec::{DecodeConfig, FeedStatus};
use imij::planes::Rect;
use imij::{Channels, ImageInfo, ImageRef, ImiEncoderConfig, Metadata, SampleType};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use xxhash_rust::xxh3::xxh3_128;
use zencodec::decode::DecoderConfig;

fn feed(file: &[u8], pixels: &[u8], stride: usize) {
    let mut reader = DecodeConfig::default()
        .job()
        .feed_decoder(16 << 10)
        .unwrap();
    let mut at = 0;
    let mut rows = 0;
    loop {
        match reader.advance().unwrap() {
            FeedStatus::NeedInput if at < file.len() => {
                let end = (at + 4093).min(file.len());
                let accepted = reader.push(&file[at..end]).unwrap();
                assert!(accepted > 0);
                at += accepted;
            }
            FeedStatus::NeedInput => reader.finish_input().unwrap(),
            FeedStatus::Row { y } => {
                assert_eq!(y as usize, rows);
                let row = reader.row().unwrap().unwrap();
                assert_eq!(row.row(0), &pixels[rows * stride..][..stride]);
                rows += 1;
            }
            FeedStatus::Complete => {
                assert_eq!(at, file.len());
                assert_eq!(rows * stride, pixels.len());
                break;
            }
            status => panic!("unexpected feed status: {status:?}"),
        }
    }
}

fn check(native: &[u8], sparse: &[u8], range: &[u8]) {
    let (info, pixels) = imij::decode(native).unwrap();
    let row = info.row_bytes();
    let x = info.width / 2 - 7;
    let y = info.height / 2 - 11;
    let rect = Rect::new(x, y, 35, 33);
    let crop_row = rect.width() as usize * 3;
    let stride = crop_row + 7;
    let mut crop = vec![0xab; stride * rect.height() as usize];
    let mut out = vec![0; pixels.len()];
    for file in [sparse, range] {
        assert_eq!(imij::header(file).unwrap().version, 4);
        for workers in [1, 2, 4, 8] {
            imij::decode_into_threads(file, &mut out, row, workers).unwrap();
            assert_eq!(out, pixels);
            imij::region::decode_region_threads(file, rect, &mut crop, stride, workers).unwrap();
            for (i, got) in crop.chunks_exact(stride).enumerate() {
                let start = (y as usize + i) * row + x as usize * 3;
                assert_eq!(&got[..crop_row], &pixels[start..][..crop_row]);
                assert!(got[crop_row..].iter().all(|&b| b == 0xab));
            }
        }
        feed(file, &pixels, row);
    }
    println!("{:032x}", xxh3_128(&pixels));
}

fn bench(sparse: Arc<[u8]>, range: Arc<[u8]>, workers: usize, id: &str, output: &Path) {
    let info = imij::header(&sparse).unwrap().info;
    let stride = info.row_bytes();
    let result = zenbench::run(|suite| {
        suite.plan().workload_id = Some(format!("v4-range-settings-{id}-{workers}t"));
        suite.group("public-reused-output", |group| {
            let config = group.config();
            config
                .min_rounds(24)
                .max_rounds(24)
                .auto_rounds(false)
                .warmup_time(Duration::from_millis(200))
                .max_wall_time(Duration::from_secs(300));
            config.min_iterations = 1;
            config.max_iterations = 1;
            group.throughput(zenbench::Throughput::Elements(
                u64::from(info.width) * u64::from(info.height),
            ));
            for (name, file) in [
                ("sparse", sparse.clone()),
                ("sparse-aa", sparse),
                ("range", range),
            ] {
                let mut out = vec![0; info.checked_buffer_len().unwrap()];
                group.bench(name, move |b| {
                    b.iter(|| {
                        imij::decode_into_threads(&file, &mut out, stride, workers).unwrap();
                        std::hint::black_box(&out);
                    })
                });
            }
        });
    });
    result.save(output).unwrap();
    zenbench::postprocess_result(&result);
}

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["prepare", raw, size, quality, output] => {
            let size = size.parse().unwrap();
            let info = ImageInfo::new(size, size, Channels::Rgb, SampleType::U8);
            let pixels = fs::read(raw).unwrap();
            let config = ImiEncoderConfig::lossy(quality.parse::<f32>().unwrap())
                .with_effort(7.5)
                .with_column_groups(Some(256))
                .tune_lossy(|t| {
                    t.adaptive_partitions = true;
                    t.small_transforms = false;
                    t.exact_blocks = Some(0.0);
                    t.stripe_block_rows = 32;
                });
            let file = config
                .encode_legacy_samples(ImageRef::packed(&pixels, &info), &Metadata::default())
                .unwrap();
            fs::write(output, file).unwrap();
        }
        ["upgrade", native, output] => {
            let native = fs::read(native).unwrap();
            let file = imij::upgrade_image(&native, &imij::UpgradeLimits::default()).unwrap();
            println!("{}\t{:032x}", file.len(), xxh3_128(&file));
            fs::write(output, file).unwrap();
        }
        ["check", native, sparse, range] => {
            check(
                &fs::read(native).unwrap(),
                &fs::read(sparse).unwrap(),
                &fs::read(range).unwrap(),
            );
        }
        ["bench", sparse, range, workers, id, result] => {
            bench(
                fs::read(sparse).unwrap().into(),
                fs::read(range).unwrap().into(),
                workers.parse().unwrap(),
                id,
                Path::new(result),
            );
        }
        _ => panic!(
            "usage: prepare RGB SIZE Q OUT | upgrade V3 OUT | check V3 SPARSE RANGE | bench SPARSE RANGE WORKERS ID JSON"
        ),
    }
}
