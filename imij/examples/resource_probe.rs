//! Bounded adversarial shapes for external heap/RSS accounting, not timing.
//! resource_probe prepare DIR | decode FILE [slice|reader|stream]
use imij::*;
use std::{fs, path::Path};

fn save(dir: &Path, name: &str, info: ImageInfo, cfg: ImiEncoderConfig) {
    let mut px = vec![0; info.checked_buffer_len().unwrap()];
    px[1] = 255;
    let last = px.len() - 1;
    px[last] = 128;
    let file = cfg
        .encode_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    assert_eq!(decode(&file).unwrap().1, px);
    fs::write(dir.join(format!("{name}.imi")), file).unwrap();
}

fn prepare(dir: &Path) {
    fs::create_dir(dir).expect("use a new directory");
    for width in [65536, 262144] {
        for lanes in [1, 16] {
            save(
                dir,
                &format!("lossless-wide-{width}-lanes{lanes}"),
                ImageInfo::new(width, 1, Channels::Gray, SampleType::U8),
                {
                    let mut cfg = imij::ImiEncoderConfig::default();
                    cfg = cfg.tune_lossless(|t| {
                        t.lanes = Some(lanes);
                    });
                    cfg = cfg.tune_lossless(|t| {
                        t.palette = false;
                    });
                    cfg = cfg.with_column_groups(Some(0));
                    cfg
                },
            );
        }
        let info = ImageInfo::new(width, 1, Channels::Gray, SampleType::U8);
        let px = vec![128; width as usize];
        for groups in [0, 1024] {
            let mut cfg = imij::ImiEncoderConfig::lossy(85.0);
            cfg = cfg.with_column_groups(Some(groups));
            let file = cfg
                .encode_samples(
                    imij::ImageRef::new(&px, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap();
            decode(&file).unwrap();
            fs::write(
                dir.join(format!("lossy-wide-{width}-group{groups}.imi")),
                file,
            )
            .unwrap();
        }
    }
    for width in [64, 256] {
        for tiny in [false, true] {
            save(
                dir,
                &format!("units-{width}-tiny{tiny}"),
                ImageInfo::new(width, width, Channels::Gray, SampleType::U8),
                {
                    let mut cfg = imij::ImiEncoderConfig::default();
                    cfg = cfg.tune_lossless(|t| {
                        t.stripe_rows = Some(if tiny { 1 } else { 64 });
                    });
                    cfg = cfg.with_column_groups(Some(if tiny { 1 } else { 0 }));
                    cfg = cfg.tune_lossless(|t| {
                        t.palette = false;
                    });
                    cfg
                },
            );
        }
    }
    for count in [1, 8192] {
        let info = ImageInfo::new(1, 1, Channels::Gray, SampleType::U8);
        let meta = Metadata {
            chunks: (0..count)
                .map(|_| Chunk {
                    tag: *b"teST",
                    data: vec![0; 64],
                })
                .collect(),
            ..Default::default()
        };
        let file = imij::ImiEncoderConfig::default()
            .encode_samples(imij::ImageRef::new(&[7], 1, &info), &meta)
            .unwrap();
        fs::write(dir.join(format!("metadata-{count}.imi")), file).unwrap();
    }
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    match args[1].as_str() {
        "prepare" => prepare(Path::new(&args[2])),
        "decode" => {
            let file = fs::read(&args[2]).unwrap();
            let info = header(&file).unwrap().info;
            let expected = info.checked_buffer_len().unwrap();
            let limits = {
                let mut limits = Limits::DEFAULT;
                limits.max_pixels = info.width as u64 * info.height as u64;
                // 256 permits the tiny metadata fixture's model/record reads.
                limits.max_bytes = (expected as u64).max(256);
                limits
            };
            let mode = args.get(3).map(String::as_str).unwrap_or("slice");
            let mut output_bytes = 0;
            let mut retained_metadata = 0;
            match mode {
                "slice" => {
                    output_bytes = decode_with_limits(&file, &limits).unwrap().1.len();
                }
                "reader" => {
                    let (_, meta) = decode_reader(&file[..], &limits, |_, _, r| {
                        output_bytes += r.len();
                        Ok(())
                    })
                    .unwrap();
                    retained_metadata = meta.chunks.iter().map(|c| c.data.len()).sum();
                }
                "stream" => {
                    let mut decoder = StreamDecoder::new(&file).unwrap();
                    let mut row = vec![0; info.row_bytes()];
                    while decoder.next_row(&mut row).unwrap() {
                        output_bytes += row.len();
                    }
                }
                _ => panic!("unknown mode"),
            }
            assert_eq!(output_bytes, expected);
            println!(
                "file={} mode={mode} input_bytes={} output_bytes={output_bytes} max_bytes={} retained_metadata_payload={retained_metadata}",
                args[2],
                file.len(),
                limits.max_bytes
            );
        }
        _ => panic!("usage: resource_probe prepare DIR | decode FILE [slice|reader|stream]"),
    }
}
