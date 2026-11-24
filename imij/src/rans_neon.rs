//! aarch64 NEON row decoder: [`rans::decode_row_scalar`] bit-identical,
//! structured like `rans::avx2` but with the 16 lanes carried as four
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
    prev: &'a [u16],
    cross: Option<&'a [u16]>,
    x: [uint32x4_t; 4],
    run: [uint32x4_t; 4],
    west: [uint32x4_t; 4],
    wp: usize,
    xi: usize,
    row: usize,
}

#[rite(import_intrinsics)]
fn ld4(_t: NeonToken, a: &[u32]) -> uint32x4_t {
    let a: &[u32; 4] = a[..4].try_into().unwrap();
    vld1q_u32(a)
}

#[rite(import_intrinsics)]
fn st4(_t: NeonToken, a: &mut [u32], v: uint32x4_t) {
    let a: &mut [u32; 4] = (&mut a[..4]).try_into().unwrap();
    vst1q_u32(a, v);
}

#[rite(import_intrinsics)]
fn st16(_t: NeonToken, a: &mut [u16], v: uint32x4_t) {
    let a: &mut [u16; 4] = (&mut a[..4]).try_into().unwrap();
    vst1_u16(a, vmovn_u32(v));
}

/// Widens four u16s (step-major slot values) into a u32x4.
#[rite(import_intrinsics)]
fn ld16(_t: NeonToken, p: &[u16]) -> uint32x4_t {
    let a: &[u16; 4] = p[..4].try_into().unwrap();
    vmovl_u16(vld1_u16(a))
}

/// Lane mask as 4 bits.
#[rite(import_intrinsics)]
fn bits4(t: NeonToken, m: uint32x4_t) -> usize {
    vaddvq_u32(vandq_u32(m, ld4(t, &[1, 2, 4, 8]))) as usize
}

/// Ordered masked refill of one quarter: lanes needing a word pull
/// `w.word(wp + rank)` in ascending lane order, `wp` advanced by the
/// count. Same ordering as the scalar loop.
#[rite(import_intrinsics)]
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
fn shr(_t: NeonToken, x: uint32x4_t, n: uint32x4_t) -> uint32x4_t {
    vshlq_u32(x, vnegq_s32(vreinterpretq_s32_u32(n)))
}

/// `ctx_map[class]` per lane through `vqtbl1q_u8`, matching the AVX2
/// kernel's `shuffle_epi8` + `& 3`.
#[rite(import_intrinsics)]
fn ctx_v(_t: NeonToken, cmapv: uint8x16_t, cls: uint32x4_t) -> uint32x4_t {
    let b = vmovn_u16(vcombine_u16(vmovn_u32(cls), vdup_n_u16(0)));
    let got = vqtbl1q_u8(cmapv, vcombine_u8(b, vdup_n_u8(0)));
    vandq_u32(
        vmovl_u16(vget_low_u16(vmovl_u8(vget_low_u8(got)))),
        vdupq_n_u32(3),
    )
}

/// Context + table index for one quarter; also returns `act` and
/// decrements in-run lanes.
#[rite(import_intrinsics)]
fn token_idx(
    t: NeonToken,
    k: &K,
    x: uint32x4_t,
    run: &mut uint32x4_t,
    sum: uint32x4_t,
    inw: uint32x4_t,
) -> (uint32x4_t, uint32x4_t) {
    let in_run = vandq_u32(vcgtq_s32(vreinterpretq_s32_u32(*run), vdupq_n_s32(0)), inw);
    *run = vaddq_u32(*run, in_run);
    let act = vbicq_u32(inw, in_run);
    let ex = vshrq_n_u32::<23>(vreinterpretq_u32_f32(vcvtq_f32_s32(vreinterpretq_s32_u32(
        sum,
    ))));
    let cls = vmaxq_s32(
        vsubq_s32(vreinterpretq_s32_u32(ex), vdupq_n_s32(126)),
        vdupq_n_s32(0),
    );
    let cls = vreinterpretq_u32_s32(vminq_s32(cls, vdupq_n_s32(15)));
    let idx = vorrq_u32(
        vshlq_n_u32::<10>(ctx_v(t, k.cmapv, cls)),
        vandq_u32(x, vdupq_n_u32(1023)),
    );
    (act, idx)
}

/// rANS state advance for one quarter.
#[rite(import_intrinsics)]
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

/// Extra-bits phase + emit for one quarter (u16 storage: nb <= 15 is
/// exact in the decode entry). Returns the emitted vector.
#[rite(import_intrinsics)]
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

/// Emit when no active lane has extra bits: tokens are direct.
#[rite(import_intrinsics)]
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

/// One step of `i` (or a skip over all-run columns), over four quarters.
#[rite(import_intrinsics)]
fn step(t: NeonToken, i: &mut Inst<'_, '_>, wid: &[uint32x4_t; 4], lw: usize, out: &mut [u16]) {
    let zero = vdupq_n_u32(0);
    let (row, x) = (i.row, i.xi);
    let k = &i.k;
    let colv = vdupq_n_u32(x as u32);
    let inw: [uint32x4_t; 4] = core::array::from_fn(|g| {
        vcgtq_s32(vreinterpretq_s32_u32(wid[g]), vreinterpretq_s32_u32(colv))
    });
    // Fast path: every lane is either past its width or inside a run.
    let busy: [uint32x4_t; 4] = core::array::from_fn(|g| {
        vorrq_u32(
            vcgtq_s32(vreinterpretq_s32_u32(i.run[g]), vdupq_n_s32(0)),
            vmvnq_u32(inw[g]),
        )
    });
    if bits4(t, busy[0]) & bits4(t, busy[1]) & bits4(t, busy[2]) & bits4(t, busy[3]) == 0xF {
        // Steps to skip: min run over active lanes, min width - x.
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
            out[x * NL..(x + kk) * NL].fill(0);
            i.xi += kk;
            return;
        }
    }
    let bi = x * NL;
    let mut s: [uint32x4_t; 4] = if row > 0 {
        if x > 0 {
            core::array::from_fn(|g| vaddq_u32(i.west[g], ld16(t, &i.prev[bi + 4 * g..])))
        } else {
            core::array::from_fn(|g| {
                let n = ld16(t, &i.prev[bi + 4 * g..]);
                vaddq_u32(n, n)
            })
        }
    } else if x > 0 {
        core::array::from_fn(|g| vaddq_u32(i.west[g], i.west[g]))
    } else {
        [zero; 4]
    };
    if let Some(cr) = i.cross {
        for g in 0..4 {
            s[g] = vaddq_u32(s[g], ld16(t, &cr[bi + 4 * g..]));
        }
    }
    // A quarter whose lanes are all in runs (or past their width) only
    // advances its run counters; it consumes no words.
    let mut act = [zero; 4];
    let mut e = [zero; 4];
    let mut live = [false; 4];
    for g in 0..4 {
        let (a, idx) = token_idx(t, k, i.x[g], &mut i.run[g], s[g], inw[g]);
        act[g] = a;
        live[g] = bits4(t, a) != 0;
        if live[g] {
            let ia = {
                let mut ia = [0u32; 4];
                st4(t, &mut ia, idx);
                ia
            };
            let dec = i.dec;
            e[g] = ld4(
                t,
                &core::array::from_fn::<u32, 4, _>(|j| dec[ia[j] as usize & (TABLE - 1)]),
            );
        }
    }
    for g in 0..4 {
        if live[g] {
            token_update(t, e[g], act[g], i.w, &mut i.wp, &mut i.x[g]);
        }
    }
    // nb lives in bits 28..32: any active lane with extra bits?
    let nbmask = vdupq_n_u32(0xF000_0000);
    let mut any_nb = vdupq_n_u32(0);
    for g in 0..4 {
        any_nb = vorrq_u32(any_nb, vandq_u32(vandq_u32(e[g], act[g]), nbmask));
    }
    let mut u = [zero; 4];
    if vmaxvq_u32(any_nb) == 0 {
        for g in 0..4 {
            if live[g] {
                u[g] = no_extra(t, k, &mut i.run[g], act[g], e[g]);
            }
        }
    } else {
        for g in 0..4 {
            if live[g] {
                u[g] = extra_phase(
                    t,
                    k,
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
    for g in 0..4 {
        st16(t, &mut out[bi + 4 * g..], u[g]);
        i.west[g] = u[g];
    }
    i.xi += 1;
}

/// Loads `st` into an `Inst`.
#[rite(import_intrinsics)]
fn inst<'a, 'w>(
    _t: NeonToken,
    md: &'a DecModel,
    w: &'a Words<'w>,
    st: &LaneState,
    prev: &'a [u16],
    cross: Option<&'a [u16]>,
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
fn finish(t: NeonToken, i: &Inst<'_, '_>, st: &mut LaneState) {
    for g in 0..4 {
        st4(t, &mut st.x[4 * g..], i.x[g]);
        st4(t, &mut st.run[4 * g..], i.run[g]);
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
    let mut i = inst(t, md, words, st, prev, cross, row);
    while i.xi < lw {
        step(t, &mut i, &wid, lw, out);
    }
    finish(t, &i, st);
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
    let [sa, sb] = st;
    let [oa, ob] = out;
    let mut a = inst(t, md[0], words[0], sa, prev[0], cross[0], rows[0]);
    let mut b = inst(t, md[1], words[1], sb, prev[1], cross[1], rows[1]);
    while a.xi < lw && b.xi < lw {
        step(t, &mut a, &wid, lw, oa);
        step(t, &mut b, &wid, lw, ob);
    }
    while a.xi < lw {
        step(t, &mut a, &wid, lw, oa);
    }
    while b.xi < lw {
        step(t, &mut b, &wid, lw, ob);
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
    let [sa, sb, sc] = st;
    let [oa, ob, oc] = out;
    let mut a = inst(t, md[0], words[0], sa, prev[0], cross[0], rows[0]);
    let mut b = inst(t, md[1], words[1], sb, prev[1], cross[1], rows[1]);
    let mut c = inst(t, md[2], words[2], sc, prev[2], cross[2], rows[2]);
    while a.xi < lw && b.xi < lw && c.xi < lw {
        step(t, &mut a, &wid, lw, oa);
        step(t, &mut b, &wid, lw, ob);
        step(t, &mut c, &wid, lw, oc);
    }
    while a.xi < lw {
        step(t, &mut a, &wid, lw, oa);
    }
    while b.xi < lw {
        step(t, &mut b, &wid, lw, ob);
    }
    while c.xi < lw {
        step(t, &mut c, &wid, lw, oc);
    }
    finish(t, &a, sa);
    finish(t, &b, sb);
    finish(t, &c, sc);
}
