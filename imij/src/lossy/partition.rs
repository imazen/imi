//! Superblock merge and 4x4 small-transform selection stage, exact-block AQ
//! code polarity helpers, and whole-image two-pass empirical pilot learning.

use alloc::vec::Vec;
use how_far::Stop;

use super::entropy_encode::{TokenHistogram, split_units};
#[cfg(feature = "lab")]
use super::small_lab;
use super::{
    Geom, LossyChannelLayout, LossyConfig, LossyEntropy, QParams, RowPlanes, StripeCoefs,
    adaptive_enabled, adaptive_rdo, lossy_group_cols, small, superblock, trellis,
};
use crate::{Error, ImageInfo, SampleType};

/// Smallest longer side the `Compact` tier merges blocks on: merges lost at
/// 64 px and gained from 128 px up on imazen-26 VALIDATE
/// (benchmarks/superblocks_2026-10-09).
#[cfg(feature = "encode")]
pub(in crate::lossy) const SUPERBLOCK_MIN_SIDE: u32 = 128;

/// Whether the encoder chooses superblock merges (superblock.rs). `eligible`:
/// the format allows them (default layouts, no keep blocks) and no trellis
/// runs (at efforts 9 and 10 the trellis took most of the gain, and merged
/// units are not trellis-quantized). The production rule: the `Compact`
/// tier, 8-bit samples, the longer side at least
/// [`SUPERBLOCK_MIN_SIDE`]. The lab switch overrides the rule, not
/// `eligible`.
#[cfg(feature = "encode")]
pub(in crate::lossy) fn superblocks_on(
    cfg: &LossyConfig,
    info: &ImageInfo,
    eligible: bool,
) -> bool {
    if !eligible {
        return false;
    }
    #[cfg(feature = "lab")]
    if let Some(on) = cfg.lab.superblocks {
        return on;
    }
    (cfg.adaptive_partitions || cfg.decode_speed == Some(crate::DecodeSpeed::Compact))
        && info.sample == SampleType::U8
        && info.width.max(info.height) >= SUPERBLOCK_MIN_SIDE
}

/// The small-transform selector's inner-edge weight: 0 (0.5, 1 and 4 cost
/// at CVVDP 6-8 and gained nothing at the top on the same set).
#[cfg(feature = "encode")]
pub(in crate::lossy) const SMALL_EDGE: f32 = 0.0;

/// Whether the encoder may code planes as four 4x4 DCTs (small transforms,
/// `small.rs`). `eligible`: the format and the encoder's other tools allow
/// them (default layouts, no keep blocks). Production encoding enables them
/// for explicit `small_transforms`, `adaptive_partitions` on U8, and
/// `DecodeSpeed::Compact` at trellis efforts (`>= 9.0`) on U8 images with
/// longer side `>= SUPERBLOCK_MIN_SIDE`. Lab overrides retain the study
/// configurations (`benchmarks/smallxf_2026-10-11/README.md`).
#[cfg(feature = "encode")]
pub(in crate::lossy) fn small_on(
    cfg: &LossyConfig,
    info: &ImageInfo,
    effort: crate::effort::Effort,
    eligible: bool,
) -> bool {
    if !eligible {
        return false;
    }
    #[cfg(feature = "lab")]
    if let Some(on) = cfg.lab.small.as_ref().and_then(|s| s.on) {
        return on;
    }
    cfg.small_transforms
        || (cfg.adaptive_partitions && info.sample == SampleType::U8)
        || (cfg.decode_speed == Some(crate::DecodeSpeed::Compact)
            && effort.trellis_passes() >= 1
            && info.sample == SampleType::U8
            && info.width.max(info.height) >= SUPERBLOCK_MIN_SIDE)
}

/// Translates exact-block wire codes (`0 = lossy, 1 = exact`) into selector /
/// trellis internal AQ codes (`64 = ordinary 1.0x quant, 0 = bypass/exact`).
pub(in crate::lossy) fn exact_aq_to_selector(stripes: &mut [StripeCoefs]) {
    for sc in stripes {
        for code in &mut sc.aq {
            *code = if *code == 0 { 64 } else { 0 };
        }
    }
}

/// Restores internal selector / trellis AQ codes (`0 = exact, 64 = lossy`)
/// back to exact-block wire codes (`1 = exact, 0 = lossy`) matching the DC
/// image.
pub(in crate::lossy) fn exact_aq_to_wire(stripes: &mut [StripeCoefs]) {
    for sc in stripes {
        for code in &mut sc.aq {
            *code = u8::from(*code == 0);
        }
    }
}

/// Builds the superblock / 4x4 selector options shared by whole-image and
/// bounded streaming lossy encoders.
pub(in crate::lossy) fn build_selector_opts(
    cfg: &LossyConfig,
    plan: crate::effort::LossyEffortPlan,
    sb_on: bool,
    small_on: bool,
    trellis_active: bool,
) -> superblock::select::Opts {
    let split_huf = cfg.entropy == LossyEntropy::SplitHuf
        || matches!(
            cfg.decode_speed,
            Some(crate::DecodeSpeed::Fast | crate::DecodeSpeed::Fastest)
        );
    #[cfg(feature = "lab")]
    let (shapes, force, forward, kappa, edge) = (
        cfg.lab.superblock_shapes.unwrap_or(7),
        cfg.lab.small.as_ref().and_then(|s| s.split),
        if cfg.lab.small.as_ref().is_some_and(|s| s.int) {
            small_lab::forward_int as fn(&[f32; 64]) -> [f32; 64]
        } else {
            small::forward as fn(&[f32; 64]) -> [f32; 64]
        },
        cfg.lab
            .small
            .as_ref()
            .and_then(|s| s.lambda)
            .unwrap_or(plan.small_kappa),
        cfg.lab
            .small
            .as_ref()
            .and_then(|s| s.edge)
            .unwrap_or(SMALL_EDGE),
    );
    #[cfg(not(feature = "lab"))]
    let (shapes, force, forward, kappa, edge) = (
        7u8,
        None,
        small::forward as fn(&[f32; 64]) -> [f32; 64],
        plan.small_kappa,
        SMALL_EDGE,
    );
    superblock::select::Opts {
        merges: sb_on,
        trellis: trellis_active,
        shapes,
        small: small_on.then_some(force),
        split_huf,
        forward,
        kappa,
        edge,
        rdo: plan,
    }
}

/// Runs the whole-image open-loop trellis-lambda AQ prepass, optional two-pass
/// empirical pilot learning, and superblock / 4x4 partition selection.
///
/// Returns `(stripes, sb_on, open_lambda_map)`.
#[allow(clippy::too_many_arguments)]
pub(super) fn select_partitions(
    stripes: Vec<StripeCoefs>,
    info: &ImageInfo,
    cfg: &LossyConfig,
    effort: crate::effort::Effort,
    plan: crate::effort::LossyEffortPlan,
    g: &Geom,
    pw: usize,
    xyb: bool,
    qp: &QParams,
    steps: &[[f32; 64]; 3],
    round: f32,
    layouts: [LossyChannelLayout; 3],
    skip_on: bool,
    has_exact: bool,
    trellis: Option<(trellis::LossyTrellis, bool)>,
    trellis_aq: &[f32],
    rows: RowPlanes<'_>,
    stop: &dyn Stop,
) -> Result<(Vec<StripeCoefs>, bool, Vec<f32>), Error> {
    let default_layouts = layouts == [LossyChannelLayout::default(); 3];
    let entropy_allows = cfg.decode_speed.is_some()
        || matches!(cfg.entropy, LossyEntropy::RunLevel | LossyEntropy::SplitHuf);
    let base_eligible = !skip_on && default_layouts && entropy_allows;
    #[cfg(feature = "lab")]
    let base_eligible = base_eligible && cfg.rdoq_lambda == 0.0 && cfg.lf_pred == 0;

    let sb_eligible = (trellis.is_none() || adaptive_enabled(cfg)) && base_eligible;
    let sb_on = superblocks_on(cfg, info, sb_eligible);
    let small_on = small_on(cfg, info, effort, base_eligible);
    let trellis_active = trellis.as_ref().is_some_and(|&(_, forced)| forced || xyb);

    let open_lambda_map: Vec<f32> =
        if plan.lambda_aq_open != 0.0 && (sb_on || small_on || trellis_active) {
            adaptive_rdo::build_open_loop_lambda_map(
                rows,
                pw,
                g.bw,
                g.bh,
                steps,
                plan.lambda_aq_open,
                cfg.threads,
                stop,
            )?
        } else {
            Vec::new()
        };

    if !(sb_on || small_on) {
        return Ok((stripes, sb_on, open_lambda_map));
    }

    let mut stripes = stripes;
    if has_exact {
        exact_aq_to_selector(&mut stripes);
    }
    let sel_opts = build_selector_opts(cfg, plan, sb_on, small_on, trellis_active);
    let gbw = lossy_group_cols(cfg, g.w) / 8;

    let mut stripes = if plan.two_pass_pilot {
        run_two_pass_pilot(
            stripes,
            cfg,
            effort,
            plan,
            g,
            pw,
            gbw,
            xyb,
            qp,
            steps,
            round,
            small_on,
            trellis,
            trellis_aq,
            &open_lambda_map,
            sel_opts,
            rows,
            stop,
        )?
    } else {
        superblock::select::apply_with_lambda(
            stripes,
            rows,
            pw,
            gbw,
            steps,
            round,
            cfg,
            effort,
            &open_lambda_map,
            sel_opts,
            stop,
        )?
    };

    if has_exact {
        exact_aq_to_wire(&mut stripes);
    }
    Ok((stripes, sb_on, open_lambda_map))
}

/// Two-pass empirical pilot without cloning the whole image's `StripeCoefs`:
/// runs pilot selection one stripe at a time per worker to gather fixed-size
/// empirical map/flag/token histograms (~25 KiB), then applies final selection
/// directly in place on `stripes`.
#[allow(clippy::too_many_arguments)]
fn run_two_pass_pilot(
    stripes: Vec<StripeCoefs>,
    cfg: &LossyConfig,
    effort: crate::effort::Effort,
    plan: crate::effort::LossyEffortPlan,
    g: &Geom,
    pw: usize,
    gbw: usize,
    xyb: bool,
    qp: &QParams,
    steps: &[[f32; 64]; 3],
    round: f32,
    small_on: bool,
    trellis: Option<(trellis::LossyTrellis, bool)>,
    trellis_aq: &[f32],
    open_lambda_map: &[f32],
    sel_opts: superblock::select::Opts,
    rows: RowPlanes<'_>,
    stop: &dyn Stop,
) -> Result<Vec<StripeCoefs>, Error> {
    let split = sel_opts.split_huf;
    let mut pilot_opts = sel_opts;
    pilot_opts.rdo.trellis_select = false;
    let pilot_coder = superblock::select::Coder::of_stripes(
        &stripes,
        gbw,
        cfg,
        superblock::select::price_effort(effort, pilot_opts),
        split,
    );
    let pilot_sel = superblock::select::Selector::new_learned(
        pilot_coder,
        pw,
        gbw,
        steps,
        round,
        pilot_opts,
        None,
        None,
    );
    let pilot_huf_model = split && trellis::huf_rate(cfg);
    let want_pilot_huf = if sel_opts.trellis {
        pilot_huf_model
    } else {
        split
    };

    struct PilotA {
        map: adaptive_rdo::MapBitStats,
        flags: adaptive_rdo::FlagStats,
        hist: TokenHistogram,
    }
    let parts_a = crate::par::par_fold(
        &stripes,
        cfg.threads,
        || {
            Ok::<PilotA, Error>(PilotA {
                map: adaptive_rdo::MapBitStats::default(),
                flags: adaptive_rdo::FlagStats::default(),
                hist: TokenHistogram::new(want_pilot_huf),
            })
        },
        |acc, sc| {
            let Ok(st) = acc else { return };
            let selected =
                match pilot_sel.stripe_with_lambda(sc.clone(), rows, open_lambda_map, 0, stop) {
                    Ok(s) => s,
                    Err(e) => {
                        *acc = Err(e);
                        return;
                    }
                };
            if let Err(e) = st.map.add_stripe(&selected, gbw) {
                *acc = Err(e);
                return;
            }
            if small_on {
                st.flags.add_stripe(&selected);
            }
            st.hist.add_owned_stripe(selected, gbw);
        },
    );

    let mut map_stats = adaptive_rdo::MapBitStats::default();
    let mut flag_stats = adaptive_rdo::FlagStats::default();
    let mut pilot_hist = TokenHistogram::new(want_pilot_huf);
    for p in parts_a {
        let p = p?;
        map_stats.merge(&p.map);
        flag_stats.merge(&p.flags);
        pilot_hist.merge(p.hist);
    }
    let learned_map = map_stats.into_table();
    let learned_flags = small_on.then(|| flag_stats.into_flags());

    let learned_coder = if let Some((mut t_set, forced)) = trellis
        && (forced || xyb)
    {
        t_set.passes = 1;
        let pilot_bias = qp.bias.filter(|b| !b.is_zero()).map(|b| b.tables());
        let pilot_pass = pilot_hist.trellis_pass(&t_set, cfg, effort, pilot_huf_model);
        let parts_b = crate::par::par_fold(
            &stripes,
            cfg.threads,
            || Ok::<TokenHistogram, Error>(TokenHistogram::new(split)),
            |acc, sc| {
                let Ok(hist) = acc else { return };
                let selected = match pilot_sel.stripe_with_lambda(
                    sc.clone(),
                    rows,
                    open_lambda_map,
                    0,
                    stop,
                ) {
                    Ok(s) => s,
                    Err(e) => {
                        *acc = Err(e);
                        return;
                    }
                };
                let mut units = if gbw == 0 {
                    alloc::vec![selected]
                } else {
                    split_units(selected, gbw)
                };
                let rewrite_args = trellis::StripeRewrite {
                    rows,
                    pw,
                    image_bw: g.bw,
                    group_bw: gbw,
                    steps,
                    cfg,
                    set: &t_set,
                    aq: trellis_aq,
                    lambda_map: open_lambda_map,
                    lambda_by0: 0,
                    merged_trellis: plan.merged_trellis,
                    bias: pilot_bias.as_ref(),
                    pass_index: 0,
                };
                if let Err(e) = trellis::rewrite_stripe_lambda(
                    &pilot_pass,
                    &mut units,
                    &rewrite_args,
                    None,
                    stop,
                ) {
                    *acc = Err(e);
                    return;
                }
                for u in &units {
                    hist.add_unit(u);
                }
            },
        );
        let mut learned_hist = TokenHistogram::new(split);
        for p in parts_b {
            learned_hist.merge(p?);
        }
        learned_hist.selector_coder(cfg, effort, split)
    } else {
        pilot_hist.selector_coder(cfg, effort, split)
    };

    superblock::select::apply_with_learned(
        stripes,
        rows,
        pw,
        gbw,
        steps,
        round,
        cfg,
        learned_coder,
        Some(learned_map),
        learned_flags,
        open_lambda_map,
        sel_opts,
        stop,
    )
}
