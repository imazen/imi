use super::coder::{dtilde, energy, nonzero};
use super::transform::*;
use super::*;
use archmage::prelude::*;

fn xorshift(seed: &mut u32) -> u32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 17;
    *seed ^= *seed << 5;
    *seed
}

/// The table-driven nonzero mask equals the zigzag gather.
#[test]
fn nonzero_mask_matches_the_gather() {
    let mut seed = 0x9b05_688cu32;
    for trial in 0..5000 {
        let density = 1 + trial % 9;
        let q: [i32; 64] = core::array::from_fn(|_| {
            let v = xorshift(&mut seed);
            if v % 9 < density as u32 {
                (v >> 8) as i32 % 7 - 3
            } else {
                0
            }
        });
        let mut want = 0u64;
        for (k, &z) in ZIGZAG.iter().enumerate().skip(1) {
            want |= ((q[z as usize] != 0) as u64) << k;
        }
        assert_eq!(nonzero(&q), want, "trial {trial}");
    }
}

/// The fused term and 4x4 candidate kernels equal the scalar
/// references (`dtilde`, `energy`, `layout::quantize`) bit for bit, on
/// the dispatched tier and the scalar instantiation.
#[test]
fn term_kernels_match_the_references() {
    let mut seed = 0x510e_527fu32;
    for trial in 0..2000 {
        let scale = [1.0f32, 30.0, 400.0][trial % 3];
        let c: [f32; 64] = core::array::from_fn(|_| {
            (xorshift(&mut seed) % 20001) as f32 / 10000.0 * scale - scale
        });
        let st: [f32; 64] =
            core::array::from_fn(|i| 0.5 + (xorshift(&mut seed) % 400) as f32 / 10.0 + i as f32);
        let inv = st.map(|v| 1.0 / v);
        let aq = (16 + xorshift(&mut seed) % 160) as f32 / 64.0;
        let round = [0.5f32, 0.38, 0.2][trial / 3 % 3];
        let mut want_q = layout::quantize(&c, &st, aq, round, 0.5);
        want_q[0] = 0;
        let want = (
            dtilde(&c, &want_q, &inv, 1.0 / aq),
            energy(&c, &want_q, &st, aq),
        );
        for tier in 0..2 {
            let mut q4 = [0i32; 64];
            let got = if tier == 0 {
                incant!(
                    cand4_impl(&c, (&inv, &st), (aq, round), &mut q4),
                    [v3, neon, wasm128, scalar]
                )
            } else {
                cand4_impl_scalar(ScalarToken, &c, (&inv, &st), (aq, round), &mut q4)
            };
            q4[0] = 0;
            assert_eq!(q4, want_q, "trial {trial} tier {tier}: units");
            assert_eq!(
                got.0.to_bits(),
                want.0.to_bits(),
                "trial {trial} tier {tier}: dtilde"
            );
            assert_eq!(
                got.1.to_bits(),
                want.1.to_bits(),
                "trial {trial} tier {tier}: energy"
            );
            // The terms of given units (pass 1's 8x8 units).
            let got = if tier == 0 {
                incant!(
                    terms_impl(&c, &want_q, (&inv, &st), aq),
                    [v3, neon, wasm128, scalar]
                )
            } else {
                terms_impl_scalar(ScalarToken, &c, &want_q, (&inv, &st), aq)
            };
            assert_eq!(
                got.0.to_bits(),
                want.0.to_bits(),
                "trial {trial} tier {tier}: terms d"
            );
            assert_eq!(
                got.1.to_bits(),
                want.1.to_bits(),
                "trial {trial} tier {tier}: terms e"
            );
        }
    }
}
