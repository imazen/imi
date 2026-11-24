//! Retained V2/V3 wire grammar controls use the explicit legacy producer.
//! The lossy head's presence bits 6 and 7 (SPEC 2.8): bit 6 is the
//! quantizer floor (one f32), bit 7 the three color scales (three f32).
//! Conformance-style files: a reference lossy file rewritten with only bit
//! 6, only bit 7, or both, holding values unlike the defaults, and again
//! holding the defaults written explicitly. An independent walk of each
//! payload (`common::payload`, written from SPEC 2.8) must end exactly on
//! the record framing, the explicit defaults must decode to the reference
//! file's pixels, and the other values must change them. A reader that
//! swapped the two bits would read twelve bytes where four were written (or
//! four where twelve were) and fail both checks.

mod common;

use common::{chunks, data_payload, payload, payload_head, put_varint, read_varint};
use imij::{Channels, ImageInfo, Metadata, SampleType};

const FLOOR: u64 = 1 << 6;
const SCALES: u64 = 1 << 7;

/// One unit (no unit table hint, no DATA locator), YCoCg: its scales
/// default to 1, 1, 1 and its floor to 1.
fn reference() -> (Vec<u8>, ImageInfo) {
    let info = ImageInfo::new(48, 40, Channels::Rgb, SampleType::U8);
    let px: Vec<u8> = (0..48 * 40 * 3)
        .map(|i| ((i * 37 + (i / 144) * 11) % 251) as u8)
        .collect();
    let cfg = imij::ImiEncoderConfig::lossy(80.0).tune_lossy(|t| t.color = imij::LossyColor::YCoCg);
    let file = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .expect("encode");
    (file, info)
}

/// `file` with the lossy head's floor and scales fields set: `None` clears
/// the field's presence bit, `Some` writes the field (SPEC 2.8: the
/// present fields follow `quant_scale` in bit order: seed 1 byte, entropy
/// 1, matrix 1, DC 3, floor 4, scales 12, multipliers 12).
fn with_fields(file: &[u8], floor: Option<f32>, scales: Option<[f32; 3]>) -> Vec<u8> {
    let info = imij::header(file).unwrap().info;
    let range = data_payload(file);
    let d = &file[range.clone()];
    let p = payload_head(d, &info, true);
    let mut at = p.present_at;
    let present = read_varint(d, &mut at);
    assert_eq!(
        present & (FLOOR | SCALES),
        0,
        "the reference writes neither"
    );
    // quant_scale, then the fields of bits 0-3 (bits 4 and 5 hold none).
    let mut fields_end = at + 4;
    for (bit, n) in [(0, 1), (1, 1), (2, 1), (3, 3)] {
        if present >> bit & 1 != 0 {
            fields_end += n;
        }
    }
    let mut new = present;
    let mut tail = Vec::new();
    if let Some(f) = floor {
        new |= FLOOR;
        tail.extend_from_slice(&f.to_le_bytes());
    }
    if let Some(s) = scales {
        new |= SCALES;
        for v in s {
            tail.extend_from_slice(&v.to_le_bytes());
        }
    }
    let mut body = d[..p.present_at].to_vec();
    put_varint(&mut body, new);
    body.extend_from_slice(&d[at..fields_end]);
    body.extend_from_slice(&tail);
    body.extend_from_slice(&d[fields_end..]);
    let (_, data_at, _) = chunks(file).into_iter().find(|c| &c.0 == b"DATA").unwrap();
    let mut out = file[..data_at + 4].to_vec();
    put_varint(&mut out, body.len() as u64 + 1);
    out.extend_from_slice(&body);
    out.extend_from_slice(&file[range.end..]);
    out
}

/// Walks the payload independently: its head per SPEC 2.8 and its unit
/// records to the payload's last byte. Returns the floor and scale bytes the
/// walk found after the lower fields.
fn walked_fields(file: &[u8]) -> (u64, Vec<u8>) {
    let info = imij::header(file).unwrap().info;
    let d = &file[data_payload(file)];
    let p = payload(d, &info, true);
    assert!(!p.units.is_empty(), "unit records follow the head");
    let mut at = p.present_at;
    let present = read_varint(d, &mut at);
    at += 4;
    for (bit, n) in [(0, 1), (1, 1), (2, 1), (3, 3)] {
        if present >> bit & 1 != 0 {
            at += n;
        }
    }
    let n = 4 * (usize::from(present & FLOOR != 0) + 3 * usize::from(present & SCALES != 0));
    (present, d[at..at + n].to_vec())
}

fn pixels(file: &[u8]) -> Vec<u8> {
    imij::decode(file).expect("decodes").1
}

#[test]
fn floor_is_bit_6_and_scales_bit_7() {
    let (file, _) = reference();
    let base = pixels(&file);

    // Values unlike the defaults.
    let floor = with_fields(&file, Some(6.0), None);
    let scales = with_fields(&file, None, Some([2.0, 0.5, 0.25]));
    let both = with_fields(&file, Some(6.0), Some([2.0, 0.5, 0.25]));
    let (present, bytes) = walked_fields(&floor);
    assert_eq!(present & (FLOOR | SCALES), FLOOR);
    assert_eq!(bytes, 6f32.to_le_bytes());
    let (present, bytes) = walked_fields(&scales);
    assert_eq!(present & (FLOOR | SCALES), SCALES);
    assert_eq!(bytes.len(), 12);
    assert_eq!(bytes[..4], 2f32.to_le_bytes());
    let (present, bytes) = walked_fields(&both);
    assert_eq!(present & (FLOOR | SCALES), FLOOR | SCALES);
    assert_eq!(bytes[..4], 6f32.to_le_bytes(), "the floor comes first");
    assert_eq!(bytes[4..8], 2f32.to_le_bytes());
    let (pf, ps, pb) = (pixels(&floor), pixels(&scales), pixels(&both));
    assert_ne!(pf, base, "a floor of 6 coarsens the steps");
    assert_ne!(ps, base, "scales of 2, 0.5, 0.25 rescale the planes");
    assert_ne!(pb, pf);
    assert_ne!(pb, ps);

    // The defaults written explicitly: the same pixels.
    let floor = with_fields(&file, Some(1.0), None);
    let scales = with_fields(&file, None, Some([1.0; 3]));
    let both = with_fields(&file, Some(1.0), Some([1.0; 3]));
    for f in [&floor, &scales, &both] {
        walked_fields(f);
        assert_eq!(pixels(f), base);
    }
}
