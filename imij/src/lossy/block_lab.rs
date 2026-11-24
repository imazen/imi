//! LAB: per-block hooks for the lossless-over-lossy estimate
//! (`crate::lab::encode_lossy_blocks`). Thread-local, so harness threads that
//! each encode one image do not see each other's settings; every hook runs on
//! the encoding thread whatever `LossyConfig::threads` is.

use alloc::vec::Vec;
use std::cell::RefCell;

use super::coef::Tables;
use super::{NCH, StripeCoefs};

std::thread_local! {
    /// Blocks (raster order) whose AC coefficients are dropped (REPLACE).
    pub(crate) static AC_ZERO: RefCell<Option<Vec<bool>>> = const { RefCell::new(None) };
    /// Set to `Some(empty)` to receive each block's run-level bits.
    pub(crate) static BLOCK_BITS: RefCell<Option<Vec<f32>>> = const { RefCell::new(None) };
}

/// Zeroes the AC coefficients of masked blocks (full-width stripes), keeping
/// their DC.
pub(super) fn zero_masked_ac(stripes: &mut [StripeCoefs]) {
    AC_ZERO.with(|m| {
        let Some(mask) = m.borrow().clone() else {
            return;
        };
        for sc in stripes.iter_mut() {
            for r in 0..sc.nby {
                for bx in 0..sc.bw {
                    if !mask[(sc.by0 + r) * sc.bw + bx] {
                        continue;
                    }
                    let b = r * sc.bw + bx;
                    for c in 0..NCH {
                        let o = (b * NCH + c) * 64;
                        sc.q[o + 1..o + 64].fill(0);
                        sc.nz[b * NCH + c] &= 1;
                    }
                }
            }
        }
    });
}

/// Fills [`BLOCK_BITS`] (when requested) with [`block_bits`].
pub(super) fn record_block_bits(stripes: &[StripeCoefs], tables: &Tables) {
    BLOCK_BITS.with(|out| {
        if let Some(out) = out.borrow_mut().as_mut() {
            *out = super::exact::block_bits(stripes, tables);
        }
    });
}

/// LAB exact blocks (`crate::lab::set_lossy_exact_blocks`): f32 bits of the
/// ratio k, 0 = off. Blocks whose lossless proxy is at most k times their
/// run-level AC bits are coded exactly.
pub(crate) static EXACT_RATIO: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

std::thread_local! {
    /// Per-thread override of [`EXACT_RATIO`] (`crate::lab::encode_lossy_exact`).
    pub(crate) static EXACT_RATIO_HERE: core::cell::Cell<Option<f32>> = const { core::cell::Cell::new(None) };
}

pub(crate) fn exact_ratio() -> Option<f32> {
    let k = EXACT_RATIO_HERE
        .with(|c| c.get())
        .unwrap_or_else(|| f32::from_bits(EXACT_RATIO.load(core::sync::atomic::Ordering::Relaxed)));
    (k > 0.0).then_some(k)
}

/// `IMIJ_EXACT_ROWS` (a multiple of 8): nested stripe height override for
/// measurements.
pub(crate) fn exact_stripe_rows() -> Option<usize> {
    std::env::var("IMIJ_EXACT_ROWS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&r| r >= 8 && r % 8 == 0)
}
