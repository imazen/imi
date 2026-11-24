//! Mixed pages (`aNIM` kind 3): pages that differ in pixel format, color,
//! metadata and coding; the outer feature set that covers every page's
//! (SPEC 0.1, 1.3); malformed documents.

use imij::{
    Channels, Cicp, Error, FrameConfig, FrameDecoder, FrameDuration, FrameEncoder, FrameKind,
    ImageInfo, ImiEncoderConfig, LossyColor, Metadata, SampleType, decode,
};

mod common;
#[cfg(feature = "extended")]
use common::feature::SAMPLES_32;
#[cfg(not(feature = "extended"))]
use common::feature::WIDE_SAMPLES;
use common::feature::{
    DEQUANT_PRESETS, EXACT_BLOCKS, LOSSY, MIXED_PAGES, SEED_BANK_1, SEED_BANK_IDS,
};
use common::{anim_of, feature_mask, put_varint, varint, with_anim, with_feature_mask};

fn lossless() -> ImiEncoderConfig {
    imij::ImiEncoderConfig::default()
}

/// Pixels of `info`: a different deterministic pattern per sample type,
/// within each channel's significant bits.
fn pixels(info: &ImageInfo, seed: u32) -> Vec<u8> {
    let (w, h, nch) = (info.width, info.height, info.channels.count());
    let mut v = Vec::with_capacity(info.row_bytes() * h as usize);
    for y in 0..h {
        for x in 0..w {
            for c in 0..nch {
                let t = x.wrapping_mul(3) + y * 5 + c as u32 * 17 + seed * 11;
                // Text-like strokes over a smooth field.
                let ink = (x / 3 + y / 5 + seed).is_multiple_of(7) && !(y / 2).is_multiple_of(3);
                let bits = info.channel_bits(c).unwrap() as u32;
                let max = if bits >= 32 {
                    u32::MAX
                } else {
                    (1u32 << bits) - 1
                };
                let code = if ink {
                    0
                } else {
                    (t * 37 + x * y) % (max.min(1 << 20) + 1)
                };
                match info.sample {
                    SampleType::U8 => v.push(code as u8),
                    SampleType::U16 => v.extend_from_slice(&(code as u16).to_ne_bytes()),
                    SampleType::F16 => {
                        // Finite halves in [0, 2): exponent 14 or 15.
                        let half = 0x3800u16 | (code as u16 & 0x07ff);
                        v.extend_from_slice(&half.to_ne_bytes());
                    }
                    SampleType::F32 => {
                        let f = (t as f32 * 0.37).sin() * 3.0 + if ink { -1.0 } else { 0.5 };
                        v.extend_from_slice(&f.to_ne_bytes());
                    }
                    SampleType::U32 => v.extend_from_slice(&code.to_ne_bytes()),
                    SampleType::I32 => v.extend_from_slice(&(code as i32 - 5000).to_ne_bytes()),
                    SampleType::I8 => v.push((code as i32 - 100) as i8 as u8),
                    SampleType::I16 => {
                        v.extend_from_slice(&(code as i32 - 3000).to_ne_bytes()[..2])
                    }
                    SampleType::U64 => {
                        v.extend_from_slice(&((code as u64) << 33 | t as u64).to_ne_bytes())
                    }
                    SampleType::I64 => v.extend_from_slice(
                        &(((code as i64) << 31) - (t as i64) * 7919).to_ne_bytes(),
                    ),
                    SampleType::F64 => {
                        let f = (t as f64 * 0.37).sin() * 1e3 + if ink { -1.0 } else { 0.25 };
                        v.extend_from_slice(&f.to_ne_bytes());
                    }
                    s => unreachable!("no pattern for {s:?}"),
                }
            }
        }
    }
    v
}

struct Page {
    info: ImageInfo,
    meta: Metadata,
    coding: ImiEncoderConfig,
    px: Vec<u8>,
}

fn page(info: ImageInfo, meta: Metadata, coding: ImiEncoderConfig, seed: u32) -> Page {
    let px = pixels(&info, seed);
    Page {
        info,
        meta,
        coding,
        px,
    }
}

/// A scanned-document-like set: bilevel, 12-bit gray with ICC, lossy color in
/// P3, RGBA16 rotated with EXIF, f16 PQ, lossy gray.
fn document() -> Vec<Page> {
    let bilevel = ImageInfo::new(57, 41, Channels::Gray, SampleType::U8)
        .with_significant_bits(&[1])
        .unwrap();
    let gray12 = ImageInfo::new(33, 29, Channels::Gray, SampleType::U16)
        .with_significant_bits(&[12])
        .unwrap();
    let mut p3 = ImageInfo::new(48, 40, Channels::Rgb, SampleType::U8);
    p3.cicp = Cicp {
        color_primaries: 12,
        ..Cicp::SRGB
    };
    let rgba16 = ImageInfo::new(19, 23, Channels::Rgba, SampleType::U16);
    let mut pq = ImageInfo::new(21, 17, Channels::Rgb, SampleType::F16);
    pq.cicp = Cicp::BT2100_PQ;
    let gray8 = ImageInfo::new(40, 24, Channels::Gray, SampleType::U8);
    vec![
        page(bilevel, Metadata::default(), lossless(), 1),
        page(
            gray12,
            Metadata {
                icc: Some(b"gray icc".to_vec()),
                xmp: Some(b"<x/>".to_vec()),
                ..Default::default()
            },
            lossless(),
            2,
        ),
        page(
            p3,
            Metadata::default(),
            imij::ImiEncoderConfig::lossy(85.0),
            3,
        ),
        page(
            rgba16,
            Metadata {
                exif: Some(b"exif page 4".to_vec()),
                orientation: 6,
                ..Default::default()
            },
            lossless(),
            4,
        ),
        page(pq, Metadata::default(), lossless(), 5),
        page(
            gray8,
            Metadata::default(),
            imij::ImiEncoderConfig::lossy(70.0).tune_lossy(|t| t.color = LossyColor::YCoCg),
            6,
        ),
    ]
}

fn encode_document(pages: &[Page]) -> Vec<u8> {
    let mut e = FrameEncoder::mixed_pages(FrameConfig::new(lossless()));
    for p in pages {
        e.push_page_with(&p.px, p.info.row_bytes(), &p.info, &p.meta, &p.coding)
            .unwrap();
    }
    {
        let file = e.finish_legacy().unwrap();
        common::assert_current_migration_pixels(&file);
        file
    }
}

/// A page coded on its own, decoded on its own: what the page must decode to.
fn standalone(p: &Page) -> (ImageInfo, Vec<u8>) {
    let f = p
        .coding
        .encode_samples(
            imij::ImageRef::new(&p.px, p.info.row_bytes(), &p.info),
            &p.meta,
        )
        .unwrap();
    decode(&f).unwrap()
}

/// Every page: header, info, metadata and pixels.
fn decode_pages(file: &[u8]) -> Vec<(imij::FrameHeader, ImageInfo, Metadata, Vec<u8>)> {
    let mut d = FrameDecoder::new(file).unwrap();
    assert_eq!(d.kind(), FrameKind::MixedPages);
    assert!(d.frame_info().is_none() && d.frame_metadata().is_none());
    let mut out = Vec::new();
    while let Some(h) = d.next_frame().unwrap() {
        let info = d.frame_info().unwrap();
        let meta = d.frame_metadata().unwrap().clone();
        assert_eq!((h.width, h.height), (info.width, info.height));
        let rb = info.row_bytes();
        // A non-tight stride.
        let stride = rb + 5;
        let mut px = vec![0u8; stride * info.height as usize];
        d.decode_into(&mut px, stride, 3).unwrap();
        let tight: Vec<u8> = px
            .chunks(stride)
            .flat_map(|r| r[..rb].iter().copied())
            .collect();
        out.push((h, info, meta, tight));
    }
    assert!(d.frame_info().is_none());
    assert_eq!(out.len(), d.frame_count());
    out
}

/// PR #7's quality targets resolve per page: pages in different longest-side
/// ranges get different quantizer scales, and each page decodes exactly like
/// the same page coded on its own with its target, and with the resolved scale.
#[test]
fn quality_targets_resolve_per_page() {
    use imij::LossyQuality;
    let target = imij::ImiEncoderConfig::lossy(90.0).with_quality(LossyQuality::Ssim2(70.0));
    let pages: Vec<Page> = [(120, 16), (300, 16), (700, 12)]
        .into_iter()
        .enumerate()
        .map(|(i, (w, h))| {
            let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
            page(info, Metadata::default(), target.clone(), i as u32 + 1)
        })
        .collect();
    let scales: Vec<f32> = pages
        .iter()
        .map(|p| {
            target
                .scale_for(LossyColor::Xyb, p.info.width, p.info.height)
                .unwrap()
        })
        .collect();
    assert!(
        scales[0] != scales[1] && scales[1] != scales[2],
        "size ranges must resolve differently: {scales:?}"
    );
    let got = decode_pages(&encode_document(&pages));
    assert_eq!(got.len(), pages.len());
    for ((p, scale), (_, info, _, px)) in pages.iter().zip(&scales).zip(&got) {
        let want = standalone(p);
        assert_eq!(*info, want.0);
        assert!(
            *px == want.1,
            "{}x{} page vs standalone",
            info.width,
            info.height
        );
        let by_scale = Page {
            coding: imij::ImiEncoderConfig::lossy(*scale),
            info: p.info.clone(),
            meta: Metadata::default(),
            px: p.px.clone(),
        };
        assert!(
            *px == standalone(&by_scale).1,
            "{}x{} page vs Scale({scale})",
            info.width,
            info.height
        );
    }
}

#[test]
fn mixed_document_roundtrip() {
    let pages = document();
    let file = encode_document(&pages);
    let got = decode_pages(&file);
    assert_eq!(got.len(), pages.len());
    for (i, ((h, info, meta, px), p)) in got.iter().zip(&pages).enumerate() {
        assert_eq!(h.index, i);
        assert_eq!((h.x0, h.y0, h.uses_previous), (0, 0, false));
        assert_eq!(info, &p.info, "page {i}");
        assert_eq!(meta.icc, p.meta.icc, "page {i}");
        assert_eq!(meta.exif, p.meta.exif, "page {i}");
        assert_eq!(meta.xmp, p.meta.xmp, "page {i}");
        assert_eq!(meta.orientation, p.meta.orientation.max(1), "page {i}");
        let (sinfo, spx) = standalone(p);
        assert_eq!(&sinfo, info);
        assert!(px == &spx, "page {i} differs from the page coded alone");
        if p.coding.coder().is_lossless() {
            assert!(px == &p.px, "lossless page {i}");
        }
    }
    // Readers that know no pages show page 1 with its own metadata.
    let (i0, p0) = decode(&file).unwrap();
    assert_eq!(i0, pages[0].info);
    assert!(p0 == pages[0].px);
    let d = FrameDecoder::new(&file).unwrap();
    assert_eq!(d.info(), &pages[0].info);
    assert_eq!(d.metadata().icc, None);
}

#[test]
fn first_page_metadata_is_the_files() {
    let mut pages = document();
    pages.swap(0, 1);
    let file = encode_document(&pages);
    let d = FrameDecoder::new(&file).unwrap();
    assert_eq!(d.metadata().icc.as_deref(), Some(&b"gray icc"[..]));
    let got = decode_pages(&file);
    assert_eq!(got[0].2.icc.as_deref(), Some(&b"gray icc"[..]));
    // The bilevel page carries none.
    assert_eq!(got[1].2.icc, None);
}

/// The nested file of page `i + 2` (after its `fRAM` header).
fn nested_page(file: &[u8], i: usize) -> &[u8] {
    let (fs, fe) = frames_of(file)[i];
    let p = &file[fs..fe];
    let at = p.windows(8).position(|w| w == common::SIGNATURE).unwrap();
    &p[at..]
}

#[test]
fn outer_mask_covers_every_page() {
    let pages = document();
    let file = encode_document(&pages);
    let (a, b) = anim_of(&file);
    // kind 3, loop 0, 6 pages, 0/1 s: no summary (SPEC 1.3).
    assert_eq!(&file[a..b], [3, 0, 6, 0, 1]);
    // The outer mask covers the varint bank-ID grammar declared by the
    // lossy pages and the YCoCg page's default dequantizer preset. Their
    // headers imply `lossy`; the first page's header does not.
    let outer = feature_mask(&file);
    assert_eq!(outer, MIXED_PAGES | SEED_BANK_IDS | DEQUANT_PRESETS | LOSSY);
    for i in 0..pages.len() - 1 {
        let own = feature_mask(nested_page(&file, i));
        assert_eq!(own & !outer, 0, "page {}", i + 2);
    }
    let open = |f: &[u8]| FrameDecoder::new(f).map(|_| ());
    let walk = |f: &[u8]| -> Result<usize, Error> {
        let mut d = FrameDecoder::new(f)?;
        let mut n = 0;
        while d.next_frame()?.is_some() {
            n += 1;
        }
        Ok(n)
    };
    assert_eq!(walk(&file), Ok(6));
    // A feature this build does not implement, or an unassigned bit:
    // rejected before any page. (Every assigned feature is implemented with
    // `extended`; without it `wide_samples` is not.)
    #[cfg(not(feature = "extended"))]
    assert_eq!(
        open(&with_feature_mask(&file, Some(outer | WIDE_SAMPLES))),
        Err(Error::Unsupported(
            "mixed pages need a profile this build does not decode"
        ))
    );
    assert_eq!(
        open(&with_feature_mask(
            &file,
            Some(outer | common::unassigned_feature())
        )),
        Err(Error::Unsupported("unknown feature"))
    );
    // A mask that omits what a page needs: corrupt when the document opens.
    assert_eq!(
        walk(&with_feature_mask(&file, Some(MIXED_PAGES))),
        Err(Error::Corrupt(
            "page needs a feature the file does not declare"
        ))
    );
    // Declaring more than the pages use is allowed (SPEC 0.1).
    assert_eq!(
        walk(&with_feature_mask(&file, Some(outer | EXACT_BLOCKS))),
        Ok(6)
    );
    // Without `mixed_pages`, kind 3 is corrupt in every entry point.
    let unsignalled = with_feature_mask(&file, Some(SEED_BANK_1));
    let gated = Err(Error::Corrupt("aNIM kind 3 needs the feature mixed_pages"));
    assert_eq!(open(&unsignalled), gated);
    assert_eq!(decode(&unsignalled).map(|_| ()), gated);
    // Newly assigned kind 4 requires its own feature on primary and frame routes.
    let unified_unsignalled = with_anim(&file, &[4, 0, 6, 0, 1, 0]);
    let missing = Err(Error::Corrupt(
        "aNIM kind 4 needs the feature sequence_items",
    ));
    assert_eq!(open(&unified_unsignalled), missing);
    assert_eq!(decode(&unified_unsignalled).map(|_| ()), missing);
    assert_eq!(
        imij::decode_reader(
            &unified_unsignalled[..],
            &imij::Limits::default(),
            |_, _, _| Ok(())
        )
        .map(|_| ()),
        missing
    );
    // Bytes after the kind-3 fields.
    assert!(matches!(
        open(&with_anim(&file, &[3, 0, 6, 0, 1, 0])),
        Err(Error::Corrupt(_))
    ));
    // An unknown kind (0 or above 4) is the frame decoder's to refuse; the
    // declared `mixed_pages` is then unused, which is allowed, so single-
    // image entry points show page one.
    for kind in [0u8, 5] {
        let f = with_anim(&file, &[kind, 0, 6, 0, 1]);
        let unknown = Err(Error::Unsupported("unknown aNIM kind"));
        assert_eq!(open(&f), unknown, "kind {kind}");
        assert_eq!(decode(&f).unwrap().1, decode(&file).unwrap().1);
        imij::header(&f).unwrap();
        imij::decode_reader(&f[..], &imij::Limits::default(), |_, _, _| Ok(())).unwrap();
    }
}

#[test]
fn lossless_document_declares_only_mixed_pages() {
    let pages: Vec<Page> = document()
        .into_iter()
        .filter(|p| p.coding.coder().is_lossless())
        .collect();
    let file = encode_document(&pages);
    let (a, b) = anim_of(&file);
    assert_eq!(&file[a..b], &[3, 0, pages.len() as u8, 0, 1]);
    assert_eq!(feature_mask(&file), MIXED_PAGES);
    assert_eq!(decode_pages(&file).len(), pages.len());
}

#[test]
fn feature_required_even_for_one_page() {
    let file = encode_document(&document()[..1]);
    assert_eq!(decode_pages(&file).len(), 1);
    assert_eq!(feature_mask(&file), MIXED_PAGES);
    for bad in [
        with_feature_mask(&file, None),
        with_feature_mask(&file, Some(0)),
    ] {
        let gated = Err(Error::Corrupt("aNIM kind 3 needs the feature mixed_pages"));
        assert_eq!(FrameDecoder::new(&bad).map(|_| ()), gated);
        assert_eq!(decode(&bad).map(|_| ()), gated);
        assert_eq!(
            imij::decode_reader(&bad[..], &imij::Limits::default(), |_, _, _| Ok(())).map(|_| ()),
            gated
        );
    }
    let unsupported = with_feature_mask(&file, Some(MIXED_PAGES | common::unassigned_feature()));
    assert!(matches!(
        imij::header(&unsupported),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        FrameDecoder::new(&unsupported),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        imij::decode_reader(&unsupported[..], &imij::Limits::default(), |_, _, _| Ok(())),
        Err(Error::Unsupported(_))
    ));
}

/// Prefix reads (`header`, `previews`) of a mixed document that end before
/// `aNIM` (after the header record or the metadata chunks) succeed as for
/// any other file, and so does one that reaches `DATA` without `aNIM`: the
/// feature is then declared but unused (SPEC 0.1). (Review witness,
/// 2026-10-09: every such prefix was `Corrupt`.)
#[test]
fn prefix_before_anim_reads_like_any_file() {
    let meta = Metadata {
        icc: Some(b"probe icc profile bytes".repeat(8)),
        ..Default::default()
    };
    let p1 = ImageInfo::new(16, 8, Channels::Gray, SampleType::U8);
    let pages = vec![
        page(p1.clone(), meta, lossless(), 0),
        page(
            ImageInfo::new(9, 5, Channels::Rgb, SampleType::U8),
            Metadata::default(),
            lossless(),
            1,
        ),
    ];
    let file = encode_document(&pages);
    let (a, b) = anim_of(&file);
    let anim_at = file[..a].windows(4).rposition(|w| w == b"aNIM").unwrap();
    let data_at = file.windows(4).position(|w| w == b"DATA").unwrap();
    assert!(anim_at < data_at);
    let (n, used) = varint(&file[8..]);
    let head_end = 8 + used + n as usize;
    for cut in [head_end, anim_at, data_at] {
        let h = imij::header(&file[..cut]).map(|h| h.info);
        assert_eq!(h, Ok(p1.clone()), "prefix of {cut} bytes");
        assert_eq!(imij::previews(&file[..cut]).map(|v| v.len()), Ok(0));
    }
    // The feature without aNIM: over-declared, so the prefix that reaches
    // DATA (its tag and length) and the whole file read like any file.
    let mut no_anim = file[..anim_at].to_vec();
    no_anim.extend_from_slice(&file[b..]);
    let d = no_anim.windows(4).position(|w| w == b"DATA").unwrap();
    let d = d + 4 + varint(&no_anim[d + 4..]).1;
    assert_eq!(imij::header(&no_anim[..d]).map(|h| h.info), Ok(p1.clone()));
    assert_eq!(imij::header(&no_anim).map(|h| h.info), Ok(p1));
}

/// Kind 3 without the feature is found at `DATA`: the streaming reader
/// delivers no row of page one before rejecting it. (Review witness,
/// 2026-10-09: all of page one's rows came first.) The feature with kind 1
/// is over-declared and reads like any animation's first frame.
#[test]
fn reader_rejects_kind_3_without_the_feature_before_rows() {
    let file = encode_document(&document()[..2]);
    assert_eq!(feature_mask(&file) & MIXED_PAGES, MIXED_PAGES);
    let unsignalled = with_feature_mask(&file, Some(feature_mask(&file) & !MIXED_PAGES));
    let mut rows = 0;
    let r = imij::decode_reader(&unsignalled[..], &imij::Limits::default(), |_, _, _| {
        rows += 1;
        Ok(())
    });
    assert_eq!(
        r.map(|_| ()),
        Err(Error::Corrupt("aNIM kind 3 needs the feature mixed_pages"))
    );
    assert_eq!(rows, 0);
    let animation = with_anim(&file, &[1, 0, 2, 0, 1]);
    imij::decode_reader(&animation[..], &imij::Limits::default(), |_, _, _| Ok(())).unwrap();
}

/// PR #9's signed samples in a mixed document: an i16 page (Lossless 0.2)
/// after a u8 page decodes exactly with `extended`; baseline rejects the
/// document at open, before exposing the supported first page.
#[test]
fn signed_pages_in_a_mixed_document() {
    let pages = vec![
        page(
            ImageInfo::new(9, 7, Channels::Gray, SampleType::U8),
            Metadata::default(),
            lossless(),
            0,
        ),
        page(
            ImageInfo::new(11, 5, Channels::Gray, SampleType::I16),
            Metadata::default(),
            lossless(),
            1,
        ),
        page(
            ImageInfo::new(5, 4, Channels::Rgb, SampleType::I8),
            Metadata::default(),
            lossless(),
            2,
        ),
    ];
    let file = encode_document(&pages);
    if cfg!(feature = "extended") {
        let got = decode_pages(&file);
        for (i, (g, p)) in got.iter().zip(&pages).enumerate() {
            assert_eq!(g.1, p.info, "page {i}");
            assert!(g.3 == p.px, "page {i}");
        }
    } else {
        assert_eq!(
            FrameDecoder::new(&file).err(),
            Some(imij::Error::Unsupported(
                "mixed pages need a profile this build does not decode"
            ))
        );
    }
}

/// PR #9's 64-bit samples (Extended 0.2) in a mixed document decode exactly.
#[cfg(feature = "extended")]
#[test]
fn wide_pages_in_a_mixed_document() {
    let pages = vec![
        page(
            ImageInfo::new(9, 7, Channels::Gray, SampleType::U8),
            Metadata::default(),
            lossless(),
            0,
        ),
        page(
            ImageInfo::new(6, 6, Channels::Rgb, SampleType::F64),
            Metadata::default(),
            lossless(),
            1,
        ),
        page(
            ImageInfo::new(7, 3, Channels::Gray, SampleType::U64),
            Metadata::default(),
            lossless(),
            2,
        ),
        page(
            ImageInfo::new(3, 7, Channels::Gray, SampleType::I64),
            Metadata::default(),
            lossless(),
            3,
        ),
    ];
    let got = decode_pages(&encode_document(&pages));
    for (i, (g, p)) in got.iter().zip(&pages).enumerate() {
        assert_eq!(g.1, p.info, "page {i}");
        assert!(g.3 == p.px, "page {i}");
    }
}

/// 32-bit samples are implied by a page's own header (no mask bit there),
/// and the outer header, whose 8-bit page one does not imply them, declares
/// `samples_32` (SPEC 0.1), so a build without `extended` rejects the
/// document at the header.
#[cfg(feature = "extended")]
#[test]
fn extended_pages_declare_samples_32_in_the_outer_mask() {
    let f32p = ImageInfo::new(13, 11, Channels::Gray, SampleType::F32);
    let pages = vec![
        page(
            ImageInfo::new(9, 9, Channels::Gray, SampleType::U8),
            Metadata::default(),
            lossless(),
            0,
        ),
        // Regular maps, so `samples_32` is the page's only feature.
        page(
            f32p,
            Metadata::default(),
            {
                let mut cfg = imij::ImiEncoderConfig::default();
                cfg = cfg.tune_lossless(|t| {
                    t.value_tables = false;
                });
                cfg
            },
            1,
        ),
    ];
    let file = encode_document(&pages);
    let (a, b) = anim_of(&file);
    assert_eq!(&file[a..b], &[3, 0, 2, 0, 1]);
    assert_eq!(feature_mask(&file), MIXED_PAGES | SAMPLES_32);
    assert_eq!(feature_mask(nested_page(&file, 0)), 0);
    let got = decode_pages(&file);
    assert!(got[1].3 == pages[1].px);
}

/// A lossy page whose nested DC image has 32-bit samples: the page declares
/// `samples_32` and what its nested heads use (the offset map), the outer
/// mask declares them too, and a mask that omits them is corrupt when the
/// document opens. A build without `extended` therefore rejects the
/// document at the outer header (review witness 2026-10-09: the document
/// used to open and fail at page 2; SPEC 0.1 since 2026-10-11).
#[cfg(feature = "extended")]
#[test]
fn lossy_page_with_32bit_dc_declares_its_features() {
    let (w, h) = (64u32, 64u32);
    let rgb = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    // Full-range content at q120 (near-lossless steps) needs i32 DCs.
    let full: Vec<u8> = (0..(w * h * 3) as usize)
        .map(|i| {
            if (i / 24) % 2 == 0 {
                255
            } else {
                (i * 7 % 256) as u8
            }
        })
        .collect();
    for color in [LossyColor::YCoCg, LossyColor::Xyb] {
        let gray = ImageInfo::new(8, 8, Channels::Gray, SampleType::U8);
        let mut e = FrameEncoder::mixed_pages(FrameConfig::new(lossless()));
        e.push_page(&[7u8; 64], 8, &gray).unwrap();
        let coding = imij::ImiEncoderConfig::lossy(120.0).tune_lossy(|t| t.color = color);
        e.push_page_with(&full, rgb.row_bytes(), &rgb, &Metadata::default(), &coding)
            .unwrap();
        let file = e.finish().unwrap();
        let (a, b) = anim_of(&file);
        assert_eq!(&file[a..b], &[3, 0, 2, 0, 1], "{color:?}");
        let own = feature_mask(nested_page(&file, 0));
        assert_eq!(
            own & common::feature::OFFSET_MAP,
            common::feature::OFFSET_MAP
        );
        // The page's mask declares `samples_32` (its i32 DC image); page one
        // is lossless, so the outer mask declares `lossy` too.
        assert_eq!(own & SAMPLES_32, SAMPLES_32, "{color:?}");
        assert_eq!(feature_mask(&file), MIXED_PAGES | LOSSY | own, "{color:?}");
        assert_eq!(decode_pages(&file).len(), 2);
        // A mask that omits them: corrupt when the document opens.
        let omitted = with_feature_mask(&file, Some(MIXED_PAGES | LOSSY));
        assert_eq!(
            FrameDecoder::new(&omitted).err(),
            Some(Error::Corrupt(
                "page needs a feature the file does not declare"
            )),
            "{color:?}"
        );
    }
}

/// A build without `extended` knows Extended 0.1 but cannot decode 32-bit
/// samples: a mixed document that needs it is rejected when opened, before
/// page one, whether a later page (via the aNIM summary) or page one needs
/// it. Header inspection still works. (Review witness, 2026-10-08: the
/// document used to open and fail only at the 32-bit page.)
#[cfg(not(feature = "extended"))]
#[test]
fn build_without_extended_rejects_extended_pages_at_open() {
    let narrow = || {
        page(
            ImageInfo::new(4, 4, Channels::Gray, SampleType::U8),
            Metadata::default(),
            lossless(),
            0,
        )
    };
    let wide = || {
        page(
            ImageInfo::new(4, 4, Channels::Gray, SampleType::U32),
            Metadata::default(),
            lossless(),
            1,
        )
    };
    for (pages, first_sample) in [
        (vec![narrow(), wide()], SampleType::U8),
        (vec![wide(), narrow()], SampleType::U32),
    ] {
        let file = encode_document(&pages);
        assert_eq!(
            FrameDecoder::new(&file).err(),
            Some(Error::Unsupported(
                "mixed pages need a profile this build does not decode"
            ))
        );
        assert_eq!(imij::header(&file).unwrap().info.sample, first_sample);
    }
}

/// Pages of a mixed document may share one format: `push_page` codes them
/// with the encoder's coding and no metadata.
#[test]
fn push_page_on_mixed_encoder() {
    let mut e = FrameEncoder::mixed_pages(FrameConfig::new(lossless()));
    let infos = [
        ImageInfo::new(20, 10, Channels::Rgb, SampleType::U8),
        ImageInfo::new(10, 20, Channels::Gray, SampleType::U16),
    ];
    let px: Vec<Vec<u8>> = infos.iter().map(|i| pixels(i, 7)).collect();
    for (i, p) in infos.iter().zip(&px) {
        e.push_page(p, i.row_bytes(), i).unwrap();
    }
    let got = decode_pages(&e.finish().unwrap());
    for ((_, info, _, got), (i, want)) in got.iter().zip(infos.iter().zip(&px)) {
        assert_eq!(info, i);
        assert!(got == want);
    }
}

#[test]
fn encoder_misuse() {
    let info = ImageInfo::new(4, 4, Channels::Gray, SampleType::U8);
    let px = pixels(&info, 0);
    let mut uniform = FrameEncoder::pages(
        info.clone(),
        Metadata::default(),
        FrameConfig::new(lossless()),
    );
    assert_eq!(
        uniform.push_page_with(&px, 4, &info, &Metadata::default(), &lossless()),
        Err(Error::InvalidInput("not a mixed-pages encoder"))
    );
    let mut mixed = FrameEncoder::mixed_pages(FrameConfig::new(lossless()));
    assert!(
        mixed
            .push_frame(&px, 4, FrameDuration::from_millis(10))
            .is_err()
    );
    assert!(
        FrameEncoder::mixed_pages(FrameConfig::new(lossless()))
            .finish()
            .is_err()
    );
}

/// The `fRAM` payloads of a file: (start, end) of each.
fn frames_of(file: &[u8]) -> Vec<(usize, usize)> {
    let mut pos = 8;
    let (n, used) = varint(&file[pos..]);
    pos += used + n as usize;
    let mut out = Vec::new();
    while pos < file.len() {
        let tag = &file[pos..pos + 4];
        let (len, used) = varint(&file[pos + 4..]);
        let start = pos + 4 + used;
        let end = start + len as usize - 1;
        if tag == b"fRAM" {
            out.push((start, end));
        }
        pos = end;
    }
    out
}

#[test]
fn malformed_pages() {
    let pages = document();
    let file = encode_document(&pages);
    let walk = |f: &[u8]| -> Result<usize, Error> {
        let mut d = FrameDecoder::new(f)?;
        let mut n = 0;
        while d.next_frame()?.is_some() {
            n += 1;
        }
        Ok(n)
    };
    assert_eq!(walk(&file), Ok(6));
    let (s, _) = frames_of(&file)[1];
    // fRAM payload: x0 0, y0 0, duration 0/1, flags 0.
    assert_eq!(&file[s..s + 5], &[0, 0, 0, 1, 0]);
    for (at, v) in [(s, 1u8), (s + 1, 2), (s + 4, 1), (s + 4, 2), (s + 4, 4)] {
        let mut bad = file.clone();
        bad[at] = v;
        assert!(
            matches!(walk(&bad), Err(Error::Corrupt(_))),
            "byte {at} = {v}"
        );
    }
    // A page that is itself a multi-page file.
    let inner = encode_document(&document()[..2]);
    let mut nested = Vec::new();
    nested.extend_from_slice(&[0, 0, 0, 1, 0]);
    nested.extend_from_slice(&inner);
    let (fs, fe) = frames_of(&file)[0];
    let mut start = fs - 1;
    while file[start - 1] & 0x80 != 0 {
        start -= 1;
    }
    let mut bad = file[..start].to_vec();
    put_varint(&mut bad, nested.len() as u64 + 1);
    bad.extend_from_slice(&nested);
    bad.extend_from_slice(&file[fe..]);
    assert_eq!(walk(&bad), Err(Error::Corrupt("page holds frames")));
}

#[test]
fn corrupt_mixed_pages_do_not_panic() {
    let file = encode_document(&document());
    let mut s = 0x2545_f491u32;
    for _ in 0..2000 {
        let mut bad = file.clone();
        for _ in 0..2 {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let i = s as usize % bad.len();
            bad[i] ^= (s >> 24) as u8 | 1;
        }
        let Ok(mut d) = FrameDecoder::new(&bad) else {
            continue;
        };
        while let Ok(Some(_)) = d.next_frame() {
            let Some(info) = d.frame_info() else {
                continue;
            };
            let rb = info.row_bytes();
            if rb * info.height as usize > 1 << 24 {
                continue;
            }
            let mut out = vec![0u8; rb * info.height as usize];
            let _ = d.decode_into(&mut out, rb, 2);
        }
    }
}

/// Mixed pages with a file checksum (SPEC 1.4): header records stay in id
/// order, one outer table covers every page, metadata remux keeps it valid,
/// and a reader without checksums sees the plain document.
#[test]
fn mixed_pages_checksum_orders_extensions_and_preserves_all_pages() {
    use common::checksum as cs;
    let mut pages = document();
    pages[0].meta.icc = Some(b"acspmntrRGB XYZ rXYZgXYZbXYZrTRCgTRCbTRC".repeat(64));
    for count in [1, pages.len()] {
        let chosen = &pages[..count];
        let plain = encode_document(chosen);
        let mut encoder = FrameEncoder::mixed_pages(FrameConfig::new(
            imij::ImiEncoderConfig::default().with_checksum(true),
        ));
        for p in chosen {
            encoder
                .push_page_with(&p.px, p.info.row_bytes(), &p.info, &p.meta, &p.coding)
                .unwrap();
        }
        let file = encoder.finish_legacy().unwrap();
        // The checksum record (2, k = 16), then the DATA locator (4) when
        // page one has two or more units.
        let ids = |f: &[u8]| -> Vec<u64> { common::extensions(f).iter().map(|e| e.0).collect() };
        assert!(matches!(&ids(&file)[..], [2] | [2, 4]), "{:?}", ids(&file));
        assert_eq!(file[cs::record_at(&file)], 16);
        // Without the record and the table it is the plain document, so a
        // reader that skips both reads the same pages.
        assert_eq!(cs::strip_all(&file), plain);
        assert_eq!(decode_pages(&file), decode_pages(&plain));
        cs::accepted(&file, &chosen[0].px, "mixed pages checksum");
        cs::accepted_on(&cs::OFF_PATHS, &file, &chosen[0].px, "verification off");
        let (a, b) = cs::csum(&file);
        assert_eq!(&file[a..b], cs::table_chunk(&cs::strip(&file), 16, 16));
        // A changed byte in the last page fails before any page decodes.
        if count > 1 {
            let last = cs::chunks(&file)
                .into_iter()
                .rfind(|c| &c.1 == b"fRAM")
                .unwrap()
                .2;
            let mut bad = file.clone();
            bad[last.start + last.len() / 2] ^= 1;
            assert_eq!(FrameDecoder::new(&bad).err(), Some(cs::MISMATCH));
        }
        let packed = imij::compress_metadata(&file, 1 << 20).unwrap();
        assert!(packed.len() < file.len());
        assert_eq!(ids(&packed), ids(&file));
        assert_eq!(packed[cs::record_at(&packed)], 16);
        assert_eq!(decode_pages(&packed), decode_pages(&file));
        cs::accepted(&packed, &chosen[0].px, "mixed pages checksum remux");
        let (pa, pb) = cs::csum(&packed);
        assert_eq!(
            &packed[pa..pb],
            cs::table_chunk(&cs::strip(&packed), 16, 16)
        );
        // Records in the wrong order: corrupt, before any checksum or pixel
        // work.
        let mut recs = common::extensions(&file);
        recs.insert(0, (6, vec![1]));
        let unordered = common::with_extensions(&file, &recs);
        assert_eq!(
            imij::header(&unordered).err(),
            Some(Error::Corrupt("header records out of order"))
        );
    }
}

/// `file` with page `i + 2`'s nested file replaced by `nested`.
fn with_page(file: &[u8], i: usize, nested: &[u8]) -> Vec<u8> {
    let (fs, fe) = frames_of(file)[i];
    let mut start = fs - 1;
    while file[start - 1] & 0x80 != 0 {
        start -= 1;
    }
    let mut payload = vec![0, 0, 0, 1, 0];
    payload.extend_from_slice(nested);
    let mut out = file[..start].to_vec();
    put_varint(&mut out, payload.len() as u64 + 1);
    out.extend_from_slice(&payload);
    out.extend_from_slice(&file[fe..]);
    out
}

/// Encoders never put a `cSUM` in a nested page, but a page that carries
/// one is read like the rest of the file: with checksum verification off
/// (`Limits::verify_checksum`), nothing is hashed and a bad nested table
/// does not stop the page.
#[test]
fn nested_page_checksums_follow_the_verify_flag() {
    use common::checksum as cs;
    let pages = document();
    let file = encode_document(&pages[..3]);
    let p = &pages[1];
    let cfg = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.with_checksum(true);
        cfg
    };
    let nested = cfg
        .encode_samples(
            imij::ImageRef::new(&p.px, p.info.row_bytes(), &p.info),
            &p.meta,
        )
        .unwrap();
    let mut bad_nested = nested.clone();
    let (_, end) = cs::csum(&bad_nested);
    bad_nested[end - 1] ^= 1;
    assert_eq!(decode(&bad_nested).err(), Some(cs::MISMATCH));
    let (_, want) = standalone(p);
    for (name, n) in [("good", &nested), ("bad", &bad_nested)] {
        let doc = with_page(&file, 0, n);
        let mut d = FrameDecoder::with_limits(&doc, &cs::OFF).unwrap();
        d.next_frame().unwrap().unwrap();
        let h = d.next_frame().unwrap().unwrap();
        let mut px = vec![0u8; p.info.row_bytes() * h.height as usize];
        d.decode_into(&mut px, p.info.row_bytes(), 2).unwrap();
        assert_eq!(px, want, "{name} nested table, verification off");
        assert!(d.next_frame().unwrap().is_some());
        assert!(d.next_frame().unwrap().is_none());
    }
}
