//! Encode a frozen fixture, verify a same-pixel pair, or measure decode with zenbench.
//! Benchmark environment variables identify the pair independently of zenbench's CLI.

use imij::codec::{DecodeConfig, FeedStatus};
use imij::planes::{ReadAt, Rect, SliceReader};
use imij::region::RegionReader;
use imij::{
    Channels, ImageInfo, ImageRef, ImiEncoderConfig, Limits, LossyColor, LossyEntropy, Metadata,
    SampleType, SmallLab,
};
use serde_json::json;
use std::{fs, hint::black_box, path::Path, sync::Arc, time::Duration};
use zencodec::decode::DecoderConfig;

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn encode(args: &[String], tiled: bool) -> Result {
    let [source, output, width, height, coder, quality] = args else {
        return Err("encode needs RGB input, output, width, height, coder and quality".into());
    };
    let info = ImageInfo::new(
        width.parse()?,
        height.parse()?,
        Channels::Rgb,
        SampleType::U8,
    );
    let entropy = match coder.as_str() {
        "rans" => LossyEntropy::RunLevel,
        "huf" => LossyEntropy::SplitHuf,
        _ => return Err("expected rans or huf".into()),
    };
    let source = fs::read(source)?;
    let config = ImiEncoderConfig::lossy(quality.parse::<f32>()?)
        .with_effort(7.5)
        .with_threads(2)
        .with_column_groups(Some(256))
        .tune_lossy(|tuning| {
            tuning.color = LossyColor::Xyb;
            tuning.entropy = entropy;
            tuning.exact_blocks = Some(0.0);
            tuning.stripe_block_rows = 16;
            if tiled {
                tuning.adaptive_partitions = true;
                tuning.lab.switches.dc_stripes = Some(1);
            } else {
                tuning.lab.switches.superblocks = Some(true);
                tuning.lab.switches.small = Some(Box::new(SmallLab::on()));
            }
        });
    if tiled {
        let cfg = imij::v4::BoundedLossyConfig::new(info.row_bytes() as u64, 8 << 20);
        imij::v4::encode_lossy_bounded(
            &mut std::io::Cursor::new(&source),
            fs::File::create(output)?,
            std::io::Cursor::new(Vec::new()),
            None,
            info,
            &Metadata::default(),
            &config,
            cfg,
            &how_far::NoPulse,
        )?;
    } else {
        let file = config.encode_samples(ImageRef::packed(&source, &info), &Metadata::default())?;
        fs::write(output, file)?;
    }
    Ok(())
}

fn crop(route: &str, info: &ImageInfo) -> Rect {
    let w = info.width;
    let h = info.height;
    match route {
        "whole" => Rect::new(0, 0, w, h),
        "roi-small" => {
            let width = w.min(48);
            let height = h.min(48);
            Rect::new((w - width) / 3, (h - height) / 3, width, height)
        }
        "roi-cross" | "roi-open" => {
            let width = w.min(384);
            let height = h.min(320);
            let x = if w > 512 {
                256 - width / 2
            } else {
                (w - width) / 2
            };
            let y = if h > 384 {
                128 - height / 3
            } else {
                (h - height) / 2
            };
            Rect::new(x, y, width, height)
        }
        _ => panic!("unknown route"),
    }
}

fn verify_feed(file: &[u8], pixels: &[u8], info: &ImageInfo) -> Result {
    let mut decoder = DecodeConfig::default().job().feed_decoder(4093)?;
    let mut at = 0;
    let mut rows = 0;
    let mut finished_input = false;
    let width = info.row_bytes();
    loop {
        match decoder.advance()? {
            FeedStatus::NeedInput if at < file.len() => {
                let accepted = decoder.push(&file[at..])?;
                assert!(accepted > 0, "feed made no input progress");
                at += accepted;
            }
            FeedStatus::NeedInput => {
                assert!(!finished_input, "feed needs input after EOF");
                decoder.finish_input()?;
                finished_input = true;
            }
            FeedStatus::Row { y } => {
                assert_eq!(y, rows);
                let row = decoder.row()?.ok_or("missing decoded row")?;
                let start = y as usize * width;
                assert_eq!(row.row(0), &pixels[start..start + width]);
                rows += 1;
            }
            FeedStatus::Complete => {
                assert_eq!(rows, info.height);
                assert_eq!(decoder.usage().input_bytes, file.len() as u64);
                return Ok(());
            }
            _ => return Err("unexpected feed status".into()),
        }
    }
}

fn verify(base: &[u8], joint: &[u8]) -> Result {
    let info = imij::header(base)?.info;
    assert_eq!(imij::header(joint)?.info, info);
    let stride = info.row_bytes();
    let mut pixels = vec![0; info.checked_buffer_len().ok_or("image extent")?];
    let mut other = vec![0; pixels.len()];
    imij::decode_into_threads(base, &mut pixels, stride, 4)?;
    imij::decode_into_threads(joint, &mut other, stride, 4)?;
    assert!(pixels == other, "whole-image mismatch");
    for bytes in [base, joint] {
        verify_feed(bytes, &pixels, &info)?;
    }
    let hash = xxhash_rust::xxh3::xxh3_128(&pixels);
    for route in ["roi-small", "roi-cross"] {
        let rect = crop(route, &info);
        let row = rect.width() as usize * 3;
        let padded = row + 7;
        for workers in [1, 2, 4, 8] {
            for (name, bytes) in [("base", base), ("joint", joint)] {
                let mut reader =
                    RegionReader::open_with_limits(SliceReader::new(bytes), &Limits::NONE)?;
                reader.reset_read_stats();
                let mut output = vec![0xa7; padded * rect.height() as usize];
                reader.decode_region_threads(rect, &mut output, padded, workers)?;
                for (dy, actual) in output.chunks_exact(padded).enumerate() {
                    let offset = (rect.y() as usize + dy) * stride + rect.x() as usize * 3;
                    assert_eq!(&actual[..row], &pixels[offset..offset + row]);
                    assert_eq!(&actual[row..], &[0xa7; 7]);
                }
                let stats = reader.read_stats();
                println!(
                    "{}",
                    json!({"route":route,"arm":name,"workers":workers,"reads":stats.reads,"bytes":stats.bytes})
                );
            }
        }
    }
    println!(
        "{}",
        json!({"width":info.width,"height":info.height,"pixels_xxh3_128":format!("{hash:032x}"),"base_bytes":base.len(),"joint_bytes":joint.len()})
    );
    Ok(())
}

struct ArcReader(Arc<[u8]>);

impl ReadAt for ArcReader {
    fn len(&mut self) -> std::result::Result<u64, imij::Error> {
        Ok(self.0.len() as u64)
    }

    fn read_exact_at(
        &mut self,
        offset: u64,
        output: &mut [u8],
    ) -> std::result::Result<(), imij::Error> {
        SliceReader::new(&self.0).read_exact_at(offset, output)
    }
}

fn measure(base: &[u8], joint: &[u8], output: &Path, route: &str, trials: &[usize]) -> Result {
    let info = imij::header(base)?.info;
    assert_eq!(imij::header(joint)?.info, info);
    let base = Arc::<[u8]>::from(base);
    let joint = Arc::<[u8]>::from(joint);
    let rect = crop(route, &info);
    let row = rect.width() as usize * 3;
    let len = row * rect.height() as usize;
    let gate = zenbench::GateConfig::strict()
        .max_heavy_processes(4)
        .min_available_ram_mb(4096)
        .max_wait(Duration::from_secs(12));
    let results = zenbench::run_gated(gate, |suite| {
        for &workers in trials {
            suite.group(format!("{route}-{workers}t"), |group| {
                group
                    .config()
                    .min_rounds(24)
                    .max_rounds(24)
                    .auto_rounds(false)
                    .sample_target_ns(1_000_000)
                    .min_sample_ns(1_000_000)
                    .warmup_time(Duration::from_millis(60))
                    .max_wall_time(Duration::from_secs(300))
                    .bootstrap_resamples(1000);
                group.throughput(zenbench::Throughput::Elements(
                    u64::from(rect.width()) * u64::from(rect.height()),
                ));
                for (name, file) in [("base", &base), ("joint", &joint), ("aa", &base)] {
                    let file = Arc::clone(file);
                    let mut pixels = vec![0u8; len];
                    match route {
                        "whole" => group.bench(name, move |b| {
                            b.iter(|| {
                                imij::decode_into_threads(&file, &mut pixels, row, workers)
                                    .unwrap();
                                black_box(&pixels);
                            });
                        }),
                        "roi-open" => group.bench(name, move |b| {
                            b.iter(|| {
                                let mut reader = RegionReader::open_with_limits(
                                    SliceReader::new(&file),
                                    &Limits::NONE,
                                )
                                .unwrap();
                                reader
                                    .decode_region_threads(rect, &mut pixels, row, workers)
                                    .unwrap();
                                black_box(&pixels);
                            });
                        }),
                        _ => {
                            let mut reader =
                                RegionReader::open_with_limits(ArcReader(file), &Limits::NONE)
                                    .unwrap();
                            group.bench(name, move |b| {
                                b.iter(|| {
                                    reader
                                        .decode_region_threads(rect, &mut pixels, row, workers)
                                        .unwrap();
                                    black_box(&pixels);
                                });
                            });
                        }
                    }
                }
            });
        }
    });
    results.save(output)?;
    Ok(())
}

fn main() -> Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("encode") => encode(&args[1..], false),
        Some("encode-tiled") => encode(&args[1..], true),
        Some("verify") if args.len() == 3 => verify(&fs::read(&args[1])?, &fs::read(&args[2])?),
        Some("identity") => {
            let path = std::env::current_exe()?;
            let executable = fs::read(&path)?;
            println!(
                "{}",
                json!({"source": option_env!("IMI_JOINT_SOURCE"), "layout": option_env!("IMI_JOINT_LAYOUT"), "binary_xxh3_128": format!("{:032x}",xxhash_rust::xxh3::xxh3_128(&executable))})
            );
            Ok(())
        }
        _ => {
            let base = fs::read(std::env::var("IMI_JOINT_BASE")?)?;
            let joint = fs::read(std::env::var("IMI_JOINT_FILE")?)?;
            let output = std::env::var("IMI_JOINT_OUTPUT")?;
            let route = std::env::var("IMI_JOINT_ROUTE")?;
            let workers =
                std::env::var("IMI_JOINT_WORKERS").unwrap_or_else(|_| "1,2,4,8".to_string());
            let workers = workers
                .split(',')
                .map(str::parse)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            measure(&base, &joint, Path::new(&output), &route, &workers)
        }
    }
}
