//! V4 sRGB XYB power reconstruction (`u8_dsqrt_d4` and `u16_dsqrt_d6`).
//!
//! Quantizer bit 5 selects this table-free, non-FMA IEEE binary32 reconstruction
//! for full-precision sRGB (`Cicp::SRGB`) XYB U8 and U16 outputs.
//!
//! Both transfers evaluate an Estrin polynomial in `t = sqrt(sqrt(a))` (with
//! `s = sqrt(a)` and `a = clamp(lin, 0, 1)`) using explicit separate multiply
//! and add (no FMA contraction), producing bit-identical outputs across x86-64
//! AVX-512 VBMI, x86-64 AVX2, AArch64 NEON, Wasm SIMD128 (strict and
//! bit-exact relaxed-SIMD), and scalar.

use super::color::{CBRT_BIAS, LossyColor, OPSIN_BIAS, rne_pos};
use crate::{Cicp, ImageInfo, SampleType};
use archmage::prelude::*;

pub(super) const QUANTIZER_BIT: u64 = 1 << 5;

const TOE: f32 = 0.003_130_8;
const SLOPE_U8: f32 = 3294.6;
const SLOPE_U16: f32 = 846_712.2;

// Pre-scaled by 255.0. C0 = -16.476618 (0xc183_d01d) gives 0 quantized U8
// monotonicity reversals across all 109,051,905 binary32 inputs in [0, 1],
// max continuous error 0.00886 codes, and 0/256 sRGB8 roundtrip errors.
#[allow(clippy::excessive_precision)]
const D4_U8: [f32; 5] = [-16.476_618, 50.019_123, 286.274, -85.544_365, 20.736_712];

// Pre-scaled by 65535.0. Gives 0/65,536 sRGB16 exact roundtrip errors and
// max continuous error 0.10433 U16 codes.
#[allow(clippy::excessive_precision)]
const D6_U16: [f32; 7] = [
    -3902.434, 9149.475, 89513.41, -55893.758, 43063.316, -20864.195, 4469.2793,
];

pub(super) fn supports(info: &ImageInfo, color: LossyColor) -> bool {
    color == LossyColor::Xyb
        && matches!(info.sample, SampleType::U8 | SampleType::U16)
        && !info.has_custom_precision()
        && info.cicp == Cicp::SRGB
}

/// Shared non-FMA binary32 factored XYB -> linear sRGB inverse.
macro_rules! xyb_linear_rgb {
    ($vector:ty, $splat:expr, $clamp:expr, $planes:expr, $inv:expr) => {{
        let splat = $splat;
        let clamp = $clamp;
        let [y, x, z] = $planes;
        let [iy, ix, iz] = $inv;
        let base = y * splat(iy) + splat(CBRT_BIAS);
        let x = x * splat(ix);
        let lo64 = splat(-64.0);
        let hi64 = splat(64.0);
        let cube = |v: $vector| {
            let v = clamp(v, lo64, hi64);
            (v * v) * v
        };
        let m = cube(base - x);
        let u = cube(base + x) - m;
        let v = cube(base + z * splat(iz)) - m;
        let q = m - splat(OPSIN_BIAS);
        let rg = q - splat(0.164_623) * v;
        [
            rg + splat(11.031_567) * u,
            rg - splat(3.254_147_3) * u,
            (q + splat(1.945_928_2) * v) - splat(3.658_851_2) * u,
        ]
    }};
}

/// Binary32 graph for `u8_dsqrt_d4` (non-FMA, 0 B table, 0 U8 reversals).
/// Output lanes are finite in `[0.0, 255.009]` (`[0, 255]` after RNE).
macro_rules! power_u8_codes {
    ($vector:ty, $splat:expr, $clamp:expr, $select:expr, $planes:expr, $inv:expr) => {{
        let rgb = xyb_linear_rgb!($vector, $splat, $clamp, $planes, $inv);
        let splat = $splat;
        let clamp = $clamp;
        let zero = splat(0.0);
        let one = splat(1.0);
        let toe = splat(TOE);
        let slope = splat(SLOPE_U8);
        let c0 = splat(D4_U8[0]);
        let c1 = splat(D4_U8[1]);
        let c2 = splat(D4_U8[2]);
        let c3 = splat(D4_U8[3]);
        let c4 = splat(D4_U8[4]);
        let [r_lin, g_lin, b_lin] = rgb;
        let a0 = clamp(r_lin, zero, one);
        let a1 = clamp(g_lin, zero, one);
        let a2 = clamp(b_lin, zero, one);
        // Overlap independent channel sqrts across execution units (bit-identical).
        let s0 = a0.sqrt();
        let s1 = a1.sqrt();
        let s2 = a2.sqrt();
        let t0 = s0.sqrt();
        let t1 = s1.sqrt();
        let t2 = s2.sqrt();
        let eval = |a: $vector, s: $vector, t: $vector| {
            let p01 = c0 + c1 * t;
            let p23 = c2 + c3 * t;
            let gamma = (p01 + p23 * s) + c4 * a;
            $select(a.simd_lt(toe), a * slope, gamma)
        };
        [eval(a0, s0, t0), eval(a1, s1, t1), eval(a2, s2, t2)]
    }};
}

/// Binary32 graph for `u16_dsqrt_d6` (non-FMA, 0 B table, 0/65536 RT misses).
/// Output lanes are finite in `[0.0, 65535.09]` (`[0, 65535]` after RNE).
macro_rules! power_u16_codes {
    ($vector:ty, $splat:expr, $clamp:expr, $select:expr, $planes:expr, $inv:expr) => {{
        let rgb = xyb_linear_rgb!($vector, $splat, $clamp, $planes, $inv);
        let splat = $splat;
        let clamp = $clamp;
        let zero = splat(0.0);
        let one = splat(1.0);
        let toe = splat(TOE);
        let slope = splat(SLOPE_U16);
        let c0 = splat(D6_U16[0]);
        let c1 = splat(D6_U16[1]);
        let c2 = splat(D6_U16[2]);
        let c3 = splat(D6_U16[3]);
        let c4 = splat(D6_U16[4]);
        let c5 = splat(D6_U16[5]);
        let c6 = splat(D6_U16[6]);
        let [r_lin, g_lin, b_lin] = rgb;
        let a0 = clamp(r_lin, zero, one);
        let a1 = clamp(g_lin, zero, one);
        let a2 = clamp(b_lin, zero, one);
        let s0 = a0.sqrt();
        let s1 = a1.sqrt();
        let s2 = a2.sqrt();
        let t0 = s0.sqrt();
        let t1 = s1.sqrt();
        let t2 = s2.sqrt();
        let eval = |a: $vector, s: $vector, t: $vector| {
            let p01 = c0 + c1 * t;
            let p23 = c2 + c3 * t;
            let p45 = c4 + c5 * t;
            let p03 = p01 + p23 * s;
            let p46 = p45 + c6 * s;
            let gamma = p03 + p46 * a;
            $select(a.simd_lt(toe), a * slope, gamma)
        };
        [eval(a0, s0, t0), eval(a1, s1, t1), eval(a2, s2, t2)]
    }};
}

// ============================================================================
// Per-tier safe RGB8 and RGB16 pack/store helpers (8 pixels/iter)
// ============================================================================

#[cfg(target_arch = "x86_64")]
#[rite(import_intrinsics)]
fn store_rgb8_v3(
    _t: X64V3Token,
    r: magetypes::simd::generic::f32x8<X64V3Token>,
    g: magetypes::simd::generic::f32x8<X64V3Token>,
    b: magetypes::simd::generic::f32x8<X64V3Token>,
    out: &mut [u8; 24],
) {
    let ri = _mm256_cvtps_epi32(r.into_repr());
    let gi = _mm256_cvtps_epi32(g.into_repr());
    let bi = _mm256_cvtps_epi32(b.into_repr());
    let rg16 = _mm256_packs_epi32(ri, gi);
    let b16 = _mm256_packs_epi32(bi, _mm256_setzero_si256());
    let rgb8 = _mm256_packus_epi16(rg16, b16);
    let shuf = _mm256_setr_epi8(
        0, 4, 8, 1, 5, 9, 2, 6, 10, 3, 7, 11, -1, -1, -1, -1, 0, 4, 8, 1, 5, 9, 2, 6, 10, 3, 7, 11,
        -1, -1, -1, -1,
    );
    let packed = _mm256_shuffle_epi8(rgb8, shuf);
    let lo12 = _mm256_castsi256_si128(packed);
    let hi12 = _mm256_extracti128_si256::<1>(packed);
    let lo16 = _mm_or_si128(lo12, _mm_slli_si128::<12>(hi12));
    let hi8 = _mm_srli_si128::<4>(hi12);
    _mm_storeu_si128(out.first_chunk_mut::<16>().unwrap(), lo16);
    out.last_chunk_mut::<8>()
        .unwrap()
        .copy_from_slice(&(_mm_cvtsi128_si64(hi8) as u64).to_ne_bytes());
}

#[cfg(target_arch = "x86_64")]
#[rite(import_intrinsics)]
fn store_rgb16_v3(
    _t: X64V3Token,
    r: magetypes::simd::generic::f32x8<X64V3Token>,
    g: magetypes::simd::generic::f32x8<X64V3Token>,
    b: magetypes::simd::generic::f32x8<X64V3Token>,
    out: &mut [u8; 48],
) {
    let ri = _mm256_cvtps_epi32(r.into_repr());
    let gi = _mm256_cvtps_epi32(g.into_repr());
    let bi = _mm256_cvtps_epi32(b.into_repr());
    let rg = _mm256_packus_epi32(ri, gi);
    let bz = _mm256_packus_epi32(bi, _mm256_setzero_si256());
    let sh_rg0 = _mm256_setr_epi8(
        0, 1, 8, 9, -1, -1, 2, 3, 10, 11, -1, -1, 4, 5, 12, 13, 0, 1, 8, 9, -1, -1, 2, 3, 10, 11,
        -1, -1, 4, 5, 12, 13,
    );
    let sh_b0 = _mm256_setr_epi8(
        -1, -1, -1, -1, 0, 1, -1, -1, -1, -1, 2, 3, -1, -1, -1, -1, -1, -1, -1, -1, 0, 1, -1, -1,
        -1, -1, 2, 3, -1, -1, -1, -1,
    );
    let sh_rg1 = _mm256_setr_epi8(
        -1, -1, 6, 7, 14, 15, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 6, 7, 14, 15, -1, -1,
        -1, -1, -1, -1, -1, -1, -1, -1,
    );
    let sh_b1 = _mm256_setr_epi8(
        4, 5, -1, -1, -1, -1, 6, 7, -1, -1, -1, -1, -1, -1, -1, -1, 4, 5, -1, -1, -1, -1, 6, 7, -1,
        -1, -1, -1, -1, -1, -1, -1,
    );
    let p0 = _mm256_or_si256(
        _mm256_shuffle_epi8(rg, sh_rg0),
        _mm256_shuffle_epi8(bz, sh_b0),
    );
    let p1 = _mm256_or_si256(
        _mm256_shuffle_epi8(rg, sh_rg1),
        _mm256_shuffle_epi8(bz, sh_b1),
    );
    _mm_storeu_si128(
        <&mut [u8; 16]>::try_from(&mut out[0..16]).unwrap(),
        _mm256_castsi256_si128(p0),
    );
    out[16..24]
        .copy_from_slice(&(_mm_cvtsi128_si64(_mm256_castsi256_si128(p1)) as u64).to_ne_bytes());
    _mm_storeu_si128(
        <&mut [u8; 16]>::try_from(&mut out[24..40]).unwrap(),
        _mm256_extracti128_si256::<1>(p0),
    );
    out[40..48].copy_from_slice(
        &(_mm_cvtsi128_si64(_mm256_extracti128_si256::<1>(p1)) as u64).to_ne_bytes(),
    );
}

#[cfg(target_arch = "aarch64")]
#[rite(import_intrinsics)]
fn store_rgb8_neon(
    _t: NeonToken,
    r: magetypes::simd::generic::f32x8<NeonToken>,
    g: magetypes::simd::generic::f32x8<NeonToken>,
    b: magetypes::simd::generic::f32x8<NeonToken>,
    out: &mut [u8; 24],
) {
    let to8 = |v: [float32x4_t; 2]| {
        let a = vcvtnq_s32_f32(v[0]);
        let c = vcvtnq_s32_f32(v[1]);
        vqmovun_s16(vcombine_s16(vqmovn_s32(a), vqmovn_s32(c)))
    };
    const IDX_LO: [u8; 16] = [0, 8, 16, 1, 9, 17, 2, 10, 18, 3, 11, 19, 4, 12, 20, 5];
    const IDX_HI: [u8; 16] = [
        13, 21, 6, 14, 22, 7, 15, 23, 255, 255, 255, 255, 255, 255, 255, 255,
    ];
    let r8 = to8(r.into_repr());
    let g8 = to8(g.into_repr());
    let b8 = to8(b.into_repr());
    let tbl = uint8x16x2_t(vcombine_u8(r8, g8), vcombine_u8(b8, b8));
    let lo = vqtbl2q_u8(tbl, vld1q_u8(&IDX_LO));
    let hi = vqtbl2q_u8(tbl, vld1q_u8(&IDX_HI));
    vst1q_u8(out.first_chunk_mut::<16>().unwrap(), lo);
    vst1_u8(out.last_chunk_mut::<8>().unwrap(), vget_low_u8(hi));
}

#[cfg(target_arch = "aarch64")]
#[rite(import_intrinsics)]
fn store_rgb16_neon(
    _t: NeonToken,
    r: magetypes::simd::generic::f32x8<NeonToken>,
    g: magetypes::simd::generic::f32x8<NeonToken>,
    b: magetypes::simd::generic::f32x8<NeonToken>,
    out: &mut [u8; 48],
) {
    let to16 = |v: [float32x4_t; 2]| {
        let a = vcvtnq_s32_f32(v[0]);
        let c = vcvtnq_s32_f32(v[1]);
        vcombine_u16(vqmovun_s32(a), vqmovun_s32(c))
    };
    const IDX0: [u8; 16] = [0, 1, 16, 17, 32, 33, 2, 3, 18, 19, 34, 35, 4, 5, 20, 21];
    const IDX1: [u8; 16] = [36, 37, 6, 7, 22, 23, 38, 39, 8, 9, 24, 25, 40, 41, 10, 11];
    const IDX2: [u8; 16] = [
        26, 27, 42, 43, 12, 13, 28, 29, 44, 45, 14, 15, 30, 31, 46, 47,
    ];
    let tbl = uint8x16x3_t(
        vreinterpretq_u8_u16(to16(r.into_repr())),
        vreinterpretq_u8_u16(to16(g.into_repr())),
        vreinterpretq_u8_u16(to16(b.into_repr())),
    );
    vst1q_u8(
        <&mut [u8; 16]>::try_from(&mut out[0..16]).unwrap(),
        vqtbl3q_u8(tbl, vld1q_u8(&IDX0)),
    );
    vst1q_u8(
        <&mut [u8; 16]>::try_from(&mut out[16..32]).unwrap(),
        vqtbl3q_u8(tbl, vld1q_u8(&IDX1)),
    );
    vst1q_u8(
        <&mut [u8; 16]>::try_from(&mut out[32..48]).unwrap(),
        vqtbl3q_u8(tbl, vld1q_u8(&IDX2)),
    );
}

#[cfg(target_arch = "wasm32")]
#[inline(always)]
fn store_rgb8_wasm128(
    token: Wasm128Token,
    r: magetypes::simd::generic::f32x8<Wasm128Token>,
    g: magetypes::simd::generic::f32x8<Wasm128Token>,
    b: magetypes::simd::generic::f32x8<Wasm128Token>,
    out: &mut [u8; 24],
) {
    use core::arch::wasm32::*;
    use magetypes::simd::generic::u8x16;

    // Exact IEEE-754 RNE for finite values in [0, 255.009] (including -0.0)
    // via 1.5 * 2^23 magic adder, followed by saturating narrows and shuffles.
    let magic = f32x4_splat(12_582_912.0);
    let bias = i32x4_splat(0x4b40_0000);
    let to_i32 = |v: v128| i32x4_sub(f32x4_add(v, magic), bias);
    let [r0, r1] = r.into_repr();
    let [g0, g1] = g.into_repr();
    let [b0, b1] = b.into_repr();
    let r16 = i16x8_narrow_i32x4(to_i32(r0), to_i32(r1));
    let g16 = i16x8_narrow_i32x4(to_i32(g0), to_i32(g1));
    let b16 = i16x8_narrow_i32x4(to_i32(b0), to_i32(b1));
    let rg = u8x16_narrow_i16x8(r16, g16);
    let bb = u8x16_narrow_i16x8(b16, b16);
    let out0 = i8x16_shuffle::<0, 8, 16, 1, 9, 17, 2, 10, 18, 3, 11, 19, 4, 12, 20, 5>(rg, bb);
    let out1 = i8x16_shuffle::<13, 21, 6, 14, 22, 7, 15, 23, 0, 0, 0, 0, 0, 0, 0, 0>(rg, bb);
    u8x16::from_repr_t(token, out0).store(out.first_chunk_mut::<16>().unwrap());
    let tail8 = i64x2_extract_lane::<0>(out1) as u64;
    out.last_chunk_mut::<8>()
        .unwrap()
        .copy_from_slice(&tail8.to_ne_bytes());
}

#[cfg(target_arch = "wasm32")]
#[inline(always)]
fn store_rgb16_wasm128(
    token: Wasm128Token,
    r: magetypes::simd::generic::f32x8<Wasm128Token>,
    g: magetypes::simd::generic::f32x8<Wasm128Token>,
    b: magetypes::simd::generic::f32x8<Wasm128Token>,
    out: &mut [u8; 48],
) {
    use core::arch::wasm32::*;
    use magetypes::simd::generic::u8x16;

    // Exact IEEE-754 RNE for finite values in [0, 65535.09] (including -0.0)
    // via 1.5 * 2^23 magic adder, followed by saturating u16 narrows and shuffles.
    let magic = f32x4_splat(12_582_912.0);
    let bias = i32x4_splat(0x4b40_0000);
    let to_i32 = |v: v128| i32x4_sub(f32x4_add(v, magic), bias);
    let [r0, r1] = r.into_repr();
    let [g0, g1] = g.into_repr();
    let [b0, b1] = b.into_repr();
    let r16 = u16x8_narrow_i32x4(to_i32(r0), to_i32(r1));
    let g16 = u16x8_narrow_i32x4(to_i32(g0), to_i32(g1));
    let b16 = u16x8_narrow_i32x4(to_i32(b0), to_i32(b1));
    let rg0 = i8x16_shuffle::<0, 1, 16, 17, 2, 3, 18, 19, 4, 5, 20, 21, 6, 7, 22, 23>(r16, g16);
    let rg1 =
        i8x16_shuffle::<8, 9, 24, 25, 10, 11, 26, 27, 12, 13, 28, 29, 14, 15, 30, 31>(r16, g16);
    let rgm =
        i8x16_shuffle::<12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27>(rg0, rg1);
    let o0 = i8x16_shuffle::<0, 1, 2, 3, 16, 17, 4, 5, 6, 7, 18, 19, 8, 9, 10, 11>(rg0, b16);
    let o1 = i8x16_shuffle::<20, 21, 0, 1, 2, 3, 22, 23, 4, 5, 6, 7, 24, 25, 8, 9>(rgm, b16);
    let o2 = i8x16_shuffle::<6, 7, 26, 27, 8, 9, 10, 11, 28, 29, 12, 13, 14, 15, 30, 31>(rg1, b16);
    u8x16::from_repr_t(token, o0).store(<&mut [u8; 16]>::try_from(&mut out[0..16]).unwrap());
    u8x16::from_repr_t(token, o1).store(<&mut [u8; 16]>::try_from(&mut out[16..32]).unwrap());
    u8x16::from_repr_t(token, o2).store(<&mut [u8; 16]>::try_from(&mut out[32..48]).unwrap());
}

#[inline(always)]
fn store_rgb8_scalar(
    _t: ScalarToken,
    r: magetypes::simd::generic::f32x8<ScalarToken>,
    g: magetypes::simd::generic::f32x8<ScalarToken>,
    b: magetypes::simd::generic::f32x8<ScalarToken>,
    out: &mut [u8; 24],
) {
    let (ra, ga, ba) = (r.to_array(), g.to_array(), b.to_array());
    for i in 0..8 {
        out[i * 3] = rne_pos(ra[i].clamp(0.0, 255.0)).min(255.0) as u8;
        out[i * 3 + 1] = rne_pos(ga[i].clamp(0.0, 255.0)).min(255.0) as u8;
        out[i * 3 + 2] = rne_pos(ba[i].clamp(0.0, 255.0)).min(255.0) as u8;
    }
}

#[inline(always)]
fn store_rgb16_scalar(
    _t: ScalarToken,
    r: magetypes::simd::generic::f32x8<ScalarToken>,
    g: magetypes::simd::generic::f32x8<ScalarToken>,
    b: magetypes::simd::generic::f32x8<ScalarToken>,
    out: &mut [u8; 48],
) {
    let (ra, ga, ba) = (r.to_array(), g.to_array(), b.to_array());
    for i in 0..8 {
        let rv = rne_pos(ra[i].clamp(0.0, 65535.0)).min(65535.0) as u16;
        let gv = rne_pos(ga[i].clamp(0.0, 65535.0)).min(65535.0) as u16;
        let bv = rne_pos(ba[i].clamp(0.0, 65535.0)).min(65535.0) as u16;
        out[i * 6..i * 6 + 2].copy_from_slice(&rv.to_ne_bytes());
        out[i * 6 + 2..i * 6 + 4].copy_from_slice(&gv.to_ne_bytes());
        out[i * 6 + 4..i * 6 + 6].copy_from_slice(&bv.to_ne_bytes());
    }
}

// ============================================================================
// Portable 8-px/iter row kernels (AVX2, NEON, Wasm128 strict/relaxed, Scalar)
// ============================================================================

#[magetypes(define(f32x8), v3, neon, wasm128, scalar)]
fn row_u8_impl(
    token: Token,
    p0: &[f32],
    p1: &[f32],
    p2: &[f32],
    inv: [f32; 3],
    dst: &mut [u8],
    width: usize,
) {
    let splat = |a| f32x8::splat_t(token, a);
    let clamp = |v: f32x8, lo: f32x8, hi: f32x8| incant!(crate::wasm_relaxed::clamp_f32x8(token, v, lo, hi) without token);
    let select = |m: f32x8, linear: f32x8, gamma: f32x8| incant!(crate::wasm_relaxed::sel_f32x8(token, m, linear, gamma) without token);
    let conv = |a: &[f32; 8], b: &[f32; 8], c: &[f32; 8]| {
        let planes = [a, b, c].map(|p| f32x8::load_t(token, p));
        power_u8_codes!(f32x8, splat, clamp, select, planes, inv)
    };
    let mut planes = p0
        .as_chunks::<8>()
        .0
        .iter()
        .zip(p1.as_chunks::<8>().0)
        .zip(p2.as_chunks::<8>().0);
    let (blocks, tail) = dst[..width * 3].as_chunks_mut::<24>();
    for block in blocks {
        let ((a, b), c) = planes.next().unwrap();
        let [r, g, b] = conv(a, b, c);
        incant!(store_rgb8(token, r, g, b, block) without token);
    }
    if !tail.is_empty() {
        let ((a, b), c) = planes.next().unwrap();
        let [r, g, b] = conv(a, b, c);
        let mut block = [0u8; 24];
        incant!(store_rgb8(token, r, g, b, &mut block) without token);
        tail.copy_from_slice(&block[..tail.len()]);
    }
}

#[magetypes(define(f32x8), v3, neon, wasm128, scalar)]
fn row_u16_impl(
    token: Token,
    p0: &[f32],
    p1: &[f32],
    p2: &[f32],
    inv: [f32; 3],
    dst: &mut [u8],
    width: usize,
) {
    let splat = |a| f32x8::splat_t(token, a);
    let clamp = |v: f32x8, lo: f32x8, hi: f32x8| incant!(crate::wasm_relaxed::clamp_f32x8(token, v, lo, hi) without token);
    let select = |m: f32x8, linear: f32x8, gamma: f32x8| incant!(crate::wasm_relaxed::sel_f32x8(token, m, linear, gamma) without token);
    let conv = |a: &[f32; 8], b: &[f32; 8], c: &[f32; 8]| {
        let planes = [a, b, c].map(|p| f32x8::load_t(token, p));
        power_u16_codes!(f32x8, splat, clamp, select, planes, inv)
    };
    let mut planes = p0
        .as_chunks::<8>()
        .0
        .iter()
        .zip(p1.as_chunks::<8>().0)
        .zip(p2.as_chunks::<8>().0);
    let (blocks, tail) = dst[..width * 6].as_chunks_mut::<48>();
    for block in blocks {
        let ((a, b), c) = planes.next().unwrap();
        let [r, g, b] = conv(a, b, c);
        incant!(store_rgb16(token, r, g, b, block) without token);
    }
    if !tail.is_empty() {
        let ((a, b), c) = planes.next().unwrap();
        let [r, g, b] = conv(a, b, c);
        let mut block = [0u8; 48];
        incant!(store_rgb16(token, r, g, b, &mut block) without token);
        tail.copy_from_slice(&block[..tail.len()]);
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn row_u8(
    simd: bool,
    p0: &[f32],
    p1: &[f32],
    p2: &[f32],
    inv: [f32; 3],
    dst: &mut [u8],
    width: usize,
) {
    if !simd || crate::v1::force_scalar() {
        row_u8_impl_scalar(ScalarToken, p0, p1, p2, inv, dst, width);
        return;
    }
    #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
    let done = if !crate::v1::force_avx2()
        && let Some(t) = X64V4xToken::summon()
    {
        super::color_v4x::xyb_row::<true>(t, p0, p1, p2, inv, dst, width)
    } else {
        0
    };
    #[cfg(not(all(target_arch = "x86_64", feature = "avx512")))]
    let done = 0;
    if done < width {
        incant!(
            row_u8_impl(
                &p0[done..],
                &p1[done..],
                &p2[done..],
                inv,
                &mut dst[3 * done..],
                width - done
            ),
            [v3, neon, wasm128, scalar]
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn row_u16(
    simd: bool,
    p0: &[f32],
    p1: &[f32],
    p2: &[f32],
    inv: [f32; 3],
    dst: &mut [u8],
    width: usize,
) {
    if !simd || crate::v1::force_scalar() {
        row_u16_impl_scalar(ScalarToken, p0, p1, p2, inv, dst, width);
        return;
    }
    #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
    let done = if !crate::v1::force_avx2()
        && let Some(t) = X64V4xToken::summon()
    {
        super::color_v4x::xyb_row_u16(t, p0, p1, p2, inv, dst, width)
    } else {
        0
    };
    #[cfg(not(all(target_arch = "x86_64", feature = "avx512")))]
    let done = 0;
    if done < width {
        incant!(
            row_u16_impl(
                &p0[done..],
                &p1[done..],
                &p2[done..],
                inv,
                &mut dst[6 * done..],
                width - done
            ),
            [v3, neon, wasm128, scalar]
        );
    }
}

// ============================================================================
// AVX-512 16-lane helpers called from color_v4x
// ============================================================================

#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[rite]
pub(super) fn codes16_u8(
    token: X64V4xToken,
    planes: [&[f32; 16]; 3],
    inv: [f32; 3],
) -> [core::arch::x86_64::__m512; 3] {
    use magetypes::simd::generic::f32x16;

    let splat = |a| f32x16::splat_t(token, a);
    let clamp = |v: f32x16<X64V4xToken>, lo, hi| v.clamp(lo, hi);
    let select = |m: f32x16<X64V4xToken>, linear, gamma| f32x16::blend(m, linear, gamma);
    let planes = planes.map(|p| f32x16::load_t(token, p));
    power_u8_codes!(f32x16<X64V4xToken>, splat, clamp, select, planes, inv).map(|v| v.raw())
}

#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[rite]
pub(super) fn codes16_u16(
    token: X64V4xToken,
    planes: [&[f32; 16]; 3],
    inv: [f32; 3],
) -> [core::arch::x86_64::__m512; 3] {
    use magetypes::simd::generic::f32x16;

    let splat = |a| f32x16::splat_t(token, a);
    let clamp = |v: f32x16<X64V4xToken>, lo, hi| v.clamp(lo, hi);
    let select = |m: f32x16<X64V4xToken>, linear, gamma| f32x16::blend(m, linear, gamma);
    let planes = planes.map(|p| f32x16::load_t(token, p));
    power_u16_codes!(f32x16<X64V4xToken>, splat, clamp, select, planes, inv).map(|v| v.raw())
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use alloc::vec;

    fn eval_scalar_u8(a: f32) -> u8 {
        let a = a.clamp(0.0, 1.0);
        let y = if a < TOE {
            a * SLOPE_U8
        } else {
            let s = libm::sqrtf(a);
            let t = libm::sqrtf(s);
            let p01 = D4_U8[0] + D4_U8[1] * t;
            let p23 = D4_U8[2] + D4_U8[3] * t;
            (p01 + p23 * s) + D4_U8[4] * a
        };
        rne_pos(y.clamp(0.0, 255.0)).min(255.0) as u8
    }

    fn eval_scalar_u16(a: f32) -> u16 {
        let a = a.clamp(0.0, 1.0);
        let y = if a < TOE {
            a * SLOPE_U16
        } else {
            let s = libm::sqrtf(a);
            let t = libm::sqrtf(s);
            let p01 = D6_U16[0] + D6_U16[1] * t;
            let p23 = D6_U16[2] + D6_U16[3] * t;
            let p45 = D6_U16[4] + D6_U16[5] * t;
            (p01 + p23 * s) + (p45 + D6_U16[6] * s) * a
        };
        rne_pos(y.clamp(0.0, 65535.0)).min(65535.0) as u16
    }

    #[test]
    fn u8_d4_constant_bits_and_256_srgb8_exact_roundtrip() {
        assert_eq!(D4_U8[0].to_bits(), 0xc183_d01d);
        for k in 0u16..=255 {
            let s = f64::from(k) / 255.0;
            let lin = if s <= 0.04045 {
                s / 12.92
            } else {
                libm::pow((s + 0.055) / 1.055, 2.4)
            };
            assert_eq!(eval_scalar_u8(lin as f32), k as u8, "sRGB8 code {k}");
        }
    }

    #[test]
    fn u16_d6_65536_srgb16_exact_roundtrip() {
        for k in 0u32..=65535 {
            let s = f64::from(k) / 65535.0;
            let lin = if s <= 0.04045 {
                s / 12.92
            } else {
                libm::pow((s + 0.055) / 1.055, 2.4)
            };
            assert_eq!(eval_scalar_u16(lin as f32), k as u16, "sRGB16 code {k}");
        }
    }

    #[test]
    fn simd_matches_scalar_across_widths_and_adversarial_inputs() {
        let specials: [f32; 16] = [
            0.0,
            -0.0,
            -1e-30,
            1e-30,
            -0.25,
            -65.0,
            65.0,
            0.003_130_8,
            f32::from_bits(TOE.to_bits() - 1),
            f32::from_bits(TOE.to_bits() + 1),
            0.5,
            0.826_820_8,
            1.0,
            1.5,
            -1e10,
            1e10,
        ];
        for width in [1usize, 7, 8, 15, 16, 17, 31, 32, 33, 63, 64, 65, 131] {
            let cap = width.div_ceil(32) * 32;
            let mut p0 = vec![0.0f32; cap];
            let mut p1 = vec![0.0f32; cap];
            let mut p2 = vec![0.0f32; cap];
            for i in 0..cap {
                if i < specials.len() {
                    p0[i] = specials[i];
                    p1[i] = specials[(i * 3 + 1) % specials.len()] * 0.02;
                    p2[i] = specials[(i * 7 + 5) % specials.len()] * 0.05;
                } else {
                    let f = i as f32;
                    p0[i] = ((i * 73) % 1009) as f32 / 1008.0 * 1.2 - 0.1;
                    p1[i] = ((i * 197) % 503) as f32 / 502.0 * 0.14 - 0.07;
                    p2[i] = ((i * 313) % 701) as f32 / 700.0 * 0.8 - 0.4 + (f * 1e-6);
                }
            }
            for inv in [[1.0, 1.0, 1.0], [1.0 / 128.0, 1.0 / 512.0, 1.0 / 64.0]] {
                let mut u8_simd = vec![0xa5u8; width * 3 + 7];
                let mut u8_scalar = vec![0xa5u8; width * 3 + 7];
                row_u8(true, &p0, &p1, &p2, inv, &mut u8_simd, width);
                row_u8(false, &p0, &p1, &p2, inv, &mut u8_scalar, width);
                assert_eq!(u8_simd, u8_scalar, "U8 mismatch width={width} inv={inv:?}");

                let mut u16_simd = vec![0x5au8; width * 6 + 7];
                let mut u16_scalar = vec![0x5au8; width * 6 + 7];
                row_u16(true, &p0, &p1, &p2, inv, &mut u16_simd, width);
                row_u16(false, &p0, &p1, &p2, inv, &mut u16_scalar, width);
                assert_eq!(
                    u16_simd, u16_scalar,
                    "U16 mismatch width={width} inv={inv:?}"
                );
            }
        }
    }
}
