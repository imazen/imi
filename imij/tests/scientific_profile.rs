//! The Scientific features are never implied (SPEC 0.1): a lossless payload
//! that uses `near_lossless` (a version 2 near-lossless head) or
//! `float_shift` (the FloatShift map) in a file whose header does not
//! declare it is `Corrupt` on every decode route, including frames, pages and
//! plane tiles, which are files with their own headers. The outcome matrix
//! (tests/matrix.rs, `hd/*-undeclared`, `hd/*-hidden`,
//! `fr/near-animation-hidden`) checks every route; these tests check pages,
//! mixed pages and plane tiles.
// Literal V2/V3 framing controls use the retained compatibility writers.

use imij::{
    Channels, Error, FrameConfig, FrameDecoder, FrameDuration, FrameEncoder, FrameKind, ImageInfo,
    Metadata, Profile, SampleType, decode, supported_profiles,
};

mod common;
use common::feature::{FLOAT_SHIFT, MIXED_PAGES, NEAR_LOSSLESS};
use common::{
    SCIENTIFIC_0_1, SCIENTIFIC_0_2, anim_of, feature_mask, header_layout, header_record,
    listed_profiles, with_feature_mask,
};

/// The Scientific features.
const SCIENCE: u64 = NEAR_LOSSLESS | FLOAT_SHIFT;

/// `file` without the Scientific features in its own header's mask (the
/// field removed when nothing else is declared).
fn strip_record(file: &[u8]) -> Vec<u8> {
    let mask = feature_mask(file);
    assert_ne!(mask & SCIENCE, 0, "the file declares a Scientific feature");
    let rest = mask & !SCIENCE;
    with_feature_mask(file, (rest != 0).then_some(rest))
}

/// Clears the Scientific feature bits of every header in `file` from byte
/// `from` on (its own and those of nested files), keeping the one-byte mask
/// so no offset or chunk length moves. Returns the file and how many masks
/// it changed.
fn hide_records(file: &[u8], from: usize) -> (Vec<u8>, usize) {
    let mut out = file.to_vec();
    let mut hidden = 0;
    let mut at = from;
    while let Some(p) = out[at..].windows(8).position(|w| w == common::SIGNATURE) {
        let s = at + p;
        at = s + 8;
        let h = header_record(&out[s..]);
        let l = header_layout(&out[s + h.start..s + h.end]);
        let m = s + h.start + l.features.start;
        if l.features.len() == 1 && out[m] as u64 & SCIENCE != 0 {
            out[m] &= !(SCIENCE as u8);
            hidden += 1;
        }
    }
    (out, hidden)
}

/// `r` failed as `Corrupt` because a Scientific feature is undeclared.
fn corrupt<T>(route: &str, r: Result<T, Error>) {
    let r = r.map(|_| ());
    assert!(
        matches!(r, Err(Error::Corrupt(why)) if why.contains("feature")),
        "{route}: {r:?}"
    );
}

/// A Gray8 image coded near-lossless (version 2 payload; palette stripes
/// off, so every unit is near-coded).
fn near_file() -> (ImageInfo, Vec<u8>) {
    let info = ImageInfo::new(128, 32, Channels::Gray, SampleType::U8);
    let px: Vec<u8> = (0..4096usize)
        .map(|x| ((x * 17 + x * x * 3 + x / 128 * 31) % 256) as u8)
        .collect();
    let cfg = {
        let mut cfg = imij::ImiEncoderConfig::default().tune_lossless(|t| t.max_error = [1; 4]);
        cfg = cfg.tune_lossless(|t| {
            t.palette = false;
        });
        cfg
    };
    let file = cfg
        .encode_legacy_samples(imij::ImageRef::new(&px, 128, &info), &Metadata::default())
        .unwrap();
    (info, file)
}

/// Mixed-sign f16 patterns with seven dead low bits: FloatShift (map 8).
fn float_shift_file() -> (ImageInfo, Vec<u8>) {
    let info = ImageInfo::new(128, 32, Channels::Gray, SampleType::F16);
    let px: Vec<u8> = (0u16..4096)
        .flat_map(|x| ((x % 512) * 128).to_ne_bytes())
        .collect();
    let cfg = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.palette = false;
        });
        cfg
    };
    let file = cfg
        .encode_legacy_samples(imij::ImageRef::new(&px, 256, &info), &Metadata::default())
        .unwrap();
    (info, file)
}

/// Pages: page 1 exact (its values are multiples of 4, so the bound is 0 in
/// Shift-coded units and the payload is version 1), page 2 near-lossless.
fn near_pages() -> (ImageInfo, Vec<u8>) {
    let info = ImageInfo::new(64, 16, Channels::Gray, SampleType::U8);
    let flat: Vec<u8> = (0..64 * 16).map(|i| (i % 64 * 4) as u8).collect();
    let (_, near) = near_file();
    let (_, busy) = decode(&near).unwrap();
    let cfg = FrameConfig::new({
        let mut cfg = imij::ImiEncoderConfig::default().tune_lossless(|t| t.max_error = [1; 4]);
        cfg = cfg.tune_lossless(|t| {
            t.palette = false;
        });
        cfg
    });
    let mut enc = FrameEncoder::pages(info.clone(), Metadata::default(), cfg);
    enc.push_page(&flat, 64, &info).unwrap();
    let page2 = ImageInfo::new(128, 32, Channels::Gray, SampleType::U8);
    enc.push_page(&busy, 128, &page2).unwrap();
    (info, enc.finish_legacy().unwrap())
}

/// Decodes every frame or page of `file` (animation frames into the
/// canvas, pages on their own); the first error, if any.
fn decode_pages(file: &[u8]) -> Result<usize, Error> {
    let mut d = FrameDecoder::new(file)?;
    let canvas = d.info().clone();
    let mut buf = Vec::new();
    let mut n = 0;
    while d.next_frame()?.is_some() {
        n += 1;
        let info = match (d.kind(), d.frame_info()) {
            (FrameKind::Animation, _) => canvas.clone(),
            (_, Some(info)) => info,
            (_, None) => continue,
        };
        let rb = info.row_bytes();
        buf.resize(rb * info.height as usize, 0);
        d.decode_into(&mut buf, rb, 1)?;
    }
    Ok(n)
}

#[test]
fn frames_and_pages_need_their_own_and_the_outer_declaration() {
    let (_, file) = near_pages();
    assert_eq!(decode_pages(&file), Ok(2));
    // The outer header declares `near_lossless` for page 2; page 1 is
    // version 1.
    assert_eq!(feature_mask(&file), NEAR_LOSSLESS);
    let outer_end = common::header_end(&file);
    // Page 2's own feature cleared: its payload is undeclared.
    let (inner_hidden, n) = hide_records(&file, outer_end);
    assert_eq!(n, 1);
    corrupt("page 2 without its feature", decode_pages(&inner_hidden));
    // The outer feature gone, page 2 still declares it: the file does not
    // declare what its pages need.
    corrupt(
        "outer without the record",
        decode_pages(&strip_record(&file)),
    );
    // Page 1 alone still decodes through the frame-unaware routes.
    assert!(decode(&strip_record(&file)).is_ok());
}

#[test]
fn near_animation_frames_need_their_declaration() {
    let info = ImageInfo::new(128, 32, Channels::Gray, SampleType::U8);
    let (_, near) = near_file();
    let (_, px) = decode(&near).unwrap();
    let mut px2 = px.clone();
    px2.reverse();
    let cfg = FrameConfig::new({
        let mut cfg = imij::ImiEncoderConfig::default().tune_lossless(|t| t.max_error = [1; 4]);
        cfg = cfg.tune_lossless(|t| {
            t.palette = false;
        });
        cfg
    });
    let mut enc = FrameEncoder::animation(info.clone(), Metadata::default(), cfg);
    enc.push_frame(&px, 128, FrameDuration::from_millis(40))
        .unwrap();
    enc.push_frame(&px2, 128, FrameDuration::from_millis(40))
        .unwrap();
    let file = enc.finish_legacy().unwrap();
    assert_eq!(decode_pages(&file), Ok(2));
    let (hidden, n) = hide_records(&file, 0);
    assert!(n >= 2, "outer and frame masks: {n}");
    corrupt("animation without the feature", decode_pages(&hidden));
}

#[test]
fn plane_tiles_need_their_declaration() {
    use imij::planes::*;
    let primary = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(
            imij::ImageRef::new(
                &[1, 2, 3, 4],
                2,
                &ImageInfo::new(2, 2, Channels::Gray, SampleType::U8),
            ),
            &Metadata::default(),
        )
        .unwrap();
    let (_, near) = near_file();
    let (_, px) = decode(&near).unwrap();
    let mut b = PlaneSetBuilder::new(&primary).unwrap();
    let id = b
        .add_plane(PlaneDescriptor::new("counts", SampleType::U8))
        .unwrap();
    let coding = {
        let mut cfg = imij::ImiEncoderConfig::default().tune_lossless(|t| t.max_error = [1; 4]);
        cfg = cfg.tune_lossless(|t| {
            t.palette = false;
        });
        cfg
    };
    b.add_level(id, LevelDescriptor::new(128, 32, 64, 32), &px, 128, coding)
        .unwrap();
    let file = b.finish().unwrap();
    let region = |f: &[u8]| {
        let mut r = PlaneSetReader::open(SliceReader::new(f), PlaneSetLimits::default())?;
        let mut out = vec![0u8; 128 * 32];
        r.decode_region(id, 0, Rect::new(0, 0, 128, 32), &mut out, 128)
    };
    region(&file).unwrap();
    let (hidden, n) = hide_records(&file, primary.len());
    assert_eq!(n, 2, "one mask per tile");
    corrupt("plane tiles without the feature", region(&hidden));
    // The primary image needs no feature and is unaffected.
    assert_eq!(decode(&hidden).unwrap().1, [1, 2, 3, 4]);
}

#[test]
fn mixed_pages_carry_scientific_in_the_outer_mask() {
    // Page one plain RGB u16, page two near-lossless Gray u8: the outer
    // header keeps page one's own description and declares
    // `near_lossless` with `mixed_pages`, and page two's own header
    // declares it too.
    let rgb = ImageInfo::new(16, 8, Channels::Rgb, SampleType::U16);
    let px: Vec<u8> = (0u16..16 * 8 * 3)
        .flat_map(|i| (i * 97).to_ne_bytes())
        .collect();
    let (gray, near) = near_file();
    let (_, gray_px) = decode(&near).unwrap();
    let plain = imij::ImiEncoderConfig::default();
    let near_cfg = {
        let mut cfg = imij::ImiEncoderConfig::default().tune_lossless(|t| t.max_error = [1; 4]);
        cfg = cfg.tune_lossless(|t| {
            t.palette = false;
        });
        cfg
    };
    let mut e = FrameEncoder::mixed_pages(FrameConfig::new(plain.clone()));
    e.push_page_with(&px, rgb.row_bytes(), &rgb, &Metadata::default(), &plain)
        .unwrap();
    e.push_page_with(&gray_px, 128, &gray, &Metadata::default(), &near_cfg)
        .unwrap();
    let file = e.finish_legacy().unwrap();
    assert_eq!(feature_mask(&file), MIXED_PAGES | NEAR_LOSSLESS);
    let (_, b) = anim_of(&file);
    assert_eq!(decode_pages(&file), Ok(2));
    // Page two's own feature cleared: its payload is undeclared.
    let (hidden, n) = hide_records(&file, b);
    assert_eq!(n, 1);
    corrupt("mixed page without its feature", decode_pages(&hidden));
    // `near_lossless` dropped from the outer mask: page two declares a
    // feature the file does not.
    let unlisted = with_feature_mask(&file, Some(MIXED_PAGES));
    assert!(matches!(decode_pages(&unlisted), Err(Error::Corrupt(_))));
}

#[test]
fn scientific_is_a_public_supported_profile() {
    // Every build decodes Scientific 0.1 and 0.2 (value tables; 32-bit
    // sample paths still need Extended), and a near-lossless file reports
    // 0.1 in its header.
    assert!(supported_profiles().contains(&SCIENTIFIC_0_1));
    let (_, near) = near_file();
    let profiles = imij::header(&near).unwrap().profiles;
    assert!(profiles.contains(&SCIENTIFIC_0_1));
    assert_eq!(listed_profiles(&near), [SCIENTIFIC_0_1]);
    let (_, float) = float_shift_file();
    assert_eq!(listed_profiles(&float), [SCIENTIFIC_0_1]);
    assert!(
        supported_profiles()
            .iter()
            .filter(|p| p.profile == Profile::Scientific)
            .eq([&SCIENTIFIC_0_1, &SCIENTIFIC_0_2])
    );
}

#[test]
fn scientific_files_take_a_file_checksum() {
    // A near-lossless still and an animation whose later frame is
    // near-lossless, with a file checksum (SPEC 1.4): the header declares
    // `near_lossless` and carries the checksum record (2), every verifying
    // route accepts the file, and without the checksum it is the plain
    // encode.
    use common::checksum as cs;
    let (info, near) = near_file();
    let (_, px) = decode(&near).unwrap();
    let mut flipped = px.clone();
    flipped.reverse();
    let cfg = |checksum| {
        let mut cfg = imij::ImiEncoderConfig::default()
            .tune_lossless(|t| t.max_error = [1; 4])
            .with_checksum(checksum);
        cfg = cfg.tune_lossless(|t| {
            t.palette = false;
        });
        cfg
    };
    let still = |checksum| {
        cfg(checksum)
            .encode_legacy_samples(imij::ImageRef::new(&px, 128, &info), &Metadata::default())
            .unwrap()
    };
    let animation = |checksum| {
        let coding = cfg(checksum);
        let mut e =
            FrameEncoder::animation(info.clone(), Metadata::default(), FrameConfig::new(coding));
        for frame in [&px, &flipped] {
            e.push_frame(frame, 128, FrameDuration::from_millis(40))
                .unwrap();
        }
        e.finish_legacy().unwrap()
    };
    let cases = [
        (still(true), still(false), 1),
        (animation(true), animation(false), 2),
    ];
    for (file, plain, frames) in cases {
        assert_eq!(feature_mask(&file), NEAR_LOSSLESS);
        assert_eq!(common::extensions(&file)[0].0, 2);
        assert_eq!(listed_profiles(&file), [SCIENTIFIC_0_1]);
        assert_eq!(cs::strip_all(&file), plain);
        let (_, first) = decode(&file).unwrap();
        cs::accepted(&file, &first, "Scientific file with a checksum");
        assert_eq!(decode_pages(&file), Ok(frames));
    }
}
