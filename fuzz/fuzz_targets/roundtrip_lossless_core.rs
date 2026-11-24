// `roundtrip_lossless` target: input-built small images (every
// sample type, channel layout and sizes 1..=96, near-lossless and exact
// settings the config exposes) encode then decode.
//
// Oracle: decoded pixels must equal the input, or differ by at most the
// declared near-lossless bound per sample. The stream and session row
// routes must equal the allocating decode. Preview layers, when requested,
// decode to the preview source exactly (both encodes are lossless there).

include!("common_core.rs");

use imij::decode_resources::DecodeSession;
use imij::{Channels, ImageInfo, Metadata, ImageRef, SampleType, StreamDecoder, decode_with_limits, previews};

const SAMPLES: [SampleType; 11] = [
    SampleType::U8,
    SampleType::I8,
    SampleType::U16,
    SampleType::I16,
    SampleType::F16,
    SampleType::U32,
    SampleType::I32,
    SampleType::F32,
    SampleType::U64,
    SampleType::I64,
    SampleType::F64,
];

const EFFORTS: [f32; 4] = [0.0, 2.5, 7.5, 10.0];

#[derive(Debug)]
struct Case<'a> {
    width: u8,
    height: u8,
    channels: u8,
    sample: u8,
    flags: u16,
    seed: u8,
    data: &'a [u8],
}

impl<'a> Case<'a> {
    fn decode(input: &'a [u8]) -> Option<Case<'a>> {
        let mut b = Bytes::new(input);
        Some(Case {
            width: b.u8()?,
            height: b.u8()?,
            channels: b.u8()?,
            sample: b.u8()?,
            flags: b.u16()?,
            seed: b.u8()?,
            data: b.rest(),
        })
    }
}

fn pixels(w: u32, h: u32, bpp: usize, data: &[u8], seed: u8) -> Vec<u8> {
    let n = w as usize * h as usize * bpp;
    let mut v: Vec<u8> = Vec::with_capacity(n);
    for i in 0..n {
        let d = if data.is_empty() {
            seed
        } else {
            data[i % data.len()]
        };
        // Mostly pseudo-random bytes with some copied neighbours, so the
        // encoder sees both full alphabets and runs.
        let b = if i >= 2 * bpp && i % 3 == 0 {
            v[i - 2 * bpp].wrapping_add(1)
        } else {
            d.wrapping_mul(31)
                .wrapping_add((i as u8).wrapping_add(seed))
        };
        v.push(b);
    }
    v
}

/// Absolute per-sample difference of one integer channel, in sample units.
fn sample_diff(sample: SampleType, a: &[u8], b: &[u8]) -> u64 {
    fn u(v: &[u8], n: usize) -> u64 {
        let mut x = 0u64;
        for (i, &byte) in v.iter().take(n).enumerate() {
            x |= (byte as u64) << (8 * i);
        }
        x
    }
    fn s(v: &[u8], n: usize) -> i64 {
        let x = u(v, n);
        if n >= 8 {
            x as i64
        } else {
            let shift = 64 - 8 * n as u32;
            ((x << shift) as i64) >> shift
        }
    }
    match sample {
        SampleType::U8 | SampleType::U16 | SampleType::U32 | SampleType::U64 => {
            (u(a, sample.bytes()) as i128 - u(b, sample.bytes()) as i128).unsigned_abs() as u64
        }
        _ => (s(a, sample.bytes()) as i128 - s(b, sample.bytes()) as i128).unsigned_abs() as u64,
    }
}

fn check_bound(sample: SampleType, channels: usize, a: &[u8], b: &[u8], bound: u32) {
    let bytes = sample.bytes();
    for c in 0..channels {
        for i in 0..(a.len() / (bytes * channels)) {
            let at = (i * channels + c) * bytes;
            let d = sample_diff(sample, &a[at..at + bytes], &b[at..at + bytes]);
            assert!(
                d <= bound as u64,
                "near-lossless bound {bound} exceeded by {d} in channel {c}, sample {i}"
            );
        }
    }
}

fn row_route(data: &[u8], rb: usize, expected: &[u8]) {
    let limits = decode_limits();
    if let Ok(mut stream) = StreamDecoder::with_limits(data, &limits) {
        assert_eq!(stream.info().row_bytes(), rb, "stream row length differs");
        let mut rows = Vec::new();
        let mut row = vec![0u8; rb];
        while stream.next_row(&mut row).unwrap_or(false) {
            rows.extend_from_slice(&row);
        }
        assert_eq!(rows, expected, "stream rows differ from the source");
    }
    let mut session = DecodeSession::new(decode_policy());
    let mut rows = Vec::new();
    let outcome = session.decode_rows(data, &mut vec![0u8; rb], |_, _, r| {
        rows.extend_from_slice(r);
        Ok(())
    });
    if outcome.is_ok() {
        assert_eq!(rows, expected, "session rows differ from the source");
    }
}

pub fn run(input: &[u8]) {
    let Some(case) = Case::decode(input) else {
        return;
    };
    let sample = sample_for_build(
        SAMPLES[case.sample as usize % SAMPLES.len()],
        case.seed as usize,
        &[
            SampleType::U8,
            SampleType::I8,
            SampleType::U16,
            SampleType::I16,
            SampleType::F16,
        ],
    );
    let channels = [
        Channels::Gray,
        Channels::GrayAlpha,
        Channels::Rgb,
        Channels::Rgba,
    ][case.channels as usize % 4];
    let w = 1 + case.width as u32 % 96;
    let h = 1 + case.height as u32 % 96;
    let mut info = ImageInfo::new(w, h, channels, sample);
    let bpp = info.bytes_per_pixel();
    let rb = info.row_bytes();
    let px = pixels(w, h, bpp, case.data, case.seed);
    assert_eq!(px.len(), rb * h as usize);

    let int = !matches!(sample, SampleType::F16 | SampleType::F32 | SampleType::F64);
    // Near-lossless is defined for u8, u16, u32 and i32 samples only.
    let near_capable = matches!(
        sample,
        SampleType::U8 | SampleType::U16 | SampleType::U32 | SampleType::I32
    );
    let max_error = if near_capable && case.flags & 1 != 0 {
        1 + (case.flags >> 1) as u32 % 4
    } else {
        0
    };
    if max_error == 0 && int && case.flags & (1 << 12) != 0 {
        let depth = (sample.bytes() * 8) as u8;
        let bits: Vec<u8> = (0..channels.count())
            .map(|c| {
                1 + (((case.flags as u32 >> (13 + 2 * c)) as u8).wrapping_add(case.seed)) % depth
            })
            .collect();
        info = info
            .with_significant_bits(&bits)
            .expect("valid significant bits");
    }

    let cfg = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.with_effort(EFFORTS[((case.flags >> 5) & 3) as usize]);
        cfg = cfg.tune_lossless(|t| { t.palette = case.flags & (1 << 7) != 0; });
        cfg = cfg.with_checksum(case.flags & (1 << 8) != 0);
        cfg = cfg.tune_lossless(|t| { t.lanes = if case.flags & (1 << 9) != 0 {
            Some(1)
        } else {
            None
        }; });
        cfg = cfg.tune_lossless(|t| { t.stripe_rows = if case.flags & (1 << 10) != 0 {
            Some(1 + ((case.flags >> 11) as u32) % 16)
        } else {
            None
        }; });
        cfg = cfg.with_column_groups(if case.flags & (1 << 3) != 0 {
            Some(0)
        } else if case.flags & (1 << 4) != 0 {
            Some(16)
        } else {
            None
        });
        cfg
    };
    let cfg = if max_error > 0 {
        cfg.tune_lossless(|t| t.max_error = [max_error; 4])
    } else {
        cfg
    };

    let encoded = cfg.encode_samples(imij::ImageRef::new(&px, rb, &info), &Metadata::default())
        .expect("lossless encode of a generated small image");
    let limits = decode_limits();
    let (dinfo, dpx) =
        decode_with_limits(&encoded, &limits).expect("decode of an encoded small image");
    assert_eq!(
        (dinfo.width, dinfo.height, dinfo.channels, dinfo.sample),
        (w, h, channels, sample),
        "decoded header differs from the source"
    );
    if max_error == 0 {
        assert_eq!(dpx, px, "lossless roundtrip changed pixels");
    } else {
        check_bound(sample, channels.count(), &dpx, &px, max_error);
    }

    row_route(&encoded, rb, &dpx);

    // Preview layers: exact when the coding is exact.
    if max_error == 0 && w >= 2 && h >= 2 && case.flags & (1 << 6) != 0 {
        let (pw, ph) = (w / 2, h / 2);
        let p2 = ImageInfo::new(pw, ph, channels, sample);
        let mut ppx = Vec::with_capacity(pw as usize * ph as usize * bpp);
        for y in 0..ph as usize {
            for x in 0..pw as usize {
                let at = (y * 2 * w as usize + x * 2) * bpp;
                ppx.extend_from_slice(&px[at..at + bpp]);
            }
        }
        let preview = ImageRef {
            pixels: &ppx,
            stride: p2.row_bytes(),
            info: &p2,
        };
        let with_previews = cfg.encode_samples_with_previews(imij::ImageRef::new(&px, rb, &info), &Metadata::default(), core::slice::from_ref(&preview))
        .expect("encode with a preview layer");
        let (_, main_px) =
            decode_with_limits(&with_previews, &limits).expect("decode with a preview layer");
        assert_eq!(main_px, px, "previewed file changed the main image");
        let found = previews(&with_previews).expect("preview listing");
        assert_eq!(found.len(), 1);
        let (pinfo, decoded_preview) =
            decode_with_limits(found[0], &limits).expect("decode of the preview layer");
        assert_eq!((pinfo.width, pinfo.height), (pw, ph));
        assert_eq!(decoded_preview, ppx, "preview layer changed pixels");
    }
}
