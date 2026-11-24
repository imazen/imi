//! Coefficient tokens, contexts and entropy tables of the lossy mode.
//!
//! Per block and channel a lane codes an end-of-block token (the zigzag
//! index of the last nonzero AC coefficient, 0 = none), then run-level
//! tokens: each nonzero coefficient is one joint symbol (run of 0-3 zeros
//! before it) * 32 + (magnitude class, see [`MAG_CLASSES`]), followed by
//! raw bits (sign, then the magnitude remainder); [`ZRL`] skips four zeros. Contexts depend only
//! on the lane's own state: the EOB context on the previous luma EOB of the
//! lane (luma) or the block's luma EOB (chroma), the AC context on the
//! channel, the next position k and how many positions remain before the
//! EOB. Lanes never read each other's data.
//!
//! Decoding rules (normative, also for corrupt streams; every token
//! advances k, so a lane's token count is bounded):
//! * EOB token e: e = 0 ends the channel, else k = 1.
//! * ZRL: k += 4; k > e ends the channel.
//! * (run r, class c): p = k + r; if p <= e the coefficient at zigzag
//!   index p is set; p >= e ends the channel, else k = p + 1.

use alloc::vec;
use alloc::vec::Vec;

use crate::Error;
use crate::bits::{BitReader, get_gamma};
#[cfg(feature = "encode")]
use crate::bits::{BitWriter, put_gamma};
#[cfg(feature = "encode")]
use crate::rans::write_weights;
use crate::rans::{LOG_M, M, choose_table, read_weights, weights_to_freq};

pub(crate) const ZIGZAG: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// Coded channels (3 for RGB input).
pub(crate) const NCH: usize = 3;
pub(crate) const EOB_CLASSES: usize = 8;
pub(crate) const K_CLASSES: usize = 63;
pub(crate) const REM_CLASSES: usize = 6;
/// Raw contexts: EOB (channel x class), then AC (channel x k class x
/// remaining class).
pub(crate) const EOB_CTX: usize = NCH * EOB_CLASSES;
pub(crate) const NRAW: usize = EOB_CTX + NCH * K_CLASSES * REM_CLASSES;
/// Most tables a stream may carry (decoders size their tables to this).
pub(crate) const MAX_TABLES: usize = 32;
/// Raw contexts padded to a power of two (decoders index a padded map).
pub(crate) const NRAW_PAD: usize = 2048;
const _: () = assert!(NRAW <= NRAW_PAD);
/// Table alphabet: EOB symbols 0..64, AC symbols run * 32 + class, ZRL.
pub(crate) const ALPHA: usize = 128;
/// Longest run in a run-level symbol.
pub(crate) const MAX_RUN: u32 = 3;
/// Magnitude classes: |v| = 1..=4 are classes 0..=3; above, class
/// 4 + bitlen(|v| - 5), with the bits below the leading one raw. |v| < 2^22.
pub(crate) const MAG_CLASSES: u32 = 27;
/// Skip-four-zeros symbol.
pub(crate) const ZRL: u32 = 127;
const _: () = assert!(MAX_RUN * 32 + MAG_CLASSES <= ZRL);

/// Entropy coding of the AC coefficients (header byte after the matrix id).
/// Wire value 1 (packed rANS) is retired 2026-10-10 and wire value 2 (split
/// rANS) 2026-10-11; both are reserved: decoders reject them as
/// `Error::Unsupported`.
///
/// The lossy `entropy` field is open (SPEC 0.3), so the enum is
/// `#[non_exhaustive]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LossyEntropy {
    /// Run-level tokens (smaller files).
    RunLevel = 0,
    /// Split with the counts and widths in static Huffman codes (contexts
    /// that follow from the counts alone; see `huf`): no per-symbol state
    /// machine; 1.6-3.7% larger files than the retired split rANS mode
    /// (photo 1407 at XYB / YCoCg q50-q95), 8-19% faster to decode on one
    /// thread (Zen 4).
    SplitHuf = 3,
}

impl LossyEntropy {
    pub(crate) fn from_u8(v: u8) -> Option<LossyEntropy> {
        match v {
            0 => Some(LossyEntropy::RunLevel),
            3 => Some(LossyEntropy::SplitHuf),
            _ => None,
        }
    }

    /// Raw contexts of this mode.
    pub(crate) fn nraw(self) -> usize {
        match self {
            LossyEntropy::RunLevel => NRAW,
            LossyEntropy::SplitHuf => PK_NRAW,
        }
    }

    /// Symbols the first table group (EOB / group count) may contain.
    fn first_ok(self, s: usize) -> bool {
        match self {
            LossyEntropy::RunLevel => s < 64,
            LossyEntropy::SplitHuf => s <= PK_GROUPS,
        }
    }

    /// Symbols the second table group (AC tokens / widths) may contain.
    fn second_ok(self, s: usize) -> bool {
        match self {
            LossyEntropy::RunLevel => valid_ac_symbol(s),
            LossyEntropy::SplitHuf => s < PK_WIDTHS,
        }
    }
}

// ---- group coding (split-Huffman's count/width side data) ----
//
// Per block and channel: a count token n (0..=16, context as the EOB token)
// = number of coded groups; group gi covers zigzag positions 1 + 4 gi ..
// (positions 61..63 for gi = 15). Per group a width token w (0..=22) =
// bit length of the largest |coefficient|, context
// (ch, band(gi), min(L - gi, 5)) with L the block's luma count (chroma) or
// the channel's own count (luma).
// Split-Huffman codes counts and widths as its Huffman side data; the
// coefficient magnitudes (len * w bits per group, coefficient j at bits
// j * w) and sign bits (one per nonzero, 1 = negative) are two plain bit
// streams in block order, located from the widths by prefix sums.
// The retired packed mode pulled its raw bits from the lane's rANS state
// after each width token; the retired split mode coded the same count and
// width tokens in the rANS lanes.

/// Groups per channel.
pub(crate) const PK_GROUPS: usize = 16;
/// Width alphabet (|v| < 2^22).
pub(crate) const PK_WIDTHS: usize = 23;
/// Context band of each group.
pub(crate) const PK_BAND: [u8; PK_GROUPS] = [0, 1, 2, 3, 3, 4, 4, 4, 5, 5, 5, 5, 5, 5, 5, 5];
/// Raw contexts: count (as EOB_CTX), then widths.
pub(crate) const PK_NRAW: usize = EOB_CTX + NCH * 6 * 6 * 6;
const _: () = assert!(PK_NRAW <= NRAW_PAD);

/// Number of coefficients in group gi.
#[inline]
#[cfg(feature = "encode")]
pub(crate) fn pk_len(gi: usize) -> usize {
    if gi == PK_GROUPS - 1 { 3 } else { 4 }
}

#[inline]
pub(crate) fn eob_class(e: u32) -> usize {
    // Bit length: 0, 1, 2-3, 4-7, ..., 32-63.
    (32 - e.leading_zeros()) as usize
}

/// Class of the number of AC positions left before the EOB (eob - k).
#[inline]
pub(crate) fn rem_class(rem: u32) -> usize {
    // floor(log2(rem + 1)), at most 5: 0, 1-2, 3-6, 7-14, 15-30, 31+.
    ((31 - (rem + 1).leading_zeros()) as usize).min(REM_CLASSES - 1)
}

/// `ref_eob`: the lane's previous luma EOB (luma) or this block's luma EOB
/// (chroma).
#[inline]
pub(crate) fn eob_ctx(ch: usize, ref_eob: u32) -> usize {
    ch * EOB_CLASSES + eob_class(ref_eob)
}

/// Context of an AC token read at next position `k` (1..=eob).
#[inline]
pub(crate) fn ac_ctx(ch: usize, k: u32, eob: u32) -> usize {
    EOB_CTX + (ch * K_CLASSES + k as usize - 1) * REM_CLASSES + rem_class(eob.saturating_sub(k))
}

/// Run-level symbol of a nonzero coefficient: (symbol, extra bits value,
/// extra bit count). Extra bits: sign in bit 0, magnitude remainder above.
#[inline]
#[cfg(feature = "encode")]
pub(crate) fn rl_token(run: u32, v: i32) -> (u32, u32, u32) {
    debug_assert!(run <= MAX_RUN && v != 0);
    let a = v.unsigned_abs();
    let sign = (v < 0) as u32;
    if a <= 4 {
        return (run * 32 + a - 1, sign, 1);
    }
    let w = a - 5;
    let nb = 32 - w.leading_zeros();
    let rem = if nb >= 1 { w - (1 << (nb - 1)) } else { 0 };
    (
        run * 32 + 4 + nb,
        (rem << 1) | sign,
        1 + nb.saturating_sub(1),
    )
}

/// Extra bit count of AC symbol `s` (0 for ZRL).
#[inline]
pub(crate) fn rl_extra_bits(s: u32) -> u32 {
    if s == ZRL {
        0
    } else {
        1 + (s & 31).saturating_sub(5)
    }
}

/// Coefficient from a run-level symbol's class and its extra bits.
#[inline]
pub(crate) fn rl_value(c: u32, extra: u32) -> i32 {
    let mag = if c < 4 {
        c + 1
    } else if c == 4 {
        5
    } else {
        5 + (1u32 << (c - 5)) + (extra >> 1)
    };
    let m = mag as i32;
    if extra & 1 != 0 { -m } else { m }
}

/// True for symbols an AC table may contain.
pub(crate) fn valid_ac_symbol(s: usize) -> bool {
    s as u32 == ZRL || ((s as u32 >> 5) <= MAX_RUN && (s as u32 & 31) < MAG_CLASSES)
}

/// Entropy tables: `ctx_map` maps raw contexts to tables; tables
/// `0..n_eob` serve EOB contexts, `n_eob..` AC contexts.
#[derive(Clone)]
pub(crate) struct Tables {
    pub mode: LossyEntropy,
    pub ctx_map: Vec<u8>,
    pub n_eob: usize,
    pub freq: Vec<Vec<u32>>,
    weights: Vec<Vec<u32>>,
    prec: Vec<u32>,
    /// With selectors (seed bank): each table's seed, `None` = explicit.
    pub seeds: Option<(&'static crate::seeds::Bank, Vec<Option<u8>>)>,
}

/// Cross-entropy (bits) of the sparse histogram `c` ((symbol, count)
/// pairs) under the log-probabilities `logp`.
fn xent(c: &[(usize, u64)], logp: &[f64]) -> f64 {
    c.iter().map(|&(s, n)| -(n as f64) * logp[s]).sum()
}

/// Log-probabilities of `c` under its own total, written to `out` (which
/// may be any recycled buffer). Symbols `c` does not contain share one
/// value: they are filled with it, not logged each.
fn log_probs_into(c: &[u64], out: &mut Vec<f64>) {
    let tot: f64 = c.iter().sum::<u64>() as f64 + 0.5 * c.len() as f64;
    out.clear();
    out.resize(c.len(), crate::mathx::log2(0.5 / tot));
    for (s, &n) in c.iter().enumerate() {
        if n > 0 {
            out[s] = crate::mathx::log2((n as f64 + 0.5) / tot);
        }
    }
}

/// The parts of [`cluster`] that do not depend on `k`: the contexts'
/// sparse histograms, each context's cross-entropy under itself, and the
/// farthest-point center sequence. The sequence for `k` centers is the
/// prefix of the sequence for any larger count, so a search over several
/// `k` extends it lazily instead of rebuilding it per count.
pub(crate) struct ClusterSet<'a> {
    hist: &'a [Vec<u64>],
    /// Nonzero (symbol, count) pairs per context.
    pub(crate) sparse: Vec<Vec<(usize, u64)>>,
    /// Contexts with at least one symbol.
    used: Vec<usize>,
    /// xent of each context under its own log-probabilities.
    own: Vec<f64>,
    /// Center histograms in farthest-point order (`seq[0]` first).
    seq: Vec<Vec<u64>>,
    /// Each context's best cross-entropy against `seq` so far.
    cur: Vec<f64>,
    /// A pick found nothing to gain (the init's `best.0 <= 0.0` break).
    stopped: bool,
    /// `seq_to`'s recycled log-probability buffer.
    lp: Vec<f64>,
    /// `run`'s per-run center log-probabilities and merged histograms
    /// (rebuilt every run; only the outer allocations persist).
    logs: Vec<Vec<f64>>,
    next: Vec<Vec<u64>>,
}

impl<'a> ClusterSet<'a> {
    pub(crate) fn new(hist: &'a [Vec<u64>]) -> Self {
        let sparse: Vec<Vec<(usize, u64)>> = (0..hist.len())
            .map(|i| {
                hist[i]
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| **n > 0)
                    .map(|(s, &n)| (s, n))
                    .collect()
            })
            .collect();
        let used: Vec<usize> = (0..hist.len()).filter(|&i| !sparse[i].is_empty()).collect();
        // (unused contexts' xent over an empty sparse row is 0 either way)
        let mut own = vec![0.0; hist.len()];
        let mut lp = Vec::new();
        for &i in &used {
            log_probs_into(&hist[i], &mut lp);
            own[i] = xent(&sparse[i], &lp);
        }
        let mut seq = Vec::new();
        // Farthest-point initialization weighted by symbol count.
        if let Some(&first) = used
            .iter()
            .max_by_key(|&&i| (hist[i].iter().sum::<u64>(), usize::MAX - i))
        {
            seq.push(hist[first].clone());
        }
        ClusterSet {
            hist,
            sparse,
            used,
            own,
            seq,
            cur: vec![f64::MAX; hist.len()],
            stopped: false,
            lp: Vec::new(),
            logs: Vec::new(),
            next: Vec::new(),
        }
    }

    /// Extends the center sequence to `k` entries (bounded by the number
    /// of used contexts), or to where a pick would gain nothing.
    fn seq_to(&mut self, k: usize) {
        let want = k.min(self.used.len());
        while self.seq.len() < want && !self.stopped {
            log_probs_into(self.seq.last().unwrap(), &mut self.lp);
            let mut best = (f64::MIN, 0usize);
            for &i in &self.used {
                self.cur[i] = self.cur[i].min(xent(&self.sparse[i], &self.lp));
                let gain = self.cur[i] - self.own[i];
                if gain > best.0 {
                    best = (gain, i);
                }
            }
            if best.0 <= 0.0 {
                self.stopped = true;
            } else {
                self.seq.push(self.hist[best.1].clone());
            }
        }
    }

    /// [`cluster`]'s assignment at `k`: `passes` k-means rounds from the
    /// `k`-center prefix of the center sequence. `seq` is not consumed, so
    /// a later call at a larger `k` extends it instead of rebuilding.
    pub(crate) fn run(&mut self, k: usize, passes: usize) -> Vec<usize> {
        let mut assign = vec![0usize; self.hist.len()];
        if self.used.is_empty() || k == 1 {
            return assign;
        }
        self.seq_to(k);
        let mut centers: Vec<Vec<u64>> = self.seq[..k.min(self.seq.len())].to_vec();
        self.logs.clear();
        self.logs.resize_with(centers.len(), Vec::new);
        self.next.clear();
        self.next.resize_with(centers.len(), Vec::new);
        for c in self.next.iter_mut() {
            c.resize(ALPHA, 0);
        }
        for _ in 0..passes {
            for (l, c) in self.logs.iter_mut().zip(&centers) {
                log_probs_into(c, l);
            }
            for &i in &self.used {
                let mut b = (f64::MAX, 0usize);
                for (j, l) in self.logs.iter().enumerate() {
                    let x = xent(&self.sparse[i], l);
                    if x < b.0 {
                        b = (x, j);
                    }
                }
                assign[i] = b.1;
            }
            for n in self.next.iter_mut() {
                n.fill(0);
            }
            for &i in &self.used {
                for &(s, n) in &self.sparse[i] {
                    self.next[assign[i]][s] += n;
                }
            }
            // Keep empty clusters' old centers.
            let mut changed = false;
            for (j, c) in self.next.iter().enumerate() {
                if c.iter().any(|&n| n > 0) {
                    changed |= centers[j] != *c;
                    centers[j] = c.clone();
                }
            }
            // An unchanged round repeats identically forever after.
            if !changed {
                break;
            }
        }
        assign
    }
}

/// k-means over context histograms with cross-entropy cost. Returns the
/// cluster of each context (contexts without symbols go to cluster 0).
/// (The encode paths cluster through [`KSearch`]'s [`ClusterSet`]s; this
/// wrapper is `lab_table_cost`'s.)
#[cfg(feature = "lab")]
pub(crate) fn cluster(hist: &[Vec<u64>], k: usize, passes: usize) -> Vec<usize> {
    if k == 1 || !hist.iter().any(|h| h.iter().any(|&n| n > 0)) {
        return vec![0usize; hist.len()];
    }
    ClusterSet::new(hist).run(k, passes)
}

impl Tables {
    /// No rANS tables (modes that code everything elsewhere): one table per
    /// group, every context mapped to it.
    pub(crate) fn placeholder(mode: LossyEntropy) -> Tables {
        let mut ctx_map = vec![1u8; mode.nraw()];
        ctx_map[..EOB_CTX].fill(0);
        let mut f = vec![0u32; ALPHA];
        f[0] = M;
        Tables {
            mode,
            ctx_map,
            n_eob: 1,
            freq: vec![f.clone(), f],
            weights: Vec::new(),
            prec: Vec::new(),
            seeds: None,
        }
    }

    /// Builds tables from raw-context histograms (`hist[ctx][sym]`).
    #[cfg_attr(
        any(not(target_arch = "x86_64"), not(feature = "std")),
        allow(dead_code)
    )]
    pub(crate) fn build(
        hist: &[Vec<u64>],
        max_eob: usize,
        max_ac: usize,
        mode: LossyEntropy,
    ) -> Tables {
        Self::build_with_effort(
            hist,
            max_eob,
            max_ac,
            mode,
            crate::effort::Effort::new(10.0).unwrap(),
        )
    }

    pub(crate) fn build_with_effort(
        hist: &[Vec<u64>],
        max_eob: usize,
        max_ac: usize,
        mode: LossyEntropy,
        effort: crate::effort::Effort,
    ) -> Tables {
        Self::build_tables(
            hist,
            max_eob,
            max_ac,
            mode,
            effort,
            None,
            false,
            u8::MAX,
            false,
        )
        .0
    }

    /// [`Self::build_with_effort`], then [`Self::apply_seeds`] with `bank`:
    /// the tables to code with and, when seeds replaced any, the explicit
    /// tables. These are the tables a file writes, so the table counts are
    /// chosen by coded size here ([`k_search_share`]; never for rate
    /// estimates). `k_mode`: the lab table-count override
    /// ([`crate::lossy::table_k_mode`]; `u8::MAX`, none, without `lab`).
    #[cfg(feature = "encode")]
    pub(crate) fn build_seeded(
        hist: &[Vec<u64>],
        max_eob: usize,
        max_ac: usize,
        mode: LossyEntropy,
        effort: crate::effort::Effort,
        bank: Option<&'static crate::seeds::Bank>,
        k_mode: u8,
    ) -> (Tables, Option<Tables>) {
        Self::build_tables(
            hist, max_eob, max_ac, mode, effort, bank, true, k_mode, false,
        )
    }

    /// Private experiment: retain the global policy while testing a separately
    /// budgeted model that may be trained once per unit.
    #[cfg(feature = "encode")]
    pub(super) fn build_local(
        hist: &[Vec<u64>],
        max_ac: usize,
        effort: crate::effort::Effort,
        bank: Option<&'static crate::seeds::Bank>,
        k_mode: u8,
    ) -> Tables {
        Self::build_tables(
            hist,
            4,
            max_ac,
            LossyEntropy::RunLevel,
            effort,
            bank,
            true,
            k_mode,
            true,
        )
        .0
    }

    /// [`Self::build_seeded`]; `written`: the file's own tables (not an
    /// estimate's), whose counts may be chosen by coded size; `k_mode`: the lab
    /// table-count override ([`crate::lossy::table_k_mode`]).
    fn build_tables(
        hist: &[Vec<u64>],
        max_eob: usize,
        max_ac: usize,
        mode: LossyEntropy,
        effort: crate::effort::Effort,
        bank: Option<&'static crate::seeds::Bank>,
        written: bool,
        k_mode: u8,
        local: bool,
    ) -> (Tables, Option<Tables>) {
        let nraw = mode.nraw();
        let share =
            k_search_share(effort, mode == LossyEntropy::RunLevel, k_mode).filter(|_| written);
        let effort = if local { effort } else { effort.table_search() };
        let passes = effort.budget(8);
        let (mut ke, mut ka) = (
            effort.budget(max_eob.clamp(1, 8)),
            effort.budget(max_ac.clamp(1, MAX_TABLES - 8)),
        );
        // The table counts that code smallest (benchmarks/coefgap_2026-10-09);
        // the search's priced groups are reused for the tables themselves.
        let mut ks = KSearch::new(hist, mode, passes, bank);
        if let Some(min_share) = share {
            (ke, ka) = ks.search(ke, ka, min_share);
        }
        ks.group(0, ke);
        ks.group(1, ka);
        let (ea, ne) = (ks.group_ref(0, ke).map.clone(), ks.group_ref(0, ke).n);
        let (aa, na) = (ks.group_ref(1, ka).map.clone(), ks.group_ref(1, ka).n);
        // LAB (coefgap study): fewer tables where that codes smaller.
        #[cfg(feature = "lab")]
        let ((ea, ne), (aa, na), remerged) = if written && k_mode == 1 {
            (
                merge_by_cost(&hist[..EOB_CTX], ea, ne),
                merge_by_cost(&hist[EOB_CTX..], aa, na),
                true,
            )
        } else {
            ((ea, ne), (aa, na), false)
        };
        #[cfg(not(feature = "lab"))]
        let remerged = false;
        let mut ctx_map = vec![0u8; nraw];
        for i in 0..nraw {
            ctx_map[i] = (if i < EOB_CTX {
                ea[i]
            } else {
                ne + aa[i - EOB_CTX]
            }) as u8;
        }
        // Unused contexts: repeat the previous mapping within the group (for
        // run-length coding of the map).
        for i in 1..nraw {
            if hist[i].iter().all(|&v| v == 0) && i != EOB_CTX {
                ctx_map[i] = ctx_map[i - 1];
            }
        }
        if hist[EOB_CTX].iter().all(|&v| v == 0) {
            ctx_map[EOB_CTX] = ne as u8;
        }
        let mut counts = vec![vec![0u64; ALPHA]; ne + na];
        for i in 0..nraw {
            let t = if i < EOB_CTX {
                ea[i]
            } else {
                ne + aa[i - EOB_CTX]
            };
            for (s, &n) in hist[i].iter().enumerate() {
                counts[t][s] += n;
            }
        }
        // Without a LAB re-merge the groups' histograms and prices are
        // `counts`' exactly.
        let priced: Option<Vec<&KTable>> = (!remerged).then(|| {
            ks.group_ref(0, ke)
                .tables
                .iter()
                .chain(&ks.group_ref(1, ka).tables)
                .collect()
        });
        let mut freq = Vec::new();
        let mut weights = Vec::new();
        let mut prec = Vec::new();
        #[cfg(feature = "std")]
        let dbg = std::env::var_os("IMIJ_TABLE_DEBUG").is_some();
        for (t, c) in counts.iter_mut().enumerate() {
            if c.iter().all(|&v| v == 0) {
                c[0] = 1;
            }
            let (w, f, p) = match priced.as_ref().map(|p| p[t]) {
                Some(kt) => (kt.weights.clone(), kt.freq.clone(), kt.prec),
                None => choose_table(c),
            };
            if let Some(kind) = crate::seeds::Kind::of_lossy(mode, t < ne) {
                crate::seeds::record_table(kind, [0; 3], c, || {
                    (
                        crate::rans::weights_cost(&w, p),
                        crate::seeds::rans_data_bits(c, &f).unwrap_or(0.0),
                    )
                });
            }
            #[cfg(feature = "std")]
            if dbg {
                let tot: u64 = c.iter().sum();
                let ideal: f64 = c
                    .iter()
                    .filter(|&&n| n > 0)
                    .map(|&n| -(n as f64) * crate::mathx::log2(n as f64 / tot as f64))
                    .sum();
                let real: f64 = c
                    .iter()
                    .zip(&f)
                    .filter(|(n, _)| **n > 0)
                    .map(|(&n, &q)| -(n as f64) * crate::mathx::log2(q as f64 / M as f64))
                    .sum();
                let used = c.iter().filter(|&&n| n > 0).count();
                std::eprintln!(
                    "table: {tot} syms, {used} used, ideal {:.0} real {:.0} bits (+{:.2}%)",
                    ideal,
                    real,
                    (real / ideal - 1.0) * 100.0
                );
            }
            weights.push(w);
            freq.push(f);
            prec.push(p);
        }
        let t = Tables {
            mode,
            ctx_map,
            n_eob: ne,
            freq,
            weights,
            prec,
            seeds: None,
        };
        if let Some(bank) = bank {
            let mut seeded = t.clone();
            match &priced {
                Some(p) => seeded.apply_seeds_priced(p, bank),
                None => seeded.apply_seeds(&counts, bank),
            }
            if seeded.seeds.is_some() {
                return (seeded, Some(t));
            }
        }
        (t, None)
    }

    /// [`Self::apply_seeds`] over tables the table-count search priced
    /// (`KTable::price` computes the same numbers in the same order).
    fn apply_seeds_priced(&mut self, priced: &[&KTable], bank: &'static crate::seeds::Bank) {
        if self.mode != LossyEntropy::RunLevel {
            return;
        }
        let n = self.freq.len();
        let mut gain = -(n as f64);
        for kt in priced {
            if let Some(s) = &kt.seed {
                gain += s.gain;
            }
        }
        if gain < crate::seeds::MIN_GAIN_BITS {
            return;
        }
        let mut sel = Vec::with_capacity(n);
        for (t, kt) in priced.iter().enumerate() {
            sel.push(kt.seed.as_ref().map(|s| {
                if let Some(kind) = crate::seeds::Kind::of_lossy(self.mode, t < self.n_eob) {
                    crate::seeds::record_seed(kind, [0; 3], s.index as u8, s.gain);
                }
                self.freq[t] = s.expanded.freq.clone();
                self.weights[t] = s.expanded.weights.clone();
                self.prec[t] = s.expanded.prec;
                s.index as u8
            }));
        }
        self.seeds = Some((bank, sel));
    }

    /// Replaces explicit tables by seeds of `bank` where that is cheaper
    /// (estimated coded bits), when the blob saves at least
    /// [`MIN_GAIN_BITS`](crate::seeds::MIN_GAIN_BITS) overall. `counts[t]` are
    /// the symbol counts table `t` codes. Run-level only.
    pub(crate) fn apply_seeds(&mut self, counts: &[Vec<u64>], bank: &'static crate::seeds::Bank) {
        if self.mode != LossyEntropy::RunLevel {
            return;
        }
        let n = self.freq.len();
        let mut pick: Vec<Option<(u8, crate::seeds::Expanded, f64)>> = Vec::with_capacity(n);
        let mut gain = -(n as f64);
        let sets = [true, false].map(|first| {
            let kind = crate::seeds::Kind::of_lossy(self.mode, first)?;
            crate::seeds::kind_set(bank, kind, [0; 3], ALPHA)
        });
        for t in 0..n {
            let Some(set) = &sets[(t >= self.n_eob) as usize] else {
                pick.push(None);
                continue;
            };
            let explicit = crate::rans::weights_cost(&self.weights[t], self.prec[t]) as f64
                + crate::seeds::rans_data_bits(&counts[t], &self.freq[t]).unwrap_or(f64::MAX);
            match set.best(&counts[t]) {
                Some((i, bits)) if bits + (set.seeds.index_bits() as f64) < explicit => {
                    let g = explicit - bits - set.seeds.index_bits() as f64;
                    gain += g;
                    pick.push(Some((i as u8, set.table(i), g)));
                }
                _ => pick.push(None),
            }
        }
        if gain < crate::seeds::MIN_GAIN_BITS {
            return;
        }
        let mut sel = Vec::with_capacity(n);
        for (t, p) in pick.into_iter().enumerate() {
            sel.push(p.map(|(i, e, g)| {
                if let Some(kind) = crate::seeds::Kind::of_lossy(self.mode, t < self.n_eob) {
                    crate::seeds::record_seed(kind, [0; 3], i, g);
                }
                self.freq[t] = e.freq;
                self.weights[t] = e.weights;
                self.prec[t] = e.prec;
                i
            }));
        }
        self.seeds = Some((bank, sel));
    }

    #[cfg(feature = "encode")]
    pub(crate) fn write(&self, w: &mut BitWriter) {
        let start = w.bit_len();
        let n = self.freq.len();
        w.put(self.n_eob as u32, 8);
        w.put((n - self.n_eob) as u32, 8);
        // ctx_map: runs of equal entries, (value, run length) gamma coded.
        let nraw = self.ctx_map.len();
        let mut i = 0;
        while i < nraw {
            let v = self.ctx_map[i];
            let mut j = i + 1;
            while j < nraw && self.ctx_map[j] == v {
                j += 1;
            }
            w.put(v as u32, 8);
            put_gamma(w, (j - i) as u32);
            i = j;
        }
        for t in 0..n {
            if let Some((bank, sel)) = &self.seeds {
                let kind = crate::seeds::Kind::of_lossy(self.mode, t < self.n_eob);
                crate::seeds::write_selector(w, kind.and_then(|k| bank.kind(k, [0; 3])), sel[t]);
                if sel[t].is_some() {
                    continue;
                }
            }
            write_weights(w, &self.weights[t], self.prec[t]);
        }
        crate::seeds::record_group(self.mode as u8, || {
            let tables = (0..n)
                .map(|t| crate::rans::weights_cost(&self.weights[t], self.prec[t]))
                .sum();
            (w.bit_len() - start, tables, n as u32)
        });
    }

    /// `bank`: the tables carry selectors (seed byte scope bit 4).
    pub(crate) fn read(
        r: &mut BitReader<'_>,
        mode: LossyEntropy,
        bank: Option<&'static crate::seeds::Bank>,
    ) -> Result<Tables, Error> {
        if bank.is_some() && mode != LossyEntropy::RunLevel {
            return Err(Error::Corrupt("seeded tables in a non-run-level stream"));
        }
        let nraw = mode.nraw();
        let n_eob = r.read_refill(8) as usize;
        let n_ac = r.read_refill(8) as usize;
        let n = n_eob + n_ac;
        if n_eob == 0 || n_ac == 0 || n > MAX_TABLES {
            return Err(Error::Corrupt("bad table count"));
        }
        let mut ctx_map = Vec::with_capacity(nraw);
        while ctx_map.len() < nraw {
            let v = r.read_refill(8) as u8;
            let run = get_gamma(r)? as usize;
            if run == 0 || ctx_map.len() + run > nraw {
                return Err(Error::Corrupt("bad context map"));
            }
            ctx_map.extend(core::iter::repeat_n(v, run));
        }
        for (i, &t) in ctx_map.iter().enumerate() {
            let ok = if i < EOB_CTX {
                (t as usize) < n_eob
            } else {
                (t as usize) >= n_eob && (t as usize) < n
            };
            if !ok {
                return Err(Error::Corrupt("context maps to wrong table group"));
            }
        }
        let mut freq = Vec::with_capacity(n);
        let mut weights = Vec::with_capacity(n);
        let mut prec = Vec::with_capacity(n);
        let mut sel = Vec::new();
        for t in 0..n {
            // With selectors, a seed stands in for the explicit table; both
            // pass the same checks.
            let (wt, p) = match bank {
                Some(bank) => {
                    let kind = crate::seeds::Kind::of_lossy(mode, t < n_eob);
                    let ks = kind.and_then(|k| bank.kind(k, [0; 3]));
                    let seed = crate::seeds::read_selector(r, ks)?;
                    sel.push(seed.map(|i| i as u8));
                    match (seed, ks) {
                        (Some(i), Some(ks)) => ks.weights(i, ALPHA)?,
                        _ => read_weights(r, ALPHA)?,
                    }
                }
                None => read_weights(r, ALPHA)?,
            };
            let bad = wt.iter().enumerate().any(|(s, &v)| {
                v > 0
                    && !(if t >= n_eob {
                        mode.second_ok(s)
                    } else {
                        mode.first_ok(s)
                    })
            });
            if bad {
                return Err(Error::Corrupt("symbol out of range"));
            }
            freq.push(weights_to_freq(&wt).ok_or(Error::Corrupt("empty table"))?);
            weights.push(wt);
            prec.push(p);
        }
        if r.overrun() {
            return Err(Error::Corrupt("truncated tables"));
        }
        Ok(Tables {
            mode,
            ctx_map,
            n_eob,
            freq,
            weights,
            prec,
            seeds: bank.map(|b| (b, sel)),
        })
    }

    /// Raw-context -> table map padded to [`NRAW_PAD`].
    pub(crate) fn padded_map(&self) -> alloc::boxed::Box<[u8; NRAW_PAD]> {
        let mut m: alloc::boxed::Box<[u8; NRAW_PAD]> =
            vec![0u8; NRAW_PAD].into_boxed_slice().try_into().unwrap();
        m[..self.ctx_map.len()].copy_from_slice(&self.ctx_map);
        m
    }

    /// Decode tables, one `M`-entry block per table (unused tables zero). Entry:
    /// (freq-1) | bias << 10 | sym << 20 | extra bits << 27.
    pub(crate) fn dec_tables(&self) -> alloc::boxed::Box<[u32; MAX_TABLES << LOG_M]> {
        let mut d: alloc::boxed::Box<[u32; MAX_TABLES << LOG_M]> = vec![0u32; MAX_TABLES << LOG_M]
            .into_boxed_slice()
            .try_into()
            .unwrap();
        for (t, f) in self.freq.iter().enumerate() {
            let mut cum = 0u32;
            for (sym, &fs) in f.iter().enumerate() {
                if fs == 0 {
                    continue;
                }
                let nb = if t < self.n_eob {
                    0
                } else {
                    rl_extra_bits(sym as u32)
                };
                for slot in cum..cum + fs {
                    d[(t << LOG_M) | slot as usize] =
                        (fs - 1) | ((slot - cum) << 10) | ((sym as u32) << 20) | (nb << 27);
                }
                cum += fs;
            }
            debug_assert_eq!(cum, M);
        }
        d
    }
}

/// The table-count search runs only when the budgets' tables are at least
/// this share of their table plus data bits (benchmarks/coefgap_2026-10-09: on
/// imazen-26 VALIDATE the file-level share at 5% keeps 3.18% of the 3.19%
/// projected saving of searching every file at effort 7.5 and searches 8% of
/// the 1024 px files).
pub(crate) const K_SEARCH_MIN_SHARE: f64 = 0.05;

/// The table-count search's `min_share` for an encode at `effort` (`None`:
/// the effort's budgets): [`K_SEARCH_MIN_SHARE`] from
/// [`crate::effort::TABLE_COUNT_SEARCH`] for the coders it was measured on
/// (`production`: run-level ANS, split-Huffman), else none. LAB: `k_mode`
/// ([`crate::lossy::table_k_mode`]) overrides it for every coder (0 and 1 the
/// budgets, 2 every file, 3 [`K_SEARCH_MIN_SHARE`]; `u8::MAX` no override).
#[cfg_attr(not(feature = "lab"), allow(unused_variables))]
pub(crate) fn k_search_share(
    effort: crate::effort::Effort,
    production: bool,
    k_mode: u8,
) -> Option<f64> {
    #[cfg(feature = "lab")]
    match k_mode {
        0 | 1 => return None,
        2 => return Some(0.0),
        3 => return Some(K_SEARCH_MIN_SHARE),
        _ => {}
    }
    (production && effort.table_count_search()).then_some(K_SEARCH_MIN_SHARE)
}

/// The candidate that costs least by `cost`; ties keep the first (lowest)
/// candidate — the `best` default is only a fallback for a `f64::MAX` cost.
pub(crate) fn k_pick(cands: &[usize], cost: &mut dyn FnMut(usize) -> f64) -> usize {
    let mut best = (f64::MAX, cands[cands.len() - 1]);
    for &k in cands {
        let c = cost(k);
        if c < best.0 {
            best = (c, k);
        }
    }
    best.1
}

/// A merged table priced as [`Tables::build_tables`] and
/// [`Tables::apply_seeds`] handle it: the table [`choose_table`] picks,
/// its written size and data bits, and the seed that beats it, if any.
struct KTable {
    weights: Vec<u32>,
    freq: Vec<u32>,
    prec: u32,
    /// `weights_cost(weights, prec)`.
    write: u64,
    /// `rans_data_bits(counts, freq)` (`None` when a counted symbol is
    /// uncodable, as both users unwrap it).
    data: Option<f64>,
    seed: Option<KSeed>,
}

/// A seed that beats a table's explicit coding: its index, the gain
/// [`Tables::apply_seeds`] accumulates (`explicit - estimated - selector
/// bits`) and the decoded seed table.
struct KSeed {
    index: usize,
    gain: f64,
    expanded: crate::seeds::Expanded,
    /// `rans_data_bits(counts, expanded.freq)`.
    data: Option<f64>,
}

impl KTable {
    /// Prices `counts` (already carrying the all-zero table's symbol-0
    /// fix) exactly as `build_tables` + `apply_seeds` do.
    fn price(counts: &[u64], set: Option<&crate::seeds::KindSet>) -> KTable {
        let (weights, freq, prec) = choose_table(counts);
        let write = crate::rans::weights_cost(&weights, prec);
        let data = crate::seeds::rans_data_bits(counts, &freq);
        let explicit = write as f64 + data.unwrap_or(f64::MAX);
        let seed = set.and_then(|set| match set.best(counts) {
            Some((i, bits)) if bits + (set.seeds.index_bits() as f64) < explicit => {
                let expanded = set.table(i);
                Some(KSeed {
                    index: i,
                    gain: explicit - bits - set.seeds.index_bits() as f64,
                    data: crate::seeds::rans_data_bits(counts, &expanded.freq),
                    expanded,
                })
            }
            _ => None,
        });
        KTable {
            weights,
            freq,
            prec,
            write,
            data,
            seed,
        }
    }
}

/// One table group (EOB or AC contexts) clustered to `k` tables: the dense
/// first-use context map (unused contexts keep 0, as the writers' dense
/// renumber leaves them), the merged histograms (an all-zero one counts
/// symbol 0 once, as the writer path fixes it) and each table's price.
struct KGroup {
    /// Dense cluster of each context.
    map: Vec<usize>,
    /// `map`'s table count (at least 1).
    n: usize,
    tables: Vec<KTable>,
    /// `set.seeds.index_bits()` of the group's seed set (0 without seeds).
    index_bits: u32,
}

impl KGroup {
    /// `assign`'s dense map and merged histograms, priced; `sparse` is the
    /// clustering's nonzero (symbol, count) rows.
    fn build(
        sparse: &[Vec<(usize, u64)>],
        assign: &[usize],
        set: Option<&crate::seeds::KindSet>,
    ) -> KGroup {
        let mut renum = vec![usize::MAX; 256];
        let mut n = 0usize;
        let mut map = vec![0usize; assign.len()];
        for (i, &c) in assign.iter().enumerate() {
            if sparse[i].is_empty() {
                continue;
            }
            if renum[c] == usize::MAX {
                renum[c] = n;
                n += 1;
            }
            map[i] = renum[c];
        }
        let n = n.max(1);
        let mut counts = vec![vec![0u64; ALPHA]; n];
        for (i, &t) in map.iter().enumerate() {
            for &(s, v) in &sparse[i] {
                counts[t][s] += v;
            }
        }
        Self::price(map, n, counts, set)
    }

    /// `k == 1`'s group without clustering (`cluster`'s early return: every
    /// context in table 0).
    fn one(sparse: &[Vec<(usize, u64)>], set: Option<&crate::seeds::KindSet>) -> KGroup {
        let mut counts = vec![vec![0u64; ALPHA]];
        for row in sparse {
            for &(s, n) in row {
                counts[0][s] += n;
            }
        }
        Self::price(vec![0; sparse.len()], 1, counts, set)
    }

    /// Prices `counts`, applying the all-zero table fix.
    fn price(
        map: Vec<usize>,
        n: usize,
        mut counts: Vec<Vec<u64>>,
        set: Option<&crate::seeds::KindSet>,
    ) -> KGroup {
        let index_bits = set.map_or(0, |s| s.seeds.index_bits());
        let tables = counts
            .iter_mut()
            .map(|c| {
                if c.iter().all(|&v| v == 0) {
                    c[0] = 1;
                }
                KTable::price(c, set)
            })
            .collect();
        KGroup {
            map,
            n,
            tables,
            index_bits,
        }
    }
}

/// The table-count search's state (`build_tables`' `KSearch::search`): each
/// group's [`ClusterSet`], every `(group, k)` clustered and priced so far
/// and every `(ke, ka)` pair costed. A pair's groups are built and priced
/// once even when several pairs share them (the search's AC counts at the
/// EOB budget, the budget groups' gate probe and the final build).
struct KSearch<'a> {
    hist: &'a [Vec<u64>],
    nraw: usize,
    passes: usize,
    /// Seed sets, EOB group then AC group (`None` where the mode or bank
    /// has none).
    sets: [Option<crate::seeds::KindSet>; 2],
    cs: [Option<ClusterSet<'a>>; 2],
    /// `k`-keyed groups per side (few keys; linear).
    groups: [Vec<(usize, KGroup)>; 2],
    /// `(ke, ka)`-keyed pair costs.
    priced: Vec<((usize, usize), (f64, f64))>,
}

impl<'a> KSearch<'a> {
    fn new(
        hist: &'a [Vec<u64>],
        mode: LossyEntropy,
        passes: usize,
        bank: Option<&'static crate::seeds::Bank>,
    ) -> Self {
        let sets = [true, false].map(|first| {
            let kind = crate::seeds::Kind::of_lossy(mode, first)?;
            crate::seeds::kind_set(bank?, kind, [0; 3], ALPHA)
        });
        KSearch {
            hist,
            nraw: mode.nraw(),
            passes,
            sets,
            cs: [None, None],
            groups: [Vec::new(), Vec::new()],
            priced: Vec::new(),
        }
    }

    /// Ensures group `which` (0 = EOB, 1 = AC) at `k` tables exists.
    fn group(&mut self, which: usize, k: usize) {
        if self.groups[which].iter().any(|&(kk, _)| kk == k) {
            return;
        }
        let hist = match which {
            0 => &self.hist[..EOB_CTX],
            _ => &self.hist[EOB_CTX..],
        };
        let cs = self.cs[which].get_or_insert_with(|| ClusterSet::new(hist));
        let g = if k == 1 {
            KGroup::one(&cs.sparse, self.sets[which].as_ref())
        } else {
            let assign = cs.run(k, self.passes);
            KGroup::build(&cs.sparse, &assign, self.sets[which].as_ref())
        };
        self.groups[which].push((k, g));
    }

    /// The group `group` built (every reader calls `group` first).
    fn group_ref(&self, which: usize, k: usize) -> &KGroup {
        &self.groups[which]
            .iter()
            .find(|&&(kk, _)| kk == k)
            .unwrap()
            .1
    }

    /// `(written table bits, rANS data bits)` of `ke` EOB and `ka` AC
    /// tables — the bits [`Tables::write`] emits for them plus the seed
    /// decision [`Tables::apply_seeds`] makes from the same numbers —
    /// costed once per pair.
    fn pair(&mut self, ke: usize, ka: usize) -> (f64, f64) {
        if let Some(&(_, c)) = self.priced.iter().find(|&&(kk, _)| kk == (ke, ka)) {
            return c;
        }
        self.group(0, ke);
        self.group(1, ka);
        let (eob, ac) = (self.group_ref(0, ke), self.group_ref(1, ka));
        let (ne, nraw) = (eob.n, self.nraw);
        let empty = |i: usize| {
            if i < EOB_CTX {
                self.cs[0].as_ref().unwrap().sparse[i].is_empty()
            } else {
                self.cs[1].as_ref().unwrap().sparse[i - EOB_CTX].is_empty()
            }
        };
        // ctx_map exactly as `build_tables` forms it (unused contexts
        // repeat the previous one within the group for run coding; the
        // first AC context maps to the first AC table).
        let mut ctx_map = vec![0u8; nraw];
        for (i, v) in ctx_map.iter_mut().enumerate() {
            *v = (if i < EOB_CTX {
                eob.map[i]
            } else {
                ne + ac.map[i - EOB_CTX]
            }) as u8;
        }
        for i in 1..nraw {
            if empty(i) && i != EOB_CTX {
                ctx_map[i] = ctx_map[i - 1];
            }
        }
        if empty(EOB_CTX) {
            ctx_map[EOB_CTX] = ne as u8;
        }
        // The map as `Tables::write` runs it: (value, run) gamma coded.
        let mut bits = 16u64;
        let mut i = 0;
        while i < nraw {
            let v = ctx_map[i];
            let mut j = i + 1;
            while j < nraw && ctx_map[j] == v {
                j += 1;
            }
            bits += 8 + crate::rans::gamma_len((j - i) as u32) as u64;
            i = j;
        }
        // `apply_seeds`' aggregate gain test over the cached picks.
        let mut gain = -((ne + ac.n) as f64);
        for t in eob.tables.iter().chain(&ac.tables) {
            if let Some(s) = &t.seed {
                gain += s.gain;
            }
        }
        let fired = gain >= crate::seeds::MIN_GAIN_BITS;
        let mut data = 0.0f64;
        for (t, idx) in eob
            .tables
            .iter()
            .zip(core::iter::repeat(eob.index_bits))
            .chain(ac.tables.iter().zip(core::iter::repeat(ac.index_bits)))
        {
            if fired {
                // write_selector: a bit per table, the index when seeded.
                bits += 1;
                if let Some(s) = &t.seed {
                    bits += idx as u64;
                    data += s.data.unwrap_or(f64::MAX);
                    continue;
                }
            }
            bits += t.write;
            data += t.data.unwrap_or(f64::MAX);
        }
        let c = (bits as f64, data);
        self.priced.push(((ke, ka), c));
        c
    }

    fn total(&mut self, ke: usize, ka: usize) -> f64 {
        let (tables, data) = self.pair(ke, ka);
        tables + data
    }

    /// The EOB and AC cluster counts whose tables, seeded as
    /// `Tables::build_seeded` seeds them, cost the fewest written table
    /// bits plus rANS data bits: AC counts 1, 2, 4, 8 and the budget `ka`
    /// with `ke` EOB tables, then EOB counts 1, 2 and the budget `ke` with
    /// the chosen AC count. The budgets are candidates, so the result
    /// never costs more than the budgets by this measure. A `min_share`
    /// above 0 runs the search only when the budgets' table bits are at
    /// least that share of their table plus data bits (small files, where
    /// the tables weigh). benchmarks/coefgap_2026-10-09.
    fn search(&mut self, ke: usize, ka: usize, min_share: f64) -> (usize, usize) {
        if min_share > 0.0 {
            let (tables, data) = self.pair(ke, ka);
            if tables < min_share * (tables + data) {
                return (ke, ka);
            }
        }
        let mut ac: Vec<usize> = [1, 2, 4, 8].into_iter().filter(|&k| k < ka).collect();
        ac.push(ka);
        let ka = k_pick(&ac, &mut |k| self.total(ke, k));
        let mut eob: Vec<usize> = [1, 2].into_iter().filter(|&k| k < ke).collect();
        eob.push(ke);
        let ke = k_pick(&eob, &mut |k| self.total(k, ka));
        (ke, ka)
    }
}

/// LAB (coefgap study, 2026-10-09): merges the `n` clusters of `assign`
/// (contexts of `hist`) greedily, the pair whose merge raises the coded bits
/// least each time, and returns the number of tables, from `n` down to one,
/// whose tables plus rANS data cost the fewest bits (each table chosen as
/// [`choose_table`] codes it), renumbered densely in first-use order.
#[cfg(feature = "lab")]
fn merge_by_cost(hist: &[Vec<u64>], assign: Vec<usize>, n: usize) -> (Vec<usize>, usize) {
    let cost = |c: &[u64]| -> f64 {
        if c.iter().all(|&v| v == 0) {
            return 0.0;
        }
        let (w, f, p) = choose_table(c);
        crate::rans::weights_cost(&w, p) as f64 + crate::seeds::rans_data_bits(c, &f).unwrap_or(0.0)
    };
    let mut groups: Vec<Vec<u64>> = vec![vec![0u64; ALPHA]; n];
    for (i, &a) in assign.iter().enumerate() {
        for (s, &v) in hist[i].iter().enumerate() {
            groups[a][s] += v;
        }
    }
    // `of[c]`: the group cluster c belongs to.
    let mut of: Vec<usize> = (0..n).collect();
    let mut costs: Vec<f64> = groups.iter().map(|g| cost(g)).collect();
    let mut live: Vec<bool> = vec![true; n];
    let mut best = (costs.iter().sum::<f64>(), of.clone());
    for _ in 1..n {
        let mut pick = (f64::MAX, 0, 0, Vec::new(), 0.0);
        for a in 0..n {
            for b in a + 1..n {
                if !live[a] || !live[b] {
                    continue;
                }
                let m: Vec<u64> = groups[a]
                    .iter()
                    .zip(&groups[b])
                    .map(|(x, y)| x + y)
                    .collect();
                let c = cost(&m);
                let d = c - costs[a] - costs[b];
                if d < pick.0 {
                    pick = (d, a, b, m, c);
                }
            }
        }
        let (_, a, b, m, c) = pick;
        groups[a] = m;
        costs[a] = c;
        live[b] = false;
        for o in of.iter_mut() {
            if *o == b {
                *o = a;
            }
        }
        let total: f64 = (0..n).filter(|&i| live[i]).map(|i| costs[i]).sum();
        if total < best.0 {
            best = (total, of.clone());
        }
    }
    let mut map = vec![usize::MAX; n];
    let mut k = 0;
    let mut out = vec![0usize; assign.len()];
    for (i, &a) in assign.iter().enumerate() {
        if hist[i].iter().all(|&v| v == 0) {
            continue;
        }
        let g = best.1[a];
        if map[g] == usize::MAX {
            map[g] = k;
            k += 1;
        }
        out[i] = map[g];
    }
    (out, k.max(1))
}

/// LAB (coefgap study, 2026-10-09): the run-level table machinery on
/// arbitrary raw-context histograms. Contexts `..n_first` cluster into at most
/// `max_first` tables and the rest into at most `max_rest` (k-means as
/// [`Tables::build_with_effort`] at effort 10), each table chosen and costed
/// as the writer codes it. Histograms have at most [`ALPHA`] symbols. Returns
/// (context-map bits as [`Tables::write`] codes runs, table bits, rANS data
/// bits, ideal data bits of the clustered tables, tables).
#[cfg(feature = "lab")]
pub(crate) fn lab_table_cost(
    hist: &[Vec<u64>],
    n_first: usize,
    max_first: usize,
    max_rest: usize,
) -> (u64, u64, f64, f64, usize) {
    let pad = |h: &[Vec<u64>]| -> Vec<Vec<u64>> {
        h.iter()
            .map(|v| {
                assert!(v.len() <= ALPHA, "lab_table_cost: alphabet above {ALPHA}");
                let mut p = v.clone();
                p.resize(ALPHA, 0);
                p
            })
            .collect()
    };
    let (first, rest) = (pad(&hist[..n_first]), pad(&hist[n_first..]));
    let mut ctx_map = Vec::with_capacity(hist.len());
    let mut counts: Vec<Vec<u64>> = Vec::new();
    for (h, cap) in [(&first, max_first), (&rest, max_rest)] {
        if h.is_empty() {
            continue;
        }
        let a = cluster(h, cap.max(1), 8);
        let base = counts.len();
        let mut renum = vec![usize::MAX; cap.max(1)];
        let mut prev = base;
        for (i, &c) in a.iter().enumerate() {
            if h[i].iter().all(|&v| v == 0) {
                // Unused contexts repeat the previous mapping (as `build`).
                ctx_map.push(prev);
                continue;
            }
            if renum[c] == usize::MAX {
                renum[c] = counts.len();
                counts.push(vec![0u64; ALPHA]);
            }
            let t = renum[c];
            for (s, &n) in h[i].iter().enumerate() {
                counts[t][s] += n;
            }
            ctx_map.push(t);
            prev = t;
        }
        if counts.len() == base {
            counts.push(vec![1u64; 1]);
            counts.last_mut().unwrap().resize(ALPHA, 0);
        }
    }
    let mut map_bits = 16u64;
    let mut i = 0;
    while i < ctx_map.len() {
        let mut j = i + 1;
        while j < ctx_map.len() && ctx_map[j] == ctx_map[i] {
            j += 1;
        }
        map_bits += 8 + crate::rans::gamma_len((j - i) as u32) as u64;
        i = j;
    }
    let (mut table_bits, mut data_bits, mut ideal) = (0u64, 0.0f64, 0.0f64);
    for c in &counts {
        let (w, f, p) = choose_table(c);
        table_bits += crate::rans::weights_cost(&w, p);
        data_bits += crate::seeds::rans_data_bits(c, &f).unwrap_or(0.0);
        let tot: u64 = c.iter().sum();
        ideal += c
            .iter()
            .filter(|&&n| n > 0)
            .map(|&n| -(n as f64) * crate::mathx::log2(n as f64 / tot as f64))
            .sum::<f64>();
    }
    (map_bits, table_bits, data_bits, ideal, counts.len())
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    #[test]
    fn zigzag_is_permutation() {
        let mut seen = [false; 64];
        for &z in &ZIGZAG {
            assert!(!seen[z as usize]);
            seen[z as usize] = true;
        }
    }

    #[test]
    fn rl_token_roundtrip() {
        for run in 0..=MAX_RUN {
            for v in (-70000..70000).chain([(1 << 22) - 1, -(1 << 22) + 1]) {
                if v == 0 {
                    continue;
                }
                let (s, e, n) = rl_token(run, v);
                assert!(valid_ac_symbol(s as usize), "{v}");
                assert_eq!(s >> 5, run);
                assert_eq!(n, rl_extra_bits(s));
                assert!(e < 1 << n);
                assert_eq!(rl_value(s & 31, e), v);
            }
        }
    }
}
