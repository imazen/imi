//! LAB (small-transform study, 2026-10-11): the integer 4x4 core candidate
//! for the normative 4x4 inverse, against the f32 butterfly of
//! [`super::small`]. Not a format feature: lab files mark it with a one-byte
//! payload of head record 23, which production decoders reject.
//!
//! The candidate is H.264's 4x4 core: per pass `e0 = v0 + v2`, `e1 = v0 -
//! v2`, `e2 = (v1 >> 1) - v3`, `e3 = v1 + (v3 >> 1)`, outputs `(e0 + e3, e1 +
//! e2, e1 - e2, e0 - e3)`, on i32 lanes (wrapping). Its basis rows (1, 1, 1,
//! 1), (1, 1/2, -1/2, -1), (1, -1, -1, 1), (1/2, -1, 1, -1/2) are orthogonal,
//! with norms 2 and sqrt(5/2); the encoder projects onto the normalized
//! rows, and the decoder folds the norms, the Haar's 1/2 and the fixed-point
//! scale `2^F` into the f32 dequantization factors, rounds each dequantized
//! coefficient to an i32 (nearest, ties to even, saturating), runs the Haar
//! and the passes in integers and converts back (`x as f32 * 2^-F`, exact
//! below 2^24).
//!
//! The slot layout, steps, DC and Haar are those of [`super::small`].

#[cfg(not(feature = "std"))]
use crate::mathx::RoundTiesEven as _;

use super::small::{haar_ref, phys};
use archmage::prelude::*;
use magetypes::simd::backends::{F32x8Convert, I32x8Backend};
use magetypes::simd::generic::{f32x8 as vf8, i32x8 as vi8};

/// Fractional bits of the fixed-point representation.
pub(crate) const FRAC: i32 = 8;
const SCALE: f32 = (1 << FRAC) as f32;
const INV_SCALE: f32 = 1.0 / SCALE;

/// `1 / sqrt(5 / 2)`, rounded to f32: the odd rows' normalization.
const N_ODD: f32 = 0.632_455_5;

/// Per-axis input scale of the integer basis (1/2 for the even rows).
const SI: [f32; 4] = [0.5, N_ODD, 0.5, N_ODD];

/// The integer candidate's dequantization factors (fixed point included):
/// `step * scale`, scale `2^F / 8` in the Haar slots and `2^F * SI[v >> 1] *
/// SI[u >> 1]` elsewhere.
pub(crate) fn dequant4_int(t: &[f32; 64]) -> [f32; 64] {
    core::array::from_fn(|i| {
        let (v, u) = (i / 8, i % 8);
        let p = if v < 2 && u < 2 {
            0.125
        } else {
            SI[(v >> 1) & 3] * SI[(u >> 1) & 3]
        };
        t[phys(i)] * (p * SCALE)
    })
}

/// Round to nearest (ties to even), saturating to `+-(2^31 - 128)`, the
/// same result on every tier.
#[inline(always)]
fn to_fixed(x: f32) -> i32 {
    let c = x.clamp(-2_147_483_520.0, 2_147_483_520.0);
    // `round_ties_even` then an exact conversion.
    c.round_ties_even() as i32
}

#[inline(always)]
fn pass_int(v0: i32, v1: i32, v2: i32, v3: i32) -> [i32; 4] {
    let e0 = v0.wrapping_add(v2);
    let e1 = v0.wrapping_sub(v2);
    let e2 = (v1 >> 1).wrapping_sub(v3);
    let e3 = v1.wrapping_add(v3 >> 1);
    [
        e0.wrapping_add(e3),
        e1.wrapping_add(e2),
        e1.wrapping_sub(e2),
        e0.wrapping_sub(e3),
    ]
}

/// The integer candidate's reference inverse of one 4x4 plane: `x[i]` the
/// dequantized coefficients (`q * dequant4_int * m`); returns the pixels.
pub(crate) fn inverse_int_ref(x: &[f32; 64]) -> [f32; 64] {
    let mut v: [i32; 64] = core::array::from_fn(|i| to_fixed(x[i]));
    // The Haar in integers: the same butterflies, exact.
    let a = v[0].wrapping_add(v[8]);
    let b = v[0].wrapping_sub(v[8]);
    let c = v[1].wrapping_add(v[9]);
    let d = v[1].wrapping_sub(v[9]);
    v[0] = a.wrapping_add(c);
    v[1] = a.wrapping_sub(c);
    v[8] = b.wrapping_add(d);
    v[9] = b.wrapping_sub(d);
    let mut y = [0i32; 64];
    for u in 0..8 {
        for ty in 0..2 {
            let r = pass_int(
                v[ty * 8 + u],
                v[(2 + ty) * 8 + u],
                v[(4 + ty) * 8 + u],
                v[(6 + ty) * 8 + u],
            );
            for n in 0..4 {
                y[(ty * 4 + n) * 8 + u] = r[n];
            }
        }
    }
    let mut out = [0.0f32; 64];
    for row in 0..8 {
        for tx in 0..2 {
            let o = row * 8;
            let r = pass_int(y[o + tx], y[o + 2 + tx], y[o + 4 + tx], y[o + 6 + tx]);
            for n in 0..4 {
                out[o + tx * 4 + n] = r[n] as f32 * INV_SCALE;
            }
        }
    }
    out
}

/// The normalized integer basis rows (encoder).
const B4: [[f32; 4]; 4] = [
    [0.5, 0.5, 0.5, 0.5],
    [N_ODD, N_ODD * 0.5, -N_ODD * 0.5, -N_ODD],
    [0.5, -0.5, -0.5, 0.5],
    [N_ODD * 0.5, -N_ODD, N_ODD, -N_ODD * 0.5],
];

/// The integer candidate's forward (encoder): [`super::small::forward`]
/// with the normalized integer basis instead of the DCT.
pub(crate) fn forward_int(px: &[f32; 64]) -> [f32; 64] {
    let mut h = [0.0f32; 64];
    for y in 0..8 {
        for tx in 0..2 {
            for fu in 0..4 {
                let mut s = 0.0f32;
                for n in 0..4 {
                    s += B4[fu][n] * px[y * 8 + tx * 4 + n];
                }
                h[y * 8 + 2 * fu + tx] = s;
            }
        }
    }
    let mut out = [0.0f32; 64];
    for u in 0..8 {
        for ty in 0..2 {
            for fv in 0..4 {
                let mut s = 0.0f32;
                for n in 0..4 {
                    s += B4[fv][n] * h[(ty * 4 + n) * 8 + u];
                }
                out[(2 * fv + ty) * 8 + u] = s;
            }
        }
    }
    let (tl, tr, bl, br) = (out[0], out[1], out[8], out[9]);
    out[0] = (tl + tr + bl + br) * 0.5;
    out[1] = (tl - tr + bl - br) * 0.5;
    out[8] = (tl + tr - bl - br) * 0.5;
    out[9] = (tl - tr - bl + br) * 0.5;
    out
}

// ---- SIMD (the speed probe; the same operations lane-wise) -------------

#[inline(always)]
fn pass_int_v<T: I32x8Backend>(v0: vi8<T>, v1: vi8<T>, v2: vi8<T>, v3: vi8<T>) -> [vi8<T>; 4] {
    let e0 = v0 + v2;
    let e1 = v0 - v2;
    let e2 = v1.shr_arithmetic_const::<1>() - v3;
    let e3 = v1 + v3.shr_arithmetic_const::<1>();
    [e0 + e3, e1 + e2, e1 - e2, e0 - e3]
}

/// One plane of a 4x4 parent through the integer candidate: `x[y]` the
/// dequantized rows (f32), `haar` the integer tile DCs of the Haar slots.
#[inline(always)]
pub(crate) fn block4_int<T: F32x8Convert>(token: T, x: [vf8<T>; 8], haar: [i32; 4]) -> [vf8<T>; 8] {
    let lo = vf8::splat_t(token, -2_147_483_520.0);
    let hi = vf8::splat_t(token, 2_147_483_520.0);
    // No closures in this generic helper (they would not inherit the
    // kernel's target features, docs/ARCHITECTURE.md).
    let z = vi8::zero_t(token);
    let mut v = [z; 8];
    for y in 0..8 {
        v[y] = x[y].clamp(lo, hi).to_i32x8_round();
    }
    // Lanes 0 and 1 of rows 0 and 1 take the tile DCs.
    let idx = vi8::from_array_t(token, [0, 1, 2, 3, 4, 5, 6, 7]);
    let m0 = idx.simd_lt(vi8::splat_t(token, 1));
    let m01 = idx.simd_lt(vi8::splat_t(token, 2));
    let top = vi8::blend(
        m0,
        vi8::splat_t(token, haar[0]),
        vi8::splat_t(token, haar[1]),
    );
    let bot = vi8::blend(
        m0,
        vi8::splat_t(token, haar[2]),
        vi8::splat_t(token, haar[3]),
    );
    v[0] = vi8::blend(m01, top, v[0]);
    v[1] = vi8::blend(m01, bot, v[1]);
    let a = pass_int_v(v[0], v[2], v[4], v[6]);
    let b = pass_int_v(v[1], v[3], v[5], v[7]);
    let zf = vf8::zero_t(token);
    let mut s = [zf; 8];
    for n in 0..4 {
        s[n] = a[n].bitcast_to_f32();
        s[4 + n] = b[n].bitcast_to_f32();
    }
    vf8::transpose_8x8(&mut s);
    let mut t = [z; 8];
    for n in 0..8 {
        t[n] = s[n].bitcast_to_i32();
    }
    let a = pass_int_v(t[0], t[2], t[4], t[6]);
    let b = pass_int_v(t[1], t[3], t[5], t[7]);
    let inv = vf8::splat_t(token, INV_SCALE);
    let mut o = [zf; 8];
    for n in 0..4 {
        o[n] = a[n].to_f32x8() * inv;
        o[4 + n] = b[n].to_f32x8() * inv;
    }
    vf8::transpose_8x8(&mut o);
    o
}

/// The integer tile DCs of a plane's Haar slots (`q * d`, fixed point).
#[inline(always)]
pub(crate) fn haar_int_of(q: &[i32], d: &[f32; 64]) -> [i32; 4] {
    let f = |i: usize| to_fixed(q[i] as f32 * d[i]);
    let (x0, x1, x8, x9) = (f(0), f(1), f(8), f(9));
    let a = x0.wrapping_add(x8);
    let b = x0.wrapping_sub(x8);
    let c = x1.wrapping_add(x9);
    let d = x1.wrapping_sub(x9);
    [
        a.wrapping_add(c),
        a.wrapping_sub(c),
        b.wrapping_add(d),
        b.wrapping_sub(d),
    ]
}

/// The f32 Haar of [`super::small`], for the probe's shared setup.
#[allow(dead_code)]
pub(crate) fn haar_f32(x: &[f32; 64]) -> [f32; 4] {
    haar_ref(x[0], x[1], x[8], x[9])
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    fn xorshift(seed: &mut u32) -> u32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        *seed
    }

    /// Forward then the integer inverse reproduces random parents within
    /// the fixed point's rounding.
    #[test]
    fn integer_candidate_inverts_its_forward() {
        let mut seed = 0x0bad_5eedu32;
        let ones = [1.0f32; 64];
        let d = dequant4_int(&ones);
        for _ in 0..500 {
            let px: [f32; 64] =
                core::array::from_fn(|_| (xorshift(&mut seed) % 2001) as f32 / 4.0 - 250.0);
            let c = forward_int(&px);
            let x: [f32; 64] = core::array::from_fn(|i| c[i] * d[i]);
            let out = inverse_int_ref(&x);
            for i in 0..64 {
                assert!(
                    (out[i] - px[i]).abs() < 0.02,
                    "{i}: {} vs {}",
                    out[i],
                    px[i]
                );
            }
        }
    }
}

// ---- kernel probe: dequantization + inverse per block plane ------------

/// The integer candidate over blocks `0..n` of a row (three planes each,
/// every plane 4x4), as `simd::recon_block_row` lays them out.
#[magetypes(define(f32x8, i32x8), v3, neon, wasm128, scalar)]
fn int_row_impl(
    token: Token,
    q: &mut [i32],
    n: usize,
    dq: &[[f32; 64]; 3],
    planes: &mut [f32],
    pw: usize,
) {
    for bx in 0..n {
        for ch in 0..3 {
            let qb = &mut q[(bx * 3 + ch) * 64..][..64];
            let d = &dq[ch];
            let mut x = [f32x8::zero_t(token); 8];
            for (y, xr) in x.iter_mut().enumerate() {
                let qi: &[i32; 8] = qb[y * 8..y * 8 + 8].try_into().unwrap();
                let dd: &[f32; 8] = d[y * 8..y * 8 + 8].try_into().unwrap();
                *xr =
                    f32x8::from_i32x8_t(token, i32x8::load_t(token, qi)) * f32x8::load_t(token, dd);
            }
            let haar = haar_int_of(qb, d);
            qb.fill(0);
            let o = block4_int(token, x, haar);
            let base = ch * 8 * pw + bx * 8;
            for (y, row) in o.iter().enumerate() {
                let out: &mut [f32; 8] = (&mut planes[base + y * pw..][..8]).try_into().unwrap();
                row.store(out);
            }
        }
    }
}

fn int_row(q: &mut [i32], n: usize, dq: &[[f32; 64]; 3], planes: &mut [f32], pw: usize) {
    // AVX-512 hosts run the 8-lane v3 form (magetypes' x86-64-v4 token has
    // no i32x8 backend); compare it with the f32 kernel under
    // `IMIJ_NO_AVX512`.
    if crate::v1::force_scalar() {
        return int_row_impl_scalar(archmage::ScalarToken, q, n, dq, planes, pw);
    }
    archmage::incant!(
        int_row_impl(q, n, dq, planes, pw),
        [v3, neon, wasm128, scalar]
    )
}

/// Nanoseconds per block plane (median of `rounds` timed passes of `iters`
/// rows each) of `kernel` over a row of `blocks` blocks with random dense
/// coefficients (no flat block): `aan8` the 8x8 decoder kernel, `f4` the
/// normative 4x4 decoder kernel (every plane 4x4), `int4` the integer
/// candidate, `ref4` / `ref8` the scalar references, `dense4` the record-19
/// 4x4 layout's dense inverse, `copy` the coefficient copy every pass
/// includes. The tier follows `IMIJ_SCALAR` / `IMIJ_NO_AVX512`.
#[cfg(feature = "std")]
pub(crate) fn kernel_bench(
    kernel: &str,
    blocks: usize,
    iters: usize,
    rounds: usize,
) -> Result<f64, crate::Error> {
    use super::{NCH, dct, simd, small};
    let mut seed = 0x1234_5678u32;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    // Quantized values of a mid-quality block: small, many zeros.
    let src: alloc::vec::Vec<i32> = (0..blocks * NCH * 64)
        .map(|i| {
            if i % 64 == 0 {
                (rnd() % 4001) as i32 - 2000
            } else if rnd() % 3 == 0 {
                (rnd() % 41) as i32 - 20
            } else {
                0
            }
        })
        .collect();
    let eobs = alloc::vec![63u8; blocks * NCH + 1];
    let t: [f32; 64] = core::array::from_fn(|i| 2.0 + i as f32 * 0.25);
    let dq8 = [dct::dequant_factors(&t); 3];
    let dq4 = [small::dequant4(&t); 3];
    let dqi = [dequant4_int(&t); 3];
    let split = alloc::vec![7u8; blocks];
    let pw = blocks * 8 + 16;
    let mut planes = alloc::vec![0.0f32; NCH * 8 * pw];
    let mut q = src.clone();
    let l4 = super::subblock::Layout {
        sx: 0,
        sy: 0,
        log_block: 2,
    };
    let pass = |q: &mut [i32], planes: &mut [f32]| -> Result<(), crate::Error> {
        match kernel {
            "copy" => {}
            "aan8" => simd::recon_block_row(
                q,
                &eobs,
                0,
                blocks,
                0,
                blocks,
                &dq8,
                &[],
                None,
                planes,
                pw,
                simd::Small::NONE,
                true,
            ),
            "f4" => simd::recon_block_row(
                q,
                &eobs,
                0,
                blocks,
                0,
                blocks,
                &dq8,
                &[],
                None,
                planes,
                pw,
                simd::Small {
                    split: &split,
                    dq: &dq4,
                    int: false,
                },
                true,
            ),
            "int4" => int_row(q, blocks, &dqi, planes, pw),
            "ref4" | "ref8" | "dense4" => {
                for bx in 0..blocks {
                    for ch in 0..NCH {
                        let qb = &mut q[(bx * NCH + ch) * 64..][..64];
                        let d = if kernel == "ref8" { &dq8[ch] } else { &dq4[ch] };
                        let mut x: [f32; 64] = core::array::from_fn(|i| qb[i] as f32 * d[i]);
                        match kernel {
                            "ref4" => small::inverse_ref(&mut x),
                            "ref8" => dct::idct8x8(&mut x),
                            _ => x = l4.inverse(&x),
                        }
                        qb.fill(0);
                        for y in 0..8 {
                            planes[ch * 8 * pw + y * pw + bx * 8..][..8]
                                .copy_from_slice(&x[y * 8..y * 8 + 8]);
                        }
                    }
                }
            }
            _ => return Err(crate::Error::InvalidInput("unknown kernel")),
        }
        Ok(())
    };
    // Warm up, then the timed rounds.
    for _ in 0..iters.min(16) {
        q.copy_from_slice(&src);
        pass(&mut q, &mut planes)?;
    }
    let mut times = alloc::vec::Vec::with_capacity(rounds);
    for _ in 0..rounds {
        let start = std::time::Instant::now();
        for _ in 0..iters {
            q.copy_from_slice(&src);
            pass(&mut q, &mut planes)?;
        }
        times.push(start.elapsed().as_nanos() as f64 / (iters * blocks * NCH) as f64);
    }
    core::hint::black_box(&planes);
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Ok(times[times.len() / 2])
}

// ---- map-coding variants (the joint map grammar study) -----------------

/// An adaptive binary model of the map coder with a configurable start and
/// adaptation (`shift`; with `fast`, the first updates of each probability
/// adapt faster: shift `min(1 + n, shift)` after n updates).
#[derive(Clone, Copy)]
struct P {
    p: u16,
    n: u8,
}

#[cfg(feature = "std")]
struct Rc {
    low: u64,
    range: u32,
    cache: u8,
    cache_size: u64,
    out: alloc::vec::Vec<u8>,
}

#[cfg(feature = "std")]
impl Rc {
    fn new() -> Self {
        Self {
            low: 0,
            range: 0xffff_ffff,
            cache: 0,
            cache_size: 1,
            out: alloc::vec::Vec::new(),
        }
    }
    fn shift_low(&mut self) {
        if (self.low as u32) < 0xff00_0000 || (self.low >> 32) != 0 {
            let carry = (self.low >> 32) as u8;
            let mut temp = self.cache;
            loop {
                self.out.push(temp.wrapping_add(carry));
                temp = 0xff;
                self.cache_size -= 1;
                if self.cache_size == 0 {
                    break;
                }
            }
            self.cache = ((self.low >> 24) & 0xff) as u8;
        }
        self.cache_size += 1;
        self.low = (self.low & 0x00ff_ffff) << 8;
    }
    fn bit(&mut self, m: &mut P, bit: bool, shift: u32, fast: bool) {
        let bound = (self.range >> 12) * m.p as u32;
        let s = if fast {
            (1 + m.n as u32).min(shift)
        } else {
            shift
        };
        if !bit {
            self.range = bound;
            m.p += (4096 - m.p) >> s;
        } else {
            self.low += bound as u64;
            self.range -= bound;
            m.p -= m.p >> s;
        }
        m.n = m.n.saturating_add(1);
        m.p = m.p.clamp(31, 4065);
        while self.range < 1 << 24 {
            self.range <<= 8;
            self.shift_low();
        }
    }
    fn finish(mut self) -> usize {
        let hi = self.low + self.range as u64;
        for k in (0..=32).rev() {
            let mask = (1u64 << k) - 1;
            let x = (self.low + mask) & !mask;
            if x < hi {
                self.low = x;
                break;
            }
        }
        for _ in 0..5 {
            self.shift_low();
        }
        let mut out = self.out;
        out.remove(0);
        while out.last() == Some(&0) {
            out.pop();
        }
        out.len()
    }
}

/// Map bytes of a unit's flags (`splits`, merged parents `roles != 0`)
/// under variant `v` (merges coded with record 21's model as in every
/// variant, counted separately by the caller): 0 the SPEC 2.11.1 grammar;
/// 1 a 3-bit plane mask first (planes without a split have no flags); 2 as
/// 1 with contexts (plane, left, up) only; 3 as 1 with flags starting at
/// p(0) = 7/8; 4 as 1 with fast first updates; 5 as 3 + 4; 6 as 5 with
/// contexts (plane, left + up, luma).
#[cfg(feature = "std")]
fn flag_bytes(v: u8, roles: &[u8], splits: &[u8], rows: usize, cols: usize) -> usize {
    let mut rc = Rc::new();
    let init = if matches!(v, 3 | 5 | 6) { 3584 } else { 2048 };
    let fast = matches!(v, 4..=6);
    let mut m = [P { p: init, n: 0 }; 54];
    let mut mask = 7u8;
    if v >= 1 {
        mask = splits.iter().fold(0, |a, &f| a | f);
        let mut raw = P { p: 2048, n: 0 };
        for c in 0..3 {
            rc.bit(&mut raw, mask >> c & 1 != 0, 0, false);
            raw.p = 2048;
        }
    }
    let class = |b: usize, c: usize| {
        if roles[b] != 0 {
            2
        } else {
            (splits[b] >> c & 1) as usize
        }
    };
    for r in 0..rows {
        for x in 0..cols {
            let b = r * cols + x;
            if roles[b] != 0 {
                continue;
            }
            for c in 0..3 {
                if mask >> c & 1 == 0 {
                    continue;
                }
                let left = if x > 0 { class(b - 1, c) } else { 2 };
                let up = if r > 0 { class(b - cols, c) } else { 2 };
                let luma = if c == 0 { 0 } else { (splits[b] & 1) as usize };
                let ctx = match v {
                    2 => (c * 3 + left) * 3 + up,
                    6 => (c * 5 + left + up) * 2 + luma,
                    _ => ((c * 3 + left) * 3 + up) * 2 + luma,
                };
                rc.bit(&mut m[ctx], splits[b] >> c & 1 != 0, 5, fast);
            }
        }
    }
    rc.finish()
}

/// Variants 7..=10 code a flag only where the plane's unit has AC (`ac`,
/// bit c per parent; a plane without AC decodes the same split or not), its
/// neighbours without a flag counting as 0: 7 with variant 0's coder, 8
/// with variant 5's (p(0) = 7/8 start, fast first updates), 9 as 8 with
/// the plane mask, 10 as 8 with contexts (plane, left + up, luma).
#[cfg(feature = "std")]
fn flag_bytes_ac(v: u8, roles: &[u8], splits: &[u8], ac: &[u8], rows: usize, cols: usize) -> usize {
    let mut rc = Rc::new();
    let (init, fast) = if v == 7 { (2048, false) } else { (3584, true) };
    let mut m = [P { p: init, n: 0 }; 54];
    let mut mask = 7u8;
    if v == 9 {
        mask = splits.iter().fold(0, |a, &f| a | f);
        let mut raw = P { p: 2048, n: 0 };
        for c in 0..3 {
            rc.bit(&mut raw, mask >> c & 1 != 0, 0, false);
            raw.p = 2048;
        }
    }
    let class = |b: usize, c: usize| {
        if roles[b] != 0 {
            2
        } else {
            (splits[b] >> c & ac[b] >> c & 1) as usize
        }
    };
    for r in 0..rows {
        for x in 0..cols {
            let b = r * cols + x;
            if roles[b] != 0 {
                continue;
            }
            for c in 0..3 {
                if mask >> c & 1 == 0 || ac[b] >> c & 1 == 0 {
                    continue;
                }
                let left = if x > 0 { class(b - 1, c) } else { 2 };
                let up = if r > 0 { class(b - cols, c) } else { 2 };
                let luma = if c == 0 {
                    0
                } else {
                    (splits[b] & ac[b] & 1) as usize
                };
                let ctx = match v {
                    10 => (c * 5 + left + up) * 2 + luma,
                    _ => ((c * 3 + left) * 3 + up) * 2 + luma,
                };
                rc.bit(&mut m[ctx], splits[b] >> c & 1 != 0, 5, fast);
            }
        }
    }
    rc.finish()
}

/// LAB: the flag-section bytes of a small-transform file's maps under the
/// grammar variants of [`flag_bytes`] (0..=6) and [`flag_bytes_ac`]
/// (7..=10), summed over its units, with the decoded decisions (one column
/// group).
#[cfg(feature = "std")]
pub(crate) fn map_variants(data: &[u8]) -> Result<[u64; 11], crate::Error> {
    let p = super::parse_lossy(data)?;
    let mut out = [0u64; 11];
    if p.map != super::MapKind::Joint {
        return Ok(out);
    }
    let (bw, _, coefs) = {
        let q = super::quantized_lab_any(data)?;
        (q.0, q.1, q.2)
    };
    let ng = p.ngroups();
    let (mut roles, mut splits) = (alloc::vec::Vec::new(), alloc::vec::Vec::new());
    for (u, &rec) in p.stripes.iter().enumerate() {
        let (s, gi) = (u / ng, u % ng);
        let (_, gg) = p.group(gi);
        let (a, b) = gg.rows(s);
        let (rows, cols) = (b - a, gg.bw);
        let (q, eobs, _, _) = super::decode_stripe_raw(
            &p,
            u,
            super::Group::ALL,
            false,
            alloc::vec::Vec::new(),
            alloc::vec::Vec::new(),
        )?;
        super::superblock::read_joint_map(
            rec,
            (rows, cols),
            (&[], false),
            (&q, &eobs),
            &mut roles,
            &mut splits,
        )?;
        if splits.is_empty() || splits.iter().all(|&f| f == 0) {
            continue;
        }
        if roles.is_empty() {
            roles.resize(rows * cols, 0);
        }
        for v in 0..7u8 {
            out[v as usize] += flag_bytes(v, &roles, &splits, rows, cols) as u64;
        }
        let ac: alloc::vec::Vec<u8> = (0..rows * cols)
            .map(|i| {
                let blk = (a + i / cols) * bw + i % cols;
                (0..3).fold(0u8, |m, c| {
                    let u = &coefs[(blk * 3 + c) * 64..][..64];
                    m | (u[1..].iter().any(|&v| v != 0) as u8) << c
                })
            })
            .collect();
        for v in 7..11u8 {
            out[v as usize] += flag_bytes_ac(v, &roles, &splits, &ac, rows, cols) as u64;
        }
    }
    Ok(out)
}

#[cfg(all(test, feature = "std"))]
mod portable_rounding_tests {
    #[test]
    fn no_std_rounding_keeps_native_fixed_conversion_at_ties_extremes_and_raw_bits() {
        let check = |x: f32| {
            let clamped = x.clamp(-2_147_483_520.0, 2_147_483_520.0);
            let portable = crate::mathx::RoundTiesEven::round_ties_even(clamped) as i32;
            assert_eq!(
                portable,
                super::to_fixed(x),
                "input bits {:08x}",
                x.to_bits()
            );
        };
        for n in -8192..8192 {
            check(n as f32 + 0.5);
        }
        for x in [
            f32::NEG_INFINITY,
            -f32::MAX,
            -2_147_483_520.0,
            -0.0,
            0.0,
            f32::from_bits(1),
            f32::MIN_POSITIVE,
            2_147_483_520.0,
            f32::MAX,
            f32::INFINITY,
            f32::NAN,
        ] {
            check(x);
        }
        let mut bits = 0xa13f_c729u32;
        for _ in 0..500_000 {
            bits ^= bits << 13;
            bits ^= bits >> 17;
            bits ^= bits << 5;
            check(f32::from_bits(bits));
        }
    }
}
