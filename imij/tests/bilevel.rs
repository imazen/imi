//! Bilevel stripes (SPEC 2.10, feature `bilevel`): the encoder chooses them
//! for 1-bit images where they are smaller, files declare the feature and
//! the Bilevel profile, and every decode route returns the source pixels.

mod common;

use common::feature::BILEVEL;
use common::{Rng, data_payload, decode_all, feature_mask, payload, with_feature_mask};
use imij::planes::Rect;
use imij::region::decode_region_threads;
use imij::{
    Channels, Error, Feature, FrameConfig, FrameDecoder, FrameEncoder, ImageInfo, ImiEncoderConfig,
    Metadata, Profile, ProfileVersion, SampleType, decode, header,
};

/// A page-like 1-bit plane (true = ink): lines of random glyphs made of
/// strokes (each glyph a 5x7 cell of horizontal and vertical bars), a frame
/// and sparse specks.
fn page(w: usize, h: usize, seed: u64) -> Vec<bool> {
    let mut r = Rng(seed | 1);
    let mut ink = vec![false; w * h];
    for y0 in (3..h.saturating_sub(9)).step_by(12) {
        for x0 in (4..w.saturating_sub(7)).step_by(7) {
            if r.next().is_multiple_of(7) {
                continue;
            }
            for _ in 0..3 {
                let (vertical, at, from, len) = (
                    r.next().is_multiple_of(2),
                    (r.next() % 5) as usize,
                    (r.next() % 3) as usize,
                    3 + (r.next() % 5) as usize,
                );
                for k in from..(from + len).min(if vertical { 8 } else { 5 }) {
                    let (x, y) = if vertical {
                        (x0 + at, y0 + k)
                    } else {
                        (x0 + k, y0 + at.min(7))
                    };
                    ink[y * w + x] = true;
                }
            }
        }
    }
    for (i, p) in ink.iter_mut().enumerate() {
        let (x, y) = (i % w, i / w);
        *p |= x == 1 || y == 1 || x + 2 == w || r.next().is_multiple_of(900);
    }
    ink
}

fn enc(px: &[u8], info: &ImageInfo, cfg: &ImiEncoderConfig) -> Vec<u8> {
    // These tests inspect/mutate V3 record bytes. Keep that wire oracle and
    // independently require its positive files to retain V4 pixel parity.
    let file = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(px, info.row_bytes(), info),
            &Metadata::default(),
        )
        .unwrap();
    common::assert_current_migration_pixels(&file);
    file
}

/// The default settings without palettes: template palette stripes (SPEC
/// 2.14, which need palettes) do not compete, so 1-bit images use bilevel
/// stripes. With palettes allowed their smaller tables (record 35's chosen
/// precision) usually win, except on large pages.
fn no_palettes() -> ImiEncoderConfig {
    let mut cfg = imij::ImiEncoderConfig::default();
    cfg = cfg.tune_lossless(|t| {
        t.palette = false;
    });
    cfg
}

fn gray8(bits: &[bool], ink: u8, paper: u8) -> Vec<u8> {
    bits.iter().map(|&b| if b { ink } else { paper }).collect()
}

#[test]
fn adaptive_working_cap_preserves_selected_bilevel() {
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
    use zencodec::{ResourceLimits, ThreadingPolicy};
    use zenpixels::{PixelDescriptor, PixelSlice};
    let pixels = gray8(&page(300, 200, 60000), 0, 255);
    let view = PixelSlice::new(&pixels, 300, 200, 300, PixelDescriptor::GRAY8_SRGB).unwrap();
    let config = no_palettes();
    let mut outputs = Vec::new();
    for cap in [None, Some(256 << 20)] {
        let mut limits = ResourceLimits::none().with_threading(ThreadingPolicy::Sequential);
        limits.max_memory_bytes = cap;
        let out = config
            .clone()
            .job()
            .with_limits(limits)
            .encoder()
            .unwrap()
            .encode(view.clone())
            .unwrap();
        assert!(
            header(out.data())
                .unwrap()
                .features
                .contains(Feature::Bilevel)
        );
        assert_eq!(decode(out.data()).unwrap().1, pixels);
        outputs.push(out);
    }
    assert_eq!(outputs[0].data(), outputs[1].data());
}

const BILEVEL_0_1: ProfileVersion = ProfileVersion {
    profile: Profile::Bilevel,
    major: 0,
    minor: 1,
};

/// Bilevel stripes chosen, declared and decoded through every whole-image
/// route.
fn check(px: &[u8], info: &ImageInfo, cfg: &ImiEncoderConfig) -> Vec<u8> {
    let file = enc(px, info, cfg);
    let h = header(&file).unwrap();
    assert!(h.features.contains(Feature::Bilevel), "{info:?}");
    assert!(h.profiles.contains(&BILEVEL_0_1));
    assert!(decode_all(&file, info) == px, "{info:?}");
    file
}

#[test]
fn pages_decode_on_every_route() {
    for &(w, h) in &[(300, 200), (2321, 70), (17, 2400)] {
        let bits = page(w, h, (w * h) as u64);
        let info = ImageInfo::new(w as u32, h as u32, Channels::Gray, SampleType::U8);
        // White paper (the encoder inverts) and black paper.
        let file = check(&gray8(&bits, 0, 255), &info, &no_palettes());
        check(&gray8(&bits, 255, 0), &info, &no_palettes());
        // The table is head record 25 and every unit is a bilevel stripe.
        let d = data_payload(&file);
        let p = payload(&file[d.clone()], &info, false);
        assert!(p.head_record(25).is_some());
        assert!(p.records().iter().all(|r| file[d.start + r.start] == 0x81));
        assert_ne!(feature_mask(&file) & BILEVEL, 0);
    }
}

#[test]
fn sample_types_channel_maps_and_constant_alpha() {
    let (w, h) = (260usize, 300usize);
    let bits = page(w, h, 7);
    // 0/1 with one declared significant bit, 0/255, 16-bit 0/65535 and
    // 0/1, gray with an opaque alpha channel.
    let i = ImageInfo::new(w as u32, h as u32, Channels::Gray, SampleType::U8)
        .with_significant_bits(&[1])
        .unwrap();
    check(&gray8(&bits, 1, 0), &i, &no_palettes());
    let i = ImageInfo::new(w as u32, h as u32, Channels::Gray, SampleType::U16);
    for (ink, paper) in [(0u16, 65535u16), (1, 0)] {
        let px: Vec<u8> = bits
            .iter()
            .flat_map(|&b| if b { ink } else { paper }.to_ne_bytes())
            .collect();
        check(&px, &i, &no_palettes());
    }
    let i = ImageInfo::new(w as u32, h as u32, Channels::GrayAlpha, SampleType::U8);
    let px: Vec<u8> = bits
        .iter()
        .flat_map(|&b| [if b { 0 } else { 255 }, 255])
        .collect();
    check(&px, &i, &no_palettes());
    #[cfg(feature = "extended")]
    {
        let i = ImageInfo::new(w as u32, h as u32, Channels::Gray, SampleType::U32);
        let px: Vec<u8> = bits
            .iter()
            .flat_map(|&b| (b as u32 * u32::MAX).to_ne_bytes())
            .collect();
        check(&px, &i, &no_palettes());
    }
}

#[test]
fn geometry_lanes_stripes_and_column_groups() {
    for &(w, h) in &[(64, 700), (97, 420), (500, 130)] {
        let bits = page(w, h, (w + 3 * h) as u64);
        let px = gray8(&bits, 0, 255);
        let info = ImageInfo::new(w as u32, h as u32, Channels::Gray, SampleType::U8);
        for (lanes, rows, groups) in [
            (Some(1), Some(5), None),
            (Some(3), Some(7), Some(40)),
            (Some(16), Some(1), None),
            (None, Some(2), Some(16)),
            (None, None, Some(33)),
        ] {
            let cfg = {
                let mut cfg = no_palettes();
                cfg = cfg.tune_lossless(|t| {
                    t.lanes = lanes;
                });
                cfg = cfg.tune_lossless(|t| {
                    t.stripe_rows = rows;
                });
                cfg = cfg.with_column_groups(groups);
                cfg
            };
            let file = check(&px, &info, &cfg);
            for (x, y, rw, rh) in [(0, 0, w, h), (5, 3, w / 2, h / 3), (w - 7, h - 2, 7, 2)] {
                let r = Rect::new(x as u32, y as u32, rw as u32, rh as u32);
                let mut got = vec![0u8; rw * rh];
                decode_region_threads(&file, r, &mut got, rw, 3).unwrap();
                for yy in 0..rh {
                    assert_eq!(
                        got[yy * rw..(yy + 1) * rw],
                        px[(y + yy) * w + x..(y + yy) * w + x + rw],
                        "{w}x{h} {cfg:?} region {r:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn pages_and_animation_frames_declare_the_feature() {
    let info = ImageInfo::new(240, 200, Channels::Gray, SampleType::U8);
    let cfg = FrameConfig::new(no_palettes());
    let mut pages = FrameEncoder::pages(info.clone(), Metadata::default(), cfg.clone());
    let mut srcs = Vec::new();
    for s in 0..3u64 {
        let px = gray8(&page(240, 200, s + 11), 0, 255);
        pages.push_page(&px, 240, &info).unwrap();
        srcs.push(px);
    }
    let file = pages.finish().unwrap();
    assert!(header(&file).unwrap().features.contains(Feature::Bilevel));
    let mut d = FrameDecoder::new(&file).unwrap();
    let mut out = vec![0u8; 240 * 200];
    for want in &srcs {
        assert!(d.next_frame().unwrap().is_some());
        d.decode_into(&mut out, 240, 1).unwrap();
        assert!(&out == want);
    }
    assert!(d.next_frame().unwrap().is_none());
}

#[test]
fn files_without_the_feature_are_corrupt() {
    let info = ImageInfo::new(400, 300, Channels::Gray, SampleType::U8);
    let file = check(&gray8(&page(400, 300, 5), 0, 255), &info, &no_palettes());
    for f in [
        with_feature_mask(&file, None),
        with_feature_mask(&file, Some(0)),
    ] {
        assert_eq!(
            decode(&f).map(|_| ()),
            Err(Error::Corrupt("bilevel table without the feature bilevel"))
        );
    }
}

#[test]
fn small_or_unsuitable_images_keep_their_coding() {
    // Too small for the table to pay: no feature declared.
    let info = ImageInfo::new(6, 4, Channels::Gray, SampleType::U8);
    let px = gray8(&page(6, 4, 1), 0, 255);
    let file = enc(&px, &info, &imij::ImiEncoderConfig::default());
    assert_eq!(feature_mask(&file) & BILEVEL, 0);
    assert_eq!(decode(&file).unwrap().1, px);
    // Two significant bits: not a 1-bit slot.
    let info = ImageInfo::new(64, 40, Channels::Gray, SampleType::U8);
    let px: Vec<u8> = page(64, 40, 2)
        .iter()
        .enumerate()
        .map(|(i, &b)| {
            if b {
                0
            } else if i.is_multiple_of(9) {
                85
            } else {
                255
            }
        })
        .collect();
    let file = enc(&px, &info, &imij::ImiEncoderConfig::default());
    assert_eq!(feature_mask(&file) & BILEVEL, 0);
    assert_eq!(decode(&file).unwrap().1, px);
}

/// Two-color images: template palette stripes of two entries (SPEC 2.14)
/// where palettes are allowed, and bilevel palette stripes where template
/// stripes are not tried (a near-lossless request for RGB; every unit is
/// then an exact palette stripe and the file declares no bounds).
#[test]
fn two_color_images_use_two_entry_palette_stripes() {
    let (w, h) = (300usize, 220usize);
    let bits = page(w, h, 21);
    let rgb16 = |v: [u16; 3]| v.iter().flat_map(|c| c.to_ne_bytes()).collect::<Vec<u8>>();
    for (ch, sample, ink, paper) in [
        (
            Channels::Rgb,
            SampleType::U8,
            vec![20u8, 40, 200],
            vec![250u8, 240, 230],
        ),
        (
            Channels::Rgba,
            SampleType::U8,
            vec![0, 0, 0, 255],
            vec![255, 255, 255, 0],
        ),
        (
            Channels::GrayAlpha,
            SampleType::U8,
            vec![0, 255],
            vec![255, 0],
        ),
        (
            Channels::Rgb,
            SampleType::U16,
            rgb16([7, 900, 65535]),
            rgb16([65535, 1, 0]),
        ),
    ] {
        let info = ImageInfo::new(w as u32, h as u32, ch, sample);
        let px: Vec<u8> = bits
            .iter()
            .flat_map(|&b| if b { ink.clone() } else { paper.clone() })
            .collect();
        for groups in [None, Some(64)] {
            let mut cfg = {
                let mut cfg = imij::ImiEncoderConfig::default();
                cfg = cfg.with_column_groups(groups);
                cfg
            };
            let file = enc(&px, &info, &cfg);
            let h = header(&file).unwrap();
            assert!(h.features.contains(Feature::TemplatePalettes), "{info:?}");
            assert!(!h.features.contains(Feature::Bilevel), "{info:?}");
            assert!(decode_all(&file, &info) == px, "{info:?}");
            // Bounds on the color channels where they code more than one
            // bit (RGB; the others' 0/255 values code as one bit, which no
            // bound applies to).
            if ch != Channels::Rgb {
                continue;
            }
            cfg = cfg.tune_lossless(|t| t.max_error[..3].fill(2));
            let file = check(&px, &info, &cfg);
            assert!(
                !header(&file)
                    .unwrap()
                    .features
                    .contains(Feature::NearLossless)
            );
            let d = data_payload(&file);
            let p = payload(&file[d.clone()], &info, false);
            assert!(
                p.records().iter().all(|r| file[d.start + r.start] == 0x82),
                "{info:?} {groups:?}"
            );
        }
    }
}
