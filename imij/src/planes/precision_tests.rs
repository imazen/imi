use super::*;
use crate::{EncodeConfig, ImageInfo, LossyColor, Metadata};
use alloc::vec;

fn lossless() -> crate::ImiEncoderConfig {
    crate::ImiEncoderConfig::default().with_effort(0.0)
}
fn primary() -> Vec<u8> {
    crate::encode(
        &[42],
        1,
        &ImageInfo::new(1, 1, Channels::Gray, SampleType::U8),
        &Metadata::default(),
        &EncodeConfig::default().with_effort(0.0),
    )
    .unwrap()
}
fn native(bits: u8) -> (Vec<u8>, Vec<u8>) {
    let pixels: Vec<u8> = [0u16, 1, 511, 1023]
        .into_iter()
        .flat_map(u16::to_ne_bytes)
        .collect();
    let info = ImageInfo::new(2, 2, Channels::Gray, SampleType::U16)
        .with_significant_bits(&[bits])
        .unwrap();
    let encoded = crate::encode(
        &pixels,
        4,
        &info,
        &Metadata::default(),
        &EncodeConfig::default().with_effort(0.0),
    )
    .unwrap();
    (encoded, pixels)
}

/// The directory `raw` in the layout before grammar v2: `PSET`, version 2,
/// the primary length as a u64, then the record payloads back to back.
fn pre_v2_layout(raw: &[u8]) -> Vec<u8> {
    let mut old = b"PSET".to_vec();
    old.extend_from_slice(&2u32.to_le_bytes());
    let mut attachments = false;
    crate::parse::records::head_records(raw, |id, rec| {
        match id {
            1 => {
                let mut r = crate::ByteReader::new(rec);
                old.extend_from_slice(&r.varint().unwrap().to_le_bytes());
            }
            7 => {
                attachments = true;
                old.extend_from_slice(rec);
            }
            _ => old.extend_from_slice(rec),
        }
        Ok(())
    })
    .unwrap();
    if !attachments {
        old.extend_from_slice(&0u32.to_le_bytes());
    }
    old
}

/// Construct an index independently of builder policy, to forge
/// descriptor/header disagreements and (with `edit`) other directories.
fn indexed(tile: &[u8], precision: u8, edit: fn(&[u8]) -> Vec<u8>) -> Vec<u8> {
    let mut file = primary();
    let primary_len = file.len() as u64;
    let mut level = LevelDescriptor::new(2, 2, 2, 2);
    level.record_start = 0;
    let mut plane = PlaneDescriptor::new("sample", SampleType::U16).with_precision(precision);
    plane.levels.push(level);
    let dir = Directory {
        images: None,
        pages: None,
        primary_len,
        records: vec![Record {
            offset: primary_len,
            len: tile.len() as u64,
        }],
        planes: vec![plane],
        attachments: Vec::new(),
    };
    wire::chunk(&mut file, &wire::DATA, tile).unwrap();
    let index = file.len() as u64;
    let raw = edit(
        &wire::encode(
            &dir,
            PlaneSetLimits::default(),
            crate::parse::header::HEADER_VERSION,
        )
        .unwrap(),
    );
    let packed = crate::metadata_pack::pack(&raw).unwrap();
    let index_len = wire::chunk_len(packed.len() as u64).unwrap();
    wire::chunk(&mut file, &wire::INDEX, &packed).unwrap();
    let size = file.len() as u64 + wire::FOOTER_LEN;
    file.extend_from_slice(&wire::footer(index, index_len, size));
    file
}

/// The directory as the encoder wrote it.
const SAME: fn(&[u8]) -> Vec<u8> = <[u8]>::to_vec;

fn open_indexed(file: &[u8]) -> Result<(), Error> {
    PlaneSetReader::open(SliceReader::new(file), PlaneSetLimits::default()).map(|_| ())
}

#[test]
fn directories_before_grammar_v2_are_rejected() {
    // The directory is a record list (docs/PLANES_AND_ROI.md). The old layout began with
    // `PSET` and a version; version 1 declared precision as advisory.
    let (tile, _) = native(16);
    assert!(open_indexed(&indexed(&tile, 16, pre_v2_layout)).is_err());
    assert_eq!(open_indexed(&indexed(&tile, 16, SAME)), Ok(()));
}

#[test]
fn directory_records_follow_the_record_rules() {
    let (tile, _) = native(16);
    // An unknown even record is skipped, an unknown odd one is Unsupported,
    // a missing one or one out of order is Corrupt.
    assert_eq!(
        open_indexed(&indexed(&tile, 16, |raw| [raw, &[98, 2, 0xaa, 0xbb]].concat())),
        Ok(())
    );
    assert_eq!(
        open_indexed(&indexed(&tile, 16, |raw| [raw, &[99, 0]].concat())),
        Err(Error::Unsupported("unknown plane directory record"))
    );
    assert_eq!(
        open_indexed(&indexed(&tile, 16, |raw| {
            let mut out = Vec::new();
            crate::parse::records::head_records(raw, |id, rec| {
                if id != crate::registry::DIRECTORY_TILES {
                    crate::parse::records::put_record(&mut out, id, rec);
                }
                Ok(())
            })
            .unwrap();
            out
        })),
        Err(Error::Corrupt("plane directory record missing"))
    );
    assert_eq!(
        open_indexed(&indexed(&tile, 16, |raw| [&[92, 0][..], raw, &[2, 0]].concat())),
        Err(Error::Corrupt("head records out of order"))
    );
    // Every record's fields fill it exactly.
    assert_eq!(
        open_indexed(&indexed(&tile, 16, |raw| {
            let mut out = Vec::new();
            crate::parse::records::head_records(raw, |id, rec| {
                let mut rec = rec.to_vec();
                if id == crate::registry::DIRECTORY_PRIMARY {
                    rec.push(0);
                }
                crate::parse::records::put_record(&mut out, id, &rec);
                Ok(())
            })
            .unwrap();
            out
        })),
        Err(Error::Corrupt("plane index trailing bytes"))
    );
}

#[test]
fn index_precision_must_match_native_before_output_changes() {
    for (native_bits, index_bits, accepted) in [
        (10, 10, true),
        (10, 12, false),
        (16, 10, false),
        (16, 16, true),
    ] {
        let (tile, pixels) = native(native_bits);
        let file = indexed(&tile, index_bits, SAME);
        let mut reader =
            PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
        let mut out = vec![0xcc; 8];
        let result = reader.decode_region(PlaneId(0), 0, Rect::new(0, 0, 2, 2), &mut out, 4);
        assert_eq!(
            result.is_ok(),
            accepted,
            "{native_bits}/{index_bits}: {result:?}"
        );
        if accepted {
            assert_eq!(out, pixels)
        } else {
            assert_eq!(out, [0xcc; 8])
        }
    }
}

#[test]
fn mixed_rgba_precision_uses_one_backing_and_preserves_lossless_samples() {
    let mut builder = PlaneSetBuilder::new(&primary()).unwrap();
    let bits = [10, 12, 16, 1];
    let ids: Vec<_> = bits
        .into_iter()
        .enumerate()
        .map(|(c, b)| {
            builder
                .add_plane(
                    PlaneDescriptor::new(alloc::format!("c{c}"), SampleType::U16).with_precision(b),
                )
                .unwrap()
        })
        .collect();
    let pixels: Vec<u8> = (0..17 * 9)
        .flat_map(|i| {
            [
                (i as u16 * 7) & 1023,
                (i as u16 * 29) & 4095,
                (i as u16).wrapping_mul(433),
                (i & 1) as u16,
            ]
            .into_iter()
            .flat_map(u16::to_ne_bytes)
        })
        .collect();
    builder
        .add_color_level(
            &ids,
            LevelDescriptor::new(17, 9, 8, 8),
            &pixels,
            17 * 8,
            Channels::Rgba,
            lossless(),
        )
        .unwrap();
    let file = builder.finish().unwrap();
    let mut reader =
        PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
    assert_eq!(
        reader
            .planes()
            .iter()
            .map(PlaneDescriptor::precision_bits)
            .collect::<Vec<_>>(),
        bits
    );
    let mut out = vec![0; pixels.len()];
    reader
        .decode_color_region(&ids, 0, Rect::new(0, 0, 17, 9), &mut out, 17 * 8)
        .unwrap();
    assert_eq!(out, pixels);
    let mut crop = [0xcc; 3 * 11];
    reader
        .decode_region(ids[0], 0, Rect::new(5, 4, 5, 3), &mut crop, 11)
        .unwrap();
    for y in 0..3 {
        for x in 0..5 {
            assert_eq!(
                &crop[y * 11 + x * 2..][..2],
                &pixels[((y + 4) * 17 + x + 5) * 8..][..2]
            );
        }
        assert_eq!(crop[y * 11 + 10], 0xcc);
    }
    // Forging an unrequested alpha component must also reject a scalar red
    // read: the whole shared native backing has one declared interpretation.
    let footer = file[file.len() - wire::FOOTER_LEN as usize..]
        .try_into()
        .unwrap();
    let (offset, size) = wire::locator(footer, file.len() as u64).unwrap();
    let packed = wire::payload(&file[offset as usize..][..size as usize], &wire::INDEX).unwrap();
    let limits = PlaneSetLimits::default();
    let raw = crate::metadata_pack::unpack(packed, limits.max_index_bytes).unwrap();
    let mut dir = wire::decode(&raw, limits, offset, crate::parse::header::HEADER_VERSION).unwrap();
    dir.planes[3].precision = 2;
    let packed = crate::metadata_pack::pack(
        &wire::encode(&dir, limits, crate::parse::header::HEADER_VERSION).unwrap(),
    )
    .unwrap();
    let mut forged = file[..offset as usize].to_vec();
    let index_len = wire::chunk_len(packed.len() as u64).unwrap();
    wire::chunk(&mut forged, &wire::INDEX, &packed).unwrap();
    let length = forged.len() as u64 + wire::FOOTER_LEN;
    forged.extend_from_slice(&wire::footer(offset, index_len, length));
    let mut reader = PlaneSetReader::open(SliceReader::new(&forged), limits).unwrap();
    let mut out = [0xcc; 8];
    assert!(
        reader
            .decode_region(ids[0], 0, Rect::new(0, 0, 2, 2), &mut out, 4)
            .is_err()
    );
    assert_eq!(out, [0xcc; 8]);
}

#[test]
fn lossy_low_bit_hdr_uses_declared_range_and_rejects_out_of_range_input() {
    for cicp in [Cicp::SRGB, Cicp::BT2100_PQ, Cicp::BT2100_HLG] {
        let mut builder = PlaneSetBuilder::new(&primary()).unwrap();
        let id = builder
            .add_plane(
                PlaneDescriptor::new("gray", SampleType::U16)
                    .with_precision(10)
                    .with_cicp(cicp),
            )
            .unwrap();
        let pixels: Vec<u8> = [1023u16; 17 * 9]
            .into_iter()
            .flat_map(u16::to_ne_bytes)
            .collect();
        let cfg = crate::ImiEncoderConfig::lossy(116.0)
            .with_effort(0.0)
            .tune_lossy(|t| t.color = LossyColor::Xyb);
        let bad: Vec<u8> = [1024u16; 17 * 9]
            .into_iter()
            .flat_map(u16::to_ne_bytes)
            .collect();
        assert!(
            builder
                .add_level(id, LevelDescriptor::new(17, 9, 8, 8), &bad, 34, cfg.clone())
                .is_err()
        );
        let encoded = builder.add_level(id, LevelDescriptor::new(17, 9, 8, 8), &pixels, 34, cfg);
        encoded.unwrap();
        let file = builder.finish().unwrap();
        let mut reader =
            PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
        let mut decoded = vec![0; pixels.len()];
        reader
            .decode_region(id, 0, Rect::new(0, 0, 17, 9), &mut decoded, 34)
            .unwrap();
        for sample in decoded.as_chunks::<2>().0 {
            let value = u16::from_ne_bytes(*sample);
            assert!((1022..=1023).contains(&value), "{cicp:?}: {value}");
        }
    }
}

#[test]
fn full_storage_precision_writes_the_bound_index() {
    let (tile, pixels) = native(16);
    let mut builder = PlaneSetBuilder::new(&primary()).unwrap();
    let id = builder
        .add_plane(PlaneDescriptor::new("sample", SampleType::U16))
        .unwrap();
    builder
        .add_level(id, LevelDescriptor::new(2, 2, 2, 2), &pixels, 4, lossless())
        .unwrap();
    let file = builder.finish().unwrap();
    assert_eq!(file, indexed(&tile, 16, SAME));
    assert!(PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).is_ok());
}

/// The directory's enumerated fields follow SPEC 0.3: an unknown value of
/// a field a later version may extend (sample type, role) is `Unsupported`.
#[test]
fn unknown_registry_values_are_unsupported() {
    let (tile, _) = native(16);
    // Record 5: plane count (u32), then the plane's name ("sample": u16
    // length and 6 bytes), sample type, precision and role bytes.
    fn set(raw: &[u8], at: usize, v: u8) -> Vec<u8> {
        let mut out = Vec::new();
        crate::parse::records::head_records(raw, |id, rec| {
            let mut rec = rec.to_vec();
            if id == crate::registry::DIRECTORY_PLANES {
                rec[at] = v;
            }
            crate::parse::records::put_record(&mut out, id, &rec);
            Ok(())
        })
        .unwrap();
        out
    }
    assert_eq!(
        open_indexed(&indexed(&tile, 16, |raw| set(raw, 14, 15))),
        Err(Error::Unsupported("unknown plane role"))
    );
    assert_eq!(
        open_indexed(&indexed(&tile, 16, |raw| set(raw, 12, 200))),
        Err(Error::Unsupported("unknown plane sample type"))
    );
}
