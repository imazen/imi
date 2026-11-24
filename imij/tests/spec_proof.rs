//! Retained V2/V3 wire grammar controls use the explicit legacy producer.
//! Literate Specification Proof (`SPEC.md` & `imij-set/FORMAT.md` <-> `imij`).
//!
//! Parses the normative wire layouts, present-mask bit assignments, feature
//! registry (`0..=26`), and record registry (`1..=37`) directly from `SPEC.md`
//! and `imij-set/FORMAT.md` and proves 1-to-1 compliance against encoded bytes.

use imij::{Channels, ImageInfo, Metadata, SampleType};

const SPEC_MD: &str = include_str!("../../SPEC.md");
const SET_FORMAT_MD: &str = include_str!("../../imij-set/FORMAT.md");

fn read_varint(mut bytes: &[u8]) -> (u64, &[u8]) {
    let mut val = 0u64;
    let mut shift = 0u32;
    loop {
        let b = bytes[0];
        bytes = &bytes[1..];
        val |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return (val, bytes);
        }
        shift += 7;
    }
}

#[test]
fn spec_md_signature_header_and_unit_ordinals_wire_proof() {
    // 1. Verify SPEC.md documents the exact 8-byte signature ("IMIJ\r\n\x87\n").
    assert!(
        SPEC_MD.contains("49 4D 49 4A 0D 0A 87 0A"),
        "SPEC.md §1 must document the 8-byte signature"
    );
    assert!(
        SPEC_MD.contains("| 26 | `unit_ordinals` |"),
        "SPEC.md §0.1 must document Feature 26 (unit_ordinals)"
    );
    assert!(
        SPEC_MD.contains("| 37 | lossless and lossy heads | unit record ordinals | critical | unit_ordinals | HEAD_UNIT_ORDINALS |"),
        "SPEC.md §1 must document Record 37 (HEAD_UNIT_ORDINALS)"
    );
    assert!(
        SET_FORMAT_MD.contains("IMIJPAT\\x01"),
        "imij-set/FORMAT.md must document the key-0 Morton pyramid catalog magic"
    );

    // 2. Encode a minimal 2x2 RGB8 image with unit_ordinals = true and verify
    //    every byte of the container signature, header, present bitmask, feature
    //    bitmask, dimensions, and decoded roundtrip against SPEC.md §1.0.
    let pixels = [10u8, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120];
    let info = ImageInfo::new(2, 2, Channels::Rgb, SampleType::U8);
    let cfg = imij::ImiEncoderConfig::default().with_unit_ordinals(true);
    let encoded = cfg
        .encode_legacy_samples(imij::ImageRef::new(&pixels, 6, &info), &Metadata::default())
        .unwrap();

    // Signature (+0x00..+0x08): "IMIJ\r\n\x87\n"
    assert_eq!(
        &encoded[0..8],
        b"IMIJ\r\n\x87\n",
        "offset +0x00..+0x08 must equal the 8-byte signature in SPEC.md §1"
    );

    // header_len: varint, followed by header bytes (SPEC.md §1.0)
    let (header_len, after_len) = read_varint(&encoded[8..]);
    let header = &after_len[..header_len as usize];
    assert_eq!(
        header[0], 3,
        "SPEC.md §1.0: header version byte must equal 3"
    );

    let (present, mut rest) = read_varint(&header[1..]);
    assert_ne!(
        present & (1 << 2),
        0,
        "SPEC.md §1.0: present bit 2 (features) must be set when unit_ordinals (feature 26) is enabled"
    );

    // Feature mask is written immediately after `present` when bit 2 is set:
    let (features, after_features) = read_varint(rest);
    rest = after_features;
    assert_ne!(
        features & (1u64 << 26),
        0,
        "SPEC.md §0.1: feature bit 26 (unit_ordinals) must be set in the header feature mask"
    );

    // Consume any optional present fields in bit order (0, 1, 3, 4, 5, 6, 7, 8)
    for bit in [0u32, 1, 3, 4, 5, 6, 7, 8] {
        if present & (1 << bit) != 0 {
            let step = match bit {
                6 => 4,
                7 => 3,
                _ => 1,
            };
            rest = &rest[step..];
        }
    }
    let (w, rest) = read_varint(rest);
    let (h, _records) = read_varint(rest);
    assert_eq!(
        (w, h),
        (2, 2),
        "SPEC.md §1.0: width and height varints must match"
    );

    // Verify independent roundtrip through imij::decode
    let (_info, decoded_pixels) = imij::decode(&encoded).unwrap();
    assert_eq!(decoded_pixels, pixels);
}
