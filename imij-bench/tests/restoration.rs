// Explicit coordinates make halo and padding coverage reviewable.
#![allow(clippy::needless_range_loop)]
use restoration::historical as original;
#[path = "../src/restoration.rs"]
#[allow(dead_code)]
mod restoration;
use restoration::{FloatFilter, Kernel, Layout, Strength};

#[test]
fn seam_matches_original_across_phases_tails_alpha_and_schedules() {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap();
    for channels in [3, 4] {
        for width in [1, 2, 7, 8, 9, 15, 16, 17, 31, 32, 65, 81] {
            let height = 19;
            let stride = width * channels + 5;
            for oy in 0..8 {
                for ox in 0..8 {
                    let l = Layout {
                        width,
                        height,
                        stride_bytes: stride,
                        channels,
                        origin: (ox, oy),
                    };
                    for pattern in 0..3 {
                        let mut pixels = vec![0xa5; stride * height + 13];
                        for y in 0..height {
                            for x in 0..width {
                                for c in 0..channels {
                                    pixels[y * stride + x * channels + c] = if c == 3 {
                                        if pattern == 2 && x % 11 == 0 { 63 } else { 255 }
                                    } else {
                                        match pattern {
                                            0 => ((x * 17 + y * 31 + c * 53) % 256) as u8,
                                            1 => {
                                                (110 + ((x + ox) / 8) % 2 * 10
                                                    + ((y + oy) / 8) % 2 * 6
                                                    + c * 2)
                                                    as u8
                                            }
                                            _ => {
                                                if (x + ox) / 8 % 2 == 0 {
                                                    2
                                                } else {
                                                    14
                                                }
                                            }
                                        }
                                    };
                                }
                            }
                        }
                        for (s, old) in [
                            (Strength::Off, original::Strength::Off),
                            (Strength::Mild, original::Strength::Mild),
                            (Strength::Medium, original::Strength::Medium),
                            (Strength::Strong, original::Strength::Strong),
                        ] {
                            let mut expected = pixels.clone();
                            original::apply(
                                &mut expected,
                                original::Layout {
                                    width,
                                    height,
                                    stride_bytes: stride,
                                    channels,
                                    origin: (ox, oy),
                                },
                                old,
                            )
                            .unwrap();
                            for kernel in Kernel::available() {
                                for parallel in [false, true] {
                                    let mut actual = pixels.clone();
                                    pool.install(|| {
                                        restoration::seam_filter(
                                            &mut actual,
                                            l,
                                            s,
                                            kernel,
                                            parallel,
                                        )
                                    })
                                    .unwrap();
                                    assert_eq!(
                                        actual, expected,
                                        "{channels}ch {width}w {ox},{oy} pattern{pattern} {s:?} {kernel:?} parallel{parallel}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn float_filter_scalar_simd_streaming_parity_and_constant_response() {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap();
    for width in 1..=35 {
        for height in [1, 2, 3, 7, 13] {
            let stride = width + 3;
            for filter in [FloatFilter::Gaborish, FloatFilter::Inverse] {
                for pattern in 0..3 {
                    let mut source = vec![-777.0f32; stride * height];
                    for y in 0..height {
                        for x in 0..width {
                            source[y * stride + x] = match pattern {
                                0 => 0.375,
                                1 => {
                                    if x == width / 2 && y == height / 2 {
                                        1.0
                                    } else {
                                        0.0
                                    }
                                }
                                _ => ((x * 17 + y * 31) % 127) as f32 / 41.0 - 1.0,
                            };
                        }
                    }
                    let mut expected = vec![-999.0; source.len()];
                    restoration::plane(
                        &source,
                        &mut expected,
                        width,
                        height,
                        stride,
                        filter,
                        Kernel::Scalar,
                        false,
                    )
                    .unwrap();
                    for kernel in Kernel::float_available() {
                        for parallel in [false, true] {
                            let mut actual = vec![-999.0; source.len()];
                            pool.install(|| {
                                restoration::plane(
                                    &source,
                                    &mut actual,
                                    width,
                                    height,
                                    stride,
                                    filter,
                                    kernel,
                                    parallel,
                                )
                            })
                            .unwrap();
                            assert_eq!(
                                actual.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
                                expected.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
                                "{width}x{height} {filter:?} {kernel:?}"
                            );
                        }
                    }
                    let r = filter.radius();
                    for y in 0..height {
                        let mut rows = vec![vec![0.0; width + 2 * r]; 2 * r + 1];
                        for dy in 0..2 * r + 1 {
                            for x in 0..width + 2 * r {
                                let xx = restoration::mirror(x as isize - r as isize, width);
                                let yy = restoration::mirror(
                                    y as isize + dy as isize - r as isize,
                                    height,
                                );
                                rows[dy][x] = source[yy * stride + xx];
                            }
                        }
                        let mut out = vec![0.0; width];
                        match filter {
                            FloatFilter::Gaborish => restoration::gaborish_row(
                                [&rows[0], &rows[1], &rows[2]],
                                &mut out,
                                Kernel::Scalar,
                            )
                            .unwrap(),
                            FloatFilter::Inverse => restoration::inverse_row(
                                [&rows[0], &rows[1], &rows[2], &rows[3], &rows[4]],
                                &mut out,
                                Kernel::Scalar,
                            )
                            .unwrap(),
                        }
                        assert_eq!(out, &expected[y * stride..y * stride + width]);
                        if pattern == 0 {
                            assert!(out.iter().all(|x| (*x - 0.375).abs() < 2e-7));
                        }
                        assert!(
                            expected[y * stride + width..(y + 1) * stride]
                                .iter()
                                .all(|&v| v == -999.0)
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn gaborish_impulse_matches_independent_weight_matrix() {
    let n = 9;
    let mut source = vec![0.0; n * n];
    source[4 * n + 4] = 1.0;
    let mut out = vec![0.0; n * n];
    restoration::plane(
        &source,
        &mut out,
        n,
        n,
        n,
        FloatFilter::Gaborish,
        Kernel::Scalar,
        false,
    )
    .unwrap();
    let axial = 0.115_169_525f64;
    let diagonal = 0.061_248_592f64;
    let total = 1.0 + 4.0 * (axial + diagonal);
    for y in 0usize..n {
        for x in 0usize..n {
            let dx = x.abs_diff(4);
            let dy = y.abs_diff(4);
            let expected = match (dx, dy) {
                (0, 0) => 1.0 / total,
                (1, 0) | (0, 1) => axial / total,
                (1, 1) => diagonal / total,
                _ => 0.0,
            };
            assert!((f64::from(out[y * n + x]) - expected).abs() < 1e-7);
        }
    }
}

#[test]
fn rejects_short_halos_and_invalid_geometry_before_mutation() {
    let mut out = [19.0; 9];
    assert!(restoration::gaborish_row([&[0.0; 10]; 3], &mut out, Kernel::Scalar).is_err());
    assert_eq!(out, [19.0; 9]);
    assert!(restoration::inverse_row([&[0.0; 12]; 5], &mut out, Kernel::Scalar).is_err());
    assert!(
        restoration::plane(
            &[0.0; 9],
            &mut out,
            usize::MAX,
            2,
            usize::MAX,
            FloatFilter::Gaborish,
            Kernel::Scalar,
            false
        )
        .is_err()
    );
    let mut rgb = [3; 12];
    let bad = Layout {
        width: 8,
        height: 8,
        stride_bytes: 1,
        channels: 3,
        origin: (0, 0),
    };
    assert!(
        restoration::seam_filter(&mut rgb, bad, Strength::Medium, Kernel::Scalar, false).is_err()
    );
    assert_eq!(rgb, [3; 12]);
}

#[test]
fn seam_gates_rounding_signs_and_endpoints_match_oracle() {
    // Repeated identical neighborhoods force vector bulk and tail in both
    // orientations. Values near endpoints exercise saturation/rejection.
    for base in [0i32, 1, 8, 16, 64, 127, 223, 239, 247, 254, 255] {
        for jump in [
            -33i32, -32, -25, -24, -17, -16, -7, -1, 0, 1, 7, 16, 17, 24, 25, 32, 33,
        ] {
            for slope in [-17i32, -16, -9, -8, 0, 8, 9, 16, 17] {
                let quad = [base - slope, base, base + jump, base + jump + slope]
                    .map(|v| v.clamp(0, 255) as u8);
                for vertical in [false, true] {
                    let (width, height, origin) = if vertical {
                        (130, 1, (0, 0))
                    } else {
                        (19, 4, (0, 6))
                    };
                    let channels = 3;
                    let stride = width * channels;
                    let mut input = vec![0; stride * height];
                    if vertical {
                        for seam in (8..width - 1).step_by(8) {
                            for (i, &v) in quad.iter().enumerate() {
                                input[(seam + i - 2) * 3..(seam + i - 1) * 3].fill(v);
                            }
                        }
                    } else {
                        for (y, &v) in quad.iter().enumerate() {
                            input[y * stride..(y + 1) * stride].fill(v);
                        }
                    }
                    for (s, old) in [
                        (Strength::Mild, original::Strength::Mild),
                        (Strength::Medium, original::Strength::Medium),
                        (Strength::Strong, original::Strength::Strong),
                    ] {
                        let mut expected = input.clone();
                        original::apply(
                            &mut expected,
                            original::Layout {
                                width,
                                height,
                                stride_bytes: stride,
                                channels,
                                origin,
                            },
                            old,
                        )
                        .unwrap();
                        for kernel in Kernel::available() {
                            let mut actual = input.clone();
                            restoration::seam_filter(
                                &mut actual,
                                Layout {
                                    width,
                                    height,
                                    stride_bytes: stride,
                                    channels,
                                    origin,
                                },
                                s,
                                kernel,
                                false,
                            )
                            .unwrap();
                            assert_eq!(
                                actual, expected,
                                "base{base} jump{jump} slope{slope} vertical{vertical} {s:?} {kernel:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}
