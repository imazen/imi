use super::*;

#[test]
fn fixed_w_residuals_match_full_search_at_lane_and_stripe_edges() {
    for bits in [0, 1, 8, 10, 16, 24, 32] {
        for ns in [1, 3, 4, 8] {
            for w in [1, 3, 17, 33] {
                for sw in [1, 2, 7, w] {
                    for y0 in [0, 2] {
                        for decor in 0..4 {
                            let mut slot_bits = vec![bits; ns];
                            for (s, depth) in slot_bits.iter_mut().enumerate().skip(3) {
                                *depth = ((s as u32 * 7) + bits) % 33;
                            }
                            let layout = Layout {
                                maps: vec![],
                                slot_channel: (0..ns).collect(),
                                slot_bits,
                                color_slots: if ns >= 3 { 3 } else { 1 },
                                decor_ok: ns >= 3,
                                near: vec![0; ns],
                                tables: None,
                            };
                            let planes: Vec<Vec<u32>> = (0..ns)
                                .map(|s| {
                                    let mask = ((1u64 << layout.slot_bits[s]) - 1) as u32;
                                    (0..w * 5)
                                        .map(|i| {
                                            let v = match i % 5 {
                                                0 => 0,
                                                1 => mask,
                                                _ => (i as u32)
                                                    .wrapping_mul(0x9e3779b9)
                                                    .rotate_left(s as u32 * 3),
                                            };
                                            v & mask
                                        })
                                        .collect()
                                })
                                .collect();
                            let planes: Vec<_> = planes.iter().map(Vec::as_slice).collect();
                            let cfg = AnalysisConfig {
                                predictor_mask: 1 << predict::P_W,
                                decor_mask: 1 << decor,
                                ..Default::default()
                            };
                            let mut fast = vec![vec![u32::MAX; w * (5 - y0)]; ns];
                            let mut search = fast.clone();
                            let a = analyze_stripe_impl::<true>(
                                &layout,
                                &planes,
                                w,
                                sw,
                                y0,
                                5,
                                &cfg,
                                &mut fast.iter_mut().map(Vec::as_mut_slice).collect::<Vec<_>>(),
                                None,
                            );
                            let b = analyze_stripe_impl::<false>(
                                &layout,
                                &planes,
                                w,
                                sw,
                                y0,
                                5,
                                &cfg,
                                &mut search.iter_mut().map(Vec::as_mut_slice).collect::<Vec<_>>(),
                                None,
                            );
                            assert_eq!(
                                a, b,
                                "bits={bits}, ns={ns}, w={w}, sw={sw}, y0={y0}, decor={decor}"
                            );
                            assert_eq!(
                                fast, search,
                                "bits={bits}, ns={ns}, w={w}, sw={sw}, y0={y0}, decor={decor}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn fixed_med_residuals_match_full_search_and_multi_decor_is_exact() {
    for bits in [0, 1, 8, 10, 16, 24, 32] {
        for ns in [1, 3, 4, 8] {
            for w in [1, 3, 17, 33] {
                for sw in [1, 2, 7, w] {
                    for y0 in [0, 2] {
                        let mut slot_bits = vec![bits; ns];
                        for (s, depth) in slot_bits.iter_mut().enumerate().skip(3) {
                            *depth = ((s as u32 * 7) + bits) % 33;
                        }
                        let layout = Layout {
                            maps: vec![],
                            slot_channel: (0..ns).collect(),
                            slot_bits,
                            color_slots: if ns >= 3 { 3 } else { 1 },
                            decor_ok: ns >= 3,
                            near: vec![0; ns],
                            tables: None,
                        };
                        let planes: Vec<Vec<u32>> = (0..ns)
                            .map(|s| {
                                let mask = ((1u64 << layout.slot_bits[s]) - 1) as u32;
                                (0..w * 5)
                                    .map(|i| {
                                        let v = match i % 5 {
                                            0 => 0,
                                            1 => mask,
                                            _ => (i as u32)
                                                .wrapping_mul(0x9e3779b9)
                                                .rotate_left(s as u32 * 3),
                                        };
                                        v & mask
                                    })
                                    .collect()
                            })
                            .collect();
                        let planes: Vec<_> = planes.iter().map(Vec::as_slice).collect();
                        for decor in 0..4 {
                            let cfg = AnalysisConfig {
                                predictor_mask: 1 << predict::P_MED,
                                decor_mask: 1 << decor,
                                ..Default::default()
                            };
                            let mut fast = vec![vec![u32::MAX; w * (5 - y0)]; ns];
                            let mut search = fast.clone();
                            let a = analyze_stripe_impl::<true>(
                                &layout,
                                &planes,
                                w,
                                sw,
                                y0,
                                5,
                                &cfg,
                                &mut fast.iter_mut().map(Vec::as_mut_slice).collect::<Vec<_>>(),
                                None,
                            );
                            let b = analyze_stripe_impl::<false>(
                                &layout,
                                &planes,
                                w,
                                sw,
                                y0,
                                5,
                                &cfg,
                                &mut search.iter_mut().map(Vec::as_mut_slice).collect::<Vec<_>>(),
                                None,
                            );
                            assert_eq!(a, b);
                            assert_eq!(fast, search);
                        }
                        // Multi-decor fast path must choose the exact same decor and produce
                        // identical residuals to the general path on both multi-row and 1-row stripes.
                        let multi_cfg = AnalysisConfig {
                            predictor_mask: 1 << predict::P_MED,
                            decor_mask: 0x0f,
                            ..Default::default()
                        };
                        for y1 in [y0 + 1, 5] {
                            let mut multi = vec![vec![u32::MAX; w * (y1 - y0)]; ns];
                            let a = analyze_stripe_impl::<true>(
                                &layout,
                                &planes,
                                w,
                                sw,
                                y0,
                                y1,
                                &multi_cfg,
                                &mut multi.iter_mut().map(Vec::as_mut_slice).collect::<Vec<_>>(),
                                None,
                            );
                            let mut expected = vec![vec![u32::MAX; w * (y1 - y0)]; ns];
                            let b = analyze_stripe_impl::<false>(
                                &layout,
                                &planes,
                                w,
                                sw,
                                y0,
                                y1,
                                &multi_cfg,
                                &mut expected
                                    .iter_mut()
                                    .map(Vec::as_mut_slice)
                                    .collect::<Vec<_>>(),
                                None,
                            );
                            assert_eq!(a, b, "bits={bits} ns={ns} w={w} sw={sw} y0={y0} y1={y1}");
                            assert_eq!(
                                multi, expected,
                                "bits={bits} ns={ns} w={w} sw={sw} y0={y0} y1={y1}"
                            );
                        }
                    }
                }
            }
        }
    }
    // Explicit 1-row RGB witness with equal nonzero channel residuals: both paths
    // exclude row 0 from decor scoring and pick decor 0.
    let layout = Layout {
        maps: vec![],
        slot_channel: vec![0, 1, 2],
        slot_bits: vec![8, 8, 8],
        color_slots: 3,
        decor_ok: true,
        near: vec![0; 3],
        tables: None,
    };
    let row: Vec<u32> = (0..16).map(|x| (x * 13 + 7) & 0xff).collect();
    let planes = [&row[..], &row[..], &row[..]];
    let cfg = AnalysisConfig {
        predictor_mask: 1 << predict::P_MED,
        decor_mask: 0x0f,
        ..Default::default()
    };
    let mut fast = vec![vec![u32::MAX; 16]; 3];
    let mut search = fast.clone();
    let a = analyze_stripe_impl::<true>(
        &layout,
        &planes,
        16,
        16,
        0,
        1,
        &cfg,
        &mut fast.iter_mut().map(Vec::as_mut_slice).collect::<Vec<_>>(),
        None,
    );
    let b = analyze_stripe_impl::<false>(
        &layout,
        &planes,
        16,
        16,
        0,
        1,
        &cfg,
        &mut search.iter_mut().map(Vec::as_mut_slice).collect::<Vec<_>>(),
        None,
    );
    assert_eq!(a, b);
    assert_eq!(a.0, 0);
    assert_eq!(fast, search);
}
