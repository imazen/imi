//! Relaxed-SIMD substitutions for the wasm128 kernel arms.
//!
//! Each helper here is a drop-in for one strict magetypes operation whose
//! relaxed-SIMD form (`+relaxed-simd`) is bit-identical **under this crate's
//! invariants**; the kernel bodies call them through
//! `incant!(... without token)`, which the `#[magetypes]` expansion rewrites
//! to the caller tier's `_<tier>` arm. Only the `_wasm128` arms change
//! behavior, and only in builds compiled with `target_feature =
//! "relaxed-simd"`: every other tier and every strict wasm build keeps the
//! exact instruction sequence it had before. The `_neon` arms are strict
//! `blend`s; the portable kernels (`rans_portable`, `recon_portable`) share
//! one body across the neon, wasm128 and scalar tiers and need the same
//! helper names for each. Nothing here is used by the encoder.
//!
//! # Preconditions (per helper)
//!
//! * `sel_*`: the mask lanes are **lane-uniform** (each lane is all-ones or
//!   all-zeros), because every caller produces them from vector comparisons
//!   (`simd_eq/ne/lt/le/gt/ge`, possibly combined with `&`, `|`, `not`).
//!   For such masks `*.relaxed_laneselect` is defined to equal
//!   `v128.bitselect` lane for lane, so the result is identical on every
//!   engine (the spec's implementation-defined behavior only applies to
//!   non-uniform lane masks).
//! * `clamp_f32x8`: intended for clamped decode paths whose operands are
//!   **finite** (decoded planes/linear values bounded by earlier clamps —
//!   NaN cannot be produced by the lossy decode arithmetic, see
//!   `docs/WASM_RELAXED_SIMD_2026-10-10.md`). For finite operands relaxed
//!   min/max agrees with the strict op except when the two operands are the
//!   pair `(-0.0, +0.0)`, where the spec allows either; every call site
//!   neutralizes that case downstream (the `+2^23` rounding trick maps both
//!   -0 and +0 to `+0`, and the XYB LUT index clamps both to slot 0). The
//!   per-site tests in this file and in `lossy/simd.rs` pin this down.
//! * `to_i32x8`: the operand is already clamped into `[0, 255]` /
//!   `[0, 65535]` (or the 2^23 round trick has run), i.e. exactly
//!   representable and in range, so relaxed truncation equals the saturating
//!   strict conversion (NaN and out-of-range lanes, the only inputs where the
//!   relaxed op is implementation-defined, cannot occur).
//!
//! Verdicts, measurements and the instruction-level audit are in
//! `docs/WASM_RELAXED_SIMD_2026-10-10.md`; the durable home for the winning
//! substitutions is a magetypes method (archmage repo).

use magetypes::simd::generic::{i16x16, i32x8, u16x16, u32x8};

// ===== Lane select: relaxed_laneselect for uniform masks =====

/// `sel_f32x8_*`: lane-select `m ? a : b` (mask lanes all-ones/all-zeros).
#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub(crate) fn sel_f32x8_v3(
    _token: archmage::X64V3Token,
    m: magetypes::simd::generic::f32x8<archmage::X64V3Token>,
    a: magetypes::simd::generic::f32x8<archmage::X64V3Token>,
    b: magetypes::simd::generic::f32x8<archmage::X64V3Token>,
) -> magetypes::simd::generic::f32x8<archmage::X64V3Token> {
    magetypes::simd::generic::f32x8::blend(m, a, b)
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub(crate) fn sel_f32x8_neon(
    _token: archmage::NeonToken,
    m: magetypes::simd::generic::f32x8<archmage::NeonToken>,
    a: magetypes::simd::generic::f32x8<archmage::NeonToken>,
    b: magetypes::simd::generic::f32x8<archmage::NeonToken>,
) -> magetypes::simd::generic::f32x8<archmage::NeonToken> {
    magetypes::simd::generic::f32x8::blend(m, a, b)
}

#[cfg(target_arch = "wasm32")]
#[inline(always)]
pub(crate) fn sel_f32x8_wasm128(
    token: archmage::Wasm128Token,
    m: magetypes::simd::generic::f32x8<archmage::Wasm128Token>,
    a: magetypes::simd::generic::f32x8<archmage::Wasm128Token>,
    b: magetypes::simd::generic::f32x8<archmage::Wasm128Token>,
) -> magetypes::simd::generic::f32x8<archmage::Wasm128Token> {
    #[cfg(target_feature = "relaxed-simd")]
    {
        let (ma, aa, ba) = (m.into_repr(), a.into_repr(), b.into_repr());
        magetypes::simd::generic::f32x8::from_repr_t(
            token,
            [
                core::arch::wasm32::i32x4_relaxed_laneselect(aa[0], ba[0], ma[0]),
                core::arch::wasm32::i32x4_relaxed_laneselect(aa[1], ba[1], ma[1]),
            ],
        )
    }
    #[cfg(not(target_feature = "relaxed-simd"))]
    {
        let _ = token;
        magetypes::simd::generic::f32x8::blend(m, a, b)
    }
}

#[inline(always)]
pub(crate) fn sel_f32x8_scalar(
    _token: archmage::ScalarToken,
    m: magetypes::simd::generic::f32x8<archmage::ScalarToken>,
    a: magetypes::simd::generic::f32x8<archmage::ScalarToken>,
    b: magetypes::simd::generic::f32x8<archmage::ScalarToken>,
) -> magetypes::simd::generic::f32x8<archmage::ScalarToken> {
    magetypes::simd::generic::f32x8::blend(m, a, b)
}

/// `sel_u32x8_*`: lane-select `m ? a : b` (mask lanes all-ones/all-zeros).
#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub(crate) fn sel_u32x8_neon(
    _token: archmage::NeonToken,
    m: u32x8<archmage::NeonToken>,
    a: u32x8<archmage::NeonToken>,
    b: u32x8<archmage::NeonToken>,
) -> u32x8<archmage::NeonToken> {
    u32x8::blend(m, a, b)
}

#[cfg(target_arch = "wasm32")]
#[inline(always)]
pub(crate) fn sel_u32x8_wasm128(
    token: archmage::Wasm128Token,
    m: u32x8<archmage::Wasm128Token>,
    a: u32x8<archmage::Wasm128Token>,
    b: u32x8<archmage::Wasm128Token>,
) -> u32x8<archmage::Wasm128Token> {
    #[cfg(target_feature = "relaxed-simd")]
    {
        let (ma, aa, ba) = (m.into_repr(), a.into_repr(), b.into_repr());
        u32x8::from_repr_t(
            token,
            [
                core::arch::wasm32::i32x4_relaxed_laneselect(aa[0], ba[0], ma[0]),
                core::arch::wasm32::i32x4_relaxed_laneselect(aa[1], ba[1], ma[1]),
            ],
        )
    }
    #[cfg(not(target_feature = "relaxed-simd"))]
    {
        let _ = token;
        u32x8::blend(m, a, b)
    }
}

#[inline(always)]
pub(crate) fn sel_u32x8_scalar(
    _token: archmage::ScalarToken,
    m: u32x8<archmage::ScalarToken>,
    a: u32x8<archmage::ScalarToken>,
    b: u32x8<archmage::ScalarToken>,
) -> u32x8<archmage::ScalarToken> {
    u32x8::blend(m, a, b)
}

/// `sel_i32x8_*`: see [`sel_u32x8_wasm128`].
#[cfg(target_arch = "wasm32")]
#[inline(always)]
pub(crate) fn sel_i32x8_wasm128(
    token: archmage::Wasm128Token,
    m: i32x8<archmage::Wasm128Token>,
    a: i32x8<archmage::Wasm128Token>,
    b: i32x8<archmage::Wasm128Token>,
) -> i32x8<archmage::Wasm128Token> {
    #[cfg(target_feature = "relaxed-simd")]
    {
        let (ma, aa, ba) = (m.into_repr(), a.into_repr(), b.into_repr());
        i32x8::from_repr_t(
            token,
            [
                core::arch::wasm32::i32x4_relaxed_laneselect(aa[0], ba[0], ma[0]),
                core::arch::wasm32::i32x4_relaxed_laneselect(aa[1], ba[1], ma[1]),
            ],
        )
    }
    #[cfg(not(target_feature = "relaxed-simd"))]
    {
        let _ = token;
        i32x8::blend(m, a, b)
    }
}

#[inline(always)]
pub(crate) fn sel_i32x8_scalar(
    _token: archmage::ScalarToken,
    m: i32x8<archmage::ScalarToken>,
    a: i32x8<archmage::ScalarToken>,
    b: i32x8<archmage::ScalarToken>,
) -> i32x8<archmage::ScalarToken> {
    i32x8::blend(m, a, b)
}

/// `sel_i32x8_neon`: strict `blend` (see [`sel_u32x8_neon`]).
#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub(crate) fn sel_i32x8_neon(
    _token: archmage::NeonToken,
    m: i32x8<archmage::NeonToken>,
    a: i32x8<archmage::NeonToken>,
    b: i32x8<archmage::NeonToken>,
) -> i32x8<archmage::NeonToken> {
    i32x8::blend(m, a, b)
}

/// `sel_u16x16_*`: 16-bit lane select (mask lanes all-ones/all-zeros).
#[cfg(target_arch = "wasm32")]
#[inline(always)]
pub(crate) fn sel_u16x16_wasm128(
    token: archmage::Wasm128Token,
    m: u16x16<archmage::Wasm128Token>,
    a: u16x16<archmage::Wasm128Token>,
    b: u16x16<archmage::Wasm128Token>,
) -> u16x16<archmage::Wasm128Token> {
    #[cfg(target_feature = "relaxed-simd")]
    {
        let (ma, aa, ba) = (m.into_repr(), a.into_repr(), b.into_repr());
        u16x16::from_repr_t(
            token,
            [
                core::arch::wasm32::i16x8_relaxed_laneselect(aa[0], ba[0], ma[0]),
                core::arch::wasm32::i16x8_relaxed_laneselect(aa[1], ba[1], ma[1]),
            ],
        )
    }
    #[cfg(not(target_feature = "relaxed-simd"))]
    {
        let _ = token;
        u16x16::blend(m, a, b)
    }
}

#[inline(always)]
pub(crate) fn sel_u16x16_scalar(
    _token: archmage::ScalarToken,
    m: u16x16<archmage::ScalarToken>,
    a: u16x16<archmage::ScalarToken>,
    b: u16x16<archmage::ScalarToken>,
) -> u16x16<archmage::ScalarToken> {
    u16x16::blend(m, a, b)
}

/// `sel_u16x16_neon`: strict `blend`.
#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub(crate) fn sel_u16x16_neon(
    _token: archmage::NeonToken,
    m: u16x16<archmage::NeonToken>,
    a: u16x16<archmage::NeonToken>,
    b: u16x16<archmage::NeonToken>,
) -> u16x16<archmage::NeonToken> {
    u16x16::blend(m, a, b)
}

/// `sel_i16x16_*`: see [`sel_u16x16_wasm128`].
#[cfg(target_arch = "wasm32")]
#[inline(always)]
pub(crate) fn sel_i16x16_wasm128(
    token: archmage::Wasm128Token,
    m: i16x16<archmage::Wasm128Token>,
    a: i16x16<archmage::Wasm128Token>,
    b: i16x16<archmage::Wasm128Token>,
) -> i16x16<archmage::Wasm128Token> {
    #[cfg(target_feature = "relaxed-simd")]
    {
        let (ma, aa, ba) = (m.into_repr(), a.into_repr(), b.into_repr());
        i16x16::from_repr_t(
            token,
            [
                core::arch::wasm32::i16x8_relaxed_laneselect(aa[0], ba[0], ma[0]),
                core::arch::wasm32::i16x8_relaxed_laneselect(aa[1], ba[1], ma[1]),
            ],
        )
    }
    #[cfg(not(target_feature = "relaxed-simd"))]
    {
        let _ = token;
        i16x16::blend(m, a, b)
    }
}

#[inline(always)]
pub(crate) fn sel_i16x16_scalar(
    _token: archmage::ScalarToken,
    m: i16x16<archmage::ScalarToken>,
    a: i16x16<archmage::ScalarToken>,
    b: i16x16<archmage::ScalarToken>,
) -> i16x16<archmage::ScalarToken> {
    i16x16::blend(m, a, b)
}

/// `sel_i16x16_neon`: strict `blend`.
#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub(crate) fn sel_i16x16_neon(
    _token: archmage::NeonToken,
    m: i16x16<archmage::NeonToken>,
    a: i16x16<archmage::NeonToken>,
    b: i16x16<archmage::NeonToken>,
) -> i16x16<archmage::NeonToken> {
    i16x16::blend(m, a, b)
}

// ===== Float clamp: relaxed min/max for finite operands =====

/// `clamp_f32x8_*`: `min(max(v, lo), hi)` for finite operands (see module
/// docs for the -0/+0 neutralization contract).
#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub(crate) fn clamp_f32x8_v3(
    _token: archmage::X64V3Token,
    v: magetypes::simd::generic::f32x8<archmage::X64V3Token>,
    lo: magetypes::simd::generic::f32x8<archmage::X64V3Token>,
    hi: magetypes::simd::generic::f32x8<archmage::X64V3Token>,
) -> magetypes::simd::generic::f32x8<archmage::X64V3Token> {
    v.clamp(lo, hi)
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub(crate) fn clamp_f32x8_neon(
    _token: archmage::NeonToken,
    v: magetypes::simd::generic::f32x8<archmage::NeonToken>,
    lo: magetypes::simd::generic::f32x8<archmage::NeonToken>,
    hi: magetypes::simd::generic::f32x8<archmage::NeonToken>,
) -> magetypes::simd::generic::f32x8<archmage::NeonToken> {
    v.clamp(lo, hi)
}

#[cfg(target_arch = "wasm32")]
#[inline(always)]
pub(crate) fn clamp_f32x8_wasm128(
    token: archmage::Wasm128Token,
    v: magetypes::simd::generic::f32x8<archmage::Wasm128Token>,
    lo: magetypes::simd::generic::f32x8<archmage::Wasm128Token>,
    hi: magetypes::simd::generic::f32x8<archmage::Wasm128Token>,
) -> magetypes::simd::generic::f32x8<archmage::Wasm128Token> {
    #[cfg(target_feature = "relaxed-simd")]
    {
        let (vr, lor, hir) = (v.into_repr(), lo.into_repr(), hi.into_repr());
        let min = core::arch::wasm32::f32x4_relaxed_min;
        let max = core::arch::wasm32::f32x4_relaxed_max;
        magetypes::simd::generic::f32x8::from_repr_t(
            token,
            [
                min(max(vr[0], lor[0]), hir[0]),
                min(max(vr[1], lor[1]), hir[1]),
            ],
        )
    }
    #[cfg(not(target_feature = "relaxed-simd"))]
    {
        let _ = token;
        v.clamp(lo, hi)
    }
}

#[inline(always)]
pub(crate) fn clamp_f32x8_scalar(
    _token: archmage::ScalarToken,
    v: magetypes::simd::generic::f32x8<archmage::ScalarToken>,
    lo: magetypes::simd::generic::f32x8<archmage::ScalarToken>,
    hi: magetypes::simd::generic::f32x8<archmage::ScalarToken>,
) -> magetypes::simd::generic::f32x8<archmage::ScalarToken> {
    v.clamp(lo, hi)
}

// ===== Float to int: relaxed truncation for clamped operands =====

/// `to_i32x8_*`: truncating conversion for operands already in the i32
/// range (see module docs).
#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub(crate) fn to_i32x8_v3(
    _token: archmage::X64V3Token,
    v: magetypes::simd::generic::f32x8<archmage::X64V3Token>,
) -> i32x8<archmage::X64V3Token> {
    v.to_i32x8()
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub(crate) fn to_i32x8_neon(
    _token: archmage::NeonToken,
    v: magetypes::simd::generic::f32x8<archmage::NeonToken>,
) -> i32x8<archmage::NeonToken> {
    v.to_i32x8()
}

#[cfg(target_arch = "wasm32")]
#[inline(always)]
pub(crate) fn to_i32x8_wasm128(
    token: archmage::Wasm128Token,
    v: magetypes::simd::generic::f32x8<archmage::Wasm128Token>,
) -> i32x8<archmage::Wasm128Token> {
    #[cfg(target_feature = "relaxed-simd")]
    {
        let vr = v.into_repr();
        i32x8::from_repr_t(
            token,
            [
                core::arch::wasm32::i32x4_relaxed_trunc_f32x4(vr[0]),
                core::arch::wasm32::i32x4_relaxed_trunc_f32x4(vr[1]),
            ],
        )
    }
    #[cfg(not(target_feature = "relaxed-simd"))]
    {
        let _ = token;
        v.to_i32x8()
    }
}

#[inline(always)]
pub(crate) fn to_i32x8_scalar(
    _token: archmage::ScalarToken,
    v: magetypes::simd::generic::f32x8<archmage::ScalarToken>,
) -> i32x8<archmage::ScalarToken> {
    v.to_i32x8()
}

#[cfg(all(
    all(test, feature = "encode"),
    target_arch = "wasm32",
    target_feature = "relaxed-simd"
))]
mod tests {
    use super::*;
    use archmage::SimdToken;

    fn token() -> archmage::Wasm128Token {
        archmage::Wasm128Token::summon().expect("wasm128 compile-time feature")
    }

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    // Lane-select over the site domain: lane-uniform masks (all-ones or
    // all-zeros per lane, exactly what vector comparisons produce) and
    // arbitrary operand bits must equal v128.bitselect bit for bit.
    #[test]
    fn relaxed_laneselect_equals_bitselect_for_uniform_masks() {
        let t = token();
        let mut rng = Rng(0x9e3779b97f4a7c15);
        for round in 0..20_000 {
            let pick = |r: &mut Rng| {
                let a = r.next().to_le_bytes();
                let b = r.next().to_le_bytes();
                core::array::from_fn::<u32, 8, _>(|i| {
                    let bit = (a[i % 8] ^ b[i % 8]) & 1;
                    if bit == 0 { 0 } else { u32::MAX }
                })
            };
            let m = pick(&mut rng);
            let a: [u32; 8] = core::array::from_fn(|_| rng.next() as u32);
            let b: [u32; 8] = core::array::from_fn(|_| rng.next() as u32);
            let (mv, av, bv) = (
                u32x8::from_array_t(t, m),
                u32x8::from_array_t(t, a),
                u32x8::from_array_t(t, b),
            );
            assert_eq!(
                sel_u32x8_wasm128(t, mv, av, bv).to_array(),
                u32x8::blend(mv, av, bv).to_array(),
                "round {round}"
            );
            if round % 4 == 2 {
                let mf = magetypes::simd::generic::f32x8::from_repr_t(t, mv.into_repr());
                let af = magetypes::simd::generic::f32x8::from_repr_t(t, av.into_repr());
                let bf = magetypes::simd::generic::f32x8::from_repr_t(t, bv.into_repr());
                assert_eq!(
                    sel_f32x8_wasm128(t, mf, af, bf)
                        .to_array()
                        .map(f32::to_bits),
                    magetypes::simd::generic::f32x8::blend(mf, af, bf)
                        .to_array()
                        .map(f32::to_bits),
                    "f32 round {round}"
                );
            }
            if round % 4 == 0 {
                let (mi, ai, bi) = (
                    i32x8::from_repr_t(t, mv.into_repr()),
                    i32x8::from_repr_t(t, av.into_repr()),
                    i32x8::from_repr_t(t, bv.into_repr()),
                );
                assert_eq!(
                    sel_i32x8_wasm128(t, mi, ai, bi).to_array(),
                    i32x8::blend(mi, ai, bi).to_array(),
                    "i32 round {round}"
                );
            }
            if round % 4 == 1 {
                let m16: [u16; 16] =
                    core::array::from_fn(|_| if rng.next() & 1 == 0 { 0 } else { u16::MAX });
                let a16: [u16; 16] = core::array::from_fn(|_| rng.next() as u16);
                let b16: [u16; 16] = core::array::from_fn(|_| rng.next() as u16);
                let (mh, ah, bh) = (
                    u16x16::from_array_t(t, m16),
                    u16x16::from_array_t(t, a16),
                    u16x16::from_array_t(t, b16),
                );
                assert_eq!(
                    sel_u16x16_wasm128(t, mh, ah, bh).to_array(),
                    u16x16::blend(mh, ah, bh).to_array(),
                    "u16 round {round}"
                );
                let (mhi, ahi, bhi) = (
                    i16x16::from_repr_t(t, mh.into_repr()),
                    i16x16::from_repr_t(t, ah.into_repr()),
                    i16x16::from_repr_t(t, bh.into_repr()),
                );
                assert_eq!(
                    sel_i16x16_wasm128(t, mhi, ahi, bhi).to_array(),
                    i16x16::blend(mhi, ahi, bhi).to_array(),
                    "i16 round {round}"
                );
            }
        }
    }

    // Relaxed truncation over the clamped site domains: every finite value
    // in [0, 255] and [0, 65535], subnormals, ±0, the 2^23 rounding-trick
    // magnitudes, and random in-range bit patterns must equal the strict
    // saturating conversion.
    #[test]
    fn relaxed_trunc_equals_saturating_conversion_in_clamp_domain() {
        let t = token();
        let mut values: alloc::vec::Vec<f32> = alloc::vec![
            0.0f32,
            -0.0,
            1.0,
            254.0,
            255.0,
            256.0,
            65_534.0,
            65_535.0,
            65_536.0,
            f32::from_bits(1),
            -f32::from_bits(1),
            8_388_608.0,
            8_388_608.0 + 255.0,
            8_388_608.0 + 65_535.0,
            8_388_863.0,
        ];
        let mut rng = Rng(0x85ebca6b12345678);
        for _ in 0..200_000 {
            let v = f32::from_bits(rng.next() as u32);
            if v.is_finite() && v >= 0.0 && v <= 65_535.0 {
                values.push(v);
            }
        }
        assert!(
            values.len() > 10_000,
            "random domain too small: {}",
            values.len()
        );
        for chunk in values.chunks(8) {
            let mut buf = [0.0f32; 8];
            buf[..chunk.len()].copy_from_slice(chunk);
            let v = magetypes::simd::generic::f32x8::from_array_t(t, buf);
            let want = v.to_i32x8().to_array();
            let got = to_i32x8_wasm128(t, v).to_array();
            assert_eq!(got, want, "inputs {buf:?}");
        }
    }

    // The clamp pattern used at every routed site: for finite operands the
    // relaxed clamp equals the strict clamp except possibly on the
    // (-0.0, +0.0) pair, and there the downstream +2^23 round trick makes
    // the observable result identical. This test pins the full pattern, so
    // it covers the pair case exactly as the kernels see it.
    #[test]
    fn relaxed_clamp_matches_strict_with_magic_rounding() {
        let t = token();
        let big = 8_388_608.0f32;
        let candidates: [f32; 12] = [
            0.0,
            -0.0,
            f32::from_bits(1),
            -f32::from_bits(1),
            63.999_996,
            -63.999_996,
            64.0,
            -64.0,
            8_388_608.0,
            -8_388_608.0,
            3.402_823_5e38,
            -3.402_823_5e38,
        ];
        let bounds: [[f32; 2]; 5] = [
            [0.0, 255.0],
            [0.0, 65_535.0],
            [-64.0, 64.0],
            [0.0, 1.0],
            [-64.0, 64.000_004],
        ];
        let mut rng = Rng(0xdeadbeefcafef00d);
        for round in 0..50_000 {
            let mut buf = [0.0f32; 8];
            for v in buf.iter_mut() {
                *v = if round % 3 == 0 {
                    candidates[(rng.next() as usize) % candidates.len()]
                } else {
                    // Finite only: NaN cannot be produced by the decode
                    // arithmetic at any routed site (module docs).
                    let x = f32::from_bits(rng.next() as u32);
                    if x.is_finite() { x } else { 0.0 }
                };
            }
            for [lo, hi] in bounds {
                let v = magetypes::simd::generic::f32x8::from_array_t(t, buf);
                let lo8 = magetypes::simd::generic::f32x8::splat_t(t, lo);
                let hi8 = magetypes::simd::generic::f32x8::splat_t(t, hi);
                let strict = v.clamp(lo8, hi8);
                let relaxed = clamp_f32x8_wasm128(t, v, lo8, hi8);
                let norm = |x: magetypes::simd::generic::f32x8<archmage::Wasm128Token>| {
                    ((x + magetypes::simd::generic::f32x8::splat_t(t, big))
                        - magetypes::simd::generic::f32x8::splat_t(t, big))
                    .to_array()
                };
                assert_eq!(
                    norm(relaxed),
                    norm(strict),
                    "inputs {buf:?} bounds {lo}..{hi} round {round}"
                );
                // Outside the ±0 pair the raw clamp is bit-identical.
                let sv = strict.to_array();
                let rv = relaxed.to_array();
                for i in 0..8 {
                    if !(buf[i] == 0.0 && (lo == 0.0)) {
                        assert_eq!(rv[i].to_bits(), sv[i].to_bits(), "lane {i} inputs {buf:?}");
                    }
                }
            }
        }
    }
}
