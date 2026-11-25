//! Per-lane adaptive pixel-domain color transform (`DECOR_ADAPT_PIX = 4`, feature `adaptive_color`).
//!
//! Transforms 8-bit RGB/RGBA pixels per active lane (1..=16) before spatial
//! prediction using reversible modular integer lifting:
//!
//! ```text
//! gs = G - 128
//! R' = (R - ((gr[l] * gs) >> 5)) & 0xFF
//! rs = R - 128
//! B' = (B - ((gb[l] * gs + rb[l] * rs) >> 5)) & 0xFF
//! ```
//!
//! Weights lie in `-127..=127`, so `|gb * gs + rb * rs| <= 32512 <= i16::MAX`
//! and exact signed 16-bit SIMD arithmetic never overflows on any architecture.

use crate::rans::NL;
use crate::{ByteReader, Error};

/// Stripe `decor` byte for per-lane adaptive pixel color transform (SPEC 2.4.1).
pub(crate) const DECOR_ADAPT_PIX: u8 = 4;

/// Maximum magnitude of each adaptive weight (`-127..=127`; `-128` is forbidden).
pub(crate) const WEIGHT_MAX: i16 = 127;

/// Fixed-point right shift (`1 << 5 = 32` represents weight `1.0`).
const SHIFT: i32 = 5;
#[cfg(feature = "encode")]
const SCALE: i32 = 1 << SHIFT;

/// Default predictors for lane 0 of `(gr, gb, rb)` in the weight bitstream.
const LANE0_PRED: [i16; 3] = [32, 16, 16];

/// Per-lane adaptive color weights for one stripe (96 B `Copy`, zero heap allocation).
///
/// Unused lanes `l >= active` are zero. Every active weight lies in `-127..=127`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AdaptWeights {
    pub gr: [i16; NL],
    pub gb: [i16; NL],
    pub rb: [i16; NL],
}

impl AdaptWeights {
    /// Reads `3 * active` weights from the LSB-first bitstream in `r` (SPEC 2.4.1).
    ///
    /// Rejects truncation, out-of-range weights (including `-128`), and any
    /// non-zero trailing padding bits in the final byte as [`Error::Corrupt`].
    pub(crate) fn read_from(r: &mut ByteReader<'_>, active: usize) -> Result<Self, Error> {
        if !(1..=NL).contains(&active) {
            return Err(Error::Corrupt("bad adaptive color active lane count"));
        }
        let mut br = WeightBitReader::new(r);
        let mut gr = [0i16; NL];
        let mut gb = [0i16; NL];
        let mut rb = [0i16; NL];
        let channels: [(&mut [i16; NL], i16); 3] = [
            (&mut gr, LANE0_PRED[0]),
            (&mut gb, LANE0_PRED[1]),
            (&mut rb, LANE0_PRED[2]),
        ];
        for (arr, pred0) in channels {
            for l in 0..active {
                let pred = if l == 0 { pred0 } else { arr[l - 1] };
                let flag = br.get(1)?;
                let w = if flag == 0 {
                    let u = br.get(4)? as i16;
                    let w = pred + (u - 8);
                    if !(-WEIGHT_MAX..=WEIGHT_MAX).contains(&w) {
                        return Err(Error::Corrupt("adaptive color weight out of range"));
                    }
                    w
                } else {
                    let raw = br.get(8)? as u8 as i8 as i16;
                    if raw == -128 {
                        return Err(Error::Corrupt("adaptive color weight out of range"));
                    }
                    raw
                };
                arr[l] = w;
            }
        }
        br.finish_zero_padded()?;
        Ok(Self { gr, gb, rb })
    }

    /// Writes `3 * active` weights to `out` in the canonical LSB-first bitstream.
    #[cfg(any(feature = "encode", test))]
    pub(crate) fn write_to(&self, out: &mut alloc::vec::Vec<u8>, active: usize) {
        debug_assert!((1..=NL).contains(&active));
        let mut bw = WeightBitWriter::new(out);
        let channels: [(&[i16; NL], i16); 3] = [
            (&self.gr, LANE0_PRED[0]),
            (&self.gb, LANE0_PRED[1]),
            (&self.rb, LANE0_PRED[2]),
        ];
        for (arr, pred0) in channels {
            for l in 0..active {
                let pred = if l == 0 { pred0 } else { arr[l - 1] };
                let w = arr[l];
                debug_assert!((-WEIGHT_MAX..=WEIGHT_MAX).contains(&w));
                let d = w - pred;
                if (-8..=7).contains(&d) {
                    bw.put(0, 1);
                    bw.put((d + 8) as u32, 4);
                } else {
                    bw.put(1, 1);
                    bw.put((w as i8 as u8) as u32, 8);
                }
            }
        }
        bw.finish();
    }

    /// Exact encoded byte length of this weight table for `active` lanes.
    #[cfg(any(feature = "encode", test))]
    pub(crate) fn byte_len(&self, active: usize) -> usize {
        debug_assert!((1..=NL).contains(&active));
        let mut bits = 0usize;
        let channels: [(&[i16; NL], i16); 3] = [
            (&self.gr, LANE0_PRED[0]),
            (&self.gb, LANE0_PRED[1]),
            (&self.rb, LANE0_PRED[2]),
        ];
        for (arr, pred0) in channels {
            for l in 0..active {
                let pred = if l == 0 { pred0 } else { arr[l - 1] };
                let d = arr[l] - pred;
                bits += if (-8..=7).contains(&d) { 5 } else { 9 };
            }
        }
        bits.div_ceil(8)
    }
}

struct WeightBitReader<'a, 'b> {
    r: &'a mut ByteReader<'b>,
    buf: u32,
    nbits: u32,
}

impl<'a, 'b> WeightBitReader<'a, 'b> {
    #[inline(always)]
    fn new(r: &'a mut ByteReader<'b>) -> Self {
        Self {
            r,
            buf: 0,
            nbits: 0,
        }
    }

    #[inline(always)]
    fn get(&mut self, n: u32) -> Result<u32, Error> {
        while self.nbits < n {
            self.buf |= (self.r.u8()? as u32) << self.nbits;
            self.nbits += 8;
        }
        let v = self.buf & ((1u32 << n) - 1);
        self.buf >>= n;
        self.nbits -= n;
        Ok(v)
    }

    #[inline(always)]
    fn finish_zero_padded(self) -> Result<(), Error> {
        if self.nbits > 0 && self.buf != 0 {
            return Err(Error::Corrupt("non-zero adaptive color padding bits"));
        }
        Ok(())
    }
}

#[cfg(any(feature = "encode", test))]
struct WeightBitWriter<'a> {
    out: &'a mut alloc::vec::Vec<u8>,
    buf: u32,
    nbits: u32,
}

#[cfg(any(feature = "encode", test))]
impl<'a> WeightBitWriter<'a> {
    #[inline(always)]
    fn new(out: &'a mut alloc::vec::Vec<u8>) -> Self {
        Self {
            out,
            buf: 0,
            nbits: 0,
        }
    }

    #[inline(always)]
    fn put(&mut self, val: u32, n: u32) {
        self.buf |= (val & ((1u32 << n) - 1)) << self.nbits;
        self.nbits += n;
        while self.nbits >= 8 {
            self.out.push(self.buf as u8);
            self.buf >>= 8;
            self.nbits -= 8;
        }
    }

    #[inline(always)]
    fn finish(self) {
        if self.nbits > 0 {
            self.out.push(self.buf as u8);
        }
    }
}

/// Exact scalar pixel-domain inverse on one transformed `(G, R', B')` triple for `lane`.
///
/// Returns `(G, R, B)` in `0..=255`.
#[inline(always)]
pub(crate) fn inverse_pixel_triple(
    aw: &AdaptWeights,
    lane: usize,
    g: u32,
    r_in: u32,
    b_in: u32,
) -> (u32, u32, u32) {
    let gs = g as i32 - 128;
    let r = r_in.wrapping_add(((aw.gr[lane] as i32 * gs) >> SHIFT) as u32) & 0xFF;
    let rs = r as i32 - 128;
    let b = b_in
        .wrapping_add(((aw.gb[lane] as i32 * gs + aw.rb[lane] as i32 * rs) >> SHIFT) as u32)
        & 0xFF;
    (g, r, b)
}

// ---- Encoder: deterministic integer OLS + transformed-MED direct refinement ----

/// Bit-length cost LUT for 8-bit modular residuals `0..256` (exact match to `encode::cost_sum`).
#[cfg(feature = "encode")]
const COST8: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut d = 0usize;
    while d < 256 {
        let sign = (d >> 7) & 1;
        let z = ((d << 1) ^ 0usize.wrapping_sub(sign)) & 0xFF;
        t[d] = if z == 0 {
            0
        } else {
            (usize::BITS - z.leading_zeros()) as u8
        };
        d += 1;
    }
    t
};

/// Rounded signed integer division clamped to `[-WEIGHT_MAX, WEIGHT_MAX]`.
/// Pure integer (`i128`) so encoder output is bit-identical across every ISA and `no_std`.
#[cfg(feature = "encode")]
#[inline(always)]
fn round_div_clamp(num: i128, den: i128) -> i32 {
    debug_assert!(den > 0);
    let half = den / 2;
    let q = if num >= 0 {
        (num + half) / den
    } else {
        (num - half) / den
    };
    q.clamp(-(WEIGHT_MAX as i128), WEIGHT_MAX as i128) as i32
}

/// Fast OLS + coordinate bit-cost seed on signed 8-bit MED residual triples.
#[cfg(feature = "encode")]
fn fit_lane_proxy(gs_s: &[i8], rs_s: &[i8], bs_s: &[i8]) -> (i16, i16, i16) {
    let n = gs_s.len();
    if n == 0 {
        return (0, 0, 0);
    }
    let (mut s_gg, mut s_gr, mut s_gb, mut s_rr, mut s_rb) = (0i64, 0i64, 0i64, 0i64, 0i64);
    for i in 0..n {
        let g = gs_s[i] as i64;
        let r = rs_s[i] as i64;
        let b = bs_s[i] as i64;
        s_gg += g * g;
        s_gr += g * r;
        s_gb += g * b;
        s_rr += r * r;
        s_rb += r * b;
    }
    const RIDGE: i128 = 16;
    let a = s_gg as i128 + RIDGE;
    let b = s_gr as i128;
    let c = s_rr as i128 + RIDGE;
    let gr0 = if s_gg > 0 {
        round_div_clamp((s_gr as i128) << SHIFT, a)
    } else {
        0
    };
    let det = a * c - b * b;
    let (gb0, rb0) = if det > 1 {
        let num_gb = (c * (s_gb as i128) - b * (s_rb as i128)) << SHIFT;
        let num_rb = (a * (s_rb as i128) - b * (s_gb as i128)) << SHIFT;
        (round_div_clamp(num_gb, det), round_div_clamp(num_rb, det))
    } else if s_gg > 0 {
        (round_div_clamp((s_gb as i128) << SHIFT, a), 0)
    } else {
        (0, 0)
    };

    let wmax = WEIGHT_MAX as i32;
    let eval_r = |gr: i32| -> u64 {
        let mut cost = 0u64;
        for i in 0..n {
            let pr = (gr * gs_s[i] as i32) >> SHIFT;
            let d = (rs_s[i] as u8 as i32).wrapping_sub(pr) as u8 as usize;
            cost += COST8[d] as u64;
        }
        cost
    };
    let eval_b = |gb: i32, rb: i32| -> u64 {
        let mut cost = 0u64;
        for i in 0..n {
            let pb = (gb * gs_s[i] as i32 + rb * rs_s[i] as i32) >> SHIFT;
            let d = (bs_s[i] as u8 as i32).wrapping_sub(pb) as u8 as usize;
            cost += COST8[d] as u64;
        }
        cost
    };

    let mut best_gr = gr0;
    let mut best_rc = eval_r(best_gr);
    for cand in [0, SCALE] {
        if cand != best_gr {
            let c = eval_r(cand);
            if c < best_rc {
                best_rc = c;
                best_gr = cand;
            }
        }
    }
    for step in [8, 4, 2, 1] {
        loop {
            let mut improved = false;
            for delta in [-step, step] {
                let cand = (best_gr + delta).clamp(-wmax, wmax);
                if cand != best_gr {
                    let c = eval_r(cand);
                    if c < best_rc {
                        best_rc = c;
                        best_gr = cand;
                        improved = true;
                    }
                }
            }
            if !improved {
                break;
            }
        }
    }

    let mut best_gb = gb0;
    let mut best_rb = rb0;
    let mut best_bc = eval_b(best_gb, best_rb);
    for &(cg, cr) in &[(0, 0), (SCALE, 0), (0, SCALE), (SCALE / 2, SCALE / 2)] {
        if cg != best_gb || cr != best_rb {
            let c = eval_b(cg, cr);
            if c < best_bc {
                best_bc = c;
                best_gb = cg;
                best_rb = cr;
            }
        }
    }
    for step in [8, 4, 2, 1] {
        loop {
            let mut improved = false;
            let (bg, br) = (best_gb, best_rb);
            for &(dg, dr) in &[
                (-step, 0),
                (step, 0),
                (0, -step),
                (0, step),
                (-step, -step),
                (step, step),
                (-step, step),
                (step, -step),
            ] {
                let cg = (bg + dg).clamp(-wmax, wmax);
                let cr = (br + dr).clamp(-wmax, wmax);
                if cg != best_gb || cr != best_rb {
                    let c = eval_b(cg, cr);
                    if c < best_bc {
                        best_bc = c;
                        best_gb = cg;
                        best_rb = cr;
                        improved = true;
                    }
                }
            }
            if !improved {
                break;
            }
        }
    }
    (best_gr as i16, best_gb as i16, best_rb as i16)
}

/// Scores MED residual bit-length cost on a single lane's transformed 8-bit plane (`bh * bw`).
#[cfg(feature = "encode")]
#[inline]
fn score_tx_med_cost8(tx: &[u8], bh: usize, bw: usize) -> u64 {
    let mut cost = 0u64;
    // Row 0: x = 0 predicts 0, x > 0 predicts west.
    let mut pw = 0u32;
    for &b in &tx[..bw] {
        let cur = b as u32;
        cost += COST8[cur.wrapping_sub(pw) as u8 as usize] as u64;
        pw = cur;
    }
    // Rows 1..bh: x = 0 predicts north, x = 1..bw predicts MED(w, n, nw).
    for r in 1..bh {
        let prev = &tx[(r - 1) * bw..r * bw];
        let cur_row = &tx[r * bw..(r + 1) * bw];
        let mut pw = cur_row[0] as u32;
        let mut pnw = prev[0] as u32;
        cost += COST8[pw.wrapping_sub(pnw) as u8 as usize] as u64;
        for x in 1..bw {
            let cur = cur_row[x] as u32;
            let pn = prev[x] as u32;
            let lo = pw.min(pn);
            let hi = pw.max(pn);
            let pred = if pnw >= hi {
                lo
            } else if pnw <= lo {
                hi
            } else {
                pw + pn - pnw
            };
            cost += COST8[cur.wrapping_sub(pred) as u8 as usize] as u64;
            pw = cur;
            pnw = pn;
        }
    }
    cost
}

/// Refines `(gr, gb, rb)` directly on transformed 2D lane MED residuals,
/// reusing caller-owned scratch buffers across lanes to avoid per-lane allocations.
#[cfg(feature = "encode")]
fn refine_lane_pixel_direct(
    gs_c: &[i16],
    r_blk: &[u8],
    rs_c: &[i16],
    b_blk: &[u8],
    bh: usize,
    bw: usize,
    init: (i16, i16, i16),
    tx: &mut [u8],
) -> (i16, i16, i16) {
    let n = bh * bw;
    if n == 0 {
        return init;
    }
    let wmax = WEIGHT_MAX as i32;

    let mut eval_r = |gr: i32| -> u64 {
        let gr = gr as i16;
        for i in 0..n {
            let pr = (gr.wrapping_mul(gs_c[i])) >> SHIFT;
            tx[i] = r_blk[i].wrapping_sub(pr as u8);
        }
        score_tx_med_cost8(&tx[..n], bh, bw)
    };
    let mut best_gr = init.0 as i32;
    let mut best_rc = eval_r(best_gr);
    for cand in [0, SCALE] {
        if cand != best_gr {
            let c = eval_r(cand);
            if c < best_rc {
                best_rc = c;
                best_gr = cand;
            }
        }
    }
    for step in [4, 2, 1] {
        loop {
            let mut improved = false;
            for delta in [-step, step] {
                let cand = (best_gr + delta).clamp(-wmax, wmax);
                if cand != best_gr {
                    let c = eval_r(cand);
                    if c < best_rc {
                        best_rc = c;
                        best_gr = cand;
                        improved = true;
                    }
                }
            }
            if !improved {
                break;
            }
        }
    }
    drop(eval_r);

    let mut eval_b = |gb: i32, rb: i32| -> u64 {
        let gb = gb as i16;
        let rb = rb as i16;
        for i in 0..n {
            let pb = (gb
                .wrapping_mul(gs_c[i])
                .wrapping_add(rb.wrapping_mul(rs_c[i])))
                >> SHIFT;
            tx[i] = b_blk[i].wrapping_sub(pb as u8);
        }
        score_tx_med_cost8(&tx[..n], bh, bw)
    };
    let mut best_gb = init.1 as i32;
    let mut best_rb = init.2 as i32;
    let mut best_bc = eval_b(best_gb, best_rb);
    for &(cg, cr) in &[(0, 0), (SCALE, 0), (0, SCALE), (SCALE / 2, SCALE / 2)] {
        if cg != best_gb || cr != best_rb {
            let c = eval_b(cg, cr);
            if c < best_bc {
                best_bc = c;
                best_gb = cg;
                best_rb = cr;
            }
        }
    }
    for step in [4, 2, 1] {
        loop {
            let mut improved = false;
            let (bg, br) = (best_gb, best_rb);
            for &(dg, dr) in &[
                (-step, 0),
                (step, 0),
                (0, -step),
                (0, step),
                (-step, -step),
                (step, step),
                (-step, step),
                (step, -step),
            ] {
                let cg = (bg + dg).clamp(-wmax, wmax);
                let cr = (br + dr).clamp(-wmax, wmax);
                if cg != best_gb || cr != best_rb {
                    let c = eval_b(cg, cr);
                    if c < best_bc {
                        best_bc = c;
                        best_gb = cg;
                        best_rb = cr;
                        improved = true;
                    }
                }
            }
            if !improved {
                break;
            }
        }
    }
    (best_gr as i16, best_gb as i16, best_rb as i16)
}

/// Allocates a `Vec<T>` with fallible `try_reserve_exact`, returning `Error::TooLarge` on OOM.
#[cfg(feature = "encode")]
fn try_vec_with_capacity<T>(cap: usize) -> Result<alloc::vec::Vec<T>, Error> {
    let mut v = alloc::vec::Vec::new();
    v.try_reserve_exact(cap).map_err(|_| Error::TooLarge)?;
    Ok(v)
}

/// Allocates a zero-initialized `Vec<T>` fallibly, returning `Error::TooLarge` on OOM.
#[cfg(feature = "encode")]
fn try_zeroed_vec<T: Clone + Default>(len: usize) -> Result<alloc::vec::Vec<T>, Error> {
    let mut v = try_vec_with_capacity(len)?;
    v.resize(len, T::default());
    Ok(v)
}

/// Fits per-lane weights for one stripe (`y0..y1`, `planes` in slot order `[G, R, B, ...]`)
/// and returns `(AdaptWeights, [r_tx, b_tx])` holding only the two transformed color planes
/// (slots 0 and 3+ are unchanged and can be borrowed directly from `planes`).
#[cfg(feature = "encode")]
pub(crate) fn fit_and_transform_stripe_pixel(
    planes: &[&[u32]],
    w: usize,
    sw: usize,
    y0: usize,
    y1: usize,
) -> Result<(AdaptWeights, [alloc::vec::Vec<u32>; 2]), Error> {
    let rows = y1 - y0;
    let mut gr = [0i16; NL];
    let mut gb = [0i16; NL];
    let mut rb = [0i16; NL];

    let max_lane_px = rows.checked_mul(sw.min(w)).ok_or(Error::TooLarge)?;
    let mut gs_buf = try_vec_with_capacity(max_lane_px)?;
    let mut rs_buf = try_vec_with_capacity(max_lane_px)?;
    let mut bs_buf = try_vec_with_capacity(max_lane_px)?;
    let mut gs_c = try_vec_with_capacity(max_lane_px)?;
    let mut rs_c = try_vec_with_capacity(max_lane_px)?;
    let mut r_blk = try_vec_with_capacity(max_lane_px)?;
    let mut b_blk = try_vec_with_capacity(max_lane_px)?;
    let mut tx = try_zeroed_vec::<u8>(max_lane_px)?;

    for l in 0..NL {
        let xs = l * sw;
        if xs >= w {
            break;
        }
        let xe = (xs + sw).min(w);
        let bw = xe - xs;
        gs_buf.clear();
        rs_buf.clear();
        bs_buf.clear();
        gs_c.clear();
        rs_c.clear();
        r_blk.clear();
        b_blk.clear();
        for r in 0..rows {
            let y = y0 + r;
            let off = y * w;
            for x in xs..xe {
                let idx = off + x;
                let g0 = planes[0][idx];
                let r0 = planes[1][idx];
                let b0 = planes[2][idx];
                gs_c.push(g0 as i16 - 128);
                rs_c.push(r0 as i16 - 128);
                r_blk.push(r0 as u8);
                b_blk.push(b0 as u8);

                let pred = |ch: usize| -> u32 {
                    if r == 0 {
                        if x > xs { planes[ch][idx - 1] } else { 0 }
                    } else if x == xs {
                        planes[ch][idx - w]
                    } else {
                        let pw = planes[ch][idx - 1];
                        let pn = planes[ch][idx - w];
                        let pnw = planes[ch][idx - w - 1];
                        let lo = pw.min(pn);
                        let hi = pw.max(pn);
                        if pnw >= hi {
                            lo
                        } else if pnw <= lo {
                            hi
                        } else {
                            pw + pn - pnw
                        }
                    }
                };
                gs_buf.push((g0.wrapping_sub(pred(0)) & 0xFF) as u8 as i8);
                rs_buf.push((r0.wrapping_sub(pred(1)) & 0xFF) as u8 as i8);
                bs_buf.push((b0.wrapping_sub(pred(2)) & 0xFF) as u8 as i8);
            }
        }
        let init = fit_lane_proxy(&gs_buf, &rs_buf, &bs_buf);
        let (wgr, wgb, wrb) =
            refine_lane_pixel_direct(&gs_c, &r_blk, &rs_c, &b_blk, rows, bw, init, &mut tx);
        gr[l] = wgr;
        gb[l] = wgb;
        rb[l] = wrb;
    }

    let aw = AdaptWeights { gr, gb, rb };
    let stripe_px = rows.checked_mul(w).ok_or(Error::TooLarge)?;
    let mut r_tx = try_zeroed_vec::<u32>(stripe_px)?;
    let mut b_tx = try_zeroed_vec::<u32>(stripe_px)?;

    for r in 0..rows {
        let src_off = (y0 + r) * w;
        let dst_off = r * w;
        for l in 0..NL {
            let xs = l * sw;
            if xs >= w {
                break;
            }
            let xe = (xs + sw).min(w);
            let wgr = aw.gr[l] as i32;
            let wgb = aw.gb[l] as i32;
            let wrb = aw.rb[l] as i32;
            for x in xs..xe {
                let g = planes[0][src_off + x];
                let rv = planes[1][src_off + x];
                let bv = planes[2][src_off + x];
                let gs = g as i32 - 128;
                let rs = rv as i32 - 128;
                r_tx[dst_off + x] = rv.wrapping_sub(((wgr * gs) >> SHIFT) as u32) & 0xFF;
                b_tx[dst_off + x] = bv.wrapping_sub(((wgb * gs + wrb * rs) >> SHIFT) as u32) & 0xFF;
            }
        }
    }
    Ok((aw, [r_tx, b_tx]))
}

// ---- Fused 4-ISA zero-allocation SIMD output writers (plain identity U8 RGB / RGBA) ----

#[cfg(target_arch = "x86_64")]
pub(crate) mod avx2 {
    use super::AdaptWeights;
    use crate::rans::NL;
    use archmage::prelude::*;

    #[arcane(import_intrinsics)]
    fn write_row8_pix_adapt_kernel<const NCH: usize>(
        t: X64V3Token,
        cur: &[&[u16]],
        alpha_const: Option<u8>,
        widths: &[u32; NL],
        lw: usize,
        aw: &AdaptWeights,
        out_row: &mut [u8],
    ) {
        let zero = _mm256_setzero_si256();
        let c128 = _mm256_set1_epi16(128);
        let mask_ff = _mm256_set1_epi16(0x00FF);
        let g_ch = &cur[0].as_chunks::<NL>().0[..lw];
        let r_ch = &cur[1].as_chunks::<NL>().0[..lw];
        let b_ch = &cur[2].as_chunks::<NL>().0[..lw];
        let a_ch = if NCH == 4 && alpha_const.is_none() {
            Some(&cur[3].as_chunks::<NL>().0[..lw])
        } else {
            None
        };
        let a_const_v = _mm256_set1_epi16(alpha_const.unwrap_or(0) as i16);

        let v_gr = _mm256_loadu_si256(&aw.gr);
        let v_gb = _mm256_loadu_si256(&aw.gb);
        let v_rb = _mm256_loadu_si256(&aw.rb);

        let blocks = lw.div_ceil(16);
        let mut tmp = [0u8; 64];
        for bk in 0..blocks {
            let x0 = bk * 16;
            let n = 16.min(lw - x0);
            let mut r_rows = [zero; 16];
            let mut g_rows = [zero; 16];
            let mut b_rows = [zero; 16];
            let mut a_rows = [zero; 16];

            for i in 0..n {
                let x = x0 + i;
                let gv = _mm256_loadu_si256(&g_ch[x]);
                let ri = _mm256_loadu_si256(&r_ch[x]);
                let bi = _mm256_loadu_si256(&b_ch[x]);

                let gs = _mm256_sub_epi16(gv, c128);
                let pr = _mm256_srai_epi16::<5>(_mm256_mullo_epi16(v_gr, gs));
                let rv = _mm256_and_si256(_mm256_add_epi16(ri, pr), mask_ff);
                let rs = _mm256_sub_epi16(rv, c128);
                let sum_b =
                    _mm256_add_epi16(_mm256_mullo_epi16(v_gb, gs), _mm256_mullo_epi16(v_rb, rs));
                let pb = _mm256_srai_epi16::<5>(sum_b);
                let bv = _mm256_and_si256(_mm256_add_epi16(bi, pb), mask_ff);

                r_rows[i] = rv;
                g_rows[i] = gv;
                b_rows[i] = bv;
                if let Some(ac) = a_ch {
                    a_rows[i] = _mm256_loadu_si256(&ac[x]);
                }
            }

            let lanes_r = crate::recon::out_avx2::transpose16(t, &r_rows);
            let lanes_g = crate::recon::out_avx2::transpose16(t, &g_rows);
            let lanes_b = crate::recon::out_avx2::transpose16(t, &b_rows);
            let lanes_a = if a_ch.is_some() {
                crate::recon::out_avx2::transpose16(t, &a_rows)
            } else {
                [a_const_v; 16]
            };

            for l in 0..NL {
                let wl = widths[l] as usize;
                if wl <= x0 {
                    continue;
                }
                let cnt = 16.min(wl - x0);
                let ch = [lanes_r[l], lanes_g[l], lanes_b[l], lanes_a[l]];
                let px = l * lw + x0;
                if cnt == 16 {
                    crate::recon::out_avx2::write8(
                        t,
                        &ch,
                        NCH,
                        &mut out_row[px * NCH..px * NCH + 16 * NCH],
                    );
                } else {
                    crate::recon::out_avx2::write8(t, &ch, NCH, &mut tmp[..16 * NCH]);
                    out_row[px * NCH..(px + cnt) * NCH].copy_from_slice(&tmp[..cnt * NCH]);
                }
            }
        }
    }

    pub(crate) fn write_row8_pix_adapt(
        t: X64V3Token,
        cur: &[&[u16]],
        nch: usize,
        alpha_const: Option<u8>,
        widths: &[u32; NL],
        lw: usize,
        aw: &AdaptWeights,
        out_row: &mut [u8],
    ) {
        if nch == 3 {
            write_row8_pix_adapt_kernel::<3>(t, cur, None, widths, lw, aw, out_row);
        } else {
            write_row8_pix_adapt_kernel::<4>(t, cur, alpha_const, widths, lw, aw, out_row);
        }
    }
}

#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
pub(crate) mod v4x {
    use super::AdaptWeights;
    use crate::rans::NL;
    use archmage::prelude::*;

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
    const HI_MASK: u64 = 0xFF00_FF00_FF00_FF00;

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

    #[arcane(import_intrinsics)]
    fn write_row8_pix_adapt_v4x_kernel<const NCH: usize>(
        _t: X64V4xToken,
        cur: &[&[u16]],
        alpha_const: Option<u8>,
        widths: &[u32; NL],
        lw: usize,
        aw: &AdaptWeights,
        out_row: &mut [u8],
    ) {
        let zero = _mm512_setzero_si512();
        let c128 = _mm512_set1_epi16(128);
        let mask_ff = _mm512_set1_epi16(0x00FF);

        let z_gr = _mm512_broadcast_i64x4(_mm256_loadu_si256(&aw.gr));
        let z_gb = _mm512_broadcast_i64x4(_mm256_loadu_si256(&aw.gb));
        let z_rb = _mm512_broadcast_i64x4(_mm256_loadu_si256(&aw.rb));

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
        let has_alpha_slot = NCH == 4 && alpha_const.is_none();
        let a_const_z = _mm512_set1_epi8(alpha_const.unwrap_or(0) as i8);

        let blocks = lw.div_ceil(16);
        let mut pad_g = [0u16; 16 * NL];
        let mut pad_r = [0u16; 16 * NL];
        let mut pad_b = [0u16; 16 * NL];
        let mut pad_a = [0u16; 16 * NL];
        let mut tmp = [0u8; 64];

        for bk in 0..blocks {
            let x0 = bk * 16;
            let n = 16.min(lw - x0);
            let (sg, sr, sb, sa): (&[u16], &[u16], &[u16], &[u16]) = if n == 16 {
                (
                    &cur[0][x0 * NL..(x0 + 16) * NL],
                    &cur[1][x0 * NL..(x0 + 16) * NL],
                    &cur[2][x0 * NL..(x0 + 16) * NL],
                    if has_alpha_slot {
                        &cur[3][x0 * NL..(x0 + 16) * NL]
                    } else {
                        &pad_a
                    },
                )
            } else {
                pad_g.fill(0);
                pad_r.fill(0);
                pad_b.fill(0);
                pad_g[..n * NL].copy_from_slice(&cur[0][x0 * NL..(x0 + n) * NL]);
                pad_r[..n * NL].copy_from_slice(&cur[1][x0 * NL..(x0 + n) * NL]);
                pad_b[..n * NL].copy_from_slice(&cur[2][x0 * NL..(x0 + n) * NL]);
                if has_alpha_slot {
                    pad_a.fill(0);
                    pad_a[..n * NL].copy_from_slice(&cur[3][x0 * NL..(x0 + n) * NL]);
                }
                (&pad_g, &pad_r, &pad_b, &pad_a)
            };

            let mut ar = [zero; 4];
            let mut ag = [zero; 4];
            let mut ab = [zero; 4];
            let mut aa = [zero; 4];

            for k in 0..4 {
                let pair = |j: usize| -> (__m256i, __m256i, __m256i, __m256i) {
                    let off = (4 * k + 2 * j) * NL;
                    let gv =
                        _mm512_loadu_si512(<&[u16; 32]>::try_from(&sg[off..off + 32]).unwrap());
                    let ri =
                        _mm512_loadu_si512(<&[u16; 32]>::try_from(&sr[off..off + 32]).unwrap());
                    let bi =
                        _mm512_loadu_si512(<&[u16; 32]>::try_from(&sb[off..off + 32]).unwrap());

                    let gs = _mm512_sub_epi16(gv, c128);
                    let pr = _mm512_srai_epi16::<5>(_mm512_mullo_epi16(z_gr, gs));
                    let rv = _mm512_and_si512(_mm512_add_epi16(ri, pr), mask_ff);
                    let rs = _mm512_sub_epi16(rv, c128);
                    let sum_b = _mm512_add_epi16(
                        _mm512_mullo_epi16(z_gb, gs),
                        _mm512_mullo_epi16(z_rb, rs),
                    );
                    let pb = _mm512_srai_epi16::<5>(sum_b);
                    let bv = _mm512_add_epi16(bi, pb);
                    let av = if has_alpha_slot {
                        _mm512_cvtepi16_epi8(_mm512_loadu_si512(
                            <&[u16; 32]>::try_from(&sa[off..off + 32]).unwrap(),
                        ))
                    } else {
                        _mm256_setzero_si256()
                    };
                    (
                        _mm512_cvtepi16_epi8(rv),
                        _mm512_cvtepi16_epi8(gv),
                        _mm512_cvtepi16_epi8(bv),
                        av,
                    )
                };
                let (r0, g0, b0, a0) = pair(0);
                let (r1, g1, b1, a1) = pair(1);
                ar[k] = _mm512_inserti64x4::<1>(_mm512_castsi256_si512(r0), r1);
                ag[k] = _mm512_inserti64x4::<1>(_mm512_castsi256_si512(g0), g1);
                ab[k] = _mm512_inserti64x4::<1>(_mm512_castsi256_si512(b0), b1);
                if has_alpha_slot {
                    aa[k] = _mm512_inserti64x4::<1>(_mm512_castsi256_si512(a0), a1);
                }
            }

            let mut lt = [[zero; 4]; 4];
            let srcs = [&ar, &ag, &ab, &aa];
            let n_tr = if has_alpha_slot { 4 } else { 3 };
            for c in 0..n_tr {
                let a = srcs[c];
                for m in 0..4 {
                    let lo = _mm512_permutex2var_epi8(a[0], tr_lo[m], a[1]);
                    let hi = _mm512_permutex2var_epi8(a[2], tr_hi[m], a[3]);
                    lt[c][m] = _mm512_mask_blend_epi8(HI_MASK, lo, hi);
                }
            }
            if NCH == 4 && !has_alpha_slot {
                lt[3] = [a_const_z; 4];
            }

            for l in 0..NL {
                let wl = widths[l] as usize;
                if wl <= x0 {
                    continue;
                }
                let cnt = 16.min(wl - x0);
                let (m, li) = (l / 4, l % 4);
                let v = if NCH == 3 {
                    let u = _mm512_permutex2var_epi8(lt[0][m], rgb1_v[li], lt[1][m]);
                    _mm512_permutex2var_epi8(u, rgb2_v[li], lt[2][m])
                } else {
                    let idx = _mm512_loadu_si512(&RGBA[li]);
                    let rg = _mm512_permutex2var_epi8(lt[0][m], idx, lt[1][m]);
                    let ba = _mm512_permutex2var_epi8(lt[2][m], idx, lt[3][m]);
                    _mm512_mask_blend_epi8(BA_MASK, rg, ba)
                };
                let px = l * lw + x0;
                let bytes = cnt * NCH;
                if cnt == 16 && (NCH == 4 || wl >= x0 + 22) {
                    let o: &mut [u8; 64] =
                        (&mut out_row[px * NCH..px * NCH + 64]).try_into().unwrap();
                    _mm512_storeu_si512(o, v);
                } else if cnt == 16 {
                    _mm512_storeu_si512(&mut tmp, v);
                    let o: &mut [u8; 48] = (&mut out_row[px * 3..px * 3 + 48]).try_into().unwrap();
                    *o = tmp[..48].try_into().unwrap();
                } else {
                    _mm512_storeu_si512(&mut tmp, v);
                    out_row[px * NCH..px * NCH + bytes].copy_from_slice(&tmp[..bytes]);
                }
            }
        }
    }

    pub(crate) fn write_row8_pix_adapt(
        t: X64V4xToken,
        cur: &[&[u16]],
        nch: usize,
        alpha_const: Option<u8>,
        widths: &[u32; NL],
        lw: usize,
        aw: &AdaptWeights,
        out_row: &mut [u8],
    ) {
        if nch == 3 {
            write_row8_pix_adapt_v4x_kernel::<3>(t, cur, None, widths, lw, aw, out_row);
        } else {
            write_row8_pix_adapt_v4x_kernel::<4>(t, cur, alpha_const, widths, lw, aw, out_row);
        }
    }
}

#[cfg(target_arch = "aarch64")]
pub(crate) mod neon {
    use super::AdaptWeights;
    use crate::rans::NL;
    use crate::recon::out_neon::{emit_rgb8, emit_rgba8, pack8, rgb8_tables, transpose16_u8};
    use archmage::prelude::*;
    use magetypes::simd::{i16x16, u8x16, u16x16};

    #[arcane]
    pub(crate) fn write_row8_pix_adapt(
        t: NeonToken,
        cur: &[&[u16]],
        nch: usize,
        alpha_const: Option<u8>,
        widths: &[u32; NL],
        lw: usize,
        aw: &AdaptWeights,
        out_row: &mut [u8],
    ) {
        let z8 = u8x16::zero_t(t);
        let c128 = i16x16::splat_t(t, 128);
        let mask_ff = i16x16::splat_t(t, 0x00FF);
        let v_gr = i16x16::load_t(t, &aw.gr);
        let v_gb = i16x16::load_t(t, &aw.gb);
        let v_rb = i16x16::load_t(t, &aw.rb);
        let t8 = rgb8_tables(t);
        let g_ch = &cur[0].as_chunks::<NL>().0[..lw];
        let r_ch = &cur[1].as_chunks::<NL>().0[..lw];
        let b_ch = &cur[2].as_chunks::<NL>().0[..lw];
        let a_ch = if nch == 4 && alpha_const.is_none() {
            Some(&cur[3].as_chunks::<NL>().0[..lw])
        } else {
            None
        };
        let n_tr = if a_ch.is_some() { 4 } else { 3 };
        let a_const_b = u8x16::splat_t(t, alpha_const.unwrap_or(0));

        let blocks = lw.div_ceil(16);
        for bk in 0..blocks {
            let x0 = bk * 16;
            let n = 16.min(lw - x0);
            let mut rows = [[z8; 16]; 4];

            for i in 0..n {
                let x = x0 + i;
                let gv = u16x16::load_t(t, &g_ch[x]).bitcast_i16x16();
                let ri = u16x16::load_t(t, &r_ch[x]).bitcast_i16x16();
                let bi = u16x16::load_t(t, &b_ch[x]).bitcast_i16x16();

                let gs = gv - c128;
                let pr = (v_gr * gs).shr_arithmetic_const::<5>();
                let rv = (ri + pr) & mask_ff;
                let rs = rv - c128;
                let pb = (v_gb * gs + v_rb * rs).shr_arithmetic_const::<5>();
                let bv = (bi + pb) & mask_ff;

                rows[0][i] = pack8(t, rv.bitcast_u16x16());
                rows[1][i] = pack8(t, gv.bitcast_u16x16());
                rows[2][i] = pack8(t, bv.bitcast_u16x16());
                if let Some(ac) = a_ch {
                    rows[3][i] = pack8(t, u16x16::load_t(t, &ac[x]));
                }
            }

            let mut lanes = [[z8; 16]; 4];
            for c in 0..n_tr {
                lanes[c] = transpose16_u8(t, &rows[c]);
            }

            for l in 0..NL {
                let wl = widths[l] as usize;
                if wl <= x0 {
                    continue;
                }
                let cnt = 16.min(wl - x0);
                let px = l * lw + x0;
                let dst = &mut out_row[px * nch..px * nch + cnt * nch];
                let rb = lanes[0][l];
                let gb = lanes[1][l];
                let bb = lanes[2][l];
                if nch == 3 {
                    emit_rgb8(t, rb, gb, bb, &t8, dst);
                } else {
                    let ab = if a_ch.is_some() {
                        lanes[3][l]
                    } else {
                        a_const_b
                    };
                    emit_rgba8(t, rb, gb, bb, ab, dst);
                }
            }
        }
    }
}

#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
pub(crate) mod wasm128 {
    use super::AdaptWeights;
    use crate::rans::NL;
    use crate::recon::out_wasm128::{emit_rgb8, emit_rgba8, pack8, rgb8_tables, transpose16_u8};
    use archmage::prelude::*;
    use magetypes::simd::{i16x16, u8x16, u16x16};

    /// Compact single-body fused wasm128 pixel-domain inverse + u8 16x16 transpose + RGB/RGBA8 writer.
    #[arcane]
    #[inline(never)]
    pub(crate) fn write_row8_pix_adapt(
        t: Wasm128Token,
        cur: &[&[u16]],
        nch: usize,
        alpha_const: Option<u8>,
        widths: &[u32; NL],
        lw: usize,
        aw: &AdaptWeights,
        out_row: &mut [u8],
    ) {
        let z8 = u8x16::zero_t(t);
        let c128 = i16x16::splat_t(t, 128);
        let mask_ff = i16x16::splat_t(t, 0x00FF);
        let v_gr = i16x16::load_t(t, &aw.gr);
        let v_gb = i16x16::load_t(t, &aw.gb);
        let v_rb = i16x16::load_t(t, &aw.rb);
        let t8 = rgb8_tables(t);
        let g_ch = &cur[0].as_chunks::<NL>().0[..lw];
        let r_ch = &cur[1].as_chunks::<NL>().0[..lw];
        let b_ch = &cur[2].as_chunks::<NL>().0[..lw];
        let a_ch = if nch == 4 && alpha_const.is_none() {
            Some(&cur[3].as_chunks::<NL>().0[..lw])
        } else {
            None
        };
        let n_tr = if a_ch.is_some() { 4 } else { 3 };
        let a_const_b = u8x16::splat_t(t, alpha_const.unwrap_or(0));

        let blocks = lw.div_ceil(16);
        for bk in 0..blocks {
            let x0 = bk * 16;
            let n = 16.min(lw - x0);
            let mut rows = [[z8; 16]; 4];

            for i in 0..n {
                let x = x0 + i;
                let gv = u16x16::load_t(t, &g_ch[x]).bitcast_i16x16();
                let ri = u16x16::load_t(t, &r_ch[x]).bitcast_i16x16();
                let bi = u16x16::load_t(t, &b_ch[x]).bitcast_i16x16();

                let gs = gv - c128;
                let pr = (v_gr * gs).shr_arithmetic_const::<5>();
                let rv = (ri + pr) & mask_ff;
                let rs = rv - c128;
                let pb = (v_gb * gs + v_rb * rs).shr_arithmetic_const::<5>();
                let bv = (bi + pb) & mask_ff;

                rows[0][i] = pack8(t, rv.bitcast_u16x16());
                rows[1][i] = pack8(t, gv.bitcast_u16x16());
                rows[2][i] = pack8(t, bv.bitcast_u16x16());
                if let Some(ac) = a_ch {
                    rows[3][i] = pack8(t, u16x16::load_t(t, &ac[x]));
                }
            }

            let mut lanes = [[z8; 16]; 4];
            for c in 0..n_tr {
                lanes[c] = transpose16_u8(t, &rows[c]);
            }

            for l in 0..NL {
                let wl = widths[l] as usize;
                if wl <= x0 {
                    continue;
                }
                let cnt = 16.min(wl - x0);
                let px = l * lw + x0;
                let dst = &mut out_row[px * nch..px * nch + cnt * nch];
                let rb = lanes[0][l];
                let gb = lanes[1][l];
                let bb = lanes[2][l];
                if nch == 3 {
                    emit_rgb8(t, rb, gb, bb, &t8, dst);
                } else {
                    let ab = if a_ch.is_some() {
                        lanes[3][l]
                    } else {
                        a_const_b
                    };
                    emit_rgba8(t, rb, gb, bb, ab, dst);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{vec, vec::Vec};

    #[test]
    fn weights_roundtrip_and_strict_validation() {
        for active in 1..=NL {
            let mut aw = AdaptWeights {
                gr: [0; NL],
                gb: [0; NL],
                rb: [0; NL],
            };
            for l in 0..active {
                // Mix small deltas (5-bit path) and large jumps / extremes (9-bit path).
                aw.gr[l] = match l % 4 {
                    0 => 32 + (l as i16) - 4,
                    1 => -127,
                    2 => 127,
                    _ => 0,
                };
                aw.gb[l] = (16 + (l as i16) * 3 - 20).clamp(-127, 127);
                aw.rb[l] = (-127 + (l as i16) * 17).clamp(-127, 127);
            }
            let mut buf = Vec::new();
            aw.write_to(&mut buf, active);
            assert_eq!(buf.len(), aw.byte_len(active));
            let mut r = ByteReader::new(&buf);
            let back = AdaptWeights::read_from(&mut r, active).unwrap();
            assert_eq!(r.remaining(), 0);
            assert_eq!(back, aw);

            // Truncation must fail.
            if !buf.is_empty() {
                let mut rt = ByteReader::new(&buf[..buf.len() - 1]);
                assert!(AdaptWeights::read_from(&mut rt, active).is_err());
            }
        }

        // Raw weight -128 must be rejected.
        // active = 1: flag=1 (bit 0), raw=0x80 (-128 in i8) -> bits: 1, 0000_0001 => byte 0 = 0x01, byte 1 low bit = 1
        let mut raw_neg128 = Vec::new();
        {
            let mut bw = WeightBitWriter::new(&mut raw_neg128);
            bw.put(1, 1);
            bw.put(0x80, 8);
            bw.put(0, 1);
            bw.put(8, 4);
            bw.put(0, 1);
            bw.put(8, 4);
            bw.finish();
        }
        let mut r = ByteReader::new(&raw_neg128);
        assert!(matches!(
            AdaptWeights::read_from(&mut r, 1),
            Err(Error::Corrupt(_))
        ));

        // Delta pushing weight below -127 or above +127 must be rejected.
        // active = 2: lane 0 raw = +127, lane 1 delta = +1 (u=9) => 128 > 127.
        let mut delta_hi = Vec::new();
        {
            let mut bw = WeightBitWriter::new(&mut delta_hi);
            bw.put(1, 1);
            bw.put(127, 8);
            bw.put(0, 1);
            bw.put(9, 4); // +1 -> 128
            for _ in 0..4 {
                bw.put(0, 1);
                bw.put(8, 4);
            }
            bw.finish();
        }
        let mut r = ByteReader::new(&delta_hi);
        assert!(matches!(
            AdaptWeights::read_from(&mut r, 2),
            Err(Error::Corrupt(_))
        ));

        // Non-zero trailing padding bit must be rejected.
        // active = 1 with three 5-bit deltas = 15 bits -> 1 padding bit at bit 7 of byte 1.
        let mut bad_pad = Vec::new();
        let aw1 = AdaptWeights {
            gr: [32; NL],
            gb: [16; NL],
            rb: [16; NL],
        };
        aw1.write_to(&mut bad_pad, 1);
        assert_eq!(bad_pad.len(), 2);
        bad_pad[1] |= 0x80;
        let mut r = ByteReader::new(&bad_pad);
        assert!(matches!(
            AdaptWeights::read_from(&mut r, 1),
            Err(Error::Corrupt(_))
        ));
    }

    #[test]
    fn extreme_weights_inverse_matches_scalar_and_simd() {
        let aw = AdaptWeights {
            gr: [
                -127, 127, -127, 127, 0, 32, -32, 64, -64, 96, -96, 1, -1, 126, -126, 15,
            ],
            gb: [
                127, -127, -127, 127, 16, -16, 127, -127, 0, 32, -32, 127, -127, 63, -63, -127,
            ],
            rb: [
                127, -127, 127, -127, 16, 127, -127, 0, 127, -127, 32, -127, 127, -64, 64, -127,
            ],
        };
        // Test full extreme corners + non-multiple-of-16 widths + narrow widths < 16.
        for &(w, nch, alpha_const) in &[
            (1usize, 3usize, None),
            (7, 3, None),
            (16, 3, None),
            (17, 3, None),
            (63, 3, None),
            (257, 3, None),
            (1, 4, None),
            (15, 4, Some(255u8)),
            (37, 4, Some(128u8)),
            (257, 4, None),
        ] {
            let lw = w.div_ceil(NL).max(1);
            let widths = crate::v1::lane_widths(w, lw);
            let ns = if nch == 4 && alpha_const.is_none() {
                4
            } else {
                3
            };
            let mut cur = vec![vec![0u16; lw * NL]; ns];
            let mut expected = vec![0u8; w * nch];
            for l in 0..NL {
                let wl = widths[l] as usize;
                for x in 0..wl {
                    let i = x * NL + l;
                    let g = ((x * 73 + l * 37 + if x == 0 { 0 } else { 255 }) & 0xFF) as u32;
                    let r_in = ((x * 131 + l * 59) & 0xFF) as u32;
                    let b_in = ((x * 197 + l * 83) & 0xFF) as u32;
                    cur[0][i] = g as u16;
                    cur[1][i] = r_in as u16;
                    cur[2][i] = b_in as u16;
                    let a = if ns == 4 {
                        let av = ((x * 29 + l * 13) & 0xFF) as u8;
                        cur[3][i] = av as u16;
                        av
                    } else {
                        alpha_const.unwrap_or(0)
                    };
                    let (go, ro, bo) = inverse_pixel_triple(&aw, l, g, r_in, b_in);
                    let px = l * lw + x;
                    expected[px * nch] = ro as u8;
                    expected[px * nch + 1] = go as u8;
                    expected[px * nch + 2] = bo as u8;
                    if nch == 4 {
                        expected[px * nch + 3] = a;
                    }
                }
            }
            let refs: Vec<&[u16]> = cur.iter().map(Vec::as_slice).collect();
            #[cfg(target_arch = "x86_64")]
            {
                use archmage::SimdToken;
                if let Some(t) = archmage::X64V3Token::summon() {
                    let mut out = vec![0xAAu8; w * nch];
                    avx2::write_row8_pix_adapt(
                        t,
                        &refs,
                        nch,
                        alpha_const,
                        &widths,
                        lw,
                        &aw,
                        &mut out,
                    );
                    assert_eq!(
                        out, expected,
                        "avx2 mismatch w={w} nch={nch} a={alpha_const:?}"
                    );
                }
                #[cfg(feature = "avx512")]
                if let Some(t) = archmage::X64V4xToken::summon() {
                    let mut out = vec![0xAAu8; w * nch];
                    v4x::write_row8_pix_adapt(
                        t,
                        &refs,
                        nch,
                        alpha_const,
                        &widths,
                        lw,
                        &aw,
                        &mut out,
                    );
                    assert_eq!(
                        out, expected,
                        "v4x mismatch w={w} nch={nch} a={alpha_const:?}"
                    );
                }
            }
            #[cfg(target_arch = "aarch64")]
            {
                use archmage::SimdToken;
                if let Some(t) = archmage::NeonToken::summon() {
                    let mut out = vec![0xAAu8; w * nch];
                    neon::write_row8_pix_adapt(
                        t,
                        &refs,
                        nch,
                        alpha_const,
                        &widths,
                        lw,
                        &aw,
                        &mut out,
                    );
                    assert_eq!(
                        out, expected,
                        "neon mismatch w={w} nch={nch} a={alpha_const:?}"
                    );
                }
            }
            #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
            {
                use archmage::SimdToken;
                if let Some(t) = archmage::Wasm128Token::summon() {
                    let mut out = vec![0xAAu8; w * nch];
                    wasm128::write_row8_pix_adapt(
                        t,
                        &refs,
                        nch,
                        alpha_const,
                        &widths,
                        lw,
                        &aw,
                        &mut out,
                    );
                    assert_eq!(
                        out, expected,
                        "wasm128 mismatch w={w} nch={nch} a={alpha_const:?}"
                    );
                }
            }
        }
    }
}
