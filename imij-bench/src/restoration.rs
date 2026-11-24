//! Experimental restoration kernels. Seam filtering uses SDR RGB8/RGBA8;
//! Gaborish operates on planar f32 XYB BEFORE conversion to output RGB.
//! Callers own all image/row storage. There is no full-image kernel scratch.
//! AVX2 is explicit, never silently promoted to AVX-512. FMA is canonical for
//! the floating-point filters; integer seam results match the historical oracle.
#![forbid(unsafe_code)]
// Kernels spell out stride/halo geometry and fixed stencil indices.
#![allow(clippy::too_many_arguments, clippy::needless_range_loop)]

use archmage::prelude::*;
use rayon::prelude::*;
#[path = "postfilter.rs"]
pub(crate) mod historical;
pub(crate) use historical::{Layout, Strength};

#[derive(Clone, Copy, Debug)]
pub(crate) enum Kernel {
    Scalar,
    /// Scalar source loop with the same hardware FMA feature context.
    ScalarFma(X64V3Token),
    Avx2(X64V3Token),
    Avx2Packed(X64V3Token),
}
impl Kernel {
    pub(crate) fn available() -> Vec<Self> {
        let mut out = vec![Self::Scalar];
        if let Some(t) = X64V3Token::summon() {
            out.push(Self::Avx2(t));
            out.push(Self::Avx2Packed(t));
        }
        out
    }
    pub(crate) fn float_available() -> Vec<Self> {
        let mut out = vec![Self::Scalar];
        if let Some(t) = X64V3Token::summon() {
            out.push(Self::ScalarFma(t));
            out.push(Self::Avx2(t));
        }
        out
    }
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Scalar => "scalar",
            Self::ScalarFma(_) => "scalar-fma",
            Self::Avx2(_) => "avx2",
            Self::Avx2Packed(_) => "avx2-packed",
        }
    }
}

fn parameters(strength: Strength) -> (i32, i32, i32) {
    match strength {
        Strength::Off => (0, 0, 0),
        Strength::Mild => (1, 16, 2),
        Strength::Medium => (2, 24, 4),
        Strength::Strong => (3, 32, 6),
    }
}

pub(crate) fn seam_filter(
    pixels: &mut [u8],
    l: Layout,
    s: Strength,
    kernel: Kernel,
    parallel: bool,
) -> Result<(), &'static str> {
    l.validate(pixels.len())?;
    if s == Strength::Off || l.width == 0 || l.height == 0 {
        return Ok(());
    }
    if matches!(kernel, Kernel::Scalar | Kernel::ScalarFma(_)) && !parallel {
        return historical::apply(pixels, l, s);
    }
    match l.channels {
        3 => passes::<3>(pixels, l, s, kernel, parallel),
        4 => passes::<4>(pixels, l, s, kernel, parallel),
        _ => unreachable!(),
    }
    Ok(())
}

fn passes<const C: usize>(
    pixels: &mut [u8],
    l: Layout,
    s: Strength,
    kernel: Kernel,
    parallel: bool,
) {
    let extent = (l.height - 1) * l.stride_bytes + l.width * C;
    let pixels = &mut pixels[..extent];
    let vertical = |row: &mut [u8]| {
        let first = (8 - l.origin.0 % 8) % 8;
        let first = first + if first < 2 { 8 } else { 0 };
        let count = if first + 1 < l.width {
            (l.width - 2 - first) / 8 + 1
        } else {
            0
        };
        run::<C, false>(row, first * C, 8 * C, C, count, parameters(s), kernel);
    };
    if parallel {
        pixels.par_chunks_mut(l.stride_bytes).for_each(vertical);
    } else {
        pixels.chunks_mut(l.stride_bytes).for_each(vertical);
    }
    // The completed Rayon call is the pass barrier. Horizontal four-row halos
    // are disjoint: seam positions are eight rows apart, so mutable bands need
    // neither copying nor locks, and both schedules reproduce the scalar order.
    let first = (8 - l.origin.1 % 8) % 8;
    let first = first + if first < 2 { 8 } else { 0 };
    if first + 1 >= l.height {
        return;
    }
    let horizontal = |rows: &mut [u8]| {
        if rows.len() < 3 * l.stride_bytes + l.width * C {
            return;
        }
        run::<C, true>(
            rows,
            2 * l.stride_bytes,
            C,
            l.stride_bytes,
            l.width,
            parameters(s),
            kernel,
        );
    };
    let rows = &mut pixels[(first - 2) * l.stride_bytes..];
    if parallel {
        rows.par_chunks_mut(8 * l.stride_bytes).for_each(horizontal);
    } else {
        rows.chunks_mut(8 * l.stride_bytes).for_each(horizontal);
    }
}

// Eight seam positions: contiguous pixels for horizontal seams and bounded
// interleaved tiles for vertical seams. No hardware gather API is required.
fn run<const C: usize, const HORIZONTAL: bool>(
    p: &mut [u8],
    start: usize,
    step: usize,
    neighbor: usize,
    count: usize,
    params: (i32, i32, i32),
    kernel: Kernel,
) {
    match kernel {
        Kernel::Scalar | Kernel::ScalarFma(_) => {
            scalar_run::<C>(p, start, step, neighbor, count, params)
        }
        Kernel::Avx2(t) => {
            #[cfg(target_arch = "x86_64")]
            seam_v3::<C, false, HORIZONTAL>(t, p, start, step, neighbor, count, params);
            #[cfg(not(target_arch = "x86_64"))]
            {
                let _ = t;
                unreachable!("AVX2 token unavailable on this architecture");
            }
        }
        Kernel::Avx2Packed(t) => {
            #[cfg(target_arch = "x86_64")]
            seam_v3::<C, true, HORIZONTAL>(t, p, start, step, neighbor, count, params);
            #[cfg(not(target_arch = "x86_64"))]
            {
                let _ = t;
                unreachable!();
            }
        }
    }
}

fn scalar_run<const C: usize>(
    p: &mut [u8],
    start: usize,
    step: usize,
    neighbor: usize,
    count: usize,
    params: (i32, i32, i32),
) {
    let (weight, gate, cap) = params;
    for i in 0..count {
        let q = start + i * step;
        let offsets = [q - 2 * neighbor, q - neighbor, q, q + neighbor];
        if C == 4 && offsets.iter().any(|&o| p[o + 3] != p[offsets[0] + 3]) {
            continue;
        }
        let mut residual = [0; 3];
        let mut accepted = true;
        for c in 0..3 {
            let [a, b, d, e] = offsets.map(|o| i32::from(p[o + c]));
            accepted &=
                (d - b).abs() <= gate && (b - a).abs() <= gate / 2 && (e - d).abs() <= gate / 2;
            residual[c] = 2 * (d - b) - (b - a) - (e - d);
        }
        if !accepted {
            continue;
        }
        for c in 0..3 {
            let jump = i32::from(p[q + c]) - i32::from(p[q - neighbor + c]);
            if residual[c].signum() != jump.signum() {
                continue;
            }
            let scaled = residual[c] * weight;
            let bound = cap.min(jump.abs() / 2);
            let delta = (scaled.signum() * ((scaled.abs() + 8) / 16)).clamp(-bound, bound);
            p[q - neighbor + c] = (i32::from(p[q - neighbor + c]) + delta).clamp(0, 255) as u8;
            p[q + c] = (i32::from(p[q + c]) - delta).clamp(0, 255) as u8;
        }
    }
}

#[magetypes(define(i32x8), v3)]
#[allow(dead_code, clippy::too_many_arguments)]
fn seam<const C: usize, const PACKED: bool, const HORIZONTAL: bool>(
    token: Token,
    p: &mut [u8],
    start: usize,
    step: usize,
    neighbor: usize,
    count: usize,
    params: (i32, i32, i32),
) {
    let (weight, gate, cap) = params;
    let step = if PACKED {
        if HORIZONTAL { C } else { 8 * C }
    } else {
        step
    };
    let neighbor = if PACKED && !HORIZONTAL { C } else { neighbor };
    let zero = i32x8::splat(token, 0);
    let bytes = i32x8::splat(token, 255);
    let alpha_mask = i32x8::splat(token, if C == 4 { 0xff000000u32 as i32 } else { 0 });
    let ones = i32x8::splat(token, -1);
    let gate = i32x8::splat(token, gate);
    let side = gate.shr_arithmetic_const::<1>();
    let cap = i32x8::splat(token, cap);
    let max = i32x8::splat(token, 255);
    let bulk = count / 8 * 8;
    for i in (0..bulk).step_by(8) {
        let q = core::array::from_fn::<_, 8, _>(|lane| start + (i + lane) * step);
        let packed: [i32x8; 4] = if PACKED {
            core::array::from_fn(|j| {
                let first = q[0] + j * neighbor - 2 * neighbor;
                let tile = &p[first..q[7] + j * neighbor - 2 * neighbor + C];
                i32x8::from_array(
                    token,
                    core::array::from_fn(|lane| {
                        let px = &tile[lane * step..lane * step + C];
                        i32::from_le_bytes([px[0], px[1], px[2], if C == 4 { px[3] } else { 0 }])
                    }),
                )
            })
        } else {
            [zero; 4]
        };
        let load = |offset: usize, c: usize| {
            if PACKED {
                packed[offset / neighbor].shr_logical_uniform((c * 8) as u32) & bytes
            } else {
                i32x8::from_array(
                    token,
                    q.map(|v| i32::from(p[v + offset - 2 * neighbor + c])),
                )
            }
        };
        let mut valid = ones;
        if C == 4 {
            let alpha = load(0, 3);
            valid = load(neighbor, 3).simd_eq(alpha)
                & load(2 * neighbor, 3).simd_eq(alpha)
                & load(3 * neighbor, 3).simd_eq(alpha);
        }
        let samples: [[i32x8; 4]; 3] =
            core::array::from_fn(|c| core::array::from_fn(|j| load(j * neighbor, c)));
        for [a, b, d, e] in samples {
            valid = valid
                & (d - b).abs().simd_le(gate)
                & (b - a).abs().simd_le(side)
                & (e - d).abs().simd_le(side);
        }
        if PACKED && !valid.any_true() {
            continue;
        }
        let mut packed_left = packed[1] & alpha_mask;
        let mut packed_right = packed[2] & alpha_mask;
        for (c, [a, b, d, e]) in samples.into_iter().enumerate() {
            let jump = d - b;
            let residual = jump * 2 - (b - a) - (e - d);
            let same_sign = residual.simd_gt(zero).simd_eq(jump.simd_gt(zero))
                & residual.simd_lt(zero).simd_eq(jump.simd_lt(zero));
            let scaled = residual * weight;
            let sign = scaled.shr_arithmetic_const::<31>();
            let magnitude = (scaled.abs() + 8).shr_arithmetic_const::<4>();
            let bound = jump.abs().shr_arithmetic_const::<1>().min(cap);
            let delta = ((magnitude ^ sign) - sign).max(-bound).min(bound) & valid & same_sign;
            let left = (b + delta).max(zero).min(max);
            let right = (d - delta).max(zero).min(max);
            if PACKED {
                packed_left |= left.shl_uniform((c * 8) as u32);
                packed_right |= right.shl_uniform((c * 8) as u32);
            } else {
                let left = left.to_array();
                let right = right.to_array();
                for lane in 0..8 {
                    p[q[lane] - neighbor + c] = left[lane] as u8;
                    p[q[lane] + c] = right[lane] as u8;
                }
            }
        }
        if PACKED {
            let left = packed_left.to_array();
            let right = packed_right.to_array();
            for lane in 0..8 {
                p[q[lane] - neighbor..q[lane] - neighbor + C]
                    .copy_from_slice(&left[lane].to_le_bytes()[..C]);
                p[q[lane]..q[lane] + C].copy_from_slice(&right[lane].to_le_bytes()[..C]);
            }
        }
    }
    scalar_run::<C>(p, start + bulk * step, step, neighbor, count - bulk, params);
}

// Gaborish algorithm/constants derived from libjxl enc_gaborish.cc and
// render_pipeline/stage_gaborish.cc, https://github.com/libjxl/libjxl.
// Copyright (c) the JPEG XL Project Authors. All rights reserved.
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions are met:
// 1. Redistributions of source code must retain the above copyright notice,
//    this list of conditions and the following disclaimer.
// 2. Redistributions in binary form must reproduce the above copyright notice,
//    this list of conditions and the following disclaimer in the documentation
//    and/or other materials provided with the distribution.
// 3. Neither the name of the copyright holder nor the names of its contributors
//    may be used to endorse or promote products derived from this software
//    without specific prior written permission.
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
// AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
// IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
// ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE
// LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
// CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
// SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
// INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
// CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
// ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
// POSSIBILITY OF SUCH DAMAGE.

#[derive(Clone, Copy, Debug)]
pub(crate) enum FloatFilter {
    Gaborish,
    Inverse,
}
impl FloatFilter {
    pub(crate) fn radius(self) -> usize {
        match self {
            Self::Gaborish => 1,
            Self::Inverse => 2,
        }
    }
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Gaborish => "gaborish",
            Self::Inverse => "inverse",
        }
    }
}

fn weights(filter: FloatFilter) -> [f32; 6] {
    match filter {
        FloatFilter::Gaborish => {
            let a = 0.115_169_525f32;
            let d = 0.061_248_592f32;
            let total = 1.0 + a * 4.0 + d * 4.0;
            [1.0 / total, a / total, d / total, 0.0, 0.0, 0.0]
        }
        FloatFilter::Inverse => {
            let k = [
                -0.094_958_16f32,
                -0.041_031_726,
                0.013_710_005,
                0.006_510_206,
                -0.001_478_906_3,
            ];
            let product = 4.0 * (k[0] + k[1] + k[2] + k[4] + 2.0 * k[3]);
            let normal = (1.0 / (1.0f64 + f64::from(product))) as f32;
            [
                normal,
                normal * k[0],
                normal * k[1],
                normal * k[2],
                normal * k[3],
                normal * k[4],
            ]
        }
    }
}

/// Exactly three rows, each output.len()+2 samples including supplied halos.
/// For streaming, retain three original rows; never overwrite an input row
/// before its final dependent output. Rows are one planar XYB channel.
pub(crate) fn gaborish_row(
    rows: [&[f32]; 3],
    out: &mut [f32],
    kernel: Kernel,
) -> Result<(), &'static str> {
    row(
        &[rows[0], rows[1], rows[2]],
        out,
        FloatFilter::Gaborish,
        kernel,
    )
}
/// Five original XYB rows with two halo samples on each horizontal side.
/// This is the JXL approximate encoder prefilter, not an exact inverse.
pub(crate) fn inverse_row(
    rows: [&[f32]; 5],
    out: &mut [f32],
    kernel: Kernel,
) -> Result<(), &'static str> {
    row(&rows, out, FloatFilter::Inverse, kernel)
}
fn row(
    rows: &[&[f32]],
    out: &mut [f32],
    filter: FloatFilter,
    kernel: Kernel,
) -> Result<(), &'static str> {
    let r = filter.radius();
    let size = out.len().checked_add(2 * r).ok_or("row extent overflow")?;
    if rows.len() != 2 * r + 1 || rows.iter().any(|row| row.len() != size) {
        return Err("invalid halo row extent");
    }
    let w = weights(filter);
    match kernel {
        Kernel::Scalar => {
            for (x, v) in out.iter_mut().enumerate() {
                *v = float_pixel(rows, x + r, r, &w);
            }
        }
        Kernel::ScalarFma(t) => {
            #[cfg(target_arch = "x86_64")]
            scalar_float_fma(t, rows, out, r, w);
            #[cfg(not(target_arch = "x86_64"))]
            {
                let _ = t;
                unreachable!();
            }
        }
        Kernel::Avx2(t) | Kernel::Avx2Packed(t) => {
            #[cfg(target_arch = "x86_64")]
            match filter {
                FloatFilter::Gaborish => float_row_v3::<1>(t, rows, out, w),
                FloatFilter::Inverse => float_row_v3::<2>(t, rows, out, w),
            }
            #[cfg(not(target_arch = "x86_64"))]
            {
                let _ = t;
                unreachable!();
            }
        }
    }
    Ok(())
}

// Scalar-source matched-feature control. The compiler may autovectorize; the
// frozen assembly records whether it does. Baseline Scalar remains the oracle.
#[cfg(target_arch = "x86_64")]
#[arcane]
fn scalar_float_fma(
    _token: X64V3Token,
    rows: &[&[f32]],
    out: &mut [f32],
    radius: usize,
    weights: [f32; 6],
) {
    for (x, value) in out.iter_mut().enumerate() {
        *value = float_pixel(rows, x + radius, radius, &weights);
    }
}

#[inline(always)]
fn float_pixel(rows: &[&[f32]], x: usize, r: usize, w: &[f32; 6]) -> f32 {
    let center = rows[r][x] * w[0];
    let axial = ((rows[r - 1][x] + rows[r][x - 1]) + rows[r + 1][x]) + rows[r][x + 1];
    let diag =
        ((rows[r - 1][x - 1] + rows[r - 1][x + 1]) + rows[r + 1][x - 1]) + rows[r + 1][x + 1];
    let result = w[2].mul_add(diag, w[1].mul_add(axial, center));
    if r == 1 {
        return result;
    }
    let far = ((rows[0][x] + rows[2][x - 2]) + rows[4][x]) + rows[2][x + 2];
    let knight = (((rows[0][x - 1] + rows[0][x + 1]) + (rows[1][x - 2] + rows[1][x + 2]))
        + (rows[3][x - 2] + rows[3][x + 2]))
        + (rows[4][x - 1] + rows[4][x + 1]);
    let corners = ((rows[0][x - 2] + rows[0][x + 2]) + rows[4][x - 2]) + rows[4][x + 2];
    w[5].mul_add(corners, w[4].mul_add(knight, w[3].mul_add(far, result)))
}

#[magetypes(define(f32x8), v3)]
#[allow(dead_code)]
fn float_row<const R: usize>(token: Token, rows: &[&[f32]], out: &mut [f32], w: [f32; 6]) {
    let weights = w.map(|v| f32x8::splat(token, v));
    let bulk = out.len() / 8 * 8;
    for x in (0..bulk).step_by(8) {
        let p = |y: usize, dx: usize| {
            f32x8::load(token, rows[y][x + dx..x + dx + 8].try_into().unwrap())
        };
        let center = p(R, R) * weights[0];
        let axial = ((p(R - 1, R) + p(R, R - 1)) + p(R + 1, R)) + p(R, R + 1);
        let diag = ((p(R - 1, R - 1) + p(R - 1, R + 1)) + p(R + 1, R - 1)) + p(R + 1, R + 1);
        let mut result =
            weights[2].mul_add_relaxed(diag, weights[1].mul_add_relaxed(axial, center));
        if R == 2 {
            let far = ((p(0, 2) + p(2, 0)) + p(4, 2)) + p(2, 4);
            let knight = (((p(0, 1) + p(0, 3)) + (p(1, 0) + p(1, 4))) + (p(3, 0) + p(3, 4)))
                + (p(4, 1) + p(4, 3));
            let corners = ((p(0, 0) + p(0, 4)) + p(4, 0)) + p(4, 4);
            result = weights[5].mul_add_relaxed(
                corners,
                weights[4].mul_add_relaxed(knight, weights[3].mul_add_relaxed(far, result)),
            );
        }
        result.store((&mut out[x..x + 8]).try_into().unwrap());
    }
    for x in bulk..out.len() {
        out[x] = float_pixel(rows, x + R, R, &w);
    }
}

/// Reflect at the outer pixel edge (..1,0 | 0,1,..n-1 | n-1,n-2..).
/// Radius <=2; the loop also handles one-pixel dimensions correctly.
pub(crate) fn mirror(mut x: isize, n: usize) -> usize {
    debug_assert!(n > 0);
    let n = n as isize;
    while x < 0 || x >= n {
        if x < 0 {
            x = -x - 1;
        } else {
            x = 2 * n - 1 - x;
        }
    }
    x as usize
}

/// Caller-owned halo rows are the streaming API. This convenience wrapper uses
/// fixed stack tiles (8 output samples + up to4 halo), no full-row/image scratch.
/// Input/output strides are f32 samples, and image edges are mirrored.
pub(crate) fn plane(
    input: &[f32],
    output: &mut [f32],
    width: usize,
    height: usize,
    stride: usize,
    filter: FloatFilter,
    kernel: Kernel,
    parallel: bool,
) -> Result<(), &'static str> {
    let end = if width == 0 || height == 0 {
        0
    } else {
        (height - 1)
            .checked_mul(stride)
            .and_then(|n| n.checked_add(width))
            .ok_or("plane extent overflow")?
    };
    if stride < width
        || end > input.len()
        || end > output.len()
        || width > isize::MAX as usize
        || height > isize::MAX as usize
    {
        return Err("invalid plane geometry");
    }
    if end == 0 {
        return Ok(());
    }
    let r = filter.radius();
    let process = |(y, out): (usize, &mut [f32])| {
        let mut edges = [[0.0; 12]; 5];
        // Bulk row spans have no copies. Only left/right image edges use the
        // bounded stack tile to synthesize their mirror halo.
        let rows: [&[f32]; 5] = core::array::from_fn(|dy| {
            let yy = mirror(y as isize + dy as isize - r as isize, height);
            &input[yy * stride..yy * stride + width]
        });
        let left = r.min(width);
        let right = width.saturating_sub(r).max(left);
        for x in 0..left {
            for dy in 0..2 * r + 1 {
                for dx in 0..2 * r + 1 {
                    edges[dy][dx] = rows[dy][mirror(x as isize + dx as isize - r as isize, width)];
                }
            }
            let views: [&[f32]; 5] = core::array::from_fn(|i| &edges[i][..2 * r + 1]);
            out[x] = float_pixel(&views[..2 * r + 1], r, r, &weights(filter));
        }
        if right > left {
            let views: [&[f32]; 5] = core::array::from_fn(|i| &rows[i][left - r..right + r]);
            row(&views[..2 * r + 1], &mut out[left..right], filter, kernel).unwrap();
        }
        for x in right..width {
            for dy in 0..2 * r + 1 {
                for dx in 0..2 * r + 1 {
                    edges[dy][dx] = rows[dy][mirror(x as isize + dx as isize - r as isize, width)];
                }
            }
            let views: [&[f32]; 5] = core::array::from_fn(|i| &edges[i][..2 * r + 1]);
            out[x] = float_pixel(&views[..2 * r + 1], r, r, &weights(filter));
        }
    };
    if parallel {
        output[..end]
            .par_chunks_mut(stride)
            .enumerate()
            .for_each(process);
    } else {
        output[..end]
            .chunks_mut(stride)
            .enumerate()
            .for_each(process);
    }
    Ok(())
}
