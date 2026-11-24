//! Byte-level oracles for row packing; deliberately no color conversions.
use super::pack_rows;
use crate::{Channels, ImageInfo, SampleType};
use alloc::{vec, vec::Vec};

fn sample_word(sample: SampleType, index: usize) -> u32 {
    // Include float signed zeros, infinities, signaling/quiet NaN payloads,
    // subnormals, and normal boundaries. Integer formats use the same raw bits.
    const WORDS32: [u32; 16] = [
        0,
        0x8000_0000,
        0x7f80_0000,
        0xff80_0000,
        0x7fc1_2345,
        0x7fa5_4321,
        0xffc2_468a,
        1,
        0x007f_ffff,
        0x0080_0000,
        0x8000_0001,
        0x3f80_0000,
        0x7f7f_ffff,
        0xffff_ffff,
        0x55aa_33cc,
        0xaa55_cc33,
    ];
    const WORDS16: [u16; 16] = [
        0, 0x8000, 0x7c00, 0xfc00, 0x7e35, 0x7d57, 0xfeab, 1, 0x03ff, 0x0400, 0x8001, 0x3c00,
        0x7bff, 0xffff, 0x55aa, 0xaa55,
    ];
    match sample.bytes() {
        1 => (index as u32).wrapping_mul(73).wrapping_add(19) & 255,
        2 => u32::from(WORDS16[index % WORDS16.len()]),
        _ => WORDS32[index % WORDS32.len()],
    }
}

fn write_word(out: &mut Vec<u8>, word: u32, bps: usize) {
    match bps {
        1 => out.push(word as u8),
        2 => out.extend_from_slice(&(word as u16).to_ne_bytes()),
        _ => out.extend_from_slice(&word.to_ne_bytes()),
    }
}

fn check(sample: SampleType, channels: Channels, width: usize, rows: usize, prefix: usize) {
    let bps = sample.bytes();
    let nco = channels.count();
    let info = ImageInfo::new(width as u32, rows as u32, channels, sample);
    let mut source = vec![0x6b; prefix];
    let mut alpha = vec![0x39; prefix];
    for index in 0..rows * width * 3 {
        write_word(&mut source, sample_word(sample, index), bps);
    }
    for index in 0..rows * width {
        write_word(&mut alpha, sample_word(sample, index * 3 + 5), bps);
    }
    let rgb = &source[prefix..];
    let alpha = channels.has_alpha().then_some(&alpha[prefix..]);
    // Crop each destination row within an odd-stride caller buffer, including
    // byte-unaligned u16/u32 starts. All padding must remain untouched.
    let left = prefix + 3 * nco * bps;
    let stride = left + width * nco * bps + 7;
    let mut actual = vec![0xa5; stride * rows + 11];
    let mut expected = actual.clone();
    for y in 0..rows {
        for x in 0..width {
            for c in 0..nco {
                for b in 0..bps {
                    let is_alpha = channels.has_alpha() && c == nco - 1;
                    let value = if is_alpha {
                        alpha.unwrap()[(y * width + x) * bps + b]
                    } else {
                        let color = if matches!(channels, Channels::Gray | Channels::GrayAlpha) {
                            1
                        } else {
                            c
                        };
                        rgb[((y * width + x) * 3 + color) * bps + b]
                    };
                    expected[y * stride + left + (x * nco + c) * bps + b] = value;
                }
            }
        }
    }
    let mut out: Vec<&mut [u8]> = actual[..stride * rows]
        .chunks_exact_mut(stride)
        .map(|row| &mut row[left..])
        .collect();
    pack_rows(&info, width, rgb, alpha, &mut out);
    assert_eq!(
        actual, expected,
        "{sample:?} {channels:?}, w={width} rows={rows}, prefix={prefix}"
    );
}

#[test]
fn all_layouts_sizes_tails_crops_and_float_payloads_match_byte_oracle() {
    for sample in [
        SampleType::U8,
        SampleType::U16,
        SampleType::F16,
        SampleType::U32,
        SampleType::I32,
        SampleType::F32,
    ] {
        for channels in [
            Channels::Gray,
            Channels::GrayAlpha,
            Channels::Rgb,
            Channels::Rgba,
        ] {
            for width in [
                0, 1, 2, 3, 7, 8, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129, 257,
            ] {
                for rows in [0, 1, 3, 9] {
                    for prefix in [0, 1, 7] {
                        check(sample, channels, width, rows, prefix);
                    }
                }
            }
        }
    }
}

#[test]
fn packing_a_row_subset_uses_its_local_origin() {
    let width = 5;
    for sample in [SampleType::U8, SampleType::U16, SampleType::F32] {
        let bps = sample.bytes();
        for channels in [
            Channels::Gray,
            Channels::GrayAlpha,
            Channels::Rgb,
            Channels::Rgba,
        ] {
            let nco = channels.count();
            let info = ImageInfo::new(99, 77, channels, sample);
            let source: Vec<u8> = (0..width * 3 * bps * 7).map(|i| (i * 29) as u8).collect();
            let alpha: Vec<u8> = (0..width * bps * 7).map(|i| (i * 13 + 1) as u8).collect();
            let mut whole = vec![vec![0u8; width * nco * bps]; 7];
            let mut subset = vec![vec![0u8; width * nco * bps]; 3];
            pack_rows(
                &info,
                width,
                &source,
                channels.has_alpha().then_some(&alpha),
                &mut whole.iter_mut().map(Vec::as_mut_slice).collect::<Vec<_>>(),
            );
            pack_rows(
                &info,
                width,
                &source[2 * width * 3 * bps..],
                channels.has_alpha().then_some(&alpha[2 * width * bps..]),
                &mut subset.iter_mut().map(Vec::as_mut_slice).collect::<Vec<_>>(),
            );
            assert_eq!(subset, whole[2..5]);
        }
    }
}
