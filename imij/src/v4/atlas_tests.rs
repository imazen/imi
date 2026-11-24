use super::*;
use crate::{Channels, ImageInfo, ImageRef, ImiEncoderConfig, Metadata, SampleType};
use alloc::vec::Vec;

fn checked(file: &[u8]) -> serde_json::Value {
    let trace: serde_json::Value =
        serde_json::from_str(&crate::lab::codestream_trace(file)).unwrap();
    assert_eq!(trace["decode"], "ok", "{}", trace["decode"]);
    assert_eq!(trace["checks"]["problems"], serde_json::json!([]));
    assert_eq!(trace["checks"]["unattributed"], serde_json::json!([]));
    assert_eq!(trace["file"]["bytes"], file.len());
    assert_eq!(trace["spec"]["V4"], "SPEC_V4.md");
    for node in trace["nodes"].as_array().unwrap() {
        let start = node["start"].as_u64().unwrap();
        let end = node["end"].as_u64().unwrap();
        assert!(start <= end && end <= file.len() as u64);
        // Model, unit, bank and settings IDs remain full-width display strings,
        // never rounded JavaScript numbers.
        if node.get("value").is_some() {
            assert!(node["value"].is_string());
        }
    }
    trace
}

#[test]
fn numeric_atlas_attributes_every_byte_and_actual_spatial_group() {
    let info = ImageInfo::new(41, 31, Channels::Rgba, SampleType::U16);
    let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 73) as u8)
        .collect();
    for quick in [false, true] {
        let config = if quick {
            ImiEncoderConfig::quick()
        } else {
            ImiEncoderConfig::lossless()
        }
        .with_row_groups(crate::RowGroups::new(9, 7, 8 << 20));
        let spatial = SpatialConfig {
            tile_columns: 3,
            tile_rows: 2,
            order: TileOrder::Morton,
            integrity: ObjectIntegrity::ObjectsAndTiles,
            limits: TileLimits {
                max_object_bytes: 1 << 20,
                max_tile_bytes: 16 << 20,
            },
        };
        let original = encode_numeric(
            info.clone(),
            &pixels,
            info.row_bytes(),
            &Metadata::default(),
            &config,
            spatial,
        )
        .unwrap();
        for (placement, open) in [
            (DirectoryPlacement::Tail, false),
            (DirectoryPlacement::Tail, true),
            (DirectoryPlacement::Front, false),
            (DirectoryPlacement::Front, true),
            (DirectoryPlacement::None, false),
            (DirectoryPlacement::None, true),
        ] {
            let file = super::file_tests::rewrite_placement(&original, placement, open);
            let trace = checked(&file);
            let nodes = trace["nodes"].as_array().unwrap();
            let tiles: Vec<_> = nodes.iter().filter(|node| node["kind"] == "tile").collect();
            assert_eq!(tiles.len(), 6);
            for tile in tiles {
                assert!(tile["region"].is_array());
            }
            let groups: Vec<_> = nodes
                .iter()
                .filter(|node| node["kind"] == "record" && node["label"] == "numeric group")
                .collect();
            assert_eq!(groups.len(), 25);
            let mut actual: Vec<[u64; 4]> = groups
                .iter()
                .map(|node| core::array::from_fn(|i| node["region"][i].as_u64().unwrap()))
                .collect();
            let mut expected = Vec::new();
            for y in (0..31u64).step_by(7) {
                for x in (0..41u64).step_by(9) {
                    expected.push([x, y, 9.min(41 - x), 7.min(31 - y)]);
                }
            }
            actual.sort_unstable();
            expected.sort_unstable();
            assert_eq!(actual, expected);

            assert_eq!(
                groups
                    .iter()
                    .map(|node| node["region"][2].as_u64().unwrap()
                        * node["region"][3].as_u64().unwrap())
                    .sum::<u64>(),
                41 * 31
            );
        }
    }
}

#[test]
fn lossy_atlas_traces_models_settings_dependencies_and_checksum_outcomes() {
    let info = ImageInfo::new(41, 31, Channels::Rgba, SampleType::U8);
    let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 73) as u8)
        .collect();
    for entropy in [crate::LossyEntropy::RunLevel, crate::LossyEntropy::SplitHuf] {
        let config = ImiEncoderConfig::lossy(71.0)
            .with_checksum(true)
            .with_column_groups(Some(16))
            .with_coding_defaults(true)
            .tune_lossy(|t| {
                t.entropy = entropy;
                t.stripe_block_rows = 2;
            });
        let native = config
            .encode_samples(ImageRef::packed(&pixels, &info), &Metadata::default())
            .unwrap();
        let file = upgrade_image(&native, &crate::UpgradeLimits::default()).unwrap();
        let trace = checked(&file);
        let nodes = trace["nodes"].as_array().unwrap();
        assert!(
            nodes
                .iter()
                .any(|node| node["label"] == "immutable model definition")
        );
        assert!(
            nodes
                .iter()
                .any(|node| node["label"] == "image defaults and reusable settings")
        );
        let groups: Vec<_> = nodes
            .iter()
            .filter(|node| node["kind"] == "record" && node["label"] == "AC group")
            .collect();
        assert_eq!(groups.len(), 6);
        for group in groups {
            assert!(group["region"].is_array());
            let note = group["notes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|note| note[0] == "V4 effective cells")
                .expect("resolved V4 settings");
            let grid: serde_json::Value = serde_json::from_str(note[1].as_str().unwrap()).unwrap();
            let width = group["region"][2].as_u64().unwrap();
            let height = group["region"][3].as_u64().unwrap();
            assert_eq!(grid["columns"], width.div_ceil(16));
            assert_eq!(grid["rows"], height.div_ceil(16));
            let runs = grid["runs"].as_array().unwrap();
            assert_eq!(runs.len(), 1, "uniform default cells form one run");
            assert_eq!(runs[0][0], width.div_ceil(16) * height.div_ceil(16));
            assert_eq!(runs[0][1], 0);
            assert_eq!(
                runs[0][2],
                serde_json::json!([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])
            );
            assert_eq!(runs[0][3], serde_json::json!([0, 0, 0, 0]));
            assert!(
                group["notes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|note| note[0] == "model dependency")
            );
        }
        let mut damaged = file;
        let last = damaged.len() - 1;
        damaged[last] ^= 1;
        let trace: serde_json::Value =
            serde_json::from_str(&crate::lab::codestream_trace(&damaged)).unwrap();
        assert_ne!(trace["decode"], "ok");
    }
}

#[test]
fn native_plane_atlas_distinguishes_records_pages_and_locator() {
    use crate::planes::{
        ImageDirectory, ImageView, ImageViewRole, LevelDescriptor, PlaneDescriptor,
        PlaneSetBuilder, ViewBacking,
    };
    let primary = ImiEncoderConfig::lossless()
        .encode_samples(
            ImageRef::packed(
                &[42; 20],
                &ImageInfo::new(5, 4, Channels::Gray, SampleType::U8),
            ),
            &Metadata::default(),
        )
        .unwrap();
    let mut builder = PlaneSetBuilder::new(&primary).unwrap();
    let plane = builder
        .add_plane(PlaneDescriptor::new("value", SampleType::U8))
        .unwrap();
    builder
        .add_level(
            plane,
            LevelDescriptor::new(19, 13, 4, 3),
            &[42; 19 * 13],
            19,
            ImiEncoderConfig::lossless(),
        )
        .unwrap();
    let images = ImageDirectory::new(
        0,
        alloc::vec![ImageView::new(
            0,
            "original",
            ImageViewRole::Original,
            ViewBacking::Level { plane, level: 0 },
        )],
    )
    .unwrap();
    builder.set_image_directory(images).unwrap();
    builder.set_record_pages(16).unwrap();
    let file = builder.finish().unwrap();
    if let Some(path) = std::env::var_os("IMIJ_ATLAS_NATIVE_PAGE_FIXTURE") {
        std::fs::write(path, &file).unwrap();
    }
    let trace = checked(&file);
    let nodes = trace["nodes"].as_array().unwrap();
    for (label, count) in [
        ("pREC bound native record", 25),
        ("pIPG native record page", 2),
        ("pIDX native directory", 1),
        ("sLOC plane directory locator", 1),
    ] {
        assert_eq!(
            nodes
                .iter()
                .filter(|n| n["kind"] == "chunk" && n["label"] == label)
                .count(),
            count,
            "{label}"
        );
    }
    let notes = |key: &str| {
        nodes
            .iter()
            .filter_map(|n| n["notes"].as_array())
            .flatten()
            .filter(|note| note[0] == key)
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(notes("length encoding").len(), 2);
    assert_eq!(
        notes("records in page"),
        [
            serde_json::json!(["records in page", "16"]),
            serde_json::json!(["records in page", "9"]),
        ]
    );
    assert_eq!(
        notes("nominal records per page"),
        [serde_json::json!(["nominal records per page", "16"]),]
    );
}

#[test]
fn atlas_attributes_scoped_check_results_to_their_exact_widths() {
    use crate::HashAlgorithm;
    use crate::decode_resources::{DecodePolicy, DecodeSession};
    let info = ImageInfo::new(37, 31, Channels::Rgb, SampleType::U8);
    let pixels: Vec<_> = (0..info.checked_buffer_len().unwrap())
        .map(|x| (x * 61) as u8)
        .collect();
    let descriptor = [
        CheckDescriptor::xxh3(HashAlgorithm::Xxh3_64),
        CheckDescriptor::xxh3(HashAlgorithm::Xxh3_128),
    ];
    let mut definition = Vec::new();
    Checks::append(&mut definition, [&descriptor; 3], CheckLimits::default()).unwrap();
    let (checks, _) = Checks::read(&definition, CheckLimits::default()).unwrap();
    let session = DecodeSession::new(DecodePolicy::default());
    for config in [ImiEncoderConfig::lossless(), ImiEncoderConfig::lossy(80.0)] {
        let encoded = config
            .encode_samples(ImageRef::packed(&pixels, &info), &Metadata::default())
            .unwrap();
        let output = add_scoped_checks(&encoded, checks, &session, 8 << 20).unwrap();
        let trace = checked(&output);
        let nodes = trace["nodes"].as_array().unwrap();
        assert!(
            nodes
                .iter()
                .any(|n| n["label"] == "scoped check declarations")
        );
        for label in [
            "group pixel check results",
            "tile pixel check results",
            "encoded object check results",
        ] {
            let results: Vec<_> = nodes
                .iter()
                .filter(|n| n["label"] == label && n["kind"] == "digest")
                .collect();
            assert!(!results.is_empty(), "{label}");
            for result in results {
                assert_eq!(
                    result["end"].as_u64().unwrap() - result["start"].as_u64().unwrap(),
                    24
                );
            }
        }
    }
}
