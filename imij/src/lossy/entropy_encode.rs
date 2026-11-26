//! AC entropy encoding, stripe/unit tokenization, and shared token histogram
//! accumulation for run-level rANS and split-Huffman modes.

use alloc::vec;
use alloc::vec::Vec;
use how_far::{ProgressExt, Pulse, Report};

use super::{
    Group, LossyConfig, LossyEntropy, NCH, StripeCoefs, coef, huf, local_ans, model_candidates,
    rawbits, table_k_mode,
};
#[cfg(all(feature = "lab", feature = "std"))]
use super::{block_lab, prof};
use crate::Error;
use crate::bits::BitWriter;
use crate::put_varint;
use crate::rans::{EncodeTables, NL, Sym, encode_lanes_tables};
use coef::{Tables, ZIGZAG, ac_ctx, eob_ctx, rl_token};

/// Fixed-size AC token/symbol frequency accumulator shared across whole-image
/// pilot passes, bounded streaming passes, and partition price estimation.
///
/// Accumulates run-level token counts (`nraw * ALPHA`), split-Huffman
/// `(context, symbol)` frequencies (`[luma, chroma, wid]`), or both in a single
/// pass. Merging two accumulators is exact integer addition, guaranteeing
/// 0-bit identical tables across thread counts and bounded streaming passes.
#[cfg(feature = "encode")]
pub(in crate::lossy) struct TokenHistogram {
    pub(in crate::lossy) rl: Vec<u64>,
    pub(in crate::lossy) freqs: super::trellis::Freqs,
}

#[cfg(feature = "encode")]
impl TokenHistogram {
    /// Accumulator for a single entropy model (`split_huf` vs run-level).
    pub(in crate::lossy) fn new(split_huf: bool) -> Self {
        Self::with_modes(!split_huf, split_huf)
    }

    /// Accumulator collecting both run-level and split-Huffman counts in one
    /// pass (used when a decode-speed tier chooses the coder after pass 1).
    pub(in crate::lossy) fn both() -> Self {
        Self::with_modes(true, true)
    }

    fn with_modes(want_rl: bool, want_huf: bool) -> Self {
        let rl_len = LossyEntropy::RunLevel.nraw() * coef::ALPHA;
        Self {
            rl: if want_rl {
                vec![0u64; rl_len]
            } else {
                Vec::new()
            },
            freqs: if want_huf {
                super::trellis::freqs_new()
            } else {
                [Vec::new(), Vec::new(), Vec::new()]
            },
        }
    }

    /// Tallies one column-group unit's run-level tokens (when run-level
    /// counting is enabled on this accumulator).
    pub(in crate::lossy) fn add_rl(&mut self, u: &StripeCoefs) {
        if !self.rl.is_empty() {
            stripe_tokens(u, u.bw, Group::ALL, |_, ctx, tok, _, _| {
                self.rl[ctx * coef::ALPHA + tok as usize] += 1;
            });
        }
    }

    /// Tallies one column-group unit's split-Huffman symbols (when
    /// split-Huffman counting is enabled on this accumulator).
    pub(in crate::lossy) fn add_huf(&mut self, u: &StripeCoefs) {
        if !self.freqs[0].is_empty() {
            let (counts, widths) = stripe_counts_widths(u, u.bw);
            let syms = huf::stripe_syms(&counts, &widths, u.nby * u.bw);
            let [luma, chroma, wid] = &mut self.freqs;
            for (h, symbols) in [(luma, &syms.luma), (chroma, &syms.chroma), (wid, &syms.wid)] {
                for &(ctx, sym) in symbols {
                    h[ctx as usize][sym as usize] += 1;
                }
            }
        }
    }

    /// Tallies one column-group unit into the enabled histogram(s).
    pub(in crate::lossy) fn add_unit(&mut self, u: &StripeCoefs) {
        self.add_rl(u);
        self.add_huf(u);
    }

    /// Tallies a full-width stripe by splitting into column-group units when
    /// `group_bw > 0`, consuming `sc` without cloning.
    pub(in crate::lossy) fn add_owned_stripe(&mut self, sc: StripeCoefs, group_bw: usize) {
        if group_bw == 0 {
            self.add_unit(&sc);
        } else {
            for u in split_units(sc, group_bw) {
                self.add_unit(&u);
            }
        }
    }

    /// Tallies a borrowed full-width stripe (splits a clone only when
    /// `group_bw > 0`).
    pub(in crate::lossy) fn add_stripe(&mut self, sc: &StripeCoefs, group_bw: usize) {
        if group_bw == 0 {
            self.add_unit(sc);
        } else {
            for u in split_units(sc.clone(), group_bw) {
                self.add_unit(&u);
            }
        }
    }

    /// Exact elementwise addition of another accumulator of the same shape.
    pub(in crate::lossy) fn merge(&mut self, other: Self) {
        for (d, s) in self.rl.iter_mut().zip(other.rl) {
            *d += s;
        }
        for (dst, src) in self.freqs.iter_mut().zip(other.freqs) {
            for (drow, srow) in dst.iter_mut().zip(src) {
                for (d, s) in drow.iter_mut().zip(srow) {
                    *d += s;
                }
            }
        }
    }

    /// Zeroes run-level counts in place without reallocating.
    pub(in crate::lossy) fn clear_rl(&mut self) {
        self.rl.fill(0);
    }

    /// Zeroes split-Huffman frequencies in place without reallocating.
    pub(in crate::lossy) fn clear_freqs(&mut self) {
        for table in &mut self.freqs {
            for row in table {
                row.fill(0);
            }
        }
    }

    /// Expands flat context-major run-level counts into per-context rows for
    /// `Tables::build_seeded`.
    pub(in crate::lossy) fn rl_rows(&self) -> Vec<Vec<u64>> {
        self.rl
            .as_chunks::<{ coef::ALPHA }>()
            .0
            .iter()
            .map(|row| row.to_vec())
            .collect()
    }

    /// Builds a trellis pricing pass from the accumulated counts.
    pub(in crate::lossy) fn trellis_pass(
        &self,
        set: &super::trellis::LossyTrellis,
        cfg: &LossyConfig,
        effort: crate::effort::Effort,
        split_huf: bool,
    ) -> super::trellis::Pass {
        if split_huf {
            super::trellis::Pass::split_huf(&self.freqs, set, cfg, effort)
        } else {
            super::trellis::Pass::run_level(&self.rl, set, cfg, effort)
        }
    }

    /// Builds frozen superblock-selector token prices from the accumulated counts.
    pub(in crate::lossy) fn selector_coder(
        &self,
        cfg: &LossyConfig,
        effort: crate::effort::Effort,
        split_huf: bool,
    ) -> super::superblock::select::Coder {
        super::superblock::select::Coder::of_counts(&self.rl, &self.freqs, cfg, effort, split_huf)
    }
}

/// Splits a stripe's coefficients into column-group units of `gb` blocks.
#[cfg(feature = "encode")]
pub(in crate::lossy) fn split_units(sc: StripeCoefs, gb: usize) -> Vec<StripeCoefs> {
    let bw = sc.bw;
    (0..bw)
        .step_by(gb)
        .map(|bx0| {
            let gbw = gb.min(bw - bx0);
            let n = sc.nby * gbw;
            let (mut q, mut nz, mut aq) = (
                Vec::with_capacity(n * NCH * 64),
                Vec::with_capacity(n * NCH),
                Vec::new(),
            );
            let mut modes = Vec::new();
            let mut splits = Vec::new();
            for r in 0..sc.nby {
                let b0 = r * bw + bx0;
                q.extend_from_slice(&sc.q[b0 * NCH * 64..(b0 + gbw) * NCH * 64]);
                nz.extend_from_slice(&sc.nz[b0 * NCH..(b0 + gbw) * NCH]);
                if !sc.modes.is_empty() {
                    modes.extend_from_slice(&sc.modes[b0..b0 + gbw]);
                }
                if !sc.splits.is_empty() {
                    splits.extend_from_slice(&sc.splits[b0..b0 + gbw]);
                }
                if !sc.aq.is_empty() {
                    aq.extend_from_slice(&sc.aq[b0..b0 + gbw]);
                }
            }
            StripeCoefs {
                by0: sc.by0,
                nby: sc.nby,
                bw: gbw,
                q,
                nz,
                aq,
                modes,
                splits,
            }
        })
        .collect()
}

/// Visits the tokens of every lane of a stripe in lane order (encoder):
/// `emit(lane, ctx, token, extra bit count, extra bits)`.
#[cfg(feature = "encode")]
pub(in crate::lossy) fn stripe_tokens(
    sc: &StripeCoefs,
    bw: usize,
    grp: Group,
    mut emit: impl FnMut(usize, usize, u32, u32, u32),
) {
    let nch = grp.nch;
    for l in 0..NL {
        let mut prev_luma = 0u32;
        for r in 0..sc.nby {
            for bx in (l..bw).step_by(NL) {
                let b = r * bw + bx;
                let mut luma = 0u32;
                for c in 0..nch {
                    let ch = grp.ch0 + c;
                    let q: &[i32; 64] = sc.q[(b * nch + c) * 64..][..64].try_into().unwrap();
                    let ac = sc.nz[b * nch + c] & !1;
                    let eob = if ac == 0 { 0 } else { 63 - ac.leading_zeros() };
                    emit(
                        l,
                        eob_ctx(ch, if c == 0 { prev_luma } else { luma }),
                        eob,
                        0,
                        0,
                    );
                    if c == 0 {
                        luma = eob;
                        prev_luma = eob;
                    }
                    // Run-level tokens; `start` is the decoder's next position.
                    let mut start = 1u32;
                    let mut bits = ac;
                    while bits != 0 {
                        let k = bits.trailing_zeros();
                        bits &= bits - 1;
                        let v = q[ZIGZAG[k as usize] as usize];
                        let mut run = k - start;
                        while run > coef::MAX_RUN {
                            emit(l, ac_ctx(ch, start, eob), coef::ZRL, 0, 0);
                            start += 4;
                            run -= 4;
                        }
                        let (t, e, n) = rl_token(run, v);
                        emit(l, ac_ctx(ch, start, eob), t, n, e);
                        start = k + 1;
                    }
                }
            }
        }
    }
}

/// The split-Huffman mode's plain bit streams of a stripe: magnitudes
/// (`len * w` bits per group, coefficient j at bit `j * w`) and signs (one
/// bit per nonzero, 1 = negative), blocks in raster order, channels,
/// groups.
#[cfg(feature = "encode")]
pub(in crate::lossy) fn stripe_raw_split(
    sc: &StripeCoefs,
    bw: usize,
    counts: &[u8],
    widths: &[u8],
) -> (Vec<u8>, Vec<u8>) {
    debug_assert_eq!(counts.len(), sc.nby * bw * NCH);
    debug_assert_eq!(widths.len(), counts.len() * coef::PK_GROUPS);
    let (mut mag, mut sgn) = (rawbits::RawWriter::default(), rawbits::RawWriter::default());
    for b in 0..sc.nby * bw {
        for c in 0..NCH {
            let bc = b * NCH + c;
            let q: &[i32; 64] = sc.q[bc * 64..][..64].try_into().unwrap();
            let n = counts[bc] as usize;
            for gi in 0..n {
                let len = coef::pk_len(gi);
                let vals: [i32; 4] = core::array::from_fn(|j| {
                    if j < len {
                        q[ZIGZAG[1 + 4 * gi + j] as usize]
                    } else {
                        0
                    }
                });
                let w = u32::from(widths[bc * coef::PK_GROUPS + gi]);
                for &v in &vals[..len] {
                    mag.put(v.unsigned_abs(), w);
                }
                for &v in &vals[..len] {
                    if v != 0 {
                        sgn.put((v < 0) as u32, 1);
                    }
                }
            }
        }
    }
    (mag.finish(), sgn.finish())
}

/// Split-mode group counts `[bc]` and widths `[bc * 16 + gi]` of a stripe.
#[cfg(feature = "encode")]
pub(in crate::lossy) fn stripe_counts_widths(sc: &StripeCoefs, bw: usize) -> (Vec<u8>, Vec<u8>) {
    let nbc = sc.nby * bw * NCH;
    let mut counts = vec![0u8; nbc];
    let mut widths = vec![0u8; nbc * coef::PK_GROUPS];
    for bc in 0..nbc {
        let q: &[i32; 64] = sc.q[bc * 64..][..64].try_into().unwrap();
        let ac = sc.nz[bc] & !1;
        let eob = if ac == 0 { 0 } else { 63 - ac.leading_zeros() };
        let n = eob.div_ceil(4) as usize;
        counts[bc] = n as u8;
        for gi in 0..n {
            let len = coef::pk_len(gi);
            let m = (0..len)
                .map(|j| q[ZIGZAG[1 + 4 * gi + j] as usize].unsigned_abs())
                .max()
                .unwrap();
            widths[bc * coef::PK_GROUPS + gi] = (32 - m.leading_zeros()) as u8;
        }
    }
    (counts, widths)
}

#[cfg(feature = "encode")]
pub(in crate::lossy) fn stripe_symbols(sc: &StripeCoefs, bw: usize, lanes: &mut [Vec<Sym>; NL]) {
    let mut steps = [0u32; NL];
    for l in lanes.iter_mut() {
        l.clear();
        l.reserve(sc.q.len() / 64 / NL * 8);
    }
    stripe_tokens(sc, bw, Group::ALL, |l, ctx, tok, nb, extra| {
        lanes[l].push(Sym {
            step: steps[l],
            ctx: ctx as u16,
            tok: tok as u8,
            nb: nb as u8,
            extra,
        });
        steps[l] += 1;
    });
    #[cfg(all(feature = "lab", feature = "std"))]
    prof::tokens(&steps);
}

/// Build image-wide ANS tables with bounded, worker-local histogram storage.
#[cfg(feature = "encode")]
pub(in crate::lossy) fn ans_tables(
    stripes: &[StripeCoefs],
    cfg: &LossyConfig,
    effort: crate::effort::Effort,
) -> (Tables, Option<Tables>) {
    ans_tables_with_bank(stripes, cfg, effort, seed_bank(cfg))
}

/// Use the usual table-count selection for coefficient and partition prices.
/// Choosing a different file bank is an entropy-writing decision.
#[cfg(feature = "encode")]
pub(in crate::lossy) fn ans_tables_for_decisions(
    stripes: &[StripeCoefs],
    cfg: &LossyConfig,
    effort: crate::effort::Effort,
) -> (Tables, Option<Tables>) {
    ans_tables_with_bank(
        stripes,
        cfg,
        effort,
        crate::seeds::bank(crate::seeds::DEFAULT_BANK),
    )
}

#[cfg(feature = "encode")]
pub(in crate::lossy) fn ans_tables_with_bank(
    stripes: &[StripeCoefs],
    cfg: &LossyConfig,
    effort: crate::effort::Effort,
    bank: Option<&'static crate::seeds::Bank>,
) -> (Tables, Option<Tables>) {
    // u64 is needed because each worker can accumulate many stripes.
    let hists = crate::par::par_fold(
        stripes,
        cfg.threads,
        || vec![0u64; LossyEntropy::RunLevel.nraw() * coef::ALPHA],
        |hist, sc| {
            stripe_tokens(sc, sc.bw, Group::ALL, |_, ctx, tok, _, _| {
                hist[ctx * coef::ALPHA + tok as usize] += 1;
            });
        },
    );
    let mut hists = hists.into_iter();
    let mut hist = hists.next().unwrap();
    for h in hists {
        for (sum, count) in hist.iter_mut().zip(h) {
            *sum += count;
        }
    }
    let hist: Vec<Vec<u64>> = hist
        .as_chunks::<{ coef::ALPHA }>()
        .0
        .iter()
        .map(|row| row.to_vec())
        .collect();
    Tables::build_seeded(
        &hist,
        4,
        cfg.max_ac_tables,
        LossyEntropy::RunLevel,
        effort,
        bank,
        table_k_mode(cfg),
    )
}

/// The seed bank `cfg` encodes with: bank 1 (lab: `seed_bank`, 0 = none).
#[cfg(feature = "encode")]
pub(in crate::lossy) fn seed_bank(cfg: &LossyConfig) -> Option<&'static crate::seeds::Bank> {
    #[cfg(feature = "lab")]
    let id = cfg.seed_bank;
    #[cfg(not(feature = "lab"))]
    let id = {
        let _ = cfg;
        crate::seeds::DEFAULT_BANK
    };
    crate::seeds::bank(id)
}

/// Seeded blobs are also coded explicitly when the lab `seed_exact` policy is on.
#[cfg(feature = "encode")]
pub(in crate::lossy) fn seed_exact(cfg: &LossyConfig) -> bool {
    #[cfg(feature = "lab")]
    return cfg.seed_exact;
    #[cfg(not(feature = "lab"))]
    {
        let _ = cfg;
        true
    }
}

/// Table-independent stripe work, shared by seeded and explicit trials.
#[cfg(feature = "encode")]
pub(in crate::lossy) struct HuffmanStripe {
    pub(in crate::lossy) symbols: huf::Syms,
    pub(in crate::lossy) magnitudes: Vec<u8>,
    pub(in crate::lossy) signs: Vec<u8>,
}

#[cfg(feature = "encode")]
pub(in crate::lossy) fn huffman_tables(
    syms: &[huf::Syms],
    threads: usize,
    effort: crate::effort::Effort,
    bank: Option<&'static crate::seeds::Bank>,
    k_mode: u8,
) -> (huf::HufTables, Option<huf::HufTables>) {
    let hists = crate::par::par_fold(
        syms,
        threads,
        || {
            [
                vec![vec![0u64; huf::CNT_ALPHA]; huf::LUMA_CTX],
                vec![vec![0u64; huf::CNT_ALPHA]; huf::CHROMA_CTX],
                vec![vec![0u64; huf::WID_ALPHA]; huf::WID_CTX],
            ]
        },
        |hist, syms| {
            for (h, symbols) in hist.iter_mut().zip([&syms.luma, &syms.chroma, &syms.wid]) {
                for &(ctx, sym) in symbols {
                    h[ctx as usize][sym as usize] += 1;
                }
            }
        },
    );
    let mut hists = hists.into_iter();
    let mut hist = hists.next().unwrap();
    for h in hists {
        for (dst, src) in hist.iter_mut().zip(h) {
            for (a, b) in dst.iter_mut().zip(src) {
                for (sum, count) in a.iter_mut().zip(b) {
                    *sum += count;
                }
            }
        }
    }
    huf::HufTables::build(&hist[0], &hist[1], &hist[2], effort, bank, k_mode)
}

/// Split-Huffman stores three side-information sections, then raw magnitudes
/// and signs. Canonical codes are shared by all stripe workers.
#[cfg(feature = "encode")]
pub(in crate::lossy) fn encode_huffman_stripes(
    stripes: &[StripeCoefs],
    cfg: &LossyConfig,
    effort: crate::effort::Effort,
    pulse: &dyn Pulse,
) -> Result<(Vec<u8>, Vec<Vec<u8>>, Option<Coded>), Error> {
    if cfg.model_references {
        return model_candidates::encode_native(
            stripes,
            cfg,
            effort,
            LossyEntropy::SplitHuf,
            pulse,
        );
    }
    if cfg.catalog_lab.is_some() {
        return model_candidates::encode(stripes, cfg, effort, LossyEntropy::SplitHuf, pulse);
    }
    let pulse = pulse.live();
    use how_far::Stop;
    pulse.check().map_err(|_| Error::Cancelled)?;
    // Enumerate each stripe once: the same section symbols feed both the
    // table histograms and the final coding.
    let syms: Vec<HuffmanStripe> = crate::par::par_map(
        stripes.iter().collect(),
        cfg.threads,
        |_, sc| -> Result<_, Error> {
            pulse.check().map_err(|_| Error::Cancelled)?;
            let (counts, widths) = stripe_counts_widths(sc, sc.bw);
            let symbols = huf::stripe_syms(&counts, &widths, sc.nby * sc.bw);
            let (magnitudes, signs) = stripe_raw_split(sc, sc.bw, &counts, &widths);
            Ok(HuffmanStripe {
                symbols,
                magnitudes,
                signs,
            })
        },
    )
    .into_iter()
    .collect::<Result<Vec<_>, Error>>()?;
    let (syms, raw): (Vec<_>, Vec<_>) = syms
        .into_iter()
        .map(|stripe| (stripe.symbols, (stripe.magnitudes, stripe.signs)))
        .unzip();
    let (tables, explicit) = huffman_tables(
        &syms,
        cfg.threads,
        effort,
        seed_bank(cfg),
        table_k_mode(cfg),
    );
    let code = |tables: &huf::HufTables| -> Result<(Vec<u8>, Vec<Vec<u8>>), Error> {
        pulse.check().map_err(|_| Error::Cancelled)?;
        let encoder = tables.encoder();
        let stripes = crate::par::par_map(
            syms.iter().zip(raw.iter()).collect(),
            cfg.threads,
            |_, (symbols, (magnitudes, signs))| -> Result<_, Error> {
                pulse.check().map_err(|_| Error::Cancelled)?;
                let mut out = Vec::new();
                for (symbols, map) in [
                    (&symbols.luma, &tables.luma_map[..]),
                    (&symbols.chroma, &tables.chroma_map[..]),
                    (&symbols.wid, &tables.wid_map[..]),
                ] {
                    let mut section = Vec::new();
                    encoder.encode_section(symbols, map, &mut section);
                    put_varint(&mut out, section.len() as u64);
                    out.extend_from_slice(&section);
                }
                put_varint(&mut out, magnitudes.len() as u64);
                out.extend_from_slice(magnitudes);
                out.extend_from_slice(signs);
                pulse.advance(1);
                pulse.check().map_err(|_| Error::Cancelled)?;
                Ok(out)
            },
        )
        .into_iter()
        .collect::<Result<Vec<_>, Error>>()?;
        let mut header = BitWriter::new();
        tables.write(&mut header);
        Ok((header.finish(), stripes))
    };
    let (tb, records) = code(&tables)?;
    let explicit = explicit.filter(|_| seed_exact(cfg));
    Ok((tb, records, explicit.map(|t| code(&t)).transpose()?))
}

/// Run-level lane coding. Returns `(header, stripe records, token count)`:
/// the count is the run-level token total [`side_stats`] needs, free from
/// the lane pass.
#[cfg(feature = "encode")]
pub(in crate::lossy) fn encode_ans_stripes(
    stripes: &[StripeCoefs],
    cfg: &LossyConfig,
    effort: crate::effort::Effort,
    pulse: &dyn Pulse,
) -> Result<(Vec<u8>, Vec<Vec<u8>>, u64, Option<Coded>), Error> {
    if cfg.model_references || cfg.catalog_lab.is_some() {
        let (tb, records, explicit) = if cfg.model_references {
            model_candidates::encode_native(stripes, cfg, effort, LossyEntropy::RunLevel, pulse)?
        } else {
            model_candidates::encode(stripes, cfg, effort, LossyEntropy::RunLevel, pulse)?
        };
        let tokens = if cfg.decode_speed.is_some() {
            stripes
                .iter()
                .map(|sc| {
                    let mut lanes = core::array::from_fn(|_| Vec::new());
                    stripe_symbols(sc, sc.bw, &mut lanes);
                    lanes.iter().map(|l| l.len() as u64).sum::<u64>()
                })
                .sum()
        } else {
            0
        };
        return Ok((tb, records, tokens, explicit));
    }
    let pulse = pulse.live();
    use how_far::Stop;
    pulse.check().map_err(|_| Error::Cancelled)?;
    // The baseline structure verbatim — the table histogram counts tokens
    // directly and each stripe's lane list exists only inside its encode
    // worker. On the tier path the run-level token count rides back with
    // the records; the default path adds nothing.
    let want = cfg.decode_speed.is_some();
    let (tables, explicit) = ans_tables(stripes, cfg, effort);
    #[cfg(all(feature = "lab", feature = "std"))]
    block_lab::record_block_bits(stripes, &tables);
    let code = |tables: &Tables| -> Result<(Vec<u8>, Vec<Vec<u8>>, u64), Error> {
        pulse.check().map_err(|_| Error::Cancelled)?;
        let encoder = EncodeTables::new(&tables.freq, &tables.ctx_map);
        let coded = crate::par::par_map(
            stripes.iter().collect(),
            cfg.threads,
            |_, sc| -> Result<_, Error> {
                pulse.check().map_err(|_| Error::Cancelled)?;
                let mut lanes: [Vec<Sym>; NL] = core::array::from_fn(|_| Vec::new());
                stripe_symbols(sc, sc.bw, &mut lanes);
                let mut out = Vec::new();
                encode_lanes_tables(&encoder, &lanes, sc.bw.min(NL), &mut out);
                if cfg.local_ans_contexts {
                    out = local_ans::encode(&lanes, sc.bw.min(NL), cfg, effort, &out)?;
                }
                let tokens = if want {
                    lanes.iter().map(|l| l.len() as u64).sum()
                } else {
                    0
                };
                pulse.advance(1);
                pulse.check().map_err(|_| Error::Cancelled)?;
                Ok((out, tokens))
            },
        )
        .into_iter()
        .collect::<Result<Vec<_>, Error>>()?;
        let tokens = coded.iter().map(|(_, t)| *t).sum();
        let stripes = coded.into_iter().map(|(out, _)| out).collect();
        let mut header = BitWriter::new();
        tables.write(&mut header);
        Ok((header.finish(), stripes, tokens))
    };
    let (tb, records, tokens) = code(&tables)?;
    let explicit = explicit
        .filter(|_| seed_exact(cfg))
        .map(|t| {
            let (tb, records, _) = code(&t)?;
            Ok::<_, Error>((tb, records))
        })
        .transpose()?;
    Ok((tb, records, tokens, explicit))
}

/// A coded table blob and its records (the explicit-table variant of a
/// seeded coding).
#[cfg(feature = "encode")]
pub(in crate::lossy) type Coded = (Vec<u8>, Vec<Vec<u8>>);
