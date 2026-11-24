//! Independent wide-integer formulas against scalar and actual SIMD predictors.

fn reference(p: u8, w: u32, n: u32, nw: u32, ne: u32, mask: u32) -> u32 {
    let (w, n, nw, ne) = (w as i64, n as i64, nw as i64, ne as i64);
    let plane = w + n - nw;
    let v = match p {
        0 => w,
        1 => n,
        2 => (w + n) / 2,
        3 => plane.clamp(w.min(n), w.max(n)),
        4 => plane,
        5 => {
            let distances = [(plane - w).abs(), (plane - n).abs(), (plane - nw).abs()];
            let best = distances
                .iter()
                .enumerate()
                .min_by_key(|&(i, d)| (*d, i))
                .unwrap()
                .0;
            [w, n, nw][best]
        }
        6 => (w + ne) / 2,
        _ => unreachable!(),
    };
    (v as u32) & mask
}

fn batch(bits: u32, w: [u16; 16], n: [u16; 16], nw: [u16; 16], ne: [u16; 16]) {
    let mask = crate::mask_of(bits);
    let expected: [[u16; 16]; 7] = core::array::from_fn(|p| {
        core::array::from_fn(|i| {
            reference(
                p as u8,
                w[i] as u32,
                n[i] as u32,
                nw[i] as u32,
                ne[i] as u32,
                mask,
            ) as u16
        })
    });
    for (p, row) in expected.iter().enumerate() {
        for i in 0..16 {
            assert_eq!(
                super::predict(
                    p as u8,
                    w[i] as u32,
                    n[i] as u32,
                    nw[i] as u32,
                    ne[i] as u32,
                    mask
                ),
                row[i] as u32,
                "scalar p{p}/{bits} ({},{},{},{})",
                w[i],
                n[i],
                nw[i],
                ne[i]
            );
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        use archmage::SimdToken;
        if let Some(t) = archmage::X64V3Token::summon() {
            assert_eq!(
                crate::recon::avx2::predictor_parity_batch(t, &w, &n, &nw, &ne, bits),
                expected,
                "AVX2 {bits} ({w:?},{n:?},{nw:?},{ne:?})"
            );
        }
    }
}

#[test]
fn predictor_boundaries() {
    for bits in 1..=16 {
        let mask = crate::mask_of(bits) as u16;
        let values = [0, 1, mask / 2, mask / 2 + 1, mask.saturating_sub(1), mask];
        for &a in &values {
            for &b in &values {
                for &c in &values {
                    batch(
                        bits,
                        [a; 16],
                        [b; 16],
                        [c; 16],
                        core::array::from_fn(|i| values[i % values.len()]),
                    );
                }
            }
        }
    }
    // Extended predictors use 32-bit modular samples and wide Paeth math.
    for bits in 17..=32 {
        let mask = crate::mask_of(bits);
        let values = [0, 1, mask / 2, mask / 2 + 1, mask - 1, mask];
        for &w in &values {
            for &n in &values {
                for &nw in &values {
                    for p in 0..7 {
                        assert_eq!(
                            super::predict(p, w, n, nw, n ^ mask, mask),
                            reference(p, w, n, nw, n ^ mask, mask)
                        );
                    }
                }
            }
        }
    }
}

#[test]
#[ignore = "explicit exhaustive gate: all 256^3 U8 neighbor triples, seven predictors"]
fn predictor_exhaustive_u8() {
    for w in 0..256 {
        for n in 0..256 {
            for start in (0..256).step_by(16) {
                let nw = core::array::from_fn(|i| start + i as u16);
                let ne = nw.map(|v| v ^ 0x55);
                batch(8, [w; 16], [n; 16], nw, ne);
            }
        }
    }
}

#[test]
#[ignore = "explicit full-value gate: every U16 value with boundary neighbors"]
fn predictor_all_u16_values() {
    for bits in [14, 15, 16] {
        let mask = crate::mask_of(bits) as u16;
        let anchors = [0, 1, mask / 2, mask / 2 + 1, mask - 1, mask];
        for &n in &anchors {
            for &nw in &anchors {
                for start in (0..=mask as usize).step_by(16) {
                    let w = core::array::from_fn(|i| (start + i) as u16);
                    batch(bits, w, [n; 16], [nw; 16], w.map(|v| v ^ mask));
                }
            }
        }
    }
}
