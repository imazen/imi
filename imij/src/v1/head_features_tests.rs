//! Feature scanning is independent of bank availability and opaque model data.
use super::*;
use crate::Channels;
use crate::parse::header::Feature;

#[test]
fn full_width_bank_ids_do_not_shift_lossless_map_features() {
    // These are metadata-scanner vectors, not complete encoded images. The
    // scanner owns prefix/map framing and skips opaque model bytes by length.
    for bank in (0..10)
        .map(|group| 1u64 << (group * 7))
        .chain([127, 16_383, u64::MAX])
    {
        for (sample, map, expected) in [
            (SampleType::U16, MapMode::Offset, Feature::OffsetMap),
            (SampleType::F32, MapMode::FloatShift, Feature::FloatShift),
            (SampleType::U16, MapMode::Table, Feature::ValueTables),
        ] {
            let info = ImageInfo::new(17, 3, Channels::Gray, sample);
            let mut head = Vec::new();
            put_varint(&mut head, H_LAB_BANK);
            put_varint(&mut head, bank);
            head.push(16); // lanes
            put_varint(&mut head, 3); // stripe rows
            put_varint(&mut head, 0); // full-width groups
            head.extend_from_slice(&[map as u8, 1, 0, 0]);
            if sample.bytes() >= 4 {
                head.extend_from_slice(&[0, 0]);
            }
            if map == MapMode::Table {
                put_varint(&mut head, 7); // one table entry
            }
            put_varint(&mut head, 3); // opaque model bytes
            head.extend_from_slice(&[0x81, 0x7f, 0xff]);
            assert!(
                head_features(&info, &head).allows(expected),
                "{bank}/{map:?}"
            );
        }
    }
}
