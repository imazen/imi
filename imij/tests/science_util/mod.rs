//! Fixtures shared by the science-record tests.
#![allow(dead_code)]

use imij::science::*;
use imij::{ImageInfo, ImiEncoderConfig, Metadata};

pub fn lossless() -> ImiEncoderConfig {
    let mut cfg = imij::ImiEncoderConfig::default();
    cfg = cfg.with_threads(1);
    cfg
}

pub fn gray16(w: u32, h: u32, seed: u16) -> Vec<u8> {
    (0..w * h)
        .flat_map(|i| (((i as u16).wrapping_mul(37) ^ seed) & 0x0fff).to_ne_bytes())
        .collect()
}

pub fn rgb8(w: u32, h: u32, seed: u8) -> Vec<u8> {
    (0..w * h * 3)
        .map(|i| (i as u8).wrapping_mul(29).wrapping_add(seed))
        .collect()
}

pub fn axis(name: &str, kind: AxisKind, binding: AxisBinding, unit: Option<&str>) -> Axis {
    let mut a = Axis::new(name, kind, binding);
    a.unit = unit.map(String::from);
    a
}

/// A CT slice: DICOM rescale to HU, a z coordinate, pixel spacing, an
/// affine and structured DICOM JSON plus a CBOR blob.
pub fn ct_records() -> ScienceRecords {
    let mut c = ChannelCalibration::default();
    c.offset = -1024.0;
    c.quantity = Some("HU".into());
    c.valid_min = Some(0.0);
    c.valid_max = Some(4095.0);
    c.nodata = vec![65535.0];
    let mut axes = Axes::new(vec![
        axis(
            "z",
            AxisKind::Space,
            AxisBinding::Coordinate,
            Some("millimeter"),
        ),
        axis("y", AxisKind::Space, AxisBinding::Rows, Some("millimeter")),
        axis(
            "x",
            AxisKind::Space,
            AxisBinding::Columns,
            Some("millimeter"),
        ),
    ]);
    axes.levels
        .push(AxisLevel::new(vec![2.5, 0.5, 0.5], vec![0.0, 0.0, 0.0]));
    axes.affine = Some(vec![
        1.0, 0.0, 0.0, 50.0, //
        0.0, 1.0, 0.0, -125.0, //
        0.0, 0.0, 1.0, -125.0,
    ]);
    let mut r = ScienceRecords::default();
    r.calibration.push(c);
    r.axes = Some(axes);
    r.coordinates = vec![42];
    r.structured.push(StructuredMetadata::new(
        "dicom-json",
        StructuredFormat::Json,
        br#"{"00281052":{"vr":"DS","Value":[-1024]},"00281053":{"vr":"DS","Value":[1]},"00281054":{"vr":"LO","Value":["HU"]}}"#.to_vec(),
    ));
    r.structured.push(StructuredMetadata::new(
        "example/cbor",
        StructuredFormat::Cbor,
        vec![0xa1, 0x61, 0x61, 0x01, 0xff, 0x00],
    ));
    r
}

pub fn with_records(info: &ImageInfo, records: &ScienceRecords) -> Metadata {
    let mut meta = Metadata::default();
    records.write_metadata(info, &mut meta).unwrap();
    meta
}

/// Signature plus header: the bytes before the first chunk.
pub fn head_len(file: &[u8]) -> usize {
    let (mut n, mut shift, mut pos) = (0usize, 0, 8);
    loop {
        let b = file[pos];
        pos += 1;
        n |= ((b & 0x7f) as usize) << shift;
        shift += 7;
        if b < 0x80 {
            return pos + n;
        }
    }
}

/// `with` is `plain` with chunks inserted after the header: the header and
/// every byte from the first chunk on (including `DATA`) are unchanged.
pub fn assert_only_inserted(plain: &[u8], with: &[u8]) {
    let h = head_len(plain);
    assert_eq!(&with[..h], &plain[..h]);
    assert!(with.ends_with(&plain[h..]));
    assert!(with.len() > plain.len());
}

pub fn tags(meta: &Metadata) -> Vec<[u8; 4]> {
    meta.chunks.iter().map(|c| c.tag).collect()
}
