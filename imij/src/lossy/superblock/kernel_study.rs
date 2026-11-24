//! Private transform-only comparison. No codec syntax or normative changes.
use super::*;
use std::hint::black_box;
use std::path::Path;
use std::time::Duration;

#[path = "kernel_study_native.rs"]
mod native;

// The production 8x8 AAN flowgraph from enc_simd::fdct_aan_impl. Keep this
// baseline rather than the recursive 8-point helper used by the 16-point DCT.
#[inline(always)]
fn fwd_aan8<T: F32x8Backend>(token: T, v: &mut [vf8<T>; 8]) {
    let c4 = vf8::splat_t(token, core::f32::consts::FRAC_1_SQRT_2);
    let c6 = vf8::splat_t(token, 0.382_683_43);
    let cm = vf8::splat_t(token, 0.541_196_1);
    let cp = vf8::splat_t(token, 1.306_563);
    let (t0, t7, t1, t6, t2, t5, t3, t4) = (
        v[0] + v[7],
        v[0] - v[7],
        v[1] + v[6],
        v[1] - v[6],
        v[2] + v[5],
        v[2] - v[5],
        v[3] + v[4],
        v[3] - v[4],
    );
    let (e0, e3, e1, e2) = (t0 + t3, t0 - t3, t1 + t2, t1 - t2);
    v[0] = e0 + e1;
    v[4] = e0 - e1;
    let z1 = (e2 + e3) * c4;
    v[2] = e3 + z1;
    v[6] = e3 - z1;
    let (o0, o1, o2) = (t4 + t5, t5 + t6, t6 + t7);
    let z5 = (o0 - o2) * c6;
    let z2 = o0.mul_add_portable(cm, z5);
    let z4 = o2.mul_add_portable(cp, z5);
    let z3 = o1 * c4;
    let (z11, z13) = (t7 + z3, t7 - z3);
    v[5] = z13 + z2;
    v[3] = z13 - z2;
    v[1] = z11 + z4;
    v[7] = z11 - z4;
}

// Factorization and constants follow our jxl-encoder-simd/src/dct16.rs,
// whose recursive DCT-II flow derives from libjxl. See the study's NOTICE.
const WC4: [f32; 2] = [0.541_196_1, 1.306_563];
const WC8: [f32; 4] = [0.509_795_6, 0.601_344_9, 0.899_976_2, 2.562_915_5];
const WC16: [f32; 8] = [
    0.502_419_3,
    0.522_498_6,
    0.566_944_06,
    0.646_821_8,
    0.788_154_65,
    1.060_677_7,
    1.722_447_1,
    5.101_148_6,
];

#[inline(always)]
fn fwd4<T: F32x8Backend>(token: T, v: &mut [vf8<T>; 4]) {
    let (a0, a1, a2, a3) = (v[0] + v[3], v[1] + v[2], v[0] - v[3], v[1] - v[2]);
    let b0 = a2 * vf8::splat_t(token, WC4[0]);
    let b1 = a3 * vf8::splat_t(token, WC4[1]);
    let (q0, q1) = (b0 + b1, b0 - b1);
    *v = [
        a0 + a1,
        q0.mul_add_portable(vf8::splat_t(token, AAN_R2), q1),
        a0 - a1,
        q1,
    ];
}

#[inline(always)]
fn fwd8<T: F32x8Backend>(token: T, v: &mut [vf8<T>; 8]) {
    let mut a = [v[0] + v[7], v[1] + v[6], v[2] + v[5], v[3] + v[4]];
    let mut b = [
        (v[0] - v[7]) * vf8::splat_t(token, WC8[0]),
        (v[1] - v[6]) * vf8::splat_t(token, WC8[1]),
        (v[2] - v[5]) * vf8::splat_t(token, WC8[2]),
        (v[3] - v[4]) * vf8::splat_t(token, WC8[3]),
    ];
    fwd4(token, &mut a);
    fwd4(token, &mut b);
    let odd = [
        b[0].mul_add_portable(vf8::splat_t(token, AAN_R2), b[1]),
        b[1] + b[2],
        b[2] + b[3],
        b[3],
    ];
    for i in 0..4 {
        v[2 * i] = a[i];
        v[2 * i + 1] = odd[i];
    }
}

#[inline(always)]
fn fwd16<T: F32x8Backend>(token: T, v: &mut [vf8<T>; 16]) {
    let z = vf8::zero_t(token);
    let (mut a, mut b) = ([z; 8], [z; 8]);
    for i in 0..8 {
        a[i] = v[i] + v[15 - i];
        b[i] = (v[i] - v[15 - i]) * vf8::splat_t(token, WC16[i]);
    }
    fwd8(token, &mut a);
    fwd8(token, &mut b);
    b[0] = b[0].mul_add_portable(vf8::splat_t(token, AAN_R2), b[1]);
    for i in 1..7 {
        b[i] += b[i + 1];
    }
    for i in 0..8 {
        v[2 * i] = a[i];
        v[2 * i + 1] = b[i];
    }
}

/// One spatial tile, arbitrary source stride, contiguous transform output.
/// Both sizes use a 16x16 raster; 8x8 coefficients occupy four quadrants.
#[inline(always)]
fn tile_math<T: F32x8Backend, const INVERSE: bool, const N: usize, const SUPPORT: usize>(
    token: T,
    input: &[f32],
    stride: usize,
    out: &mut [f32; 256],
) {
    let k = Kc::new(token);
    if INVERSE && SUPPORT == 1 {
        for by in 0..16 / N {
            for bx in 0..16 / N {
                let v = vf8::splat_t(token, input[by * N * stride + bx * N]);
                for y in 0..N {
                    for x in (0..N).step_by(8) {
                        v.store(
                            (&mut out[(by * N + y) * 16 + bx * N + x..][..8])
                                .try_into()
                                .unwrap(),
                        );
                    }
                }
            }
        }
        return;
    }
    if N == 16 {
        let mut a = [[k.z; 16]; 2];
        let rows = 16;
        for oct in 0..2 {
            if INVERSE && oct * 8 >= SUPPORT {
                continue;
            }
            for y in 0..rows {
                a[oct][y] = vf8::load_t(
                    token,
                    input[y * stride + oct * 8..][..8].try_into().unwrap(),
                );
            }
            if INVERSE {
                pass(&k, &mut a[oct], 16, SUPPORT);
            } else {
                fwd16(token, &mut a[oct]);
            }
        }
        let mut b = [[k.z; 16]; 2];
        transpose(&k, &a, &mut b, [16, 16], 16, 2);
        for oct in &mut b {
            if INVERSE {
                pass(&k, oct, 16, SUPPORT);
            } else {
                fwd16(token, oct);
            }
        }
        for ty in 0..2 {
            for tx in 0..2 {
                let mut t = take8(&b[ty], tx * 8);
                vf8::transpose_8x8(&mut t);
                for (y, v) in t.iter().enumerate() {
                    let value = if INVERSE {
                        *v
                    } else {
                        *v * vf8::splat_t(token, 1.0 / 16.0)
                    };
                    value.store(
                        (&mut out[(ty * 8 + y) * 16 + tx * 8..][..8])
                            .try_into()
                            .unwrap(),
                    );
                }
            }
        }
    } else {
        for by in 0..2 {
            for bx in 0..2 {
                let mut a = [k.z; 8];
                for y in 0..8 {
                    a[y] = vf8::load_t(
                        token,
                        input[(by * 8 + y) * stride + bx * 8..][..8]
                            .try_into()
                            .unwrap(),
                    );
                }
                if INVERSE {
                    if SUPPORT == 4 {
                        pass8_2(&k, &mut a);
                    } else {
                        pass8(&k, &mut a);
                    }
                } else {
                    fwd_aan8(token, &mut a);
                }
                vf8::transpose_8x8(&mut a);
                if INVERSE {
                    if SUPPORT == 4 {
                        pass8_2(&k, &mut a);
                    } else {
                        pass8(&k, &mut a);
                    }
                } else {
                    fwd_aan8(token, &mut a);
                }
                vf8::transpose_8x8(&mut a);
                for (y, v) in a.iter().enumerate() {
                    let value = if INVERSE {
                        *v
                    } else {
                        let mut norm = [0.0; 8];
                        for x in 0..8 {
                            norm[x] = 0.125 / (dct::AAN[y] * dct::AAN[x]);
                        }
                        *v * vf8::from_array_t(token, norm)
                    };
                    value.store(
                        (&mut out[(by * 8 + y) * 16 + bx * 8..][..8])
                            .try_into()
                            .unwrap(),
                    );
                }
            }
        }
    }
}

#[magetypes(v4(cfg(avx512)), v3, neon, scalar)]
fn tile_impl<const INV: bool, const N: usize, const SUPPORT: usize>(
    token: Token,
    input: &[f32],
    stride: usize,
    out: &mut [f32; 256],
) {
    tile_math::<_, INV, N, SUPPORT>(token, input, stride, out);
}

#[magetypes(v4(cfg(avx512)), v3, neon, scalar)]
fn tile_batch_impl<const INV: bool, const N: usize, const SUPPORT: usize>(
    token: Token,
    input: &[Vec<f32>],
    stride: usize,
    out: &mut [[f32; 256]],
) {
    for (src, dst) in input.iter().zip(out) {
        tile_math::<_, INV, N, SUPPORT>(token, src, stride, dst);
    }
}

#[derive(Clone, Copy, Debug)]
enum Tier {
    Avx2,
    Avx512,
    Neon,
    Scalar,
}
impl Tier {
    fn supported(self) -> bool {
        use archmage::SimdToken;
        match self {
            #[cfg(target_arch = "x86_64")]
            Self::Avx2 => archmage::X64V3Token::summon().is_some(),
            #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
            Self::Avx512 => archmage::X64V4Token::summon().is_some(),
            #[cfg(target_arch = "aarch64")]
            Self::Neon => archmage::NeonToken::summon().is_some(),
            Self::Scalar => true,
            #[allow(unreachable_patterns)]
            _ => false,
        }
    }
    fn run<const INVERSE: bool, const N: usize>(
        self,
        input: &[f32],
        stride: usize,
        out: &mut [f32; 256],
    ) {
        self.run_sparse::<INVERSE, N, 16>(input, stride, out)
    }
    fn run_batch<const INV: bool, const N: usize, const SUPPORT: usize>(
        self,
        input: &[Vec<f32>],
        stride: usize,
        out: &mut [[f32; 256]],
    ) {
        use archmage::SimdToken;
        match self {
            #[cfg(target_arch = "x86_64")]
            Self::Avx2 => tile_batch_impl_v3::<INV, N, SUPPORT>(
                archmage::X64V3Token::summon().unwrap(),
                input,
                stride,
                out,
            ),
            #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
            Self::Avx512 => tile_batch_impl_v4::<INV, N, SUPPORT>(
                archmage::X64V4Token::summon().unwrap(),
                input,
                stride,
                out,
            ),
            #[cfg(target_arch = "aarch64")]
            Self::Neon => tile_batch_impl_neon::<INV, N, SUPPORT>(
                archmage::NeonToken::summon().unwrap(),
                input,
                stride,
                out,
            ),
            Self::Scalar => {
                tile_batch_impl_scalar::<INV, N, SUPPORT>(ScalarToken, input, stride, out)
            }
            #[allow(unreachable_patterns)]
            _ => panic!("unsupported requested batch tier"),
        }
    }
    fn run_sparse<const INVERSE: bool, const N: usize, const SUPPORT: usize>(
        self,
        input: &[f32],
        stride: usize,
        out: &mut [f32; 256],
    ) {
        use archmage::SimdToken;
        match self {
            #[cfg(target_arch = "x86_64")]
            Self::Avx2 => tile_impl_v3::<INVERSE, N, SUPPORT>(
                archmage::X64V3Token::summon().unwrap(),
                input,
                stride,
                out,
            ),
            #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
            Self::Avx512 => tile_impl_v4::<INVERSE, N, SUPPORT>(
                archmage::X64V4Token::summon().unwrap(),
                input,
                stride,
                out,
            ),
            #[cfg(target_arch = "aarch64")]
            Self::Neon => tile_impl_neon::<INVERSE, N, SUPPORT>(
                archmage::NeonToken::summon().unwrap(),
                input,
                stride,
                out,
            ),
            Self::Scalar => {
                tile_impl_scalar::<INVERSE, N, SUPPORT>(ScalarToken, input, stride, out)
            }
            #[allow(unreachable_patterns)]
            _ => panic!("explicitly requested unsupported ISA"),
        }
    }
}

fn reference(input: &[f32], stride: usize, n: usize, inverse: bool) -> [f32; 256] {
    let mut out = [0.0; 256];
    if inverse {
        if n == 16 {
            for y in 0..16 {
                out[y * 16..][..16].copy_from_slice(&input[y * stride..][..16]);
            }
            inverse_ref(Shape::S16x16, &mut out);
        } else {
            for by in 0..2 {
                for bx in 0..2 {
                    let mut a = [0.0; 64];
                    for y in 0..8 {
                        a[y * 8..][..8]
                            .copy_from_slice(&input[(by * 8 + y) * stride + bx * 8..][..8]);
                    }
                    dct::idct8x8(&mut a);
                    for y in 0..8 {
                        out[(by * 8 + y) * 16 + bx * 8..][..8].copy_from_slice(&a[y * 8..][..8]);
                    }
                }
            }
        }
    } else {
        for by in 0..16 / n {
            for bx in 0..16 / n {
                for v in 0..n {
                    for u in 0..n {
                        let mut sum = 0.0f64;
                        for y in 0..n {
                            for x in 0..n {
                                let c = |k: usize, p: usize| {
                                    if k == 0 {
                                        1.0 / (n as f64).sqrt()
                                    } else {
                                        (2.0 / n as f64).sqrt()
                                            * ((2 * p + 1) as f64
                                                * k as f64
                                                * core::f64::consts::PI
                                                / (2 * n) as f64)
                                                .cos()
                                    }
                                };
                                sum += f64::from(input[(by * n + y) * stride + bx * n + x])
                                    * c(v, y)
                                    * c(u, x);
                            }
                        }
                        out[(by * n + v) * 16 + bx * n + u] = sum as f32;
                    }
                }
            }
        }
    }
    out
}

#[test]
fn transform16_kernel_oracle() {
    for stride in [16, 23] {
        let mut input = vec![0.0; stride * 15 + 16];
        for pattern in 0..4 {
            for y in 0..16 {
                for x in 0..16 {
                    input[y * stride + x] = match pattern {
                        0 => 0.0,
                        1 => 1.0,
                        2 => {
                            if x == 0 && y == 0 {
                                13.0
                            } else {
                                0.0
                            }
                        }
                        _ => ((x * 37 + y * 97 + x * y * 3) % 257) as f32 - 128.0,
                    };
                }
            }
            for inverse in [false, true] {
                for n in [8, 16] {
                    let expected = reference(&input, stride, n, inverse);
                    for tier in [Tier::Avx2, Tier::Avx512, Tier::Neon, Tier::Scalar]
                        .into_iter()
                        .filter(|t| t.supported())
                    {
                        let mut actual = [0.0; 256];
                        match (inverse, n) {
                            (true, 16) => tier.run::<true, 16>(&input, stride, &mut actual),
                            (true, _) => tier.run::<true, 8>(&input, stride, &mut actual),
                            (false, 16) => tier.run::<false, 16>(&input, stride, &mut actual),
                            _ => tier.run::<false, 8>(&input, stride, &mut actual),
                        }
                        for (i, (a, b)) in actual.iter().zip(&expected).enumerate() {
                            if inverse {
                                assert_eq!(a, b, "inverse {tier:?} n={n} sample={i}");
                            } else {
                                assert!(
                                    (a - b).abs() <= 0.005,
                                    "forward {tier:?} n={n} sample={i}: {a} vs {b}"
                                );
                            }
                        }
                        let mut batch = [[0.0; 256]];
                        call_native(tier, inverse, n, 16, &[input.clone()], stride, &mut batch);
                        for (a, b) in batch[0].iter().zip(&expected) {
                            if inverse {
                                assert_eq!(a, b, "native inverse {tier:?} n={n}");
                            } else {
                                assert!(
                                    (a - b).abs() <= 0.005,
                                    "native forward {tier:?} n={n}: {a} vs {b}"
                                );
                            }
                        }
                        let mut batch = [[0.0; 256]];
                        call_hw(tier, inverse, n, 16, &[input.clone()], stride, &mut batch);
                        for (a, b) in batch[0].iter().zip(&expected) {
                            if inverse {
                                assert_eq!(a, b, "hardware inverse {tier:?} n={n}");
                            } else {
                                assert!(
                                    (a - b).abs() <= 0.005,
                                    "hardware forward {tier:?} n={n}: {a} vs {b}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    if let Some(path) = std::env::var_os("IMI_TRANSFORM16_OUTPUT") {
        measure(Path::new(&path));
    }
    if let Ok(spec) = std::env::var("IMI_TRANSFORM16_PROFILE") {
        assert!(std::env::var_os("IMI_TRANSFORM16_OUTPUT").is_none());
        profile(&spec);
    }
}

// Dense inputs are identical spatial samples for both transform sizes.
// Sparse inverse inputs have equal nonzero counts (16 LF coefficients or
// one DC per independent transform), placed in each transform's own corner.
fn fixture(n: usize, support: usize, batch: usize) -> Vec<Vec<f32>> {
    let mut input = vec![vec![0.0; 23 * 15 + 16]; batch];
    for (b, tile) in input.iter_mut().enumerate() {
        for by in 0..16 / n {
            for bx in 0..16 / n {
                let nz = if support == 16 {
                    n
                } else if support == 4 {
                    n / 4
                } else {
                    1
                };
                for y in 0..nz {
                    for x in 0..nz {
                        let (gx, gy) = if support == 16 {
                            (bx * n + x, by * n + y)
                        } else {
                            (x, y)
                        };
                        tile[(by * n + y) * 23 + bx * n + x] =
                            ((gx * 37 + gy * 97 + b * 11) % 257) as f32 - 128.0;
                    }
                }
            }
        }
    }
    input
}

fn measure(path: &Path) {
    std::fs::create_dir(path).unwrap();
    let source = std::env::var("IMI_TRANSFORM16_SOURCE").expect("explicit frozen source identity");
    let binary = std::fs::read(std::env::current_exe().unwrap()).unwrap();
    let digest = crate::HashAlgorithm::Xxh3_128.digest(&binary);
    let binary_id = format!(
        "{:032x}",
        u128::from_le_bytes(digest.as_bytes().try_into().unwrap())
    );
    let filter = std::env::var("IMI_TRANSFORM16_GROUP").ok();
    std::fs::write(path.join("identity.json"),serde_json::to_vec_pretty(&serde_json::json!({
        "source_revision":source,"binary_xxh3_128":binary_id,"architecture":std::env::consts::ARCH,
        "workload":"transform16-identical-raster-stride23-v2","group_filter":filter,
        "inverse_contract":"existing prescaled IMI inverse, exact scalar output",
        "packing_and_stores_timed":true,"batch_sizes":[1,64],"maps":false,"nas_writes":0
    })).unwrap()).unwrap();
    let gate = zenbench::GateConfig::strict()
        .max_heavy_processes(4)
        .min_available_ram_mb(4096)
        .max_cpu_temp_c(Some(85.0))
        .max_wait(Duration::from_secs(30));
    let result = zenbench::run_gated(gate, |suite| {
        suite.plan().workload_id = Some("transform16-identical-raster-stride23-v2".into());
        for tier in [Tier::Avx2, Tier::Avx512, Tier::Neon]
            .into_iter()
            .filter(|t| t.supported())
        {
            for inverse in [false, true] {
                for batch in [1usize, 64] {
                    for support in if inverse {
                        &[16usize, 4, 1][..]
                    } else {
                        &[16usize][..]
                    } {
                        let group_name = format!(
                            "{tier:?}-{}-support{support}-batch{batch}",
                            if inverse { "inverse" } else { "forward" }
                        );
                        if filter.as_ref().is_some_and(|f| f != &group_name) {
                            continue;
                        }
                        suite.group(group_name, |g| {
                            g.config()
                                .min_rounds(24)
                                .max_rounds(24)
                                .auto_rounds(false)
                                .warmup_time(Duration::from_millis(200))
                                .max_wall_time(Duration::from_secs(90));
                            g.throughput(zenbench::Throughput::Elements((256 * batch) as u64));
                            for (arm, n) in [8usize, 16, 8, 16, 8, 16, 8].into_iter().enumerate() {
                                if matches!(tier, Tier::Avx2) && (arm == 4 || arm == 5) {
                                    continue;
                                }
                                let wide = arm == 2 || arm == 3;
                                let hw = arm == 4 || arm == 5;
                                let name = [
                                    "four8-tile",
                                    "one16-tile",
                                    "four8-native",
                                    "one16-native",
                                    "four8-hw",
                                    "one16-hw",
                                    "aa8-tile",
                                ][arm];
                                let input = fixture(n, *support, batch);
                                let mut outputs = vec![[0.0; 256]; batch];
                                // Sparse optimizations must retain the full scalar oracle.
                                if hw {
                                    call_hw(tier, inverse, n, *support, &input, 23, &mut outputs);
                                }
                                if wide {
                                    call_native(
                                        tier,
                                        inverse,
                                        n,
                                        *support,
                                        &input,
                                        23,
                                        &mut outputs,
                                    );
                                }
                                for (src, dst) in input.iter().zip(&mut outputs) {
                                    if !wide && !hw {
                                        call(tier, inverse, n, *support, src, dst);
                                    }
                                    let expected = reference(src, 23, n, inverse);
                                    for (a, b) in dst.iter().zip(expected) {
                                        if inverse {
                                            assert_eq!(*a, b);
                                        } else {
                                            assert!((*a - b).abs() <= 0.005);
                                        }
                                    }
                                }
                                let support = *support;
                                g.bench(name, move |b| {
                                    b.iter(|| {
                                        if hw {
                                            call_hw(
                                                tier,
                                                inverse,
                                                n,
                                                support,
                                                black_box(&input),
                                                23,
                                                &mut outputs,
                                            );
                                        } else if wide {
                                            call_native(
                                                tier,
                                                inverse,
                                                n,
                                                support,
                                                black_box(&input),
                                                23,
                                                &mut outputs,
                                            );
                                        } else {
                                            call_tile_batch(
                                                tier,
                                                inverse,
                                                n,
                                                support,
                                                black_box(&input),
                                                23,
                                                &mut outputs,
                                            );
                                        }
                                        black_box(&outputs);
                                    })
                                });
                            }
                        });
                    }
                }
            }
        }
    });
    result.save(path.join("timing.json")).unwrap();
    let result: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path.join("timing.json")).unwrap()).unwrap();
    assert_eq!(result["incomplete"], false);
    assert_eq!(result["unreliable"], false);
    for group in result["comparisons"].as_array().unwrap() {
        assert_eq!(group["completed_rounds"], 24);
        assert!(
            group["samples"]
                .as_array()
                .unwrap()
                .iter()
                .all(|s| s["gate_clean"] == true)
        );
    }
    std::fs::write(
        path.join("COMPLETE"),
        b"all oracles and paired round admission passed\n",
    )
    .unwrap();
}

/// A fixed-work profiling entry, never a wall-clock measurement substitute.
fn profile(spec: &str) {
    let fields: Vec<_> = spec.split(':').collect();
    assert_eq!(
        fields.len(),
        5,
        "ISA:forward|inverse:8|16:tile|native|hw:16|4|1"
    );
    let tier = match fields[0] {
        "Avx2" => Tier::Avx2,
        "Avx512" => Tier::Avx512,
        "Neon" => Tier::Neon,
        _ => panic!("explicit ISA"),
    };
    assert!(tier.supported());
    let inverse = match fields[1] {
        "forward" => false,
        "inverse" => true,
        _ => panic!("operation"),
    };
    let n = fields[2].parse::<usize>().unwrap();
    assert!([8, 16].contains(&n));
    let layout = match fields[3] {
        "tile" => 0,
        "native" => 1,
        "hw" => 2,
        _ => panic!("layout"),
    };
    let support = fields[4].parse::<usize>().unwrap();
    assert!([1, 4, 16].contains(&support));
    let input = fixture(n, support, 64);
    let mut outputs = vec![[0.0; 256]; 64];
    let iterations = std::env::var("IMI_TRANSFORM16_PROFILE_ITERATIONS")
        .map_or(100000, |v| v.parse::<usize>().unwrap());
    for _ in 0..iterations {
        if layout == 2 {
            call_hw(
                tier,
                inverse,
                n,
                support,
                black_box(&input),
                23,
                &mut outputs,
            );
        } else if layout == 1 {
            call_native(
                tier,
                inverse,
                n,
                support,
                black_box(&input),
                23,
                &mut outputs,
            );
        } else {
            call_tile_batch(
                tier,
                inverse,
                n,
                support,
                black_box(&input),
                23,
                &mut outputs,
            );
        }
        black_box(&outputs);
    }
}

fn call(
    tier: Tier,
    inverse: bool,
    n: usize,
    support: usize,
    input: &[f32],
    output: &mut [f32; 256],
) {
    match (inverse, n, support) {
        (false, 16, _) => tier.run::<false, 16>(input, 23, output),
        (false, _, _) => tier.run::<false, 8>(input, 23, output),
        (true, 16, 1) => tier.run_sparse::<true, 16, 1>(input, 23, output),
        (true, 8, 1) => tier.run_sparse::<true, 8, 1>(input, 23, output),
        (true, 16, 4) => tier.run_sparse::<true, 16, 4>(input, 23, output),
        (true, 8, 4) => tier.run_sparse::<true, 8, 4>(input, 23, output),
        (true, 16, _) => tier.run::<true, 16>(input, 23, output),
        _ => tier.run::<true, 8>(input, 23, output),
    }
}

fn call_native(
    tier: Tier,
    inverse: bool,
    n: usize,
    support: usize,
    input: &[Vec<f32>],
    stride: usize,
    output: &mut [[f32; 256]],
) {
    match (inverse, n, support) {
        (false, 16, _) => native::run::<false, 16, 16>(tier, input, stride, output),
        (false, _, _) => native::run::<false, 8, 16>(tier, input, stride, output),
        (true, 16, 1) => native::run::<true, 16, 1>(tier, input, stride, output),
        (true, 8, 1) => native::run::<true, 8, 1>(tier, input, stride, output),
        (true, 16, 4) => native::run::<true, 16, 4>(tier, input, stride, output),
        (true, 8, 4) => native::run::<true, 8, 4>(tier, input, stride, output),
        (true, 16, _) => native::run::<true, 16, 16>(tier, input, stride, output),
        _ => native::run::<true, 8, 16>(tier, input, stride, output),
    }
}

fn call_tile_batch(
    tier: Tier,
    inverse: bool,
    n: usize,
    support: usize,
    input: &[Vec<f32>],
    stride: usize,
    output: &mut [[f32; 256]],
) {
    match (inverse, n, support) {
        (false, 16, _) => tier.run_batch::<false, 16, 16>(input, stride, output),
        (false, _, _) => tier.run_batch::<false, 8, 16>(input, stride, output),
        (true, 16, 1) => tier.run_batch::<true, 16, 1>(input, stride, output),
        (true, 8, 1) => tier.run_batch::<true, 8, 1>(input, stride, output),
        (true, 16, 4) => tier.run_batch::<true, 16, 4>(input, stride, output),
        (true, 8, 4) => tier.run_batch::<true, 8, 4>(input, stride, output),
        (true, 16, _) => tier.run_batch::<true, 16, 16>(input, stride, output),
        _ => tier.run_batch::<true, 8, 16>(input, stride, output),
    }
}

fn call_hw(
    tier: Tier,
    inverse: bool,
    n: usize,
    support: usize,
    input: &[Vec<f32>],
    stride: usize,
    output: &mut [[f32; 256]],
) {
    match (inverse, n, support) {
        (false, 16, _) => native::run_hw::<false, 16, 16>(tier, input, stride, output),
        (false, _, _) => native::run_hw::<false, 8, 16>(tier, input, stride, output),
        (true, 16, 1) => native::run_hw::<true, 16, 1>(tier, input, stride, output),
        (true, 8, 1) => native::run_hw::<true, 8, 1>(tier, input, stride, output),
        (true, 16, 4) => native::run_hw::<true, 16, 4>(tier, input, stride, output),
        (true, 8, 4) => native::run_hw::<true, 8, 4>(tier, input, stride, output),
        (true, 16, _) => native::run_hw::<true, 16, 16>(tier, input, stride, output),
        _ => native::run_hw::<true, 8, 16>(tier, input, stride, output),
    }
}
