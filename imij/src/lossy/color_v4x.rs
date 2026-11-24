//! AVX-512 (x86-64-v4x: BW for word permutes, VBMI for byte permutes)
//! XYB -> sRGB8 row kernel.
//!
//! Bit-identical to [`super::simd`]'s XYB row and to
//! [`super::color::inverse_px`]: the float math is the same IEEE operations
//! in the same order, 16 lanes wide. The normative sRGB table lookup is done
//! without a gather. The table is non-decreasing with steps of at most one
//! code per index, so a per-octave quadratic fit of it, truncated, is the
//! table value or one less (checked for every index in the tests); one
//! threshold comparison (`NEXT`, a 256-entry `vpermi2w` lookup) corrects
//! it. Two `vpermt2b` interleave the R, G, B bytes.

use super::color::{CBRT_BIAS, INV_OPSIN, OPSIN_BIAS, SRGB8_LOG_BASE, SRGB8_LOG_N};
use super::srgb_table::SRGB8_LOG;
use archmage::prelude::*;

/// `THR[v]`: first table index whose value is >= v (`SRGB8_LOG_N` if none;
/// `THR[0] = 0`).
const THR: [u16; 256] = {
    let mut t = [SRGB8_LOG_N as u16; 256];
    let mut v = 0;
    let mut i = 0;
    while i < SRGB8_LOG.len() {
        while v <= SRGB8_LOG[i] as usize {
            t[v] = i as u16;
            v += 1;
        }
        i += 1;
    }
    t
};

/// Byte n of 32 interleaved RGB pixels is R (0..32), G (32..64) or B
/// (64..96) of pixel n / 3 in the (RG, B) register pair.
static ILV: [u8; 128] = {
    let mut t = [0u8; 128];
    let mut n = 0;
    while n < 96 {
        t[n] = ((n % 3) * 32 + n / 3) as u8;
        n += 1;
    }
    t
};

/// Word n of 16 interleaved RGB16 pixels is R (0..16), G (16..32) or B
/// (32..48) of pixel n / 3 in the (RG, B) register pair.
static ILV16: [u16; 64] = {
    let mut t = [0u16; 64];
    let mut n = 0;
    while n < 48 {
        t[n] = ((n % 3) * 16 + n / 3) as u16;
        n += 1;
    }
    t
};

/// Per-octave (512 table indices) quadratic fit of the table, in the index
/// offset within the octave: within 0.28 codes of the exact transfer at
/// every index, so its truncation is the table value or one less.
static Q0: [f32; 16] = [
    0.4025656, 0.8051312, 1.6102624, 3.2205248, 6.412806, 12.712582, 21.665388, 33.615955,
    49.568043, 70.86153, 99.28492, 137.2256, 187.87033, 255.0, 255.0, 255.0,
];
static Q1: [f32; 16] = [
    0.0007854939,
    0.0015709877,
    0.0031419755,
    0.006283951,
    0.013100502,
    0.020769851,
    0.027724424,
    0.037007667,
    0.04939931,
    0.065940164,
    0.088019565,
    0.11749202,
    0.15683302,
    0.0,
    0.0,
    0.0,
];
static Q2: [f32; 16] = [
    6.8286735e-23,
    1.3657347e-22,
    2.7314694e-22,
    5.462939e-22,
    -1.5871087e-06,
    -6.7343462e-06,
    -8.989274e-06,
    -1.1999241e-05,
    -1.6017066e-05,
    -2.1380216e-05,
    -2.8539165e-05,
    -3.8095215e-05,
    -5.0851013e-05,
    0.0,
    0.0,
    0.0,
];

/// `NEXT[a] = THR[a + 1]`: the table value is `a + (NEXT[a] <= i)` when
/// it is a or a + 1.
static NEXT: [u16; 256] = {
    let mut t = [SRGB8_LOG_N as u16; 256];
    let mut a = 0;
    while a < 255 {
        t[a] = THR[a + 1];
        a += 1;
    }
    t
};

/// Scalar model of the fit + correction (checked against the table).
#[cfg(all(test, feature = "encode"))]
pub(crate) fn approx_scalar(i: u16) -> (i32, u8) {
    let o = (i >> 9) as usize;
    let m = (i & 511) as f32;
    let r = Q2[o].mul_add(m, Q1[o]).mul_add(m, Q0[o]);
    let a = r as i32;
    (a, (a + (NEXT[a.clamp(0, 255) as usize] <= i) as i32) as u8)
}

/// Table values from indices via the fit (`idx`: two i32x16 halves of 32
/// indices; `x`: the same indices as u16).
#[rite(import_intrinsics)]
fn approx(
    _t: X64V4xToken,
    idx: [__m512i; 2],
    x: __m512i,
    q: &[__m512; 3],
    nx: &[__m512i; 8],
) -> __m512i {
    let a: [__m256i; 2] = idx.map(|i| {
        let o = _mm512_srli_epi32::<9>(i);
        let m = _mm512_cvtepi32_ps(_mm512_and_si512(i, _mm512_set1_epi32(511)));
        let c2 = _mm512_permutexvar_ps(o, q[2]);
        let c1 = _mm512_permutexvar_ps(o, q[1]);
        let c0 = _mm512_permutexvar_ps(o, q[0]);
        let r = _mm512_fmadd_ps(_mm512_fmadd_ps(c2, m, c1), m, c0);
        _mm512_cvtepi32_epi16(_mm512_cvttps_epi32(r))
    });
    let a = _mm512_inserti64x4::<1>(_mm512_castsi256_si512(a[0]), a[1]);
    let p0 = _mm512_permutex2var_epi16(nx[0], a, nx[1]);
    let p1 = _mm512_permutex2var_epi16(nx[2], a, nx[3]);
    let p2 = _mm512_permutex2var_epi16(nx[4], a, nx[5]);
    let p3 = _mm512_permutex2var_epi16(nx[6], a, nx[7]);
    let b6 = _mm512_test_epi16_mask(a, _mm512_set1_epi16(64));
    let b7 = _mm512_test_epi16_mask(a, _mm512_set1_epi16(128));
    let lo = _mm512_mask_mov_epi16(p0, b6, p1);
    let hi = _mm512_mask_mov_epi16(p2, b6, p3);
    let th = _mm512_mask_mov_epi16(lo, b7, hi);
    let m = _mm512_cmple_epu16_mask(th, x);
    _mm512_mask_add_epi16(a, m, a, _mm512_set1_epi16(1))
}

/// XYB inverse of 16 pixels to table indices (i32 lanes), per channel.
#[rite(import_intrinsics)]
fn indices16(
    _t: X64V4xToken,
    a: &[f32; 16],
    b: &[f32; 16],
    c: &[f32; 16],
    inv: [f32; 3],
) -> [__m512i; 3] {
    let y = _mm512_mul_ps(_mm512_loadu_ps(a), _mm512_set1_ps(inv[0]));
    let xx = _mm512_mul_ps(_mm512_loadu_ps(b), _mm512_set1_ps(inv[1]));
    let bb = _mm512_add_ps(_mm512_mul_ps(_mm512_loadu_ps(c), _mm512_set1_ps(inv[2])), y);
    let bias = _mm512_set1_ps(CBRT_BIAS);
    let lo = _mm512_set1_ps(-64.0);
    let hi = _mm512_set1_ps(64.0);
    let clamp = |v: __m512, lo: __m512, hi: __m512| _mm512_min_ps(_mm512_max_ps(v, lo), hi);
    let l = clamp(_mm512_add_ps(_mm512_add_ps(y, xx), bias), lo, hi);
    let mm = clamp(_mm512_add_ps(_mm512_sub_ps(y, xx), bias), lo, hi);
    let s = clamp(_mm512_add_ps(bb, bias), lo, hi);
    let ob = _mm512_set1_ps(OPSIN_BIAS);
    let cube = |v: __m512| _mm512_sub_ps(_mm512_mul_ps(_mm512_mul_ps(v, v), v), ob);
    let (l3, m3, s3) = (cube(l), cube(mm), cube(s));
    let zero = _mm512_setzero_ps();
    let one = _mm512_set1_ps(1.0);
    let base = _mm512_set1_epi32(SRGB8_LOG_BASE);
    let imax = _mm512_set1_epi32(SRGB8_LOG_N - 1);
    core::array::from_fn(|ch| {
        let m = INV_OPSIN[ch];
        let lin = _mm512_add_ps(
            _mm512_add_ps(
                _mm512_mul_ps(_mm512_set1_ps(m[0]), l3),
                _mm512_mul_ps(_mm512_set1_ps(m[1]), m3),
            ),
            _mm512_mul_ps(_mm512_set1_ps(m[2]), s3),
        );
        let bits = _mm512_castps_si512(clamp(lin, zero, one));
        let i = _mm512_sub_epi32(_mm512_srai_epi32::<14>(bits), base);
        _mm512_min_epi32(_mm512_max_epi32(i, _mm512_setzero_si512()), imax)
    })
}

/// Converts the first `w / 32 * 32` pixels of three XYB plane rows to RGB8;
/// returns the number of pixels done.
#[arcane(import_intrinsics)]
pub(crate) fn xyb_row<const POWER: bool>(
    t: X64V4xToken,
    p0: &[f32],
    p1: &[f32],
    p2: &[f32],
    inv: [f32; 3],
    out: &mut [u8],
    w: usize,
) -> usize {
    let q = [Q0, Q1, Q2].map(|c| _mm512_loadu_ps(&c));
    let nx: [__m512i; 8] = core::array::from_fn(|i| {
        _mm512_loadu_si512(<&[u16; 32]>::try_from(&NEXT[i * 32..i * 32 + 32]).unwrap())
    });
    let ilv0 = _mm512_loadu_si512(<&[u8; 64]>::try_from(&ILV[..64]).unwrap());
    let ilv1 = _mm512_loadu_si512(<&[u8; 64]>::try_from(&ILV[64..]).unwrap());
    let n = w / 32;
    let a32 = p0.as_chunks::<32>().0;
    let b32 = p1.as_chunks::<32>().0;
    let c32 = p2.as_chunks::<32>().0;
    for (g, o) in out[..n * 96].as_chunks_mut::<96>().0.iter_mut().enumerate() {
        let (a, b, c) = (&a32[g], &b32[g], &c32[g]);
        let (a0, a1) = a.as_chunks::<16>().0.split_at(1);
        let (b0, b1) = b.as_chunks::<16>().0.split_at(1);
        let (c0, c1) = c.as_chunks::<16>().0.split_at(1);
        let ch: [__m256i; 3] = if POWER {
            let lo = super::xyb_power::codes16_u8(t, [&a0[0], &b0[0], &c0[0]], inv);
            let hi = super::xyb_power::codes16_u8(t, [&a1[0], &b1[0], &c1[0]], inv);
            core::array::from_fn(|k| {
                // Both transfer branches give finite codes in [0, 255.009].
                let pack = |v| {
                    let q = _mm512_cvt_roundps_epi32::<
                        { _MM_FROUND_TO_NEAREST_INT | _MM_FROUND_NO_EXC },
                    >(v);
                    _mm512_cvtusepi32_epi8(q)
                };
                _mm256_inserti128_si256::<1>(_mm256_castsi128_si256(pack(lo[k])), pack(hi[k]))
            })
        } else {
            let lo = indices16(t, &a0[0], &b0[0], &c0[0], inv);
            let hi = indices16(t, &a1[0], &b1[0], &c1[0], inv);
            // Per channel: 32 u16 indices in pixel order -> 32 bytes.
            core::array::from_fn(|k| {
                let x = _mm512_inserti64x4::<1>(
                    _mm512_castsi256_si512(_mm512_cvtepi32_epi16(lo[k])),
                    _mm512_cvtepi32_epi16(hi[k]),
                );
                _mm512_cvtepi16_epi8(approx(t, [lo[k], hi[k]], x, &q, &nx))
            })
        };
        let rg = _mm512_inserti64x4::<1>(_mm512_castsi256_si512(ch[0]), ch[1]);
        let bz = _mm512_castsi256_si512(ch[2]);
        let o0 = _mm512_permutex2var_epi8(rg, ilv0, bz);
        let o1 = _mm512_permutex2var_epi8(rg, ilv1, bz);
        let (h0, h1) = o.split_at_mut(64);
        _mm512_storeu_si512(<&mut [u8; 64]>::try_from(h0).unwrap(), o0);
        _mm256_storeu_si256(
            <&mut [u8; 32]>::try_from(h1).unwrap(),
            _mm512_castsi512_si256(o1),
        );
    }
    n * 32
}

/// Converts the first `w / 16 * 16` pixels of three XYB plane rows to RGB16
/// using the V4 `u16_dsqrt_d6` power transfer; returns the number of pixels done.
#[arcane(import_intrinsics)]
pub(crate) fn xyb_row_u16(
    t: X64V4xToken,
    p0: &[f32],
    p1: &[f32],
    p2: &[f32],
    inv: [f32; 3],
    out: &mut [u8],
    w: usize,
) -> usize {
    let ilv0 = _mm512_loadu_si512(<&[u16; 32]>::try_from(&ILV16[..32]).unwrap());
    let ilv1 = _mm512_loadu_si512(<&[u16; 32]>::try_from(&ILV16[32..]).unwrap());
    let n = w / 16;
    let a16 = p0.as_chunks::<16>().0;
    let b16 = p1.as_chunks::<16>().0;
    let c16 = p2.as_chunks::<16>().0;
    for (g, o) in out[..n * 96].as_chunks_mut::<96>().0.iter_mut().enumerate() {
        let codes = super::xyb_power::codes16_u16(t, [&a16[g], &b16[g], &c16[g]], inv);
        let ch: [__m256i; 3] = codes.map(|v| {
            let q =
                _mm512_cvt_roundps_epi32::<{ _MM_FROUND_TO_NEAREST_INT | _MM_FROUND_NO_EXC }>(v);
            _mm512_cvtusepi32_epi16(q)
        });
        let rg = _mm512_inserti64x4::<1>(_mm512_castsi256_si512(ch[0]), ch[1]);
        let bz = _mm512_castsi256_si512(ch[2]);
        let o0 = _mm512_permutex2var_epi16(rg, ilv0, bz);
        let o1 = _mm512_permutex2var_epi16(rg, ilv1, bz);
        let (h0, h1) = o.split_at_mut(64);
        _mm512_storeu_si512(<&mut [u8; 64]>::try_from(h0).unwrap(), o0);
        _mm256_storeu_si256(
            <&mut [u8; 32]>::try_from(h1).unwrap(),
            _mm512_castsi512_si256(o1),
        );
    }
    n * 16
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    #[test]
    fn fit_correction_matches_table() {
        for i in 0..SRGB8_LOG_N as usize {
            let (a, v) = approx_scalar(i as u16);
            let t = SRGB8_LOG[i] as i32;
            assert!(a == t || a == t - 1, "index {i}: fit {a}, table {t}");
            assert!(
                (0..=255).contains(&a),
                "index {i}: fit {a} (the kernel does not clamp)"
            );
            assert_eq!(v, SRGB8_LOG[i], "index {i}");
        }
    }
}
