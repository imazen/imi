//! Animations and pages: round trips, headers, compositing, corruption.

mod common;

use imij::{
    Channels, Error, FrameConfig, FrameDecoder, FrameDuration, FrameEncoder, FrameKind, ImageInfo,
    Metadata, SampleType, decode,
};

fn frame(w: u32, h: u32, nch: usize, t: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h) as usize * nch);
    for y in 0..h {
        for x in 0..w {
            for c in 0..nch {
                // A moving box over a gradient.
                let inside = x >= t * 3 % w && x < t * 3 % w + 9 && y >= t % h && y < t % h + 7;
                v.push(if inside {
                    250 - c as u8 * 40
                } else {
                    ((x * 2 + y + c as u32 * 30) % 256) as u8
                });
            }
        }
    }
    v
}

fn decode_all(file: &[u8]) -> (FrameKind, Vec<(imij::FrameHeader, Vec<u8>)>) {
    let mut d = FrameDecoder::new(file).unwrap();
    let info = d.info().clone();
    let rb = info.row_bytes();
    let mut canvas = vec![0u8; rb * info.height as usize];
    let mut out = Vec::new();
    while let Some(h) = d.next_frame().unwrap() {
        if d.kind() == FrameKind::Pages {
            let prb = rb / info.width as usize * h.width as usize;
            let mut page = vec![0u8; prb * h.height as usize];
            d.decode_into(&mut page, prb, 3).unwrap();
            out.push((h, page));
        } else {
            d.decode_into(&mut canvas, rb, 3).unwrap();
            out.push((h, canvas.clone()));
        }
    }
    (d.kind(), out)
}

#[test]
fn animation_roundtrip() {
    for (ch, nch) in [(Channels::Rgb, 3), (Channels::Rgba, 4), (Channels::Gray, 1)] {
        let (w, h) = (61u32, 37u32);
        let info = ImageInfo::new(w, h, ch, SampleType::U8);
        for coding in [
            imij::ImiEncoderConfig::default(),
            imij::ImiEncoderConfig::lossy(90.0),
        ] {
            let lossless = coding.coder().is_lossless();
            let mut cfg = FrameConfig::new(coding);
            cfg.loop_count = 3;
            let meta = Metadata {
                exif: Some(b"exif".to_vec()),
                ..Default::default()
            };
            let mut e = FrameEncoder::animation(info.clone(), meta.clone(), cfg);
            let frames: Vec<Vec<u8>> = (0..6).map(|t| frame(w, h, nch, t)).collect();
            for (t, f) in frames.iter().enumerate() {
                e.push_frame(
                    f,
                    info.row_bytes(),
                    FrameDuration {
                        num: t as u64 + 1,
                        den: 30,
                    },
                )
                .unwrap();
            }
            let file = e.finish().unwrap();
            // Plain decoders see frame 0.
            let (i0, p0) = decode(&file).unwrap();
            assert_eq!(i0, info);
            let (kind, got) = decode_all(&file);
            assert_eq!(kind, FrameKind::Animation);
            assert_eq!(got.len(), 6);
            assert_eq!(got[0].1, p0);
            let d = FrameDecoder::new(&file).unwrap();
            assert_eq!(d.loop_count(), 3);
            assert_eq!(d.metadata().exif.as_deref(), Some(&b"exif"[..]));
            for (t, (h, px)) in got.iter().enumerate() {
                assert_eq!(h.index, t);
                assert_eq!(
                    h.duration,
                    FrameDuration {
                        num: t as u64 + 1,
                        den: 30
                    }
                );
                if lossless {
                    assert!(px == &frames[t], "{ch:?} frame {t}");
                } else {
                    let err = px
                        .iter()
                        .zip(&frames[t])
                        .map(|(a, b)| (*a as i32 - *b as i32).unsigned_abs())
                        .max()
                        .unwrap();
                    assert!(err < 80, "{ch:?} lossy frame {t}: max error {err}");
                }
            }
        }
    }
}

#[test]
fn pages_roundtrip() {
    let sizes = [(40u32, 30u32), (17, 50), (64, 8)];
    let base = ImageInfo::new(1, 1, Channels::Rgb, SampleType::U8);
    let mut e = FrameEncoder::pages(
        base,
        Metadata::default(),
        FrameConfig::new(imij::ImiEncoderConfig::default()),
    );
    let pages: Vec<(ImageInfo, Vec<u8>)> = sizes
        .iter()
        .enumerate()
        .map(|(i, &(w, h))| {
            (
                ImageInfo::new(w, h, Channels::Rgb, SampleType::U8),
                frame(w, h, 3, i as u32),
            )
        })
        .collect();
    for (info, px) in &pages {
        e.push_page(px, info.row_bytes(), info).unwrap();
    }
    let file = e.finish().unwrap();
    let (kind, got) = decode_all(&file);
    assert_eq!(kind, FrameKind::Pages);
    assert_eq!(got.len(), 3);
    for ((h, px), (info, want)) in got.iter().zip(&pages) {
        assert_eq!((h.width, h.height), (info.width, info.height));
        assert!(px == want);
    }
}

#[test]
fn still_file_is_one_frame() {
    let info = ImageInfo::new(9, 7, Channels::Rgb, SampleType::U8);
    let px = frame(9, 7, 3, 0);
    let file = imij::ImiEncoderConfig::default()
        .encode_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let (kind, got) = decode_all(&file);
    assert_eq!(kind, FrameKind::Still);
    assert_eq!(got.len(), 1);
    assert!(got[0].1 == px);
}

/// The file with the second frame's `fRAM` controls after the duration
/// replaced by `controls` (the nested file follows unchanged).
fn with_frame_controls(file: &[u8], controls: &[u8]) -> Vec<u8> {
    let (header, mut chunks) = common::parts(file);
    let (_, p) = chunks.iter_mut().find(|(t, _)| t == b"fRAM").unwrap();
    let mut at = 0;
    for _ in 0..4 {
        common::read_varint(p, &mut at);
    }
    let present = common::read_varint(p, &mut at);
    // This encoder writes the presence bits alone, in one byte.
    assert_eq!(present & !3, 0);
    p.splice(at - 1..at, controls.iter().copied());
    common::join(&header, &chunks, false)
}

#[test]
fn frame_controls_are_a_presence_varint() {
    // SPEC 1.3: bit 0 uses the previous frame, bit 1 empty, bit 2 a frame
    // record list follows; bits 3 and up are Unsupported. No frame record
    // is defined: even ids are skipped, odd ids Unsupported.
    let (w, h) = (16u32, 8u32);
    let info = ImageInfo::new(w, h, Channels::Gray, SampleType::U8);
    let mut e = FrameEncoder::pages(
        info.clone(),
        Metadata::default(),
        FrameConfig::new(imij::ImiEncoderConfig::default()),
    );
    for t in 0..2 {
        e.push_page(&frame(w, h, 1, t), w as usize, &info).unwrap();
    }
    let file = e.finish().unwrap();
    let second = |f: &[u8]| -> Result<Vec<u8>, Error> {
        let mut d = FrameDecoder::new(f)?;
        d.next_frame()?;
        d.next_frame()?.ok_or(Error::Corrupt("no second page"))?;
        let mut page = vec![0u8; (w * h) as usize];
        d.decode_into(&mut page, w as usize, 1)?;
        Ok(page)
    };
    let want = second(&file).unwrap();
    assert_eq!(second(&with_frame_controls(&file, &[0])), Ok(want.clone()));
    // A record list with an ignorable record decodes the same page.
    assert_eq!(
        second(&with_frame_controls(&file, &[4, 3, 2, 1, 0xaa])),
        Ok(want.clone())
    );
    assert_eq!(second(&with_frame_controls(&file, &[4, 0])), Ok(want));
    assert_eq!(
        second(&with_frame_controls(&file, &[4, 2, 1, 0])),
        Err(Error::Unsupported("unknown frame record"))
    );
    assert_eq!(
        second(&with_frame_controls(&file, &[8])),
        Err(Error::Unsupported("unknown frame field"))
    );
    assert_eq!(
        second(&with_frame_controls(&file, &[0x80, 0x01])),
        Err(Error::Unsupported("unknown frame field"))
    );
    // A page never uses the previous one.
    assert_eq!(
        second(&with_frame_controls(&file, &[1])),
        Err(Error::Corrupt("page with a rectangle"))
    );
    // The record list stays inside the chunk.
    assert_eq!(
        second(&with_frame_controls(&file, &[4, 0xff, 0xff, 0x03])),
        Err(Error::Corrupt("frame records past fRAM"))
    );
}

#[test]
fn corrupt_frames_do_not_panic() {
    let (w, h) = (33u32, 21u32);
    let info = ImageInfo::new(w, h, Channels::Rgba, SampleType::U8);
    for coding in [
        imij::ImiEncoderConfig::default(),
        imij::ImiEncoderConfig::lossy(80.0),
    ] {
        let mut e =
            FrameEncoder::animation(info.clone(), Metadata::default(), FrameConfig::new(coding));
        for t in 0..5 {
            e.push_frame(
                &frame(w, h, 4, t),
                info.row_bytes(),
                FrameDuration::from_millis(40),
            )
            .unwrap();
        }
        let file = e.finish().unwrap();
        let mut s = 0x9e37_79b9u32;
        for _ in 0..1500 {
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
            let rb = d.info().row_bytes();
            let mut canvas = vec![0u8; rb * d.info().height as usize];
            while let Ok(Some(_)) = d.next_frame() {
                let _ = d.decode_into(&mut canvas, rb, 2);
            }
        }
    }
}

/// Two small objects moving far apart over a photo-like background: the
/// changed rectangle spans most of the canvas but few pixels change.
fn two_objects(w: u32, h: u32, nch: usize, bps: usize, t: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h) as usize * nch * bps);
    for y in 0..h {
        for x in 0..w {
            for c in 0..nch {
                let a = (2 + t..8 + t).contains(&x) && (3..9).contains(&y);
                let b = x + 12 + t >= w && x + 6 + t < w && y + 10 >= h && y + 4 < h;
                let bg = ((x * 7 + y * 3 + c as u32 * 50) ^ (x * y / 7)) % 251;
                let val = if a || b { 255 - c as u32 * 30 } else { bg };
                if bps == 1 {
                    v.push(val as u8);
                } else {
                    // 16-bit (or f16 bit patterns kept finite).
                    let s = ((val << 8) | (x & 255))
                        & if bps == 2 && nch > 0 { 0x3BFF } else { 0xFFFF };
                    v.extend_from_slice(&(s as u16).to_ne_bytes());
                }
            }
        }
    }
    v
}

#[test]
fn lossless_inter_frames() {
    for (ch, st) in [
        (Channels::Rgb, SampleType::U8),
        (Channels::Rgba, SampleType::U8),
        (Channels::Gray, SampleType::U8),
        (Channels::GrayAlpha, SampleType::U16),
        (Channels::Rgb, SampleType::F16),
    ] {
        let (w, h) = (90u32, 60u32);
        let info = ImageInfo::new(w, h, ch, st);
        let frames: Vec<Vec<u8>> = (0..8)
            .map(|t| two_objects(w, h, ch.count(), st.bytes(), t))
            .collect();
        let mut sizes = [0usize; 2];
        for (k, inter) in [false, true].into_iter().enumerate() {
            for groups in [None, Some(32u32)] {
                let mut c = {
                    let mut cfg = imij::ImiEncoderConfig::default();
                    cfg = cfg.tune_lossless(|t| {
                        t.stripe_rows = Some(16);
                    });
                    cfg
                };
                c = c.with_column_groups(groups);
                let mut cfg = FrameConfig::new(c);
                cfg.inter = inter;
                let mut e = FrameEncoder::animation(info.clone(), Metadata::default(), cfg);
                for f in &frames {
                    e.push_frame(f, info.row_bytes(), FrameDuration::from_millis(50))
                        .unwrap();
                }
                let file = e.finish().unwrap();
                if groups.is_none() {
                    sizes[k] = file.len();
                }
                let (_, got) = decode_all(&file);
                for (t, (hd, px)) in got.iter().enumerate() {
                    assert!(
                        px == &frames[t],
                        "{ch:?} {st:?} inter {inter} groups {groups:?} frame {t}"
                    );
                    if inter && t > 0 {
                        assert!(hd.uses_previous, "frame {t} not inter");
                    }
                }
            }
        }
        assert!(
            sizes[1] * 2 < sizes[0],
            "{ch:?} {st:?}: inter {} vs intra-rect {}",
            sizes[1],
            sizes[0]
        );
    }
}

/// A nested inter frame that predicts from the previous frame is not a
/// valid stand-alone image.
#[test]
fn inter_frame_needs_its_reference() {
    let (w, h) = (90u32, 60u32);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let mut e = FrameEncoder::animation(
        info.clone(),
        Metadata::default(),
        FrameConfig::new(imij::ImiEncoderConfig::default()),
    );
    for t in 0..3 {
        e.push_frame(
            &two_objects(w, h, 3, 1, t),
            info.row_bytes(),
            FrameDuration::from_millis(50),
        )
        .unwrap();
    }
    let file = e.finish().unwrap();
    // The last fRAM chunk's nested file starts at its imij signature.
    let sig = *b"IMIJ\r\n\x87\n";
    let at = file.windows(8).rposition(|x| x == sig).unwrap();
    assert!(at > 0);
    assert!(decode(&file[at..]).is_err());
}

#[test]
fn lossy_inter_frames() {
    let (w, h) = (120u32, 72u32);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let frames: Vec<Vec<u8>> = (0..8).map(|t| two_objects(w, h, 3, 1, t)).collect();
    let mut sizes = [0usize; 2];
    let mut maes: [Vec<f64>; 2] = [Vec::new(), Vec::new()];
    for (k, inter) in [false, true].into_iter().enumerate() {
        let mut cfg = FrameConfig::new(imij::ImiEncoderConfig::lossy(85.0));
        cfg.inter = inter;
        let mut e = FrameEncoder::animation(info.clone(), Metadata::default(), cfg);
        for f in &frames {
            e.push_frame(f, info.row_bytes(), FrameDuration::from_millis(50))
                .unwrap();
        }
        let file = e.finish().unwrap();
        sizes[k] = file.len();
        let (_, got) = decode_all(&file);
        for (t, (_, px)) in got.iter().enumerate() {
            let sum: u64 = px
                .iter()
                .zip(&frames[t])
                .map(|(a, b)| (*a as i32 - *b as i32).unsigned_abs() as u64)
                .sum();
            maes[k].push(sum as f64 / px.len() as f64);
        }
    }
    for (t, (inter, intra)) in maes[1].iter().zip(&maes[0]).enumerate() {
        assert!(
            *inter <= intra * 1.1 + 0.25,
            "frame {t}: inter mean error {inter} vs intra {intra}"
        );
    }
    assert!(
        sizes[1] * 2 < sizes[0],
        "lossy inter {} vs intra {}",
        sizes[1],
        sizes[0]
    );
}

/// Three distinct full images: the failing admission must be for the last
/// nested frame/page, beyond both a primary image and a valid nested image.
fn admission_document(kind: FrameKind) -> Vec<u8> {
    let info = ImageInfo::new(16, 8, Channels::Gray, SampleType::U8);
    let mut config = FrameConfig::new(imij::ImiEncoderConfig::default());
    config.inter = false;
    let mut encoder = match kind {
        FrameKind::Animation => FrameEncoder::animation(info.clone(), Metadata::default(), config),
        FrameKind::Pages => FrameEncoder::pages(info.clone(), Metadata::default(), config),
        FrameKind::MixedPages => FrameEncoder::mixed_pages(config),
        _ => unreachable!(),
    };
    for t in 0..3 {
        let pixels = frame(16, 8, 1, t);
        if kind == FrameKind::Animation {
            encoder
                .push_frame(&pixels, 16, FrameDuration::from_millis(10))
                .unwrap();
        } else {
            encoder.push_page(&pixels, 16, &info).unwrap();
        }
    }
    let file = encoder.finish().unwrap();
    let mut decoder = FrameDecoder::new(&file).unwrap();
    for t in 0..3 {
        assert_eq!(decoder.next_frame().unwrap().unwrap().index, t as usize);
        let mut pixels = vec![0; 128];
        decoder.decode_into(&mut pixels, 16, 2).unwrap();
        assert_eq!(pixels, frame(16, 8, 1, t));
    }
    assert!(decoder.next_frame().unwrap().is_none());
    file
}

fn with_last_frame(file: &[u8], update: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let (header, mut chunks) = common::parts(file);
    let frames: Vec<_> = chunks
        .iter_mut()
        .filter(|(tag, _)| tag == b"fRAM")
        .collect();
    assert_eq!(frames.len(), 2);
    let (_, payload) = frames.into_iter().next_back().unwrap();
    update(payload);
    common::join(&header, &chunks, false)
}

fn last_frame_image_at(payload: &[u8]) -> usize {
    let mut at = 0;
    for _ in 0..4 {
        common::read_varint(payload, &mut at);
    }
    assert_eq!(common::read_varint(payload, &mut at), 0);
    at
}

fn assert_constructor_error(file: &[u8], expected: Error) {
    for result in [
        FrameDecoder::new(file),
        FrameDecoder::with_limits(file, &imij::Limits::DEFAULT),
        FrameDecoder::with_limits(file, &{
            let mut limits = imij::Limits::DEFAULT;
            limits.verify_checksum = false;
            limits
        }),
    ] {
        assert_eq!(result.err().as_ref(), Some(&expected));
    }
}

#[test]
fn later_frame_headers_are_admitted_at_construction() {
    for kind in [
        FrameKind::Animation,
        FrameKind::Pages,
        FrameKind::MixedPages,
    ] {
        let file = admission_document(kind);
        let over = common::with_feature_mask(&file, Some(common::feature_mask(&file) | (1 << 18)));
        assert_constructor_error(&over, Error::Unsupported("native components retired in v3"));
        // Over-declaration remains valid for supported, non-retired capabilities.
        let ordinary =
            common::with_feature_mask(&file, Some(common::feature_mask(&file) | (1 << 17)));
        FrameDecoder::new(&ordinary).unwrap();
        let retired_nested = with_last_frame(&file, |payload| {
            let at = last_frame_image_at(payload);
            let mask = common::feature_mask(&payload[at..]) | (1 << 18);
            let nested = common::with_feature_mask(&payload[at..], Some(mask));
            payload.truncate(at);
            payload.extend_from_slice(&nested);
        });
        assert_constructor_error(
            &retired_nested,
            Error::Unsupported("native components retired in v3"),
        );
        let first_unknown = common::unassigned_feature().trailing_zeros() as usize;
        for bit in [first_unknown, first_unknown + 1, 63, 64, 300] {
            let bad = with_last_frame(&file, |payload| {
                let at = last_frame_image_at(payload);
                let mut mask = vec![0x80; bit / 7 + 1];
                mask[bit / 7] = 1 << (bit % 7);
                let nested = common::with_feature_mask_bytes(&payload[at..], Some(&mask));
                assert_eq!(
                    imij::header(&nested).err(),
                    Some(Error::Unsupported("unknown feature"))
                );
                payload.truncate(at);
                payload.extend_from_slice(&nested);
            });
            assert_constructor_error(&bad, Error::Unsupported("unknown feature"));
        }
        let bad = with_last_frame(&file, |payload| {
            let at = last_frame_image_at(payload);
            let end = common::header_end(&payload[at..]);
            payload.truncate(at + end - 1);
        });
        assert_constructor_error(&bad, Error::Corrupt("truncated"));

        assert_eq!(common::feature_mask(&file) & common::feature::LOSSY, 0);
        let bad = with_last_frame(&file, |payload| {
            let at = last_frame_image_at(payload);
            let mask = common::feature_mask(&payload[at..]) | common::feature::LOSSY;
            let nested = common::with_feature_mask(&payload[at..], Some(mask));
            payload.truncate(at);
            payload.extend_from_slice(&nested);
        });
        assert_constructor_error(
            &bad,
            Error::Corrupt(if kind == FrameKind::MixedPages {
                "page needs a feature the file does not declare"
            } else {
                "frame needs a feature the file does not declare"
            }),
        );
    }
}

#[test]
fn later_frame_controls_are_admitted_at_construction() {
    // SPEC 0.3/1.3: open container presence bits and critical frame records
    // must be checked before any pixel, including in the last frame/page.
    for kind in [
        FrameKind::Animation,
        FrameKind::Pages,
        FrameKind::MixedPages,
    ] {
        let file = admission_document(kind);
        for (controls, expected) in [
            (&[8][..], Error::Unsupported("unknown frame field")),
            (
                &[4, 2, 1, 0][..],
                Error::Unsupported("unknown frame record"),
            ),
            (
                &[4, 0xff, 0xff, 0x03][..],
                Error::Corrupt("frame records past fRAM"),
            ),
        ] {
            let bad = with_last_frame(&file, |payload| {
                let at = last_frame_image_at(payload);
                let nested = payload[at..].to_vec();
                payload.truncate(at - 1);
                payload.extend_from_slice(controls);
                payload.extend_from_slice(&nested);
            });
            assert_constructor_error(&bad, expected);
        }
    }
}

#[test]
fn nested_admission_reads_no_chunk_or_pixel_payloads() {
    for kind in [
        FrameKind::Animation,
        FrameKind::Pages,
        FrameKind::MixedPages,
    ] {
        let file = admission_document(kind);
        let header_only = with_last_frame(&file, |payload| {
            let at = last_frame_image_at(payload);
            let end = common::header_end(&payload[at..]);
            payload.truncate(at + end);
        });
        let mut decoder = FrameDecoder::new(&header_only).unwrap();
        for t in 0..2 {
            assert_eq!(decoder.next_frame().unwrap().unwrap().index, t as usize);
            let mut pixels = vec![0; 128];
            decoder.decode_into(&mut pixels, 16, 1).unwrap();
            assert_eq!(pixels, frame(16, 8, 1, t));
        }
        assert!(matches!(decoder.next_frame(), Err(Error::Corrupt(_))));
    }
}

#[test]
fn direct_selection_keeps_nested_admission_and_can_return_to_primary() {
    for kind in [
        FrameKind::Animation,
        FrameKind::Pages,
        FrameKind::MixedPages,
    ] {
        let file = admission_document(kind);
        let header_only = with_last_frame(&file, |payload| {
            let at = last_frame_image_at(payload);
            let end = common::header_end(&payload[at..]);
            payload.truncate(at + end);
        });
        let mut decoder = FrameDecoder::new(&header_only).unwrap();
        decoder.seek_frame(0).unwrap();
        assert!(decoder.frame_seek_info(2).unwrap().encoded.is_some());
        assert!(matches!(decoder.seek_frame(2), Err(Error::Corrupt(_))));
        assert!(decoder.frame_info().is_none());
        assert_eq!(
            decoder.decode_into(&mut [0; 128], 16, 1),
            Err(Error::InvalidInput("no current frame"))
        );
        decoder.seek_frame(0).unwrap();
        let mut pixels = vec![0; 128];
        decoder.decode_into(&mut pixels, 16, 1).unwrap();
        assert_eq!(pixels, frame(16, 8, 1, 0));
        decoder.seek_frame(1).unwrap();
        decoder.decode_into(&mut pixels, 16, 1).unwrap();
        assert_eq!(pixels, frame(16, 8, 1, 1));
    }
}
