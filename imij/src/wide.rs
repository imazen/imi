//! AVX2 kernels for 32-bit storage (extended tier, coded depth > 16).
//! Bit-identical to the scalar reference in `rans.rs` / `v1.rs`.

#[cfg(target_arch = "x86_64")]
pub(crate) mod avx2 {
    use crate::predict;
    use crate::rans::{DecModel, LaneState, NL, TABLE, Words};
    use archmage::prelude::*;

    static RANK8: [[u32; 8]; 256] = {
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

    #[rite(import_intrinsics)]
    fn ld(_t: X64V3Token, p: &[u32]) -> __m256i {
        let a: &[u32; 8] = p[..8].try_into().unwrap();
        _mm256_loadu_si256(a)
    }

    #[rite(import_intrinsics)]
    fn st(_t: X64V3Token, p: &mut [u32], v: __m256i) {
        let a: &mut [u32; 8] = (&mut p[..8]).try_into().unwrap();
        _mm256_storeu_si256(a, v);
    }

    #[rite(import_intrinsics)]
    fn refill(_t: X64V3Token, x: __m256i, need: __m256i, w: &Words<'_>, wp: &mut usize) -> __m256i {
        let m = _mm256_movemask_ps(_mm256_castsi256_ps(need)) as usize;
        let c = w.chunk(*wp);
        let wv: &[u8; 16] = c[..16].try_into().unwrap();
        let v = _mm256_cvtepu16_epi32(_mm_loadu_si128(wv));
        let v = _mm256_permutevar8x32_epi32(v, _mm256_loadu_si256(&RANK8[m]));
        *wp += m.count_ones() as usize;
        _mm256_blendv_epi8(x, _mm256_or_si256(_mm256_slli_epi32::<16>(x), v), need)
    }

    /// `x < 2^16` (unsigned) as a lane mask.
    #[rite(import_intrinsics)]
    fn below_l(_t: X64V3Token, x: __m256i) -> __m256i {
        _mm256_cmpeq_epi32(_mm256_srli_epi32::<16>(x), _mm256_setzero_si256())
    }

    /// Pulls `n` (per lane, 0..=16) raw bits for lanes in `m`.
    #[rite(import_intrinsics)]
    fn pull(
        t: X64V3Token,
        x: &mut __m256i,
        n: __m256i,
        m: __m256i,
        w: &Words<'_>,
        wp: &mut usize,
    ) -> __m256i {
        let one = _mm256_set1_epi32(1);
        let bits = _mm256_and_si256(*x, _mm256_sub_epi32(_mm256_sllv_epi32(one, n), one));
        let xx = _mm256_blendv_epi8(*x, _mm256_srlv_epi32(*x, n), m);
        *x = refill(t, xx, _mm256_and_si256(m, below_l(t, xx)), w, wp);
        _mm256_and_si256(bits, m)
    }

    /// Token -> (base, extra bits, is_run mask) by hybrid-uint arithmetic.
    #[rite(import_intrinsics)]
    fn tok_info(_t: X64V3Token, md: &DecModel, sym: __m256i) -> (__m256i, __m256i, __m256i) {
        let zero = _mm256_setzero_si256();
        let one = _mm256_set1_epi32(1);
        let is_run = _mm256_cmpgt_epi32(sym, _mm256_set1_epi32(md.vt as i32 - 1));
        let t = _mm256_sub_epi32(
            sym,
            _mm256_and_si256(is_run, _mm256_set1_epi32(md.vt as i32)),
        );
        let kk = _mm256_blendv_epi8(
            _mm256_set1_epi32(md.k as i32),
            _mm256_set1_epi32(md.kr as i32),
            is_run,
        );
        let mm = _mm256_set1_epi32(md.m as i32);
        let dlim = _mm256_sllv_epi32(one, kk);
        let direct = _mm256_cmpgt_epi32(dlim, t);
        let tt = _mm256_sub_epi32(t, dlim);
        let msb = _mm256_add_epi32(kk, _mm256_srlv_epi32(tt, mm));
        let top = _mm256_and_si256(tt, _mm256_sub_epi32(_mm256_sllv_epi32(one, mm), one));
        let nb = _mm256_sub_epi32(msb, mm);
        let base = _mm256_sllv_epi32(_mm256_or_si256(_mm256_sllv_epi32(one, mm), top), nb);
        (
            _mm256_blendv_epi8(base, t, direct),
            _mm256_blendv_epi8(nb, zero, direct),
            is_run,
        )
    }

    /// One row of one slot, 32-bit residual storage.
    #[arcane(import_intrinsics)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn decode_row(
        t: X64V3Token,
        md: &DecModel,
        words: &Words<'_>,
        stt: &mut LaneState,
        widths: &[u32; NL],
        lw: usize,
        row: usize,
        prev: &[u32],
        cross: Option<&[u32]>,
        out: &mut [u32],
    ) {
        let zero = _mm256_setzero_si256();
        let ones = _mm256_set1_epi32(-1);
        let mut cm = [0u8; 32];
        cm[..16].copy_from_slice(&md.ctx_map);
        cm[16..].copy_from_slice(&md.ctx_map);
        let cmapv = _mm256_loadu_si256(&cm);
        let sh = _mm_cvtsi32_si128(md.ctx_shift as i32);
        let cap = _mm256_set1_epi32(1 << 16);
        let term = |u: __m256i| _mm256_min_epu32(_mm256_srl_epi32(u, sh), cap);
        let c16 = _mm256_set1_epi32(16);
        let wid = [ld(t, &widths[..8]), ld(t, &widths[8..])];
        let mut x = [ld(t, &stt.x[..8]), ld(t, &stt.x[8..])];
        let mut run = [ld(t, &stt.run[..8]), ld(t, &stt.run[8..])];
        let mut uw = [zero; 2];
        let mut wp = stt.wp;
        let w = words;
        let dec = &*md.dec;
        let mut xi = 0usize;
        while xi < lw {
            let colv = _mm256_set1_epi32(xi as i32);
            let inw = [
                _mm256_cmpgt_epi32(wid[0], colv),
                _mm256_cmpgt_epi32(wid[1], colv),
            ];
            let busy0 = _mm256_or_si256(
                _mm256_cmpgt_epi32(run[0], zero),
                _mm256_xor_si256(inw[0], ones),
            );
            let busy1 = _mm256_or_si256(
                _mm256_cmpgt_epi32(run[1], zero),
                _mm256_xor_si256(inw[1], ones),
            );
            if _mm256_movemask_ps(_mm256_castsi256_ps(_mm256_and_si256(busy0, busy1))) == 0xFF {
                let big = _mm256_set1_epi32(i32::MAX);
                let m0 = _mm256_blendv_epi8(
                    big,
                    _mm256_min_epu32(run[0], _mm256_sub_epi32(wid[0], colv)),
                    inw[0],
                );
                let m1 = _mm256_blendv_epi8(
                    big,
                    _mm256_min_epu32(run[1], _mm256_sub_epi32(wid[1], colv)),
                    inw[1],
                );
                let mut mm = [0u32; 8];
                _mm256_storeu_si256(&mut mm, _mm256_min_epu32(m0, m1));
                let k = (mm.iter().copied().min().unwrap() as usize).min(lw - xi);
                if k > 0 {
                    let kv = _mm256_set1_epi32(k as i32);
                    run[0] = _mm256_sub_epi32(run[0], _mm256_and_si256(kv, inw[0]));
                    run[1] = _mm256_sub_epi32(run[1], _mm256_and_si256(kv, inw[1]));
                    out[xi * NL..(xi + k) * NL].fill(0);
                    uw = [zero; 2];
                    xi += k;
                    continue;
                }
            }
            let bi = xi * NL;
            let mut act = [zero; 2];
            let mut e = [zero; 2];
            let mut ia = [0u32; 16];
            for h in 0..2 {
                let in_run = _mm256_and_si256(_mm256_cmpgt_epi32(run[h], zero), inw[h]);
                run[h] = _mm256_add_epi32(run[h], in_run);
                act[h] = _mm256_andnot_si256(in_run, inw[h]);
                let mut sum = if row > 0 {
                    let n = term(ld(t, &prev[bi + 8 * h..]));
                    if xi > 0 {
                        _mm256_add_epi32(term(uw[h]), n)
                    } else {
                        _mm256_add_epi32(n, n)
                    }
                } else if xi > 0 {
                    let a = term(uw[h]);
                    _mm256_add_epi32(a, a)
                } else {
                    zero
                };
                if let Some(cr) = cross {
                    sum = _mm256_add_epi32(sum, term(ld(t, &cr[bi + 8 * h..])));
                }
                let ex = _mm256_srli_epi32::<23>(_mm256_castps_si256(_mm256_cvtepi32_ps(sum)));
                let cls = _mm256_min_epi32(
                    _mm256_max_epi32(_mm256_sub_epi32(ex, _mm256_set1_epi32(126)), zero),
                    _mm256_set1_epi32(15),
                );
                let ctx = _mm256_and_si256(_mm256_shuffle_epi8(cmapv, cls), _mm256_set1_epi32(3));
                let idx = _mm256_or_si256(
                    _mm256_slli_epi32::<10>(ctx),
                    _mm256_and_si256(x[h], _mm256_set1_epi32(1023)),
                );
                let a: &mut [u32; 8] = (&mut ia[8 * h..8 * h + 8]).try_into().unwrap();
                _mm256_storeu_si256(a, idx);
            }
            let ev: [u32; 16] = core::array::from_fn(|i| dec[ia[i] as usize & (TABLE - 1)]);
            for h in 0..2 {
                e[h] = ld(t, &ev[8 * h..]);
                let f = _mm256_add_epi32(
                    _mm256_and_si256(e[h], _mm256_set1_epi32(1023)),
                    _mm256_set1_epi32(1),
                );
                let bias = _mm256_and_si256(_mm256_srli_epi32::<10>(e[h]), _mm256_set1_epi32(1023));
                let nx =
                    _mm256_add_epi32(_mm256_mullo_epi32(f, _mm256_srli_epi32::<10>(x[h])), bias);
                let xx = _mm256_blendv_epi8(x[h], nx, act[h]);
                x[h] = refill(t, xx, _mm256_and_si256(act[h], below_l(t, xx)), w, &mut wp);
            }
            let mut info = [(zero, zero, zero); 2];
            for h in 0..2 {
                let sym = _mm256_and_si256(_mm256_srli_epi32::<20>(e[h]), _mm256_set1_epi32(255));
                info[h] = tok_info(t, md, sym);
            }
            // Low extra bits (phase 1), then high (phase 2), lanes ascending.
            let mut ebits = [zero; 2];
            let nb_any = _mm256_or_si256(
                _mm256_and_si256(info[0].1, act[0]),
                _mm256_and_si256(info[1].1, act[1]),
            );
            if _mm256_testz_si256(nb_any, nb_any) == 0 {
                for h in 0..2 {
                    let n1 = _mm256_min_epu32(info[h].1, c16);
                    let m = _mm256_and_si256(act[h], _mm256_cmpgt_epi32(n1, zero));
                    ebits[h] = pull(t, &mut x[h], n1, m, w, &mut wp);
                }
                let hi_any = _mm256_or_si256(
                    _mm256_and_si256(_mm256_cmpgt_epi32(info[0].1, c16), act[0]),
                    _mm256_and_si256(_mm256_cmpgt_epi32(info[1].1, c16), act[1]),
                );
                if _mm256_testz_si256(hi_any, hi_any) == 0 {
                    for h in 0..2 {
                        let m = _mm256_and_si256(act[h], _mm256_cmpgt_epi32(info[h].1, c16));
                        let n2 = _mm256_and_si256(_mm256_sub_epi32(info[h].1, c16), m);
                        let hb = pull(t, &mut x[h], n2, m, w, &mut wp);
                        ebits[h] = _mm256_or_si256(ebits[h], _mm256_slli_epi32::<16>(hb));
                    }
                }
            }
            for h in 0..2 {
                let (base, _, is_run) = info[h];
                let u = _mm256_add_epi32(base, ebits[h]);
                let run_now = _mm256_and_si256(act[h], is_run);
                run[h] = _mm256_blendv_epi8(run[h], u, run_now);
                let val = _mm256_andnot_si256(run_now, _mm256_and_si256(act[h], u));
                uw[h] = val;
                st(t, &mut out[bi + 8 * h..], val);
            }
            xi += 1;
        }
        st(t, &mut stt.x[..8], x[0]);
        st(t, &mut stt.x[8..], x[1]);
        st(t, &mut stt.run[..8], run[0]);
        st(t, &mut stt.run[8..], run[1]);
        stt.wp = wp;
    }

    /// a <= b (unsigned 32-bit) as a lane mask.
    #[rite(import_intrinsics)]
    fn le(_t: X64V3Token, a: __m256i, b: __m256i) -> __m256i {
        _mm256_cmpeq_epi32(_mm256_min_epu32(a, b), a)
    }

    #[rite(import_intrinsics)]
    fn absdiff(_t: X64V3Token, a: __m256i, b: __m256i) -> __m256i {
        _mm256_sub_epi32(_mm256_max_epu32(a, b), _mm256_min_epu32(a, b))
    }

    #[rite(import_intrinsics)]
    fn favg(_t: X64V3Token, a: __m256i, b: __m256i) -> __m256i {
        let one = _mm256_set1_epi32(1);
        _mm256_add_epi32(
            _mm256_add_epi32(_mm256_srli_epi32::<1>(a), _mm256_srli_epi32::<1>(b)),
            _mm256_and_si256(_mm256_and_si256(a, b), one),
        )
    }

    #[rite(import_intrinsics)]
    fn predict<const P: u8>(
        t: X64V3Token,
        w: __m256i,
        n: __m256i,
        nw: __m256i,
        ne: __m256i,
        mask: __m256i,
    ) -> __m256i {
        match P {
            predict::P_W => w,
            predict::P_N => n,
            predict::P_AVG_WN => favg(t, w, n),
            predict::P_MED => {
                let lo = _mm256_min_epu32(w, n);
                let hi = _mm256_max_epu32(w, n);
                let grad = _mm256_sub_epi32(_mm256_add_epi32(w, n), nw);
                let r = _mm256_blendv_epi8(grad, hi, le(t, nw, lo));
                _mm256_blendv_epi8(r, lo, le(t, hi, nw))
            }
            predict::P_GRAD => _mm256_and_si256(_mm256_sub_epi32(_mm256_add_epi32(w, n), nw), mask),
            predict::P_PAETH => {
                // pa = |b-c|, pb = |a-c|; pc = |pa-pb| when a-c and b-c have
                // opposite signs, else pa+pb (never the strict minimum).
                let (a, b, c) = (w, n, nw);
                let pa = absdiff(t, b, c);
                let pb = absdiff(t, a, c);
                let same = _mm256_xor_si256(
                    le(t, c, a),
                    _mm256_xor_si256(le(t, c, b), _mm256_set1_epi32(-1)),
                );
                let pc = _mm256_or_si256(absdiff(t, pa, pb), same);
                let a_ok = _mm256_and_si256(le(t, pa, pb), le(t, pa, pc));
                let b_ok = le(t, pb, pc);
                _mm256_blendv_epi8(_mm256_blendv_epi8(c, b, b_ok), a, a_ok)
            }
            _ => favg(t, w, ne),
        }
    }

    /// Near-lossless reconstruction (SPEC 2.6) for u32 lanes:
    /// `clamp(p + sext(d) * step, 0, mask)` with `step = 2 delta + 1 < 2^32`
    /// and `sh = 32 - bits`. 64-bit products saturate at 0xFFFF_FFFF, which
    /// the clamp makes exact.
    #[rite(import_intrinsics)]
    fn near_add(
        _t: X64V3Token,
        p: __m256i,
        d: __m256i,
        sh: __m128i,
        step: __m256i,
        mask: __m256i,
    ) -> __m256i {
        let zero = _mm256_setzero_si256();
        let ones = _mm256_set1_epi32(-1);
        let q = _mm256_sra_epi32(_mm256_sll_epi32(d, sh), sh);
        let mag = _mm256_abs_epi32(q);
        let even = _mm256_mul_epu32(mag, step);
        let odd = _mm256_mul_epu32(_mm256_srli_epi64::<32>(mag), step);
        let sat = |v: __m256i| {
            let fits = _mm256_cmpeq_epi64(_mm256_srli_epi64::<32>(v), zero);
            _mm256_or_si256(v, _mm256_xor_si256(fits, ones))
        };
        let t = _mm256_blend_epi32::<0b1010_1010>(sat(even), _mm256_slli_epi64::<32>(sat(odd)));
        let sum = _mm256_add_epi32(p, t);
        let no_carry = _mm256_cmpeq_epi32(_mm256_max_epu32(sum, p), sum);
        let up = _mm256_min_epu32(_mm256_or_si256(sum, _mm256_xor_si256(no_carry, ones)), mask);
        let down = _mm256_sub_epi32(_mm256_max_epu32(p, t), t);
        _mm256_blendv_epi8(up, down, _mm256_cmpgt_epi32(zero, q))
    }

    /// One row of reconstruction with 32-bit storage; `bits` and `near` are
    /// each slot's coded width and near-lossless bound. `NEAR`: some bound
    /// is nonzero (slots with bound 0 stay modular).
    #[arcane(import_intrinsics)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn recon_row<const NS: usize, const P: u8, const NEAR: bool>(
        t: X64V3Token,
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
        let zero = _mm256_setzero_si256();
        let one = _mm256_set1_epi32(1);
        let masks: [__m256i; NS] =
            core::array::from_fn(|s| _mm256_set1_epi32(crate::mask_of(bits[s]) as i32));
        let near_on: [bool; NS] = core::array::from_fn(|s| NEAR && near[s] != 0);
        let steps: [__m256i; NS] =
            core::array::from_fn(|s| _mm256_set1_epi32((2 * near[s] as u64 + 1) as u32 as i32));
        let shs: [__m128i; NS] =
            core::array::from_fn(|s| _mm_cvtsi32_si128(32 - bits[s].max(1) as i32));
        let cbits = bits[0];
        let shc = _mm_cvtsi32_si128(32 - cbits.max(1) as i32);
        let do_decor = color_slots == 3 && decor != 0 && NS >= 3;
        let wid = [ld(t, &widths[..8]), ld(t, &widths[8..])];
        for h in 0..2 {
            let mut wv = [zero; NS];
            let mut nwv = [zero; NS];
            for xi in 0..lw {
                let bi = xi * NL + 8 * h;
                let mut d: [__m256i; NS] = core::array::from_fn(|s| {
                    let u = ld(t, &res[s][bi..]);
                    let neg = _mm256_sub_epi32(zero, _mm256_and_si256(u, one));
                    _mm256_and_si256(_mm256_xor_si256(_mm256_srli_epi32::<1>(u), neg), masks[s])
                });
                if do_decor {
                    let m = masks[0];
                    let g = d[0];
                    d[1] = _mm256_and_si256(_mm256_add_epi32(d[1], g), m);
                    d[2] = match decor {
                        1 => _mm256_and_si256(_mm256_add_epi32(d[2], g), m),
                        2 => _mm256_and_si256(_mm256_add_epi32(d[2], d[1]), m),
                        _ => {
                            let a = _mm256_sra_epi32(_mm256_sll_epi32(d[1], shc), shc);
                            let b = _mm256_sra_epi32(_mm256_sll_epi32(g, shc), shc);
                            let avg = _mm256_add_epi32(
                                _mm256_add_epi32(
                                    _mm256_srai_epi32::<1>(a),
                                    _mm256_srai_epi32::<1>(b),
                                ),
                                _mm256_and_si256(_mm256_and_si256(a, b), one),
                            );
                            _mm256_and_si256(_mm256_add_epi32(d[2], avg), m)
                        }
                    };
                }
                for s in 0..NS {
                    let v = if row == 0 {
                        let w = if xi > 0 { wv[s] } else { zero };
                        if near_on[s] {
                            near_add(t, w, d[s], shs[s], steps[s], masks[s])
                        } else {
                            _mm256_and_si256(_mm256_add_epi32(w, d[s]), masks[s])
                        }
                    } else {
                        let n = ld(t, &prev[s][bi..]);
                        let (w, nw) = if xi > 0 { (wv[s], nwv[s]) } else { (n, n) };
                        let ne = if xi + 1 < lw {
                            let has =
                                _mm256_cmpgt_epi32(wid[h], _mm256_set1_epi32((xi + 1) as i32));
                            _mm256_blendv_epi8(n, ld(t, &prev[s][bi + NL..]), has)
                        } else {
                            n
                        };
                        nwv[s] = n;
                        let p = predict::<P>(t, w, n, nw, ne, masks[s]);
                        if near_on[s] {
                            near_add(t, p, d[s], shs[s], steps[s], masks[s])
                        } else {
                            _mm256_and_si256(_mm256_add_epi32(p, d[s]), masks[s])
                        }
                    };
                    wv[s] = v;
                    st(t, &mut cur[s][bi..], v);
                }
            }
        }
    }
}

/// Order map inverse of one 32-bit output word (SPEC 2.1): u32 and the lo
/// word of a 64-bit sample are plain; i32 and the hi word of an i64 flip the
/// sign bit; f32 and the hi word of an f64 use the float map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub(crate) enum Ord32 {
    Plain,
    Sign,
    Float,
}

/// Per-channel transform for the 32-bit output writer (its operands are
/// read only by the x86 writer).
#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub(crate) enum Out32 {
    Ident,
    Add(u32),
    Shl(u32),
    Const(u32),
    /// Lookup in the channel's expansion table (coded bits <= 16).
    Lut,
    /// `MapMode::FloatShift` of `bits` > 16 coded bits (f32).
    FloatShift(u32),
}

#[cfg(target_arch = "x86_64")]
pub(crate) mod out_avx2 {
    use super::{Ord32, Out32};
    use crate::rans::NL;
    use archmage::prelude::*;

    #[rite(import_intrinsics)]
    fn transpose8(_t: X64V3Token, r: &[__m256i; 8]) -> [__m256i; 8] {
        let a0 = _mm256_unpacklo_epi32(r[0], r[1]);
        let a1 = _mm256_unpackhi_epi32(r[0], r[1]);
        let a2 = _mm256_unpacklo_epi32(r[2], r[3]);
        let a3 = _mm256_unpackhi_epi32(r[2], r[3]);
        let a4 = _mm256_unpacklo_epi32(r[4], r[5]);
        let a5 = _mm256_unpackhi_epi32(r[4], r[5]);
        let a6 = _mm256_unpacklo_epi32(r[6], r[7]);
        let a7 = _mm256_unpackhi_epi32(r[6], r[7]);
        let b0 = _mm256_unpacklo_epi64(a0, a2);
        let b1 = _mm256_unpackhi_epi64(a0, a2);
        let b2 = _mm256_unpacklo_epi64(a1, a3);
        let b3 = _mm256_unpackhi_epi64(a1, a3);
        let b4 = _mm256_unpacklo_epi64(a4, a6);
        let b5 = _mm256_unpackhi_epi64(a4, a6);
        let b6 = _mm256_unpacklo_epi64(a5, a7);
        let b7 = _mm256_unpackhi_epi64(a5, a7);
        [
            _mm256_permute2x128_si256::<0x20>(b0, b4),
            _mm256_permute2x128_si256::<0x20>(b1, b5),
            _mm256_permute2x128_si256::<0x20>(b2, b6),
            _mm256_permute2x128_si256::<0x20>(b3, b7),
            _mm256_permute2x128_si256::<0x31>(b0, b4),
            _mm256_permute2x128_si256::<0x31>(b1, b5),
            _mm256_permute2x128_si256::<0x31>(b2, b6),
            _mm256_permute2x128_si256::<0x31>(b3, b7),
        ]
    }

    #[rite(import_intrinsics)]
    fn st32(_t: X64V3Token, d: &mut [u8], v: __m256i) {
        let a: &mut [u8; 32] = (&mut d[..32]).try_into().unwrap();
        _mm256_storeu_si256(a, v);
    }

    /// Writes 8 pixels (`nch` words, u32 each: channels, or the lo and hi
    /// words of 64-bit channels) to `dst`.
    #[rite(import_intrinsics)]
    fn write8px(t: X64V3Token, ch: &[__m256i; 8], nch: usize, dst: &mut [u8]) {
        match nch {
            1 => st32(t, dst, ch[0]),
            2 => {
                let lo = _mm256_unpacklo_epi32(ch[0], ch[1]);
                let hi = _mm256_unpackhi_epi32(ch[0], ch[1]);
                st32(t, &mut dst[..32], _mm256_permute2x128_si256::<0x20>(lo, hi));
                st32(t, &mut dst[32..], _mm256_permute2x128_si256::<0x31>(lo, hi));
            }
            3 | 6 => {
                let mut a = [[0u32; 8]; 6];
                for c in 0..nch {
                    _mm256_storeu_si256(&mut a[c], ch[c]);
                }
                for i in 0..8 {
                    for c in 0..nch {
                        let o = (i * nch + c) * 4;
                        dst[o..o + 4].copy_from_slice(&a[c][i].to_ne_bytes());
                    }
                }
            }
            8 => {
                // One transposed vector per pixel.
                for (i, v) in transpose8(t, ch).iter().enumerate() {
                    st32(t, &mut dst[32 * i..32 * i + 32], *v);
                }
            }
            _ => {
                let rg_lo = _mm256_unpacklo_epi32(ch[0], ch[1]);
                let rg_hi = _mm256_unpackhi_epi32(ch[0], ch[1]);
                let ba_lo = _mm256_unpacklo_epi32(ch[2], ch[3]);
                let ba_hi = _mm256_unpackhi_epi32(ch[2], ch[3]);
                let p0 = _mm256_unpacklo_epi64(rg_lo, ba_lo);
                let p1 = _mm256_unpackhi_epi64(rg_lo, ba_lo);
                let p2 = _mm256_unpacklo_epi64(rg_hi, ba_hi);
                let p3 = _mm256_unpackhi_epi64(rg_hi, ba_hi);
                st32(t, &mut dst[..32], _mm256_permute2x128_si256::<0x20>(p0, p1));
                st32(
                    t,
                    &mut dst[32..64],
                    _mm256_permute2x128_si256::<0x20>(p2, p3),
                );
                st32(
                    t,
                    &mut dst[64..96],
                    _mm256_permute2x128_si256::<0x31>(p0, p1),
                );
                st32(
                    t,
                    &mut dst[96..128],
                    _mm256_permute2x128_si256::<0x31>(p2, p3),
                );
            }
        }
    }

    /// Writes one row of 32-bit output words from u16 (`narrow`) or u32
    /// storage: `kinds`, `slot_of` and `luts` per coded channel, `words` the
    /// output words of a pixel in memory order (coded channel, order map).
    #[arcane(import_intrinsics)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn write_row32(
        t: X64V3Token,
        cur16: Option<&[&[u16]]>,
        cur32: Option<&[&[u32]]>,
        slot_of: &[Option<usize>],
        kinds: &[Out32],
        luts: &[Option<alloc::vec::Vec<u32>>],
        words: &[(usize, Ord32)],
        widths: &[u32; NL],
        lw: usize,
        out_row: &mut [u8],
    ) {
        let ncc = kinds.len();
        let nch = words.len();
        let bpp = 4 * nch;
        let mut tmp = [0u8; 256];
        let load = |s: usize, i: usize| -> __m256i {
            match (cur16, cur32) {
                (Some(c), _) => {
                    let a: &[u16; 8] = c[s][i..i + 8].try_into().unwrap();
                    _mm256_cvtepu16_epi32(_mm_loadu_si128(a))
                }
                (_, Some(c)) => {
                    let a: &[u32; 8] = c[s][i..i + 8].try_into().unwrap();
                    _mm256_loadu_si256(a)
                }
                _ => _mm256_setzero_si256(),
            }
        };
        for xb in (0..lw).step_by(8) {
            let n = 8.min(lw - xb);
            for h in 0..2 {
                // lanes[c][j] = lane 8h+j, steps xb..xb+8.
                let mut lanes = [[_mm256_setzero_si256(); 8]; 8];
                for c in 0..ncc {
                    let Some(s) = slot_of[c] else { continue };
                    let mut rows = [_mm256_setzero_si256(); 8];
                    for (i, r) in rows.iter_mut().enumerate().take(n) {
                        *r = load(s, (xb + i) * NL + 8 * h);
                    }
                    lanes[c] = transpose8(t, &rows);
                }
                for j in 0..8 {
                    let l = 8 * h + j;
                    let wl = widths[l] as usize;
                    if wl <= xb {
                        continue;
                    }
                    let cnt = 8.min(wl - xb);
                    let ch: [__m256i; 8] = core::array::from_fn(|k| {
                        let Some(&(c, ord)) = words.get(k) else {
                            return _mm256_setzero_si256();
                        };
                        let v = lanes[c][j];
                        let v = match kinds[c] {
                            Out32::Ident => v,
                            Out32::Add(a) => _mm256_add_epi32(v, _mm256_set1_epi32(a as i32)),
                            Out32::Shl(k) => _mm256_sll_epi32(v, _mm_cvtsi32_si128(k as i32)),
                            Out32::Const(k) => _mm256_set1_epi32(k as i32),
                            Out32::Lut => {
                                let lut = luts[c].as_ref().unwrap();
                                let mut iv = [0u32; 8];
                                _mm256_storeu_si256(&mut iv, v);
                                let o: [u32; 8] = core::array::from_fn(|i| lut[iv[i] as usize]);
                                _mm256_loadu_si256(&o)
                            }
                            Out32::FloatShift(n) => {
                                // Order map of the n-bit pattern, then the
                                // raw bits in place, then the f32 order map.
                                let half = _mm256_set1_epi32(crate::mask_of(n - 1) as i32);
                                let top = _mm256_and_si256(
                                    _mm256_srl_epi32(v, _mm_cvtsi32_si128(n as i32 - 1)),
                                    _mm256_set1_epi32(1),
                                );
                                let neg = _mm256_sub_epi32(_mm256_setzero_si256(), top);
                                let raw = _mm256_sll_epi32(
                                    _mm256_xor_si256(v, _mm256_and_si256(neg, half)),
                                    _mm_cvtsi32_si128(32 - n as i32),
                                );
                                _mm256_xor_si256(
                                    raw,
                                    _mm256_and_si256(
                                        _mm256_srai_epi32::<31>(raw),
                                        _mm256_set1_epi32(0x7FFF_FFFF),
                                    ),
                                )
                            }
                        };
                        match ord {
                            Ord32::Sign => _mm256_xor_si256(v, _mm256_set1_epi32(i32::MIN)),
                            Ord32::Float => _mm256_xor_si256(
                                v,
                                _mm256_and_si256(
                                    _mm256_srai_epi32::<31>(v),
                                    _mm256_set1_epi32(0x7FFF_FFFF),
                                ),
                            ),
                            Ord32::Plain => v,
                        }
                    });
                    let px = l * lw + xb;
                    if cnt == 8 {
                        write8px(t, &ch, nch, &mut out_row[px * bpp..(px + 8) * bpp]);
                    } else {
                        write8px(t, &ch, nch, &mut tmp[..8 * bpp]);
                        out_row[px * bpp..(px + cnt) * bpp].copy_from_slice(&tmp[..cnt * bpp]);
                    }
                }
            }
        }
    }
}

/// wasm128 output writer for 32-bit samples: same bytes as
/// `out_avx2::write_row32`, with 4x4 u32 transposes per 128-bit lane group
/// and 16-byte stores. Channel counts 3, 6 and 8 stay scalar per pixel,
/// exactly as the x86 writer does.
#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
pub(crate) mod out_wasm128 {
    use super::{Ord32, Out32};
    use crate::rans::NL;
    use archmage::prelude::*;
    use core::arch::wasm32::{i32x4_shuffle, i64x2_shuffle, v128};
    use magetypes::simd::{u8x16, u16x8, u32x4, u32x8};

    /// 4x4 u32 transpose (one `v128` per row): `out[j][i] = r[i][j]`.
    #[rite]
    fn t4x4(_t: Wasm128Token, r: [v128; 4]) -> [v128; 4] {
        let t0 = i32x4_shuffle::<0, 4, 1, 5>(r[0], r[1]);
        let t1 = i32x4_shuffle::<2, 6, 3, 7>(r[0], r[1]);
        let t2 = i32x4_shuffle::<0, 4, 1, 5>(r[2], r[3]);
        let t3 = i32x4_shuffle::<2, 6, 3, 7>(r[2], r[3]);
        [
            i64x2_shuffle::<0, 2>(t0, t2),
            i64x2_shuffle::<1, 3>(t0, t2),
            i64x2_shuffle::<0, 2>(t1, t3),
            i64x2_shuffle::<1, 3>(t1, t3),
        ]
    }

    /// Stores 16-byte chunks covering `dst`; a partial tail copies.
    #[rite]
    fn emit(t: Wasm128Token, chunks: &[v128], dst: &mut [u8]) {
        let mut off = 0;
        for &c in chunks {
            if off >= dst.len() {
                break;
            }
            let n = (dst.len() - off).min(16);
            if n == 16 {
                let d: &mut [u8; 16] = (&mut dst[off..off + 16]).try_into().unwrap();
                u8x16::from_raw_t(t, c).store(d);
            } else {
                let mut tmp = [0u8; 16];
                u8x16::from_raw_t(t, c).store(&mut tmp);
                dst[off..off + n].copy_from_slice(&tmp[..n]);
            }
            off += n;
        }
    }

    /// Applies the per-channel transform and order map to eight lanes.
    #[rite]
    fn apply8(t: Wasm128Token, v: u32x8, kind: Out32, ord: Ord32, lut: Option<&[u32]>) -> u32x8 {
        let v = match kind {
            Out32::Ident => v,
            Out32::Add(a) => v + u32x8::splat_t(t, a),
            Out32::Shl(k) => v.shl_uniform(k),
            Out32::Const(k) => u32x8::splat_t(t, k),
            Out32::Lut => {
                let lut = lut.unwrap();
                let a = v.to_array();
                u32x8::from_array_t(t, core::array::from_fn(|i| lut[a[i] as usize]))
            }
            Out32::FloatShift(n) => {
                // Order map of the n-bit pattern, then the raw bits in
                // place, then the f32 order map.
                let half = u32x8::splat_t(t, crate::mask_of(n - 1));
                let top = v.shr_logical_uniform(n - 1) & u32x8::splat_t(t, 1);
                let neg = u32x8::zero_t(t) - top;
                let raw = (v ^ (neg & half)).shl_uniform(32 - n);
                raw ^ (raw.simd_ge(u32x8::splat_t(t, 1 << 31)) & u32x8::splat_t(t, 0x7FFF_FFFF))
            }
        };
        match ord {
            Ord32::Plain => v,
            Ord32::Sign => v ^ u32x8::splat_t(t, 1 << 31),
            Ord32::Float => {
                v ^ (v.simd_ge(u32x8::splat_t(t, 1 << 31)) & u32x8::splat_t(t, 0x7FFF_FFFF))
            }
        }
    }

    /// Writes 8 pixels (`nch` words, u32 each) to `dst`.
    #[rite]
    fn write8px(t: Wasm128Token, ch: &[u32x8; 8], nch: usize, dst: &mut [u8]) {
        let ilv = |a: v128, b: v128, hi: bool| -> v128 {
            if hi {
                i32x4_shuffle::<2, 6, 3, 7>(a, b)
            } else {
                i32x4_shuffle::<0, 4, 1, 5>(a, b)
            }
        };
        let p64 = |a: v128, b: v128, hi: bool| -> v128 {
            if hi {
                i64x2_shuffle::<1, 3>(a, b)
            } else {
                i64x2_shuffle::<0, 2>(a, b)
            }
        };
        match nch {
            1 => {
                let h: [v128; 2] = ch[0].into_repr();
                emit(t, &[h[0], h[1]], dst);
            }
            2 => {
                let (a, b) = (ch[0].into_repr(), ch[1].into_repr());
                emit(
                    t,
                    &[
                        ilv(a[0], b[0], false),
                        ilv(a[0], b[0], true),
                        ilv(a[1], b[1], false),
                        ilv(a[1], b[1], true),
                    ],
                    dst,
                );
            }
            4 => {
                let (r, g, b, a) = (
                    ch[0].into_repr(),
                    ch[1].into_repr(),
                    ch[2].into_repr(),
                    ch[3].into_repr(),
                );
                let half = |i: usize| -> [v128; 4] {
                    let rg_lo = ilv(r[i], g[i], false);
                    let rg_hi = ilv(r[i], g[i], true);
                    let ba_lo = ilv(b[i], a[i], false);
                    let ba_hi = ilv(b[i], a[i], true);
                    [
                        p64(rg_lo, ba_lo, false),
                        p64(rg_lo, ba_lo, true),
                        p64(rg_hi, ba_hi, false),
                        p64(rg_hi, ba_hi, true),
                    ]
                };
                let p0 = half(0);
                let p1 = half(1);
                emit(
                    t,
                    &[p0[0], p0[1], p0[2], p0[3], p1[0], p1[1], p1[2], p1[3]],
                    dst,
                );
            }
            _ => {
                // 3, 6 and 8 words: scalar per-pixel writes (as on x86).
                let a: [[u32; 8]; 8] =
                    core::array::from_fn(|k| if k < nch { ch[k].to_array() } else { [0; 8] });
                for i in 0..8 {
                    for k in 0..nch {
                        let o = (i * nch + k) * 4;
                        dst[o..o + 4].copy_from_slice(&a[k][i].to_ne_bytes());
                    }
                }
            }
        }
    }

    /// Writes one row of 32-bit output words from u16 (`narrow`) or u32
    /// storage; `words` lists (coded channel, order map) per output word.
    #[arcane]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn write_row32(
        t: Wasm128Token,
        cur16: Option<&[&[u16]]>,
        cur32: Option<&[&[u32]]>,
        slot_of: &[Option<usize>],
        kinds: &[Out32],
        luts: &[Option<alloc::vec::Vec<u32>>],
        words: &[(usize, Ord32)],
        widths: &[u32; NL],
        lw: usize,
        out_row: &mut [u8],
    ) {
        let ncc = kinds.len();
        let nch = words.len();
        let bpp = 4 * nch;
        let zero = u32x4::zero_t(t).raw();
        let mut tmp = [0u8; 256];
        // Loads eight step values of slot `s` from `i` as two u32x4 halves.
        let load = |s: usize, i: usize| -> [v128; 2] {
            match (cur16, cur32) {
                (Some(c), _) => {
                    let w = u16x8::from_slice_t(t, &c[s][i..i + 8]);
                    [w.widen_low().into_repr(), w.widen_high().into_repr()]
                }
                (_, Some(c)) => {
                    let w = u32x8::from_slice_t(t, &c[s][i..i + 8]);
                    w.into_repr()
                }
                _ => [zero, zero],
            }
        };
        for xb in (0..lw).step_by(8) {
            let n = 8.min(lw - xb);
            for h in 0..2 {
                // `lanes[c][j]` = lane 8h + j, steps xb..xb + 8.
                let mut lanes = [[u32x8::zero_t(t); 8]; 8];
                for c in 0..ncc {
                    let Some(s) = slot_of[c] else { continue };
                    let lane = &mut lanes[c];
                    let mut rows = [[zero; 2]; 8];
                    for (i, r) in rows.iter_mut().enumerate().take(n) {
                        *r = load(s, (xb + i) * NL + 8 * h);
                    }
                    let a = t4x4(t, [rows[0][0], rows[1][0], rows[2][0], rows[3][0]]);
                    let b = t4x4(t, [rows[4][0], rows[5][0], rows[6][0], rows[7][0]]);
                    let c2 = t4x4(t, [rows[0][1], rows[1][1], rows[2][1], rows[3][1]]);
                    let d = t4x4(t, [rows[4][1], rows[5][1], rows[6][1], rows[7][1]]);
                    for j in 0..4 {
                        lane[j] = u32x8::from_repr_t(t, [a[j], b[j]]);
                        lane[4 + j] = u32x8::from_repr_t(t, [c2[j], d[j]]);
                    }
                }
                for j in 0..8 {
                    let l = 8 * h + j;
                    let wl = widths[l] as usize;
                    if wl <= xb {
                        continue;
                    }
                    let cnt = 8.min(wl - xb);
                    let ch: [u32x8; 8] = core::array::from_fn(|k| {
                        let Some(&(c, ord)) = words.get(k) else {
                            return u32x8::zero_t(t);
                        };
                        apply8(t, lanes[c][j], kinds[c], ord, luts[c].as_deref())
                    });
                    let px = l * lw + xb;
                    if cnt == 8 {
                        write8px(t, &ch, nch, &mut out_row[px * bpp..(px + 8) * bpp]);
                    } else {
                        write8px(t, &ch, nch, &mut tmp[..8 * bpp]);
                        out_row[px * bpp..(px + cnt) * bpp].copy_from_slice(&tmp[..cnt * bpp]);
                    }
                }
            }
        }
    }
}

/// aarch64 NEON output writer for 32-bit samples: same bytes as
/// `out_wasm128` and `out_avx2`, with 4x4 u32 transposes per 128-bit lane
/// group and 16-byte stores. Channel counts 3, 6 and 8 stay scalar per
/// pixel, exactly as the x86 writer does.
#[cfg(target_arch = "aarch64")]
pub(crate) mod out_neon {
    use super::{Ord32, Out32};
    use crate::rans::NL;
    use archmage::prelude::*;
    use core::arch::aarch64::{
        uint8x16_t, uint32x4_t, vdupq_n_u32, vreinterpretq_u8_u32, vreinterpretq_u32_u64,
        vreinterpretq_u64_u32, vzip1q_u32, vzip1q_u64, vzip2q_u32, vzip2q_u64,
    };
    use magetypes::simd::{u8x16, u16x8, u32x8};

    /// 4x4 u32 transpose (one `uint32x4_t` per row): `out[j][i] = r[i][j]`.
    #[rite]
    fn t4x4(_t: NeonToken, r: [uint32x4_t; 4]) -> [uint32x4_t; 4] {
        let t0 = vzip1q_u32(r[0], r[1]);
        let t1 = vzip2q_u32(r[0], r[1]);
        let t2 = vzip1q_u32(r[2], r[3]);
        let t3 = vzip2q_u32(r[2], r[3]);
        let p64 = |a: uint32x4_t, b: uint32x4_t, hi: bool| -> uint32x4_t {
            let (a, b) = (vreinterpretq_u64_u32(a), vreinterpretq_u64_u32(b));
            vreinterpretq_u32_u64(if hi {
                vzip2q_u64(a, b)
            } else {
                vzip1q_u64(a, b)
            })
        };
        [
            p64(t0, t2, false),
            p64(t0, t2, true),
            p64(t1, t3, false),
            p64(t1, t3, true),
        ]
    }

    /// Stores 16-byte chunks covering `dst`; a partial tail copies.
    #[rite]
    fn emit(t: NeonToken, chunks: &[uint8x16_t], dst: &mut [u8]) {
        let mut off = 0;
        for &c in chunks {
            if off >= dst.len() {
                break;
            }
            let n = (dst.len() - off).min(16);
            if n == 16 {
                let d: &mut [u8; 16] = (&mut dst[off..off + 16]).try_into().unwrap();
                u8x16::from_raw_t(t, c).store(d);
            } else {
                let mut tmp = [0u8; 16];
                u8x16::from_raw_t(t, c).store(&mut tmp);
                dst[off..off + n].copy_from_slice(&tmp[..n]);
            }
            off += n;
        }
    }

    /// Applies the per-channel transform and order map to eight lanes.
    #[rite]
    fn apply8(t: NeonToken, v: u32x8, kind: Out32, ord: Ord32, lut: Option<&[u32]>) -> u32x8 {
        let v = match kind {
            Out32::Ident => v,
            Out32::Add(a) => v + u32x8::splat_t(t, a),
            Out32::Shl(k) => v.shl_uniform(k),
            Out32::Const(k) => u32x8::splat_t(t, k),
            Out32::Lut => {
                let lut = lut.unwrap();
                let a = v.to_array();
                u32x8::from_array_t(t, core::array::from_fn(|i| lut[a[i] as usize]))
            }
            Out32::FloatShift(n) => {
                // Order map of the n-bit pattern, then the raw bits in
                // place, then the f32 order map.
                let half = u32x8::splat_t(t, crate::mask_of(n - 1));
                let top = v.shr_logical_uniform(n - 1) & u32x8::splat_t(t, 1);
                let neg = u32x8::zero_t(t) - top;
                let raw = (v ^ (neg & half)).shl_uniform(32 - n);
                raw ^ (raw.simd_ge(u32x8::splat_t(t, 1 << 31)) & u32x8::splat_t(t, 0x7FFF_FFFF))
            }
        };
        match ord {
            Ord32::Plain => v,
            Ord32::Sign => v ^ u32x8::splat_t(t, 1 << 31),
            Ord32::Float => {
                v ^ (v.simd_ge(u32x8::splat_t(t, 1 << 31)) & u32x8::splat_t(t, 0x7FFF_FFFF))
            }
        }
    }

    /// Writes 8 pixels (`nch` words, u32 each) to `dst`.
    #[rite]
    fn write8px(t: NeonToken, ch: &[u32x8; 8], nch: usize, dst: &mut [u8]) {
        let ilv = |a: uint32x4_t, b: uint32x4_t, hi: bool| -> uint32x4_t {
            if hi {
                vzip2q_u32(a, b)
            } else {
                vzip1q_u32(a, b)
            }
        };
        let p64 = |a: uint32x4_t, b: uint32x4_t, hi: bool| -> uint32x4_t {
            let (a, b) = (vreinterpretq_u64_u32(a), vreinterpretq_u64_u32(b));
            vreinterpretq_u32_u64(if hi {
                vzip2q_u64(a, b)
            } else {
                vzip1q_u64(a, b)
            })
        };
        let as8 = |x: uint32x4_t| vreinterpretq_u8_u32(x);
        match nch {
            1 => {
                let h: [uint32x4_t; 2] = ch[0].into_repr();
                emit(t, &[as8(h[0]), as8(h[1])], dst);
            }
            2 => {
                let (a, b) = (ch[0].into_repr(), ch[1].into_repr());
                emit(
                    t,
                    &[
                        as8(ilv(a[0], b[0], false)),
                        as8(ilv(a[0], b[0], true)),
                        as8(ilv(a[1], b[1], false)),
                        as8(ilv(a[1], b[1], true)),
                    ],
                    dst,
                );
            }
            4 => {
                let (r, g, b, a) = (
                    ch[0].into_repr(),
                    ch[1].into_repr(),
                    ch[2].into_repr(),
                    ch[3].into_repr(),
                );
                let half = |i: usize| -> [uint32x4_t; 4] {
                    let rg_lo = ilv(r[i], g[i], false);
                    let rg_hi = ilv(r[i], g[i], true);
                    let ba_lo = ilv(b[i], a[i], false);
                    let ba_hi = ilv(b[i], a[i], true);
                    [
                        p64(rg_lo, ba_lo, false),
                        p64(rg_lo, ba_lo, true),
                        p64(rg_hi, ba_hi, false),
                        p64(rg_hi, ba_hi, true),
                    ]
                };
                let p0 = half(0);
                let p1 = half(1);
                emit(
                    t,
                    &[
                        as8(p0[0]),
                        as8(p0[1]),
                        as8(p0[2]),
                        as8(p0[3]),
                        as8(p1[0]),
                        as8(p1[1]),
                        as8(p1[2]),
                        as8(p1[3]),
                    ],
                    dst,
                );
            }
            _ => {
                // 3, 6 and 8 words: scalar per-pixel writes (as on x86).
                let a: [[u32; 8]; 8] =
                    core::array::from_fn(|k| if k < nch { ch[k].to_array() } else { [0; 8] });
                for i in 0..8 {
                    for k in 0..nch {
                        let o = (i * nch + k) * 4;
                        dst[o..o + 4].copy_from_slice(&a[k][i].to_ne_bytes());
                    }
                }
            }
        }
    }

    /// Writes one row of 32-bit output words from u16 (`narrow`) or u32
    /// storage; `words` lists (coded channel, order map) per output word.
    #[arcane]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn write_row32(
        t: NeonToken,
        cur16: Option<&[&[u16]]>,
        cur32: Option<&[&[u32]]>,
        slot_of: &[Option<usize>],
        kinds: &[Out32],
        luts: &[Option<alloc::vec::Vec<u32>>],
        words: &[(usize, Ord32)],
        widths: &[u32; NL],
        lw: usize,
        out_row: &mut [u8],
    ) {
        let ncc = kinds.len();
        let nch = words.len();
        let bpp = 4 * nch;
        let zero = vdupq_n_u32(0);
        let mut tmp = [0u8; 256];
        // Loads eight step values of slot `s` from `i` as two u32x4 halves.
        let load = |s: usize, i: usize| -> [uint32x4_t; 2] {
            match (cur16, cur32) {
                (Some(c), _) => {
                    let w = u16x8::from_slice_t(t, &c[s][i..i + 8]);
                    [w.widen_low().into_repr(), w.widen_high().into_repr()]
                }
                (_, Some(c)) => {
                    let w = u32x8::from_slice_t(t, &c[s][i..i + 8]);
                    w.into_repr()
                }
                _ => [zero, zero],
            }
        };
        for xb in (0..lw).step_by(8) {
            let n = 8.min(lw - xb);
            for h in 0..2 {
                // `lanes[c][j]` = lane 8h + j, steps xb..xb + 8.
                let mut lanes = [[u32x8::zero_t(t); 8]; 8];
                for c in 0..ncc {
                    let Some(s) = slot_of[c] else { continue };
                    let lane = &mut lanes[c];
                    let mut rows = [[zero; 2]; 8];
                    for (i, r) in rows.iter_mut().enumerate().take(n) {
                        *r = load(s, (xb + i) * NL + 8 * h);
                    }
                    let a = t4x4(t, [rows[0][0], rows[1][0], rows[2][0], rows[3][0]]);
                    let b = t4x4(t, [rows[4][0], rows[5][0], rows[6][0], rows[7][0]]);
                    let c2 = t4x4(t, [rows[0][1], rows[1][1], rows[2][1], rows[3][1]]);
                    let d = t4x4(t, [rows[4][1], rows[5][1], rows[6][1], rows[7][1]]);
                    for j in 0..4 {
                        lane[j] = u32x8::from_repr_t(t, [a[j], b[j]]);
                        lane[4 + j] = u32x8::from_repr_t(t, [c2[j], d[j]]);
                    }
                }
                for j in 0..8 {
                    let l = 8 * h + j;
                    let wl = widths[l] as usize;
                    if wl <= xb {
                        continue;
                    }
                    let cnt = 8.min(wl - xb);
                    let ch: [u32x8; 8] = core::array::from_fn(|k| {
                        let Some(&(c, ord)) = words.get(k) else {
                            return u32x8::zero_t(t);
                        };
                        apply8(t, lanes[c][j], kinds[c], ord, luts[c].as_deref())
                    });
                    let px = l * lw + xb;
                    if cnt == 8 {
                        write8px(t, &ch, nch, &mut out_row[px * bpp..(px + 8) * bpp]);
                    } else {
                        write8px(t, &ch, nch, &mut tmp[..8 * bpp]);
                        out_row[px * bpp..(px + cnt) * bpp].copy_from_slice(&tmp[..cnt * bpp]);
                    }
                }
            }
        }
    }
}
