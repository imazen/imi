//! Exact sample storage at every meaningful integer depth. These are arithmetic
//! and API parity fixtures, not a photographic compression benchmark.
use imij::{Channels, ImageInfo, ImiEncoderConfig, Metadata, SampleType, StreamDecoder};

fn mask(bits: u32) -> u32 {
    ((1u64 << bits) - 1) as u32
}

/// Independent container construction; Scale uses wider rational arithmetic.
fn stored(code: u32, bits: u32, depth: u32, layout: u32) -> u32 {
    match layout {
        0 => code,
        1 => code << (depth - bits),
        2 => (0..depth).fold(0, |out, i| {
            out | (((code >> (bits - 1 - i % bits)) & 1) << (depth - 1 - i))
        }),
        _ => {
            ((u128::from(code) * u128::from(mask(depth)) + u128::from(mask(bits)) / 2)
                / u128::from(mask(bits))) as u32
        }
    }
}

fn push_sample(out: &mut Vec<u8>, value: u32, sample: SampleType) {
    match sample.bytes() {
        1 => out.push(value as u8),
        2 => out.extend_from_slice(&(value as u16).to_ne_bytes()),
        _ => out.extend_from_slice(&value.to_ne_bytes()),
    }
}

fn check(info: &ImageInfo, tight: &[u8], cfg: &ImiEncoderConfig) {
    let row = info.row_bytes();
    let height = info.height as usize;
    let stride = row + 3;
    let mut input = vec![0xd9; stride * height];
    for y in 0..height {
        input[y * stride..y * stride + row].copy_from_slice(&tight[y * row..(y + 1) * row]);
    }
    let file = cfg
        .encode_samples(
            imij::ImageRef::new(&input, stride, info),
            &Metadata::default(),
        )
        .unwrap();
    let (actual_info, actual) = imij::decode(&file).unwrap();
    assert_eq!(actual_info, *info);
    assert_eq!(actual, tight);
    let thread_counts: &[usize] = if cfg!(feature = "std") { &[1, 4] } else { &[1] };
    for &threads in thread_counts {
        let mut output = vec![0xa7; stride * height + 11];
        #[cfg(feature = "std")]
        imij::decode_into_threads(&file, &mut output, stride, threads).unwrap();
        #[cfg(not(feature = "std"))]
        {
            let _ = threads;
            imij::decode_into(&file, &mut output, stride).unwrap();
        }
        for y in 0..height {
            assert_eq!(
                &output[y * stride..y * stride + row],
                &tight[y * row..(y + 1) * row]
            );
            assert!(
                output[y * stride + row..(y + 1) * stride]
                    .iter()
                    .all(|&v| v == 0xa7)
            );
        }
        assert!(output[stride * height..].iter().all(|&v| v == 0xa7));
    }
    let mut stream = StreamDecoder::new(&file).unwrap();
    let mut output = vec![0; row];
    for expected in tight.chunks_exact(row) {
        assert!(stream.next_row(&mut output).unwrap());
        assert_eq!(output, expected);
    }
    assert!(!stream.next_row(&mut output).unwrap());
    #[cfg(feature = "std")]
    {
        let mut output = Vec::new();
        imij::decode_reader(&file[..], &imij::Limits::DEFAULT, |actual_info, _, row| {
            assert_eq!(actual_info, info);
            output.extend_from_slice(row);
            Ok(())
        })
        .unwrap();
        assert_eq!(output, tight);
    }
}

#[test]
fn all_meaningful_integer_depths_with_independent_channels() {
    let mut samples = vec![SampleType::U8, SampleType::U16];
    if cfg!(feature = "extended") {
        samples.extend([SampleType::U32, SampleType::I32]);
    }
    for sample in samples {
        let depth = sample.bytes() as u32 * 8;
        let info = ImageInfo::new(19, 11, Channels::Rgba, sample);
        for bits in 1..=depth {
            let mut tight = Vec::new();
            for index in 0..info.width * info.height {
                for channel in 0..4 {
                    let n = [
                        bits,
                        bits.saturating_sub(1).max(1),
                        (bits + 1).min(depth),
                        bits.min(5),
                    ][channel as usize];
                    let random = index.wrapping_mul(0x9e37_79b9).rotate_left(channel * 7);
                    let code = match index % 8 {
                        0 => 0,
                        1 => mask(n),
                        2 => 1 << (n - 1),
                        3 => 1,
                        _ => random & mask(n),
                    };
                    let value = stored(code, n, depth, (bits + channel) % 4);
                    let value = if sample != SampleType::I32 {
                        value
                    } else if (bits + channel) % 4 == 0 {
                        // LSB storage contains an n-bit signed integer,
                        // sign-extended to its 32-bit container.
                        ((code << (32 - n)) as i32 >> (32 - n)) as u32
                    } else {
                        // Other layouts are exact embeddings in the ordered
                        // signed domain, exercising the full container range.
                        value ^ 0x8000_0000
                    };
                    push_sample(&mut tight, value, sample);
                }
            }
            let cfg = {
                let mut cfg = imij::ImiEncoderConfig::default();
                cfg = cfg.tune_lossless(|t| {
                    t.stripe_rows = Some(3);
                });
                cfg = cfg.tune_lossless(|t| {
                    t.lanes = Some(3);
                });
                cfg = cfg.tune_lossless(|t| {
                    t.predictor_mask = 1 << (bits % 7);
                });
                cfg = cfg.tune_lossless(|t| {
                    t.decor_mask = 1 << (bits % 4);
                });
                cfg
            }
            .with_effort(0.0);
            check(&info, &tight, &cfg);
        }
    }
}

#[test]
fn floating_precision_and_special_patterns_are_storage_exact() {
    let samples = [SampleType::F16, SampleType::F32];
    for sample in samples {
        let (mantissa, exponent_bits, sign) = if sample == SampleType::F16 {
            (10u32, 5u32, 0x8000)
        } else {
            (23, 8, 0x8000_0000)
        };
        let info = ImageInfo::new(37, 9, Channels::GrayAlpha, sample);
        for meaningful_fraction in 0..=mantissa {
            let mut tight = Vec::new();
            for i in 0..info.width * info.height * 2 {
                let exponent = match i % 5 {
                    0 => 0,
                    1 => 1,
                    2 => mask(exponent_bits),
                    _ => (i * 17) & mask(exponent_bits),
                };
                let fraction = (i.wrapping_mul(0x9e37_79b9) & mask(meaningful_fraction))
                    << (mantissa - meaningful_fraction);
                // Include quiet/signaling NaNs, signed zero, infinities and
                // subnormals as raw integers; do not quiet payloads in FP ops.
                let bits = (if i & 2 == 0 { sign } else { 0 }) | (exponent << mantissa) | fraction;
                push_sample(&mut tight, bits, sample);
            }
            check(
                &info,
                &tight,
                &{
                    let mut cfg = imij::ImiEncoderConfig::default();
                    cfg = cfg.tune_lossless(|t| {
                        t.stripe_rows = Some(3);
                    });
                    cfg = cfg.tune_lossless(|t| {
                        t.lanes = Some(2);
                    });
                    cfg
                }
                .with_effort(0.0),
            );
        }
    }
}

#[test]
fn modular_color_residuals_keep_all_significant_bits() {
    for bits in [1u32, 8, 9, 10, 11, 12, 13, 15, 16, 17, 23, 24, 31, 32] {
        if bits > 16 && !cfg!(feature = "extended") {
            continue;
        }
        let sample = if bits <= 16 {
            SampleType::U16
        } else {
            SampleType::U32
        };
        let info = ImageInfo::new(23, 13, Channels::Rgb, sample);
        let mut tight = Vec::new();
        for i in 0..info.width * info.height {
            for channel in 0..3 {
                let value = match i % 7 {
                    0 => 0,
                    1 => mask(bits),
                    2 => 1 << (bits - 1),
                    3 => (1 << (bits - 1)) - 1,
                    _ => i.wrapping_mul(0x9e37_79b9).rotate_left(channel * 9) & mask(bits),
                };
                push_sample(&mut tight, value, sample);
            }
        }
        // Production decorrelation is modular residual arithmetic, not
        // YCoCg-R. Force each nonidentity mode; palette must not bypass it.
        for decor in 1..=3 {
            check(
                &info,
                &tight,
                &{
                    let mut cfg = imij::ImiEncoderConfig::default();
                    cfg = cfg.tune_lossless(|t| {
                        t.palette = false;
                    });
                    cfg = cfg.tune_lossless(|t| {
                        t.stripe_rows = Some(3);
                    });
                    cfg = cfg.tune_lossless(|t| {
                        t.lanes = Some(3);
                    });
                    cfg = cfg.tune_lossless(|t| {
                        t.predictor_mask = 1 << 3;
                    });
                    cfg = cfg.tune_lossless(|t| {
                        t.decor_mask = 1 << decor;
                    });
                    cfg
                }
                .with_effort(0.0),
            );
        }
    }
}
