#![allow(
    dead_code,
    unused_mut,
    unused_variables,
    private_interfaces,
    clippy::all
)]
//! Lab: 16-lane interleaved rANS with step-major lane layout.
//!
//! Layout per stripe and slot: `u[step * 16 + lane]` where
//! `step = row * lw + x`. Lane `l` covers image columns
//! `l*lw .. min((l+1)*lw, width)`; positions past a lane's width are
//! inactive (u = 0, nothing coded).
//!
//! Stream per (stripe, slot): 16 initial u32 states, then u16 words in
//! decode-consumption order: for each step, token phase (lanes ascending)
//! then extra-bits phase (lanes ascending).

use alloc::vec;
use alloc::vec::Vec;

use crate::ans;
use crate::coding_layout::CTX_CLASSES;
use crate::encode::choose_ctx_map;
use crate::lab::LaneStripe;
use crate::tokens::{TokenCfg, hybrid_base};

pub const NL: usize = 16;
pub const NCTX: usize = 4;
pub const CLASSES16: usize = 16;
const RANS_L: u32 = 1 << 16;

/// One stripe in step-major layout.
pub struct IStripe {
    pub rows: usize,
    pub lw: usize,
    /// Active width of each lane.
    pub widths: [usize; NL],
    /// Per slot: u[step * NL + lane].
    pub u: Vec<Vec<u16>>,
}

pub fn interleave(st: &LaneStripe) -> IStripe {
    assert!(st.widths.len() <= NL);
    let lw = st.widths[0];
    let mut widths = [0usize; NL];
    for (l, &w) in st.widths.iter().enumerate() {
        widths[l] = w;
    }
    let steps = st.rows * lw;
    let u =
        st.u.iter()
            .map(|plane| {
                let mut v = vec![0u16; steps * NL];
                for l in 0..st.widths.len() {
                    let off = st.lane_offset(l);
                    let w = st.widths[l];
                    for r in 0..st.rows {
                        for x in 0..w {
                            v[(r * lw + x) * NL + l] = plane[off + r * w + x];
                        }
                    }
                }
                v
            })
            .collect();
    IStripe {
        rows: st.rows,
        lw,
        widths,
        u,
    }
}

#[inline(always)]
pub fn class16(sum: u32) -> usize {
    ((32 - sum.leading_zeros()) as usize).min(CLASSES16 - 1)
}

/// Context sum at (step j, lane l) given the slot plane and optional cross plane.
#[inline(always)]
fn ctx_sum(u: &[u16], cross: Option<&[u16]>, lw: usize, j: usize, l: usize) -> u32 {
    let x = j % lw;
    let row = j / lw;
    let uw = if x > 0 { u[(j - 1) * NL + l] as u32 } else { 0 };
    let s = match (x > 0, row > 0) {
        (true, true) => uw + u[(j - lw) * NL + l] as u32,
        (true, false) => 2 * uw,
        (false, true) => 2 * u[(j - lw) * NL + l] as u32,
        (false, false) => 0,
    };
    s + cross.map_or(0, |c| c[j * NL + l] as u32)
}

#[derive(Clone, Copy)]
struct Sym {
    j: u32,
    ctx: u8,
    tok: u8,
    nb: u8,
    extra: u32,
}

pub struct RansModel {
    pub log_m: u32,
    pub ctx_map: [u8; CLASSES16],
    /// Decode table [ctx * M + slot] = sym | bias << 7 | freq << 19.
    pub dec: Vec<u32>,
    /// v2 entries (log_m == 10): (freq-1) | bias << 10 | sym << 20 | nb << 28.
    pub dec2: Vec<u32>,
    /// Token info `[sym] = base | nb << 16 | is_run << 21`.
    pub tok: [u32; 256],
    freq: Vec<Vec<u32>>,
    cum: Vec<Vec<u32>>,
    pub tc: TokenCfg,
    pub cross: bool,
}

pub struct RansEncoded {
    pub models: Vec<RansModel>,
    /// `[stripe][slot]` -> u16 words (first 32 words = 16 initial states).
    pub streams: Vec<Vec<Vec<u16>>>,
    pub payload_bytes: usize,
}

fn lane_syms(
    st: &IStripe,
    s: usize,
    cross: bool,
    tc: &TokenCfg,
    run_min: u32,
    map: &[u8; CLASSES16],
    l: usize,
    out: &mut Vec<Sym>,
) {
    let u = &st.u[s];
    let cr = if cross {
        Some(st.u[0].as_slice())
    } else {
        None
    };
    let w = st.widths[l];
    let lw = st.lw;
    let steps = st.rows * lw;
    // Active positions in lane order.
    let active = |j: usize| j % lw < w;
    let mut j = 0;
    while j < steps {
        if !active(j) {
            j += 1;
            continue;
        }
        let ctx = map[class16(ctx_sum(u, cr, lw, j, l))];
        let v = u[j * NL + l] as u32;
        if v == 0 && run_min > 0 {
            // Count zeros over active positions.
            let mut len = 0usize;
            let mut jj = j;
            while jj < steps {
                if active(jj) {
                    if u[jj * NL + l] != 0 {
                        break;
                    }
                    len += 1;
                }
                jj += 1;
            }
            if len as u32 >= run_min {
                // Keep run extra bits <= 14 so one 16-bit renorm suffices.
                let len = len.min(1 << 15);
                let (t, e, nb) = tc.run(len as u32);
                out.push(Sym {
                    j: j as u32,
                    ctx,
                    tok: t as u8,
                    nb: nb as u8,
                    extra: e,
                });
                // Skip len active positions.
                let mut left = len;
                while left > 0 {
                    if active(j) {
                        left -= 1;
                    }
                    j += 1;
                }
                continue;
            }
        }
        let (t, e, nb) = tc.value(v);
        out.push(Sym {
            j: j as u32,
            ctx,
            tok: t as u8,
            nb: nb as u8,
            extra: e,
        });
        j += 1;
    }
}

pub fn encode(
    stripes: &[IStripe],
    slot_bits: &[u32],
    color_slots: usize,
    log_m: u32,
    k: u32,
    m: u32,
    run_min: u32,
) -> RansEncoded {
    let ns = slot_bits.len();
    let mm = 1usize << log_m;
    let mut models = Vec::with_capacity(ns);
    let ident: [u8; CLASSES16] = core::array::from_fn(|i| i as u8);
    for s in 0..ns {
        // Run tokens share the value token scheme (k, m).
        let tc = TokenCfg::new(k, m, slot_bits[s], k, m);
        assert!(tc.alphabet() <= 256);
        let cross = s >= 1 && s < color_slots;
        let asz = tc.alphabet() as usize;
        let mut hist = vec![vec![0u64; asz]; CTX_CLASSES];
        let mut syms = Vec::new();
        for st in stripes {
            for l in 0..NL {
                syms.clear();
                lane_syms(st, s, cross, &tc, run_min, &ident, l, &mut syms);
                for y in &syms {
                    hist[y.ctx as usize][y.tok as usize] += 1;
                }
            }
        }
        let (map18, groups) = choose_ctx_map(&hist, NCTX, 12);
        let mut ctx_map = [0u8; CLASSES16];
        ctx_map.copy_from_slice(&map18[..CLASSES16]);
        let mut dec = vec![0u32; NCTX * mm];
        let mut dec2 = vec![0u32; NCTX * mm];
        let mut freq = Vec::new();
        let mut cum = Vec::new();
        for c in 0..NCTX {
            let g = groups.get(c).cloned().unwrap_or_else(|| vec![0u64; asz]);
            let mut q = ans::normalize(&g, log_m).unwrap();
            if q.iter().all(|&v| v == 0) {
                q[0] = mm as u32;
            }
            let mut cm = vec![0u32; asz + 1];
            for i in 0..asz {
                cm[i + 1] = cm[i] + q[i];
            }
            for sym in 0..asz {
                for slot in cm[sym]..cm[sym + 1] {
                    dec[c * mm + slot as usize] =
                        (sym as u32 & 127) | ((slot - cm[sym]) << 7) | (q[sym] << 19);
                    dec2[c * mm + slot as usize] =
                        (q[sym] - 1) | ((slot - cm[sym]) << 10) | ((sym as u32) << 20);
                }
            }
            freq.push(q);
            cum.push(cm);
        }
        let mut tok = [0u32; 256];
        let vt = tc.value_tokens();
        for t in 0..asz as u32 {
            tok[t as usize] = if t < vt {
                let (b, nb) = hybrid_base(t, tc.k, tc.m);
                b | (nb << 16)
            } else {
                let (b, nb) = hybrid_base(t - vt, tc.rk, tc.rm);
                b | (nb << 16) | (1 << 21)
            };
        }
        if log_m == 10 {
            for e in dec2.iter_mut() {
                let sym = (*e >> 20) & 255;
                *e |= ((tok[sym as usize] >> 16) & 15) << 28;
            }
        }
        models.push(RansModel {
            log_m,
            ctx_map,
            dec,
            dec2,
            tok,
            freq,
            cum,
            tc,
            cross,
        });
    }
    let mut streams = Vec::with_capacity(stripes.len());
    let mut payload = 0usize;
    for st in stripes {
        let mut per = Vec::with_capacity(ns);
        for s in 0..ns {
            let md = &models[s];
            let mut states = [0u32; NL];
            // (j, phase, lane, word)
            let mut words: Vec<(u32, u8, u8, u16)> = Vec::new();
            let mut syms = Vec::new();
            for l in 0..NL {
                syms.clear();
                lane_syms(st, s, md.cross, &md.tc, run_min, &md.ctx_map, l, &mut syms);
                let mut x = RANS_L;
                for y in syms.iter().rev() {
                    // Extra bits (decoded after the token): encode first.
                    if y.nb > 0 {
                        let nb = y.nb as u32;
                        let x_max = (RANS_L >> nb) << 16;
                        while x >= x_max {
                            words.push((y.j, 1, l as u8, x as u16));
                            x >>= 16;
                        }
                        x = (x << nb) | y.extra;
                    }
                    let f = md.freq[y.ctx as usize][y.tok as usize];
                    let c = md.cum[y.ctx as usize][y.tok as usize];
                    let x_max = ((RANS_L >> log_m) << 16) * f;
                    while x >= x_max {
                        words.push((y.j, 0, l as u8, x as u16));
                        x >>= 16;
                    }
                    x = ((x / f) << log_m) + (x % f) + c;
                }
                states[l] = x;
            }
            // Words were produced in reverse decode order per lane; sort into
            // decode-consumption order. Within one (j, phase, lane) at most one word.
            words.sort_by_key(|&(j, p, l, _)| (j, p, l));
            let mut out = Vec::with_capacity(2 * NL + words.len());
            for &x in &states {
                out.push(x as u16);
                out.push((x >> 16) as u16);
            }
            out.extend(words.iter().map(|w| w.3));
            payload += out.len() * 2;
            // Padding so SIMD refills may load 16 words past the end.
            out.extend([0u16; 16]);
            per.push(out);
        }
        streams.push(per);
    }
    RansEncoded {
        models,
        streams,
        payload_bytes: payload,
    }
}

/// Scalar reference decoder (same consumption order as the SIMD kernels).
pub fn decode_scalar(enc: &RansEncoded, si: usize, geom: &IStripe, out: &mut [Vec<u16>]) {
    let ns = enc.models.len();
    for s in 0..ns {
        let md = &enc.models[s];
        let words = &enc.streams[si][s];
        let mut x: [u32; NL] =
            core::array::from_fn(|l| words[2 * l] as u32 | (words[2 * l + 1] as u32) << 16);
        let mut wp = 2 * NL;
        let mut run = [0u32; NL];
        let lw = geom.lw;
        let mm = 1u32 << md.log_m;
        let steps = geom.rows * lw;
        let (before, rest) = out.split_at_mut(s);
        let plane = &mut rest[0];
        let cr: Option<&[u16]> = if md.cross { Some(&before[0]) } else { None };
        let mut extra_nb = [0u32; NL];
        let mut base = [0u32; NL];
        let mut is_run = [false; NL];
        let mut decoding = [false; NL];
        for j in 0..steps {
            let xcol = j % lw;
            // Token phase.
            for l in 0..NL {
                decoding[l] = false;
                if xcol >= geom.widths[l] {
                    plane[j * NL + l] = 0;
                    continue;
                }
                if run[l] > 0 {
                    run[l] -= 1;
                    plane[j * NL + l] = 0;
                    continue;
                }
                decoding[l] = true;
                let ctx = md.ctx_map[class16(ctx_sum(plane, cr, lw, j, l))] as usize;
                let slot = x[l] & (mm - 1);
                let e = md.dec[ctx * mm as usize + slot as usize];
                let sym = e & 127;
                let bias = (e >> 7) & 0xFFF;
                let f = e >> 19;
                x[l] = f * (x[l] >> md.log_m) + bias;
                if x[l] < RANS_L {
                    x[l] = (x[l] << 16) | words[wp] as u32;
                    wp += 1;
                }
                let ti = md.tok[sym as usize];
                base[l] = ti & 0xFFFF;
                extra_nb[l] = (ti >> 16) & 31;
                is_run[l] = ti >> 21 != 0;
            }
            // Extra phase.
            for l in 0..NL {
                if !decoding[l] {
                    continue;
                }
                let nb = extra_nb[l];
                let v = x[l] & ((1 << nb) - 1);
                x[l] >>= nb;
                if x[l] < RANS_L {
                    x[l] = (x[l] << 16) | words[wp] as u32;
                    wp += 1;
                }
                let u = base[l] + v;
                if is_run[l] {
                    run[l] = u; // run length - 1 more zeros after this one
                    plane[j * NL + l] = 0;
                } else {
                    plane[j * NL + l] = u as u16;
                }
            }
        }
    }
}

/// Rank LUT for mask-driven refill: for an 8-bit lane mask, lane i takes
/// the word at index popcount(mask & ((1 << i) - 1)).
pub(crate) static RANK8: [[u32; 8]; 256] = {
    let mut t = [[0u32; 8]; 256];
    let mut m = 0;
    while m < 256 {
        let mut i = 0;
        let mut r = 0u32;
        while i < 8 {
            t[m][i] = r;
            if m & (1 << i) != 0 {
                r += 1;
            }
            i += 1;
        }
        m += 1;
    }
    t
};

/// Fixed-size decode table view for bounds-check-free software gathers.
pub struct FlatTables<const N: usize> {
    pub dec: alloc::boxed::Box<[u32; N]>,
}

impl RansModel {
    pub fn flat<const N: usize>(&self) -> FlatTables<N> {
        let mut b: alloc::boxed::Box<[u32; N]> =
            vec![0u32; N].into_boxed_slice().try_into().ok().unwrap();
        b[..self.dec.len()].copy_from_slice(&self.dec);
        FlatTables { dec: b }
    }
}

#[cfg(target_arch = "x86_64")]
pub mod avx2 {
    use super::*;
    use archmage::prelude::*;

    /// Decodes one slot of one stripe. `N` = NCTX << LOG_M.
    #[arcane(import_intrinsics)]
    pub fn decode_slot<const LOG_M: i32, const N: usize>(
        _t: X64V3Token,
        md: &RansModel,
        dec: &[u32; N],
        words: &[u16],
        geom: &IStripe,
        plane: &mut [u16],
        cross: Option<&[u16]>,
    ) {
        let mm = 1u32 << LOG_M as u32;
        let lw = geom.lw;
        let steps = geom.rows * lw;
        let mut xs = [_mm256_setzero_si256(); 2];
        let mut st = [0u32; 16];
        for l in 0..NL {
            st[l] = words[2 * l] as u32 | (words[2 * l + 1] as u32) << 16;
        }
        let lo: &[u32; 8] = st[..8].try_into().unwrap();
        let hi: &[u32; 8] = st[8..].try_into().unwrap();
        xs[0] = _mm256_loadu_si256(lo);
        xs[1] = _mm256_loadu_si256(hi);
        let mut wp = 2 * NL;
        let mut runs = [_mm256_setzero_si256(); 2];
        let mut uw = [_mm256_setzero_si256(); 2];
        let wid: [u32; 16] = core::array::from_fn(|l| geom.widths[l] as u32);
        let widv = [
            _mm256_loadu_si256(<&[u32; 8]>::try_from(&wid[..8]).unwrap()),
            _mm256_loadu_si256(<&[u32; 8]>::try_from(&wid[8..]).unwrap()),
        ];
        let mut cmap = [0u8; 32];
        cmap[..16].copy_from_slice(&md.ctx_map);
        cmap[16..].copy_from_slice(&md.ctx_map);
        let cmapv = _mm256_loadu_si256(&cmap);
        let slot_mask = _mm256_set1_epi32((mm - 1) as i32);
        let zero = _mm256_setzero_si256();
        let one = _mm256_set1_epi32(1);
        let l16 = _mm256_set1_epi32(0xFFFF);
        let tok = &md.tok;
        let mut xcol = 0usize;
        let mut row = 0usize;
        let mut acts = [zero; 2];
        let mut syms = [zero; 2];
        for j in 0..steps {
            for h in 0..2 {
                let base_i = j * NL + h * 8;
                let colv = _mm256_set1_epi32(xcol as i32);
                // active = width > xcol (signed compare fine: small values)
                let inw = _mm256_cmpgt_epi32(widv[h], colv);
                let in_run = _mm256_and_si256(_mm256_cmpgt_epi32(runs[h], zero), inw);
                runs[h] = _mm256_sub_epi32(runs[h], _mm256_and_si256(in_run, one));
                let act = _mm256_andnot_si256(in_run, inw);
                // Context.
                let un = if row > 0 {
                    let p: &[u16; 8] = plane[base_i - lw * NL..base_i - lw * NL + 8]
                        .try_into()
                        .unwrap();
                    _mm256_cvtepu16_epi32(_mm_loadu_si128(p))
                } else {
                    zero
                };
                let mut sum = match (xcol > 0, row > 0) {
                    (true, true) => _mm256_add_epi32(uw[h], un),
                    (true, false) => _mm256_add_epi32(uw[h], uw[h]),
                    (false, true) => _mm256_add_epi32(un, un),
                    (false, false) => zero,
                };
                if let Some(c) = cross {
                    let p: &[u16; 8] = c[base_i..base_i + 8].try_into().unwrap();
                    sum = _mm256_add_epi32(sum, _mm256_cvtepu16_epi32(_mm_loadu_si128(p)));
                }
                // class = min(bitlen(sum), 15) via float exponent.
                let e = _mm256_srli_epi32::<23>(_mm256_castps_si256(_mm256_cvtepi32_ps(sum)));
                let cls = _mm256_min_epi32(
                    _mm256_max_epi32(_mm256_sub_epi32(e, _mm256_set1_epi32(126)), zero),
                    _mm256_set1_epi32(15),
                );
                let ctx =
                    _mm256_and_si256(_mm256_shuffle_epi8(cmapv, cls), _mm256_set1_epi32(0xFF));
                let x = xs[h];
                let slot = _mm256_and_si256(x, slot_mask);
                let idx = _mm256_or_si256(_mm256_slli_epi32::<LOG_M>(ctx), slot);
                // Software gather.
                let mut ia = [0u32; 8];
                _mm256_storeu_si256(&mut ia, idx);
                let ev: [u32; 8] = core::array::from_fn(|i| dec[ia[i] as usize & (N - 1)]);
                let e = _mm256_loadu_si256(&ev);
                let sym = _mm256_and_si256(e, _mm256_set1_epi32(127));
                let bias = _mm256_and_si256(_mm256_srli_epi32::<7>(e), _mm256_set1_epi32(0xFFF));
                let f = _mm256_srli_epi32::<19>(e);
                let nx =
                    _mm256_add_epi32(_mm256_mullo_epi32(f, _mm256_srli_epi32::<LOG_M>(x)), bias);
                let mut x = _mm256_blendv_epi8(x, nx, act);
                // Renorm (token phase).
                let need =
                    _mm256_and_si256(act, _mm256_cmpeq_epi32(_mm256_srli_epi32::<16>(x), zero));
                let m = _mm256_movemask_ps(_mm256_castsi256_ps(need)) as usize;
                if m != 0 {
                    let wv: &[u16; 8] = words[wp..wp + 8].try_into().unwrap();
                    let w = _mm256_cvtepu16_epi32(_mm_loadu_si128(wv));
                    let perm = _mm256_loadu_si256(&RANK8[m]);
                    let w = _mm256_permutevar8x32_epi32(w, perm);
                    x = _mm256_blendv_epi8(x, _mm256_or_si256(_mm256_slli_epi32::<16>(x), w), need);
                    wp += m.count_ones() as usize;
                }
                xs[h] = x;
                acts[h] = act;
                syms[h] = sym;
            }
            for h in 0..2 {
                let base_i = j * NL + h * 8;
                let act = acts[h];
                let sym = syms[h];
                let mut x = xs[h];
                // Token info gather.
                let mut sa = [0u32; 8];
                _mm256_storeu_si256(&mut sa, sym);
                let tv: [u32; 8] = core::array::from_fn(|i| tok[sa[i] as usize & 255]);
                let ti = _mm256_loadu_si256(&tv);
                let base = _mm256_and_si256(ti, l16);
                let nb = _mm256_and_si256(_mm256_srli_epi32::<16>(ti), _mm256_set1_epi32(31));
                let is_run = _mm256_cmpgt_epi32(_mm256_srli_epi32::<21>(ti), zero);
                // Extra bits phase.
                let ebits = _mm256_and_si256(x, _mm256_sub_epi32(_mm256_sllv_epi32(one, nb), one));
                x = _mm256_blendv_epi8(x, _mm256_srlv_epi32(x, nb), act);
                let need =
                    _mm256_and_si256(act, _mm256_cmpeq_epi32(_mm256_srli_epi32::<16>(x), zero));
                let m = _mm256_movemask_ps(_mm256_castsi256_ps(need)) as usize;
                if m != 0 {
                    let wv: &[u16; 8] = words[wp..wp + 8].try_into().unwrap();
                    let w = _mm256_cvtepu16_epi32(_mm_loadu_si128(wv));
                    let perm = _mm256_loadu_si256(&RANK8[m]);
                    let w = _mm256_permutevar8x32_epi32(w, perm);
                    x = _mm256_blendv_epi8(x, _mm256_or_si256(_mm256_slli_epi32::<16>(x), w), need);
                    wp += m.count_ones() as usize;
                }
                xs[h] = x;
                let u = _mm256_add_epi32(base, ebits);
                let run_now = _mm256_and_si256(act, is_run);
                runs[h] = _mm256_blendv_epi8(runs[h], u, run_now);
                let val = _mm256_andnot_si256(run_now, _mm256_and_si256(act, u));
                uw[h] = val;
                let packed = _mm256_packus_epi32(val, val);
                let packed = _mm256_permute4x64_epi64::<0b1000>(packed);
                let o: &mut [u16; 8] = (&mut plane[base_i..base_i + 8]).try_into().unwrap();
                _mm_storeu_si128(o, _mm256_castsi256_si128(packed));
            }
            xcol += 1;
            if xcol == lw {
                xcol = 0;
                row += 1;
            }
        }
    }
}

/// Decodes all slots of a stripe with the best available kernel.
pub fn decode_simd(
    enc: &RansEncoded,
    si: usize,
    geom: &IStripe,
    out: &mut [Vec<u16>],
    flats: &[FlatTables<{ NCTX << 12 }>],
) {
    #[cfg(target_arch = "x86_64")]
    if let Some(t) = {
        use archmage::SimdToken;
        archmage::X64V3Token::summon()
    } {
        for s in 0..enc.models.len() {
            let md = &enc.models[s];
            let (before, rest) = out.split_at_mut(s);
            let cr: Option<&[u16]> = if md.cross { Some(&before[0]) } else { None };
            match md.log_m {
                12 => avx2::decode_slot::<12, { NCTX << 12 }>(
                    t,
                    md,
                    &flats[s].dec,
                    &enc.streams[si][s],
                    geom,
                    &mut rest[0],
                    cr,
                ),
                10 => {
                    let d: &[u32; NCTX << 10] = flats[s].dec[..NCTX << 10].try_into().unwrap();
                    avx2::decode_slot::<10, { NCTX << 10 }>(
                        t,
                        md,
                        d,
                        &enc.streams[si][s],
                        geom,
                        &mut rest[0],
                        cr,
                    )
                }
                _ => unimplemented!(),
            }
        }
        return;
    }
    decode_scalar(enc, si, geom, out);
}

#[cfg(target_arch = "x86_64")]
pub mod avx2b {
    use super::*;
    use archmage::prelude::*;

    /// Per-slot constant data for the fused kernel.
    pub struct SlotK<'a, const N: usize> {
        pub dec: &'a [u32; N],
        pub words: &'a [u16],
        pub ctx_map: [u8; CLASSES16],
        pub k: u32,
        pub m: u32,
        pub vt: u32,
        pub rk: u32,
        pub rm: u32,
        pub cross: bool,
    }

    /// Token -> (base, nb, is_run) with vector arithmetic (hybrid uint).
    #[rite(import_intrinsics)]
    fn tok_info(
        _t: X64V3Token,
        sym: __m256i,
        k: u32,
        m: u32,
        vt: u32,
        rk: u32,
        rm: u32,
    ) -> (__m256i, __m256i, __m256i) {
        let zero = _mm256_setzero_si256();
        let is_run = _mm256_cmpgt_epi32(sym, _mm256_set1_epi32(vt as i32 - 1));
        // Select per-lane (k, m, token) for value vs run tokens.
        let t = _mm256_sub_epi32(sym, _mm256_and_si256(is_run, _mm256_set1_epi32(vt as i32)));
        let kk = _mm256_blendv_epi8(
            _mm256_set1_epi32(k as i32),
            _mm256_set1_epi32(rk as i32),
            is_run,
        );
        let mm = _mm256_blendv_epi8(
            _mm256_set1_epi32(m as i32),
            _mm256_set1_epi32(rm as i32),
            is_run,
        );
        let one = _mm256_set1_epi32(1);
        let direct_lim = _mm256_sllv_epi32(one, kk);
        let direct = _mm256_cmpgt_epi32(direct_lim, t);
        let tt = _mm256_sub_epi32(t, direct_lim);
        let msb = _mm256_add_epi32(kk, _mm256_srlv_epi32(tt, mm));
        let top = _mm256_and_si256(tt, _mm256_sub_epi32(_mm256_sllv_epi32(one, mm), one));
        let nb = _mm256_sub_epi32(msb, mm);
        let base = _mm256_sllv_epi32(_mm256_or_si256(_mm256_sllv_epi32(one, mm), top), nb);
        let base = _mm256_blendv_epi8(base, t, direct);
        let nb = _mm256_blendv_epi8(nb, zero, direct);
        (base, nb, is_run)
    }

    #[rite(import_intrinsics)]
    fn refill(_t: X64V3Token, x: __m256i, need: __m256i, words: &[u16], wp: &mut usize) -> __m256i {
        let m = _mm256_movemask_ps(_mm256_castsi256_ps(need)) as usize;
        let wv: &[u16; 8] = words[*wp..*wp + 8].try_into().unwrap();
        let w = _mm256_cvtepu16_epi32(_mm_loadu_si128(wv));
        let w = _mm256_permutevar8x32_epi32(w, _mm256_loadu_si256(&RANK8[m]));
        *wp += m.count_ones() as usize;
        _mm256_blendv_epi8(x, _mm256_or_si256(_mm256_slli_epi32::<16>(x), w), need)
    }

    /// Decodes all `NS` slots of a stripe in one step loop.
    #[arcane(import_intrinsics)]
    pub fn decode_stripe<const NS: usize, const LOG_M: i32, const N: usize>(
        t: X64V3Token,
        slots: &[SlotK<'_, N>; NS],
        geom: &IStripe,
        planes: &mut [&mut [u16]; NS],
    ) {
        let lw = geom.lw;
        let steps = geom.rows * lw;
        let zero = _mm256_setzero_si256();
        let one = _mm256_set1_epi32(1);
        let slot_mask = _mm256_set1_epi32(((1u32 << LOG_M as u32) - 1) as i32);
        let wid: [u32; 16] = core::array::from_fn(|l| geom.widths[l] as u32);
        let widv = [
            _mm256_loadu_si256(<&[u32; 8]>::try_from(&wid[..8]).unwrap()),
            _mm256_loadu_si256(<&[u32; 8]>::try_from(&wid[8..]).unwrap()),
        ];
        let mut xs = [[zero; 2]; NS];
        let mut runs = [[zero; 2]; NS];
        let mut uw = [[zero; 2]; NS];
        let mut wps = [2 * NL; NS];
        let mut cmaps = [zero; NS];
        for s in 0..NS {
            let mut st = [0u32; 16];
            for l in 0..NL {
                st[l] = slots[s].words[2 * l] as u32 | (slots[s].words[2 * l + 1] as u32) << 16;
            }
            xs[s][0] = _mm256_loadu_si256(<&[u32; 8]>::try_from(&st[..8]).unwrap());
            xs[s][1] = _mm256_loadu_si256(<&[u32; 8]>::try_from(&st[8..]).unwrap());
            let mut cm = [0u8; 32];
            cm[..16].copy_from_slice(&slots[s].ctx_map);
            cm[16..].copy_from_slice(&slots[s].ctx_map);
            cmaps[s] = _mm256_loadu_si256(&cm);
        }
        let mut xcol = 0usize;
        let mut row = 0usize;
        for j in 0..steps {
            let colv = _mm256_set1_epi32(xcol as i32);
            let mut u0 = [zero; 2];
            for s in 0..NS {
                let sk = &slots[s];
                let mut acts = [zero; 2];
                let mut syms = [zero; 2];
                for h in 0..2 {
                    let base_i = j * NL + h * 8;
                    let inw = _mm256_cmpgt_epi32(widv[h], colv);
                    let in_run = _mm256_and_si256(_mm256_cmpgt_epi32(runs[s][h], zero), inw);
                    runs[s][h] = _mm256_sub_epi32(runs[s][h], _mm256_and_si256(in_run, one));
                    let act = _mm256_andnot_si256(in_run, inw);
                    let un = if row > 0 {
                        let p: &[u16; 8] = planes[s][base_i - lw * NL..base_i - lw * NL + 8]
                            .try_into()
                            .unwrap();
                        _mm256_cvtepu16_epi32(_mm_loadu_si128(p))
                    } else {
                        zero
                    };
                    let mut sum = match (xcol > 0, row > 0) {
                        (true, true) => _mm256_add_epi32(uw[s][h], un),
                        (true, false) => _mm256_add_epi32(uw[s][h], uw[s][h]),
                        (false, true) => _mm256_add_epi32(un, un),
                        (false, false) => zero,
                    };
                    if sk.cross {
                        sum = _mm256_add_epi32(sum, u0[h]);
                    }
                    let e = _mm256_srli_epi32::<23>(_mm256_castps_si256(_mm256_cvtepi32_ps(sum)));
                    let cls = _mm256_min_epi32(
                        _mm256_max_epi32(_mm256_sub_epi32(e, _mm256_set1_epi32(126)), zero),
                        _mm256_set1_epi32(15),
                    );
                    let ctx = _mm256_and_si256(
                        _mm256_shuffle_epi8(cmaps[s], cls),
                        _mm256_set1_epi32(0xFF),
                    );
                    let x = xs[s][h];
                    let idx = _mm256_or_si256(
                        _mm256_slli_epi32::<LOG_M>(ctx),
                        _mm256_and_si256(x, slot_mask),
                    );
                    let mut ia = [0u32; 8];
                    _mm256_storeu_si256(&mut ia, idx);
                    let ev: [u32; 8] = core::array::from_fn(|i| sk.dec[ia[i] as usize & (N - 1)]);
                    let e = _mm256_loadu_si256(&ev);
                    let bias =
                        _mm256_and_si256(_mm256_srli_epi32::<7>(e), _mm256_set1_epi32(0xFFF));
                    let f = _mm256_srli_epi32::<19>(e);
                    let nx = _mm256_add_epi32(
                        _mm256_mullo_epi32(f, _mm256_srli_epi32::<LOG_M>(x)),
                        bias,
                    );
                    let x = _mm256_blendv_epi8(x, nx, act);
                    let need =
                        _mm256_and_si256(act, _mm256_cmpeq_epi32(_mm256_srli_epi32::<16>(x), zero));
                    xs[s][h] = refill(t, x, need, sk.words, &mut wps[s]);
                    acts[h] = act;
                    syms[h] = _mm256_and_si256(e, _mm256_set1_epi32(127));
                }
                for h in 0..2 {
                    let base_i = j * NL + h * 8;
                    let act = acts[h];
                    let (base, nb, is_run) = tok_info(t, syms[h], sk.k, sk.m, sk.vt, sk.rk, sk.rm);
                    let x = xs[s][h];
                    let ebits =
                        _mm256_and_si256(x, _mm256_sub_epi32(_mm256_sllv_epi32(one, nb), one));
                    let x = _mm256_blendv_epi8(x, _mm256_srlv_epi32(x, nb), act);
                    let need =
                        _mm256_and_si256(act, _mm256_cmpeq_epi32(_mm256_srli_epi32::<16>(x), zero));
                    xs[s][h] = refill(t, x, need, sk.words, &mut wps[s]);
                    let u = _mm256_add_epi32(base, ebits);
                    let run_now = _mm256_and_si256(act, is_run);
                    runs[s][h] = _mm256_blendv_epi8(runs[s][h], u, run_now);
                    let val = _mm256_andnot_si256(run_now, _mm256_and_si256(act, u));
                    uw[s][h] = val;
                    if s == 0 {
                        u0[h] = val;
                    }
                    let packed = _mm256_permute4x64_epi64::<0b1000>(_mm256_packus_epi32(val, val));
                    let o: &mut [u16; 8] = (&mut planes[s][base_i..base_i + 8]).try_into().unwrap();
                    _mm_storeu_si128(o, _mm256_castsi256_si128(packed));
                }
            }
            xcol += 1;
            if xcol == lw {
                xcol = 0;
                row += 1;
            }
        }
    }
}

/// Fused all-slot AVX2 decode.
pub fn decode_fused(
    enc: &RansEncoded,
    si: usize,
    geom: &IStripe,
    out: &mut [Vec<u16>],
    flats: &[FlatTables<{ NCTX << 12 }>],
) {
    #[cfg(target_arch = "x86_64")]
    if let Some(t) = {
        use archmage::SimdToken;
        archmage::X64V3Token::summon()
    } {
        fn go<const NS: usize, const LM: i32, const N: usize>(
            t: archmage::X64V3Token,
            enc: &RansEncoded,
            si: usize,
            geom: &IStripe,
            out: &mut [Vec<u16>],
            flats: &[FlatTables<{ NCTX << 12 }>],
        ) {
            let slots: [avx2b::SlotK<'_, N>; NS] = core::array::from_fn(|s| {
                let md = &enc.models[s];
                avx2b::SlotK {
                    dec: flats[s].dec[..N].try_into().unwrap(),
                    words: &enc.streams[si][s],
                    ctx_map: md.ctx_map,
                    k: md.tc.k,
                    m: md.tc.m,
                    vt: md.tc.value_tokens(),
                    rk: md.tc.rk,
                    rm: md.tc.rm,
                    cross: md.cross,
                }
            });
            let mut it = out.iter_mut();
            let mut planes: [&mut [u16]; NS] =
                core::array::from_fn(|_| it.next().unwrap().as_mut_slice());
            avx2b::decode_stripe::<NS, LM, N>(t, &slots, geom, &mut planes);
        }
        match (enc.models.len(), enc.models[0].log_m) {
            (3, 12) => go::<3, 12, { NCTX << 12 }>(t, enc, si, geom, out, flats),
            (3, 10) => go::<3, 10, { NCTX << 10 }>(t, enc, si, geom, out, flats),
            (4, 12) => go::<4, 12, { NCTX << 12 }>(t, enc, si, geom, out, flats),
            (4, 10) => go::<4, 10, { NCTX << 10 }>(t, enc, si, geom, out, flats),
            (1, 12) => go::<1, 12, { NCTX << 12 }>(t, enc, si, geom, out, flats),
            (1, 10) => go::<1, 10, { NCTX << 10 }>(t, enc, si, geom, out, flats),
            _ => decode_simd(enc, si, geom, out, flats),
        }
        return;
    }
    decode_scalar(enc, si, geom, out);
}

#[cfg(target_arch = "x86_64")]
pub mod avx2c {
    use super::*;
    use archmage::prelude::*;

    pub struct Consts {
        pub cmapv: __m256i,
        pub vt: __m256i,
        pub vtm1: __m256i,
        pub dlim: __m256i,
        pub mpow: __m256i,
        pub mmask: __m256i,
    }

    #[rite(import_intrinsics)]
    fn refill(_t: X64V3Token, x: __m256i, need: __m256i, words: &[u16], wp: &mut usize) -> __m256i {
        let m = _mm256_movemask_ps(_mm256_castsi256_ps(need)) as usize;
        let wv: &[u16; 8] = words[*wp..*wp + 8].try_into().unwrap();
        let w = _mm256_cvtepu16_epi32(_mm_loadu_si128(wv));
        let w = _mm256_permutevar8x32_epi32(w, _mm256_loadu_si256(&RANK8[m]));
        *wp += m.count_ones() as usize;
        _mm256_blendv_epi8(x, _mm256_or_si256(_mm256_slli_epi32::<16>(x), w), need)
    }

    /// Token phase for one half. Returns (act, entry).
    #[rite(import_intrinsics)]
    #[allow(clippy::too_many_arguments)]
    fn token_phase(
        t: X64V3Token,
        c: &Consts,
        dec: &[u32; 4096],
        words: &[u16],
        wp: &mut usize,
        x: &mut __m256i,
        run: &mut __m256i,
        sum: __m256i,
        inw: __m256i,
    ) -> (__m256i, __m256i) {
        let zero = _mm256_setzero_si256();
        let in_run = _mm256_and_si256(_mm256_cmpgt_epi32(*run, zero), inw);
        *run = _mm256_add_epi32(*run, in_run);
        let act = _mm256_andnot_si256(in_run, inw);
        let ex = _mm256_srli_epi32::<23>(_mm256_castps_si256(_mm256_cvtepi32_ps(sum)));
        let cls = _mm256_min_epi32(
            _mm256_max_epi32(_mm256_sub_epi32(ex, _mm256_set1_epi32(126)), zero),
            _mm256_set1_epi32(15),
        );
        let ctx = _mm256_and_si256(_mm256_shuffle_epi8(c.cmapv, cls), _mm256_set1_epi32(3));
        let idx = _mm256_or_si256(
            _mm256_slli_epi32::<10>(ctx),
            _mm256_and_si256(*x, _mm256_set1_epi32(1023)),
        );
        let mut ia = [0u32; 8];
        _mm256_storeu_si256(&mut ia, idx);
        let ev: [u32; 8] = core::array::from_fn(|i| dec[ia[i] as usize & 4095]);
        let e = _mm256_loadu_si256(&ev);
        let f = _mm256_add_epi32(
            _mm256_and_si256(e, _mm256_set1_epi32(1023)),
            _mm256_set1_epi32(1),
        );
        let bias = _mm256_and_si256(_mm256_srli_epi32::<10>(e), _mm256_set1_epi32(1023));
        let nx = _mm256_add_epi32(_mm256_mullo_epi32(f, _mm256_srli_epi32::<10>(*x)), bias);
        let xx = _mm256_blendv_epi8(*x, nx, act);
        let need = _mm256_and_si256(act, _mm256_cmpeq_epi32(_mm256_srli_epi32::<16>(xx), zero));
        *x = refill(t, xx, need, words, wp);
        (act, e)
    }

    /// Extra-bits phase for one half. Returns the decoded value (0 for runs/inactive).
    #[rite(import_intrinsics)]
    #[allow(clippy::too_many_arguments)]
    fn extra_phase(
        t: X64V3Token,
        c: &Consts,
        words: &[u16],
        wp: &mut usize,
        x: &mut __m256i,
        run: &mut __m256i,
        act: __m256i,
        e: __m256i,
    ) -> __m256i {
        let zero = _mm256_setzero_si256();
        let one = _mm256_set1_epi32(1);
        let nb = _mm256_srli_epi32::<28>(e);
        let sym = _mm256_and_si256(_mm256_srli_epi32::<20>(e), _mm256_set1_epi32(255));
        let ebits = _mm256_and_si256(*x, _mm256_sub_epi32(_mm256_sllv_epi32(one, nb), one));
        let xx = _mm256_blendv_epi8(*x, _mm256_srlv_epi32(*x, nb), act);
        let need = _mm256_and_si256(act, _mm256_cmpeq_epi32(_mm256_srli_epi32::<16>(xx), zero));
        *x = refill(t, xx, need, words, wp);
        let is_run = _mm256_cmpgt_epi32(sym, c.vtm1);
        let tt = _mm256_sub_epi32(sym, _mm256_and_si256(is_run, c.vt));
        let direct = _mm256_cmpgt_epi32(c.dlim, tt);
        let hb = _mm256_sllv_epi32(_mm256_or_si256(c.mpow, _mm256_and_si256(tt, c.mmask)), nb);
        let base = _mm256_blendv_epi8(hb, tt, direct);
        let u = _mm256_add_epi32(base, ebits);
        let run_now = _mm256_and_si256(act, is_run);
        *run = _mm256_blendv_epi8(*run, u, run_now);
        _mm256_andnot_si256(run_now, _mm256_and_si256(act, u))
    }

    #[rite(import_intrinsics)]
    fn load8(_t: X64V3Token, p: &[u16]) -> __m256i {
        let a: &[u16; 8] = p[..8].try_into().unwrap();
        _mm256_cvtepu16_epi32(_mm_loadu_si128(a))
    }

    #[rite(import_intrinsics)]
    fn store8(_t: X64V3Token, p: &mut [u16], v: __m256i) {
        let packed = _mm256_permute4x64_epi64::<0b1000>(_mm256_packus_epi32(v, v));
        let o: &mut [u16; 8] = (&mut p[..8]).try_into().unwrap();
        _mm_storeu_si128(o, _mm256_castsi256_si128(packed));
    }

    /// One slot of one stripe, 16 lanes as two AVX2 halves, log_m = 10.
    #[arcane(import_intrinsics)]
    #[allow(clippy::too_many_arguments)]
    pub fn decode_slot(
        t: X64V3Token,
        dec: &[u32; 4096],
        words: &[u16],
        ctx_map: &[u8; CLASSES16],
        k: u32,
        m: u32,
        vt: u32,
        geom: &IStripe,
        plane: &mut [u16],
        cross: Option<&[u16]>,
    ) {
        let lw = geom.lw;
        let steps = geom.rows * lw;
        let zero = _mm256_setzero_si256();
        let mut cm = [0u8; 32];
        cm[..16].copy_from_slice(ctx_map);
        cm[16..].copy_from_slice(ctx_map);
        let c = Consts {
            cmapv: _mm256_loadu_si256(&cm),
            vt: _mm256_set1_epi32(vt as i32),
            vtm1: _mm256_set1_epi32(vt as i32 - 1),
            dlim: _mm256_set1_epi32(1 << k),
            mpow: _mm256_set1_epi32(1 << m),
            mmask: _mm256_set1_epi32((1 << m) - 1),
        };
        let wid: [u32; 16] = core::array::from_fn(|l| geom.widths[l] as u32);
        let wid0 = _mm256_loadu_si256(<&[u32; 8]>::try_from(&wid[..8]).unwrap());
        let wid1 = _mm256_loadu_si256(<&[u32; 8]>::try_from(&wid[8..]).unwrap());
        let mut st = [0u32; 16];
        for l in 0..NL {
            st[l] = words[2 * l] as u32 | (words[2 * l + 1] as u32) << 16;
        }
        let mut x0 = _mm256_loadu_si256(<&[u32; 8]>::try_from(&st[..8]).unwrap());
        let mut x1 = _mm256_loadu_si256(<&[u32; 8]>::try_from(&st[8..]).unwrap());
        let mut r0 = zero;
        let mut r1 = zero;
        let mut w0 = zero;
        let mut w1 = zero;
        let mut wp = 2 * NL;
        let mut j = 0usize;
        for row in 0..geom.rows {
            for xcol in 0..lw {
                let colv = _mm256_set1_epi32(xcol as i32);
                let inw0 = _mm256_cmpgt_epi32(wid0, colv);
                let inw1 = _mm256_cmpgt_epi32(wid1, colv);
                let bi = j * NL;
                let (mut s0, mut s1) = if row > 0 {
                    let n0 = load8(t, &plane[bi - lw * NL..]);
                    let n1 = load8(t, &plane[bi - lw * NL + 8..]);
                    if xcol > 0 {
                        (_mm256_add_epi32(w0, n0), _mm256_add_epi32(w1, n1))
                    } else {
                        (_mm256_add_epi32(n0, n0), _mm256_add_epi32(n1, n1))
                    }
                } else if xcol > 0 {
                    (_mm256_add_epi32(w0, w0), _mm256_add_epi32(w1, w1))
                } else {
                    (zero, zero)
                };
                if let Some(cr) = cross {
                    s0 = _mm256_add_epi32(s0, load8(t, &cr[bi..]));
                    s1 = _mm256_add_epi32(s1, load8(t, &cr[bi + 8..]));
                }
                let (a0, e0) = token_phase(t, &c, dec, words, &mut wp, &mut x0, &mut r0, s0, inw0);
                let (a1, e1) = token_phase(t, &c, dec, words, &mut wp, &mut x1, &mut r1, s1, inw1);
                w0 = extra_phase(t, &c, words, &mut wp, &mut x0, &mut r0, a0, e0);
                w1 = extra_phase(t, &c, words, &mut wp, &mut x1, &mut r1, a1, e1);
                store8(t, &mut plane[bi..], w0);
                store8(t, &mut plane[bi + 8..], w1);
                j += 1;
            }
        }
    }
}

/// v3: per-slot AVX2 passes with packed v2 entries (log_m must be 10).
pub fn decode_v3(
    enc: &RansEncoded,
    si: usize,
    geom: &IStripe,
    out: &mut [Vec<u16>],
    flats: &[FlatTables<4096>],
) {
    #[cfg(target_arch = "x86_64")]
    if let Some(t) = {
        use archmage::SimdToken;
        archmage::X64V3Token::summon()
    } {
        for s in 0..enc.models.len() {
            let md = &enc.models[s];
            let (before, rest) = out.split_at_mut(s);
            let cr: Option<&[u16]> = if md.cross { Some(&before[0]) } else { None };
            avx2c::decode_slot(
                t,
                &flats[s].dec,
                &enc.streams[si][s],
                &md.ctx_map,
                md.tc.k,
                md.tc.m,
                md.tc.value_tokens(),
                geom,
                &mut rest[0],
                cr,
            );
        }
        return;
    }
    decode_scalar(enc, si, geom, out);
}

impl RansModel {
    pub fn flat_v2(&self) -> FlatTables<4096> {
        let mut b: alloc::boxed::Box<[u32; 4096]> =
            vec![0u32; 4096].into_boxed_slice().try_into().ok().unwrap();
        b[..self.dec2.len()].copy_from_slice(&self.dec2);
        FlatTables { dec: b }
    }
}
