//! Per-CPU choice between the AVX-512 and AVX2 entropy kernels, by timing
//! the real kernels once per process on a synthetic stripe: valid decode
//! tables built from a histogram shaped like real token statistics, and a
//! random word stream, so every decoded token is a valid draw from that
//! distribution. The winner depends on the core (on Zen 4, which runs
//! 512-bit operations as two halves, the AVX2 run-level kernel beat the
//! AVX-512 one by 7-15%; on Zen 5 the order was the reverse). Output is the
//! same whichever kernel runs.

#[cfg(feature = "std")]
use super::coef::{self, ALPHA, EOB_CTX, LossyEntropy, Tables};
#[cfg(feature = "std")]
use crate::rans::{NL, RANS_L, Words};
#[cfg(feature = "std")]
use alloc::vec;
#[cfg(feature = "std")]
use alloc::vec::Vec;

/// Run-level kernel: AVX-512 with hardware gathers, AVX-512 with software
/// lookups, or AVX2.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Rl {
    Avx512Hw,
    Avx512Sw,
    Avx2,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Choice {
    pub rl: Rl,
}

/// Synthetic decode inputs.
#[cfg(feature = "std")]
struct Synth {
    cmap: alloc::boxed::Box<[u8; coef::NRAW_PAD]>,
    cmap32: alloc::boxed::Box<[u32; coef::NRAW_PAD]>,
    dec: alloc::boxed::Box<[u32; coef::MAX_TABLES << crate::rans::LOG_M]>,
    bytes: Vec<u8>,
}

#[cfg(feature = "std")]
fn synth() -> Synth {
    let nraw = LossyEntropy::RunLevel.nraw();
    let mut hist = vec![vec![0u64; ALPHA]; nraw];
    for (ctx, h) in hist.iter_mut().enumerate() {
        if ctx < EOB_CTX {
            // EOB: many zeros (chroma), the rest spread.
            h[0] = 40 * 100;
            for (s, v) in h.iter_mut().enumerate().take(64).skip(1) {
                *v = 100 + (64 - s) as u64 * 3;
            }
        } else {
            // AC: run r, class c with weight 2^-r * 0.55^c; ZRL.
            for r in 0..=coef::MAX_RUN as usize {
                let mut w = 100_000.0 / (1u64 << r) as f64;
                for c in 0..coef::MAG_CLASSES as usize {
                    let s = r * 32 + c;
                    if coef::valid_ac_symbol(s) {
                        h[s] = w.max(1.0) as u64;
                    }
                    w *= 0.55;
                }
            }
            h[coef::ZRL as usize] = 5000;
        }
    }
    let t = Tables::build(&hist, 4, 16, LossyEntropy::RunLevel);
    let dec = t.dec_tables();
    let cmap = t.padded_map();
    let cmap32: alloc::boxed::Box<[u32; coef::NRAW_PAD]> = cmap
        .iter()
        .map(|&v| v as u32)
        .collect::<Vec<u32>>()
        .into_boxed_slice()
        .try_into()
        .unwrap();
    // Initial states (>= RANS_L), then random words.
    let mut bytes = Vec::with_capacity(4 * NL + (1 << 18));
    let mut s = 0x9E37_79B9u32;
    for _ in 0..NL {
        bytes.extend_from_slice(&(RANS_L | 0x1234).to_le_bytes());
    }
    for _ in 0..(1 << 15) {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        bytes.extend_from_slice(&(s as u16).to_le_bytes());
    }
    Synth {
        cmap,
        cmap32,
        dec,
        bytes,
    }
}

#[cfg(feature = "std")]
const BW: usize = 64;
#[cfg(feature = "std")]
const NBY: usize = 4;

/// Best-of-3 time of `f` (two stripes per call), each candidate in turn.
#[cfg(feature = "std")]
fn race(cands: &mut [&mut dyn FnMut()]) -> usize {
    let mut best = vec![u128::MAX; cands.len()];
    for f in cands.iter_mut() {
        f();
    }
    for _ in 0..3 {
        for (i, f) in cands.iter_mut().enumerate() {
            let t = std::time::Instant::now();
            f();
            best[i] = best[i].min(t.elapsed().as_nanos());
        }
    }
    (0..best.len()).min_by_key(|&i| best[i]).unwrap()
}

#[cfg(feature = "std")]
fn measure() -> Choice {
    use archmage::SimdToken;
    let (Some(t4), Some(t3)) = (
        archmage::X64V4Token::summon(),
        archmage::X64V3Token::summon(),
    ) else {
        return Choice { rl: Rl::Avx2 };
    };
    let nq = NBY * BW * coef::NCH * 64 + 64;
    let ne = NBY * BW * coef::NCH + 1;
    // Run-level.
    let sy = synth();
    let (w, x) = Words::from_bytes(&sy.bytes, NL).unwrap();
    let mut q = [vec![0i32; nq], vec![0i32; nq]];
    let mut e = [vec![0u8; ne], vec![0u8; ne]];
    let m = super::entropy_avx512::Maps {
        cmap: &sy.cmap,
        cmap32: &sy.cmap32,
        dec: &sy.dec,
    };
    let rl = {
        let mut pair = |hw: bool| {
            let [q0, q1] = &mut q;
            let [e0, e1] = &mut e;
            core::hint::black_box(super::entropy_avx512::decode_stripes2(
                t4,
                hw,
                [&w, &w],
                [x, x],
                &m,
                BW,
                [NBY, NBY],
                NL,
                0,
                coef::NCH as u32,
                [&mut q0[..], &mut q1[..]],
                [&mut e0[..], &mut e1[..]],
                None,
            ));
        };
        let mut c_hw = || pair(true);
        let mut q2 = [vec![0i32; nq], vec![0i32; nq]];
        let mut e2 = [vec![0u8; ne], vec![0u8; ne]];
        let mut c_sw = || {
            let [q0, q1] = &mut q2;
            let [e0, e1] = &mut e2;
            core::hint::black_box(super::entropy_avx512::decode_stripes2(
                t4,
                false,
                [&w, &w],
                [x, x],
                &m,
                BW,
                [NBY, NBY],
                NL,
                0,
                coef::NCH as u32,
                [&mut q0[..], &mut q1[..]],
                [&mut e0[..], &mut e1[..]],
                None,
            ));
        };
        let mut q3 = vec![0i32; nq];
        let mut e3 = vec![0u8; ne];
        let mut c_avx2 = || {
            for _ in 0..2 {
                core::hint::black_box(super::entropy_avx2::decode_stripe(
                    t3,
                    &w,
                    x,
                    &sy.cmap,
                    &sy.dec,
                    BW,
                    NBY,
                    NL,
                    0,
                    coef::NCH as u32,
                    &mut q3,
                    &mut e3,
                ));
            }
        };
        let force = super::entropy_avx512::FORCE.load(core::sync::atomic::Ordering::Relaxed);
        match (force, std::env::var("IMIJ_GATHER").as_deref()) {
            (1, _) | (_, Ok("1")) => Rl::Avx512Hw,
            (2, _) | (_, Ok("0")) => Rl::Avx512Sw,
            _ => [Rl::Avx512Hw, Rl::Avx512Sw, Rl::Avx2]
                [race(&mut [&mut c_hw, &mut c_sw, &mut c_avx2])],
        }
    };
    Choice { rl }
}

/// LAB: pins the raced kernel for same-binary A/B timing, where a
/// per-process race would add its own variance. `IMIJ_RL_KERNEL` =
/// `hw`|`sw`|`avx2`; unset keeps the race result.
#[cfg(all(feature = "std", feature = "lab"))]
fn lab_pin(mut c: Choice) -> Choice {
    match std::env::var("IMIJ_RL_KERNEL").as_deref() {
        Ok("hw") => c.rl = Rl::Avx512Hw,
        Ok("sw") => c.rl = Rl::Avx512Sw,
        Ok("avx2") => c.rl = Rl::Avx2,
        _ => {}
    }
    c
}

#[cfg(all(feature = "std", not(feature = "lab")))]
#[inline(always)]
fn lab_pin(c: Choice) -> Choice {
    c
}

/// The kernel to use on this CPU (AVX-512 present). Measured once per
/// process (std); without std, AVX-512 with software lookups.
pub(crate) fn choice() -> Choice {
    // Tests force the AVX-512 kernel (lookup style per FORCE).
    match super::entropy_avx512::FORCE.load(core::sync::atomic::Ordering::Relaxed) {
        1 => {
            return Choice { rl: Rl::Avx512Hw };
        }
        2 => {
            return Choice { rl: Rl::Avx512Sw };
        }
        _ => {}
    }
    #[cfg(feature = "std")]
    {
        static C: std::sync::OnceLock<Choice> = std::sync::OnceLock::new();
        *C.get_or_init(|| lab_pin(measure()))
    }
    #[cfg(not(feature = "std"))]
    Choice { rl: Rl::Avx512Sw }
}
