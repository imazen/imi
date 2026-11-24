//! Small transforms (feature `small_transforms`, SPEC 2.11): a color plane
//! of an 8x8 parent coded as four 4x4 DCTs.
//!
//! * **Coefficients.** Tile (ty, tx) (rows 4 ty.., columns 4 tx..) has the
//!   orthonormal 4x4 DCT coefficients `C[fv][fu]`. Its coefficient (fv, fu)
//!   travels in the parent's raster slot (2 fv + ty, 2 fu + tx), so the
//!   slots interleave the four tiles by frequency and slot (v, u) carries
//!   about the physical frequency (v, u) of the 8x8 grid. The four tile DCs
//!   go through an orthonormal 2x2 Haar transform: slot 0 holds its LL (the
//!   parent's DC, in the nested DC image, the same value an 8x8 block of
//!   the same pixels has), slots 1, 8 and 9 its horizontal, vertical and
//!   diagonal details. Entropy coding, contexts and lanes are unchanged.
//! * **Quantization.** Slot (v, u) takes the 8x8 step of its physical
//!   frequency: `T[v & 6][u & 6]`, the Haar slots `T[v][u]` ([`steps4`]).
//!   One step costs the same error energy as in an 8x8 block (orthonormal
//!   transforms). The AC multiplier (AQ) scales every slot but the DC; the
//!   reconstruction bias does not apply.
//! * **Inverse** ([`inverse_ref`], normative): the Haar butterflies, then a
//!   4-point pass down every tile column, then along every tile row; each
//!   pass `e0 = v0 + v2, e1 = v0 - v2, o0 = v1 + v3 T, o1 = v1 T - v3`,
//!   outputs `(e0 + o0, e1 + o1, e1 - o1, e0 - o0)`, two multiplies and
//!   eight adds; separate f32 operations in this order, no fused
//!   multiply-add. The passes' input scales and the Haar's 1/2 are folded
//!   into the dequantization factors ([`P4`], [`dequant4`]), as the 8x8
//!   AAN scales are.
//!
//! Every SIMD tier runs the same operations lane by lane: with the
//! interleaved slots a column pass is lane-wise over rows (2 k + ty) and a
//! row pass lane-wise over the transposed columns (2 k + tx), so no tier
//! permutes coefficients ([`block4`]).

#[cfg(all(test, feature = "encode"))]
use super::dct;
use magetypes::simd::backends::F32x8Backend;

/// Lossy head record id of small transforms (critical, empty payload;
/// feature `small_transforms`): every unit record starts with a joint map
/// of superblock merges and 4x4 split flags.
pub(crate) const H_PARTITIONS: u64 = crate::registry::HEAD_SMALL_TRANSFORMS;
use magetypes::simd::generic::f32x8 as vf8;

/// `tan(pi / 8)`, rounded to f32: the 4-point pass's rotation ratio.
pub(crate) const T4: f32 = f32::from_bits(0x3ed4_13cd);

/// `cos(pi / 8) / sqrt 2`, rounded to f32 (exactly half of the 8x8
/// transform's `AAN[2]`): the input scale of odd frequencies of a 4-point
/// axis; even frequencies take 1/2.
pub(crate) const S4_ODD: f32 = f32::from_bits(0x3f27_3d75);

const P_EE: f32 = 0.25;
/// `S4_ODD / 2` (exact).
const P_EO: f32 = S4_ODD * 0.5;
/// `cos^2(pi / 8) / 2`, rounded to f32 from the exact value.
const P_OO: f32 = f32::from_bits(0x3eda_827a);
/// The Haar slots: the Haar's 1/2 times the DC scale 1/4 of both axes.
const P_DC: f32 = 0.125;

/// Input scale of every raster slot of a 4x4 parent: `P_DC` in the Haar
/// slots 0, 1, 8 and 9, else `S4[v >> 1] * S4[u >> 1]` (1/2 for even,
/// [`S4_ODD`] for odd tile frequencies), pinned as f32 values.
pub(crate) const P4: [f32; 64] = {
    let mut p = [0.0f32; 64];
    let mut i = 0;
    while i < 64 {
        let (v, u) = (i / 8, i % 8);
        p[i] = if v < 2 && u < 2 {
            P_DC
        } else {
            match ((v >> 1) & 1, (u >> 1) & 1) {
                (0, 0) => P_EE,
                (1, 1) => P_OO,
                _ => P_EO,
            }
        };
        i += 1;
    }
    p
};

/// The 8x8 step-table entry of raster slot `i` of a 4x4 parent (its
/// physical frequency): `(v, u)` for the Haar slots, else `(v & 6, u & 6)`.
#[inline]
pub(crate) const fn phys(i: usize) -> usize {
    let (v, u) = (i / 8, i % 8);
    if v < 2 && u < 2 {
        i
    } else {
        (v & 6) * 8 + (u & 6)
    }
}

/// Quantizer steps of a 4x4 parent's slots from the plane's 8x8 steps `t`
/// (raster; the DC entry includes the DC multiplier).
pub(crate) fn steps4(t: &[f32; 64]) -> [f32; 64] {
    core::array::from_fn(|i| t[phys(i)])
}

/// Dequantization factors of a 4x4 parent: `step * P4`, one f32 multiply.
/// Slot 0 equals the 8x8 factor of the DC (`T[0] * 0.125`), so a 4x4 plane
/// without AC decodes to the 8x8 flat value.
pub(crate) fn dequant4(t: &[f32; 64]) -> [f32; 64] {
    let s = steps4(t);
    core::array::from_fn(|i| s[i] * P4[i])
}

/// One 4-point pass over `(v0, v1, v2, v3)`, the normative order.
#[inline(always)]
pub(crate) fn pass4_ref(v0: f32, v1: f32, v2: f32, v3: f32) -> [f32; 4] {
    let e0 = v0 + v2;
    let e1 = v0 - v2;
    let o0 = v1 + v3 * T4;
    let o1 = v1 * T4 - v3;
    [e0 + o0, e1 + o1, e1 - o1, e0 - o0]
}

/// The tile DCs from the Haar slots of `x` (0 = LL, 1 = H, 8 = V, 9 = D),
/// written back to slots 0 (top left), 1 (top right), 8 (bottom left) and
/// 9 (bottom right): columns of the 2x2 Haar first, then its rows.
#[inline(always)]
pub(crate) fn haar_ref(x0: f32, x1: f32, x8: f32, x9: f32) -> [f32; 4] {
    let a = x0 + x8;
    let b = x0 - x8;
    let c = x1 + x9;
    let d = x1 - x9;
    [a + c, a - c, b + d, b - d]
}

/// The normative inverse of one plane of a 4x4 parent: `x[i]` holds the
/// dequantized coefficient of raster slot i (`q * factor`, [`dequant4`]
/// times the AC multiplier off the DC); on return `x[y * 8 + c]` holds the
/// pixel at row y, column c.
pub(crate) fn inverse_ref(x: &mut [f32; 64]) {
    let [tl, tr, bl, br] = haar_ref(x[0], x[1], x[8], x[9]);
    x[0] = tl;
    x[1] = tr;
    x[8] = bl;
    x[9] = br;
    let mut y = [0.0f32; 64];
    for u in 0..8 {
        for ty in 0..2 {
            let r = pass4_ref(
                x[ty * 8 + u],
                x[(2 + ty) * 8 + u],
                x[(4 + ty) * 8 + u],
                x[(6 + ty) * 8 + u],
            );
            for n in 0..4 {
                y[(ty * 4 + n) * 8 + u] = r[n];
            }
        }
    }
    for row in 0..8 {
        for tx in 0..2 {
            let o = row * 8;
            let r = pass4_ref(y[o + tx], y[o + 2 + tx], y[o + 4 + tx], y[o + 6 + tx]);
            for n in 0..4 {
                x[o + tx * 4 + n] = r[n];
            }
        }
    }
}

/// `cos(pi / 8) / sqrt 2` and `sin(pi / 8) / sqrt 2` (encoder).
#[cfg(feature = "encode")]
const F1: f32 = 0.653_281_5;
#[cfg(feature = "encode")]
const F3: f32 = 0.270_598_05;

/// The orthonormal 4-point DCT-II of `(x0, x1, x2, x3)` (encoder; f32, any
/// rounding): sums and differences, then a rotation.
#[inline(always)]
#[cfg(feature = "encode")]
fn fdct4(x0: f32, x1: f32, x2: f32, x3: f32) -> [f32; 4] {
    let (s0, s1) = (x0 + x3, x1 + x2);
    let (d0, d1) = (x0 - x3, x1 - x2);
    [
        (s0 + s1) * 0.5,
        d0 * F1 + d1 * F3,
        (s0 - s1) * 0.5,
        d0 * F3 - d1 * F1,
    ]
}

/// The orthonormal coefficients of one plane of an 8x8 parent (raster
/// pixels) in the 4x4 slots (encoder; f32, any rounding): the tiles' 4x4
/// DCTs and the Haar of their DCs (slot 0 is the parent's DC, equal to its
/// 8x8 DC).
#[cfg(feature = "encode")]
pub(crate) fn forward(px: &[f32; 64]) -> [f32; 64] {
    let mut h = [0.0f32; 64];
    // Rows: tile column tx, frequency fu -> slot column 2 fu + tx.
    for y in 0..8 {
        let r = &px[y * 8..y * 8 + 8];
        for tx in 0..2 {
            let o = tx * 4;
            let c = fdct4(r[o], r[o + 1], r[o + 2], r[o + 3]);
            for (fu, v) in c.into_iter().enumerate() {
                h[y * 8 + 2 * fu + tx] = v;
            }
        }
    }
    let mut out = [0.0f32; 64];
    for u in 0..8 {
        for ty in 0..2 {
            let o = ty * 4 * 8 + u;
            let c = fdct4(h[o], h[o + 8], h[o + 16], h[o + 24]);
            for (fv, v) in c.into_iter().enumerate() {
                out[(2 * fv + ty) * 8 + u] = v;
            }
        }
    }
    // Haar of the tile DCs (slots 0, 1, 8, 9 = TL, TR, BL, BR).
    let (tl, tr, bl, br) = (out[0], out[1], out[8], out[9]);
    out[0] = (tl + tr + bl + br) * 0.5;
    out[1] = (tl - tr + bl - br) * 0.5;
    out[8] = (tl + tr - bl - br) * 0.5;
    out[9] = (tl - tr - bl + br) * 0.5;
    out
}

/// The encoder's 4x4 forward: [`forward`], or in lab builds the integer
/// candidate's basis (`SmallLab::int`, `small_lab.rs`).
#[inline]
#[cfg(feature = "encode")]
pub(crate) fn forward_cfg(cfg: &super::LossyConfig, px: &[f32; 64]) -> [f32; 64] {
    #[cfg(feature = "lab")]
    if cfg.lab.small.as_ref().is_some_and(|s| s.int) {
        return super::small_lab::forward_int(px);
    }
    let _ = cfg;
    forward(px)
}

// ---- SIMD (every tier: the operations of `inverse_ref`, lane-wise) -----

/// [`pass4_ref`] lane-wise.
#[inline(always)]
pub(crate) fn pass4<T: F32x8Backend>(
    t4: vf8<T>,
    v0: vf8<T>,
    v1: vf8<T>,
    v2: vf8<T>,
    v3: vf8<T>,
) -> [vf8<T>; 4] {
    let e0 = v0 + v2;
    let e1 = v0 - v2;
    let o0 = v1 + v3 * t4;
    let o1 = v1 * t4 - v3;
    [e0 + o0, e1 + o1, e1 - o1, e0 - o0]
}

/// Lane masks of the 4x4 kernel: lane 0, and lanes 0 and 1.
#[derive(Clone, Copy)]
pub(crate) struct Masks<T: F32x8Backend> {
    m0: vf8<T>,
    m01: vf8<T>,
    t4: vf8<T>,
}

impl<T: F32x8Backend> Masks<T> {
    #[inline(always)]
    pub(crate) fn new(token: T) -> Self {
        let idx = vf8::from_array_t(token, [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
        Self {
            m0: idx.simd_lt(vf8::splat_t(token, 0.5)),
            m01: idx.simd_lt(vf8::splat_t(token, 1.5)),
            t4: vf8::splat_t(token, T4),
        }
    }
}

/// One plane of a 4x4 parent: `v[y]` holds raster row y of the dequantized
/// coefficients (`q * factor`); `haar` the tile DCs of [`haar_ref`] from
/// the same products. Returns the pixel rows.
#[inline(always)]
pub(crate) fn block4<T: F32x8Backend>(
    token: T,
    k: &Masks<T>,
    mut v: [vf8<T>; 8],
    haar: [f32; 4],
) -> [vf8<T>; 8] {
    let [tl, tr, bl, br] = haar;
    let top = vf8::blend(k.m0, vf8::splat_t(token, tl), vf8::splat_t(token, tr));
    let bot = vf8::blend(k.m0, vf8::splat_t(token, bl), vf8::splat_t(token, br));
    v[0] = vf8::blend(k.m01, top, v[0]);
    v[1] = vf8::blend(k.m01, bot, v[1]);
    let a = pass4(k.t4, v[0], v[2], v[4], v[6]);
    let b = pass4(k.t4, v[1], v[3], v[5], v[7]);
    let mut s = [a[0], a[1], a[2], a[3], b[0], b[1], b[2], b[3]];
    vf8::transpose_8x8(&mut s);
    let a = pass4(k.t4, s[0], s[2], s[4], s[6]);
    let b = pass4(k.t4, s[1], s[3], s[5], s[7]);
    let mut o = [a[0], a[1], a[2], a[3], b[0], b[1], b[2], b[3]];
    vf8::transpose_8x8(&mut o);
    o
}

/// The Haar inputs of a 4x4 plane from its quantized slots `q` and factors
/// `d` (the same products the vector rows hold in those lanes).
#[inline(always)]
pub(crate) fn haar_of(q: &[i32], d: &[f32; 64]) -> [f32; 4] {
    haar_ref(
        q[0] as f32 * d[0],
        q[1] as f32 * d[1],
        q[8] as f32 * d[8],
        q[9] as f32 * d[9],
    )
}

/// The 8x8 DC dequantization factor equals slot 0's ([`dequant4`]).
#[cfg(all(test, feature = "encode"))]
fn dc_factor_matches(t: &[f32; 64]) -> bool {
    dequant4(t)[0].to_bits() == dct::dequant_factors(t)[0].to_bits()
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

    #[test]
    fn constants_are_the_rounded_exact_values() {
        let pi = core::f64::consts::PI;
        assert_eq!(T4, (pi / 8.0).tan() as f32);
        assert_eq!(S4_ODD, ((pi / 8.0).cos() / 2f64.sqrt()) as f32);
        assert_eq!(S4_ODD * 2.0, dct::AAN[2]);
        assert_eq!(P_EO, (((pi / 8.0).cos() / 2f64.sqrt()) * 0.5) as f32);
        assert_eq!(P_OO, ((pi / 8.0).cos().powi(2) / 2.0) as f32);
    }

    #[test]
    fn physical_frequency_steps_and_dc_factor() {
        let t: [f32; 64] = core::array::from_fn(|i| 1.0 + i as f32 * 0.37);
        let s = steps4(&t);
        assert_eq!(s[0], t[0]);
        assert_eq!(s[1], t[1]);
        assert_eq!(s[8], t[8]);
        assert_eq!(s[9], t[9]);
        // Tile frequency (1, 2) of every tile: slots (2 + ty, 4 + tx).
        for ty in 0..2 {
            for tx in 0..2 {
                assert_eq!(s[(2 + ty) * 8 + 4 + tx], t[2 * 8 + 4]);
            }
        }
        assert!(dc_factor_matches(&t));
    }

    /// The f64 orthonormal 4-point basis.
    fn c4(k: usize, n: usize) -> f64 {
        let ck = if k == 0 {
            0.5
        } else {
            core::f64::consts::FRAC_1_SQRT_2
        };
        ck * ((k * (2 * n + 1)) as f64 * core::f64::consts::PI / 8.0).cos()
    }

    /// Forward then the reference inverse (dequantized with unit steps)
    /// reproduces random parents; slot 0 is the 8x8 DC.
    #[test]
    fn reference_inverts_the_forward_transform() {
        let mut seed = 0x5bd1_e995u32;
        let ones = [1.0f32; 64];
        let p = dequant4(&ones);
        for _ in 0..500 {
            let px: [f32; 64] =
                core::array::from_fn(|_| (xorshift(&mut seed) % 2001) as f32 / 4.0 - 250.0);
            let c = forward(&px);
            let dc8: f32 = px.iter().sum::<f32>() / 8.0;
            assert!((c[0] - dc8).abs() < 1e-2, "{} vs {dc8}", c[0]);
            let mut x: [f32; 64] = core::array::from_fn(|i| c[i] * p[i]);
            inverse_ref(&mut x);
            for i in 0..64 {
                assert!((x[i] - px[i]).abs() < 2e-3, "{i}: {} vs {}", x[i], px[i]);
            }
        }
    }

    /// The forward against direct f64 sums of the basis functions.
    #[test]
    fn forward_matches_the_f64_basis() {
        let mut seed = 0x0123_4567u32;
        for _ in 0..200 {
            let px: [f32; 64] =
                core::array::from_fn(|_| (xorshift(&mut seed) % 2001) as f32 / 4.0 - 250.0);
            let got = forward(&px);
            for v in 0..8 {
                for u in 0..8 {
                    if v < 2 && u < 2 {
                        continue;
                    }
                    let (fv, ty, fu, tx) = (v >> 1, v & 1, u >> 1, u & 1);
                    let mut want = 0.0f64;
                    for r in 0..4 {
                        for c in 0..4 {
                            want +=
                                px[(ty * 4 + r) * 8 + tx * 4 + c] as f64 * c4(fv, r) * c4(fu, c);
                        }
                    }
                    let g = got[v * 8 + u] as f64;
                    assert!((g - want).abs() < 2e-3, "({v}, {u}): {g} vs {want}");
                }
            }
        }
    }

    /// The reference against direct f64 sums of the basis functions, slot
    /// by slot.
    #[test]
    fn reference_matches_the_f64_basis() {
        let ones = [1.0f32; 64];
        let p = dequant4(&ones);
        for slot in 0..64 {
            let mut x = [0.0f32; 64];
            x[slot] = 64.0;
            let mut y = x;
            for i in 0..64 {
                y[i] = x[i] * p[i];
            }
            inverse_ref(&mut y);
            let (v, u) = (slot / 8, slot % 8);
            for r in 0..8 {
                for c in 0..8 {
                    let want = if v < 2 && u < 2 {
                        // Haar basis over the four tiles, each tile constant
                        // (its DC basis is 1/4).
                        let (tyy, txx) = (r / 4, c / 4);
                        let hv = if v == 0 || tyy == 0 { 1.0 } else { -1.0 };
                        let hu = if u == 0 || txx == 0 { 1.0 } else { -1.0 };
                        64.0 * 0.5 * hv * hu * 0.25
                    } else {
                        let (fv, ty, fu, tx) = (v >> 1, v & 1, u >> 1, u & 1);
                        if r / 4 != ty || c / 4 != tx {
                            0.0
                        } else {
                            64.0 * c4(fv, r % 4) * c4(fu, c % 4)
                        }
                    };
                    let got = y[r * 8 + c] as f64;
                    assert!(
                        (got - want).abs() < 1e-4,
                        "slot {slot} ({r}, {c}): {got} vs {want}"
                    );
                }
            }
        }
    }
}

#[cfg(all(test, feature = "encode"))]
mod kernel_tests {
    use super::super::simd::{Small, recon_block_row};
    use super::super::{NCH, dct};
    use super::*;

    fn xorshift(seed: &mut u32) -> u32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        *seed
    }

    /// The block-row kernel (8x8 and 4x4 planes mixed at random, flat and
    /// dense blocks, AQ, the reconstruction bias on 8x8 planes) equals the
    /// scalar references bit for bit on the dispatched tier, AVX2 (on
    /// AVX-512 hosts) and the scalar instantiation; zeroes the coefficients.
    #[test]
    fn block_row_kernel_matches_the_references() {
        let mut seed = 0x6a09_e667u32;
        let t: [f32; 64] = core::array::from_fn(|i| 1.5 + i as f32 * 0.3);
        let dq8 = [dct::dequant_factors(&t); NCH];
        let dq4 = [dequant4(&t); NCH];
        for trial in 0..200 {
            let bw = 1 + xorshift(&mut seed) as usize % 17;
            let mut q = alloc::vec![0i32; bw * NCH * 64 + 64];
            let mut eobs = alloc::vec![0u8; bw * NCH + 1];
            // Half the rows mostly plain: runs of blocks without 4x4 planes
            // (AVX-512 hosts send runs of two or more to the 16-lane kernel).
            let split: alloc::vec::Vec<u8> = (0..bw)
                .map(|_| {
                    let v = (xorshift(&mut seed) % 8) as u8;
                    if trial % 2 == 1 && !xorshift(&mut seed).is_multiple_of(4) {
                        0
                    } else {
                        v
                    }
                })
                .collect();
            for b in 0..bw {
                for ch in 0..NCH {
                    let u = (b * NCH + ch) * 64;
                    q[u] = (xorshift(&mut seed) % 2001) as i32 - 1000;
                    // A third of the planes flat (no AC).
                    if !xorshift(&mut seed).is_multiple_of(3) {
                        for i in 1..64 {
                            if xorshift(&mut seed).is_multiple_of(3) {
                                q[u + i] = (xorshift(&mut seed) % 61) as i32 - 30;
                            }
                        }
                        eobs[b * NCH + ch] = 63;
                    }
                }
            }
            let aq: alloc::vec::Vec<f32> = if trial % 2 == 0 {
                alloc::vec::Vec::new()
            } else {
                (0..bw)
                    .map(|_| (32 + xorshift(&mut seed) % 96) as f32 / 64.0)
                    .collect()
            };
            let bias_t: [[[f32; 64]; 2]; 3] = core::array::from_fn(|_| {
                [
                    core::array::from_fn(|i| if i == 0 { 0.0 } else { 0.1 }),
                    core::array::from_fn(|i| if i == 0 { 0.0 } else { 0.05 }),
                ]
            });
            let bias = (trial % 3 == 0).then_some(&bias_t);
            let pw = bw * 8 + 16;
            // The scalar references.
            let mut want = alloc::vec![0.0f32; NCH * 8 * pw];
            for b in 0..bw {
                let m = aq.get(b).copied();
                for ch in 0..NCH {
                    let u = &q[(b * NCH + ch) * 64..][..64];
                    let four = split[b] >> ch & 1 != 0;
                    let d = super::super::aq_dequant(if four { &dq4[ch] } else { &dq8[ch] }, m);
                    let mut x: [f32; 64] = core::array::from_fn(|i| match bias {
                        Some(bt) if !four => {
                            let s = u[i].clamp(-1, 1);
                            (u[i] as f32 - s as f32 * bt[ch][(u[i] != s) as usize][i]) * d[i]
                        }
                        _ => u[i] as f32 * d[i],
                    });
                    if eobs[b * NCH + ch] == 0 {
                        x = [u[0] as f32 * d[0]; 64];
                    } else if four {
                        inverse_ref(&mut x);
                    } else {
                        dct::idct8x8(&mut x);
                    }
                    for y in 0..8 {
                        want[ch * 8 * pw + y * pw + b * 8..][..8]
                            .copy_from_slice(&x[y * 8..y * 8 + 8]);
                    }
                }
            }
            let small = Small {
                split: &split,
                dq: &dq4,
                int: false,
            };
            for tier in 0..3 {
                let mut qq = q.clone();
                let mut got = alloc::vec![f32::NAN; NCH * 8 * pw];
                match tier {
                    0 => recon_block_row(
                        &mut qq, &eobs, 0, bw, 0, bw, &dq8, &aq, bias, &mut got, pw, small, true,
                    ),
                    1 => {
                        // AVX2 on AVX-512 hosts (`IMIJ_NO_AVX512` is a process
                        // switch; the scalar instantiation stands in elsewhere).
                        super::super::simd::recon_block_row_tier(
                            1, &mut qq, &eobs, 0, bw, 0, bw, &dq8, &aq, bias, &mut got, pw, small,
                        )
                    }
                    _ => super::super::simd::recon_block_row_tier(
                        2, &mut qq, &eobs, 0, bw, 0, bw, &dq8, &aq, bias, &mut got, pw, small,
                    ),
                }
                assert!(
                    qq.iter().all(|&v| v == 0),
                    "trial {trial} tier {tier}: not cleared"
                );
                for ch in 0..NCH {
                    for y in 0..8 {
                        for x in 0..bw * 8 {
                            let i = ch * 8 * pw + y * pw + x;
                            assert!(
                                got[i].to_bits() == want[i].to_bits(),
                                "trial {trial} tier {tier} ch {ch} ({y}, {x}): {} vs {}",
                                got[i],
                                want[i]
                            );
                        }
                    }
                }
            }
        }
    }
}
