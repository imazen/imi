//! SIMD reconstruction of one row in step-major lane layout.
//!
//! All 16 lanes of a step are processed as one vector of u16, so the
//! left-neighbor dependency runs along steps while lanes stay independent.

#[cfg(target_arch = "x86_64")]
pub(crate) mod avx2 {
    /// Pseudo-predictor id: Paeth with 32-bit intermediates (>14-bit data).
    pub(crate) const PAETH32: u8 = 15;
    use crate::predict;
    use crate::rans::NL;
    use archmage::prelude::*;

    #[rite(import_intrinsics)]
    fn ld(_t: X64V3Token, p: &[u16]) -> __m256i {
        let a: &[u16; 16] = p[..16].try_into().unwrap();
        _mm256_loadu_si256(a)
    }

    #[rite(import_intrinsics)]
    fn st(_t: X64V3Token, p: &mut [u16], v: __m256i) {
        let a: &mut [u16; 16] = (&mut p[..16]).try_into().unwrap();
        _mm256_storeu_si256(a, v);
    }

    /// Floor average of u16 lanes.
    #[rite(import_intrinsics)]
    fn favg(_t: X64V3Token, a: __m256i, b: __m256i) -> __m256i {
        let r = _mm256_avg_epu16(a, b);
        _mm256_sub_epi16(
            r,
            _mm256_and_si256(_mm256_xor_si256(a, b), _mm256_set1_epi16(1)),
        )
    }

    /// a >= b (unsigned 16-bit) as a lane mask.
    #[rite(import_intrinsics)]
    fn ge(_t: X64V3Token, a: __m256i, b: __m256i) -> __m256i {
        _mm256_cmpeq_epi16(_mm256_max_epu16(a, b), a)
    }

    #[rite(import_intrinsics)]
    fn med(t: X64V3Token, w: __m256i, n: __m256i, nw: __m256i) -> __m256i {
        let lo = _mm256_min_epu16(w, n);
        let hi = _mm256_max_epu16(w, n);
        let grad = _mm256_sub_epi16(_mm256_add_epi16(w, n), nw);
        let r = _mm256_blendv_epi8(grad, hi, ge(t, lo, nw));
        _mm256_blendv_epi8(r, lo, ge(t, nw, hi))
    }

    /// Paeth for values < 2^14 (i16 arithmetic cannot overflow).
    #[rite(import_intrinsics)]
    fn paeth(_t: X64V3Token, a: __m256i, b: __m256i, c: __m256i) -> __m256i {
        let pa = _mm256_abs_epi16(_mm256_sub_epi16(b, c));
        let pb = _mm256_abs_epi16(_mm256_sub_epi16(a, c));
        let pc = _mm256_abs_epi16(_mm256_sub_epi16(
            _mm256_add_epi16(a, b),
            _mm256_add_epi16(c, c),
        ));
        // pick a if pa <= pb && pa <= pc; else b if pb <= pc; else c
        let a_ok = _mm256_and_si256(
            _mm256_cmpeq_epi16(_mm256_min_epi16(pa, pb), pa),
            _mm256_cmpeq_epi16(_mm256_min_epi16(pa, pc), pa),
        );
        let b_ok = _mm256_cmpeq_epi16(_mm256_min_epi16(pb, pc), pb);
        let r = _mm256_blendv_epi8(c, b, b_ok);
        _mm256_blendv_epi8(r, a, a_ok)
    }

    /// Paeth for full 16-bit values using 32-bit intermediates.
    #[rite(import_intrinsics)]
    fn paeth32(_t: X64V3Token, a: __m256i, b: __m256i, c: __m256i) -> __m256i {
        let half = |v: __m256i, hi: bool| -> __m256i {
            if hi {
                _mm256_cvtepu16_epi32(_mm256_extracti128_si256::<1>(v))
            } else {
                _mm256_cvtepu16_epi32(_mm256_castsi256_si128(v))
            }
        };
        let mut res = [_mm256_setzero_si256(); 2];
        for (h, r) in res.iter_mut().enumerate() {
            let (a, b, c) = (half(a, h == 1), half(b, h == 1), half(c, h == 1));
            let pa = _mm256_abs_epi32(_mm256_sub_epi32(b, c));
            let pb = _mm256_abs_epi32(_mm256_sub_epi32(a, c));
            let pc = _mm256_abs_epi32(_mm256_sub_epi32(
                _mm256_add_epi32(a, b),
                _mm256_add_epi32(c, c),
            ));
            let a_ok = _mm256_and_si256(
                _mm256_cmpeq_epi32(_mm256_min_epi32(pa, pb), pa),
                _mm256_cmpeq_epi32(_mm256_min_epi32(pa, pc), pa),
            );
            let b_ok = _mm256_cmpeq_epi32(_mm256_min_epi32(pb, pc), pb);
            *r = _mm256_blendv_epi8(_mm256_blendv_epi8(c, b, b_ok), a, a_ok);
        }
        // Pack 2 x 8 u32 back to 16 u16 in order.
        _mm256_permute4x64_epi64::<0b11011000>(_mm256_packus_epi32(res[0], res[1]))
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
            predict::P_MED => med(t, w, n, nw),
            predict::P_GRAD => _mm256_and_si256(_mm256_sub_epi16(_mm256_add_epi16(w, n), nw), mask),
            predict::P_PAETH => paeth(t, w, n, nw),
            PAETH32 => paeth32(t, w, n, nw),
            predict::P_AVG_WNE => favg(t, w, ne),
            _ => unreachable!(),
        }
    }

    #[rite(import_intrinsics)]
    fn unzz(_t: X64V3Token, u: __m256i, mask: __m256i) -> __m256i {
        let neg = _mm256_sub_epi16(
            _mm256_setzero_si256(),
            _mm256_and_si256(u, _mm256_set1_epi16(1)),
        );
        _mm256_and_si256(_mm256_xor_si256(_mm256_srli_epi16::<1>(u), neg), mask)
    }

    /// Test the actual vector predictor operations without entropy or mapping
    /// obscuring which input values reach them.
    #[cfg(all(test, feature = "encode"))]
    #[arcane(import_intrinsics)]
    pub(crate) fn predictor_parity_batch(
        t: X64V3Token,
        w: &[u16; 16],
        n: &[u16; 16],
        nw: &[u16; 16],
        ne: &[u16; 16],
        bits: u32,
    ) -> [[u16; 16]; 7] {
        let (w, n, nw, ne) = (ld(t, w), ld(t, n), ld(t, nw), ld(t, ne));
        let mask = _mm256_set1_epi16(crate::mask_of(bits) as i16);
        let paeth = if bits > 14 {
            predict::<PAETH32>(t, w, n, nw, ne, mask)
        } else {
            predict::<5>(t, w, n, nw, ne, mask)
        };
        let vectors = [
            predict::<0>(t, w, n, nw, ne, mask),
            predict::<1>(t, w, n, nw, ne, mask),
            predict::<2>(t, w, n, nw, ne, mask),
            predict::<3>(t, w, n, nw, ne, mask),
            predict::<4>(t, w, n, nw, ne, mask),
            paeth,
            predict::<6>(t, w, n, nw, ne, mask),
        ];
        let mut out = [[0u16; 16]; 7];
        for (dst, value) in out.iter_mut().zip(vectors) {
            st(t, dst, value);
        }
        out
    }

    /// Sign-extends `bits`-bit modular values to i16 (bits <= 16).
    #[rite(import_intrinsics)]
    fn sext(_t: X64V3Token, v: __m256i, sh: __m128i) -> __m256i {
        _mm256_sra_epi16(_mm256_sll_epi16(v, sh), sh)
    }

    /// Near-lossless reconstruction (SPEC 2.6): `clamp(p + sext(d) * step,
    /// 0, mask)` for u16 lanes, where `step = 2 delta + 1 < 2^16` and `sh` is
    /// `16 - bits`. The product saturates at 0xFFFF, which the clamp makes
    /// exact.
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
        let q = _mm256_sra_epi16(_mm256_sll_epi16(d, sh), sh);
        let mag = _mm256_abs_epi16(q);
        let lo = _mm256_mullo_epi16(mag, step);
        let fits = _mm256_cmpeq_epi16(_mm256_mulhi_epu16(mag, step), zero);
        let t = _mm256_or_si256(lo, _mm256_xor_si256(fits, _mm256_set1_epi16(-1)));
        let up = _mm256_min_epu16(_mm256_adds_epu16(p, t), mask);
        let down = _mm256_subs_epu16(p, t);
        _mm256_blendv_epi8(up, down, _mm256_cmpgt_epi16(zero, q))
    }

    #[rite(import_intrinsics)]
    #[inline(always)]
    fn unzz_signed(_t: X64V3Token, u: __m256i) -> __m256i {
        let neg = _mm256_sub_epi16(
            _mm256_setzero_si256(),
            _mm256_and_si256(u, _mm256_set1_epi16(1)),
        );
        _mm256_xor_si256(_mm256_srli_epi16::<1>(u), neg)
    }

    /// Fast specialized 8-bit exact reconstruction (`NEAR == false`, all slots 8-bit).
    /// Zero inner-loop bounds checks, compile-time `P_AVG_WNE` guard, const-generic
    /// `DECOR` & `FIRST_ROW`, and signed unzigzag without redundant intermediate `& 0xFF`.
    #[rite(import_intrinsics)]
    #[inline(always)]
    fn recon_row_8bit<const NS: usize, const P: u8, const DECOR: u8, const FIRST_ROW: bool>(
        t: X64V3Token,
        res: &[&[u16]; NS],
        prev: &[&[u16]; NS],
        cur: &mut [&mut [u16]; NS],
        widv: __m256i,
        lw: usize,
    ) {
        let zero = _mm256_setzero_si256();
        let mask = _mm256_set1_epi16(0x00FF);
        let r_ch: [&[[u16; NL]]; NS] = core::array::from_fn(|s| &res[s].as_chunks::<NL>().0[..lw]);
        let p_ch: [&[[u16; NL]]; NS] = core::array::from_fn(|s| &prev[s].as_chunks::<NL>().0[..lw]);
        let c_ch: [&mut [[u16; NL]]; NS] = {
            let mut it = cur.iter_mut();
            core::array::from_fn(|_| &mut it.next().unwrap().as_chunks_mut::<NL>().0[..lw])
        };
        let mut wv = [zero; NS];
        let mut nwv = [zero; NS];
        for x in 0..lw {
            let mut d: [__m256i; NS] =
                core::array::from_fn(|s| unzz_signed(t, _mm256_loadu_si256(&r_ch[s][x])));
            if DECOR != 0 && NS >= 3 {
                let g = d[0];
                d[1] = _mm256_add_epi16(d[1], g);
                d[2] = match DECOR {
                    1 => _mm256_add_epi16(d[2], g),
                    2 => _mm256_add_epi16(d[2], d[1]),
                    _ => {
                        // Exact floor((sext8(r) + g) / 2): g is already signed i16 in [-128..127].
                        let rs = _mm256_srai_epi16::<8>(_mm256_slli_epi16::<8>(d[1]));
                        let avg = _mm256_srai_epi16::<1>(_mm256_add_epi16(rs, g));
                        _mm256_add_epi16(d[2], avg)
                    }
                };
            }
            for s in 0..NS {
                let v = if FIRST_ROW {
                    let w = if x > 0 { wv[s] } else { zero };
                    _mm256_and_si256(_mm256_add_epi16(w, d[s]), mask)
                } else {
                    let n = _mm256_loadu_si256(&p_ch[s][x]);
                    let (w, nw) = if x > 0 { (wv[s], nwv[s]) } else { (n, n) };
                    let ne = if P == predict::P_AVG_WNE && x + 1 < lw {
                        let ner = _mm256_loadu_si256(&p_ch[s][x + 1]);
                        let has = _mm256_cmpgt_epi16(widv, _mm256_set1_epi16((x + 1) as i16));
                        _mm256_blendv_epi8(n, ner, has)
                    } else {
                        n
                    };
                    nwv[s] = n;
                    let p = predict::<P>(t, w, n, nw, ne, mask);
                    _mm256_and_si256(_mm256_add_epi16(p, d[s]), mask)
                };
                wv[s] = v;
                _mm256_storeu_si256(&mut c_ch[s][x], v);
            }
        }
    }

    /// Reconstructs one row; `res`, `prev`, `cur` are per-slot step-major rows,
    /// `bits` and `near` each slot's coded width and near-lossless bound.
    /// `NEAR`: some bound is nonzero (slots with bound 0 stay modular).
    #[arcane(import_intrinsics)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn recon_row<const NS: usize, const P: u8, const NEAR: bool>(
        t: X64V3Token,
        bits: &[u32],
        near: &[u32],
        color_slots: usize,
        res: &[&[u16]; NS],
        prev: &[&[u16]; NS],
        cur: &mut [&mut [u16]; NS],
        widths: &[u32; NL],
        lw: usize,
        row: usize,
        decor: u8,
    ) {
        let wid: [u16; 16] = core::array::from_fn(|l| widths[l].min(u16::MAX as u32) as u16);
        let widv = _mm256_loadu_si256(&wid);
        if !NEAR && bits[..NS].iter().all(|&b| b == 8) {
            let eff_decor = if color_slots == 3 && NS >= 3 {
                decor
            } else {
                0
            };
            if row == 0 {
                match eff_decor {
                    0 => recon_row_8bit::<NS, P, 0, true>(t, res, prev, cur, widv, lw),
                    1 => recon_row_8bit::<NS, P, 1, true>(t, res, prev, cur, widv, lw),
                    2 => recon_row_8bit::<NS, P, 2, true>(t, res, prev, cur, widv, lw),
                    _ => recon_row_8bit::<NS, P, 3, true>(t, res, prev, cur, widv, lw),
                }
            } else {
                match eff_decor {
                    0 => recon_row_8bit::<NS, P, 0, false>(t, res, prev, cur, widv, lw),
                    1 => recon_row_8bit::<NS, P, 1, false>(t, res, prev, cur, widv, lw),
                    2 => recon_row_8bit::<NS, P, 2, false>(t, res, prev, cur, widv, lw),
                    _ => recon_row_8bit::<NS, P, 3, false>(t, res, prev, cur, widv, lw),
                }
            }
            return;
        }
        let zero = _mm256_setzero_si256();
        let masks: [__m256i; NS] =
            core::array::from_fn(|s| _mm256_set1_epi16(((1u32 << bits[s]) - 1) as u16 as i16));
        let near_on: [bool; NS] = core::array::from_fn(|s| NEAR && near[s] != 0);
        let steps: [__m256i; NS] =
            core::array::from_fn(|s| _mm256_set1_epi16((2 * near[s] + 1) as u16 as i16));
        let shs: [__m128i; NS] = core::array::from_fn(|s| _mm_cvtsi32_si128(16 - bits[s] as i32));
        let cbits = bits[0];
        let sh = _mm_cvtsi32_si128(16 - cbits.max(1) as i32);
        let do_decor = color_slots == 3 && decor != 0 && NS >= 3;
        let r_ch: [&[[u16; NL]]; NS] = core::array::from_fn(|s| &res[s].as_chunks::<NL>().0[..lw]);
        let p_ch: [&[[u16; NL]]; NS] = core::array::from_fn(|s| &prev[s].as_chunks::<NL>().0[..lw]);
        let c_ch: [&mut [[u16; NL]]; NS] = {
            let mut it = cur.iter_mut();
            core::array::from_fn(|_| &mut it.next().unwrap().as_chunks_mut::<NL>().0[..lw])
        };
        let mut wv = [zero; NS];
        let mut nwv = [zero; NS];
        for x in 0..lw {
            let mut d: [__m256i; NS] =
                core::array::from_fn(|s| unzz(t, _mm256_loadu_si256(&r_ch[s][x]), masks[s]));
            if do_decor {
                let m = masks[0];
                let g = d[0];
                d[1] = _mm256_and_si256(_mm256_add_epi16(d[1], g), m);
                d[2] = match decor {
                    1 => _mm256_and_si256(_mm256_add_epi16(d[2], g), m),
                    2 => _mm256_and_si256(_mm256_add_epi16(d[2], d[1]), m),
                    _ => {
                        let a = sext(t, d[1], sh);
                        let b = sext(t, g, sh);
                        let avg = _mm256_add_epi16(
                            _mm256_add_epi16(_mm256_srai_epi16::<1>(a), _mm256_srai_epi16::<1>(b)),
                            _mm256_and_si256(_mm256_and_si256(a, b), _mm256_set1_epi16(1)),
                        );
                        _mm256_and_si256(_mm256_add_epi16(d[2], avg), m)
                    }
                };
            }
            for s in 0..NS {
                let v = if row == 0 {
                    let w = if x > 0 { wv[s] } else { zero };
                    if near_on[s] {
                        near_add(t, w, d[s], shs[s], steps[s], masks[s])
                    } else {
                        _mm256_and_si256(_mm256_add_epi16(w, d[s]), masks[s])
                    }
                } else {
                    let n = _mm256_loadu_si256(&p_ch[s][x]);
                    let (w, nw) = if x > 0 { (wv[s], nwv[s]) } else { (n, n) };
                    let ne = if P == predict::P_AVG_WNE && x + 1 < lw {
                        let ner = _mm256_loadu_si256(&p_ch[s][x + 1]);
                        let has = _mm256_cmpgt_epi16(widv, _mm256_set1_epi16((x + 1) as i16));
                        _mm256_blendv_epi8(n, ner, has)
                    } else {
                        n
                    };
                    nwv[s] = n;
                    let p = predict::<P>(t, w, n, nw, ne, masks[s]);
                    if near_on[s] {
                        near_add(t, p, d[s], shs[s], steps[s], masks[s])
                    } else {
                        _mm256_and_si256(_mm256_add_epi16(p, d[s]), masks[s])
                    }
                };
                wv[s] = v;
                _mm256_storeu_si256(&mut c_ch[s][x], v);
            }
        }
    }

    /// Reconstructs one inter-predicted row (predictors 7..=9), fused with
    /// the residual unzigzag and decorrelation like `recon_row`. `gcur`
    /// and `gprev` are the reference frame's coded values of this row and
    /// the row above, step-major like `res`/`prev`/`cur`. Inter rows are
    /// never near-lossless, and predictors 8 and 9 never occur on a
    /// stripe's first row.
    #[arcane(import_intrinsics)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn recon_row_inter<const NS: usize, const P: u8>(
        t: X64V3Token,
        bits: &[u32],
        color_slots: usize,
        res: &[&[u16]; NS],
        prev: &[&[u16]; NS],
        cur: &mut [&mut [u16]; NS],
        gcur: &[&[u16]; NS],
        gprev: &[&[u16]; NS],
        lw: usize,
        decor: u8,
    ) {
        let masks: [__m256i; NS] =
            core::array::from_fn(|s| _mm256_set1_epi16(((1u32 << bits[s]) - 1) as u16 as i16));
        let cbits = bits[0];
        let sh = _mm_cvtsi32_si128(16 - cbits.max(1) as i32);
        let do_decor = color_slots == 3 && decor != 0 && NS >= 3;
        let mut wv = [_mm256_setzero_si256(); NS];
        let mut gwv = [_mm256_setzero_si256(); NS];
        for x in 0..lw {
            let bi = x * NL;
            let mut d: [__m256i; NS] =
                core::array::from_fn(|s| unzz(t, ld(t, &res[s][bi..]), masks[s]));
            if do_decor {
                let m = masks[0];
                let g = d[0];
                d[1] = _mm256_and_si256(_mm256_add_epi16(d[1], g), m);
                d[2] = match decor {
                    1 => _mm256_and_si256(_mm256_add_epi16(d[2], g), m),
                    2 => _mm256_and_si256(_mm256_add_epi16(d[2], d[1]), m),
                    _ => {
                        // floor((a + b) / 2) without i16 overflow.
                        let a = sext(t, d[1], sh);
                        let b = sext(t, g, sh);
                        let avg = _mm256_add_epi16(
                            _mm256_add_epi16(_mm256_srai_epi16::<1>(a), _mm256_srai_epi16::<1>(b)),
                            _mm256_and_si256(_mm256_and_si256(a, b), _mm256_set1_epi16(1)),
                        );
                        _mm256_and_si256(_mm256_add_epi16(d[2], avg), m)
                    }
                };
            }
            for s in 0..NS {
                let m = masks[s];
                let g = ld(t, &gcur[s][bi..]);
                let v = match P {
                    predict::P_REF => _mm256_add_epi16(g, d[s]),
                    predict::P_REF_N => {
                        // N + (ref - refN) + d
                        let n = ld(t, &prev[s][bi..]);
                        let gn = ld(t, &gprev[s][bi..]);
                        _mm256_add_epi16(_mm256_sub_epi16(_mm256_add_epi16(n, g), gn), d[s])
                    }
                    _ => {
                        // P_REF_W: W + (ref - refW) + d; at each lane's
                        // first column W = N and refW = refN.
                        let w = if x > 0 { wv[s] } else { ld(t, &prev[s][bi..]) };
                        let gw = if x > 0 {
                            gwv[s]
                        } else {
                            ld(t, &gprev[s][bi..])
                        };
                        _mm256_add_epi16(_mm256_sub_epi16(_mm256_add_epi16(w, g), gw), d[s])
                    }
                };
                let v = _mm256_and_si256(v, m);
                wv[s] = v;
                gwv[s] = g;
                st(t, &mut cur[s][bi..], v);
            }
        }
    }
}

/// Order map inverse of the 16-bit SIMD writer's samples (SPEC 2.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub(crate) enum Ord16 {
    /// u16: the value is the sample.
    Plain,
    /// f16: `v ^ ((v >> 15) * 0x7FFF)`.
    Float,
    /// i16: `v ^ 0x8000`.
    Sign,
}

/// Per-channel output transform for the SIMD writer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutKind {
    /// Coded value is the sample.
    Ident,
    /// `v << k`.
    Shl(u32),
    /// 1-bit channel: `v * max`.
    Bit(u16),
    /// Constant sample (no coded slot).
    Const(u16),
    /// Bit replication of an n-bit value to 16 bits, n >= 8.
    Rep16(u32),
    /// `(v * m) >> k` (bit replication of short values; exact only where
    /// the caller verified it).
    MulShr(u16, u32),
    /// `mulhi16(v * mul + add, magic) >> k`: rounded scaling of short values
    /// (exact only where the caller verified it).
    MulHi {
        mul: u16,
        add: u16,
        magic: u16,
        shr: u32,
    },
    /// Value-table map (SPEC 2.1 mode 9): the writer has already looked the
    /// coded value up in the channel's expansion table ([`block_src`]), so
    /// the transform itself is the identity.
    Lut,
}

/// One 16-step block of a slot's step-major coded values, `src` (`n * NL`
/// values for `n <= 16` steps), as the writers load it: through the
/// channel's expansion table `lut` into `buf` when it has one (a power-of-two
/// length covering every coded value), else `src` itself.
#[cfg_attr(
    not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        all(target_arch = "wasm32", target_feature = "simd128")
    )),
    allow(dead_code)
)]
#[inline(always)]
pub(crate) fn block_src<'a>(
    src: &'a [u16],
    lut: &[u16],
    buf: &'a mut [u16; 16 * crate::rans::NL],
) -> &'a [u16] {
    if lut.is_empty() {
        return src;
    }
    let mask = lut.len() - 1;
    for (d, &v) in buf.iter_mut().zip(src) {
        *d = lut[v as usize & mask];
    }
    &buf[..src.len()]
}

impl OutKind {
    /// Scalar value of this transform (for verification).
    pub(crate) fn eval(self, v: u32) -> u32 {
        match self {
            OutKind::Ident => v,
            OutKind::Shl(s) => (v << s) & 0xFFFF,
            OutKind::Bit(m) => (v * m as u32) & 0xFFFF,
            OutKind::Const(c) => c as u32,
            OutKind::Rep16(n) => ((v << (16 - n)) | (v >> (2 * n - 16))) & 0xFFFF,
            OutKind::MulShr(m, k) => ((v * m as u32) & 0xFFFF) >> k,
            OutKind::Lut => v,
            OutKind::MulHi {
                mul,
                add,
                magic,
                shr,
            } => {
                let x = (v * mul as u32 + add as u32) & 0xFFFF;
                ((x * magic as u32) >> 16) >> shr
            }
        }
    }

    /// An exact SIMD transform for `f` over coded values `0..1 << bits`
    /// (8-bit output), or None.
    pub(crate) fn short8(bits: u32, replicate: bool, f: impl Fn(u32) -> u32) -> Option<OutKind> {
        let n = 1u32 << bits;
        let ok = |k: OutKind| (0..n).all(|v| k.eval(v) == f(v));
        if replicate {
            let reps = 8u32.div_ceil(bits);
            let m: u32 = (0..reps).map(|i| 1u32 << (i * bits)).sum();
            let k = OutKind::MulShr(m as u16, reps * bits - 8);
            return (m < 1 << 16 && ok(k)).then_some(k);
        }
        let d = n - 1;
        for shr in 0..16 {
            let magic = (1u64 << (16 + shr)).div_ceil(d as u64);
            if magic >= 1 << 16 {
                break;
            }
            let k = OutKind::MulHi {
                mul: 255,
                add: (d / 2) as u16,
                magic: magic as u16,
                shr,
            };
            if ok(k) {
                return Some(k);
            }
        }
        None
    }
}

/// Byte-shuffle indices interleaving three planes of `esize`-byte samples:
/// for output part `part` (16 bytes) and source channel `ch`.
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
const fn interleave3(esize: usize) -> [[[i8; 16]; 3]; 3] {
    let mut t = [[[-128i8; 16]; 3]; 3];
    let mut part = 0;
    while part < 3 {
        let mut i = 0;
        while i < 16 {
            let o = part * 16 + i;
            let px = o / (3 * esize);
            let rem = o % (3 * esize);
            let ch = rem / esize;
            t[part][ch][i] = (px * esize + rem % esize) as i8;
            i += 1;
        }
        part += 1;
    }
    t
}

#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub(crate) static RGB8_SHUF: [[[i8; 16]; 3]; 3] = interleave3(1);
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub(crate) static RGB16_SHUF: [[[i8; 16]; 3]; 3] = interleave3(2);

#[cfg(target_arch = "x86_64")]
pub(crate) mod out_avx2 {
    use super::{Ord16, OutKind, RGB8_SHUF, RGB16_SHUF};
    use crate::rans::NL;
    use archmage::prelude::*;

    #[rite(import_intrinsics)]
    pub(crate) fn ld(_t: X64V3Token, p: &[u16]) -> __m256i {
        let a: &[u16; 16] = p[..16].try_into().unwrap();
        _mm256_loadu_si256(a)
    }

    /// Transposes 16 step vectors (16 lanes each) into 16 lane vectors.
    #[rite(import_intrinsics)]
    pub(crate) fn transpose16(_t: X64V3Token, r: &[__m256i; 16]) -> [__m256i; 16] {
        let mut d = [_mm256_setzero_si256(); 16];
        for half in 0..2 {
            let a = &r[half * 8..half * 8 + 8];
            let b0 = _mm256_unpacklo_epi16(a[0], a[1]);
            let b1 = _mm256_unpackhi_epi16(a[0], a[1]);
            let b2 = _mm256_unpacklo_epi16(a[2], a[3]);
            let b3 = _mm256_unpackhi_epi16(a[2], a[3]);
            let b4 = _mm256_unpacklo_epi16(a[4], a[5]);
            let b5 = _mm256_unpackhi_epi16(a[4], a[5]);
            let b6 = _mm256_unpacklo_epi16(a[6], a[7]);
            let b7 = _mm256_unpackhi_epi16(a[6], a[7]);
            let c0 = _mm256_unpacklo_epi32(b0, b2);
            let c1 = _mm256_unpackhi_epi32(b0, b2);
            let c2 = _mm256_unpacklo_epi32(b1, b3);
            let c3 = _mm256_unpackhi_epi32(b1, b3);
            let c4 = _mm256_unpacklo_epi32(b4, b6);
            let c5 = _mm256_unpackhi_epi32(b4, b6);
            let c6 = _mm256_unpacklo_epi32(b5, b7);
            let c7 = _mm256_unpackhi_epi32(b5, b7);
            let o = &mut d[half * 8..half * 8 + 8];
            o[0] = _mm256_unpacklo_epi64(c0, c4);
            o[1] = _mm256_unpackhi_epi64(c0, c4);
            o[2] = _mm256_unpacklo_epi64(c1, c5);
            o[3] = _mm256_unpackhi_epi64(c1, c5);
            o[4] = _mm256_unpacklo_epi64(c2, c6);
            o[5] = _mm256_unpackhi_epi64(c2, c6);
            o[6] = _mm256_unpacklo_epi64(c3, c7);
            o[7] = _mm256_unpackhi_epi64(c3, c7);
        }
        // d[j] (rows 0-7) and d[8+j] (rows 8-15): low half = lane j, high = lane 8+j.
        let mut out = [_mm256_setzero_si256(); 16];
        for j in 0..8 {
            out[j] = _mm256_permute2x128_si256::<0x20>(d[j], d[8 + j]);
            out[8 + j] = _mm256_permute2x128_si256::<0x31>(d[j], d[8 + j]);
        }
        out
    }

    #[rite(import_intrinsics)]
    pub(crate) fn apply(_t: X64V3Token, v: __m256i, k: OutKind) -> __m256i {
        match k {
            OutKind::Ident | OutKind::Lut => v,
            OutKind::Shl(s) => _mm256_sll_epi16(v, _mm_cvtsi32_si128(s as i32)),
            OutKind::Bit(m) => _mm256_mullo_epi16(v, _mm256_set1_epi16(m as i16)),
            OutKind::Const(c) => _mm256_set1_epi16(c as i16),
            OutKind::Rep16(n) => _mm256_or_si256(
                _mm256_sll_epi16(v, _mm_cvtsi32_si128(16 - n as i32)),
                _mm256_srl_epi16(v, _mm_cvtsi32_si128(2 * n as i32 - 16)),
            ),
            OutKind::MulShr(m, k) => _mm256_srl_epi16(
                _mm256_mullo_epi16(v, _mm256_set1_epi16(m as i16)),
                _mm_cvtsi32_si128(k as i32),
            ),
            OutKind::MulHi {
                mul,
                add,
                magic,
                shr,
            } => {
                let x = _mm256_add_epi16(
                    _mm256_mullo_epi16(v, _mm256_set1_epi16(mul as i16)),
                    _mm256_set1_epi16(add as i16),
                );
                _mm256_srl_epi16(
                    _mm256_mulhi_epu16(x, _mm256_set1_epi16(magic as i16)),
                    _mm_cvtsi32_si128(shr as i32),
                )
            }
        }
    }

    /// Writes 16 pixels of 16-bit samples (native little-endian).
    #[rite(import_intrinsics)]
    pub(crate) fn write16(
        _t: X64V3Token,
        ch: &[__m256i; 4],
        nch: usize,
        ord: Ord16,
        dst: &mut [u8],
    ) {
        let mut c = *ch;
        match ord {
            Ord16::Plain => {}
            Ord16::Float => {
                // Inverse of the f16 order map: v ^ ((v >> 15) * 0x7FFF).
                for v in c.iter_mut().take(nch) {
                    *v = _mm256_xor_si256(
                        *v,
                        _mm256_and_si256(_mm256_srai_epi16::<15>(*v), _mm256_set1_epi16(0x7FFF)),
                    );
                }
            }
            Ord16::Sign => {
                for v in c.iter_mut().take(nch) {
                    *v = _mm256_xor_si256(*v, _mm256_set1_epi16(i16::MIN));
                }
            }
        }
        let mut out = [0u8; 128];
        match nch {
            1 => _mm256_storeu_si256(<&mut [u8; 32]>::try_from(&mut out[..32]).unwrap(), c[0]),
            2 => {
                let lo = _mm256_unpacklo_epi16(c[0], c[1]);
                let hi = _mm256_unpackhi_epi16(c[0], c[1]);
                // Fix 128-bit lane order: px 0-3 | 8-11 in lo, 4-7 | 12-15 in hi.
                let a = _mm256_permute2x128_si256::<0x20>(lo, hi);
                let b = _mm256_permute2x128_si256::<0x31>(lo, hi);
                _mm256_storeu_si256(<&mut [u8; 32]>::try_from(&mut out[..32]).unwrap(), a);
                _mm256_storeu_si256(<&mut [u8; 32]>::try_from(&mut out[32..64]).unwrap(), b);
            }
            3 => {
                // Per 8-pixel half: 3 x 16 output bytes from r, g, b (u16 each).
                for half in 0..2 {
                    let (r, g, b) = if half == 0 {
                        (
                            _mm256_castsi256_si128(c[0]),
                            _mm256_castsi256_si128(c[1]),
                            _mm256_castsi256_si128(c[2]),
                        )
                    } else {
                        (
                            _mm256_extracti128_si256::<1>(c[0]),
                            _mm256_extracti128_si256::<1>(c[1]),
                            _mm256_extracti128_si256::<1>(c[2]),
                        )
                    };
                    for part in 0..3 {
                        let tb = &RGB16_SHUF[part];
                        let v = _mm_or_si128(
                            _mm_or_si128(
                                _mm_shuffle_epi8(r, _mm_loadu_si128(&tb[0])),
                                _mm_shuffle_epi8(g, _mm_loadu_si128(&tb[1])),
                            ),
                            _mm_shuffle_epi8(b, _mm_loadu_si128(&tb[2])),
                        );
                        let o = half * 48 + part * 16;
                        _mm_storeu_si128(
                            <&mut [u8; 16]>::try_from(&mut out[o..o + 16]).unwrap(),
                            v,
                        );
                    }
                }
            }
            _ => {
                let rg_lo = _mm256_unpacklo_epi16(c[0], c[1]);
                let rg_hi = _mm256_unpackhi_epi16(c[0], c[1]);
                let ba_lo = _mm256_unpacklo_epi16(c[2], c[3]);
                let ba_hi = _mm256_unpackhi_epi16(c[2], c[3]);
                let p0 = _mm256_unpacklo_epi32(rg_lo, ba_lo); // px 0,1 | 8,9
                let p1 = _mm256_unpackhi_epi32(rg_lo, ba_lo); // px 2,3 | 10,11
                let p2 = _mm256_unpacklo_epi32(rg_hi, ba_hi); // px 4,5 | 12,13
                let p3 = _mm256_unpackhi_epi32(rg_hi, ba_hi); // px 6,7 | 14,15
                let parts = [
                    _mm256_permute2x128_si256::<0x20>(p0, p1),
                    _mm256_permute2x128_si256::<0x20>(p2, p3),
                    _mm256_permute2x128_si256::<0x31>(p0, p1),
                    _mm256_permute2x128_si256::<0x31>(p2, p3),
                ];
                for (i, p) in parts.iter().enumerate() {
                    _mm256_storeu_si256(
                        <&mut [u8; 32]>::try_from(&mut out[i * 32..i * 32 + 32]).unwrap(),
                        *p,
                    );
                }
            }
        }
        dst[..32 * nch].copy_from_slice(&out[..32 * nch]);
    }

    /// Reads one row of single-channel 16-bit samples back into step-major
    /// coded refs (the transpose `write_row16` performs in the other
    /// direction), masking to the slot's bits. Every lane must have the full
    /// `lw` samples and `lw` a multiple of 16; callers keep the scalar loop
    /// otherwise. `d` is the slot's `lw * NL` coded refs.
    #[arcane(import_intrinsics)]
    pub(crate) fn ref_row16(t: X64V3Token, row: &[u8], lw: usize, mask: u16, d: &mut [u16]) {
        let mv = _mm256_set1_epi16(mask as i16);
        for x0 in (0..lw).step_by(16) {
            let mut rows = [_mm256_setzero_si256(); NL];
            for (l, r) in rows.iter_mut().enumerate() {
                let a: &[u8; 32] = row[(l * lw + x0) * 2..(l * lw + x0) * 2 + 32]
                    .try_into()
                    .unwrap();
                *r = _mm256_loadu_si256(a);
            }
            let cols = transpose16(t, &rows);
            for (x, c) in cols.iter().enumerate() {
                let b: &mut [u16; 16] = (&mut d[(x0 + x) * NL..(x0 + x) * NL + NL])
                    .try_into()
                    .unwrap();
                _mm256_storeu_si256(b, _mm256_and_si256(*c, mv));
            }
        }
    }

    /// Writes one row of 16-bit (u16, f16 or i16) output.
    #[arcane(import_intrinsics)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn write_row16<const LUT: bool>(
        t: X64V3Token,
        cur: &[&[u16]],
        slot_of: &[Option<usize>],
        kinds: &[OutKind],
        luts: &[alloc::vec::Vec<u16>],
        ord: Ord16,
        widths: &[u32; NL],
        lw: usize,
        out_row: &mut [u8],
    ) {
        let nch = kinds.len();
        let blocks = lw.div_ceil(16);
        let mut tmp = [0u8; 128];
        let mut mapped = [0u16; 16 * NL];
        for bk in 0..blocks {
            let x0 = bk * 16;
            let n = 16.min(lw - x0);
            let mut lanes: [[__m256i; 16]; 4] = [[_mm256_setzero_si256(); 16]; 4];
            for c in 0..nch {
                let Some(s) = slot_of[c] else { continue };
                let mut rows = [_mm256_setzero_si256(); 16];
                // `LUT`: the row has a table channel. Without one this
                // compiles to the plain loads.
                if !LUT || luts[c].is_empty() {
                    for (i, r) in rows.iter_mut().enumerate().take(n) {
                        *r = ld(t, &cur[s][(x0 + i) * NL..]);
                    }
                } else {
                    let src =
                        super::block_src(&cur[s][x0 * NL..(x0 + n) * NL], &luts[c], &mut mapped);
                    for (i, r) in rows.iter_mut().enumerate().take(n) {
                        *r = ld(t, &src[i * NL..]);
                    }
                }
                lanes[c] = transpose16(t, &rows);
            }
            for l in 0..NL {
                let wl = widths[l] as usize;
                if wl <= x0 {
                    continue;
                }
                let cnt = 16.min(wl - x0);
                let ch: [__m256i; 4] = core::array::from_fn(|c| {
                    if c < nch {
                        apply(t, lanes[c][l], kinds[c])
                    } else {
                        _mm256_setzero_si256()
                    }
                });
                let px = l * lw + x0;
                let bpp = 2 * nch;
                if cnt == 16 {
                    write16(
                        t,
                        &ch,
                        nch,
                        ord,
                        &mut out_row[px * bpp..px * bpp + 16 * bpp],
                    );
                } else {
                    write16(t, &ch, nch, ord, &mut tmp[..16 * bpp]);
                    out_row[px * bpp..(px + cnt) * bpp].copy_from_slice(&tmp[..cnt * bpp]);
                }
            }
        }
    }

    /// Packs 16 u16 (values < 256) to 16 bytes in order.
    #[rite(import_intrinsics)]
    fn pack8(_t: X64V3Token, v: __m256i) -> __m128i {
        let p = _mm256_packus_epi16(v, v);
        _mm256_castsi256_si128(_mm256_permute4x64_epi64::<0b1000>(p))
    }

    /// Writes 16 pixels of 8-bit samples for `nch` channels into `dst`
    /// (`16 * nch` bytes).
    #[rite(import_intrinsics)]
    pub(crate) fn write8(t: X64V3Token, ch: &[__m256i; 4], nch: usize, dst: &mut [u8]) {
        match nch {
            1 => {
                let d: &mut [u8; 16] = (&mut dst[..16]).try_into().unwrap();
                _mm_storeu_si128(d, pack8(t, ch[0]));
            }
            2 => {
                let a = pack8(t, ch[0]);
                let b = pack8(t, ch[1]);
                let d0: &mut [u8; 16] = (&mut dst[..16]).try_into().unwrap();
                _mm_storeu_si128(d0, _mm_unpacklo_epi8(a, b));
                let d1: &mut [u8; 16] = (&mut dst[16..32]).try_into().unwrap();
                _mm_storeu_si128(d1, _mm_unpackhi_epi8(a, b));
            }
            3 => {
                let r = pack8(t, ch[0]);
                let g = pack8(t, ch[1]);
                let b = pack8(t, ch[2]);
                // 16 px -> 48 bytes via three 16-byte shuffles of (r, g, b).
                let dst: &mut [u8; 48] = (&mut dst[..48]).try_into().unwrap();
                let (d0, rest) = dst.split_at_mut(16);
                let (d1, d2) = rest.split_at_mut(16);
                for (part, d) in [d0, d1, d2].into_iter().enumerate() {
                    let tb = &RGB8_SHUF[part];
                    let v = _mm_or_si128(
                        _mm_or_si128(
                            _mm_shuffle_epi8(r, _mm_loadu_si128(&tb[0])),
                            _mm_shuffle_epi8(g, _mm_loadu_si128(&tb[1])),
                        ),
                        _mm_shuffle_epi8(b, _mm_loadu_si128(&tb[2])),
                    );
                    let d: &mut [u8; 16] = d.try_into().unwrap();
                    _mm_storeu_si128(d, v);
                }
            }
            _ => {
                let r = pack8(t, ch[0]);
                let g = pack8(t, ch[1]);
                let b = pack8(t, ch[2]);
                let a = pack8(t, ch[3]);
                let rg_lo = _mm_unpacklo_epi8(r, g);
                let rg_hi = _mm_unpackhi_epi8(r, g);
                let ba_lo = _mm_unpacklo_epi8(b, a);
                let ba_hi = _mm_unpackhi_epi8(b, a);
                let parts = [
                    _mm_unpacklo_epi16(rg_lo, ba_lo),
                    _mm_unpackhi_epi16(rg_lo, ba_lo),
                    _mm_unpacklo_epi16(rg_hi, ba_hi),
                    _mm_unpackhi_epi16(rg_hi, ba_hi),
                ];
                for (i, p) in parts.iter().enumerate() {
                    let d: &mut [u8; 16] = (&mut dst[i * 16..i * 16 + 16]).try_into().unwrap();
                    _mm_storeu_si128(d, *p);
                }
            }
        }
    }

    /// Writes one row of 8-bit output. `slot_of[c]` maps container channel
    /// to coded slot; `kinds[c]` its transform; `sign` flips bit 7 (the i8
    /// order map inverse).
    #[arcane(import_intrinsics)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn write_row8<const LUT: bool>(
        t: X64V3Token,
        cur: &[&[u16]],
        slot_of: &[Option<usize>],
        kinds: &[OutKind],
        luts: &[alloc::vec::Vec<u16>],
        sign: bool,
        widths: &[u32; NL],
        lw: usize,
        out_row: &mut [u8],
    ) {
        let nch = kinds.len();
        let blocks = lw.div_ceil(16);
        let mut tmp = [0u8; 64];
        let mut mapped = [0u16; 16 * NL];
        for bk in 0..blocks {
            let x0 = bk * 16;
            let n = 16.min(lw - x0);
            // Gather 16 step vectors per channel (pad with zeros past lw).
            let mut lanes: [[__m256i; 16]; 4] = [[_mm256_setzero_si256(); 16]; 4];
            for c in 0..nch {
                let Some(s) = slot_of[c] else { continue };
                let mut rows = [_mm256_setzero_si256(); 16];
                // `LUT`: the row has a table channel. Without one this
                // compiles to the plain loads.
                if !LUT || luts[c].is_empty() {
                    for (i, r) in rows.iter_mut().enumerate().take(n) {
                        *r = ld(t, &cur[s][(x0 + i) * NL..]);
                    }
                } else {
                    let src =
                        super::block_src(&cur[s][x0 * NL..(x0 + n) * NL], &luts[c], &mut mapped);
                    for (i, r) in rows.iter_mut().enumerate().take(n) {
                        *r = ld(t, &src[i * NL..]);
                    }
                }
                lanes[c] = transpose16(t, &rows);
            }
            for l in 0..NL {
                let wl = widths[l] as usize;
                if wl <= x0 {
                    continue;
                }
                let cnt = 16.min(wl - x0);
                let ch: [__m256i; 4] = core::array::from_fn(|c| {
                    if c >= nch {
                        _mm256_setzero_si256()
                    } else if sign {
                        _mm256_xor_si256(apply(t, lanes[c][l], kinds[c]), _mm256_set1_epi16(0x80))
                    } else {
                        apply(t, lanes[c][l], kinds[c])
                    }
                });
                let px = l * lw + x0;
                if cnt == 16 {
                    write8(t, &ch, nch, &mut out_row[px * nch..px * nch + 16 * nch]);
                } else {
                    write8(t, &ch, nch, &mut tmp[..16 * nch]);
                    out_row[px * nch..(px + cnt) * nch].copy_from_slice(&tmp[..cnt * nch]);
                }
            }
        }
    }
}

#[cfg(all(test, feature = "encode"))]
mod short_tests {
    use super::OutKind;
    use crate::mapping::{ChannelMap, MapMode};

    #[test]
    fn short_maps_have_exact_simd_transforms() {
        for bits in 2u8..8 {
            for mode in [MapMode::Replicate, MapMode::Scale] {
                let m = ChannelMap {
                    mode,
                    bits,
                    value: 0,
                };
                let k =
                    OutKind::short8(bits as u32, mode == MapMode::Replicate, |v| m.expand(v, 8))
                        .unwrap_or_else(|| panic!("{mode:?} {bits}: no SIMD transform"));
                for v in 0..1u32 << bits {
                    assert_eq!(k.eval(v), m.expand(v, 8), "{mode:?} {bits} {v}");
                }
            }
        }
    }
}

/// AVX-512 VBMI 8-bit RGB / RGBA output writer: two steps per 512-bit load,
/// narrowed to bytes, a 16 x 16 byte transpose with `vpermt2b`, and the
/// channel interleave with `vpermt2b`. Same bytes as `out_avx2::write_row8`.
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
pub(crate) mod out_v4x {
    use super::{Ord16, OutKind};
    use crate::rans::NL;
    use archmage::prelude::*;

    /// Transpose sources for lanes 4m..4m+3: output byte l * 16 + s (lane l
    /// of the four, step s). `LO`: steps 0..8 from (steps 0-3 | steps 4-7);
    /// `HI`: steps 8..16 from (steps 8-11 | steps 12-15).
    const fn tr(m: usize, hi: bool) -> [u8; 64] {
        let mut t = [0u8; 64];
        let mut p = 0;
        while p < 64 {
            let (l, s): (usize, usize) = (p / 16, p % 16);
            let s = if hi { s.wrapping_sub(8) } else { s };
            if s < 8 {
                t[p] = ((s / 4) * 64 + (s % 4) * 16 + 4 * m + l) as u8;
            }
            p += 1;
        }
        t
    }
    static TR_LO: [[u8; 64]; 4] = [tr(0, false), tr(1, false), tr(2, false), tr(3, false)];
    static TR_HI: [[u8; 64]; 4] = [tr(0, true), tr(1, true), tr(2, true), tr(3, true)];
    /// Bytes with step >= 8 (taken from the HI permute).
    const HI_MASK: u64 = 0xFF00_FF00_FF00_FF00;

    /// RGB interleave for lane `l` of a transposed group: first R and G
    /// (B slots zero), then B over that.
    const fn rgb1(l: usize) -> [u8; 64] {
        let mut t = [0u8; 64];
        let mut p = 0;
        while p < 48 {
            let (s, c) = (p / 3, p % 3);
            t[p] = match c {
                0 => (l * 16 + s) as u8,
                1 => (64 + l * 16 + s) as u8,
                _ => 0,
            };
            p += 1;
        }
        t
    }
    const fn rgb2(l: usize) -> [u8; 64] {
        let mut t = [0u8; 64];
        let mut p = 0;
        while p < 64 {
            t[p] = if p < 48 && p % 3 == 2 {
                (64 + l * 16 + p / 3) as u8
            } else {
                p as u8
            };
            p += 1;
        }
        t
    }
    static RGB1: [[u8; 64]; 4] = [rgb1(0), rgb1(1), rgb1(2), rgb1(3)];
    static RGB2: [[u8; 64]; 4] = [rgb2(0), rgb2(1), rgb2(2), rgb2(3)];
    /// RGBA: (R, G) into bytes 4s, 4s + 1 and (B, A) into 4s + 2, 4s + 3.
    const fn rgba(l: usize) -> [u8; 64] {
        let mut t = [0u8; 64];
        let mut p = 0;
        while p < 64 {
            let (s, c) = (p / 4, p % 4);
            t[p] = ((c % 2) * 64 + l * 16 + s) as u8;
            p += 1;
        }
        t
    }
    static RGBA: [[u8; 64]; 4] = [rgba(0), rgba(1), rgba(2), rgba(3)];
    const BA_MASK: u64 = 0xCCCC_CCCC_CCCC_CCCC;

    #[rite(import_intrinsics)]
    fn apply(_t: X64V4xToken, v: __m512i, k: OutKind) -> __m512i {
        match k {
            OutKind::Ident | OutKind::Lut => v,
            OutKind::Shl(s) => _mm512_sll_epi16(v, _mm_cvtsi32_si128(s as i32)),
            OutKind::Bit(m) => _mm512_mullo_epi16(v, _mm512_set1_epi16(m as i16)),
            OutKind::Const(c) => _mm512_set1_epi16(c as i16),
            OutKind::Rep16(n) => _mm512_or_si512(
                _mm512_sll_epi16(v, _mm_cvtsi32_si128(16 - n as i32)),
                _mm512_srl_epi16(v, _mm_cvtsi32_si128(2 * n as i32 - 16)),
            ),
            OutKind::MulShr(m, k) => _mm512_srl_epi16(
                _mm512_mullo_epi16(v, _mm512_set1_epi16(m as i16)),
                _mm_cvtsi32_si128(k as i32),
            ),
            OutKind::MulHi {
                mul,
                add,
                magic,
                shr,
            } => {
                let x = _mm512_add_epi16(
                    _mm512_mullo_epi16(v, _mm512_set1_epi16(mul as i16)),
                    _mm512_set1_epi16(add as i16),
                );
                _mm512_srl_epi16(
                    _mm512_mulhi_epu16(x, _mm512_set1_epi16(magic as i16)),
                    _mm_cvtsi32_si128(shr as i32),
                )
            }
        }
    }

    /// Writes one row of 8-bit RGB (`nch` 3) or RGBA (4) output; false for
    /// other channel counts (the caller uses the AVX2 writer).
    #[arcane(import_intrinsics)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn write_row8(
        t: X64V4xToken,
        cur: &[&[u16]],
        slot_of: &[Option<usize>],
        kinds: &[OutKind],
        luts: &[alloc::vec::Vec<u16>],
        sign: bool,
        widths: &[u32; NL],
        lw: usize,
        out_row: &mut [u8],
    ) -> bool {
        let nch = kinds.len();
        if nch != 3 && nch != 4 {
            return false;
        }
        // Channels whose u8 value table fits byte permutes (<= 256 entries):
        // the table packed to bytes, looked up in registers per 64 samples.
        // Built only when a channel has such a table, so rows without one
        // fill nothing.
        let tab8: Option<[[u8; 256]; 4]> = luts[..nch]
            .iter()
            .any(|l| (1..=256).contains(&l.len()))
            .then(|| {
                let mut tab8 = [[0u8; 256]; 4];
                for (c, t) in tab8.iter_mut().enumerate().take(nch) {
                    if (1..=256).contains(&luts[c].len()) {
                        for (d, &v) in t.iter_mut().zip(&luts[c]) {
                            *d = v as u8;
                        }
                    }
                }
                tab8
            });
        let blocks = lw.div_ceil(16);
        let mut pad = [0u16; 16 * NL];
        let mut mapped = [0u16; 16 * NL];
        let mut tmp = [0u8; 64];
        let tr_lo = [
            _mm512_loadu_si512(&TR_LO[0]),
            _mm512_loadu_si512(&TR_LO[1]),
            _mm512_loadu_si512(&TR_LO[2]),
            _mm512_loadu_si512(&TR_LO[3]),
        ];
        let tr_hi = [
            _mm512_loadu_si512(&TR_HI[0]),
            _mm512_loadu_si512(&TR_HI[1]),
            _mm512_loadu_si512(&TR_HI[2]),
            _mm512_loadu_si512(&TR_HI[3]),
        ];
        let rgb1_v = [
            _mm512_loadu_si512(&RGB1[0]),
            _mm512_loadu_si512(&RGB1[1]),
            _mm512_loadu_si512(&RGB1[2]),
            _mm512_loadu_si512(&RGB1[3]),
        ];
        let rgb2_v = [
            _mm512_loadu_si512(&RGB2[0]),
            _mm512_loadu_si512(&RGB2[1]),
            _mm512_loadu_si512(&RGB2[2]),
            _mm512_loadu_si512(&RGB2[3]),
        ];
        let plain_ident = tab8.is_none()
            && !sign
            && luts[..nch].iter().all(|l| l.is_empty())
            && kinds[..nch].iter().all(|k| matches!(k, OutKind::Ident))
            && slot_of[..nch].iter().all(Option::is_some);
        for bk in 0..blocks {
            let x0 = bk * 16;
            let n = 16.min(lw - x0);
            // Per channel: four transposed groups (lanes 4m..4m+3, 16 steps).
            let mut lt = [[_mm512_setzero_si512(); 4]; 4];
            for c in 0..nch {
                let src: &[u16] = match slot_of[c] {
                    Some(s) if n == 16 => &cur[s][x0 * NL..(x0 + 16) * NL],
                    Some(s) => {
                        pad.fill(0);
                        pad[..n * NL].copy_from_slice(&cur[s][x0 * NL..(x0 + n) * NL]);
                        &pad
                    }
                    None => &pad[..0],
                };
                let lut8 = luts[c].len();
                // A[k]: steps 4k..4k+3 as bytes (step * 16 + lane).
                let a: [__m512i; 4] = if plain_ident {
                    core::array::from_fn(|k| {
                        let w0: &[u16; 32] = src[(4 * k) * NL..][..32].try_into().unwrap();
                        let w1: &[u16; 32] = src[(4 * k + 2) * NL..][..32].try_into().unwrap();
                        let b0 = _mm512_cvtepi16_epi8(_mm512_loadu_si512(w0));
                        let b1 = _mm512_cvtepi16_epi8(_mm512_loadu_si512(w1));
                        _mm512_inserti64x4::<1>(_mm512_castsi256_si512(b0), b1)
                    })
                } else if let (Some(tab8), true) =
                    (&tab8, !src.is_empty() && (1..=256).contains(&lut8))
                {
                    let tb = &tab8[c];
                    let t0 = _mm512_loadu_si512(<&[u8; 64]>::try_from(&tb[..64]).unwrap());
                    let t1 = _mm512_loadu_si512(<&[u8; 64]>::try_from(&tb[64..128]).unwrap());
                    let (t2, t3) = if lut8 > 128 {
                        (
                            _mm512_loadu_si512(<&[u8; 64]>::try_from(&tb[128..192]).unwrap()),
                            _mm512_loadu_si512(<&[u8; 64]>::try_from(&tb[192..256]).unwrap()),
                        )
                    } else {
                        (t0, t1)
                    };
                    core::array::from_fn(|k| {
                        let idx = |j: usize| -> __m256i {
                            let w: &[u16; 32] =
                                src[(4 * k + 2 * j) * NL..][..32].try_into().unwrap();
                            _mm512_cvtepi16_epi8(_mm512_loadu_si512(w))
                        };
                        let iz = _mm512_inserti64x4::<1>(_mm512_castsi256_si512(idx(0)), idx(1));
                        let lo = _mm512_permutex2var_epi8(t0, iz, t1);
                        let v = if lut8 > 128 {
                            let hi = _mm512_permutex2var_epi8(t2, iz, t3);
                            _mm512_mask_blend_epi8(_mm512_movepi8_mask(iz), lo, hi)
                        } else {
                            lo
                        };
                        if sign {
                            _mm512_xor_si512(v, _mm512_set1_epi8(i8::MIN))
                        } else {
                            v
                        }
                    })
                } else {
                    let src = super::block_src(src, &luts[c], &mut mapped);
                    core::array::from_fn(|k| {
                        let half = |j: usize| -> __m256i {
                            let v = if src.is_empty() {
                                _mm512_setzero_si512()
                            } else {
                                let w: &[u16; 32] =
                                    src[(4 * k + 2 * j) * NL..][..32].try_into().unwrap();
                                _mm512_loadu_si512(w)
                            };
                            let v = apply(t, v, kinds[c]);
                            let v = if sign {
                                _mm512_xor_si512(v, _mm512_set1_epi16(0x80))
                            } else {
                                v
                            };
                            _mm512_cvtepi16_epi8(v)
                        };
                        _mm512_inserti64x4::<1>(_mm512_castsi256_si512(half(0)), half(1))
                    })
                };
                for m in 0..4 {
                    let lo = _mm512_permutex2var_epi8(a[0], tr_lo[m], a[1]);
                    let hi = _mm512_permutex2var_epi8(a[2], tr_hi[m], a[3]);
                    lt[c][m] = _mm512_mask_blend_epi8(HI_MASK, lo, hi);
                }
            }
            for l in 0..NL {
                let wl = widths[l] as usize;
                if wl <= x0 {
                    continue;
                }
                let cnt = 16.min(wl - x0);
                let (m, li) = (l / 4, l % 4);
                let v = if nch == 3 {
                    let u = _mm512_permutex2var_epi8(lt[0][m], rgb1_v[li], lt[1][m]);
                    _mm512_permutex2var_epi8(u, rgb2_v[li], lt[2][m])
                } else {
                    let idx = _mm512_loadu_si512(&RGBA[li]);
                    let rg = _mm512_permutex2var_epi8(lt[0][m], idx, lt[1][m]);
                    let ba = _mm512_permutex2var_epi8(lt[2][m], idx, lt[3][m]);
                    _mm512_mask_blend_epi8(BA_MASK, rg, ba)
                };
                let px = l * lw + x0;
                let bytes = cnt * nch;
                if cnt == 16 && (nch == 4 || wl >= x0 + 22) {
                    // RGB: the 16 bytes past this lane's 48 belong to its
                    // next block (6+ more pixels), which overwrites them.
                    let o: &mut [u8; 64] =
                        (&mut out_row[px * nch..px * nch + 64]).try_into().unwrap();
                    _mm512_storeu_si512(o, v);
                } else if cnt == 16 {
                    _mm512_storeu_si512(&mut tmp, v);
                    let o: &mut [u8; 48] = (&mut out_row[px * 3..px * 3 + 48]).try_into().unwrap();
                    *o = tmp[..48].try_into().unwrap();
                } else {
                    _mm512_storeu_si512(&mut tmp, v);
                    out_row[px * nch..px * nch + bytes].copy_from_slice(&tmp[..bytes]);
                }
            }
        }
        true
    }

    /// Expands 16 u16 coded values (one lane vector) through the channel's
    /// u32 value table with one masked gather (AVX-512F).
    #[rite(import_intrinsics)]
    fn lut_gather(t4: X64V4Token, lut: &[u32], idx: __m256i) -> __m256i {
        use magetypes::simd::generic::u32x16;
        let i = u32x16::<X64V4Token>::from_raw_t(t4, _mm512_cvtepu16_epi32(idx));
        let g = u32x16::gather_or(lut, i, u32x16::zero_t(t4));
        _mm512_cvtepi32_epi16(g.raw())
    }

    /// Looks 32 u16 coded values (two lane vectors paired into one zmm)
    /// up in a register-resident table of at most 512 entries held in
    /// `K` >= 2 zmm registers: `K/2` `vpermt2w` produce per-64-entry
    /// candidates, then a binary blend tree on the high index bits
    /// selects the right bank. Table lengths are powers of two and coded
    /// values are bounded by the length, so the index bits the permutes
    /// ignore still select correctly through the tree; for len < 32
    /// every index has bit 5 clear and only `t[0]` is read.
    #[rite(import_intrinsics)]
    fn lut_perm_zmm<const K: usize>(t4: X64V4Token, t: &[__m512i; K], idx: __m512i) -> __m512i {
        let _ = t4;
        // Every candidate the blend tree reads is written by a permute
        // first (K/2 banks, then pairwise reduces), so undef is safe.
        let mut cand = [_mm512_undefined_epi32(); 8];
        for j in 0..K / 2 {
            cand[j] = _mm512_permutex2var_epi16(t[2 * j], idx, t[2 * j + 1]);
        }
        let mut n = K / 2;
        let mut bit = 6u32;
        while n > 1 {
            let m = _mm512_test_epi16_mask(idx, _mm512_set1_epi16(1 << bit));
            for j in 0..n / 2 {
                cand[j] = _mm512_mask_blend_epi16(m, cand[2 * j], cand[2 * j + 1]);
            }
            n /= 2;
            bit += 1;
        }
        cand[0]
    }

    /// Expands 16 u16 coded values through the pair-packed table
    /// (`lutp[j] = T[2j] | T[2j + 1] << 16`, half the table bytes):
    /// index `v >> 1` gathers a u32 pair and one `vpermw` picks the
    /// low or high u16 of each pair by the low index bit.
    #[rite(import_intrinsics)]
    fn lut_gather16(t4: X64V4Token, lutp: &[u32], idx: __m256i) -> __m256i {
        use magetypes::simd::generic::u32x16;
        let i = _mm512_cvtepu16_epi32(idx);
        let j = u32x16::<X64V4Token>::from_raw_t(t4, _mm512_srli_epi32::<1>(i));
        let g = u32x16::gather_or(lutp, j, u32x16::zero_t(t4)).raw();
        let sel = _mm256_or_si256(
            _mm256_setr_epi16(0, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22, 24, 26, 28, 30),
            _mm256_and_si256(idx, _mm256_set1_epi16(1)),
        );
        _mm512_castsi512_si256(_mm512_permutexvar_epi16(_mm512_castsi256_si512(sel), g))
    }

    /// Runs `lut_perm_zmm` over all eight lane-vector pairs of a
    /// channel block, writing the expanded values back as ymm lanes.
    #[rite(import_intrinsics)]
    fn perm_pairs<const K: usize>(
        t4: X64V4Token,
        t: &[__m512i; K],
        tr: &[__m256i; 16],
        d: &mut [__m256i; 16],
    ) {
        for (d, p) in d
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(tr.as_chunks::<2>().0.iter())
        {
            let idx = _mm512_inserti64x4::<1>(_mm512_castsi256_si512(p[0]), p[1]);
            let r = lut_perm_zmm::<K>(t4, t, idx);
            d[0] = _mm512_castsi512_si256(r);
            d[1] = _mm512_extracti64x4_epi64::<1>(r);
        }
    }

    /// Whole 1024-entry table in registers: two K=16 trees cover the low
    /// and high halves, and one blend on index bit 9 picks between them —
    /// branchless, no hit test.
    #[rite(import_intrinsics)]
    fn perm_pairs_1k(
        t4: X64V4Token,
        lo: &[__m512i; 16],
        hi: &[__m512i; 16],
        tr: &[__m256i; 16],
        d: &mut [__m256i; 16],
    ) {
        for (d, p) in d
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(tr.as_chunks::<2>().0.iter())
        {
            let idx = _mm512_inserti64x4::<1>(_mm512_castsi256_si512(p[0]), p[1]);
            let lo_r = lut_perm_zmm::<16>(t4, lo, idx);
            let hi_r = lut_perm_zmm::<16>(t4, hi, _mm512_sub_epi16(idx, _mm512_set1_epi16(512)));
            let m = _mm512_test_epi16_mask(idx, _mm512_set1_epi16(512));
            let r = _mm512_mask_blend_epi16(m, lo_r, hi_r);
            d[0] = _mm512_castsi512_si256(r);
            d[1] = _mm512_extracti64x4_epi64::<1>(r);
        }
    }

    /// Writes one row of 16-bit (u16, f16 or i16) output: the bytes of
    /// `out_avx2::write_row16`, but table channels gather their expanded
    /// values per transposed lane vector instead of running the scalar
    /// `block_src` loop. False when no channel has a table (the AVX2
    /// writer then runs identical code).
    #[arcane(import_intrinsics)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn write_row16(
        t: X64V4xToken,
        cur: &[&[u16]],
        slot_of: &[Option<usize>],
        kinds: &[OutKind],
        luts: &[alloc::vec::Vec<u16>],
        luts32: &[Option<alloc::vec::Vec<u32>>],
        lutp: &[alloc::vec::Vec<u32>],
        ord: Ord16,
        widths: &[u32; NL],
        lw: usize,
        out_row: &mut [u8],
    ) -> bool {
        if !kinds.iter().any(|k| matches!(k, OutKind::Lut)) {
            return false;
        }
        let (t3, t4) = (t.v3(), t.v4());
        let nch = kinds.len();
        let blocks = lw.div_ceil(16);
        // Table prefixes are register-resident at zmm width: up to 512
        // entries per channel are built once per row, not per block. For
        // len > 512 this is a hot prefix only — coded values cluster at
        // low indices, so most 32-index groups resolve from registers and
        // the rest fall through to the gather.
        let mut tzs: [[__m512i; 16]; 4] = [[_mm512_undefined_epi32(); 16]; 4];
        let mut tzs_hi: [[__m512i; 16]; 4] = [[_mm512_undefined_epi32(); 16]; 4];
        let mut ks = [0usize; 4];
        for c in 0..nch {
            let len = luts[c].len();
            // Tables >=32768 entries never hit a 512-entry prefix on real
            // index mixes: skip the register build and the two-stage first
            // block; they go straight to the pair-packed gather.
            if len != 0 && len < 32768 {
                let h = len.min(512);
                let k = (h / 32).max(2);
                if h < 64 {
                    // Only the tiny tables need a zero pad: every index has
                    // bit 5 clear so just the first register pair is read.
                    let mut z = [0u16; 64];
                    z[..h].copy_from_slice(&luts[c]);
                    for (j, t) in tzs[c].iter_mut().enumerate().take(2) {
                        let a: &[u16; 32] = z[j * 32..j * 32 + 32].try_into().unwrap();
                        *t = _mm512_loadu_si512(a);
                    }
                } else {
                    for (j, t) in tzs[c].iter_mut().enumerate().take(k) {
                        let a: &[u16; 32] = luts[c][j * 32..j * 32 + 32].try_into().unwrap();
                        *t = _mm512_loadu_si512(a);
                    }
                }
                if len == 1024 {
                    for (j, t) in tzs_hi[c].iter_mut().enumerate() {
                        let a: &[u16; 32] =
                            luts[c][512 + j * 32..512 + j * 32 + 32].try_into().unwrap();
                        *t = _mm512_loadu_si512(a);
                    }
                }
                ks[c] = k;
            }
        }
        let lim256 = _mm512_set1_epi16(256);
        let lim512 = _mm512_set1_epi16(512);
        // Per-row adaptive hot-prefix mode per channel, decided on the
        // first block by counting hits at <256 (K=8 tree, cheap) and <512
        // (K=16): 0=undecided, 1=gather only, 2=hot-256, 3=hot-512,
        // 4=two-stage. Low-hit channels self-disable; the mode that
        // minimizes expected uops wins where indices cluster low.
        const UNDECIDED: u8 = 0;
        const OFF: u8 = 1;
        const H256: u8 = 2;
        const H512: u8 = 3;
        const TWO: u8 = 4;
        let mut hot_mode = [UNDECIDED; 4];
        for c in 0..nch {
            if luts[c].len() >= 32768 {
                hot_mode[c] = OFF;
            }
        }
        let mut tmp = [0u8; 128];
        for bk in 0..blocks {
            let x0 = bk * 16;
            let n = 16.min(lw - x0);
            // `lanes[c]` for a channel without a coded slot feeds
            // `apply(.., OutKind::Const)`, which ignores the input:
            // leaving it `undef` costs no stores.
            let mut lanes: [[__m256i; 16]; 4] = [[_mm256_undefined_si256(); 16]; 4];
            for c in 0..nch {
                let Some(s) = slot_of[c] else { continue };
                // Rows past `n` feed only dead lanes: tail writes cap at
                // `cnt <= n` pixels, gathers mask out-of-range indices and
                // permutes truncate index bits, so undef is safe here too.
                let mut rows = [_mm256_undefined_si256(); 16];
                for (i, r) in rows.iter_mut().enumerate().take(n) {
                    *r = super::out_avx2::ld(t3, &cur[s][(x0 + i) * NL..]);
                }
                let tr = super::out_avx2::transpose16(t3, &rows);
                if luts[c].is_empty() {
                    lanes[c] = tr;
                } else if luts[c].len() <= 512 {
                    // Register-resident table: pair lane vectors into zmm
                    // indices and run the vpermt2w bank tree. len < 32
                    // keeps bit 5 of every index clear, so only t[0] is
                    // read and a single permute resolves the lookup.
                    let tr = &tr;
                    let d = &mut lanes[c];
                    match ks[c] {
                        2 => perm_pairs::<2>(t4, tzs[c][..2].try_into().unwrap(), tr, d),
                        4 => perm_pairs::<4>(t4, tzs[c][..4].try_into().unwrap(), tr, d),
                        8 => perm_pairs::<8>(t4, tzs[c][..8].try_into().unwrap(), tr, d),
                        _ => perm_pairs::<16>(t4, &tzs[c], tr, d),
                    }
                } else if luts[c].len() == 1024 {
                    perm_pairs_1k(t4, &tzs[c], &tzs_hi[c], &tr, &mut lanes[c]);
                } else {
                    // Hot prefix: each 32-index pair whose lanes all fit
                    // the register prefix resolves through vpermt2w trees;
                    // pairs reaching past it gather. The first block runs
                    // the two-stage form and counts hits to pick the mode.
                    let lut = luts32[c].as_deref().unwrap();
                    let lp = &lutp[c][..];
                    let big = luts[c].len() >= 32768;
                    match hot_mode[c] {
                        TWO | UNDECIDED => {
                            let (mut h256, mut h512) = (0usize, 0usize);
                            for (d, p) in lanes[c]
                                .as_chunks_mut::<2>()
                                .0
                                .iter_mut()
                                .zip(tr.as_chunks::<2>().0.iter())
                            {
                                let idx =
                                    _mm512_inserti64x4::<1>(_mm512_castsi256_si512(p[0]), p[1]);
                                if _mm512_cmplt_epu16_mask(idx, lim256) == !0 {
                                    h256 += 1;
                                    let t: &[__m512i; 8] = tzs[c][..8].try_into().unwrap();
                                    let r = lut_perm_zmm::<8>(t4, t, idx);
                                    d[0] = _mm512_castsi512_si256(r);
                                    d[1] = _mm512_extracti64x4_epi64::<1>(r);
                                } else if _mm512_cmplt_epu16_mask(idx, lim512) == !0 {
                                    h512 += 1;
                                    let r = lut_perm_zmm::<16>(t4, &tzs[c], idx);
                                    d[0] = _mm512_castsi512_si256(r);
                                    d[1] = _mm512_extracti64x4_epi64::<1>(r);
                                } else if big {
                                    d[0] = lut_gather16(t4, lp, p[0]);
                                    d[1] = lut_gather16(t4, lp, p[1]);
                                } else {
                                    d[0] = lut_gather(t4, lut, p[0]);
                                    d[1] = lut_gather(t4, lut, p[1]);
                                }
                            }
                            if hot_mode[c] == UNDECIDED {
                                // Expected uops per 8 pairs: test~4,
                                // K=8 tree~10, K=16 tree~18, gather pair~50.
                                // h512 counts the 256..512 band only.
                                let lo = h256 + h512;
                                let c_off = 8 * 50;
                                let c_256 = 8 * 4 + h256 * 10 + (8 - h256) * 50;
                                let c_512 = 8 * 4 + lo * 18 + (8 - lo) * 50;
                                let c_two = 8 * 8 + h256 * 10 + h512 * 18 + (8 - lo) * 50;
                                hot_mode[c] = if c_256 <= c_512 && c_256 <= c_two && c_256 < c_off {
                                    H256
                                } else if c_512 <= c_two && c_512 < c_off {
                                    H512
                                } else if c_two < c_off {
                                    TWO
                                } else {
                                    OFF
                                };
                            }
                        }
                        H256 => {
                            for (d, p) in lanes[c]
                                .as_chunks_mut::<2>()
                                .0
                                .iter_mut()
                                .zip(tr.as_chunks::<2>().0.iter())
                            {
                                let idx =
                                    _mm512_inserti64x4::<1>(_mm512_castsi256_si512(p[0]), p[1]);
                                if _mm512_cmplt_epu16_mask(idx, lim256) == !0 {
                                    let t: &[__m512i; 8] = tzs[c][..8].try_into().unwrap();
                                    let r = lut_perm_zmm::<8>(t4, t, idx);
                                    d[0] = _mm512_castsi512_si256(r);
                                    d[1] = _mm512_extracti64x4_epi64::<1>(r);
                                } else if big {
                                    d[0] = lut_gather16(t4, lp, p[0]);
                                    d[1] = lut_gather16(t4, lp, p[1]);
                                } else {
                                    d[0] = lut_gather(t4, lut, p[0]);
                                    d[1] = lut_gather(t4, lut, p[1]);
                                }
                            }
                        }
                        H512 => {
                            for (d, p) in lanes[c]
                                .as_chunks_mut::<2>()
                                .0
                                .iter_mut()
                                .zip(tr.as_chunks::<2>().0.iter())
                            {
                                let idx =
                                    _mm512_inserti64x4::<1>(_mm512_castsi256_si512(p[0]), p[1]);
                                if _mm512_cmplt_epu16_mask(idx, lim512) == !0 {
                                    let r = lut_perm_zmm::<16>(t4, &tzs[c], idx);
                                    d[0] = _mm512_castsi512_si256(r);
                                    d[1] = _mm512_extracti64x4_epi64::<1>(r);
                                } else if big {
                                    d[0] = lut_gather16(t4, lp, p[0]);
                                    d[1] = lut_gather16(t4, lp, p[1]);
                                } else {
                                    d[0] = lut_gather(t4, lut, p[0]);
                                    d[1] = lut_gather(t4, lut, p[1]);
                                }
                            }
                        }
                        _ if big => {
                            for (d, &v) in lanes[c].iter_mut().zip(&tr) {
                                *d = lut_gather16(t4, lp, v);
                            }
                        }
                        _ => {
                            for (d, &v) in lanes[c].iter_mut().zip(&tr) {
                                *d = lut_gather(t4, lut, v);
                            }
                        }
                    }
                }
            }
            for l in 0..NL {
                let wl = widths[l] as usize;
                if wl <= x0 {
                    continue;
                }
                let cnt = 16.min(wl - x0);
                let ch: [__m256i; 4] = core::array::from_fn(|c| {
                    if c < nch {
                        super::out_avx2::apply(t3, lanes[c][l], kinds[c])
                    } else {
                        _mm256_setzero_si256()
                    }
                });
                let px = l * lw + x0;
                let bpp = 2 * nch;
                if cnt == 16 {
                    super::out_avx2::write16(
                        t3,
                        &ch,
                        nch,
                        ord,
                        &mut out_row[px * bpp..px * bpp + 16 * bpp],
                    );
                } else {
                    super::out_avx2::write16(t3, &ch, nch, ord, &mut tmp[..16 * bpp]);
                    out_row[px * bpp..(px + cnt) * bpp].copy_from_slice(&tmp[..cnt * bpp]);
                }
            }
        }
        true
    }
}

/// wasm128 output writers for 8-bit and 16-bit samples: the same byte output
/// as `out_avx2`, with the per-lane vectors transposed by 8x8 blocks and
/// interleaved with the same byte shuffle tables. magetypes supplies the
/// value operations; the shuffles use safe `core::arch::wasm32` immediate
/// shuffles and swizzles, which magetypes does not expose.
#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
pub(crate) mod out_wasm128 {
    use super::{Ord16, OutKind, RGB8_SHUF, RGB16_SHUF};
    use crate::rans::NL;
    use archmage::prelude::*;
    use core::arch::wasm32::{
        i8x16_swizzle, i16x8_shuffle, i32x4_shuffle, i64x2_shuffle, u8x16_narrow_i16x8, v128,
    };
    use magetypes::simd::{u8x16, u16x16, u32x8};

    /// Byte swizzle through an index vector (indices >= 16 read zero).
    #[rite]
    fn swz(t: Wasm128Token, v: u8x16, idx: v128) -> u8x16 {
        u8x16::from_raw_t(t, i8x16_swizzle(v.raw(), idx))
    }

    /// Interleave the low / high 8 bytes of two vectors.
    #[rite]
    fn ilv8_lo(t: Wasm128Token, a: u8x16, b: u8x16) -> u8x16 {
        u8x16::from_raw_t(
            t,
            i8x16_shuffle::<0, 16, 1, 17, 2, 18, 3, 19, 4, 20, 5, 21, 6, 22, 7, 23>(
                a.raw(),
                b.raw(),
            ),
        )
    }

    #[rite]
    fn ilv8_hi(t: Wasm128Token, a: u8x16, b: u8x16) -> u8x16 {
        u8x16::from_raw_t(
            t,
            i8x16_shuffle::<8, 24, 9, 25, 10, 26, 11, 27, 12, 28, 13, 29, 14, 30, 15, 31>(
                a.raw(),
                b.raw(),
            ),
        )
    }

    /// 8x8 transpose of u16 lanes (one `v128` per row): `out[j][i] = r[i][j]`.
    #[rite]
    #[inline(never)]
    fn t8x8(_t: Wasm128Token, r: [v128; 8]) -> [v128; 8] {
        let p16 = |a: v128, b: v128, hi: bool| {
            if hi {
                i16x8_shuffle::<4, 12, 5, 13, 6, 14, 7, 15>(a, b)
            } else {
                i16x8_shuffle::<0, 8, 1, 9, 2, 10, 3, 11>(a, b)
            }
        };
        let b0 = p16(r[0], r[1], false);
        let b1 = p16(r[0], r[1], true);
        let b2 = p16(r[2], r[3], false);
        let b3 = p16(r[2], r[3], true);
        let b4 = p16(r[4], r[5], false);
        let b5 = p16(r[4], r[5], true);
        let b6 = p16(r[6], r[7], false);
        let b7 = p16(r[6], r[7], true);
        let p32 = |a: v128, b: v128, hi: bool| {
            if hi {
                i32x4_shuffle::<2, 6, 3, 7>(a, b)
            } else {
                i32x4_shuffle::<0, 4, 1, 5>(a, b)
            }
        };
        let c0 = p32(b0, b2, false);
        let c1 = p32(b0, b2, true);
        let c2 = p32(b1, b3, false);
        let c3 = p32(b1, b3, true);
        let c4 = p32(b4, b6, false);
        let c5 = p32(b4, b6, true);
        let c6 = p32(b5, b7, false);
        let c7 = p32(b5, b7, true);
        [
            i64x2_shuffle::<0, 2>(c0, c4),
            i64x2_shuffle::<1, 3>(c0, c4),
            i64x2_shuffle::<0, 2>(c1, c5),
            i64x2_shuffle::<1, 3>(c1, c5),
            i64x2_shuffle::<0, 2>(c2, c6),
            i64x2_shuffle::<1, 3>(c2, c6),
            i64x2_shuffle::<0, 2>(c3, c7),
            i64x2_shuffle::<1, 3>(c3, c7),
        ]
    }

    /// 16x16 byte transpose (`out[l][x] = r[x][l]`): half the stack, registers,
    /// and shuffles of `transpose16` when values already fit in `u8`.
    #[rite]
    #[inline(never)]
    pub(crate) fn transpose16_u8(t: Wasm128Token, r: &[u8x16; 16]) -> [u8x16; 16] {
        let z = u8x16::zero_t(t).raw();
        let mut s1 = [z; 16];
        for i in 0..8 {
            let a = r[2 * i].raw();
            let b = r[2 * i + 1].raw();
            s1[i] = i8x16_shuffle::<0, 16, 1, 17, 2, 18, 3, 19, 4, 20, 5, 21, 6, 22, 7, 23>(a, b);
            s1[8 + i] =
                i8x16_shuffle::<8, 24, 9, 25, 10, 26, 11, 27, 12, 28, 13, 29, 14, 30, 15, 31>(a, b);
        }
        let mut s2 = [z; 16];
        for h in [0usize, 8] {
            for i in 0..4 {
                let a = s1[h + 2 * i];
                let b = s1[h + 2 * i + 1];
                s2[h + i] = i16x8_shuffle::<0, 8, 1, 9, 2, 10, 3, 11>(a, b);
                s2[h + 4 + i] = i16x8_shuffle::<4, 12, 5, 13, 6, 14, 7, 15>(a, b);
            }
        }
        let mut s3 = [z; 16];
        for q in [0usize, 4, 8, 12] {
            for i in 0..2 {
                let a = s2[q + 2 * i];
                let b = s2[q + 2 * i + 1];
                s3[q + i] = i32x4_shuffle::<0, 4, 1, 5>(a, b);
                s3[q + 2 + i] = i32x4_shuffle::<2, 6, 3, 7>(a, b);
            }
        }
        let mut out = [u8x16::zero_t(t); 16];
        for p in (0..16).step_by(2) {
            let a = s3[p];
            let b = s3[p + 1];
            out[p] = u8x16::from_raw_t(t, i64x2_shuffle::<0, 2>(a, b));
            out[p + 1] = u8x16::from_raw_t(t, i64x2_shuffle::<1, 3>(a, b));
        }
        out
    }

    /// Transposes 16 step vectors (16 lanes each) into 16 lane vectors.
    #[rite]
    #[inline(never)]
    pub(crate) fn transpose16(t: Wasm128Token, r: &[u16x16; 16]) -> [u16x16; 16] {
        let z = u8x16::zero_t(t).raw();
        let mut lo = [z; 16];
        let mut hi = [z; 16];
        for (x, v) in r.iter().enumerate() {
            let h: [v128; 2] = v.into_repr();
            lo[x] = h[0];
            hi[x] = h[1];
        }
        let a = t8x8(t, core::array::from_fn(|i| lo[i]));
        let b = t8x8(t, core::array::from_fn(|i| lo[8 + i]));
        let c = t8x8(t, core::array::from_fn(|i| hi[i]));
        let d = t8x8(t, core::array::from_fn(|i| hi[8 + i]));
        core::array::from_fn(|l| {
            if l < 8 {
                u16x16::from_repr_t(t, [a[l], b[l]])
            } else {
                u16x16::from_repr_t(t, [c[l - 8], d[l - 8]])
            }
        })
    }

    /// Applies an output transform to 16 lanes.
    #[rite]
    #[inline(never)]
    fn apply(t: Wasm128Token, v: u16x16, k: OutKind) -> u16x16 {
        match k {
            OutKind::Ident | OutKind::Lut => v,
            OutKind::Shl(s) => v.shl_uniform(s),
            OutKind::Bit(m) => v * u16x16::splat_t(t, m),
            OutKind::Const(c) => u16x16::splat_t(t, c),
            OutKind::Rep16(n) => (v.shl_uniform(16 - n)) | v.shr_logical_uniform(2 * n - 16),
            OutKind::MulShr(m, s) => (v * u16x16::splat_t(t, m)).shr_logical_uniform(s),
            OutKind::MulHi {
                mul,
                add,
                magic,
                shr,
            } => {
                let x = v * u16x16::splat_t(t, mul) + u16x16::splat_t(t, add);
                let m32 = u32x8::splat_t(t, magic as u32);
                let lo = (x.widen_low() * m32).shr_logical_const::<16>();
                let hi = (x.widen_high() * m32).shr_logical_const::<16>();
                lo.bitcast_i32x8()
                    .narrow_saturating_u16(hi.bitcast_i32x8())
                    .shr_logical_uniform(shr)
            }
        }
    }

    /// Inverse of the 16-bit order map on 16 lanes.
    #[rite]
    fn ord16(t: Wasm128Token, v: u16x16, ord: Ord16) -> u16x16 {
        match ord {
            Ord16::Plain => v,
            Ord16::Float => v ^ (v.shr_logical_const::<15>() * u16x16::splat_t(t, 0x7FFF)),
            Ord16::Sign => v ^ u16x16::splat_t(t, 0x8000),
        }
    }

    /// Packs 16 u16 (values < 256) to 16 bytes in order (one WASM instruction).
    #[rite]
    #[inline(always)]
    pub(crate) fn pack8(t: Wasm128Token, v: u16x16) -> u8x16 {
        let h: [v128; 2] = v.into_repr();
        u8x16::from_raw_t(t, u8x16_narrow_i16x8(h[0], h[1]))
    }

    /// Stores 16-byte chunks covering `dst`, zero-filling a partial tail.
    #[rite]
    #[inline(never)]
    pub(crate) fn emit(t: Wasm128Token, chunks: &[v128], dst: &mut [u8]) {
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

    /// Interleaves and stores up to 16 RGB8 pixels from 3 byte vectors.
    #[rite]
    #[inline(always)]
    pub(crate) fn emit_rgb8(
        t: Wasm128Token,
        r: u8x16,
        g: u8x16,
        b: u8x16,
        t8: &[[v128; 3]; 3],
        dst: &mut [u8],
    ) {
        let part = |p: usize| (swz(t, r, t8[p][0]) | swz(t, g, t8[p][1])) | swz(t, b, t8[p][2]);
        emit(t, &[part(0).raw(), part(1).raw(), part(2).raw()], dst);
    }

    /// Interleaves and stores up to 16 RGBA8 pixels from 4 byte vectors.
    #[rite]
    #[inline(always)]
    pub(crate) fn emit_rgba8(
        t: Wasm128Token,
        r: u8x16,
        g: u8x16,
        b: u8x16,
        a: u8x16,
        dst: &mut [u8],
    ) {
        let rg_lo = ilv8_lo(t, r, g);
        let rg_hi = ilv8_hi(t, r, g);
        let ba_lo = ilv8_lo(t, b, a);
        let ba_hi = ilv8_hi(t, b, a);
        let p16 = |a: u8x16, b: u8x16, hi: bool| -> v128 {
            if hi {
                i16x8_shuffle::<4, 12, 5, 13, 6, 14, 7, 15>(a.raw(), b.raw())
            } else {
                i16x8_shuffle::<0, 8, 1, 9, 2, 10, 3, 11>(a.raw(), b.raw())
            }
        };
        emit(
            t,
            &[
                p16(rg_lo, ba_lo, false),
                p16(rg_lo, ba_lo, true),
                p16(rg_hi, ba_hi, false),
                p16(rg_hi, ba_hi, true),
            ],
            dst,
        );
    }

    #[rite]
    pub(crate) fn rgb8_tables(t: Wasm128Token) -> [[v128; 3]; 3] {
        core::array::from_fn(|p| core::array::from_fn(|c| v8(t, RGB8_SHUF[p][c])))
    }

    /// Writes one row of 8-bit output; same bytes as `out_avx2::write_row8`.
    #[arcane]
    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn write_row8(
        t: Wasm128Token,
        cur: &[&[u16]],
        slot_of: &[Option<usize>],
        kinds: &[OutKind],
        luts: &[alloc::vec::Vec<u16>],
        sign: bool,
        widths: &[u32; NL],
        lw: usize,
        out_row: &mut [u8],
    ) {
        let nch = kinds.len();
        let zero = u16x16::zero_t(t);
        let t8 = rgb8_tables(t);
        let plain_ident = !sign
            && luts[..nch].iter().all(|l| l.is_empty())
            && kinds[..nch].iter().all(|k| matches!(k, OutKind::Ident))
            && slot_of[..nch].iter().all(Option::is_some);
        let blocks = lw.div_ceil(16);
        if plain_ident {
            let z8 = u8x16::zero_t(t);
            for bk in 0..blocks {
                let x0 = bk * 16;
                let n = 16.min(lw - x0);
                let mut lanes = [[z8; 16]; 4];
                for c in 0..nch {
                    let s = slot_of[c].unwrap();
                    let mut rows = [z8; 16];
                    let ch = &cur[s].as_chunks::<NL>().0[x0..x0 + n];
                    for (r, src) in rows.iter_mut().zip(ch) {
                        *r = pack8(t, u16x16::load_t(t, src));
                    }
                    lanes[c] = transpose16_u8(t, &rows);
                }
                for l in 0..NL {
                    let wl = widths[l] as usize;
                    if wl <= x0 {
                        continue;
                    }
                    let cnt = 16.min(wl - x0);
                    let px = l * lw + x0;
                    let dst = &mut out_row[px * nch..px * nch + cnt * nch];
                    match nch {
                        1 => emit(t, &[lanes[0][l].raw()], dst),
                        2 => emit(
                            t,
                            &[
                                ilv8_lo(t, lanes[0][l], lanes[1][l]).raw(),
                                ilv8_hi(t, lanes[0][l], lanes[1][l]).raw(),
                            ],
                            dst,
                        ),
                        3 => emit_rgb8(t, lanes[0][l], lanes[1][l], lanes[2][l], &t8, dst),
                        _ => emit_rgba8(t, lanes[0][l], lanes[1][l], lanes[2][l], lanes[3][l], dst),
                    }
                }
            }
            return;
        }
        let mut mapped = [0u16; 16 * NL];
        for bk in 0..blocks {
            let x0 = bk * 16;
            let n = 16.min(lw - x0);
            let mut lanes: [Option<[u16x16; 16]>; 4] = [None, None, None, None];
            for c in 0..nch {
                let Some(s) = slot_of[c] else { continue };
                let mut rows = [zero; 16];
                if luts[c].is_empty() {
                    let ch = &cur[s].as_chunks::<NL>().0[x0..x0 + n];
                    for (r, src) in rows.iter_mut().zip(ch) {
                        *r = u16x16::load_t(t, src);
                    }
                } else {
                    let src =
                        super::block_src(&cur[s][x0 * NL..(x0 + n) * NL], &luts[c], &mut mapped);
                    for (i, r) in rows.iter_mut().enumerate().take(n) {
                        *r = u16x16::from_slice_t(t, &src[i * NL..i * NL + NL]);
                    }
                }
                lanes[c] = Some(transpose16(t, &rows));
            }
            for l in 0..NL {
                let wl = widths[l] as usize;
                if wl <= x0 {
                    continue;
                }
                let cnt = 16.min(wl - x0);
                let b: [u8x16; 4] = core::array::from_fn(|c| {
                    if c >= nch {
                        return u8x16::zero_t(t);
                    }
                    let src = lanes[c].map(|v| v[l]).unwrap_or(zero);
                    let v = apply(t, src, kinds[c]);
                    let v = if sign {
                        v ^ u16x16::splat_t(t, 0x80)
                    } else {
                        v
                    };
                    pack8(t, v)
                });
                let px = l * lw + x0;
                let dst = &mut out_row[px * nch..px * nch + cnt * nch];
                match nch {
                    1 => emit(t, &[b[0].raw()], dst),
                    2 => emit(
                        t,
                        &[ilv8_lo(t, b[0], b[1]).raw(), ilv8_hi(t, b[0], b[1]).raw()],
                        dst,
                    ),
                    3 => emit_rgb8(t, b[0], b[1], b[2], &t8, dst),
                    _ => emit_rgba8(t, b[0], b[1], b[2], b[3], dst),
                }
            }
        }
    }

    #[rite]
    fn v8(t: Wasm128Token, a: [i8; 16]) -> v128 {
        u8x16::from_array_t(t, a.map(|x| x as u8)).raw()
    }

    /// Writes one row of 16-bit (u16, f16 or i16) output; same bytes as
    /// `out_avx2::write_row16`.
    #[arcane]
    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn write_row16(
        t: Wasm128Token,
        cur: &[&[u16]],
        slot_of: &[Option<usize>],
        kinds: &[OutKind],
        luts: &[alloc::vec::Vec<u16>],
        ord: Ord16,
        widths: &[u32; NL],
        lw: usize,
        out_row: &mut [u8],
    ) {
        let nch = kinds.len();
        let zero = u16x16::zero_t(t);
        let t16: [[v128; 3]; 3] =
            core::array::from_fn(|p| core::array::from_fn(|c| v8(t, RGB16_SHUF[p][c])));
        let blocks = lw.div_ceil(16);
        let mut mapped = [0u16; 16 * NL];
        for bk in 0..blocks {
            let x0 = bk * 16;
            let n = 16.min(lw - x0);
            let mut lanes: [Option<[u16x16; 16]>; 4] = [None, None, None, None];
            for c in 0..nch {
                let Some(s) = slot_of[c] else { continue };
                let mut rows = [zero; 16];
                if luts[c].is_empty() {
                    for (i, r) in rows.iter_mut().enumerate().take(n) {
                        *r = u16x16::from_slice_t(t, &cur[s][(x0 + i) * NL..(x0 + i) * NL + NL]);
                    }
                } else {
                    let src =
                        super::block_src(&cur[s][x0 * NL..(x0 + n) * NL], &luts[c], &mut mapped);
                    for (i, r) in rows.iter_mut().enumerate().take(n) {
                        *r = u16x16::from_slice_t(t, &src[i * NL..i * NL + NL]);
                    }
                }
                lanes[c] = Some(transpose16(t, &rows));
            }
            for l in 0..NL {
                let wl = widths[l] as usize;
                if wl <= x0 {
                    continue;
                }
                let cnt = 16.min(wl - x0);
                let ch: [[v128; 2]; 4] = core::array::from_fn(|c| {
                    if c >= nch {
                        return [u8x16::zero_t(t).raw(); 2];
                    }
                    let src = lanes[c].map(|v| v[l]).unwrap_or(zero);
                    ord16(t, apply(t, src, kinds[c]), ord).into_repr()
                });
                let as8 = |x: v128| u8x16::from_raw_t(t, x);
                let ilv = |a: v128, b: v128, hi: bool| -> v128 {
                    if hi {
                        i16x8_shuffle::<4, 12, 5, 13, 6, 14, 7, 15>(a, b)
                    } else {
                        i16x8_shuffle::<0, 8, 1, 9, 2, 10, 3, 11>(a, b)
                    }
                };
                let px = l * lw + x0;
                let dst = &mut out_row[px * 2 * nch..px * 2 * nch + cnt * 2 * nch];
                match nch {
                    1 => emit(t, &[ch[0][0], ch[0][1]], dst),
                    2 => emit(
                        t,
                        &[
                            ilv(ch[0][0], ch[1][0], false),
                            ilv(ch[0][0], ch[1][0], true),
                            ilv(ch[0][1], ch[1][1], false),
                            ilv(ch[0][1], ch[1][1], true),
                        ],
                        dst,
                    ),
                    3 => {
                        let part = |p: usize, h: usize| -> v128 {
                            let x =
                                swz(t, as8(ch[0][h]), t16[p][0]) | swz(t, as8(ch[1][h]), t16[p][1]);
                            (x | swz(t, as8(ch[2][h]), t16[p][2])).raw()
                        };
                        emit(
                            t,
                            &[
                                part(0, 0),
                                part(1, 0),
                                part(2, 0),
                                part(0, 1),
                                part(1, 1),
                                part(2, 1),
                            ],
                            dst,
                        );
                    }
                    _ => {
                        let p32 = |a: v128, b: v128, hi: bool| -> v128 {
                            if hi {
                                i32x4_shuffle::<2, 6, 3, 7>(a, b)
                            } else {
                                i32x4_shuffle::<0, 4, 1, 5>(a, b)
                            }
                        };
                        let half = |h: usize| -> [v128; 4] {
                            let (rg, ba) = ((ch[0][h], ch[1][h]), (ch[2][h], ch[3][h]));
                            let rg_lo = ilv(rg.0, rg.1, false);
                            let rg_hi = ilv(rg.0, rg.1, true);
                            let ba_lo = ilv(ba.0, ba.1, false);
                            let ba_hi = ilv(ba.0, ba.1, true);
                            [
                                p32(rg_lo, ba_lo, false),
                                p32(rg_lo, ba_lo, true),
                                p32(rg_hi, ba_hi, false),
                                p32(rg_hi, ba_hi, true),
                            ]
                        };
                        let a = half(0);
                        let b = half(1);
                        emit(t, &[a[0], a[1], a[2], a[3], b[0], b[1], b[2], b[3]], dst);
                    }
                }
            }
        }
    }
}

/// aarch64 NEON output writer: same bytes as `out_wasm128` (and
/// `out_avx2`), with the 8x8/16x16 transposes and interleaves expressed as
/// NEON `vzip` operations and the byte shuffles through `vqtbl1q`.
#[cfg(target_arch = "aarch64")]
pub(crate) mod out_neon {
    use super::{Ord16, OutKind, RGB8_SHUF, RGB16_SHUF};
    use crate::rans::NL;
    use archmage::prelude::*;
    use core::arch::aarch64::{
        uint8x16_t, uint16x8_t, vcombine_u8, vdupq_n_u16, vqmovun_s16, vqtbl1q_u8,
        vreinterpretq_u8_u16, vreinterpretq_u8_u32, vreinterpretq_u8_u64, vreinterpretq_u16_u8,
        vreinterpretq_u16_u32, vreinterpretq_u32_u8, vreinterpretq_u32_u16, vreinterpretq_u64_u8,
        vreinterpretq_u64_u16, vzip1q_u8, vzip1q_u16, vzip1q_u32, vzip1q_u64, vzip2q_u8,
        vzip2q_u16, vzip2q_u32, vzip2q_u64,
    };
    use magetypes::simd::{u8x16, u16x16, u32x8};

    /// Byte swizzle through an index vector (indices >= 16 read zero).
    #[rite]
    fn swz(t: NeonToken, v: u8x16, idx: uint8x16_t) -> u8x16 {
        u8x16::from_raw_t(t, vqtbl1q_u8(v.raw(), idx))
    }

    /// Interleave the low / high 8 bytes of two vectors.
    #[rite]
    fn ilv8_lo(t: NeonToken, a: u8x16, b: u8x16) -> u8x16 {
        u8x16::from_raw_t(t, vzip1q_u8(a.raw(), b.raw()))
    }

    #[rite]
    fn ilv8_hi(t: NeonToken, a: u8x16, b: u8x16) -> u8x16 {
        u8x16::from_raw_t(t, vzip2q_u8(a.raw(), b.raw()))
    }

    /// 8x8 transpose of u16 lanes (one `uint16x8_t` per row): `out[j][i] = r[i][j]`.
    #[rite]
    fn t8x8(_t: NeonToken, r: [uint16x8_t; 8]) -> [uint16x8_t; 8] {
        let p16 = |a: uint16x8_t, b: uint16x8_t, hi: bool| {
            if hi {
                vzip2q_u16(a, b)
            } else {
                vzip1q_u16(a, b)
            }
        };
        let b0 = p16(r[0], r[1], false);
        let b1 = p16(r[0], r[1], true);
        let b2 = p16(r[2], r[3], false);
        let b3 = p16(r[2], r[3], true);
        let b4 = p16(r[4], r[5], false);
        let b5 = p16(r[4], r[5], true);
        let b6 = p16(r[6], r[7], false);
        let b7 = p16(r[6], r[7], true);
        let p32 = |a: uint16x8_t, b: uint16x8_t, hi: bool| -> uint16x8_t {
            let (a, b) = (vreinterpretq_u32_u16(a), vreinterpretq_u32_u16(b));
            vreinterpretq_u16_u32(if hi {
                vzip2q_u32(a, b)
            } else {
                vzip1q_u32(a, b)
            })
        };
        let c0 = p32(b0, b2, false);
        let c1 = p32(b0, b2, true);
        let c2 = p32(b1, b3, false);
        let c3 = p32(b1, b3, true);
        let c4 = p32(b4, b6, false);
        let c5 = p32(b4, b6, true);
        let c6 = p32(b5, b7, false);
        let c7 = p32(b5, b7, true);
        let p64 = |a: uint16x8_t, b: uint16x8_t, hi: bool| -> uint16x8_t {
            let (a, b) = (vreinterpretq_u64_u16(a), vreinterpretq_u64_u16(b));
            vreinterpretq_u16_u64(if hi {
                vzip2q_u64(a, b)
            } else {
                vzip1q_u64(a, b)
            })
        };
        [
            p64(c0, c4, false),
            p64(c0, c4, true),
            p64(c1, c5, false),
            p64(c1, c5, true),
            p64(c2, c6, false),
            p64(c2, c6, true),
            p64(c3, c7, false),
            p64(c3, c7, true),
        ]
    }

    /// 16x16 byte transpose (`out[l][x] = r[x][l]`): 64 single-cycle NEON ZIP1/ZIP2 ops.
    #[rite]
    pub(crate) fn transpose16_u8(t: NeonToken, r: &[u8x16; 16]) -> [u8x16; 16] {
        let z = u8x16::zero_t(t).raw();
        let mut s1 = [z; 16];
        for i in 0..8 {
            let a = r[2 * i].raw();
            let b = r[2 * i + 1].raw();
            s1[i] = vzip1q_u8(a, b);
            s1[8 + i] = vzip2q_u8(a, b);
        }
        let mut s2 = [z; 16];
        for h in [0usize, 8] {
            for i in 0..4 {
                let a = vreinterpretq_u16_u8(s1[h + 2 * i]);
                let b = vreinterpretq_u16_u8(s1[h + 2 * i + 1]);
                s2[h + i] = vreinterpretq_u8_u16(vzip1q_u16(a, b));
                s2[h + 4 + i] = vreinterpretq_u8_u16(vzip2q_u16(a, b));
            }
        }
        let mut s3 = [z; 16];
        for q in [0usize, 4, 8, 12] {
            for i in 0..2 {
                let a = vreinterpretq_u32_u8(s2[q + 2 * i]);
                let b = vreinterpretq_u32_u8(s2[q + 2 * i + 1]);
                s3[q + i] = vreinterpretq_u8_u32(vzip1q_u32(a, b));
                s3[q + 2 + i] = vreinterpretq_u8_u32(vzip2q_u32(a, b));
            }
        }
        let mut out = [u8x16::zero_t(t); 16];
        for p in (0..16).step_by(2) {
            let a = vreinterpretq_u64_u8(s3[p]);
            let b = vreinterpretq_u64_u8(s3[p + 1]);
            out[p] = u8x16::from_raw_t(t, vreinterpretq_u8_u64(vzip1q_u64(a, b)));
            out[p + 1] = u8x16::from_raw_t(t, vreinterpretq_u8_u64(vzip2q_u64(a, b)));
        }
        out
    }

    /// Transposes 16 step vectors (16 lanes each) into 16 lane vectors.
    #[rite]
    pub(crate) fn transpose16(t: NeonToken, r: &[u16x16; 16]) -> [u16x16; 16] {
        let z = vdupq_n_u16(0);
        let mut lo = [z; 16];
        let mut hi = [z; 16];
        for (x, v) in r.iter().enumerate() {
            let h: [uint16x8_t; 2] = v.into_repr();
            lo[x] = h[0];
            hi[x] = h[1];
        }
        let a = t8x8(t, core::array::from_fn(|i| lo[i]));
        let b = t8x8(t, core::array::from_fn(|i| lo[8 + i]));
        let c = t8x8(t, core::array::from_fn(|i| hi[i]));
        let d = t8x8(t, core::array::from_fn(|i| hi[8 + i]));
        core::array::from_fn(|l| {
            if l < 8 {
                u16x16::from_repr_t(t, [a[l], b[l]])
            } else {
                u16x16::from_repr_t(t, [c[l - 8], d[l - 8]])
            }
        })
    }

    /// Applies an output transform to 16 lanes.
    #[rite]
    fn apply(t: NeonToken, v: u16x16, k: OutKind) -> u16x16 {
        match k {
            OutKind::Ident | OutKind::Lut => v,
            OutKind::Shl(s) => v.shl_uniform(s),
            OutKind::Bit(m) => v * u16x16::splat_t(t, m),
            OutKind::Const(c) => u16x16::splat_t(t, c),
            OutKind::Rep16(n) => (v.shl_uniform(16 - n)) | v.shr_logical_uniform(2 * n - 16),
            OutKind::MulShr(m, s) => (v * u16x16::splat_t(t, m)).shr_logical_uniform(s),
            OutKind::MulHi {
                mul,
                add,
                magic,
                shr,
            } => {
                let x = v * u16x16::splat_t(t, mul) + u16x16::splat_t(t, add);
                let m32 = u32x8::splat_t(t, magic as u32);
                let lo = (x.widen_low() * m32).shr_logical_const::<16>();
                let hi = (x.widen_high() * m32).shr_logical_const::<16>();
                lo.bitcast_i32x8()
                    .narrow_saturating_u16(hi.bitcast_i32x8())
                    .shr_logical_uniform(shr)
            }
        }
    }

    /// Inverse of the 16-bit order map on 16 lanes.
    #[rite]
    fn ord16(t: NeonToken, v: u16x16, ord: Ord16) -> u16x16 {
        match ord {
            Ord16::Plain => v,
            Ord16::Float => v ^ (v.shr_logical_const::<15>() * u16x16::splat_t(t, 0x7FFF)),
            Ord16::Sign => v ^ u16x16::splat_t(t, 0x8000),
        }
    }

    /// Packs 16 signed values (values < 256) to 16 bytes in order.
    #[rite]
    pub(crate) fn pack8(t: NeonToken, v: u16x16) -> u8x16 {
        let h: [core::arch::aarch64::int16x8_t; 2] = v.bitcast_i16x16().into_repr();
        u8x16::from_raw_t(t, vcombine_u8(vqmovun_s16(h[0]), vqmovun_s16(h[1])))
    }

    /// Stores 16-byte chunks covering `dst`, zero-filling a partial tail.
    #[rite]
    pub(crate) fn emit(t: NeonToken, chunks: &[uint8x16_t], dst: &mut [u8]) {
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

    #[rite]
    pub(crate) fn rgb8_tables(t: NeonToken) -> [[uint8x16_t; 3]; 3] {
        core::array::from_fn(|p| core::array::from_fn(|c| v8(t, RGB8_SHUF[p][c])))
    }

    #[rite]
    #[inline(always)]
    pub(crate) fn emit_rgb8(
        t: NeonToken,
        r: u8x16,
        g: u8x16,
        b: u8x16,
        t8: &[[uint8x16_t; 3]; 3],
        dst: &mut [u8],
    ) {
        let part = |p: usize| (swz(t, r, t8[p][0]) | swz(t, g, t8[p][1])) | swz(t, b, t8[p][2]);
        emit(t, &[part(0).raw(), part(1).raw(), part(2).raw()], dst);
    }

    #[rite]
    #[inline(always)]
    pub(crate) fn emit_rgba8(t: NeonToken, r: u8x16, g: u8x16, b: u8x16, a: u8x16, dst: &mut [u8]) {
        let rg_lo = ilv8_lo(t, r, g);
        let rg_hi = ilv8_hi(t, r, g);
        let ba_lo = ilv8_lo(t, b, a);
        let ba_hi = ilv8_hi(t, b, a);
        let p16 = |a: u8x16, b: u8x16, hi: bool| -> uint8x16_t {
            let (a, b) = (vreinterpretq_u16_u8(a.raw()), vreinterpretq_u16_u8(b.raw()));
            let r = if hi {
                vzip2q_u16(a, b)
            } else {
                vzip1q_u16(a, b)
            };
            vreinterpretq_u8_u16(r)
        };
        emit(
            t,
            &[
                p16(rg_lo, ba_lo, false),
                p16(rg_lo, ba_lo, true),
                p16(rg_hi, ba_hi, false),
                p16(rg_hi, ba_hi, true),
            ],
            dst,
        );
    }

    /// Writes one row of 8-bit output; same bytes as `out_avx2::write_row8`.
    #[arcane]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn write_row8(
        t: NeonToken,
        cur: &[&[u16]],
        slot_of: &[Option<usize>],
        kinds: &[OutKind],
        luts: &[alloc::vec::Vec<u16>],
        sign: bool,
        widths: &[u32; NL],
        lw: usize,
        out_row: &mut [u8],
    ) {
        let nch = kinds.len();
        let zero = u16x16::zero_t(t);
        let t8 = rgb8_tables(t);
        let plain_ident = !sign
            && luts[..nch].iter().all(|l| l.is_empty())
            && kinds[..nch].iter().all(|k| matches!(k, OutKind::Ident))
            && slot_of[..nch].iter().all(Option::is_some);
        let blocks = lw.div_ceil(16);
        if plain_ident {
            let z8 = u8x16::zero_t(t);
            for bk in 0..blocks {
                let x0 = bk * 16;
                let n = 16.min(lw - x0);
                let mut lanes = [[z8; 16]; 4];
                for c in 0..nch {
                    let s = slot_of[c].unwrap();
                    let mut rows = [z8; 16];
                    let ch = &cur[s].as_chunks::<NL>().0[x0..x0 + n];
                    for (r, src) in rows.iter_mut().zip(ch) {
                        *r = pack8(t, u16x16::load_t(t, src));
                    }
                    lanes[c] = transpose16_u8(t, &rows);
                }
                for l in 0..NL {
                    let wl = widths[l] as usize;
                    if wl <= x0 {
                        continue;
                    }
                    let cnt = 16.min(wl - x0);
                    let px = l * lw + x0;
                    let dst = &mut out_row[px * nch..px * nch + cnt * nch];
                    match nch {
                        1 => emit(t, &[lanes[0][l].raw()], dst),
                        2 => emit(
                            t,
                            &[
                                ilv8_lo(t, lanes[0][l], lanes[1][l]).raw(),
                                ilv8_hi(t, lanes[0][l], lanes[1][l]).raw(),
                            ],
                            dst,
                        ),
                        3 => emit_rgb8(t, lanes[0][l], lanes[1][l], lanes[2][l], &t8, dst),
                        _ => emit_rgba8(t, lanes[0][l], lanes[1][l], lanes[2][l], lanes[3][l], dst),
                    }
                }
            }
            return;
        }
        let mut mapped = [0u16; 16 * NL];
        for bk in 0..blocks {
            let x0 = bk * 16;
            let n = 16.min(lw - x0);
            let mut lanes: [Option<[u16x16; 16]>; 4] = [None, None, None, None];
            for c in 0..nch {
                let Some(s) = slot_of[c] else { continue };
                let mut rows = [zero; 16];
                if luts[c].is_empty() {
                    let ch = &cur[s].as_chunks::<NL>().0[x0..x0 + n];
                    for (r, src) in rows.iter_mut().zip(ch) {
                        *r = u16x16::load_t(t, src);
                    }
                } else {
                    let src =
                        super::block_src(&cur[s][x0 * NL..(x0 + n) * NL], &luts[c], &mut mapped);
                    for (i, r) in rows.iter_mut().enumerate().take(n) {
                        *r = u16x16::from_slice_t(t, &src[i * NL..i * NL + NL]);
                    }
                }
                lanes[c] = Some(transpose16(t, &rows));
            }
            for l in 0..NL {
                let wl = widths[l] as usize;
                if wl <= x0 {
                    continue;
                }
                let cnt = 16.min(wl - x0);
                let b: [u8x16; 4] = core::array::from_fn(|c| {
                    if c >= nch {
                        return u8x16::zero_t(t);
                    }
                    let src = lanes[c].map(|v| v[l]).unwrap_or(zero);
                    let v = apply(t, src, kinds[c]);
                    let v = if sign {
                        v ^ u16x16::splat_t(t, 0x80)
                    } else {
                        v
                    };
                    pack8(t, v)
                });
                let px = l * lw + x0;
                let dst = &mut out_row[px * nch..px * nch + cnt * nch];
                match nch {
                    1 => emit(t, &[b[0].raw()], dst),
                    2 => emit(
                        t,
                        &[ilv8_lo(t, b[0], b[1]).raw(), ilv8_hi(t, b[0], b[1]).raw()],
                        dst,
                    ),
                    3 => emit_rgb8(t, b[0], b[1], b[2], &t8, dst),
                    _ => emit_rgba8(t, b[0], b[1], b[2], b[3], dst),
                }
            }
        }
    }

    #[rite]
    fn v8(t: NeonToken, a: [i8; 16]) -> uint8x16_t {
        u8x16::from_array_t(t, a.map(|x| x as u8)).raw()
    }

    /// Writes one row of 16-bit (u16, f16 or i16) output; same bytes as
    /// `out_avx2::write_row16`.
    #[arcane]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn write_row16(
        t: NeonToken,
        cur: &[&[u16]],
        slot_of: &[Option<usize>],
        kinds: &[OutKind],
        luts: &[alloc::vec::Vec<u16>],
        ord: Ord16,
        widths: &[u32; NL],
        lw: usize,
        out_row: &mut [u8],
    ) {
        let nch = kinds.len();
        let zero = u16x16::zero_t(t);
        let t16: [[uint8x16_t; 3]; 3] =
            core::array::from_fn(|p| core::array::from_fn(|c| v8(t, RGB16_SHUF[p][c])));
        let blocks = lw.div_ceil(16);
        let mut mapped = [0u16; 16 * NL];
        for bk in 0..blocks {
            let x0 = bk * 16;
            let n = 16.min(lw - x0);
            let mut lanes: [Option<[u16x16; 16]>; 4] = [None, None, None, None];
            for c in 0..nch {
                let Some(s) = slot_of[c] else { continue };
                let mut rows = [zero; 16];
                if luts[c].is_empty() {
                    for (i, r) in rows.iter_mut().enumerate().take(n) {
                        *r = u16x16::from_slice_t(t, &cur[s][(x0 + i) * NL..(x0 + i) * NL + NL]);
                    }
                } else {
                    let src =
                        super::block_src(&cur[s][x0 * NL..(x0 + n) * NL], &luts[c], &mut mapped);
                    for (i, r) in rows.iter_mut().enumerate().take(n) {
                        *r = u16x16::from_slice_t(t, &src[i * NL..i * NL + NL]);
                    }
                }
                lanes[c] = Some(transpose16(t, &rows));
            }
            for l in 0..NL {
                let wl = widths[l] as usize;
                if wl <= x0 {
                    continue;
                }
                let cnt = 16.min(wl - x0);
                let ch: [[uint16x8_t; 2]; 4] = core::array::from_fn(|c| {
                    if c >= nch {
                        return [vdupq_n_u16(0); 2];
                    }
                    let src = lanes[c].map(|v| v[l]).unwrap_or(zero);
                    ord16(t, apply(t, src, kinds[c]), ord).into_repr()
                });
                let as8 = |x: uint16x8_t| u8x16::from_raw_t(t, vreinterpretq_u8_u16(x));
                let ilv = |a: uint16x8_t, b: uint16x8_t, hi: bool| -> uint16x8_t {
                    if hi {
                        vzip2q_u16(a, b)
                    } else {
                        vzip1q_u16(a, b)
                    }
                };
                let px = l * lw + x0;
                let dst = &mut out_row[px * 2 * nch..px * 2 * nch + cnt * 2 * nch];
                match nch {
                    1 => emit(
                        t,
                        &[
                            vreinterpretq_u8_u16(ch[0][0]),
                            vreinterpretq_u8_u16(ch[0][1]),
                        ],
                        dst,
                    ),
                    2 => emit(
                        t,
                        &[
                            vreinterpretq_u8_u16(ilv(ch[0][0], ch[1][0], false)),
                            vreinterpretq_u8_u16(ilv(ch[0][0], ch[1][0], true)),
                            vreinterpretq_u8_u16(ilv(ch[0][1], ch[1][1], false)),
                            vreinterpretq_u8_u16(ilv(ch[0][1], ch[1][1], true)),
                        ],
                        dst,
                    ),
                    3 => {
                        let part = |p: usize, h: usize| -> uint8x16_t {
                            let x =
                                swz(t, as8(ch[0][h]), t16[p][0]) | swz(t, as8(ch[1][h]), t16[p][1]);
                            (x | swz(t, as8(ch[2][h]), t16[p][2])).raw()
                        };
                        emit(
                            t,
                            &[
                                part(0, 0),
                                part(1, 0),
                                part(2, 0),
                                part(0, 1),
                                part(1, 1),
                                part(2, 1),
                            ],
                            dst,
                        );
                    }
                    _ => {
                        let p32 = |a: uint16x8_t, b: uint16x8_t, hi: bool| -> uint16x8_t {
                            let (a, b) = (vreinterpretq_u32_u16(a), vreinterpretq_u32_u16(b));
                            vreinterpretq_u16_u32(if hi {
                                vzip2q_u32(a, b)
                            } else {
                                vzip1q_u32(a, b)
                            })
                        };
                        let half = |h: usize| -> [uint16x8_t; 4] {
                            let (rg, ba) = ((ch[0][h], ch[1][h]), (ch[2][h], ch[3][h]));
                            let rg_lo = ilv(rg.0, rg.1, false);
                            let rg_hi = ilv(rg.0, rg.1, true);
                            let ba_lo = ilv(ba.0, ba.1, false);
                            let ba_hi = ilv(ba.0, ba.1, true);
                            [
                                p32(rg_lo, ba_lo, false),
                                p32(rg_lo, ba_lo, true),
                                p32(rg_hi, ba_hi, false),
                                p32(rg_hi, ba_hi, true),
                            ]
                        };
                        let a = half(0);
                        let b = half(1);
                        emit(
                            t,
                            &[
                                vreinterpretq_u8_u16(a[0]),
                                vreinterpretq_u8_u16(a[1]),
                                vreinterpretq_u8_u16(a[2]),
                                vreinterpretq_u8_u16(a[3]),
                                vreinterpretq_u8_u16(b[0]),
                                vreinterpretq_u8_u16(b[1]),
                                vreinterpretq_u8_u16(b[2]),
                                vreinterpretq_u8_u16(b[3]),
                            ],
                            dst,
                        );
                    }
                }
            }
        }
    }
}
