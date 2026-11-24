//! AVX2 coefficient decoder for one stripe: bit-identical to the scalar
//! lane loop in `decode_stripe_raw`. The 16 lanes are two halves of 8.
//! Lane state, contexts, rANS updates, refills and extra bits are vector
//! code; the table lookups and the coefficient stores are per-lane scalar
//! (software gather/scatter).
//!
//! Per half the loop keeps five state vectors (rANS state, packed fields,
//! block column, block index, live mask) so that both halves plus
//! constants fit the 16 ymm registers.

use super::coef::{EOB_CTX, K_CLASSES, MAX_TABLES, NRAW_PAD, REM_CLASSES, ZIGZAG, ZRL};
use crate::rans::avx2::refill;
use crate::rans::{LOG_M, NL, Words};
use archmage::prelude::*;

/// Vector lane state of one half.
#[derive(Clone, Copy)]
struct Half {
    x: __m256i,
    /// k | eob << 8 | ch << 16 | luma << 24.
    st: __m256i,
    bx: __m256i,
    /// Block index within the stripe (r * bw + bx).
    bi: __m256i,
    /// All-ones while the lane has symbols left.
    live: __m256i,
}

/// floor(log2(v)) + 1 for v >= 1, 0 for v == 0 (v < 2^24), via the float
/// exponent.
#[rite(import_intrinsics)]
pub(super) fn bitlen(_t: X64V3Token, v: __m256i) -> __m256i {
    let e = _mm256_srli_epi32::<23>(_mm256_castps_si256(_mm256_cvtepi32_ps(v)));
    _mm256_max_epi32(
        _mm256_sub_epi32(e, _mm256_set1_epi32(126)),
        _mm256_setzero_si256(),
    )
}

#[rite(import_intrinsics)]
pub(super) fn field<const SH: i32>(_t: X64V3Token, st: __m256i) -> __m256i {
    _mm256_and_si256(_mm256_srli_epi32::<SH>(st), _mm256_set1_epi32(0xFF))
}

#[rite(import_intrinsics)]
pub(super) fn st8(_t: X64V3Token, v: __m256i) -> [u32; 8] {
    let mut a = [0u32; 8];
    _mm256_storeu_si256(&mut a, v);
    a
}

/// Pulls `n` (0..=16 per lane) raw bits; returns them.
#[rite(import_intrinsics)]
pub(super) fn pull(
    t: X64V3Token,
    x: &mut __m256i,
    n: __m256i,
    w: &Words<'_>,
    wp: &mut usize,
) -> __m256i {
    let zero = _mm256_setzero_si256();
    let one = _mm256_set1_epi32(1);
    let bits = _mm256_and_si256(*x, _mm256_sub_epi32(_mm256_sllv_epi32(one, n), one));
    let has = _mm256_cmpgt_epi32(n, zero);
    let xx = _mm256_blendv_epi8(*x, _mm256_srlv_epi32(*x, n), has);
    let need = _mm256_and_si256(has, _mm256_cmpeq_epi32(_mm256_srli_epi32::<16>(xx), zero));
    *x = refill(t, xx, need, w, wp);
    bits
}

/// Context + table lookup + rANS symbol update of one half (a macro so it
/// expands inside the `#[arcane]` caller). Evaluates to the table entries.
macro_rules! symbol {
    ($t:ident, $h:expr, $cmap:ident, $dec:ident, $w:ident, $wp:ident, $ch0:ident) => {{
        let t = $t;
        let h: &mut Half = $h;
        let (cmap, dec, w, wp) = ($cmap, $dec, $w, &mut $wp);
        let zero = _mm256_setzero_si256();
        let one = _mm256_set1_epi32(1);
        let k = field::<0>(t, h.st);
        let eob = field::<8>(t, h.st);
        // Absolute channel (contexts) = group's first channel + relative.
        let ch = _mm256_add_epi32(field::<16>(t, h.st), $ch0);
        let luma = _mm256_srli_epi32::<24>(h.st);
        let is_eob = _mm256_cmpeq_epi32(k, zero);
        let rawe = _mm256_add_epi32(_mm256_slli_epi32::<3>(ch), bitlen(t, luma));
        // floor(log2(eob - k + 1)) capped = bitlen(eob - k + 1) - 1.
        let rem1 = _mm256_add_epi32(_mm256_sub_epi32(eob, k), one);
        let rc = _mm256_min_epi32(
            _mm256_sub_epi32(bitlen(t, rem1), one),
            _mm256_set1_epi32(REM_CLASSES as i32 - 1),
        );
        // EOB_CTX + (ch * 63 + k - 1) * 6 + rc, with ch * 63 = ch * 64 - ch.
        let ck = _mm256_sub_epi32(
            _mm256_add_epi32(_mm256_sub_epi32(_mm256_slli_epi32::<6>(ch), ch), k),
            one,
        );
        const _: () = assert!(K_CLASSES == 63);
        let ck6 = _mm256_add_epi32(_mm256_slli_epi32::<2>(ck), _mm256_slli_epi32::<1>(ck));
        let rawa = _mm256_add_epi32(_mm256_add_epi32(ck6, _mm256_set1_epi32(EOB_CTX as i32)), rc);
        let raw = _mm256_and_si256(_mm256_blendv_epi8(rawa, rawe, is_eob), h.live);
        // Software gather of the table entries.
        let ra = st8(t, raw);
        let xa = st8(t, h.x);
        let ev: [u32; 8] = core::array::from_fn(|i| {
            let tb = cmap[ra[i] as usize & (NRAW_PAD - 1)] as usize;
            dec[((tb << LOG_M) | (xa[i] & 1023) as usize) & ((MAX_TABLES << LOG_M) - 1)]
        });
        let e = _mm256_loadu_si256(&ev);
        // rANS update and renormalization.
        let f = _mm256_add_epi32(_mm256_and_si256(e, _mm256_set1_epi32(1023)), one);
        let bias = _mm256_and_si256(_mm256_srli_epi32::<10>(e), _mm256_set1_epi32(1023));
        let nx = _mm256_add_epi32(_mm256_mullo_epi32(f, _mm256_srli_epi32::<10>(h.x)), bias);
        let xx = _mm256_blendv_epi8(h.x, nx, h.live);
        let need = _mm256_and_si256(
            h.live,
            _mm256_cmpeq_epi32(_mm256_srli_epi32::<16>(xx), zero),
        );
        h.x = refill(t, xx, need, w, wp);
        e
    }};
}

/// Value, stores and state transition of one half after its extra bits
/// (a macro so it expands inside the `#[arcane]` caller).
macro_rules! apply {
    ($t:ident, $h:expr, $lane:expr, $e:expr, $extra:expr, $bwv:ident, $endv:ident, $q:ident, $eobs:ident, $nchv:ident) => {{
        let t = $t;
        let h: &mut Half = $h;
        let (lane, e, extra, bwv, endv) = ($lane, $e, $extra, $bwv, $endv);
        let q: &mut [i32] = $q;
        let eobs: &mut [u8] = $eobs;
    let zero = _mm256_setzero_si256();
    let one = _mm256_set1_epi32(1);
    let k = field::<0>(t, h.st);
    let eob = field::<8>(t, h.st);
    let ch = field::<16>(t, h.st);
    let luma = _mm256_srli_epi32::<24>(h.st);
    let live = h.live;
    let sym = _mm256_and_si256(_mm256_srli_epi32::<20>(e), _mm256_set1_epi32(127));
    let is_eob = _mm256_cmpeq_epi32(k, zero);
    let is_zrl = _mm256_cmpeq_epi32(sym, _mm256_set1_epi32(ZRL as i32));
    let cls = _mm256_and_si256(sym, _mm256_set1_epi32(31));
    // Position: k + run, or k + 4 for ZRL.
    let adv = _mm256_blendv_epi8(_mm256_srli_epi32::<5>(sym), _mm256_set1_epi32(4), is_zrl);
    let p = _mm256_add_epi32(k, adv);
    // Value: |v| = c + 1 for c < 4, else 5 (+ 2^(c-5) + remainder for c > 4).
    let four = _mm256_set1_epi32(4);
    let sh = _mm256_max_epi32(_mm256_sub_epi32(cls, _mm256_set1_epi32(5)), zero);
    let big = _mm256_and_si256(
        _mm256_cmpgt_epi32(cls, four),
        _mm256_add_epi32(_mm256_sllv_epi32(one, sh), _mm256_srli_epi32::<1>(extra)),
    );
    let m = _mm256_add_epi32(_mm256_set1_epi32(5), big);
    let mag = _mm256_blendv_epi8(m, _mm256_add_epi32(cls, one), _mm256_cmpgt_epi32(four, cls));
    let sign = _mm256_sub_epi32(zero, _mm256_and_si256(extra, one));
    let v = _mm256_sub_epi32(_mm256_xor_si256(mag, sign), sign);
    // Stores, branchless: inactive targets go to the spare slots at the end.
    let bc = _mm256_add_epi32(_mm256_mullo_epi32(h.bi, $nchv), ch);
    let ac_lane = _mm256_andnot_si256(is_eob, live);
    let coef_lane = _mm256_andnot_si256(
        _mm256_or_si256(is_zrl, _mm256_cmpgt_epi32(p, eob)),
        ac_lane,
    );
    let eob_lane = _mm256_and_si256(is_eob, live);
    let qdummy = _mm256_set1_epi32((q.len() - 64) as i32);
    let edummy = _mm256_set1_epi32((eobs.len() - 1) as i32);
    let qd = _mm256_blendv_epi8(qdummy, _mm256_slli_epi32::<6>(bc), coef_lane);
    let ed = _mm256_blendv_epi8(edummy, bc, eob_lane);
    let (qa, pa, ea, sa, va) = (st8(t, qd), st8(t, p), st8(t, ed), st8(t, sym), st8(t, v));
    for i in 0..8 {
        q[qa[i] as usize + ZIGZAG[pa[i] as usize & 63] as usize] = va[i] as i32;
        eobs[ea[i] as usize] = sa[i] as u8;
    }
    // Transitions.
    let neob = _mm256_blendv_epi8(eob, sym, eob_lane);
    let nluma = _mm256_blendv_epi8(
        luma,
        sym,
        _mm256_and_si256(eob_lane, _mm256_cmpeq_epi32(ch, zero)),
    );
    // A coefficient at p >= eob ends the channel; ZRL ends it past eob.
    let not_zrl = _mm256_andnot_si256(is_zrl, _mm256_set1_epi32(-1));
    let thr = _mm256_add_epi32(eob, not_zrl);
    let end_ac = _mm256_cmpgt_epi32(p, thr);
    let next_ch = _mm256_and_si256(
        live,
        _mm256_or_si256(
            _mm256_and_si256(is_eob, _mm256_cmpeq_epi32(sym, zero)),
            _mm256_andnot_si256(is_eob, end_ac),
        ),
    );
    // Next k: 1 after an EOB token, p after ZRL, p + 1 after a coefficient.
    let kac = _mm256_sub_epi32(p, not_zrl);
    let nk = _mm256_blendv_epi8(kac, one, is_eob);
    let nk = _mm256_blendv_epi8(k, _mm256_andnot_si256(next_ch, nk), live);
    let nch = _mm256_sub_epi32(ch, next_ch);
    let wrap = _mm256_cmpeq_epi32(nch, $nchv);
    let nch = _mm256_andnot_si256(wrap, nch);
    h.st = _mm256_or_si256(
        _mm256_or_si256(nk, _mm256_slli_epi32::<8>(neob)),
        _mm256_or_si256(_mm256_slli_epi32::<16>(nch), _mm256_slli_epi32::<24>(nluma)),
    );
    // Next block: +16 columns, or the lane's first column of the next row.
    let sixteen = _mm256_and_si256(wrap, _mm256_set1_epi32(NL as i32));
    let nbx = _mm256_add_epi32(h.bx, sixteen);
    let nbi = _mm256_add_epi32(h.bi, sixteen);
    let roww = _mm256_and_si256(wrap, _mm256_cmpgt_epi32(nbx, _mm256_sub_epi32(bwv, one)));
    // Row start: bi - bx + bw + lane.
    let rs = _mm256_add_epi32(_mm256_add_epi32(_mm256_sub_epi32(h.bi, h.bx), bwv), lane);
    h.bx = _mm256_blendv_epi8(nbx, lane, roww);
    h.bi = _mm256_blendv_epi8(nbi, rs, roww);
    let fin = _mm256_and_si256(roww, _mm256_cmpgt_epi32(h.bi, _mm256_sub_epi32(endv, one)));
    h.live = _mm256_andnot_si256(fin, live);
    }};
}

/// Decodes the stripe's lane streams into `q` (raster order) / `eobs`,
/// zeroed by the caller, with 64 / 1 spare slots at the end. Returns the
/// number of words consumed.
#[arcane(import_intrinsics)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_stripe(
    t: X64V3Token,
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
    let ch0v = _mm256_set1_epi32(ch0 as i32);
    let nchv = _mm256_set1_epi32(nch as i32);
    let zero = _mm256_setzero_si256();
    let lanes: [u32; 16] = core::array::from_fn(|i| i as u32);
    let lane_v = [0, 1].map(|hf| {
        let a: &[u32; 8] = lanes[hf * 8..hf * 8 + 8].try_into().unwrap();
        _mm256_loadu_si256(a)
    });
    let mut hs = [0, 1].map(|hf| {
        let xs: &[u32; 8] = x0[hf * 8..hf * 8 + 8].try_into().unwrap();
        let live = if nby == 0 {
            zero
        } else {
            _mm256_cmpgt_epi32(_mm256_set1_epi32(active as i32), lane_v[hf])
        };
        Half {
            x: _mm256_loadu_si256(xs),
            st: zero,
            bx: lane_v[hf],
            bi: lane_v[hf],
            live,
        }
    });
    let mut wp = 0usize;
    let bwv = _mm256_set1_epi32(bw as i32);
    let endv = _mm256_set1_epi32((nby * bw) as i32);
    loop {
        let any = _mm256_or_si256(hs[0].live, hs[1].live);
        if _mm256_testz_si256(any, any) != 0 {
            break;
        }
        let e0 = symbol!(t, &mut hs[0], cmap, dec, words, wp, ch0v);
        let e1 = symbol!(t, &mut hs[1], cmap, dec, words, wp, ch0v);
        // Extra bits (zero for inactive lanes): low part, then high part.
        let nb0 = _mm256_and_si256(_mm256_srli_epi32::<27>(e0), hs[0].live);
        let nb1 = _mm256_and_si256(_mm256_srli_epi32::<27>(e1), hs[1].live);
        let mut x0 = zero;
        let mut x1 = zero;
        let anynb = _mm256_or_si256(nb0, nb1);
        if _mm256_testz_si256(anynb, anynb) == 0 {
            let s16 = _mm256_set1_epi32(16);
            x0 = pull(t, &mut hs[0].x, _mm256_min_epi32(nb0, s16), words, &mut wp);
            x1 = pull(t, &mut hs[1].x, _mm256_min_epi32(nb1, s16), words, &mut wp);
            let h0 = _mm256_max_epi32(_mm256_sub_epi32(nb0, s16), zero);
            let h1 = _mm256_max_epi32(_mm256_sub_epi32(nb1, s16), zero);
            let anyhi = _mm256_or_si256(h0, h1);
            if _mm256_testz_si256(anyhi, anyhi) == 0 {
                let b0 = pull(t, &mut hs[0].x, h0, words, &mut wp);
                let b1 = pull(t, &mut hs[1].x, h1, words, &mut wp);
                x0 = _mm256_or_si256(x0, _mm256_slli_epi32::<16>(b0));
                x1 = _mm256_or_si256(x1, _mm256_slli_epi32::<16>(b1));
            }
        }
        apply!(t, &mut hs[0], lane_v[0], e0, x0, bwv, endv, q, eobs, nchv);
        apply!(t, &mut hs[1], lane_v[1], e1, x1, bwv, endv, q, eobs, nchv);
        // Past the end of the stream the decode is already invalid.
        if wp > words.len {
            break;
        }
    }
    wp
}
