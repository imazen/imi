//! AVX-512 coefficient decoder for one stripe: bit-identical to the scalar
//! lane loop in `decode_stripe_raw` and to the AVX2 kernel. All 16 lanes
//! live in one zmm per state field; inactive lanes are a mask. Refills use
//! `vpexpandd` (one expand places words into exactly the lanes that need
//! them, in lane order). Table lookups and coefficient stores use either
//! hardware gathers/scatter (magetypes `gather_wrapping`/`scatter_select`,
//! bounds-safe, no `unsafe` in this crate) or per-lane scalar lookups,
//! chosen per process by `kernel_choice`.

use super::coef::{EOB_CTX, MAX_TABLES, NRAW_PAD, REM_CLASSES, ZIGZAG, ZRL};
use crate::rans::{LOG_M, NL, RANS_L, Words};
use archmage::prelude::*;
use magetypes::simd::generic::{i32x16, u32x16};

#[rite(import_intrinsics)]
pub(super) fn refill(
    _t: X64V4Token,
    x: __m512i,
    need: __mmask16,
    w: &Words<'_>,
    wp: &mut usize,
) -> __m512i {
    let wv = w.chunk(*wp);
    let v = _mm512_maskz_expand_epi32(need, _mm512_cvtepu16_epi32(_mm256_loadu_si256(wv)));
    *wp += need.count_ones() as usize;
    _mm512_mask_mov_epi32(x, need, _mm512_or_si512(_mm512_slli_epi32::<16>(x), v))
}

/// Bit length (0 for 0).
#[rite(import_intrinsics)]
pub(super) fn bitlen(_t: X64V4Token, v: __m512i) -> __m512i {
    _mm512_sub_epi32(_mm512_set1_epi32(32), _mm512_lzcnt_epi32(v))
}

#[rite(import_intrinsics)]
pub(super) fn st16(_t: X64V4Token, v: __m512i) -> [u32; 16] {
    let mut a = [0u32; 16];
    _mm512_storeu_si512(&mut a, v);
    a
}

/// Pulls `n` (0..=16 per lane) raw bits from lanes with `n > 0`.
#[rite(import_intrinsics)]
fn pull(t: X64V4Token, x: &mut __m512i, n: __m512i, w: &Words<'_>, wp: &mut usize) -> __m512i {
    let one = _mm512_set1_epi32(1);
    let bits = _mm512_and_si512(*x, _mm512_sub_epi32(_mm512_sllv_epi32(one, n), one));
    let has = _mm512_cmpgt_epi32_mask(n, _mm512_setzero_si512());
    let xx = _mm512_mask_mov_epi32(*x, has, _mm512_srlv_epi32(*x, n));
    let need = has & _mm512_cmplt_epu32_mask(xx, _mm512_set1_epi32(RANS_L as i32));
    *x = refill(t, xx, need, w, wp);
    bits
}

/// Vector lane state of one stripe.
#[derive(Clone, Copy)]
struct St {
    x: __m512i,
    k: __m512i,
    eob: __m512i,
    ch: __m512i,
    luma: __m512i,
    bx: __m512i,
    bi: __m512i,
    endv: __m512i,
    live: __mmask16,
    wp: usize,
    /// LAB (coefgap decode-cost proxy): the block's EOB context offset.
    bk: __m512i,
}

/// Constants shared by the phases.
#[derive(Clone, Copy)]
struct Kc {
    lane: __m512i,
    bwv: __m512i,
    ch0v: __m512i,
    nchv: __m512i,
    zz: [__m512i; 4],
}

#[rite(import_intrinsics)]
fn st_new(
    _t: X64V4Token,
    x0: &[u32; NL],
    nby: usize,
    bw: usize,
    active: usize,
    lane: __m512i,
) -> St {
    St {
        x: _mm512_loadu_si512(x0),
        k: _mm512_setzero_si512(),
        eob: _mm512_setzero_si512(),
        ch: _mm512_setzero_si512(),
        luma: _mm512_setzero_si512(),
        bx: lane,
        bi: lane,
        endv: _mm512_set1_epi32((nby * bw) as i32),
        live: if nby == 0 {
            0
        } else {
            ((1u32 << active) - 1) as __mmask16
        },
        wp: 0,
        bk: _mm512_setzero_si512(),
    }
}

#[rite(import_intrinsics)]
fn kc_new(_t: X64V4Token, bw: usize, ch0: u32, nch: u32) -> Kc {
    let lanes: [u32; 16] = core::array::from_fn(|i| i as u32);
    Kc {
        lane: _mm512_loadu_si512(&lanes),
        bwv: _mm512_set1_epi32(bw as i32),
        ch0v: _mm512_set1_epi32(ch0 as i32),
        nchv: _mm512_set1_epi32(nch as i32),
        // ZIGZAG as four 16-lane i32 tables.
        zz: core::array::from_fn(|i| {
            let a: [u32; 16] = core::array::from_fn(|j| ZIGZAG[i * 16 + j] as u32);
            _mm512_loadu_si512(&a)
        }),
    }
}

/// Phase 1: context, table lookups, rANS update and refill; returns the
/// table entries.
#[rite(import_intrinsics)]
#[allow(clippy::too_many_arguments)]
fn phase_sym<const B: bool>(
    t: X64V4Token,
    hw: bool,
    s: &mut St,
    kc: &Kc,
    words: &Words<'_>,
    cmap: &[u8; NRAW_PAD],
    cmap32: &[u32; NRAW_PAD],
    dec: &[u32; MAX_TABLES << LOG_M],
) -> __m512i {
    let zero = _mm512_setzero_si512();
    let one = _mm512_set1_epi32(1);
    let is_eob = _mm512_cmpeq_epi32_mask(s.k, zero);
    // Contexts use the absolute channel (group's first + relative).
    let cha = _mm512_add_epi32(s.ch, kc.ch0v);
    let mut rawe = _mm512_add_epi32(_mm512_slli_epi32::<3>(cha), bitlen(t, s.luma));
    if B {
        rawe = _mm512_add_epi32(rawe, s.bk);
    }
    let rem1 = _mm512_add_epi32(_mm512_sub_epi32(s.eob, s.k), one);
    let rc = _mm512_min_epi32(
        _mm512_sub_epi32(bitlen(t, rem1), one),
        _mm512_set1_epi32(REM_CLASSES as i32 - 1),
    );
    let ck = _mm512_sub_epi32(
        _mm512_add_epi32(_mm512_sub_epi32(_mm512_slli_epi32::<6>(cha), cha), s.k),
        one,
    );
    let ck6 = _mm512_add_epi32(_mm512_slli_epi32::<2>(ck), _mm512_slli_epi32::<1>(ck));
    let rawa = _mm512_add_epi32(_mm512_add_epi32(ck6, _mm512_set1_epi32(EOB_CTX as i32)), rc);
    let raw = _mm512_maskz_mov_epi32(s.live, _mm512_mask_mov_epi32(rawa, is_eob, rawe));
    let c1023 = _mm512_set1_epi32(1023);
    let e = if hw {
        // Two hardware gathers (bounds-safe magetypes wrappers).
        let tb =
            u32x16::<X64V4Token>::gather_wrapping(cmap32, u32x16::<X64V4Token>::from_raw_t(t, raw))
                .raw();
        let di = _mm512_or_si512(_mm512_slli_epi32::<LOG_M>(tb), _mm512_and_si512(s.x, c1023));
        u32x16::<X64V4Token>::gather_wrapping(dec, u32x16::<X64V4Token>::from_raw_t(t, di)).raw()
    } else {
        let ra = st16(t, raw);
        let xa = st16(t, s.x);
        let ev: [u32; 16] = core::array::from_fn(|i| {
            let tb = cmap[ra[i] as usize & (NRAW_PAD - 1)] as usize;
            dec[((tb << LOG_M) | (xa[i] & 1023) as usize) & ((MAX_TABLES << LOG_M) - 1)]
        });
        _mm512_loadu_si512(&ev)
    };
    let f = _mm512_add_epi32(_mm512_and_si512(e, c1023), one);
    let bias = _mm512_and_si512(_mm512_srli_epi32::<10>(e), c1023);
    let nx = _mm512_add_epi32(_mm512_mullo_epi32(f, _mm512_srli_epi32::<10>(s.x)), bias);
    s.x = _mm512_mask_mov_epi32(s.x, s.live, nx);
    let need = s.live & _mm512_cmplt_epu32_mask(s.x, _mm512_set1_epi32(RANS_L as i32));
    s.x = refill(t, s.x, need, words, &mut s.wp);
    e
}

/// Phase 2: extra bits (low part, then high part).
#[rite(import_intrinsics)]
fn phase_extra(t: X64V4Token, s: &mut St, e: __m512i, words: &Words<'_>) -> __m512i {
    let zero = _mm512_setzero_si512();
    let nb = _mm512_maskz_srli_epi32::<27>(s.live, e);
    let mut extra = zero;
    if _mm512_test_epi32_mask(nb, nb) != 0 {
        let s16 = _mm512_set1_epi32(16);
        extra = pull(t, &mut s.x, _mm512_min_epi32(nb, s16), words, &mut s.wp);
        let hi = _mm512_max_epi32(_mm512_sub_epi32(nb, s16), zero);
        if _mm512_test_epi32_mask(hi, hi) != 0 {
            let b = pull(t, &mut s.x, hi, words, &mut s.wp);
            extra = _mm512_or_si512(extra, _mm512_slli_epi32::<16>(b));
        }
    }
    extra
}

/// Phase 3: value, stores and state transitions.
#[rite(import_intrinsics)]
#[allow(clippy::too_many_arguments)]
fn phase_apply<const B: bool>(
    t: X64V4Token,
    hw: bool,
    s: &mut St,
    kc: &Kc,
    e: __m512i,
    extra: __m512i,
    q: &mut [i32],
    eobs: &mut [u8],
    bkt: &[u32],
) {
    let zero = _mm512_setzero_si512();
    let one = _mm512_set1_epi32(1);
    let live = s.live;
    let is_eob = _mm512_cmpeq_epi32_mask(s.k, zero);
    let sym = _mm512_and_si512(_mm512_srli_epi32::<20>(e), _mm512_set1_epi32(127));
    let is_zrl = _mm512_cmpeq_epi32_mask(sym, _mm512_set1_epi32(ZRL as i32));
    let cls = _mm512_and_si512(sym, _mm512_set1_epi32(31));
    let adv = _mm512_mask_mov_epi32(_mm512_srli_epi32::<5>(sym), is_zrl, _mm512_set1_epi32(4));
    let p = _mm512_add_epi32(s.k, adv);
    let four = _mm512_set1_epi32(4);
    let sh = _mm512_max_epi32(_mm512_sub_epi32(cls, _mm512_set1_epi32(5)), zero);
    let big = _mm512_maskz_mov_epi32(
        _mm512_cmpgt_epi32_mask(cls, four),
        _mm512_add_epi32(_mm512_sllv_epi32(one, sh), _mm512_srli_epi32::<1>(extra)),
    );
    let m = _mm512_add_epi32(_mm512_set1_epi32(5), big);
    let mag = _mm512_mask_mov_epi32(
        m,
        _mm512_cmplt_epi32_mask(cls, four),
        _mm512_add_epi32(cls, one),
    );
    let sign = _mm512_sub_epi32(zero, _mm512_and_si512(extra, one));
    let v = _mm512_sub_epi32(_mm512_xor_si512(mag, sign), sign);
    // Stores: compress the lanes that hold a coefficient / an EOB and
    // store only those (zigzag position looked up in registers).
    let bc = _mm512_add_epi32(_mm512_mullo_epi32(s.bi, kc.nchv), s.ch);
    let ac_lane = live & !is_eob;
    let coef_lane = ac_lane & !is_zrl & !_mm512_cmpgt_epi32_mask(p, s.eob);
    let eob_lane = live & is_eob;
    if coef_lane != 0 {
        let zz = &kc.zz;
        let pm = _mm512_and_si512(p, _mm512_set1_epi32(63));
        let zlo = _mm512_permutex2var_epi32(zz[0], pm, zz[1]);
        let zhi = _mm512_permutex2var_epi32(zz[2], pm, zz[3]);
        let z = _mm512_mask_mov_epi32(zlo, _mm512_test_epi32_mask(pm, _mm512_set1_epi32(32)), zhi);
        let addr = _mm512_add_epi32(_mm512_slli_epi32::<6>(bc), z);
        if hw {
            // Clamp first (corrupt streams may index past `q`), then the
            // bounds-safe magetypes scatter writes the enabled lanes.
            let qa = u32x16::<X64V4Token>::from_raw_t(
                t,
                _mm512_min_epu32(addr, _mm512_set1_epi32((q.len() - 1) as i32)),
            );
            i32x16::<X64V4Token>::from_raw_t(t, v).scatter_select(q, coef_lane, qa);
        } else {
            let n = coef_lane.count_ones() as usize;
            let aa = st16(t, _mm512_maskz_compress_epi32(coef_lane, addr));
            let vv = st16(t, _mm512_maskz_compress_epi32(coef_lane, v));
            let qlast = q.len() - 1;
            for i in 0..n {
                q[(aa[i] as usize).min(qlast)] = vv[i] as i32;
            }
        }
    }
    if eob_lane != 0 {
        let n = eob_lane.count_ones() as usize;
        let ea = st16(t, _mm512_maskz_compress_epi32(eob_lane, bc));
        let sa = st16(t, _mm512_maskz_compress_epi32(eob_lane, sym));
        let elast = eobs.len() - 1;
        for i in 0..n {
            eobs[(ea[i] as usize).min(elast)] = sa[i] as u8;
        }
    }
    // Transitions.
    s.eob = _mm512_mask_mov_epi32(s.eob, eob_lane, sym);
    s.luma = _mm512_mask_mov_epi32(s.luma, eob_lane & _mm512_cmpeq_epi32_mask(s.ch, zero), sym);
    // A coefficient at p >= eob ends the channel; ZRL ends it past eob.
    let end_ac = (_mm512_cmpgt_epi32_mask(p, s.eob) & is_zrl)
        | (_mm512_cmpge_epi32_mask(p, s.eob) & !is_zrl);
    let next_ch = live & ((is_eob & _mm512_cmpeq_epi32_mask(sym, zero)) | (!is_eob & end_ac));
    // Next k: 1 after an EOB token, p after ZRL, p + 1 after a coefficient.
    let kac = _mm512_mask_add_epi32(p, !is_zrl, p, one);
    let nk = _mm512_mask_mov_epi32(kac, is_eob, one);
    let nk = _mm512_maskz_mov_epi32(!next_ch, nk);
    s.k = _mm512_mask_mov_epi32(s.k, live, nk);
    s.ch = _mm512_mask_add_epi32(s.ch, next_ch, s.ch, one);
    let wrap = _mm512_cmpeq_epi32_mask(s.ch, kc.nchv);
    s.ch = _mm512_mask_mov_epi32(s.ch, wrap, zero);
    let sixteen = _mm512_set1_epi32(NL as i32);
    let nbx = _mm512_mask_add_epi32(s.bx, wrap, s.bx, sixteen);
    let nbi = _mm512_mask_add_epi32(s.bi, wrap, s.bi, sixteen);
    let roww = wrap & _mm512_cmpge_epi32_mask(nbx, kc.bwv);
    // Row start: bi - bx + bw + lane.
    let rs = _mm512_add_epi32(
        _mm512_add_epi32(_mm512_sub_epi32(s.bi, s.bx), kc.bwv),
        kc.lane,
    );
    s.bx = _mm512_mask_mov_epi32(nbx, roww, kc.lane);
    s.bi = _mm512_mask_mov_epi32(nbi, roww, rs);
    let fin = roww & _mm512_cmpge_epi32_mask(s.bi, s.endv);
    s.live &= !fin;
    if B && wrap != 0 {
        load_bk(t, hw, s, wrap, bkt);
    }
}

/// LAB (coefgap decode-cost proxy): loads the EOB context offsets of the
/// blocks the `m` lanes start (`bkt[bi]`, kept when out of range).
#[rite(import_intrinsics)]
fn load_bk(t: X64V4Token, hw: bool, s: &mut St, m: __mmask16, bkt: &[u32]) {
    if hw {
        let idx = _mm512_mask_mov_epi32(_mm512_set1_epi32(-1), m, s.bi);
        s.bk = u32x16::<X64V4Token>::gather_or(
            bkt,
            u32x16::<X64V4Token>::from_raw_t(t, idx),
            u32x16::<X64V4Token>::from_raw_t(t, s.bk),
        )
        .raw();
    } else {
        let ia = st16(t, s.bi);
        let mut ba = st16(t, s.bk);
        for (i, b) in ba.iter_mut().enumerate() {
            if m & (1 << i) != 0
                && let Some(&v) = bkt.get(ia[i] as usize)
            {
                *b = v;
            }
        }
        s.bk = _mm512_loadu_si512(&ba);
    }
}

/// Context tables for the lookups: `cmap` for software lookups, `cmap32`
/// (the same map widened) for hardware gathers.
pub(crate) struct Maps<'a> {
    pub cmap: &'a [u8; NRAW_PAD],
    pub cmap32: &'a [u32; NRAW_PAD],
    pub dec: &'a [u32; MAX_TABLES << LOG_M],
}

#[rite(import_intrinsics)]
#[allow(clippy::too_many_arguments)]
fn run1<const B: bool>(
    t: X64V4Token,
    hw: bool,
    words: &Words<'_>,
    x0: [u32; NL],
    m: &Maps<'_>,
    bw: usize,
    nby: usize,
    active: usize,
    ch0: u32,
    nch: u32,
    q: &mut [i32],
    eobs: &mut [u8],
    bkt: &[u32],
) -> usize {
    let kc = kc_new(t, bw, ch0, nch);
    let mut s = st_new(t, &x0, nby, bw, active, kc.lane);
    if B {
        let live = s.live;
        load_bk(t, hw, &mut s, live, bkt);
    }
    while s.live != 0 {
        let e = phase_sym::<B>(t, hw, &mut s, &kc, words, m.cmap, m.cmap32, m.dec);
        let extra = phase_extra(t, &mut s, e, words);
        phase_apply::<B>(t, hw, &mut s, &kc, e, extra, q, eobs, bkt);
        // Past the end of the stream the decode is already invalid.
        if s.wp > words.len {
            break;
        }
    }
    s.wp
}

#[rite(import_intrinsics)]
#[allow(clippy::too_many_arguments)]
fn run2<const B: bool, const DIFFERENT: bool>(
    t: X64V4Token,
    hw: bool,
    words: [&Words<'_>; 2],
    x0: [[u32; NL]; 2],
    m: [&Maps<'_>; 2],
    bw: usize,
    nby: [usize; 2],
    active: usize,
    ch0: u32,
    nch: u32,
    q: [&mut [i32]; 2],
    eobs: [&mut [u8]; 2],
    bkt: [&[u32]; 2],
) -> [usize; 2] {
    let kc = kc_new(t, bw, ch0, nch);
    let mut a = st_new(t, &x0[0], nby[0], bw, active, kc.lane);
    let mut b = st_new(t, &x0[1], nby[1], bw, active, kc.lane);
    if B {
        let (la, lb) = (a.live, b.live);
        load_bk(t, hw, &mut a, la, bkt[0]);
        load_bk(t, hw, &mut b, lb, bkt[1]);
    }
    let [qa, qb] = q;
    let [ea, eb] = eobs;
    let ma = m[0];
    let mb = if DIFFERENT { m[1] } else { m[0] };
    while (a.live | b.live) != 0 {
        let e0 = phase_sym::<B>(t, hw, &mut a, &kc, words[0], ma.cmap, ma.cmap32, ma.dec);
        let e1 = phase_sym::<B>(t, hw, &mut b, &kc, words[1], mb.cmap, mb.cmap32, mb.dec);
        let x0 = phase_extra(t, &mut a, e0, words[0]);
        let x1 = phase_extra(t, &mut b, e1, words[1]);
        phase_apply::<B>(t, hw, &mut a, &kc, e0, x0, qa, ea, bkt[0]);
        phase_apply::<B>(t, hw, &mut b, &kc, e1, x1, qb, eb, bkt[1]);
        // Past the end of its stream a stripe's decode is already invalid.
        if a.wp > words[0].len {
            a.live = 0;
        }
        if b.wp > words[1].len {
            b.live = 0;
        }
    }
    [a.wp, b.wp]
}

/// Decodes the stripe's lane streams into `q` (raster order) / `eobs`,
/// zeroed by the caller, with 64 / 1 spare slots at the end; `hw` selects
/// hardware gathers/scatter or software lookups and compressed scalar
/// stores (bit-identical). Returns the number of words consumed.
#[arcane(import_intrinsics)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_stripe(
    t: X64V4Token,
    hw: bool,
    words: &Words<'_>,
    x0: [u32; NL],
    m: &Maps<'_>,
    bw: usize,
    nby: usize,
    active: usize,
    ch0: u32,
    nch: u32,
    q: &mut [i32],
    eobs: &mut [u8],
    bkt: Option<&[u32]>,
) -> usize {
    match (hw, bkt) {
        (true, None) => run1::<false>(
            t,
            true,
            words,
            x0,
            m,
            bw,
            nby,
            active,
            ch0,
            nch,
            q,
            eobs,
            &[],
        ),
        (false, None) => run1::<false>(
            t,
            false,
            words,
            x0,
            m,
            bw,
            nby,
            active,
            ch0,
            nch,
            q,
            eobs,
            &[],
        ),
        (true, Some(b)) => {
            run1::<true>(t, true, words, x0, m, bw, nby, active, ch0, nch, q, eobs, b)
        }
        (false, Some(b)) => run1::<true>(
            t, false, words, x0, m, bw, nby, active, ch0, nch, q, eobs, b,
        ),
    }
}

/// Two stripes at once (same group and block width): each phase runs for
/// stripe A, then stripe B, so the two independent dependency chains
/// overlap. Per stripe identical to [`decode_stripe`]; returns the words
/// consumed by each.
#[arcane(import_intrinsics)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_stripes2(
    t: X64V4Token,
    hw: bool,
    words: [&Words<'_>; 2],
    x0: [[u32; NL]; 2],
    m: &Maps<'_>,
    bw: usize,
    nby: [usize; 2],
    active: usize,
    ch0: u32,
    nch: u32,
    q: [&mut [i32]; 2],
    eobs: [&mut [u8]; 2],
    bkt: Option<[&[u32]; 2]>,
) -> [usize; 2] {
    match (hw, bkt) {
        (true, None) => run2::<false, false>(
            t,
            true,
            words,
            x0,
            [m, m],
            bw,
            nby,
            active,
            ch0,
            nch,
            q,
            eobs,
            [&[], &[]],
        ),
        (false, None) => run2::<false, false>(
            t,
            false,
            words,
            x0,
            [m, m],
            bw,
            nby,
            active,
            ch0,
            nch,
            q,
            eobs,
            [&[], &[]],
        ),
        (true, Some(b)) => run2::<true, false>(
            t,
            true,
            words,
            x0,
            [m, m],
            bw,
            nby,
            active,
            ch0,
            nch,
            q,
            eobs,
            b,
        ),
        (false, Some(b)) => run2::<true, false>(
            t,
            false,
            words,
            x0,
            [m, m],
            bw,
            nby,
            active,
            ch0,
            nch,
            q,
            eobs,
            b,
        ),
    }
}

/// Test override of the AVX-512 kernels: 0 automatic, 1 hardware gathers,
/// 2 software lookups.
pub(crate) static FORCE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// Two explicit table sets, selected once before the entropy loop.
#[arcane(import_intrinsics)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_stripes2_models(
    t: X64V4Token,
    hw: bool,
    words: [&Words<'_>; 2],
    x0: [[u32; NL]; 2],
    m: [&Maps<'_>; 2],
    bw: usize,
    nby: [usize; 2],
    active: usize,
    ch0: u32,
    nch: u32,
    q: [&mut [i32]; 2],
    eobs: [&mut [u8]; 2],
    bkt: Option<[&[u32]; 2]>,
) -> [usize; 2] {
    match (hw, bkt) {
        (true, None) => run2::<false, true>(
            t,
            true,
            words,
            x0,
            m,
            bw,
            nby,
            active,
            ch0,
            nch,
            q,
            eobs,
            [&[], &[]],
        ),
        (false, None) => run2::<false, true>(
            t,
            false,
            words,
            x0,
            m,
            bw,
            nby,
            active,
            ch0,
            nch,
            q,
            eobs,
            [&[], &[]],
        ),
        (true, Some(b)) => {
            run2::<true, true>(t, true, words, x0, m, bw, nby, active, ch0, nch, q, eobs, b)
        }
        (false, Some(b)) => run2::<true, true>(
            t, false, words, x0, m, bw, nby, active, ch0, nch, q, eobs, b,
        ),
    }
}
