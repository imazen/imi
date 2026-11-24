//! Independent lossless reconstruction for unified pages and animations.
// The independent decoder implements V2/V3; fixtures select that grammar explicitly.
use imij::{
    Channels, FrameConfig, ImageInfo, ImageReference, SampleType, SequenceEncoder, SequenceItem,
    SequenceKind,
};

#[test]
fn independent_oracle_reconstructs_nonprevious_item_references() {
    for kind in [SequenceKind::Pages, SequenceKind::Animation] {
        for sample in [
            SampleType::U8,
            SampleType::U16,
            SampleType::F32,
            SampleType::U32,
            SampleType::I32,
        ] {
            let info = ImageInfo::new(131, 19, Channels::Rgb, sample);
            let mut state = 37u32;
            let a: Vec<_> = (0..info.checked_buffer_len().unwrap())
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    state as u8
                })
                .collect();
            let b: Vec<_> = a.iter().map(|x| !x).collect();
            let mut c = a.clone();
            c[61] ^= 17;
            let expected = [a, b, c];
            let config = FrameConfig::new({
                let mut cfg = imij::ImiEncoderConfig::default();
                cfg = cfg.with_threads(1);
                cfg = cfg.with_effort(4.0);
                cfg = cfg.with_column_groups((sample.bytes() == 4).then_some(32));
                cfg
            });
            let mut encoder = SequenceEncoder::new(kind, config);
            for (i, raster) in expected.iter().enumerate() {
                let mut item = SequenceItem::new(raster, info.row_bytes(), &info);
                if i == 2 {
                    item = item.with_reference(ImageReference {
                        index: 0,
                        pixels: &expected[0],
                        stride: info.row_bytes(),
                    });
                }
                encoder.push(item).unwrap();
            }
            let file = encoder.finish_legacy().unwrap();
            let mut native = imij::FrameDecoder::new(&file).unwrap();
            native.next_frame().unwrap();
            native.next_frame().unwrap();
            native.next_frame().unwrap();
            assert_eq!(native.reference_index(), Some(0));
            let decoded = imij_oracle::decode(&file).unwrap();
            let expected: Vec<Vec<u8>> = expected
                .into_iter()
                .map(|bytes| match sample.bytes() {
                    1 => bytes,
                    2 => bytes
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .flat_map(|v| u16::from_ne_bytes(*v).to_le_bytes())
                        .collect(),
                    4 => bytes
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .flat_map(|v| u32::from_ne_bytes(*v).to_le_bytes())
                        .collect(),
                    _ => unreachable!(),
                })
                .collect();
            assert_eq!(decoded.frames, expected);
        }
    }
}
