//! Overshoot deringing (encoder only; the format is unchanged).
//!
//! Integer outputs clamp to their maximum code, so the encoder may give a
//! run of white samples any value at or above white: the decoder shows white
//! either way. A flat plateau ending in a hard edge needs many AC
//! coefficients and rings where they are quantized; a smooth bump peaking
//! above white costs fewer bits and rings less. Kornel Lesiński described
//! overshoot deringing and contributed its mozjpeg implementation
//! (`jcdctmgr.c`); zenjpeg ports it. We apply it to the luma plane
//! (plane 0 of XYB and YCoCg), whose maximum is reached only at white.

#[cfg(feature = "encode")]
use super::coef::ZIGZAG;

/// Minimum edge depth, as a fraction of the white level, for a block to be
/// reshaped (2026-10-06 sweep: blocks of exact white inside near-white
/// texture regressed without it).
#[cfg(feature = "encode")]
const EDGE: f32 = 0.125;

/// Catmull-Rom interpolation between `v2` and `v3` at `t` in 0..=1, with
/// tangents from `v1` and `v4` scaled by the segment length `size`.
#[cfg(feature = "encode")]
fn catmull_rom(v1: f32, v2: f32, v3: f32, v4: f32, t: f32, size: f32) -> f32 {
    let tan1 = (v3 - v1) * size;
    let tan2 = (v4 - v2) * size;
    let t2 = t * t;
    let t3 = t2 * t;
    v2 * (2.0 * t3 - 3.0 * t2 + 1.0)
        + v3 * (-2.0 * t3 + 3.0 * t2)
        + tan1 * (t3 - 2.0 * t2 + t)
        + tan2 * (t3 - t2)
}

/// Reshapes the white runs of one 8x8 block of plane 0 before its DCT.
/// The block is `plane[y * pw + x0..][..8]` for rows `y` in 0..8; bit `i` of
/// `white` (raster order) marks sample `i` as white, whose plane value is
/// `top`. `cap` bounds the overshoot above `top`. Runs follow the zigzag
/// scan, as in mozjpeg; a block with no white sample, or only white ones,
/// is unchanged.
#[cfg(feature = "encode")]
pub(crate) fn dering_block(
    plane: &mut [f32],
    pw: usize,
    x0: usize,
    white: u64,
    top: f32,
    cap: f32,
) {
    let count = white.count_ones();
    if count == 0 || count == 64 {
        return;
    }
    let at = |i: usize| (i / 8) * pw + x0 + i % 8;
    let is_white = |i: usize| white >> i & 1 != 0;
    // Only an edge rings. A block whose other samples are all near white
    // (a light texture or noise around exact white) has nothing to fix, and
    // lifting just its white samples would add a faint tile: require one
    // sample at least 1/8 of the white level darker.
    let darkest = (0..64)
        .filter(|&i| !is_white(i))
        .map(|i| plane[at(i)])
        .fold(f32::INFINITY, f32::min);
    if top - darkest < top.abs() * EDGE {
        return;
    }
    // Overshoot stays within the block's headroom below white, so the DC
    // (block mean) never rises above white.
    let headroom = (0..64).map(|i| top - plane[at(i)]).sum::<f32>() / count as f32;
    let max = top + cap.min(headroom).max(0.0);
    let zz = |n: usize| ZIGZAG[n] as usize;
    let mut n = 0;
    while n < 64 {
        if !is_white(zz(n)) {
            n += 1;
            continue;
        }
        let start = n;
        while n + 1 < 64 && is_white(zz(n + 1)) {
            n += 1;
        }
        let end = n + 1;
        let f1 = plane[at(zz(start.saturating_sub(1)))];
        let f2 = plane[at(zz(start.saturating_sub(2)))];
        let l1 = plane[at(zz(end.min(63)))];
        let l2 = plane[at(zz((end + 1).min(63)))];
        let mut fslope = (f1 - f2).max(top - f1);
        let mut lslope = (l1 - l2).max(top - l1);
        if start == 0 {
            fslope = lslope;
        }
        if end == 64 {
            lslope = fslope;
        }
        let length = (end - start) as f32;
        let step = 1.0 / (length + 1.0);
        let mut t = step;
        for i in start..end {
            let v = catmull_rom(top - fslope, top, top, top - lslope, t, length);
            plane[at(zz(i))] = v.min(max);
            t += step;
        }
        n += 1;
    }
}

/// Lab A/B switch (on by default).
#[cfg(feature = "lab")]
pub(crate) static ENABLED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(true);

/// Whether an encode deringes (lab: [`super::LossyLabSwitches::dering`]).
#[cfg(feature = "encode")]
pub(crate) fn enabled(cfg: &super::LossyConfig) -> bool {
    #[cfg(feature = "lab")]
    {
        cfg.lab
            .dering
            .unwrap_or_else(|| ENABLED.load(core::sync::atomic::Ordering::Relaxed))
    }
    #[cfg(not(feature = "lab"))]
    {
        let _ = cfg;
        true
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    #[test]
    fn plateau_becomes_a_bump_above_white_and_rest_is_unchanged() {
        // Left half white (1.0), right half a dark gradient.
        let mut p = [0.0f32; 64];
        let mut white = 0u64;
        for y in 0..8 {
            for x in 0..8 {
                let i = y * 8 + x;
                if x < 4 {
                    p[i] = 1.0;
                    white |= 1 << i;
                } else {
                    p[i] = 0.1 * x as f32;
                }
            }
        }
        let before = p;
        dering_block(&mut p, 8, 0, white, 1.0, 0.2);
        for i in 0..64 {
            if white >> i & 1 == 0 {
                assert_eq!(p[i], before[i], "non-white sample {i} changed");
            } else {
                // Still white after the decoder's clamp.
                assert!(p[i] >= 1.0 - 1e-6, "white sample {i} fell to {}", p[i]);
                assert!(p[i] <= 1.2 + 1e-6, "sample {i} above cap: {}", p[i]);
            }
        }
        assert!(p.iter().any(|&v| v > 1.0), "no overshoot");
        let mean = |b: &[f32; 64]| b.iter().sum::<f32>() / 64.0;
        assert!(mean(&p) <= 1.0 + 1e-6);
    }

    #[test]
    fn near_white_texture_is_left_alone() {
        // Exact white mixed with samples 1-5% below it: no edge, no change.
        let mut p = [0.0f32; 64];
        let mut white = 0u64;
        for (i, v) in p.iter_mut().enumerate() {
            if i % 3 == 0 {
                *v = 1.0;
                white |= 1 << i;
            } else {
                *v = 0.95 + 0.01 * (i % 5) as f32;
            }
        }
        let before = p;
        dering_block(&mut p, 8, 0, white, 1.0, 0.2);
        assert_eq!(p, before);
    }

    #[test]
    fn all_or_nothing_white_blocks_are_unchanged() {
        let mut p = [1.0f32; 64];
        dering_block(&mut p, 8, 0, u64::MAX, 1.0, 0.2);
        assert!(p.iter().all(|&v| v == 1.0));
        let mut q = [0.5f32; 64];
        dering_block(&mut q, 8, 0, 0, 1.0, 0.2);
        assert!(q.iter().all(|&v| v == 0.5));
    }
}
