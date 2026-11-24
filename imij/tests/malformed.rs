//! Small hostile fixtures beyond the outcome matrix (tests/matrix.rs, which
//! holds the header, varint, length and DATA cases every route must agree
//! on): reader positions, caller extents, frame fields, palette predictors.
// Literal V2/V3 wire mutation controls select the retained grammar explicitly.
#![cfg(feature = "std")]
mod common;
use common::{join, parts, put_varint};
use imij::*;

fn base() -> Vec<u8> {
    let info = ImageInfo::new(1, 2, Channels::Gray, SampleType::U8);
    imij::ImiEncoderConfig::default()
        .encode_legacy_samples(imij::ImageRef::new(&[0, 1], 1, &info), &Metadata::default())
        .unwrap()
}

#[test]
fn invalid_caller_extents_return_errors() {
    for lossy in [false, true] {
        let info = ImageInfo::new(1, 2, Channels::Gray, SampleType::U8);
        let file = if lossy {
            imij::ImiEncoderConfig::lossy(90.0)
                .encode_legacy_samples(imij::ImageRef::new(&[0, 1], 1, &info), &Metadata::default())
                .unwrap()
        } else {
            base()
        };
        let mut out = [0xa5; 2];
        assert!(decode_into(&file, &mut out, usize::MAX).is_err());
        assert!(decode_into_threads(&file, &mut out, usize::MAX, 4).is_err());
        assert_eq!(out, [0xa5; 2]);
    }
    let info = ImageInfo::new(1, 0, Channels::Gray, SampleType::U8);
    let mut enc = FrameEncoder::animation(
        info,
        Metadata::default(),
        FrameConfig::new(imij::ImiEncoderConfig::default()),
    );
    assert!(enc.push_frame(&[], 1, FrameDuration::ZERO).is_err());
}

#[test]
fn invalid_lossy_parameters_never_produce_an_undecodable_file() {
    let info = ImageInfo::new(8, 8, Channels::Rgb, SampleType::U8);
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(
            imij::ImiEncoderConfig::lossy(value)
                .encode_legacy_samples(
                    imij::ImageRef::new(&[128; 192], 24, &info),
                    &Metadata::default()
                )
                .is_err()
        );
    }
    for value in [-1.0, 0.0, f32::NAN, f32::INFINITY, 65536.0] {
        let mut cfg = imij::ImiEncoderConfig::lossy(90.0);
        cfg = cfg.tune_lossy(|t| {
            t.channel_mul = Some([1.0, value, 1.0]);
        });
        assert!(
            cfg.encode_legacy_samples(
                imij::ImageRef::new(&[128; 192], 24, &info),
                &Metadata::default()
            )
            .is_err()
        );
    }
}

/// The reader stops at a bounded `DATA` chunk's end: a record past it is
/// never read. (Trailing bytes in `DATA`, per lossy entropy mode, are
/// matrix cases.)
#[test]
fn reader_cannot_cross_the_data_boundary() {
    let (h, chunks) = parts(&base());
    let mut file = join(&h, &Vec::new(), false);
    file.extend(b"DATA");
    file.push(1); // Bounded DATA with zero payload bytes.
    let end = file.len();
    file.extend(&chunks[0].1);
    let mut reader = std::io::Cursor::new(file);
    let mut rows = 0;
    assert!(
        decode_reader(&mut reader, &Limits::DEFAULT, |_, _, _| {
            rows += 1;
            Ok(())
        })
        .is_err()
    );
    assert_eq!(reader.position(), end as u64);
    assert_eq!(rows, 0);
}

#[test]
fn frame_fields_are_checked_before_narrowing_or_offset_math() {
    let (h, c) = parts(&base());
    for (x, den) in [
        (u64::MAX, 1),
        (u32::MAX as u64, 1),
        (0, u32::MAX as u64 + 2),
    ] {
        let mut chunks = c.clone();
        chunks.insert(0, (*b"aNIM", vec![1, 0, 2, 1, 1]));
        let mut frame = Vec::new();
        for value in [x, 0, 1, den] {
            put_varint(&mut frame, value);
        }
        frame.push(1);
        frame.extend(base());
        chunks.push((*b"fRAM", frame));
        let file = join(&h, &chunks, false);
        if x > u32::MAX as u64 || den > u32::MAX as u64 {
            // Control parsing runs at construction, before either image.
            assert_eq!(
                FrameDecoder::new(&file).map(|_| ()),
                Err(Error::Corrupt("integer exceeds u32"))
            );
        } else {
            // The origin fits u32, but its rectangle does not fit the canvas.
            // Geometry semantics are still checked when the frame is reached.
            let mut d = FrameDecoder::new(&file).unwrap();
            d.next_frame().unwrap();
            assert_eq!(d.next_frame(), Err(Error::Corrupt("frame rectangle")));
            assert!(d.decode_into(&mut [0; 2], 1, 1).is_err());
        }
    }
    for den in [0, u32::MAX as u64 + 1] {
        let mut chunks = c.clone();
        let mut anim = vec![1, 0, 1, 1];
        put_varint(&mut anim, den);
        chunks.insert(0, (*b"aNIM", anim));
        assert!(FrameDecoder::new(&join(&h, &chunks, false)).is_err());
    }
    let mut chunks = c;
    let mut anim = vec![1, 0, 1];
    put_varint(&mut anim, u64::MAX);
    put_varint(&mut anim, u32::MAX as u64);
    chunks.insert(0, (*b"aNIM", anim));
    let file = join(&h, &chunks, false);
    assert_eq!(
        FrameDecoder::new(&file)
            .unwrap()
            .next_frame()
            .unwrap()
            .unwrap()
            .duration,
        FrameDuration {
            num: u64::MAX,
            den: u32::MAX
        }
    );
}

#[test]
fn palette_reference_predictors_are_rejected_before_dispatch() {
    let info = ImageInfo::new(37, 50, Channels::Rgb, SampleType::U8);
    let px: Vec<u8> = (0..37 * 50)
        .flat_map(|i| {
            if (i / 5 + i / 37) % 2 == 0 {
                [71, 192, 35]
            } else {
                [183, 45, 211]
            }
        })
        .collect();
    let file = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(imij::ImageRef::new(&px, 111, &info), &Metadata::default())
        .unwrap();
    let (h, chunks) = parts(&file);
    for predictor in 7..=9 {
        let mut nested = chunks.clone();
        let body = &mut nested[0].1;
        let p = common::payload(body, &info, false);
        assert_eq!(p.present & 1, 1); // Palette model.
        let at = p.records()[0].start;
        assert_eq!(body[at], 128); // Palette stripe.
        let n = body[at + 1] as usize + 1;
        let pred = at + 2 + n * 3;
        body[pred] = (body[pred] & 15) | predictor << 4;
        let mut frame = vec![0, 0, 1, 1, 1];
        frame.extend(join(&h, &nested, false));
        let mut outer = chunks.clone();
        outer.insert(0, (*b"aNIM", vec![1, 0, 2, 1, 1]));
        outer.push((*b"fRAM", frame));
        let bad = join(&h, &outer, false);
        let mut d = FrameDecoder::new(&bad).unwrap();
        d.next_frame().unwrap();
        d.next_frame().unwrap();
        assert!(d.decode_into(&mut px.clone(), 111, 1).is_err());
    }
}

#[test]
fn nested_frame_color_is_inherited_from_the_canvas() {
    let info = ImageInfo::new(8, 8, Channels::Rgb, SampleType::U16);
    let pixels: Vec<u8> = (0..192u16)
        .flat_map(|n| (10000 + n * 101).to_ne_bytes())
        .collect();
    let original = imij::ImiEncoderConfig::lossy(90.0)
        .encode_legacy_samples(
            imij::ImageRef::new(&pixels, 48, &info),
            &Metadata::default(),
        )
        .unwrap();
    let expected = decode(&original).unwrap().1;
    let (header, nested_chunks) = parts(&original);
    // Masked header (SPEC 1.0): version, present, the fields before the
    // dimensions, then the rest. Give the nested header an explicit PQ CICP
    // (present bit 6: after coding, sample and the feature mask, before the
    // width here, as no significant bits or alpha mode follow); the outer
    // canvas stays sRGB.
    let layout = common::header_layout(&header);
    assert_eq!(layout.present & !0x3f, 0, "no CICP, bits or alpha mode yet");
    let mut nested_header = vec![3, (layout.present | 0x40) as u8];
    nested_header.extend(&header[2..layout.dims]);
    nested_header.extend([1, 16, 0, 1]);
    nested_header.extend(&header[layout.dims..]);
    let mut frame = vec![0, 0, 1, 1, 0];
    frame.extend(join(&nested_header, &nested_chunks, false));
    let (outer_header, mut chunks) = parts(&original);
    chunks.insert(0, (*b"aNIM", vec![1, 0, 2, 1, 1]));
    chunks.push((*b"fRAM", frame));
    let file = join(&outer_header, &chunks, false);
    let mut decoder = FrameDecoder::new(&file).unwrap();
    decoder.next_frame().unwrap();
    decoder.next_frame().unwrap();
    let mut out = vec![0; expected.len()];
    decoder.decode_into(&mut out, 48, 1).unwrap();
    assert_eq!(out, expected);
}
