//! Private fixed-point inverse experiment. No native reconstruction change.
//! Signed i16 coefficients -> signed i16 residuals, row first, Q12 rotations,
//! nearest ties toward +infinity, 19-bit butterfly clipping, final / (N/2)
//! rounding and explicit signed saturation. See tools/integer-transform-study.
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
use archmage::prelude::*;
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
use std::hint::black_box;
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
use std::path::Path;
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
use std::time::Duration;
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[path = "integer_study_network.rs"]
mod network;
#[path = "integer_study_reference.rs"]
mod reference;
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
use archmage::intrinsics::x86_64::__m512i;

#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
use magetypes::simd::{backends::I32x16Backend, generic::i32x16};
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[inline(always)]
fn clip_generic<T: I32x16Backend>(token: T, v: i32x16<T>) -> i32x16<T> {
    v.max(i32x16::splat_t(token, -262144))
        .min(i32x16::splat_t(token, 262143))
}
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[inline(always)]
fn rotate_generic<T: I32x16Backend>(
    token: T,
    a: i32x16<T>,
    b: i32x16<T>,
    c: i32,
    d: i32,
) -> i32x16<T> {
    (a * i32x16::splat_t(token, c) + b * i32x16::splat_t(token, d) + i32x16::splat_t(token, 2048))
        .shr_arithmetic_const::<12>()
}
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
struct Stage {
    a: [i32; 16],
    b: [i32; 16],
    c: [i32; 16],
    d: [i32; 16],
    rotate: u16,
    add: u16,
    sub: u16,
}

#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[rite(import_intrinsics)]
fn clip(_token: X64V4Token, v: __m512i) -> __m512i {
    _mm512_min_epi32(
        _mm512_max_epi32(v, _mm512_set1_epi32(-262144)),
        _mm512_set1_epi32(262143),
    )
}
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[rite(import_intrinsics)]
fn rotate(_token: X64V4Token, a: __m512i, b: __m512i, c: i32, d: i32) -> __m512i {
    _mm512_srai_epi32::<12>(_mm512_add_epi32(
        _mm512_add_epi32(
            _mm512_mullo_epi32(a, _mm512_set1_epi32(c)),
            _mm512_mullo_epi32(b, _mm512_set1_epi32(d)),
        ),
        _mm512_set1_epi32(2048),
    ))
}
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[rite(import_intrinsics)]
fn lane_stage(token: X64V4Token, v: __m512i, s: &Stage) -> __m512i {
    let a = _mm512_permutexvar_epi32(_mm512_loadu_si512(&s.a), v);
    let b = _mm512_permutexvar_epi32(_mm512_loadu_si512(&s.b), v);
    let mut out = a;
    if s.add != 0 {
        out = _mm512_mask_add_epi32(out, s.add, a, b);
    }
    if s.sub != 0 {
        out = _mm512_mask_sub_epi32(out, s.sub, a, b);
    }
    if s.add | s.sub != 0 {
        out = _mm512_mask_mov_epi32(out, s.add | s.sub, clip(token, out));
    }
    if s.rotate != 0 {
        let sum = _mm512_add_epi32(
            _mm512_mullo_epi32(a, _mm512_loadu_si512(&s.c)),
            _mm512_mullo_epi32(b, _mm512_loadu_si512(&s.d)),
        );
        let r = _mm512_srai_epi32::<12>(_mm512_add_epi32(sum, _mm512_set1_epi32(2048)));
        out = _mm512_mask_mov_epi32(out, s.rotate, r);
    }
    out
}

// Same complete fixed transpose network as the float study. Bitcasts and
// float-named shuffles only rearrange bits: no floating arithmetic is used.
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[rite(import_intrinsics)]
fn transpose16(_token: X64V4Token, v: &mut [__m512i; 16]) {
    let z = _mm512_setzero_si512();
    let mut t = [z; 16];
    let mut s = [z; 16];
    for pair in 0..8 {
        let i = pair * 2;
        t[i] = _mm512_unpacklo_epi32(v[i], v[i + 1]);
        t[i + 1] = _mm512_unpackhi_epi32(v[i], v[i + 1]);
    }
    for group in 0..4 {
        let i = group * 4;
        s[i] = _mm512_castps_si512(_mm512_shuffle_ps::<0x44>(
            _mm512_castsi512_ps(t[i]),
            _mm512_castsi512_ps(t[i + 2]),
        ));
        s[i + 1] = _mm512_castps_si512(_mm512_shuffle_ps::<0xee>(
            _mm512_castsi512_ps(t[i]),
            _mm512_castsi512_ps(t[i + 2]),
        ));
        s[i + 2] = _mm512_castps_si512(_mm512_shuffle_ps::<0x44>(
            _mm512_castsi512_ps(t[i + 1]),
            _mm512_castsi512_ps(t[i + 3]),
        ));
        s[i + 3] = _mm512_castps_si512(_mm512_shuffle_ps::<0xee>(
            _mm512_castsi512_ps(t[i + 1]),
            _mm512_castsi512_ps(t[i + 3]),
        ));
    }
    for j in 0..4 {
        let a = _mm512_shuffle_i32x4::<0x88>(s[j], s[j + 4]);
        let b = _mm512_shuffle_i32x4::<0xdd>(s[j], s[j + 4]);
        let c = _mm512_shuffle_i32x4::<0x88>(s[j + 8], s[j + 12]);
        let d = _mm512_shuffle_i32x4::<0xdd>(s[j + 8], s[j + 12]);
        v[j] = _mm512_shuffle_i32x4::<0x88>(a, c);
        v[j + 8] = _mm512_shuffle_i32x4::<0xdd>(a, c);
        v[j + 4] = _mm512_shuffle_i32x4::<0x88>(b, d);
        v[j + 12] = _mm512_shuffle_i32x4::<0xdd>(b, d);
    }
}
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[rite(import_intrinsics)]
fn transpose_pair(token: X64V4Token, v: &mut [__m512i; 8]) {
    let mut bits = v.map(|a| _mm512_castsi512_ps(a));
    crate::lossy::recon_v4::study_transpose2(token, &mut bits);
    for (dst, src) in v.iter_mut().zip(bits) {
        *dst = _mm512_castps_si512(src);
    }
}
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[rite(import_intrinsics)]
fn pack<const N: usize>(_token: X64V4Token, v: __m512i, out: &mut [i16; 16]) {
    let scaled = if N == 16 {
        _mm512_srai_epi32::<3>(_mm512_add_epi32(v, _mm512_set1_epi32(4)))
    } else {
        _mm512_srai_epi32::<2>(_mm512_add_epi32(v, _mm512_set1_epi32(2)))
    };
    _mm256_storeu_si256(out, _mm512_cvtsepi32_epi16(scaled));
}
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[derive(Clone, Copy, Debug)]
enum Layout {
    Within,
    Across,
    Generic,
}
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[arcane(import_intrinsics)]
fn tile<const N: usize>(
    token: X64V4Token,
    src: &[i16],
    stride: usize,
    dst: &mut [i16],
    out_stride: usize,
    layout: Layout,
) {
    if N == 16 {
        let mut v = core::array::from_fn(|y| {
            _mm512_cvtepi16_epi32(_mm256_loadu_si256::<[i16; 16]>(
                src[y * stride..][..16].try_into().unwrap(),
            ))
        });
        match layout {
            Layout::Within => {
                for row in &mut v {
                    *row = network::lanes16(token, *row);
                }
                transpose16(token, &mut v);
                for row in &mut v {
                    *row = network::lanes16(token, *row);
                }
                transpose16(token, &mut v);
            }
            Layout::Across => {
                transpose16(token, &mut v);
                network::columns16(token, &mut v);
                transpose16(token, &mut v);
                network::columns16(token, &mut v);
            }
            Layout::Generic => {
                transpose16(token, &mut v);
                let mut q = v.map(|v| i32x16::from_repr_t(token, v));
                network::columns16_generic(token, &mut q);
                v = q.map(|q| q.into_repr());
                transpose16(token, &mut v);
                let mut q = v.map(|v| i32x16::from_repr_t(token, v));
                network::columns16_generic(token, &mut q);
                v = q.map(|q| q.into_repr());
            }
        }
        for (y, row) in v.into_iter().enumerate() {
            pack::<16>(
                token,
                row,
                (&mut dst[y * out_stride..][..16]).try_into().unwrap(),
            );
        }
    } else {
        for by in [0, 8] {
            let mut v = core::array::from_fn(|y| {
                _mm512_cvtepi16_epi32(_mm256_loadu_si256::<[i16; 16]>(
                    src[(by + y) * stride..][..16].try_into().unwrap(),
                ))
            });
            match layout {
                Layout::Within => {
                    for row in &mut v {
                        *row = network::lanes8(token, *row);
                    }
                    transpose_pair(token, &mut v);
                    for row in &mut v {
                        *row = network::lanes8(token, *row);
                    }
                    transpose_pair(token, &mut v);
                }
                Layout::Across => {
                    transpose_pair(token, &mut v);
                    network::columns8(token, &mut v);
                    transpose_pair(token, &mut v);
                    network::columns8(token, &mut v);
                }
                Layout::Generic => {
                    transpose_pair(token, &mut v);
                    let mut q = v.map(|v| i32x16::from_repr_t(token, v));
                    network::columns8_generic(token, &mut q);
                    v = q.map(|q| q.into_repr());
                    transpose_pair(token, &mut v);
                    let mut q = v.map(|v| i32x16::from_repr_t(token, v));
                    network::columns8_generic(token, &mut q);
                    v = q.map(|q| q.into_repr());
                }
            }
            for (y, row) in v.into_iter().enumerate() {
                pack::<8>(
                    token,
                    row,
                    (&mut dst[(by + y) * out_stride..][..16])
                        .try_into()
                        .unwrap(),
                );
            }
        }
    }
}
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
fn checked(
    src: &[i16],
    stride: usize,
    dst: &mut [i16],
    out_stride: usize,
    n: usize,
    layout: Layout,
) -> Result<(), crate::Error> {
    if stride < 16 || out_stride < 16 || ![8, 16].contains(&n) {
        return Err(crate::Error::InvalidInput("integer study geometry"));
    }
    let input = stride
        .checked_mul(15)
        .and_then(|n| n.checked_add(16))
        .ok_or(crate::Error::TooLarge)?;
    let output = out_stride
        .checked_mul(15)
        .and_then(|n| n.checked_add(16))
        .ok_or(crate::Error::TooLarge)?;
    if src.len() < input || dst.len() < output {
        return Err(crate::Error::InvalidInput("integer study extent"));
    }
    let token =
        X64V4Token::summon().ok_or(crate::Error::Unsupported("integer study needs x86 v4"))?;
    if n == 16 {
        tile::<16>(token, src, stride, dst, out_stride, layout);
    } else {
        tile::<8>(token, src, stride, dst, out_stride, layout);
    }
    Ok(())
}
fn scalar(src: &[i16], stride: usize, n: usize) -> [i16; 256] {
    let mut intermediate = [0i32; 256];
    let mut out = [0i16; 256];
    for by in 0..16 / n {
        for bx in 0..16 / n {
            for y in 0..n {
                let mut input = [0; 16];
                let mut output = [0; 16];
                for x in 0..n {
                    input[x] = i32::from(src[(by * n + y) * stride + bx * n + x]);
                }
                reference::inverse(&input, &mut output, n);
                intermediate[(by * n + y) * 16 + bx * n..][..n].copy_from_slice(&output[..n]);
            }
            for x in 0..n {
                let mut input = [0; 16];
                let mut output = [0; 16];
                for y in 0..n {
                    input[y] = intermediate[(by * n + y) * 16 + bx * n + x];
                }
                reference::inverse(&input, &mut output, n);
                for y in 0..n {
                    let shift = if n == 16 { 3 } else { 2 };
                    out[(by * n + y) * 16 + bx * n + x] =
                        ((output[y] + (1 << (shift - 1))) >> shift)
                            .clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                }
            }
        }
    }
    out
}
#[test]
fn integer_inverse_oracle() {
    for stride in [16, 23] {
        for pattern in 0..12 {
            let mut input = vec![0i16; 15 * stride + 16];
            for y in 0..16 {
                for x in 0..16 {
                    input[y * stride + x] = match pattern {
                        0 => 0,
                        1 => i16::MAX,
                        2 => i16::MIN,
                        3 => {
                            if (x + y) % 2 == 0 {
                                i16::MIN
                            } else {
                                i16::MAX
                            }
                        }
                        4 => {
                            if x == 0 && y == 0 {
                                2048
                            } else {
                                0
                            }
                        }
                        5 => {
                            if x == 0 && y == 0 {
                                -2048
                            } else {
                                0
                            }
                        }
                        _ => {
                            ((x * 3701 + y * 9743 + x * y * 211 + pattern * 1879) % 65536) as i32
                                as i16
                        }
                    };
                }
            }
            for n in [8, 16] {
                let expected = scalar(&input, stride, n);
                #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
                if X64V4Token::summon().is_some() {
                    for layout in [Layout::Within, Layout::Across, Layout::Generic] {
                        let mut actual = vec![12345i16; 19 * 15 + 16];
                        checked(&input, stride, &mut actual, 19, n, layout).unwrap();
                        for y in 0..16 {
                            assert_eq!(
                                &actual[y * 19..][..16],
                                &expected[y * 16..][..16],
                                "{layout:?} N={n} pattern={pattern} row={y}"
                            );
                            if y < 15 {
                                assert_eq!(&actual[y * 19 + 16..][..3], &[12345; 3]);
                            }
                        }
                    }
                }
                assert_eq!(expected.len(), 256);
            }
        }
    }
    // All coefficient basis positions, both signs, independent cosine check.
    // The wide integer reference remains the exact reconstruction contract;
    // the f64 oracle checks transform completeness, sign and normalization.
    let mut max_error = 0.0f64;
    for n in [8, 16] {
        for at in 0..256 {
            for sign in [-1i16, 1] {
                let mut input = [0i16; 256];
                input[at] = 4096 * sign;
                let expected = scalar(&input, 16, n);
                #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
                if X64V4Token::summon().is_some() {
                    for layout in [Layout::Within, Layout::Across, Layout::Generic] {
                        let mut actual = [0; 256];
                        checked(&input, 16, &mut actual, 16, n, layout).unwrap();
                        assert_eq!(actual, expected, "basis {at} N={n} {layout:?}");
                    }
                }
                let (by, bx) = (at / 16 / n, at % 16 / n);
                let (v, u) = (at / 16 % n, at % 16 % n);
                let basis = |k: usize, x: usize| {
                    if k == 0 {
                        1.0 / (n as f64).sqrt()
                    } else {
                        (2.0 / n as f64).sqrt()
                            * (((2 * x + 1) * k) as f64 * core::f64::consts::PI / (2 * n) as f64)
                                .cos()
                    }
                };
                for y in 0..n {
                    for x in 0..n {
                        let ideal = 4096.0 * f64::from(sign) * basis(v, y) * basis(u, x);
                        let error =
                            (f64::from(expected[(by * n + y) * 16 + bx * n + x]) - ideal).abs();
                        max_error = max_error.max(error);
                        assert!(
                            error <= 8.0,
                            "cosine basis N={n} ({v},{u}) sample({y},{x}): {error}"
                        );
                    }
                }
            }
        }
    }
    #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
    {
        let mut dst = [12345; 256];
        assert!(checked(&[0; 256], usize::MAX, &mut dst, 16, 16, Layout::Within).is_err());
        assert_eq!(dst, [12345; 256]);
        if let Some(path) = std::env::var_os("IMI_INTEGER_OUTPUT") {
            measure(Path::new(&path), max_error);
        }
        if let Ok(layout) = std::env::var("IMI_INTEGER_PROFILE") {
            profile(&layout);
        }
    }
    let _ = max_error;
}

#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
fn fixture(n: usize, support: usize, batch: usize) -> Vec<Vec<i16>> {
    let mut out = vec![vec![0; 23 * 15 + 16]; batch];
    for (b, tile) in out.iter_mut().enumerate() {
        for by in 0..16 / n {
            for bx in 0..16 / n {
                let count = if support == 16 {
                    n
                } else if support == 4 {
                    n / 4
                } else {
                    1
                };
                for y in 0..count {
                    for x in 0..count {
                        let (gx, gy) = (bx * n + x, by * n + y);
                        tile[gy * 23 + gx] = if support == 1 {
                            n as i16 * 16
                        } else {
                            ((gx * 37 + gy * 97 + b * 11) % 257) as i16 - 128
                        };
                    }
                }
            }
        }
    }
    out
}
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[arcane]
fn batch<const N: usize>(
    token: X64V4Token,
    input: &[Vec<i16>],
    out: &mut [[i16; 256]],
    layout: Layout,
) {
    for (src, dst) in input.iter().zip(out) {
        tile::<N>(token, src, 23, dst, 16, layout);
    }
}
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
fn measure(root: &Path, max_error: f64) {
    use serde_json::json;
    let token = X64V4Token::summon().expect("explicit AVX-512 measurement");
    std::fs::create_dir(root).unwrap();
    let source = std::env::var("IMI_INTEGER_SOURCE").expect("frozen source identity");
    let digest = crate::HashAlgorithm::Xxh3_128
        .digest(&std::fs::read(std::env::current_exe().unwrap()).unwrap());
    let id = format!(
        "{:032x}",
        u128::from_le_bytes(digest.as_bytes().try_into().unwrap())
    );
    std::fs::write(root.join("identity.json"),serde_json::to_vec_pretty(&json!({"source_revision":source,"binary_xxh3_128":id,"workload":"integer-q12-rowfirst-clip19-i16-v2","isa":"X64V4Token","source_stride":23,"output_stride":16,"input":"full signed i16 domain admitted; timed dense values -128..128","output":"signed saturated i16 residual, no level shift","rotation":"(c*a+d*b+2048)>>12, ties toward positive infinity","butterfly_clip":[-262144,262143],"normalization":"16: rounded >>3; 8: rounded >>2 after both passes","independent_cosine_basis_max_error":max_error,"metric_maps":false,"nas_writes":0,"production_reconstruction_changed":false})).unwrap()).unwrap();
    let gate = zenbench::GateConfig::strict()
        .max_heavy_processes(4)
        .min_available_ram_mb(4096)
        .max_cpu_temp_c(Some(85.0))
        .max_wait(Duration::from_secs(30));
    let result = zenbench::run_gated(gate, |suite| {
        suite.plan().workload_id = Some("integer-q12-rowfirst-clip19-i16-v2".into());
        for count in [1, 64] {
            for support in [16, 4, 1] {
                suite.group(format!("inverse-support{support}-batch{count}"), |g| {
                    g.config()
                        .min_rounds(24)
                        .max_rounds(24)
                        .auto_rounds(false)
                        .warmup_time(Duration::from_millis(200))
                        .max_wall_time(Duration::from_secs(90));
                    g.throughput(zenbench::Throughput::Elements((256 * count) as u64));
                    for (name, n, layout) in [
                        ("four8-within", 8, Layout::Within),
                        ("one16-within", 16, Layout::Within),
                        ("four8-across", 8, Layout::Across),
                        ("one16-across", 16, Layout::Across),
                        ("aa8-across", 8, Layout::Across),
                        ("four8-generic", 8, Layout::Generic),
                        ("one16-generic", 16, Layout::Generic),
                        ("aa8-generic", 8, Layout::Generic),
                    ] {
                        let input = fixture(n, support, count);
                        let mut out = vec![[0; 256]; count];
                        for (src, dst) in input.iter().zip(&mut out) {
                            checked(src, 23, dst, 16, n, layout).unwrap();
                            assert_eq!(*dst, scalar(src, 23, n));
                        }
                        g.bench(name, move |b| {
                            b.iter(|| {
                                if n == 16 {
                                    batch::<16>(token, black_box(&input), &mut out, layout);
                                } else {
                                    batch::<8>(token, black_box(&input), &mut out, layout);
                                }
                                black_box(&out);
                            })
                        });
                    }
                });
            }
        }
    });
    result.save(root.join("timing.json")).unwrap();
    let r: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("timing.json")).unwrap()).unwrap();
    assert_eq!(r["incomplete"], false);
    assert_eq!(r["unreliable"], false);
    for g in r["comparisons"].as_array().unwrap() {
        assert_eq!(g["completed_rounds"], 24);
        assert!(
            g["samples"]
                .as_array()
                .unwrap()
                .iter()
                .all(|s| s["gate_clean"] == true)
        );
    }
    std::fs::write(
        root.join("COMPLETE"),
        b"integer scalar/cosine/SIMD/stride oracles and paired gates pass\n",
    )
    .unwrap();
}
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
fn profile(spec: &str) {
    let fields: Vec<_> = spec.split(':').collect();
    assert_eq!(fields.len(), 2);
    let n = fields[0].parse::<usize>().unwrap();
    assert!([8, 16].contains(&n));
    let layout = match fields[1] {
        "within" => Layout::Within,
        "across" => Layout::Across,
        "generic" => Layout::Generic,
        _ => panic!("explicit integer layout"),
    };
    let input = fixture(n, 16, 64);
    let mut out = vec![[0; 256]; 64];
    let token = X64V4Token::summon().unwrap();
    for _ in 0..100000 {
        if n == 16 {
            batch::<16>(token, black_box(&input), &mut out, layout);
        } else {
            batch::<8>(token, black_box(&input), &mut out, layout);
        }
        black_box(&out);
    }
}
