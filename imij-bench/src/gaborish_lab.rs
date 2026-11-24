//! Paired encoder inverse-filter / decoder Gaborish experiment, RGB8 sRGB.
//!
//! An eight-byte lab header precedes a complete native IMI file. Its only
//! variable field enables the fixed filter pair; color and quantizer steps
//! come from the native stream. Complete wrapper bytes count toward rate.
//! Off delegates to production encoding/decoding for exact control parity.
//!
//! On: production color planes -> approximate inverse 5x5 filter -> canonical
//! DCT/quantization -> native entropy stream -> canonical IDCT -> Gaborish 3x3
//! -> production inverse color. Filters use scalar canonical FMA here; this
//! full-image lab adapter is not a production streaming or timing claim.
//!
//! Production source edge replication to an 8x8 grid happens BEFORE filtering;
//! filter halos mirror at that padded grid, and RGB output crops afterward.
//! The historical lab LF predictor mutates coefficients without an on-wire
//! identifier. This encoder disables it; a decoder cannot detect its prior use
//! and interprets incoming stored coefficients exactly as the native decoder.
#![cfg(feature = "lab")]

use crate::restoration::{self, FloatFilter, Kernel};
use imij::{
    Channels, Coding, Error, ImageInfo, ImiEncoderConfig, LossyColor, LossyEntropy, Metadata,
    SampleType, chroma_lab,
};

pub const HEADER_BYTES: usize = 8;
const SIGNATURE: [u8; 4] = *b"ZQGB";
const VERSION: u8 = 1;
// Native parsing retains metadata. Bound serialized input as well as decoded
// geometry so an otherwise tiny lab image cannot smuggle enormous metadata.
const MAX_WRAPPER_BYTES: usize = 128 * 1024 * 1024;

/// Tightly packed RGB8 sRGB result; stride is explicit in bytes.
#[derive(Debug, PartialEq, Eq)]
pub struct RgbImage {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub pixels: Vec<u8>,
}

/// Exact pre-Gaborish reconstruction, including all padded samples. Stride is
/// in f32 samples. Plane order is Y/X/B−Y for XYB and Y/Co/Cg for YCoCg.
/// Original width/height describe the output crop; padded dimensions describe
/// the filter domain and its mirror boundaries.
#[derive(Debug, PartialEq)]
pub struct PaddedPlanes {
    pub width: usize,
    pub height: usize,
    pub padded_width: usize,
    pub padded_height: usize,
    pub stride: usize,
    pub color: LossyColor,
    pub planes: [Vec<f32>; 3],
}

struct Profile<'a> {
    inner: &'a [u8],
    info: ImageInfo,
    enabled: bool,
    color: LossyColor,
    steps: [[f32; 64]; 3],
    pw: usize,
    ph: usize,
    pixels: usize,
}

fn zeros<T: Clone + Default>(len: usize) -> Result<Vec<T>, Error> {
    let mut out = Vec::new();
    out.try_reserve_exact(len).map_err(|_| Error::TooLarge)?;
    out.resize(len, T::default());
    Ok(out)
}

fn filter_planes(
    planes: &mut [Vec<f32>; 3],
    pw: usize,
    ph: usize,
    filter: FloatFilter,
) -> Result<(), Error> {
    let mut scratch = zeros(pw.checked_mul(ph).ok_or(Error::TooLarge)?)?;
    for plane in planes {
        restoration::plane(
            plane,
            &mut scratch,
            pw,
            ph,
            pw,
            filter,
            Kernel::Scalar,
            false,
        )
        .map_err(Error::InvalidInput)?;
        core::mem::swap(plane, &mut scratch);
    }
    Ok(())
}

/// Encodes RGB8 sRGB with byte stride. The fixed lab profile has run-level
/// entropy, one 8px block row per stripe, no column groups, AQ or alpha.
/// Quality must be finite in 0..=116; native q0 and q1 have identical steps.
pub fn encode(
    rgb: &[u8],
    stride: usize,
    width: usize,
    height: usize,
    quality: f32,
    color: LossyColor,
    enabled: bool,
) -> Result<Vec<u8>, Error> {
    let (w, h, pw, ph, pixels) = chroma_lab::checked_geometry(width, height)?;
    if !quality.is_finite() || !(0.0..=116.0).contains(&quality) {
        return Err(Error::InvalidInput("Gaborish lab quality"));
    }
    if !matches!(color, LossyColor::Xyb | LossyColor::YCoCg) {
        return Err(Error::Unsupported("Gaborish lab RGB8 color"));
    }
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let row_bytes = width.checked_mul(3).ok_or(Error::TooLarge)?;
    let extent = (height - 1)
        .checked_mul(stride)
        .and_then(|len| len.checked_add(row_bytes))
        .ok_or(Error::InvalidInput("Gaborish lab RGB8 stride"))?;
    if stride < row_bytes {
        return Err(Error::InvalidInput("Gaborish lab RGB8 stride"));
    }
    if rgb.len() < extent {
        return Err(Error::InvalidInput("Gaborish lab RGB8 extent"));
    }
    let mut cfg = imij::ImiEncoderConfig::lossy(90.0);
    cfg = cfg.with_quality(quality);
    cfg = cfg.tune_lossy(|t| {
        t.color = color;
    });
    cfg = cfg.with_threads(6);
    cfg = cfg.tune_lossy(|t| {
        t.stripe_block_rows = 1;
    });
    cfg = cfg.tune_lossy(|t| {
        t.entropy = LossyEntropy::RunLevel;
    });
    cfg = cfg.with_column_groups(Some(0));
    cfg = cfg.tune_lossy(|t| {
        t.lab.lf_pred = 0;
    });
    let inner = if enabled {
        let mut planes = chroma_lab::planes(rgb, stride, width, height, color)?;
        filter_planes(&mut planes, pw, ph, FloatFilter::Inverse)?;
        let (_, steps) = chroma_lab::parameters(quality, color);
        let mut coefficients = zeros(pixels.checked_mul(3).ok_or(Error::TooLarge)?)?;
        let bw = pw / 8;
        for by in 0..ph / 8 {
            for bx in 0..bw {
                let offset = (by * bw + bx) * 192;
                for channel in 0..3 {
                    let block = core::array::from_fn(|i| {
                        planes[channel][(by * 8 + i / 8) * pw + bx * 8 + i % 8]
                    });
                    let forward = chroma_lab::forward8(&block);
                    let quantized = chroma_lab::quantize8(&forward, &steps[channel], cfg.round);
                    coefficients[offset + channel * 64..offset + (channel + 1) * 64]
                        .copy_from_slice(&quantized);
                }
            }
        }
        drop(planes);
        chroma_lab::pack(&coefficients, w, h, quality, color)?
    } else {
        cfg.encode_samples(
            imij::ImageRef::new(rgb, stride, &info),
            &Metadata::default(),
        )?
    };
    let len = inner
        .len()
        .checked_add(HEADER_BYTES)
        .ok_or(Error::TooLarge)?;
    if len > MAX_WRAPPER_BYTES {
        return Err(Error::TooLarge);
    }
    let mut wrapper = Vec::new();
    wrapper
        .try_reserve_exact(len)
        .map_err(|_| Error::TooLarge)?;
    wrapper.extend_from_slice(&SIGNATURE);
    wrapper.extend_from_slice(&[VERSION, u8::from(enabled), 0, 0]);
    wrapper.extend_from_slice(&inner);
    Ok(wrapper)
}

fn profile(wrapper: &[u8]) -> Result<Profile<'_>, Error> {
    if wrapper.len() > MAX_WRAPPER_BYTES {
        return Err(Error::TooLarge);
    }
    let head = wrapper
        .get(..HEADER_BYTES)
        .ok_or(Error::Corrupt("short Gaborish lab header"))?;
    if head[..4] != SIGNATURE {
        return Err(Error::Corrupt("Gaborish lab signature"));
    }
    if head[4] != VERSION {
        return Err(Error::Unsupported("Gaborish lab version"));
    }
    if head[5] > 1 || head[6] != 0 || head[7] != 0 {
        return Err(Error::Corrupt("Gaborish lab flags"));
    }
    let inner = &wrapper[HEADER_BYTES..];
    let header = imij::header(inner)?;
    let info = header.info;
    let width = info.width as usize;
    let height = info.height as usize;
    let (_, _, pw, ph, pixels) = chroma_lab::checked_geometry(width, height)?;
    if header.coding != Coding::Lossy
        || info.channels != Channels::Rgb
        || info.sample != SampleType::U8
        || info.cicp != imij::Cicp::SRGB
        || (info.alpha_mode == imij::AlphaMode::Premultiplied)
        || header.orientation != 1
    {
        return Err(Error::Unsupported("Gaborish lab requires RGB8 sRGB"));
    }
    let (color, steps) = chroma_lab::unpack_parameters(inner)?;
    if !matches!(color, LossyColor::Xyb | LossyColor::YCoCg) {
        return Err(Error::Unsupported("Gaborish lab RGB8 color"));
    }
    Ok(Profile {
        inner,
        info,
        enabled: head[5] != 0,
        color,
        steps,
        pw,
        ph,
        pixels,
    })
}

fn reconstruct(p: &Profile<'_>) -> Result<PaddedPlanes, Error> {
    let coefficients = chroma_lab::unpack(p.inner)?;
    if coefficients.len() != p.pixels.checked_mul(3).ok_or(Error::TooLarge)? {
        return Err(Error::Corrupt("Gaborish lab coefficient extent"));
    }
    let mut planes = [zeros(p.pixels)?, zeros(p.pixels)?, zeros(p.pixels)?];
    let bw = p.pw / 8;
    for by in 0..p.ph / 8 {
        for bx in 0..bw {
            let offset = (by * bw + bx) * 192;
            for channel in 0..3 {
                let block = chroma_lab::inverse8(
                    &coefficients[offset + channel * 64..offset + (channel + 1) * 64],
                    &p.steps[channel],
                );
                for (row, src) in block.as_chunks::<8>().0.iter().enumerate() {
                    let start = (by * 8 + row) * p.pw + bx * 8;
                    planes[channel][start..start + 8].copy_from_slice(src);
                }
            }
        }
    }
    Ok(PaddedPlanes {
        width: p.info.width as usize,
        height: p.info.height as usize,
        padded_width: p.pw,
        padded_height: p.ph,
        stride: p.pw,
        color: p.color,
        planes,
    })
}

/// Exports the exact padded IDCT planes before decoder filtering. Both enabled
/// and off wrappers are accepted; no filter or inverse color is applied.
pub fn pre_gaborish_planes(wrapper: &[u8]) -> Result<PaddedPlanes, Error> {
    reconstruct(&profile(wrapper)?)
}

/// Decodes only this nonrecursive lab wrapper and its restricted native
/// profile. Header-only geometry validation precedes nested entropy parsing.
pub fn decode(wrapper: &[u8]) -> Result<RgbImage, Error> {
    let p = profile(wrapper)?;
    let width = p.info.width as usize;
    let height = p.info.height as usize;
    let stride = width.checked_mul(3).ok_or(Error::TooLarge)?;
    if !p.enabled {
        let (decoded_info, pixels) = imij::decode(p.inner)?;
        if decoded_info != p.info {
            return Err(Error::Corrupt("Gaborish lab decoded geometry"));
        }
        return Ok(RgbImage {
            width,
            height,
            stride,
            pixels,
        });
    }
    let mut planes = reconstruct(&p)?.planes;
    filter_planes(&mut planes, p.pw, p.ph, FloatFilter::Gaborish)?;
    let mut pixels = zeros(stride.checked_mul(height).ok_or(Error::TooLarge)?)?;
    for y in 0..height {
        for x in 0..width {
            let values = core::array::from_fn(|c| planes[c][y * p.pw + x]);
            let rgb = chroma_lab::inverse_color(p.color, values);
            pixels[y * stride + x * 3..y * stride + x * 3 + 3].copy_from_slice(&rgb);
        }
    }
    Ok(RgbImage {
        width,
        height,
        stride,
        pixels,
    })
}
