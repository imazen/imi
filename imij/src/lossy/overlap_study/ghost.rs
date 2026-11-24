//! Encoder-only search in the discarded border's fifteen spatial degrees of
//! freedom. IRLS minimizes a smooth coefficient sparsity proxy. Neither that
//! proxy nor prequantization energy is reported as image quality.
use super::*;

fn solve(a: &[[f64; 15]; 15], rhs: &[f64; 15]) -> [f64; 15] {
    let mut l = [[0.0; 15]; 15];
    for i in 0..15 {
        for j in 0..=i {
            let mut v = a[i][j];
            for k in 0..j {
                v -= l[i][k] * l[j][k];
            }
            if i == j {
                assert!(v > 0.0 && v.is_finite(), "positive definite border system");
                l[i][j] = v.sqrt();
            } else {
                l[i][j] = v / l[j][j];
            }
        }
    }
    let mut x = [0.0; 15];
    for i in 0..15 {
        let mut v = rhs[i];
        for k in 0..i {
            v -= l[i][k] * x[k];
        }
        x[i] = v / l[i][i];
    }
    for i in (0..15).rev() {
        let mut v = x[i];
        for k in i + 1..15 {
            v -= l[k][i] * x[k];
        }
        x[i] = v / l[i][i];
    }
    x
}

fn basis() -> [[f64; 64]; 64] {
    let c = dct::dct_matrix();
    core::array::from_fn(|k| {
        core::array::from_fn(|n| c[k / 8][n / 8] as f64 * c[k % 8][n % 8] as f64)
    })
}

fn project(
    input: &[f32; 64],
    steps: &[f32; 64],
    basis: &[[f64; 64]; 64],
    iterations: usize,
) -> [f64; 64] {
    let indices: [usize; 15] = core::array::from_fn(|i| if i < 7 { i * 8 + 7 } else { 56 + i - 7 });
    let mut fixed = [0.0; 64];
    let mut coeff = [0.0; 64];
    for k in 0..64 {
        for n in 0..64 {
            let v = basis[k][n] * input[n] as f64;
            coeff[k] += v;
            if n / 8 < 7 && n % 8 < 7 {
                fixed[k] += v;
            }
        }
    }
    for _ in 0..iterations {
        let mut a = [[0.0; 15]; 15];
        let mut rhs = [0.0; 15];
        for k in 0..64 {
            let weight = 1.0 / ((steps[k] as f64).powi(2) + coeff[k] * coeff[k]);
            for i in 0..15 {
                let d = basis[k][indices[i]] * weight;
                rhs[i] -= d * fixed[k];
                for j in 0..=i {
                    a[i][j] += d * basis[k][indices[j]];
                }
            }
        }
        for i in 0..15 {
            for j in 0..i {
                a[j][i] = a[i][j];
            }
        }
        let border = solve(&a, &rhs);
        for k in 0..64 {
            coeff[k] = fixed[k];
            for i in 0..15 {
                coeff[k] += basis[k][indices[i]] * border[i];
            }
        }
    }
    coeff
}

pub(super) fn refine(
    file: &[u8],
    src: &[u8],
    w: usize,
    h: usize,
    stride: usize,
    cfg: &LossyConfig,
    iterations: usize,
    reconstruction: bool,
) -> Vec<u8> {
    assert!(stride >= w * 3 && src.len() >= (h - 1) * stride + w * 3);
    let p = parse_lossy(file).unwrap();
    admitted(&p);
    assert!(p.bias.is_none());
    let qp = super::rdo::quantizer(&p, cfg);
    let steps = qp.steps();
    let (_, aq) = dc_all(&p).unwrap();
    let scales = p.color.default_scales();
    let basis = basis();
    let original_q = quantized_lab(file).unwrap().2;
    let mut q = vec![0; p.g.bw * p.g.bh * 192];
    for by in 0..p.g.bh {
        for bx in 0..p.g.bw {
            let block = by * p.g.bw + bx;
            let mut planes = [[0.0; 64]; 3];
            let (mut white, mut black) = (0u64, true);
            for y in 0..8 {
                for x in 0..8 {
                    let offset = (by * 7 + y).min(h - 1) * stride + (bx * 7 + x).min(w - 1) * 3;
                    let pixel = [src[offset], src[offset + 1], src[offset + 2]];
                    if pixel == [255; 3] {
                        white |= 1 << (y * 8 + x);
                    }
                    black &= pixel == [0; 3];
                    let value = color::forward(p.color, scales, pixel);
                    for c in 0..3 {
                        planes[c][y * 8 + x] = value[c];
                    }
                }
            }
            if reconstruction {
                for c in 0..3 {
                    let dq = aq_dequant(&p.dq[c], aq.get(block).copied());
                    for k in 0..64 {
                        planes[c][k] = original_q[(block * 3 + c) * 64 + k] as f32 * dq[k];
                    }
                    dct::idct8x8(&mut planes[c]);
                }
            } else if dering::enabled(cfg) {
                dering_row(&mut planes[0], 8, &[(white, black)], 2.0 * steps[0][0]);
            }
            for c in 0..3 {
                let factors: [f32; 64] = core::array::from_fn(|k| {
                    steps[c][k]
                        * if k == 0 {
                            1.0
                        } else {
                            aq.get(block).copied().unwrap_or(1.0)
                        }
                });
                let coeff = project(&planes[c], &factors, &basis, iterations);
                for k in 0..64 {
                    let scaled = coeff[k] / factors[k] as f64;
                    let v = if k == 0 && dc_nearest(cfg) {
                        if !reconstruction && c == 0 && white == u64::MAX {
                            scaled.ceil()
                        } else if !reconstruction && c == 0 && black {
                            scaled.floor()
                        } else {
                            scaled.round()
                        }
                    } else {
                        (scaled.abs() + cfg.round as f64).floor() * scaled.signum()
                    };
                    assert!(
                        v.is_finite() && v.abs() < (1 << 22) as f64,
                        "coefficient domain"
                    );
                    q[(block * 3 + c) * 64 + k] = v as i32;
                }
            }
        }
    }
    super::rdo::repack(&p, cfg, &q, &aq)
}

#[test]
fn discarded_border_keeps_visible_signal_and_reduces_its_proxy() {
    let basis = basis();
    let input = core::array::from_fn(|i| ((i * 37 + i * i * 11) % 251) as f32);
    let steps = core::array::from_fn(|i| 2.0 + (i / 8 + i % 8) as f32 * 3.0);
    let before = project(&input, &steps, &basis, 0);
    let after = project(&input, &steps, &basis, 4);
    let objective = |c: &[f64; 64]| {
        (0..64)
            .map(|k| (1.0 + (c[k] / steps[k] as f64).powi(2)).ln())
            .sum::<f64>()
    };
    assert!(objective(&after) < objective(&before) - 1.0);
    let factors = dct::dequant_factors(&[1.0; 64]);
    let mut pixels = core::array::from_fn(|k| after[k] as f32 * factors[k]);
    dct::idct8x8(&mut pixels);
    for y in 0..7 {
        for x in 0..7 {
            assert!(
                (pixels[y * 8 + x] - input[y * 8 + x]).abs() < 0.001,
                "visible prequantization sample {x},{y}"
            );
        }
    }
}

#[test]
fn reconstructed_zero_search_preserves_encoder_symbols_and_pixel_routes() {
    let (w, h, stride) = (35, 29, 35 * 3 + 5);
    let mut src = vec![0xCC; stride * h];
    for y in 0..h {
        for x in 0..w {
            for c in 0..3 {
                src[y * stride + x * 3 + c] = if x < 12 {
                    255
                } else if y > 17 {
                    0
                } else {
                    ((x * 37 + y * 71 + c * 19 + x * y) % 256) as u8
                };
            }
        }
    }
    for (effort, seed) in [(7.5, 1), (9.0, 0), (10.0, 1)] {
        for color in [LossyColor::Xyb, LossyColor::YCoCg] {
            for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                for quality in [5.0, 35.0, 90.0] {
                    for aq in [0.0, 1.0] {
                        let (info, atlas) = patches(&src, w, h, stride, Mode::Own);
                        let mut cfg = LossyConfig::new(quality)
                            .with_color(color)
                            .with_entropy(entropy)
                            .with_stripe_block_rows(2);
                        cfg.aq_gamma = aq;
                        cfg.effort = effort;
                        cfg.seed_bank = seed;
                        cfg.lab.superblocks = Some(false);
                        cfg.column_groups = Some(0);
                        let file = encode_lossy(
                            &atlas,
                            info.row_bytes(),
                            &info,
                            &Metadata::default(),
                            &cfg,
                        )
                        .unwrap();
                        let identity = refine(&file, &src, w, h, stride, &cfg, 0, true);
                        assert_eq!(
                            quantized_lab(&file).unwrap().2,
                            quantized_lab(&identity).unwrap().2,
                            "{color:?} {entropy:?} q{quality} aq{aq}"
                        );
                        let changed = refine(&file, &src, w, h, stride, &cfg, 4, true);
                        let expected = reference(&changed, w, h, Mode::Own);
                        let mut decoded = vec![0; w * h * 3];
                        optimized::decode_pruned(
                            &changed,
                            w,
                            h,
                            w * 3,
                            &mut decoded,
                            Mode::Own,
                            4,
                            16,
                        )
                        .unwrap();
                        assert_eq!(decoded, expected);
                    }
                }
            }
        }
    }
}
