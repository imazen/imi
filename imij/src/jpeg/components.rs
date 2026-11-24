//! Quantized 8x8 component data, independent of JPEG scan/entropy syntax.

use crate::{Channels, Error, ImageInfo, SampleType};
use alloc::vec::Vec;

/// Meaning of the native centered component values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ComponentColor {
    Gray,
    YCbCr,
}

/// Source sampling declaration. Unspecified JPEG positioning has the same
/// centered reconstruction as Centered. Co-sited data can be held for
/// intermediate use; this first reconstruction policy does not render it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChromaSiting {
    Unspecified,
    Centered,
    Cosited,
}

/// Original component identity and native coded block grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComponentInfo {
    pub id: u8,
    pub horizontal_sampling: u8,
    pub vertical_sampling: u8,
    pub quant_table: u8,
    pub blocks_wide: u32,
    pub blocks_high: u32,
}

/// Signed quantized coefficients in block-row-major, natural frequency order.
/// All 64 coefficients of every coded block, including edge padding, remain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuantizedComponent {
    pub info: ComponentInfo,
    pub coefficients: Vec<i16>,
}

/// Native 8-bit Gray or YCbCr components, with exact quantization tables.
/// This represents coefficient preservation, not byte-exact JPEG recreation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoefficientImage {
    pub width: u32,
    pub height: u32,
    pub color: ComponentColor,
    pub siting: ChromaSiting,
    /// JPEG table slots, 64 unsigned natural-order entries per occupied slot.
    pub tables: [Option<[u16; 64]>; 4],
    /// Source component order (Y, Cb, Cr for YCbCr).
    pub components: Vec<QuantizedComponent>,
}

fn invalid() -> Error {
    Error::InvalidInput("native coefficient component geometry or tables")
}

impl CoefficientImage {
    pub fn image_info(&self) -> ImageInfo {
        ImageInfo::new(
            self.width,
            self.height,
            if self.color == ComponentColor::Gray {
                Channels::Gray
            } else {
                Channels::Rgb
            },
            SampleType::U8,
        )
    }

    /// Checks the source representation without changing a coefficient/table.
    /// Reconstruction supports Gray, 4:4:4 and centered 4:2:0 only.
    pub fn validate(&self) -> Result<(), Error> {
        let count = if self.color == ComponentColor::Gray {
            1
        } else {
            3
        };
        if self.width == 0
            || self.height == 0
            || self.components.len() != count
            || self.image_info().checked_buffer_len().is_none()
        {
            return Err(invalid());
        }
        if self.tables.iter().flatten().any(|t| t.contains(&0)) {
            return Err(Error::InvalidInput("zero native quantization step"));
        }
        let sampling = |i: usize| {
            let p = self.components[i].info;
            (p.horizontal_sampling, p.vertical_sampling)
        };
        if self.components.iter().any(|p| {
            !(1..=4).contains(&p.info.horizontal_sampling)
                || !(1..=4).contains(&p.info.vertical_sampling)
        }) {
            return Err(invalid());
        }
        let valid_sampling = count == 1 || {
            let (y, cb, cr) = (sampling(0), sampling(1), sampling(2));
            cb == cr && (y == cb || y == (cb.0 * 2, cb.1 * 2))
        };
        if !valid_sampling {
            return Err(Error::Unsupported(
                "native coefficient sampling: Gray, 444 or 420",
            ));
        }
        let (max_h, max_v) = sampling(0);
        for (i, component) in self.components.iter().enumerate() {
            let p = component.info;
            if self.components[..i].iter().any(|q| q.info.id == p.id)
                || self
                    .tables
                    .get(p.quant_table as usize)
                    .is_none_or(Option::is_none)
            {
                return Err(invalid());
            }
            let (w, h) = self.native_size_unchecked(i);
            let (bw, bh) = (p.blocks_wide as usize, p.blocks_high as usize);
            // Permit source MCU padding in addition to the visible native grid.
            let max_bw =
                (self.width as u64).div_ceil(8 * max_h as u64) * p.horizontal_sampling as u64;
            let max_bh =
                (self.height as u64).div_ceil(8 * max_v as u64) * p.vertical_sampling as u64;
            if bw < w.div_ceil(8)
                || bh < h.div_ceil(8)
                || bw as u64 > max_bw
                || bh as u64 > max_bh
                || bw.checked_mul(bh).and_then(|n| n.checked_mul(64))
                    != Some(component.coefficients.len())
            {
                return Err(invalid());
            }
        }
        Ok(())
    }

    fn native_size_unchecked(&self, index: usize) -> (usize, usize) {
        let p = self.components[index].info;
        let max = self.components[0].info;
        (
            ((self.width as u64 * p.horizontal_sampling as u64)
                .div_ceil(max.horizontal_sampling as u64)) as usize,
            ((self.height as u64 * p.vertical_sampling as u64)
                .div_ceil(max.vertical_sampling as u64)) as usize,
        )
    }

    /// Visible component dimensions, independent of coded MCU padding.
    pub fn component_size(&self, index: usize) -> Result<(usize, usize), Error> {
        self.validate()?;
        if index >= self.components.len() {
            return Err(invalid());
        }
        Ok(self.native_size_unchecked(index))
    }

    /// Reconstructs native centered f32 samples, clamped to [-128, 127], into
    /// strided rows. The stored coefficients and tables remain unchanged.
    pub fn reconstruct_component_into(
        &self,
        index: usize,
        output: &mut [f32],
        stride: usize,
    ) -> Result<(), Error> {
        let (w, h) = self.component_size(index)?;
        if stride < w
            || (h - 1)
                .checked_mul(stride)
                .and_then(|n| n.checked_add(w))
                .is_none_or(|n| n > output.len())
        {
            return Err(Error::InvalidInput("native component output rows"));
        }
        let component = &self.components[index];
        let table = self.tables[component.info.quant_table as usize]
            .as_ref()
            .unwrap();
        let bw = component.info.blocks_wide as usize;
        for by in 0..h.div_ceil(8) {
            for bx in 0..w.div_ceil(8) {
                let at = (by * bw + bx) * 64;
                let q = component.coefficients[at..at + 64].try_into().unwrap();
                let block = crate::lossy::jpeg::idct(q, table);
                for y in 0..(h - by * 8).min(8) {
                    let dst = &mut output[(by * 8 + y) * stride + bx * 8..];
                    for x in 0..(w - bx * 8).min(8) {
                        dst[x] = block[y * 8 + x].clamp(-128.0, 127.0);
                    }
                }
            }
        }
        Ok(())
    }
}

fn zeroed<T: Clone>(length: usize, value: T) -> Result<Vec<T>, Error> {
    let mut out = Vec::new();
    out.try_reserve_exact(length).map_err(|_| Error::TooLarge)?;
    out.resize(length, value);
    Ok(out)
}

const YCBCR: [[f32; 4]; 3] = [
    [1.0, 0.0, 1.402, 128.0],
    [1.0, -0.344136, -0.714136, 128.0],
    [1.0, 1.772, 0.0, 128.0],
];

impl CoefficientImage {
    /// Reconstructs Gray8/RGB8 into strided rows using the single native
    /// component policy. Explicit co-sited 4:2:0 is unsupported; its native
    /// components and source declaration remain accessible.
    pub fn reconstruct_into(&self, output: &mut [u8], stride: usize) -> Result<ImageInfo, Error> {
        self.validate()?;
        let info = self.image_info();
        let (width, height) = (self.width as usize, self.height as usize);
        let row_bytes = info.row_bytes();
        if stride < row_bytes
            || (height - 1)
                .checked_mul(stride)
                .and_then(|n| n.checked_add(row_bytes))
                .is_none_or(|n| n > output.len())
        {
            return Err(Error::InvalidInput("native coefficient output rows"));
        }
        let sampled = self.components.len() == 3
            && self.components[0].info.horizontal_sampling
                != self.components[1].info.horizontal_sampling;
        if sampled && self.siting == ChromaSiting::Cosited {
            return Err(Error::Unsupported(
                "co-sited native component reconstruction",
            ));
        }
        let mut planes = Vec::new();
        for index in 0..self.components.len() {
            let (w, h) = self.native_size_unchecked(index);
            let mut plane = zeroed(w.checked_mul(h).ok_or(Error::TooLarge)?, 0.0f32)?;
            self.reconstruct_component_into(index, &mut plane, w)?;
            planes.push(plane);
        }
        let mut cb = zeroed(if sampled { width } else { 0 }, 0.0f32)?;
        let mut cr = zeroed(if sampled { width } else { 0 }, 0.0f32)?;
        for y in 0..height {
            if sampled {
                let (cw, ch) = self.native_size_unchecked(1);
                let near = y / 2;
                let far = if y % 2 == 0 {
                    near.saturating_sub(1)
                } else {
                    (near + 1).min(ch - 1)
                };
                for (plane, row) in [(&planes[1], &mut cb), (&planes[2], &mut cr)] {
                    crate::lossy::jpeg::triangle_row(
                        &plane[near * cw..near * cw + cw],
                        &plane[far * cw..far * cw + cw],
                        row,
                    );
                }
            }
            let dst = &mut output[y * stride..y * stride + row_bytes];
            for x in 0..width {
                let i = y * width + x;
                let luma = planes[0][i];
                if self.color == ComponentColor::Gray {
                    dst[x] = crate::lossy::color::inverse_linear_px(
                        &[[1.0, 0.0, 0.0, 128.0]; 3],
                        [1.0; 3],
                        [luma, 0.0, 0.0],
                    )[0];
                } else {
                    let values = [
                        luma,
                        if sampled { cb[x] } else { planes[1][i] },
                        if sampled { cr[x] } else { planes[2][i] },
                    ];
                    let rgb = crate::lossy::color::inverse_linear_px(&YCBCR, [1.0; 3], values);
                    dst[x * 3..x * 3 + 3].copy_from_slice(&rgb);
                }
            }
        }
        Ok(info)
    }
}
