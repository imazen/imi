//! aarch64 NEON row decoder: [`rans::decode_row_scalar`] bit-identical,
//! structured like `rans::avx2` with the 16 lanes carried as four
//! `u32x4` quarters. Lane state (`x`, `run`) and the west context term
//! stay in registers across the row; per-lane scalar work remains only
//! for the decode-table gather. Refill and pull idioms match
//! `lossy::entropy_neon`.

use crate::rans::{DecModel, LaneState, NL, TABLE, Words};
use archmage::prelude::*;

/// Refill permutation per 4-lane need mask: byte indices moving the r-th
/// loaded word into each needing lane (r = needing lanes below it). Same
/// table as `lossy::entropy_neon::PERM4`.
static PERM4: [[u8; 16]; 16] = {
    let mut t = [[0u8; 16]; 16];
    let mut m = 0;
    while m < 16 {
        let mut r = 0;
        let mut i = 0;
        while i < 4 {
            let src = if m & (1 << i) != 0 {
                r += 1;
                r - 1
            } else {
                0
            };
            let mut b = 0;
            while b < 4 {
                t[m][i * 4 + b] = (src * 4 + b) as u8;
                b += 1;
            }
            i += 1;
        }
        m += 1;
    }
    t
};

/// Per-row model constants, in registers.
struct K {
    cmapv: uint8x16_t,
    vt: uint32x4_t,
    vtm1: uint32x4_t,
    dlim_v: uint32x4_t,
    dlim_r: uint32x4_t,
    mpow: uint32x4_t,
    mmask: uint32x4_t,
}

/// One row decode in flight: lane state and west terms as four quarters.
struct Inst<'a, 'w> {
    k: K,
    dec: &'a [u32; TABLE],
    w: &'a Words<'w>,
    prev: &'a [[u16; NL]],
    cross: Option<&'a [[u16; NL]]>,
    x: [uint32x4_t; 4],
    run: [uint32x4_t; 4],
    west: [uint32x4_t; 4],
    wp: usize,
    xi: usize,
    row: usize,
}

#[rite(import_intrinsics)]
#[inline(always)]
fn ld4(_t: NeonToken, a: &[u32; 4]) -> uint32x4_t {
    vld1q_u32(a)
}

#[rite(import_intrinsics)]
#[inline(always)]
fn st4(_t: NeonToken, a: &mut [u32; 4], v: uint32x4_t) {
    vst1q_u32(a, v);
}

/// Loads all 16 u16 values of a step into four widened `uint32x4_t` quarters.
#[rite(import_intrinsics)]
#[inline(always)]
fn ld16_all(_t: NeonToken, p: &[u16; NL]) -> [uint32x4_t; 4] {
    let lo = vld1q_u16(p[..8].try_into().unwrap());
    let hi = vld1q_u16(p[8..].try_into().unwrap());
    [
        vmovl_u16(vget_low_u16(lo)),
        vmovl_high_u16(lo),
        vmovl_u16(vget_low_u16(hi)),
        vmovl_high_u16(hi),
    ]
}

/// Stores four `uint32x4_t` quarters (values <= u16::MAX) into one 16-lane step.
#[rite(import_intrinsics)]
#[inline(always)]
fn st16_all(_t: NeonToken, dst: &mut [u16; NL], v: [uint32x4_t; 4]) {
    let lo = vcombine_u16(vmovn_u32(v[0]), vmovn_u32(v[1]));
    let hi = vcombine_u16(vmovn_u32(v[2]), vmovn_u32(v[3]));
    vst1q_u16((&mut dst[..8]).try_into().unwrap(), lo);
    vst1q_u16((&mut dst[8..]).try_into().unwrap(), hi);
}

/// Lane mask as 4 bits.
#[rite(import_intrinsics)]
#[inline(always)]
fn bits4(t: NeonToken, m: uint32x4_t) -> usize {
    vaddvq_u32(vandq_u32(m, ld4(t, &[1, 2, 4, 8]))) as usize
}

/// Ordered masked refill of one quarter: lanes needing a word pull
/// `w.word(wp + rank)` in ascending lane order, `wp` advanced by the
/// count. Same ordering as the scalar loop.
#[rite(import_intrinsics)]
#[inline(always)]
fn refill(
    t: NeonToken,
    x: uint32x4_t,
    need: uint32x4_t,
    w: &Words<'_>,
    wp: &mut usize,
) -> uint32x4_t {
    let m = bits4(t, need);
    if m == 0 {
        return x;
    }
    let c = w.chunk(*wp);
    let b8: &[u8; 8] = c[..8].try_into().unwrap();
    let words = vmovl_u16(vreinterpret_u16_u8(vld1_u8(b8)));
    let v = vreinterpretq_u32_u8(vqtbl1q_u8(vreinterpretq_u8_u32(words), vld1q_u8(&PERM4[m])));
    *wp += m.count_ones() as usize;
    vbslq_u32(need, vorrq_u32(vshlq_n_u32::<16>(x), v), x)
}

/// x >> n per lane.
#[rite(import_intrinsics)]
#[inline(always)]
fn shr(_t: NeonToken, x: uint32x4_t, n: uint32x4_t) -> uint32x4_t {
    vshlq_u32(x, vnegq_s32(vreinterpretq_s32_u32(n)))
}

/// Computes all 16 lanes' table indices in one `vqtbl1q_u8` lookup.
#[rite(import_intrinsics)]
#[inline(always)]
fn idx_all(
    _t: NeonToken,
    cmapv: uint8x16_t,
    x: &[uint32x4_t; 4],
    sum: &[uint32x4_t; 4],
) -> [uint32x4_t; 4] {
    let zero = vdupq_n_s32(0);
    let c126 = vdupq_n_s32(126);
    let c15 = vdupq_n_s32(15);
    let cls: [uint32x4_t; 4] = core::array::from_fn(|g| {
        let ex = vshrq_n_u32::<23>(vreinterpretq_u32_f32(vcvtq_f32_s32(vreinterpretq_s32_u32(
            sum[g],
        ))));
        let c = vmaxq_s32(vsubq_s32(vreinterpretq_s32_u32(ex), c126), zero);
        vreinterpretq_u32_s32(vminq_s32(c, c15))
    });
    let c01 = vcombine_u16(vmovn_u32(cls[0]), vmovn_u32(cls[1]));
    let c23 = vcombine_u16(vmovn_u32(cls[2]), vmovn_u32(cls[3]));
    let c_all = vcombine_u8(vmovn_u16(c01), vmovn_u16(c23));
    let ctx_all = vandq_u8(vqtbl1q_u8(cmapv, c_all), vdupq_n_u8(3));
    let lo16 = vmovl_u8(vget_low_u8(ctx_all));
    let hi16 = vmovl_high_u8(ctx_all);
    let ctx: [uint32x4_t; 4] = [
        vmovl_u16(vget_low_u16(lo16)),
        vmovl_high_u16(lo16),
        vmovl_u16(vget_low_u16(hi16)),
        vmovl_high_u16(hi16),
    ];
    let c1023 = vdupq_n_u32(1023);
    core::array::from_fn(|g| vorrq_u32(vshlq_n_u32::<10>(ctx[g]), vandq_u32(x[g], c1023)))
}

/// Gathers 16 decode entries across four quarters.
#[rite(import_intrinsics)]
#[inline(always)]
fn gather_all(t: NeonToken, dec: &[u32; TABLE], idx: &[uint32x4_t; 4]) -> [uint32x4_t; 4] {
    core::array::from_fn(|g| {
        let mut ia = [0u32; 4];
        st4(t, &mut ia, idx[g]);
        ld4(
            t,
            &[
                dec[ia[0] as usize & (TABLE - 1)],
                dec[ia[1] as usize & (TABLE - 1)],
                dec[ia[2] as usize & (TABLE - 1)],
                dec[ia[3] as usize & (TABLE - 1)],
            ],
        )
    })
}

/// rANS state advance for one quarter (masked).
#[rite(import_intrinsics)]
#[inline(always)]
fn token_update(
    t: NeonToken,
    e: uint32x4_t,
    act: uint32x4_t,
    w: &Words<'_>,
    wp: &mut usize,
    x: &mut uint32x4_t,
) {
    let zero = vdupq_n_u32(0);
    let c1023 = vdupq_n_u32(1023);
    let f = vaddq_u32(vandq_u32(e, c1023), vdupq_n_u32(1));
    let bias = vandq_u32(vshrq_n_u32::<10>(e), c1023);
    let nx = vaddq_u32(vmulq_u32(f, vshrq_n_u32::<10>(*x)), bias);
    let xx = vbslq_u32(act, nx, *x);
    let need = vandq_u32(act, vceqq_u32(vshrq_n_u32::<16>(xx), zero));
    *x = refill(t, xx, need, w, wp);
}

/// rANS state advance for one quarter when all 4 lanes are active.
#[rite(import_intrinsics)]
#[inline(always)]
fn token_update_all(
    t: NeonToken,
    e: uint32x4_t,
    w: &Words<'_>,
    wp: &mut usize,
    x: &mut uint32x4_t,
) {
    let zero = vdupq_n_u32(0);
    let c1023 = vdupq_n_u32(1023);
    let f = vaddq_u32(vandq_u32(e, c1023), vdupq_n_u32(1));
    let bias = vandq_u32(vshrq_n_u32::<10>(e), c1023);
    let nx = vaddq_u32(vmulq_u32(f, vshrq_n_u32::<10>(*x)), bias);
    let need = vceqq_u32(vshrq_n_u32::<16>(nx), zero);
    *x = refill(t, nx, need, w, wp);
}

/// Extra-bits phase + emit for one quarter (masked).
#[rite(import_intrinsics)]
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn extra_phase(
    t: NeonToken,
    k: &K,
    w: &Words<'_>,
    wp: &mut usize,
    x: &mut uint32x4_t,
    run: &mut uint32x4_t,
    act: uint32x4_t,
    e: uint32x4_t,
) -> uint32x4_t {
    let zero = vdupq_n_u32(0);
    let one = vdupq_n_u32(1);
    let nb = vshrq_n_u32::<28>(e);
    let sym = vandq_u32(vshrq_n_u32::<20>(e), vdupq_n_u32(255));
    let ebits = vandq_u32(
        *x,
        vsubq_u32(vshlq_u32(one, vreinterpretq_s32_u32(nb)), one),
    );
    let xx = vbslq_u32(act, shr(t, *x, nb), *x);
    let need = vandq_u32(act, vceqq_u32(vshrq_n_u32::<16>(xx), zero));
    *x = refill(t, xx, need, w, wp);
    let is_run = vcgtq_s32(vreinterpretq_s32_u32(sym), vreinterpretq_s32_u32(k.vtm1));
    let tt = vsubq_u32(sym, vandq_u32(is_run, k.vt));
    let dlim = vbslq_u32(is_run, k.dlim_r, k.dlim_v);
    let direct = vcgtq_s32(vreinterpretq_s32_u32(dlim), vreinterpretq_s32_u32(tt));
    let hb = vshlq_u32(
        vorrq_u32(k.mpow, vandq_u32(tt, k.mmask)),
        vreinterpretq_s32_u32(nb),
    );
    let base = vbslq_u32(direct, tt, hb);
    let u = vaddq_u32(base, ebits);
    let run_now = vandq_u32(act, is_run);
    *run = vbslq_u32(run_now, u, *run);
    vbicq_u32(vandq_u32(act, u), run_now)
}

/// Extra-bits phase + emit for one quarter when all 4 lanes are active (`run` was 0).
#[rite(import_intrinsics)]
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn extra_phase_all(
    t: NeonToken,
    k: &K,
    w: &Words<'_>,
    wp: &mut usize,
    x: &mut uint32x4_t,
    run: &mut uint32x4_t,
    e: uint32x4_t,
) -> uint32x4_t {
    let zero = vdupq_n_u32(0);
    let one = vdupq_n_u32(1);
    let nb = vshrq_n_u32::<28>(e);
    let sym = vandq_u32(vshrq_n_u32::<20>(e), vdupq_n_u32(255));
    let ebits = vandq_u32(
        *x,
        vsubq_u32(vshlq_u32(one, vreinterpretq_s32_u32(nb)), one),
    );
    let xx = shr(t, *x, nb);
    let need = vceqq_u32(vshrq_n_u32::<16>(xx), zero);
    *x = refill(t, xx, need, w, wp);
    let is_run = vcgtq_s32(vreinterpretq_s32_u32(sym), vreinterpretq_s32_u32(k.vtm1));
    let tt = vsubq_u32(sym, vandq_u32(is_run, k.vt));
    let dlim = vbslq_u32(is_run, k.dlim_r, k.dlim_v);
    let direct = vcgtq_s32(vreinterpretq_s32_u32(dlim), vreinterpretq_s32_u32(tt));
    let hb = vshlq_u32(
        vorrq_u32(k.mpow, vandq_u32(tt, k.mmask)),
        vreinterpretq_s32_u32(nb),
    );
    let base = vbslq_u32(direct, tt, hb);
    let u = vaddq_u32(base, ebits);
    *run = vandq_u32(is_run, u);
    vbicq_u32(u, is_run)
}

/// Emit when no active lane has extra bits (masked).
#[rite(import_intrinsics)]
#[inline(always)]
fn no_extra(
    _t: NeonToken,
    k: &K,
    run: &mut uint32x4_t,
    act: uint32x4_t,
    e: uint32x4_t,
) -> uint32x4_t {
    let sym = vandq_u32(vshrq_n_u32::<20>(e), vdupq_n_u32(255));
    let is_run = vcgtq_s32(vreinterpretq_s32_u32(sym), vreinterpretq_s32_u32(k.vtm1));
    let u = vsubq_u32(sym, vandq_u32(is_run, k.vt));
    let run_now = vandq_u32(act, is_run);
    *run = vbslq_u32(run_now, u, *run);
    vbicq_u32(vandq_u32(act, u), run_now)
}

/// Emit when all 4 lanes are active and have `nb == 0` (top 4 bits of `e` are 0).
#[rite(import_intrinsics)]
#[inline(always)]
fn no_extra_all(_t: NeonToken, k: &K, run: &mut uint32x4_t, e: uint32x4_t) -> uint32x4_t {
    let sym = vshrq_n_u32::<20>(e);
    let is_run = vcgtq_s32(vreinterpretq_s32_u32(sym), vreinterpretq_s32_u32(k.vtm1));
    let u = vsubq_u32(sym, vandq_u32(is_run, k.vt));
    *run = vandq_u32(is_run, u);
    vbicq_u32(u, is_run)
}

#[rite(import_intrinsics)]
#[inline(always)]
fn context_sum(t: NeonToken, i: &Inst<'_, '_>, x: usize) -> [uint32x4_t; 4] {
    let zero = vdupq_n_u32(0);
    let mut s = if i.row > 0 {
        let n = ld16_all(t, &i.prev[x]);
        if x > 0 {
            core::array::from_fn(|g| vaddq_u32(i.west[g], n[g]))
        } else {
            core::array::from_fn(|g| vaddq_u32(n[g], n[g]))
        }
    } else if x > 0 {
        core::array::from_fn(|g| vaddq_u32(i.west[g], i.west[g]))
    } else {
        [zero; 4]
    };
    if let Some(cr) = i.cross {
        let c = ld16_all(t, &cr[x]);
        for g in 0..4 {
            s[g] = vaddq_u32(s[g], c[g]);
        }
    }
    s
}

/// General step when some lane is past its width or all 16 lanes are in runs.
#[rite(import_intrinsics)]
fn step_slow(
    t: NeonToken,
    i: &mut Inst<'_, '_>,
    wid: &[uint32x4_t; 4],
    lw: usize,
    out: &mut [[u16; NL]],
) {
    let zero = vdupq_n_u32(0);
    let x = i.xi;
    let colv = vdupq_n_u32(x as u32);
    let inw: [uint32x4_t; 4] = core::array::from_fn(|g| {
        vcgtq_s32(vreinterpretq_s32_u32(wid[g]), vreinterpretq_s32_u32(colv))
    });
    let busy: [uint32x4_t; 4] = core::array::from_fn(|g| {
        vorrq_u32(
            vcgtq_s32(vreinterpretq_s32_u32(i.run[g]), vdupq_n_s32(0)),
            vmvnq_u32(inw[g]),
        )
    });
    let all_busy = vandq_u32(vandq_u32(busy[0], busy[1]), vandq_u32(busy[2], busy[3]));
    if vminvq_u32(all_busy) != 0 {
        let big = vdupq_n_u32(i32::MAX as u32);
        let mut m = big;
        for g in 0..4 {
            m = vminq_u32(
                m,
                vbslq_u32(inw[g], vminq_u32(i.run[g], vsubq_u32(wid[g], colv)), big),
            );
        }
        let kk = (vminvq_u32(m) as usize).min(lw - x);
        if kk > 0 {
            let kv = vdupq_n_u32(kk as u32);
            for g in 0..4 {
                i.run[g] = vsubq_u32(i.run[g], vandq_u32(kv, inw[g]));
                i.west[g] = zero;
            }
            for step_row in &mut out[x..x + kk] {
                *step_row = [0u16; NL];
            }
            i.xi += kk;
            return;
        }
    }
    let s = context_sum(t, i, x);
    let mut act = [zero; 4];
    for g in 0..4 {
        let in_run = vandq_u32(
            vcgtq_s32(vreinterpretq_s32_u32(i.run[g]), vdupq_n_s32(0)),
            inw[g],
        );
        i.run[g] = vaddq_u32(i.run[g], in_run);
        act[g] = vbicq_u32(inw[g], in_run);
    }
    let idx = idx_all(t, i.k.cmapv, &i.x, &s);
    let mut e = [zero; 4];
    let mut live = [false; 4];
    for g in 0..4 {
        live[g] = vmaxvq_u32(act[g]) != 0;
        if live[g] {
            let mut ia = [0u32; 4];
            st4(t, &mut ia, idx[g]);
            let dec = i.dec;
            e[g] = ld4(
                t,
                &[
                    dec[ia[0] as usize & (TABLE - 1)],
                    dec[ia[1] as usize & (TABLE - 1)],
                    dec[ia[2] as usize & (TABLE - 1)],
                    dec[ia[3] as usize & (TABLE - 1)],
                ],
            );
        }
    }
    for g in 0..4 {
        if live[g] {
            token_update(t, e[g], act[g], i.w, &mut i.wp, &mut i.x[g]);
        }
    }
    let nbmask = vdupq_n_u32(0xF000_0000);
    let mut any_nb = zero;
    for g in 0..4 {
        any_nb = vorrq_u32(any_nb, vandq_u32(vandq_u32(e[g], act[g]), nbmask));
    }
    let mut u = [zero; 4];
    if vmaxvq_u32(any_nb) == 0 {
        for g in 0..4 {
            if live[g] {
                u[g] = no_extra(t, &i.k, &mut i.run[g], act[g], e[g]);
            }
        }
    } else {
        for g in 0..4 {
            if live[g] {
                u[g] = extra_phase(
                    t,
                    &i.k,
                    i.w,
                    &mut i.wp,
                    &mut i.x[g],
                    &mut i.run[g],
                    act[g],
                    e[g],
                );
            }
        }
    }
    st16_all(t, &mut out[x], u);
    i.west = u;
    i.xi += 1;
}

/// Inline full-width (`xi < min_w`) step:
/// - Fast all-active path when all 16 lanes have `run == 0` (zero masks).
/// - Inline partial-run path when `1..=15` lanes are in runs.
/// - Delegates to `step_slow` only when all 16 lanes are simultaneously in runs.
macro_rules! neon_step_inw {
    ($t:expr, $i:expr, $wid:expr, $lw:expr, $out:expr) => {{
        let any_run = vorrq_u32(
            vorrq_u32($i.run[0], $i.run[1]),
            vorrq_u32($i.run[2], $i.run[3]),
        );
        if vmaxvq_u32(any_run) == 0 {
            let x = $i.xi;
            let s = context_sum($t, &$i, x);
            let idx = idx_all($t, $i.k.cmapv, &$i.x, &s);
            let e = gather_all($t, $i.dec, &idx);
            for g in 0..4 {
                token_update_all($t, e[g], $i.w, &mut $i.wp, &mut $i.x[g]);
            }
            let any_nb = vandq_u32(
                vorrq_u32(vorrq_u32(e[0], e[1]), vorrq_u32(e[2], e[3])),
                vdupq_n_u32(0xF000_0000),
            );
            let u: [uint32x4_t; 4] = if vmaxvq_u32(any_nb) == 0 {
                core::array::from_fn(|g| no_extra_all($t, &$i.k, &mut $i.run[g], e[g]))
            } else {
                core::array::from_fn(|g| {
                    extra_phase_all(
                        $t,
                        &$i.k,
                        $i.w,
                        &mut $i.wp,
                        &mut $i.x[g],
                        &mut $i.run[g],
                        e[g],
                    )
                })
            };
            st16_all($t, &mut $out[x], u);
            $i.west = u;
            $i.xi = x + 1;
        } else {
            let in_r: [uint32x4_t; 4] = core::array::from_fn(|g| {
                vcgtq_s32(vreinterpretq_s32_u32($i.run[g]), vdupq_n_s32(0))
            });
            let all_r = vandq_u32(vandq_u32(in_r[0], in_r[1]), vandq_u32(in_r[2], in_r[3]));
            if vminvq_u32(all_r) != 0 {
                step_slow($t, &mut $i, &$wid, $lw, $out);
            } else {
                let x = $i.xi;
                let s = context_sum($t, &$i, x);
                let mut act = [vdupq_n_u32(0); 4];
                for g in 0..4 {
                    $i.run[g] = vaddq_u32($i.run[g], in_r[g]);
                    act[g] = vmvnq_u32(in_r[g]);
                }
                let idx = idx_all($t, $i.k.cmapv, &$i.x, &s);
                let e = gather_all($t, $i.dec, &idx);
                for g in 0..4 {
                    token_update($t, e[g], act[g], $i.w, &mut $i.wp, &mut $i.x[g]);
                }
                let any_nb = vandq_u32(
                    vorrq_u32(
                        vorrq_u32(vandq_u32(e[0], act[0]), vandq_u32(e[1], act[1])),
                        vorrq_u32(vandq_u32(e[2], act[2]), vandq_u32(e[3], act[3])),
                    ),
                    vdupq_n_u32(0xF000_0000),
                );
                let u: [uint32x4_t; 4] = if vmaxvq_u32(any_nb) == 0 {
                    core::array::from_fn(|g| no_extra($t, &$i.k, &mut $i.run[g], act[g], e[g]))
                } else {
                    core::array::from_fn(|g| {
                        extra_phase(
                            $t,
                            &$i.k,
                            $i.w,
                            &mut $i.wp,
                            &mut $i.x[g],
                            &mut $i.run[g],
                            act[g],
                            e[g],
                        )
                    })
                };
                st16_all($t, &mut $out[x], u);
                $i.west = u;
                $i.xi = x + 1;
            }
        }
    }};
}

/// Loads `st` into an `Inst`.
#[rite(import_intrinsics)]
#[inline(always)]
fn inst<'a, 'w>(
    _t: NeonToken,
    md: &'a DecModel,
    w: &'a Words<'w>,
    st: &LaneState,
    prev: &'a [[u16; NL]],
    cross: Option<&'a [[u16; NL]]>,
    row: usize,
) -> Inst<'a, 'w> {
    let mut i = Inst {
        k: K {
            cmapv: vld1q_u8(&md.ctx_map),
            vt: vdupq_n_u32(md.vt),
            vtm1: vdupq_n_u32(md.vt.wrapping_sub(1)),
            dlim_v: vdupq_n_u32(1 << md.k),
            dlim_r: vdupq_n_u32(1 << md.kr),
            mpow: vdupq_n_u32(1 << md.m),
            mmask: vdupq_n_u32((1 << md.m) - 1),
        },
        dec: &md.dec,
        w,
        prev,
        cross,
        x: [vdupq_n_u32(0); 4],
        run: [vdupq_n_u32(0); 4],
        west: [vdupq_n_u32(0); 4],
        wp: st.wp,
        xi: 0,
        row,
    };
    for g in 0..4 {
        i.x[g] = vld1q_u32(st.x[4 * g..4 * g + 4].try_into().unwrap());
        i.run[g] = vld1q_u32(st.run[4 * g..4 * g + 4].try_into().unwrap());
    }
    i
}

/// Writes `st` back from an `Inst`.
#[rite(import_intrinsics)]
#[inline(always)]
fn finish(t: NeonToken, i: &Inst<'_, '_>, st: &mut LaneState) {
    for g in 0..4 {
        st4(t, (&mut st.x[4 * g..4 * g + 4]).try_into().unwrap(), i.x[g]);
        st4(
            t,
            (&mut st.run[4 * g..4 * g + 4]).try_into().unwrap(),
            i.run[g],
        );
    }
    st.wp = i.wp;
}

/// NEON row decoder; bit-identical to [`super::decode_row_scalar`].
#[arcane(import_intrinsics)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_row(
    t: NeonToken,
    md: &DecModel,
    words: &Words<'_>,
    st: &mut LaneState,
    widths: &[u32; NL],
    lw: usize,
    row: usize,
    prev: &[u16],
    cross: Option<&[u16]>,
    out: &mut [u16],
) {
    let wid: [uint32x4_t; 4] =
        core::array::from_fn(|g| vld1q_u32(widths[4 * g..4 * g + 4].try_into().unwrap()));
    let min_w = widths.iter().copied().min().unwrap_or(0) as usize;
    let prev_c = if row > 0 {
        &prev.as_chunks::<NL>().0[..lw]
    } else {
        &[]
    };
    let cross_c = cross.map(|c| &c.as_chunks::<NL>().0[..lw]);
    let out_c = &mut out.as_chunks_mut::<NL>().0[..lw];
    let mut a = inst(t, md, words, st, prev_c, cross_c, row);
    while a.xi < min_w {
        neon_step_inw!(t, a, wid, lw, out_c);
    }
    while a.xi < lw {
        step_slow(t, &mut a, &wid, lw, out_c);
    }
    finish(t, &a, st);
}

/// Two independent row decodes stepped together: their dependency chains
/// interleave in the out-of-order core (see `rans::avx2::decode_row2`).
#[arcane(import_intrinsics)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_row2(
    t: NeonToken,
    md: [&DecModel; 2],
    words: [&Words<'_>; 2],
    st: [&mut LaneState; 2],
    widths: &[u32; NL],
    lw: usize,
    rows: [usize; 2],
    prev: [&[u16]; 2],
    cross: [Option<&[u16]>; 2],
    out: [&mut [u16]; 2],
) {
    let wid: [uint32x4_t; 4] =
        core::array::from_fn(|g| vld1q_u32(widths[4 * g..4 * g + 4].try_into().unwrap()));
    let min_w = widths.iter().copied().min().unwrap_or(0) as usize;
    let [sa, sb] = st;
    let [oa, ob] = out;
    let pa_c = if rows[0] > 0 {
        &prev[0].as_chunks::<NL>().0[..lw]
    } else {
        &[]
    };
    let pb_c = if rows[1] > 0 {
        &prev[1].as_chunks::<NL>().0[..lw]
    } else {
        &[]
    };
    let ca_c = cross[0].map(|c| &c.as_chunks::<NL>().0[..lw]);
    let cb_c = cross[1].map(|c| &c.as_chunks::<NL>().0[..lw]);
    let oa_c = &mut oa.as_chunks_mut::<NL>().0[..lw];
    let ob_c = &mut ob.as_chunks_mut::<NL>().0[..lw];
    let mut a = inst(t, md[0], words[0], sa, pa_c, ca_c, rows[0]);
    let mut b = inst(t, md[1], words[1], sb, pb_c, cb_c, rows[1]);
    while a.xi < min_w && b.xi < min_w {
        neon_step_inw!(t, a, wid, lw, oa_c);
        neon_step_inw!(t, b, wid, lw, ob_c);
    }
    while a.xi < min_w {
        neon_step_inw!(t, a, wid, lw, oa_c);
    }
    while b.xi < min_w {
        neon_step_inw!(t, b, wid, lw, ob_c);
    }
    while a.xi < lw {
        step_slow(t, &mut a, &wid, lw, oa_c);
    }
    while b.xi < lw {
        step_slow(t, &mut b, &wid, lw, ob_c);
    }
    finish(t, &a, sa);
    finish(t, &b, sb);
}

/// Three independent row decodes stepped together (see
/// `rans::avx512::decode_row3`).
#[arcane(import_intrinsics)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_row3(
    t: NeonToken,
    md: [&DecModel; 3],
    words: [&Words<'_>; 3],
    st: [&mut LaneState; 3],
    widths: &[u32; NL],
    lw: usize,
    rows: [usize; 3],
    prev: [&[u16]; 3],
    cross: [Option<&[u16]>; 3],
    out: [&mut [u16]; 3],
) {
    let wid: [uint32x4_t; 4] =
        core::array::from_fn(|g| vld1q_u32(widths[4 * g..4 * g + 4].try_into().unwrap()));
    let min_w = widths.iter().copied().min().unwrap_or(0) as usize;
    let [sa, sb, sc] = st;
    let [oa, ob, oc] = out;
    let pa_c = if rows[0] > 0 {
        &prev[0].as_chunks::<NL>().0[..lw]
    } else {
        &[]
    };
    let pb_c = if rows[1] > 0 {
        &prev[1].as_chunks::<NL>().0[..lw]
    } else {
        &[]
    };
    let pc_c = if rows[2] > 0 {
        &prev[2].as_chunks::<NL>().0[..lw]
    } else {
        &[]
    };
    let ca_c = cross[0].map(|c| &c.as_chunks::<NL>().0[..lw]);
    let cb_c = cross[1].map(|c| &c.as_chunks::<NL>().0[..lw]);
    let cc_c = cross[2].map(|c| &c.as_chunks::<NL>().0[..lw]);
    let oa_c = &mut oa.as_chunks_mut::<NL>().0[..lw];
    let ob_c = &mut ob.as_chunks_mut::<NL>().0[..lw];
    let oc_c = &mut oc.as_chunks_mut::<NL>().0[..lw];
    let mut a = inst(t, md[0], words[0], sa, pa_c, ca_c, rows[0]);
    let mut b = inst(t, md[1], words[1], sb, pb_c, cb_c, rows[1]);
    let mut c = inst(t, md[2], words[2], sc, pc_c, cc_c, rows[2]);
    while a.xi < min_w && b.xi < min_w && c.xi < min_w {
        neon_step_inw!(t, a, wid, lw, oa_c);
        neon_step_inw!(t, b, wid, lw, ob_c);
        neon_step_inw!(t, c, wid, lw, oc_c);
    }
    while a.xi < min_w {
        neon_step_inw!(t, a, wid, lw, oa_c);
    }
    while b.xi < min_w {
        neon_step_inw!(t, b, wid, lw, ob_c);
    }
    while c.xi < min_w {
        neon_step_inw!(t, c, wid, lw, oc_c);
    }
    while a.xi < lw {
        step_slow(t, &mut a, &wid, lw, oa_c);
    }
    while b.xi < lw {
        step_slow(t, &mut b, &wid, lw, ob_c);
    }
    while c.xi < lw {
        step_slow(t, &mut c, &wid, lw, oc_c);
    }
    finish(t, &a, sa);
    finish(t, &b, sb);
    finish(t, &c, sc);
}
