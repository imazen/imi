//! Experimental encoder-only coordinate search against actual synthesis support.
//! The search uses a coding-space quadratic surrogate with a magnitude proxy
//! or frozen coder-token prices. Evaluation uses finalized files and metrics.
//! This is a candidate encoder, not a claim of globally optimal coefficients.
use super::*;

pub(super) fn quantizer(p: &LossyParsed<'_>, cfg: &LossyConfig) -> QParams {
    // Recover the signalled preset by exact decoded dequantization factors.
    // This avoids duplicating the encoder's automatic matrix-selection policy.
    (0..=MATRIX_LAST_PRESET)
        .map(|m| {
            QParams::from_quality(
                cfg.scale_for(p.color, p.info.width, p.info.height),
                p.color,
                m,
            )
        })
        .find(|qp| (0..3).all(|c| dct::dequant_factors(&qp.steps()[c]) == p.dq[c]))
        .expect("study refinement requires an exactly recovered preset quantizer")
}

fn weight(mode: Mode, x: usize, y: usize) -> f64 {
    if mode != Mode::Blend {
        return 1.0;
    }
    let nx = if x > 0 && x.is_multiple_of(7) { 2 } else { 1 };
    let ny = if y > 0 && y.is_multiple_of(7) { 2 } else { 1 };
    1.0 / (nx * ny) as f64
}

/// Repack the modified symbols with the original colour/quantization and real
/// entropy/DC assembly. Unsupported features are refused, never silently lost.
pub(super) fn repack(p: &LossyParsed<'_>, cfg: &LossyConfig, q: &[i32], aq: &[f32]) -> Vec<u8> {
    assert!(p.bias.is_none() && p.transform.is_none());
    assert!(cfg.channel_mul.is_none());
    let qp = quantizer(p, cfg);
    let steps = qp.steps();
    for c in 0..3 {
        assert_eq!(
            dct::dequant_factors(&steps[c]),
            p.dq[c],
            "repack quantization must agree exactly"
        );
    }
    let nz: Vec<u64> = q
        .as_chunks::<64>()
        .0
        .iter()
        .map(|b| {
            coef::ZIGZAG
                .iter()
                .enumerate()
                .fold(0, |m, (k, &i)| m | u64::from(b[i as usize] != 0) << k)
        })
        .collect();
    let stripes = (0..p.g.stripes())
        .map(|s| {
            let (a, b) = p.g.rows(s);
            StripeCoefs {
                by0: a,
                nby: b - a,
                bw: p.g.bw,
                q: q[a * p.g.bw * 192..b * p.g.bw * 192].to_vec(),
                nz: nz[a * p.g.bw * 3..b * p.g.bw * 3].to_vec(),
                aq: if aq.is_empty() {
                    Vec::new()
                } else {
                    aq[a * p.g.bw..b * p.g.bw]
                        .iter()
                        .map(|&v| (v * 64.0) as u8)
                        .collect()
                },
                modes: Vec::new(),
                splits: Vec::new(),
            }
        })
        .collect();
    let frame = finish_lossy_frame(
        &p.info,
        &Metadata::default(),
        cfg,
        crate::effort::Effort::new(cfg.effort).unwrap(),
        None,
        None,
        &p.g,
        p.g.sb as u32,
        p.color,
        InputKind::U8,
        p.color.default_scales(),
        qp,
        steps,
        p.layouts,
        false,
        None,
        false,
        None,
        &[],
        &[],
        None,
        stripes,
        &how_far::NoPulse,
    )
    .unwrap();
    super::framing::finish(frame, cfg.checksum.then_some(cfg.hash))
}

/// Choose coefficients using every emitted sample and, optionally, its source
/// boundary gradient. Border samples discarded by ownership have zero weight.
pub(super) fn refine(
    file: &[u8],
    src: &[u8],
    w: usize,
    h: usize,
    stride: usize,
    mode: Mode,
    cfg: &LossyConfig,
    rho: f64,
    edge: f64,
    passes: usize,
    token_prices: bool,
) -> Vec<u8> {
    let p = parse_lossy(file).unwrap();
    admitted(&p);
    assert!(p.bias.is_none());
    let (_, _, mut q, _, _) = quantized_lab(file).unwrap();
    let (_, aq) = dc_all(&p).unwrap();
    let step = mode.step();
    let extent = if mode == Mode::Blend { 8 } else { step };
    let scales = p.color.default_scales();
    // Target conversion is kept separate from the per-coefficient loops.
    assert!(stride >= w * 3 && src.len() >= (h - 1) * stride + w * 3);
    let target: Vec<[f32; 3]> = src
        .chunks(stride)
        .take(h)
        .flat_map(|row| row[..w * 3].as_chunks::<3>().0.iter())
        .map(|v| color::forward(p.color, scales, [v[0], v[1], v[2]]))
        .collect();
    let mut residual = vec![[0.0f64; 3]; w * h];
    for (r, t) in residual.iter_mut().zip(&target) {
        for c in 0..3 {
            r[c] = -(t[c] as f64);
        }
    }
    for by in 0..p.g.bh {
        for bx in 0..p.g.bw {
            let block = by * p.g.bw + bx;
            for c in 0..3 {
                let factors = aq_dequant(&p.dq[c], aq.get(block).copied());
                let mut b = [0.0; 64];
                for k in 0..64 {
                    b[k] = q[(block * 3 + c) * 64 + k] as f32 * factors[k];
                }
                dct::idct8x8(&mut b);
                for y in 0..extent.min(h - by * step) {
                    for x in 0..extent.min(w - bx * step) {
                        let (xx, yy) = (bx * step + x, by * step + y);
                        residual[yy * w + xx][c] += b[y * 8 + x] as f64 * weight(mode, xx, yy);
                    }
                }
            }
        }
    }
    // Unit basis responses let each trial use dot products rather than a new
    // IDCT. They only guide the encoder; final pixels always use the decoder.
    let mut bases = [[[0.0f32; 64]; 64]; 3];
    for c in 0..3 {
        for k in 0..64 {
            bases[c][k][k] = p.dq[c][k];
            dct::idct8x8(&mut bases[c][k]);
        }
    }
    let qp = quantizer(&p, cfg);
    let lambda = if token_prices {
        // A quantized first-luma-AC unit's full synthesis energy tracks the
        // actual quantization scale. The nested DC step is largely independent
        // of quality and is unsuitable for scaling this AC-only search.
        // Use the same untruncated reference for all three placements.
        rho * bases[0][1]
            .iter()
            .map(|&v| (v as f64 * p.inv_scale[0] as f64).powi(2))
            .sum::<f64>()
    } else {
        rho * (qp.steps()[1][0] as f64 * p.inv_scale[1] as f64).powi(2)
    };
    let price = |v: i32| (1.0 + v.unsigned_abs() as f64).log2();
    let model = token_prices.then(|| super::rate::Model::new(&p));
    for _ in 0..passes {
        for by in 0..p.g.bh {
            for bx in 0..p.g.bw {
                let block = by * p.g.bw + bx;
                let (x0, y0) = (bx * step, by * step);
                let (nx, ny) = (extent.min(w - x0), extent.min(h - y0));
                for c in 0..3 {
                    for k in 0..64 {
                        // DC is coded by the nested lossless grammar. Keep it
                        // fixed in this AC-token search, rather than pricing it
                        // as if it were an AC coefficient.
                        if token_prices && k == 0 {
                            continue;
                        }
                        let qi = (block * 3 + c) * 64 + k;
                        let old = q[qi];
                        let aqs = if k == 0 {
                            1.0
                        } else {
                            aq.get(block).copied().unwrap_or(1.0)
                        } as f64;
                        let mut jac = [0.0f64; 64];
                        let (mut dot, mut norm) = (0.0, 0.0);
                        let cw = (p.inv_scale[c] as f64).powi(2);
                        for y in 0..ny {
                            for x in 0..nx {
                                let (xx, yy) = (x0 + x, y0 + y);
                                let a = bases[c][k][y * 8 + x] as f64 * aqs * weight(mode, xx, yy);
                                jac[y * 8 + x] = a;
                                dot += residual[yy * w + xx][c] * a * cw;
                                norm += a * a * cw;
                            }
                        }
                        if edge != 0.0 {
                            // Each boundary pair is enumerated once, including a pair
                            // with only one endpoint in this block's synthesis support.
                            for y in 0..ny {
                                for xx in x0.saturating_sub(1)..(x0 + nx).min(w - 1) {
                                    if !(xx + 1).is_multiple_of(step) {
                                        continue;
                                    }
                                    let a = if xx >= x0 { jac[y * 8 + xx - x0] } else { 0.0 };
                                    let b = if xx + 1 < x0 + nx {
                                        jac[y * 8 + xx + 1 - x0]
                                    } else {
                                        0.0
                                    };
                                    let d = b - a;
                                    dot += edge
                                        * (residual[(y0 + y) * w + xx + 1][c]
                                            - residual[(y0 + y) * w + xx][c])
                                        * d
                                        * cw;
                                    norm += edge * d * d * cw;
                                }
                            }
                            for yy in y0.saturating_sub(1)..(y0 + ny).min(h - 1) {
                                if !(yy + 1).is_multiple_of(step) {
                                    continue;
                                }
                                for x in 0..nx {
                                    let a = if yy >= y0 {
                                        jac[(yy - y0) * 8 + x]
                                    } else {
                                        0.0
                                    };
                                    let b = if yy + 1 < y0 + ny {
                                        jac[(yy + 1 - y0) * 8 + x]
                                    } else {
                                        0.0
                                    };
                                    let d = b - a;
                                    dot += edge
                                        * (residual[(yy + 1) * w + x0 + x][c]
                                            - residual[yy * w + x0 + x][c])
                                        * d
                                        * cw;
                                    norm += edge * d * d * cw;
                                }
                            }
                        }
                        let mut choice = 0;
                        let mut best = 0.0;
                        let rates = model.as_ref().map(|m| m.deltas(&mut q, qi));
                        for (j, delta) in [-1, 1].into_iter().enumerate() {
                            let candidate = old + delta;
                            if candidate.unsigned_abs() >= 1 << 22 {
                                continue;
                            }
                            let cost = 2.0 * delta as f64 * dot
                                + norm
                                + lambda
                                    * rates.map_or_else(|| price(candidate) - price(old), |r| r[j]);
                            if cost < best {
                                best = cost;
                                choice = delta;
                            }
                        }
                        if choice != 0 {
                            q[qi] += choice;
                            for y in 0..ny {
                                for x in 0..nx {
                                    residual[(y0 + y) * w + x0 + x][c] +=
                                        choice as f64 * jac[y * 8 + x];
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    repack(&p, cfg, &q, &aq)
}

#[test]
fn repack_preserves_symbols_and_native_pixels() {
    let (w, h) = (35, 29);
    let src: Vec<_> = (0..w * h * 3)
        .map(|i| ((i * 71 + i * i * 13) % 256) as u8)
        .collect();
    for mode in [Mode::Base, Mode::Own, Mode::Blend] {
        for color in [LossyColor::Xyb, LossyColor::YCoCg] {
            for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                for checksum in [false, true] {
                    for (effort, aq_gamma, seed) in [
                        (7.5, 0.0, 1),
                        (7.5, 1.0, 0),
                        (9.0, 1.0, 1),
                        (9.0, 0.0, 0),
                        (10.0, 0.0, 0),
                        (10.0, 1.0, 1),
                    ] {
                        let (info, atlas) = patches(&src, w, h, w * 3, mode);
                        let mut cfg = LossyConfig::new(35.0)
                            .with_color(color)
                            .with_entropy(entropy)
                            .with_effort(effort)
                            .with_stripe_block_rows(2);
                        cfg.lab.superblocks = Some(false);
                        cfg.column_groups = Some(0);
                        cfg.checksum = checksum;
                        cfg.aq_gamma = aq_gamma;
                        cfg.seed_bank = seed;
                        let file = encode_lossy(
                            &atlas,
                            info.row_bytes(),
                            &info,
                            &Metadata::default(),
                            &cfg,
                        )
                        .unwrap();
                        for token_prices in [false, true] {
                            let identity = refine(
                                &file,
                                &src,
                                w,
                                h,
                                w * 3,
                                mode,
                                &cfg,
                                0.25,
                                1.0,
                                0,
                                token_prices,
                            );
                            assert_eq!(
                                identity, file,
                                "zero-search repacking must retain whole-file framing and checksum bytes"
                            );
                            assert_eq!(
                                quantized_lab(&file).unwrap().2,
                                quantized_lab(&identity).unwrap().2
                            );
                            assert_eq!(
                                reference(&file, w, h, mode),
                                reference(&identity, w, h, mode)
                            );
                            let changed = refine(
                                &file,
                                &src,
                                w,
                                h,
                                w * 3,
                                mode,
                                &cfg,
                                0.25,
                                1.0,
                                1,
                                token_prices,
                            );
                            let mut pixels = vec![0; w * h * 3];
                            decode(&changed, w, h, w * 3, &mut pixels, mode, 4, false).unwrap();
                            assert_eq!(pixels, reference(&changed, w, h, mode));
                        }
                    }
                }
            }
        }
    }
}
