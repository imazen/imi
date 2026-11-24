#![forbid(unsafe_code)]
#[path = "../src/noise.rs"]
mod noise;

use noise::{Kernel, apply_kernel, apply_parallel_kernel};
use noise::{Layout, Parameters, Variant, apply, apply_parallel};

fn layout(w: usize, h: usize, channels: usize, origin: (usize, usize)) -> Layout {
    Layout {
        width: w,
        height: h,
        channels,
        stride_bytes: w * channels + 5,
        origin,
    }
}

fn source(l: Layout) -> Vec<u8> {
    let mut out = vec![199; l.stride_bytes * l.height + 7];
    for y in 0..l.height {
        for x in 0..l.width {
            let i = y * l.stride_bytes + x * l.channels;
            out[i..i + 3].copy_from_slice(&[
                ((x * 13 + y * 19) % 256) as u8,
                if (x + y) % 2 == 0 { 0 } else { 255 },
                ((x * 29 + y * 7) % 256) as u8,
            ]);
            if l.channels == 4 {
                out[i + 3] = ((x * 17 + y * 23) % 256) as u8;
            }
        }
    }
    out
}

// Independent scalar oracle: explicit indexing, i64 arithmetic and division
// rather than the production loop's specialized dispatch/bit shifts.
fn reference(input: &[u8], l: Layout, seed: u32, low: u8, high: u8, v: Variant) -> Vec<u8> {
    fn hash(x: usize, y: usize, seed: u32) -> u32 {
        let mut n =
            (x as u32).wrapping_mul(0x9e3779b1) ^ (y as u32).wrapping_mul(0x85ebca77) ^ seed;
        n = (n ^ (n >> 16)).wrapping_mul(0x7feb352d);
        n = (n ^ (n >> 15)).wrapping_mul(0x846ca68b);
        n ^ (n >> 16)
    }
    let mut out = input.to_vec();
    for y in 0..l.height {
        for x in 0..l.width {
            let i = y * l.stride_bytes + x * l.channels;
            let rgb: Vec<i64> = input[i..i + 3].iter().map(|&n| i64::from(n)).collect();
            let intensity = (77 * rgb[0] + 150 * rgb[1] + 29 * rgb[2] + 128) / 256;
            let amplitude = i64::from(low)
                + (i64::from(high - low) * intensity.min(255 - intensity) + 63) / 127;
            let bits = hash(l.origin.0 + x, l.origin.1 + y, seed);
            let signed = |n: u32| 2 * i64::from(n % 128) - 127;
            let common = signed(bits);
            for c in 0..3 {
                let n = match v {
                    Variant::Shared => common,
                    Variant::Rgb => (2 * common + signed(bits / 2u32.pow(7 * (c as u32 + 1)))) / 3,
                    Variant::Template => {
                        (common
                            + signed(hash(
                                (l.origin.0 + x) / 2,
                                (l.origin.1 + y) / 2,
                                seed ^ 0xa511e9b3,
                            )))
                            / 2
                    }
                };
                let d = n.signum() * ((n.abs() * amplitude + 64) / 128);
                out[i + c] = (rgb[c] + d).clamp(0, 255) as u8;
            }
        }
    }
    out
}

#[test]
fn scalar_reference_extremes_patterns_and_bounded_parameters() {
    for channels in [3, 4] {
        let l = layout(33, 27, channels, (81, 97));
        let mut inputs = vec![source(l)];
        for value in [0, 1, 127, 128, 254, 255] {
            inputs.push(vec![value; l.stride_bytes * l.height + 7]);
        }
        for input in inputs {
            for (low, high) in [(0, 0), (0, 32), (4, 12), (32, 32)] {
                for seed in [0, 1, u32::MAX, 0x6d2b79f5] {
                    let p = Parameters::new(seed, low, high).unwrap();
                    for v in Variant::ALL {
                        let mut actual = input.clone();
                        apply(&mut actual, l, &p, v).unwrap();
                        assert_eq!(actual, reference(&input, l, seed, low, high, v));
                        for y in 0..l.height {
                            for x in 0..l.width {
                                let i = y * l.stride_bytes + x * channels;
                                assert!(
                                    (0..3).all(|c| actual[i + c].abs_diff(input[i + c]) <= high)
                                );
                                if channels == 4 {
                                    assert_eq!(actual[i + 3], input[i + 3]);
                                }
                            }
                            assert_eq!(
                                &actual[y * l.stride_bytes + l.width * channels
                                    ..(y + 1) * l.stride_bytes],
                                &input[y * l.stride_bytes + l.width * channels
                                    ..(y + 1) * l.stride_bytes]
                            );
                        }
                        assert_eq!(
                            &actual[l.height * l.stride_bytes..],
                            &input[l.height * l.stride_bytes..]
                        );
                    }
                }
            }
        }
    }
    assert!(Parameters::new(0, 12, 4).is_err());
    assert!(Parameters::new(0, 0, 33).is_err());
}

#[test]
fn every_origin_crop_and_parallel_stripe_schedule_match() {
    let pools: Vec<_> = [1, 4]
        .into_iter()
        .map(|n| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .build()
                .unwrap()
        })
        .collect();
    for channels in [3, 4] {
        for ox in 0..16 {
            for oy in 0..16 {
                let l = layout(19, 13, channels, (ox, oy));
                let input = source(l);
                for v in Variant::ALL {
                    let p = Parameters::fixed();
                    let mut whole = input.clone();
                    apply(&mut whole, l, &p, v).unwrap();
                    for kernel in Kernel::available() {
                        let mut tier = input.clone();
                        apply_kernel(&mut tier, l, &p, v, kernel).unwrap();
                        assert_eq!(tier, whole);
                        for pool in &pools {
                            for stripe_rows in [1, 3, 8, 64, 4096] {
                                let mut actual = input.clone();
                                pool.install(|| {
                                    apply_parallel_kernel(
                                        &mut actual,
                                        l,
                                        &p,
                                        v,
                                        stripe_rows,
                                        kernel,
                                    )
                                })
                                .unwrap();
                                assert_eq!(actual, whole);
                            }
                        }
                    }
                    // All possible nonempty top-left crop positions, odd stride
                    // and no last-row padding: independent output needs no halo.
                    for cy in 0..l.height {
                        for cx in 0..l.width {
                            let crop = Layout {
                                width: l.width - cx,
                                height: l.height - cy,
                                origin: (ox + cx, oy + cy),
                                ..l
                            };
                            let start = cy * l.stride_bytes + cx * channels;
                            let end = start
                                + (crop.height - 1) * crop.stride_bytes
                                + crop.width * channels;
                            let mut tile = input[start..end].to_vec();
                            apply(&mut tile, crop, &p, v).unwrap();
                            for y in 0..crop.height {
                                assert_eq!(
                                    &tile[y * crop.stride_bytes
                                        ..y * crop.stride_bytes + crop.width * channels],
                                    &whole[start + y * l.stride_bytes
                                        ..start + y * l.stride_bytes + crop.width * channels]
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn tiny_geometry_seed_determinism_and_hostile_inputs() {
    let p = Parameters::fixed();
    for w in 0..9 {
        for h in 0..9 {
            for channels in [3, 4] {
                let l = layout(
                    w,
                    h,
                    channels,
                    (u32::MAX as usize - w, u32::MAX as usize - h),
                );
                let input = source(l);
                for v in Variant::ALL {
                    let mut a = input.clone();
                    let mut b = input.clone();
                    apply(&mut a, l, &p, v).unwrap();
                    apply_parallel(&mut b, l, &p, v, 8).unwrap();
                    assert_eq!(a, b);
                }
            }
        }
    }
    let l = layout(33, 27, 3, (0, 0));
    let input = vec![128; l.stride_bytes * l.height];
    let mut a = input.clone();
    let mut b = input.clone();
    apply(
        &mut a,
        l,
        &Parameters::new(1, 4, 12).unwrap(),
        Variant::Shared,
    )
    .unwrap();
    apply(
        &mut b,
        l,
        &Parameters::new(2, 4, 12).unwrap(),
        Variant::Shared,
    )
    .unwrap();
    assert_ne!(a, b);
    assert_ne!(a, input);
    for bad in [
        Layout { channels: 2, ..l },
        Layout {
            stride_bytes: 0,
            ..l
        },
        Layout {
            width: usize::MAX,
            ..l
        },
        Layout {
            height: usize::MAX,
            ..l
        },
        Layout {
            origin: (u32::MAX as usize, 0),
            ..l
        },
        Layout {
            origin: (usize::MAX, 0),
            ..l
        },
    ] {
        assert!(apply(&mut [], bad, &p, Variant::Shared).is_err());
    }
    assert!(apply(&mut [0; 5], layout(2, 1, 3, (0, 0)), &p, Variant::Shared).is_err());
    for rows in [0, 4097, usize::MAX] {
        assert!(
            apply_parallel(&mut [], layout(0, 0, 3, (0, 0)), &p, Variant::Shared, rows).is_err()
        );
    }
    for v in Variant::ALL {
        assert_eq!(Variant::parse(v.name()), Ok(v));
    }
    assert!(Variant::parse("unknown").is_err());
}

#[test]
fn explicit_simd_tier_matches_oracle_all_intensities_tails_and_schedules() {
    let kernels = Kernel::available();
    eprintln!(
        "checked noise tiers: {:?}",
        kernels.iter().map(|k| k.name()).collect::<Vec<_>>()
    );
    let pools: Vec<_> = [1, 4]
        .into_iter()
        .map(|n| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .build()
                .unwrap()
        })
        .collect();
    for channels in [3, 4] {
        for width in [
            0usize, 1, 2, 3, 4, 5, 6, 7, 8, 9, 15, 16, 17, 23, 24, 25, 31, 32, 33,
        ] {
            for origin in [
                (0, 0),
                (1, 1),
                (7, 9),
                (17, 31),
                (
                    u32::MAX as usize - width.saturating_sub(1),
                    u32::MAX as usize - 255,
                ),
            ] {
                let l = layout(width, 256, channels, origin);
                let mut input = source(l);
                for y in 0..256 {
                    for x in 0..width {
                        let i = y * l.stride_bytes + x * channels;
                        // First pixels cover every luma index exactly; others
                        // cover chromatic and clipping extremes at every level.
                        let rgb = match x % 4 {
                            0 => [y as u8; 3],
                            1 => [0, y as u8, 255],
                            2 => [255, 0, y as u8],
                            _ => [y as u8, 255, 0],
                        };
                        input[i..i + 3].copy_from_slice(&rgb);
                    }
                }
                for seed in [0, 1, u32::MAX, 0x6d2b79f5] {
                    for (low, high) in [(0, 0), (4, 12), (0, 32), (32, 32)] {
                        let params = Parameters::new(seed, low, high).unwrap();
                        for v in Variant::ALL {
                            let expected = reference(&input, l, seed, low, high, v);
                            for &kernel in &kernels {
                                let mut actual = input.clone();
                                apply_kernel(&mut actual, l, &params, v, kernel).unwrap();
                                assert_eq!(
                                    actual,
                                    expected,
                                    "tier={} width={width} origin={origin:?} variant={v:?}",
                                    kernel.name()
                                );
                                // All tail lengths and crop offsets are also
                                // exercised by applying into an offset slice.
                                for offset in 0..width.min(8) {
                                    let crop = Layout {
                                        width: width - offset,
                                        origin: (origin.0 + offset, origin.1),
                                        ..l
                                    };
                                    let end = (l.height - 1) * l.stride_bytes + width * channels;
                                    let start = offset * channels;
                                    let mut tile = input[start..end].to_vec();
                                    apply_kernel(&mut tile, crop, &params, v, kernel).unwrap();
                                    for y in 0..l.height {
                                        assert_eq!(
                                            &tile[y * l.stride_bytes
                                                ..y * l.stride_bytes + crop.width * channels],
                                            &expected[start + y * l.stride_bytes
                                                ..start
                                                    + y * l.stride_bytes
                                                    + crop.width * channels]
                                        );
                                    }
                                }
                                // Scheduling grid is limited to fixed params;
                                // parameter extremes above still cover all math.
                                if seed == 0x6d2b79f5 && low == 4 {
                                    for pool in &pools {
                                        for stripes in [1, 3, 8, 64, 4096] {
                                            let mut actual = input.clone();
                                            pool.install(|| {
                                                apply_parallel_kernel(
                                                    &mut actual,
                                                    l,
                                                    &params,
                                                    v,
                                                    stripes,
                                                    kernel,
                                                )
                                            })
                                            .unwrap();
                                            assert_eq!(actual, expected);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
