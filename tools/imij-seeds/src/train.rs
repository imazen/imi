//! Seed-bank training: picks at most `k` seeds per table kind
//! that maximize the bits saved over a training set of tables.
//!
//! A table `r` (symbol counts `c_r`, explicit cost `E_r` = table bits + data
//! bits under its own table) saves `E_r - index_bits - sum_s c_r[s] * l(s)`
//! with a seed whose per-symbol cost is `l` (rANS: `10 - log2 f`, Huffman:
//! the code length), or nothing. The bank maximizes the total over all
//! tables of the best saving. Greedy facility location (lazy, CELF) over
//! candidate seeds picks the initial set; Lloyd steps then rebuild each
//! seed from the pooled counts of the tables it serves (the cross-entropy
//! minimizer for that assignment) until the total stops improving.

use imij::seeds::codec;

/// One training table.
#[derive(Clone, Debug)]
pub struct Rec {
    /// (symbol, count), count > 0.
    pub syms: Vec<(u16, u32)>,
    /// Bits of the explicit table plus its data bits.
    pub explicit: f64,
}

/// Training knobs.
#[derive(Clone, Debug)]
pub struct TrainCfg {
    /// Seeds to pick (index bits follow: ceil(log2 k)).
    pub k: usize,
    /// Candidate seeds for the greedy pass.
    pub candidates: usize,
    /// Lloyd refinement passes.
    pub lloyd: usize,
    /// rANS seed weight precision (1..=8).
    pub prec: u32,
    /// Pooled counts are scaled to this total before quantizing.
    pub scale: u64,
    /// Prior mass mixed into candidate seeds (fraction of the record's own).
    pub prior: f64,
    /// Threads.
    pub threads: usize,
}

impl Default for TrainCfg {
    fn default() -> Self {
        TrainCfg {
            k: 64,
            candidates: 2000,
            lloyd: 8,
            prec: 6,
            scale: 4096,
            prior: 0.25,
            threads: 8,
        }
    }
}

/// A trained kind: each seed in its kind's table grammar, byte-aligned.
#[derive(Clone, Debug)]
pub struct Trained {
    pub seeds: Vec<Vec<u8>>,
    /// Training tables served by each seed.
    pub served: Vec<usize>,
    /// Total training saving, bits.
    pub saving: f64,
    /// Training tables that use a seed.
    pub using: usize,
}

/// A seed under training: its coded form and per-symbol costs.
#[derive(Clone)]
struct Cand {
    bytes: Vec<u8>,
    cost: Vec<f32>,
}

/// Split-Huffman count (4) and width (5) kinds.
fn is_huffman(kind: u8) -> bool {
    kind == 4 || kind == 5
}

/// Bits of frequency `f` of 1024.
fn bits_of(f: u32) -> f32 {
    (10.0 - (f as f64).log2()) as f32
}

/// Code lengths as the bank stores them: 4 bits each, LSB-first.
fn pack_lengths(lens: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; lens.len().div_ceil(2)];
    for (i, &l) in lens.iter().enumerate() {
        out[i / 2] |= (l & 15) << (4 * (i % 2));
    }
    out
}

fn unpack_lengths(b: &[u8], alpha: usize) -> Vec<u8> {
    (0..alpha)
        .map(|i| b.get(i / 2).map_or(0, |v| (v >> (4 * (i % 2))) & 15))
        .collect()
}

/// Builds a seed from (non-negative, f64) symbol masses.
fn make(kind: u8, alpha: usize, mass: &[f64], cfg: &TrainCfg) -> Option<Cand> {
    let total: f64 = mass.iter().sum();
    if total <= 0.0 {
        return None;
    }
    if is_huffman(kind) {
        let counts: Vec<u64> = mass
            .iter()
            .map(|&m| {
                if m > 0.0 {
                    ((m / total * cfg.scale as f64).round() as u64).max(1)
                } else {
                    0
                }
            })
            .collect();
        let lens = codec::huffman_lengths(&counts);
        let bytes = pack_lengths(&lens);
        let cost = lens
            .iter()
            .map(|&l| if l == 0 { f32::INFINITY } else { l as f32 })
            .collect();
        return Some(Cand { bytes, cost });
    }
    let counts: Vec<u64> = mass
        .iter()
        .map(|&m| {
            if m > 0.0 {
                ((m / total * cfg.scale as f64).round() as u64).max(1)
            } else {
                0
            }
        })
        .collect();
    let weights: Vec<u32> = counts
        .iter()
        .map(|&c| codec::quantize_weight(c, cfg.prec))
        .collect();
    let freq = codec::weights_to_freq(&weights)?;
    let bytes = codec::write_weights(&weights, cfg.prec);
    // The decoder reads exactly this table back.
    debug_assert_eq!(codec::read_weights(&bytes, alpha).unwrap().0, weights);
    let cost = freq
        .iter()
        .map(|&f| if f == 0 { f32::INFINITY } else { bits_of(f) })
        .collect();
    Some(Cand { bytes, cost })
}

/// Saving of record `r` with seed costs `cost` (may be negative).
#[inline]
fn saving(r: &Rec, cost: &[f32], idx: f64) -> f64 {
    let mut bits = idx;
    for &(s, c) in &r.syms {
        bits += c as f64 * cost[s as usize] as f64;
    }
    r.explicit - bits
}

/// Parallel map over chunks of `0..n`.
fn par<T: Send>(n: usize, threads: usize, f: impl Fn(usize, usize) -> T + Sync) -> Vec<T> {
    let threads = threads.max(1).min(n.max(1));
    let chunk = n.div_ceil(threads).max(1);
    std::thread::scope(|sc| {
        let hs: Vec<_> = (0..threads)
            .map(|t| {
                let f = &f;
                sc.spawn(move || f(t * chunk, ((t + 1) * chunk).min(n)))
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    })
}

/// Trains `cfg.k` seeds for tables of `kind` with `alpha` symbols.
pub fn train(kind: u8, alpha: usize, recs: &[Rec], cfg: &TrainCfg) -> Trained {
    let k = cfg.k.max(1);
    let idx = if k <= 1 {
        0.0
    } else {
        (32 - (k as u32 - 1).leading_zeros()) as f64
    };
    // Global distribution (prior for candidates).
    let mut global = vec![0f64; alpha];
    for r in recs {
        let n: u64 = r.syms.iter().map(|x| x.1 as u64).sum();
        for &(s, c) in &r.syms {
            global[s as usize] += c as f64 / n as f64;
        }
    }
    let gsum: f64 = global.iter().sum::<f64>().max(1e-9);
    for g in &mut global {
        *g /= gsum;
    }
    // Candidates: records spread over the set (deterministic stride), each
    // mixed with the prior.
    let nc = cfg.candidates.min(recs.len());
    let mut cands: Vec<Cand> = Vec::with_capacity(nc + 1);
    if let Some(c) = make(kind, alpha, &global, cfg) {
        cands.push(c);
    }
    for i in 0..nc {
        let r = &recs[i * recs.len() / nc];
        let n: f64 = r.syms.iter().map(|x| x.1 as f64).sum();
        let mut mass: Vec<f64> = global.iter().map(|g| g * n * cfg.prior).collect();
        for &(s, c) in &r.syms {
            mass[s as usize] += c as f64;
        }
        // Huffman seeds keep every symbol the kind uses; rANS seeds keep
        // the record's symbols plus the prior's.
        if let Some(c) = make(kind, alpha, &mass, cfg) {
            cands.push(c);
        }
    }
    // Greedy facility location with lazy gains.
    let mut cur = vec![0f64; recs.len()];
    let gain_of = |c: &Cand, cur: &[f64]| -> f64 {
        par(recs.len(), cfg.threads, |a, b| {
            let mut g = 0.0;
            for i in a..b {
                let s = saving(&recs[i], &c.cost, idx);
                if s > cur[i] {
                    g += s - cur[i];
                }
            }
            g
        })
        .into_iter()
        .sum()
    };
    let mut bound: Vec<f64> = cands.iter().map(|c| gain_of(c, &cur)).collect();
    let mut chosen: Vec<Cand> = Vec::new();
    let mut used = vec![false; cands.len()];
    while chosen.len() < k {
        // Highest bound first; recompute until it stays on top.
        let mut best = None;
        while let Some(j) = (0..cands.len())
            .filter(|&j| !used[j])
            .max_by(|&a, &b| bound[a].total_cmp(&bound[b]))
        {
            let g = gain_of(&cands[j], &cur);
            bound[j] = g;
            let next = (0..cands.len())
                .filter(|&x| !used[x] && x != j)
                .map(|x| bound[x])
                .fold(f64::MIN, f64::max);
            if g >= next {
                best = Some((j, g));
                break;
            }
        }
        let Some((j, g)) = best else { break };
        if g <= 0.0 {
            break;
        }
        used[j] = true;
        for (i, r) in recs.iter().enumerate() {
            let s = saving(r, &cands[j].cost, idx);
            if s > cur[i] {
                cur[i] = s;
            }
        }
        chosen.push(cands[j].clone());
    }
    // Lloyd: assign, pool, rebuild.
    let assign = |seeds: &[Cand]| -> (Vec<Option<usize>>, f64) {
        let parts = par(recs.len(), cfg.threads, |a, b| {
            let mut out = Vec::with_capacity(b - a);
            let mut tot = 0.0;
            for r in &recs[a..b] {
                let mut best: Option<(usize, f64)> = None;
                for (j, c) in seeds.iter().enumerate() {
                    let s = saving(r, &c.cost, idx);
                    if s > 0.0 && best.is_none_or(|b| s > b.1) {
                        best = Some((j, s));
                    }
                }
                tot += best.map_or(0.0, |b| b.1);
                out.push(best.map(|b| b.0));
            }
            (out, tot)
        });
        let mut a = Vec::with_capacity(recs.len());
        let mut tot = 0.0;
        for (o, t) in parts {
            a.extend(o);
            tot += t;
        }
        (a, tot)
    };
    let (mut asg, mut total) = assign(&chosen);
    for _ in 0..cfg.lloyd {
        let mut pools = vec![vec![0f64; alpha]; chosen.len()];
        for (r, a) in recs.iter().zip(&asg) {
            if let Some(j) = a {
                for &(s, c) in &r.syms {
                    pools[*j][s as usize] += c as f64;
                }
            }
        }
        let mut next = chosen.clone();
        for (j, p) in pools.iter().enumerate() {
            if let Some(c) = make(kind, alpha, p, cfg) {
                next[j] = c;
            }
        }
        let (a2, t2) = assign(&next);
        if t2 <= total {
            break;
        }
        chosen = next;
        asg = a2;
        total = t2;
    }
    let mut served = vec![0usize; chosen.len()];
    for j in asg.iter().flatten() {
        served[*j] += 1;
    }
    // Most-used seeds first (stable, deterministic).
    let mut order: Vec<usize> = (0..chosen.len()).collect();
    order.sort_by(|&a, &b| served[b].cmp(&served[a]).then(a.cmp(&b)));
    Trained {
        seeds: order.iter().map(|&j| chosen[j].bytes.clone()).collect(),
        served: order.iter().map(|&j| served[j]).collect(),
        saving: total,
        using: asg.iter().filter(|a| a.is_some()).count(),
    }
}

/// Alphabet of a table kind (lossless: of token scheme `tc`).
pub fn alphabet(kind: u8, tc: [u8; 3]) -> usize {
    codec::alphabet(kind, tc)
}

/// Bits saved by the best of `seeds` for each record (0 when none helps),
/// with `index_bits` per seeded table: an evaluation of a trained bank on
/// other tables.
pub fn evaluate(kind: u8, alpha: usize, seeds: &[Vec<u8>], recs: &[Rec]) -> (f64, usize) {
    let idx = if seeds.len() <= 1 {
        0.0
    } else {
        (32 - (seeds.len() as u32 - 1).leading_zeros()) as f64
    };
    let costs: Vec<Vec<f32>> = seeds
        .iter()
        .map(|b| {
            if is_huffman(kind) {
                unpack_lengths(b, alpha)
                    .iter()
                    .map(|&l| if l == 0 { f32::INFINITY } else { l as f32 })
                    .collect()
            } else {
                let (w, _) = codec::read_weights(b, alpha).unwrap();
                let f = codec::weights_to_freq(&w).unwrap();
                f.iter()
                    .map(|&f| if f == 0 { f32::INFINITY } else { bits_of(f) })
                    .collect()
            }
        })
        .collect();
    let mut tot = 0.0;
    let mut n = 0;
    for r in recs {
        let best = costs
            .iter()
            .map(|c| saving(r, c, idx))
            .fold(0.0f64, f64::max);
        if best > 0.0 {
            tot += best;
            n += 1;
        }
    }
    (tot, n)
}

/// One seed as text: rANS `p<prec> | symbol:weight ...` (nonzero weights),
/// Huffman `symbol:length ...` (nonzero lengths).
pub fn describe(kind: u8, alpha: usize, seed: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    if is_huffman(kind) {
        for (s, l) in unpack_lengths(seed, alpha).into_iter().enumerate() {
            if l > 0 {
                let _ = write!(out, "{s}:{l} ");
            }
        }
    } else {
        match codec::read_weights(seed, alpha) {
            Ok((w, p)) => {
                let _ = write!(out, "p{p} |");
                for (s, &v) in w.iter().enumerate().filter(|(_, v)| **v > 0) {
                    let _ = write!(out, " {s}:{v}");
                }
            }
            Err(e) => {
                let _ = write!(out, "undecodable: {e:?}");
            }
        }
    }
    out.trim_end().into()
}
