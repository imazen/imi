//! Lossy-mode experiments (docs/LOSSY.md, E1-E4).
//!
//! Everything here is a model of the proposed coder, written for clarity, not
//! speed: f64 arithmetic that is exact for the integer paths (all values are
//! dyadic with few fractional bits), adaptive-entropy size estimates instead
//! of a real bitstream.

use crate::corpus::Img;
use rayon::prelude::*;

// ---------------------------------------------------------------------------
// 8-point transforms
// ---------------------------------------------------------------------------

/// Fixed-point precision of the lifting multipliers.
const FIX: f64 = 4096.0;

#[derive(Clone, Copy)]
struct Rot {
    t: f64,
    s: f64,
}

impl Rot {
    fn new(n: f64, int: bool) -> Rot {
        let th = n * std::f64::consts::PI / 16.0;
        let q = |v: f64| if int { (v * FIX).round() / FIX } else { v };
        Rot {
            t: q((th / 2.0).tan()),
            s: q(th.sin()),
        }
    }
}

#[derive(Clone, Copy)]
pub struct Lift {
    int: bool,
    r1: Rot,
    r3: Rot,
    r6: Rot,
}

impl Lift {
    pub fn new(int: bool) -> Lift {
        Lift {
            int,
            r1: Rot::new(1.0, int),
            r3: Rot::new(3.0, int),
            r6: Rot::new(6.0, int),
        }
    }
    #[inline]
    fn r(&self, v: f64) -> f64 {
        if self.int { (v + 0.5).floor() } else { v }
    }
    #[inline]
    fn half(&self, d: f64) -> f64 {
        if self.int { (d / 2.0).floor() } else { d / 2.0 }
    }
    /// (x, y) -> (x c + y s, -x s + y c) as three lifting steps.
    #[inline]
    fn rot_f(&self, x: &mut f64, y: &mut f64, r: Rot) {
        *x += self.r(r.t * *y);
        *y -= self.r(r.s * *x);
        *x += self.r(r.t * *y);
    }
    #[inline]
    fn rot_i(&self, x: &mut f64, y: &mut f64, r: Rot) {
        *x -= self.r(r.t * *y);
        *y += self.r(r.s * *x);
        *x -= self.r(r.t * *y);
    }
    /// (a, b) -> (≈(a+b)/2, a-b).
    #[inline]
    fn bf_f(&self, a: f64, b: f64) -> (f64, f64) {
        let d = a - b;
        (b + self.half(d), d)
    }
    #[inline]
    fn bf_i(&self, s: f64, d: f64) -> (f64, f64) {
        let b = s - self.half(d);
        (d + b, b)
    }

    /// Loeffler 8-point DCT with lifting butterflies and rotations.
    pub fn fwd8(&self, x: &mut [f64; 8]) {
        let (l0, l7) = self.bf_f(x[0], x[7]);
        let (l1, l6) = self.bf_f(x[1], x[6]);
        let (l2, l5) = self.bf_f(x[2], x[5]);
        let (l3, l4) = self.bf_f(x[3], x[4]);
        // even
        let (e0, mut e3) = self.bf_f(l0, l3);
        let (e1, mut e2) = self.bf_f(l1, l2);
        let (x0, x4) = self.bf_f(e0, e1);
        self.rot_f(&mut e2, &mut e3, self.r6);
        // odd: l4 = x3-x4, l5 = x2-x5, l6 = x1-x6, l7 = x0-x7
        let (mut o4, mut o7) = (l4, l7);
        self.rot_f(&mut o4, &mut o7, self.r3);
        let (mut o5, mut o6) = (l5, l6);
        self.rot_f(&mut o5, &mut o6, self.r1);
        let (a4, a6) = self.bf_f(o4, o6);
        let (a7, a5) = self.bf_f(o7, o5);
        let (x1, x7) = self.bf_f(a7, a4);
        *x = [x0, x1, e2, a5, x4, a6, e3, x7];
    }

    pub fn inv8(&self, x: &mut [f64; 8]) {
        let [x0, x1, mut e2, a5, x4, a6, mut e3, x7] = *x;
        let (a7, a4) = self.bf_i(x1, x7);
        let (o7, o5) = self.bf_i(a7, a5);
        let (o4, o6) = self.bf_i(a4, a6);
        let (mut o5, mut o6) = (o5, o6);
        self.rot_i(&mut o5, &mut o6, self.r1);
        let (mut o4, mut o7) = (o4, o7);
        self.rot_i(&mut o4, &mut o7, self.r3);
        self.rot_i(&mut e2, &mut e3, self.r6);
        let (e0, e1) = self.bf_i(x0, x4);
        let (l1, l2) = self.bf_i(e1, e2);
        let (l0, l3) = self.bf_i(e0, e3);
        let (a, b) = self.bf_i(l3, o4);
        x[3] = a;
        x[4] = b;
        let (a, b) = self.bf_i(l2, o5);
        x[2] = a;
        x[5] = b;
        let (a, b) = self.bf_i(l1, o6);
        x[1] = a;
        x[6] = b;
        let (a, b) = self.bf_i(l0, o7);
        x[0] = a;
        x[7] = b;
    }
}

fn dct_matrix() -> [[f64; 8]; 8] {
    let mut c = [[0.0; 8]; 8];
    for (k, row) in c.iter_mut().enumerate() {
        let ck = if k == 0 { (1.0f64 / 8.0).sqrt() } else { 0.5 };
        for (n, v) in row.iter_mut().enumerate() {
            *v = ck * (std::f64::consts::PI * (2 * n + 1) as f64 * k as f64 / 16.0).cos();
        }
    }
    c
}

/// Per-coefficient scale of the lifting DCT relative to the orthonormal DCT,
/// verified to be diagonal.
pub fn lift_scales() -> [f64; 8] {
    let c = dct_matrix();
    let l = Lift::new(false);
    let mut s = [0.0; 8];
    for k in 0..8 {
        let mut v = c[k];
        l.fwd8(&mut v);
        for (j, &x) in v.iter().enumerate() {
            if j != k {
                assert!(x.abs() < 1e-9, "lift DCT not diagonal: k={k} j={j} {x}");
            }
        }
        s[k] = v[k];
    }
    s
}

// ---------------------------------------------------------------------------
// Color
// ---------------------------------------------------------------------------

#[inline]
fn ycocg_f(r: i32, g: i32, b: i32) -> (i32, i32, i32) {
    let co = r - b;
    let t = b + (co >> 1);
    let cg = g - t;
    (t + (cg >> 1), co, cg)
}

#[inline]
fn ycocg_i(y: i32, co: i32, cg: i32) -> (i32, i32, i32) {
    let t = y - (cg >> 1);
    let g = cg + t;
    let b = t - (co >> 1);
    (b + co, g, b)
}

// ---------------------------------------------------------------------------
// Coding model
// ---------------------------------------------------------------------------

pub const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

const JPEG_LUMA: [u16; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56,
    14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113,
    92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
];
const JPEG_CHROMA: [u16; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99,
    47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
];

fn jpeg_table(base: &[u16; 64], q: u32) -> [f64; 64] {
    let s = if q < 50 { 5000 / q } else { 200 - 2 * q };
    let mut t = [0.0; 64];
    for i in 0..64 {
        t[i] = ((base[i] as u32 * s + 50) / 100).clamp(1, 255) as f64;
    }
    t
}

/// Quantization: ortho-domain step per (channel, raster position).
#[derive(Clone)]
pub struct Quant {
    pub steps: [[f64; 64]; 3],
}

impl Quant {
    pub fn jpeg(q: u32, chroma_mul: f64) -> Quant {
        Self::jpeg3(q, [1.0, chroma_mul, chroma_mul])
    }
    /// The luma table on all three channels, times `mul`.
    pub fn luma3(q: u32, mul: [f64; 3]) -> Quant {
        let l = jpeg_table(&JPEG_LUMA, q);
        Quant {
            steps: mul.map(|m| {
                let mut t = l;
                for v in t.iter_mut() {
                    *v *= m;
                }
                t
            }),
        }
    }
    pub fn jpeg3(q: u32, mul: [f64; 3]) -> Quant {
        let l = jpeg_table(&JPEG_LUMA, q);
        let c = jpeg_table(&JPEG_CHROMA, q);
        let mut steps = [l, c, c];
        for ch in 0..3 {
            for v in steps[ch].iter_mut() {
                *v *= mul[ch];
            }
        }
        Quant { steps }
    }
    /// Flat luma step `q`; chroma `q * chroma_mul`; high frequencies grow by
    /// `1 + slope * (u + v) / 14`.
    pub fn flat(q: f64, chroma_mul: f64, slope: f64) -> Quant {
        let mut steps = [[0.0; 64]; 3];
        for (ch, st) in steps.iter_mut().enumerate() {
            for (i, v) in st.iter_mut().enumerate() {
                let f = 1.0 + slope * ((i % 8) + (i / 8)) as f64 / 14.0;
                *v = q * f * if ch == 0 { 1.0 } else { chroma_mul };
            }
        }
        Quant { steps }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Xform {
    /// Reversible lifting DCT with `f` fractional bits of prescale.
    Lift { f: u32 },
    /// Orthonormal float DCT.
    Float,
    /// Lifting structure without rounding (must equal Float).
    LiftNoRound,
}

#[derive(Clone)]
pub struct LossyCfg {
    pub xform: Xform,
    pub quant: Quant,
    /// Rounding offset: q = floor(|c|/step + round).
    pub round: f64,
    /// Neighbor (left + above) magnitude contexts.
    pub nb_ctx: bool,
    pub color: Color,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Color {
    YCoCgR,
    /// No color transform: R, G, B planes.
    Rgb,
    /// XYB scaled to ~0..255 luma; X scaled by `x_gain`; third channel B - Y.
    Xyb {
        x_gain: f64,
    },
}

const XYB_SCALE: f64 = 300.0;
const OPSIN: [[f64; 3]; 3] = [
    [0.30, 0.622, 0.078],
    [0.23, 0.692, 0.078],
    [
        0.243_422_689_245_478_2,
        0.204_767_444_244_968_2,
        0.551_809_866_509_553,
    ],
];
const OPSIN_BIAS: f64 = 0.003_793_073_255_275_449_3;

fn srgb_to_lin(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
fn lin_to_srgb(v: f64) -> f64 {
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn xyb_f(rgb: [u8; 3], xg: f64) -> [f64; 3] {
    let l = rgb.map(|v| srgb_to_lin(v as f64 / 255.0));
    let bc = OPSIN_BIAS.cbrt();
    let mut lms = [0.0; 3];
    for i in 0..3 {
        let m = OPSIN[i][0] * l[0] + OPSIN[i][1] * l[1] + OPSIN[i][2] * l[2] + OPSIN_BIAS;
        lms[i] = m.max(0.0).cbrt() - bc;
    }
    let x = 0.5 * (lms[0] - lms[1]);
    let y = 0.5 * (lms[0] + lms[1]);
    [y * XYB_SCALE, x * XYB_SCALE * xg, (lms[2] - y) * XYB_SCALE]
}

fn xyb_i(p: [f64; 3], xg: f64, inv: &[[f64; 3]; 3]) -> [u8; 3] {
    let y = p[0] / XYB_SCALE;
    let x = p[1] / XYB_SCALE / xg;
    let b = p[2] / XYB_SCALE + y;
    let bc = OPSIN_BIAS.cbrt();
    let c = [y + x + bc, y - x + bc, b + bc];
    let m = c.map(|v| v * v * v - OPSIN_BIAS);
    let mut out = [0u8; 3];
    for i in 0..3 {
        let lin = inv[i][0] * m[0] + inv[i][1] * m[1] + inv[i][2] * m[2];
        out[i] = (lin_to_srgb(lin.clamp(0.0, 1.0)) * 255.0).round() as u8;
    }
    out
}

fn inv3(m: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            let (a, b) = ((j + 1) % 3, (j + 2) % 3);
            let (c, d) = ((i + 1) % 3, (i + 2) % 3);
            r[i][j] = (m[a][c] * m[b][d] - m[a][d] * m[b][c]) / det;
        }
    }
    r
}

/// Adaptive multi-symbol cost estimator (KT-like with prior `alpha`).
struct Adaptive {
    counts: Vec<u32>,
    totals: Vec<u32>,
    nsym: usize,
    alpha: f64,
    bits: f64,
}

impl Adaptive {
    fn new(nctx: usize, nsym: usize, alpha: f64) -> Adaptive {
        Adaptive {
            counts: vec![0; nctx * nsym],
            totals: vec![0; nctx],
            nsym,
            alpha,
            bits: 0.0,
        }
    }
    #[inline]
    fn code(&mut self, ctx: usize, sym: usize) {
        let c = self.counts[ctx * self.nsym + sym] as f64;
        let t = self.totals[ctx] as f64;
        let p = (c + self.alpha) / (t + self.alpha * self.nsym as f64);
        self.bits -= p.log2();
        self.counts[ctx * self.nsym + sym] += 1;
        self.totals[ctx] += 1;
    }
}

const NTOK: usize = 50;

/// Hybrid-uint token (16 direct values, then exponent + 1 mantissa bit).
#[inline]
fn token(v: u64) -> (usize, u32) {
    if v < 16 {
        (v as usize, 0)
    } else {
        let n = 63 - v.leading_zeros();
        let m = (v >> (n - 1)) & 1;
        (16 + (n as usize - 4) * 2 + m as usize, n - 1)
    }
}

#[inline]
fn zz(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

#[inline]
fn mag_class(m: i64) -> usize {
    match m {
        0 => 0,
        1 => 1,
        2 => 2,
        3..=4 => 3,
        5..=8 => 4,
        9..=16 => 5,
        17..=32 => 6,
        _ => 7,
    }
}

pub struct LossyResult {
    /// Quantized coefficients per channel, raster block order.
    pub coefs: Vec<Vec<[i64; 64]>>,
    pub bw: usize,
    pub bh: usize,
    pub tokens_per_px: f64,
    /// Per channel: mean EOB position, mean nonzero AC count, per block.
    pub eob_nz: [(f64, f64); 3],
    pub bits_ac: f64,
    pub bits_dc: f64,
    pub bits_eob: f64,
    pub recon: Vec<u8>,
}

/// Encode-model + reconstruct an RGB8 image.
pub fn run(img: &Img, cfg: &LossyCfg) -> LossyResult {
    assert_eq!(img.info.channels, imij::Channels::Rgb);
    assert_eq!(img.info.sample, imij::SampleType::U8);
    let w = img.info.width as usize;
    let h = img.info.height as usize;
    let bw = w.div_ceil(8);
    let bh = h.div_ceil(8);
    let pw = bw * 8;
    let ph = bh * 8;
    // Planes, edge-replicated padding.
    let mut planes = vec![vec![0f64; pw * ph]; 3];
    for y in 0..ph {
        let sy = y.min(h - 1);
        for x in 0..pw {
            let sx = x.min(w - 1);
            let p = &img.pixels[(sy * w + sx) * 3..][..3];
            let v = match cfg.color {
                Color::YCoCgR => {
                    let (yy, co, cg) = ycocg_f(p[0] as i32, p[1] as i32, p[2] as i32);
                    [yy as f64, co as f64, cg as f64]
                }
                Color::Xyb { x_gain } => xyb_f([p[0], p[1], p[2]], x_gain),
                Color::Rgb => [p[0] as f64, p[1] as f64, p[2] as f64],
            };
            for c in 0..3 {
                planes[c][y * pw + x] = v[c];
            }
        }
    }
    if matches!(cfg.color, Color::Xyb { .. }) {
        assert_eq!(cfg.xform, Xform::Float, "XYB is float-only");
    }
    let mut ntok = 0usize;
    let mut eob_nz = [(0.0f64, 0.0f64); 3];
    let lift = Lift::new(cfg.xform != Xform::LiftNoRound);
    let sc = lift_scales();
    let cm = dct_matrix();
    let (fbits, is_lift) = match cfg.xform {
        Xform::Lift { f } => (f, true),
        Xform::Float => (0, false),
        Xform::LiftNoRound => (0, true),
    };
    let pre = (1u64 << fbits) as f64;
    // Internal step per channel/position.
    let mut isteps = [[0.0f64; 64]; 3];
    for ch in 0..3 {
        for i in 0..64 {
            let (u, v) = (i % 8, i / 8);
            let s = cfg.quant.steps[ch][i];
            isteps[ch][i] = if is_lift {
                let st = s * pre * sc[u] * sc[v];
                if cfg.xform == Xform::LiftNoRound {
                    st
                } else {
                    st.max(1.0)
                }
            } else {
                s
            };
        }
    }

    let nbc = if cfg.nb_ctx { 8 } else { 1 };
    let mut ac = Adaptive::new(3 * 64 * nbc, NTOK, 0.25);
    let mut eob = Adaptive::new(3 * 8, 65, 0.25);
    let mut dcm = Adaptive::new(3 * 8, NTOK, 0.25);
    let mut extra_bits = 0.0f64;
    let mut out_planes = vec![vec![0f64; pw * ph]; 3];
    let mut coefs: Vec<Vec<[i64; 64]>> = vec![Vec::with_capacity(bw * bh); 3];

    for ch in 0..3 {
        let st = &isteps[ch];
        // Quantized coefficients of the previous block row and current row.
        let mut prev_row: Vec<[i64; 64]> = vec![[0; 64]; bw];
        let mut cur_row: Vec<[i64; 64]> = vec![[0; 64]; bw];
        let mut prev_eob = vec![0usize; bw];
        let mut cur_eob = vec![0usize; bw];
        for by in 0..bh {
            for bx in 0..bw {
                let mut blk = [0.0f64; 64];
                for y in 0..8 {
                    for x in 0..8 {
                        blk[y * 8 + x] = (planes[ch][(by * 8 + y) * pw + bx * 8 + x] as f64) * pre;
                    }
                }
                // forward 2D
                if is_lift {
                    for y in 0..8 {
                        let mut r: [f64; 8] = blk[y * 8..y * 8 + 8].try_into().unwrap();
                        lift.fwd8(&mut r);
                        blk[y * 8..y * 8 + 8].copy_from_slice(&r);
                    }
                    for x in 0..8 {
                        let mut c = [0.0; 8];
                        for y in 0..8 {
                            c[y] = blk[y * 8 + x];
                        }
                        lift.fwd8(&mut c);
                        for y in 0..8 {
                            blk[y * 8 + x] = c[y];
                        }
                    }
                } else {
                    let mut tmp = [0.0; 64];
                    for y in 0..8 {
                        for k in 0..8 {
                            tmp[y * 8 + k] = (0..8).map(|n| cm[k][n] * blk[y * 8 + n]).sum();
                        }
                    }
                    for x in 0..8 {
                        for k in 0..8 {
                            blk[k * 8 + x] = (0..8).map(|n| cm[k][n] * tmp[n * 8 + x]).sum();
                        }
                    }
                }
                // quantize
                let mut q = [0i64; 64];
                for i in 0..64 {
                    let c = blk[i];
                    let a = (c.abs() / st[i] + cfg.round).floor();
                    q[i] = if c < 0.0 { -a as i64 } else { a as i64 };
                }
                // entropy model: EOB, AC dense, DC handled below
                let mut last = 0usize;
                for k in 1..64 {
                    if q[ZIGZAG[k]] != 0 {
                        last = k;
                    }
                }
                let ectx = if bx > 0 {
                    cur_eob[bx - 1]
                } else {
                    prev_eob[bx]
                };
                eob.code(ch * 8 + (ectx * 8 / 64).min(7), last);
                ntok += 2;
                cur_eob[bx] = last;
                eob_nz[ch].0 += last as f64;
                eob_nz[ch].1 += (1..64).filter(|&k| q[ZIGZAG[k]] != 0).count() as f64;
                for k in 1..=last {
                    let pos = ZIGZAG[k];
                    let nb = if cfg.nb_ctx {
                        let l = if bx > 0 {
                            cur_row[bx - 1][pos].abs()
                        } else {
                            0
                        };
                        let a = if by > 0 { prev_row[bx][pos].abs() } else { 0 };
                        mag_class(l + a)
                    } else {
                        0
                    };
                    let (t, e) = token(zz(q[pos]));
                    ac.code((ch * 64 + k) * nbc + nb, t);
                    ntok += 1;
                    extra_bits += e as f64;
                }
                cur_row[bx] = q;
                coefs[ch].push(q);
                // dequantize + inverse
                let mut r = [0.0f64; 64];
                for i in 0..64 {
                    r[i] = q[i] as f64 * st[i];
                }
                if is_lift {
                    if cfg.xform != Xform::LiftNoRound {
                        for v in r.iter_mut() {
                            *v = v.round();
                        }
                    }
                    for x in 0..8 {
                        let mut c = [0.0; 8];
                        for y in 0..8 {
                            c[y] = r[y * 8 + x];
                        }
                        lift.inv8(&mut c);
                        for y in 0..8 {
                            r[y * 8 + x] = c[y];
                        }
                    }
                    for y in 0..8 {
                        let mut rr: [f64; 8] = r[y * 8..y * 8 + 8].try_into().unwrap();
                        lift.inv8(&mut rr);
                        r[y * 8..y * 8 + 8].copy_from_slice(&rr);
                    }
                } else {
                    let mut tmp = [0.0; 64];
                    for x in 0..8 {
                        for n in 0..8 {
                            tmp[n * 8 + x] = (0..8).map(|k| cm[k][n] * r[k * 8 + x]).sum();
                        }
                    }
                    for y in 0..8 {
                        for n in 0..8 {
                            r[y * 8 + n] = (0..8).map(|k| cm[k][n] * tmp[y * 8 + k]).sum();
                        }
                    }
                }
                for y in 0..8 {
                    for x in 0..8 {
                        out_planes[ch][(by * 8 + y) * pw + bx * 8 + x] = r[y * 8 + x] / pre;
                    }
                }
            }
            // DC plane of this block row: MED prediction on quantized DCs.
            for bx in 0..bw {
                let d = cur_row[bx][0];
                let left = if bx > 0 {
                    cur_row[bx - 1][0]
                } else if by > 0 {
                    prev_row[bx][0]
                } else {
                    0
                };
                let up = if by > 0 { prev_row[bx][0] } else { left };
                let ul = if bx > 0 && by > 0 {
                    prev_row[bx - 1][0]
                } else {
                    up
                };
                let pred = if ul >= left.max(up) {
                    left.min(up)
                } else if ul <= left.min(up) {
                    left.max(up)
                } else {
                    left + up - ul
                };
                let ctx = mag_class((left - ul).abs() + (up - ul).abs());
                let (t, e) = token(zz(d - pred));
                dcm.code(ch * 8 + ctx, t);
                extra_bits += e as f64;
            }
            std::mem::swap(&mut prev_row, &mut cur_row);
            std::mem::swap(&mut prev_eob, &mut cur_eob);
        }
    }
    let mut recon = vec![0u8; w * h * 3];
    let inv = inv3(&OPSIN);
    for y in 0..h {
        for x in 0..w {
            let i = y * pw + x;
            let o = &mut recon[(y * w + x) * 3..][..3];
            match cfg.color {
                Color::YCoCgR => {
                    let q = |v: f64| v.round() as i32;
                    let (r, g, b) = ycocg_i(
                        q(out_planes[0][i]),
                        q(out_planes[1][i]),
                        q(out_planes[2][i]),
                    );
                    o[0] = r.clamp(0, 255) as u8;
                    o[1] = g.clamp(0, 255) as u8;
                    o[2] = b.clamp(0, 255) as u8;
                }
                Color::Xyb { x_gain } => {
                    let v = xyb_i(
                        [out_planes[0][i], out_planes[1][i], out_planes[2][i]],
                        x_gain,
                        &inv,
                    );
                    o.copy_from_slice(&v);
                }
                Color::Rgb => {
                    for c in 0..3 {
                        o[c] = out_planes[c][i].round().clamp(0.0, 255.0) as u8;
                    }
                }
            }
        }
    }
    LossyResult {
        coefs,
        bw,
        bh,
        tokens_per_px: ntok as f64 / (w * h) as f64,
        eob_nz: eob_nz.map(|(a, b)| (a / (bw * bh) as f64, b / (bw * bh) as f64)),
        bits_ac: ac.bits + extra_bits,
        bits_dc: dcm.bits,
        bits_eob: eob.bits,
        recon,
    }
}

pub fn ssim2(reference: &fast_ssim2::Ssimulacra2Reference, img: &Img, px: &[u8]) -> f64 {
    let w = img.info.width;
    let s = fast_ssim2::PixelSlice::new(
        px,
        w,
        img.info.height,
        w as usize * 3,
        fast_ssim2::PixelDescriptor::RGB8_SRGB,
    )
    .unwrap();
    reference.compare(&s).unwrap()
}

pub fn jpeg_encode(img: &Img, q: f32) -> Vec<u8> {
    jpeg_encode_aq(img, q, true)
}

/// zenjpeg baseline (sequential) with its default restart interval (every
/// 4 MCU rows), 4:4:4 or 4:2:0.
pub fn jpeg_encode_sub(img: &Img, q: f32, quarter: bool) -> Vec<u8> {
    use zenjpeg::encoder::{ChromaSubsampling, EncoderConfig, PixelLayout, Unstoppable};
    let sub = if quarter {
        ChromaSubsampling::Quarter
    } else {
        ChromaSubsampling::None
    };
    // ycbcr() defaults to progressive, which omits restart markers.
    let cfg = EncoderConfig::ycbcr(q, sub).progressive(false);
    let mut enc = cfg
        .encode_from_bytes(img.info.width, img.info.height, PixelLayout::Rgb8Srgb)
        .unwrap();
    enc.push_packed(&img.pixels, Unstoppable).unwrap();
    enc.finish().unwrap()
}

/// zenjpeg default config (progressive, jpegli AQ) at 4:4:4 or 4:2:0.
pub fn jpeg_encode_chroma(img: &Img, q: f32, quarter: bool) -> Vec<u8> {
    use zenjpeg::encoder::{ChromaSubsampling, EncoderConfig, PixelLayout, Unstoppable};
    let sub = if quarter {
        ChromaSubsampling::Quarter
    } else {
        ChromaSubsampling::None
    };
    let mut enc = EncoderConfig::ycbcr(q, sub)
        .encode_from_bytes(img.info.width, img.info.height, PixelLayout::Rgb8Srgb)
        .unwrap();
    enc.push_packed(&img.pixels, Unstoppable).unwrap();
    enc.finish().unwrap()
}

/// zenjpeg 4:4:4 with its (jpegli) adaptive quantization on or off.
pub fn jpeg_encode_aq(img: &Img, q: f32, aq: bool) -> Vec<u8> {
    use zenjpeg::encoder::{ChromaSubsampling, EncoderConfig, PixelLayout, Unstoppable};
    let cfg = EncoderConfig::ycbcr(q, ChromaSubsampling::None).aq_enabled(aq);
    let mut enc = cfg
        .encode_from_bytes(img.info.width, img.info.height, PixelLayout::Rgb8Srgb)
        .unwrap();
    enc.push_packed(&img.pixels, Unstoppable).unwrap();
    enc.finish().unwrap()
}

pub fn jpeg_decode(data: &[u8]) -> Vec<u8> {
    let r = zenjpeg::decoder::Decoder::new()
        .decode(data, zenjpeg::encoder::Unstoppable)
        .unwrap();
    r.pixels_u8().unwrap().to_vec()
}

/// One measured point.
pub struct Point {
    pub img: usize,
    pub variant: String,
    pub setting: String,
    pub bpp: f64,
    pub ssim2: f64,
    pub tok: f64,
    pub zensim: f64,
    /// Butteraugli libjxl 3-norm (lower is better).
    pub ba3: f64,
    /// Butteraugli max-norm (lower is better).
    pub bamax: f64,
    /// ColorVideoVDP JOD, standard FHD display (0..10, higher is better;
    /// NaN when not computed).
    pub jod: f64,
}

/// Butteraugli (3-norm, max-norm) of an RGB8 reconstruction, default
/// parameters (80-nit intensity target).
pub fn butteraugli_scores(img: &Img, px: &[u8]) -> (f64, f64) {
    let w = img.info.width as usize;
    let h = img.info.height as usize;
    let to = |b: &[u8]| -> Vec<butteraugli::RGB8> {
        b.chunks_exact(3)
            .map(|c| butteraugli::RGB8::new(c[0], c[1], c[2]))
            .collect()
    };
    let a = butteraugli::Img::new(to(&img.pixels), w, h);
    let b = butteraugli::Img::new(to(px), w, h);
    let r = butteraugli::butteraugli(
        a.as_ref(),
        b.as_ref(),
        &butteraugli::ButteraugliParams::default(),
    )
    .unwrap();
    (r.pnorm_3, r.score)
}

/// ColorVideoVDP scorer for the standard FHD display (24", 0.6 m,
/// 200 cd/m2): display model and geometry both `STANDARD_FHD`.
pub fn cvvdp_fhd(w: u32, h: u32) -> cvvdp::Cvvdp {
    let params = cvvdp::CvvdpParams {
        display: cvvdp::DisplayModel::STANDARD_FHD,
        ..cvvdp::CvvdpParams::default()
    };
    cvvdp::Cvvdp::with_geometry(w, h, params, cvvdp::DisplayGeometry::STANDARD_FHD).unwrap()
}

/// JOD (FHD) of an RGB8 reconstruction, with its per-pixel diffmap.
pub fn cvvdp_jod_map(img: &Img, px: &[u8], map: &mut Vec<f32>) -> f64 {
    let mut c = cvvdp_fhd(img.info.width, img.info.height);
    c.score_with_diffmap(&img.pixels, px, map).unwrap() as f64
}

/// Edge-artifact statistics from a per-pixel distortion map (row-major,
/// `w` wide): mean distortion within `r` rows of a stripe boundary (every
/// `stripe` rows) over the mean within `r` rows of the stripe middles
/// (also block boundaries, so only the stripe edge differs); and mean
/// within one column of 8-px block boundaries over the block interiors.
pub fn edge_ratios(map: &[f32], w: usize, stripe: usize, r: usize) -> (f64, f64) {
    let h = map.len() / w;
    let row_mean = |y: usize| {
        map[y * w..(y + 1) * w]
            .iter()
            .map(|&v| v as f64)
            .sum::<f64>()
            / w as f64
    };
    let (mut seam, mut ns, mut mid, mut nm) = (0.0, 0, 0.0, 0);
    let half = stripe / 2;
    for y in 0..h {
        let (k, m) = (y / stripe, y % stripe);
        // Interior stripe boundary above (between rows k*stripe-1 and
        // k*stripe) or below.
        let seam_above = m < r && k >= 1;
        let seam_below = stripe - 1 - m < r && (k + 1) * stripe < h;
        let mid_dist = if m < half { half - 1 - m } else { m - half };
        if seam_above || seam_below {
            seam += row_mean(y);
            ns += 1;
        } else if mid_dist < r && (k + 1) * stripe <= h {
            mid += row_mean(y);
            nm += 1;
        }
    }
    let (mut edge, mut ne, mut inner, mut ni) = (0.0, 0, 0.0, 0);
    for row in map.chunks_exact(w) {
        for (x, &v) in row.iter().enumerate() {
            match x % 8 {
                0 | 7 => {
                    edge += v as f64;
                    ne += 1;
                }
                3 | 4 => {
                    inner += v as f64;
                    ni += 1;
                }
                _ => {}
            }
        }
    }
    (
        (seam / ns.max(1) as f64) / (mid / nm.max(1) as f64).max(1e-30),
        (edge / ne.max(1) as f64) / (inner / ni.max(1) as f64).max(1e-30),
    )
}

/// Writes a distortion map as an 8-bit PNG heat map (black..red..yellow..
/// white), scaled so `scale` maps to full intensity.
pub fn write_heatmap(path: &str, map: &[f32], w: usize, scale: f32) {
    let px: Vec<rgb::RGB8> = map
        .iter()
        .map(|&v| {
            let t = (v / scale).clamp(0.0, 1.0) * 3.0;
            let c = |o: f32| ((t - o).clamp(0.0, 1.0) * 255.0) as u8;
            rgb::RGB8::new(c(0.0), c(1.0), c(2.0))
        })
        .collect();
    write_png(path, &px, w);
}

pub fn write_png(path: &str, px: &[rgb::RGB8], w: usize) {
    let png = zenpng::encode_rgb8(
        imgref::Img::new(px, w, px.len() / w),
        None,
        &zenpng::EncodeConfig::default(),
        &enough::Unstoppable,
        &enough::Unstoppable,
    )
    .unwrap();
    std::fs::write(path, png).unwrap();
}

/// Size vs `base` at equal SSIMULACRA2, zensim, butteraugli 3-norm and
/// butteraugli max-norm.
pub fn metric_tables(pts: &[Point], nimg: usize, base: &str) -> String {
    let mut s = format!("bpp (geo mean) and size vs {base} at equal SSIMULACRA2:\n");
    s += &bd_table(
        pts,
        nimg,
        base,
        &[30.0, 50.0, 60.0, 70.0, 80.0, 85.0, 90.0, 94.0],
        0,
    );
    s += "at equal zensim (codec_target):\n";
    s += &bd_table(
        pts,
        nimg,
        base,
        &[40.0, 55.0, 65.0, 75.0, 80.0, 85.0, 90.0, 92.0],
        2,
    );
    s += "at equal butteraugli 3-norm:\n";
    s += &bd_table(
        pts,
        nimg,
        base,
        &[4.0, 3.0, 2.0, 1.5, 1.0, 0.8, 0.6, 0.4],
        3,
    );
    s += "at equal butteraugli max-norm:\n";
    s += &bd_table(
        pts,
        nimg,
        base,
        &[10.0, 7.0, 5.0, 4.0, 3.0, 2.0, 1.5, 1.0],
        4,
    );
    s
}

pub fn zensim_score(img: &Img, px: &[u8]) -> f64 {
    let w = img.info.width as usize;
    let h = img.info.height as usize;
    let a: Vec<[u8; 3]> = img
        .pixels
        .chunks_exact(3)
        .map(|c| [c[0], c[1], c[2]])
        .collect();
    let b: Vec<[u8; 3]> = px.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
    let z = zensim::Zensim::new(zensim::ZensimProfile::codec_target());
    z.compute(
        &zensim::RgbSlice::new(&a, w, h),
        &zensim::RgbSlice::new(&b, w, h),
    )
    .unwrap()
    .score()
}

pub fn sweep(
    imgs: &[Img],
    variants: &[(String, Vec<(String, LossyCfg)>)],
    jpeg_q: &[f32],
) -> Vec<Point> {
    imgs.par_iter()
        .enumerate()
        .flat_map(|(ii, img)| {
            let refslice = fast_ssim2::PixelSlice::new(
                &img.pixels,
                img.info.width,
                img.info.height,
                img.info.width as usize * 3,
                fast_ssim2::PixelDescriptor::RGB8_SRGB,
            )
            .unwrap();
            let reference = fast_ssim2::Ssimulacra2Reference::new(&refslice).unwrap();
            let npx = img.pixels_count() as f64;
            let mut pts = Vec::new();
            for &q in jpeg_q {
                let enc = jpeg_encode(img, q);
                let dec = jpeg_decode(&enc);
                let ba = butteraugli_scores(img, &dec);
                pts.push(Point {
                    img: ii,
                    variant: "zenjpeg444".into(),
                    setting: format!("q{q}"),
                    bpp: enc.len() as f64 * 8.0 / npx,
                    ssim2: ssim2(&reference, img, &dec),
                    tok: 0.0,
                    zensim: zensim_score(img, &dec),
                    ba3: ba.0,
                    bamax: ba.1,
                    jod: f64::NAN,
                });
            }
            for (vn, settings) in variants {
                for (sn, cfg) in settings {
                    let r = run(img, cfg);
                    let ba = butteraugli_scores(img, &r.recon);
                    pts.push(Point {
                        img: ii,
                        variant: vn.clone(),
                        setting: sn.clone(),
                        bpp: (r.bits_ac + r.bits_dc + r.bits_eob) / npx,
                        ssim2: ssim2(&reference, img, &r.recon),
                        tok: r.tokens_per_px,
                        zensim: zensim_score(img, &r.recon),
                        ba3: ba.0,
                        bamax: ba.1,
                        jod: f64::NAN,
                    });
                }
            }
            pts
        })
        .collect()
}

/// Per-image log-bpp at a target quality, by linear interpolation in
/// (quality, ln bpp) along the sweep (sorted by quality). None if the target
/// is outside the swept range.
fn bpp_at(points: &[(f64, f64)], target: f64) -> Option<f64> {
    let mut p: Vec<(f64, f64)> = points.to_vec();
    p.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    for w in p.windows(2) {
        let (q0, b0) = w[0];
        let (q1, b1) = w[1];
        if target >= q0 && target <= q1 && q1 > q0 {
            let t = (target - q0) / (q1 - q0);
            return Some(b0.ln() + t * (b1.ln() - b0.ln()));
        }
    }
    None
}

/// Geometric-mean bpp ratio of each variant vs `base` at quality targets,
/// over images where both reach the target. `metric`: 0 SSIMULACRA2, 2 zensim, 3 butteraugli 3-norm, 4 butteraugli
/// max-norm, 5 cvvdp JOD (1 was PSNR, removed).
pub fn bd_table(pts: &[Point], nimg: usize, base: &str, targets: &[f64], metric: u8) -> String {
    let mut variants: Vec<String> = pts.iter().map(|p| p.variant.clone()).collect();
    variants.sort();
    variants.dedup();
    let curve = |v: &str, i: usize| -> Vec<(f64, f64)> {
        pts.iter()
            .filter(|p| p.variant == v && p.img == i)
            .map(|p| {
                (
                    match metric {
                        0 => p.ssim2,
                        2 => p.zensim,
                        3 => p.ba3,
                        4 => p.bamax,
                        _ => p.jod,
                    },
                    p.bpp,
                )
            })
            .filter(|p| !p.0.is_nan())
            .collect()
    };
    let mut s = format!(
        "{:<22}",
        [
            "SSIM2 target",
            "-",
            "zensim target",
            "BA 3-norm target",
            "BA max target",
            "cvvdp JOD target",
        ][metric as usize]
    );
    for t in targets {
        s += &format!(" {:>13}", t);
    }
    s += "\n";
    for v in &variants {
        s += &format!("{v:<22}");
        for &t in targets {
            let mut sum = 0.0;
            let mut n = 0;
            let mut bsum = 0.0;
            for i in 0..nimg {
                if let (Some(a), Some(b)) = (bpp_at(&curve(v, i), t), bpp_at(&curve(base, i), t)) {
                    sum += a - b;
                    bsum += a;
                    n += 1;
                }
            }
            if n == 0 {
                s += &format!(" {:>13}", "-");
            } else {
                s += &format!(
                    " {:>5.2}b {:>+5.1}%",
                    (bsum / n as f64).exp(),
                    ((sum / n as f64).exp() - 1.0) * 100.0
                );
            }
        }
        s += "\n";
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lift_is_scaled_dct_and_reversible() {
        let s = lift_scales();
        let prod: f64 = s.iter().product();
        assert!((prod.abs() - 1.0).abs() < 1e-9, "{s:?}");
        let l = Lift::new(true);
        let mut seed = 12345u64;
        for _ in 0..10000 {
            let mut x = [0.0; 8];
            for v in x.iter_mut() {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                *v = ((seed >> 33) % 65536) as f64 - 32768.0;
            }
            let orig = x;
            l.fwd8(&mut x);
            for v in x {
                assert_eq!(v, v.round());
            }
            l.inv8(&mut x);
            assert_eq!(x, orig);
        }
    }

    #[test]
    fn ycocg_reversible() {
        for r in (0..256).step_by(3) {
            for g in (0..256).step_by(5) {
                for b in 0..256 {
                    let (y, co, cg) = ycocg_f(r, g, b);
                    assert_eq!(ycocg_i(y, co, cg), (r, g, b));
                }
            }
        }
    }
}

/// Context options for the AC/EOB model (E3).
#[derive(Clone, Copy, Debug)]
pub struct CtxCfg {
    /// 0 none, 1 above only, 2 left + above.
    pub nb: u8,
    /// Block activity class from the (already decoded) DC plane.
    pub dc_act: bool,
    /// Chroma EOB context from the luma EOB of the same block.
    pub luma_eob: bool,
    /// EOB context source: 0 above block, 1 none, 2 previous block in the
    /// lane (bx - 16, same row), 3 = 2 for luma, luma EOB for chroma.
    pub eob_mode: u8,
}

/// Ideal adaptive cost (bits) of AC + EOB + DC under `cc`.
pub fn model_bits(r: &LossyResult, cc: CtxCfg) -> (f64, f64, f64) {
    let (bw, bh) = (r.bw, r.bh);
    let nact = if cc.dc_act { 3 } else { 1 };
    let mut ac = Adaptive::new(3 * 64 * 8 * nact, NTOK, 0.25);
    let mut eob = Adaptive::new(3 * 8 * nact * 4, 65, 0.25);
    let mut dcm = Adaptive::new(3 * 8, NTOK, 0.25);
    let mut extra = 0.0;
    let mut luma_last = vec![0usize; bw * bh];
    for ch in 0..3 {
        let c = &r.coefs[ch];
        let last_of = |q: &[i64; 64]| (1..64).rev().find(|&k| q[ZIGZAG[k]] != 0).unwrap_or(0);
        let dc = |bx: usize, by: usize| c[by * bw + bx][0];
        for by in 0..bh {
            for bx in 0..bw {
                let q = &c[by * bw + bx];
                let act = if cc.dc_act {
                    let l = dc(bx.saturating_sub(1), by);
                    let rr = dc((bx + 1).min(bw - 1), by);
                    let u = dc(bx, by.saturating_sub(1));
                    let d = dc(bx, (by + 1).min(bh - 1));
                    let a = (l - rr).abs() + (u - d).abs();
                    match a {
                        0..=1 => 0,
                        2..=6 => 1,
                        _ => 2,
                    }
                } else {
                    0
                };
                let last = last_of(q);
                let above_eob = if by > 0 {
                    last_of(&c[(by - 1) * bw + bx])
                } else {
                    0
                };
                let lane_prev = if bx >= 16 {
                    last_of(&c[by * bw + bx - 16])
                } else {
                    0
                };
                let ectx = match (cc.eob_mode, cc.nb) {
                    (1, _) => 0,
                    (2, _) => lane_prev,
                    (3, _) => {
                        if ch == 0 {
                            lane_prev
                        } else {
                            luma_last[by * bw + bx]
                        }
                    }
                    (4, _) => above_eob,
                    (_, 0) => 0,
                    (_, 1) => above_eob,
                    _ => {
                        let a = if by > 0 {
                            last_of(&c[(by - 1) * bw + bx])
                        } else {
                            0
                        };
                        let l = if bx > 0 {
                            last_of(&c[by * bw + bx - 1])
                        } else {
                            a
                        };
                        (a + l) / 2
                    }
                };
                let lctx = if cc.luma_eob && ch > 0 {
                    (luma_last[by * bw + bx] * 4 / 64).min(3)
                } else {
                    0
                };
                eob.code(
                    ((ch * 8 + (ectx * 8 / 64).min(7)) * nact + act) * 4 + lctx,
                    last,
                );
                if ch == 0 {
                    luma_last[by * bw + bx] = last;
                }
                for k in 1..=last {
                    let pos = ZIGZAG[k];
                    let a = if by > 0 {
                        c[(by - 1) * bw + bx][pos].abs()
                    } else {
                        0
                    };
                    let l = if bx > 0 {
                        c[by * bw + bx - 1][pos].abs()
                    } else {
                        0
                    };
                    let oc = match last {
                        0..=2 => 0,
                        3..=5 => 1,
                        6..=9 => 2,
                        10..=15 => 3,
                        16..=24 => 4,
                        25..=39 => 5,
                        40..=52 => 6,
                        _ => 7,
                    };
                    let nb = match cc.nb {
                        0 => 0,
                        1 => mag_class(2 * a),
                        3 => oc,
                        4 => (oc / 2) * 2 + (a > 0) as usize,
                        5 => {
                            let rem = last - k; // coefficients left in this block
                            match rem {
                                0 => 0,
                                1..=2 => 1,
                                3..=6 => 2,
                                7..=14 => 3,
                                15..=30 => 4,
                                _ => 5,
                            }
                        }
                        6 => {
                            let rem = last - k;
                            let r = match rem {
                                0 => 0,
                                1..=2 => 1,
                                3..=6 => 2,
                                7..=14 => 3,
                                15..=30 => 4,
                                _ => 5,
                            };
                            r * 2 + (a > 0) as usize
                        }
                        _ => mag_class(a + l),
                    };
                    let (t, e) = token(zz(q[pos]));
                    ac.code(((ch * 64 + k) * 8 + nb) * nact + act, t);
                    extra += e as f64;
                }
                // DC: MED
                let d = q[0];
                let left = if bx > 0 {
                    dc(bx - 1, by)
                } else if by > 0 {
                    dc(bx, by - 1)
                } else {
                    0
                };
                let up = if by > 0 { dc(bx, by - 1) } else { left };
                let ul = if bx > 0 && by > 0 {
                    dc(bx - 1, by - 1)
                } else {
                    up
                };
                let pred = if ul >= left.max(up) {
                    left.min(up)
                } else if ul <= left.min(up) {
                    left.max(up)
                } else {
                    left + up - ul
                };
                let (t, e) = token(zz(d - pred));
                dcm.code(ch * 8 + mag_class((left - ul).abs() + (up - ul).abs()), t);
                extra += e as f64;
            }
        }
    }
    (ac.bits + extra, eob.bits, dcm.bits)
}

pub fn imij_lossy(img: &Img, q: f32, color: imij::LossyColor) -> Vec<u8> {
    let mut cfg = imij::ImiEncoderConfig::lossy(q).tune_lossy(|t| t.color = color);
    // IMIJ_SPLITHUF=1 selects split with Huffman side information.
    if std::env::var_os("IMIJ_SPLITHUF").is_some() {
        cfg = cfg.tune_lossy(|t| t.entropy = imij::LossyEntropy::SplitHuf);
    }
    // IMIJ_SB overrides the stripe height in block rows.
    if let Ok(v) = std::env::var("IMIJ_SB") {
        cfg = cfg.tune_lossy(|t| t.stripe_block_rows = v.parse().unwrap());
    }
    cfg.encode_samples(
        imij::ImageRef::new(&img.pixels, img.info.row_bytes(), &img.info),
        &imij::Metadata::default(),
    )
    .unwrap()
}

/// E4: real imij lossy files vs zenjpeg.
pub fn sweep_real(
    imgs: &[Img],
    qs: &[f32],
    colors: &[(&str, imij::LossyColor)],
    jpeg_q: &[f32],
) -> Vec<Point> {
    imgs.par_iter()
        .enumerate()
        .flat_map(|(ii, img)| {
            let refslice = fast_ssim2::PixelSlice::new(
                &img.pixels,
                img.info.width,
                img.info.height,
                img.info.width as usize * 3,
                fast_ssim2::PixelDescriptor::RGB8_SRGB,
            )
            .unwrap();
            let reference = fast_ssim2::Ssimulacra2Reference::new(&refslice).unwrap();
            let npx = img.pixels_count() as f64;
            let mut pts = Vec::new();
            let mut add = |variant: &str, setting: String, n: usize, dec: &[u8]| {
                let ba = butteraugli_scores(img, dec);
                pts.push(Point {
                    img: ii,
                    variant: variant.into(),
                    setting,
                    bpp: n as f64 * 8.0 / npx,
                    ssim2: ssim2(&reference, img, dec),
                    tok: 0.0,
                    zensim: zensim_score(img, dec),
                    ba3: ba.0,
                    bamax: ba.1,
                    jod: f64::NAN,
                });
            };
            for &q in jpeg_q {
                let enc = jpeg_encode(img, q);
                add("zenjpeg444", format!("q{q}"), enc.len(), &jpeg_decode(&enc));
            }
            if std::env::var_os("IMIJ_JPEG_NOAQ").is_some() {
                for &q in jpeg_q {
                    let enc = jpeg_encode_aq(img, q, false);
                    add(
                        "zenjpeg444_noaq",
                        format!("q{q}"),
                        enc.len(),
                        &jpeg_decode(&enc),
                    );
                }
            }
            for (name, color) in colors {
                for &q in qs {
                    let enc = imij_lossy(img, q, *color);
                    let (_, dec) = imij::decode(&enc).unwrap();
                    add(name, format!("q{q}"), enc.len(), &dec);
                }
            }
            pts
        })
        .collect()
}

/// Token-count/size model for run-level coding (E3c): per block and
/// channel an EOB token (as in the dense model), then for each nonzero AC
/// coefficient one joint (zero run, magnitude class) token plus raw bits
/// (sign and magnitude remainder). Returns (bits, tokens) for the AC+EOB
/// part; DC is the same as the dense model and is excluded.
/// `run_cap`: longest run in one token (longer runs emit a run-only token).
/// Dense packed model: per block and channel an EOB token (as in the
/// run-level model), then per group of `g` zigzag positions up to the EOB a
/// width token w = bitlen(max |v|) (context: channel, group index, previous
/// width in the block) and raw bits. `sign_all`: w + 1 bits two's
/// complement per coefficient; else w magnitude bits + a sign bit for
/// nonzero values. Returns (bits, tokens).
pub fn model_packed(r: &LossyResult, g: usize, sign_all: bool) -> (f64, usize) {
    model_packed_mode(r, g, if sign_all { 1 } else { 0 })
}

/// `mode` 0: w magnitude bits + sign per nonzero; 1: w + 1 bits per
/// coefficient; 2: joint (w, nonzero mask) token (g <= 4), then per nonzero
/// value (w > 1 ? w : 0) magnitude bits + sign.
pub fn model_packed_mode(r: &LossyResult, g: usize, mode: u8) -> (f64, usize) {
    let (bw, bh) = (r.bw, r.bh);
    let mut eob = Adaptive::new(3 * 8, 65, 0.25);
    let nsym = if mode == 2 { 17 << g } else { 17 };
    let mut wid = Adaptive::new(3 * 16 * 17, nsym, 0.25);
    let (mut raw, mut ntok) = (0.0f64, 0usize);
    let mut luma_prev = 0usize;
    for by in 0..bh {
        for bx in 0..bw {
            let mut luma = 0usize;
            for ch in 0..3 {
                let q = &r.coefs[ch][by * bw + bx];
                let last = (1..64).rev().find(|&k| q[ZIGZAG[k]] != 0).unwrap_or(0);
                let ectx = if ch == 0 { luma_prev } else { luma };
                eob.code(ch * 8 + (64 - (ectx as u64).leading_zeros()) as usize, last);
                ntok += 1;
                if ch == 0 {
                    luma = last;
                    luma_prev = last;
                }
                let mut prev = 16usize;
                let mut s0 = 1;
                let mut gi = 0;
                while s0 <= last {
                    let e = (s0 + g - 1).min(last);
                    let m = (s0..=e).map(|k| q[ZIGZAG[k]].unsigned_abs()).max().unwrap();
                    let w = (64 - m.leading_zeros()) as usize;
                    let mask = (s0..=e)
                        .enumerate()
                        .fold(0usize, |m, (i, k)| m | ((q[ZIGZAG[k]] != 0) as usize) << i);
                    let sym = if mode == 2 {
                        (w.min(16) << g) | mask
                    } else {
                        w.min(16)
                    };
                    wid.code((ch * 16 + gi.min(15)) * 17 + prev.min(16), sym);
                    ntok += 1;
                    for k in s0..=e {
                        let v = q[ZIGZAG[k]];
                        raw += match mode {
                            1 => {
                                if w > 0 {
                                    (w + 1) as f64
                                } else {
                                    0.0
                                }
                            }
                            2 => {
                                if v != 0 {
                                    (if w > 1 { w } else { 0 }) as f64 + 1.0
                                } else {
                                    0.0
                                }
                            }
                            _ => w as f64 + (v != 0) as u32 as f64,
                        };
                    }
                    prev = w;
                    s0 = e + 1;
                    gi += 1;
                }
            }
        }
    }
    (eob.bits + wid.bits + raw, ntok)
}

/// Packed with no entropy coder at all (pure SIMD unpack): per block and
/// channel the group count in 5 bits, per group of `g` zigzag positions its
/// width in 5 bits, then w magnitude bits per coefficient and a sign bit per
/// nonzero. Returns (bits, fields).
pub fn model_packed_raw(r: &LossyResult, g: usize) -> (f64, usize) {
    let (bw, bh) = (r.bw, r.bh);
    let (mut bits, mut fields) = (0.0f64, 0usize);
    for ch in 0..3 {
        for q in r.coefs[ch].iter().take(bw * bh) {
            let last = (1..64).rev().find(|&k| q[ZIGZAG[k]] != 0).unwrap_or(0);
            let ng = last.div_ceil(g);
            bits += 5.0;
            fields += 1;
            for gi in 0..ng {
                let (s0, e) = (1 + gi * g, (gi * g + g).min(63));
                let m = (s0..=e).map(|k| q[ZIGZAG[k]].unsigned_abs()).max().unwrap();
                let w = (64 - m.leading_zeros()) as f64;
                bits += 5.0;
                fields += 1;
                for k in s0..=e {
                    bits += w + (q[ZIGZAG[k]] != 0) as u32 as f64;
                }
            }
        }
    }
    (bits, fields)
}

/// Packed model with arbitrary groups and richer width contexts.
/// `starts`: zigzag start position of each group (first is 1), ascending.
/// ctx 0: (ch, group, prev width); 1: (ch, group band, prev class,
/// above-block class at the same group, or luma's for chroma).
/// Raw: w magnitude bits + sign per nonzero. Returns (bits, tokens).
pub fn model_packed_groups(r: &LossyResult, starts: &[usize], ctxm: u8) -> (f64, usize) {
    let (bw, bh) = (r.bw, r.bh);
    let ng = starts.len();
    let end = |gi: usize| if gi + 1 < ng { starts[gi + 1] - 1 } else { 63 };
    let mut eob = Adaptive::new(3 * 8 * 8, 65, 0.25);
    let mut wid = Adaptive::new(3 * 16 * 17 * 6, 17, 0.25);
    let mut w1mask = Adaptive::new(3 * 6 * 2, 16, 0.25);
    let (mut raw, mut ntok) = (0.0f64, 0usize);
    let cls = |w: usize| w.min(4);
    // widths of the previous block row per (bx, ch, group) for the above context
    let mut above = vec![[[0u8; 32]; 3]; bw];
    let mut cur_row = vec![[[0u8; 32]; 3]; bw];
    let mut luma_prev = 0usize;
    for by in 0..bh {
        for bx in 0..bw {
            let mut luma = 0usize;
            let mut luma_w = [0u8; 32];
            for ch in 0..3 {
                let q = &r.coefs[ch][by * bw + bx];
                let last = (1..64).rev().find(|&k| q[ZIGZAG[k]] != 0).unwrap_or(0);
                let ectx = if ch == 0 { luma_prev } else { luma };
                let actx = if ctxm == 1 && by > 0 {
                    (above[bx][ch].iter().filter(|&&w| w > 0).count()).min(7)
                } else {
                    0
                };
                eob.code(
                    (ch * 8 + (64 - (ectx as u64).leading_zeros()) as usize) * 8 + actx,
                    last,
                );
                ntok += 1;
                if ch == 0 {
                    luma = last;
                    luma_prev = last;
                }
                let mut ws = [0u8; 32];
                let mut prev = 16usize;
                for gi in 0..ng {
                    let s0 = starts[gi];
                    if s0 > last {
                        break;
                    }
                    let e = end(gi).min(last);
                    let m = (s0..=e).map(|k| q[ZIGZAG[k]].unsigned_abs()).max().unwrap();
                    let w = (64 - m.leading_zeros()) as usize;
                    let c = if ctxm == 0 {
                        (ch * 16 + gi.min(15)) * 17 + prev.min(16)
                    } else {
                        let nb = if ch > 0 {
                            luma_w[gi] as usize
                        } else if by > 0 {
                            above[bx][ch][gi] as usize
                        } else {
                            5
                        };
                        let band = [0, 1, 2, 3, 3, 4, 4, 4, 5, 5, 5, 5, 5, 5, 5, 5][gi.min(15)];
                        (((ch * 6 + band) * 6 + cls(prev.min(5)).min(5)) * 6 + nb.min(5)) * 1
                    };
                    wid.code(c, w.min(16));
                    ntok += 1;
                    ws[gi] = w.min(255) as u8;
                    if ctxm == 2 && w == 1 {
                        // Entropy-coded nonzero mask for width-1 groups.
                        let mask = (s0..=e)
                            .enumerate()
                            .fold(0usize, |m, (i, k)| m | ((q[ZIGZAG[k]] != 0) as usize) << i);
                        let band = [0, 1, 2, 3, 3, 4, 4, 4, 5, 5, 5, 5, 5, 5, 5, 5][gi.min(15)];
                        let full = (e - s0 + 1 == 4) as usize;
                        w1mask.code((ch * 6 + band) * 2 + full, mask);
                        ntok += 1;
                        raw += (s0..=e).filter(|&k| q[ZIGZAG[k]] != 0).count() as f64;
                    } else {
                        for k in s0..=e {
                            let v = q[ZIGZAG[k]];
                            raw += w as f64 + (v != 0) as u32 as f64;
                        }
                    }
                    prev = w;
                }
                if ch == 0 {
                    luma_w = ws;
                }
                cur_row[bx][ch] = ws;
            }
        }
        core::mem::swap(&mut above, &mut cur_row);
    }
    (eob.bits + wid.bits + w1mask.bits + raw, ntok)
}

/// Group starts: fixed size `g`.
pub fn groups_fixed(g: usize) -> Vec<usize> {
    (0..).map(|i| 1 + i * g).take_while(|&s| s < 64).collect()
}

/// Group starts: one group per zigzag anti-diagonal (u + v = d, d >= 1).
pub fn groups_diag() -> Vec<usize> {
    let mut v = Vec::new();
    let mut pos = 1;
    for d in 1..15usize {
        v.push(pos);
        pos += if d < 8 { d + 1 } else { 15 - d };
    }
    v
}

/// Decoder-oriented packed model, groups of 4: per block and channel a
/// count token n (EOB exact when `exact_eob`, else the group count 0..=16;
/// zeros after the last nonzero in the last group are raw), then per group
/// a width token (ctx: ch, band, previous width class, and for `nbctx` the
/// luma width class at the same group for chroma / the group count class
/// for luma), raw w magnitude bits + sign per nonzero. (bits, tokens).
pub fn model_packed_dec(r: &LossyResult, exact_eob: bool, nbctx: bool) -> (f64, usize) {
    let (bw, bh) = (r.bw, r.bh);
    let mut cnt = Adaptive::new(3 * 8, 65, 0.25);
    let mut wid = Adaptive::new(3 * 6 * 6 * 6, 24, 0.25);
    let (mut raw, mut ntok) = (0.0f64, 0usize);
    let cls = |w: usize| w.min(5);
    let band = |gi: usize| [0, 1, 2, 3, 3, 4, 4, 4, 5, 5, 5, 5, 5, 5, 5, 5][gi];
    let mut luma_prev = 0usize;
    for by in 0..bh {
        for bx in 0..bw {
            let mut luma = 0usize;
            let mut luma_w = [0usize; 16];
            for ch in 0..3 {
                let q = &r.coefs[ch][by * bw + bx];
                let last = (1..64).rev().find(|&k| q[ZIGZAG[k]] != 0).unwrap_or(0);
                let ng = last.div_ceil(4);
                let n = if exact_eob { last } else { ng };
                let ectx = if ch == 0 { luma_prev } else { luma };
                cnt.code(ch * 8 + (64 - (ectx as u64).leading_zeros()) as usize, n);
                ntok += 1;
                if ch == 0 {
                    luma = n;
                    luma_prev = n;
                }
                let mut prev = 5usize;
                for gi in 0..ng {
                    let s0 = 1 + gi * 4;
                    let e = if exact_eob {
                        (s0 + 3).min(last)
                    } else {
                        (s0 + 3).min(63)
                    };
                    let m = (s0..=e).map(|k| q[ZIGZAG[k]].unsigned_abs()).max().unwrap();
                    let w = (64 - m.leading_zeros()) as usize;
                    let nb = if !nbctx {
                        0
                    } else if ch > 0 {
                        cls(luma_w[gi])
                    } else {
                        (ng - gi).min(5)
                    };
                    wid.code(((ch * 6 + band(gi)) * 6 + cls(prev)) * 6 + nb, w.min(23));
                    ntok += 1;
                    if ch == 0 {
                        luma_w[gi] = w;
                    }
                    for k in s0..=e {
                        raw += w as f64 + (q[ZIGZAG[k]] != 0) as u32 as f64;
                    }
                    prev = w;
                }
                if ch == 0 {
                    for v in luma_w.iter_mut().skip(ng) {
                        *v = 0;
                    }
                }
            }
        }
    }
    (cnt.bits + wid.bits + raw, ntok)
}

/// Packed model over arbitrary coefficient groups (raster positions, DC
/// excluded) in scan order: per block and channel a group-count token, then
/// per group a width token (ctx as `model_packed_dec` with nbctx) and raw w
/// magnitude bits + sign per nonzero. (bits, tokens).
pub fn model_packed_shapes(r: &LossyResult, groups: &[Vec<usize>]) -> (f64, usize) {
    let (bw, bh) = (r.bw, r.bh);
    let ngr = groups.len();
    let mut cnt = Adaptive::new(3 * 8, ngr + 1, 0.25);
    let mut wid = Adaptive::new(3 * 6 * 6 * 6, 24, 0.25);
    let (mut raw, mut ntok) = (0.0f64, 0usize);
    let cls = |w: usize| w.min(5);
    let band = |gi: usize| [0, 1, 2, 3, 3, 4, 4, 4, 5, 5, 5, 5, 5, 5, 5, 5][gi.min(15)];
    let mut luma_prev = 0usize;
    for by in 0..bh {
        for bx in 0..bw {
            let mut luma = 0usize;
            let mut luma_w = vec![0usize; ngr];
            for ch in 0..3 {
                let q = &r.coefs[ch][by * bw + bx];
                let ws: Vec<usize> = groups
                    .iter()
                    .map(|g| {
                        let m = g.iter().map(|&p| q[p].unsigned_abs()).max().unwrap();
                        (64 - m.leading_zeros()) as usize
                    })
                    .collect();
                let ng = ws.iter().rposition(|&w| w > 0).map_or(0, |i| i + 1);
                let ectx = if ch == 0 { luma_prev } else { luma };
                cnt.code(ch * 8 + (64 - (ectx as u64).leading_zeros()) as usize, ng);
                ntok += 1;
                if ch == 0 {
                    luma = ng;
                    luma_prev = ng;
                }
                let mut prev = 5usize;
                for gi in 0..ng {
                    let w = ws[gi];
                    let nb = if ch > 0 {
                        cls(luma_w[gi])
                    } else {
                        (ng - gi).min(5)
                    };
                    wid.code(((ch * 6 + band(gi)) * 6 + cls(prev)) * 6 + nb, w.min(23));
                    ntok += 1;
                    for &p in &groups[gi] {
                        raw += w as f64 + (q[p] != 0) as u32 as f64;
                    }
                    prev = w;
                }
                if ch == 0 {
                    for (gi, v) in luma_w.iter_mut().enumerate() {
                        *v = if gi < ng { ws[gi] } else { 0 };
                    }
                }
            }
        }
    }
    (cnt.bits + wid.bits + raw, ntok)
}

/// Zigzag groups of 4 (raster positions).
pub fn shapes_zigzag4() -> Vec<Vec<usize>> {
    (0..16)
        .map(|g| {
            (1 + 4 * g..(5 + 4 * g).min(64))
                .map(|k| ZIGZAG[k])
                .collect()
        })
        .collect()
}

/// Half-row groups (row r, columns 4h..4h+3), ordered by r + 4h then r.
pub fn shapes_halfrow() -> Vec<Vec<usize>> {
    let mut g: Vec<(usize, usize, Vec<usize>)> = (0..8)
        .flat_map(|r| (0..2).map(move |h| (r, h)))
        .map(|(r, h)| {
            (
                r + 4 * h,
                r,
                (4 * h..4 * h + 4)
                    .map(|c| r * 8 + c)
                    .filter(|&p| p != 0)
                    .collect(),
            )
        })
        .collect();
    g.sort_by_key(|x| (x.0, x.1));
    g.into_iter().map(|x| x.2).collect()
}

/// 2x2 groups, ordered by zigzag over the 4x4 grid of squares.
pub fn shapes_square() -> Vec<Vec<usize>> {
    let mut g: Vec<(usize, usize, Vec<usize>)> = (0..4)
        .flat_map(|i| (0..4).map(move |j| (i, j)))
        .map(|(i, j)| {
            let ps = [
                (2 * i) * 8 + 2 * j,
                (2 * i) * 8 + 2 * j + 1,
                (2 * i + 1) * 8 + 2 * j,
                (2 * i + 1) * 8 + 2 * j + 1,
            ];
            (
                i + j,
                if (i + j) % 2 == 0 { j } else { i },
                ps.into_iter().filter(|&p| p != 0).collect(),
            )
        })
        .collect();
    g.sort_by_key(|x| (x.0, x.1));
    g.into_iter().map(|x| x.2).collect()
}

pub fn model_runlevel(r: &LossyResult, run_cap: usize, ctx_rem: bool) -> (f64, usize, f64, usize) {
    let (bw, bh) = (r.bw, r.bh);
    // Joint alphabet: run 0..=run_cap x 8 magnitude classes, plus ZRL.
    let nsym = (run_cap + 1) * 8 + 1;
    let zrl = nsym - 1;
    let mclass = |a: u32| -> (usize, u32) {
        match a {
            1 => (0, 0),
            2 => (1, 0),
            3..=4 => (2, 1),
            5..=8 => (3, 2),
            9..=16 => (4, 3),
            17..=32 => (5, 4),
            33..=64 => (6, 5),
            _ => (7, 32 - (a - 1).leading_zeros() - 1),
        }
    };
    let kband = |k: usize| (64 - (k as u64).leading_zeros()) as usize; // 1..=6
    let mut ac = Adaptive::new(3 * 7 * 6, nsym, 0.25);
    let mut dense = Adaptive::new(3 * 64 * 6, NTOK, 0.25);
    let mut eob = Adaptive::new(3 * 8, 65, 0.25);
    let (mut raw, mut ntok, mut draw, mut dtok) = (0.0f64, 0usize, 0.0f64, 0usize);
    let mut luma_prev = 0usize;
    for by in 0..bh {
        for bx in 0..bw {
            let mut luma = 0usize;
            for ch in 0..3 {
                let q = &r.coefs[ch][by * bw + bx];
                let last = (1..64).rev().find(|&k| q[ZIGZAG[k]] != 0).unwrap_or(0);
                let ectx = if ch == 0 { luma_prev } else { luma };
                eob.code(ch * 8 + (64 - (ectx as u64).leading_zeros()) as usize, last);
                ntok += 1;
                dtok += 1;
                if ch == 0 {
                    luma = last;
                    luma_prev = last;
                }
                // dense reference with the codec's contexts
                for k in 1..=last {
                    let rem = last - k;
                    let rc = ((usize::BITS - (rem + 1).leading_zeros() - 1) as usize).min(5);
                    let (t, e) = token(zz(q[ZIGZAG[k]]));
                    dense.code((ch * 64 + k) * 6 + rc, t);
                    draw += e as f64;
                    dtok += 1;
                }
                // run-level
                let mut run = 0usize;
                for k in 1..=last {
                    let v = q[ZIGZAG[k]];
                    if v == 0 {
                        run += 1;
                        continue;
                    }
                    while run > run_cap {
                        let kb = kband(k);
                        ac.code((ch * 7 + kb) * 6 + 0, zrl);
                        ntok += 1;
                        run -= run_cap + 1;
                    }
                    let (mc, eb) = mclass(v.unsigned_abs() as u32);
                    let rc = if ctx_rem {
                        let rem = last - k;
                        ((usize::BITS - (rem + 1).leading_zeros() - 1) as usize).min(5)
                    } else {
                        0
                    };
                    ac.code((ch * 7 + kband(k)) * 6 + rc, run * 8 + mc);
                    raw += 1.0 + eb as f64;
                    ntok += 1;
                    run = 0;
                }
            }
        }
    }
    (
        ac.bits + raw + eob.bits,
        ntok,
        dense.bits + draw + eob.bits,
        dtok,
    )
}

/// Exact model of the codec's run-level tokens (E3d) with a configurable
/// magnitude split: magnitudes 1..=direct are their own class; above, a
/// class is (exponent, `mant` mantissa bits) with the rest raw. Returns
/// (AC+EOB bits, tokens) per image. `rc_at_start`: remaining-class context
/// at the token start (as the codec) instead of the coefficient position.
pub fn model_rl_exact(r: &LossyResult, max_run: usize, direct: u32, mant: u32) -> (f64, usize) {
    let (bw, bh) = (r.bw, r.bh);
    let class = |a: u32| -> (usize, u32) {
        // returns (class, raw bits excluding sign)
        if a <= direct {
            return ((a - 1) as usize, 0);
        }
        let v = a - direct - 1; // >= 0
        let nb = 32 - v.leading_zeros(); // bit length
        if nb <= mant {
            return ((direct + v) as usize, 0);
        }
        let e = nb - mant; // >= 1
        let base = direct as usize + (1usize << mant);
        (
            base + ((e as usize - 1) << mant)
                + ((v >> (nb - 1 - mant + 1)) as usize & ((1 << mant) - 1)),
            nb - mant - 1 + 1,
        )
    };
    let nsym = (max_run + 1) * 64 + 1;
    let zrl = nsym - 1;
    let mut ac = Adaptive::new(3 * 7 * 6, nsym, 0.25);
    let mut eob = Adaptive::new(3 * 8, 65, 0.25);
    let (mut raw, mut ntok) = (0.0f64, 0usize);
    let mut luma_prev = 0usize;
    let ctx = |ch: usize, start: usize, last: usize| -> usize {
        let kc = (64 - (start as u64).leading_zeros()) as usize;
        let rem = last.saturating_sub(start);
        let rc = ((usize::BITS - (rem + 1).leading_zeros() - 1) as usize).min(5);
        (ch * 7 + kc) * 6 + rc
    };
    for by in 0..bh {
        for bx in 0..bw {
            let mut luma = 0usize;
            for ch in 0..3 {
                let q = &r.coefs[ch][by * bw + bx];
                let last = (1..64).rev().find(|&k| q[ZIGZAG[k]] != 0).unwrap_or(0);
                let ectx = if ch == 0 { luma_prev } else { luma };
                eob.code(ch * 8 + (64 - (ectx as u64).leading_zeros()) as usize, last);
                ntok += 1;
                if ch == 0 {
                    luma = last;
                    luma_prev = last;
                }
                let mut start = 1usize;
                for k in 1..=last {
                    let v = q[ZIGZAG[k]];
                    if v == 0 {
                        continue;
                    }
                    let mut run = k - start;
                    while run > max_run {
                        ac.code(ctx(ch, start, last), zrl);
                        ntok += 1;
                        start += max_run + 1;
                        run -= max_run + 1;
                    }
                    let (c, rb) = class(v.unsigned_abs() as u32);
                    assert!(c < 64, "class {c}");
                    ac.code(ctx(ch, start, last), run * 64 + c);
                    raw += 1.0 + rb as f64;
                    ntok += 1;
                    start = k + 1;
                }
            }
        }
    }
    (ac.bits + raw + eob.bits, ntok)
}

pub fn imij_lossy_cfg(img: &Img, cfg: &imij::ImiEncoderConfig) -> Vec<u8> {
    cfg.encode_samples(
        imij::ImageRef::new(&img.pixels, img.info.row_bytes(), &img.info),
        &imij::Metadata::default(),
    )
    .unwrap()
}
