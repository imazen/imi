//! Streaming encode output through zencodec's `Encoder::begin` /
//! `pending_output`: row groups stream their header at `begin` and each row's
//! groups as the row arrives, byte-identical to the one-shot group encode;
//! the whole-image encoders refuse `begin` with a typed `RowLevelEncode`.
use imij::ImiEncoderConfig;
use imij::codec::DecodeConfig;
use zencodec::CodecErrorExt;
use zencodec::encode::{EncodeJob, Encoder, EncoderConfig, OutputTiming};
use zencodec::{ResourceLimits, UnsupportedOperation};
use zenpixels::{PixelDescriptor, PixelSlice};

const WORKING: usize = 16 << 20;

fn pixels(w: u32, h: u32, bpp: usize) -> Vec<u8> {
    let mut x = 0x2545_f491u32;
    (0..w as usize * h as usize * bpp)
        .map(|i| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            ((i % 97) as u32 + (x & 31)) as u8
        })
        .collect()
}

/// `begin`, push `strip` rows at a time, drain after each. Returns the
/// timing, the file, and the bytes available after `begin` and each strip.
fn streamed(
    cfg: &ImiEncoderConfig,
    px: &[u8],
    w: u32,
    h: u32,
    desc: PixelDescriptor,
    strip: u32,
) -> (OutputTiming, Vec<u8>, Vec<usize>) {
    let stride = w as usize * desc.bytes_per_pixel();
    let mut enc = cfg.clone().job().encoder().unwrap();
    let timing = enc.begin(w, h, desc).unwrap();
    let mut out = Vec::new();
    let mut seen = Vec::new();
    let mut y = 0;
    loop {
        seen.push(out.len() + enc.pending_output().len());
        let n = enc.pending_output().len();
        out.extend_from_slice(enc.pending_output());
        enc.consume_output(n);
        if y == h {
            break;
        }
        let rows = strip.min(h - y);
        let s = PixelSlice::new(
            &px[y as usize * stride..(y + rows) as usize * stride],
            w,
            rows,
            stride,
            desc,
        )
        .unwrap();
        enc.push_rows(s).unwrap();
        y += rows;
    }
    out.extend_from_slice(enc.finish().unwrap().data());
    (timing, out, seen)
}

#[test]
fn row_groups_stream_byte_identical_to_one_shot() {
    for (desc, bpp) in [
        (PixelDescriptor::RGB8_SRGB, 3usize),
        (PixelDescriptor::RGBA8_SRGB, 4),
        (PixelDescriptor::GRAY8_SRGB, 1),
        (PixelDescriptor::RGB16_SRGB, 6),
    ] {
        for cfg in [
            imij::ImiEncoderConfig::default()
                .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING))),
            imij::ImiEncoderConfig::default()
                .with_row_groups(Some(imij::RowGroups::new(64, 1, WORKING))),
            imij::ImiEncoderConfig::quick()
                .with_row_groups(Some(imij::RowGroups::new(16, 1, WORKING))),
        ] {
            for (w, h) in [(1u32, 1u32), (19, 7), (64, 33)] {
                let px = pixels(w, h, bpp);
                let stride = w as usize * bpp;
                let one = cfg
                    .clone()
                    .job()
                    .encoder()
                    .unwrap()
                    .encode(PixelSlice::new(&px, w, h, stride, desc).unwrap())
                    .unwrap()
                    .into_vec();
                for strip in [1u32, 3, h] {
                    let (timing, got, seen) = streamed(&cfg, &px, w, h, desc, strip);
                    assert_eq!(timing, OutputTiming::streaming(0));
                    assert_eq!(got, one, "{desc:?} {w}x{h} strip {strip}");
                    // Header at begin; every pushed strip adds bytes.
                    assert!(seen[0] > 0, "header pending at begin");
                    assert!(
                        seen.windows(2).all(|p| p[1] > p[0]),
                        "{desc:?} {w}x{h} strip {strip}: {seen:?}"
                    );
                    assert_eq!(imij::decode(&got).unwrap().1, px);
                }
            }
        }
    }
}

#[test]
fn without_begin_nothing_is_pending() {
    let (w, h) = (19u32, 7u32);
    let px = pixels(w, h, 3);
    let cfg = imij::ImiEncoderConfig::default()
        .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)));
    let mut enc = cfg.clone().job().with_canvas_size(w, h).encoder().unwrap();
    for y in 0..h {
        let s = PixelSlice::new(
            &px[y as usize * 57..][..57],
            w,
            1,
            57,
            PixelDescriptor::RGB8_SRGB,
        )
        .unwrap();
        enc.push_rows(s).unwrap();
        assert!(enc.pending_output().is_empty());
    }
    let (_, streamed, _) = streamed(&cfg, &px, w, h, PixelDescriptor::RGB8_SRGB, 2);
    assert_eq!(enc.finish().unwrap().into_vec(), streamed);
}

#[test]
fn whole_image_encoders_refuse_begin_with_a_typed_error() {
    for cfg in [
        imij::ImiEncoderConfig::default(),
        imij::ImiEncoderConfig::default().with_lossless(false),
    ] {
        let mut enc = cfg.job().encoder().unwrap();
        let e = enc.begin(8, 8, PixelDescriptor::RGB8_SRGB).unwrap_err();
        assert_eq!(
            e.unsupported_operation(),
            Some(&UnsupportedOperation::RowLevelEncode),
            "{e}"
        );
    }
}

#[test]
fn begin_checks_the_declaration() {
    let cfg = imij::ImiEncoderConfig::default()
        .with_row_groups(Some(imij::RowGroups::new(5, 1, WORKING)));
    // A canvas set on the job must match.
    let mut enc = cfg.clone().job().with_canvas_size(9, 9).encoder().unwrap();
    assert!(enc.begin(8, 8, PixelDescriptor::RGB8_SRGB).is_err());
    // Rows must arrive in the declared format, and not exceed the height.
    let px = pixels(8, 8, 4);
    let mut enc = cfg.clone().job().encoder().unwrap();
    enc.begin(8, 4, PixelDescriptor::RGB8_SRGB).unwrap();
    let rgba = PixelSlice::new(&px, 8, 1, 32, PixelDescriptor::RGBA8_SRGB).unwrap();
    assert!(enc.push_rows(rgba).is_err());
    let mut enc = cfg.clone().job().encoder().unwrap();
    enc.begin(8, 4, PixelDescriptor::RGBA8_SRGB).unwrap();
    let all = PixelSlice::new(&px, 8, 8, 32, PixelDescriptor::RGBA8_SRGB).unwrap();
    assert!(enc.push_rows(all).is_err());
    // begin twice.
    let mut enc = cfg.job().encoder().unwrap();
    enc.begin(8, 8, PixelDescriptor::RGB8_SRGB).unwrap();
    assert!(enc.begin(8, 8, PixelDescriptor::RGB8_SRGB).is_err());
}

#[test]
fn output_limit_counts_bytes_already_sent() {
    let (w, h) = (64u32, 33u32);
    let px = pixels(w, h, 3);
    let cfg = imij::ImiEncoderConfig::default()
        .with_row_groups(Some(imij::RowGroups::new(16, 1, WORKING)));
    let (_, file, _) = streamed(&cfg, &px, w, h, PixelDescriptor::RGB8_SRGB, 4);
    let run = |max: u64| -> Result<(), imij::Error> {
        let mut enc = cfg
            .clone()
            .job()
            .with_limits(ResourceLimits::none().with_max_output(max))
            .encoder()
            .unwrap();
        enc.begin(w, h, PixelDescriptor::RGB8_SRGB)?;
        for y in 0..h {
            let n = enc.pending_output().len();
            enc.consume_output(n);
            let s = PixelSlice::new(
                &px[y as usize * 192..][..192],
                w,
                1,
                192,
                PixelDescriptor::RGB8_SRGB,
            )
            .unwrap();
            enc.push_rows(s)?;
        }
        enc.finish().map(|_| ())
    };
    assert!(run(file.len() as u64).is_ok());
    assert!(run(file.len() as u64 - 1).is_err());
}

#[test]
fn testkit_streaming_output_honesty() {
    let img = zencodec_testkit::TestImage::rgba8_gradient(40, 24);
    zencodec_testkit::check_streaming_output_honesty(
        imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(7, 1, WORKING))),
        DecodeConfig::default(),
        &img,
    )
    .unwrap();
    // The whole-image encoder declines at `begin`, which the check accepts.
    zencodec_testkit::check_streaming_output_honesty(
        imij::ImiEncoderConfig::default(),
        DecodeConfig::default(),
        &img,
    )
    .unwrap();
    zencodec_testkit::check_streaming_output_honesty(
        imij::ImiEncoderConfig::default().with_prefer_streaming(true),
        DecodeConfig::default(),
        &img,
    )
    .unwrap();
}

#[test]
fn prefer_streaming_selects_row_groups_for_a_default_lossless_encode() {
    let (w, h) = (37, 29);
    let desc = PixelDescriptor::RGB8_SRGB;
    let px = pixels(w, h, 3);
    let preferred = imij::ImiEncoderConfig::default().with_prefer_streaming(true);
    assert_eq!(preferred.prefers_streaming(), Some(true));
    let (timing, file, _) = streamed(&preferred, &px, w, h, desc, 5);
    assert_eq!(timing, OutputTiming::streaming(0));
    // Still exact lossless: the same pixels as the whole-image encode.
    let decode = |bytes: &[u8]| {
        use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};
        DecodeConfig::default()
            .job()
            .decoder(std::borrow::Cow::Borrowed(bytes), &[desc])
            .unwrap()
            .decode()
            .unwrap()
            .pixels()
            .contiguous_bytes()
            .into_owned()
    };
    assert_eq!(decode(&file), px);
    // Builder order does not matter: the preference resolves at `job()`.
    let effort_after = imij::ImiEncoderConfig::default()
        .with_prefer_streaming(true)
        .with_generic_effort(5);
    let declined = [
        effort_after,
        imij::ImiEncoderConfig::default()
            .with_prefer_streaming(true)
            .with_lossless(false),
        imij::ImiEncoderConfig::default().with_prefer_streaming(false),
    ];
    for cfg in declined {
        let e = cfg.job().encoder().unwrap().begin(w, h, desc).unwrap_err();
        assert_eq!(
            e.unsupported_operation(),
            Some(&UnsupportedOperation::RowLevelEncode),
            "{e}"
        );
    }
}

#[test]
fn take_output_hands_over_the_buffer_and_keeps_the_bytes() {
    let (w, h) = (40, 24);
    let desc = PixelDescriptor::RGB8_SRGB;
    let px = pixels(w, h, 3);
    let cfg = imij::ImiEncoderConfig::default()
        .with_row_groups(Some(imij::RowGroups::new(7, 1, WORKING)));
    let (_, drained, _) = streamed(&cfg, &px, w, h, desc, 4);
    let stride = w as usize * 3;
    let mut enc = cfg.job().encoder().unwrap();
    enc.begin(w, h, desc).unwrap();
    let mut out = Vec::new();
    for y in (0..h).step_by(4) {
        let mut chunk = Vec::new();
        let ptr = enc.pending_output().as_ptr();
        let had = enc.pending_output().len();
        enc.take_output(&mut chunk);
        if had > 0 {
            assert_eq!(chunk.as_ptr(), ptr, "the buffer itself, not a copy");
        }
        assert!(enc.pending_output().is_empty());
        out.extend_from_slice(&chunk);
        let s = PixelSlice::new(
            &px[y as usize * stride..(y + 4) as usize * stride],
            w,
            4,
            stride,
            desc,
        )
        .unwrap();
        enc.push_rows(s).unwrap();
    }
    let mut chunk = vec![1u8];
    enc.take_output(&mut chunk);
    out.extend_from_slice(&chunk[1..]);
    out.extend_from_slice(enc.finish().unwrap().data());
    assert_eq!(out, drained);
}

#[test]
fn categorized_error_and_unit_ordinals_on_codec_config() {
    use zencodec::{
        CategorizedError as _, ErrorCategory, ImageError, InvalidKind, RequestError, ResourceError,
        UnsupportedImageKind,
        decode::{Decode as _, DecodeJob as _, DecoderConfig as _},
        enough::StopReason,
    };
    let (w, h) = (48, 32);
    let desc = PixelDescriptor::RGB8_SRGB;
    let px = pixels(w, h, 3);
    let slice = PixelSlice::new(&px, w, h, w as usize * 3, desc).unwrap();
    let encoded = imij::ImiEncoderConfig::default()
        .with_unit_ordinals(true)
        .job()
        .encoder()
        .unwrap()
        .encode(slice)
        .unwrap();
    let header = imij::header(encoded.data()).unwrap();
    assert_eq!(header.version, 4);
    // Group identities are mandatory V4 syntax, so the legacy optional
    // capability is no longer serialized even when the caller requests it.
    assert!(!header.features.contains(imij::Feature::UnitOrdinals));
    let decoded = DecodeConfig::default()
        .job()
        .decoder(std::borrow::Cow::Borrowed(encoded.data()), &[desc])
        .unwrap()
        .decode()
        .unwrap();
    assert_eq!(decoded.pixels().contiguous_bytes().as_ref(), &px[..]);

    assert_eq!(imij::Error::NotImij.codec_name(), Some("imij"));
    assert_eq!(
        imij::Error::NotImij.category(),
        ErrorCategory::Image(ImageError::Unsupported(UnsupportedImageKind::Type))
    );
    assert_eq!(
        imij::Error::Corrupt("truncated header").category(),
        ErrorCategory::Image(ImageError::UnexpectedEof)
    );
    assert_eq!(
        imij::Error::Corrupt("bad unit table").category(),
        ErrorCategory::Image(ImageError::Malformed)
    );
    assert_eq!(
        imij::Error::InvalidInput("bad stride").category(),
        ErrorCategory::Request(RequestError::Invalid(InvalidKind::Parameters))
    );
    assert_eq!(
        imij::Error::TooLarge.category(),
        ErrorCategory::Resource(ResourceError::OutOfMemory)
    );
    assert_eq!(
        imij::Error::Cancelled.category(),
        ErrorCategory::Stopped(StopReason::Cancelled)
    );
    let dec_err = imij::codec::DecodeError::from(imij::Error::Corrupt("bad"));
    assert_eq!(dec_err.codec_name(), Some("imij"));
    assert_eq!(
        dec_err.category(),
        ErrorCategory::Image(ImageError::Malformed)
    );
}
