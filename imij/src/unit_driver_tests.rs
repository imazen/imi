//! The lossless unit driver ([`decode_unit_rows`]) through each of its
//! sinks: strided into an image, row slices, and one row buffer with a call
//! per row. Every unit of a file, through every sink, must give the
//! whole-image decode's bytes.

use super::*;
use crate::{Channels, EncodeConfig, Metadata, SampleType};

fn image(info: &ImageInfo) -> Vec<u8> {
    let n = info.row_bytes() * info.height as usize;
    let mut s = 0x1234_5678u32;
    (0..n)
        .map(|i| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            // Smooth with some noise, so models and predictors vary.
            ((i / 7) as u32 % 200 + s % 9) as u8
        })
        .collect()
}

#[test]
fn unit_sinks_agree_with_the_whole_image_decode() {
    let mut saw_groups = false;
    let mut saw_stripes = false;
    for (w, h, ch, sample, rows) in [
        (37, 29, Channels::Rgb, SampleType::U8, Some(8)),
        (64, 40, Channels::Rgba, SampleType::U16, Some(16)),
        (9000, 3, Channels::Gray, SampleType::U8, None),
        (17, 70, Channels::GrayAlpha, SampleType::U8, Some(8)),
    ] {
        let info = ImageInfo::new(w, h, ch, sample);
        let px = image(&info);
        let rb = info.row_bytes();
        let cfg = EncodeConfig {
            stripe_rows: rows,
            ..EncodeConfig::default()
        };
        let file = crate::encode(&px, rb, &info, &Metadata::default(), &cfg).unwrap();
        let whole = crate::decode(&file).unwrap().1;
        assert_eq!(whole, px);
        let f = crate::container::parse(&file, false, &payload_len).unwrap();
        let (p, _) = parse_body_checked(
            f.header.info.clone(),
            f.header.version,
            f.declared,
            f.data,
            &crate::parse::limits::Unrestricted,
        )
        .unwrap();
        let om = OutMap::new(&p.info, &p.layout);
        let (ng, bpp) = (p.ngroups(), p.bpp());
        saw_groups |= ng > 1;
        saw_stripes |= p.stripe_count() > 1;
        let n = rb * h as usize;
        let (mut strided, mut sliced, mut each) = (vec![0u8; n], vec![0u8; n], vec![0u8; n]);
        for u in 0..p.stripe_count() * ng {
            let (s, g) = (u / ng, u % ng);
            let (x0, gw, _) = p.group(g);
            let y0 = s * p.hs;
            let rows = p.hs.min(h as usize - y0);
            let at = y0 * rb + x0 * bpp;
            decode_unit_into_checked(&p, u, &om, &mut strided[at..], rb, &|| Ok(())).unwrap();
            let mut slices: Vec<&mut [u8]> = sliced[y0 * rb..]
                .chunks_mut(rb)
                .take(rows)
                .map(|r| &mut r[x0 * bpp..][..gw * bpp])
                .collect();
            decode_unit(&p, u, &om, &mut slices).unwrap();
            let mut row = vec![0u8; rb];
            let mut i = 0;
            unit_rows(
                &p,
                &om,
                u,
                p.stripes[u],
                &mut row,
                |r| {
                    assert_eq!(r.len(), gw * bpp);
                    each[(y0 + i) * rb + x0 * bpp..][..r.len()].copy_from_slice(r);
                    i += 1;
                    Ok(())
                },
                &|| Ok(()),
            )
            .unwrap();
            assert_eq!(i, rows, "unit {u}: rows delivered");
        }
        // Crops of every unit (rows first..last, columns from `col`), as
        // region decoding takes them, against the same rectangle of the image.
        for u in 0..p.stripe_count() * ng {
            let (s, g) = (u / ng, u % ng);
            let (x0, gw, _) = p.group(g);
            let y0 = s * p.hs;
            let rows = p.hs.min(h as usize - y0);
            for (first, last, c0, c1) in [
                (0, rows, 0, gw),
                (rows / 3, rows - rows / 4, gw / 5, gw - gw / 3),
                (rows - 1, rows, gw - 1, gw),
            ] {
                if first >= last || c0 >= c1 {
                    continue;
                }
                let mut out = vec![0u8; (last - first) * (c1 - c0) * bpp];
                let mut dst: Vec<&mut [u8]> = out.chunks_mut((c1 - c0) * bpp).collect();
                let mut sink = Crop {
                    buf: vec![0u8; gw * bpp],
                    first,
                    last,
                    col: c0 * bpp,
                    rows: &mut dst,
                };
                decode_unit_rows(&p, &om, u, p.stripes[u], &mut sink, &|| Ok(())).unwrap();
                for (i, r) in out.chunks((c1 - c0) * bpp).enumerate() {
                    let y = y0 + first + i;
                    assert!(
                        r == &whole[y * rb + (x0 + c0) * bpp..][..(c1 - c0) * bpp],
                        "{w}x{h} unit {u} crop {first}..{last} x {c0}..{c1}"
                    );
                }
            }
        }
        assert!(strided == whole, "{w}x{h} {ch:?} strided");
        assert!(sliced == whole, "{w}x{h} {ch:?} slices");
        assert!(each == whole, "{w}x{h} {ch:?} one row");
    }
    assert!(saw_stripes);
    assert!(saw_groups, "no file with column groups");
}

/// A sink that stops early ([`UnitSink::rows`]) and one that fails after a
/// row both end the unit there, without touching later rows.
#[test]
fn unit_sinks_stop_where_they_say() {
    struct Count<'a> {
        row: &'a mut [u8],
        upto: usize,
        fail_at: Option<usize>,
        done: usize,
    }
    impl UnitSink for Count<'_> {
        fn rows(&self, rows: usize) -> usize {
            rows.min(self.upto)
        }
        fn row(&mut self, _: usize) -> &mut [u8] {
            self.row
        }
        fn done(&mut self, i: usize) -> Result<(), Error> {
            self.done += 1;
            if Some(i) == self.fail_at {
                return Err(Error::InvalidInput("stop"));
            }
            Ok(())
        }
    }
    let info = ImageInfo::new(20, 16, Channels::Rgb, SampleType::U8);
    let px = image(&info);
    let cfg = EncodeConfig {
        stripe_rows: Some(16),
        ..EncodeConfig::default()
    };
    let file = crate::encode(&px, info.row_bytes(), &info, &Metadata::default(), &cfg).unwrap();
    let f = crate::container::parse(&file, false, &payload_len).unwrap();
    let (p, _) = parse_body_checked(
        f.header.info.clone(),
        f.header.version,
        f.declared,
        f.data,
        &crate::parse::limits::Unrestricted,
    )
    .unwrap();
    let om = OutMap::new(&p.info, &p.layout);
    let mut row = vec![0u8; info.row_bytes()];
    for (upto, fail_at, want_done, ok) in [
        (16, None, 16, true),
        (5, None, 5, true),
        (16, Some(3), 4, false),
    ] {
        let mut sink = Count {
            row: &mut row,
            upto,
            fail_at,
            done: 0,
        };
        let r = decode_unit_rows(&p, &om, 0, p.stripes[0], &mut sink, &|| Ok(()));
        assert_eq!(r.is_ok(), ok);
        assert_eq!(sink.done, want_done);
    }
}
