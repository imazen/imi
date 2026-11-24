//! Portable (magetypes) lossless row decoder: one canonical body compiled
//! for NEON, wasm128, and scalar tiers. Bit-identical to
//! `rans::decode_row_scalar` — same per-lane rANS order, context classes,
//! renorm points, and stream-word consumption order.
//!
//! One `decode_row_impl<const U32: bool>` body covers both storage widths:
//! state/refill/emit logic is shared; only the context-term vector loads
//! (u16x16 widen vs u32x8 load+clamp) and the emit stores differ, and those
//! are gated on the const parameter, so each monomorphization keeps exactly
//! its own storage path. `Px` is implemented only for `u16` and `u32`
//! storage, so `decode_row::<S>` selects the instantiation on
//! `core::mem::size_of::<S>()` — a constant per monomorphization; in
//! observed optimized code the inactive storage path is removed.
//!
//! Vectorized (u32x8 halves of the 16 lanes): width/run/active masks, run
//! countdown, context-term loads and sums, class computation via the
//! f32-exponent trick (exact for sums < 2^24), the rANS state update, and
//! the renorm-need mask. Per-lane scalar work remains for ctx_map/table
//! gathers, extra-bit pulls (wasm128 has no per-lane variable shifts),
//! emit stores, and renorm word loads — matching the NEON lossy kernel's
//! vector-scalar split.

#[cfg(not(all(target_arch = "wasm32", target_feature = "relaxed-simd")))]
use crate::mask_of;
use crate::px::Px;
use crate::rans::{DecModel, LOG_M, LaneState, M, NCTX, NL, RANS_L, Words};
use archmage::prelude::*;

// WASM has byte swizzles rather than integer gathers. Compact stream words
// into the lanes needing refill, in the original low-to-high lane order.
#[cfg(target_arch = "wasm32")]
const REFILL_SHUFFLES: [[u8; 16]; 256] = {
    let mut table = [[128; 16]; 256];
    let mut mask = 0;
    while mask < 256 {
        let mut rank = 0;
        let mut lane = 0;
        while lane < 8 {
            if mask & (1 << lane) != 0 {
                table[mask][2 * lane] = 2 * rank;
                table[mask][2 * lane + 1] = 2 * rank + 1;
                rank += 1;
            }
            lane += 1;
        }
        mask += 1;
    }
    table
};

#[cfg(target_arch = "wasm32")]
#[inline(always)]
fn refill_wasm128(
    token: archmage::Wasm128Token,
    value: magetypes::simd::generic::u32x8<archmage::Wasm128Token>,
    need: magetypes::simd::generic::u32x8<archmage::Wasm128Token>,
    words: &Words<'_>,
    wp: &mut usize,
) -> magetypes::simd::generic::u32x8<archmage::Wasm128Token> {
    use core::arch::wasm32::*;
    use magetypes::simd::generic::{u8x16, u32x8};
    let mask = need.bitmask();
    if mask == 0 {
        return value;
    }
    let input = u8x16::load_t(token, words.chunk(*wp)[..16].try_into().unwrap()).into_repr();
    let indices = u8x16::load_t(token, &REFILL_SHUFFLES[mask as usize & 255]).into_repr();
    let packed = i8x16_swizzle(input, indices);
    let refill = u32x8::from_repr_t(
        token,
        [
            u32x4_extend_low_u16x8(packed),
            u32x4_extend_high_u16x8(packed),
        ],
    );
    *wp += mask.count_ones() as usize;
    u32x8::blend(need, value.shl_const::<16>() | refill, value)
}

#[cfg(target_arch = "wasm32")]
#[inline(always)]
fn refill_scalar(
    token: archmage::ScalarToken,
    value: magetypes::simd::generic::u32x8<archmage::ScalarToken>,
    need: magetypes::simd::generic::u32x8<archmage::ScalarToken>,
    words: &Words<'_>,
    wp: &mut usize,
) -> magetypes::simd::generic::u32x8<archmage::ScalarToken> {
    use magetypes::simd::generic::u32x8;
    let mut out = value.to_array();
    let mut mask = need.bitmask();
    while mask != 0 {
        let lane = mask.trailing_zeros() as usize;
        mask &= mask - 1;
        out[lane] = (out[lane] << 16) | words.word(*wp) as u32;
        *wp += 1;
    }
    u32x8::from_array_t(token, out)
}

// WASM has no per-lane integer shifts. Extract each of the eight fixed lanes
// once, then reassemble vectors so extra-bit refills still use ordered swizzles.
// Callers bound each width to 0..=16, including the second half of wide tokens.
// Retain the original strict-package path: cold 4K worker tests regressed there;
// the measured gains apply to relaxed packages. This adds no wire-mode choice.
#[cfg(all(target_arch = "wasm32", target_feature = "relaxed-simd"))]
#[magetypes(define(u32x8), wasm128, scalar)]
fn pull_integer(
    token: Token,
    value: magetypes::simd::generic::u32x8<Token>,
    widths: magetypes::simd::generic::u32x8<Token>,
) -> (
    magetypes::simd::generic::u32x8<Token>,
    magetypes::simd::generic::u32x8<Token>,
) {
    let values = value.to_array();
    let widths = widths.to_array();
    let bits = core::array::from_fn(|i| values[i] & (1u32.wrapping_shl(widths[i]).wrapping_sub(1)));
    let shifted = core::array::from_fn(|i| values[i].wrapping_shr(widths[i]));
    (
        u32x8::from_array_t(token, shifted),
        u32x8::from_array_t(token, bits),
    )
}

#[allow(clippy::too_many_arguments)]
#[magetypes(define(u32x8, i32x8, u16x16), neon, wasm128, scalar)]
fn decode_row_impl<const U32: bool>(
    token: Token,
    md: &DecModel,
    words: &Words<'_>,
    st: &mut LaneState,
    widths: &[u32; NL],
    lw: usize,
    row: usize,
    // Storage views: exactly one pair is real (non-empty); the other is
    // `&[]`/`&mut []` passed by the caller and never touched because every
    // use sits behind `if U32` / `if !U32`.
    prev16: &[u16],
    cross16: Option<&[u16]>,
    out16: &mut [u16],
    prev32: &[u32],
    cross32: Option<&[u32]>,
    out32: &mut [u32],
) {
    let lim = u32x8::splat_t(token, RANS_L);
    let zero = u32x8::zero_t(token);
    let one = u32x8::splat_t(token, 1);
    let m10 = u32x8::splat_t(token, M - 1);
    let c126 = i32x8::splat_t(token, 126);
    let czero = i32x8::zero_t(token);
    let c15 = i32x8::splat_t(token, 15);
    let c16 = u32x8::splat_t(token, 1 << 16);
    let sh = md.ctx_shift;
    let wv = [
        u32x8::from_slice_t(token, &widths[..8]),
        u32x8::from_slice_t(token, &widths[8..]),
    ];

    #[cfg(not(all(target_arch = "wasm32", target_feature = "relaxed-simd")))]
    let mut sym_l = [0u32; NL];
    #[cfg(not(all(target_arch = "wasm32", target_feature = "relaxed-simd")))]
    let mut extra = [0u32; NL];
    #[cfg(all(target_arch = "wasm32", target_feature = "relaxed-simd"))]
    let (mut bases, mut bit_widths, mut runs) = ([0u32; NL], [0u32; NL], [0u32; NL]);
    let mut e_arr = [0u32; NL];
    let mut cls_arr = [0i32; NL];

    let mut wp = st.wp;
    for px in 0..lw {
        let pxv = u32x8::splat_t(token, px as u32);
        let xv = [
            u32x8::from_slice_t(token, &st.x[..8]),
            u32x8::from_slice_t(token, &st.x[8..]),
        ];
        #[cfg(all(target_arch = "wasm32", target_feature = "relaxed-simd"))]
        let mut xv = xv;
        let mut amask = 0u32;
        let mut act = [zero, zero];
        for h in 0..2 {
            let rv = u32x8::from_slice_t(token, &st.run[8 * h..8 * h + 8]);
            let inrun = rv.simd_gt(zero);
            let inw = pxv.simd_lt(wv[h]);
            // Width is checked before the run gate: lanes past their width
            // keep run[l] untouched, so only in-width lanes decrement.
            (rv + (inw & inrun)).store((&mut st.run[8 * h..8 * h + 8]).try_into().unwrap());
            act[h] = inw & inrun.not();
            amask |= act[h].bitmask() << (8 * h);
        }

        // No active entropy lanes: run counters were already decremented in
        // width, and every output is zero. Preserve states and stream position.
        #[cfg(all(target_arch = "wasm32", target_feature = "relaxed-simd"))]
        if amask == 0 {
            if U32 {
                out32[px * NL..px * NL + NL].fill(0);
            } else {
                out16[px * NL..px * NL + NL].fill(0);
            }
            continue;
        }

        // Context sums: (x>0,row>0) -> uw+un; edges double the live term.
        let ka = u32x8::splat_t(
            token,
            if px > 0 { 1 } else { 0 } + if row == 0 && px > 0 { 1 } else { 0 },
        );
        let kb = u32x8::splat_t(
            token,
            if row > 0 { 1 } else { 0 } + if px == 0 && row > 0 { 1 } else { 0 },
        );
        // u16 storage: materialize all three terms once per pixel as a
        // widened u16x16 (values < 2^16, no clamp needed). u32 storage
        // loads per half below with the model shift and 2^16 clamp.
        let mut uw16 = u16x16::zero_t(token);
        let mut un16 = u16x16::zero_t(token);
        let mut cv16 = u16x16::zero_t(token);
        if !U32 {
            if px > 0 {
                uw16 = u16x16::from_slice_t(token, &out16[(px - 1) * NL..px * NL])
                    .shr_logical_uniform(sh);
            }
            if row > 0 {
                un16 = u16x16::from_slice_t(token, &prev16[px * NL..px * NL + NL])
                    .shr_logical_uniform(sh);
            }
            if let Some(c) = cross16 {
                cv16 =
                    u16x16::from_slice_t(token, &c[px * NL..px * NL + NL]).shr_logical_uniform(sh);
            }
        }
        let term32 = |v: u32x8| v.shr_logical_uniform(sh).min(c16);
        for h in 0..2 {
            let at = px * NL + 8 * h;
            let (uw, un, cv) = if U32 {
                (
                    if px > 0 {
                        term32(u32x8::from_slice_t(token, &out32[at - NL..at]))
                    } else {
                        zero
                    },
                    if row > 0 {
                        term32(u32x8::from_slice_t(token, &prev32[at..at + 8]))
                    } else {
                        zero
                    },
                    if let Some(c) = cross32 {
                        term32(u32x8::from_slice_t(token, &c[at..at + 8]))
                    } else {
                        zero
                    },
                )
            } else {
                (
                    if h == 0 {
                        uw16.widen_low()
                    } else {
                        uw16.widen_high()
                    },
                    if h == 0 {
                        un16.widen_low()
                    } else {
                        un16.widen_high()
                    },
                    if h == 0 {
                        cv16.widen_low()
                    } else {
                        cv16.widen_high()
                    },
                )
            };
            let sum = uw * ka + un * kb + cv;
            // class = min(bit_length(sum), 15): the f32 exponent of an
            // integer < 2^24 converts exactly, so field-126 = bit_length.
            let e = sum
                .bitcast_i32x8()
                .to_f32x8()
                .bitcast_i32x8()
                .shr_logical_const::<23>()
                - c126;
            cls_arr[8 * h..8 * h + 8].copy_from_slice(&e.clamp(czero, c15).to_array());
        }

        // Scalar gather: ctx_map + decode table per active lane; dead lanes
        // emit 0 now (exactly the scalar pass-1 behavior).
        for l in 0..NL {
            if amask & (1 << l) == 0 {
                // Relaxed SIMD emits every lane in the vector output pass.
                #[cfg(not(all(target_arch = "wasm32", target_feature = "relaxed-simd")))]
                if U32 {
                    out32[px * NL + l] = 0;
                } else {
                    out16[px * NL + l] = 0;
                }
                continue;
            }
            let ctx = md.ctx_map[cls_arr[l] as usize] as usize & (NCTX - 1);
            e_arr[l] = md.dec[(ctx << LOG_M) | (st.x[l] & (M - 1)) as usize];
            let symbol = ((e_arr[l] >> 20) & 255) as usize;
            #[cfg(not(all(target_arch = "wasm32", target_feature = "relaxed-simd")))]
            {
                sym_l[l] = symbol as u32;
                extra[l] = 0;
            }
            #[cfg(all(target_arch = "wasm32", target_feature = "relaxed-simd"))]
            {
                let (base, bits, run) = md.tok[symbol];
                bases[l] = base;
                bit_widths[l] = bits;
                runs[l] = if run { u32::MAX } else { 0 };
            }
        }

        // rANS update + renorm-need, then ordered word refill.
        for h in 0..2 {
            let ev = u32x8::from_slice_t(token, &e_arr[8 * h..8 * h + 8]);
            let nx = ((ev & m10) + one) * xv[h].shr_logical_const::<10>() // LOG_M
                + (ev.shr_logical_const::<10>() & m10);
            // `act` is a comparison mask (lane-uniform), so the relaxed
            // lane-select is bit-identical; `wasm_relaxed` falls back to
            // `blend` on every strict build and tier.
            let xx =
                incant!(crate::wasm_relaxed::sel_u32x8(token, act[h], nx, xv[h]) without token);
            #[cfg(target_arch = "wasm32")]
            {
                let filled = incant!(refill(token, xx, act[h] & xx.simd_lt(lim), words, &mut wp) without token);
                #[cfg(target_feature = "relaxed-simd")]
                {
                    xv[h] = filled;
                }
                #[cfg(not(target_feature = "relaxed-simd"))]
                filled.store((&mut st.x[8 * h..8 * h + 8]).try_into().unwrap());
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let need = (act[h] & xx.simd_lt(lim)).bitmask();
                xx.store((&mut st.x[8 * h..8 * h + 8]).try_into().unwrap());
                let mut m = need;
                let mut r = 0usize;
                while m != 0 {
                    let i = m.trailing_zeros() as usize;
                    st.x[8 * h + i] = (st.x[8 * h + i] << 16) | words.word(wp + r) as u32;
                    r += 1;
                    m &= m - 1;
                }
                wp += r;
            }
        }

        #[cfg(all(target_arch = "wasm32", target_feature = "relaxed-simd"))]
        {
            let bit_widths = [
                u32x8::from_slice_t(token, &bit_widths[..8]),
                u32x8::from_slice_t(token, &bit_widths[8..]),
            ];
            let sixteen = u32x8::splat_t(token, 16);
            let mut extras = [zero; 2];
            // Direct tokens need neither extra-bit shifts nor refills. Keep
            // emission shared, but avoid executing sixteen zero-width pulls.
            let any_bits = ((bit_widths[0] & act[0]) | (bit_widths[1] & act[1])).simd_gt(zero);
            if any_bits.bitmask() != 0 {
                for h in 0..2 {
                    let n = bit_widths[h].min(sixteen);
                    let mask = act[h] & n.simd_gt(zero);
                    let (shifted, bits) = incant!(pull_integer(token, xv[h], n) without token);
                    let shifted = u32x8::blend(mask, shifted, xv[h]);
                    xv[h] = incant!(refill(token, shifted, mask & shifted.simd_lt(lim), words, &mut wp) without token);
                    extras[h] = bits & mask;
                }
                let high = [
                    act[0] & bit_widths[0].simd_gt(sixteen),
                    act[1] & bit_widths[1].simd_gt(sixteen),
                ];
                if (high[0] | high[1]).bitmask() != 0 {
                    for h in 0..2 {
                        let n = (bit_widths[h] - sixteen) & high[h];
                        let (shifted, bits) = incant!(pull_integer(token, xv[h], n) without token);
                        let shifted = u32x8::blend(high[h], shifted, xv[h]);
                        xv[h] = incant!(refill(token, shifted, high[h] & shifted.simd_lt(lim), words, &mut wp) without token);
                        extras[h] |= bits.shl_const::<16>();
                    }
                }
            }
            for h in 0..2 {
                xv[h].store((&mut st.x[8 * h..8 * h + 8]).try_into().unwrap());
                let base = u32x8::from_slice_t(token, &bases[8 * h..8 * h + 8]);
                let run_mask = act[h] & u32x8::from_slice_t(token, &runs[8 * h..8 * h + 8]);
                let value = base + extras[h];
                let previous = u32x8::from_slice_t(token, &st.run[8 * h..8 * h + 8]);
                u32x8::blend(run_mask, value, previous)
                    .store((&mut st.run[8 * h..8 * h + 8]).try_into().unwrap());
                let output = value & act[h] & run_mask.not();
                let at = px * NL + 8 * h;
                if U32 {
                    output.store((&mut out32[at..at + 8]).try_into().unwrap());
                } else {
                    out16[at..at + 8].copy_from_slice(&output.to_array().map(|v| v as u16));
                }
            }
        }
        #[cfg(not(all(target_arch = "wasm32", target_feature = "relaxed-simd")))]
        {
            // Extra-bit pulls, low half (nb <= 16) — per-lane scalar shifts.
            let mut m = amask;
            while m != 0 {
                let l = m.trailing_zeros() as usize;
                m &= m - 1;
                let nb = md.tok[sym_l[l] as usize].1.min(16);
                if nb > 0 {
                    extra[l] = st.x[l] & mask_of(nb);
                    st.x[l] >>= nb;
                    if st.x[l] < RANS_L {
                        st.x[l] = (st.x[l] << 16) | words.word(wp) as u32;
                        wp += 1;
                    }
                }
            }
            // High half (nb > 16) + emit.
            let mut m = amask;
            while m != 0 {
                let l = m.trailing_zeros() as usize;
                m &= m - 1;
                let nb = md.tok[sym_l[l] as usize].1;
                if nb > 16 {
                    let n2 = nb - 16;
                    extra[l] |= (st.x[l] & mask_of(n2)) << 16;
                    st.x[l] >>= n2;
                    if st.x[l] < RANS_L {
                        st.x[l] = (st.x[l] << 16) | words.word(wp) as u32;
                        wp += 1;
                    }
                }
                let (base, _, is_run) = md.tok[sym_l[l] as usize];
                let u = base.wrapping_add(extra[l]);
                if is_run {
                    st.run[l] = u;
                    if U32 {
                        out32[px * NL + l] = 0;
                    } else {
                        out16[px * NL + l] = 0;
                    }
                } else if U32 {
                    out32[px * NL + l] = u;
                } else {
                    out16[px * NL + l] = u as u16;
                }
            }
        }
    }
    st.wp = wp;
}

/// Decode one row through the best available portable tier.
///
/// `Px` is implemented only for `u16` and `u32` storage; dispatch is on
/// `size_of::<S>()`, constant per monomorphization, so each instantiation
/// calls exactly one `incant!` arm (neon / wasm128 / scalar tiers inside
/// `decode_row_impl`). In observed optimized code the inactive storage
/// path is removed from each monomorph.
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_row<S: Px>(
    md: &DecModel,
    words: &Words<'_>,
    st: &mut LaneState,
    widths: &[u32; NL],
    lw: usize,
    row: usize,
    prev: &[S],
    cross: Option<&[S]>,
    out: &mut [S],
) {
    if core::mem::size_of::<S>() == 2 {
        let prev16 = S::slice16(prev).unwrap_or(&[]);
        let out16 = S::slice16_mut(out).unwrap_or(&mut []);
        let cross16 = cross.and_then(|c| S::slice16(c));
        incant!(
            decode_row_impl::<false>(
                md,
                words,
                st,
                widths,
                lw,
                row,
                prev16,
                cross16,
                out16,
                &[],
                None,
                &mut []
            ),
            [neon, wasm128, scalar]
        );
    } else if core::mem::size_of::<S>() == 4 {
        let prev32 = S::slice32(prev).unwrap_or(&[]);
        let out32 = S::slice32_mut(out).unwrap_or(&mut []);
        let cross32 = cross.and_then(|c| S::slice32(c));
        incant!(
            decode_row_impl::<true>(
                md,
                words,
                st,
                widths,
                lw,
                row,
                &[],
                None,
                &mut [],
                prev32,
                cross32,
                out32
            ),
            [neon, wasm128, scalar]
        );
    }
}

#[cfg(all(all(test, feature = "encode"), target_arch = "wasm32"))]
mod refill_tests {
    use super::*;
    use archmage::SimdToken;
    use magetypes::simd::generic::u32x8;

    #[cfg(target_feature = "relaxed-simd")]
    #[test]
    fn wasm_integer_pulls_match_exact_division_for_all_widths() {
        let token = archmage::Wasm128Token::summon().expect("requires SIMD128 test target");
        let edges = [
            0,
            1,
            65535,
            65536,
            (1 << 24) - 1,
            (1 << 24) + 1,
            1 << 31,
            u32::MAX,
        ];
        for offset in 0..17 {
            for rotation in 0..edges.len() {
                let values = core::array::from_fn(|i| edges[(i + rotation) % edges.len()]);
                let widths = core::array::from_fn(|i| ((offset + i * 3) % 17) as u32);
                let (shifted, bits) = pull_integer_wasm128(
                    token,
                    u32x8::from_array_t(token, values),
                    u32x8::from_array_t(token, widths),
                );
                let expected_shift =
                    core::array::from_fn(|i| (u64::from(values[i]) / (1u64 << widths[i])) as u32);
                let expected_bits =
                    core::array::from_fn(|i| (u64::from(values[i]) % (1u64 << widths[i])) as u32);
                assert_eq!(shifted.to_array(), expected_shift);
                assert_eq!(bits.to_array(), expected_bits);
            }
        }
    }

    #[test]
    fn wasm_refills_match_ordered_words_for_every_mask_and_tail() {
        let token = archmage::Wasm128Token::summon().expect("requires SIMD128 test target");
        for len in [0, 1, 2, 7, 8, 15, 16, 17, 63, 64] {
            let body: alloc::vec::Vec<_> = (0..2 * len).map(|i| (i * 37 + 19) as u8).collect();
            let words = Words::from_body(&body).unwrap();
            for start in 0..len + 80 {
                for mask in 0..256u32 {
                    let values = [0, 1, 65535, 65536, u32::MAX, 719, 1441, 16385];
                    let mut expected = values;
                    let mut expected_wp = start;
                    for (lane, value) in expected.iter_mut().enumerate() {
                        if mask & (1 << lane) != 0 {
                            *value = (*value << 16) | words.word(expected_wp) as u32;
                            expected_wp += 1;
                        }
                    }
                    let need =
                        core::array::from_fn(
                            |lane| if mask & (1 << lane) != 0 { u32::MAX } else { 0 },
                        );
                    let mut actual_wp = start;
                    let actual = refill_wasm128(
                        token,
                        u32x8::from_array_t(token, values),
                        u32x8::from_array_t(token, need),
                        &words,
                        &mut actual_wp,
                    )
                    .to_array();
                    assert_eq!(actual, expected, "len={len} start={start} mask={mask}");
                    assert_eq!(actual_wp, expected_wp);
                }
            }
        }
    }
}

#[cfg(all(test, feature = "encode"))]
mod row_state_tests {
    use super::*;
    use crate::rans::{CLASSES, TABLE, decode_row_scalar};
    use alloc::{boxed::Box, vec};

    fn next(seed: &mut u64) -> u32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed as u32
    }

    fn compare<S: Px>(
        seed: &mut u64,
        width: usize,
        row: usize,
        cross: bool,
        shift: u32,
        all_runs: bool,
        zero_bits: bool,
    ) {
        let dec: Box<[u32; TABLE]> = (0..TABLE)
            .map(|i| {
                let context = i >> LOG_M;
                let slot = i & (M as usize - 1);
                let symbol = ((slot >> 7) + context) % 8;
                127 | ((slot as u32 & 127) << 10) | ((symbol as u32) << 20)
            })
            .collect::<alloc::vec::Vec<_>>()
            .into_boxed_slice()
            .try_into()
            .unwrap();
        let mut model = DecModel {
            dec,
            tok: vec![
                (1, 0, false),
                (2, 1, false),
                (32, 5, false),
                (65536, 17, false),
                (u32::MAX, 31, false),
                (u32::MAX, 32, false),
                (0, 3, true),
                (3, 0, true),
            ],
            ctx_shift: shift,
            ctx_map: core::array::from_fn::<_, CLASSES, _>(|i| (i % NCTX) as u8),
            k: 0,
            kr: 0,
            m: 0,
            vt: 0,
            cross,
        };
        if zero_bits {
            for token in &mut model.tok {
                token.1 = 0;
            }
        }
        let body: alloc::vec::Vec<_> = (0..512).map(|_| next(seed) as u8).collect();
        let words = Words::from_body(&body).unwrap();
        let widths = core::array::from_fn(|_| (next(seed) as usize % (width + 1)) as u32);
        let initial_x = core::array::from_fn(|_| next(seed) | RANS_L);
        let initial_run =
            core::array::from_fn(|_| if all_runs { u32::MAX } else { next(seed) % 9 });
        let start = (next(seed) % 320) as usize;
        let mut expected = LaneState {
            x: initial_x,
            run: initial_run,
            wp: start,
        };
        let mut actual = LaneState {
            x: initial_x,
            run: initial_run,
            wp: start,
        };
        let prev: alloc::vec::Vec<_> = (0..width * NL).map(|_| S::put(next(seed))).collect();
        let cross_values: alloc::vec::Vec<_> =
            (0..width * NL).map(|_| S::put(next(seed))).collect();
        let cross_values = cross.then_some(cross_values.as_slice());
        let mut a = vec![S::put(77); width * NL];
        let mut b = a.clone();
        decode_row_scalar(
            &model,
            &words,
            &mut expected,
            &widths,
            width,
            row,
            &prev,
            cross_values,
            &mut a,
        );
        decode_row(
            &model,
            &words,
            &mut actual,
            &widths,
            width,
            row,
            &prev,
            cross_values,
            &mut b,
        );
        assert!(
            a.iter().zip(&b).all(|(a, b)| a.get() == b.get()),
            "output width={width} row={row} cross={cross} shift={shift}"
        );
        assert_eq!(actual.x, expected.x);
        assert_eq!(actual.run, expected.run);
        assert_eq!(actual.wp, expected.wp);
    }

    #[test]
    fn row_local_state_matches_scalar_with_runs_widths_cross_and_padded_words() {
        let mut seed = 0x8b2a_1934_74e1_7265;
        for _ in 0..12 {
            for width in [0, 1, 2, 17, 64] {
                for row in [0, 3] {
                    for cross in [false, true] {
                        for shift in [0, 3, 15] {
                            compare::<u16>(&mut seed, width, row, cross, shift, false, false);
                            compare::<u32>(&mut seed, width, row, cross, shift, false, false);
                        }
                    }
                }
            }
        }
        for width in [1, 17, 64] {
            for row in [0, 3] {
                for cross in [false, true] {
                    for shift in [16, 23, 31] {
                        compare::<u32>(&mut seed, width, row, cross, shift, false, false);
                    }
                }
            }
        }
        for width in [1, 2, 17, 64] {
            for row in [0, 3] {
                for cross in [false, true] {
                    compare::<u16>(&mut seed, width, row, cross, 0, true, false);
                    compare::<u32>(&mut seed, width, row, cross, 15, true, false);
                }
            }
        }
    }
    #[test]
    fn zero_extra_rows_match_scalar_with_mixed_runs() {
        let mut seed = 0x913b_742a_1cc2_1d85;
        for width in [1, 2, 17, 64] {
            for row in [0, 3] {
                for cross in [false, true] {
                    for shift in [0, 15] {
                        compare::<u16>(&mut seed, width, row, cross, shift, false, true);
                        compare::<u32>(&mut seed, width, row, cross, shift, false, true);
                    }
                }
            }
        }
    }
}
