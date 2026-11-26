//! Frozen rate tables and frequency accumulators for run-level rANS
//! ([`Prices`]) and split-Huffman ([`HufPrices`], [`Freqs`]).

use super::huf_dp::{group_count, group_widths};
use crate::lossy::*;
use coef::PK_GROUPS;

/// [`LossyTrellis::powers`].
#[derive(Clone, Copy)]
pub(super) struct Powers {
    /// `2^s1`, or the whole lambda `2^(s1 - 12)` when `s2 <= 0`.
    pub(super) num: f64,
    /// `2^s2` (unused when `s2 <= 0`).
    pub(super) den: f64,
}

// The trellis's f64 SIMD runs on magetypes lanes (`f64x4` of the same
// IEEE adds/mins the scalar expressions emit): `#[magetypes]` generates a
// variant per dispatch tier (`v3`, `neon`, `wasm128`, `scalar`) from one
// body, so every tier is bit-identical by construction and the token only
// changes how a value is computed, never what.

/// Token costs in bits from frozen tables.
pub(in crate::lossy) struct Prices {
    pub(super) map: Vec<u8>,
    pub(super) bits: Vec<[f64; coef::ALPHA]>,
    /// `channel_impl`'s initial `tk` per channel: the saturated
    /// remaining-positions class for every position. Fixed per `map`;
    /// the DP copies it and rewrites only the tail per candidate EOB.
    pub(super) tk0: [[u8; 64]; NCH],
}

impl Prices {
    pub(in crate::lossy) fn new(tables: &Tables) -> Self {
        let map = tables.ctx_map.clone();
        let mut tk0 = [[0u8; 64]; NCH];
        for (ch, tk) in tk0.iter_mut().enumerate() {
            for (k, t) in tk.iter_mut().enumerate().skip(1) {
                *t = map[coef::EOB_CTX
                    + (ch * coef::K_CLASSES + k - 1) * coef::REM_CLASSES
                    + coef::REM_CLASSES
                    - 1];
            }
        }
        Self {
            map,
            bits: tables
                .freq
                .iter()
                .map(|f| {
                    core::array::from_fn(|s| {
                        crate::mathx::log2(M as f64 / f.get(s).copied().unwrap_or(0).max(1) as f64)
                    })
                })
                .collect(),
            tk0,
        }
    }
    #[inline]
    pub(in crate::lossy) fn token(&self, ctx: usize, symbol: u32) -> f64 {
        self.bits[self.map[ctx] as usize][symbol as usize]
    }
}

/// Split-Huffman code lengths in bits per context, from the tables the
/// writer would build for the units (explicit, no seeds); a symbol the
/// tables lack costs [`huf::LOG`] bits. `wid` rows are padded to [`WID_PAD`]
/// lanes for the V3 scan (lanes past `WID_ALPHA` are `INFINITY`).
pub(super) const WID_PAD: usize = 24;

pub(in crate::lossy) struct HufPrices {
    pub(in crate::lossy) luma: [[f64; huf::CNT_ALPHA]; huf::LUMA_CTX],
    pub(in crate::lossy) chroma: [[f64; huf::CNT_ALPHA]; huf::CHROMA_CTX],
    pub(in crate::lossy) wid: [[f64; WID_PAD]; huf::WID_CTX],
}

impl HufPrices {
    /// The first pass's prices: each unit's counts and widths scan into
    /// section symbols one at a time (later passes take the frequencies
    /// the trellis emitted while it rewrote the blocks).
    /// `k_mode`: the encode's table-count mode ([`super::table_k_mode`]).
    pub(in crate::lossy) fn new(
        units: &[StripeCoefs],
        threads: usize,
        effort: crate::effort::Effort,
        k_mode: u8,
    ) -> Self {
        let syms: Vec<huf::Syms> = crate::par::par_map(units.iter().collect(), threads, |_, sc| {
            let (counts, widths) = stripe_counts_widths(sc, sc.bw);
            huf::stripe_syms(&counts, &widths, sc.nby * sc.bw)
        });
        let (t, _) = huffman_tables(&syms, threads, effort, None, k_mode);
        Self::from_tables(&t)
    }

    /// Prices from the (context, symbol) frequencies the previous pass's
    /// block rewrites emitted — the same tables `new` derives from a
    /// rescan. `k_mode`: the encode's table-count mode
    /// ([`super::table_k_mode`]).
    pub(in crate::lossy) fn from_freqs(
        f: &Freqs,
        effort: crate::effort::Effort,
        k_mode: u8,
    ) -> Self {
        let (t, _) = huf::HufTables::build(&f[0], &f[1], &f[2], effort, None, k_mode);
        Self::from_tables(&t)
    }

    pub(super) fn from_tables(t: &huf::HufTables) -> Self {
        let bits = |table: u8, s: usize| match t.lens[table as usize].get(s) {
            Some(&n) if n > 0 => n as f64,
            _ => huf::LOG as f64,
        };
        Self {
            luma: core::array::from_fn(|c| core::array::from_fn(|s| bits(t.luma_map[c], s))),
            chroma: core::array::from_fn(|c| core::array::from_fn(|s| bits(t.chroma_map[c], s))),
            wid: core::array::from_fn(|c| {
                core::array::from_fn(|s| {
                    if s < huf::WID_ALPHA {
                        bits(t.wid_map[c], s)
                    } else {
                        f64::INFINITY
                    }
                })
            }),
        }
    }
}

/// Split-Huffman (context, symbol) frequencies a pass emits for the next
/// pass's [`HufPrices`]: the same multiset `stripe_syms` would list and
/// `huffman_tables` would count, accumulated per worker as the trellis
/// rewrites the blocks (fixed ~23KB where a retained per-block record
/// would grow with the image).
pub(in crate::lossy) type Freqs = [Vec<Vec<u64>>; 3];

/// A [`Freqs`] as shared atomic cells: each worker merges its per-stripe
/// frequencies into one accumulator at stripe end, so nothing sized by
/// the image survives the pass.
pub(super) type SharedFreqs = [Vec<Vec<core::sync::atomic::AtomicU64>>; 3];

pub(in crate::lossy) fn freqs_new() -> Freqs {
    [
        vec![vec![0; huf::CNT_ALPHA]; huf::LUMA_CTX],
        vec![vec![0; huf::CNT_ALPHA]; huf::CHROMA_CTX],
        vec![vec![0; huf::WID_ALPHA]; huf::WID_CTX],
    ]
}

pub(super) fn shared_freqs() -> SharedFreqs {
    let rows = |ctx: usize, alpha: usize| {
        (0..ctx)
            .map(|_| {
                (0..alpha)
                    .map(|_| core::sync::atomic::AtomicU64::new(0))
                    .collect()
            })
            .collect()
    };
    [
        rows(huf::LUMA_CTX, huf::CNT_ALPHA),
        rows(huf::CHROMA_CTX, huf::CNT_ALPHA),
        rows(huf::WID_CTX, huf::WID_ALPHA),
    ]
}

/// Fold block `b`'s final split-Huffman counts and widths into `f`: the
/// same (context, symbol) pairs `stripe_syms` lists for the block.
pub(super) fn emit_huf(
    f: &mut Freqs,
    sc: &StripeCoefs,
    b: usize,
    hw: Option<&[(u32, [u8; PK_GROUPS]); NCH]>,
) {
    // `hw` carries the channel loop's chosen (count, widths); blocks it
    // skipped (aq-0) or a run-level call rescan `nz`/`q` for the same
    // values (`at` lifts a lane to `2^(w-1)`, so the scan reads `w` back).
    let count = |bc: usize, c: usize| match hw {
        Some(hw) => hw[c].0 as usize,
        None => group_count(sc.nz[bc] & !1) as usize,
    };
    let n0 = count(b * NCH, 0);
    let prev = if b.is_multiple_of(huf::slice_len(sc.nby * sc.bw)) {
        0
    } else {
        group_count(sc.nz[(b - 1) * NCH] & !1) as usize
    };
    f[0][huf::class(prev as u32)][n0] += 1;
    let cl = huf::class(n0 as u32);
    for c in 1..NCH {
        f[1][(c - 1) * huf::CLASSES + cl][count(b * NCH + c, c)] += 1;
    }
    for c in 0..NCH {
        let bc = b * NCH + c;
        let n = count(bc, c);
        let scan;
        let ws: &[u8; PK_GROUPS] = match hw {
            Some(hw) => &hw[c].1,
            None => {
                scan = group_widths(&sc.q[bc * 64..][..64], n as u32);
                &scan
            }
        };
        for (gi, &w) in ws.iter().enumerate().take(n) {
            f[2][huf::wid_ctx(c, gi, n0 as u32)][w as usize] += 1;
        }
    }
}

/// The trellis's rate model: run-level tokens, or split-Huffman counts and
/// widths (lab).
// One `Rates` per encode, read in the hot loop: keep `Prices` inline.
#[allow(clippy::large_enum_variant)]
pub(super) enum Rates {
    RunLevel(Prices),
    Huf(alloc::boxed::Box<HufPrices>),
}
