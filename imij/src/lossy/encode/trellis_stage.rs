//! AC trellis resolution/execution and entropy tier selection (run-level rANS
//! vs split-Huffman) for whole-image lossy frames.

use alloc::vec;
use alloc::vec::Vec;
use how_far::Pulse;

use crate::effort::Effort;
#[cfg(all(feature = "lab", feature = "std"))]
use crate::lossy::prof;
#[cfg(feature = "lab")]
use crate::lossy::rdoq_lab;
use crate::lossy::{
    Coded, Geom, LossyColor, LossyConfig, LossyEntropy, RowPlanes, SMALL_IMAGE, StripeCoefs,
    encode_ans_stripes, encode_huffman_stripes, exact_aq_to_selector, exact_aq_to_wire,
    huf_max_ratio, predicted_huf_ratio, side_stats, side_stats_lanes, split_units, trellis,
};
use crate::{Error, ImageInfo};

/// Table headers and independently coded stripe records in the selected
/// mode; with seed tables, also the same blob with explicit tables.
/// `uses_alt` is true when the records were coded from the separate
/// split-Huffman pass (`pass1`) rather than the primary `stripes` slice, so
/// partition maps and stripe metadata stay matched to the coded coefficients.
pub(super) struct AcCoding {
    pub entropy: LossyEntropy,
    pub tb: Vec<u8>,
    pub records: Vec<Vec<u8>>,
    pub explicit: Option<Coded>,
    pub uses_alt: bool,
}

impl AcCoding {
    /// The coding without seed tables.
    pub(super) fn explicit(&self) -> (&[u8], &[Vec<u8>]) {
        match &self.explicit {
            Some((t, r)) => (t, r),
            None => (&self.tb, &self.records),
        }
    }
}

/// Resolved AC trellis execution mode after pass-1 side statistics.
pub(super) struct ResolvedTrellis {
    pub trellis: Option<trellis::LossyTrellis>,
    pub tier_huf: bool,
    pub separate_tier_coefficients: bool,
    pub small: bool,
    pub stats: Option<[u64; 3]>,
}

/// Determines whether the effort's AC trellis runs and which rate model it
/// prices against (run-level vs split-Huffman), reusing pass-1 side stats.
pub(super) fn resolve_trellis_mode(
    stripes: &[StripeCoefs],
    cfg: &LossyConfig,
    g: &Geom,
    xyb: bool,
    trellis: Option<(trellis::LossyTrellis, bool)>,
) -> ResolvedTrellis {
    let small = g.w * g.h <= SMALL_IMAGE;
    let want_stats =
        (cfg.decode_speed.is_some() && !small) || cfg!(all(feature = "lab", feature = "std"));
    let mut stats = want_stats.then(|| side_stats(stripes, g.bw, cfg.threads));
    // The effort's trellis runs where the AC is coded run-level or
    // split-Huffman in XYB, priced with the coding's own rate model (in
    // YCoCg it lost 18-20% Butteraugli max; benchmarks/trellis_effort_2026-10-08,
    // aq_trellis_2026-10-09). Under a tier the cost model's choice from these
    // pass-1 counts (the tier decision below reuses them) picks the rate
    // model (`tier_huf`: an image sent to split-Huffman); small images, which
    // also try split-Huffman and keep the smaller file, code their
    // split-Huffman candidate from the split-Huffman trellis of the pass-1
    // coefficients. The lab override runs wherever it is set (`forced`);
    // lab `set_trellis_huf_rate(false)` keeps split-Huffman without it.
    let (trellis, forced, tier_huf) = match trellis {
        Some((t, true)) => (Some(t), true, false),
        Some((t, false)) if xyb => match cfg.decode_speed {
            None => (
                (cfg.entropy == LossyEntropy::RunLevel
                    || (cfg.entropy == LossyEntropy::SplitHuf && trellis::huf_rate(cfg)))
                .then_some(t),
                false,
                false,
            ),
            Some(speed) => {
                let st = *stats.get_or_insert_with(|| side_stats(stripes, g.bw, cfg.threads));
                let ratio = predicted_huf_ratio((g.w * g.h) as f64, &st);
                if huf_max_ratio(speed).is_some_and(|m| ratio <= m) {
                    (trellis::huf_rate(cfg).then_some(t), false, true)
                } else {
                    (Some(t), false, false)
                }
            }
        },
        _ => (None, false, false),
    };
    let separate_tier_coefficients =
        trellis.is_some() && !forced && !tier_huf && small && cfg.decode_speed.is_some();
    ResolvedTrellis {
        trellis,
        tier_huf,
        separate_tier_coefficients,
        small,
        stats,
    }
}

/// Splits stripes into column-group units and runs lab RDOQ + AC trellis
/// (including the separate split-Huffman candidate on small tiered images).
#[allow(clippy::too_many_arguments)]
pub(super) fn run_trellis_and_rdoq(
    stripes: Vec<StripeCoefs>,
    info: &ImageInfo,
    cfg: &LossyConfig,
    effort: Effort,
    rows: Option<RowPlanes<'_>>,
    g: &Geom,
    gcols: usize,
    ng: usize,
    color: LossyColor,
    steps: &[[f32; 64]; 3],
    has_exact: bool,
    resolved: &ResolvedTrellis,
    trellis_aq: &[f32],
    open_lambda_map: &[f32],
    trellis_bias: Option<&[[[f32; 64]; 2]; 3]>,
    rdoq_dc: Option<bool>,
    pulse: &dyn Pulse,
) -> Result<(Vec<StripeCoefs>, Option<Vec<StripeCoefs>>), Error> {
    let pw = g.bw * 8;
    #[cfg(not(feature = "lab"))]
    let _ = (info, color, rdoq_dc);

    // Coding units: (stripe, column group), stripe-major.
    let stripes: Vec<StripeCoefs> = if ng == 1 {
        stripes
    } else {
        stripes
            .into_iter()
            .flat_map(|sc| split_units(sc, gcols / 8))
            .collect()
    };
    #[cfg(feature = "lab")]
    let stripes = if cfg.rdoq_lambda != 0.0
        && (cfg.rdoq_policy == 0 || cfg.scale_for(color, info.width, info.height) <= 80.0)
    {
        rdoq_lab::optimize(
            stripes,
            rows.expect("the coefficient path runs no pixel stages"),
            pw,
            g.bw,
            cfg,
            steps,
            gcols,
            effort,
            rdoq_dc,
            pulse,
        )?
    } else {
        stripes
    };
    pulse.check().map_err(|_| Error::Cancelled)?;

    // Small tier images keep the pass-1 coefficients for their split-Huffman
    // coding when the effort's trellis runs (at most 64K pixels).
    let pass1 = resolved.separate_tier_coefficients.then(|| stripes.clone());

    // The AC trellis re-decides the AC coefficients from each stripe's
    // recomputed transform.
    // Exact masks use 0 = lossy, 1 = exact; trellis block codes use
    // 64 = ordinary quantization, 0 = keep. Translate only for the search,
    // then restore the wire codes used by the already encoded DC image.
    let mut stripes = stripes;
    if resolved.trellis.is_some() && has_exact {
        exact_aq_to_selector(&mut stripes);
    }
    let mut stripes = match &resolved.trellis {
        Some(t) => trellis::optimize(
            stripes,
            rows.expect("the coefficient path runs no pixel stages"),
            pw,
            g.bw,
            gcols / 8,
            steps,
            cfg,
            effort,
            t,
            trellis_aq,
            open_lambda_map,
            trellis_bias,
            resolved.tier_huf
                || (cfg.decode_speed.is_none()
                    && cfg.entropy == LossyEntropy::SplitHuf
                    && trellis::huf_rate(cfg)),
            pulse,
        )?,
        None => stripes,
    };
    // Small tier images' split-Huffman candidate: the split-Huffman rate
    // model's trellis of the pass-1 coefficients.
    let pass1 = match (pass1, &resolved.trellis) {
        (Some(mut p), Some(t)) if trellis::huf_rate(cfg) => {
            if has_exact {
                exact_aq_to_selector(&mut p);
            }
            let mut p = trellis::optimize(
                p,
                rows.expect("the coefficient path runs no pixel stages"),
                pw,
                g.bw,
                gcols / 8,
                steps,
                cfg,
                effort,
                t,
                trellis_aq,
                open_lambda_map,
                trellis_bias,
                true,
                pulse,
            )?;
            if has_exact {
                exact_aq_to_wire(&mut p);
            }
            Some(p)
        }
        (p, _) => p,
    };
    if resolved.trellis.is_some() && has_exact {
        exact_aq_to_wire(&mut stripes);
    }
    // Lab counters of the coded coefficients (`lab::lossy_side_stats`).
    #[cfg(all(feature = "lab", feature = "std"))]
    if resolved.trellis.is_some() {
        prof::side_stats(&trellis::unit_stats(&stripes, cfg.threads));
    } else if let Some(st) = &resolved.stats {
        prof::side_stats(st);
    }
    Ok((stripes, pass1))
}

/// Codes the AC stripe units under the configured entropy mode or decode-speed
/// tier, returning `(codings, pick, pick_explicit)`.
#[allow(clippy::too_many_arguments)]
pub(super) fn select_ac_coding(
    stripes: &[StripeCoefs],
    pass1: Option<&[StripeCoefs]>,
    cfg: &LossyConfig,
    effort: Effort,
    g: &Geom,
    resolved: &ResolvedTrellis,
    #[cfg(feature = "lab")] reference_entropy: Option<u8>,
    pulse: &dyn Pulse,
) -> Result<(Vec<AcCoding>, usize, usize), Error> {
    let code_units = |units: &[StripeCoefs], entropy, uses_alt| -> Result<AcCoding, Error> {
        let (tb, records, explicit) = if entropy == LossyEntropy::SplitHuf {
            encode_huffman_stripes(units, cfg, effort, pulse)?
        } else {
            let (tb, sb, _, explicit) = encode_ans_stripes(units, cfg, effort, pulse)?;
            (tb, sb, explicit)
        };
        Ok(AcCoding {
            entropy,
            tb,
            records,
            explicit,
            uses_alt,
        })
    };
    let code = |entropy| code_units(stripes, entropy, false);
    // The tier's choice: split-Huffman when the cost model predicts it
    // decodes within the tier's ratio of run-level's time; small images
    // try both and also take split-Huffman when it is smaller (its tables
    // cost less than the rANS tables). With seed tables the size compares
    // each mode's smaller coding; the mode the encoder picks without seeds
    // is kept too, for the final comparison below.
    match cfg.decode_speed {
        None => Ok((vec![code(cfg.entropy)?], 0, 0)),
        Some(speed) => {
            let size = |(t, r): (&[u8], &[Vec<u8>])| {
                t.len() + r.iter().map(|s| s.len() + 3).sum::<usize>()
            };
            let smaller = |c: &AcCoding| size((&c.tb, &c.records)).min(size(c.explicit()));
            if resolved.small {
                // The run-level coding's token count stands in for the
                // side-stats coefficient walk: splitting column groups
                // preserves per-block emits.
                let (rl, n) = {
                    let (tb, records, n, explicit) =
                        encode_ans_stripes(stripes, cfg, effort, pulse)?;
                    let rl = AcCoding {
                        entropy: LossyEntropy::RunLevel,
                        tb,
                        records,
                        explicit,
                        uses_alt: false,
                    };
                    (rl, n)
                };
                let st = resolved
                    .stats
                    .unwrap_or_else(|| side_stats_lanes(stripes, n));
                let ratio = predicted_huf_ratio((g.w * g.h) as f64, &st);
                let fast = huf_max_ratio(speed).is_some_and(|m| ratio <= m);
                // Split-Huffman never carries the run-level trellis's
                // coefficients (with the effort's trellis: the split-Huffman
                // rate model's, from pass 1).
                let sh = code_units(
                    pass1.unwrap_or(stripes),
                    LossyEntropy::SplitHuf,
                    pass1.is_some(),
                )?;
                let pick = (fast || smaller(&sh) < smaller(&rl)) as usize;
                let pick_explicit = (fast || size(sh.explicit()) < size(rl.explicit())) as usize;
                #[cfg(feature = "lab")]
                let (pick, pick_explicit) =
                    reference_entropy.map_or((pick, pick_explicit), |entropy| {
                        let pick = usize::from(entropy == LossyEntropy::SplitHuf as u8);
                        (pick, pick)
                    });
                Ok((vec![rl, sh], pick, pick_explicit))
            } else {
                let st = resolved.stats.as_ref().unwrap();
                let ratio = predicted_huf_ratio((g.w * g.h) as f64, st);
                let fast = huf_max_ratio(speed).is_some_and(|m| ratio <= m);
                let mode = if fast {
                    LossyEntropy::SplitHuf
                } else {
                    LossyEntropy::RunLevel
                };
                Ok((vec![code(mode)?], 0, 0))
            }
        }
    }
}
