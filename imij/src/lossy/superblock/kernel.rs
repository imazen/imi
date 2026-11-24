//! The inverse transform of merged blocks (SPEC 2.9.5): separable, the
//! vertical pass down every column first, then the horizontal pass along
//! every row, each pass a fixed flowgraph of single f32 multiplies, adds and
//! subtracts (no fused multiply-add), on coefficients pre-scaled by
//! [`super::input_scale`].
//!
//! * The 8-point pass is the AAN pass of the 8x8 transform
//!   ([`crate::lossy::dct`]), operation for operation.
//! * The 16-point pass splits its input into even and odd halves: the even
//!   coefficients through the 8-point pass, the odd ones through an inverse
//!   DCT-IV (four rotations at `(2m + 1) pi / 32` into two 4-point inverse
//!   DCT-IVs, each two rotations at `(2m + 1) pi / 16` and two at `pi / 8`),
//!   then `x[n] = e[n] + o[n]`, `x[15 - n] = e[n] - o[n]`.
//!
//! [`inverse_ref`] is the normative scalar form. The SIMD kernel evaluates
//! the same operations lane-wise, columns (then rows, after a transpose) in
//! the lanes, so every tier and lane width computes identical results. It
//! skips work whose inputs are zero: a pass over all-zero inputs yields
//! zeros, and an input that is zero contributes `x + 0`, `x - 0` or `0 c`,
//! which leave every other value unchanged (up to the sign of a zero, which
//! no later stage observes).

use super::*;
use archmage::prelude::*;
use magetypes::simd::backends::{F32x8Backend, F32x8Convert};
use magetypes::simd::generic::{f32x8 as vf8, i32x8 as vi8};

#[cfg(all(test, feature = "lab"))]
#[path = "kernel_study.rs"]
mod study;

#[cfg(all(test, feature = "lab", feature = "std"))]
#[path = "integer_study.rs"]
mod integer_study;

/// `cos((2m + 1) pi / 32)`, m = 0..4, rounded to f32.
pub(super) const C1: [f32; 4] = [
    f32::from_bits(0x3f7e_c46d),
    f32::from_bits(0x3f74_fa0b),
    f32::from_bits(0x3f61_c598),
    f32::from_bits(0x3f45_e403),
];
/// `sin((2m + 1) pi / 32)`, m = 0..4, rounded to f32.
pub(super) const S1: [f32; 4] = [
    f32::from_bits(0x3dc8_bd36),
    f32::from_bits(0x3e94_a031),
    f32::from_bits(0x3ef1_5aea),
    f32::from_bits(0x3f22_6799),
];
/// `cos((2m + 1) pi / 16)`, m = 0..2, rounded to f32.
pub(super) const C2: [f32; 2] = [f32::from_bits(0x3f7b_14be), f32::from_bits(0x3f54_db31)];
/// `sin((2m + 1) pi / 16)`, m = 0..2, rounded to f32.
pub(super) const S2: [f32; 2] = [f32::from_bits(0x3e47_c5c2), f32::from_bits(0x3f0e_39da)];
/// `cos(pi / 8)` and `sin(pi / 8)`, rounded to f32.
pub(super) const CA: f32 = f32::from_bits(0x3f6c_835e);
pub(super) const SA: f32 = f32::from_bits(0x3ec3_ef15);

use dct::{AAN_C1, AAN_C2, AAN_C3, AAN_R2};

// ---- the normative scalar reference -------------------------------------

/// The 8-point AAN pass over `v[0..8]` (`dct::idct8`).
#[cfg_attr(not(all(test, feature = "encode")), allow(dead_code))]
fn idct8_ref(v: &mut [f32]) {
    let tmp10 = v[0] + v[4];
    let tmp11 = v[0] - v[4];
    let tmp13 = v[2] + v[6];
    let tmp12 = (v[2] - v[6]) * AAN_R2 - tmp13;
    let e0 = tmp10 + tmp13;
    let e3 = tmp10 - tmp13;
    let e1 = tmp11 + tmp12;
    let e2 = tmp11 - tmp12;
    let z13 = v[5] + v[3];
    let z10 = v[5] - v[3];
    let z11 = v[1] + v[7];
    let z12 = v[1] - v[7];
    let t7 = z11 + z13;
    let t11 = (z11 - z13) * AAN_R2;
    let z5 = (z10 + z12) * AAN_C1;
    let t10 = z5 - z12 * AAN_C2;
    let t12 = z5 - z10 * AAN_C3;
    let t6 = t12 - t7;
    let t5 = t11 - t6;
    let t4 = t10 - t5;
    v[0] = e0 + t7;
    v[7] = e0 - t7;
    v[1] = e1 + t6;
    v[6] = e1 - t6;
    v[2] = e2 + t5;
    v[5] = e2 - t5;
    v[3] = e3 + t4;
    v[4] = e3 - t4;
}

/// The 4-point inverse DCT-IV of the 16-point pass: rotations at pi/16 and
/// 3 pi/16 on (p0, p3) and (p1, p2), then rotations at pi/8 and the
/// combine.
#[cfg_attr(not(all(test, feature = "encode")), allow(dead_code))]
fn iiv4_ref(p: [f32; 4]) -> [f32; 4] {
    let a0 = p[0] * C2[0] - p[3] * S2[0];
    let b0 = p[0] * S2[0] + p[3] * C2[0];
    let a1 = p[1] * C2[1] - p[2] * S2[1];
    let b1 = p[1] * S2[1] + p[2] * C2[1];
    let c0 = a0 * CA + a1 * SA;
    let c1 = a0 * SA - a1 * CA;
    let d0 = b0 * CA - b1 * SA;
    let d1 = b0 * SA + b1 * CA;
    [c0 + d1, c0 - d1, c1 + d0, c1 - d0]
}

/// The 16-point pass over `v[0..16]`.
#[cfg_attr(not(all(test, feature = "encode")), allow(dead_code))]
fn idct16_ref(v: &mut [f32]) {
    let mut e: [f32; 8] = core::array::from_fn(|m| v[2 * m]);
    let g: [f32; 8] = core::array::from_fn(|m| v[2 * m + 1]);
    idct8_ref(&mut e);
    let mut a = [0.0f32; 4];
    let mut b = [0.0f32; 4];
    for m in 0..4 {
        a[m] = g[m] * C1[m] - g[7 - m] * S1[m];
        b[m] = g[m] * S1[m] + g[7 - m] * C1[m];
    }
    let c = iiv4_ref(a);
    let d = iiv4_ref([b[0], -b[1], b[2], -b[3]]);
    let mut o = [0.0f32; 8];
    for t in 0..4 {
        o[2 * t] = c[t] + d[3 - t];
        o[2 * t + 1] = c[t] - d[3 - t];
    }
    for y in 0..8 {
        v[y] = e[y] + o[y];
        v[15 - y] = e[y] - o[y];
    }
}

/// The normative inverse of a merged block: `x[v * 16 + u]` holds the
/// pre-scaled coefficient (v, u) (v < H, u < W); on return `x[y * 16 + x]`
/// holds the pixel at row y, column x. Columns first, then rows. Decoders
/// run [`group`]; tests hold every tier to this reference.
#[cfg_attr(not(all(test, feature = "encode")), allow(dead_code))]
pub(crate) fn inverse_ref(s: Shape, x: &mut [f32; 256]) {
    let (w, h) = (s.w(), s.h());
    let mut col = [0.0f32; 16];
    for u in 0..w {
        for v in 0..h {
            col[v] = x[v * 16 + u];
        }
        if h == 16 {
            idct16_ref(&mut col);
        } else {
            idct8_ref(&mut col);
        }
        for y in 0..h {
            x[y * 16 + u] = col[y];
        }
    }
    for y in 0..h {
        let row = &mut x[y * 16..y * 16 + w];
        if w == 16 {
            idct16_ref(row);
        } else {
            idct8_ref(row);
        }
    }
}

// ---- the SIMD kernel ----------------------------------------------------

/// The flowgraph's constants as vectors.
struct Kc<T: F32x8Backend> {
    z: vf8<T>,
    one: vf8<T>,
    m0: vf8<T>,
    m01: vf8<T>,
    /// -0.0: XOR with it negates (flips the sign bit).
    sign: vf8<T>,
    r2: vf8<T>,
    c1: vf8<T>,
    c2: vf8<T>,
    c3: vf8<T>,
    k1c: [vf8<T>; 4],
    k1s: [vf8<T>; 4],
    k2c: [vf8<T>; 2],
    k2s: [vf8<T>; 2],
    ca: vf8<T>,
    sa: vf8<T>,
}

// No closures in these generic helpers: a closure does not inherit the
// target features of the `#[arcane]` function it is inlined into, so every
// vector operation inside one becomes an out-of-line call (2026-10-10 profile:
// a third of the merged-block time on Zen 4).

impl<T: F32x8Backend> Kc<T> {
    #[inline(always)]
    fn new(token: T) -> Self {
        let idx = vf8::from_array_t(token, [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
        Self {
            z: vf8::zero_t(token),
            one: vf8::splat_t(token, 1.0),
            m0: idx.simd_lt(vf8::splat_t(token, 0.5)),
            m01: idx.simd_lt(vf8::splat_t(token, 1.5)),
            sign: vf8::splat_t(token, -0.0),
            r2: vf8::splat_t(token, AAN_R2),
            c1: vf8::splat_t(token, AAN_C1),
            c2: vf8::splat_t(token, AAN_C2),
            c3: vf8::splat_t(token, AAN_C3),
            k1c: splat4(token, C1),
            k1s: splat4(token, S1),
            k2c: [vf8::splat_t(token, C2[0]), vf8::splat_t(token, C2[1])],
            k2s: [vf8::splat_t(token, S2[0]), vf8::splat_t(token, S2[1])],
            ca: vf8::splat_t(token, CA),
            sa: vf8::splat_t(token, SA),
        }
    }
}

#[inline(always)]
fn splat4<T: F32x8Backend>(token: T, c: [f32; 4]) -> [vf8<T>; 4] {
    [
        vf8::splat_t(token, c[0]),
        vf8::splat_t(token, c[1]),
        vf8::splat_t(token, c[2]),
        vf8::splat_t(token, c[3]),
    ]
}

/// `v[o..o + 8]` as an array.
#[inline(always)]
fn take8<T: F32x8Backend>(v: &[vf8<T>], o: usize) -> [vf8<T>; 8] {
    let v = &v[o..o + 8];
    [v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7]]
}

/// [`idct8_ref`], lane-wise over `v[0..8]`.
#[inline(always)]
fn pass8<T: F32x8Backend>(k: &Kc<T>, v: &mut [vf8<T>]) {
    let tmp10 = v[0] + v[4];
    let tmp11 = v[0] - v[4];
    let tmp13 = v[2] + v[6];
    let tmp12 = (v[2] - v[6]) * k.r2 - tmp13;
    let e0 = tmp10 + tmp13;
    let e3 = tmp10 - tmp13;
    let e1 = tmp11 + tmp12;
    let e2 = tmp11 - tmp12;
    let z13 = v[5] + v[3];
    let z10 = v[5] - v[3];
    let z11 = v[1] + v[7];
    let z12 = v[1] - v[7];
    let t7 = z11 + z13;
    let t11 = (z11 - z13) * k.r2;
    let z5 = (z10 + z12) * k.c1;
    let t10 = z5 - z12 * k.c2;
    let t12 = z5 - z10 * k.c3;
    let t6 = t12 - t7;
    let t5 = t11 - t6;
    let t4 = t10 - t5;
    v[0] = e0 + t7;
    v[7] = e0 - t7;
    v[1] = e1 + t6;
    v[6] = e1 - t6;
    v[2] = e2 + t5;
    v[5] = e2 - t5;
    v[3] = e3 + t4;
    v[4] = e3 - t4;
}

/// [`iiv4_ref`], lane-wise.
#[inline(always)]
fn iiv4<T: F32x8Backend>(k: &Kc<T>, p: [vf8<T>; 4]) -> [vf8<T>; 4] {
    let a0 = p[0] * k.k2c[0] - p[3] * k.k2s[0];
    let b0 = p[0] * k.k2s[0] + p[3] * k.k2c[0];
    let a1 = p[1] * k.k2c[1] - p[2] * k.k2s[1];
    let b1 = p[1] * k.k2s[1] + p[2] * k.k2c[1];
    let c0 = a0 * k.ca + a1 * k.sa;
    let c1 = a0 * k.sa - a1 * k.ca;
    let d0 = b0 * k.ca - b1 * k.sa;
    let d1 = b0 * k.sa + b1 * k.ca;
    [c0 + d1, c0 - d1, c1 + d0, c1 - d0]
}

/// [`idct16_ref`], lane-wise over `v[0..16]`; inputs `nz..16` are zero.
/// With `nz <= 8` the rotations drop their `g[7 - m]` terms (zero inputs)
/// and the even half runs [`pass8_4`].
#[inline(always)]
fn pass16<T: F32x8Backend>(k: &Kc<T>, v: &mut [vf8<T>; 16], nz: usize) {
    let mut e = [k.z; 8];
    let mut g = [k.z; 8];
    let half = if nz <= 8 { 4 } else { 8 };
    for m in 0..half {
        e[m] = v[2 * m];
        g[m] = v[2 * m + 1];
    }
    if nz <= 8 {
        pass8_4(k, &mut e);
    } else {
        pass8(k, &mut e[..]);
    }
    let mut a = [k.z; 4];
    let mut b = [k.z; 4];
    if nz <= 8 {
        for m in 0..4 {
            a[m] = g[m] * k.k1c[m];
            b[m] = g[m] * k.k1s[m];
        }
    } else {
        for m in 0..4 {
            a[m] = g[m] * k.k1c[m] - g[7 - m] * k.k1s[m];
            b[m] = g[m] * k.k1s[m] + g[7 - m] * k.k1c[m];
        }
    }
    let c = iiv4(k, a);
    // Negation flips the sign bit on every tier (magetypes' `-` on x86 is
    // `0 - x`, which gives +0 for +0).
    let d = iiv4(k, [b[0], b[1] ^ k.sign, b[2], b[3] ^ k.sign]);
    let mut o = [k.z; 8];
    for t in 0..4 {
        o[2 * t] = c[t] + d[3 - t];
        o[2 * t + 1] = c[t] - d[3 - t];
    }
    for y in 0..8 {
        v[y] = e[y] + o[y];
        v[15 - y] = e[y] - o[y];
    }
}

/// [`pass8`] when only `v[0..4]` may be nonzero (`v[4..8]` are zero).
#[inline(always)]
fn pass8_4<T: F32x8Backend>(k: &Kc<T>, v: &mut [vf8<T>; 8]) {
    let (v0, v1, v2, v3) = (v[0], v[1], v[2], v[3]);
    let tmp12 = v2 * k.r2 - v2;
    let e0 = v0 + v2;
    let e3 = v0 - v2;
    let e1 = v0 + tmp12;
    let e2 = v0 - tmp12;
    let z13 = v3;
    let z10 = k.z - v3;
    let z11 = v1;
    let z12 = v1;
    let t7 = z11 + z13;
    let t11 = (z11 - z13) * k.r2;
    let z5 = (z10 + z12) * k.c1;
    let t10 = z5 - z12 * k.c2;
    let t12 = z5 - z10 * k.c3;
    let t6 = t12 - t7;
    let t5 = t11 - t6;
    let t4 = t10 - t5;
    v[0] = e0 + t7;
    v[7] = e0 - t7;
    v[1] = e1 + t6;
    v[6] = e1 - t6;
    v[2] = e2 + t5;
    v[5] = e2 - t5;
    v[3] = e3 + t4;
    v[4] = e3 - t4;
}

/// [`iiv4`] of `(p0, p1, 0, 0)`.
#[inline(always)]
fn iiv4_2<T: F32x8Backend>(k: &Kc<T>, p0: vf8<T>, p1: vf8<T>) -> [vf8<T>; 4] {
    let a0 = p0 * k.k2c[0];
    let b0 = p0 * k.k2s[0];
    let a1 = p1 * k.k2c[1];
    let b1 = p1 * k.k2s[1];
    let c0 = a0 * k.ca + a1 * k.sa;
    let c1 = a0 * k.sa - a1 * k.ca;
    let d0 = b0 * k.ca - b1 * k.sa;
    let d1 = b0 * k.sa + b1 * k.ca;
    [c0 + d1, c0 - d1, c1 + d0, c1 - d0]
}

/// [`pass16`] when only `v[0..4]` may be nonzero (`v[4..16]` are zero).
#[inline(always)]
fn pass16_4<T: F32x8Backend>(k: &Kc<T>, v: &mut [vf8<T>; 16]) {
    let mut e = [v[0], v[2], k.z, k.z, k.z, k.z, k.z, k.z];
    pass8_2(k, &mut e);
    let (g0, g1) = (v[1], v[3]);
    let a0 = g0 * k.k1c[0];
    let b0 = g0 * k.k1s[0];
    let a1 = g1 * k.k1c[1];
    let b1 = g1 * k.k1s[1];
    let c = iiv4_2(k, a0, a1);
    let d = iiv4_2(k, b0, b1 ^ k.sign);
    for t in 0..4 {
        let o0 = c[t] + d[3 - t];
        let o1 = c[t] - d[3 - t];
        v[2 * t] = e[2 * t] + o0;
        v[15 - 2 * t] = e[2 * t] - o0;
        v[2 * t + 1] = e[2 * t + 1] + o1;
        v[14 - 2 * t] = e[2 * t + 1] - o1;
    }
}

/// [`pass8`] when only `v[0]` and `v[1]` may be nonzero: the operations
/// whose inputs are all zero leave `v0` in every even-part output and the
/// odd part a function of `v1` alone (`x + 0`, `x - 0`, `0 c` dropped).
#[inline(always)]
fn pass8_2<T: F32x8Backend>(k: &Kc<T>, v: &mut [vf8<T>]) {
    let (v0, v1) = (v[0], v[1]);
    let t7 = v1;
    let t11 = v1 * k.r2;
    let z5 = v1 * k.c1;
    let t10 = z5 - v1 * k.c2;
    let t6 = z5 - t7;
    let t5 = t11 - t6;
    let t4 = t10 - t5;
    v[0] = v0 + t7;
    v[7] = v0 - t7;
    v[1] = v0 + t6;
    v[6] = v0 - t6;
    v[2] = v0 + t5;
    v[5] = v0 - t5;
    v[3] = v0 + t4;
    v[4] = v0 - t4;
}

/// [`iiv4`] of (p0, 0, 0, 0).
#[inline(always)]
fn iiv4_1<T: F32x8Backend>(k: &Kc<T>, p0: vf8<T>) -> [vf8<T>; 4] {
    let a0 = p0 * k.k2c[0];
    let b0 = p0 * k.k2s[0];
    let (c0, c1) = (a0 * k.ca, a0 * k.sa);
    let (d0, d1) = (b0 * k.ca, b0 * k.sa);
    [c0 + d1, c0 - d1, c1 + d0, c1 - d0]
}

/// [`pass16`] when only `v[0]` and `v[1]` may be nonzero: the even half is
/// `v0` everywhere and each inverse DCT-IV-4 has one nonzero input.
#[inline(always)]
fn pass16_2<T: F32x8Backend>(k: &Kc<T>, v: &mut [vf8<T>; 16]) {
    let (v0, g0) = (v[0], v[1]);
    let c = iiv4_1(k, g0 * k.k1c[0]);
    let d = iiv4_1(k, g0 * k.k1s[0]);
    for t in 0..4 {
        let (o0, o1) = (c[t] + d[3 - t], c[t] - d[3 - t]);
        v[2 * t] = v0 + o0;
        v[15 - 2 * t] = v0 - o0;
        v[2 * t + 1] = v0 + o1;
        v[14 - 2 * t] = v0 - o1;
    }
}

/// One pass of length `n` (8 or 16) over `v`, whose inputs `nz..n` are
/// zero: none nonzero leaves the zeros, only the first broadcasts it (a DC
/// input maps every output to itself), two take the two-input forms.
#[cfg(feature = "encode")]
#[inline(always)]
fn pass<T: F32x8Backend>(k: &Kc<T>, v: &mut [vf8<T>; 16], n: usize, nz: usize) {
    match nz {
        0 => {}
        1 => {
            let a = v[0];
            v[..n].fill(a);
        }
        2 if n == 8 => pass8_2(k, &mut v[..8]),
        2 => pass16_2(k, v),
        _ if n == 8 => pass8(k, &mut v[..8]),
        _ => pass16(k, v, nz),
    }
}

/// A merged block's destination: rows `y < 8` of plane `ch` at
/// `top[(ch * 8 + y) * stride + off..]`, rows `y >= 8` at
/// `bot[(ch * 8 + y - 8) * stride + off..]` (the next block row's band).
pub(crate) struct Bands<'a> {
    pub top: &'a mut [f32],
    pub bot: &'a mut [f32],
    pub stride: usize,
    pub off: usize,
}

/// One plane of a merged block, as the decoder glue prepared it
/// ([`super::recon_merged`]).
#[derive(Clone, Copy, Default)]
pub(crate) struct Src {
    /// Offset of each unit `p = py * pw + px` in the coefficients.
    pub base: [usize; 4],
    /// Raster rows of each unit that may hold nonzero AC (0..=8).
    pub er: [u8; 4],
    /// The pre-scaled LLF coefficients (row v < ph, column u < pw).
    pub llf: [[f32; 2]; 2],
    /// The pre-scaled quadrant origins of units p > 0 (0 when zero).
    pub ori: [f32; 4],
    /// Per column octet: spectrum rows that may be nonzero.
    pub vr: [u8; 2],
    /// Spectrum columns that may be nonzero.
    pub cn: u8,
    /// Only the coefficient (0, 0) is nonzero: every pixel is `llf[0][0]`.
    pub flat: bool,
}

/// Transposes the 8x8 tiles of `a` (`a[j][i]`: octet j, row i) into `b`
/// (`b[i / 8][8 j + i % 8]`); octets with `nz[j] == 0` were all zero and
/// give zero tiles.
#[cfg(feature = "encode")]
#[inline(always)]
fn transpose<T: F32x8Backend>(
    k: &Kc<T>,
    a: &[[vf8<T>; 16]; 2],
    b: &mut [[vf8<T>; 16]; 2],
    nz: [usize; 2],
    rows: usize,
    octets: usize,
) {
    for ty in 0..rows / 8 {
        for tx in 0..octets {
            if nz[tx] == 0 {
                b[ty][tx * 8..tx * 8 + 8].fill(k.z);
                continue;
            }
            let mut tile = take8(&a[tx], ty * 8);
            vf8::transpose_8x8(&mut tile);
            b[ty][tx * 8..tx * 8 + 8].copy_from_slice(&tile);
        }
    }
}

#[inline(always)]
fn pass8_nz<T: F32x8Backend>(k: &Kc<T>, v: &mut [vf8<T>; 8], nz: usize) {
    match nz {
        0 => {}
        1 => {
            let a = v[0];
            *v = [a; 8];
        }
        2 => pass8_2(k, v),
        3 | 4 => pass8_4(k, v),
        _ => pass8(k, v),
    }
}

#[magetypes(define(f32x8), v4(cfg(avx512)), v3, neon, wasm128, scalar)]
fn pass16_body_tier(_token: Token, k: &Kc<Token>, v: &mut [vf8<Token>; 16], nz: usize) {
    match nz {
        2 => pass16_2(k, v),
        3 | 4 => pass16_4(k, v),
        _ => pass16(k, v, nz),
    }
}

#[inline(always)]
fn store_tile8<T: F32x8Backend>(
    band: &mut [f32],
    ch: usize,
    stride: usize,
    off: usize,
    tile: &[vf8<T>; 8],
) {
    let base = ch * 8 * stride + off;
    for (y, row) in tile.iter().enumerate() {
        let o: &mut [f32; 8] = (&mut band[base + y * stride..][..8]).try_into().unwrap();
        row.store(o);
    }
}

#[inline(always)]
fn fill_tile8<T: F32x8Backend>(band: &mut [f32], ch: usize, stride: usize, off: usize, v: vf8<T>) {
    let base = ch * 8 * stride + off;
    for y in 0..8 {
        let o: &mut [f32; 8] = (&mut band[base + y * stride..][..8]).try_into().unwrap();
        v.store(o);
    }
}

#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn load_octet_8<T: F32x8Convert>(
    token: T,
    k: &Kc<T>,
    q: &mut [i32],
    base: usize,
    er: usize,
    rows: &[[f32; 8]],
    px: usize,
    aqv: vf8<T>,
    clear_q: bool,
) -> [vf8<T>; 8] {
    let mut col = [k.z; 8];
    let unit = &mut q[base..base + 64];
    let (qrows, _) = unit[..er * 8].as_chunks::<8>();
    for (y, qr) in qrows.iter().enumerate() {
        col[y] = vf8::from_i32x8_t(token, vi8::load_t(token, qr))
            * (vf8::load_t(token, &rows[y * 2 + px]) * aqv);
    }
    if clear_q {
        unit[..er * 8].fill(0);
        unit[0] = 0;
    }
    col
}

#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn load_octet_16<T: F32x8Convert>(
    token: T,
    k: &Kc<T>,
    q: &mut [i32],
    base0: usize,
    er0: usize,
    base1: usize,
    er1: usize,
    rows: &[[f32; 8]],
    px: usize,
    aqv: vf8<T>,
    clear_q: bool,
) -> [vf8<T>; 16] {
    let mut col = [k.z; 16];
    {
        let unit = &mut q[base0..base0 + 64];
        let (qrows, _) = unit[..er0 * 8].as_chunks::<8>();
        for (y, qr) in qrows.iter().enumerate() {
            col[y] = vf8::from_i32x8_t(token, vi8::load_t(token, qr))
                * (vf8::load_t(token, &rows[y * 2 + px]) * aqv);
        }
        if clear_q {
            unit[..er0 * 8].fill(0);
            unit[0] = 0;
        }
    }
    {
        let unit = &mut q[base1..base1 + 64];
        let (qrows, _) = unit[..er1 * 8].as_chunks::<8>();
        for (y, qr) in qrows.iter().enumerate() {
            col[8 + y] = vf8::from_i32x8_t(token, vi8::load_t(token, qr))
                * (vf8::load_t(token, &rows[(8 + y) * 2 + px]) * aqv);
        }
        if clear_q {
            unit[..er1 * 8].fill(0);
            unit[0] = 0;
        }
    }
    col
}

#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn make_src_ch<const PW: usize, const PH: usize>(
    t: &Tables,
    mode: LossyEntropy,
    ch: usize,
    q: &[i32],
    eobs: &[u8],
    (r, x0, bw): (usize, usize, usize),
    aq: f32,
) -> Src {
    let shape_idx = match (PW, PH) {
        (2, 2) => 0,
        (2, 1) => 1,
        _ => 2,
    };
    let shape = match shape_idx {
        0 => Shape::S16x16,
        1 => Shape::S16x8,
        _ => Shape::S8x16,
    };
    let dq = &t.dq[ch][shape_idx];
    let sc = &t.sc[shape_idx];
    let mut src = Src::default();
    let mut dcs = [[0.0f32; 2]; 2];
    let (mut cn, mut vr) = (PW, [PH, 0]);
    let mut ac = false;
    let base0 = (r * bw + x0) * NCH * 64 + ch * 64;
    for py in 0..PH {
        for px in 0..PW {
            let p = py * PW + px;
            let b = (r + py) * bw + x0 + px;
            let base = (b * NCH + ch) * 64;
            src.base[p] = base;
            dcs[py][px] = q[base] as f32 * t.dc_step[ch];
            let (er, ec) = super::unit_extent(t, mode, eobs[b * NCH + ch]);
            src.er[p] = er;
            if er > 0 {
                ac = true;
                vr[px] = vr[px].max(py * 8 + er as usize);
                cn = cn.max(px * 8 + ec as usize);
            }
            if p > 0 {
                let o = q[base0 + py * 8 + px];
                if o != 0 {
                    src.ori[p] = o as f32 * (dq[py * 8 * 16 + px * 8] * aq);
                    vr[px] = vr[px].max(py * 8 + 1);
                    cn = cn.max(px * 8 + 1);
                }
            }
        }
    }
    let l = super::dcs_to_llf(shape, dcs);
    let mut llf_ac = false;
    for v in 0..PH {
        for u in 0..PW {
            src.llf[v][u] = l[v][u] * sc[v * 16 + u];
            llf_ac |= (v, u) != (0, 0) && src.llf[v][u] != 0.0;
        }
    }
    src.vr = [vr[0] as u8, vr[1] as u8];
    src.cn = cn as u8;
    src.flat = !ac && !llf_ac;
    src
}

#[magetypes(define(f32x8, i32x8), v4(cfg(avx512)), v3, neon, wasm128, scalar)]
#[allow(clippy::too_many_arguments)]
fn group_channel_16x16(
    token: Token,
    k: &Kc<Token>,
    ch: usize,
    q: &mut [i32],
    s: &Src,
    dq: &[f32; 256],
    aqv: vf8<Token>,
    top: &mut [f32],
    bot: &mut [f32],
    stride: usize,
    off: usize,
    clear_q: bool,
) {
    if s.flat {
        let v = f32x8::splat_t(token, s.llf[0][0]);
        fill_tile8(top, ch, stride, off, v);
        fill_tile8(top, ch, stride, off + 8, v);
        fill_tile8(bot, ch, stride, off, v);
        fill_tile8(bot, ch, stride, off + 8, v);
        if clear_q {
            for &b in &s.base[..4] {
                q[b] = 0;
            }
        }
        return;
    }
    let rows: &[[f32; 8]] = dq.as_chunks::<8>().0;
    let mut d0 = load_octet_16::<Token>(
        token,
        k,
        q,
        s.base[0],
        s.er[0] as usize,
        s.base[2],
        s.er[2] as usize,
        rows,
        0,
        aqv,
        clear_q,
    );
    let p0 = f32x8::blend(
        k.m0,
        f32x8::splat_t(token, s.llf[0][0]),
        f32x8::splat_t(token, s.llf[0][1]),
    );
    d0[0] = f32x8::blend(k.m01, p0, d0[0]);
    let p1 = f32x8::blend(
        k.m0,
        f32x8::splat_t(token, s.llf[1][0]),
        f32x8::splat_t(token, s.llf[1][1]),
    );
    d0[1] = f32x8::blend(k.m01, p1, d0[1]);
    d0[8] = f32x8::blend(k.m0, f32x8::splat_t(token, s.ori[2]), d0[8]);
    let vr0 = s.vr[0] as usize;
    if vr0 == 1 {
        d0 = [d0[0]; 16];
    } else if vr0 >= 2 {
        incant!(pass16_body_tier(token, k, &mut d0, vr0) without token);
    }
    let cn = s.cn as usize;
    let has_d1 = s.vr[1] != 0;
    let mut d1 = [k.z; 16];
    if has_d1 {
        d1 = load_octet_16::<Token>(
            token,
            k,
            q,
            s.base[1],
            s.er[1] as usize,
            s.base[3],
            s.er[3] as usize,
            rows,
            1,
            aqv,
            clear_q,
        );
        d1[0] = f32x8::blend(k.m0, f32x8::splat_t(token, s.ori[1]), d1[0]);
        d1[8] = f32x8::blend(k.m0, f32x8::splat_t(token, s.ori[3]), d1[8]);
        let vr1 = s.vr[1] as usize;
        if vr1 == 1 {
            d1 = [d1[0]; 16];
        } else {
            incant!(pass16_body_tier(token, k, &mut d1, vr1) without token);
        }
    } else if clear_q {
        q[s.base[1]] = 0;
        q[s.base[3]] = 0;
    }
    for ty in 0..2 {
        let mut t0 = take8(&d0, ty * 8);
        f32x8::transpose_8x8(&mut t0);
        let t1 = if has_d1 {
            let mut t = take8(&d1, ty * 8);
            f32x8::transpose_8x8(&mut t);
            t
        } else {
            [k.z; 8]
        };
        let mut row16 = [
            t0[0], t0[1], t0[2], t0[3], t0[4], t0[5], t0[6], t0[7], t1[0], t1[1], t1[2], t1[3],
            t1[4], t1[5], t1[6], t1[7],
        ];
        if cn == 1 {
            row16 = [row16[0]; 16];
        } else if cn >= 2 {
            incant!(pass16_body_tier(token, k, &mut row16, cn) without token);
        }
        let band = if ty == 0 { &mut *top } else { &mut *bot };
        let mut out0 = take8(&row16, 0);
        f32x8::transpose_8x8(&mut out0);
        store_tile8(band, ch, stride, off, &out0);
        let mut out1 = take8(&row16, 8);
        f32x8::transpose_8x8(&mut out1);
        store_tile8(band, ch, stride, off + 8, &out1);
    }
}

#[magetypes(define(f32x8, i32x8), v4(cfg(avx512)), v3, neon, wasm128, scalar)]
#[allow(clippy::too_many_arguments)]
fn group_channel_16x8(
    token: Token,
    k: &Kc<Token>,
    ch: usize,
    q: &mut [i32],
    s: &Src,
    dq: &[f32; 256],
    aqv: vf8<Token>,
    top: &mut [f32],
    stride: usize,
    off: usize,
    clear_q: bool,
) {
    if s.flat {
        let v = f32x8::splat_t(token, s.llf[0][0]);
        fill_tile8(top, ch, stride, off, v);
        fill_tile8(top, ch, stride, off + 8, v);
        if clear_q {
            q[s.base[0]] = 0;
            q[s.base[1]] = 0;
        }
        return;
    }
    let rows: &[[f32; 8]] = dq.as_chunks::<8>().0;
    let mut d0 = load_octet_8::<Token>(
        token,
        k,
        q,
        s.base[0],
        s.er[0] as usize,
        rows,
        0,
        aqv,
        clear_q,
    );
    let p0 = f32x8::blend(
        k.m0,
        f32x8::splat_t(token, s.llf[0][0]),
        f32x8::splat_t(token, s.llf[0][1]),
    );
    d0[0] = f32x8::blend(k.m01, p0, d0[0]);
    pass8_nz(k, &mut d0, s.vr[0] as usize);
    f32x8::transpose_8x8(&mut d0);
    let d1 = if s.vr[1] == 0 {
        if clear_q {
            q[s.base[1]] = 0;
        }
        [k.z; 8]
    } else {
        let mut c = load_octet_8::<Token>(
            token,
            k,
            q,
            s.base[1],
            s.er[1] as usize,
            rows,
            1,
            aqv,
            clear_q,
        );
        c[0] = f32x8::blend(k.m0, f32x8::splat_t(token, s.ori[1]), c[0]);
        pass8_nz(k, &mut c, s.vr[1] as usize);
        f32x8::transpose_8x8(&mut c);
        c
    };
    let mut row16 = [
        d0[0], d0[1], d0[2], d0[3], d0[4], d0[5], d0[6], d0[7], d1[0], d1[1], d1[2], d1[3], d1[4],
        d1[5], d1[6], d1[7],
    ];
    let cn = s.cn as usize;
    if cn == 1 {
        row16 = [row16[0]; 16];
    } else if cn >= 2 {
        incant!(pass16_body_tier(token, k, &mut row16, cn) without token);
    }
    let mut out0 = take8(&row16, 0);
    f32x8::transpose_8x8(&mut out0);
    store_tile8(top, ch, stride, off, &out0);
    let mut out1 = take8(&row16, 8);
    f32x8::transpose_8x8(&mut out1);
    store_tile8(top, ch, stride, off + 8, &out1);
}

#[magetypes(define(f32x8, i32x8), v4(cfg(avx512)), v3, neon, wasm128, scalar)]
#[allow(clippy::too_many_arguments)]
fn group_channel_8x16(
    token: Token,
    k: &Kc<Token>,
    ch: usize,
    q: &mut [i32],
    s: &Src,
    dq: &[f32; 256],
    aqv: vf8<Token>,
    top: &mut [f32],
    bot: &mut [f32],
    stride: usize,
    off: usize,
    clear_q: bool,
) {
    if s.flat {
        let v = f32x8::splat_t(token, s.llf[0][0]);
        fill_tile8(top, ch, stride, off, v);
        fill_tile8(bot, ch, stride, off, v);
        if clear_q {
            q[s.base[0]] = 0;
            q[s.base[1]] = 0;
        }
        return;
    }
    let rows: &[[f32; 8]] = dq.as_chunks::<8>().0;
    let mut d0 = load_octet_16::<Token>(
        token,
        k,
        q,
        s.base[0],
        s.er[0] as usize,
        s.base[1],
        s.er[1] as usize,
        rows,
        0,
        aqv,
        clear_q,
    );
    d0[0] = f32x8::blend(k.m0, f32x8::splat_t(token, s.llf[0][0]), d0[0]);
    d0[1] = f32x8::blend(k.m0, f32x8::splat_t(token, s.llf[1][0]), d0[1]);
    d0[8] = f32x8::blend(k.m0, f32x8::splat_t(token, s.ori[1]), d0[8]);
    let vr0 = s.vr[0] as usize;
    if vr0 == 1 {
        d0 = [d0[0]; 16];
    } else if vr0 >= 2 {
        incant!(pass16_body_tier(token, k, &mut d0, vr0) without token);
    }
    let cn = s.cn as usize;
    let mut t0 = take8(&d0, 0);
    f32x8::transpose_8x8(&mut t0);
    pass8_nz(k, &mut t0, cn);
    f32x8::transpose_8x8(&mut t0);
    store_tile8(top, ch, stride, off, &t0);
    let mut t1 = take8(&d0, 8);
    f32x8::transpose_8x8(&mut t1);
    pass8_nz(k, &mut t1, cn);
    f32x8::transpose_8x8(&mut t1);
    store_tile8(bot, ch, stride, off, &t1);
}

#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn recon_unmerged_block<T: F32x8Convert>(
    token: T,
    k: &Kc<T>,
    k4: &super::super::small::Masks<T>,
    t: &Tables,
    mode: LossyEntropy,
    q: &mut [i32],
    eobs: &[u8],
    r: usize,
    bw: usize,
    bx: usize,
    px0: usize,
    dq8: &[[f32; 64]; 3],
    aqr: &[f32],
    bias: Option<&[[[f32; 64]; 2]; 3]>,
    top: &mut [f32],
    stride: usize,
    small: super::super::simd::Small<'_>,
    clear_q: bool,
) {
    let b = r * bw + bx;
    let split = small.split.get(b).copied().unwrap_or(0);
    let aq_s = aqr.get(bx).copied();
    let has_aq = matches!(aq_s, Some(s) if s != 1.0);
    let a = aq_s.unwrap_or(1.0);
    let aqv = vf8::splat_t(token, a);
    let aqv0 = vf8::blend(k.m0, k.one, aqv);
    let off = (bx - px0) * 8;
    let (m1, p1) = (vi8::splat_t(token, -1), vi8::splat_t(token, 1));
    for ch in 0..3 {
        let qb = &mut q[(b * 3 + ch) * 64..][..64];
        let eob = eobs[b * 3 + ch];
        if split >> ch & 1 != 0 {
            let dq4 = &small.dq[ch];
            if eob == 0 {
                let v = vf8::splat_t(token, qb[0] as f32 * dq4[0]);
                if clear_q {
                    qb[0] = 0;
                }
                fill_tile8(top, ch, stride, off, v);
                continue;
            }
            let mut v = [k.z; 8];
            for y in 0..8 {
                let qi: &[i32; 8] = qb[y * 8..y * 8 + 8].try_into().unwrap();
                let dd: &[f32; 8] = dq4[y * 8..y * 8 + 8].try_into().unwrap();
                let dv = if has_aq {
                    vf8::load_t(token, dd) * if y == 0 { aqv0 } else { aqv }
                } else {
                    vf8::load_t(token, dd)
                };
                v[y] = vf8::from_i32x8_t(token, vi8::load_t(token, qi)) * dv;
            }
            let haar = if has_aq {
                super::super::small::haar_ref(
                    qb[0] as f32 * dq4[0],
                    qb[1] as f32 * (dq4[1] * a),
                    qb[8] as f32 * (dq4[8] * a),
                    qb[9] as f32 * (dq4[9] * a),
                )
            } else {
                super::super::small::haar_of(qb, dq4)
            };
            if clear_q {
                qb.fill(0);
            }
            let o = super::super::small::block4(token, k4, v, haar);
            store_tile8(top, ch, stride, off, &o);
            continue;
        }
        let dqc = &dq8[ch];
        if eob == 0 {
            let v = vf8::splat_t(token, qb[0] as f32 * dqc[0]);
            if clear_q {
                qb[0] = 0;
            }
            fill_tile8(top, ch, stride, off, v);
            continue;
        }
        let mut v = [k.z; 8];
        if let Some(bt) = bias {
            let bt = &bt[ch];
            for y in 0..8 {
                let qi: &[i32; 8] = qb[y * 8..y * 8 + 8].try_into().unwrap();
                let dd: &[f32; 8] = dqc[y * 8..y * 8 + 8].try_into().unwrap();
                let dv = if has_aq {
                    vf8::load_t(token, dd) * if y == 0 { aqv0 } else { aqv }
                } else {
                    vf8::load_t(token, dd)
                };
                let b1: &[f32; 8] = bt[0][y * 8..y * 8 + 8].try_into().unwrap();
                let b2: &[f32; 8] = bt[1][y * 8..y * 8 + 8].try_into().unwrap();
                let qv = vi8::load_t(token, qi);
                let sg = qv.clamp(m1, p1);
                let bm = vf8::blend(
                    qv.simd_eq(sg).bitcast_to_f32(),
                    vf8::load_t(token, b1),
                    vf8::load_t(token, b2),
                );
                let t = vf8::from_i32x8_t(token, qv) - vf8::from_i32x8_t(token, sg) * bm;
                v[y] = t * dv;
            }
            if clear_q {
                qb.fill(0);
            }
            pass8(k, &mut v);
            vf8::transpose_8x8(&mut v);
            pass8(k, &mut v);
            vf8::transpose_8x8(&mut v);
            store_tile8(top, ch, stride, off, &v);
        } else {
            let (er, ec) = super::unit_extent(t, mode, eob);
            let er = (er as usize).max(1);
            let ec = (ec as usize).max(1);
            let (qrows, _) = qb[..er * 8].as_chunks::<8>();
            let (drows, _) = dqc[..er * 8].as_chunks::<8>();
            for y in 0..er {
                let dv = if has_aq {
                    vf8::load_t(token, &drows[y]) * if y == 0 { aqv0 } else { aqv }
                } else {
                    vf8::load_t(token, &drows[y])
                };
                v[y] = vf8::from_i32x8_t(token, vi8::load_t(token, &qrows[y])) * dv;
            }
            if clear_q {
                qb[..er * 8].fill(0);
            }
            pass8_nz(k, &mut v, er);
            vf8::transpose_8x8(&mut v);
            pass8_nz(k, &mut v, ec);
            vf8::transpose_8x8(&mut v);
            store_tile8(top, ch, stride, off, &v);
        }
    }
}

/// Reconstructs one coded run `bx0..bx1` of block row `r` with merged and
/// unmerged (8x8 / 4x4) blocks under a single SIMD dispatch and one `Kc`.
#[magetypes(define(f32x8, i32x8), v4(cfg(avx512)), v3, neon, wasm128, scalar)]
#[allow(clippy::too_many_arguments)]
fn recon_run_impl(
    token: Token,
    dq8: &[[f32; 64]; 3],
    bias: Option<&[[[f32; 64]; 2]; 3]>,
    t: &Tables,
    mode: LossyEntropy,
    q: &mut [i32],
    eobs: &[u8],
    r: usize,
    bw: usize,
    bx0: usize,
    bx1: usize,
    px0: usize,
    aqr: &[f32],
    rl: &[u8],
    top: &mut [f32],
    bot: &mut [f32],
    stride: usize,
    small: super::super::simd::Small<'_>,
    clear_q: bool,
) {
    let k = Kc::new(token);
    let k4 = super::super::small::Masks::new(token);
    let mut x = bx0;
    while x < bx1 {
        let Some(sh) = Shape::from_role(rl[x]) else {
            recon_unmerged_block::<Token>(
                token, &k, &k4, t, mode, q, eobs, r, bw, x, px0, dq8, aqr, bias, top, stride,
                small, clear_q,
            );
            x += 1;
            continue;
        };
        debug_assert_eq!((x - px0) % sh.pw(), 0);
        if r.is_multiple_of(sh.ph()) {
            let a = aqr.get(x).copied().unwrap_or(1.0);
            let aqv = f32x8::splat_t(token, a);
            let off = (x - px0) * 8;
            match sh {
                Shape::S16x16 => {
                    for ch in 0..NCH {
                        let s = make_src_ch::<2, 2>(t, mode, ch, q, eobs, (r, x, bw), a);
                        incant!(
                            group_channel_16x16(
                                token,
                                &k,
                                ch,
                                q,
                                &s,
                                &t.dq[ch][0],
                                aqv,
                                top,
                                bot,
                                stride,
                                off,
                                clear_q
                            ) without token
                        );
                    }
                }
                Shape::S16x8 => {
                    for ch in 0..NCH {
                        let s = make_src_ch::<2, 1>(t, mode, ch, q, eobs, (r, x, bw), a);
                        incant!(
                            group_channel_16x8(
                                token,
                                &k,
                                ch,
                                q,
                                &s,
                                &t.dq[ch][1],
                                aqv,
                                top,
                                stride,
                                off,
                                clear_q
                            ) without token
                        );
                    }
                }
                Shape::S8x16 => {
                    for ch in 0..NCH {
                        let s = make_src_ch::<1, 2>(t, mode, ch, q, eobs, (r, x, bw), a);
                        incant!(
                            group_channel_8x16(
                                token,
                                &k,
                                ch,
                                q,
                                &s,
                                &t.dq[ch][2],
                                aqv,
                                top,
                                bot,
                                stride,
                                off,
                                clear_q
                            ) without token
                        );
                    }
                }
            }
        }
        x += sh.pw();
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn recon_run(
    dq8: &[[f32; 64]; 3],
    bias: Option<&[[[f32; 64]; 2]; 3]>,
    t: &Tables,
    mode: LossyEntropy,
    q: &mut [i32],
    eobs: &[u8],
    r: usize,
    bw: usize,
    (bx0, bx1): (usize, usize),
    px0: usize,
    aqr: &[f32],
    rl: &[u8],
    top: &mut [f32],
    bot: &mut [f32],
    stride: usize,
    small: super::super::simd::Small<'_>,
    clear_q: bool,
) {
    if crate::v1::force_scalar() {
        return recon_run_impl_scalar(
            ScalarToken,
            dq8,
            bias,
            t,
            mode,
            q,
            eobs,
            r,
            bw,
            bx0,
            bx1,
            px0,
            aqr,
            rl,
            top,
            bot,
            stride,
            small,
            clear_q,
        );
    }
    #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
    if !crate::v1::force_avx2()
        && let Some(tok) = archmage::X64V4Token::summon()
    {
        return recon_run_impl_v4(
            tok, dq8, bias, t, mode, q, eobs, r, bw, bx0, bx1, px0, aqr, rl, top, bot, stride,
            small, clear_q,
        );
    }
    incant!(
        recon_run_impl(
            dq8, bias, t, mode, q, eobs, r, bw, bx0, bx1, px0, aqr, rl, top, bot, stride, small,
            clear_q
        ),
        [v3, neon, wasm128, scalar]
    )
}

/// The SIMD form of [`super::recon_merged`]'s transform for every plane of
/// one merged block of shape `shape` (0, 1, 2: [`Shape`] order): dequantize
/// the units (`q` rows times `dq` rows times the AQ multiplier `aq`) into
/// column octets of spectrum rows, place the LLF coefficients and quadrant
/// origins, run the vertical passes, transpose, run the horizontal passes,
/// transpose back and store. Zeroes every coefficient it reads.
#[magetypes(define(f32x8, i32x8), v4(cfg(avx512)), v3, neon, wasm128, scalar)]
fn group_impl(
    token: Token,
    shape: usize,
    q: &mut [i32],
    srcs: &[Src; NCH],
    dq: &[&[f32; 256]; NCH],
    aq: f32,
    dst: &mut Bands<'_>,
) {
    let k = Kc::new(token);
    let aqv = f32x8::splat_t(token, aq);
    match shape {
        0 => {
            for (ch, s) in srcs.iter().enumerate() {
                incant!(
                    group_channel_16x16(
                        token, &k, ch, q, s, dq[ch], aqv, dst.top, dst.bot, dst.stride, dst.off,
                        true
                    ) without token
                );
            }
        }
        1 => {
            for (ch, s) in srcs.iter().enumerate() {
                incant!(
                    group_channel_16x8(
                        token, &k, ch, q, s, dq[ch], aqv, dst.top, dst.stride, dst.off, true
                    ) without token
                );
            }
        }
        _ => {
            for (ch, s) in srcs.iter().enumerate() {
                incant!(
                    group_channel_8x16(
                        token, &k, ch, q, s, dq[ch], aqv, dst.top, dst.bot, dst.stride, dst.off,
                        true
                    ) without token
                );
            }
        }
    }
}

/// [`inverse_ref`] on f32x8 lanes (identical results): the spectrum's rows
/// as column octets, the vertical passes, a tile transpose, the horizontal
/// passes and the transpose back. The encoder's ringing estimate uses it.
#[magetypes(define(f32x8), v4(cfg(avx512)), v3, neon, wasm128, scalar)]
#[cfg(feature = "encode")]
fn inverse_full_impl(token: Token, shape: usize, x: &mut [f32; 256]) {
    let (w, h) = match shape {
        0 => (16usize, 16usize),
        1 => (16, 8),
        _ => (8, 16),
    };
    let k = Kc::new(token);
    let mut d = [[k.z; 16]; 2];
    {
        let rows: &[[f32; 8]] = x.as_chunks::<8>().0;
        for v in 0..h {
            for (j, dj) in d.iter_mut().enumerate().take(w / 8) {
                dj[v] = f32x8::load_t(token, &rows[v * 2 + j]);
            }
        }
    }
    for dj in d.iter_mut().take(w / 8) {
        pass(&k, dj, h, h);
    }
    let mut e = [[k.z; 16]; 2];
    transpose(&k, &d, &mut e, [h, h], h, w / 8);
    for row in e.iter_mut().take(h / 8) {
        pass(&k, row, w, w);
    }
    let rows: &mut [[f32; 8]] = x.as_chunks_mut::<8>().0;
    for ty in 0..w / 8 {
        for tx in 0..h / 8 {
            let mut tile = take8(&e[tx], ty * 8);
            f32x8::transpose_8x8(&mut tile);
            for (i, t) in tile.iter().enumerate() {
                t.store(&mut rows[(tx * 8 + i) * 2 + ty]);
            }
        }
    }
}

/// [`inverse_ref`] through the dispatched SIMD tier.
#[cfg(feature = "encode")]
pub(crate) fn inverse_full(s: Shape, x: &mut [f32; 256]) {
    #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
    if !crate::v1::force_avx2()
        && let Some(t) = archmage::X64V4Token::summon()
    {
        return inverse_full_impl_v4(t, s as usize, x);
    }
    incant!(
        inverse_full_impl(s as usize, x),
        [v3, neon, wasm128, scalar]
    )
}

/// Reconstructs every plane of one merged block into `dst` (see
/// [`group_impl`]); `simd` false runs the scalar instantiation.
pub(crate) fn group(
    s: Shape,
    q: &mut [i32],
    srcs: &[Src; NCH],
    dq: &[&[f32; 256]; NCH],
    aq: f32,
    dst: &mut Bands<'_>,
    simd: bool,
) {
    if !simd || crate::v1::force_scalar() {
        return group_impl_scalar(ScalarToken, s as usize, q, srcs, dq, aq, dst);
    }
    // AVX-512 hosts: the same f32x8 operations with AVX-512VL's 32 vector
    // registers (the 16-point pass and its constants spill with AVX2's 16);
    // `IMIJ_NO_AVX512` caps it at AVX2 like the 8x8 kernel.
    #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
    if !crate::v1::force_avx2()
        && let Some(t) = archmage::X64V4Token::summon()
    {
        return group_impl_v4(t, s as usize, q, srcs, dq, aq, dst);
    }
    incant!(
        group_impl(s as usize, q, srcs, dq, aq, dst),
        [v3, neon, wasm128, scalar]
    )
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    /// The orthonormal DCT-II basis of length n, C[k][i], in f64.
    fn basis(n: usize) -> alloc::vec::Vec<alloc::vec::Vec<f64>> {
        (0..n)
            .map(|k| {
                let ck = if k == 0 {
                    (1.0 / n as f64).sqrt()
                } else {
                    (2.0 / n as f64).sqrt()
                };
                (0..n)
                    .map(|i| {
                        ck * ((k * (2 * i + 1)) as f64 * core::f64::consts::PI / (2 * n) as f64)
                            .cos()
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn constants_are_the_rounded_exact_values() {
        let pi = core::f64::consts::PI;
        for m in 0..4 {
            let t = (2 * m + 1) as f64 * pi / 32.0;
            assert_eq!(C1[m], t.cos() as f32, "C1[{m}]");
            assert_eq!(S1[m], t.sin() as f32, "S1[{m}]");
        }
        for m in 0..2 {
            let t = (2 * m + 1) as f64 * pi / 16.0;
            assert_eq!(C2[m], t.cos() as f32, "C2[{m}]");
            assert_eq!(S2[m], t.sin() as f32, "S2[{m}]");
        }
        assert_eq!(CA, (pi / 8.0).cos() as f32);
        assert_eq!(SA, (pi / 8.0).sin() as f32);
    }

    fn xorshift(seed: &mut u32) -> u32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        *seed
    }

    /// The reference inverts the orthonormal forward transform (f64) of
    /// random blocks to within float rounding.
    #[test]
    fn reference_inverts_the_orthonormal_transform() {
        let mut seed = 0x2545_f491u32;
        for s in Shape::ALL {
            let (w, h) = (s.w(), s.h());
            let (bh, bw) = (basis(h), basis(w));
            for _ in 0..200 {
                let px: alloc::vec::Vec<f64> = (0..256)
                    .map(|_| (xorshift(&mut seed) % 2001) as f64 / 4.0 - 250.0)
                    .collect();
                let mut x = [0.0f32; 256];
                for v in 0..h {
                    for u in 0..w {
                        let mut c = 0.0;
                        for y in 0..h {
                            for xx in 0..w {
                                c += px[y * 16 + xx] * bh[v][y] * bw[u][xx];
                            }
                        }
                        x[v * 16 + u] = c as f32 * input_scale(s, v, u);
                    }
                }
                inverse_ref(s, &mut x);
                for y in 0..h {
                    for xx in 0..w {
                        let (a, b) = (x[y * 16 + xx] as f64, px[y * 16 + xx]);
                        assert!((a - b).abs() < 2e-3, "{s:?} ({y}, {xx}): {a} vs {b}");
                    }
                }
            }
        }
    }

    #[test]
    fn inverse_full_matches_the_reference() {
        let mut seed = 0x7f4a_7c15u32;
        for s in Shape::ALL {
            for _ in 0..200 {
                let mut x = [0.0f32; 256];
                for v in 0..s.h() {
                    for u in 0..s.w() {
                        x[v * 16 + u] = (xorshift(&mut seed) % 4001) as f32 / 8.0 - 250.0;
                    }
                }
                let mut want = x;
                inverse_ref(s, &mut want);
                let mut got = x;
                inverse_full(s, &mut got);
                let mut sc = x;
                inverse_full_impl_scalar(ScalarToken, s as usize, &mut sc);
                let mut tiers = alloc::vec![("dispatched", got), ("scalar", sc)];
                #[cfg(target_arch = "x86_64")]
                if let Some(t) = archmage::X64V3Token::summon() {
                    let mut v = x;
                    inverse_full_impl_v3(t, s as usize, &mut v);
                    tiers.push(("v3", v));
                }
                #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
                if let Some(t) = archmage::X64V4Token::summon() {
                    let mut v = x;
                    inverse_full_impl_v4(t, s as usize, &mut v);
                    tiers.push(("v4", v));
                }
                #[cfg(target_arch = "aarch64")]
                if let Some(t) = archmage::NeonToken::summon() {
                    let mut v = x;
                    inverse_full_impl_neon(t, s as usize, &mut v);
                    tiers.push(("neon", v));
                }
                #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
                if let Some(t) = archmage::Wasm128Token::summon() {
                    let mut v = x;
                    inverse_full_impl_wasm128(t, s as usize, &mut v);
                    tiers.push(("wasm128", v));
                }
                for (name, got) in &tiers {
                    for y in 0..s.h() {
                        for c in 0..s.w() {
                            let i = y * 16 + c;
                            assert_eq!(got[i].to_bits(), want[i].to_bits(), "{name} {s:?} {y} {c}");
                        }
                    }
                }
            }
        }
    }

    /// Random pre-scaled spectra through the kernel's tiers equal the
    /// reference bit for bit, with the zero-skipping bounds exercised.
    /// Runs [`group`]'s kernel on tier `tier` (0 the dispatched one, 1
    /// scalar, 2 v3, 3 v4); false when this host lacks the tier.
    #[allow(clippy::too_many_arguments)]
    fn run_tier(
        tier: usize,
        s: Shape,
        q: &mut [i32],
        srcs: &[Src; NCH],
        dq: &[&[f32; 256]; NCH],
        aq: f32,
        dst: &mut Bands<'_>,
    ) -> bool {
        let sh = s as usize;
        match tier {
            0 => group(s, q, srcs, dq, aq, dst, true),
            1 => group_impl_scalar(ScalarToken, sh, q, srcs, dq, aq, dst),
            #[cfg(target_arch = "x86_64")]
            2 => match archmage::X64V3Token::summon() {
                Some(t) => group_impl_v3(t, sh, q, srcs, dq, aq, dst),
                None => return false,
            },
            #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
            3 => match archmage::X64V4Token::summon() {
                Some(t) => group_impl_v4(t, sh, q, srcs, dq, aq, dst),
                None => return false,
            },
            _ => return false,
        }
        true
    }

    #[test]
    fn kernel_tiers_match_the_reference() {
        let mut seed = 0x1357_9bdfu32;
        for s in Shape::ALL {
            let (w, h, pw, ph) = (s.w(), s.h(), s.pw(), s.ph());
            for trial in 0..400 {
                // Spectrum extent: a random corner, sometimes only the LLF.
                let (rn, cn) = match trial % 4 {
                    0 => (ph, pw),
                    1 => (
                        1 + xorshift(&mut seed) as usize % h,
                        1 + xorshift(&mut seed) as usize % w,
                    ),
                    2 => (h, w),
                    _ => (
                        1 + xorshift(&mut seed) as usize % 3,
                        1 + xorshift(&mut seed) as usize % 3,
                    ),
                };
                let rn = rn.max(ph);
                let cn = cn.max(pw);
                let mut spec = [0.0f32; 256];
                let mut q = alloc::vec![0i32; 4 * 64];
                let dq_tab: [f32; 256] = core::array::from_fn(|i| 0.5 + (i % 37) as f32 * 0.25);
                let aq = if trial % 3 == 0 { 1.0 } else { 0.75 };
                let mut src = Src::default();
                for p in 0..4 {
                    src.base[p] = p * 64;
                }
                for v in 0..rn {
                    for u in 0..cn {
                        if v < ph && u < pw {
                            continue;
                        }
                        let val = (xorshift(&mut seed) % 41) as i32 - 20;
                        let (p, z) = slot(s, v, u);
                        q[p * 64 + z] = val;
                        spec[v * 16 + u] = val as f32 * (dq_tab[v * 16 + u] * aq);
                    }
                }
                // Units' row bounds from what was written.
                for p in 0..ph * pw {
                    let rows = (1..64)
                        .filter(|&z| q[p * 64 + z] != 0)
                        .map(|z| z / 8 + 1)
                        .max();
                    src.er[p] = rows.unwrap_or(0) as u8;
                }
                for v in 0..ph {
                    for u in 0..pw {
                        let l = (xorshift(&mut seed) % 2001) as f32 - 1000.0;
                        let l = if trial % 7 == 0 && (v, u) != (0, 0) {
                            0.0
                        } else {
                            l
                        };
                        src.llf[v][u] = l;
                        spec[v * 16 + u] = l;
                    }
                }
                for p in 1..ph * pw {
                    let (py, px) = (p / pw, p % pw);
                    src.ori[p] = spec[py * 8 * 16 + px * 8];
                }
                let mut vr = [0u8; 2];
                for j in 0..w / 8 {
                    vr[j] = (0..h)
                        .filter(|&v| (j * 8..j * 8 + 8).any(|u| spec[v * 16 + u] != 0.0))
                        .map(|v| v + 1)
                        .max()
                        .unwrap_or(0) as u8;
                }
                src.vr = vr;
                src.cn = (0..w)
                    .filter(|&u| (0..h).any(|v| spec[v * 16 + u] != 0.0))
                    .map(|u| u + 1)
                    .max()
                    .unwrap_or(0) as u8;
                src.flat = (0..256).all(|i| i == 0 || spec[i] == 0.0);
                // The DCs' slots are cleared by the kernel; mark them.
                for p in 0..ph * pw {
                    q[p * 64] = 7;
                }
                let mut want = spec;
                inverse_ref(s, &mut want);
                let srcs = [src; NCH];
                let dqs = [&dq_tab; NCH];
                // Every tier's output, bit for bit (zeros' signs included).
                let mut first: Option<(usize, Vec<u32>)> = None;
                for tier in 0..4 {
                    let mut qq = q.clone();
                    // Units of all three planes share `q` here: each plane
                    // reads the same coefficients, so give each its copy.
                    let mut out_top = alloc::vec![f32::NAN; NCH * 8 * 24];
                    let mut out_bot = alloc::vec![f32::NAN; NCH * 8 * 24];
                    let mut all = alloc::vec![0i32; 3 * 256];
                    for ch in 0..NCH {
                        all[ch * 256..ch * 256 + 256].copy_from_slice(&qq);
                    }
                    let srcs3: [Src; NCH] = core::array::from_fn(|ch| {
                        let mut s = srcs[ch];
                        for p in 0..4 {
                            s.base[p] += ch * 256;
                        }
                        s
                    });
                    let mut bands = Bands {
                        top: &mut out_top,
                        bot: &mut out_bot,
                        stride: 24,
                        off: 4,
                    };
                    if !run_tier(tier, s, &mut all, &srcs3, &dqs, aq, &mut bands) {
                        continue;
                    }
                    assert!(
                        all.iter().all(|&v| v == 0),
                        "{s:?} coefficients not cleared"
                    );
                    for ch in 0..NCH {
                        for y in 0..h {
                            for xx in 0..w {
                                let got = if y < 8 {
                                    out_top[(ch * 8 + y) * 24 + 4 + xx]
                                } else {
                                    out_bot[(ch * 8 + y - 8) * 24 + 4 + xx]
                                };
                                let wv = want[y * 16 + xx];
                                // The reference computes every operation; the
                                // kernel skips zero inputs (`x + 0`), so only a
                                // zero's sign may differ from it.
                                assert!(
                                    got == wv,
                                    "{s:?} trial {trial} tier {tier} ({y}, {xx}): {got} vs {wv}"
                                );
                            }
                        }
                    }
                    let bits: Vec<u32> = out_top
                        .iter()
                        .chain(&out_bot)
                        .map(|v| v.to_bits())
                        .collect();
                    match &first {
                        Some((t0, b0)) => assert!(
                            *b0 == bits,
                            "{s:?} trial {trial}: tiers {t0} and {tier} differ in bits"
                        ),
                        None => first = Some((tier, bits)),
                    }
                    qq.fill(0);
                }
            }
        }
    }
}
