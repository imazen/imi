//! tANS helpers (frequency normalization; coder lives here once selected).

use alloc::vec;
use alloc::vec::Vec;

/// Normalizes counts to sum to `1 << log_m`, every used symbol >= 1.
/// Returns `None` if more symbols are used than states exist.
pub(crate) fn normalize(counts: &[u64], log_m: u32) -> Option<Vec<u32>> {
    let m = 1u64 << log_m;
    let total: u64 = counts.iter().sum();
    let used = counts.iter().filter(|&&c| c > 0).count() as u64;
    if used == 0 {
        return Some(vec![0; counts.len()]);
    }
    if used > m {
        return None;
    }
    let mut q: Vec<u32> = counts
        .iter()
        .map(|&c| {
            if c == 0 {
                0
            } else {
                (crate::mathx::round_pos((c as f64) * m as f64 / total as f64) as u32).max(1)
            }
        })
        .collect();
    let mut sum: i64 = q.iter().map(|&v| v as i64).sum();
    // Greedy fix-up by marginal cost.
    while sum != m as i64 {
        let mut best = (f64::INFINITY, usize::MAX);
        for (i, &c) in counts.iter().enumerate() {
            if c == 0 {
                continue;
            }
            let qi = q[i] as f64;
            let delta = if sum > m as i64 {
                if q[i] <= 1 {
                    continue;
                }
                c as f64 * crate::mathx::log2(qi / (qi - 1.0))
            } else {
                -(c as f64) * crate::mathx::log2((qi + 1.0) / qi)
            };
            if delta < best.0 {
                best = (delta, i);
            }
        }
        if sum > m as i64 {
            q[best.1] -= 1;
            sum -= 1;
        } else {
            q[best.1] += 1;
            sum += 1;
        }
    }
    Some(q)
}

/// Ideal coded size in bits for `counts` under normalized `q`.
pub(crate) fn cost_bits(counts: &[u64], q: &[u32], log_m: u32) -> f64 {
    counts
        .iter()
        .zip(q)
        .filter(|(c, _)| **c > 0)
        .map(|(&c, &qi)| c as f64 * (log_m as f64 - crate::mathx::log2(qi as f64)))
        .sum()
}
