//! Pixel conversion for `encode` (PNG/raw input -> declared sample/channel
//! format) and `decode-rgba` (any decoded format -> RGBA8 preview).
//!
//! Code-value semantics: integer samples are magnitude code values, i32 is
//! handled as its u32 bit pattern, f16/f32 samples carry IEEE bit patterns
//! in memory but convert through their numeric value ([0,1] normalized is
//! assumed when converting to/from integers).

use crate::args::{EncodeArgs, Expand};
use imij::{Channels, ImageInfo, SampleType};

pub const TONING_NOTE: &str = "8-bit direct; 16-bit shifted (v>>8); 32-bit shifted (v>>24); \
     64-bit shifted (v>>56); signed: negatives as 0, then the top byte of the non-negative \
     range; f16/f32/f64 clamped [0,1]*255";

/// Max code value of an integer container (signed types share the
/// unsigned range of their width).
fn int_max(s: SampleType) -> u64 {
    match s {
        SampleType::U8 | SampleType::I8 => 0xFF,
        SampleType::U16 | SampleType::I16 => 0xFFFF,
        SampleType::U32 | SampleType::I32 => 0xFFFF_FFFF,
        SampleType::U64 | SampleType::I64 => u64::MAX,
        SampleType::F16 | SampleType::F32 | SampleType::F64 => 0, // not integer
        s => unreachable!("no range for {s:?}"),
    }
}

fn is_float(s: SampleType) -> bool {
    matches!(s, SampleType::F16 | SampleType::F32 | SampleType::F64)
}

/// Read one sample as u64 code value (floats: bit pattern).
fn read_code(row: &[u8], at: usize, s: SampleType) -> u64 {
    match s.bytes() {
        1 => row[at] as u64,
        2 => u16::from_ne_bytes(row[at..at + 2].try_into().unwrap()) as u64,
        4 => u32::from_ne_bytes(row[at..at + 4].try_into().unwrap()) as u64,
        _ => u64::from_ne_bytes(row[at..at + 8].try_into().unwrap()),
    }
}

fn write_code(out: &mut [u8], at: usize, s: SampleType, v: u64) {
    match s.bytes() {
        1 => out[at] = v as u8,
        2 => out[at..at + 2].copy_from_slice(&(v as u16).to_ne_bytes()),
        4 => out[at..at + 4].copy_from_slice(&(v as u32).to_ne_bytes()),
        _ => out[at..at + 8].copy_from_slice(&v.to_ne_bytes()),
    }
}

/// Integer code value -> integer code value under the expand rule.
fn remap(v: u64, src: SampleType, dst: SampleType, expand: &Expand) -> u64 {
    let (sb, db) = (src.depth_bits(), dst.depth_bits());
    let (smax, dmax) = (int_max(src), int_max(dst));
    match expand {
        Expand::Scale => ((v as u128 * dmax as u128 + smax as u128 / 2) / smax as u128) as u64,
        Expand::Shift => {
            if db >= sb {
                v << (db - sb)
            } else {
                v >> (sb - db)
            }
        }
        Expand::Replicate => {
            if db > sb {
                // Repeat the source bit pattern to fill the destination.
                let mut v = v;
                let mut have = sb;
                while have < db {
                    v = (v << sb) | v;
                    have += sb;
                }
                v >> (have - db)
            } else {
                v >> (sb - db)
            }
        }
    }
}

/// IEEE half -> f32 (exact; `f16` is unstable on rustc 1.99).
fn f16_to_f32(bits: u16) -> f32 {
    let sign = (bits >> 15) & 1;
    let exp = ((bits >> 10) & 0x1f) as i32;
    let man = (bits & 0x3ff) as f32;
    let v = match exp {
        0 => man * 5.960_464_5e-8, // subnormal: man * 2^-24
        31 => {
            if man == 0.0 {
                f32::INFINITY
            } else {
                f32::NAN
            }
        }
        e => (1.0 + man / 1024.0) * 2f32.powi(e - 15),
    };
    if sign == 1 {
        -v
    } else {
        v
    }
}

/// f32 -> IEEE half bits, round-to-nearest-even (deterministic).
pub(crate) fn f32_to_f16(v: f32) -> u16 {
    if v.is_nan() {
        return 0x7e00;
    }
    let sign = if v.is_sign_negative() { 0x8000u16 } else { 0 };
    let f = v.abs();
    if f < 6.103_515_6e-5 {
        // f16 subnormal range: mantissa = round(f / 2^-24); a result of
        // 0x400 carries correctly into the smallest normal.
        if f < 2.980_232_2e-8 {
            return sign; // rounds to zero
        }
        return sign | (f * 16_777_216.0).round_ties_even() as u16;
    }
    if f >= 65504.0 {
        return sign | 0x7c00;
    }
    let b = f.to_bits();
    let e = ((b >> 23) & 0xff) as i32 - 127 + 15;
    // RNE on the top 10 mantissa bits; mantissa overflow carries into the
    // exponent (which yields inf at the boundary).
    let m = (b & 0x7fffff) + 0xfff + ((b >> 13) & 1);
    sign | (((e as u32) << 10) + (m >> 13)) as u16
}

/// f32 numeric value of a code value: floats decode their bits, integers
/// normalize by the container max.
fn code_to_f32(v: u64, s: SampleType) -> f32 {
    match s {
        SampleType::F16 => f16_to_f32(v as u16),
        SampleType::F32 => f32::from_bits(v as u32),
        SampleType::F64 => f64::from_bits(v) as f32,
        _ => v as f32 / int_max(s) as f32,
    }
}

/// f32 numeric value -> code value in `s`: floats store their bits,
/// integers clamp to [0,1] and scale to the container max.
fn f32_to_code(v: f32, s: SampleType) -> u64 {
    match s {
        SampleType::F16 => f32_to_f16(v) as u64,
        SampleType::F32 => v.to_bits() as u64,
        SampleType::F64 => (v as f64).to_bits(),
        _ => (v.clamp(0.0, 1.0) * int_max(s) as f32).round_ties_even() as u64,
    }
}

fn convert_sample(v: u64, src: SampleType, dst: SampleType, expand: &Expand) -> u64 {
    match (is_float(src), is_float(dst)) {
        // Both integer: direct magnitude remap.
        (false, false) => remap(v, src, dst, expand),
        // Anything else goes through the numeric f32 value.
        _ => f32_to_code(code_to_f32(v, src), dst),
    }
}

trait IntDepth {
    fn depth_bits(self) -> u32;
}
impl IntDepth for SampleType {
    fn depth_bits(self) -> u32 {
        8 * self.bytes() as u32
    }
}

/// Channel-map one pixel's worth of code values between channel counts, in
/// the destination sample domain. rgb->gray uses 299/587/114 luma; gray
/// replicates; added alpha is opaque.
fn map_channels(px: &mut [u64; 4], snc: usize, dst_ch: Channels, dst: SampleType) {
    let (r, g, b, a) = match snc {
        1 => (px[0], px[0], px[0], None),
        2 => (px[0], px[0], px[0], Some(px[1])),
        3 => (px[0], px[1], px[2], None),
        _ => (px[0], px[1], px[2], Some(px[3])),
    };
    let opaque = if is_float(dst) {
        f32_to_code(1.0, dst)
    } else {
        int_max(dst)
    };
    match dst_ch {
        Channels::Gray | Channels::GrayAlpha => {
            let luma = if is_float(dst) {
                let (r, g, b) = (
                    code_to_f32(r, dst),
                    code_to_f32(g, dst),
                    code_to_f32(b, dst),
                );
                f32_to_code(0.299 * r + 0.587 * g + 0.114 * b, dst)
            } else {
                (r * 299 + g * 587 + b * 114 + 500) / 1000
            };
            px[0] = luma;
            if dst_ch == Channels::GrayAlpha {
                px[1] = a.unwrap_or(opaque);
            }
        }
        Channels::Rgb => {
            // gray/ga -> rgb replicates luma; rgb/rgba keep converted values.
            px[0] = r;
            px[1] = g;
            px[2] = b;
        }
        Channels::Rgba => {
            px[0] = r;
            px[1] = g;
            px[2] = b;
            px[3] = a.unwrap_or(opaque);
        }
        Channels::Cmyk | Channels::Oklab | Channels::OklabAlpha => {
            unreachable!("model conversion checked by caller")
        }
    }
}

/// Apply `--channels`/`--sample`/`--expand` conversions. Returns the tightly
/// packed buffer and converted `ImageInfo` (cicp carried over).
pub fn convert(
    pixels: &[u8],
    stride: usize,
    info: ImageInfo,
    a: &EncodeArgs,
) -> Result<(Vec<u8>, ImageInfo), String> {
    let dst_ch = a.channels.unwrap_or(info.channels);
    let dst_s = a.sample.unwrap_or(info.sample);
    if dst_ch == info.channels && dst_s == info.sample {
        return Ok((pixels.to_vec(), info));
    }
    if matches!(
        info.channels,
        Channels::Cmyk | Channels::Oklab | Channels::OklabAlpha
    ) || matches!(
        dst_ch,
        Channels::Cmyk | Channels::Oklab | Channels::OklabAlpha
    ) {
        return Err("native CMYK/Oklab conversion requires a color-managed converter".into());
    }
    let (w, h) = (info.width as usize, info.height as usize);
    let (snc, sbytes) = (info.channels.count(), info.sample.bytes());
    let (dnc, dbytes) = (dst_ch.count(), dst_s.bytes());
    let row_len = w
        .checked_mul(snc)
        .and_then(|v| v.checked_mul(sbytes))
        .ok_or("input row does not fit usize")?;
    if stride < row_len || pixels.len() < stride.saturating_mul(h - 1) + row_len {
        return Err(format!(
            "input buffer {} too small for {}x{} {:?} {:?} at stride {stride}",
            pixels.len(),
            info.width,
            info.height,
            info.channels,
            info.sample
        ));
    }
    let mut out = vec![0u8; w * dnc * dbytes * h];
    let mut src_px = [0u64; 4];
    for y in 0..h {
        let srow = &pixels[y * stride..y * stride + row_len];
        let drow = &mut out[y * w * dnc * dbytes..(y + 1) * w * dnc * dbytes];
        for x in 0..w {
            // Sample conversion in the source channel count.
            for (c, slot) in src_px.iter_mut().enumerate().take(snc) {
                *slot = convert_sample(
                    read_code(srow, (x * snc + c) * sbytes, info.sample),
                    info.sample,
                    dst_s,
                    &a.expand,
                );
            }
            let mut px = src_px;
            map_channels(&mut px, snc, dst_ch, dst_s);
            for (c, &v) in px.iter().enumerate().take(dnc) {
                write_code(drow, (x * dnc + c) * dbytes, dst_s, v);
            }
        }
    }
    let mut out_info = ImageInfo::new(info.width, info.height, dst_ch, dst_s);
    out_info.cicp = info.cicp; // --cicp overrides are applied by the caller
    Ok((out, out_info))
}

/// Shared with the browser preview; native color/alpha transforms are explicit.
pub fn to_rgba8(info: &ImageInfo, pixels: &[u8]) -> Result<Vec<u8>, String> {
    crate::preview::to_rgba8(info, pixels).map_err(|error| error.to_string())
}
