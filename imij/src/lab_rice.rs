#![allow(
    dead_code,
    unused_mut,
    unused_variables,
    private_interfaces,
    clippy::all
)]
//! Lab: 16-lane interleaved Rice coding (table-free SIMD decode).
//!
//! Per lane: LSB-first bits in 32-bit words. A lane holds (lo, hi, pos);
//! the 32-bit window is `(lo >> pos) | (hi << (32 - pos))`. After a read
//! of `n <= 32` bits, `pos += n`; when `pos >= 32` the lane shifts in the
//! next word of the stream. Words are stored in decode-consumption order:
//! step ascending, then read phase (first, second), then lane ascending.
//! The stream starts with 2 words per lane (lane order).
//!
//! Code: q = trailing zeros (q < QMAX), then a 1 bit, then k bits:
//! `v = q << k | rem`. q == QMAX is an escape: QMAX zero bits then `raw`
//! bits of v. Class 0 (flat context) reads a run length L instead; L > 0
//! emits L zeros and the first position after the run reads `u - 1` with
//! `k_post`; L == 0 reads `u - 1` with `k_post` in the same step.

use alloc::vec;
use alloc::vec::Vec;

use crate::lab_rans::{CLASSES16, IStripe, NL, class16};

pub const QMAX: u32 = 12;
pub const RUN_CAP: u32 = 65535;

#[derive(Clone, Debug)]
pub struct RiceParams {
    /// k per class (class 0 entry = k_run).
    pub k: [u8; CLASSES16],
    pub k_post: u8,
    /// Raw escape width for values.
    pub raw: u32,
    pub cross: bool,
}

pub struct RiceEncoded {
    pub params: Vec<RiceParams>,
    /// `[stripe][slot]` words (2*NL initial words first, then padding at end).
    pub streams: Vec<Vec<Vec<u32>>>,
    pub payload_bytes: usize,
}

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

/// One coded read: (step, phase, value, k, raw width).
#[derive(Clone, Copy)]
struct Read {
    j: u32,
    phase: u8,
    v: u32,
    k: u8,
    raw: u8,
}

#[inline]
fn rice_len(v: u32, k: u32, raw: u32) -> u32 {
    let q = v >> k;
    if q < QMAX { q + 1 + k } else { QMAX + raw }
}

/// Produces the read sequence of one lane. `kmap` None = collect stats with
/// placeholder k (only `v` and class are used).
fn lane_reads(st: &IStripe, s: usize, p: &RiceParams, l: usize, out: &mut Vec<(Read, u8)>) {
    let u = &st.u[s];
    let cr = if p.cross {
        Some(st.u[0].as_slice())
    } else {
        None
    };
    let lw = st.lw;
    let w = st.widths[l];
    let steps = st.rows * lw;
    let active = |j: usize| j % lw < w;
    let mut j = 0usize;
    let mut post = false;
    while j < steps {
        if !active(j) {
            j += 1;
            continue;
        }
        let uv = u[j * NL + l] as u32;
        if post {
            post = false;
            out.push((
                Read {
                    j: j as u32,
                    phase: 0,
                    v: uv - 1,
                    k: p.k_post,
                    raw: p.raw as u8,
                },
                255,
            ));
            j += 1;
            continue;
        }
        let c = class16(ctx_sum(u, cr, lw, j, l));
        if c == 0 {
            let mut len = 0u32;
            let mut jj = j;
            while jj < steps && len < RUN_CAP {
                if active(jj) {
                    if u[jj * NL + l] != 0 {
                        break;
                    }
                    len += 1;
                }
                jj += 1;
            }
            out.push((
                Read {
                    j: j as u32,
                    phase: 0,
                    v: len,
                    k: p.k[0],
                    raw: 16,
                },
                0,
            ));
            if len == 0 {
                out.push((
                    Read {
                        j: j as u32,
                        phase: 1,
                        v: uv - 1,
                        k: p.k_post,
                        raw: p.raw as u8,
                    },
                    254,
                ));
                j += 1;
            } else {
                // Skip `len` active positions.
                let mut left = len;
                while left > 0 {
                    if active(j) {
                        left -= 1;
                    }
                    j += 1;
                }
                // Post-run value follows unless the run was capped or ended the lane.
                post = len < RUN_CAP && (j..steps).any(active);
            }
            continue;
        }
        out.push((
            Read {
                j: j as u32,
                phase: 0,
                v: uv,
                k: p.k[c],
                raw: p.raw as u8,
            },
            c as u8,
        ));
        j += 1;
    }
}

fn best_k(hist: &[u64], raw: u32) -> u8 {
    let mut best = (u64::MAX, 0u8);
    for k in 0..=raw.min(15) {
        let c: u64 = hist
            .iter()
            .enumerate()
            .filter(|(_, n)| **n > 0)
            .map(|(v, &n)| n * rice_len(v as u32, k, raw) as u64)
            .sum();
        if c < best.0 {
            best = (c, k as u8);
        }
    }
    best.1
}

pub fn encode(stripes: &[IStripe], slot_bits: &[u32], color_slots: usize) -> RiceEncoded {
    let ns = slot_bits.len();
    let mut params = Vec::with_capacity(ns);
    for s in 0..ns {
        let raw = slot_bits[s].max(1);
        let cross = s >= 1 && s < color_slots;
        // Stats pass with dummy k (reads depend only on values/classes).
        let mut p = RiceParams {
            k: [0; CLASSES16],
            k_post: 0,
            raw,
            cross,
        };
        let nv = (1usize << raw).max(RUN_CAP as usize + 1);
        let mut hist = vec![vec![0u64; nv]; CLASSES16 + 1];
        let mut reads = Vec::new();
        for st in stripes {
            for l in 0..NL {
                reads.clear();
                lane_reads(st, s, &p, l, &mut reads);
                for (r, c) in &reads {
                    let hi = if *c >= 254 { CLASSES16 } else { *c as usize };
                    hist[hi][r.v as usize] += 1;
                }
            }
        }
        for c in 0..CLASSES16 {
            p.k[c] = best_k(&hist[c], if c == 0 { 16 } else { raw });
        }
        p.k_post = best_k(&hist[CLASSES16], raw);
        params.push(p);
    }
    let mut streams = Vec::with_capacity(stripes.len());
    let mut payload = 0usize;
    for st in stripes {
        let mut per = Vec::with_capacity(ns);
        for s in 0..ns {
            let p = &params[s];
            // Per lane: bit vector -> words; record when each word >= 2 is consumed.
            let mut init = vec![0u32; 2 * NL];
            let mut events: Vec<(u32, u8, u8, u32)> = Vec::new();
            let mut reads = Vec::new();
            for l in 0..NL {
                reads.clear();
                lane_reads(st, s, p, l, &mut reads);
                let mut bits: Vec<u32> = Vec::new(); // words
                let mut acc = 0u64;
                let mut nacc = 0u32;
                let mut consumed = 0u64;
                let mut req: Vec<(u32, u8)> = Vec::new(); // (j, phase) for words 2..
                let mut push =
                    |v: u32, n: u32, acc: &mut u64, nacc: &mut u32, bits: &mut Vec<u32>| {
                        if n == 0 {
                            return;
                        }
                        *acc |= (v as u64) << *nacc;
                        *nacc += n;
                        while *nacc >= 32 {
                            bits.push(*acc as u32);
                            *acc >>= 32;
                            *nacc -= 32;
                        }
                    };
                for (r, _) in &reads {
                    let k = r.k as u32;
                    let q = r.v >> k;
                    let len;
                    if q < QMAX {
                        // q zeros, a one, k bits
                        push(1u32 << q, q + 1, &mut acc, &mut nacc, &mut bits);
                        push(r.v & ((1u32 << k) - 1), k, &mut acc, &mut nacc, &mut bits);
                        len = q + 1 + k;
                    } else {
                        push(0, QMAX, &mut acc, &mut nacc, &mut bits);
                        push(r.v, r.raw as u32, &mut acc, &mut nacc, &mut bits);
                        len = QMAX + r.raw as u32;
                    }
                    let before = consumed / 32;
                    consumed += len as u64;
                    if consumed / 32 > before {
                        req.push((r.j, r.phase));
                    }
                }
                if nacc > 0 {
                    bits.push(acc as u32);
                }
                // Words 0,1 initial; word 2+i requested at req[i].
                while bits.len() < 2 + req.len() {
                    bits.push(0);
                }
                init[2 * l] = bits[0];
                init[2 * l + 1] = bits[1];
                for (i, &(j, ph)) in req.iter().enumerate() {
                    events.push((j, ph, l as u8, bits[2 + i]));
                }
            }
            events.sort_by_key(|e| (e.0, e.1, e.2));
            let mut out = init;
            out.extend(events.iter().map(|e| e.3));
            payload += out.len() * 4;
            out.extend([0u32; 16]);
            per.push(out);
        }
        streams.push(per);
    }
    RiceEncoded {
        params,
        streams,
        payload_bytes: payload,
    }
}

struct Lane {
    lo: u32,
    hi: u32,
    pos: u32,
}

impl Lane {
    #[inline(always)]
    fn window(&self) -> u32 {
        (self.lo >> self.pos)
            | if self.pos == 0 {
                0
            } else {
                self.hi << (32 - self.pos)
            }
    }
}

/// Scalar reference decoder.
pub fn decode_scalar(enc: &RiceEncoded, si: usize, geom: &IStripe, out: &mut [Vec<u16>]) {
    for s in 0..enc.params.len() {
        let p = &enc.params[s];
        let words = &enc.streams[si][s];
        let mut lanes: [Lane; NL] = core::array::from_fn(|l| Lane {
            lo: words[2 * l],
            hi: words[2 * l + 1],
            pos: 0,
        });
        let mut wp = 2 * NL;
        let mut run = [0u32; NL];
        let mut post = [false; NL];
        let (before, rest) = out.split_at_mut(s);
        let plane = &mut rest[0];
        let cr: Option<&[u16]> = if p.cross { Some(&before[0]) } else { None };
        let lw = geom.lw;
        let steps = geom.rows * lw;
        let read = |ln: &mut Lane, k: u32, raw: u32| -> (u32, u32) {
            let w = ln.window();
            let q = (w | (1 << QMAX)).trailing_zeros();
            if q < QMAX {
                (((q << k) | ((w >> (q + 1)) & ((1u32 << k) - 1))), q + 1 + k)
            } else {
                ((w >> QMAX) & ((1u32 << raw) - 1), QMAX + raw)
            }
        };
        let mut second = [false; NL];
        for j in 0..steps {
            let xcol = j % lw;
            // First read phase.
            for l in 0..NL {
                second[l] = false;
                if xcol >= geom.widths[l] {
                    plane[j * NL + l] = 0;
                    continue;
                }
                if run[l] > 0 {
                    run[l] -= 1;
                    plane[j * NL + l] = 0;
                    continue;
                }
                let ln = &mut lanes[l];
                let (u, n);
                if post[l] {
                    post[l] = false;
                    let (v, len) = read(ln, p.k_post as u32, p.raw);
                    u = v + 1;
                    n = len;
                } else {
                    let c = class16(ctx_sum(plane, cr, lw, j, l));
                    if c == 0 {
                        let (v, len) = read(ln, p.k[0] as u32, 16);
                        n = len;
                        if v == 0 {
                            second[l] = true;
                            u = 0;
                        } else {
                            u = 0;
                            run[l] = v - 1;
                            post[l] = v < RUN_CAP;
                        }
                    } else {
                        let (v, len) = read(ln, p.k[c] as u32, p.raw);
                        u = v;
                        n = len;
                    }
                }
                ln.pos += n;
                if ln.pos >= 32 {
                    ln.pos -= 32;
                    ln.lo = ln.hi;
                    ln.hi = words[wp];
                    wp += 1;
                }
                plane[j * NL + l] = u as u16;
            }
            // Second read phase.
            for l in 0..NL {
                if !second[l] {
                    continue;
                }
                let ln = &mut lanes[l];
                let (v, n) = read(ln, p.k_post as u32, p.raw);
                ln.pos += n;
                if ln.pos >= 32 {
                    ln.pos -= 32;
                    ln.lo = ln.hi;
                    ln.hi = words[wp];
                    wp += 1;
                }
                plane[j * NL + l] = (v + 1) as u16;
            }
            // A run reaching the lane end leaves `post` set harmlessly.
        }
    }
}

#[cfg(target_arch = "x86_64")]
pub mod avx2 {
    use super::*;
    use crate::lab_rans::RANK8;
    use archmage::prelude::*;

    struct St {
        lo: __m256i,
        hi: __m256i,
        pos: __m256i,
        run: __m256i,
        post: __m256i,
        uw: __m256i,
    }

    /// Rice read for lanes in `m`; returns value. Advances state.
    #[rite(import_intrinsics)]
    fn read(
        _t: X64V3Token,
        st: &mut St,
        m: __m256i,
        k: __m256i,
        raw: __m256i,
        words: &[u32],
        wp: &mut usize,
    ) -> __m256i {
        let one = _mm256_set1_epi32(1);
        let c32 = _mm256_set1_epi32(32);
        let w = _mm256_or_si256(
            _mm256_srlv_epi32(st.lo, st.pos),
            _mm256_sllv_epi32(st.hi, _mm256_sub_epi32(c32, st.pos)),
        );
        let wq = _mm256_or_si256(w, _mm256_set1_epi32(1 << QMAX));
        let low = _mm256_and_si256(wq, _mm256_sub_epi32(_mm256_setzero_si256(), wq));
        let q = _mm256_sub_epi32(
            _mm256_srli_epi32::<23>(_mm256_castps_si256(_mm256_cvtepi32_ps(low))),
            _mm256_set1_epi32(127),
        );
        let esc = _mm256_cmpeq_epi32(q, _mm256_set1_epi32(QMAX as i32));
        let q1 = _mm256_add_epi32(q, one);
        let kmask = _mm256_sub_epi32(_mm256_sllv_epi32(one, k), one);
        let rem = _mm256_and_si256(_mm256_srlv_epi32(w, q1), kmask);
        let v_n = _mm256_or_si256(_mm256_sllv_epi32(q, k), rem);
        let rmask = _mm256_sub_epi32(_mm256_sllv_epi32(one, raw), one);
        let v_e = _mm256_and_si256(_mm256_srli_epi32::<{ QMAX as i32 }>(w), rmask);
        let v = _mm256_blendv_epi8(v_n, v_e, esc);
        let len_n = _mm256_add_epi32(q1, k);
        let len_e = _mm256_add_epi32(raw, _mm256_set1_epi32(QMAX as i32));
        let len = _mm256_and_si256(_mm256_blendv_epi8(len_n, len_e, esc), m);
        let pos = _mm256_add_epi32(st.pos, len);
        let need = _mm256_cmpgt_epi32(pos, _mm256_set1_epi32(31));
        st.pos = _mm256_sub_epi32(pos, _mm256_and_si256(need, c32));
        let mk = _mm256_movemask_ps(_mm256_castsi256_ps(need)) as usize;
        let wv: &[u32; 8] = words[*wp..*wp + 8].try_into().unwrap();
        let nw =
            _mm256_permutevar8x32_epi32(_mm256_loadu_si256(wv), _mm256_loadu_si256(&RANK8[mk]));
        *wp += mk.count_ones() as usize;
        st.lo = _mm256_blendv_epi8(st.lo, st.hi, need);
        st.hi = _mm256_blendv_epi8(st.hi, nw, need);
        v
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

    #[arcane(import_intrinsics)]
    pub fn decode_slot(
        t: X64V3Token,
        p: &RiceParams,
        words: &[u32],
        geom: &IStripe,
        plane: &mut [u16],
        cross: Option<&[u16]>,
    ) {
        let lw = geom.lw;
        let zero = _mm256_setzero_si256();
        let one = _mm256_set1_epi32(1);
        let mut kt = [0u8; 32];
        for c in 0..CLASSES16 {
            kt[c] = p.k[c];
            kt[c + 16] = p.k[c];
        }
        let ktab = _mm256_loadu_si256(&kt);
        let kpost = _mm256_set1_epi32(p.k_post as i32);
        let rawv = _mm256_set1_epi32(p.raw as i32);
        let raw16 = _mm256_set1_epi32(16);
        let runcap = _mm256_set1_epi32(RUN_CAP as i32);
        let wid: [u32; 16] = core::array::from_fn(|l| geom.widths[l] as u32);
        let widv = [
            _mm256_loadu_si256(<&[u32; 8]>::try_from(&wid[..8]).unwrap()),
            _mm256_loadu_si256(<&[u32; 8]>::try_from(&wid[8..]).unwrap()),
        ];
        let init = |h: usize| -> St {
            let lo: [u32; 8] = core::array::from_fn(|i| words[2 * (h * 8 + i)]);
            let hi: [u32; 8] = core::array::from_fn(|i| words[2 * (h * 8 + i) + 1]);
            St {
                lo: _mm256_loadu_si256(&lo),
                hi: _mm256_loadu_si256(&hi),
                pos: zero,
                run: zero,
                post: zero,
                uw: zero,
            }
        };
        let mut sts = [init(0), init(1)];
        let mut wp = 2 * NL;
        let mut j = 0usize;
        for row in 0..geom.rows {
            for xcol in 0..lw {
                let colv = _mm256_set1_epi32(xcol as i32);
                let bi = j * NL;
                let mut seconds = [zero; 2];
                let mut vals = [zero; 2];
                for h in 0..2 {
                    let st = &mut sts[h];
                    let inw = _mm256_cmpgt_epi32(widv[h], colv);
                    let in_run = _mm256_and_si256(_mm256_cmpgt_epi32(st.run, zero), inw);
                    st.run = _mm256_add_epi32(st.run, in_run);
                    let act = _mm256_andnot_si256(in_run, inw);
                    let mut sum = if row > 0 {
                        let n = load8(t, &plane[bi - lw * NL + h * 8..]);
                        if xcol > 0 {
                            _mm256_add_epi32(st.uw, n)
                        } else {
                            _mm256_add_epi32(n, n)
                        }
                    } else if xcol > 0 {
                        _mm256_add_epi32(st.uw, st.uw)
                    } else {
                        zero
                    };
                    if let Some(cr) = cross {
                        sum = _mm256_add_epi32(sum, load8(t, &cr[bi + h * 8..]));
                    }
                    let ex = _mm256_srli_epi32::<23>(_mm256_castps_si256(_mm256_cvtepi32_ps(sum)));
                    let cls = _mm256_min_epi32(
                        _mm256_max_epi32(_mm256_sub_epi32(ex, _mm256_set1_epi32(126)), zero),
                        _mm256_set1_epi32(15),
                    );
                    let is_post = _mm256_and_si256(st.post, act);
                    let c0 = _mm256_andnot_si256(is_post, _mm256_cmpeq_epi32(cls, zero));
                    let k =
                        _mm256_and_si256(_mm256_shuffle_epi8(ktab, cls), _mm256_set1_epi32(0xFF));
                    let k = _mm256_blendv_epi8(k, kpost, is_post);
                    let raw = _mm256_blendv_epi8(rawv, raw16, c0);
                    let v = read(t, st, act, k, raw, words, &mut wp);
                    // Interpret.
                    st.post = _mm256_andnot_si256(is_post, st.post);
                    let runl =
                        _mm256_and_si256(_mm256_and_si256(c0, act), _mm256_cmpgt_epi32(v, zero));
                    let sec =
                        _mm256_and_si256(_mm256_and_si256(c0, act), _mm256_cmpeq_epi32(v, zero));
                    st.run = _mm256_blendv_epi8(st.run, _mm256_sub_epi32(v, one), runl);
                    let capped = _mm256_cmpeq_epi32(v, runcap);
                    st.post = _mm256_or_si256(st.post, _mm256_andnot_si256(capped, runl));
                    let val = _mm256_add_epi32(v, _mm256_and_si256(is_post, one));
                    let val = _mm256_andnot_si256(c0, _mm256_and_si256(act, val));
                    seconds[h] = sec;
                    vals[h] = val;
                }
                for h in 0..2 {
                    let st = &mut sts[h];
                    let sec = seconds[h];
                    let mut val = vals[h];
                    if _mm256_testz_si256(sec, sec) == 0 {
                        let v2 = read(t, st, sec, kpost, rawv, words, &mut wp);
                        val = _mm256_blendv_epi8(val, _mm256_add_epi32(v2, one), sec);
                    }
                    st.uw = val;
                    store8(t, &mut plane[bi + h * 8..], val);
                }
                j += 1;
            }
        }
    }
}

pub fn decode_simd(enc: &RiceEncoded, si: usize, geom: &IStripe, out: &mut [Vec<u16>]) {
    #[cfg(target_arch = "x86_64")]
    if let Some(t) = {
        use archmage::SimdToken;
        archmage::X64V3Token::summon()
    } {
        for s in 0..enc.params.len() {
            let p = &enc.params[s];
            let (before, rest) = out.split_at_mut(s);
            let cr: Option<&[u16]> = if p.cross { Some(&before[0]) } else { None };
            avx2::decode_slot(t, p, &enc.streams[si][s], geom, &mut rest[0], cr);
        }
        return;
    }
    decode_scalar(enc, si, geom, out);
}
