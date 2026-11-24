//! Bounded experimental 420-4x4: luma retains exact production 8x8 coefficients.
//! This module is included only by the probe and its integration tests.
#![allow(dead_code)]
use imij::{
    Channels, ImageInfo, ImiEncoderConfig, LossyColor, LossyEntropy, Metadata, SampleType,
    chroma_lab,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Fixed444,
    Fixed420,
    Adaptive,
}
impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Fixed444 => "chroma4-wrapper444",
            Self::Fixed420 => "chroma4-fixed420",
            Self::Adaptive => "chroma4-adaptive",
        }
    }
    fn code(self) -> u8 {
        match self {
            Self::Fixed444 => 0,
            Self::Fixed420 => 1,
            Self::Adaptive => 2,
        }
    }
}

/// Const-generic separable orthonormal DCT-II. Float64 analytic basis built once;
/// all transform sums f32. Production 8x8 control uses its canonical inverse.
pub struct Dct<const N: usize> {
    basis: [[f32; N]; N],
}
impl<const N: usize> Dct<N> {
    pub fn new() -> Self {
        Self {
            basis: std::array::from_fn(|k| {
                std::array::from_fn(|n| {
                    let scale = if k == 0 {
                        1.0 / (N as f64).sqrt()
                    } else {
                        (2.0 / N as f64).sqrt()
                    };
                    (scale
                        * (std::f64::consts::PI * k as f64 * (2 * n + 1) as f64 / (2 * N) as f64)
                            .cos()) as f32
                })
            }),
        }
    }
    pub fn forward(&self, x: &[[f32; N]; N]) -> [[f32; N]; N] {
        let t: [[f32; N]; N] = std::array::from_fn(|v| {
            std::array::from_fn(|i| (0..N).map(|j| self.basis[v][j] * x[j][i]).sum())
        });
        std::array::from_fn(|v| {
            std::array::from_fn(|u| (0..N).map(|i| self.basis[u][i] * t[v][i]).sum())
        })
    }
    pub fn inverse(&self, x: &[[f32; N]; N]) -> [[f32; N]; N] {
        let t: [[f32; N]; N] = std::array::from_fn(|j| {
            std::array::from_fn(|u| (0..N).map(|v| self.basis[v][j] * x[v][u]).sum())
        });
        std::array::from_fn(|j| {
            std::array::from_fn(|i| (0..N).map(|u| self.basis[u][i] * t[j][u]).sum())
        })
    }
}

/// Same physical frequencies over an 8px footprint, with half the orthonormal
/// amplitude: a constant has DC=N*c; q4 step=q8 step/2, including DC/floor.
pub fn step4(steps: &[f32; 64], v: usize, u: usize) -> f32 {
    steps[v * 8 + u] * 0.5
}
pub fn quant(v: f32, step: f32) -> i32 {
    let m = (v.abs() / step + 0.5).min(4_194_303.0) as i32;
    if v < 0.0 { -m } else { m }
}

pub struct Prepared {
    pub width: usize,
    pub height: usize,
    pub pw: usize,
    pub ph: usize,
    pub color: LossyColor,
    pub source: [Vec<f32>; 3],
}
impl Prepared {
    /// RGB8 sRGB, stride in bytes, alpha absent. Right/bottom replicated before
    /// centered box sampling; no hidden crop origin (caller supplies exact crop).
    pub fn new(
        rgb: &[u8],
        stride: usize,
        width: usize,
        height: usize,
        color: LossyColor,
    ) -> Result<Self, imij::Error> {
        let (_, _, pw, ph, _) = chroma_lab::checked_geometry(width, height)?;
        let source = chroma_lab::planes(rgb, stride, width, height, color)?;
        Ok(Self {
            width,
            height,
            pw,
            ph,
            color,
            source,
        })
    }
    pub fn production(&self, rgb: &[u8], stride: usize, q: f32) -> Result<Vec<u8>, imij::Error> {
        let (w, h, _, _, _) = chroma_lab::checked_geometry(self.width, self.height)?;
        let mut cfg = imij::ImiEncoderConfig::lossy(90.0);
        cfg = cfg.with_quality(q);
        cfg = cfg.tune_lossy(|t| {
            t.color = self.color;
        });
        cfg = cfg.tune_lossy(|t| {
            t.entropy = LossyEntropy::RunLevel;
        });
        cfg = cfg.with_threads(6);
        cfg = cfg.with_column_groups(Some(0));
        cfg.encode_samples(
            imij::ImageRef::new(
                rgb,
                stride,
                &ImageInfo::new(w, h, Channels::Rgb, SampleType::U8),
            ),
            &Metadata::default(),
        )
    }
    pub fn encode(
        &self,
        production: &[u8],
        quality: f32,
        mode: Mode,
    ) -> Result<Encoded, imij::Error> {
        let (_, _, pw, ph, pixels) = chroma_lab::checked_geometry(self.width, self.height)?;
        if self.pw != pw || self.ph != ph || self.source.iter().any(|p| p.len() != pixels) {
            return Err(imij::Error::InvalidInput("prepared chroma geometry"));
        }
        let (_, steps) = chroma_lab::parameters(quality, self.color);
        let original = chroma_lab::unpack(production)?;
        let bw = self.pw / 8;
        let nb = bw * (self.ph / 8);
        if original.len() != nb * 192 {
            return Err(imij::Error::InvalidInput("production geometry"));
        }
        let baseline = reconstruct_planes(&original, &vec![0; nb], self.pw, self.ph, &steps);
        // Baseline444 supplies deterministic local decision halos. The final
        // decoder builds its grid from the selected modes, never source pixels.
        let halo: [Vec<f32>; 2] =
            std::array::from_fn(|c| downsample(&baseline[c + 1], self.pw, self.ph));
        let mut coefficients = original.clone();
        let mut modes = vec![0u8; nb];
        let dct = Dct::<4>::new();
        let mut selected = 0usize;
        let mut estimated_bits = 0.0;
        for by in 0..self.ph / 8 {
            for bx in 0..bw {
                let b = by * bw + bx;
                let mut cand = [[0i32; 64]; 2];
                let mut half = [[[0.0; 4]; 4]; 2];
                let mut d444 = 0.0f64;
                let mut d420 = 0.0f64;
                for c in 0..2 {
                    let x = std::array::from_fn(|v| {
                        std::array::from_fn(|u| {
                            let y = by * 8 + v * 2;
                            let x = bx * 8 + u * 2;
                            let p = &self.source[c + 1];
                            ((p[y * self.pw + x] + p[y * self.pw + x + 1])
                                + (p[(y + 1) * self.pw + x] + p[(y + 1) * self.pw + x + 1]))
                                * 0.25
                        })
                    });
                    let f = dct.forward(&x);
                    for v in 0..4 {
                        for u in 0..4 {
                            cand[c][v * 8 + u] = quant(f[v][u], step4(&steps[c + 1], v, u));
                        }
                    }
                    let dq = std::array::from_fn(|v| {
                        std::array::from_fn(|u| {
                            cand[c][v * 8 + u] as f32 * step4(&steps[c + 1], v, u)
                        })
                    });
                    half[c] = dct.inverse(&dq);
                    let get = |x: usize, y: usize| {
                        if x / 4 == bx && y / 4 == by {
                            half[c][y % 4][x % 4]
                        } else {
                            halo[c][y * (self.pw / 2) + x]
                        }
                    };
                    // Quantizer-normalized spatial distortion, solely original
                    // coded planes and candidate reconstruction; no held-out metric.
                    let unit = (steps[c + 1][1] / 8.0) as f64;
                    for v in 0..8 {
                        for u in 0..8 {
                            let x = bx * 8 + u;
                            let y = by * 8 + v;
                            if x >= self.width || y >= self.height {
                                continue;
                            }
                            let s = self.source[c + 1][y * self.pw + x] as f64;
                            let a = baseline[c + 1][y * self.pw + x] as f64;
                            let z = triangle(x, y, self.pw / 2, self.ph / 2, &get) as f64;
                            d444 += ((s - a) / unit).powi(2);
                            d420 += ((s - z) / unit).powi(2);
                        }
                    }
                }
                let rate444 = rate(&original[(b * 3 + 1) * 64..(b * 3 + 3) * 64]);
                let rate420 = rate(&cand.concat());
                let map_price = if mode == Mode::Adaptive {
                    (nb.div_ceil(8) * 8) as f64 / nb as f64
                } else {
                    0.0
                };
                // Fixed lambda .08 frozen before scores; one actual map bit/block
                // and conservative incremental 1-bit420 charge. No entropy oracle.
                let pick = mode == Mode::Fixed420
                    || (mode == Mode::Adaptive
                        && d420 + 0.08 * (rate420 + map_price + 1.0)
                            < d444 + 0.08 * (rate444 + map_price));
                if pick {
                    modes[b] = 1;
                    selected += 1;
                    for c in 0..2 {
                        coefficients[(b * 3 + c + 1) * 64..(b * 3 + c + 2) * 64]
                            .copy_from_slice(&cand[c]);
                    }
                }
                estimated_bits += if pick { rate420 } else { rate444 };
            }
        }
        if mode == Mode::Adaptive {
            estimated_bits += (nb.div_ceil(8) * 8) as f64;
        }
        let inner = chroma_lab::pack(
            &coefficients,
            self.width as u32,
            self.height as u32,
            quality,
            self.color,
        )?;
        // The native inner stream cannot be decoded directly as420: the outer
        // magic is mandatory and owns interpretation of mapped coefficients.
        let map = if mode == Mode::Adaptive {
            let mut m = vec![0; nb.div_ceil(8)];
            for (i, &v) in modes.iter().enumerate() {
                m[i / 8] |= v << (i % 8);
            }
            m
        } else {
            Vec::new()
        };
        let mut file = b"CHR4P001".to_vec();
        file.extend_from_slice(&(self.width as u32).to_le_bytes());
        file.extend_from_slice(&(self.height as u32).to_le_bytes());
        file.extend_from_slice(&quality.to_le_bytes());
        file.extend_from_slice(&[self.color as u8, mode.code(), 0, 0]);
        file.extend_from_slice(&(map.len() as u32).to_le_bytes());
        file.extend_from_slice(&(inner.len() as u64).to_le_bytes());
        file.extend_from_slice(&map);
        file.extend_from_slice(&inner);
        let mut compact = coefficients.clone();
        let zz4 = [0usize, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];
        for (b, &m) in modes.iter().enumerate() {
            if m == 1 {
                for c in 1..3 {
                    let off = (b * 3 + c) * 64;
                    compact[off..off + 64].fill(0);
                    for k in 0..16 {
                        let i = zz4[k];
                        compact[off + chroma_lab::ZIGZAG8[k] as usize] =
                            coefficients[off + (i / 4) * 8 + i % 4];
                    }
                }
            }
        }
        let compact_file = chroma_lab::pack(
            &compact,
            self.width as u32,
            self.height as u32,
            quality,
            self.color,
        )?;
        if chroma_lab::unpack(&compact_file)? != compact {
            return Err(imij::Error::Corrupt("compact mapping coefficient parity"));
        }
        let compact_inner_bytes = compact_file.len();
        Ok(Encoded {
            file,
            compact_inner_bytes,
            inner_bytes: inner.len(),
            map_bytes: map.len(),
            selected,
            blocks: nb,
            estimated_chroma_bits: estimated_bits,
            coefficients,
        })
    }
}

/// Deliberately simple local bit surrogate: per block/channel DC+EOB charge,
/// nonzero sign and magnitude lengths. It is not an entropy-size estimate.
fn rate(q: &[i32]) -> f64 {
    q.as_chunks::<64>()
        .0
        .iter()
        .map(|b| {
            12.0 + b
                .iter()
                .filter(|&&x| x != 0)
                .map(|&x| 2.0 + 2.0 * (x.unsigned_abs() as f64 + 1.0).log2())
                .sum::<f64>()
        })
        .sum()
}
pub struct Encoded {
    pub compact_inner_bytes: usize,
    pub file: Vec<u8>,
    pub inner_bytes: usize,
    pub map_bytes: usize,
    pub selected: usize,
    pub blocks: usize,
    pub estimated_chroma_bits: f64,
    pub coefficients: Vec<i32>,
}

pub fn downsample(p: &[f32], pw: usize, ph: usize) -> Vec<f32> {
    (0..pw * ph / 4)
        .map(|i| {
            let x = (i % (pw / 2)) * 2;
            let y = (i / (pw / 2)) * 2;
            ((p[y * pw + x] + p[y * pw + x + 1]) + (p[(y + 1) * pw + x] + p[(y + 1) * pw + x + 1]))
                * 0.25
        })
        .collect()
}
/// Chroma centers (2i+.5,2j+.5); RGB pixel centers are integer coordinates.
/// Bilinear triangle .75 nearest/.25 adjacent; clamp only at true padded image
/// edges. Includes both diagonals, and crosses every 8px/stripe boundary.
pub fn triangle(
    x: usize,
    y: usize,
    cw: usize,
    ch: usize,
    get: &impl Fn(usize, usize) -> f32,
) -> f32 {
    let (cx, cy) = (x / 2, y / 2);
    let nx = if x & 1 == 0 {
        cx.saturating_sub(1)
    } else {
        (cx + 1).min(cw - 1)
    };
    let ny = if y & 1 == 0 {
        cy.saturating_sub(1)
    } else {
        (cy + 1).min(ch - 1)
    };
    let a = get(cx, cy) * 0.75 + get(cx, ny) * 0.25;
    let b = get(nx, cy) * 0.75 + get(nx, ny) * 0.25;
    a * 0.75 + b * 0.25
}
pub(crate) fn reconstruct_planes(
    q: &[i32],
    modes: &[u8],
    pw: usize,
    ph: usize,
    steps: &[[f32; 64]; 3],
) -> [Vec<f32>; 3] {
    let mut p: [Vec<f32>; 3] = std::array::from_fn(|_| vec![0.0; pw * ph]);
    let mut half: [Vec<f32>; 2] = std::array::from_fn(|_| vec![0.0; pw * ph / 4]);
    let dct = Dct::<4>::new();
    for (b, &mode) in modes.iter().enumerate() {
        let (bx, by) = (b % (pw / 8), b / (pw / 8));
        for c in 0..3 {
            let block = &q[(b * 3 + c) * 64..(b * 3 + c + 1) * 64];
            if c == 0 || mode == 0 {
                let x = chroma_lab::inverse8(block, &steps[c]);
                for v in 0..8 {
                    p[c][(by * 8 + v) * pw + bx * 8..(by * 8 + v) * pw + bx * 8 + 8]
                        .copy_from_slice(&x[v * 8..v * 8 + 8]);
                }
                if c > 0 {
                    for v in 0..4 {
                        for u in 0..4 {
                            let o = (by * 8 + v * 2) * pw + bx * 8 + u * 2;
                            half[c - 1][(by * 4 + v) * (pw / 2) + bx * 4 + u] =
                                ((p[c][o] + p[c][o + 1]) + (p[c][o + pw] + p[c][o + pw + 1]))
                                    * 0.25;
                        }
                    }
                }
            } else {
                let x = std::array::from_fn(|v| {
                    std::array::from_fn(|u| block[v * 8 + u] as f32 * step4(&steps[c], v, u))
                });
                let h = dct.inverse(&x);
                for (v, row) in h.iter().enumerate() {
                    half[c - 1]
                        [(by * 4 + v) * (pw / 2) + bx * 4..(by * 4 + v) * (pw / 2) + bx * 4 + 4]
                        .copy_from_slice(row);
                }
            }
        }
    }
    // Ownership:444 blocks keep all full-resolution pixels. Only420-owned
    // output pixels use the common centered grid, including444 averaged halos.
    for y in 0..ph {
        for x in 0..pw {
            if modes[(y / 8) * (pw / 8) + x / 8] == 1 {
                for c in 1..3 {
                    p[c][y * pw + x] = triangle(x, y, pw / 2, ph / 2, &|xx, yy| {
                        half[c - 1][yy * (pw / 2) + xx]
                    });
                }
            }
        }
    }
    p
}

pub fn decode(file: &[u8]) -> Result<Vec<u8>, String> {
    if file.len() < 36 || &file[..8] != b"CHR4P001" {
        return Err("chroma4 proxy magic/length".into());
    }
    let read = |o| u32::from_le_bytes(file[o..o + 4].try_into().unwrap());
    let (w, h) = (read(8) as usize, read(12) as usize);
    let quality = f32::from_bits(read(16));
    let color = match file[20] {
        0 => LossyColor::Xyb,
        1 => LossyColor::YCoCg,
        _ => return Err("color".into()),
    };
    if w == 0
        || h == 0
        || !quality.is_finite()
        || !(0.0..=116.0).contains(&quality)
        || file[22..24] != [0, 0]
    {
        return Err("geometry/quality/mapping".into());
    }
    let (_, _, pw, ph, pixels) = chroma_lab::checked_geometry(w, h).map_err(|e| e.to_string())?;
    let nmap = read(24) as usize;
    let innerlen = usize::try_from(u64::from_le_bytes(file[28..36].try_into().unwrap()))
        .map_err(|_| "length")?;
    if nmap > file.len() - 36 || innerlen != file.len() - 36 - nmap {
        return Err("sections".into());
    }
    let nb = pixels / 64;
    let modes = match file[21] {
        0 | 1 if nmap == 0 => vec![file[21]; nb],
        2 if nmap == nb.div_ceil(8) => {
            let m = &file[36..36 + nmap];
            if nb % 8 != 0 && m[nmap - 1] >> (nb % 8) != 0 {
                return Err("map padding".into());
            }
            (0..nb).map(|i| (m[i / 8] >> (i % 8)) & 1).collect()
        }
        _ => return Err("mode/map".into()),
    };
    let inner = &file[36 + nmap..];
    let info = imij::header(inner).map_err(|e| e.to_string())?.info;
    if info.width as usize != w || info.height as usize != h {
        return Err("inner geometry".into());
    }
    let q = chroma_lab::unpack(inner).map_err(|e| e.to_string())?;
    let (_, steps) = chroma_lab::parameters(quality, color);
    if q.len() != nb * 192 {
        return Err("coefficient extent".into());
    }
    for (b, &mode) in modes.iter().enumerate() {
        if mode == 1 {
            for c in 1..3 {
                for v in 0..8 {
                    for u in 0..8 {
                        if (v >= 4 || u >= 4) && q[(b * 3 + c) * 64 + v * 8 + u] != 0 {
                            return Err("nonzero unmapped slot".into());
                        }
                    }
                }
            }
        }
    }
    let p = reconstruct_planes(&q, &modes, pw, ph, &steps);
    let mut rgb = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            rgb.extend_from_slice(&chroma_lab::inverse_color(
                color,
                std::array::from_fn(|c| p[c][y * pw + x]),
            ));
        }
    }
    Ok(rgb)
}
