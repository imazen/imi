//! Exercise the actual producer executables and independent source/V3 oracles.
use imij::planes::{Rect, SliceReader};
use imij::{Channels, ImageInfo, ImageRef, ImiEncoderConfig, Metadata, SampleType};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
fn fresh(label: &str) -> PathBuf {
    let root = PathBuf::from(std::env::var_os("HOME").unwrap()).join("tmp");
    fs::create_dir_all(&root).unwrap();
    let dir = root.join(format!(
        "imi-v4-producer-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&dir).unwrap();
    dir
}
fn run(binary: &str, args: &[&str]) {
    let out = Command::new(binary).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{args:?}\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
fn name(p: &Path) -> &str {
    p.to_str().unwrap()
}
fn pixels(w: usize, h: usize) -> Vec<u8> {
    (0..w * h)
        .flat_map(|i| {
            [
                (i % w * 17) as u8,
                (i / w * 29) as u8,
                (i.wrapping_mul(71) ^ (i / w)) as u8,
            ]
        })
        .collect()
}
fn image(path: &Path, w: u32, h: u32, expected: &[u8]) {
    let file = fs::read(path).unwrap();
    let declared = imij::v4::header(&file, 64 << 10).unwrap();
    assert_eq!((declared.info.width, declared.info.height), (w, h));
    assert_eq!(imij::decode(&file).unwrap().1, expected);
    let mut reader = imij::region::RegionReader::open(SliceReader::new(&file)).unwrap();
    assert!(reader.has_unit_table());
    let rw = w.min(19);
    let rh = h.min(17);
    let stride = rw as usize * 3 + 7;
    let mut out = vec![231; stride * rh as usize];
    reader
        .decode_region(Rect::new(w - rw, h - rh, rw, rh), &mut out, stride)
        .unwrap();
    for y in 0..rh as usize {
        let at = ((h - rh) as usize + y) * w as usize * 3 + (w - rw) as usize * 3;
        assert_eq!(
            &out[y * stride..y * stride + rw as usize * 3],
            &expected[at..at + rw as usize * 3]
        );
        assert!(
            out[y * stride + rw as usize * 3..(y + 1) * stride]
                .iter()
                .all(|&b| b == 231)
        );
    }
}
#[test]
fn four_source_mosaic_is_v4_with_exact_seams_and_clipped_indexed_regions() {
    let dir = fresh("mosaic");
    let side = 129usize;
    let w = side * 2;
    let raw = pixels(w, w);
    let mut paths = Vec::new();
    for ty in 0..2 {
        for tx in 0..2 {
            let mut tile = Vec::new();
            for y in 0..side {
                let at = ((ty * side + y) * w + tx * side) * 3;
                tile.extend_from_slice(&raw[at..at + side * 3]);
            }
            let p = dir.join(format!("{tx}-{ty}.rgb"));
            fs::write(&p, tile).unwrap();
            paths.push(p);
        }
    }
    let output = dir.join("mosaic.imi");
    run(
        env!("CARGO_BIN_EXE_nasa_prepare"),
        &[
            "grid",
            "129",
            "64",
            "32",
            name(&paths[0]),
            name(&paths[1]),
            name(&paths[2]),
            name(&paths[3]),
            name(&output),
        ],
    );
    image(&output, w as u32, w as u32, &raw);
}
#[test]
fn png_import_preserves_source_codes() {
    let dir = fresh("png");
    let raw = pixels(65, 37);
    let rgb: Vec<rgb::RGB8> = raw
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| rgb::RGB8::new(p[0], p[1], p[2]))
        .collect();
    let png = zenpng::encode_rgb8(
        imgref::ImgRef::new(&rgb, 65, 37),
        None,
        &zenpng::EncodeConfig::default(),
        &enough::Unstoppable,
        &enough::Unstoppable,
    )
    .unwrap();
    let input = dir.join("source.png");
    let output = dir.join("source.rgb");
    fs::write(&input, png).unwrap();
    run(
        env!("CARGO_BIN_EXE_nasa_prepare"),
        &["png", name(&input), name(&output)],
    );
    assert_eq!(fs::read(output).unwrap(), raw);
}
#[test]
fn full_pyramid_and_archive_entries_use_v4_and_preserve_native_reconstruction() {
    let dir = fresh("pyramid");
    let (w, h) = (1033u32, 577u32);
    let raw = pixels(w as usize, h as usize);
    let input = dir.join("source.rgb");
    let output = dir.join("complete");
    fs::write(&input, &raw).unwrap();
    run(
        env!("CARGO_BIN_EXE_nasa_pyramid"),
        &[
            "complete-raw",
            &w.to_string(),
            &h.to_string(),
            name(&input),
            name(&output),
        ],
    );
    image(&output.join("full-lossless.imi"), w, h, &raw);
    for (divisor, lw, lh) in [(2, 517, 289), (4, 259, 145)] {
        let level = output.join(format!("pyramid/level-{divisor}"));
        let reference = fs::read(level.join("source.rgb")).unwrap();
        image(&level.join("lossless.imi"), lw, lh, &reference);
        assert!(level.join("lossless-cache.bin").is_file());
    }
    let bytes = fs::read(output.join("full-lossy.imijset")).unwrap();
    let sidecar = fs::read(output.join("full-lossy.bin")).unwrap();
    assert_eq!(&sidecar[..8], b"IMITILE1");
    let mut reader = imij_set::Reader::open(SliceReader::new(&bytes)).unwrap();
    let entry = reader.get(imij_set::CATALOG_KEY).unwrap().unwrap();
    let encoded_catalog = reader.read_entry_bounded(&entry, 64 << 10).unwrap();
    let catalog = imij_set::PyramidCatalog::decode(&encoded_catalog).unwrap();
    let level = catalog.level(0).unwrap();
    assert_eq!((level.width, level.height), (w, h));
    assert_eq!((level.tile_width, level.tile_height), (512, 256));
    assert_eq!(level.order, imij_set::TileOrder::RowMajor);
    assert_eq!(level.key_base, 1);
    let mut listing = reader.listing().unwrap();
    let mut count = 0;
    while let Some(entry) = listing.next(&mut reader).unwrap() {
        if entry.key() == imij_set::CATALOG_KEY {
            continue;
        }
        let col = count % w.div_ceil(512);
        let y = count / w.div_ceil(512) * 256;
        let x = col * 512;
        let cw = 512.min(w - x);
        let ch = 256.min(h - y);
        assert_eq!(entry.key(), level.tile_key(col, y / 256).unwrap());
        let mut input = Vec::new();
        for row in y..y + ch {
            let at = (row * w + x) as usize * 3;
            input.extend_from_slice(&raw[at..at + cw as usize * 3]);
        }
        let info = ImageInfo::new(cw, ch, Channels::Rgb, SampleType::U8);
        let cfg = ImiEncoderConfig::lossy(90.)
            .with_quality(imij::LossyQuality::JpegQuality(80.))
            .with_effort(10.)
            .with_decode_speed(imij::DecodeSpeed::Fast)
            .with_threads(1);
        let native = cfg
            .encode_legacy_samples(ImageRef::packed(&input, &info), &Metadata::default())
            .unwrap();
        let file = reader.read_entry_bounded(&entry, 8 << 20).unwrap();
        imij::v4::header(&file, 64 << 10).unwrap();
        let at = 28 + count as usize * 44;
        assert_eq!(
            u64::from_le_bytes(sidecar[at..at + 8].try_into().unwrap()),
            entry.offset()
        );
        assert_eq!(
            u32::from_le_bytes(sidecar[at + 8..at + 12].try_into().unwrap()) as u64,
            entry.len()
        );
        assert_eq!(
            imij::decode(&file).unwrap().1,
            imij::decode(&native).unwrap().1
        );
        count += 1;
    }
    assert_eq!(count, w.div_ceil(512) * h.div_ceil(256));
    assert_eq!(
        u32::from_le_bytes(sidecar[24..28].try_into().unwrap()),
        count
    );
    assert_eq!(sidecar.len(), 28 + count as usize * 44);
}

#[test]
fn native_pyramid_reuses_primary_and_matches_source_across_levels() {
    use imij::planes::{PlaneSetLimits, PlaneSetReader, ViewBacking};

    let dir = fresh("native-pyramid");
    let full = dir.join("source.rgb");
    let full_pixels = pixels(1033, 577);
    fs::write(&full, &full_pixels).unwrap();
    let levels = dir.join("levels");
    let binary = env!("CARGO_BIN_EXE_nasa_pyramid");
    run(
        binary,
        &["rasters-raw", "1033", "577", name(&full), name(&levels)],
    );
    let out = dir.join("native");
    run(
        binary,
        &[
            "native-raw",
            "1033",
            "577",
            name(&full),
            name(&levels),
            name(&out),
        ],
    );

    let file = fs::read(out.join("pyramid.imi")).unwrap();
    let mut reader =
        PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
    assert!(reader.has_record_pages());
    let images = reader.image_directory().unwrap();
    assert_eq!(images.original(), 1);
    assert_eq!(images.views().len(), 3);
    assert_eq!(images.view(4).unwrap().backing(), ViewBacking::Primary);
    assert!(!out.join("directory.scratch").exists());
    let manifest = fs::read_to_string(out.join("pyramid.json")).unwrap();
    assert!(manifest.contains(&format!(
        "{:x}",
        imij::HashAlgorithm::Xxh3_128.digest(&file)
    )));
    for (id, width, height, source) in [
        (1, 1033, 577, full_pixels),
        (
            2,
            517,
            289,
            fs::read(levels.join("level-2/source.rgb")).unwrap(),
        ),
        (
            4,
            259,
            145,
            fs::read(levels.join("level-4/source.rgb")).unwrap(),
        ),
    ] {
        let rect = Rect::new(0, 0, width, height);
        let row_bytes = width as usize * 3;
        let stride = row_bytes + 9;
        let mut decoded = vec![0xa7; stride * height as usize];
        reader
            .decode_view_region(id, rect, &mut decoded, stride)
            .unwrap();
        for (want, got) in source
            .chunks_exact(row_bytes)
            .zip(decoded.chunks_exact(stride))
        {
            assert_eq!(&got[..row_bytes], want);
            assert_eq!(&got[row_bytes..], &[0xa7; 9]);
        }
        if id == 4 {
            assert!(reader.view_region_pages(id, rect).unwrap().is_empty());
        } else {
            assert_eq!(reader.view_region_pages(id, rect).unwrap().len(), 1);
        }
        assert!(!reader.view_region_ranges(id, rect).unwrap().is_empty());
    }
}
