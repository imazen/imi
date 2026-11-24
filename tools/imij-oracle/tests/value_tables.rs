//! Value-table channel maps (SPEC 2.1 map mode 9): the reference encoder
//! writes, only the independent oracle reconstructs.
// The independent decoder implements V2/V3; fixtures select that grammar explicitly.
use imij::{
    Channels, Feature, FrameConfig, FrameDuration, FrameEncoder, ImageInfo, Metadata, SampleType,
};

/// The SPEC 2.1 order map of an 8-, 16- or 32-bit word (an involution).
fn order(sample: SampleType, v: u32) -> u32 {
    match sample {
        SampleType::F16 => v ^ ((v >> 15) * 0x7fff),
        SampleType::F32 | SampleType::F64 => v ^ ((v >> 31) * 0x7fff_ffff),
        SampleType::I8 => v ^ 0x80,
        SampleType::I16 => v ^ 0x8000,
        SampleType::I32 | SampleType::I64 => v ^ 0x8000_0000,
        _ => v,
    }
}

fn next(s: &mut u32) -> u32 {
    *s ^= *s << 13;
    *s ^= *s >> 17;
    *s ^= *s << 5;
    *s
}

/// A smooth index field through `count` scattered ordered values per coded
/// word (hi and lo words of 64-bit samples), moved by `t`.
fn sparse(info: &ImageInfo, seed: u32, count: usize, t: usize) -> Vec<u8> {
    let (w, h) = (info.width as usize, info.height as usize);
    let nch = info.channels.count();
    let bytes = info.sample.bytes();
    let depth = 8 * bytes.min(4) as u32;
    let mut s = seed | 1;
    let sets: Vec<Vec<u32>> = (0..2 * nch)
        .map(|_| {
            let mut v = std::collections::BTreeSet::new();
            while v.len() < count {
                let x = next(&mut s);
                v.insert(if depth == 32 {
                    x
                } else {
                    x & ((1 << depth) - 1)
                });
            }
            v.into_iter().collect()
        })
        .collect();
    let mut out = Vec::with_capacity(info.row_bytes() * h);
    for y in 0..h {
        for x in 0..w {
            for c in 0..nch {
                let mut word = |k: usize| {
                    let set: &Vec<u32> = &sets[k * nch + c];
                    let f = (x * 5 + y * 3 + c * 17 + t * 7) % (2 * set.len());
                    let i = if f < set.len() {
                        f
                    } else {
                        2 * set.len() - 1 - f
                    };
                    set[(i + (next(&mut s) % 3) as usize).min(set.len() - 1)]
                };
                match bytes {
                    1 => out.push(order(info.sample, word(0)) as u8),
                    2 => out.extend_from_slice(&(order(info.sample, word(0)) as u16).to_ne_bytes()),
                    4 => out.extend_from_slice(&order(info.sample, word(0)).to_ne_bytes()),
                    _ => {
                        let hi = order(info.sample, word(0)) as u64;
                        out.extend_from_slice(&(hi << 32 | word(1) as u64).to_ne_bytes());
                    }
                }
            }
        }
    }
    out
}

/// Native-endian samples as the oracle returns them (little-endian).
fn little_endian(source: &[u8], bytes: usize) -> Vec<u8> {
    source
        .chunks_exact(bytes)
        .flat_map(|word| {
            let mut word = word.to_vec();
            if cfg!(target_endian = "big") {
                word.reverse();
            }
            word
        })
        .collect()
}

#[test]
fn value_tables_match_the_source_in_the_independent_decoder() {
    // The encoder writes value tables for words wider than 8 bits.
    for sample in [
        SampleType::U16,
        SampleType::I16,
        SampleType::F16,
        SampleType::U32,
        SampleType::I32,
        SampleType::F32,
        SampleType::U64,
        SampleType::I64,
        SampleType::F64,
    ] {
        for channels in [Channels::Gray, Channels::Rgb, Channels::Rgba] {
            let info = ImageInfo::new(83, 37, channels, sample);
            let source = sparse(&info, 5, 300, 0);
            for cfg in [imij::ImiEncoderConfig::default(), {
                let mut cfg = imij::ImiEncoderConfig::default();
                cfg = cfg.with_column_groups(Some(32));
                cfg = cfg.tune_lossless(|t| {
                    t.stripe_rows = Some(8);
                });
                cfg = cfg.tune_lossless(|t| {
                    t.lanes = Some(3);
                });
                cfg
            }] {
                let file = cfg
                    .encode_legacy_samples(
                        imij::ImageRef::new(&source, info.row_bytes(), &info),
                        &Metadata::default(),
                    )
                    .unwrap();
                assert!(
                    imij::header(&file)
                        .unwrap()
                        .features
                        .contains(Feature::ValueTables),
                    "{sample:?} {channels:?} uses value tables"
                );
                let decoded = imij_oracle::decode(&file).unwrap();
                assert!(
                    decoded.frames == [little_endian(&source, sample.bytes())],
                    "{sample:?} {channels:?} {cfg:?}"
                );
            }
        }
    }
}

/// Inter frames predict from the previous canvas reduced through their own
/// tables: most reference values lie between or outside the entries.
#[test]
fn inter_frames_with_value_tables_match_in_the_independent_decoder() {
    for (channels, sample) in [
        (Channels::Gray, SampleType::U16),
        (Channels::Rgb, SampleType::F16),
        (Channels::Rgb, SampleType::F32),
    ] {
        let info = ImageInfo::new(70, 44, channels, sample);
        let frames: Vec<Vec<u8>> = (0..4)
            .map(|t| sparse(&info, 9 + t as u32, 150, t))
            .collect();
        let mut cfg = FrameConfig::new(imij::ImiEncoderConfig::default());
        cfg.inter = true;
        let mut e = FrameEncoder::animation(info.clone(), Metadata::default(), cfg);
        for f in &frames {
            e.push_frame(f, info.row_bytes(), FrameDuration::from_millis(40))
                .unwrap();
        }
        let file = e.finish_legacy().unwrap();
        assert!(
            imij::header(&file)
                .unwrap()
                .features
                .contains(Feature::ValueTables)
        );
        let decoded = imij_oracle::decode(&file).unwrap();
        let expected: Vec<Vec<u8>> = frames
            .iter()
            .map(|f| little_endian(f, sample.bytes()))
            .collect();
        assert!(decoded.frames == expected, "{channels:?} {sample:?}");
    }
}

#[test]
fn unit_ordinals_match_source_in_the_independent_decoder() {
    let info = ImageInfo::new(96, 48, Channels::Rgb, SampleType::U8);
    let source = sparse(&info, 33, 64, 0);
    let mut cfg = imij::ImiEncoderConfig::default().with_unit_ordinals(true);
    cfg = cfg.with_column_groups(Some(32));
    cfg = cfg.tune_lossless(|t| {
        t.stripe_rows = Some(16);
    });
    let file = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&source, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    assert!(
        imij::header(&file)
            .unwrap()
            .features
            .contains(Feature::UnitOrdinals)
    );
    let decoded = imij_oracle::decode(&file).unwrap();
    assert_eq!(decoded.frames, [source]);
}
