//! Portable (magetypes) run-level stripe entropy decoder: one canonical
//! body compiled for the wasm128 and scalar tiers. Bit-identical to the
//! scalar lane loop in `decode_stripe_raw` — same per-lane rANS order,
//! context classes, renorm points, and stream-word consumption order —
//! and to the NEON kernel it mirrors (`entropy_neon.rs`, owned by the
//! kernels lane; this file is the portable copy so wasm32 can run it).
//!
//! The 16 lanes are two `u32x8` halves; per-half lane order is ascending,
//! so word consumption order matches scalar/NEON exactly. Vectorized:
//! context index math (i32x8 domain — every operand is < 2^24, so the
//! f32-exponent bit-length trick is exact and signed `to_f32x8` is safe),
//! the rANS state update, store masks, and channel/block transitions.
//! Per-lane scalar work remains for `cmap`/`dec` gathers, extra-bit pulls,
//! renorm word loads, and `q`/`eobs` stores — wasm128 has no per-lane
//! variable shifts and no gather/scatter.

use super::coef::{EOB_CTX, MAX_TABLES, NRAW_PAD, REM_CLASSES, ZIGZAG, ZRL, rl_value};
use crate::mask_of;
use crate::rans::{LOG_M, NL, RANS_L, Words};
use archmage::prelude::*;

#[allow(clippy::too_many_arguments)]
#[magetypes(define(u32x8, i32x8), wasm128, scalar)]
fn decode_stripe_impl(
    token: Token,
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
) -> usize {
    let zero = u32x8::zero_t(token);
    let izero = i32x8::zero_t(token);
    let one = u32x8::splat_t(token, 1);
    let m255 = u32x8::splat_t(token, 255);
    let c1023 = u32x8::splat_t(token, 1023);
    let lim = u32x8::splat_t(token, RANS_L);
    let c126 = i32x8::splat_t(token, 126);
    let c127 = u32x8::splat_t(token, 127);
    let zrl = u32x8::splat_t(token, ZRL);
    let four = u32x8::splat_t(token, 4);
    let nl16 = u32x8::splat_t(token, NL as u32);
    let bwv = u32x8::splat_t(token, bw as u32);
    let endv = u32x8::splat_t(token, (nby * bw) as u32);
    let nchv = u32x8::splat_t(token, nch);
    let ch0_i = i32x8::splat_t(token, ch0 as i32);
    let rcm_i = i32x8::splat_t(token, REM_CLASSES as i32 - 1);
    let eob_ctx_v = i32x8::splat_t(token, EOB_CTX as i32);
    let ione = i32x8::splat_t(token, 1);

    // Bit length per lane (0 for 0): the f32 exponent of a positive
    // integer < 2^24 converts exactly, so field-126 is the bit length.
    // Operands are luma (<= 127) and eob-k+1 (1..=64), always in range.
    macro_rules! bitlen {
        ($v:expr) => {{
            let v = $v;
            let e = v.to_f32x8().bitcast_i32x8().shr_logical_const::<23>() - c126;
            incant!(crate::wasm_relaxed::sel_i32x8(token, v.simd_ne(izero), e, izero) without token)
        }};
    }

    let lane_v = [
        u32x8::from_array_t(token, [0, 1, 2, 3, 4, 5, 6, 7]),
        u32x8::from_array_t(token, [8, 9, 10, 11, 12, 13, 14, 15]),
    ];
    let active_v = u32x8::splat_t(token, active as u32);
    let mut x = [
        u32x8::from_slice_t(token, &x0[..8]),
        u32x8::from_slice_t(token, &x0[8..]),
    ];
    // Lane state: k | eob << 8 | ch << 16 | luma << 24.
    let mut st = [zero, zero];
    let mut bx = lane_v;
    let mut bi = lane_v;
    let mut live = if nby == 0 {
        [zero, zero]
    } else {
        [lane_v[0].simd_lt(active_v), lane_v[1].simd_lt(active_v)]
    };
    let mut wp = 0usize;

    loop {
        if !(live[0] | live[1]).any_true() {
            break;
        }

        // Token phase: context, table lookup, rANS update + refill.
        let mut e = [zero, zero];
        let mut nb = [[0u32; 8]; 2];
        let mut anynb = false;
        for h in 0..2 {
            if !live[h].any_true() {
                continue;
            }
            let sti = st[h].bitcast_i32x8();
            let k = sti & i32x8::splat_t(token, 255);
            let eob = sti.shr_logical_const::<8>() & i32x8::splat_t(token, 255);
            // Absolute channel (contexts) = group's first channel + relative.
            let ch = (sti.shr_logical_const::<16>() & i32x8::splat_t(token, 255)) + ch0_i;
            let luma = sti.shr_logical_const::<24>();
            let is_eob = k.simd_eq(izero);
            let rawe = (ch * i32x8::splat_t(token, 8)) + bitlen!(luma);
            let rem1 = eob - k + ione;
            let rc = (bitlen!(rem1) - ione).min(rcm_i);
            // EOB_CTX + (ch * 63 + k - 1) * 6 + rc
            let ck = ch * i32x8::splat_t(token, 63) + k - ione;
            let rawa = ck * i32x8::splat_t(token, 6) + eob_ctx_v + rc;
            let raw = incant!(crate::wasm_relaxed::sel_i32x8(token, is_eob, rawe, rawa) without token)
                & live[h].bitcast_i32x8();
            let ra = raw.to_array();
            let xa = x[h].to_array();
            let ev: [u32; 8] = core::array::from_fn(|i| {
                let tb = cmap[ra[i] as usize & (NRAW_PAD - 1)] as usize;
                dec[((tb << LOG_M) | (xa[i] & 1023) as usize) & ((MAX_TABLES << LOG_M) - 1)]
            });
            let ev = u32x8::from_array_t(token, ev);
            e[h] = ev;
            let ea = ev.to_array();
            let lm = live[h].bitmask();
            for i in 0..8 {
                nb[h][i] = if lm & (1 << i) != 0 { ea[i] >> 27 } else { 0 };
                anynb |= nb[h][i] != 0;
            }
            let f = (ev & c1023) + one;
            let bias = ev.shr_logical_const::<10>() & c1023;
            let nx = f * x[h].shr_logical_const::<10>() + bias;
            let xx =
                incant!(crate::wasm_relaxed::sel_u32x8(token, live[h], nx, x[h]) without token);
            // Ordered refill: needing lanes take consecutive stream words.
            let mut need = (live[h] & xx.simd_lt(lim)).bitmask();
            if need != 0 {
                let mut xa = xx.to_array();
                let mut r = 0usize;
                while need != 0 {
                    let i = need.trailing_zeros() as usize;
                    xa[i] = (xa[i] << 16) | words.word(wp + r) as u32;
                    r += 1;
                    need &= need - 1;
                }
                wp += r;
                x[h] = u32x8::from_array_t(token, xa);
            } else {
                x[h] = xx;
            }
        }

        // Extra bits, scalar per lane in lane order: low part (<= 16),
        // then the high part for counts > 16.
        let mut extra = [0u32; NL];
        if anynb {
            for h in 0..2 {
                let mut xa = x[h].to_array();
                for i in 0..8 {
                    let n = nb[h][i].min(16);
                    if n > 0 {
                        extra[8 * h + i] = xa[i] & mask_of(n);
                        xa[i] >>= n;
                        if xa[i] < RANS_L {
                            xa[i] = (xa[i] << 16) | words.word(wp) as u32;
                            wp += 1;
                        }
                    }
                }
                x[h] = u32x8::from_array_t(token, xa);
            }
            for h in 0..2 {
                let mut xa = x[h].to_array();
                for i in 0..8 {
                    if nb[h][i] > 16 {
                        let n = nb[h][i] - 16;
                        extra[8 * h + i] |= (xa[i] & mask_of(n)) << 16;
                        xa[i] >>= n;
                        if xa[i] < RANS_L {
                            xa[i] = (xa[i] << 16) | words.word(wp) as u32;
                            wp += 1;
                        }
                    }
                }
                x[h] = u32x8::from_array_t(token, xa);
            }
        }

        // Apply: value stores (scalar) and state transitions (vector).
        for h in 0..2 {
            let lh = live[h];
            if !lh.any_true() {
                continue;
            }
            let k = st[h] & m255;
            let eob = st[h].shr_logical_const::<8>() & m255;
            let ch = st[h].shr_logical_const::<16>() & m255;
            let luma = st[h].shr_logical_const::<24>();
            let sym = e[h].shr_logical_const::<20>() & c127;
            let is_eob = k.simd_eq(zero);
            let is_zrl = sym.simd_eq(zrl);
            let adv = incant!(crate::wasm_relaxed::sel_u32x8(token, is_zrl, four, sym.shr_logical_const::<5>()) without token);
            let p = k + adv;
            let bc = bi[h] * nchv + ch;
            let not_eob = is_eob.not();
            let past = p.simd_gt(eob);
            let coef_lane = lh & not_eob & (is_zrl | past).not();
            let eob_lane = lh & is_eob;
            let mut mc = coef_lane.bitmask();
            let mut me = eob_lane.bitmask();
            if mc | me != 0 {
                let (bca, pa, sa) = (bc.to_array(), p.to_array(), sym.to_array());
                while mc != 0 {
                    let i = mc.trailing_zeros() as usize;
                    q[((bca[i] as usize) << 6) + ZIGZAG[pa[i] as usize & 63] as usize] =
                        rl_value(sa[i] & 31, extra[8 * h + i]);
                    mc &= mc - 1;
                }
                while me != 0 {
                    let i = me.trailing_zeros() as usize;
                    eobs[bca[i] as usize] = sa[i] as u8;
                    me &= me - 1;
                }
            }
            // Transitions: an EOB token starts the channel; a coefficient
            // at p >= eob or a ZRL past eob ends it (channel wraps).
            let neob =
                incant!(crate::wasm_relaxed::sel_u32x8(token, eob_lane, sym, eob) without token);
            let nluma = incant!(crate::wasm_relaxed::sel_u32x8(token, eob_lane & ch.simd_eq(zero), sym, luma) without token);
            let end_ac = incant!(crate::wasm_relaxed::sel_u32x8(token, is_zrl, past, p.simd_ge(eob)) without token);
            let next_ch = lh & ((is_eob & sym.simd_eq(zero)) | (not_eob & end_ac));
            // Next k: 1 after an EOB token, p after ZRL, p + 1 otherwise;
            // cleared when the channel ended. Dead lanes keep their k.
            let kac =
                incant!(crate::wasm_relaxed::sel_u32x8(token, is_zrl, p, p + one) without token);
            let eob_k =
                incant!(crate::wasm_relaxed::sel_u32x8(token, is_eob, one, kac) without token);
            let nk = incant!(crate::wasm_relaxed::sel_u32x8(token, lh, eob_k & next_ch.not(), k) without token);
            // Subtracting the all-ones mask adds 1.
            let nchn = ch - next_ch;
            let wrap = nchn.simd_eq(nchv);
            let nchn = nchn & wrap.not();
            st[h] = nk | neob.shl_const::<8>() | nchn.shl_const::<16>() | nluma.shl_const::<24>();
            // Next block: +16 columns on wrap, or the lane's first column
            // of the next block-row when the row is full.
            let sixteen = wrap & nl16;
            let nbx = bx[h] + sixteen;
            let nbi = bi[h] + sixteen;
            let roww = wrap & nbx.simd_ge(bwv);
            let rs = bi[h] - bx[h] + bwv + lane_v[h];
            bx[h] =
                incant!(crate::wasm_relaxed::sel_u32x8(token, roww, lane_v[h], nbx) without token);
            bi[h] = incant!(crate::wasm_relaxed::sel_u32x8(token, roww, rs, nbi) without token);
            let fin = roww & bi[h].simd_ge(endv);
            live[h] = lh & fin.not();
        }
        // Past the end of the stream the decode is already invalid.
        if wp > words.len {
            break;
        }
    }
    wp
}

/// Decodes the stripe's lane streams into `q` (raster order) / `eobs`,
/// zeroed by the caller, with 64 / 1 spare slots at the end. Returns the
/// number of words consumed. Same contract as `entropy_neon::decode_stripe`
/// and the scalar loop in `decode_stripe_raw`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_stripe(
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
) -> usize {
    incant!(
        decode_stripe_impl(words, x0, cmap, dec, bw, nby, active, ch0, nch, q, eobs),
        [wasm128, scalar]
    )
}

/// Hostile-input sweeps for the wrapper: degenerate tables,
/// boundary lane/geometry shapes, and truncated/empty word streams. The
/// impls trust `x >= RANS_L` (enforced by `Words::from_bytes` upstream),
/// so sweeps keep that contract; `q`/`eobs` are sized to the exact
/// caller contract (plus only the documented spare slots) so any stray
/// store fails bounds-checking instead of landing in slack space.
///
/// Compiled only where the module is: wasm32 (all builds) — so
/// `cargo test --target wasm32-wasip1` runs them against the production
/// wasm128 tier — and any other arch's `cfg(test)` build that compiles
/// this module for the scalar tier.
#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use alloc::boxed::Box;
    use alloc::vec;
    use alloc::vec::Vec;

    fn dec_of(fill: u32) -> Box<[u32; MAX_TABLES << LOG_M]> {
        vec![fill; MAX_TABLES << LOG_M]
            .into_boxed_slice()
            .try_into()
            .unwrap()
    }

    fn bodies() -> Vec<Vec<u8>> {
        // Odd-length bodies are rejected upstream by `from_body`.
        let mut v: Vec<Vec<u8>> = vec![vec![], vec![0, 7], vec![0xff; 2], vec![0; 8]];
        let mut s = 0x243f6a88u32;
        v.push(
            (0..514)
                .map(|_| {
                    s ^= s << 13;
                    s ^= s >> 17;
                    s ^= s << 5;
                    s as u8
                })
                .collect(),
        );
        v
    }

    /// rANS initial states, all at/above `RANS_L` (contract floor).
    fn states() -> Vec<[u32; NL]> {
        let mut v = vec![[RANS_L; NL], [u32::MAX; NL]];
        let mut alt = [RANS_L; NL];
        for (l, x) in alt.iter_mut().enumerate() {
            if l % 3 == 1 {
                *x = u32::MAX - l as u32;
            } else if l % 3 == 2 {
                *x = RANS_L + l as u32;
            }
        }
        v.push(alt);
        v
    }

    /// `dec` entries are `(freq - 1) | bias << 10 | sym << 20 | nb << 27`.
    /// Hostile fills keep `freq - 1 = 0` (freq 1 — a bigger factor times
    /// `x >> 10` overflows u32, a panic the scalar loop shares) and bound
    /// `sym` below the `mask_of` limit; `nb` ranges hit both extra-bit
    /// pull legs and `sym` 127 reaches ZRL / max-token paths.
    const D_SYM63_NB7: u32 = 0x03F0_0000;
    const D_SYM127_NB7: u32 = 0x07F0_0000;

    #[test]
    fn hostile_runlevel() {
        let (d0, d1, d2) = (dec_of(0), dec_of(D_SYM63_NB7), dec_of(D_SYM127_NB7));
        let cm = [0u8; NRAW_PAD];
        let mut pat = [0u8; NRAW_PAD];
        for (i, b) in pat.iter_mut().enumerate() {
            *b = (i % MAX_TABLES) as u8;
        }
        for (bw, nby) in [(1usize, 1usize), (16, 2), (3, 5), (17, 1)] {
            // Caller contract: `active == bw.min(NL)` — lanes own columns
            // l + 16k, so active may never exceed bw.
            for active in [0usize, 1, bw.min(9), bw.min(16)] {
                for x0 in states() {
                    for body in bodies() {
                        for (dec, cmap) in [(&*d0, &cm), (&*d1, &pat), (&*d2, &cm)] {
                            let w = Words::from_body(&body).unwrap();
                            let mut q = vec![0i32; nby * bw * 3 * 64 + 64];
                            let mut eobs = vec![0u8; nby * bw * 3 + 1];
                            // No assertion on `wp`: hostile streams may
                            // overconsume past `len` — callers treat that
                            // as corruption. The sweep only pins "no
                            // panic, no out-of-contract store".
                            let _ = decode_stripe(
                                &w, x0, cmap, dec, bw, nby, active, 0, 3, &mut q, &mut eobs,
                            );
                        }
                    }
                }
            }
        }
    }
}
