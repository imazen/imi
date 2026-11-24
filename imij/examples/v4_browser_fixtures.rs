//! Bounded V4 integration fixtures. Source/native V3 pixels are the oracle.
//! Generated files belong outside the repository; output must be a new directory.
#[path = "../../tools/imij-wasm/web/crate/examples/browser_fixtures.rs"]
#[allow(dead_code)]
mod current;
use imij::v4::{NumericFileEncoder, ObjectIntegrity, SpatialConfig, TileLimits, TileOrder};
use imij::{Channels, ImageInfo, ImageRef, ImiEncoderConfig, Metadata, SampleType};
use std::{fs, io::Cursor, path::Path};

fn pixels(info: &ImageInfo) -> Vec<u8> {
    (0..info.width * info.height)
        .flat_map(|i| {
            let x = i % info.width;
            let y = i / info.width;
            [
                (x.wrapping_mul(29) ^ y.wrapping_mul(13)) as u8,
                (x / 7 + y / 5) as u8,
                ((x ^ y).wrapping_mul(41)) as u8,
            ]
        })
        .collect()
}
fn trace(dir: &Path, name: &str, file: &[u8], info: &ImageInfo, expected: &[u8]) {
    let json = imij::lab::codestream_trace(file);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["decode"], "ok");
    assert_eq!(value["checks"]["problems"], serde_json::json!([]));
    assert_eq!(value["checks"]["unattributed"], serde_json::json!([]));
    fs::write(dir.join(format!("{name}.json")), json).unwrap();
    let rgba: Vec<u8> = expected
        .chunks_exact(info.channels.count())
        .flat_map(|p| [p[0], p[1], p[2], 255])
        .collect();
    fs::write(dir.join(format!("{name}.rgba")), rgba).unwrap();
}
fn main() {
    let root = std::env::args().nth(1).expect("fresh output root required");
    let root = Path::new(&root);
    fs::create_dir(root).unwrap();
    current::generate(&root.join("current"));
    let dir = root.join("v4");
    fs::create_dir(&dir).unwrap();
    let traces = root.join("traces");
    fs::create_dir(&traces).unwrap();
    let mut entries = Vec::new();
    for quick in [false, true] {
        let info = ImageInfo::new(769, 1793, Channels::Rgb, SampleType::U8);
        let expected = pixels(&info);
        let cfg = if quick {
            ImiEncoderConfig::quick()
        } else {
            ImiEncoderConfig::lossless()
        }
        .with_threads(1)
        .with_row_groups(imij::RowGroups::new(128, 64, 32 << 20));
        let spatial = SpatialConfig {
            tile_columns: 2,
            tile_rows: 9,
            order: TileOrder::Morton,
            integrity: ObjectIntegrity::ObjectsAndTiles,
            limits: TileLimits {
                max_object_bytes: 1 << 20,
                max_tile_bytes: 16 << 20,
            },
        };
        let mut writer = NumericFileEncoder::new(
            info.clone(),
            &Metadata::default(),
            &cfg,
            spatial,
            Vec::new(),
            Cursor::new(Vec::new()),
            1 << 20,
            32 << 20,
        )
        .unwrap();
        let mut tiles = Vec::new();
        for _ in 0..writer.layout().tile_count() {
            let start = writer.bytes_written();
            writer.begin_tile().unwrap();
            let mut rects = Vec::new();
            while let Some(g) = writer.next_group() {
                let at = g.y as usize * info.row_bytes() + g.x as usize * 3;
                writer
                    .push_group(&expected[at..], info.row_bytes())
                    .unwrap();
                rects.push([g.x, g.y, g.width, g.height]);
            }
            let tile = writer.end_tile().unwrap();
            tiles.push(serde_json::json!({"id":tile.tile_id.to_string(),"start":start,"end":writer.bytes_written(),"groups":rects}));
        }
        let (file, _) = writer.finish().unwrap();
        assert_eq!(imij::decode(&file).unwrap().1, expected);
        let name = if quick {
            "portrait-quick"
        } else {
            "portrait-lossless"
        };
        fs::write(dir.join(format!("{name}.imi")), &file).unwrap();
        fs::write(dir.join(format!("{name}.raw")), expected).unwrap();
        entries.push(
            serde_json::json!({"name":name,"width":info.width,"height":info.height,"tiles":tiles}),
        );
        let small = ImageInfo::new(65, 97, Channels::Rgb, SampleType::U8);
        let raw = pixels(&small);
        let file = imij::v4::encode_numeric(
            small.clone(),
            &raw,
            small.row_bytes(),
            &Metadata::default(),
            &cfg,
            spatial,
        )
        .unwrap();
        trace(&traces, name, &file, &small, &raw);
    }
    for entropy in [imij::LossyEntropy::RunLevel, imij::LossyEntropy::SplitHuf] {
        for (label, transform, sampling) in [
            (
                "full",
                imij::LossyTransform::Dct8,
                imij::ChannelSampling::Full,
            ),
            (
                "small",
                imij::LossyTransform::Dct4,
                imij::ChannelSampling::Full,
            ),
            (
                "sampled",
                imij::LossyTransform::Adaptive4,
                imij::ChannelSampling::HalfBoth,
            ),
        ] {
            let info = ImageInfo::new(65, 97, Channels::Rgb, SampleType::U8);
            let raw = pixels(&info);
            let cfg = ImiEncoderConfig::lossy(71.)
                .with_effort(0.)
                .with_column_groups(Some(32))
                .with_checksum(true)
                .tune_lossy(|t| {
                    t.entropy = entropy;
                    t.stripe_block_rows = 2;
                    t.channel_layouts = [imij::LossyChannelLayout::new(transform, sampling); 3];
                });
            let native = cfg
                .encode_legacy_samples(ImageRef::packed(&raw, &info), &Metadata::default())
                .unwrap();
            let expected = imij::decode(&native).unwrap().1;
            let file = cfg
                .encode_samples(ImageRef::packed(&raw, &info), &Metadata::default())
                .unwrap();
            assert_eq!(imij::decode(&file).unwrap().1, expected);
            let name = format!("lossy-{entropy:?}-{label}");
            fs::write(dir.join(format!("{name}.imi")), &file).unwrap();
            fs::write(dir.join(format!("{name}.raw")), &expected).unwrap();
            trace(&traces, &name, &file, &info, &expected);
            entries.push(serde_json::json!({"name":name,"width":info.width,"height":info.height}));
        }
        let info = ImageInfo::new(273, 259, Channels::Rgb, SampleType::U8);
        let mut raw = pixels(&info);
        for y in 0..info.height {
            for x in 0..128 {
                let at = (y * info.width + x) as usize * 3;
                raw[at..at + 3].copy_from_slice(&[
                    ((x + y) / 2) as u8,
                    (45 + x / 4) as u8,
                    (65 + y / 4) as u8,
                ]);
            }
        }
        let cfg = ImiEncoderConfig::lossy(20.)
            .with_column_groups(Some(256))
            .with_checksum(true)
            .tune_lossy(|t| {
                t.entropy = entropy;
                t.adaptive_partitions = true;
                t.small_transforms = false;
                t.exact_blocks = Some(0.0);
                t.stripe_block_rows = 32;
            });
        let native = cfg
            .encode_legacy_samples(ImageRef::packed(&raw, &info), &Metadata::default())
            .unwrap();
        let expected = imij::decode(&native).unwrap().1;
        let file = imij::upgrade_image(&native, &imij::UpgradeLimits::default()).unwrap();
        assert_eq!(imij::decode(&file).unwrap().1, expected);
        let name = format!("lossy-{entropy:?}-range-partitions");
        let json = imij::lab::codestream_trace(&file);
        assert!(
            json.contains("partition range map"),
            "{name} did not emit a range map"
        );
        fs::write(dir.join(format!("{name}.imi")), &file).unwrap();
        fs::write(dir.join(format!("{name}.raw")), &expected).unwrap();
        trace(&traces, &name, &file, &info, &expected);
        entries.push(serde_json::json!({"name":name,"width":info.width,"height":info.height}));
    }
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_string_pretty(&entries).unwrap(),
    )
    .unwrap();
    println!(
        "{} V4 tile/lossy fixtures; native bits and V3 lossy reconstruction checked",
        entries.len()
    );
}
