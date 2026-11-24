//! AVX2 (x86-64-v3) YCoCg -> RGB8 row kernel with vector byte packing.
//!
//! Bit-identical to [`super::simd`]'s YCoCg row: the same IEEE operations in
//! the same order; only the output packing differs (16 pixels interleaved
//! with byte shuffles instead of per-pixel stores).

use crate::recon::RGB8_SHUF;
use archmage::prelude::*;

/// 16 u32 lanes (two halves, values 0..=255) -> 16 bytes in order.
#[rite(import_intrinsics)]
fn bytes16(_t: X64V3Token, lo: __m256i, hi: __m256i) -> __m128i {
    let lo = _mm_packus_epi32(
        _mm256_castsi256_si128(lo),
        _mm256_extracti128_si256::<1>(lo),
    );
    let hi = _mm_packus_epi32(
        _mm256_castsi256_si128(hi),
        _mm256_extracti128_si256::<1>(hi),
    );
    _mm_packus_epi16(lo, hi)
}

/// Interleaves 16 pixels of R, G, B bytes into 48 bytes.
#[rite(import_intrinsics)]
fn store_rgb16(_t: X64V3Token, r: __m128i, g: __m128i, b: __m128i, dst: &mut [u8; 48]) {
    let (d0, rest) = dst.split_at_mut(16);
    let (d1, d2) = rest.split_at_mut(16);
    for (part, d) in [d0, d1, d2].into_iter().enumerate() {
        let tb = &RGB8_SHUF[part];
        let v = _mm_or_si128(
            _mm_or_si128(
                _mm_shuffle_epi8(r, _mm_loadu_si128(&tb[0])),
                _mm_shuffle_epi8(g, _mm_loadu_si128(&tb[1])),
            ),
            _mm_shuffle_epi8(b, _mm_loadu_si128(&tb[2])),
        );
        let d: &mut [u8; 16] = d.try_into().unwrap();
        _mm_storeu_si128(d, v);
    }
}

/// YCoCg of 8 pixels -> R, G, B as i32 (0..=255).
#[rite(import_intrinsics)]
fn ycocg8(_t: X64V3Token, a: &[f32; 8], b: &[f32; 8], c: &[f32; 8], inv: [f32; 3]) -> [__m256i; 3] {
    let h = _mm256_set1_ps(0.5);
    let max = _mm256_set1_ps(255.0);
    // Same result as the reference's clamp(0, 255) + round-half-even: cvtps
    // rounds half-even, packus saturates negatives (and NaN's 0x80000000)
    // to 0, and min() keeps huge values from wrapping.
    let q = |v: __m256| _mm256_cvtps_epi32(_mm256_min_ps(v, max));
    let y = _mm256_mul_ps(_mm256_loadu_ps(a), _mm256_set1_ps(inv[0]));
    let co = _mm256_mul_ps(_mm256_loadu_ps(b), _mm256_set1_ps(inv[1]));
    let cg = _mm256_mul_ps(_mm256_loadu_ps(c), _mm256_set1_ps(inv[2]));
    let t = _mm256_sub_ps(y, _mm256_mul_ps(cg, h));
    let g = _mm256_add_ps(cg, t);
    let bb = _mm256_sub_ps(t, _mm256_mul_ps(co, h));
    let r = _mm256_add_ps(bb, co);
    [q(r), q(g), q(bb)]
}

fn halves(s: &[f32; 16]) -> (&[f32; 8], &[f32; 8]) {
    let (x, y) = s.as_chunks::<8>().0.split_at(1);
    (&x[0], &y[0])
}

/// Converts the first `w / 16 * 16` pixels of three YCoCg plane rows to
/// RGB8; returns the number of pixels done.
#[arcane(import_intrinsics)]
pub(crate) fn ycocg_row(
    t: X64V3Token,
    p0: &[f32],
    p1: &[f32],
    p2: &[f32],
    inv: [f32; 3],
    out: &mut [u8],
    w: usize,
) -> usize {
    let n = w / 16;
    let a16 = &p0.as_chunks::<16>().0[..n];
    let b16 = &p1.as_chunks::<16>().0[..n];
    let c16 = &p2.as_chunks::<16>().0[..n];
    let rgb = out[..n * 48].as_chunks_mut::<48>().0;
    for (((a, b), c), o) in a16.iter().zip(b16).zip(c16).zip(rgb) {
        let (a0, a1) = halves(a);
        let (b0, b1) = halves(b);
        let (c0, c1) = halves(c);
        let lo = ycocg8(t, a0, b0, c0, inv);
        let hi = ycocg8(t, a1, b1, c1, inv);
        let r = bytes16(t, lo[0], hi[0]);
        let gg = bytes16(t, lo[1], hi[1]);
        let b = bytes16(t, lo[2], hi[2]);
        store_rgb16(t, r, gg, b, o);
    }
    n * 16
}
