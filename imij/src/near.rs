//! Error-bounded near-lossless coding (Scientific profile; SPEC 2.6).
//!
//! JPEG-LS NEAR-style closed-loop residual quantization inside the predictive
//! lossless path. A slot with bound `delta > 0` codes the quantized residual
//! `q = sign(e) * floor((|e| + delta) / (2 delta + 1))` of `e = value - pred`,
//! and both sides reconstruct `clamp(pred + q * (2 delta + 1), 0, 2^n - 1)`,
//! so every coded value is within `delta` of the source. The encoder predicts
//! from reconstructed values, exactly as the decoder does. Slots with
//! `delta = 0` keep the modular lossless rule.

#[cfg(feature = "encode")]
use alloc::vec;
#[cfg(feature = "encode")]
use alloc::vec::Vec;

#[cfg(feature = "near-lossless")]
use crate::SampleType;
#[cfg(feature = "encode")]
use crate::coding_layout::{Layout, NUM_DECOR};
#[cfg(feature = "encode")]
use crate::encode::{AnalysisConfig, cost_sum, decor_rows, zz};
#[cfg(feature = "encode")]
use crate::mapping::{ChannelMap, MapMode};
#[cfg(feature = "encode")]
use crate::predict::{self, NUM_PREDICTORS};
#[cfg(feature = "encode")]
use crate::{Error, ImageInfo};

/// The sample types near-lossless coding defines bounds for (SPEC 2.6):
/// u8, u16, u32 and i32. Floats are quantized to integers first; i8, i16
/// and 64-bit samples have no near-lossless coding.
#[inline]
#[cfg(feature = "near-lossless")]
pub(crate) fn defined_for(sample: SampleType) -> bool {
    matches!(
        sample,
        SampleType::U8 | SampleType::U16 | SampleType::U32 | SampleType::I32
    )
}

/// Largest coded bound a slot of `bits` bits may declare: `2^(bits-1) - 1`
/// (0 for 0- and 1-bit slots). It keeps `2 delta + 1` below `2^bits`.
#[inline]
#[cfg(any(feature = "near-lossless", test))]
pub(crate) fn delta_cap(bits: u32) -> u32 {
    if bits <= 1 {
        0
    } else {
        ((1u64 << (bits - 1)) - 1) as u32
    }
}

/// `sign(e) * floor((|e| + delta) / (2 delta + 1))`: `e / (2 delta + 1)`
/// rounded to nearest (an odd step has no ties).
#[inline(always)]
#[cfg(feature = "encode")]
pub(crate) fn quantize(e: i64, delta: u32) -> i64 {
    let step = 2 * delta as i64 + 1;
    let q = (e.abs() + delta as i64) / step;
    if e < 0 { -q } else { q }
}

/// Sign-extends an `bits`-bit value (`bits` 1..=32).
#[inline(always)]
pub(crate) fn sext(d: u32, bits: u32) -> i64 {
    if bits == 0 {
        return 0;
    }
    let sh = 32 - bits;
    (((d << sh) as i32) >> sh) as i64
}

/// The decoder's near-lossless reconstruction of one coded value: residual
/// `d` (a `bits`-bit modular value after color decorrelation) is the
/// quantized residual `q = sext(d)`; the value is `pred + q (2 delta + 1)`
/// clamped to `[0, 2^bits - 1]`, computed exactly.
#[inline(always)]
pub(crate) fn reconstruct(pred: u32, d: u32, bits: u32, delta: u32) -> u32 {
    let q = sext(d, bits);
    let v = pred as i64 + q * (2 * delta as i64 + 1);
    v.clamp(0, crate::mask_of(bits) as i64) as u32
}

/// The coded bound for channel map `m` (container depth `depth`) and a
/// container-unit bound `delta`: the largest `k` such that any two coded
/// values at most `k` apart expand to container samples at most `delta`
/// apart, capped at [`delta_cap`]. Maps that are not affine in the integer
/// sample (floats, constants, value tables) get 0.
#[cfg(feature = "encode")]
pub(crate) fn coded_delta(m: &ChannelMap, depth: u32, delta: u32) -> u32 {
    let n = m.bits as u32;
    let cap = delta_cap(n);
    if delta == 0 || cap == 0 {
        return 0;
    }
    let k = match m.mode {
        MapMode::Const
        | MapMode::F16InF32
        | MapMode::UnormF32
        | MapMode::FloatShift
        | MapMode::Table => 0,
        MapMode::Lsb | MapMode::Offset => delta,
        MapMode::Shift => delta >> (depth - n),
        MapMode::Replicate | MapMode::Scale => {
            // Monotone maps of at most 16 coded bits: the widest container
            // step of k coded steps, searched exactly.
            let top = crate::mask_of(n);
            let spread = |k: u32| {
                (0..=top - k)
                    .map(|s| m.expand(s + k, depth) - m.expand(s, depth))
                    .max()
                    .unwrap_or(0)
            };
            let (mut lo, mut hi) = (0u32, cap.min(top));
            while lo < hi {
                let mid = lo + (hi - lo).div_ceil(2);
                if spread(mid) <= delta {
                    lo = mid;
                } else {
                    hi = mid - 1;
                }
            }
            lo
        }
    };
    k.min(cap)
}

/// Prepares near-lossless coding of the channels with a nonzero bound in
/// `max_error` (container units, memory channel order; entries past the
/// channel count are ignored): adjusts maps so that
/// every reconstructable coded value expands inside the source's range
/// (declared `sig_bits` range, no wrap of an Offset base), and returns the
/// coded bound of each coded channel (`maps.len()` entries). `planes` hold
/// the ordered container samples.
#[cfg(feature = "encode")]
pub(crate) fn fit_maps(
    info: &ImageInfo,
    planes: &[Vec<u32>],
    maps: &mut [ChannelMap],
    max_error: &[u32; 4],
    mut check: impl FnMut() -> Result<(), Error>,
) -> Result<Vec<u32>, Error> {
    let nch = info.channels.count();
    let depth = info.sample.depth();
    let mut out = vec![0u32; maps.len()];
    if max_error[..nch].iter().all(|&d| d == 0) {
        return Ok(out);
    }
    if !defined_for(info.sample) {
        return Err(Error::InvalidInput(if info.sample.is_float() {
            "near-lossless needs integer samples (quantize floats to integers first)"
        } else {
            "near-lossless coding is defined for u8, u16, u32 and i32 samples"
        }));
    }
    for c in 0..nch {
        let delta = max_error[c];
        let m = &mut maps[c];
        if delta == 0 || m.mode == MapMode::Const {
            continue;
        }
        let n = m.bits as u32;
        // Unsigned samples declared narrower than their storage: keep the
        // clamp range inside the declaration when the source is inside it.
        let sig = info.channel_bits(c).unwrap_or(depth as u8) as u32;
        if info.sample != SampleType::I32 && sig < depth && n > 0 {
            let limit = crate::mask_of(sig);
            let top = m.expand(crate::mask_of(n), depth);
            let mut max = 0;
            for chunk in planes[c].chunks(crate::mapping::SCAN_WORDS) {
                check()?;
                max = max.max(chunk.iter().copied().max().unwrap_or(0));
            }
            if top > limit && max <= limit {
                *m = ChannelMap {
                    mode: MapMode::Lsb,
                    bits: (32 - max.leading_zeros()).max(1) as u8,
                    value: 0,
                };
            }
        }
        // An Offset base whose coded range runs past 2^32 would wrap a
        // clamped value around: lower the base (every sample stays exact).
        let n = m.bits as u32;
        if m.mode == MapMode::Offset
            && n < 32
            && m.value as u64 + crate::mask_of(n) as u64 > u32::MAX as u64
        {
            m.value = ((1u64 << 32) - (1u64 << n)) as u32;
        }
        out[c] = coded_delta(m, depth, delta);
    }
    Ok(out)
}

/// One row of slot values under predictor `P`, reconstructed in the loop:
/// `d` receives the coded residuals (`bits`-bit modular), `rec` the values
/// a decoder reconstructs. `prev` is the reconstructed row above (`None` on
/// a stripe's first row). Lanes of `sw` columns follow the format's edge
/// rules.
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "encode")]
fn sim_row<const P: u8>(
    src: &[u32],
    prev: Option<&[u32]>,
    sw: usize,
    bits: u32,
    delta: u32,
    d: &mut [u32],
    rec: &mut [u32],
) {
    let w = src.len();
    let mask = crate::mask_of(bits);
    let step = 2 * delta as i64 + 1;
    let mut xs = 0;
    while xs < w {
        let xe = (xs + sw).min(w);
        for x in xs..xe {
            let (wv, n, nw, ne) = match prev {
                None => {
                    let wv = if x > xs { rec[x - 1] } else { 0 };
                    (wv, wv, wv, wv)
                }
                Some(up) => {
                    let n = up[x];
                    let (wv, nw) = if x > xs {
                        (rec[x - 1], up[x - 1])
                    } else {
                        (n, n)
                    };
                    let ne = if x + 1 < xe { up[x + 1] } else { n };
                    (wv, n, nw, ne)
                }
            };
            let p = predict::predict(P, wv, n, nw, ne, mask);
            let cur = src[x];
            if delta == 0 {
                d[x] = cur.wrapping_sub(p) & mask;
                rec[x] = cur;
            } else {
                let q = quantize(cur as i64 - p as i64, delta);
                d[x] = (q as u32) & mask;
                rec[x] = (p as i64 + q * step).clamp(0, mask as i64) as u32;
            }
        }
        xs = xe;
    }
}

#[allow(clippy::too_many_arguments)]
#[cfg(feature = "encode")]
fn sim_row_dyn(
    p: u8,
    src: &[u32],
    prev: Option<&[u32]>,
    sw: usize,
    bits: u32,
    delta: u32,
    d: &mut [u32],
    rec: &mut [u32],
) {
    match p {
        0 => sim_row::<0>(src, prev, sw, bits, delta, d, rec),
        1 => sim_row::<1>(src, prev, sw, bits, delta, d, rec),
        2 => sim_row::<2>(src, prev, sw, bits, delta, d, rec),
        3 => sim_row::<3>(src, prev, sw, bits, delta, d, rec),
        4 => sim_row::<4>(src, prev, sw, bits, delta, d, rec),
        5 => sim_row::<5>(src, prev, sw, bits, delta, d, rec),
        _ => sim_row::<6>(src, prev, sw, bits, delta, d, rec),
    }
}

/// Closed-loop counterpart of `encode::analyze_stripe_fast` for a layout
/// with near-lossless slots: picks each row's predictor greedily (its cost
/// under the best decorrelation mode, given the rows already reconstructed)
/// and the stripe's decorrelation mode by the total cost of the chosen
/// rows; writes zigzag residuals of stripe rows `y0..y1` into `res`.
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "encode")]
pub(crate) fn analyze_stripe_near(
    layout: &Layout,
    planes: &[&[u32]],
    w: usize,
    sw: usize,
    y0: usize,
    y1: usize,
    cfg: &AnalysisConfig,
    res: &mut [&mut [u32]],
) -> (u8, Vec<u8>) {
    analyze_stripe_near_rec(layout, planes, w, sw, y0, y1, cfg, res).0
}

/// [`analyze_stripe_near`] that also returns the reconstructed coded values
/// of the stripe (per slot, `rows x w`): what a decoder must reproduce.
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "encode")]
pub(crate) fn analyze_stripe_near_rec(
    layout: &Layout,
    planes: &[&[u32]],
    w: usize,
    sw: usize,
    y0: usize,
    y1: usize,
    cfg: &AnalysisConfig,
    res: &mut [&mut [u32]],
) -> ((u8, Vec<u8>), Vec<Vec<u32>>) {
    let ns = planes.len();
    let do_decor = layout.color_slots == 3 && layout.decor_ok;
    let decors: Vec<u8> = (0..NUM_DECOR)
        .filter(|&m| (cfg.decor_mask >> m) & 1 == 1 && (m == 0 || do_decor))
        .collect();
    let decors = if decors.is_empty() { vec![0] } else { decors };
    let preds: Vec<u8> = (0..NUM_PREDICTORS)
        .filter(|&p| (cfg.predictor_mask >> p) & 1 == 1)
        .collect();
    let preds = if preds.is_empty() { vec![0] } else { preds };
    let rows = y1 - y0;
    let bits = &layout.slot_bits;
    let masks: Vec<u32> = bits.iter().map(|&b| crate::mask_of(b)).collect();
    let near = &layout.near;
    // Reconstructed values and pre-decorrelation residuals of the stripe.
    let mut rec: Vec<Vec<u32>> = vec![vec![0u32; rows * w]; ns];
    let mut dres: Vec<Vec<u32>> = vec![vec![0u32; rows * w]; ns];
    let mut d: Vec<Vec<u32>> = vec![vec![0u32; w]; ns];
    let mut rr: Vec<Vec<u32>> = vec![vec![0u32; w]; ns];
    let (mut ro, mut bo) = (vec![0u32; w], vec![0u32; w]);
    let mut acc = vec![0u64; decors.len()];
    let mut chosen = Vec::with_capacity(rows);
    let row_costs = |d: &[Vec<u32>], ro: &mut [u32], bo: &mut [u32], out: &mut [u64]| {
        let base: u64 = (0..ns)
            .filter(|&s| !(do_decor && (s == 1 || s == 2)))
            .map(|s| cost_sum(&d[s], bits[s], masks[s]))
            .sum();
        for (di, &m) in decors.iter().enumerate() {
            out[di] = if !do_decor {
                base
            } else if m == 0 {
                base + cost_sum(&d[1], bits[1], masks[1]) + cost_sum(&d[2], bits[2], masks[2])
            } else {
                decor_rows(m, &d[0], &d[1], &d[2], bits[0], masks[0], ro, bo);
                base + cost_sum(ro, bits[0], masks[0]) + cost_sum(bo, bits[0], masks[0])
            };
        }
    };
    let mut costs = vec![0u64; decors.len()];
    let mut best_costs = vec![0u64; decors.len()];
    for r in 0..rows {
        let y = y0 + r;
        let mut best: Option<(u64, u8)> = None;
        for &p in &preds {
            if r == 0 && p != predict::P_W {
                continue;
            }
            for s in 0..ns {
                let prev = (r > 0).then(|| &rec[s][(r - 1) * w..r * w]);
                sim_row_dyn(
                    p,
                    &planes[s][y * w..(y + 1) * w],
                    prev,
                    sw,
                    bits[s],
                    near[s],
                    &mut d[s],
                    &mut rr[s],
                );
            }
            row_costs(&d, &mut ro, &mut bo, &mut costs);
            let c = *costs.iter().min().unwrap();
            if best.is_none_or(|b| c < b.0) {
                best = Some((c, p));
                best_costs.copy_from_slice(&costs);
            }
        }
        // Row 0 always has a candidate (W); recompute the winner's rows.
        let p = best.map_or(predict::P_W, |b| b.1);
        for s in 0..ns {
            let (above, here) = rec[s].split_at_mut(r * w);
            let prev = (r > 0).then(|| &above[(r - 1) * w..]);
            sim_row_dyn(
                p,
                &planes[s][y * w..(y + 1) * w],
                prev,
                sw,
                bits[s],
                near[s],
                &mut dres[s][r * w..(r + 1) * w],
                &mut here[..w],
            );
        }
        for (a, c) in acc.iter_mut().zip(&best_costs) {
            *a += c;
        }
        chosen.push(p);
    }
    let di = (0..decors.len()).min_by_key(|&i| (acc[i], i)).unwrap();
    let decor = if do_decor { decors[di] } else { 0 };
    for r in 0..rows {
        let row = r * w..(r + 1) * w;
        if do_decor && decor != 0 {
            decor_rows(
                decor,
                &dres[0][row.clone()],
                &dres[1][row.clone()],
                &dres[2][row.clone()],
                bits[0],
                masks[0],
                &mut ro,
                &mut bo,
            );
            dres[1][row.clone()].copy_from_slice(&ro);
            dres[2][row.clone()].copy_from_slice(&bo);
        }
        for s in 0..ns {
            let out = &mut res[s][row.clone()];
            for (o, &v) in out.iter_mut().zip(&dres[s][row.clone()]) {
                *o = if bits[s] == 0 {
                    0
                } else {
                    zz(v, bits[s], masks[s])
                };
            }
        }
    }
    ((decor, chosen), rec)
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    #[test]
    fn quantizer_rounds_to_nearest_and_bounds_the_error() {
        for delta in 1..=9u32 {
            let step = 2 * delta as i64 + 1;
            for e in -300i64..=300 {
                let q = quantize(e, delta);
                assert!((e - q * step).abs() <= delta as i64, "e={e} delta={delta}");
                // Shift equivariance: whole steps move q by whole steps.
                assert_eq!(quantize(e + 3 * step, delta), q + 3);
            }
        }
    }

    #[test]
    fn quantized_residual_fits_its_slot() {
        // |q| < 2^(n-1) for every residual of an n-bit slot and delta >= 1,
        // so q mod 2^n sign-extends back to q.
        for bits in 2..=16u32 {
            let top = crate::mask_of(bits) as i64;
            for delta in [1, 2, 3, delta_cap(bits)] {
                if delta == 0 {
                    continue;
                }
                for e in [-top, -top + 1, -1, 0, 1, top - 1, top] {
                    let q = quantize(e, delta);
                    let d = (q as u32) & crate::mask_of(bits);
                    assert_eq!(sext(d, bits), q, "bits={bits} delta={delta} e={e}");
                }
            }
        }
    }

    #[test]
    fn reconstruction_clamps_and_matches_the_encoder() {
        for bits in [1u32, 2, 8, 12, 16, 31, 32] {
            let mask = crate::mask_of(bits);
            for delta in [0, 1, 2, 7, delta_cap(bits)] {
                let delta = delta.min(delta_cap(bits));
                if delta == 0 {
                    continue;
                }
                for &(cur, pred) in &[(0, mask), (mask, 0), (mask / 2, 3), (1, 0), (mask, mask)] {
                    let q = quantize(cur as i64 - pred as i64, delta);
                    let d = (q as u32) & mask;
                    let v = reconstruct(pred, d, bits, delta);
                    assert!((v as i64 - cur as i64).abs() <= delta as i64);
                    assert!(v <= mask);
                }
            }
        }
    }

    #[test]
    fn coded_delta_respects_every_map() {
        #[cfg(feature = "encode")]
        use crate::mapping::{ChannelMap, MapMode};
        let lsb = ChannelMap {
            mode: MapMode::Lsb,
            bits: 16,
            value: 0,
        };
        assert_eq!(coded_delta(&lsb, 16, 5), 5);
        assert_eq!(coded_delta(&lsb, 16, 1 << 20), delta_cap(16));
        let shift = ChannelMap {
            mode: MapMode::Shift,
            bits: 12,
            value: 0,
        };
        assert_eq!(coded_delta(&shift, 16, 15), 0);
        assert_eq!(coded_delta(&shift, 16, 16), 1);
        assert_eq!(coded_delta(&shift, 16, 50), 3);
        for mode in [MapMode::Replicate, MapMode::Scale] {
            for bits in [8u8, 10, 12] {
                let m = ChannelMap {
                    mode,
                    bits,
                    value: 0,
                };
                for delta in [1u32, 100, 257, 1000, 5000] {
                    let k = coded_delta(&m, 16, delta);
                    // Exhaustively: k coded steps never exceed delta.
                    let top = crate::mask_of(bits as u32);
                    for s in 0..=top.saturating_sub(k) {
                        assert!(m.expand(s + k, 16) - m.expand(s, 16) <= delta);
                    }
                }
            }
        }
    }
}
