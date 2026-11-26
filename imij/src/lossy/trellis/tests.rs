use super::huf_dp::*;
use super::rates::{Powers, Rates};
use super::rl_dp::*;
use super::*;
use crate::lossy::*;
use archmage::prelude::*;
use coef::PK_GROUPS;

#[test]
fn trellis_cancellation_interrupts_inside_the_first_stripe() {
    use core::sync::atomic::{AtomicUsize, Ordering};
    struct StopInRow {
        rows: AtomicUsize,
        checks: AtomicUsize,
    }
    impl how_far::Stop for StopInRow {
        fn check(&self) -> Result<(), how_far::StopReason> {
            if self.rows.load(Ordering::Relaxed) > 0
                && self.checks.fetch_add(1, Ordering::Relaxed) >= 4
            {
                Err(how_far::StopReason::Cancelled)
            } else {
                Ok(())
            }
        }
    }
    let stop = StopInRow {
        rows: AtomicUsize::new(0),
        checks: AtomicUsize::new(0),
    };
    let bw = 32;
    let sc = StripeCoefs {
        by0: 0,
        nby: 4,
        bw,
        q: vec![0; bw * 4 * NCH * 64],
        nz: vec![0; bw * 4 * NCH],
        aq: Vec::new(),
        modes: Vec::new(),
        splits: Vec::new(),
    };
    let rows = |_: usize, planes: &mut [f32], _: &mut Vec<(u64, bool)>| {
        stop.rows.fetch_add(1, Ordering::Relaxed);
        planes.fill(0.0);
    };
    let cfg = LossyConfig::default().with_threads(1);
    let set = for_effort(crate::effort::Effort::new(9.0).unwrap()).unwrap();
    let result = optimize(
        vec![sc],
        &rows,
        bw * 8,
        bw,
        0,
        &[[1.0; 64]; 3],
        &cfg,
        crate::effort::Effort::new(9.0).unwrap(),
        &set,
        &[],
        &[],
        None,
        false,
        &stop,
    );
    assert!(matches!(result, Err(Error::Cancelled)));
    assert_eq!(
        stop.rows.load(Ordering::Relaxed),
        1,
        "cancellation must not finish all rows of the stripe"
    );
}

/// Every block of a unit in raster order; `src` laid out like `sc.q`.
fn unit(
    sc: &mut StripeCoefs,
    src: &[f32],
    steps: &[[f32; 64]; 3],
    rates: &Rates,
    set: &LossyTrellis,
    _aq: impl Fn(usize) -> f32,
) {
    let mut cands: alloc::boxed::Box<Cands> = alloc::boxed::Box::default();
    let pw = set.powers();
    let lab = Lab::default();
    let cspw: [(LossyTrellis, Powers); NCH] = core::array::from_fn(|ch| {
        let cs = for_channel(set, ch, &lab);
        (cs, if cs == *set { pw } else { cs.powers() })
    });
    for b in 0..sc.nby * sc.bw {
        let at = |ch: usize| &src[(b * NCH + ch) * 64..][..64];
        block_scalar(
            ScalarToken,
            sc,
            b,
            at,
            steps,
            rates,
            set,
            &lab,
            pw,
            None,
            &mut cands,
            Some(&cspw),
        );
    }
}

fn prices_for(sc: &StripeCoefs) -> Prices {
    let (tables, explicit) = ans_tables(
        core::slice::from_ref(sc),
        &LossyConfig::default(),
        crate::effort::Effort::new(10.0).unwrap(),
    );
    Prices::new(&explicit.unwrap_or(tables))
}

/// Exact cost of a unit's tokens under `prices` (the writer's sequence).
fn rate(sc: &StripeCoefs, prices: &Prices) -> f64 {
    let mut bits = 0.0;
    stripe_tokens(sc, sc.bw, Group::ALL, |_, ctx, tok, n, _| {
        bits += prices.token(ctx, tok) + n as f64;
    });
    bits
}

fn lcg(seed: &mut u64) -> f64 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (*seed >> 11) as f64 / (1u64 << 53) as f64
}

/// A unit of random coefficients: AC nonzero with probability `dense`,
/// mostly small, some large.
fn fixture_with(
    bw: usize,
    nby: usize,
    dense: f64,
    seed: &mut u64,
) -> (StripeCoefs, Vec<f32>, [[f32; 64]; 3]) {
    let steps: [[f32; 64]; 3] =
        core::array::from_fn(|c| core::array::from_fn(|i| 1.0 + ((i * 7 + c) % 13) as f32 / 3.0));
    let nb = bw * nby;
    let mut src = vec![0f32; nb * NCH * 64];
    let mut q = vec![0i32; nb * NCH * 64];
    let mut nz = vec![0u64; nb * NCH];
    for bc in 0..nb * NCH {
        let ch = bc % NCH;
        for (k, &z) in ZIGZAG.iter().enumerate() {
            let z = z as usize;
            let r = lcg(seed);
            let mag = if k == 0 {
                40.0 * lcg(seed)
            } else if r >= dense {
                0.0
            } else if r < 0.78 * dense {
                1.6 * lcg(seed) / (1.0 + k as f64 / 16.0)
            } else {
                9.0 * lcg(seed)
            };
            let v = if lcg(seed) < 0.5 { -mag } else { mag };
            src[bc * 64 + z] = (v * steps[ch][z] as f64) as f32;
            let m = (mag + 0.4).floor() as i32;
            q[bc * 64 + z] = if v < 0.0 { -m } else { m };
            if m != 0 {
                nz[bc] |= 1 << k;
            }
        }
    }
    let sc = StripeCoefs {
        by0: 0,
        nby,
        bw,
        q,
        nz,
        aq: vec![],
        modes: vec![],

        splits: Vec::new(),
    };
    (sc, src, steps)
}

fn fixture(bw: usize, nby: usize, seed: &mut u64) -> (StripeCoefs, Vec<f32>, [[f32; 64]; 3]) {
    fixture_with(bw, nby, 0.45, seed)
}

fn block_distortion(src: &[f32], steps: &[f32; 64], q: &[i32], w: f64) -> f64 {
    (1..64)
        .map(|i| {
            let v = src[i] as f64 / steps[i] as f64;
            w * (v - q[i] as f64).powi(2)
        })
        .sum()
}

/// Enumerate every candidate combination of one block channel (each
/// position: zero or one of its candidates) and check the trellis
/// reaches the minimum of distortion + the writer's exact rate (this
/// channel's tokens, its EOB token and the EOB tokens whose context it
/// sets).
#[test]
fn trellis_matches_exhaustive_search() {
    let mut seed = 7u64;
    let set = LossyTrellis {
        thorough: true,
        ..LossyTrellis::default()
    };
    let mut checked = 0;
    for trial in 0..400 {
        let (bw, nby) = [(1, 1), (3, 2), (17, 2), (20, 1)][trial % 4];
        // Sparse blocks keep the enumeration small; some dense ones
        // around them shape the tables and contexts.
        let (mut sc, src, steps) = fixture_with(bw, nby, 0.07, &mut seed);
        let prices = prices_for(&sc);
        let b = (lcg(&mut seed) * (bw * nby) as f64) as usize;
        let ch = trial % NCH;
        let bc = b * NCH + ch;
        // Keep enumeration small: at most 5 positions with a candidate.
        let s = &src[bc * 64..][..64];
        let nonzero: Vec<usize> = (1..64)
            .filter(|&k| {
                let z = ZIGZAG[k] as usize;
                (s[z].abs() / steps[ch][z] + 0.5).floor() >= 1.0
            })
            .collect();
        if nonzero.len() > 5 || nonzero.is_empty() {
            continue;
        }
        let norm = (1..64).map(|i| (s[i] as f64).powi(2)).sum::<f64>() * 64.0 / 63.0;
        let w = 64.0 * set.lambda(norm);
        // Process blocks before b with the trellis so the lane state is
        // what `unit` would see; then compare at block b, channel ch.
        let objective = |sc: &StripeCoefs| {
            block_distortion(&src[bc * 64..][..64], &steps[ch], &sc.q[bc * 64..][..64], w)
                + rate(sc, &prices)
        };
        // Candidate magnitudes per position (zero included).
        let options: Vec<Vec<u32>> = nonzero
            .iter()
            .map(|&k| {
                let z = ZIGZAG[k] as usize;
                let a = s[z].abs() as f64 / steps[ch][z] as f64;
                let top = (a + 0.5).floor() as u32;
                let mut o = vec![0, top];
                for c in 0..class(top) {
                    o.push(class_max(c));
                }
                o
            })
            .collect();
        let mut best = f64::INFINITY;
        let total: usize = options.iter().map(Vec::len).product();
        let original = sc.q[bc * 64..][..64].to_vec();
        let original_nz = sc.nz[bc];
        for mut idx in 0..total {
            let mut nzm = original_nz & 1;
            for (o, &k) in options.iter().zip(&nonzero) {
                let m = o[idx % o.len()] as i32;
                idx /= o.len();
                let z = ZIGZAG[k] as usize;
                sc.q[bc * 64 + z] = if s[z] < 0.0 { -m } else { m };
                if m != 0 {
                    nzm |= 1 << k;
                }
            }
            for k in 1..64 {
                if !nonzero.contains(&k) {
                    sc.q[bc * 64 + ZIGZAG[k] as usize] = 0;
                }
            }
            sc.nz[bc] = nzm;
            best = best.min(objective(&sc));
        }
        sc.q[bc * 64..][..64].copy_from_slice(&original);
        sc.nz[bc] = original_nz;
        // The trellis on this channel alone, in its lane context.
        let (previous, next) = lane_neighbors(b, bw, nby);
        let reference = if ch == 0 {
            previous.map_or(0, |p| support(sc.nz[p * NCH] & !1))
        } else {
            support(sc.nz[b * NCH] & !1)
        };
        let la: [f64; coef::EOB_CLASSES] = if ch == 0 {
            let chroma = [
                support(sc.nz[b * NCH + 1] & !1),
                support(sc.nz[b * NCH + 2] & !1),
            ];
            core::array::from_fn(|cls| {
                let e = if cls == 0 { 0 } else { 1u32 << (cls - 1) };
                prices.token(eob_ctx(1, e), chroma[0])
                    + prices.token(eob_ctx(2, e), chroma[1])
                    + next.map_or(0.0, |n| {
                        prices.token(eob_ctx(0, e), support(sc.nz[n * NCH] & !1))
                    })
            })
        } else {
            [0.0; coef::EOB_CLASSES]
        };
        let mut q = sc.q[bc * 64..][..64].to_vec();
        let mut nzm = sc.nz[bc];
        let mut cands = alloc::boxed::Box::<Cands>::default();
        channel(
            s,
            &steps[ch],
            ch,
            reference,
            &la,
            &prices,
            &set,
            set.powers(),
            &mut cands,
            &mut q,
            &mut nzm,
        );
        sc.q[bc * 64..][..64].copy_from_slice(&q);
        sc.nz[bc] = nzm;
        let got = objective(&sc);
        assert!(
            (got - best).abs() <= 1e-9 * best.abs().max(1.0),
            "trial {trial}: trellis {got} vs exhaustive {best}"
        );
        assert_eq!(sc.q[bc * 64], original[0], "DC kept");
        checked += 1;
    }
    assert!(checked > 100, "{checked}");
}

#[test]
fn huge_lambda_rounds_to_nearest_and_tiny_lambda_zeroes() {
    let mut seed = 3u64;
    let (sc0, src, steps) = fixture(17, 2, &mut seed);
    let prices = Rates::RunLevel(prices_for(&sc0));
    let near = LossyTrellis {
        lambda_log_scale1: 40.0,
        lambda_log_scale2: 0.0,
        thorough: true,
        ..LossyTrellis::default()
    };
    let mut sc = StripeCoefs {
        q: sc0.q.clone(),
        nz: sc0.nz.clone(),
        aq: vec![],
        modes: vec![],

        splits: Vec::new(),
        ..sc0
    };
    unit(&mut sc, &src, &steps, &prices, &near, |_| 0.0);
    for bc in 0..sc.nz.len() {
        let ch = bc % NCH;
        for k in 1..64 {
            let z = ZIGZAG[k] as usize;
            let v = src[bc * 64 + z] / steps[ch][z];
            let m = (v.abs() as f64 + 0.5).floor() as i32;
            assert_eq!(sc.q[bc * 64 + z], if v < 0.0 { -m } else { m });
            assert_eq!((sc.nz[bc] >> k) & 1, (m != 0) as u64);
        }
    }
}

fn huf_prices_for(sc: &StripeCoefs) -> HufPrices {
    HufPrices::new(
        core::slice::from_ref(sc),
        1,
        crate::effort::Effort::new(10.0).unwrap(),
        0,
    )
}

/// Exact split-Huffman bits of a unit under `hp`: the three sections'
/// codes, magnitude bits and sign bits (the writer's sequence).
fn huf_rate_of(sc: &StripeCoefs, hp: &HufPrices) -> f64 {
    let (counts, widths) = stripe_counts_widths(sc, sc.bw);
    let syms = huf::stripe_syms(&counts, &widths, sc.nby * sc.bw);
    let mut bits = 0.0;
    for &(c, v) in &syms.luma {
        bits += hp.luma[c as usize][v as usize];
    }
    for &(c, v) in &syms.chroma {
        bits += hp.chroma[c as usize][v as usize];
    }
    for &(c, v) in &syms.wid {
        bits += hp.wid[c as usize][v as usize];
    }
    for bc in 0..counts.len() {
        for gi in 0..counts[bc] as usize {
            let w = widths[bc * PK_GROUPS + gi] as usize;
            bits += (coef::pk_len(gi) * w) as f64;
            bits += (0..coef::pk_len(gi))
                .filter(|&j| sc.q[bc * 64 + ZIGZAG[1 + 4 * gi + j] as usize] != 0)
                .count() as f64;
        }
    }
    bits
}

/// Enumerate every magnitude 0..=nearest of the nonzero candidates of one
/// block channel and check the split-Huffman trellis reaches at most
/// that minimum of distortion + the writer's exact bits (its search also
/// covers one width above).
#[test]
fn huf_trellis_matches_exhaustive_search() {
    let mut seed = 11u64;
    let set = LossyTrellis::default();
    let mut checked = 0;
    for trial in 0..600 {
        let (bw, nby) = [(1, 1), (3, 2), (17, 2), (9, 1)][trial % 4];
        let (mut sc, src, steps) = fixture_with(bw, nby, 0.06, &mut seed);
        let hp = huf_prices_for(&sc);
        let b = (lcg(&mut seed) * (bw * nby) as f64) as usize;
        let ch = trial % NCH;
        let bc = b * NCH + ch;
        let s = &src[bc * 64..][..64];
        let nearest: Vec<(usize, u32)> = (1..64)
            .filter_map(|k| {
                let z = ZIGZAG[k] as usize;
                let m = (s[z].abs() / steps[ch][z] + 0.5).floor() as u32;
                (m >= 1).then_some((k, m))
            })
            .collect();
        let total: usize = nearest.iter().map(|&(_, m)| m as usize + 1).product();
        if nearest.is_empty() || total > 3000 {
            continue;
        }
        let norm = (1..64).map(|i| (s[i] as f64).powi(2)).sum::<f64>() * 64.0 / 63.0;
        let w = 64.0 * set.lambda(norm);
        let objective = |sc: &StripeCoefs| {
            block_distortion(s, &steps[ch], &sc.q[bc * 64..][..64], w) + huf_rate_of(sc, &hp)
        };
        let original = sc.q[bc * 64..][..64].to_vec();
        let original_nz = sc.nz[bc];
        let mut best = f64::INFINITY;
        for mut idx in 0..total {
            let mut nzm = original_nz & 1;
            for k in 1..64 {
                sc.q[bc * 64 + ZIGZAG[k] as usize] = 0;
            }
            for &(k, top) in &nearest {
                let m = (idx % (top as usize + 1)) as i32;
                idx /= top as usize + 1;
                let z = ZIGZAG[k] as usize;
                sc.q[bc * 64 + z] = if s[z] < 0.0 { -m } else { m };
                if m != 0 {
                    nzm |= 1 << k;
                }
            }
            sc.nz[bc] = nzm;
            best = best.min(objective(&sc));
        }
        sc.q[bc * 64..][..64].copy_from_slice(&original);
        sc.nz[bc] = original_nz;
        huf_block_channel_scalar(
            ScalarToken,
            &mut sc,
            b,
            ch,
            s,
            &steps[ch],
            &hp,
            &set,
            set.powers(),
            None,
        );
        let got = objective(&sc);
        let scalar_out = (sc.q[bc * 64..][..64].to_vec(), sc.nz[bc]);
        sc.q[bc * 64..][..64].copy_from_slice(&original);
        sc.nz[bc] = original_nz;
        // The vector lanes compute the same values where the CPU
        // offers a tier (bit-identical is what the byte gate checks).
        #[cfg(target_arch = "x86_64")]
        if let Some(t) = <archmage::X64V3Token as archmage::SimdToken>::summon() {
            huf_block_channel_v3(
                t,
                &mut sc,
                b,
                ch,
                s,
                &steps[ch],
                &hp,
                &set,
                set.powers(),
                None,
            );
            assert_eq!(
                (sc.q[bc * 64..][..64].to_vec(), sc.nz[bc]),
                scalar_out,
                "trial {trial}: V3 differs from scalar"
            );
        }
        #[cfg(target_arch = "aarch64")]
        if let Some(t) = <archmage::NeonToken as archmage::SimdToken>::summon() {
            huf_block_channel_neon(
                t,
                &mut sc,
                b,
                ch,
                s,
                &steps[ch],
                &hp,
                &set,
                set.powers(),
                None,
            );
            assert_eq!(
                (sc.q[bc * 64..][..64].to_vec(), sc.nz[bc]),
                scalar_out,
                "trial {trial}: NEON differs from scalar"
            );
        }
        #[cfg(target_arch = "wasm32")]
        if let Some(t) = <archmage::Wasm128Token as archmage::SimdToken>::summon() {
            huf_block_channel_wasm128(
                t,
                &mut sc,
                b,
                ch,
                s,
                &steps[ch],
                &hp,
                &set,
                set.powers(),
                None,
            );
            assert_eq!(
                (sc.q[bc * 64..][..64].to_vec(), sc.nz[bc]),
                scalar_out,
                "trial {trial}: wasm128 differs from scalar"
            );
        }
        assert!(
            got <= best + 1e-9 * best.abs().max(1.0),
            "trial {trial}: trellis {got} vs exhaustive {best}"
        );
        assert_eq!(scalar_out.0[0], original[0], "DC kept");
        checked += 1;
    }
    assert!(checked > 150, "{checked}");
}

#[test]
fn huf_trellis_limits() {
    let mut seed = 13u64;
    let (sc0, src, steps) = fixture(17, 2, &mut seed);
    let rates = Rates::Huf(alloc::boxed::Box::new(huf_prices_for(&sc0)));
    let fresh = || StripeCoefs {
        q: sc0.q.clone(),
        nz: sc0.nz.clone(),
        aq: vec![],
        modes: vec![],

        splits: Vec::new(),
        ..sc0
    };
    let near = LossyTrellis {
        lambda_log_scale1: 40.0,
        lambda_log_scale2: 0.0,
        ..LossyTrellis::default()
    };
    let mut sc = fresh();
    unit(&mut sc, &src, &steps, &rates, &near, |_| 0.0);
    for bc in 0..sc.nz.len() {
        let ch = bc % NCH;
        for k in 1..64 {
            let z = ZIGZAG[k] as usize;
            let v = src[bc * 64 + z] / steps[ch][z];
            let m = (v.abs() as f64 + 0.5).floor() as i32;
            assert_eq!(sc.q[bc * 64 + z], if v < 0.0 { -m } else { m });
            assert_eq!((sc.nz[bc] >> k) & 1, (m != 0) as u64);
        }
    }
    let zero = LossyTrellis {
        lambda_log_scale1: 0.0,
        lambda_log_scale2: 0.0,
        ..LossyTrellis::default()
    };
    // A luma count also prices its block's previous-pass chroma tokens,
    // so a luma group may stay; chroma has no lookahead.
    let mut sc = fresh();
    unit(&mut sc, &src, &steps, &rates, &zero, |_| 0.0);
    let ones = |nz: &[u64], ch: usize| -> u32 {
        nz.iter()
            .skip(ch)
            .step_by(NCH)
            .map(|v| (v & !1).count_ones())
            .sum()
    };
    for bc in 0..sc.nz.len() {
        assert_eq!(sc.q[bc * 64], sc0.q[bc * 64], "DC kept");
        if bc % NCH != 0 {
            assert_eq!(sc.nz[bc] & !1, 0, "block channel {bc}");
        }
    }
    assert!(
        ones(&sc.nz, 0) * 20 <= ones(&sc0.nz, 0),
        "{}",
        ones(&sc.nz, 0)
    );
}

#[test]
fn shared_prefix_and_pruning_match_the_plain_search() {
    let mut seed = 5u64;
    for (dense, s1) in [
        (0.45, 14.5),
        (0.9, 14.5),
        (0.9, 12.0),
        (0.97, 17.0),
        (0.6, 15.5),
    ] {
        let (sc, src, steps) = fixture_with(17, 2, dense, &mut seed);
        let prices = prices_for(&sc);
        for thorough in [false, true] {
            let set = LossyTrellis {
                lambda_log_scale1: s1,
                thorough,
                ..LossyTrellis::default()
            };
            let mut cands = alloc::boxed::Box::<Cands>::default();
            for bc in 0..sc.nz.len() {
                let ch = bc % NCH;
                let la = [0.5; coef::EOB_CLASSES];
                let pw = set.powers();
                let run = |variant: u8, cands: &mut Cands, v3: bool| {
                    let mut q = sc.q[bc * 64..][..64].to_vec();
                    let mut nzm = sc.nz[bc];
                    let s = &src[bc * 64..][..64];
                    macro_rules! ci {
                        ($f:ident, $t:expr) => {
                            match variant {
                                0 => $f::<false, false>(
                                    $t, s, &steps[ch], ch, 3, &la, &prices, &set, pw, None, cands,
                                    &mut q, &mut nzm,
                                ),
                                1 => $f::<true, false>(
                                    $t, s, &steps[ch], ch, 3, &la, &prices, &set, pw, None, cands,
                                    &mut q, &mut nzm,
                                ),
                                2 => $f::<false, true>(
                                    $t, s, &steps[ch], ch, 3, &la, &prices, &set, pw, None, cands,
                                    &mut q, &mut nzm,
                                ),
                                _ => $f::<true, true>(
                                    $t, s, &steps[ch], ch, 3, &la, &prices, &set, pw, None, cands,
                                    &mut q, &mut nzm,
                                ),
                            }
                        };
                    }
                    #[cfg(target_arch = "x86_64")]
                    if v3 {
                        ci!(
                            channel_impl_v3,
                            <archmage::X64V3Token as archmage::SimdToken>::summon().unwrap()
                        );
                        return (q, nzm);
                    }
                    let _ = v3;
                    ci!(channel_impl_scalar, ScalarToken);
                    (q, nzm)
                };
                let plain = run(0, &mut cands, false);
                for variant in 1..4 {
                    assert_eq!(
                        run(variant, &mut cands, false),
                        plain,
                        "dense {dense} s1 {s1} bc {bc} v{variant}"
                    );
                }
                // The V3 lane eval computes the same values where the
                // CPU offers it.
                #[cfg(target_arch = "x86_64")]
                if <archmage::X64V3Token as archmage::SimdToken>::summon().is_some() {
                    for variant in 0..4 {
                        assert_eq!(
                            run(variant, &mut cands, true),
                            plain,
                            "v3 dense {dense} s1 {s1} bc {bc} v{variant}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn class_maxima_follow_rl_token() {
    for a in 1..5000u32 {
        let (s, _, n) = coef::rl_token(0, a as i32);
        assert_eq!(s & 31, class(a));
        assert_eq!(n, raw_bits(a));
        assert!(class_max(class(a)) >= a);
        assert_eq!(class(class_max(class(a))), class(a));
        if class(a) > 0 {
            assert!(class_max(class(a) - 1) < a);
        }
    }
}

#[cfg(all(test, feature = "encode"))]
mod difffuzz {
    use super::*;
    use crate::lossy::trellis_base as tb;

    struct Rng(u64);
    impl Rng {
        fn u(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let x = self.0;
            (x ^ (x >> 29)).wrapping_mul(0xbf58476d1ce4e5b9) >> 11
        }
        fn f(&mut self) -> f64 {
            self.u() as f64 / (1u64 << 53) as f64
        }
        fn below(&mut self, n: u64) -> u64 {
            self.u() % n.max(1)
        }
    }

    fn iters() -> usize {
        std::env::var("DIFF_ITERS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20000)
    }
    fn seed0() -> u64 {
        std::env::var("DIFF_SEED")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1)
    }

    /// Steps: random, powers of two (exact quotients), equal, or tiny.
    fn steps(r: &mut Rng) -> [f32; 64] {
        // DIFF_STEPSCALE: multiply every step (a caller's tiny `channel_mul`).
        let k: f32 = std::env::var("DIFF_STEPSCALE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0);
        steps_base(r).map(|s| s * k)
    }

    fn steps_base(r: &mut Rng) -> [f32; 64] {
        match r.below(5) {
            0 => core::array::from_fn(|_| (0.05 + 60.0 * r.f()) as f32),
            1 => core::array::from_fn(|_| (2f32).powi(r.below(8) as i32 - 2)),
            2 => [(0.25 + 10.0 * r.f()) as f32; 64],
            3 => core::array::from_fn(|_| (0.004 + 0.05 * r.f()) as f32),
            _ => core::array::from_fn(|i| 1.0 + ((i * 7) % 13) as f32 / 3.0),
        }
    }

    /// One block channel's raster coefficients in units of `st`, from one of
    /// several shapes: dense, sparse, half-integer ties, integer values,
    /// class-boundary values, one late coefficient, huge, all tiny.
    fn coefs(r: &mut Rng, st: &[f32; 64]) -> [f32; 64] {
        let mode = r.below(9);
        let dense = [0.02, 0.1, 0.3, 0.6, 0.95][r.below(5) as usize];
        let scale = [0.7, 2.0, 6.0, 40.0, 900.0, 3.0e5][r.below(6) as usize];
        let mut a = [0f64; 64];
        for k in 1..64 {
            if r.f() >= dense {
                continue;
            }
            a[k] = match mode {
                0 | 1 => scale * r.f() * r.f(),
                2 => r.below(scale as u64 + 1) as f64 + 0.5,
                3 => r.below(scale as u64 + 1) as f64,
                4 => {
                    let c = r.below(12) as u32;
                    let m = tb::class_max(c) as f64;
                    m + [-0.5, -0.25, 0.0, 0.25, 0.5, 0.49999][r.below(6) as usize]
                }
                5 => {
                    if k > 40 {
                        scale * (0.5 + r.f())
                    } else {
                        0.3 * r.f()
                    }
                }
                6 => 0.4999 * r.f(),
                7 => scale * 1e3 * r.f(),
                _ => (r.below(4) as f64) * 0.5 + 0.5 * r.f() * r.f(),
            };
        }
        let mut src = [0f32; 64];
        for k in 1..64 {
            let z = ZIGZAG[k] as usize;
            let v = (a[k] * st[z] as f64) as f32;
            src[z] = if r.below(2) == 0 { -v } else { v };
        }
        src[0] = (r.f() * 400.0) as f32;
        src
    }

    fn settings(r: &mut Rng) -> (LossyTrellis, tb::BaseTrellis) {
        let s1 = match r.below(4) {
            0 | 1 => 14.5,
            2 => (40.0 * r.f()) as f32,
            _ => [0.0, 8.0, 11.0, 17.0, 24.0, 40.0][r.below(6) as usize],
        };
        let s2 = match r.below(4) {
            0 | 1 => 16.5,
            2 => 0.0,
            _ => (40.0 * r.f()) as f32,
        };
        let thorough = r.below(3) == 0;
        let n = LossyTrellis {
            lambda_log_scale1: s1,
            lambda_log_scale2: s2,
            thorough,
            ..LossyTrellis::default()
        };
        (n, tb::base_set(s1, s2, thorough))
    }

    /// A random unit and its realistic run-level tables, then (mode 1/2)
    /// the frequencies replaced by power-of-two counts (integer prices,
    /// many ties) or sparse ones (absent symbols cost a whole slot).
    fn rl_tables(r: &mut Rng) -> Tables {
        let (bw, nby) = (1 + r.below(12) as usize, 1 + r.below(2) as usize);
        let (sc, _) = unit(r, bw, nby);
        let (t, e) = ans_tables(
            core::slice::from_ref(&sc),
            &LossyConfig::default(),
            crate::effort::Effort::new(10.0).unwrap(),
        );
        let mut t = e.unwrap_or(t);
        match r.below(3) {
            0 => {}
            1 => {
                for f in t.freq.iter_mut() {
                    for v in f.iter_mut() {
                        *v = 1 << r.below(10);
                    }
                }
            }
            _ => {
                for f in t.freq.iter_mut() {
                    for v in f.iter_mut() {
                        *v = if r.below(3) == 0 {
                            0
                        } else {
                            1 + r.below(50) as u32
                        };
                    }
                }
            }
        }
        if r.below(4) == 0 {
            let nt = t.freq.len() as u64;
            for m in t.ctx_map.iter_mut() {
                *m = r.below(nt) as u8;
            }
        }
        t
    }

    /// A unit of random nearest-quantized coefficients and its sources.
    fn unit(r: &mut Rng, bw: usize, nby: usize) -> (StripeCoefs, Vec<f32>) {
        let nb = bw * nby;
        let mut src = vec![0f32; nb * NCH * 64];
        let mut q = vec![0i32; nb * NCH * 64];
        let mut nz = vec![0u64; nb * NCH];
        let st = steps(r);
        for bc in 0..nb * NCH {
            let s = coefs(r, &st);
            src[bc * 64..][..64].copy_from_slice(&s);
            for k in 0..64 {
                let z = ZIGZAG[k] as usize;
                let a = (s[z].abs() / st[z]) as f64;
                let m = (a + 0.5).floor().min(tb::MAX_MAGNITUDE as f64) as i32;
                q[bc * 64 + z] = if s[z] < 0.0 { -m } else { m };
                if m != 0 {
                    nz[bc] |= 1 << k;
                }
            }
        }
        let sc = StripeCoefs {
            by0: 0,
            nby,
            bw,
            q,
            nz,
            aq: vec![],
            modes: vec![],

            splits: Vec::new(),
        };
        (sc, src)
    }

    /// Which tiers each loop runs: scalar always, plus the host's vector
    /// tier where its token summons.
    fn arms() -> &'static [&'static str] {
        #[cfg(target_arch = "x86_64")]
        {
            assert!(
                <archmage::X64V3Token as archmage::SimdToken>::summon().is_some(),
                "run on an AVX2 host"
            );
            return &["scalar", "v3"];
        }
        #[cfg(target_arch = "aarch64")]
        {
            assert!(
                <archmage::NeonToken as archmage::SimdToken>::summon().is_some(),
                "run on a NEON host"
            );
            return &["scalar", "neon"];
        }
        #[cfg(target_arch = "wasm32")]
        {
            assert!(
                <archmage::Wasm128Token as archmage::SimdToken>::summon().is_some(),
                "run on a wasm128 host"
            );
            return &["scalar", "wasm128"];
        }
        #[allow(unreachable_code)]
        &["scalar"]
    }

    /// `channel_impl::<true, true>` on the named tier.
    #[allow(clippy::too_many_arguments)]
    fn channel_arm(
        arm: &str,
        src: &[f32],
        st: &[f32; 64],
        ch: usize,
        ref_eob: u32,
        la: &[f64; coef::EOB_CLASSES],
        pn: &Prices,
        sn: &LossyTrellis,
        cn: &mut Cands,
        qn: &mut [i32],
        nzn: &mut u64,
    ) {
        let pw = sn.powers();
        match arm {
            #[cfg(target_arch = "x86_64")]
            "v3" => channel_impl_v3::<true, true>(
                <archmage::X64V3Token as archmage::SimdToken>::summon().unwrap(),
                src,
                st,
                ch,
                ref_eob,
                la,
                pn,
                sn,
                pw,
                None,
                cn,
                qn,
                nzn,
            ),
            #[cfg(target_arch = "aarch64")]
            "neon" => channel_impl_neon::<true, true>(
                <archmage::NeonToken as archmage::SimdToken>::summon().unwrap(),
                src,
                st,
                ch,
                ref_eob,
                la,
                pn,
                sn,
                pw,
                None,
                cn,
                qn,
                nzn,
            ),
            #[cfg(target_arch = "wasm32")]
            "wasm128" => channel_impl_wasm128::<true, true>(
                <archmage::Wasm128Token as archmage::SimdToken>::summon().unwrap(),
                src,
                st,
                ch,
                ref_eob,
                la,
                pn,
                sn,
                pw,
                None,
                cn,
                qn,
                nzn,
            ),
            _ => channel_impl_scalar::<true, true>(
                archmage::ScalarToken,
                src,
                st,
                ch,
                ref_eob,
                la,
                pn,
                sn,
                pw,
                None,
                cn,
                qn,
                nzn,
            ),
        }
    }

    /// `huf_block_channel` on the named tier.
    #[allow(clippy::too_many_arguments)]
    fn huf_arm(
        arm: &str,
        s: &mut StripeCoefs,
        b: usize,
        ch: usize,
        src: &[f32],
        st: &[f32; 64],
        hn: &HufPrices,
        sn: &LossyTrellis,
    ) -> (u32, [u8; PK_GROUPS]) {
        let pw = sn.powers();
        match arm {
            #[cfg(target_arch = "x86_64")]
            "v3" => huf_block_channel_v3(
                <archmage::X64V3Token as archmage::SimdToken>::summon().unwrap(),
                s,
                b,
                ch,
                src,
                st,
                hn,
                sn,
                pw,
                None,
            ),
            #[cfg(target_arch = "aarch64")]
            "neon" => huf_block_channel_neon(
                <archmage::NeonToken as archmage::SimdToken>::summon().unwrap(),
                s,
                b,
                ch,
                src,
                st,
                hn,
                sn,
                pw,
                None,
            ),
            #[cfg(target_arch = "wasm32")]
            "wasm128" => huf_block_channel_wasm128(
                <archmage::Wasm128Token as archmage::SimdToken>::summon().unwrap(),
                s,
                b,
                ch,
                src,
                st,
                hn,
                sn,
                pw,
                None,
            ),
            _ => {
                huf_block_channel_scalar(archmage::ScalarToken, s, b, ch, src, st, hn, sn, pw, None)
            }
        }
    }

    #[test]
    fn rl_channel_matches_base() {
        let mut r = Rng(seed0());
        let mut cn: alloc::boxed::Box<Cands> = alloc::boxed::Box::default();
        let mut cb = tb::new_cands();
        let mut tables = rl_tables(&mut r);
        let (mut pn, mut pb) = (Prices::new(&tables), tb::prices_from(&tables));
        let mut fails = 0;
        for it in 0..iters() {
            if it % 200 == 0 {
                tables = rl_tables(&mut r);
                (pn, pb) = (Prices::new(&tables), tb::prices_from(&tables));
            }
            let st = steps(&mut r);
            let src = coefs(&mut r, &st);
            let (sn, sb) = settings(&mut r);
            let ch = r.below(3) as usize;
            let ref_eob = r.below(64) as u32;
            let la: [f64; coef::EOB_CLASSES] = match r.below(3) {
                0 => [0.0; coef::EOB_CLASSES],
                1 => core::array::from_fn(|_| r.below(24) as f64),
                _ => core::array::from_fn(|_| 20.0 * r.f()),
            };
            let q0: [i32; 64] =
                core::array::from_fn(|i| if i == 0 { 17 } else { r.below(3) as i32 });
            let nz0 = 1 | (r.u() & !1);
            let (mut qb, mut nzb) = (q0, nz0);
            tb::channel(
                &src, &st, ch, ref_eob, &la, &pb, &sb, &mut cb, &mut qb, &mut nzb,
            );
            for &arm in arms() {
                let (mut qn, mut nzn) = (q0, nz0);
                channel_arm(
                    arm, &src, &st, ch, ref_eob, &la, &pn, &sn, &mut cn, &mut qn, &mut nzn,
                );
                if qn != qb || nzn != nzb {
                    fails += 1;
                    if fails <= 5 {
                        std::eprintln!(
                            "RL MISMATCH it {it} arm {arm} set {sn:?} ch {ch} ref {ref_eob} la {la:?}\nsrc {src:?}\nsteps {st:?}\nbase {qb:?} {nzb:#x}\nnew  {qn:?} {nzn:#x}"
                        );
                    }
                }
            }
        }
        assert_eq!(fails, 0, "run-level mismatches");
    }

    fn huf_tables(r: &mut Rng) -> huf::HufTables {
        let (bw, nby) = (1 + r.below(12) as usize, 1 + r.below(2) as usize);
        let (sc, _) = unit(r, bw, nby);
        let (counts, widths) = stripe_counts_widths(&sc, sc.bw);
        let syms = [huf::stripe_syms(&counts, &widths, sc.nby * sc.bw)];
        let (mut t, _) =
            huffman_tables(&syms, 1, crate::effort::Effort::new(10.0).unwrap(), None, 0);
        match r.below(3) {
            0 => {}
            1 => {
                for l in t.lens.iter_mut() {
                    for v in l.iter_mut() {
                        *v = r.below(10) as u8;
                    }
                }
            }
            _ => {
                let v = 1 + r.below(9) as u8;
                for l in t.lens.iter_mut() {
                    for x in l.iter_mut() {
                        *x = if r.below(4) == 0 { 0 } else { v };
                    }
                }
            }
        }
        t
    }

    #[test]
    fn huf_block_channel_matches_base() {
        let mut r = Rng(seed0() ^ 0x5eed);
        let mut t = huf_tables(&mut r);
        let (mut hn, mut hb) = (HufPrices::from_tables(&t), tb::huf_prices_from(&t));
        let mut fails = 0;
        for it in 0..iters() {
            if it % 200 == 0 {
                t = huf_tables(&mut r);
                (hn, hb) = (HufPrices::from_tables(&t), tb::huf_prices_from(&t));
            }
            let (bw, nby) = (1 + r.below(9) as usize, 1 + r.below(2) as usize);
            let (sc0, _) = unit(&mut r, bw, nby);
            let st = steps(&mut r);
            let src = coefs(&mut r, &st);
            let (sn, sb) = settings(&mut r);
            let b = r.below((bw * nby) as u64) as usize;
            let ch = r.below(3) as usize;
            let bc = b * NCH + ch;
            // Previous-pass state of this block channel: nearest values, or
            // zeroed (the all-zero fast path needs `nz` AC empty).
            let mut start = StripeCoefs {
                q: sc0.q.clone(),
                nz: sc0.nz.clone(),
                aq: vec![],
                modes: vec![],

                splits: Vec::new(),
                ..sc0
            };
            if r.below(3) == 0 {
                start.q[bc * 64 + 1..bc * 64 + 64].fill(0);
                start.nz[bc] &= 1;
            }
            let mut sb_ = StripeCoefs {
                q: start.q.clone(),
                nz: start.nz.clone(),
                aq: vec![],
                modes: vec![],

                splits: Vec::new(),
                ..start
            };
            tb::huf_block_channel(&mut sb_, b, ch, &src, &st, &hb, &sb);
            for &arm in arms() {
                let mut s = StripeCoefs {
                    q: start.q.clone(),
                    nz: start.nz.clone(),
                    aq: vec![],
                    modes: vec![],

                    splits: Vec::new(),
                    ..start
                };
                let (n, w) = huf_arm(arm, &mut s, b, ch, &src, &st, &hn, &sn);
                let ok = s.q[bc * 64..][..64] == sb_.q[bc * 64..][..64] && s.nz[bc] == sb_.nz[bc];
                let n_scan = group_count(s.nz[bc] & !1);
                let w_scan = group_widths(&s.q[bc * 64..][..64], n_scan);
                let hw_ok = n == n_scan && w == w_scan;
                if !ok || !hw_ok {
                    fails += 1;
                    if fails <= 5 {
                        std::eprintln!(
                            "HUF MISMATCH it {it} arm {arm} ok {ok} hw_ok {hw_ok} set {sn:?} b {b} ch {ch} bw {bw} nby {nby}\nsrc {src:?}\nsteps {st:?}\nbase {:?} {:#x}\nnew  {:?} {:#x} ret ({n}, {w:?}) scan ({n_scan}, {w_scan:?})",
                            &sb_.q[bc * 64..][..64],
                            sb_.nz[bc],
                            &s.q[bc * 64..][..64],
                            s.nz[bc]
                        );
                    }
                }
            }
        }
        assert_eq!(fails, 0, "split-Huffman mismatches");
    }
}
