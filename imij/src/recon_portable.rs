//! Portable (magetypes) lossless row reconstruction: one canonical body
//! compiled for the neon, wasm128 and scalar tiers, bit-identical to
//! `recon_row_scalar` in `v1.rs` (and to the x86 `recon::avx2` /
//! `wide::avx2` kernels) for both storage widths.
//!
//! One `recon_row16_impl` / `recon_row32_impl` body per storage width;
//! `NS` slots are reconstructed per call, `P` selects the spatial
//! predictor (with `PAETH32` for coded widths above 14 bits), `NEAR`
//! switches on the per-slot near-lossless bound. Residues are
//! unzigzagged, color decorrelation is inverted, the predictor runs over
//! the 16 lanes of a step as one vector, and the result is stored back to
//! the step-major rows. Callers chunk slots to at most four, as the x86
//! kernels do. The 16-bit path substitutes `PAETH32` when any coded
//! width exceeds 14 bits; the 32-bit path already uses exact Paeth.
//!
//! The body only uses magetypes operations whose semantics match the
//! scalar reference lane for lane: wrapping add/sub/mul, bitwise ops,
//! min/max, lane masks, blends, and shifts. Near-lossless uses the exact
//! saturation trick of the x86 kernels (`mag <= u16::MAX / step` decides
//! whether the product fits, and the clamp makes a saturated product
//! exact).

use crate::predict;
use crate::rans::NL;
use alloc::vec::Vec;
use archmage::prelude::*;

/// Pseudo-predictor id: Paeth with 32-bit intermediates (>14-bit data);
/// mirrors `recon::avx2::PAETH32` and `wide::avx2`'s exact 32-bit Paeth.
pub(crate) const PAETH32: u8 = 15;

// First rows predict from the west irrespective of the stripe predictor.
const FIRST_ROW: u8 = u8::MAX;

/// 16-bit storage reconstruction of one row; `bits` and `near` are each
/// slot's coded width and near-lossless bound (at most four slots).
#[allow(clippy::too_many_arguments)]
#[magetypes(define(u16x16, i16x16, u32x8, i32x8), neon, wasm128, scalar)]
fn recon_row16_impl<const NS: usize, const P: u8, const NEAR: bool>(
    token: Token,
    bits: &[u32],
    near: &[u32],
    color_slots: usize,
    res: &[&[u16]; NS],
    prev: &[&[u16]; NS],
    cur: &mut [&mut [u16]; NS],
    slots: usize,
    widths: &[u32; NL],
    lw: usize,
    decor: u8,
) {
    // Share three- and four-slot bounded Wasm rows. The smaller layouts
    // retain their specialized loops to avoid paying for inactive slots.
    let slots = if NEAR && NS == 4 && cfg!(target_arch = "wasm32") {
        slots
    } else {
        NS
    };
    let zero = u16x16::zero_t(token);
    let uone = u16x16::splat_t(token, 1);
    let active = |s| s < slots;
    let masks: [u16x16; NS] = core::array::from_fn(|s| {
        if active(s) {
            u16x16::splat_t(token, crate::mask_of(bits[s]) as u16)
        } else {
            zero
        }
    });
    let near_on: [bool; NS] = core::array::from_fn(|s| NEAR && active(s) && near[s] != 0);
    let steps: [u16x16; NS] = core::array::from_fn(|s| {
        if active(s) {
            u16x16::splat_t(token, (2 * near[s] + 1) as u16)
        } else {
            zero
        }
    });
    let limits: [u16x16; NS] = core::array::from_fn(|s| {
        if active(s) {
            u16x16::splat_t(token, (u16::MAX as u32 / (2 * near[s] + 1)) as u16)
        } else {
            zero
        }
    });
    let shs: [u32; NS] = core::array::from_fn(|s| if active(s) { 16 - bits[s].max(1) } else { 0 });
    let csh = 16 - bits[0].max(1);
    let do_decor = color_slots == 3 && decor != 0 && NS >= 3;

    let unzz = |u: u16x16, mask: u16x16| -> u16x16 {
        let neg = zero - (u & uone);
        (u.shr_logical_const::<1>() ^ neg) & mask
    };
    let avg = |a: u16x16, b: u16x16| -> u16x16 {
        (a.shr_logical_const::<1>() + b.shr_logical_const::<1>()) + ((a & b) & uone)
    };
    // Paeth for values < 2^14: i16 differences cannot overflow.
    let paeth = |w: u16x16, n: u16x16, nw: u16x16| -> u16x16 {
        let (a, b, c) = (w.bitcast_i16x16(), n.bitcast_i16x16(), nw.bitcast_i16x16());
        let pa = (b - c).abs();
        let pb = (a - c).abs();
        let pc = ((a + b) - (c + c)).abs();
        let a_ok = pa.simd_le(pb) & pa.simd_le(pc);
        let b_ok = pb.simd_le(pc);
        let bc = incant!(crate::wasm_relaxed::sel_i16x16(token, b_ok, b, c) without token);
        incant!(crate::wasm_relaxed::sel_i16x16(token, a_ok, a, bc) without token).bitcast_u16x16()
    };
    // Paeth for full 16-bit values through 32-bit intermediates.
    let paeth32 = |w: u16x16, n: u16x16, nw: u16x16| -> u16x16 {
        let half = |v: u16x16, hi: bool| -> i32x8 {
            (if hi { v.widen_high() } else { v.widen_low() }).bitcast_i32x8()
        };
        let mut out = [i32x8::zero_t(token); 2];
        for (h, o) in out.iter_mut().enumerate() {
            let (a, b, c) = (half(w, h == 1), half(n, h == 1), half(nw, h == 1));
            let pa = (b - c).abs();
            let pb = (a - c).abs();
            let pc = ((a + b) - (c + c)).abs();
            let a_ok = pa.simd_le(pb) & pa.simd_le(pc);
            let b_ok = pb.simd_le(pc);
            let bc = incant!(crate::wasm_relaxed::sel_i32x8(token, b_ok, b, c) without token);
            *o = incant!(crate::wasm_relaxed::sel_i32x8(token, a_ok, a, bc) without token);
        }
        out[0].narrow_saturating_u16(out[1])
    };
    let med = |w: u16x16, n: u16x16, nw: u16x16| -> u16x16 {
        let lo = w.min(n);
        let hi = w.max(n);
        let grad = (w + n) - nw;
        let mid =
            incant!(crate::wasm_relaxed::sel_u16x16(token, nw.simd_le(lo), hi, grad) without token);
        incant!(crate::wasm_relaxed::sel_u16x16(token, nw.simd_ge(hi), lo, mid) without token)
    };
    let predict = |w: u16x16, n: u16x16, nw: u16x16, ne: u16x16, mask: u16x16| -> u16x16 {
        match P {
            predict::P_W => w,
            predict::P_N => n,
            predict::P_AVG_WN => avg(w, n),
            predict::P_MED => med(w, n, nw),
            predict::P_GRAD => ((w + n) - nw) & mask,
            predict::P_PAETH => paeth(w, n, nw),
            PAETH32 => paeth32(w, n, nw),
            predict::P_AVG_WNE => avg(w, ne),
            _ => unreachable!(),
        }
    };
    // `clamp(p + sext(d) * step, 0, mask)`; a product that does not fit
    // 16 bits saturates, which the clamp makes exact.
    let near_add = |p: u16x16, d: u16x16, sh: u32, step: u16x16, lim: u16x16, mask: u16x16| {
        let q = d
            .bitcast_i16x16()
            .shl_uniform(sh)
            .shr_arithmetic_uniform(sh);
        let mag = q.abs().bitcast_u16x16();
        let lo = mag * step;
        let fits = mag.simd_le(lim);
        let sat = incant!(crate::wasm_relaxed::sel_u16x16(token, fits, lo, u16x16::splat_t(token, u16::MAX)) without token);
        let up = p.saturating_add(sat).min(mask);
        let down = p.saturating_sub(sat);
        incant!(crate::wasm_relaxed::sel_u16x16(token, q.simd_lt(i16x16::zero_t(token)).bitcast_u16x16(), down, up) without token)
    };

    let widv = u16x16::from_array_t(
        token,
        core::array::from_fn(|l| widths[l].min(u16::MAX as u32) as u16),
    );
    let mut wv = [zero; NS];
    let mut nwv = [zero; NS];
    for x in 0..lw {
        let bi = x * NL;
        let mut d: [u16x16; NS] = core::array::from_fn(|s| {
            if active(s) {
                let u = u16x16::from_slice_t(token, &res[s][bi..bi + NL]);
                unzz(u, masks[s])
            } else {
                zero
            }
        });
        if do_decor {
            let m = masks[0];
            let g = d[0];
            d[1] = (d[1] + g) & m;
            d[2] = match decor {
                1 => (d[2] + g) & m,
                2 => (d[2] + d[1]) & m,
                _ => {
                    let a = d[1]
                        .bitcast_i16x16()
                        .shl_uniform(csh)
                        .shr_arithmetic_uniform(csh);
                    let b = g
                        .bitcast_i16x16()
                        .shl_uniform(csh)
                        .shr_arithmetic_uniform(csh);
                    let av = (a.shr_arithmetic_const::<1>() + b.shr_arithmetic_const::<1>())
                        + ((a & b) & i16x16::splat_t(token, 1));
                    (d[2] + av.bitcast_u16x16()) & m
                }
            };
        }
        for s in 0..slots {
            let v = if P == FIRST_ROW {
                // Predictor W; first column predicts 0.
                let w = if x > 0 { wv[s] } else { zero };
                if near_on[s] {
                    near_add(w, d[s], shs[s], steps[s], limits[s], masks[s])
                } else {
                    (w + d[s]) & masks[s]
                }
            } else {
                let n = u16x16::from_slice_t(token, &prev[s][bi..bi + NL]);
                let (w, nw) = if x > 0 { (wv[s], nwv[s]) } else { (n, n) };
                let ne = if x + 1 < lw {
                    let ner = u16x16::from_slice_t(token, &prev[s][bi + NL..bi + 2 * NL]);
                    let has = widv.simd_gt(u16x16::splat_t(token, (x + 1) as u16));
                    incant!(crate::wasm_relaxed::sel_u16x16(token, has, ner, n) without token)
                } else {
                    n
                };
                nwv[s] = n;
                let p = predict(w, n, nw, ne, masks[s]);
                if near_on[s] {
                    near_add(p, d[s], shs[s], steps[s], limits[s], masks[s])
                } else {
                    (p + d[s]) & masks[s]
                }
            };
            wv[s] = v;
            v.store((&mut cur[s][bi..bi + NL]).try_into().unwrap());
        }
    }
}

/// 32-bit storage reconstruction of one row (extended tier); at most four
/// slots per call, two 8-lane halves of the 16-lane step layout.
#[allow(clippy::too_many_arguments)]
#[magetypes(define(u32x8, i32x8), neon, wasm128, scalar)]
fn recon_row32_impl<const NS: usize, const P: u8, const NEAR: bool>(
    token: Token,
    bits: &[u32],
    near: &[u32],
    color_slots: usize,
    res: &[&[u32]; NS],
    prev: &[&[u32]; NS],
    cur: &mut [&mut [u32]; NS],
    widths: &[u32; NL],
    lw: usize,
    row: usize,
    decor: u8,
) {
    let zero = u32x8::zero_t(token);
    let one = u32x8::splat_t(token, 1);
    let masks: [u32x8; NS] =
        core::array::from_fn(|s| u32x8::splat_t(token, crate::mask_of(bits[s])));
    let near_on: [bool; NS] = core::array::from_fn(|s| NEAR && near[s] != 0);
    let steps: [u32x8; NS] =
        core::array::from_fn(|s| u32x8::splat_t(token, (2 * near[s] as u64 + 1) as u32));
    let limits: [u32x8; NS] = core::array::from_fn(|s| {
        u32x8::splat_t(token, u32::MAX / ((2 * near[s] as u64 + 1) as u32))
    });
    // Sign bit of each slot's coded value: `q < 0` for the residual.
    let signs: [u32x8; NS] =
        core::array::from_fn(|s| u32x8::splat_t(token, 1u32 << (bits[s].max(1) - 1)));
    let cb = bits[0].max(1);
    let ctop = u32x8::splat_t(token, !((1u32 << (cb - 1)) - 1));
    let do_decor = color_slots == 3 && decor != 0 && NS >= 3;

    let unzz = |u: u32x8, mask: u32x8| -> u32x8 {
        let neg = zero - (u & one);
        (u.shr_logical_const::<1>() ^ neg) & mask
    };
    let avg = |a: u32x8, b: u32x8| -> u32x8 {
        (a.shr_logical_const::<1>() + b.shr_logical_const::<1>()) + ((a & b) & one)
    };
    let le = |a: u32x8, b: u32x8| a.simd_le(b);
    let absdiff = |a: u32x8, b: u32x8| a.max(b) - a.min(b);
    // Exact Paeth without wider intermediates: when `a - c` and `b - c`
    // have opposite signs, `pc = |pa - pb|`; otherwise `pa + pb` (which is
    // never the strict minimum, so an all-ones stand-in is exact).
    let paeth = |a: u32x8, b: u32x8, c: u32x8| -> u32x8 {
        let pa = absdiff(b, c);
        let pb = absdiff(a, c);
        let same = le(c, a) ^ le(c, b).not();
        let pc = absdiff(pa, pb) | same;
        let a_ok = le(pa, pb) & le(pa, pc);
        let b_ok = le(pb, pc);
        let bc = incant!(crate::wasm_relaxed::sel_u32x8(token, b_ok, b, c) without token);
        incant!(crate::wasm_relaxed::sel_u32x8(token, a_ok, a, bc) without token)
    };
    let med = |w: u32x8, n: u32x8, nw: u32x8| -> u32x8 {
        let lo = w.min(n);
        let hi = w.max(n);
        let grad = (w + n) - nw;
        let mid =
            incant!(crate::wasm_relaxed::sel_u32x8(token, nw.simd_le(lo), hi, grad) without token);
        incant!(crate::wasm_relaxed::sel_u32x8(token, nw.simd_ge(hi), lo, mid) without token)
    };
    let predict = |w: u32x8, n: u32x8, nw: u32x8, ne: u32x8, mask: u32x8| -> u32x8 {
        match P {
            predict::P_W => w,
            predict::P_N => n,
            predict::P_AVG_WN => avg(w, n),
            predict::P_MED => med(w, n, nw),
            predict::P_GRAD => ((w + n) - nw) & mask,
            predict::P_PAETH => paeth(w, n, nw),
            predict::P_AVG_WNE => avg(w, ne),
            _ => unreachable!(),
        }
    };
    // `|sext(d)|`: `d` for a non-negative residual, `2^bits - d` for a
    // negative one (the sign bit of the coded value is `signs[s]`).
    let near_add = |p: u32x8, d: u32x8, step: u32x8, lim: u32x8, mask: u32x8, sign: u32x8| {
        let neg = (d & sign).simd_gt(zero);
        let mag =
            incant!(crate::wasm_relaxed::sel_u32x8(token, neg, (mask + one) - d, d) without token);
        let lo = mag * step;
        let fits = mag.simd_le(lim);
        let sat = incant!(crate::wasm_relaxed::sel_u32x8(token, fits, lo, u32x8::splat_t(token, u32::MAX)) without token);
        let sum = p + sat;
        let no_carry = sum.max(p).simd_eq(sum);
        let up = (sum | no_carry.not()).min(mask);
        let down = p.max(sat) - sat;
        incant!(crate::wasm_relaxed::sel_u32x8(token, neg, down, up) without token)
    };

    let wid = [
        u32x8::from_slice_t(token, &widths[..8]),
        u32x8::from_slice_t(token, &widths[8..]),
    ];
    for h in 0..2 {
        let mut wv = [zero; NS];
        let mut nwv = [zero; NS];
        for x in 0..lw {
            let bi = x * NL + 8 * h;
            let mut d: [u32x8; NS] = core::array::from_fn(|s| {
                let u = u32x8::from_slice_t(token, &res[s][bi..bi + 8]);
                unzz(u, masks[s])
            });
            if do_decor {
                let m = masks[0];
                let g = d[0];
                d[1] = (d[1] + g) & m;
                d[2] = match decor {
                    1 => (d[2] + g) & m,
                    2 => (d[2] + d[1]) & m,
                    _ => {
                        // `sra(sext(v), 1)` in the unsigned domain: shift
                        // the sign-extended value right by one, filling in
                        // from the sign bit.
                        let half = |v: u32x8| -> u32x8 {
                            let sign = (v.shr_logical_uniform(cb - 1) & one).simd_eq(one);
                            v.shr_logical_const::<1>()
                                | incant!(crate::wasm_relaxed::sel_u32x8(token, sign, ctop, zero) without token)
                        };
                        let av = half(d[1]) + half(g) + ((d[1] & g) & one);
                        (d[2] + av) & m
                    }
                };
            }
            for s in 0..NS {
                let v = if row == 0 {
                    let w = if x > 0 { wv[s] } else { zero };
                    if near_on[s] {
                        near_add(w, d[s], steps[s], limits[s], masks[s], signs[s])
                    } else {
                        (w + d[s]) & masks[s]
                    }
                } else {
                    let n = u32x8::from_slice_t(token, &prev[s][bi..bi + 8]);
                    let (w, nw) = if x > 0 { (wv[s], nwv[s]) } else { (n, n) };
                    let ne = if x + 1 < lw {
                        let ner = u32x8::from_slice_t(token, &prev[s][bi + NL..bi + NL + 8]);
                        let has = wid[h].simd_gt(u32x8::splat_t(token, (x + 1) as u32));
                        incant!(crate::wasm_relaxed::sel_u32x8(token, has, ner, n) without token)
                    } else {
                        n
                    };
                    nwv[s] = n;
                    let p = predict(w, n, nw, ne, masks[s]);
                    if near_on[s] {
                        near_add(p, d[s], steps[s], limits[s], masks[s], signs[s])
                    } else {
                        (p + d[s]) & masks[s]
                    }
                };
                wv[s] = v;
                v.store((&mut cur[s][bi..bi + 8]).try_into().unwrap());
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
#[cfg_attr(target_arch = "wasm32", inline(never))]
fn go16<const NS: usize, const NEAR: bool>(
    pred: u8,
    bits: &[u32],
    near: &[u32],
    color_slots: usize,
    res: &[Vec<u16>],
    prev: &[Vec<u16>],
    cur: &mut [Vec<u16>],
    widths: &[u32; NL],
    lw: usize,
    row: usize,
    decor: u8,
) {
    let slots = res.len();
    let shared = NEAR && NS == 4 && cfg!(target_arch = "wasm32");
    let r: [&[u16]; NS] = core::array::from_fn(|s| {
        if shared && s >= slots {
            &[]
        } else {
            res[s].as_slice()
        }
    });
    let pv: [&[u16]; NS] = core::array::from_fn(|s| {
        if shared && s >= slots {
            &[]
        } else {
            prev[s].as_slice()
        }
    });
    let mut it = cur.iter_mut();
    let mut c: [&mut [u16]; NS] = core::array::from_fn(|s| {
        if shared && s >= slots {
            &mut []
        } else {
            it.next().unwrap().as_mut_slice()
        }
    });
    macro_rules! arm {
        ($p:expr) => {
            incant!(
                recon_row16_impl::<NS, $p, NEAR>(
                    bits,
                    near,
                    color_slots,
                    &r,
                    &pv,
                    &mut c,
                    slots,
                    widths,
                    lw,
                    decor
                ),
                [neon, wasm128, scalar]
            )
        };
    }
    if row == 0 {
        arm!(FIRST_ROW);
        return;
    }
    match pred {
        predict::P_W => arm!(0),
        predict::P_N => arm!(1),
        predict::P_AVG_WN => arm!(2),
        predict::P_MED => arm!(3),
        predict::P_GRAD => arm!(4),
        predict::P_PAETH => arm!(5),
        predict::P_AVG_WNE => arm!(6),
        _ => arm!(PAETH32),
    }
}

#[allow(clippy::too_many_arguments)]
fn go32<const NS: usize, const NEAR: bool>(
    pred: u8,
    bits: &[u32],
    near: &[u32],
    color_slots: usize,
    res: &[Vec<u32>],
    prev: &[Vec<u32>],
    cur: &mut [Vec<u32>],
    widths: &[u32; NL],
    lw: usize,
    row: usize,
    decor: u8,
) {
    let r: [&[u32]; NS] = core::array::from_fn(|s| res[s].as_slice());
    let pv: [&[u32]; NS] = core::array::from_fn(|s| prev[s].as_slice());
    let mut it = cur.iter_mut();
    let mut c: [&mut [u32]; NS] = core::array::from_fn(|_| it.next().unwrap().as_mut_slice());
    macro_rules! arm {
        ($p:expr) => {
            incant!(
                recon_row32_impl::<NS, $p, NEAR>(
                    bits,
                    near,
                    color_slots,
                    &r,
                    &pv,
                    &mut c,
                    widths,
                    lw,
                    row,
                    decor
                ),
                [neon, wasm128, scalar]
            )
        };
    }
    match pred {
        predict::P_W => arm!(0),
        predict::P_N => arm!(1),
        predict::P_AVG_WN => arm!(2),
        predict::P_MED => arm!(3),
        predict::P_GRAD => arm!(4),
        predict::P_PAETH => arm!(5),
        predict::P_AVG_WNE => arm!(6),
        // Wide storage already uses exact Paeth for P_PAETH. PAETH32 is
        // only a widening choice in go16, never a valid predictor here.
        _ => unreachable!("validated spatial predictor"),
    }
}

/// Reconstructs one row of `res` into `cur` for 16-bit storage, through the
/// best available portable tier. `bits`, `near` and `color_slots` are the
/// layout's per-slot coded widths, near-lossless bounds and color-slot
/// count. The caller guarantees `lw < 32000`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn recon_row16(
    bits: &[u32],
    near: &[u32],
    color_slots: usize,
    res: &[Vec<u16>],
    prev: &[Vec<u16>],
    cur: &mut [Vec<u16>],
    widths: &[u32; NL],
    lw: usize,
    row: usize,
    pred: u8,
    decor: u8,
) {
    let max_bits = bits.iter().copied().max().unwrap_or(0);
    let pred = if pred == predict::P_PAETH && max_bits > 14 {
        PAETH32
    } else {
        pred
    };
    let near_on = cfg!(feature = "near-lossless") && near.iter().any(|&d| d != 0);
    let mut s0 = 0;
    while s0 < res.len() {
        let n = (res.len() - s0).min(4);
        let (b, nr, cs) = (
            &bits[s0..],
            &near[s0..],
            if s0 == 0 { color_slots } else { 0 },
        );
        let (r, pv, c) = (&res[s0..s0 + n], &prev[s0..s0 + n], &mut cur[s0..s0 + n]);
        match (n, near_on) {
            (1, false) => go16::<1, false>(pred, b, nr, cs, r, pv, c, widths, lw, row, decor),
            (2, false) => go16::<2, false>(pred, b, nr, cs, r, pv, c, widths, lw, row, decor),
            (3, false) => go16::<3, false>(pred, b, nr, cs, r, pv, c, widths, lw, row, decor),
            (_, false) => go16::<4, false>(pred, b, nr, cs, r, pv, c, widths, lw, row, decor),
            (1, true) => go16::<1, true>(pred, b, nr, cs, r, pv, c, widths, lw, row, decor),
            #[cfg(target_arch = "wasm32")]
            (2, true) => {
                // Two slots have no residual decorrelation dependency.
                // Run the shared single-slot loops with the same predictor.
                for s in 0..2 {
                    let (r, pv, c) = (&r[s..=s], &pv[s..=s], &mut c[s..=s]);
                    if nr[s] == 0 {
                        go16::<1, false>(pred, &b[s..], &nr[s..], 0, r, pv, c, widths, lw, row, 0);
                    } else {
                        go16::<1, true>(pred, &b[s..], &nr[s..], 0, r, pv, c, widths, lw, row, 0);
                    }
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            (2, true) => go16::<2, true>(pred, b, nr, cs, r, pv, c, widths, lw, row, decor),
            #[cfg(not(target_arch = "wasm32"))]
            (3, true) => go16::<3, true>(pred, b, nr, cs, r, pv, c, widths, lw, row, decor),
            (_, true) => go16::<4, true>(pred, b, nr, cs, r, pv, c, widths, lw, row, decor),
        }
        s0 += n;
    }
}

/// [`recon_row16`] for 32-bit storage (extended tier).
#[allow(clippy::too_many_arguments)]
pub(crate) fn recon_row32(
    bits: &[u32],
    near: &[u32],
    color_slots: usize,
    res: &[Vec<u32>],
    prev: &[Vec<u32>],
    cur: &mut [Vec<u32>],
    widths: &[u32; NL],
    lw: usize,
    row: usize,
    pred: u8,
    decor: u8,
) {
    let near_on = cfg!(feature = "near-lossless") && near.iter().any(|&d| d != 0);
    let mut s0 = 0;
    while s0 < res.len() {
        let n = (res.len() - s0).min(4);
        let (b, nr, cs) = (
            &bits[s0..],
            &near[s0..],
            if s0 == 0 { color_slots } else { 0 },
        );
        let (r, pv, c) = (&res[s0..s0 + n], &prev[s0..s0 + n], &mut cur[s0..s0 + n]);
        match (n, near_on) {
            (1, false) => go32::<1, false>(pred, b, nr, cs, r, pv, c, widths, lw, row, decor),
            (2, false) => go32::<2, false>(pred, b, nr, cs, r, pv, c, widths, lw, row, decor),
            (3, false) => go32::<3, false>(pred, b, nr, cs, r, pv, c, widths, lw, row, decor),
            (_, false) => go32::<4, false>(pred, b, nr, cs, r, pv, c, widths, lw, row, decor),
            (1, true) => go32::<1, true>(pred, b, nr, cs, r, pv, c, widths, lw, row, decor),
            (2, true) => go32::<2, true>(pred, b, nr, cs, r, pv, c, widths, lw, row, decor),
            (3, true) => go32::<3, true>(pred, b, nr, cs, r, pv, c, widths, lw, row, decor),
            (_, true) => go32::<4, true>(pred, b, nr, cs, r, pv, c, widths, lw, row, decor),
        }
        s0 += n;
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::coding_layout::Layout;
    use crate::mapping::{ChannelMap, MapMode};
    use crate::v1::{OutMap, recon_row_scalar};
    use crate::{Channels, ImageInfo, SampleType};
    use alloc::vec;
    use alloc::vec::Vec;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n.max(1)
        }
    }

    const CHANNELS: [Channels; 4] = [
        Channels::Gray,
        Channels::GrayAlpha,
        Channels::Rgb,
        Channels::Rgba,
    ];

    fn layout_for(ch: Channels, bits: &[u32], near: &[u32]) -> Layout {
        let maps = bits
            .iter()
            .map(|&b| ChannelMap {
                mode: MapMode::Lsb,
                bits: b as u8,
                value: 0,
            })
            .collect();
        let layout = Layout::new(ch, maps);
        #[cfg(feature = "near-lossless")]
        let layout = layout.with_near(near);
        #[cfg(not(feature = "near-lossless"))]
        assert!(near.iter().all(|&bound| bound == 0));
        layout
    }

    fn lane_widths(w: usize, lw: usize) -> [u32; NL] {
        core::array::from_fn(|l| {
            let xs = l * lw;
            if xs >= w {
                0
            } else {
                ((xs + lw).min(w) - xs) as u32
            }
        })
    }

    fn run_portable<S: crate::px::Px>(
        layout: &Layout,
        res: &[Vec<S>],
        prev: &[Vec<S>],
        cur: &mut [Vec<S>],
        widths: &[u32; NL],
        lw: usize,
        row: usize,
        pred: u8,
        decor: u8,
    ) {
        match (S::rows16(res), S::rows16(prev), S::rows16_mut(cur)) {
            (Some(r), Some(pv), Some(c)) => recon_row16(
                &layout.slot_bits,
                &layout.near,
                layout.color_slots,
                r,
                pv,
                c,
                widths,
                lw,
                row,
                pred,
                decor,
            ),
            _ => recon_row32(
                &layout.slot_bits,
                &layout.near,
                layout.color_slots,
                S::rows32(res).unwrap(),
                S::rows32(prev).unwrap(),
                S::rows32_mut(cur).unwrap(),
                widths,
                lw,
                row,
                pred,
                decor,
            ),
        }
    }

    /// The portable kernel against the scalar reference for every
    /// predictor, decorrelation mode and row kind, exact and near.
    fn parity_case<S: crate::px::Px>(
        r: &mut Rng,
        ch: Channels,
        sample: SampleType,
        max_bits: u32,
    ) -> usize {
        let nch = ch.count();
        // Equal coded widths for all channels are common, so the
        // decorrelation modes get real coverage.
        let same = r.below(2) == 0;
        let b0 = 1 + r.below(max_bits as u64) as u32;
        let bits: Vec<u32> = (0..nch)
            .map(|_| {
                if same {
                    b0
                } else {
                    1 + r.below(max_bits as u64) as u32
                }
            })
            .collect();
        let near: Vec<u32> = if cfg!(feature = "near-lossless") {
            bits.iter()
                .map(|&b| match r.below(4) {
                    0 | 1 => 0,
                    2 => crate::near::delta_cap(b).min(1),
                    _ => (r.next() as u32) % (crate::near::delta_cap(b) + 1),
                })
                .collect()
        } else {
            vec![0; nch]
        };
        let layout = layout_for(ch, &bits, &near);
        let ns = layout.slot_channel.len();
        let w = 1 + r.below(150) as usize;
        let lanes = 1 + r.below(16) as usize;
        let lw = w.div_ceil(lanes);
        let widths = lane_widths(w, lw);
        let step_row = lw * NL;
        let info = ImageInfo::new(w as u32, 1, ch, sample);
        let om = OutMap::new(&info, &layout);
        let rand_rows = |r: &mut Rng| -> Vec<Vec<S>> {
            (0..ns)
                .map(|s| {
                    let mask = crate::mask_of(layout.slot_bits[s]) as u64;
                    (0..step_row)
                        .map(|_| {
                            let v = match r.below(4) {
                                0 => 0,
                                1 => mask,
                                _ => r.next() & mask,
                            };
                            S::put(v as u32)
                        })
                        .collect()
                })
                .collect()
        };
        let res = rand_rows(r);
        let prev = rand_rows(r);
        let decors: &[u8] = if layout.decor_ok { &[0, 1, 2, 3] } else { &[0] };
        let mut compared = 0;
        for &decor in decors {
            for pred in 0..predict::NUM_PREDICTORS {
                for row in [0usize, 1] {
                    if row == 0 && pred != predict::P_W {
                        continue;
                    }
                    let mut want = vec![vec![S::default(); step_row]; ns];
                    let mut out = vec![0u8; info.row_bytes()];
                    recon_row_scalar(
                        &layout, &res, &prev, &mut want, &widths, lw, row, pred, decor, &om,
                        &mut out,
                    );
                    let mut got = vec![vec![S::default(); step_row]; ns];
                    run_portable(
                        &layout, &res, &prev, &mut got, &widths, lw, row, pred, decor,
                    );
                    compared += 1;
                    for s in 0..ns {
                        for (l, &wl) in widths.iter().enumerate() {
                            for x in 0..wl as usize {
                                let i = x * NL + l;
                                assert_eq!(
                                    got[s][i].get(),
                                    want[s][i].get(),
                                    "{ch:?} bits {bits:?} near {near:?} pred {pred} \
                                     decor {decor} row {row} slot {s} lane {l} x {x}"
                                );
                            }
                        }
                    }
                }
            }
        }
        compared
    }

    #[test]
    fn portable_recon_matches_scalar_u16() {
        let mut r = Rng(0x9e37_79b9_7f4a_7c15);
        let mut n = 0;
        for _ in 0..30 {
            for ch in CHANNELS {
                n += parity_case::<u16>(&mut r, ch, SampleType::U16, 16);
            }
        }
        assert!(n > 1500, "compared {n} rows");
    }

    #[test]
    fn portable_recon_matches_scalar_u32() {
        let mut r = Rng(0xc2b2_ae3d_27d4_eb4f);
        let mut n = 0;
        for _ in 0..30 {
            for ch in CHANNELS {
                n += parity_case::<u32>(&mut r, ch, SampleType::U32, 32);
            }
        }
        assert!(n > 1500, "compared {n} rows");
    }

    /// Every compiled tier arm of both storage widths against the scalar
    /// reference on one mixed case each (PAETH32 substitution, decor 3
    /// widening, near-lossless on one slot).
    #[test]
    fn tier_arms_match_scalar_on_mixed_cases() {
        let widths: [u32; NL] = core::array::from_fn(|l| if l < 7 { 7 } else { 0 });
        let (lw, step_row) = (7usize, 7 * NL);

        let layout = layout_for(Channels::Rgb, &[15, 15, 15], &[0, 0, 0]);
        let mk = |v: &[u32; 3]| -> Vec<Vec<u16>> {
            v.iter()
                .map(|&seed| {
                    (0..step_row)
                        .map(|i| ((seed as usize * 31 + i * 17) % (1 << 15)) as u16)
                        .collect()
                })
                .collect()
        };
        let res = mk(&[1, 2, 3]);
        let prev = mk(&[5, 7, 11]);
        let mut want = vec![vec![0u16; step_row]; 3];
        let info = ImageInfo::new(49, 1, Channels::Rgb, SampleType::U16);
        let om = OutMap::new(&info, &layout);
        let mut out = vec![0u8; info.row_bytes()];
        recon_row_scalar(
            &layout,
            &res,
            &prev,
            &mut want,
            &widths,
            lw,
            1,
            predict::P_PAETH,
            3,
            &om,
            &mut out,
        );
        let bits = &layout.slot_bits[..];
        let near = &layout.near[..];
        let r: [&[u16]; 3] = core::array::from_fn(|s| res[s].as_slice());
        let pv: [&[u16]; 3] = core::array::from_fn(|s| prev[s].as_slice());
        let scalar = |cur: &mut [Vec<u16>]| {
            let mut it = cur.iter_mut();
            let mut c: [&mut [u16]; 3] =
                core::array::from_fn(|_| it.next().unwrap().as_mut_slice());
            recon_row16_impl_scalar::<3, PAETH32, false>(
                archmage::ScalarToken,
                bits,
                near,
                3,
                &r,
                &pv,
                &mut c,
                3,
                &widths,
                lw,
                3,
            );
        };
        let check = |got: &[Vec<u16>]| {
            for s in 0..3 {
                for (l, &wl) in widths.iter().enumerate() {
                    for x in 0..wl as usize {
                        assert_eq!(got[s][x * NL + l], want[s][x * NL + l]);
                    }
                }
            }
        };
        let mut got = vec![vec![0u16; step_row]; 3];
        scalar(&mut got);
        check(&got);
        #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
        {
            use archmage::SimdToken;
            let t = archmage::Wasm128Token::summon().unwrap();
            let mut got = vec![vec![0u16; step_row]; 3];
            let mut it = got.iter_mut();
            let mut c: [&mut [u16]; 3] =
                core::array::from_fn(|_| it.next().unwrap().as_mut_slice());
            recon_row16_impl_wasm128::<3, PAETH32, false>(
                t, bits, near, 3, &r, &pv, &mut c, 3, &widths, lw, 3,
            );
            check(&got);
        }
    }
}
