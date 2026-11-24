//! Bounded transforms inside one 8×8 parent, independently per color channel.
//!
//! Sampling is a centered 2:1 box in each selected axis. Reconstruction uses
//! centered linear interpolation, clamped to this parent's sampled edge. It
//! needs no neighbor halo, but does not eliminate sampling seams between parents.
//! All buffers are raster arrays with stride eight, independent of image stride.
//! Transform coefficients occupy the first `samples()` positions of the existing
//! zigzag; unused positions are zero. Tile DCs have a separable orthonormal Haar
//! hierarchy (one parent DC), followed by tile ACs in frequency-major order.
//!
//! Inverse arithmetic is f32 multiply then add, without contraction, columns
//! before rows. Default full-resolution 8×8 retains the existing production AAN
//! path; this module's full8 case is also useful as an independent reference.

use super::coef::ZIGZAG;
use crate::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    pub sx: u8,
    pub sy: u8,
    pub log_block: u8,
}

impl Layout {
    pub const FULL8: Self = Self {
        sx: 0,
        sy: 0,
        log_block: 3,
    };

    pub fn byte(self) -> u8 {
        self.sx | (self.sy << 1) | ((self.log_block - 1) << 2)
    }

    pub fn from_byte(byte: u8) -> Result<Self, Error> {
        if byte & 0xf0 != 0 || byte & 12 == 12 {
            return Err(Error::Corrupt("subblock layout"));
        }
        Ok(Self {
            sx: byte & 1,
            sy: (byte >> 1) & 1,
            log_block: ((byte >> 2) & 3) + 1,
        })
    }

    pub fn samples(self) -> usize {
        64 >> (self.sx + self.sy)
    }

    fn geometry(self) -> (usize, usize, usize, usize) {
        let w = 8 >> self.sx;
        let h = 8 >> self.sy;
        let b = 1 << self.log_block;
        (w, h, w.min(b), h.min(b))
    }

    /// The caller supplies a valid layout and one color plane of an 8×8 parent.
    #[cfg(feature = "encode")]
    pub fn forward(self, pixels: &[f32; 64]) -> [f32; 64] {
        let (w, h, bw, bh) = self.geometry();
        let mut tiles = [0.0; 64];
        let mut dc = [0.0; 64];
        let sampled = self.sample(pixels);
        for ty in 0..h / bh {
            for tx in 0..w / bw {
                let mut tile = [0.0; 64];
                for y in 0..bh {
                    for x in 0..bw {
                        tile[y * 8 + x] = sampled[(ty * bh + y) * 8 + tx * bw + x];
                    }
                }
                let transformed = transform::<false>(&tile, bw, bh);
                dc[ty * 8 + tx] = transformed[0];
                for y in 0..bh {
                    for x in 0..bw {
                        tiles[(ty * bh + y) * 8 + tx * bw + x] = transformed[y * 8 + x];
                    }
                }
            }
        }
        haar2::<false>(&mut dc, w / bw, h / bh);
        let mut out = [0.0; 64];
        self.visit(|slot, tile, freq| {
            out[slot] = if let Some((tx, ty)) = tile {
                tiles[(ty * bh + freq / 8) * 8 + tx * bw + freq % 8]
            } else {
                dc[freq]
            };
        });
        out
    }

    /// Dequantized coefficients, without AAN scaling, to one 8×8 color plane.
    pub fn inverse(self, coefs: &[f32; 64]) -> [f32; 64] {
        let (w, h, bw, bh) = self.geometry();
        let mut tiles = [0.0; 64];
        let mut dc = [0.0; 64];
        self.visit(|slot, tile, freq| {
            if let Some((tx, ty)) = tile {
                tiles[(ty * bh + freq / 8) * 8 + tx * bw + freq % 8] = coefs[slot];
            } else {
                dc[freq] = coefs[slot];
            }
        });
        haar2::<true>(&mut dc, w / bw, h / bh);
        let mut sampled = [0.0; 64];
        for ty in 0..h / bh {
            for tx in 0..w / bw {
                let mut tile = [0.0; 64];
                for y in 0..bh {
                    for x in 0..bw {
                        tile[y * 8 + x] = tiles[(ty * bh + y) * 8 + tx * bw + x];
                    }
                }
                tile[0] = dc[ty * 8 + tx];
                let decoded = transform::<true>(&tile, bw, bh);
                for y in 0..bh {
                    for x in 0..bw {
                        sampled[(ty * bh + y) * 8 + tx * bw + x] = decoded[y * 8 + x];
                    }
                }
            }
        }
        self.expand(&sampled)
    }

    /// Encoder weights in the compact coefficient arrangement. Tile AC uses
    /// the same physical frequency in the 8×8 matrix and sqrt(tile samples/64)
    /// normalization. Haar details use their dyadic support and nominal
    /// frequency: this is a quantization policy, not a claim that Haar is DCT.
    /// Unused slots get a positive step so ordinary quantization stays defined.
    pub fn steps(self, base: &[f32; 64]) -> [f32; 64] {
        let (w, h, bw, bh) = self.geometry();
        let mut out = [base[0]; 64];
        self.visit(|slot, tile, freq| {
            let (u, v, area) = if tile.is_some() {
                (
                    freq % 8 * 8 / (bw << self.sx),
                    freq / 8 * 8 / (bh << self.sy),
                    bw * bh,
                )
            } else {
                let fx = haar_frequency(freq % 8);
                let fy = haar_frequency(freq / 8);
                (fx, fy, w * h / fx.max(1) / fy.max(1))
            };
            out[slot] = base[v * 8 + u] * norm(area);
        });
        out
    }

    #[cfg(feature = "encode")]
    fn sample(self, pixels: &[f32; 64]) -> [f32; 64] {
        let (w, h, _, _) = self.geometry();
        let nx = 1 << self.sx;
        let ny = 1 << self.sy;
        let mut out = [0.0; 64];
        for y in 0..h {
            for x in 0..w {
                let mut sum = 0.0;
                for dy in 0..ny {
                    for dx in 0..nx {
                        sum += pixels[(y * ny + dy) * 8 + x * nx + dx];
                    }
                }
                out[y * 8 + x] = sum / (nx * ny) as f32;
            }
        }
        out
    }

    fn expand(self, sampled: &[f32; 64]) -> [f32; 64] {
        let mut out = [0.0; 64];
        for y in 0..8 {
            let (y0, y1, fy) = sample_pair(y, self.sy);
            for x in 0..8 {
                let (x0, x1, fx) = sample_pair(x, self.sx);
                let a = sampled[y0 * 8 + x0];
                let b = sampled[y0 * 8 + x1];
                let c = sampled[y1 * 8 + x0];
                let d = sampled[y1 * 8 + x1];
                let top = a + (b - a) * fx;
                let bottom = c + (d - c) * fx;
                out[y * 8 + x] = top + (bottom - top) * fy;
            }
        }
        out
    }

    /// One shared ordering for packing, unpacking and quantization weights.
    fn visit(self, mut f: impl FnMut(usize, Option<(usize, usize)>, usize)) {
        let (w, h, bw, bh) = self.geometry();
        let mut k = 0;
        for y in 0..h / bh {
            for x in 0..w / bw {
                f(ZIGZAG[k] as usize, None, y * 8 + x);
                k += 1;
            }
        }
        for &freq in &ZIGZAG[1..] {
            let freq = freq as usize;
            if freq % 8 < bw && freq / 8 < bh {
                for ty in 0..h / bh {
                    for tx in 0..w / bw {
                        f(ZIGZAG[k] as usize, Some((tx, ty)), freq);
                        k += 1;
                    }
                }
            }
        }
        debug_assert_eq!(k, self.samples());
    }
}

fn sample_pair(x: usize, shift: u8) -> (usize, usize, f32) {
    if shift == 0 {
        return (x, x, 0.0);
    }
    // Centered sample coordinates 0.5, 2.5, 4.5, 6.5. Clamped endpoints.
    if x == 0 {
        return (0, 0, 0.0);
    }
    let a = (x - 1) / 2;
    (
        a.min(3),
        (a + 1).min(3),
        if x & 1 == 0 { 0.75 } else { 0.25 },
    )
}

fn haar_frequency(k: usize) -> usize {
    match k {
        0 => 0,
        1 => 1,
        _ => 2,
    }
}

fn norm(area: usize) -> f32 {
    match area {
        4 => 0.25,
        8 => core::f32::consts::FRAC_1_SQRT_2 * 0.5,
        16 => 0.5,
        32 => core::f32::consts::FRAC_1_SQRT_2,
        64 => 1.0,
        _ => unreachable!("bounded dyadic transform support"),
    }
}

fn haar2<const INVERSE: bool>(x: &mut [f32; 64], w: usize, h: usize) {
    if INVERSE {
        for col in 0..w {
            haar::<true>(x, col, 8, h);
        }
        for row in 0..h {
            haar::<true>(x, row * 8, 1, w);
        }
    } else {
        for row in 0..h {
            haar::<false>(x, row * 8, 1, w);
        }
        for col in 0..w {
            haar::<false>(x, col, 8, h);
        }
    }
}

fn haar<const INVERSE: bool>(x: &mut [f32; 64], offset: usize, stride: usize, n: usize) {
    let mut len = if INVERSE { 2 } else { n };
    while len >= 2 && len <= n {
        let mut tmp = [0.0; 4];
        for i in 0..len / 2 {
            let (a, b) = if INVERSE {
                (x[offset + i * stride], x[offset + (i + len / 2) * stride])
            } else {
                (x[offset + 2 * i * stride], x[offset + (2 * i + 1) * stride])
            };
            let sum = (a + b) * core::f32::consts::FRAC_1_SQRT_2;
            let diff = (a - b) * core::f32::consts::FRAC_1_SQRT_2;
            if INVERSE {
                tmp[2 * i] = sum;
                tmp[2 * i + 1] = diff;
            } else {
                tmp[i] = sum;
                tmp[i + len / 2] = diff;
            }
        }
        for i in 0..len {
            x[offset + i * stride] = tmp[i];
        }
        if INVERSE {
            len *= 2;
        } else {
            len /= 2;
        }
    }
}

// Orthonormal DCT-II basis, rounded once to f32; no runtime trig or allocation.
const D2: [[f32; 2]; 2] = [
    [core::f32::consts::FRAC_1_SQRT_2; 2],
    [
        core::f32::consts::FRAC_1_SQRT_2,
        -core::f32::consts::FRAC_1_SQRT_2,
    ],
];
const D4: [[f32; 4]; 4] = [
    [0.5, 0.5, 0.5, 0.5],
    [0.653_281_5, 0.270_598_05, -0.270_598_05, -0.653_281_5],
    [0.5, -0.5, -0.5, 0.5],
    [0.270_598_05, -0.653_281_5, 0.653_281_5, -0.270_598_05],
];
const D8: [[f32; 8]; 8] = [
    [0.353_553_38; 8],
    [
        0.490_392_63,
        0.415_734_8,
        0.277_785_12,
        0.097_545_16,
        -0.097_545_16,
        -0.277_785_12,
        -0.415_734_8,
        -0.490_392_63,
    ],
    [
        0.461_939_75,
        0.191_341_71,
        -0.191_341_71,
        -0.461_939_75,
        -0.461_939_75,
        -0.191_341_71,
        0.191_341_71,
        0.461_939_75,
    ],
    [
        0.415_734_8,
        -0.097_545_16,
        -0.490_392_63,
        -0.277_785_12,
        0.277_785_12,
        0.490_392_63,
        0.097_545_16,
        -0.415_734_8,
    ],
    [
        0.353_553_38,
        -0.353_553_38,
        -0.353_553_38,
        0.353_553_38,
        0.353_553_38,
        -0.353_553_38,
        -0.353_553_38,
        0.353_553_38,
    ],
    [
        0.277_785_12,
        -0.490_392_63,
        0.097_545_16,
        0.415_734_8,
        -0.415_734_8,
        -0.097_545_16,
        0.490_392_63,
        -0.277_785_12,
    ],
    [
        0.191_341_71,
        -0.461_939_75,
        0.461_939_75,
        -0.191_341_71,
        -0.191_341_71,
        0.461_939_75,
        -0.461_939_75,
        0.191_341_71,
    ],
    [
        0.097_545_16,
        -0.277_785_12,
        0.415_734_8,
        -0.490_392_63,
        0.490_392_63,
        -0.415_734_8,
        0.277_785_12,
        -0.097_545_16,
    ],
];

#[inline(always)]
fn basis<const N: usize>(k: usize, x: usize) -> f32 {
    match N {
        2 => D2[k][x],
        4 => D4[k][x],
        8 => D8[k][x],
        _ => unreachable!(),
    }
}

fn transform<const INVERSE: bool>(x: &[f32; 64], w: usize, h: usize) -> [f32; 64] {
    match (w, h) {
        (2, 2) => rect::<2, 2, INVERSE>(x),
        (4, 4) => rect::<4, 4, INVERSE>(x),
        (4, 8) => rect::<4, 8, INVERSE>(x),
        (8, 4) => rect::<8, 4, INVERSE>(x),
        (8, 8) => rect::<8, 8, INVERSE>(x),
        _ => unreachable!("validated layout dimensions"),
    }
}

fn rect<const W: usize, const H: usize, const INVERSE: bool>(x: &[f32; 64]) -> [f32; 64] {
    let mut tmp = [0.0; 64];
    let mut out = [0.0; 64];
    if INVERSE {
        for u in 0..W {
            for y in 0..H {
                let mut sum = 0.0;
                for v in 0..H {
                    sum += basis::<H>(v, y) * x[v * 8 + u];
                }
                tmp[y * 8 + u] = sum;
            }
        }
        for y in 0..H {
            for x in 0..W {
                let mut sum = 0.0;
                for u in 0..W {
                    sum += basis::<W>(u, x) * tmp[y * 8 + u];
                }
                out[y * 8 + x] = sum;
            }
        }
    } else {
        for y in 0..H {
            for u in 0..W {
                let mut sum = 0.0;
                for n in 0..W {
                    sum += basis::<W>(u, n) * x[y * 8 + n];
                }
                tmp[y * 8 + u] = sum;
            }
        }
        for u in 0..W {
            for v in 0..H {
                let mut sum = 0.0;
                for y in 0..H {
                    sum += basis::<H>(v, y) * tmp[y * 8 + u];
                }
                out[v * 8 + u] = sum;
            }
        }
    }
    out
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    fn layouts() -> impl Iterator<Item = Layout> {
        (0..12).map(|v| Layout::from_byte(v).unwrap())
    }

    // Analytic f64 oracle: direct two-dimensional cosine sums and explicit
    // Haar basis vectors, independent of the production pass/packing helpers.
    fn oracle(layout: Layout, input: &[f32; 64]) -> [f64; 64] {
        let w = 8usize >> layout.sx;
        let h = 8usize >> layout.sy;
        let bw = w.min(1 << layout.log_block);
        let bh = h.min(1 << layout.log_block);
        let nx = 1usize << layout.sx;
        let ny = 1usize << layout.sy;
        let mut sampled = [[0.0; 8]; 8];
        for y in 0..h {
            for x in 0..w {
                for dy in 0..ny {
                    for dx in 0..nx {
                        sampled[y][x] +=
                            f64::from(input[(y * ny + dy) * 8 + x * nx + dx]) / (nx * ny) as f64;
                    }
                }
            }
        }
        let c = |n: usize, k: usize, x: usize| {
            let scale = (if k == 0 { 1.0 } else { 2.0 } / n as f64).sqrt();
            scale * (core::f64::consts::PI * ((2 * x + 1) * k) as f64 / (2 * n) as f64).cos()
        };
        let hb = |n: usize, k: usize, x: usize| {
            if k == 0 {
                return 1.0 / (n as f64).sqrt();
            }
            let level = 1usize << k.ilog2();
            let support = n / level;
            let start = (k - level) * support;
            if x < start || x >= start + support {
                0.0
            } else if x < start + support / 2 {
                1.0 / (support as f64).sqrt()
            } else {
                -1.0 / (support as f64).sqrt()
            }
        };
        let mut tile_coefs = [[0.0; 64]; 16];
        let gw = w / bw;
        let gh = h / bh;
        for ty in 0..gh {
            for tx in 0..gw {
                for v in 0..bh {
                    for u in 0..bw {
                        let mut sum = 0.0;
                        for y in 0..bh {
                            for x in 0..bw {
                                sum +=
                                    sampled[ty * bh + y][tx * bw + x] * c(bw, u, x) * c(bh, v, y);
                            }
                        }
                        tile_coefs[ty * gw + tx][v * 8 + u] = sum;
                    }
                }
            }
        }
        let mut out = [0.0; 64];
        let mut k = 0;
        for v in 0..gh {
            for u in 0..gw {
                for ty in 0..gh {
                    for tx in 0..gw {
                        out[ZIGZAG[k] as usize] +=
                            tile_coefs[ty * gw + tx][0] * hb(gw, u, tx) * hb(gh, v, ty);
                    }
                }
                k += 1;
            }
        }
        for &freq in &ZIGZAG[1..] {
            if freq as usize % 8 < bw && (freq as usize) / 8 < bh {
                for tile in &tile_coefs[..gw * gh] {
                    out[ZIGZAG[k] as usize] = tile[freq as usize];
                    k += 1;
                }
            }
        }
        out
    }

    #[test]
    fn descriptor_exhaustive() {
        for byte in 0..=255 {
            match Layout::from_byte(byte) {
                Ok(layout) => {
                    assert!(byte < 12);
                    assert_eq!(layout.byte(), byte);
                }
                Err(_) => assert!(byte >= 12),
            }
        }
        assert_eq!(Layout::FULL8.byte(), 8);
    }

    #[test]
    fn all_layouts_match_analytical_forward_and_energy() {
        let mut random = [0.0; 64];
        let mut seed = 17u32;
        for x in &mut random {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *x = (seed >> 24) as f32 - 128.0;
        }
        for layout in layouts() {
            for pattern in 0..66 {
                let mut input = if pattern == 64 {
                    random
                } else if pattern == 65 {
                    [37.0; 64]
                } else {
                    [0.0; 64]
                };
                if pattern < 64 {
                    input[pattern] = 128.0;
                }
                let got = layout.forward(&input);
                let expected = oracle(layout, &input);
                for i in 0..64 {
                    assert!(
                        (f64::from(got[i]) - expected[i]).abs() < 0.0002,
                        "{layout:?} pattern {pattern} coefficient {i}: {} != {}",
                        got[i],
                        expected[i]
                    );
                }
                for &slot in &ZIGZAG[layout.samples()..] {
                    assert_eq!(got[slot as usize], 0.0);
                }
                let sampled = layout.sample(&input);
                let energy: f64 = sampled.iter().map(|&x| f64::from(x).powi(2)).sum();
                let transformed: f64 = got.iter().map(|&x| f64::from(x).powi(2)).sum();
                assert!((energy - transformed).abs() <= energy * 1e-6 + 1e-7);
                let actual = layout.inverse(&got);
                let reconstructed = layout.expand(&sampled);
                for i in 0..64 {
                    assert!(
                        (actual[i] - reconstructed[i]).abs() < 0.0002,
                        "{layout:?} pattern {pattern} pixel {i}"
                    );
                }
            }
        }
    }

    #[test]
    fn full8_matches_existing_reference() {
        let pixels = core::array::from_fn(|i| ((i * 47 + 13) % 251) as f32 - 100.0);
        let reference = super::super::dct::fdct8x8(&super::super::dct::dct_matrix(), &pixels);
        let got = Layout::FULL8.forward(&pixels);
        for i in 0..64 {
            assert!((got[i] - reference[i]).abs() < 0.0002);
        }
        let base = core::array::from_fn(|i| 1.0 + i as f32);
        assert_eq!(Layout::FULL8.steps(&base), base);
    }

    #[test]
    fn box_phase_and_centered_interpolation_are_explicit() {
        let ramp = core::array::from_fn(|i| (i % 8 + 10 * (i / 8)) as f32);
        for layout in layouts() {
            let sampled = layout.sample(&ramp);
            let reconstructed = layout.expand(&sampled);
            for y in 0..8 {
                for x in 0..8 {
                    let px = if layout.sx == 1 {
                        (x as f32).clamp(0.5, 6.5)
                    } else {
                        x as f32
                    };
                    let py = if layout.sy == 1 {
                        (y as f32).clamp(0.5, 6.5)
                    } else {
                        y as f32
                    };
                    assert_eq!(reconstructed[y * 8 + x], px + 10.0 * py);
                }
            }
        }
    }

    #[test]
    fn constant_has_one_dc_and_quantization_is_normalized() {
        for layout in layouts() {
            let coef = layout.forward(&[16.0; 64]);
            let expected_dc = 16.0 * (layout.samples() as f32).sqrt();
            assert!((coef[0] - expected_dc).abs() < 0.0001);
            for &slot in &ZIGZAG[1..] {
                assert!(coef[slot as usize].abs() < 0.0001);
            }
            let steps = layout.steps(&[8.0; 64]);
            assert!((steps[0] - (layout.samples() as f32).sqrt()).abs() < 0.00001);
            assert!(steps.iter().all(|s| s.is_finite() && *s > 0.0));
            // One quantized parent DC step means one unit in every pixel,
            // independent of sampling rate and subdivision.
            let mut one = [0.0; 64];
            one[0] = steps[0];
            for value in layout.inverse(&one) {
                assert!((value - 1.0).abs() < 0.000001);
            }
        }
    }
}
