use super::closed_loop::{ClosedLoopStore, StripeClosedLoop};
use super::*;
use crate::lossy::{LossyConfig, LossyEntropy, NCH, encode_lossy};
use crate::{Channels, Error, ImageInfo, Metadata, SampleType};
use alloc::vec;
use alloc::vec::Vec;

fn lcg(seed: &mut u64) -> f64 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    ((*seed >> 11) as f64) * (1.0 / ((1u64 << 53) as f64))
}

/// Verifies that the exact 8-state row-strip Viterbi DP equals exhaustive
/// `8^G` brute-force search to `< 1e-12` on random costs, contexts, masks,
/// and pairwise transition tables.
#[test]
fn row_strip_dp_matches_brute_force() {
    let mut seed = 0x2026_1125_cafe_babeu64;
    for g in 1..=4usize {
        for trial in 0..25 {
            let mut unary = vec![[f64::INFINITY; 8]; g];
            let mut up_class = vec![0u8; g];
            for gc in 0..g {
                up_class[gc] = (lcg(&mut seed) * 3.0) as u8 % 3;
                unary[gc][0] = lcg(&mut seed) * 50.0;
                for s in 1..8 {
                    if lcg(&mut seed) < 0.75 {
                        unary[gc][s] = lcg(&mut seed) * 50.0;
                    }
                }
            }
            let mut map_bits = MapBitTable::prior();
            for ctx in 0..9 {
                for s in 0..8 {
                    map_bits.bits[ctx][s] = 0.2 + lcg(&mut seed) * 5.0;
                }
            }
            let mut horiz = vec![[[0.0f64; 8]; 8]; g];
            for gc in 1..g {
                for sp in 0..8 {
                    for s in 0..8 {
                        horiz[gc][sp][s] = lcg(&mut seed) * 10.0;
                    }
                }
            }

            let (dp_cost, dp_path) =
                solve_row_strip_dp(&unary, &up_class, &map_bits, |gc, sp, s| horiz[gc][sp][s])
                    .unwrap();

            let total_seqs = 1usize << (3 * g);
            let mut bf_cost = f64::INFINITY;
            let mut bf_path = vec![0u8; g];
            for mut code in 0..total_seqs {
                let mut seq = [0u8; 4];
                let mut ok = true;
                for gc in 0..g {
                    let s = (code & 7) as u8;
                    code >>= 3;
                    if !unary[gc][s as usize].is_finite() {
                        ok = false;
                        break;
                    }
                    seq[gc] = s;
                }
                if !ok {
                    continue;
                }
                let mut c = unary[0][seq[0] as usize]
                    + map_bits.bits[up_class[0] as usize][seq[0] as usize];
                for gc in 1..g {
                    let sp = seq[gc - 1] as usize;
                    let s = seq[gc] as usize;
                    let ctx = sym_class(sp as u8) * 3 + up_class[gc] as usize;
                    c += map_bits.bits[ctx][s] + horiz[gc][sp][s] + unary[gc][s];
                }
                if c < bf_cost {
                    bf_cost = c;
                    bf_path.copy_from_slice(&seq[..g]);
                }
            }

            assert!(
                (dp_cost - bf_cost).abs() < 1e-12,
                "g={g} trial={trial}: dp={dp_cost} bf={bf_cost} dp_path={dp_path:?} bf_path={bf_path:?}"
            );
        }
    }
}

#[test]
fn map_bit_table_prior_probabilities_sum_to_one() {
    let t = MapBitTable::prior();
    for ctx in 0..9 {
        let sum: f64 = (0..8)
            .map(|s| crate::lossy::transfer::pow(2.0, -t.bits[ctx][s]))
            .sum();
        assert!((sum - 1.0).abs() < 1e-6, "ctx={ctx} sum={sum}");
    }
}

/// Verifies that the open-loop surrounding-block lambda map assigns higher
/// protection to smooth regions and flat-adjacent edges than to dense
/// chaotic texture, is bit-identical across 1T/4T/8T, AND matches
/// [`OpenLoopLambdaStream`] bit-for-bit across work-band sizes.
#[test]
fn open_loop_lambda_map_protects_smooth_and_matches_stream_and_threads() {
    let (bw, bh) = (12usize, 37usize);
    let pw = bw * 8;
    let steps = [[4.0f32; 64]; NCH];
    let rows = |by: usize, planes: &mut [f32], _ext: &mut Vec<(u64, bool)>| {
        planes.fill(0.0);
        for y in 0..8 {
            let gy = by * 8 + y;
            for x in 0..pw {
                let bx = x / 8;
                let v = if bx < 4 {
                    (x as f32) * 0.25 + (gy as f32) * 0.15
                } else if bx < 8 {
                    if x % 8 < 4 { 0.0 } else { 40.0 }
                } else if ((x + gy) & 1) == 0 {
                    35.0
                } else {
                    -35.0
                };
                planes[y * pw + x] = v;
            }
        }
    };
    let m1 =
        build_open_loop_lambda_map(&rows, pw, bw, bh, &steps, 0.30, 1, &how_far::NoPulse).unwrap();
    let m4 =
        build_open_loop_lambda_map(&rows, pw, bw, bh, &steps, 0.30, 4, &how_far::NoPulse).unwrap();
    let m8 =
        build_open_loop_lambda_map(&rows, pw, bw, bh, &steps, 0.30, 8, &how_far::NoPulse).unwrap();
    assert_eq!(m1, m4, "open-loop lambda map 1T vs 4T mismatch");
    assert_eq!(m1, m8, "open-loop lambda map 1T vs 8T mismatch");

    let lo = crate::lossy::transfer::exp(-0.85) as f32 - 1e-5;
    let hi = crate::lossy::transfer::exp(0.85) as f32 + 1e-5;
    for &v in &m1 {
        assert!(v.is_finite() && v >= lo && v <= hi, "out of bounds: {v}");
    }
    let smooth_interior = m1[2 * bw + 1];
    let edge_interior = m1[2 * bw + 5];
    let texture_interior = m1[2 * bw + 10];
    assert!(smooth_interior > texture_interior);
    assert!(edge_interior > texture_interior);

    // Verify streaming OpenLoopLambdaStream == whole-image build_open_loop_lambda_map bit-for-bit.
    for work in [1usize, 3, 8, 16] {
        let mut stream = OpenLoopLambdaStream::new(bw, bh, work, &steps, 0.30).unwrap();
        let mut planes = vec![0.0f32; NCH * 8 * pw];
        let mut ext = Vec::new();
        for by in 0..bh {
            ext.clear();
            rows(by, &mut planes, &mut ext);
            stream.push_row(by, &planes, pw, &steps);
        }
        let model = stream.finish().unwrap();
        let mut cursor = model.cursor(pw).unwrap();
        let mut streamed = Vec::with_capacity(bw * bh);
        let mut band_out = Vec::new();
        for by0 in (0..bh).step_by(work) {
            let by1 = (by0 + work).min(bh);
            model
                .fill_band(&mut cursor, by0, by1, &rows, pw, &steps, &mut band_out)
                .unwrap();
            streamed.extend_from_slice(&band_out);
        }
        assert_eq!(
            streamed, m1,
            "OpenLoopLambdaStream diverged from whole-image at work={work}"
        );
    }
}

#[test]
fn overflow_dimensions_return_too_large() {
    let rows = |_: usize, _: &mut [f32], _: &mut Vec<(u64, bool)>| {};
    let res = build_open_loop_lambda_map(
        &rows,
        8,
        usize::MAX / 2,
        4,
        &[[4.0; 64]; NCH],
        0.30,
        1,
        &how_far::NoPulse,
    );
    assert!(matches!(res, Err(Error::TooLarge)));
}

/// Encodes a synthetic multi-region image (smooth gradient, sharp text/edge,
/// dense texture, saturated color patch, and exact-eligible flat UI strip)
/// with `adaptive_partitions` across efforts 7.5, 9.0, 9.5, 10.0, both entropy
/// coders, column groups, and exact blocks, verifying roundtrip decode and
/// 0-bit cross-ISA (SIMD vs scalar) and multi-thread parity.
#[test]
fn adaptive_partitions_roundtrip_exact_and_cross_isa_parity() {
    let (w, h) = (160u32, 128u32);
    let mut px = vec![0u8; (w * h * 3) as usize];
    let mut seed = 0x1234_5678_9abc_def0u64;
    for y in 0..h {
        for x in 0..w {
            let (r, g, b) = if y < 32 {
                (
                    ((x * 255) / w) as u8,
                    ((y * 255) / 32) as u8,
                    140u8.saturating_sub((x / 2) as u8),
                )
            } else if y < 64 {
                let stroke = ((x % 16) < 2 && (y % 12) < 9) || (y % 12 == 0 && (x % 16) < 7);
                if stroke {
                    (18, 22, 28)
                } else {
                    (246, 246, 248)
                }
            } else if y < 96 {
                let n = (lcg(&mut seed) * 180.0) as u8;
                (
                    n.saturating_add(30),
                    (n ^ 0x55).saturating_add(20),
                    220u8.saturating_sub(n / 2),
                )
            } else if x < 80 {
                (230, 35 + ((x / 4) as u8), 45)
            } else {
                (30, 185, 225u8.saturating_sub(((x - 80) / 3) as u8))
            };
            let o = ((y * w + x) * 3) as usize;
            px[o] = r;
            px[o + 1] = g;
            px[o + 2] = b;
        }
    }
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);

    for ent in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
        for effort in [7.5f32, 9.0, 9.5, 10.0] {
            for q in [20.0f32, 75.0] {
                let mut cfg = LossyConfig::new(q)
                    .with_effort(effort)
                    .with_entropy(ent)
                    .with_adaptive_partitions(true);
                cfg.column_groups = Some(64);
                if q > 50.0 {
                    cfg.exact_blocks = Some(0.5);
                }
                let encoded =
                    encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), &cfg).unwrap();
                let (dec_simd, dec_scalar) = crate::lossy::tests::both(&encoded);
                let (dec_simd, dec_scalar) = (dec_simd.unwrap(), dec_scalar.unwrap());
                assert_eq!(dec_simd.len(), px.len());
                assert_eq!(
                    dec_simd, dec_scalar,
                    "SIMD vs scalar decode mismatch on {ent:?} e{effort} q{q}"
                );
                #[cfg(feature = "std")]
                {
                    let mut dec_mt = vec![0u8; dec_simd.len()];
                    crate::decode_into_threads(&encoded, &mut dec_mt, info.row_bytes(), 4).unwrap();
                    assert_eq!(
                        dec_mt, dec_simd,
                        "4T vs 1T decode mismatch on {ent:?} e{effort} q{q}"
                    );
                }
            }
        }
    }
}

fn synthetic_multi_region(w: u32, h: u32) -> Vec<u8> {
    let mut px = vec![0u8; (w * h * 3) as usize];
    let mut seed = 0x51a7_9921_c0de_f00du64;
    for y in 0..h {
        for x in 0..w {
            let (r, g, b) = if y < h / 4 {
                (
                    ((x * 255) / w.max(1)) as u8,
                    ((y * 255) / (h / 4).max(1)) as u8,
                    150u8.saturating_sub((x / 2) as u8),
                )
            } else if y < h / 2 {
                let stroke = ((x % 14) < 2 && (y % 11) < 8) || (y % 11 == 0 && (x % 14) < 6);
                if stroke {
                    (16, 20, 26)
                } else {
                    (244, 245, 247)
                }
            } else if y < (3 * h) / 4 {
                let n = (lcg(&mut seed) * 190.0) as u8;
                (
                    n.saturating_add(25),
                    (n ^ 0x33).saturating_add(15),
                    225u8.saturating_sub(n / 2),
                )
            } else {
                (220, 40 + ((x / 3) as u8), 60 + ((y / 4) as u8))
            };
            let o = ((y * w + x) * 3) as usize;
            px[o] = r;
            px[o + 1] = g;
            px[o + 2] = b;
        }
    }
    px
}

/// Verifies bit-exact encoder determinism across 1T/4T/8T encoder thread
/// counts, odd block-grid dimensions (exercising the trailing 1x1 corner
/// omitted by V4 partition maps), odd column-group widths, and ungrouped
/// encodes. Cross-ISA `for_each_token_permutation` parity runs in the
/// isolated `tests/trellis.rs` integration test binary so process-wide token
/// disabling never races with parallel `--lib` SIMD unit tests.
#[test]
fn adaptive_partitions_encoder_determinism_and_odd_geometry() {
    // 133x117 => 17x15 blocks (odd bw & odd bh => trailing 1x1 corner).
    // 133x109 => 17x14 blocks (odd bw & even bh).
    for &(w, h) in &[(133u32, 117u32), (133u32, 109u32)] {
        let px = synthetic_multi_region(w, h);
        let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
        for col_groups in [None, Some(56u32), Some(64u32)] {
            for ent in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                for effort in [7.5f32, 9.0, 9.5, 10.0] {
                    let mut cfg1 = LossyConfig::new(60.0)
                        .with_effort(effort)
                        .with_entropy(ent)
                        .with_adaptive_partitions(true)
                        .with_threads(1);
                    cfg1.column_groups = col_groups;
                    let bytes1 =
                        encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), &cfg1)
                            .unwrap();

                    #[cfg(feature = "std")]
                    for t in [4usize, 8] {
                        let mut cfgt = cfg1.clone().with_threads(t);
                        cfgt.column_groups = col_groups;
                        let bytest =
                            encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), &cfgt)
                                .unwrap();
                        assert_eq!(
                            bytes1, bytest,
                            "encoder output must be thread-count invariant (1T vs {t}T) on {w}x{h} cg={col_groups:?} {ent:?} e{effort}"
                        );
                    }

                    let (dec_simd, dec_scalar) = crate::lossy::tests::both(&bytes1);
                    assert_eq!(
                        dec_simd.unwrap(),
                        dec_scalar.unwrap(),
                        "decode parity mismatch on {w}x{h} cg={col_groups:?} {ent:?} e{effort}"
                    );
                }
            }
        }
    }
}

/// Verifies cooperative cancellation interrupts open-loop lambda map
/// construction, adaptive layout selection, and closed-loop feedback
/// promptly instead of waiting for full-image passes to finish.
#[test]
fn adaptive_rdo_observes_cancellation() {
    use core::sync::atomic::{AtomicUsize, Ordering};

    struct StopAfterRows {
        rows: AtomicUsize,
        limit: usize,
    }
    impl how_far::Stop for StopAfterRows {
        fn check(&self) -> Result<(), how_far::StopReason> {
            if self.rows.load(Ordering::Relaxed) >= self.limit {
                Err(how_far::StopReason::Cancelled)
            } else {
                Ok(())
            }
        }
    }

    // 1. Open-loop lambda map stops after 2 rows of a 16-row grid.
    let stop = StopAfterRows {
        rows: AtomicUsize::new(0),
        limit: 2,
    };
    let (bw, bh) = (16usize, 16usize);
    let rows = |_: usize, planes: &mut [f32], _: &mut Vec<(u64, bool)>| {
        stop.rows.fetch_add(1, Ordering::Relaxed);
        planes.fill(0.0);
    };
    let res = build_open_loop_lambda_map(&rows, bw * 8, bw, bh, &[[4.0; 64]; NCH], 0.30, 1, &stop);
    assert!(matches!(res, Err(Error::Cancelled)));
    assert_eq!(
        stop.rows.load(Ordering::Relaxed),
        2,
        "open-loop lambda map must stop promptly at cancellation row"
    );

    // 2. End-to-end adaptive_partitions e10 encode observes cancellation mid-flight.
    struct StopAfterChecks {
        checks: AtomicUsize,
        limit: usize,
    }
    impl how_far::Stop for StopAfterChecks {
        fn check(&self) -> Result<(), how_far::StopReason> {
            if self.checks.fetch_add(1, Ordering::Relaxed) >= self.limit {
                Err(how_far::StopReason::Cancelled)
            } else {
                Ok(())
            }
        }
    }
    let (w, h) = (160u32, 128u32);
    let px = synthetic_multi_region(w, h);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let cfg = LossyConfig::new(60.0)
        .with_effort(10.0)
        .with_adaptive_partitions(true)
        .with_threads(1);
    for limit in [2usize, 25, 60] {
        let stop = StopAfterChecks {
            checks: AtomicUsize::new(0),
            limit,
        };
        let pulse = how_far::WithStop::borrowed(&how_far::NoPulse, &stop);
        let out = crate::lossy::encode_lossy_with_progress(
            &px,
            info.row_bytes(),
            &info,
            &Metadata::default(),
            &cfg,
            &pulse,
        );
        assert!(
            matches!(out, Err(Error::Cancelled)),
            "expected Error::Cancelled at limit={limit}, got {out:?}"
        );
    }
}

/// Verifies that `ClosedLoopLambdaModel::apply_in_place` (whole-image path)
/// and band-by-band `fill_band` (bounded streaming path) produce bit-exact
/// multipliers under both `Exact` (`<= CLOSED_LOOP_EXACT_MAX_BLOCKS`) and
/// `Compact` (`> CLOSED_LOOP_EXACT_MAX_BLOCKS`) stores, and that `Compact`
/// stays closely aligned with `Exact`.
#[test]
fn closed_loop_stream_matches_whole_image_exact_and_compact() {
    let bw = 12usize;
    let bh = 10usize;
    let nb = bw * bh;
    let steps = [[1.0f32; 64]; NCH];
    let mut seed = 0x5EED_C105_ED10_0901u64;

    let make_stripe = |by0: usize, nby: usize, seed: &mut u64| -> StripeClosedLoop {
        let n = nby * bw;
        let mut blk_err = vec![0.0f64; n];
        let mut skip_blk = vec![false; n];
        for i in 0..n {
            let u = lcg(seed);
            if i % 17 == 0 {
                skip_blk[i] = true;
            } else if i % 13 == 0 {
                blk_err[i] = 0.0;
            } else {
                blk_err[i] = u * u * 25.0;
            }
        }
        let row_px = bw * 8;
        let fill_row = |seed: &mut u64| -> [Vec<f32>; NCH] {
            core::array::from_fn(|_| (0..row_px).map(|_| (lcg(seed) - 0.5) as f32).collect())
        };
        StripeClosedLoop {
            by0,
            nby,
            blk_err,
            skip_blk,
            top_orig: fill_row(seed),
            top_err: fill_row(seed),
            bot_orig: fill_row(seed),
            bot_err: fill_row(seed),
        }
    };

    let stripes = vec![
        make_stripe(0, 4, &mut seed),
        make_stripe(4, 4, &mut seed),
        make_stripe(8, 2, &mut seed),
    ];

    let open_base: Vec<f32> = (0..nb)
        .map(|i| 0.75 + 0.5 * ((i % 11) as f32 / 10.0))
        .collect();

    let run_mode = |exact_max: usize| -> (Vec<f32>, Vec<f32>) {
        let mut stream =
            ClosedLoopLambdaStream::new_with_exact_max(bw, bh, &steps, 0.38, exact_max).unwrap();
        for st in &stripes {
            stream
                .push_stripe(StripeClosedLoop {
                    by0: st.by0,
                    nby: st.nby,
                    blk_err: st.blk_err.clone(),
                    skip_blk: st.skip_blk.clone(),
                    top_orig: st.top_orig.clone(),
                    top_err: st.top_err.clone(),
                    bot_orig: st.bot_orig.clone(),
                    bot_err: st.bot_err.clone(),
                })
                .unwrap();
        }
        let model = stream.finish();
        let mut whole = open_base.clone();
        model.apply_in_place(&mut whole, &how_far::NoPulse).unwrap();

        let mut banded = Vec::with_capacity(nb);
        let mut scratch = Vec::new();
        for by0 in (0..bh).step_by(3) {
            let by1 = (by0 + 3).min(bh);
            model
                .fill_band(&open_base[by0 * bw..by1 * bw], by0, by1, &mut scratch)
                .unwrap();
            banded.extend_from_slice(&scratch);
        }
        (whole, banded)
    };

    let (exact_whole, exact_banded) = run_mode(usize::MAX);
    let (compact_whole, compact_banded) = run_mode(0);

    for i in 0..nb {
        assert_eq!(
            exact_whole[i].to_bits(),
            exact_banded[i].to_bits(),
            "exact whole vs banded mismatch at block {i}"
        );
        assert_eq!(
            compact_whole[i].to_bits(),
            compact_banded[i].to_bits(),
            "compact whole vs banded mismatch at block {i}"
        );
        let diff = (exact_whole[i] - compact_whole[i]).abs();
        assert!(
            diff < 0.05,
            "compact vs exact diverged at block {i}: exact={} compact={} diff={diff}",
            exact_whole[i],
            compact_whole[i]
        );
    }

    // Verify the production threshold switches cleanly to Compact above
    // `CLOSED_LOOP_EXACT_MAX_BLOCKS` without overflowing.
    let big_bw = 513usize;
    let big_bh = 512usize; // 262,656 > 262,144
    assert!(big_bw * big_bh > CLOSED_LOOP_EXACT_MAX_BLOCKS);
    let big = ClosedLoopLambdaStream::new(big_bw, big_bh, &steps, 0.38).unwrap();
    assert!(matches!(big.store, ClosedLoopStore::Compact(_)));
}
