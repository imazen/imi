// `roundtrip_lossy` target: input-built small images through the
// lossy encoder.
//
// Oracle: encoding must not panic, the decoded size and layout must match
// the source, alpha must stay exact, and encoding the same input twice with
// the same configuration must give identical bytes. No quality bound is
// asserted here (lossy pixels may differ); the stable gates are identity of
// the two encodes, layout, and bit-identical alpha.

include!("common_core.rs");

use imij::{Channels, ImageInfo, LossyChannelLayout, LossyColor, LossyEntropy, LossyTransform, ChannelSampling, Metadata, SampleType, StreamDecoder, decode_with_limits};

const SAMPLES: [SampleType; 5] = [
    SampleType::U8,
    SampleType::U16,
    SampleType::U32,
    SampleType::F16,
    SampleType::F32,
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
        let b = if i >= 2 * bpp && i % 4 == 0 {
            v[i - 2 * bpp].wrapping_add(3)
        } else {
            d.wrapping_mul(37).wrapping_add((i as u8).wrapping_add(seed))
        };
        v.push(b);
    }
    v
}

/// The layout set for one input, from two flag bits.
fn layouts(sel: usize) -> [LossyChannelLayout; 3] {
    use ChannelSampling::{Full, HalfBoth, HalfHorizontal, HalfVertical};
    use LossyTransform::{Adaptive4, Dct2, Dct4, Dct8};
    match sel % 4 {
        0 => [LossyChannelLayout::default(); 3],
        1 => [
            LossyChannelLayout::new(Dct8, Full),
            LossyChannelLayout::new(Dct4, Full),
            LossyChannelLayout::new(Dct2, Full),
        ],
        2 => [
            LossyChannelLayout::new(Dct8, Full),
            LossyChannelLayout::new(Dct8, HalfHorizontal),
            LossyChannelLayout::new(Dct8, HalfVertical),
        ],
        _ => [
            LossyChannelLayout::new(Adaptive4, Full),
            LossyChannelLayout::new(Adaptive4, HalfBoth),
            LossyChannelLayout::new(Dct4, HalfBoth),
        ],
    }
}

pub fn run(input: &[u8]) {
    let Some(case) = Case::decode(input) else {
        return;
    };
    let sample = sample_for_build(
        SAMPLES[case.sample as usize % SAMPLES.len()],
        case.seed as usize,
        &[SampleType::U8, SampleType::U16, SampleType::F16],
    );
    let channels = [
        Channels::Gray,
        Channels::GrayAlpha,
        Channels::Rgb,
        Channels::Rgba,
    ][case.channels as usize % 4];
    let w = 1 + case.width as u32 % 64;
    let h = 1 + case.height as u32 % 64;
    let info = ImageInfo::new(w, h, channels, sample);
    let bpp = info.bytes_per_pixel();
    let rb = info.row_bytes();
    let px = pixels(w, h, bpp, case.data, case.seed);

    let quality = 1.0 + (case.flags & 0x7f) as f32 % 116.0;
    let mut cfg = imij::ImiEncoderConfig::lossy(quality);
    cfg = cfg.with_effort(EFFORTS[((case.flags >> 7) & 3) as usize]);
    cfg = cfg.tune_lossy(|t| { t.color = if case.flags & (1 << 9) != 0 {
        LossyColor::YCoCg
    } else {
        LossyColor::Xyb
    }; });
    // Packed and Split are retired (SPEC 2.8: both are reserved values),
    // so only the two surviving modes are exercised.
    cfg = cfg.tune_lossy(|t| { t.entropy = if case.flags & (1 << 10) != 0 {
        LossyEntropy::SplitHuf
    } else {
        LossyEntropy::RunLevel
    }; });
    cfg = cfg.tune_lossy(|t| { t.channel_layouts = layouts((case.flags >> 12) as usize & 3); });
    cfg = cfg.tune_lossy(|t| { t.compact_streams = case.flags & (1 << 2) != 0; });
    cfg = cfg.with_checksum(case.flags & (1 << 3) != 0);
    cfg = cfg.tune_lossy(|t| { t.stripe_block_rows = if case.flags & (1 << 4) != 0 {
        1 + (((case.flags >> 5) as u32) % 4)
    } else {
        0
    }; });
    cfg = cfg.with_column_groups(if case.flags & (1 << 14) != 0 {
        Some(0)
    } else if case.flags & (1 << 15) != 0 {
        Some(16)
    } else {
        None
    });
    if case.flags & 1 != 0 {
        // A decode-speed tier picks the entropy mode itself (entropy ignored).
        cfg = cfg.with_decode_speed(
            [
                imij::DecodeSpeed::Compact,
                imij::DecodeSpeed::Balanced,
                imij::DecodeSpeed::Fast,
                imij::DecodeSpeed::Fastest,
            ][((case.flags >> 13) & 3) as usize],
        );
    }
    cfg = cfg.with_threads(1);
    let encoded =
        cfg.encode_samples(imij::ImageRef::new(&px, rb, &info), &Metadata::default()).expect("lossy encode of a generated image");
    // Determinism: the same input and configuration must encode to the same
    // bytes.
    let again = cfg.encode_samples(imij::ImageRef::new(&px, rb, &info), &Metadata::default())
        .expect("second lossy encode of the same input");
    assert_eq!(encoded, again, "lossy encode is not deterministic");

    let limits = decode_limits();
    let (dinfo, dpx) =
        decode_with_limits(&encoded, &limits).expect("decode of an encoded lossy image");
    assert_eq!(
        (dinfo.width, dinfo.height, dinfo.channels, dinfo.sample),
        (w, h, channels, sample),
        "decoded lossy header differs from the source"
    );
    assert_eq!(dpx.len(), px.len(), "decoded lossy buffer length differs");

    // Alpha is stored losslessly: its samples must be bit-identical.
    if channels.has_alpha() {
        let bytes = sample.bytes();
        let n = w as usize * h as usize;
        for i in 0..n {
            let at = (i * channels.count() + channels.count() - 1) * bytes;
            assert_eq!(
                &dpx[at..at + bytes],
                &px[at..at + bytes],
                "lossy alpha changed at sample {i}"
            );
        }
    }

    // The stream route must reproduce the allocating decode.
    if let Ok(mut stream) = StreamDecoder::with_limits(&encoded, &limits) {
        let mut rows = Vec::new();
        let mut row = vec![0u8; rb];
        while stream.next_row(&mut row).unwrap_or(false) {
            rows.extend_from_slice(&row);
        }
        assert_eq!(rows, dpx, "lossy stream rows differ from the full decode");
    }
}
