//! Exact cost models for candidate entropy layers, evaluated on the residual
//! planes produced by `imij::analyze`.

use imij::Analysis;

pub const CLASSES: usize = 18;
/// Unary prefix limit: q >= QMAX means escape (QMAX zeros + raw value).
pub const QMAX: u32 = 12;

#[inline]
pub fn class_of(s: u32) -> usize {
    ((32 - s.leading_zeros()) as usize).min(CLASSES - 1)
}

#[inline]
pub fn rice_len(u: u32, k: u32, raw_bits: u32) -> u64 {
    let q = u >> k;
    if q < QMAX {
        (q + 1 + k) as u64
    } else {
        (QMAX + raw_bits) as u64
    }
}

/// Histogram of values that is cheap to evaluate rice cost on.
struct Hist {
    counts: Vec<u64>,
}

impl Hist {
    fn new(n: usize) -> Self {
        Self { counts: vec![0; n] }
    }
    fn add(&mut self, u: u32) {
        let i = (u as usize).min(self.counts.len() - 1);
        self.counts[i] += 1;
    }
    fn best_rice(&self, raw_bits: u32) -> (u32, u64) {
        let mut best = (0, u64::MAX);
        for k in 0..=raw_bits.min(16) {
            let c: u64 = self
                .counts
                .iter()
                .enumerate()
                .filter(|(_, n)| **n > 0)
                .map(|(u, &n)| n * rice_len(u as u32, k, raw_bits))
                .sum();
            if c < best.1 {
                best = (k, c);
            }
        }
        best
    }
    fn entropy_bits(&self) -> f64 {
        let t: u64 = self.counts.iter().sum();
        if t == 0 {
            return 0.0;
        }
        self.counts
            .iter()
            .filter(|&&n| n > 0)
            .map(|&n| -(n as f64) * (n as f64 / t as f64).log2())
            .sum()
    }
}

pub struct CostReport {
    /// Rice with per-class static k and JPEG-LS-like run mode.
    pub rice_bits: u64,
    /// Order-0 entropy within the same classes (ideal static ANS, no tables).
    pub entropy_bits: f64,
    /// Rice without run mode.
    pub rice_norun_bits: u64,
}

/// `segments`: number of column segments per row (contexts reset at each).
pub fn evaluate(a: &Analysis, segments: usize) -> CostReport {
    let (w, h, hs) = (a.w, a.h, a.hs);
    let mut rice = 0u64;
    let mut ent = 0.0f64;
    let mut rice_norun = 0u64;
    let seg_w = w.div_ceil(segments.max(1));
    for (s, r) in a.res.iter().enumerate() {
        let bits = a.slot_bits()[s];
        let raw_bits = bits.max(1);
        let nvals = 1usize << bits.max(1);
        // classes + POST(=CLASSES) ; run lengths in RUN hist
        let mut hists: Vec<Hist> = (0..=CLASSES).map(|_| Hist::new(nvals + 1)).collect();
        let mut hists_norun: Vec<Hist> = (0..CLASSES).map(|_| Hist::new(nvals + 1)).collect();
        let mut run_hist = Hist::new(seg_w + 2);
        for y0 in (0..h).step_by(hs) {
            let y1 = (y0 + hs).min(h);
            for y in y0..y1 {
                for sx in (0..w).step_by(seg_w) {
                    let ex = (sx + seg_w).min(w);
                    let mut pending = 0usize;
                    let mut post = false;
                    for x in sx..ex {
                        let i = y * w + x;
                        let uw = if x > sx { Some(r[i - 1] as u32) } else { None };
                        let un = if y > y0 { Some(r[i - w] as u32) } else { None };
                        let sum = match (uw, un) {
                            (Some(a), Some(b)) => a + b,
                            (Some(a), None) => 2 * a,
                            (None, Some(b)) => 2 * b,
                            (None, None) => 0,
                        };
                        let class = class_of(sum);
                        let u = r[i] as u32;
                        hists_norun[class].add(u);
                        if pending > 0 {
                            pending -= 1;
                            continue;
                        }
                        if post {
                            post = false;
                            debug_assert!(u >= 1);
                            hists[CLASSES].add(u - 1);
                            continue;
                        }
                        if class == 0 {
                            let mut l = 0;
                            while x + l < ex && r[i + l] == 0 {
                                l += 1;
                            }
                            run_hist.add(l as u32);
                            if l > 0 {
                                pending = l - 1;
                                post = x + l < ex;
                            } else {
                                hists[CLASSES].add(u - 1);
                            }
                        } else {
                            hists[class].add(u);
                        }
                    }
                }
            }
        }
        for hh in &hists {
            let (_, c) = hh.best_rice(raw_bits);
            rice += c;
        }
        let (_, rc) = run_hist.best_rice(16);
        rice += rc;
        for hh in &hists_norun {
            rice_norun += hh.best_rice(raw_bits).1;
            ent += hh.entropy_bits();
        }
    }
    CostReport {
        rice_bits: rice,
        entropy_bits: ent,
        rice_norun_bits: rice_norun,
    }
}

/// Scale-normalized coding: per context class a shift `sh`, token of
/// `u >> sh` (hybrid k/m) coded with ONE table per slot (plus run tokens),
/// `sh` low bits raw. Returns ideal tANS bits per log_m (and inf if too
/// many symbols), plus the bits with 4 Huffman-like contexts as reference.
pub fn eval_scaled(a: &Analysis, k: u32, m: u32, log_ms: &[u32], iters: usize) -> Vec<f64> {
    let (w, h, hs, sw) = (a.w, a.h, a.hs, a.sw);
    let mut totals = vec![0.0f64; log_ms.len()];
    for (s, r) in a.res.iter().enumerate() {
        let bits = a.slot_bits()[s].max(1);
        // Collect (class, u) or run lengths, lane order.
        let mut vals: Vec<(u8, u32)> = Vec::new();
        let mut runs: Vec<u32> = Vec::new();
        for y0 in (0..h).step_by(hs) {
            let y1 = (y0 + hs).min(h);
            for xs in (0..w).step_by(sw) {
                let xe = (xs + sw).min(w);
                let lw = xe - xs;
                let n = (y1 - y0) * lw;
                let at = |j: usize| (y0 + j / lw) * w + xs + j % lw;
                let mut j = 0;
                while j < n {
                    let i = at(j);
                    let x = j % lw;
                    let uw = if x > 0 { Some(r[i - 1] as u32) } else { None };
                    let un = if j >= lw { Some(r[i - w] as u32) } else { None };
                    let mut sum = match (uw, un) {
                        (Some(a), Some(b)) => a + b,
                        (Some(a), None) => 2 * a,
                        (None, Some(b)) => 2 * b,
                        (None, None) => 0,
                    };
                    if s >= 1 && s < 3 && a.res.len() >= 3 && a.slot_bits().len() >= 3 {
                        sum += a.res[0][i] as u32;
                    }
                    let class = class_of(sum) as u8;
                    let u = r[i] as u32;
                    if u == 0 {
                        let mut len = 1;
                        while j + len < n && r[at(j + len)] == 0 {
                            len += 1;
                        }
                        if len >= 4 {
                            runs.push(len as u32);
                            j += len;
                            continue;
                        }
                    }
                    vals.push((class, u));
                    j += 1;
                }
            }
        }
        let tok = |v: u32| -> (u32, u32) {
            if v < (1 << k) {
                (v, 0)
            } else {
                let msb = 31 - v.leading_zeros();
                let ne = msb - m;
                (
                    (1 << k) + (msb - k) * (1 << m) + ((v >> ne) & ((1 << m) - 1)),
                    ne,
                )
            }
        };
        let vt = if k >= bits {
            1u32 << bits
        } else {
            (1 << k) + (bits - k) * (1 << m)
        } as usize;
        let rtok = |l: u32| -> (u32, u32) {
            let v = l - 1;
            if v < 16 {
                (v, 0)
            } else {
                let msb = 31 - v.leading_zeros();
                let ne = msb - 1;
                (16 + (msb - 4) * 2 + ((v >> ne) & 1), ne)
            }
        };
        let asz = vt + 16 + 40;
        let mut shift = [0u32; CLASSES];
        for c in 0..CLASSES {
            shift[c] = (c as u32).saturating_sub(3).min(bits.saturating_sub(1));
        }
        let mut best_for_logm = vec![f64::INFINITY; log_ms.len()];
        for _it in 0..iters.max(1) {
            // Histogram under current shifts.
            let mut hist = vec![0u64; asz];
            let mut raw = 0u64;
            for &(c, u) in &vals {
                let sh = shift[c as usize];
                let (t, ne) = tok(u >> sh);
                hist[t as usize] += 1;
                raw += (sh + ne) as u64;
            }
            for &l in &runs {
                let (t, ne) = rtok(l);
                hist[vt + t as usize] += 1;
                raw += ne as u64;
            }
            for (li, &lm) in log_ms.iter().enumerate() {
                let c = match crate::normalize(&hist, lm) {
                    Some(q) => crate::tans_cost(&hist, &q, lm) + raw as f64,
                    None => f64::INFINITY,
                };
                best_for_logm[li] = best_for_logm[li].min(c);
            }
            // Re-pick shifts per class under the current (ideal) code lengths.
            let tot: u64 = hist.iter().sum();
            let len_of = |t: usize| -> f64 {
                let p = (hist[t].max(1)) as f64 / tot as f64;
                -p.log2()
            };
            let mut per_class: Vec<Vec<u32>> = vec![Vec::new(); CLASSES];
            for &(c, u) in &vals {
                per_class[c as usize].push(u);
            }
            for c in 0..CLASSES {
                if per_class[c].is_empty() {
                    continue;
                }
                let mut best = (f64::INFINITY, 0);
                for sh in 0..bits {
                    let cost: f64 = per_class[c]
                        .iter()
                        .map(|&u| {
                            let (t, ne) = tok(u >> sh);
                            len_of(t as usize) + (sh + ne) as f64
                        })
                        .sum();
                    if cost < best.0 {
                        best = (cost, sh);
                    }
                }
                shift[c] = best.1;
            }
        }
        for li in 0..log_ms.len() {
            totals[li] += best_for_logm[li];
        }
    }
    totals
}

/// Rice in lane layout (runs continue across rows within a lane).
/// `fine`: classes at half-octave resolution. Returns (rice_bits, rice_bits_escape_check).
pub fn eval_rice_lanes(a: &Analysis, fine: bool, run_any: bool) -> u64 {
    let (w, h, hs, sw) = (a.w, a.h, a.hs, a.sw);
    let ncls = if fine { 2 * CLASSES } else { CLASSES };
    let cls = |s: u32| -> usize {
        let b = class_of(s);
        if !fine || b < 2 {
            if fine { 2 * b } else { b }
        } else {
            (2 * b + ((s >> (b - 2)) & 1) as usize).min(2 * CLASSES - 1)
        }
    };
    let mut total = 0u64;
    for (s, r) in a.res.iter().enumerate() {
        let bits = a.slot_bits()[s].max(1);
        let nvals = 1usize << bits;
        let mut hists: Vec<Hist> = (0..=ncls).map(|_| Hist::new(nvals + 1)).collect();
        let mut run_hist = Hist::new(1 << 16);
        let mut flags = 0u64;
        for y0 in (0..h).step_by(hs) {
            let y1 = (y0 + hs).min(h);
            for xs in (0..w).step_by(sw) {
                let xe = (xs + sw).min(w);
                let lw = xe - xs;
                let n = (y1 - y0) * lw;
                let at = |j: usize| (y0 + j / lw) * w + xs + j % lw;
                let mut j = 0;
                let mut post = false;
                while j < n {
                    let i = at(j);
                    let x = j % lw;
                    let uw = if x > 0 { Some(r[i - 1] as u32) } else { None };
                    let un = if j >= lw { Some(r[i - w] as u32) } else { None };
                    let mut sum = match (uw, un) {
                        (Some(a), Some(b)) => a + b,
                        (Some(a), None) => 2 * a,
                        (None, Some(b)) => 2 * b,
                        (None, None) => 0,
                    };
                    if s >= 1 && s < 3 && a.slot_bits().len() >= 3 {
                        sum += a.res[0][i] as u32;
                    }
                    let c = cls(sum);
                    let u = r[i] as u32;
                    if post {
                        post = false;
                        hists[ncls].add(u - 1);
                        j += 1;
                        continue;
                    }
                    let run_mode = if run_any { true } else { sum == 0 };
                    if run_mode {
                        let mut l = 0;
                        while j + l < n && r[at(j + l)] == 0 {
                            l += 1;
                        }
                        if run_any {
                            // 1 flag bit per symbol: run (>= 4 zeros) or value.
                            flags += 1;
                            if l >= 4 {
                                run_hist.add((l - 4).min(65535) as u32);
                                j += l;
                                continue;
                            }
                            hists[c].add(u);
                            j += 1;
                            continue;
                        }
                        run_hist.add(l.min(65535) as u32);
                        if l > 0 {
                            j += l;
                            post = j < n;
                        } else {
                            hists[ncls].add(u - 1);
                            j += 1;
                        }
                        continue;
                    }
                    hists[c].add(u);
                    j += 1;
                }
            }
        }
        for hh in &hists {
            total += hh.best_rice(bits).1;
        }
        total += run_hist.best_rice(16).1 + flags;
    }
    total
}
