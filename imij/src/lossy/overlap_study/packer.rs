//! Copy seven lanes of every eight without scalar per-block slice operations.
//! AVX-512 uses two-source permutations; AVX2 uses two permutations and a blend.
//! Other targets retain the exact slice-copy fallback. No floating arithmetic.
use super::*;
use archmage::prelude::*;

#[magetypes(define(f32x8), v3, neon, wasm128, scalar)]
fn mean_impl(token: Token, above: &[f32], current: &mut [f32]) {
    let half = f32x8::splat_t(token, 0.5);
    for (a, b) in above
        .as_chunks::<8>()
        .0
        .iter()
        .zip(current.as_chunks_mut::<8>().0)
    {
        let value = (f32x8::load_t(token, a) + f32x8::load_t(token, b)) * half;
        value.store(b);
    }
    let start = current.len() / 8 * 8;
    for (a, b) in above[start..].iter().zip(&mut current[start..]) {
        *b = (*a + *b) * 0.5;
    }
}

/// The caller replaces four-contributor corners in their normative sum order.
pub(super) fn vertical_mean(above: &[f32], current: &mut [f32]) {
    assert_eq!(above.len(), current.len());
    if crate::v1::force_scalar() {
        return mean_impl_scalar(ScalarToken, above, current);
    }
    incant!(mean_impl(above, current), [v3, neon, wasm128, scalar]);
}

fn scalar(input: &[f32], output: &mut [f32], blocks: usize) {
    for (source, target) in input[..blocks * 8]
        .as_chunks::<8>()
        .0
        .iter()
        .zip(output[..blocks * 7].as_chunks_mut::<7>().0)
    {
        target.copy_from_slice(&source[..7]);
    }
}

#[cfg(target_arch = "x86_64")]
const INDEX8: [[u32; 8]; 7] = {
    let mut result = [[0; 8]; 7];
    let mut k = 0;
    while k < 7 {
        let mut lane = 0;
        while lane < 8 {
            result[k][lane] = (lane + (k * 8 + lane) / 7) as u32;
            lane += 1;
        }
        k += 1;
    }
    result
};
#[cfg(target_arch = "x86_64")]
const MASK8: [[u32; 8]; 7] = {
    let mut result = [[0; 8]; 7];
    let mut k = 0;
    while k < 7 {
        let mut lane = 0;
        while lane < 8 {
            result[k][lane] = if INDEX8[k][lane] >= 8 { u32::MAX } else { 0 };
            lane += 1;
        }
        k += 1;
    }
    result
};

#[cfg(target_arch = "x86_64")]
#[arcane(import_intrinsics)]
fn v3(_token: X64V3Token, input: &[f32], output: &mut [f32]) {
    for (source, target) in input
        .as_chunks::<64>()
        .0
        .iter()
        .zip(output.as_chunks_mut::<56>().0)
    {
        let values: [__m256; 8] =
            core::array::from_fn(|i| _mm256_loadu_ps(source[i * 8..][..8].try_into().unwrap()));
        for k in 0..7 {
            let indices = _mm256_loadu_si256(&INDEX8[k]);
            let a = _mm256_permutevar8x32_ps(values[k], indices);
            let b = _mm256_permutevar8x32_ps(values[k + 1], indices);
            let mask = _mm256_castsi256_ps(_mm256_loadu_si256(&MASK8[k]));
            let value = _mm256_blendv_ps(a, b, mask);
            _mm256_storeu_ps((&mut target[k * 8..][..8]).try_into().unwrap(), value);
        }
    }
}

#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
const INDEX16: [[u32; 16]; 7] = {
    let mut result = [[0; 16]; 7];
    let mut k = 0;
    while k < 7 {
        let mut lane = 0;
        while lane < 16 {
            result[k][lane] = (lane + (k * 16 + lane) / 7) as u32;
            lane += 1;
        }
        k += 1;
    }
    result
};

#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[arcane(import_intrinsics)]
fn v4(_token: X64V4Token, input: &[f32], output: &mut [f32]) {
    for (source, target) in input
        .as_chunks::<128>()
        .0
        .iter()
        .zip(output.as_chunks_mut::<112>().0)
    {
        let values: [__m512; 8] =
            core::array::from_fn(|i| _mm512_loadu_ps(source[i * 16..][..16].try_into().unwrap()));
        for k in 0..7 {
            let indices = _mm512_loadu_si512(&INDEX16[k]);
            let value = _mm512_permutex2var_ps(values[k], indices, values[k + 1]);
            _mm512_storeu_ps((&mut target[k * 16..][..16]).try_into().unwrap(), value);
        }
    }
}

pub(super) fn row(input: &[f32], output: &mut [f32], blocks: usize) {
    assert!(input.len() >= blocks * 8 && output.len() >= blocks * 7);
    if crate::v1::force_scalar() {
        return scalar(input, output, blocks);
    }
    #[cfg(target_arch = "x86_64")]
    {
        let mut done = 0;
        #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
        if !crate::v1::force_avx2()
            && blocks >= 16
            && let Some(token) = X64V4Token::summon()
        {
            done = blocks / 16 * 16;
            v4(token, &input[..done * 8], &mut output[..done * 7]);
        }
        if blocks - done >= 8
            && let Some(token) = X64V3Token::summon()
        {
            let end = done + (blocks - done) / 8 * 8;
            v3(
                token,
                &input[done * 8..end * 8],
                &mut output[done * 7..end * 7],
            );
            done = end;
        }
        scalar(&input[done * 8..], &mut output[done * 7..], blocks - done);
    }
    #[cfg(not(target_arch = "x86_64"))]
    scalar(input, output, blocks);
}

#[test]
fn packing_preserves_every_bit_and_never_touches_tail() {
    let mut state = 31u32;
    for blocks in 0..71 {
        let input: Vec<_> = (0..blocks * 8)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                f32::from_bits(state)
            })
            .collect();
        let mut expected = vec![f32::from_bits(0x7FA1_2345); blocks * 7 + 13];
        scalar(&input, &mut expected, blocks);
        let mut actual = vec![f32::from_bits(0x7FA1_2345); blocks * 7 + 13];
        row(&input, &mut actual, blocks);
        assert_eq!(
            actual.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            expected.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            "blocks={blocks}"
        );
    }
}

#[test]
fn vertical_mean_preserves_scalar_arithmetic_and_tail() {
    for n in 0..73 {
        let above: Vec<_> = (0..n)
            .map(|i| ((i * 7919 % 997) as f32 - 498.0) / 17.0)
            .collect();
        let mut current: Vec<_> = (0..n)
            .map(|i| ((i * 4591 % 991) as f32 - 495.0) / 13.0)
            .collect();
        if n > 0 {
            current[0] = -0.0;
        }
        let expected: Vec<_> = above
            .iter()
            .zip(&current)
            .map(|(a, b)| ((*a + *b) * 0.5).to_bits())
            .collect();
        vertical_mean(&above, &mut current);
        assert_eq!(
            current.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            expected
        );
    }
}
