#[path = "../src/chroma4.rs"]
mod chroma4;
use chroma4::*;
use imij::{LossyColor, chroma_lab};

fn fixture(w: usize, h: usize, stride: usize, ox: usize, oy: usize) -> Vec<u8> {
    let mut p = vec![177; stride * h];
    for y in 0..h {
        for x in 0..w {
            let (x, y) = (x + ox, y + oy);
            let i = (y - oy) * stride + (x - ox) * 3;
            p[i] = (x * 73 + y * 31) as u8;
            p[i + 1] = (x * 17 + y * 99) as u8;
            p[i + 2] = (x * 43 + y * 57) as u8;
        }
    }
    p
}
#[test]
fn transform_matches_independent_float_definition_dc_and_roundtrip() {
    fn verify<const N: usize>() {
        let dct = Dct::<N>::new();
        for case in 0..N * N + 3 {
            let x: [[f32; N]; N] = std::array::from_fn(|y| {
                std::array::from_fn(|x| {
                    if case < N * N {
                        if y * N + x == case { 255.0 } else { 0.0 }
                    } else if case == N * N {
                        37.25
                    } else {
                        ((y * 149 + x * 83 + case * 51) % 511) as f32 - 255.0
                    }
                })
            });
            let f = dct.forward(&x);
            for (v, row) in f.iter().enumerate() {
                for (u, &value) in row.iter().enumerate() {
                    let mut reference = 0.0f64;
                    for (y, row) in x.iter().enumerate() {
                        for (i, &p) in row.iter().enumerate() {
                            let c = |k, n| {
                                let a = if k == 0 {
                                    1.0 / (N as f64).sqrt()
                                } else {
                                    (2.0 / N as f64).sqrt()
                                };
                                a * (std::f64::consts::PI * k as f64 * (2 * n + 1) as f64
                                    / (2 * N) as f64)
                                    .cos()
                            };
                            reference += p as f64 * c(v, y) * c(u, i);
                        }
                    }
                    assert!(
                        (value as f64 - reference).abs() < 0.0004,
                        "N{N} case{case} u{u} v{v}"
                    );
                }
            }
            let recon = dct.inverse(&f);
            for y in 0..N {
                for i in 0..N {
                    assert!((recon[y][i] - x[y][i]).abs() < 0.0004);
                }
            }
            if case == N * N {
                assert!((f[0][0] - 37.25 * N as f32).abs() < 0.0001);
            }
        }
    }
    verify::<4>();
    verify::<8>();
    for color in [LossyColor::Xyb, LossyColor::YCoCg] {
        for q in [0.0, 1.0, 3.0, 20.0, 80.0, 100.0, 116.0] {
            let (_, s) = chroma_lab::parameters(q, color);
            for sc in &s {
                assert_eq!(
                    quant(4.0 * 37.25, step4(sc, 0, 0)),
                    quant(8.0 * 37.25, sc[0])
                );
                for v in 0..4 {
                    for u in 0..4 {
                        assert_eq!(step4(sc, v, u), sc[v * 8 + u] / 2.0);
                    }
                }
            }
        }
    }
}
#[test]
fn triangle_is_affine_at_every_interior_phase_and_across8px_boundaries() {
    // Independent continuous affine field sampled at centered half positions.
    for y in 2..30 {
        for x in 2..30 {
            let got = triangle(x, y, 16, 16, &|i, j| {
                2.0 * (2 * i) as f32 + 3.0 * (2 * j) as f32 + 2.5
            });
            assert_eq!(got, 2.0 * x as f32 + 3.0 * y as f32);
        }
    }
    assert_eq!(triangle(0, 0, 2, 2, &|_, _| 37.0), 37.0);
}
#[test]
fn exact_production_coefficients_wrapper_bytes_and_rgb() {
    for color in [LossyColor::Xyb, LossyColor::YCoCg] {
        for q in [
            0.0, 1.0, 2.0, 3.0, 5.0, 10.0, 20.0, 40.0, 60.0, 80.0, 100.0, 116.0,
        ] {
            let rgb = fixture(23, 17, 74, 0, 0);
            let p = Prepared::new(&rgb, 74, 23, 17, color).unwrap();
            let production = p.production(&rgb, 74, q).unwrap();
            let control = p.encode(&production, q, Mode::Fixed444).unwrap();
            assert_eq!(
                &control.file[36..],
                production,
                "inner byte parity {color:?} q{q}"
            );
            assert_eq!(
                chroma_lab::unpack(&control.file[36..]).unwrap(),
                chroma_lab::unpack(&production).unwrap()
            );
            assert_eq!(
                decode(&control.file).unwrap(),
                imij::decode(&production).unwrap().1,
                "canonical RGB parity {color:?} q{q}"
            );
            assert_eq!(control.file.len(), production.len() + 36);
            assert_eq!(
                chroma_lab::byte_parts(&production)
                    .unwrap()
                    .iter()
                    .sum::<usize>(),
                production.len()
            );
        }
    }
}
#[test]
fn all_crop_origins_stride_odd_edges_modes_and_serialized_roundtrip() {
    for oy in 0..8 {
        for ox in 0..8 {
            for color in [LossyColor::Xyb, LossyColor::YCoCg] {
                let rgb = fixture(13, 11, 44, ox, oy);
                let tight: Vec<_> = rgb
                    .chunks(44)
                    .flat_map(|r| r[..39].iter().copied())
                    .collect();
                let p = Prepared::new(&rgb, 44, 13, 11, color).unwrap();
                let t = Prepared::new(&tight, 39, 13, 11, color).unwrap();
                assert_eq!(p.source, t.source);
                let production = p.production(&rgb, 44, 40.0).unwrap();
                for mode in [Mode::Fixed444, Mode::Fixed420, Mode::Adaptive] {
                    let encoded = p.encode(&production, 40.0, mode).unwrap();
                    let second = t.encode(&production, 40.0, mode).unwrap();
                    assert_eq!(encoded.file, second.file);
                    let decoded = decode(&encoded.file).unwrap();
                    assert_eq!(decoded.len(), 13 * 11 * 3);
                    assert_eq!(
                        encoded.file.len(),
                        36 + encoded.inner_bytes + encoded.map_bytes
                    );
                    if mode == Mode::Adaptive {
                        assert_eq!(encoded.map_bytes, encoded.blocks.div_ceil(8));
                    }
                    // Last padded source sample replicates actual bottom-right pixel.
                    for c in 0..3 {
                        assert_eq!(p.source[c][p.pw * p.ph - 1], p.source[c][10 * p.pw + 12]);
                    }
                }
            }
        }
    }
}
#[test]
fn malformed_map_and_truncation_are_rejected() {
    let rgb = fixture(13, 11, 39, 0, 0);
    let p = Prepared::new(&rgb, 39, 13, 11, LossyColor::Xyb).unwrap();
    let production = p.production(&rgb, 39, 40.0).unwrap();
    let e = p.encode(&production, 40.0, Mode::Adaptive).unwrap();
    let mut m = e.file.clone();
    m[36] |= 128;
    assert!(decode(&m).is_err());
    for cut in [0, 8, 35, 36, e.file.len() - 1] {
        assert!(decode(&e.file[..cut]).is_err());
    }
}
#[test]
fn mixed_block_ownership_preserves444_and_uses_neighbor_halo() {
    let (pw, ph) = (24, 24);
    let nb = 9;
    let rgb = fixture(pw, ph, pw * 3, 0, 0);
    let p = Prepared::new(&rgb, pw * 3, pw, ph, LossyColor::YCoCg).unwrap();
    let production = p.production(&rgb, pw * 3, 80.0).unwrap();
    let full = chroma_lab::unpack(&production).unwrap();
    let sub = p.encode(&production, 80.0, Mode::Fixed420).unwrap();
    let mut q = full.clone();
    let mut modes = vec![0; nb];
    modes[4] = 1;
    q[4 * 192 + 64..5 * 192].copy_from_slice(&sub.coefficients[4 * 192 + 64..5 * 192]);
    let (_, steps) = chroma_lab::parameters(80.0, LossyColor::YCoCg);
    let mixed = reconstruct_planes(&q, &modes, pw, ph, &steps);
    let baseline = reconstruct_planes(&full, &vec![0; nb], pw, ph, &steps);
    let mut half: [Vec<f32>; 2] = std::array::from_fn(|c| downsample(&baseline[c + 1], pw, ph));
    let dct = Dct::<4>::new();
    for c in 1..3 {
        let block = &q[(4 * 3 + c) * 64..];
        let x = std::array::from_fn(|v| {
            std::array::from_fn(|u| block[v * 8 + u] as f32 * step4(&steps[c], v, u))
        });
        let x = dct.inverse(&x);
        for v in 0..4 {
            for u in 0..4 {
                half[c - 1][(4 + v) * 12 + 4 + u] = x[v][u];
            }
        }
    }
    for y in 0..ph {
        for x in 0..pw {
            for c in 1..3 {
                if x / 8 == 1 && y / 8 == 1 {
                    assert_eq!(
                        mixed[c][y * pw + x],
                        triangle(x, y, 12, 12, &|xx, yy| half[c - 1][yy * 12 + xx])
                    );
                } else {
                    assert_eq!(mixed[c][y * pw + x], baseline[c][y * pw + x]);
                }
            }
        }
    }
    // At the top/left owned corner, both444 neighbor halos affect the pixel.
    let t = triangle(8, 8, 12, 12, &|x, y| half[0][y * 12 + x]);
    assert_ne!(t, half[0][4 * 12 + 4]);
}

#[test]
fn hostile_geometry_rejected_before_buffers_or_nested_file_parse() {
    assert_eq!(
        chroma_lab::checked_geometry(2048, 2048).unwrap().4,
        chroma_lab::MAX_PADDED_PIXELS
    );
    for (w, h) in [
        (0, 1),
        (1, 0),
        (usize::MAX, 1),
        (1, usize::MAX),
        (16384, 16384),
        (2049, 2048),
        (4096, 1025),
        (u32::MAX as usize, u32::MAX as usize),
    ] {
        assert!(chroma_lab::checked_geometry(w, h).is_err());
        assert!(chroma_lab::planes(&[], 0, w, h, LossyColor::Xyb).is_err());
        assert!(Prepared::new(&[], 0, w, h, LossyColor::Xyb).is_err());
    }
    if usize::BITS > 32 {
        let truncated = (u32::MAX as usize).checked_add(2).unwrap();
        assert!(chroma_lab::checked_geometry(truncated, 1).is_err());
        assert!(Prepared::new(&[0, 0, 0], 3, truncated, 1, LossyColor::Xyb).is_err());
    }
    assert!(chroma_lab::pack(&[], 16384, 16384, 40.0, LossyColor::Xyb).is_err());
    let mut outer = b"CHR4P001".to_vec();
    outer.extend_from_slice(&16384u32.to_le_bytes());
    outer.extend_from_slice(&16384u32.to_le_bytes());
    outer.extend_from_slice(&40.0f32.to_le_bytes());
    outer.extend_from_slice(&[0; 16]);
    let error = decode(&outer).unwrap_err();
    assert!(error.contains("geometry/working-set cap"), "{error}");
}
