//! `decode_reader`: rows and metadata identical to the in-memory decoder,
//! from readers that return a few bytes at a time; truncation, read
//! errors and limits.

use std::io::Read;

mod common;

use imij::{
    Channels, DecodeSpeed, Error, ImageInfo, Limits, LossyColor, Metadata, SampleType, decode,
    decode_reader, probe,
};

/// Returns 1..=7 bytes per `read` call.
struct Trickle<'a> {
    data: &'a [u8],
    n: usize,
}

impl Read for Trickle<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.n = self.n % 7 + 1;
        let k = self.n.min(buf.len()).min(self.data.len());
        buf[..k].copy_from_slice(&self.data[..k]);
        self.data = &self.data[k..];
        Ok(k)
    }
}

struct Failing;

impl Read for Failing {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("boom"))
    }
}

fn pixels(info: &ImageInfo, seed: u32) -> Vec<u8> {
    let mut s = seed | 1;
    let (w, h) = (info.width as usize, info.height as usize);
    let rb = info.row_bytes();
    let mut px = vec![0u8; rb * h];
    for y in 0..h {
        for i in 0..rb {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            // Smooth gradient plus some noise.
            let base = ((i * 3 + y * 5) % 256) as u32;
            px[y * rb + i] = (base + (s & 15)) as u8;
        }
    }
    if info.sample == SampleType::F16 {
        // Finite half floats in [0, 1).
        for c in px.as_chunks_mut::<2>().0 {
            let v = u16::from_ne_bytes([c[0], c[1]]) & 0x3BFF;
            c.copy_from_slice(&v.to_ne_bytes());
        }
    }
    let _ = w;
    px
}

fn check(file: &[u8], what: &str) {
    let (info, want) = decode(file).unwrap();
    let (_, want_meta) = probe(file).unwrap();
    for trickle in [false, true] {
        let mut got = Vec::new();
        let mut next = 0u32;
        let cb = |i: &ImageInfo, y: u32, row: &[u8]| {
            assert_eq!(i, &info);
            assert_eq!(y, next, "{what}: row order");
            next += 1;
            got.extend_from_slice(row);
            Ok(())
        };
        let (ri, meta) = if trickle {
            decode_reader(Trickle { data: file, n: 0 }, &Limits::DEFAULT, cb).unwrap()
        } else {
            decode_reader(file, &Limits::DEFAULT, cb).unwrap()
        };
        assert_eq!(ri, info, "{what}");
        assert_eq!(meta, want_meta, "{what}");
        assert_eq!(got.len(), want.len(), "{what}");
        assert!(got == want, "{what}: pixels differ (trickle {trickle})");
    }
}

#[test]
fn reader_matches_in_memory_decode() {
    let meta = Metadata {
        exif: Some(b"exif".to_vec()),
        orientation: 3,
        ..Default::default()
    };
    for (w, h) in [(1u32, 1u32), (37, 21), (130, 75)] {
        for (ch, st) in [
            (Channels::Rgb, SampleType::U8),
            (Channels::Rgba, SampleType::U8),
            (Channels::Gray, SampleType::U8),
            (Channels::GrayAlpha, SampleType::U16),
            (Channels::Rgb, SampleType::U16),
            (Channels::Rgb, SampleType::F16),
        ] {
            let info = ImageInfo::new(w, h, ch, st);
            let px = pixels(&info, w * 31 + h);
            let ll = imij::ImiEncoderConfig::default()
                .encode_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &meta)
                .unwrap();
            check(&ll, &format!("lossless {w}x{h} {ch:?} {st:?}"));
            for (q, cfg) in [
                (60.0, imij::ImiEncoderConfig::lossy(60.0)),
                (
                    85.0,
                    imij::ImiEncoderConfig::lossy(85.0).with_decode_speed(DecodeSpeed::Fast),
                ),
                (
                    75.0,
                    imij::ImiEncoderConfig::lossy(75.0)
                        .tune_lossy(|t| t.color = LossyColor::YCoCg)
                        .tune_lossy(|t| t.stripe_block_rows = 3),
                ),
            ] {
                let Ok(ly) =
                    cfg.encode_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &meta)
                else {
                    continue;
                };
                check(&ly, &format!("lossy q{q} {w}x{h} {ch:?} {st:?}"));
            }
        }
    }
}

#[test]
fn reader_errors() {
    let info = ImageInfo::new(64, 40, Channels::Rgb, SampleType::U8);
    let px = pixels(&info, 7);
    let f = imij::ImiEncoderConfig::lossy(70.0)
        .encode_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let ok = |_: &ImageInfo, _: u32, _: &[u8]| Ok(());
    for cut in [0, 5, 20, f.len() / 2, f.len() - 1] {
        let r = decode_reader(&f[..cut], &Limits::DEFAULT, ok);
        assert!(r.is_err(), "cut at {cut}");
    }
    assert!(matches!(
        decode_reader(Failing, &Limits::DEFAULT, ok),
        Err(Error::Read)
    ));
    let tight = {
        let mut limits = Limits::DEFAULT;
        limits.max_pixels = 100;
        limits
    };
    assert!(matches!(
        decode_reader(&f[..], &tight, ok),
        Err(Error::TooLarge)
    ));
    // A callback error stops decoding and is returned.
    let r = decode_reader(&f[..], &Limits::DEFAULT, |_, y, _| {
        if y == 3 {
            Err(Error::InvalidInput("stop"))
        } else {
            Ok(())
        }
    });
    assert_eq!(r.unwrap_err(), Error::InvalidInput("stop"));
}

/// `Limits::max_bytes` bounds the whole decoded buffer, the reader's
/// included (not one row): an image one byte over is `TooLarge` on every
/// route that takes limits (the matrix's `lm/bytes-over` and
/// `lm/lossy-bytes-over`), and no row reaches the reader's callback.
#[test]
fn max_bytes_refuses_the_reader_before_any_row() {
    for lossy in [false, true] {
        let info = ImageInfo::new(40, 24, Channels::Rgb, SampleType::U8);
        let px: Vec<u8> = (0..info.row_bytes() * 24)
            .map(|i| (i % 251) as u8)
            .collect();
        let file = if lossy {
            imij::ImiEncoderConfig::lossy(80.0).encode_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
        } else {
            imij::ImiEncoderConfig::default().encode_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
        }
        .unwrap();
        let total = info.checked_buffer_len().unwrap() as u64;
        // No row reaches the reader's callback.
        let mut rows = 0;
        let over = {
            let mut limits = Limits::DEFAULT;
            limits.max_bytes = total - 1;
            limits
        };
        let r = decode_reader(file.as_slice(), &over, |_, _, _| {
            rows += 1;
            Ok(())
        });
        assert_eq!((r.err(), rows), (Some(Error::TooLarge), 0));
    }
}
