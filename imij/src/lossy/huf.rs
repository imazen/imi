//! Huffman side information for [`LossyEntropy::SplitHuf`](super::coef::LossyEntropy):
//! the split payload's group counts and widths coded with static Huffman
//! tables instead of the rANS lanes, so they decode with no per-symbol
//! state machine.
//!
//! Per stripe, three sections, each one symbol sequence in block raster
//! order coded as [`STREAMS`] interleaved bit streams (stream i holds the
//! i-th of `STREAMS` equal slices, the last one shortest):
//!
//! 1. luma group counts; table from the class (bit length) of the previous
//!    luma count in the same slice (0 for a slice's first);
//! 2. chroma group counts, two per block (Co, Cg); table from the channel
//!    and the class of the block's luma count;
//! 3. widths of every coded group, blocks in raster order, channels, then
//!    groups; table from (channel, band(gi), min(L - gi, 5)) with L the
//!    block's luma count (chroma) or the channel's own count (luma), 0 when
//!    gi >= L. Every width context follows from the counts, so the table
//!    of every symbol is known before decoding.
//!
//! Section layout: `varint(len0) varint(len1) varint(len2)` then the four
//! streams (the last runs to the section end). Streams are LSB-first; codes
//! are canonical, at most [`LOG`] bits, emitted bit-reversed so a decoder
//! indexes its table with the low bits of a 64-bit container.
//!
//! Tables (in the lossy header): `n_cnt` (8 bits), `n_wid` (8 bits), then
//! per table its code lengths (4 bits per symbol; [`CNT_ALPHA`] symbols for
//! count tables, [`WID_ALPHA`] for width tables), then the context maps
//! (5 bits per entry): [`LUMA_CTX`] luma-count contexts and [`CHROMA_CTX`]
//! chroma-count contexts to count tables, [`WID_CTX`] width contexts to
//! width tables (indices relative to their group).

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use super::coef::{self, NCH, PK_BAND, PK_GROUPS};
#[cfg(feature = "encode")]
use super::rawbits::RawWriter;
use super::rawbits::{self};
use crate::Error;
use crate::bits::BitReader;
#[cfg(feature = "encode")]
use crate::bits::BitWriter;

/// Longest code; tables have `1 << LOG` entries.
pub(crate) const LOG: u32 = 9;
const TSIZE: usize = 1 << LOG;
/// Interleaved streams per section.
pub(crate) const STREAMS: usize = 4;
/// Symbols per refill (a refill leaves at least 56 valid bits).
const PER_REFILL: usize = 56 / LOG as usize;
/// Most tables of each group.
pub(crate) const MAX_CNT_TABLES: usize = 8;
pub(crate) const MAX_WID_TABLES: usize = 24;
const MAX_TABLES: usize = MAX_CNT_TABLES + MAX_WID_TABLES;
/// Decode tables of every Huffman table, `1 << LOG` entries each.
pub(crate) type Dec = Box<[u16; MAX_TABLES << LOG]>;
/// Count alphabet (0..=16) and width alphabet (0..=22).
pub(crate) const CNT_ALPHA: usize = PK_GROUPS + 1;
pub(crate) const WID_ALPHA: usize = coef::PK_WIDTHS;
/// Count classes: bit length of a count, 0..=5.
pub(crate) const CLASSES: usize = 6;
pub(crate) const LUMA_CTX: usize = CLASSES;
pub(crate) const CHROMA_CTX: usize = 2 * CLASSES;
pub(crate) const WID_CTX: usize = NCH * 6 * 6;

#[inline(always)]
pub(crate) fn class(n: u32) -> usize {
    (32 - n.leading_zeros()) as usize
}

/// Width context of group `gi` of channel `ch`, `l` = luma's count
/// (chroma) or the channel's own count (luma).
#[inline(always)]
pub(crate) fn wid_ctx(ch: usize, gi: usize, l: u32) -> usize {
    (ch * 6 + PK_BAND[gi] as usize) * 6 + (l.saturating_sub(gi as u32)).min(5) as usize
}

/// Slice length of a section of `n` symbols.
#[inline(always)]
pub(crate) fn slice_len(n: usize) -> usize {
    n.div_ceil(STREAMS).max(1)
}

// ---- encoder --------------------------------------------------------------

/// The three symbol sequences of a stripe: (context, symbol) pairs.
#[derive(Default)]
#[cfg(feature = "encode")]
pub(crate) struct Syms {
    pub luma: Vec<(u8, u8)>,
    pub chroma: Vec<(u8, u8)>,
    pub wid: Vec<(u8, u8)>,
}

/// Split-mode counts and widths of a stripe, in section order.
/// `counts[bc]` / `widths[bc * 16 + gi]` as the decoder produces them.
#[cfg(feature = "encode")]
pub(crate) fn stripe_syms(counts: &[u8], widths: &[u8], nblocks: usize) -> Syms {
    let mut s = Syms::default();
    let q = slice_len(nblocks);
    let mut prev = 0u8;
    for b in 0..nblocks {
        if b % q == 0 {
            prev = 0;
        }
        let n = counts[b * NCH];
        s.luma.push((class(prev as u32) as u8, n));
        prev = n;
    }
    for b in 0..nblocks {
        let cl = class(counts[b * NCH] as u32);
        for c in 1..NCH {
            s.chroma
                .push((((c - 1) * CLASSES + cl) as u8, counts[b * NCH + c]));
        }
    }
    for b in 0..nblocks {
        let ln = counts[b * NCH] as u32;
        for c in 0..NCH {
            let bc = b * NCH + c;
            let n = counts[bc] as usize;
            let l = if c == 0 { n as u32 } else { ln };
            for gi in 0..n {
                s.wid
                    .push((wid_ctx(c, gi, l) as u8, widths[bc * PK_GROUPS + gi]));
            }
        }
    }
    s
}

/// Code lengths for `freq` (at most [`LOG`] bits; unused symbols 0).
#[cfg(feature = "encode")]
pub(crate) fn lengths(freq: &[u64]) -> Vec<u8> {
    let na = freq.len();
    let mut len = vec![0u8; na];
    let mut syms: Vec<usize> = (0..na).filter(|&s| freq[s] > 0).collect();
    match syms.len() {
        0 => {
            len[0] = 1;
            return len;
        }
        1 => {
            len[syms[0]] = 1;
            return len;
        }
        _ => {}
    }
    syms.sort_by_key(|&s| (freq[s], s));
    // Two-queue Huffman over the sorted leaves; parent links give depths.
    let nl = syms.len();
    let mut w: Vec<u64> = syms.iter().map(|&s| freq[s]).collect();
    let mut parent = vec![0usize; 2 * nl - 1];
    let (mut li, mut ii) = (0usize, nl);
    for k in nl..2 * nl - 1 {
        let mut pick = || {
            if li < nl && (ii >= k || w[li] <= w[ii]) {
                li += 1;
                li - 1
            } else {
                ii += 1;
                ii - 1
            }
        };
        let (a, b) = (pick(), pick());
        w.push(w[a] + w[b]);
        parent[a] = k;
        parent[b] = k;
    }
    let mut depth = vec![0u32; 2 * nl - 1];
    for k in (0..2 * nl - 2).rev() {
        depth[k] = depth[parent[k]] + 1;
    }
    for (i, &s) in syms.iter().enumerate() {
        len[s] = depth[i].min(LOG) as u8;
    }
    let kraft = |len: &[u8]| -> u64 {
        len.iter()
            .filter(|&&l| l > 0)
            .map(|&l| 1u64 << (LOG - l as u32))
            .sum()
    };
    // Over-subscribed after the clamp: lengthen the longest codes below
    // LOG (cheapest first), then spend leftover space on frequent symbols.
    while kraft(&len) > TSIZE as u64 {
        let s = (0..na)
            .filter(|&s| len[s] > 0 && (len[s] as u32) < LOG)
            .max_by_key(|&s| (len[s], u64::MAX - freq[s]))
            .unwrap();
        len[s] += 1;
    }
    loop {
        let k = kraft(&len);
        let Some(s) = (0..na)
            .filter(|&s| len[s] > 1 && k + (1u64 << (LOG - len[s] as u32)) <= TSIZE as u64)
            .max_by_key(|&s| freq[s])
        else {
            break;
        };
        len[s] -= 1;
    }
    len
}

/// Canonical codes, bit-reversed for LSB-first emission.
fn codes(len: &[u8]) -> Vec<u32> {
    let mut order: Vec<usize> = (0..len.len()).filter(|&s| len[s] > 0).collect();
    order.sort_by_key(|&s| (len[s], s));
    let mut code = vec![0u32; len.len()];
    let (mut c, mut prev) = (0u32, 0u8);
    for &s in &order {
        c <<= len[s] - prev;
        prev = len[s];
        code[s] = c.reverse_bits() >> (32 - len[s] as u32);
        c += 1;
    }
    code
}

/// Huffman tables and context maps of a lossy image.
#[derive(Clone)]
pub(crate) struct HufTables {
    /// Code lengths per table: count tables first.
    pub lens: Vec<Vec<u8>>,
    /// With selectors (seed bank): each table's seed, `None` = explicit.
    // Retained in the shared parsed representation for wire re-emission.
    #[cfg_attr(not(feature = "encode"), allow(dead_code))]
    pub seeds: Option<(&'static crate::seeds::Bank, Vec<Option<u8>>)>,
    // Retained in the shared parsed representation for wire re-emission.
    #[cfg_attr(not(feature = "encode"), allow(dead_code))]
    pub n_cnt: usize,
    /// Context -> global table index.
    pub luma_map: [u8; LUMA_CTX],
    pub chroma_map: [u8; CHROMA_CTX],
    pub wid_map: [u8; WID_CTX],
}

/// A merged table priced as [`HufTables::build`] and
/// [`HufTables::apply_seeds`] handle it: the code lengths `lengths` picks
/// and the seed whose lengths beat them, if any.
#[cfg(feature = "encode")]
struct HKTable {
    lens: Vec<u8>,
    /// Sum of count * len over the merged histogram.
    data: u64,
    seed: Option<HKSeed>,
}

/// A seed whose code lengths beat the explicit table's (`gain > 0`):
/// the index, the gain `apply_seeds` accumulates and the seed's lengths.
#[cfg(feature = "encode")]
struct HKSeed {
    index: usize,
    gain: f64,
    lens: Vec<u8>,
    data: u64,
}

#[cfg(feature = "encode")]
impl HKTable {
    /// Prices the merged histogram `h` (of `alpha` symbols) exactly as
    /// `build` + `apply_seeds` do.
    fn price(h: &[u64], set: Option<&crate::seeds::KindSet>) -> HKTable {
        let lens = lengths(h);
        let data: u64 = h.iter().zip(&lens).map(|(&c, &n)| c * n as u64).sum();
        let explicit = 4.0 * h.len() as f64 + data as f64;
        let seed = set.and_then(|set| match set.best(h) {
            Some((i, bits)) if bits + (set.seeds.index_bits() as f64) < explicit => {
                let lens = set.table(i).lens;
                Some(HKSeed {
                    index: i,
                    gain: explicit - bits - set.seeds.index_bits() as f64,
                    data: h.iter().zip(&lens).map(|(&c, &n)| c * n as u64).sum(),
                    lens,
                })
            }
            _ => None,
        });
        HKTable { lens, data, seed }
    }
}

/// One histogram group (count or width contexts) clustered to `k` tables:
/// each context's table (0-based; unused contexts keep table 0, as the
/// maps code them) and the merged histograms priced.
#[cfg(feature = "encode")]
struct HKGroup {
    map: Vec<u8>,
    merged: Vec<Vec<u64>>,
    tables: Vec<HKTable>,
    /// `set.seeds.index_bits()` of the group's seed set (0 without seeds).
    index_bits: u32,
}

/// The split-Huffman table-count search's state (as [`coef::KSearch`]):
/// each side's [`coef::ClusterSet`], the `(side, k)` groups priced so far
/// and the `(kc, kw)` pairs costed — each candidate priced once.
#[cfg(feature = "encode")]
struct HSearch<'a> {
    hists: [&'a [Vec<u64>]; 2],
    alphas: [usize; 2],
    passes: usize,
    /// Seed sets, count group then width group (`None` without a bank).
    sets: [Option<crate::seeds::KindSet>; 2],
    cs: [Option<coef::ClusterSet<'a>>; 2],
    /// `k`-keyed groups per side (few keys; linear).
    groups: [Vec<(usize, HKGroup)>; 2],
    /// `(kc, kw)`-keyed pair costs.
    priced: Vec<((usize, usize), (f64, f64))>,
}

#[cfg(feature = "encode")]
impl<'a> HSearch<'a> {
    fn new(
        cnt: &'a [Vec<u64>],
        wid: &'a [Vec<u64>],
        passes: usize,
        bank: Option<&'static crate::seeds::Bank>,
    ) -> Self {
        let sets = [
            (crate::seeds::Kind::HufCount, CNT_ALPHA),
            (crate::seeds::Kind::HufWidth, WID_ALPHA),
        ]
        .map(|(kind, alpha)| crate::seeds::kind_set(bank?, kind, [0; 3], alpha));
        HSearch {
            hists: [cnt, wid],
            alphas: [CNT_ALPHA, WID_ALPHA],
            passes,
            sets,
            cs: [None, None],
            groups: [Vec::new(), Vec::new()],
            priced: Vec::new(),
        }
    }

    /// Ensures group `which` (0 = counts, 1 = widths) at `k` tables exists.
    fn group(&mut self, which: usize, k: usize) {
        if self.groups[which].iter().any(|&(kk, _)| kk == k) {
            return;
        }
        let hist = self.hists[which];
        let alpha = self.alphas[which];
        let cs = self.cs[which].get_or_insert_with(|| coef::ClusterSet::new(hist));
        let a = if k == 1 {
            // `cluster`'s early return: every context in table 0.
            vec![0usize; hist.len()]
        } else {
            cs.run(k, self.passes)
        };
        let mut map = vec![usize::MAX; hist.len()];
        let mut merged: Vec<Vec<u64>> = Vec::new();
        let mut out = vec![0u8; hist.len()];
        for (i, &c) in a.iter().enumerate() {
            if cs.sparse[i].is_empty() {
                continue;
            }
            if map[c] == usize::MAX {
                map[c] = merged.len();
                merged.push(vec![0; alpha]);
            }
            out[i] = map[c] as u8;
            for &(s, n) in &cs.sparse[i] {
                if s < alpha {
                    merged[map[c]][s] += n;
                }
            }
        }
        if merged.is_empty() {
            merged.push(vec![0; alpha]);
        }
        let set = self.sets[which].as_ref();
        let index_bits = set.map_or(0, |s| s.seeds.index_bits());
        let tables = merged.iter().map(|h| HKTable::price(h, set)).collect();
        self.groups[which].push((
            k,
            HKGroup {
                map: out,
                merged,
                tables,
                index_bits,
            },
        ));
    }

    /// The group `group` built (every reader calls `group` first).
    fn group_ref(&self, which: usize, k: usize) -> &HKGroup {
        &self.groups[which]
            .iter()
            .find(|&&(kk, _)| kk == k)
            .unwrap()
            .1
    }

    /// `(written table bits, Huffman data bits)` of `kc` count and `kw`
    /// width tables — [`HufTables::write`]'s size for them plus the seed
    /// decision [`HufTables::apply_seeds`] makes — costed once per pair.
    fn pair(&mut self, kc: usize, kw: usize) -> (f64, f64) {
        if let Some(&(_, c)) = self.priced.iter().find(|&&(kk, _)| kk == (kc, kw)) {
            return c;
        }
        self.group(0, kc);
        self.group(1, kw);
        let (cg, wg) = (self.group_ref(0, kc), self.group_ref(1, kw));
        let mut gain = -((cg.merged.len() + wg.merged.len()) as f64);
        for t in cg.tables.iter().chain(&wg.tables) {
            if let Some(s) = &t.seed {
                gain += s.gain;
            }
        }
        let fired = gain >= crate::seeds::MIN_GAIN_BITS;
        let mut bits = 16 + 5 * (LUMA_CTX + CHROMA_CTX + WID_CTX) as u64;
        let mut data = 0u64;
        for (t, idx) in cg
            .tables
            .iter()
            .zip(core::iter::repeat(cg.index_bits))
            .chain(wg.tables.iter().zip(core::iter::repeat(wg.index_bits)))
        {
            if fired {
                // write_selector: a bit per table, the index when seeded.
                bits += 1;
                if let Some(s) = &t.seed {
                    bits += idx as u64;
                    data += s.data;
                    continue;
                }
            }
            bits += 4 * t.lens.len() as u64;
            data += t.data;
        }
        let c = (bits as f64, data as f64);
        self.priced.push(((kc, kw), c));
        c
    }

    /// Split-Huffman's table-count search (as `coef`'s;
    /// benchmarks/coefgap_2026-10-09): the width-table count (1, 2, 4, 8,
    /// 16 or the budget `kw`) with `kc` count tables, then the count-table
    /// count (1, 2, 4 or the budget `kc`) with the chosen width count,
    /// whose tables, seeded as [`HufTables::build`] seeds them, cost the
    /// fewest written table bits plus Huffman data bits. A `min_share`
    /// above 0 runs the search only when the budgets' table bits are at
    /// least that share of their table plus data bits.
    fn search(&mut self, kc: usize, kw: usize, min_share: f64) -> (usize, usize) {
        if min_share > 0.0 {
            let (tables, data) = self.pair(kc, kw);
            if tables < min_share * (tables + data) {
                return (kc, kw);
            }
        }
        let mut ws: Vec<usize> = [1, 2, 4, 8, 16].into_iter().filter(|&k| k < kw).collect();
        ws.push(kw);
        let kw = coef::k_pick(&ws, &mut |k| {
            let (t, d) = self.pair(kc, k);
            t + d
        });
        let mut cs: Vec<usize> = [1, 2, 4].into_iter().filter(|&k| k < kc).collect();
        cs.push(kc);
        let kc = coef::k_pick(&cs, &mut |k| {
            let (t, d) = self.pair(k, kw);
            t + d
        });
        (kc, kw)
    }
}

impl HufTables {
    /// Tables from per-context histograms (`[ctx][symbol]`); with `bank`,
    /// the seeded tables and the explicit ones when seeds replaced any.
    /// `k_mode`: the lab table-count override ([`super::table_k_mode`]).
    #[cfg(feature = "encode")]
    pub(crate) fn build(
        luma: &[Vec<u64>],
        chroma: &[Vec<u64>],
        wid: &[Vec<u64>],
        effort: crate::effort::Effort,
        bank: Option<&'static crate::seeds::Bank>,
        k_mode: u8,
    ) -> (HufTables, Option<HufTables>) {
        Self::build_budget(luma, chroma, wid, effort, bank, k_mode, false)
    }

    #[cfg(feature = "encode")]
    pub(super) fn build_budget(
        luma: &[Vec<u64>],
        chroma: &[Vec<u64>],
        wid: &[Vec<u64>],
        effort: crate::effort::Effort,
        bank: Option<&'static crate::seeds::Bank>,
        k_mode: u8,
        local: bool,
    ) -> (HufTables, Option<HufTables>) {
        let cnt: Vec<Vec<u64>> = luma.iter().chain(chroma).cloned().collect();
        let share = coef::k_search_share(effort, true, k_mode);
        let effort = if local { effort } else { effort.table_search() };
        let passes = effort.budget(8);
        let (mut kc, mut kw) = (effort.budget(MAX_CNT_TABLES), effort.budget(MAX_WID_TABLES));
        // The table counts that code smallest (benchmarks/coefgap_2026-10-09);
        // the search's priced groups are reused for the tables themselves.
        let mut hs = HSearch::new(&cnt, wid, passes, bank);
        if let Some(min_share) = share {
            (kc, kw) = hs.search(kc, kw, min_share);
        }
        hs.group(0, kc);
        hs.group(1, kw);
        let (cg, wg) = (hs.group_ref(0, kc), hs.group_ref(1, kw));
        let (cmap, cm) = (&cg.map[..], &cg.merged[..]);
        let (wmap, wm) = (&wg.map[..], &wg.merged[..]);
        let n_cnt = cm.len();
        let priced: Vec<&HKTable> = cg.tables.iter().chain(&wg.tables).collect();
        let lens: Vec<Vec<u8>> = priced.iter().map(|t| t.lens.clone()).collect();
        for (t, (h, l)) in cm.iter().chain(wm).zip(&lens).enumerate() {
            let kind = if t < n_cnt {
                crate::seeds::Kind::HufCount
            } else {
                crate::seeds::Kind::HufWidth
            };
            crate::seeds::record_table(kind, [0; 3], h, || {
                let data: u64 = h.iter().zip(l).map(|(&c, &n)| c * n as u64).sum();
                (4 * l.len() as u64, data as f64)
            });
        }
        let t = HufTables {
            lens,
            seeds: None,
            n_cnt,
            luma_map: core::array::from_fn(|i| cmap[i]),
            chroma_map: core::array::from_fn(|i| cmap[LUMA_CTX + i]),
            wid_map: core::array::from_fn(|i| wmap[i] + n_cnt as u8),
        };
        if let Some(bank) = bank {
            let mut seeded = t.clone();
            seeded.apply_seeds_priced(&priced, bank);
            if seeded.seeds.is_some() {
                return (seeded, Some(t));
            }
        }
        (t, None)
    }

    #[cfg(feature = "encode")]
    fn kind(&self, t: usize) -> (crate::seeds::Kind, usize) {
        if t < self.n_cnt {
            (crate::seeds::Kind::HufCount, CNT_ALPHA)
        } else {
            (crate::seeds::Kind::HufWidth, WID_ALPHA)
        }
    }

    /// The seed decision `build` makes over tables the table-count search
    /// already priced (`HKTable::price` computes the same numbers in the
    /// same order): where the estimated saving exceeds
    /// [`MIN_GAIN_BITS`](crate::seeds::MIN_GAIN_BITS), a table's code
    /// lengths become its seed's.
    #[cfg(feature = "encode")]
    fn apply_seeds_priced(&mut self, priced: &[&HKTable], bank: &'static crate::seeds::Bank) {
        let n = self.lens.len();
        let mut gain = -(n as f64);
        for t in priced {
            if let Some(s) = &t.seed {
                gain += s.gain;
            }
        }
        if gain < crate::seeds::MIN_GAIN_BITS {
            return;
        }
        let mut sel = Vec::with_capacity(n);
        for (t, kt) in priced.iter().enumerate() {
            sel.push(kt.seed.as_ref().map(|s| {
                crate::seeds::record_seed(self.kind(t).0, [0; 3], s.index as u8, s.gain);
                self.lens[t] = s.lens.clone();
                s.index as u8
            }));
        }
        self.seeds = Some((bank, sel));
    }

    #[cfg(feature = "encode")]
    pub(crate) fn write(&self, w: &mut BitWriter) {
        w.put(self.n_cnt as u32, 8);
        w.put((self.lens.len() - self.n_cnt) as u32, 8);
        for (t, l) in self.lens.iter().enumerate() {
            if let Some((bank, sel)) = &self.seeds {
                crate::seeds::write_selector(w, bank.kind(self.kind(t).0, [0; 3]), sel[t]);
                if sel[t].is_some() {
                    continue;
                }
            }
            for &v in l {
                w.put(v as u32, 4);
            }
        }
        for &t in self.luma_map.iter().chain(&self.chroma_map) {
            w.put(t as u32, 5);
        }
        for &t in &self.wid_map {
            w.put(t as u32 - self.n_cnt as u32, 5);
        }
        crate::seeds::record_group(super::LossyEntropy::SplitHuf as u8, || {
            let table_bits: u64 = self.lens.iter().map(|l| 4 * l.len() as u64).sum();
            let map_bits = 5 * (LUMA_CTX + CHROMA_CTX + WID_CTX) as u64;
            (
                16 + table_bits + map_bits,
                table_bits,
                self.lens.len() as u32,
            )
        });
    }

    /// `bank`: the tables carry selectors (seed byte scope bit 4).
    pub(crate) fn read(
        r: &mut BitReader<'_>,
        bank: Option<&'static crate::seeds::Bank>,
    ) -> Result<HufTables, Error> {
        let n_cnt = r.read_refill(8) as usize;
        let n_wid = r.read_refill(8) as usize;
        if n_cnt == 0 || n_wid == 0 || n_cnt > MAX_CNT_TABLES || n_wid > MAX_WID_TABLES {
            return Err(Error::Corrupt("bad Huffman table count"));
        }
        let mut lens = Vec::with_capacity(n_cnt + n_wid);
        let mut sel = Vec::new();
        for t in 0..n_cnt + n_wid {
            let (kind, alpha) = if t < n_cnt {
                (crate::seeds::Kind::HufCount, CNT_ALPHA)
            } else {
                (crate::seeds::Kind::HufWidth, WID_ALPHA)
            };
            // With selectors, a seed stands in for the explicit lengths; both
            // pass the same checks.
            let mut seeded = None;
            if let Some(bank) = bank {
                let ks = bank.kind(kind, [0; 3]);
                let seed = crate::seeds::read_selector(r, ks)?;
                sel.push(seed.map(|i| i as u8));
                if let (Some(i), Some(ks)) = (seed, ks) {
                    seeded = Some(ks.lengths(i, alpha)?);
                }
            }
            let l: Vec<u8> = match seeded {
                Some(l) => l,
                None => (0..alpha).map(|_| r.read_refill(4) as u8).collect(),
            };
            let kraft: u64 = l
                .iter()
                .filter(|&&v| v > 0)
                .map(|&v| (TSIZE as u64) >> v.min(LOG as u8 + 1))
                .sum();
            if l.iter().any(|&v| v as u32 > LOG) || kraft == 0 || kraft > TSIZE as u64 {
                return Err(Error::Corrupt("bad Huffman code lengths"));
            }
            lens.push(l);
        }
        let mut cmap = [0u8; LUMA_CTX + CHROMA_CTX];
        for v in cmap.iter_mut() {
            *v = r.read_refill(5) as u8;
            if *v as usize >= n_cnt {
                return Err(Error::Corrupt("bad Huffman context map"));
            }
        }
        let mut wid_map = [0u8; WID_CTX];
        for v in wid_map.iter_mut() {
            let t = r.read_refill(5) as usize;
            if t >= n_wid {
                return Err(Error::Corrupt("bad Huffman context map"));
            }
            *v = (t + n_cnt) as u8;
        }
        if r.overrun() {
            return Err(Error::Corrupt("truncated Huffman tables"));
        }
        Ok(HufTables {
            lens,
            seeds: bank.map(|b| (b, sel)),
            n_cnt,
            luma_map: core::array::from_fn(|i| cmap[i]),
            chroma_map: core::array::from_fn(|i| cmap[LUMA_CTX + i]),
            wid_map,
        })
    }

    /// Decode tables: entry `length | symbol << 8`, `1 << LOG` per table.
    /// Entries no code reaches (an incomplete code) decode symbol 0 with
    /// length [`LOG`].
    pub(crate) fn dec(&self) -> Dec {
        let mut d: Box<[u16; MAX_TABLES << LOG]> = vec![LOG as u16; MAX_TABLES << LOG]
            .into_boxed_slice()
            .try_into()
            .unwrap();
        for (t, len) in self.lens.iter().enumerate() {
            let code = codes(len);
            for (s, &l) in len.iter().enumerate() {
                let l = l as u32;
                if l == 0 {
                    continue;
                }
                for k in 0..1usize << (LOG - l) {
                    d[(t << LOG) | code[s] as usize | k << l] = l as u16 | (s as u16) << 8;
                }
            }
        }
        d
    }

    /// Compile canonical codes once for all sections and stripes of an image.
    #[cfg(feature = "encode")]
    pub(crate) fn encoder(&self) -> Encoder<'_> {
        Encoder {
            tables: self,
            codes: self.lens.iter().map(|l| codes(l)).collect(),
        }
    }
}

#[cfg(feature = "encode")]
pub(crate) struct Encoder<'a> {
    tables: &'a HufTables,
    codes: Vec<Vec<u32>>,
}

#[cfg(feature = "encode")]
impl Encoder<'_> {
    #[cfg(feature = "encode")]
    pub(super) fn tables_for_lab(&self) -> &HufTables {
        self.tables
    }
    /// Exact section size including all stream lengths and byte padding.
    #[cfg(feature = "encode")]
    pub(super) fn section_len(&self, syms: &[(u8, u8)], map: &[u8]) -> Option<usize> {
        let q = slice_len(syms.len());
        let mut lengths = [0usize; STREAMS];
        for (i, part) in syms.chunks(q).enumerate() {
            let mut bits = 0usize;
            for &(ctx, symbol) in part {
                let n = self.tables.lens[map[ctx as usize] as usize][symbol as usize];
                if n == 0 {
                    return None;
                }
                bits += n as usize;
            }
            lengths[i] = bits.div_ceil(8);
        }
        Some(
            lengths.iter().sum::<usize>()
                + lengths[..STREAMS - 1]
                    .iter()
                    .map(|&n| crate::varint_len(n as u64))
                    .sum::<usize>(),
        )
    }
    /// Encodes one section: `syms` as (context, symbol), `map` = context ->
    /// table.
    pub(crate) fn encode_section(&self, syms: &[(u8, u8)], map: &[u8], out: &mut Vec<u8>) {
        let q = slice_len(syms.len());
        let mut parts: Vec<Vec<u8>> = syms
            .chunks(q)
            .map(|part| {
                let mut w = RawWriter::default();
                for &(ctx, s) in part {
                    let t = map[ctx as usize] as usize;
                    w.put(
                        self.codes[t][s as usize],
                        self.tables.lens[t][s as usize] as u32,
                    );
                }
                w.finish()
            })
            .collect();
        parts.resize(STREAMS, Vec::new());
        for p in &parts[..STREAMS - 1] {
            crate::put_varint(out, p.len() as u64);
        }
        for p in &parts {
            out.extend_from_slice(p);
        }
    }
}

// ---- decoder --------------------------------------------------------------

/// The four streams of a section.
fn section_streams(sec: &[u8]) -> Result<[&[u8]; STREAMS], Error> {
    let mut r = crate::ByteReader::new(sec);
    let mut lens = [0usize; STREAMS - 1];
    for l in lens.iter_mut() {
        *l = r.var_usize()?;
    }
    let mut out = [&[][..]; STREAMS];
    for (i, &l) in lens.iter().enumerate() {
        out[i] = r.bytes(l)?;
    }
    out[STREAMS - 1] = r.bytes(r.remaining())?;
    Ok(out)
}

/// Sentinel refill: the container keeps its valid bits below a single 1
/// bit, so no per-symbol bit count is kept; the count is the sentinel's
/// position, recovered here once per refill. Reads past the stream see
/// zeros (the caller checks the consumed length afterwards).
#[inline(always)]
fn refill(buf: &[u8], c: &mut u64, p: &mut usize) {
    let n = 63 - c.leading_zeros();
    let w = match buf.get(*p..*p + 8) {
        Some(b) => u64::from_le_bytes(b.try_into().unwrap()),
        None => rawbits::window(buf, *p),
    };
    let n2 = n | 56;
    let v = (*c ^ (1u64 << n)) | (w << n);
    *c = (v & ((1u64 << n2) - 1)) | (1u64 << n2);
    *p += ((63 - n) >> 3) as usize;
}

/// Decodes one symbol with table `t`.
#[inline(always)]
fn sym(dec: &[u16; MAX_TABLES << LOG], t: usize, c: &mut u64) -> u8 {
    let e = dec[((t << LOG) | (*c as usize & (TSIZE - 1))) & ((MAX_TABLES << LOG) - 1)];
    // The low bits of the entry are the length.
    *c = c.wrapping_shr(e as u32 & 15);
    (e >> 8) as u8
}

/// Bits consumed of a stream; `Corrupt` past its end.
fn check_end(buf: &[u8], c: u64, p: usize) -> Result<(), Error> {
    let held = 63 - c.leading_zeros() as u64;
    if p as u64 * 8 > buf.len() as u64 * 8 + held {
        return Err(Error::Corrupt("Huffman stream overrun"));
    }
    Ok(())
}

/// Static tables: `out[i]` decoded with table `tids[i]`
/// (`tids.len() >= out.len()`).
#[archmage::autoversion]
fn decode_static(
    dec: &[u16; MAX_TABLES << LOG],
    tids: &[u8],
    streams: [&[u8]; STREAMS],
    out: &mut [u8],
) -> [(u64, usize); STREAMS] {
    let q = slice_len(out.len());
    let mut c = [1u64; STREAMS];
    let mut p = [0usize; STREAMS];
    let mut parts: [&mut [u8]; STREAMS] = {
        let mut it = out.chunks_mut(q);
        core::array::from_fn(|_| it.next().unwrap_or(&mut []))
    };
    let tparts: [&[u8]; STREAMS] = core::array::from_fn(|i| {
        let a = (i * q).min(tids.len());
        &tids[a..(a + q).min(tids.len())]
    });
    // Lockstep rounds while every slice has PER_REFILL symbols left (the
    // last slice is the shortest).
    let m = parts[STREAMS - 1].len() / PER_REFILL * PER_REFILL;
    {
        let mut its = parts
            .each_mut()
            .map(|x| x[..m].as_chunks_mut::<PER_REFILL>().0.iter_mut());
        let mut tis = tparts.map(|x| x[..m].as_chunks::<PER_REFILL>().0.iter());
        for _ in 0..m / PER_REFILL {
            for i in 0..STREAMS {
                refill(streams[i], &mut c[i], &mut p[i]);
            }
            let d: [&mut [u8; PER_REFILL]; STREAMS] =
                core::array::from_fn(|i| its[i].next().unwrap());
            let t: [&[u8; PER_REFILL]; STREAMS] = core::array::from_fn(|i| tis[i].next().unwrap());
            for k in 0..PER_REFILL {
                for i in 0..STREAMS {
                    d[i][k] = sym(dec, t[i][k] as usize, &mut c[i]);
                }
            }
        }
    }
    for i in 0..STREAMS {
        for j in m..parts[i].len() {
            refill(streams[i], &mut c[i], &mut p[i]);
            parts[i][j] = sym(dec, tparts[i][j] as usize, &mut c[i]);
        }
    }
    core::array::from_fn(|i| (c[i], p[i]))
}

/// Luma counts: the table of each symbol follows from the previous one.
#[archmage::autoversion]
fn decode_luma(
    dec: &[u16; MAX_TABLES << LOG],
    map: &[u8; LUMA_CTX],
    streams: [&[u8]; STREAMS],
    out: &mut [u8],
) -> [(u64, usize); STREAMS] {
    // Class -> table (classes above 5 cannot occur: counts are <= 16).
    let tab: [u8; 8] = core::array::from_fn(|i| map[i.min(LUMA_CTX - 1)]);
    let q = slice_len(out.len());
    let mut c = [1u64; STREAMS];
    let mut p = [0usize; STREAMS];
    let mut prev = [0u8; STREAMS];
    let mut parts: [&mut [u8]; STREAMS] = {
        let mut it = out.chunks_mut(q);
        core::array::from_fn(|_| it.next().unwrap_or(&mut []))
    };
    // Lockstep rounds (four independent chains), then the tails.
    let m = parts[STREAMS - 1].len() / PER_REFILL * PER_REFILL;
    {
        let mut its = parts
            .each_mut()
            .map(|x| x[..m].as_chunks_mut::<PER_REFILL>().0.iter_mut());
        for _ in 0..m / PER_REFILL {
            for i in 0..STREAMS {
                refill(streams[i], &mut c[i], &mut p[i]);
            }
            let d: [&mut [u8; PER_REFILL]; STREAMS] =
                core::array::from_fn(|i| its[i].next().unwrap());
            for k in 0..PER_REFILL {
                for i in 0..STREAMS {
                    let t = tab[class(prev[i] as u32) & 7] as usize;
                    prev[i] = sym(dec, t, &mut c[i]);
                    d[i][k] = prev[i];
                }
            }
        }
    }
    for i in 0..STREAMS {
        for j in m..parts[i].len() {
            refill(streams[i], &mut c[i], &mut p[i]);
            let t = tab[class(prev[i] as u32) & 7] as usize;
            prev[i] = sym(dec, t, &mut c[i]);
            parts[i][j] = prev[i];
        }
    }
    core::array::from_fn(|i| (c[i], p[i]))
}

/// Decoder state of a stripe's side information.
#[derive(Default)]
pub(crate) struct Scratch {
    luma: Vec<u8>,
    chroma: Vec<u8>,
    tids: Vec<u8>,
    flat: Vec<u8>,
}

/// Decodes the three sections into `counts[bc]` and `wid[bc * 16 + gi]`
/// (`wid` sized by the caller; slots past a block-channel's count are
/// unspecified).
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_side(
    t: &HufTables,
    dec: &[u16; MAX_TABLES << LOG],
    rows: &WidRows,
    sections: [&[u8]; 3],
    nblocks: usize,
    counts: &mut [u8],
    wid: &mut [u8],
    sc: &mut Scratch,
) -> Result<(), Error> {
    let check = |s: &[&[u8]; STREAMS], st: &[(u64, usize); STREAMS]| -> Result<(), Error> {
        for i in 0..STREAMS {
            check_end(s[i], st[i].0, st[i].1)?;
        }
        Ok(())
    };
    // 1. Luma counts.
    let s = section_streams(sections[0])?;
    sc.luma.clear();
    sc.luma.resize(nblocks, 0);
    let st = decode_luma(dec, &t.luma_map, s, &mut sc.luma);
    check(&s, &st)?;
    // 2. Chroma counts.
    sc.tids.clear();
    for &n in &sc.luma {
        let cl = class(n as u32);
        sc.tids
            .extend_from_slice(&[t.chroma_map[cl], t.chroma_map[CLASSES + cl]]);
    }
    let s = section_streams(sections[1])?;
    sc.chroma.clear();
    sc.chroma.resize(2 * nblocks, 0);
    let st = decode_static(dec, &sc.tids, s, &mut sc.chroma);
    check(&s, &st)?;
    for b in 0..nblocks {
        counts[b * NCH] = sc.luma[b];
        counts[b * NCH + 1] = sc.chroma[2 * b];
        counts[b * NCH + 2] = sc.chroma[2 * b + 1];
    }
    // 3. Widths: the table of every group from the counts, 16 at a time.
    let nbc = nblocks * NCH;
    let total: usize = counts[..nbc].iter().map(|&n| n as usize).sum();
    sc.tids.clear();
    sc.tids.resize(total + PK_GROUPS, 0);
    let mut off = 0;
    for b in 0..nblocks {
        let ln = counts[b * NCH] as usize;
        for c in 0..NCH {
            let n = counts[b * NCH + c] as usize;
            let l = if c == 0 { n } else { ln };
            let dst: &mut [u8; PK_GROUPS] =
                (&mut sc.tids[off..off + PK_GROUPS]).try_into().unwrap();
            *dst = rows.0[c][l];
            off += n;
        }
    }
    let s = section_streams(sections[2])?;
    sc.flat.clear();
    sc.flat.resize(total + PK_GROUPS, 0);
    let st = decode_static(dec, &sc.tids, s, &mut sc.flat[..total]);
    check(&s, &st)?;
    let mut off = 0;
    for bc in 0..nbc {
        let src: &[u8; PK_GROUPS] = sc.flat[off..off + PK_GROUPS].try_into().unwrap();
        let dst: &mut [u8; PK_GROUPS] = (&mut wid[bc * PK_GROUPS..][..PK_GROUPS])
            .try_into()
            .unwrap();
        *dst = *src;
        off += counts[bc] as usize;
    }
    Ok(())
}

/// Width tables by (channel, L, group): `rows[c][l][gi]`.
pub(crate) struct WidRows(pub [[[u8; PK_GROUPS]; PK_GROUPS + 1]; NCH]);

impl WidRows {
    pub(crate) fn new(t: &HufTables) -> WidRows {
        WidRows(core::array::from_fn(|c| {
            core::array::from_fn(|l| core::array::from_fn(|gi| t.wid_map[wid_ctx(c, gi, l as u32)]))
        }))
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    #[test]
    fn lengths_are_prefix_free_and_limited() {
        let mut s = 0x1234_5678u32;
        for case in 0..200 {
            let na = 2 + case % 30;
            let freq: Vec<u64> = (0..na)
                .map(|_| {
                    s ^= s << 13;
                    s ^= s >> 17;
                    s ^= s << 5;
                    if s.is_multiple_of(4) {
                        0
                    } else {
                        (s >> (s % 24)) as u64
                    }
                })
                .collect();
            let l = lengths(&freq);
            assert!(l.iter().all(|&v| v as u32 <= LOG));
            let k: u64 = l
                .iter()
                .filter(|&&v| v > 0)
                .map(|&v| 1u64 << (LOG - v as u32))
                .sum();
            assert!(k <= TSIZE as u64 && k > 0);
            for (i, &f) in freq.iter().enumerate() {
                if f > 0 {
                    assert!(l[i] > 0, "used symbol without a code");
                }
            }
        }
    }
}
