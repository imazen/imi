//! Template palette stripes (SPEC 2.14): the reference encoder writes, only
//! the independent oracle reconstructs.
// The independent decoder implements V2/V3; fixtures select that grammar explicitly.
use imij::{Channels, Feature, ImageInfo, ImiEncoderConfig, Metadata, SampleType};

fn rng(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

/// Map-like indices: blocky regions with ragged edges and specks, `n`
/// colors; rows `0..blank` all index 0 and rows `two..` (when set) use only
/// indices 1 and 2.
fn indices(w: usize, h: usize, n: usize, seed: u64, two: Option<usize>) -> Vec<usize> {
    let mut s = seed | 1;
    let mut v = vec![0usize; w * h];
    for y in 0..h {
        for x in 0..w {
            let r = rng(&mut s);
            let mut c = ((x + (r % 3) as usize) / 9 + 3 * ((y + (r >> 9) as usize % 2) / 7)) % n;
            if r.is_multiple_of(41) {
                c = (r >> 20) as usize % n;
            }
            if two.is_some_and(|t| y >= t) {
                c = 1 + (x / 5 + y / 3) % 2;
            }
            v[y * w + x] = c;
        }
        if y > 0 && rng(&mut s).is_multiple_of(4) {
            v.copy_within((y - 1) * w..y * w, y * w);
        }
    }
    v
}

fn check(info: &ImageInfo, source: &[u8], cfg: &ImiEncoderConfig) {
    let file = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(source, info.row_bytes(), info),
            &Metadata::default(),
        )
        .unwrap();
    assert!(
        imij::header(&file)
            .unwrap()
            .features
            .contains(Feature::TemplatePalettes),
        "{info:?} {cfg:?} uses template palette stripes"
    );
    let decoded = imij_oracle::decode(&file).unwrap();
    let expected: Vec<u8> = match info.sample.bytes() {
        1 => source.to_vec(),
        _ => source
            .chunks(2)
            .flat_map(|c| u16::from_ne_bytes([c[0], c[1]]).to_le_bytes())
            .collect(),
    };
    assert!(decoded.frames == [expected], "{info:?} {cfg:?}");
}

#[test]
fn template_palette_stripes_match_the_source_in_the_independent_decoder() {
    let (w, h) = (230usize, 150usize);
    let configs = [
        imij::ImiEncoderConfig::default(),
        {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.tune_lossless(|t| {
                t.lanes = Some(5);
            });
            cfg = cfg.tune_lossless(|t| {
                t.stripe_rows = Some(13);
            });
            cfg
        },
        {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.with_column_groups(Some(64));
            cfg = cfg.tune_lossless(|t| {
                t.stripe_rows = Some(40);
            });
            cfg
        },
    ];
    for cfg in &configs {
        // Gray, shared entries; the lower part two colors (own entries).
        let info = ImageInfo::new(w as u32, h as u32, Channels::Gray, SampleType::U8);
        let source: Vec<u8> = indices(w, h, 9, 1, Some(100))
            .iter()
            .map(|&i| (i * 23 + 7) as u8)
            .collect();
        check(&info, &source, cfg);
        // RGB, 40 colors.
        let info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8);
        let source: Vec<u8> = indices(w, h, 40, 2, None)
            .iter()
            .flat_map(|&i| [(i * 6) as u8, (255 - i * 5) as u8, (i * i % 251) as u8])
            .collect();
        check(&info, &source, cfg);
        // 16-bit gray with alpha, 6 colors.
        let info = ImageInfo::new(w as u32, h as u32, Channels::GrayAlpha, SampleType::U16);
        let source: Vec<u8> = indices(w, h, 6, 3, Some(120))
            .iter()
            .flat_map(|&i| {
                let g = (i as u16 * 9_001).to_ne_bytes();
                let a = if i == 0 { 0u16 } else { 65_535 }.to_ne_bytes();
                [g[0], g[1], a[0], a[1]]
            })
            .collect();
        check(&info, &source, cfg);
    }
    // More than 256 colors in the image, at most 256 per unit: every unit
    // has its own entries.
    let info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8);
    let source: Vec<u8> = indices(w, h, 100, 4, None)
        .iter()
        .enumerate()
        .flat_map(|(p, &i)| [i as u8, (p / w / 64) as u8, (i * 3) as u8])
        .collect();
    check(&info, &source, &imij::ImiEncoderConfig::default());
}
