//! Whole-image lossy encoder pipeline: validation/setup, pass-1 color/DCT/quant,
//! exact-block selection, partition selection, AC trellis, DC/alpha nesting,
//! and V3 file assembly.

mod assemble;
mod nested;
mod pass1;
mod setup;
mod trellis_stage;

use alloc::vec::Vec;
use how_far::{Complete, PhaseSpec, ProgressExt, Pulse, Stages, Total};

use super::{
    Coded, Geom, InputKind, LossyChannelLayout, LossyColor, LossyConfig, QParams, RowPlanes,
    StripeCoefs, aq_map, aq_on_cfg, layout, local_ans, lossy_group_cols, partition, trellis,
};
#[cfg(feature = "lab")]
use super::{ColorTransform, MATRIX_EXPLICIT, MATRIX_LAST_PRESET, envelope, lab_dc_step};
#[cfg(any(feature = "lab", feature = "native-jpeg"))]
use super::{NCH, coef};
#[cfg(feature = "lab")]
use crate::{Channels, SampleType};
use crate::{Error, ImageInfo, Metadata};

/// Test helper: the lossy coder without progress.
#[cfg(test)]
pub(crate) fn encode_lossy(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
) -> Result<Vec<u8>, Error> {
    encode_lossy_with_progress(pixels, stride, info, meta, cfg, &how_far::NoPulse)
}

pub(crate) fn encode_lossy_with_progress(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    pulse: &dyn Pulse,
) -> Result<Vec<u8>, Error> {
    pulse.check().map_err(|_| Error::Cancelled)?;
    Stages::new(
        pulse,
        &[
            PhaseSpec::new("lossy payload", 1, Total::Unknown),
            PhaseSpec::new("unit index", 1, Total::Unknown),
            PhaseSpec::new("locator", 1, Total::Unknown),
            PhaseSpec::new("file checksum", 1, Total::Unknown),
        ],
    )
    .complete_with(|stages| {
        let out = stages
            .run(|p| encode_lossy_frame_with_progress(pixels, stride, info, meta, cfg, None, p))?;
        let out = stages.run(|p| {
            p.check().map_err(|_| Error::Cancelled)?;
            let out = crate::container::finish_unit_table(out)?;
            p.check().map_err(|_| Error::Cancelled)?;
            Ok(out)
        })?;
        let out = stages.run(|p| {
            p.check().map_err(|_| Error::Cancelled)?;
            let out = crate::container::locate(out)?;
            p.check().map_err(|_| Error::Cancelled)?;
            Ok(out)
        })?;
        if !cfg.checksum {
            stages.skip();
            pulse.check().map_err(|_| Error::Cancelled)?;
            return Ok(out);
        }
        stages.run(|p| {
            p.check().map_err(|_| Error::Cancelled)?;
            let out = crate::checksum::finish_file(out, Some(cfg.hash))?;
            p.check().map_err(|_| Error::Cancelled)?;
            Ok(out)
        })
    })
}

/// [`encode_lossy`] of an inter frame (or any nested image: never a checksum):
/// `previous` (pixels, stride; the image's size and format) is the previous
/// frame's source under the frame. Blocks whose pixels equal it are coded
/// "keep": the decoder's canvas already holds them as coded before (RGB
/// layouts; ignored otherwise).
pub(crate) fn encode_lossy_frame(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    previous: Option<(&[u8], usize)>,
) -> Result<Vec<u8>, Error> {
    encode_lossy_frame_with_progress(pixels, stride, info, meta, cfg, previous, &how_far::NoPulse)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn encode_lossy_frame_with_progress(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    previous: Option<(&[u8], usize)>,
    pulse: &dyn Pulse,
) -> Result<Vec<u8>, Error> {
    pulse.check().map_err(|_| Error::Cancelled)?;
    let (cfg, mut setup, previous) = setup::prepare_frame(pixels, stride, info, cfg, previous)?;
    let cfg: &LossyConfig = &cfg;
    let (trellis_aq, field_codes) = {
        let filler = setup::RowFiller::new(pixels, stride, &setup);
        setup::prepare_lab_aq(cfg, &setup, &filler)?
    };
    let map = if field_codes.is_empty() {
        aq_map(cfg)
    } else {
        Some(&field_codes[..])
    };
    Stages::new(
        pulse,
        &[
            PhaseSpec::new("transform rows", 1, Total::Unknown).units("block rows"),
            PhaseSpec::new("block refinement", 1, Total::Unknown),
            PhaseSpec::new("coefficient coding", 1, Total::Unknown).units("encoded units"),
        ],
    )
    .complete_with(|stages| {
        let stripe_parts = stages.run(|pulse| {
            let filler = setup::RowFiller::new(pixels, stride, &setup);
            pass1::quantize_pass1_stripes(
                pixels, stride, previous, cfg, &setup, map, &filler, pulse,
            )
        })?;
        // Reborrow `setup` mutably inside stage 2 for optional lab centroid
        // reconstruction bias; `filler` is not used while `setup.qp.bias` is updated.
        let (stripes, exact, sb_on, open_lambda_map) = stages.run(|pulse| {
            pulse.check().map_err(|_| Error::Cancelled)?;
            let (stripes, bias_stats): (Vec<StripeCoefs>, Vec<super::BiasSums>) =
                stripe_parts.into_iter().unzip();
            let stripes = pass1::apply_lab_post_pass1(stripes, bias_stats, cfg, &mut setup);
            let (stripes, exact) =
                pass1::select_exact_blocks(stripes, pixels, stride, info, cfg, &setup)?;
            pulse.check().map_err(|_| Error::Cancelled)?;
            let filler = setup::RowFiller::new(pixels, stride, &setup);
            let fill_row = |by: usize, planes: &mut [f32], extremes: &mut Vec<(u64, bool)>| {
                filler.fill_block_row(by, planes, extremes);
            };
            let (stripes, sb_on, open_lambda_map) = partition::select_partitions(
                stripes,
                info,
                cfg,
                setup.effort,
                setup.plan,
                &setup.g,
                setup.pw,
                setup.xyb,
                &setup.qp,
                &setup.steps,
                setup.round,
                setup.layouts,
                setup.skip_on,
                exact.is_some(),
                setup.trellis,
                &trellis_aq,
                &fill_row,
                pulse,
            )?;
            pulse.check().map_err(|_| Error::Cancelled)?;
            Ok((stripes, exact, sb_on, open_lambda_map))
        })?;
        let filler = setup::RowFiller::new(pixels, stride, &setup);
        let fill_block_row = |by: usize, planes: &mut [f32], extremes: &mut Vec<(u64, bool)>| {
            filler.fill_block_row(by, planes, extremes);
        };
        stages.run(|pulse| {
            finish_lossy_frame(
                info,
                meta,
                cfg,
                setup.effort,
                Some((pixels, stride)),
                Some(&fill_block_row),
                &setup.g,
                setup.sb,
                setup.color,
                setup.kind,
                setup.scale,
                setup.qp,
                setup.steps,
                setup.layouts,
                setup.skip_on,
                exact,
                sb_on,
                setup.trellis,
                &trellis_aq,
                &open_lambda_map,
                setup.dc_fix.then_some(setup.white_code.is_some()),
                stripes,
                pulse,
            )
        })
    })
}

/// Everything after the stripes' quantized coefficients exist: the nested
/// DC image, the per-unit AC coding, the seed-table choice and the file
/// assembly. Shared by the pixel path (`encode_lossy_frame`, which passes
/// `pixels` and `rows`) and the lab coefficient path (`encode_lossy_coefs`,
/// both `None`): `rows` recomputes block-row planes for the stages that
/// still need pixels (the AC trellis, lab RDOQ), all of which a
/// coefficient-path `LossyConfig` keeps off.
#[allow(clippy::too_many_arguments)]
pub(in crate::lossy) fn finish_lossy_frame(
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    effort: crate::effort::Effort,
    pixels: Option<(&[u8], usize)>,
    rows: Option<RowPlanes<'_>>,
    g: &Geom,
    sb: u32,
    color: LossyColor,
    kind: InputKind,
    scale: [f32; 3],
    qp: QParams,
    steps: [[f32; 64]; 3],
    layouts: [LossyChannelLayout; 3],
    skip_on: bool,
    exact: Option<Coded>,
    sb_on: bool,
    trellis: Option<(trellis::LossyTrellis, bool)>,
    trellis_aq: &[f32],
    open_lambda_map: &[f32],
    rdoq_dc: Option<bool>,
    stripes: Vec<StripeCoefs>,
    pulse: &dyn Pulse,
) -> Result<Vec<u8>, Error> {
    pulse.check().map_err(|_| Error::Cancelled)?;
    local_ans::validate(cfg)?;
    #[cfg(feature = "lab")]
    crate::seeds::validate_id(cfg.seed_bank)?;
    let stop = pulse.live();
    let nested = how_far::WithStop::borrowed(&how_far::NoPulse, &stop);
    let xyb = color == LossyColor::Xyb;
    let trellis_bias = qp.bias.filter(|b| !b.is_zero()).map(|b| b.tables());
    let adaptive_mask = layout::adaptive_mask(&layouts);
    let aq_on = aq_on_cfg(cfg);
    // Column groups: the DC image's groups cover the same blocks.
    let gcols = lossy_group_cols(cfg, g.w);
    let ng = if gcols == 0 { 1 } else { g.w.div_ceil(gcols) };

    let dc = nested::encode_dc_image(
        &stripes,
        info,
        cfg,
        g,
        sb,
        gcols,
        ng,
        skip_on,
        exact.is_some(),
        &nested,
        pulse,
    )?;
    // Features the nested heads use (a 32-bit DC image's offset maps): the
    // file declares them (SPEC 2.8).
    let mut nested_features = crate::v1::head_features(&dc.dc_info, &dc.head);
    if let Some(e) = &dc.explicit {
        nested_features = nested_features.union(crate::v1::head_features(&dc.dc_info, &e.0));
    }

    let alpha = nested::encode_alpha_image(info, cfg, pixels, g, sb, gcols, &nested, pulse)?;
    if let Some((ai, ah, _, ae)) = &alpha {
        nested_features = nested_features.union(crate::v1::head_features(ai, ah));
        if let Some(e) = ae {
            nested_features = nested_features.union(crate::v1::head_features(ai, &e.0));
        }
    }

    pulse.check().map_err(|_| Error::Cancelled)?;
    let resolved = trellis_stage::resolve_trellis_mode(&stripes, cfg, g, xyb, trellis);

    // A small automatic-tier encode can compare two different reconstructions.
    // For a lab request with a different bank, preserve the mode the normal
    // complete-file comparison would select, including its explicit fallback.
    #[cfg(feature = "lab")]
    let reference_entropy =
        if resolved.separate_tier_coefficients && cfg.seed_bank != crate::seeds::DEFAULT_BANK {
            let mut reference = cfg.clone();
            reference.seed_bank = crate::seeds::DEFAULT_BANK;
            let file = finish_lossy_frame(
                info,
                meta,
                &reference,
                effort,
                pixels,
                rows,
                g,
                sb,
                color,
                kind,
                scale,
                qp,
                steps,
                layouts,
                skip_on,
                exact.clone(),
                sb_on,
                resolved.trellis.map(|t| (t, false)),
                trellis_aq,
                open_lambda_map,
                rdoq_dc,
                stripes.clone(),
                &nested,
            )?;
            Some(crate::seeds::lossy_entropy_seed(&file)?.0)
        } else {
            None
        };

    let (stripes, pass1) = trellis_stage::run_trellis_and_rdoq(
        stripes,
        info,
        cfg,
        effort,
        rows,
        g,
        gcols,
        ng,
        color,
        &steps,
        exact.is_some(),
        &resolved,
        trellis_aq,
        open_lambda_map,
        trellis_bias.as_ref(),
        rdoq_dc,
        pulse,
    )?;
    pulse.check().map_err(|_| Error::Cancelled)?;

    let (codings, pick, pick_explicit) = trellis_stage::select_ac_coding(
        &stripes,
        pass1.as_deref(),
        cfg,
        effort,
        g,
        &resolved,
        #[cfg(feature = "lab")]
        reference_entropy,
        pulse,
    )?;

    let pmaps =
        assemble::build_partition_maps(&stripes, cfg, layouts, sb_on, exact.is_some(), skip_on)?;
    // When small-image tier selection ran a separate split-Huffman trellis on
    // `pass1`, build its own partition maps from `pass1`'s AC support mask so
    // joint 4x4 split flags stay synchronized with the decoded AC coefficients.
    let alt_pmaps = pass1
        .as_deref()
        .map(|p| assemble::build_partition_maps(p, cfg, layouts, sb_on, exact.is_some(), skip_on))
        .transpose()?;
    let ctx = assemble::AssemblyCtx {
        info,
        meta,
        cfg,
        sb,
        gcols,
        ng,
        color,
        kind,
        scale,
        qp,
        layouts,
        adaptive_mask,
        aq_on,
        skip_on,
        fits16: dc.fits16,
        dc_rows: dc.dc_rows,
        coverage_head: dc.coverage_head.as_deref(),
        coverage: dc.coverage.as_ref(),
        exact: exact.as_ref(),
        stripes: &stripes,
        pmaps: &pmaps,
        alt_stripes: pass1.as_deref(),
        alt_pmaps: alt_pmaps.as_ref(),
        nested_features,
    };
    assemble::select_seeded_or_explicit(
        &ctx,
        &codings,
        pick,
        pick_explicit,
        &dc,
        alpha.as_ref(),
        pulse,
    )
}

/// LAB: assemble a lossy file from already-quantized coefficients — the
/// encoder's back half ([`finish_lossy_frame`]) with coefficients in place
/// of its forward DCT and quantizer output, for coefficient-import studies
/// (`imij::lab::encode_lossy_quantized`).
///
/// `q` covers the image's whole `ceil(w/8) x ceil(h/8)` block grid in
/// [`StripeCoefs`] layout: `q[(block * 3 + ch) * 64 + raster]` holds the
/// quantized coefficients with the quantized DC at index 0 (the nested DC
/// image is rebuilt from it). Each (block, channel)'s zigzag nonzero mask
/// is derived from `q` itself.
///
/// Every coefficient-reading or coefficient-modifying stage must be off or
/// it would silently not run; this is checked, not assumed: no AQ
/// (`aq_gamma`, `aq_map`, `aq_field`), subblock layouts, lab trellis
/// (`trellis` passes), RDOQ, LF prediction, exact blocks, forced
/// superblocks or a decoder-memory plan. The effort's trellis is never run
/// (it would re-decide AC); `effort` still drives the entropy-table
/// search. `color` must be YCoCg: the coefficients are code-value domain,
/// which is also what a signalled `lab.color_transform` is defined on.
#[cfg(feature = "lab")]
pub(crate) fn encode_lossy_coefs(
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    q: &[i32],
) -> Result<Vec<u8>, Error> {
    let pinned_lab;
    let cfg = {
        let mut c = cfg.clone();
        c.lab = c.lab.pinned();
        pinned_lab = c;
        &pinned_lab
    };
    info.validate_precision()?;
    if info.sample != SampleType::U8
        || !matches!(info.channels, Channels::Rgb | Channels::Gray)
        || info.has_custom_precision()
    {
        return Err(Error::Unsupported(
            "coefficient input supports U8 RGB or Gray image infos",
        ));
    }
    let color = LossyColor::YCoCg;
    let effort = crate::effort::Effort::new(cfg.effort)?;
    // The coefficient path has no pixels: every stage that recomputes or
    // edits coefficients must be off, or the file would not hold `q`.
    if aq_on_cfg(cfg)
        || cfg.channel_layouts != [LossyChannelLayout::default(); 3]
        || cfg.exact_blocks.is_some_and(|k| k > 0.0)
        || cfg.decoder_memory_limit.is_some()
        || cfg.lf_pred != 0
        || cfg.rdoq_lambda != 0.0
        || cfg.trellis.is_some_and(|t| t.passes > 0)
        || cfg.lab.superblocks == Some(true)
        || cfg.lab.recon_bias_centroid.is_some()
    {
        return Err(Error::Unsupported(
            "coefficient input needs AQ, subblock layouts, trellis, RDOQ, LF prediction, exact blocks, superblocks, centroid bias and decoder-memory plans off",
        ));
    }
    if info.width == 0 || info.height == 0 || info.checked_buffer_len().is_none() {
        return Err(Error::InvalidInput("coefficient image geometry"));
    }
    let count = (info.width as usize)
        .div_ceil(8)
        .checked_mul((info.height as usize).div_ceil(8))
        .and_then(|n| n.checked_mul(NCH * 64))
        .ok_or(Error::InvalidInput("coefficient grid is not representable"))?;
    if q.len() != count {
        return Err(Error::InvalidInput(
            "coefficient planes must cover the image's block grid",
        ));
    }
    if q.as_chunks::<64>()
        .0
        .iter()
        .any(|block| block[1..].iter().any(|v| v.unsigned_abs() >= 1 << 22))
    {
        return Err(Error::InvalidInput(
            "AC coefficient magnitude must be below 2^22",
        ));
    }
    let sb = cfg.stripe_rows_for(info.width);
    let g = Geom::new(info.width, info.height, sb);
    let quantizer_scale = cfg.scale_for(color, info.width, info.height);
    if !quantizer_scale.is_finite() {
        return Err(Error::InvalidInput("invalid lossy numeric parameter"));
    }
    let matrix = match cfg.matrix {
        m @ 0..=MATRIX_LAST_PRESET => m,
        MATRIX_EXPLICIT => MATRIX_EXPLICIT,
        _ => 0,
    };
    let mut qp = QParams::from_quality(quantizer_scale, color, matrix);
    if let Some(m) = cfg.channel_mul {
        qp.mul = m;
    }
    if let Some(t) = &cfg.lab.explicit_matrix {
        qp.explicit = Some(envelope::round_trip_explicit(**t)?);
        qp.matrix = MATRIX_EXPLICIT;
    }
    qp.bias = cfg.lab.recon_bias.filter(|b| !b.is_zero());
    qp.transform = cfg.lab.color_transform;
    if let Some(t) = qp.transform
        && let ColorTransform::Explicit(m) = t
        && (!ColorTransform::explicit_valid(&m) || t.forward().is_none())
    {
        return Err(Error::InvalidInput("invalid explicit color transform"));
    }
    for (i, v) in lab_dc_step(cfg).into_iter().enumerate() {
        if v != 0 {
            for c in if i == 0 { 0..1 } else { 1..3 } {
                qp.dc[c] = v;
            }
        }
    }
    qp.apply_ac_scale(cfg.ac_scale)?;
    if !qp.valid() {
        return Err(Error::InvalidInput("invalid quantization parameters"));
    }
    finish_quantized_coefficients(info, meta, cfg, effort, &g, sb, qp, q)
}

/// Entropy/DC assembly shared by native coefficient import and the study
/// hook. It never transforms pixels or changes the supplied coefficients.
#[cfg(any(feature = "lab", feature = "native-jpeg"))]
pub(in crate::lossy) fn finish_quantized_coefficients(
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    effort: crate::effort::Effort,
    g: &Geom,
    sb: u32,
    qp: QParams,
    q: &[i32],
) -> Result<Vec<u8>, Error> {
    let color = LossyColor::YCoCg;
    let kind = InputKind::U8;
    let steps = qp.steps();
    // The zigzag nonzero masks the entropy coders emit, from `q` (bit k:
    // zigzag coefficient k nonzero — `quant_row`'s contract).
    let nz: Vec<u64> = q
        .as_chunks::<64>()
        .0
        .iter()
        .map(|b| {
            coef::ZIGZAG
                .iter()
                .enumerate()
                .fold(0u64, |m, (k, &i)| m | u64::from(b[i as usize] != 0) << k)
        })
        .collect();
    let stripes: Vec<StripeCoefs> = (0..g.stripes())
        .map(|s| {
            let (by0, by1) = g.rows(s);
            StripeCoefs {
                by0,
                nby: by1 - by0,
                bw: g.bw,
                q: q[by0 * g.bw * NCH * 64..by1 * g.bw * NCH * 64].to_vec(),
                nz: nz[by0 * g.bw * NCH..by1 * g.bw * NCH].to_vec(),
                aq: Vec::new(),
                modes: Vec::new(),
                splits: Vec::new(),
            }
        })
        .collect();
    finish_lossy_frame(
        info,
        meta,
        cfg,
        effort,
        None,
        None,
        g,
        sb,
        color,
        kind,
        color.default_scales(),
        qp,
        steps,
        cfg.channel_layouts,
        false,
        None,
        false,
        None,
        &[],
        &[],
        None,
        stripes,
        &how_far::NoPulse,
    )
}
