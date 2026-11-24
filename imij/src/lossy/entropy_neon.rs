//! NEON coefficient decoder for one stripe: bit-identical to the scalar
//! lane loop in `decode_stripe_raw` and to the x86 kernels. The 16 lanes
//! are four quarters of 4. Lane state, contexts, rANS updates, refills and
//! extra bits are vector code; table lookups and coefficient stores are
//! per-lane scalar (software gather/scatter).

use super::coef::{EOB_CTX, MAX_TABLES, NRAW_PAD, REM_CLASSES, ZIGZAG, ZRL};
use crate::rans::{LOG_M, NL, RANS_L, Words};
use archmage::prelude::*;

/// Refill permutation per 4-lane need mask: byte indices moving the r-th
/// loaded word into each needing lane (r = needing lanes below it).
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

/// Lane state of one quarter.
#[derive(Clone, Copy)]
struct Q {
    x: uint32x4_t,
    /// k | eob << 8 | ch << 16 | luma << 24.
    st: uint32x4_t,
    bx: uint32x4_t,
    bi: uint32x4_t,
    /// All-ones while the lane has symbols left.
    live: uint32x4_t,
    /// LAB (coefgap decode-cost proxy): the block's EOB context offset.
    bk: uint32x4_t,
}

#[rite(import_intrinsics)]
fn ld4(_t: NeonToken, a: &[u32; 4]) -> uint32x4_t {
    vld1q_u32(a)
}

#[rite(import_intrinsics)]
fn st4(_t: NeonToken, v: uint32x4_t) -> [u32; 4] {
    let mut a = [0u32; 4];
    vst1q_u32(&mut a, v);
    a
}

/// Lane mask as 4 bits.
#[rite(import_intrinsics)]
fn bits4(t: NeonToken, m: uint32x4_t) -> usize {
    vaddvq_u32(vandq_u32(m, ld4(t, &[1, 2, 4, 8]))) as usize
}

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

/// Bit length (0 for 0).
#[rite(import_intrinsics)]
fn bitlen(_t: NeonToken, v: uint32x4_t) -> uint32x4_t {
    vsubq_u32(vdupq_n_u32(32), vclzq_u32(v))
}

/// x >> n per lane.
#[rite(import_intrinsics)]
fn shr(_t: NeonToken, x: uint32x4_t, n: uint32x4_t) -> uint32x4_t {
    vshlq_u32(x, vnegq_s32(vreinterpretq_s32_u32(n)))
}

/// 1 << n per lane.
#[rite(import_intrinsics)]
fn pow2(_t: NeonToken, n: uint32x4_t) -> uint32x4_t {
    vshlq_u32(vdupq_n_u32(1), vreinterpretq_s32_u32(n))
}

/// Pulls `n` (0..=16 per lane) raw bits from lanes with `n > 0`.
#[rite(import_intrinsics)]
fn pull(
    t: NeonToken,
    x: &mut uint32x4_t,
    n: uint32x4_t,
    w: &Words<'_>,
    wp: &mut usize,
) -> uint32x4_t {
    let one = vdupq_n_u32(1);
    let bits = vandq_u32(*x, vsubq_u32(pow2(t, n), one));
    let has = vcgtq_u32(n, vdupq_n_u32(0));
    let xx = vbslq_u32(has, shr(t, *x, n), *x);
    let need = vandq_u32(has, vcltq_u32(xx, vdupq_n_u32(RANS_L)));
    *x = refill(t, xx, need, w, wp);
    bits
}

#[rite(import_intrinsics)]
fn field<const SH: i32>(_t: NeonToken, st: uint32x4_t) -> uint32x4_t {
    vandq_u32(vshrq_n_u32::<SH>(st), vdupq_n_u32(0xFF))
}

/// Context, table lookup and rANS symbol update of one quarter; returns the
/// table entries.
#[rite(import_intrinsics)]
fn symbol<const B: bool>(
    t: NeonToken,
    h: &mut Q,
    cmap: &[u8; NRAW_PAD],
    dec: &[u32; MAX_TABLES << LOG_M],
    w: &Words<'_>,
    wp: &mut usize,
    ch0: u32,
) -> uint32x4_t {
    let one = vdupq_n_u32(1);
    let k = vandq_u32(h.st, vdupq_n_u32(0xFF));
    let eob = field::<8>(t, h.st);
    // Absolute channel (contexts) = group's first channel + relative.
    let ch = vaddq_u32(field::<16>(t, h.st), vdupq_n_u32(ch0));
    let luma = vshrq_n_u32::<24>(h.st);
    let is_eob = vceqq_u32(k, vdupq_n_u32(0));
    let mut rawe = vaddq_u32(vshlq_n_u32::<3>(ch), bitlen(t, luma));
    if B {
        rawe = vaddq_u32(rawe, h.bk);
    }
    let rem1 = vaddq_u32(vsubq_u32(eob, k), one);
    let rc = vminq_u32(
        vsubq_u32(bitlen(t, rem1), one),
        vdupq_n_u32(REM_CLASSES as u32 - 1),
    );
    // EOB_CTX + (ch * 63 + k - 1) * 6 + rc
    let ck = vsubq_u32(vaddq_u32(vsubq_u32(vshlq_n_u32::<6>(ch), ch), k), one);
    let ck6 = vaddq_u32(vshlq_n_u32::<2>(ck), vshlq_n_u32::<1>(ck));
    let rawa = vaddq_u32(vaddq_u32(ck6, vdupq_n_u32(EOB_CTX as u32)), rc);
    let raw = vandq_u32(vbslq_u32(is_eob, rawe, rawa), h.live);
    let ra = st4(t, raw);
    let xa = st4(t, h.x);
    let ev: [u32; 4] = core::array::from_fn(|i| {
        let tb = cmap[ra[i] as usize & (NRAW_PAD - 1)] as usize;
        dec[((tb << LOG_M) | (xa[i] & 1023) as usize) & ((MAX_TABLES << LOG_M) - 1)]
    });
    let e = ld4(t, &ev);
    let c1023 = vdupq_n_u32(1023);
    let f = vaddq_u32(vandq_u32(e, c1023), one);
    let bias = vandq_u32(vshrq_n_u32::<10>(e), c1023);
    let nx = vaddq_u32(vmulq_u32(f, vshrq_n_u32::<10>(h.x)), bias);
    let xx = vbslq_u32(h.live, nx, h.x);
    let need = vandq_u32(h.live, vcltq_u32(xx, vdupq_n_u32(RANS_L)));
    h.x = refill(t, xx, need, w, wp);
    e
}

/// Value, stores and state transition of one quarter.
#[rite(import_intrinsics)]
#[allow(clippy::too_many_arguments)]
fn apply<const B: bool>(
    t: NeonToken,
    h: &mut Q,
    lane: uint32x4_t,
    e: uint32x4_t,
    extra: uint32x4_t,
    bw: u32,
    end: u32,
    q: &mut [i32],
    eobs: &mut [u8],
    nch: u32,
    bkt: &[u32],
) {
    let zero = vdupq_n_u32(0);
    let one = vdupq_n_u32(1);
    let k = vandq_u32(h.st, vdupq_n_u32(0xFF));
    let eob = field::<8>(t, h.st);
    let ch = field::<16>(t, h.st);
    let luma = vshrq_n_u32::<24>(h.st);
    let live = h.live;
    let sym = vandq_u32(vshrq_n_u32::<20>(e), vdupq_n_u32(127));
    let is_eob = vceqq_u32(k, zero);
    let is_zrl = vceqq_u32(sym, vdupq_n_u32(ZRL));
    let cls = vandq_u32(sym, vdupq_n_u32(31));
    let adv = vbslq_u32(is_zrl, vdupq_n_u32(4), vshrq_n_u32::<5>(sym));
    let p = vaddq_u32(k, adv);
    // |v| = c + 1 for c < 4, else 5 (+ 2^(c-5) + remainder for c > 4).
    let four = vdupq_n_u32(4);
    let sh = vreinterpretq_u32_s32(vmaxq_s32(
        vsubq_s32(vreinterpretq_s32_u32(cls), vdupq_n_s32(5)),
        vdupq_n_s32(0),
    ));
    let big = vandq_u32(
        vcgtq_u32(cls, four),
        vaddq_u32(pow2(t, sh), vshrq_n_u32::<1>(extra)),
    );
    let m = vaddq_u32(vdupq_n_u32(5), big);
    let mag = vbslq_u32(vcltq_u32(cls, four), vaddq_u32(cls, one), m);
    let sign = vsubq_u32(zero, vandq_u32(extra, one));
    let v = vsubq_u32(veorq_u32(mag, sign), sign);
    // Stores, branchless: inactive targets go to the spare slots.
    let bc = vaddq_u32(vmulq_u32(h.bi, vdupq_n_u32(nch)), ch);
    let not_eob = vmvnq_u32(is_eob);
    let past = vcgtq_u32(p, eob);
    let coef_lane = vandq_u32(vandq_u32(live, not_eob), vmvnq_u32(vorrq_u32(is_zrl, past)));
    let eob_lane = vandq_u32(live, is_eob);
    let qd = vbslq_u32(
        coef_lane,
        vshlq_n_u32::<6>(bc),
        vdupq_n_u32((q.len() - 64) as u32),
    );
    let ed = vbslq_u32(eob_lane, bc, vdupq_n_u32((eobs.len() - 1) as u32));
    let (qa, pa, ea, sa, va) = (st4(t, qd), st4(t, p), st4(t, ed), st4(t, sym), st4(t, v));
    for i in 0..4 {
        q[qa[i] as usize + ZIGZAG[pa[i] as usize & 63] as usize] = va[i] as i32;
        eobs[ea[i] as usize] = sa[i] as u8;
    }
    // Transitions.
    let neob = vbslq_u32(eob_lane, sym, eob);
    let nluma = vbslq_u32(vandq_u32(eob_lane, vceqq_u32(ch, zero)), sym, luma);
    // A coefficient at p >= eob ends the channel; ZRL ends it past eob.
    let end_ac = vbslq_u32(is_zrl, past, vcgeq_u32(p, eob));
    let next_ch = vandq_u32(
        live,
        vorrq_u32(
            vandq_u32(is_eob, vceqq_u32(sym, zero)),
            vandq_u32(not_eob, end_ac),
        ),
    );
    // Next k: 1 after an EOB token, p after ZRL, p + 1 after a coefficient.
    let kac = vbslq_u32(is_zrl, p, vaddq_u32(p, one));
    let nk = vbicq_u32(vbslq_u32(is_eob, one, kac), next_ch);
    let nk = vbslq_u32(live, nk, k);
    let nchn = vsubq_u32(ch, next_ch);
    let wrap = vceqq_u32(nchn, vdupq_n_u32(nch));
    let nchn = vbicq_u32(nchn, wrap);
    h.st = vorrq_u32(
        vorrq_u32(nk, vshlq_n_u32::<8>(neob)),
        vorrq_u32(vshlq_n_u32::<16>(nchn), vshlq_n_u32::<24>(nluma)),
    );
    // Next block: +16 columns, or the lane's first column of the next row.
    let sixteen = vandq_u32(wrap, vdupq_n_u32(NL as u32));
    let nbx = vaddq_u32(h.bx, sixteen);
    let nbi = vaddq_u32(h.bi, sixteen);
    let roww = vandq_u32(wrap, vcgeq_u32(nbx, vdupq_n_u32(bw)));
    // Row start: bi - bx + bw + lane.
    let rs = vaddq_u32(vaddq_u32(vsubq_u32(h.bi, h.bx), vdupq_n_u32(bw)), lane);
    h.bx = vbslq_u32(roww, lane, nbx);
    h.bi = vbslq_u32(roww, rs, nbi);
    let fin = vandq_u32(roww, vcgeq_u32(h.bi, vdupq_n_u32(end)));
    h.live = vbicq_u32(live, fin);
    if B && vmaxvq_u32(wrap) != 0 {
        load_bk(t, h, wrap, bkt);
    }
}

/// LAB (coefgap decode-cost proxy): loads the EOB context offsets of the
/// blocks the `m` lanes start (`bkt[bi]`, kept when out of range).
#[rite(import_intrinsics)]
fn load_bk(t: NeonToken, h: &mut Q, m: uint32x4_t, bkt: &[u32]) {
    let ia = st4(t, h.bi);
    let ma = st4(t, m);
    let mut ba = st4(t, h.bk);
    for i in 0..4 {
        if ma[i] != 0
            && let Some(&v) = bkt.get(ia[i] as usize)
        {
            ba[i] = v;
        }
    }
    h.bk = ld4(t, &ba);
}

/// Decodes the stripe's lane streams into `q` (raster order) / `eobs`,
/// zeroed by the caller, with 64 / 1 spare slots at the end. Returns the
/// number of words consumed.
#[arcane(import_intrinsics)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_stripe(
    t: NeonToken,
    words: &Words<'_>,
    x0: [u32; NL],
    cmap: &[u8; NRAW_PAD],
    dec: &[u32; MAX_TABLES << LOG_M],
    bw: usize,
    nby: usize,
    active: usize,
    ch0: u32,
    nch: u32,
    q: &mut [i32],
    eobs: &mut [u8],
    bkt: Option<&[u32]>,
) -> usize {
    match bkt {
        None => run::<false>(
            t,
            words,
            x0,
            cmap,
            dec,
            bw,
            nby,
            active,
            ch0,
            nch,
            q,
            eobs,
            &[],
        ),
        Some(b) => run::<true>(
            t, words, x0, cmap, dec, bw, nby, active, ch0, nch, q, eobs, b,
        ),
    }
}

#[rite(import_intrinsics)]
#[allow(clippy::too_many_arguments)]
fn run<const B: bool>(
    t: NeonToken,
    words: &Words<'_>,
    x0: [u32; NL],
    cmap: &[u8; NRAW_PAD],
    dec: &[u32; MAX_TABLES << LOG_M],
    bw: usize,
    nby: usize,
    active: usize,
    ch0: u32,
    nch: u32,
    q: &mut [i32],
    eobs: &mut [u8],
    bkt: &[u32],
) -> usize {
    let lane_v: [uint32x4_t; 4] = core::array::from_fn(|i| {
        let l = (i * 4) as u32;
        ld4(t, &[l, l + 1, l + 2, l + 3])
    });
    let mut hs: [Q; 4] = core::array::from_fn(|i| {
        let xs: &[u32; 4] = x0[i * 4..i * 4 + 4].try_into().unwrap();
        let live = if nby == 0 {
            vdupq_n_u32(0)
        } else {
            vcltq_u32(lane_v[i], vdupq_n_u32(active as u32))
        };
        Q {
            x: ld4(t, xs),
            st: vdupq_n_u32(0),
            bx: lane_v[i],
            bi: lane_v[i],
            live,
            bk: vdupq_n_u32(0),
        }
    });
    if B {
        for h in hs.iter_mut() {
            let live = h.live;
            load_bk(t, h, live, bkt);
        }
    }
    let mut wp = 0usize;
    let end = (nby * bw) as u32;
    loop {
        let any = vorrq_u32(
            vorrq_u32(hs[0].live, hs[1].live),
            vorrq_u32(hs[2].live, hs[3].live),
        );
        if vmaxvq_u32(any) == 0 {
            break;
        }
        // A fully-dead quarter runs no symbol/apply and its pulls are
        // skipped (n == 0 makes them no-ops anyway): every observable
        // effect is live-masked, so this is byte-exact.
        let mut e = [vdupq_n_u32(0); 4];
        let live: [bool; 4] = core::array::from_fn(|i| vmaxvq_u32(hs[i].live) != 0);
        for i in 0..4 {
            if live[i] {
                e[i] = symbol::<B>(t, &mut hs[i], cmap, dec, words, &mut wp, ch0);
            }
        }
        // Extra bits (zero for inactive lanes): low part, then high part.
        let nb: [uint32x4_t; 4] =
            core::array::from_fn(|i| vandq_u32(vshrq_n_u32::<27>(e[i]), hs[i].live));
        let mut extra = [vdupq_n_u32(0); 4];
        let anynb = vorrq_u32(vorrq_u32(nb[0], nb[1]), vorrq_u32(nb[2], nb[3]));
        if vmaxvq_u32(anynb) != 0 {
            let s16 = vdupq_n_u32(16);
            for i in 0..4 {
                if live[i] {
                    extra[i] = pull(t, &mut hs[i].x, vminq_u32(nb[i], s16), words, &mut wp);
                }
            }
            let hi: [uint32x4_t; 4] = core::array::from_fn(|i| vqsubq_u32(nb[i], s16));
            let anyhi = vorrq_u32(vorrq_u32(hi[0], hi[1]), vorrq_u32(hi[2], hi[3]));
            if vmaxvq_u32(anyhi) != 0 {
                for i in 0..4 {
                    if live[i] {
                        let b = pull(t, &mut hs[i].x, hi[i], words, &mut wp);
                        extra[i] = vorrq_u32(extra[i], vshlq_n_u32::<16>(b));
                    }
                }
            }
        }
        for i in 0..4 {
            if live[i] {
                apply::<B>(
                    t, &mut hs[i], lane_v[i], e[i], extra[i], bw as u32, end, q, eobs, nch, bkt,
                );
            }
        }
        // Past the end of the stream the decode is already invalid.
        if wp > words.len {
            break;
        }
    }
    wp
}
