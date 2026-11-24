//! Additional frozen cases: palette boundaries, frame rectangles and pages.
#[cfg(feature = "encode")]
use super::*;
#[cfg(feature = "encode")]
use imij::{FrameConfig, FrameDuration, FrameEncoder};
use imij::{FrameDecoder, FrameKind};

fn frame_pixels(file: &[u8], threads: usize, padding: usize) -> (Vec<u8>, String, FrameKind) {
    let mut decoder = FrameDecoder::new(file).unwrap();
    let info = decoder.info().clone();
    let bpp = info.bytes_per_pixel();
    let mut stride = info.row_bytes() + padding;
    let mut canvas = vec![0xa5; stride * info.height as usize];
    let mut pixels = Vec::new();
    let mut headers = String::new();
    let mut history: Vec<(Vec<u8>, usize)> = Vec::new();
    while let Some(header) = decoder.next_frame().unwrap() {
        let bpp = if decoder.kind() == FrameKind::Animation {
            bpp
        } else {
            decoder.frame_info().unwrap().bytes_per_pixel()
        };
        let (w, h) = if decoder.kind() != FrameKind::Animation {
            stride = header.width as usize * bpp + padding;
            canvas = vec![0xa5; stride * header.height as usize];
            (header.width, header.height)
        } else {
            (info.width, info.height)
        };
        if let Some(index) = decoder.reference_index() {
            let (reference, reference_stride) = &history[index];
            let row_bytes = w as usize * bpp;
            for y in 0..h as usize {
                canvas[y * stride..y * stride + row_bytes].copy_from_slice(
                    &reference[y * reference_stride..y * reference_stride + row_bytes],
                );
            }
        }
        decoder.decode_into(&mut canvas, stride, threads).unwrap();
        for row in canvas.chunks_exact(stride).take(h as usize) {
            let rb = w as usize * bpp;
            pixels.extend_from_slice(&row[..rb]);
            assert!(row[rb..].iter().all(|&v| v == 0xa5), "frame padding");
        }
        history.push((canvas.clone(), stride));
        headers.push_str(&format!(
            "{},{},{},{},{},{},{},{},{:?};",
            header.index,
            header.x0,
            header.y0,
            header.width,
            header.height,
            header.duration.num,
            header.duration.den,
            header.uses_previous,
            header.coding
        ));
    }
    (pixels, headers, decoder.kind())
}

pub(super) fn check_frames(
    name: &str,
    file: &[u8],
    expected: &[u8],
    kind: &str,
    headers: &str,
    threads: usize,
) {
    for padding in [0, 13] {
        let (got, actual_headers, actual_kind) = frame_pixels(file, threads, padding);
        assert_eq!(got, expected, "{name}: frame samples");
        assert_eq!(actual_headers, headers, "{name}: frame headers");
        assert_eq!(format!("{actual_kind:?}"), kind, "{name}: frame kind");
    }
}

#[cfg(feature = "encode")]
pub(super) fn save_frames(
    f: &mut Fixtures<'_>,
    name: &str,
    info: &ImageInfo,
    encoded: Vec<u8>,
    originals: &[Vec<u8>],
    lossless: bool,
) {
    let (expected, headers, kind) = frame_pixels(&encoded, 1, 0);
    let original = originals.concat();
    assert_eq!(original.len(), expected.len());
    if lossless {
        assert_eq!(expected, original, "{name}: original frames");
    }
    if info.channels.has_alpha() {
        let bpp = info.bytes_per_pixel();
        let bps = info.sample.bytes();
        for (a, b) in original.chunks_exact(bpp).zip(expected.chunks_exact(bpp)) {
            assert_eq!(&a[bpp - bps..], &b[bpp - bps..], "{name}: frame alpha");
        }
    }
    fs::write(f.dir.join(format!("{name}.imi")), encoded).unwrap();
    fs::write(f.dir.join(format!("{name}.raw")), expected).unwrap();
    f.manifest.push_str(&format!(
        "{name}\t{}\t{}\t{:?}\t{:?}\t{lossless}\t{kind:?}\t{headers}\n",
        info.width, info.height, info.channels, info.sample
    ));
    f.count += 1;
}

/// Preserve the immutable initial fixture set: generate this extension in a
/// separate directory, using the same checker and SHA-256 seal.
#[cfg(feature = "encode")]
pub(super) fn generate(dir: &Path) {
    let mut f = Fixtures::new(dir);
    for (si, sample) in SAMPLES.into_iter().enumerate() {
        for (ci, channels) in CHANNELS.into_iter().enumerate() {
            // Exact unique counts exercise palette eligibility around 256.
            // U8 gray has at most 256 values, so only the 257 case is skipped.
            for count in [1, 2, 15, 16, 17, 255, 256, 257] {
                if count == 257 && sample == SampleType::U8 && channels == Channels::Gray {
                    continue;
                }
                let info = ImageInfo::new(257, 19, channels, sample);
                let bpp = info.bytes_per_pixel();
                let mut px = vec![0; info.row_bytes() * info.height as usize];
                for (i, p) in px.chunks_exact_mut(bpp).enumerate() {
                    let value = ((i * 37) % count) as u32;
                    for (c, byte) in p.iter_mut().enumerate() {
                        *byte = (value >> ((c % 4) * 8)) as u8;
                    }
                }
                for groups in [None, Some(32)] {
                    let cfg = {
                        let mut cfg = imij::ImiEncoderConfig::default();
                        cfg = cfg.tune_lossless(|t| {
                            t.palette = true;
                        });
                        cfg = cfg.with_column_groups(groups);
                        cfg = cfg.tune_lossless(|t| {
                            t.stripe_rows = Some(7);
                        });
                        cfg
                    };
                    let encoded = cfg
                        .encode_samples(
                            imij::ImageRef::new(&px, info.row_bytes(), &info),
                            &Metadata::default(),
                        )
                        .unwrap();
                    f.save(
                        &format!("palette-s{si}-c{ci}-n{count}-g{}", groups.unwrap_or(0)),
                        &info,
                        &px,
                        encoded,
                        true,
                    );
                }
            }
            let info = ImageInfo::new(37, 29, channels, sample);
            // Coding index eN: 0 lossless, else the entropy wire value + 1
            // (so 2, packed, and 3, split, are retired — sealed fixtures
            // with those names are rejection checks).
            let mut codings = vec![{
                let mut cfg = imij::ImiEncoderConfig::default();
                cfg = cfg.tune_lossless(|t| {
                    t.palette = false;
                });
                cfg = cfg.tune_lossless(|t| {
                    t.stripe_rows = Some(7);
                });
                cfg = cfg.with_column_groups(Some(16));
                cfg
            }];
            if !matches!(sample, SampleType::U32 | SampleType::I32) {
                for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                    codings.push(
                        imij::ImiEncoderConfig::lossy(85.0).tune_lossy(|t| t.entropy = entropy),
                    );
                }
            }
            for (coding_index, coding) in codings.into_iter().enumerate() {
                // The eN name index is 0 lossless / wire value + 1:
                // remap the vec index past the retired packed and split
                // slots.
                let coding_index = if coding_index < 2 {
                    coding_index
                } else {
                    coding_index + 2
                };
                let lossless = coding.coder().is_lossless();
                let mut originals = vec![pixels(&info, 6, !lossless)];
                originals.push(originals[0].clone()); // Empty repeat frame.
                let replacement = pixels(&info, 2, !lossless);
                for (x, y, w, h) in [(3, 5, 7, 9), (34, 27, 3, 2), (0, 0, 37, 29)] {
                    let mut next = originals.last().unwrap().clone();
                    for row in y..y + h {
                        let start = row * info.row_bytes() + x * info.bytes_per_pixel();
                        let end = start + w * info.bytes_per_pixel();
                        next[start..end].copy_from_slice(&replacement[start..end]);
                    }
                    originals.push(next);
                }
                originals.push(originals.last().unwrap().clone());
                for interval in [0, 3] {
                    let mut cfg = FrameConfig::new(coding.clone());
                    cfg.keyframe_interval = interval;
                    let mut encoder =
                        FrameEncoder::animation(info.clone(), Metadata::default(), cfg);
                    for (index, px) in originals.iter().enumerate() {
                        encoder
                            .push_frame(
                                px,
                                info.row_bytes(),
                                FrameDuration {
                                    num: index as u64 + 1,
                                    den: 60,
                                },
                            )
                            .unwrap();
                    }
                    save_frames(
                        &mut f,
                        &format!("animation-s{si}-c{ci}-e{coding_index}-i{interval}"),
                        &info,
                        encoder.finish().unwrap(),
                        &originals,
                        lossless,
                    );
                }
            }
            let mut encoder = FrameEncoder::pages(
                info.clone(),
                Metadata::default(),
                FrameConfig::new(imij::ImiEncoderConfig::default()),
            );
            let mut pages = Vec::new();
            for (width, height) in [(37, 29), (1, 1), (7, 9), (17, 15)] {
                let page = ImageInfo {
                    width,
                    height,
                    ..info.clone()
                };
                let px = pixels(&page, 7, false);
                encoder.push_page(&px, page.row_bytes(), &page).unwrap();
                pages.push(px);
            }
            save_frames(
                &mut f,
                &format!("pages-s{si}-c{ci}"),
                &info,
                encoder.finish().unwrap(),
                &pages,
                true,
            );
        }
    }
    bilevel(&mut f);
    f.finish();
}

/// Bilevel stripes (SPEC 2.10): two-valued page-like images (strokes of
/// random glyphs) in every channel layout that can carry them, with and
/// without column groups. Each fixture must use the feature.
#[cfg(feature = "encode")]
fn bilevel(f: &mut Fixtures<'_>) {
    let (w, h) = (300usize, 200usize);
    let mut s = 0x2545_f491u32;
    let mut ink = vec![false; w * h];
    for y0 in (3..h - 9).step_by(12) {
        for x0 in (4..w - 7).step_by(7) {
            for _ in 0..3 {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                let (v, at, len) = (s & 1 == 0, (s >> 1) as usize % 5, 3 + (s >> 4) as usize % 5);
                for k in 0..len.min(if v { 8 } else { 5 }) {
                    let (x, y) = if v {
                        (x0 + at, y0 + k)
                    } else {
                        (x0 + k, y0 + at)
                    };
                    ink[y * w + x] = true;
                }
            }
        }
    }
    for (ci, channels) in CHANNELS.into_iter().enumerate() {
        for (si, sample) in [SampleType::U8, SampleType::U16].into_iter().enumerate() {
            let info = ImageInfo::new(w as u32, h as u32, channels, sample);
            let (bps, nch) = (sample.bytes(), channels.count());
            let mut px = Vec::with_capacity(info.row_bytes() * h);
            for &b in &ink {
                for c in 0..nch {
                    // Ink and paper differ in every color channel; alpha
                    // stays opaque except in RGBA, where paper is clear.
                    let v: u16 = match (b, c == nch - 1 && channels.has_alpha()) {
                        (_, true) if channels == Channels::Rgba => [0, 0xffff][b as usize],
                        (_, true) => 0xffff,
                        (true, false) => 0x0f0f * c as u16,
                        (false, false) => 0xffff - c as u16,
                    };
                    px.extend_from_slice(&v.to_ne_bytes()[..bps]);
                }
            }
            for groups in [None, Some(64)] {
                let cfg = {
                    let mut cfg = imij::ImiEncoderConfig::default();
                    cfg = cfg.with_column_groups(groups);
                    cfg = cfg.tune_lossless(|t| {
                        t.stripe_rows = Some(24);
                    });
                    cfg
                };
                let enc = cfg
                    .encode_samples(
                        imij::ImageRef::new(&px, info.row_bytes(), &info),
                        &Metadata::default(),
                    )
                    .unwrap();
                let features = imij::header(&enc).unwrap().features;
                assert!(
                    features.contains(imij::Feature::Bilevel),
                    "bilevel fixture {channels:?}/{sample:?} groups={groups:?}: {features:?}"
                );
                let name = format!("bilevel-s{si}-c{ci}-g{}", groups.unwrap_or(0));
                f.save(&name, &info, &px, enc, true);
            }
        }
    }
}
