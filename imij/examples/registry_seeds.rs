//! Migrate the curated version-2 seeds to the current numeric registry.
//! Usage: registry_seeds OLD_SEEDS NEW_DIRECTORY (the output must be new).
//! Compressed units and record payloads stay unchanged except the stale f32
//! parameter layout, retired animation entropy and superseded exact-block heads,
//! regenerated below. Header records keep
//! their assignments; only payload-head ids and the unknown-feature witness move.
#![forbid(unsafe_code)]

extern crate alloc;

#[path = "../tests/common/mod.rs"]
mod common;
#[path = "../src/registry.rs"]
mod registry;

use imij::{Error, FrameDecoder, FrameKind};
use std::{fs, path::Path};

fn migrate(file: &[u8]) -> Vec<u8> {
    let h = imij::header(file).unwrap();
    let range = common::data_payload(file);
    let d = &file[range.clone()];
    let lossy = h.coding.is_lossy();
    let p = common::payload_head(d, &h.info, lossy);
    let mut out = file.to_vec();
    if let Some((_, list)) = p.list {
        let mut records: Vec<_> = p
            .head_records
            .iter()
            .map(|(id, payload)| {
                let id = match (*id, lossy) {
                    (2, _) => registry::HEAD_UNIT_TABLE,
                    (4, _) => registry::HEAD_UNIT_RESERVE,
                    (21, false) => registry::HEAD_BILEVEL,
                    (19, true) => registry::HEAD_LAYOUTS,
                    _ => *id,
                };
                (id, d[payload.clone()].to_vec())
            })
            .collect();
        records.sort_by_key(|r| r.0);
        let mut replacement = Vec::new();
        for (id, payload) in records {
            common::put_varint(&mut replacement, id);
            common::put_varint(&mut replacement, payload.len() as u64);
            replacement.extend_from_slice(&payload);
        }
        assert_eq!(replacement.len(), list.len(), "only one-byte ids move");
        out[range.start + list.start..range.start + list.end].copy_from_slice(&replacement);
    }
    let mut declared = common::feature_mask(&out);
    for (tag, at, end) in common::chunks(&out) {
        if &tag != b"fRAM" {
            continue;
        }
        let mut pos = at + 4 + common::varint(&out[at + 4..]).1;
        for _ in 0..4 {
            pos += common::varint(&out[pos..]).1;
        }
        let (present, n) = common::varint(&out[pos..]);
        pos += n;
        if present & 4 != 0 {
            let (len, n) = common::varint(&out[pos..]);
            pos += n + len as usize;
        }
        if present & 2 == 0 {
            declared |= imij::header(&out[pos..end]).unwrap().features.bits();
        }
    }
    if declared != common::feature_mask(&out) {
        out = common::with_feature_mask(&out, Some(declared));
    }
    if out != file && common::checksum::find(file, b"cSUM").is_some() {
        let (_, table) = common::checksum::find(file, b"cSUM").unwrap();
        if file[table.start] == 0 {
            let width = file[table.start + 1];
            let k = file[table.start + 2];
            let (a, b) = common::checksum::csum(&out);
            let covered = [&out[..a], &out[b..]].concat();
            let fresh = common::checksum::table_chunk(&covered, k, width);
            assert_eq!(fresh.len(), b - a);
            out[a..b].copy_from_slice(&fresh);
        }
    }
    out
}

// This seed predates the floor/color-scale presence-bit swap. Regenerate
// its f32 RGBA split-Huffman coverage using the current parameter layout.
fn f32_seed(file: &[u8]) -> Vec<u8> {
    let info = imij::header(file).unwrap().info;
    assert_eq!(info.sample, imij::SampleType::F32);
    assert_eq!(info.channels, imij::Channels::Rgba);
    let pixels: Vec<u8> = (0..info.row_bytes() * info.height as usize / 4)
        .flat_map(|i| ((i % 251) as f32 / 250.0).to_ne_bytes())
        .collect();
    let out = imij::ImiEncoderConfig::lossy(80.0)
        .tune_lossy(|t| t.entropy = imij::LossyEntropy::SplitHuf)
        .encode_samples(
            imij::ImageRef::new(&pixels, info.row_bytes(), &info),
            &imij::Metadata::default(),
        )
        .unwrap();
    let decoded = imij::decode(&out).unwrap().1;
    assert_eq!(decoded.len(), pixels.len(), "complete f32 output");
    for (source, actual) in pixels
        .as_chunks::<16>()
        .0
        .iter()
        .zip(decoded.as_chunks::<16>().0)
    {
        assert_eq!(&source[12..], &actual[12..], "lossy alpha remains exact");
    }
    out
}

// The old u16 inter-animation used a retired entropy mode. Keep six
// grayscale frames and inter prediction, with split-Huffman entropy.
fn inter_seed(file: &[u8]) -> Vec<u8> {
    use imij::{FrameConfig, FrameDuration, FrameEncoder};
    let info = imij::header(file).unwrap().info;
    assert_eq!(info.sample, imij::SampleType::U16);
    assert_eq!(info.channels, imij::Channels::Gray);
    let cfg = FrameConfig::new(
        imij::ImiEncoderConfig::lossy(90.0)
            .tune_lossy(|t| t.entropy = imij::LossyEntropy::SplitHuf),
    );
    let mut encoder = FrameEncoder::animation(info.clone(), imij::Metadata::default(), cfg);
    let originals: Vec<Vec<u8>> = (0..6)
        .map(|t| {
            (0..info.width * info.height)
                .flat_map(|i| {
                    let gray = if i % info.width == t { 60000u16 } else { 10000 };
                    gray.to_ne_bytes()
                })
                .collect()
        })
        .collect();
    for pixels in &originals {
        encoder
            .push_frame(pixels, info.row_bytes(), FrameDuration::from_millis(50))
            .unwrap();
    }
    let out = encoder.finish().unwrap();
    let mut decoder = FrameDecoder::new(&out).unwrap();
    let mut canvas = vec![0; info.checked_buffer_len().unwrap()];
    for index in 0..originals.len() {
        let head = decoder.next_frame().unwrap().unwrap();
        if index > 0 {
            assert!(head.uses_previous, "u16 animation retains inter prediction");
        }
        decoder
            .decode_into(&mut canvas, info.row_bytes(), 1)
            .unwrap();
    }
    assert!(decoder.next_frame().unwrap().is_none());
    out
}

#[cfg(feature = "lab")]
fn superblock_seed(file: &[u8], entropy: imij::LossyEntropy) -> Vec<u8> {
    let mut info = imij::header(file).unwrap().info;
    assert_eq!(info.sample, imij::SampleType::U8);
    assert_eq!(info.channels, imij::Channels::Rgb);
    // The frozen exact+superblock parity domain uses enough rows to retain
    // both selected text blocks and merged ramps after current selection.
    info.height = 96;
    let mut pixels = vec![0; info.checked_buffer_len().unwrap()];
    for y in 0..info.height as usize {
        for x in 0..info.width as usize {
            let at = y * info.row_bytes() + x * 3;
            for c in 0..3 {
                let ramp = (x as f32 * 0.7 + y as f32 * 0.4 + c as f32 * 40.0) % 200.0 + 20.0;
                let tex = if x > info.width as usize / 2 {
                    18.0 * ((x as f32 * 0.9 + c as f32).sin() * (y as f32 * 0.7).cos())
                } else {
                    0.0
                };
                let edge = if (x / 23 + y / 17) % 5 == 0 {
                    40.0
                } else {
                    0.0
                };
                pixels[at + c] = (ramp + tex + edge).clamp(0.0, 255.0) as u8;
            }
            if y < info.height as usize / 2 {
                let ink = (x % 9 < 2 && y % 13 > 2) || (y % 13 == 7 && x % 17 < 11);
                pixels[at..at + 3].copy_from_slice(if ink {
                    &[20, 30, 90]
                } else {
                    &[250, 250, 245]
                });
            }
        }
    }
    let mut cfg = imij::ImiEncoderConfig::lossy(40.0)
        .with_decode_speed(imij::DecodeSpeed::Compact)
        .tune_lossy(|t| t.entropy = entropy);
    cfg = cfg.tune_lossy(|t| t.lab.switches.superblocks = Some(true));
    let out = cfg
        .encode_samples(
            imij::ImageRef::new(&pixels, info.row_bytes(), &info),
            &imij::Metadata::default(),
        )
        .unwrap();
    let head = common::payload_head(&out[common::data_payload(&out)], &info, true);
    assert!(
        head.head_record(registry::HEAD_EXACT).is_some(),
        "seed needs exact blocks"
    );
    assert!(
        head.head_record(registry::HEAD_SUPERBLOCKS).is_some(),
        "seed needs superblocks"
    );
    let got = common::decode_all(&out, &info);
    let (dc_info, dc, _) = imij::lab::lossy_dc_image(&out).unwrap();
    let bytes = dc_info.sample.bytes();
    let mut selected = 0;
    for (b, p) in dc.chunks_exact(4 * bytes).enumerate() {
        let code = match dc_info.sample {
            imij::SampleType::U16 => u16::from_ne_bytes(p[6..8].try_into().unwrap()) as i32,
            imij::SampleType::I32 => i32::from_ne_bytes(p[12..16].try_into().unwrap()),
            other => panic!("unexpected DC sample {other:?}"),
        };
        assert!(code == 0 || code == 1);
        if code == 1 {
            selected += 1;
            let bx = b % (info.width as usize).div_ceil(8);
            let by = b / (info.width as usize).div_ceil(8);
            for y in by * 8..(by * 8 + 8).min(info.height as usize) {
                let row = y * info.row_bytes() + bx * 24
                    ..y * info.row_bytes() + ((bx + 1) * 8).min(info.width as usize) * 3;
                assert_eq!(&pixels[row.clone()], &got[row], "exact seed block {b}");
            }
        }
    }
    assert!(selected > 0);
    out
}

#[cfg(not(feature = "lab"))]
fn superblock_seed(_: &[u8], _: imij::LossyEntropy) -> Vec<u8> {
    panic!("superblock seed regeneration requires --features lab");
}

fn verify(name: &str, file: &[u8]) {
    let decoded = imij::decode(file);
    if name.starts_with("37-") || name.starts_with("38-") {
        assert!(
            matches!(decoded, Err(Error::Unsupported(_))),
            "{name}: {decoded:?}"
        );
        return;
    }
    let (info, _) = decoded.unwrap_or_else(|e| panic!("{name}: {e:?}"));
    let mut frames = FrameDecoder::new(file).unwrap();
    let kind = frames.kind();
    let mut history: Vec<(Vec<u8>, usize)> = Vec::new();
    while frames.next_frame().unwrap().is_some() {
        let frame_info = if kind == FrameKind::Animation {
            info.clone()
        } else {
            frames.frame_info().unwrap().clone()
        };
        let mut pixels = vec![0; frame_info.checked_buffer_len().unwrap()];
        if let Some(index) = frames.reference_index() {
            pixels.copy_from_slice(&history[index].0);
        }
        frames
            .decode_into(&mut pixels, frame_info.row_bytes(), 1)
            .unwrap();
        history.push((pixels, frame_info.row_bytes()));
    }
}

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    assert_eq!(
        args.len(),
        2,
        "usage: registry_seeds OLD_SEEDS NEW_DIRECTORY"
    );
    let old = Path::new(&args[0]);
    let new = Path::new(&args[1]);
    fs::create_dir(new).expect("use a new output directory");
    let mut entries: Vec<_> = fs::read_dir(old).unwrap().map(Result::unwrap).collect();
    entries.sort_by_key(|e| e.file_name());
    let mut failures = Vec::new();
    for entry in entries {
        if entry.path().extension().and_then(|s| s.to_str()) != Some("imi") {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_str().unwrap();
        let file = fs::read(entry.path()).unwrap();
        let result = std::panic::catch_unwind(|| {
            let changed = if name.starts_with("18-") {
                f32_seed(&file)
            } else if name.starts_with("20-") {
                inter_seed(&file)
            } else if name.starts_with("39-") {
                superblock_seed(&file, imij::LossyEntropy::RunLevel)
            } else if name.starts_with("40-") {
                superblock_seed(&file, imij::LossyEntropy::SplitHuf)
            } else if name.starts_with("38-") {
                common::with_feature_mask(&file, Some(1 << registry::FEATURE_SMALL_TRANSFORMS))
            } else if name.starts_with("37-") {
                file.clone()
            } else {
                migrate(&file)
            };
            verify(name, &changed);
            fs::write(new.join(name), &changed).unwrap();
            println!(
                "{name}\t{}\t{}\t{}",
                file.len(),
                changed.len(),
                file != changed
            );
        });
        if result.is_err() {
            failures.push(name.to_owned());
        }
    }
    assert!(
        failures.is_empty(),
        "seed verification failed: {failures:?}"
    );
}
