//! Explicit source code ranges. These are independent of transformed channel
//! scales and of the signed coefficient width. Full-storage inputs keep their
//! existing kernels; custom ranges share this path across all bit counts.
use super::{LossyColor, color, color_index, transfer};
#[cfg(feature = "encode")]
use crate::Error;
use crate::{ImageInfo, SampleType};
use alloc::vec::Vec;

pub(super) fn custom_color(info: &ImageInfo) -> bool {
    color_index(info.channels)
        .iter()
        .any(|&c| info.channel_bits(c) != Some(info.sample.depth() as u8))
}

#[cfg(feature = "encode")]
pub(super) fn validate_pixels(info: &ImageInfo, pixels: &[u8], stride: usize) -> Result<(), Error> {
    info.validate_precision()?;
    if !info.has_custom_precision() && info.sample != SampleType::U32 {
        return Ok(());
    }
    if !matches!(
        info.sample,
        SampleType::U8 | SampleType::U16 | SampleType::U32
    ) {
        return Err(Error::Unsupported(
            "lossy significant bits require unsigned 8/16/32-bit storage",
        ));
    }
    if !info.cicp.full_range || info.cicp.matrix_coefficients != 0 {
        return Err(Error::Unsupported(
            "lossy significant bits require full-range RGB code values",
        ));
    }
    if !info.has_custom_precision() {
        return Ok(());
    }
    let bps = info.sample.bytes();
    let nc = info.channels.count();
    let max: [u32; 4] =
        core::array::from_fn(|c| info.channel_bits(c).map_or(0, |n| crate::mask_of(n as u32)));
    for y in 0..info.height as usize {
        for (i, sample) in pixels[y * stride..][..info.row_bytes()]
            .chunks_exact(bps)
            .enumerate()
        {
            let v = match bps {
                1 => sample[0] as u32,
                2 => u16::from_ne_bytes([sample[0], sample[1]]) as u32,
                4 => u32::from_ne_bytes(sample.try_into().unwrap()),
                _ => unreachable!(),
            };
            if v > max[i % nc] {
                return Err(Error::InvalidInput(
                    "sample exceeds declared significant-bit range",
                ));
            }
        }
    }
    Ok(())
}

#[cfg(feature = "encode")]
pub(super) struct Input {
    factor: [f32; 3],
    table: [Vec<f32>; 3],
    table_index: [usize; 3],
    // U32 never indexes a source-code table. Its f64 normalization preserves
    // endpoint/range arithmetic before the existing f32 transform; it does
    // not promise 32 bits of lossy color precision.
    normalized: Option<([u32; 3], Option<transfer::Transfer>)>,
}
#[cfg(feature = "encode")]
impl Input {
    // Validate without materializing a code-value table. Bounded preparation
    // shares this check with the actual conversion startup.
    pub(super) fn validate(info: &ImageInfo, color: LossyColor) -> Result<(), Error> {
        if color == LossyColor::Xyb
            && transfer::Transfer::from_cicp(info.cicp.transfer_characteristics).is_none()
        {
            return Err(Error::Unsupported(if info.sample == SampleType::U32 {
                "unknown transfer for U32 XYB"
            } else {
                "unknown transfer for significant-bit XYB"
            }));
        }
        Ok(())
    }

    pub(super) fn new(info: &ImageInfo, color: LossyColor) -> Result<Self, Error> {
        Self::validate(info, color)?;
        let indices = color_index(info.channels);
        let bits = indices.map(|c| info.channel_bits(c).unwrap());
        let max = bits.map(|n| crate::mask_of(n as u32));
        let full = crate::mask_of(info.sample.depth()) as f32;
        let mut result = Self {
            factor: max.map(|m| full / m as f32),
            table: core::array::from_fn(|_| Vec::new()),
            table_index: [0, 1, 2],
            normalized: None,
        };
        if info.sample == SampleType::U32 {
            let transfer = if color == LossyColor::Xyb {
                Some(
                    transfer::Transfer::from_cicp(info.cicp.transfer_characteristics)
                        .ok_or(Error::Unsupported("unknown transfer for U32 XYB"))?,
                )
            } else {
                None
            };
            result.normalized = Some((max, transfer));
            return Ok(result);
        }
        if color == LossyColor::Xyb {
            let t = transfer::Transfer::from_cicp(info.cicp.transfer_characteristics).ok_or(
                Error::Unsupported("unknown transfer for significant-bit XYB"),
            )?;
            for c in 0..3 {
                if let Some(previous) = (0..c).find(|&p| bits[p] == bits[c]) {
                    result.table_index[c] = result.table_index[previous];
                } else {
                    result.table[c] = (0..=max[c])
                        .map(|v| t.decode(v as f64 / max[c] as f64) as f32)
                        .collect();
                }
            }
        }
        Ok(result)
    }
    #[inline]
    pub(super) fn read(&self, c: usize, value: u32) -> f32 {
        if let Some((max, transfer)) = self.normalized {
            let unit = f64::from(value) / f64::from(max[c]);
            return match transfer {
                Some(t) => t.decode(unit) as f32,
                None => (unit * 255.0) as f32,
            };
        }
        let table = &self.table[self.table_index[c]];
        if table.is_empty() {
            value as f32 * self.factor[c]
        } else {
            table[value as usize]
        }
    }
}

/// Requantize directly from the reconstructed float to each source code grid.
/// No intermediate rounded 8/16-bit RGB value is introduced.
pub(super) fn output_row(
    info: &ImageInfo,
    color: LossyColor,
    inv_scale: [f32; 3],
    planes: [&[f32]; 3],
    xfer: Option<&(Vec<f32>, f32)>,
    row: &mut [u8],
    n: usize,
) {
    if info.sample == SampleType::U32 {
        let max = color_index(info.channels)
            .map(|c| crate::mask_of(info.channel_bits(c).unwrap() as u32));
        for x in 0..n {
            let v = planes.map(|p| p[x]);
            let rgb = if color == LossyColor::Xyb {
                color::inverse_xyb_linear(inv_scale, v)
            } else {
                color::inverse_ycocg(inv_scale, v)
            };
            for c in 0..3 {
                let unit = if let Some((table, lmax)) = xfer {
                    f64::from(transfer::unrounded_code(table, *lmax, rgb[c])) / 65535.0
                } else {
                    f64::from(rgb[c]) / 255.0
                };
                let code = unit_to_u32(unit, max[c]);
                row[(x * 3 + c) * 4..][..4].copy_from_slice(&code.to_ne_bytes());
            }
        }
        return;
    }
    let max = color_index(info.channels)
        .map(|c| crate::mask_of(info.channel_bits(c).unwrap() as u32) as f32);
    let full = crate::mask_of(info.sample.depth()) as f32;
    let bps = info.sample.bytes();
    for x in 0..n {
        let v = planes.map(|p| p[x]);
        let rgb = if color == LossyColor::Xyb {
            color::inverse_xyb_linear(inv_scale, v)
        } else {
            color::inverse_ycocg(inv_scale, v)
        };
        for c in 0..3 {
            let code = if let Some((table, lmax)) = xfer {
                transfer::encode_code(table, *lmax, rgb[c], max[c])
            } else {
                color::rne_pos((rgb[c] * (max[c] / full)).clamp(0.0, max[c])) as u16
            };
            let offset = (x * 3 + c) * bps;
            if bps == 1 {
                row[offset] = code as u8;
            } else {
                row[offset..offset + 2].copy_from_slice(&code.to_ne_bytes());
            }
        }
    }
}

/// Clamp normalized color and round once to its declared U32 grid. f64 can
/// represent all U32 endpoints and half-code ties; the f32 reconstruction
/// feeding this conversion has already lost low bits. No f32 adder rounding
/// trick or intermediate U16 quantization is valid for this range.
#[inline]
fn unit_to_u32(unit: f64, max: u32) -> u32 {
    if unit <= 0.0 || unit.is_nan() {
        return 0;
    }
    if unit >= 1.0 {
        return max;
    }
    let value = unit * f64::from(max);
    let lower = value as u32;
    let fraction = value - f64::from(lower);
    let up = fraction > 0.5 || (fraction == 0.5 && lower & 1 != 0);
    lower.saturating_add(u32::from(up)).min(max)
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::{Channels, Cicp};
    use alloc::vec;

    #[cfg(not(feature = "extended"))]
    #[test]
    fn u32_lossy_encode_requires_existing_extended_feature() {
        let info = ImageInfo::new(1, 1, Channels::Rgb, SampleType::U32);
        assert!(matches!(
            super::super::encode_lossy(
                &[0; 12],
                12,
                &info,
                &crate::Metadata::default(),
                &super::super::LossyConfig::new(90.0)
            ),
            Err(Error::Unsupported("32-bit samples (extended tier)"))
        ));
    }

    #[test]
    fn u32_final_rounding_is_bounded_and_ties_even() {
        for max in [1, 3, (1 << 24) - 1, (1 << 31) - 1, u32::MAX] {
            for unit in [f64::NEG_INFINITY, -1.0, 0.0, f64::NAN] {
                assert_eq!(unit_to_u32(unit, max), 0);
            }
            for unit in [1.0, 2.0, f64::INFINITY] {
                assert_eq!(unit_to_u32(unit, max), max);
            }
            // The conversion must reproduce every selected exactly represented
            // integer code, including values above f32's exact-integer range.
            for code in [0, 1, max / 2, max - 1, max] {
                assert_eq!(unit_to_u32(f64::from(code) / f64::from(max), max), code);
            }
        }
        // Odd full-range maxima give exact half-code ties at unit 0.5.
        assert_eq!(unit_to_u32(0.5, 1), 0);
        assert_eq!(unit_to_u32(0.5, 3), 2);
        assert_eq!(unit_to_u32(0.5, u32::MAX), 1 << 31);
        assert_eq!(unit_to_u32(2.5 / 7.0, 7), 2);
        assert_eq!(unit_to_u32(3.5 / 7.0, 7), 4);
    }

    #[test]
    fn u32_input_never_allocates_a_code_lut_and_has_explicit_f32_limit() {
        for bits in 1..=32 {
            let mut info = ImageInfo::new(1, 1, Channels::Rgb, SampleType::U32)
                .with_significant_bits(&[bits, 32, 1])
                .unwrap();
            for cicp in [Cicp::SRGB, Cicp::BT2100_PQ, Cicp::BT2100_HLG] {
                info.cicp = cicp;
                for color in [LossyColor::YCoCg, LossyColor::Xyb] {
                    let input = Input::new(&info, color).unwrap();
                    assert!(input.table.iter().all(|table| table.capacity() == 0));
                    let max = [crate::mask_of(bits as u32), u32::MAX, 1];
                    let end = if color == LossyColor::YCoCg {
                        255.0
                    } else {
                        transfer::Transfer::from_cicp(cicp.transfer_characteristics)
                            .unwrap()
                            .decode(1.0) as f32
                    };
                    for (c, max) in max.into_iter().enumerate() {
                        assert_eq!(input.read(c, 0), 0.0);
                        assert_eq!(input.read(c, max), end);
                    }
                    assert_eq!(
                        input.read(1, u32::MAX),
                        input.read(1, u32::MAX - 1),
                        "f32 transform input does not preserve all 32 color bits"
                    );
                }
            }
        }
    }

    #[test]
    fn u32_output_uses_direct_unequal_grids_without_u16_rounding() {
        let info = ImageInfo::new(4, 1, Channels::Rgb, SampleType::U32)
            .with_significant_bits(&[17, 24, 32])
            .unwrap();
        let y = [0.0, 255.0, 255.0 * 0.25, -1.0];
        let zero = [0.0; 4];
        let mut bytes = [0; 48];
        output_row(
            &info,
            LossyColor::YCoCg,
            [1.0; 3],
            [&y, &zero, &zero],
            None,
            &mut bytes,
            4,
        );
        let values: Vec<u32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| u32::from_ne_bytes(*b))
            .collect();
        assert_eq!(&values[..3], &[0, 0, 0]);
        assert_eq!(&values[3..6], &[131071, 16777215, u32::MAX]);
        assert_eq!(&values[6..9], &[32768, 4194304, 1073741824]);
        assert_eq!(&values[9..], &[0, 0, 0]);
        assert_ne!(
            values[8] % 65537,
            0,
            "output must not first quantize to U16"
        );
    }

    #[test]
    fn direct_code_grid_simd_matches_scalar_at_all_depths_and_tails() {
        for sample in [SampleType::U8, SampleType::U16] {
            for bits in 1..=sample.depth() as u8 {
                let depths = [bits, 1, (bits + 1).min(sample.depth() as u8)];
                let mut info = ImageInfo::new(23, 1, Channels::Rgb, sample)
                    .with_significant_bits(&depths)
                    .unwrap();
                let ranges = depths.map(|n| crate::mask_of(n as u32) as f32);
                let full = crate::mask_of(sample.depth()) as f32;
                // Include negatives, out-of-range colors and exact rounding
                // boundaries. Planar padding lets vector tails load safely.
                let planes: [Vec<f32>; 3] = core::array::from_fn(|c| {
                    (0..24)
                        .map(|x| ((x * (c + 1) * 337) as f32 - 157.5) / 40.0)
                        .collect()
                });
                for t in [
                    transfer::Transfer::Srgb,
                    transfer::Transfer::Pq,
                    transfer::Transfer::Hlg,
                ] {
                    info.cicp = match t {
                        transfer::Transfer::Srgb => Cicp::SRGB,
                        transfer::Transfer::Pq => Cicp::BT2100_PQ,
                        transfer::Transfer::Hlg => Cicp::BT2100_HLG,
                    };
                    let table = (t.table(), t.linear_max_f32());
                    for color in [LossyColor::Xyb, LossyColor::YCoCg] {
                        let inv = if color == LossyColor::YCoCg && sample == SampleType::U16 {
                            [257.0; 3]
                        } else {
                            color.default_scales().map(|v| 1.0 / v)
                        };
                        for n in 1..=23 {
                            let mut expected = vec![0; n * 3 * sample.bytes()];
                            let mut actual = expected.clone();
                            let p = planes.each_ref().map(Vec::as_slice);
                            output_row(
                                &info,
                                color,
                                inv,
                                p,
                                (color == LossyColor::Xyb).then_some(&table),
                                &mut expected,
                                n,
                            );
                            if color == LossyColor::Xyb {
                                let mut lin = vec![0.0; n * 3];
                                super::super::simd::xyb_row_linear(
                                    p[0], p[1], p[2], inv, &mut lin, n,
                                );
                                super::super::simd::encode_range_row(
                                    &lin,
                                    &table.0,
                                    table.1,
                                    &mut actual,
                                    ranges,
                                    sample.bytes(),
                                );
                            } else {
                                super::super::simd::ycocg_range_row(
                                    p[0],
                                    p[1],
                                    p[2],
                                    inv,
                                    &mut actual,
                                    n,
                                    ranges,
                                    full,
                                    sample.bytes(),
                                );
                            }
                            assert_eq!(
                                actual, expected,
                                "{sample:?} {bits} {t:?} {color:?} tail{n}"
                            );
                        }
                    }
                }
            }
        }
    }
}
