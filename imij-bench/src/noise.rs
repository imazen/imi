//! Decoder-only experiment: integer noise over interleaved SDR sRGB code values.
//! RGBA is straight alpha; all RGB (including hidden RGB) is processed while
//! alpha, row padding, and trailing bytes are preserved. Stride is in bytes.
//! Absolute origins key noise, so cropped/striped calls need no halo or state.
//! No encoder, estimation, bitstream syntax, library PRNG, or image scratch.
#![forbid(unsafe_code)]

use archmage::prelude::*;
use rayon::prelude::*;

/// Explicit experiment tier. No hidden AVX-512 promotion or global race.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Kernel {
    Scalar,
    Avx2(X64V3Token),
}

impl Kernel {
    pub(crate) fn available() -> Vec<Self> {
        let mut kernels = vec![Self::Scalar];
        if let Some(token) = X64V3Token::summon() {
            kernels.push(Self::Avx2(token));
        }
        kernels
    }
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Scalar => "scalar",
            Self::Avx2(_) => "avx2",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Variant {
    Shared,
    Rgb,
    Template,
}

impl Variant {
    pub(crate) const ALL: [Self; 3] = [Self::Shared, Self::Rgb, Self::Template];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::Rgb => "rgb",
            Self::Template => "template",
        }
    }

    pub(crate) fn parse(name: &str) -> Result<Self, &'static str> {
        Self::ALL
            .into_iter()
            .find(|v| v.name() == name)
            .ok_or("unknown noise variant")
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Parameters {
    seed: u32,
    amplitude: [u8; 256],
}

impl Parameters {
    // Amplitude is a code-value delta bound, not a fitted standard deviation.
    // LUT has nonzero endpoints and a triangular peak at luma 127/128.
    pub(crate) fn new(seed: u32, low: u8, high: u8) -> Result<Self, &'static str> {
        if low > high || high > 32 {
            return Err("amplitude requires 0 <= low <= high <= 32");
        }
        let mut amplitude = [0; 256];
        for (i, a) in amplitude.iter_mut().enumerate() {
            *a = low + ((usize::from(high - low) * i.min(255 - i) + 63) / 127) as u8;
        }
        Ok(Self { seed, amplitude })
    }

    pub(crate) fn fixed() -> Self {
        Self::new(0x6d2b_79f5, 4, 12).expect("bounded fixed parameters")
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Layout {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) stride_bytes: usize,
    pub(crate) channels: usize,
    /// Absolute pixel coordinate of the top-left sample, each <= u32::MAX.
    pub(crate) origin: (usize, usize),
}

impl Layout {
    /// Returns the extent through the last pixel, excluding last-row padding.
    pub(crate) fn validate(self, len: usize) -> Result<usize, &'static str> {
        if ![3, 4].contains(&self.channels) {
            return Err("requires RGB8 or straight RGBA8");
        }
        let row = self
            .width
            .checked_mul(self.channels)
            .ok_or("row overflow")?;
        if self.stride_bytes < row {
            return Err("stride smaller than pixel row");
        }
        for (start, count) in [(self.origin.0, self.width), (self.origin.1, self.height)] {
            let last = start
                .checked_add(count.saturating_sub(1))
                .ok_or("coordinate overflow")?;
            if last > u32::MAX as usize {
                return Err("absolute coordinate exceeds u32 range");
            }
        }
        let end = if self.width == 0 || self.height == 0 {
            0
        } else {
            (self.height - 1)
                .checked_mul(self.stride_bytes)
                .and_then(|v| v.checked_add(row))
                .ok_or("image extent overflow")?
        };
        if end > len {
            return Err("pixel buffer too short");
        }
        Ok(end)
    }
}

// Two multiplies avalanche a coordinate-keyed word; wrapping is specified.
// This is synthetic noise, not a cryptographic or statistical PRNG guarantee.
#[inline]
fn word(x: u32, y: u32, seed: u32) -> u32 {
    let mut v = x.wrapping_mul(0x9e37_79b1) ^ y.wrapping_mul(0x85eb_ca77) ^ seed;
    v ^= v >> 16;
    v = v.wrapping_mul(0x7feb_352d);
    v ^= v >> 15;
    v = v.wrapping_mul(0x846c_a68b);
    v ^ (v >> 16)
}

#[inline]
fn sample(v: u32) -> i32 {
    // Odd signed values [-127,127], exactly symmetric, excluding zero.
    2 * (v & 127) as i32 - 127
}

#[inline]
fn delta(n: i32, amplitude: u8) -> i32 {
    // Round nearest, ties away from zero; entirely specified integer math.
    n.signum() * ((n.abs() * i32::from(amplitude) + 64) >> 7)
}

#[inline]
fn pixel<const MODE: u8>(p: &mut [u8], x: u32, y: u32, params: &Parameters) {
    let luma =
        (77 * usize::from(p[0]) + 150 * usize::from(p[1]) + 29 * usize::from(p[2]) + 128) >> 8;
    let amplitude = params.amplitude[luma];
    let bits = word(x, y, params.seed);
    let common = sample(bits);
    let shared = if MODE == 2 {
        // Half white sample + half shared 2x2 lattice sample, no neighbor reads.
        (common + sample(word(x >> 1, y >> 1, params.seed ^ 0xa511_e9b3))) / 2
    } else {
        common
    };
    for (c, value) in p[..3].iter_mut().enumerate() {
        let n = if MODE == 1 {
            // Common and channel-specific 7-bit fields from one hashed word.
            (2 * common + sample(bits >> (7 * (c + 1)))) / 3
        } else {
            shared
        };
        *value = (i32::from(*value) + delta(n, amplitude)).clamp(0, 255) as u8;
    }
}

// Keep the repository's chunk iteration convention instead of adopting the
// newer as_chunks syntax solely to satisfy the Rust 1.99 preference lint.
#[allow(clippy::chunks_exact_to_as_chunks)]
fn rows<const CHANNELS: usize, const MODE: u8>(pixels: &mut [u8], l: Layout, params: &Parameters) {
    for (y, row) in pixels.chunks_mut(l.stride_bytes).enumerate() {
        for (x, p) in row[..l.width * CHANNELS]
            .chunks_exact_mut(CHANNELS)
            .enumerate()
        {
            pixel::<MODE>(p, (l.origin.0 + x) as u32, (l.origin.1 + y) as u32, params);
        }
    }
}

fn dispatch<const CHANNELS: usize>(pixels: &mut [u8], l: Layout, p: &Parameters, v: Variant) {
    match v {
        Variant::Shared => rows::<CHANNELS, 0>(pixels, l, p),
        Variant::Rgb => rows::<CHANNELS, 1>(pixels, l, p),
        Variant::Template => rows::<CHANNELS, 2>(pixels, l, p),
    }
}

pub(crate) fn apply(
    pixels: &mut [u8],
    l: Layout,
    p: &Parameters,
    v: Variant,
) -> Result<(), &'static str> {
    let end = l.validate(pixels.len())?;
    if end == 0 {
        return Ok(());
    }
    match l.channels {
        3 => dispatch::<3>(&mut pixels[..end], l, p, v),
        4 => dispatch::<4>(&mut pixels[..end], l, p, v),
        _ => unreachable!("validated channels"),
    }
    Ok(())
}

/// Runs on the current Rayon pool; caller owns and installs the decode pool.
/// Stripe scheduling uses disjoint output slices; synthesis has no image scratch.
/// Rayon task bookkeeping is not claimed allocation-free. No decode fusion.
pub(crate) fn apply_parallel(
    pixels: &mut [u8],
    l: Layout,
    p: &Parameters,
    v: Variant,
    stripe_rows: usize,
) -> Result<(), &'static str> {
    let end = l.validate(pixels.len())?;
    if !(1..=4096).contains(&stripe_rows) {
        return Err("stripe rows requires 1..=4096");
    }
    if end == 0 {
        return Ok(());
    }
    let stripe_rows = stripe_rows.min(l.height);
    let chunk_bytes = l
        .stride_bytes
        .checked_mul(stripe_rows)
        .ok_or("stripe extent overflow")?;
    pixels[..end]
        .par_chunks_mut(chunk_bytes)
        .enumerate()
        .for_each(|(i, chunk)| {
            let stripe = Layout {
                origin: (l.origin.0, l.origin.1 + i * stripe_rows),
                ..l
            };
            match l.channels {
                3 => dispatch::<3>(chunk, stripe, p, v),
                4 => dispatch::<4>(chunk, stripe, p, v),
                _ => unreachable!("validated channels"),
            }
        });
    Ok(())
}

/// Select a tier explicitly. Scalar retains the original frozen implementation.
pub(crate) fn apply_kernel(
    pixels: &mut [u8],
    l: Layout,
    p: &Parameters,
    v: Variant,
    kernel: Kernel,
) -> Result<(), &'static str> {
    if let Kernel::Scalar = kernel {
        return apply(pixels, l, p, v);
    }
    let end = l.validate(pixels.len())?;
    if end == 0 {
        return Ok(());
    }
    dispatch_kernel(&mut pixels[..end], l, p, v, kernel);
    Ok(())
}

pub(crate) fn apply_parallel_kernel(
    pixels: &mut [u8],
    l: Layout,
    p: &Parameters,
    v: Variant,
    stripe_rows: usize,
    kernel: Kernel,
) -> Result<(), &'static str> {
    if let Kernel::Scalar = kernel {
        return apply_parallel(pixels, l, p, v, stripe_rows);
    }
    let end = l.validate(pixels.len())?;
    if !(1..=4096).contains(&stripe_rows) {
        return Err("stripe rows requires 1..=4096");
    }
    if end == 0 {
        return Ok(());
    }
    let stripe_rows = stripe_rows.min(l.height);
    let chunk = l
        .stride_bytes
        .checked_mul(stripe_rows)
        .ok_or("stripe extent overflow")?;
    pixels[..end]
        .par_chunks_mut(chunk)
        .enumerate()
        .for_each(|(i, pixels)| {
            dispatch_kernel(
                pixels,
                Layout {
                    origin: (l.origin.0, l.origin.1 + i * stripe_rows),
                    ..l
                },
                p,
                v,
                kernel,
            );
        });
    Ok(())
}

fn dispatch_kernel(pixels: &mut [u8], l: Layout, p: &Parameters, v: Variant, kernel: Kernel) {
    match kernel {
        Kernel::Scalar => match l.channels {
            3 => dispatch::<3>(pixels, l, p, v),
            4 => dispatch::<4>(pixels, l, p, v),
            _ => unreachable!("validated channels"),
        },
        Kernel::Avx2(token) => {
            #[cfg(target_arch = "x86_64")]
            match (l.channels, v) {
                (3, Variant::Shared) => rows_simd_v3::<3, 0>(token, pixels, l, p),
                (3, Variant::Rgb) => rows_simd_v3::<3, 1>(token, pixels, l, p),
                (3, Variant::Template) => rows_simd_v3::<3, 2>(token, pixels, l, p),
                (4, Variant::Shared) => rows_simd_v3::<4, 0>(token, pixels, l, p),
                (4, Variant::Rgb) => rows_simd_v3::<4, 1>(token, pixels, l, p),
                (4, Variant::Template) => rows_simd_v3::<4, 2>(token, pixels, l, p),
                _ => unreachable!("validated channels"),
            }
            #[cfg(not(target_arch = "x86_64"))]
            {
                let _ = token;
                unreachable!("V3 token cannot be summoned off x86_64");
            }
        }
    }
}

// Eight pixels per stack tile. Scalar interleaved load/LUT/store surrounding
// vector hash, luma, correlation, signed rounding and saturating code values.
// One const-generic kernel; only the explicit AVX2 entry is generated.
#[magetypes(define(u32x8, i32x8), v3)]
#[allow(clippy::chunks_exact_to_as_chunks)]
// The macro also generates a scalar vector fallback; the historical scalar
// loop is the explicit reference tier instead, so that generated entry is unused.
#[allow(dead_code)]
fn rows_simd<const CHANNELS: usize, const MODE: u8>(
    token: Token,
    pixels: &mut [u8],
    l: Layout,
    p: &Parameters,
) {
    let hash = |x: u32x8, y: u32x8, seed: u32| {
        let mut v = (x * u32x8::splat(token, 0x9e3779b1))
            ^ (y * u32x8::splat(token, 0x85ebca77))
            ^ u32x8::splat(token, seed);
        v = (v ^ v.shr_logical_const::<16>()) * u32x8::splat(token, 0x7feb352d);
        v = (v ^ v.shr_logical_const::<15>()) * u32x8::splat(token, 0x846ca68b);
        v ^ v.shr_logical_const::<16>()
    };
    let signed = |bits: u32x8| (bits & u32x8::splat(token, 127)).bitcast_to_i32() * 2 - 127;
    let zero = i32x8::splat(token, 0);
    let max = i32x8::splat(token, 255);
    for (y, row) in pixels.chunks_mut(l.stride_bytes).enumerate() {
        let y = u32x8::splat(token, (l.origin.1 + y) as u32);
        let bulk = l.width / 8 * 8;
        let (tiles, tail) = row[..l.width * CHANNELS].split_at_mut(bulk * CHANNELS);
        for (block, tile) in tiles.chunks_exact_mut(8 * CHANNELS).enumerate() {
            let first = l.origin.0 + block * 8;
            let x = u32x8::from_array(token, core::array::from_fn(|lane| (first + lane) as u32));
            let rgb: [i32x8; 3] = core::array::from_fn(|c| {
                i32x8::from_array(
                    token,
                    core::array::from_fn(|lane| i32::from(tile[lane * CHANNELS + c])),
                )
            });
            let luma = (rgb[0] * 77 + rgb[1] * 150 + rgb[2] * 29 + 128)
                .shr_arithmetic_const::<8>()
                .to_array();
            let amplitude =
                i32x8::from_array(token, luma.map(|i| i32::from(p.amplitude[i as usize])));
            let bits = hash(x, y, p.seed);
            let common = signed(bits);
            let shared = if MODE == 2 {
                let n = common
                    + signed(hash(
                        x.shr_logical_const::<1>(),
                        y.shr_logical_const::<1>(),
                        p.seed ^ 0xa511e9b3,
                    ));
                // Arithmetic shift floors negatives; add one for negative odd
                // inputs to reproduce division toward zero (also safe for zero).
                (n + n.shr_logical_const::<31>()).shr_arithmetic_const::<1>()
            } else {
                common
            };
            for (c, value) in rgb.into_iter().enumerate() {
                let n = if MODE == 1 {
                    let n = common * 2 + signed(bits.shr_logical_uniform(7 * (c as u32 + 1)));
                    let sign = n.shr_arithmetic_const::<31>();
                    // Exact floor(abs(n)/3) for abs(n)<=381: for n=3k+r,
                    // 683*n=2048*k+k+683*r and k+683*r<2048.
                    let q = (n.abs() * 683).shr_arithmetic_const::<11>();
                    (q ^ sign) - sign
                } else {
                    shared
                };
                let sign = n.shr_arithmetic_const::<31>();
                let magnitude = (n.abs() * amplitude + 64).shr_arithmetic_const::<7>();
                let delta = (magnitude ^ sign) - sign;
                let out = (value + delta).max(zero).min(max).to_array();
                for (lane, value) in out.into_iter().enumerate() {
                    tile[lane * CHANNELS + c] = value as u8;
                }
            }
        }
        for (x, pixel_out) in tail.chunks_exact_mut(CHANNELS).enumerate() {
            pixel::<MODE>(
                pixel_out,
                (l.origin.0 + bulk + x) as u32,
                y.to_array()[0],
                p,
            );
        }
    }
}
