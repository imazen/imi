//! Whole-image lossy encoder pipeline: color/dering/DCT pass, exact-block
//! selection, partition selection, AC trellis, DC/alpha nesting, and file
//! assembly.

use alloc::vec;
use alloc::vec::Vec;
use how_far::{Complete, PhaseSpec, ProgressExt, Pulse, Report, Stages, Total};

#[cfg(all(feature = "lab", test))]
use super::model_candidates;
use super::{
    BiasSums, Coded, ColorTransform, Geom, H_AQ, H_BIAS, H_DC32, H_EXACT, H_EXPLICIT, H_KEEP,
    H_LAYOUTS, H_TRANSFORM, InputKind, LossyChannelLayout, LossyColor, LossyConfig, LossyEntropy,
    MATRIX_EXPLICIT, MATRIX_LAST_PRESET, MapKind, NCH, QParams, RowPlanes, SMALL_IMAGE,
    StripeCoefs, adaptive_mode, aq_map, aq_on_cfg, block_extremes, coding_defaults, color,
    color_index, compact, dc_coverage, dc_lanes, dc_nearest, dc_row, dc_stripe_rows, dct, dering,
    dering_row, enc_simd, encode_ans_stripes, encode_huffman_stripes, envelope, exact,
    exact_aq_to_selector, exact_aq_to_wire, fill_color_row, finite, huf_max_ratio, layout,
    local_ans, lossy_group_cols, model_refs, partition, precision, predicted_huf_ratio, seed_bank,
    seed_exact, side_stats, side_stats_lanes, small, split_units, subblock, superblock, transfer,
    trellis,
};
#[cfg(feature = "lab")]
use super::{aq_field_lab, coef, lab_dc_step, lf_pred_lab, rdoq_lab};
#[cfg(all(feature = "lab", feature = "std"))]
use super::{block_lab, prof};
#[cfg(all(feature = "lab", test))]
use crate::ByteReader;
use crate::rans::NL;
use crate::{Channels, Error, ImageInfo, Metadata, SampleType, put_varint};

/// Test helper: the lossy coder without progress.
#[cfg(all(test, feature = "encode"))]
#[cfg(feature = "encode")]
pub(crate) fn encode_lossy(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
) -> Result<Vec<u8>, Error> {
    encode_lossy_with_progress(pixels, stride, info, meta, cfg, &how_far::NoPulse)
}

#[cfg(feature = "encode")]
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

/// [`encode_lossy`] of an inter frame (or any nested image: never a checksum): `previous` (pixels, stride; the
/// image's size and format) is the previous frame's source under the frame.
/// Blocks whose pixels equal it are coded "keep": the decoder's canvas
/// already holds them as coded before (RGB layouts; ignored otherwise).
#[cfg(feature = "encode")]
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
#[cfg(feature = "encode")]
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
    if info.channels.is_native_model() {
        return Err(Error::Unsupported("pixel model requires lossless coding"));
    }
    // LAB: this encode's switches, fixed now for every thread it uses.
    #[cfg(feature = "lab")]
    let pinned_lab;
    #[cfg(feature = "lab")]
    let cfg = {
        let mut c = cfg.clone();
        c.lab = c.lab.pinned();
        pinned_lab = c;
        &pinned_lab
    };
    local_ans::validate(cfg)?;
    #[cfg(feature = "lab")]
    if cfg.coding_defaults && cfg.lab.small.as_ref().is_some_and(|s| s.int) {
        return Err(Error::InvalidInput(
            "coding defaults use native float transforms",
        ));
    }
    #[cfg(feature = "lab")]
    crate::seeds::validate_id(cfg.seed_bank)?;
    info.validate_precision()?;
    let effort = crate::effort::Effort::new(cfg.effort)?;
    let planned;
    let cfg = if cfg.decoder_memory_limit.is_some() {
        planned = crate::decoder_memory::plan_lossy(info, cfg)?.0;
        &planned
    } else {
        cfg
    };
    if !cfg.quality.is_finite()
        || !cfg.round.is_finite()
        || !cfg.aq_gamma.is_finite()
        || !cfg.aq_r0.is_finite()
        || (cfg.aq_gamma != 0.0 && cfg.aq_r0 <= 0.0)
    {
        return Err(Error::InvalidInput("invalid lossy numeric parameter"));
    }
    #[cfg(feature = "lab")]
    {
        if !cfg.rdoq_lambda.is_finite() || !(0.0..=4.0).contains(&cfg.rdoq_lambda) {
            return Err(Error::InvalidInput("invalid lab RDOQ lambda"));
        }
        if cfg.rdoq_policy > 4 {
            return Err(Error::InvalidInput("invalid lab RDOQ policy"));
        }
        if cfg.rdoq_policy != 0
            && cfg.rdoq_lambda != 0.0
            && (info.channels != Channels::Rgb
                || info.cicp != crate::Cicp::SRGB
                || cfg.color != LossyColor::Xyb)
        {
            return Err(Error::Unsupported(
                "bounded lab RDOQ requires sRGB RGB8 XYB",
            ));
        }
        if cfg.rdoq_lambda != 0.0
            && (cfg.entropy != LossyEntropy::RunLevel
                || cfg.decode_speed.is_some()
                || aq_on_cfg(cfg)
                || cfg.lf_pred != 0
                || previous.is_some()
                || info.sample != SampleType::U8
                || info.has_custom_precision())
        {
            return Err(Error::Unsupported(
                "lab RDOQ requires run-level U8 stills without AQ, tiers or LF prediction",
            ));
        }
        if let Some(t) = &cfg.trellis
            && !t.valid()
        {
            return Err(Error::InvalidInput("invalid lab trellis settings"));
        }
        if cfg.lab.recon_bias.is_some() && cfg.lab.recon_bias_centroid.is_some() {
            return Err(Error::InvalidInput(
                "fixed and centroid reconstruction bias exclude each other",
            ));
        }
        if let Some(k) = cfg.lab.recon_bias_centroid
            && !(k == 64 || (2..=63).contains(&k))
        {
            return Err(Error::InvalidInput("invalid reconstruction bias band"));
        }
    }
    // Centralized lossy effort policy (effort.rs).
    let plan = effort.lossy_plan(adaptive_mode(cfg));
    // The AC trellis: (settings, set by the lab override). The effort's
    // trellis is narrowed to run-level XYB coding after pass 1. Subblock
    // layouts and the lab RDOQ / LF prediction modes encode without it.
    let trellis = {
        #[cfg(feature = "lab")]
        let set = match cfg.trellis {
            // min_quality is compared with the resolved quantizer scale below
            // (a quality target leaves `cfg.quality` as it was).
            Some(t) => (t.passes > 0).then_some((t, true)),
            None => trellis::for_effort_plan(plan).map(|t| (t, false)),
        };
        #[cfg(not(feature = "lab"))]
        let set = trellis::for_effort_plan(plan).map(|t| (t, false));
        #[cfg(feature = "lab")]
        let set = set.filter(|_| cfg.rdoq_lambda == 0.0 && cfg.lf_pred == 0);
        set.filter(|_| cfg.channel_layouts == [LossyChannelLayout::default(); 3])
    };
    if cfg.small_transforms
        && (cfg.decode_speed != Some(crate::DecodeSpeed::Compact)
            || info.sample != SampleType::U8
            || previous.is_some()
            || cfg.channel_layouts != [LossyChannelLayout::default(); 3]
            || cfg.effort >= 9.0
            || trellis.is_some())
    {
        return Err(Error::Unsupported(
            "small transforms require Compact U8 stills with default layouts, effort below 9 and no trellis",
        ));
    }
    #[cfg(feature = "lab")]
    if cfg.small_transforms && (cfg.rdoq_lambda != 0.0 || cfg.lf_pred != 0) {
        return Err(Error::Unsupported(
            "production small transforms exclude lab RDOQ and LF prediction",
        ));
    }
    let layouts = cfg.channel_layouts;
    let adaptive_mask = layout::adaptive_mask(&layouts);
    #[cfg(feature = "lab")]
    if layouts != [LossyChannelLayout::default(); 3] && (cfg.lf_pred != 0 || cfg.rdoq_lambda != 0.0)
    {
        return Err(Error::Unsupported(
            "subblocks with lab LF prediction or RDOQ",
        ));
    }
    #[cfg(feature = "lab")]
    if cfg.lab.recon_bias_centroid.is_some() && layouts != [LossyChannelLayout::default(); 3] {
        return Err(Error::Unsupported(
            "the centroid reconstruction bias needs full 8x8 layouts",
        ));
    }
    let previous = previous.filter(|_| info.channels == Channels::Rgb);
    let skip_on = previous.is_some();
    let kind = InputKind::of(info).ok_or(Error::Unsupported(
        "lossy mode supports u8, u16, u32, f16 or f32 samples",
    ))?;
    if kind == InputKind::U32 && !cfg!(feature = "extended") {
        return Err(Error::Unsupported("32-bit samples (extended tier)"));
    }
    let nci = info.channels.count();
    let bps = info.sample.bytes();
    let ci = color_index(info.channels);
    // 16-bit input is coded as YCoCg on the code values (any transfer:
    // sRGB, PQ, HLG), in units of 1/257 so quality maps like 8-bit. Float
    // input is linear light and coded as XYB.
    // 16-bit XYB needs a known transfer (sRGB, PQ, HLG) to reach linear light.
    let xfer16 = if kind == InputKind::U16 && cfg.color == LossyColor::Xyb {
        transfer::Transfer::from_cicp(info.cicp.transfer_characteristics)
    } else {
        None
    };
    if precision::custom_color(info)
        && cfg.color == LossyColor::Xyb
        && transfer::Transfer::from_cicp(info.cicp.transfer_characteristics).is_none()
    {
        return Err(Error::Unsupported(
            "unknown transfer for significant-bit XYB",
        ));
    }
    let color = match kind {
        InputKind::U8 => cfg.color,
        InputKind::U32 => cfg.color,
        InputKind::U16 if xfer16.is_some() => LossyColor::Xyb,
        InputKind::U16 => LossyColor::YCoCg,
        InputKind::F16 | InputKind::F32 => LossyColor::Xyb,
    };
    let wide = kind == InputKind::U16 && color == LossyColor::YCoCg;
    // Code value -> linear light for 16-bit XYB.
    let custom_color = precision::custom_color(info);
    let lut16: Vec<f32> = match xfer16.filter(|_| !custom_color) {
        Some(t) => (0..65536)
            .map(|c| t.decode(c as f64 / 65535.0) as f32)
            .collect(),
        None => Vec::new(),
    };
    if info.width == 0 || info.height == 0 {
        return Err(Error::InvalidInput("empty image"));
    }
    let extent = info
        .checked_stride_len(stride)
        .ok_or(Error::InvalidInput("invalid image layout"))?;
    if pixels.len() < extent {
        return Err(Error::InvalidInput("pixel buffer too small"));
    }
    precision::validate_pixels(info, pixels, stride)?;
    let input_precision = if custom_color || kind == InputKind::U32 {
        Some(precision::Input::new(info, color)?)
    } else {
        None
    };
    let rb = info.row_bytes();
    let sb = cfg.stripe_rows_for(info.width);
    let g = Geom::new(info.width, info.height, sb);
    let map = aq_map(cfg);
    if let Some(m) = map
        && (cfg.aq_gamma != 0.0 || m.len() != g.bw * g.bh || m.contains(&0))
    {
        return Err(Error::InvalidInput(
            "AQ map needs one nonzero code per block and no aq_gamma",
        ));
    }
    #[cfg(feature = "lab")]
    if let Some(f) = cfg.aq_field
        && (cfg.aq_gamma != 0.0
            || map.is_some()
            || !(f.strength.is_finite() && (0.0..=4.0).contains(&f.strength))
            || f.levels > 16
            || !(1..=8).contains(&f.cell))
    {
        return Err(Error::InvalidInput("invalid lab AQ field settings"));
    }
    let quantizer_scale = cfg.scale_for(color, info.width, info.height);
    if !quantizer_scale.is_finite() {
        return Err(Error::InvalidInput("invalid lossy numeric parameter"));
    }
    // jpegli's XYB shapes beat Annex K on SSIMULACRA2, zensim and both
    // butteraugli norms (benchmarks/lossy_matrix_2026-10-02.txt). The
    // automatic choice is matrix 5 for YCoCg (maintainer decision
    // 2026-10-10: jpegli's distance-dependent tables) and matrix 2 for XYB;
    // the Lossy 0.4 presets 3/4/5 and the explicit id 63 are selectable
    // (lab builds add the tablemath study's probes 6-9, which did not
    // qualify and are never automatic).
    let matrix = match cfg.matrix {
        m @ 0..=MATRIX_LAST_PRESET => m,
        MATRIX_EXPLICIT => MATRIX_EXPLICIT,
        _ if color == LossyColor::Xyb => 2,
        _ => {
            // Matrix 5's coarsest chroma steps exceed the format's 65536
            // bound at quality at or below 1.52 (scale >= 32.8) with the
            // YCoCg multiplier 2; fall back to Annex K there.
            if QParams::from_quality(quantizer_scale, color, 5).valid() {
                5
            } else {
                0
            }
        }
    };
    // Both effort-selected and explicit lab trellis gates use the resolved scale.
    let trellis = trellis.filter(|(t, _)| quantizer_scale >= t.min_quality);
    let mut qp = QParams::from_quality(quantizer_scale, color, matrix);
    if cfg.color_transform == Some(ColorTransform::Identity) {
        qp.mul = [1.0, 1.0, 1.0];
    }
    if let Some(m) = cfg.channel_mul {
        qp.mul = m;
    }
    qp.transform = cfg.color_transform;
    #[cfg(feature = "lab")]
    {
        if let Some(t) = &cfg.lab.explicit_matrix {
            qp.explicit = Some(envelope::round_trip_explicit(**t)?);
            qp.matrix = MATRIX_EXPLICIT;
        }
        qp.bias = cfg.lab.recon_bias.filter(|b| !b.is_zero());
        if qp.transform.is_none() {
            qp.transform = cfg.lab.color_transform;
        }
    }
    if let Some(t) = qp.transform {
        // The signalled transform is defined for the U8 YCoCg family;
        // XYB is its own transform and other sample types keep their
        // paths.
        if color != LossyColor::YCoCg || kind != InputKind::U8 || input_precision.is_some() {
            return Err(Error::Unsupported(
                "the signalled color transform needs U8 YCoCg input",
            ));
        }
        if let ColorTransform::Explicit(m) = t
            && (!ColorTransform::explicit_valid(&m) || t.forward().is_none())
        {
            return Err(Error::InvalidInput("invalid explicit color transform"));
        }
    }
    #[cfg(feature = "lab")]
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
    let steps = qp.steps();
    // Lossy 0.4: the encoder's forward of a signalled color transform
    // (`coded = A * rgb + B`), applied with the plane scaling by
    // `enc_simd::color_rows`.
    let transform_forward = qp.transform.and_then(|t| t.forward());
    let scale = if wide {
        [1.0 / 257.0; 3]
    } else {
        color.default_scales()
    };
    let layout_steps: [[[_; 64]; 2]; 3] = core::array::from_fn(|c| {
        [false, true].map(|split| {
            let l = layouts[c].layout(split);
            if l == subblock::Layout::FULL8 {
                steps[c]
            } else {
                l.steps(&steps[c])
            }
        })
    });
    let cm = dct::dct_matrix();
    let base_round = cfg.round.clamp(0.0, 0.5);
    let round = if cfg.freq_zero_bias {
        -base_round.max(0.40)
    } else {
        base_round
    };

    // Pass 1: transform + quantize every stripe, one block row at a time:
    // fill planar rows (linear light for XYB, code values for YCoCg), color
    // convert, then DCT + quantize.
    let inv_steps: [[f32; 64]; 3] = steps.map(|st| st.map(|v| 1.0 / v));
    let xyb = color == LossyColor::Xyb;
    let u8_lin: [f32; 256] = core::array::from_fn(|i| color::SRGB8_TO_LINEAR[i] as f32);
    let pw = g.bw * 8;
    // Overshoot deringing on the luma plane (dering.rs): integer outputs
    // only (the decoder clamps them), full 8x8 luma layout. `dering_white`
    // holds each color channel's maximum code; the cap is mozjpeg's: twice
    // the DC step (our DC is 8x the block mean, the scale of a JPEG
    // quantizer), and at most 31/255 of the plane's white level.
    let full8_luma = layouts[0] == LossyChannelLayout::default();
    let dering_on = dering::enabled(cfg) && full8_luma;
    let dc_fix = dc_nearest(cfg);
    // Maximum code per color channel, for integer inputs (the decoder
    // clamps those): white and black blocks for deringing and DC rounding.
    let white_code: Option<[u32; 3]> =
        (matches!(kind, InputKind::U8 | InputKind::U16) && full8_luma && (dering_on || dc_fix))
            .then(|| core::array::from_fn(|c| (1u32 << info.channel_bits(ci[c]).unwrap()) - 1));
    let dering_cap = 2.0 * steps[0][0];
    // LAB: jpegli's AQ strengths for the trellis's AQ coupling (U8 input).
    #[cfg(not(feature = "lab"))]
    let trellis_aq: Vec<f32> = Vec::new();
    // LAB: jpegli's field from the luma plane (U8 input), for the trellis's
    // AQ coupling strengths and the lab AQ field map.
    #[cfg(feature = "lab")]
    let luma_rows = |by: usize, out: &mut [f32]| {
        let mut planes = vec![0.0f32; NCH * 8 * pw];
        for y in 0..8 {
            let row = &pixels[(by * 8 + y).min(g.h - 1) * stride..][..rb];
            if xyb {
                fill_color_row::<1>(row, &mut planes, pw, y, g.w, nci, ci, |_, b| {
                    u8_lin[b[0] as usize]
                });
            } else {
                fill_color_row::<1>(row, &mut planes, pw, y, g.w, nci, ci, |_, b| b[0] as f32);
            }
        }
        let (p0, rest) = planes.split_at_mut(8 * pw);
        let (p1, p2) = rest.split_at_mut(8 * pw);
        match transform_forward.as_ref() {
            Some(t) => enc_simd::color_rows_transform(
                xyb,
                scale,
                &color::OPSIN_F32,
                color::OPSIN_BIAS,
                color::CBRT_BIAS,
                Some(t),
                p0,
                p1,
                p2,
            ),
            None => enc_simd::color_rows(
                xyb,
                scale,
                &color::OPSIN_F32,
                color::OPSIN_BIAS,
                color::CBRT_BIAS,
                p0,
                p1,
                p2,
            ),
        }
        out.copy_from_slice(&planes[..8 * pw]);
    };
    #[cfg(feature = "lab")]
    let coupling = matches!(&trellis, Some((t, _)) if t.aq_coupling != 0.0);
    #[cfg(feature = "lab")]
    if (coupling || cfg.aq_field.is_some()) && (kind != InputKind::U8 || input_precision.is_some())
    {
        return Err(Error::Unsupported(
            "lab trellis AQ coupling and AQ field need U8 input",
        ));
    }
    #[cfg(feature = "lab")]
    let trellis_aq: Vec<f32> = if coupling {
        trellis::aq_strengths(g.bw, g.bh, xyb, steps[0][1], cfg.threads, luma_rows)
    } else {
        Vec::new()
    };
    #[cfg(feature = "lab")]
    let field_codes: Vec<u8> = match cfg.aq_field {
        Some(f) => {
            let units = aq_field_lab::luma_units(xyb);
            let qf = aq_field_lab::field(
                g.bw,
                g.bh,
                steps[0][1] * units,
                f.fade,
                cfg.threads,
                |by, out| {
                    luma_rows(by, out);
                    for v in out.iter_mut() {
                        *v *= units;
                    }
                },
            );
            aq_field_lab::step_codes(
                &qf,
                g.bw,
                g.bh,
                f.strength,
                f.levels,
                f.cell as usize,
                f.relative,
            )
        }
        None => Vec::new(),
    };
    #[cfg(feature = "lab")]
    let map = if field_codes.is_empty() {
        map
    } else {
        Some(&field_codes[..])
    };
    // Planar color for the 8 pixel rows of block row `by` (linear light for
    // XYB, code values for YCoCg; rows past the bottom repeat the last one),
    // color converted, with overshoot deringing on the luma plane;
    // `extremes` gets the row's white/black blocks (integer inputs). Shared
    // by the quantization pass and the trellis, which recomputes rows.
    let fill_block_row = |by: usize, planes: &mut [f32], extremes: &mut Vec<(u64, bool)>| {
        for y in 0..8 {
            let sy = (by * 8 + y).min(g.h - 1);
            let row = &pixels[sy * stride..][..rb];
            match kind {
                InputKind::U8 if input_precision.is_some() => {
                    let input = input_precision.as_ref().unwrap();
                    fill_color_row::<1>(row, planes, pw, y, g.w, nci, ci, |c, b| {
                        input.read(c, b[0] as u32)
                    });
                }
                InputKind::U16 if input_precision.is_some() => {
                    let input = input_precision.as_ref().unwrap();
                    fill_color_row::<2>(row, planes, pw, y, g.w, nci, ci, |c, b| {
                        input.read(c, u16::from_ne_bytes(*b) as u32)
                    });
                }
                InputKind::U8 if xyb => {
                    fill_color_row::<1>(row, planes, pw, y, g.w, nci, ci, |_, b| {
                        u8_lin[b[0] as usize]
                    })
                }
                InputKind::U8 => {
                    fill_color_row::<1>(row, planes, pw, y, g.w, nci, ci, |_, b| b[0] as f32)
                }
                InputKind::U16 if !lut16.is_empty() => {
                    fill_color_row::<2>(row, planes, pw, y, g.w, nci, ci, |_, b| {
                        lut16[u16::from_ne_bytes(*b) as usize]
                    })
                }
                InputKind::U16 => fill_color_row::<2>(row, planes, pw, y, g.w, nci, ci, |_, b| {
                    u16::from_ne_bytes(*b) as f32
                }),
                InputKind::U32 => {
                    let input = input_precision.as_ref().unwrap();
                    fill_color_row::<4>(row, planes, pw, y, g.w, nci, ci, |c, b| {
                        input.read(c, u32::from_ne_bytes(*b))
                    })
                }
                InputKind::F16 => fill_color_row::<2>(row, planes, pw, y, g.w, nci, ci, |_, b| {
                    finite(f32::from_bits(crate::mapping::f16_to_f32_bits(
                        u16::from_ne_bytes(*b),
                    )))
                }),
                InputKind::F32 => fill_color_row::<4>(row, planes, pw, y, g.w, nci, ci, |_, b| {
                    finite(f32::from_ne_bytes(*b))
                }),
            }
        }
        let (p0, rest) = planes.split_at_mut(8 * pw);
        let (p1, p2) = rest.split_at_mut(8 * pw);
        match transform_forward.as_ref() {
            Some(t) => enc_simd::color_rows_transform(
                xyb,
                scale,
                &color::OPSIN_F32,
                color::OPSIN_BIAS,
                color::CBRT_BIAS,
                Some(t),
                p0,
                p1,
                p2,
            ),
            None => enc_simd::color_rows(
                xyb,
                scale,
                &color::OPSIN_F32,
                color::OPSIN_BIAS,
                color::CBRT_BIAS,
                p0,
                p1,
                p2,
            ),
        }
        if let Some(white_code) = white_code {
            block_extremes(
                &g,
                pixels,
                stride,
                by * 8,
                nci,
                ci,
                kind,
                &white_code,
                extremes,
            );
            if dering_on {
                dering_row(p0, pw, extremes, dering_cap);
            }
        }
    };
    // LAB: per-image centroid reconstruction bias (Lossy 0.4). The stats
    // ride along the pass-1 quantization, one accumulator per stripe.
    #[cfg(feature = "lab")]
    let centroid_k = cfg.lab.recon_bias_centroid;
    #[cfg(not(feature = "lab"))]
    let centroid_k: Option<u8> = None;
    Stages::new(
        pulse,
        &[
            PhaseSpec::new("transform rows", 1, Total::Unknown).units("block rows"),
            PhaseSpec::new("block refinement", 1, Total::Unknown),
            PhaseSpec::new("coefficient coding", 1, Total::Unknown).units("encoded units"),
        ],
    )
    .complete_with(|stages| {
        let stripe_parts: Vec<(StripeCoefs, BiasSums)> = stages.run(|pulse| {
            let pulse = pulse.live();
            use how_far::Stop;
            crate::par::par_map(
                (0..g.stripes()).collect(),
                cfg.threads,
                |_, s| -> Result<_, Error> {
                    pulse.check().map_err(|_| Error::Cancelled)?;
                    let (by0, by1) = g.rows(s);
                    let nby = by1 - by0;
                    let mut q = vec![0i32; nby * g.bw * NCH * 64];
                    let mut nz = vec![0u64; nby * g.bw * NCH];
                    let mut modes = vec![0u8; if adaptive_mask != 0 { nby * g.bw } else { 0 }];
                    let mut planes = vec![0.0f32; NCH * 8 * pw];
                    let mut extremes: Vec<(u64, bool)> = Vec::with_capacity(g.bw);
                    let mut coefs = vec![0.0f32; g.bw * 64];
                    let mut bias_stats = BiasSums::default();
                    let activity_on = cfg.aq_gamma != 0.0;
                    let aq_on = activity_on || map.is_some();
                    // Block codes (DC image channel 3): AQ multiplier a / 64, or 0 =
                    // keep the reference block (inter frames).
                    let codes_on = aq_on || skip_on;
                    let mut aq_codes = vec![64u8; if codes_on { nby * g.bw } else { 0 }];
                    let mut aq_row = vec![1.0f32; if aq_on { g.bw } else { 0 }];
                    for r in 0..nby {
                        pulse.check().map_err(|_| Error::Cancelled)?;
                        if let Some(m) = map {
                            let codes = &m[(by0 + r) * g.bw..][..g.bw];
                            aq_codes[r * g.bw..][..g.bw].copy_from_slice(codes);
                            for (s, &a) in aq_row.iter_mut().zip(codes) {
                                *s = a as f32 * (1.0 / 64.0);
                            }
                        }
                        fill_block_row(by0 + r, &mut planes, &mut extremes);
                        let qr = &mut q[r * g.bw * NCH * 64..][..g.bw * NCH * 64];
                        let nr = &mut nz[r * g.bw * NCH..][..g.bw * NCH];
                        for ch in 0..NCH {
                            if layouts[ch].layout(false) == subblock::Layout::FULL8
                                || (activity_on && ch == 0)
                            {
                                enc_simd::fdct_row(
                                    &planes[ch * 8 * pw..][..8 * pw],
                                    pw,
                                    g.bw,
                                    &cm,
                                    &mut coefs,
                                );
                            }
                            if activity_on && ch == 0 {
                                // Activity: mean |AC| of luma relative to the first AC step.
                                let r0 = cfg.aq_r0 * steps[0][1];
                                for bx in 0..g.bw {
                                    let c = &coefs[bx * 64..][..64];
                                    let act = c[1..].iter().map(|v| v.abs()).sum::<f32>() / 63.0;
                                    let m = transfer::pow(
                                        (act / r0).max(1e-6) as f64,
                                        cfg.aq_gamma as f64,
                                    )
                                    .clamp(0.5, 2.0);
                                    let a = (m * 64.0 + 0.5) as u8; // m in [0.5, 2]: round half up
                                    aq_codes[r * g.bw + bx] = a;
                                    aq_row[bx] = a as f32 * (1.0 / 64.0);
                                }
                            }
                            if layouts[ch] == LossyChannelLayout::default() {
                                enc_simd::quant_row(
                                    &coefs,
                                    g.bw,
                                    ch,
                                    &inv_steps[ch],
                                    &aq_row,
                                    round,
                                    qr,
                                    nr,
                                );
                                if let Some(k) = centroid_k {
                                    bias_stats.add_row(
                                        &coefs,
                                        qr,
                                        g.bw,
                                        ch,
                                        &inv_steps[ch],
                                        &aq_row,
                                        k,
                                    );
                                }
                                if dc_fix {
                                    let ext =
                                        (ch == 0 && white_code.is_some()).then_some(&extremes[..]);
                                    dc_row(&coefs, g.bw, ch, inv_steps[ch][0], ext, qr, nr);
                                }
                            } else {
                                let mr = if modes.is_empty() {
                                    &mut [][..]
                                } else {
                                    &mut modes[r * g.bw..(r + 1) * g.bw]
                                };
                                layout::encode_row(
                                    &planes[ch * 8 * pw..][..8 * pw],
                                    pw,
                                    g.bw,
                                    ch,
                                    layouts[ch],
                                    &layout_steps[ch],
                                    steps[ch][1],
                                    &coefs,
                                    &aq_row,
                                    round,
                                    if dc_fix { 0.5 } else { round },
                                    qr,
                                    nr,
                                    mr,
                                );
                            }
                        }
                        // Inter frames: blocks whose pixels equal the previous frame's
                        // keep the decoder's canvas (coded "keep"; the DC stays for a
                        // smooth DC image, the AC goes).
                        if let Some((pp, ps)) = previous {
                            let y0 = (by0 + r) * 8;
                            let y1 = (y0 + 8).min(g.h);
                            for bx in 0..g.bw {
                                let (x0, x1) =
                                    (bx * 8 * nci * bps, ((bx * 8 + 8).min(g.w)) * nci * bps);
                                let same = (y0..y1).all(|y| {
                                    pixels[y * stride + x0..y * stride + x1]
                                        == pp[y * ps + x0..y * ps + x1]
                                });
                                if same {
                                    for ch in 0..NCH {
                                        let o = (bx * NCH + ch) * 64;
                                        qr[o + 1..o + 64].fill(0);
                                        nr[bx * NCH + ch] = (qr[o] != 0) as u64;
                                    }
                                    aq_codes[r * g.bw + bx] = 0;
                                }
                            }
                        }
                        pulse.advance(1);
                        pulse.check().map_err(|_| Error::Cancelled)?;
                    }
                    Ok((
                        StripeCoefs {
                            by0,
                            nby,
                            bw: g.bw,
                            q,
                            nz,
                            aq: aq_codes,
                            modes,
                            splits: Vec::new(),
                        },
                        bias_stats,
                    ))
                },
            )
            .into_iter()
            .collect::<Result<Vec<_>, Error>>()
        })?;
        let (stripes, exact, sb_on, open_lambda_map) = stages.run(|pulse| {
            pulse.check().map_err(|_| Error::Cancelled)?;
            let (stripes, bias_stats): (Vec<StripeCoefs>, Vec<BiasSums>) =
                stripe_parts.into_iter().unzip();
            #[cfg(feature = "lab")]
            if let Some(k) = centroid_k {
                let b = bias_stats.iter().fold(BiasSums::default(), |mut a, s| {
                    for c in 0..3 {
                        for band in 0..2 {
                            for g in 0..2 {
                                a.sum[c][band][g] += s.sum[c][band][g];
                                a.count[c][band][g] += s.count[c][band][g];
                            }
                        }
                    }
                    a
                });
                let b = b.bias(k);
                qp.bias = (!b.is_zero()).then_some(b);
            }
            #[cfg(not(feature = "lab"))]
            {
                let _ = bias_stats;
            }

            #[cfg(feature = "lab")]
            let stripes = {
                let mut stripes = stripes;
                #[cfg(feature = "std")]
                block_lab::zero_masked_ac(&mut stripes);
                if cfg.lf_pred != 0 && !aq_on_cfg(cfg) {
                    lf_pred_lab(
                        &mut stripes,
                        &g,
                        &steps,
                        cfg.lf_pred,
                        (sb * (8 / sb).max(1)) as usize,
                    );
                }
                stripes
            };
            // Exact blocks (exact.rs): blocks whose estimated lossless bits are at
            // most the policy's ratio of their run-level AC bits (within its
            // decode-cost budget) drop their AC and are coded exactly in a nested
            // lossless image. Without a qualifying block nothing changes.
            let (stripes, exact) = {
                let mut stripes = stripes;
                let eligible =
                    !aq_on_cfg(cfg) && !skip_on && layouts == [LossyChannelLayout::default(); 3];
                let exact = match exact::policy(cfg, info, eligible) {
                    Some(pol) => {
                        let bits = exact::estimate_bits(&stripes, cfg.threads);
                        let mask = exact::select_mask(
                            pixels,
                            stride,
                            nci * bps,
                            g.w,
                            g.h,
                            &bits,
                            &pol,
                            cfg.threads,
                        );
                        if mask.contains(&true) {
                            exact::mark(&mut stripes, &mask);
                            Some(exact::encode(
                                pixels,
                                stride,
                                nci * bps,
                                &g,
                                lossy_group_cols(cfg, g.w),
                                &mask,
                                pol.stripe_rows,
                                cfg.effort,
                                cfg.threads,
                            )?)
                        } else {
                            None
                        }
                    }
                    None => None,
                };
                (stripes, exact)
            };
            pulse.check().map_err(|_| Error::Cancelled)?;
            let (stripes, sb_on, open_lambda_map) = partition::select_partitions(
                stripes,
                info,
                cfg,
                effort,
                plan,
                &g,
                pw,
                xyb,
                &qp,
                &steps,
                round,
                layouts,
                skip_on,
                exact.is_some(),
                trellis,
                &trellis_aq,
                &fill_block_row,
                pulse,
            )?;
            pulse.check().map_err(|_| Error::Cancelled)?;
            Ok((stripes, exact, sb_on, open_lambda_map))
        })?;
        stages.run(|pulse| {
            finish_lossy_frame(
                info,
                meta,
                cfg,
                effort,
                Some((pixels, stride)),
                Some(&fill_block_row),
                &g,
                sb,
                color,
                kind,
                scale,
                qp,
                steps,
                layouts,
                skip_on,
                exact,
                sb_on,
                trellis,
                &trellis_aq,
                &open_lambda_map,
                dc_fix.then_some(white_code.is_some()),
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
#[cfg(feature = "encode")]
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
    let nci = info.channels.count();
    let bps = info.sample.bytes();
    let rb = info.row_bytes();
    let pw = g.bw * 8;
    let xyb = color == LossyColor::Xyb;
    let trellis_bias = qp.bias.filter(|b| !b.is_zero()).map(|b| b.tables());
    let adaptive_mask = layout::adaptive_mask(&layouts);
    #[cfg(not(feature = "lab"))]
    let _ = rdoq_dc;
    // DC image: U16 (value + 32768) when every DC fits, else I32.
    let fits16 = stripes.iter().all(|sc| {
        sc.q.as_chunks::<64>()
            .0
            .iter()
            .all(|b| (-32768..=32767).contains(&b[0]))
    });
    let dcs = if fits16 { 2 } else { 4 };
    let aq_on = aq_on_cfg(cfg);
    let dch = if aq_on || skip_on || exact.is_some() {
        4
    } else {
        NCH
    };
    let mut dc = vec![0u8; g.bw * g.bh * dch * dcs];
    for sc in &stripes {
        for r in 0..sc.nby {
            pulse.check().map_err(|_| Error::Cancelled)?;
            for bx in 0..g.bw {
                let b = r * g.bw + bx;
                let o0 = (((sc.by0 + r) * g.bw + bx) * dch) * dcs;
                let put = |dc: &mut Vec<u8>, c: usize, v: i32| {
                    let o = o0 + c * dcs;
                    if fits16 {
                        dc[o..o + 2].copy_from_slice(&(v as u16).to_ne_bytes());
                    } else {
                        dc[o..o + 4].copy_from_slice(&v.to_ne_bytes());
                    }
                };
                for c in 0..NCH {
                    let v = sc.q[(b * NCH + c) * 64];
                    put(&mut dc, c, if fits16 { v + 32768 } else { v });
                }
                if dch == 4 {
                    put(&mut dc, 3, sc.aq[b] as i32);
                }
            }
        }
    }
    let dc_info = ImageInfo::new(
        g.bw as u32,
        g.bh as u32,
        if dch == 4 {
            Channels::Rgba
        } else {
            Channels::Rgb
        },
        if fits16 {
            SampleType::U16
        } else {
            SampleType::I32
        },
    );
    let mut dc_rows = dc_stripe_rows(cfg, sb, info.width, info.height);
    // Column groups: the DC image's groups cover the same blocks.
    let gcols = lossy_group_cols(cfg, g.w);
    let ng = if gcols == 0 { 1 } else { g.w.div_ceil(gcols) };
    let coverage_head = cfg
        .dc_coverage
        .as_ref()
        .map(|lists| dc_coverage::Coverage::write(lists));
    let coverage = coverage_head
        .as_ref()
        .map(|head| {
            dc_coverage::Coverage::read(
                head,
                g.stripes(),
                ng,
                g.sb,
                g.bh,
                &crate::parse::limits::Unrestricted,
            )
        })
        .transpose()?;
    if let Some(c) = &coverage {
        dc_rows = c
            .rectangles
            .iter()
            .map(|&(a, b, _)| b - a)
            .max()
            .unwrap()
            .div_ceil(g.sb)
            * g.sb;
    }
    let dc_cfg = crate::EncodeConfig {
        effort: cfg.effort,
        stripe_rows: Some(dc_rows as u32),
        lanes: dc_lanes(cfg),
        threads: cfg.threads,
        column_groups: Some((gcols / 8) as u32),
        // Nested images stay within Lossless (and Extended) 0.1.
        float_shift: false,
        ..Default::default()
    };
    let (dc_head, dc_stripes, dc_explicit) =
        crate::seeds::with_role(crate::seeds::Role::Dc, || {
            let seeds = seed_bank(cfg).map(|b| (b, seed_exact(cfg)));
            if let Some(c) = &coverage {
                return crate::v1::encode_dc_rectangles(
                    &dc,
                    g.bw * dch * dcs,
                    &dc_info,
                    &dc_cfg,
                    seeds,
                    &c.rectangles,
                    &nested,
                );
            }
            crate::v1::encode_parts_seeded_with_progress(
                &dc,
                g.bw * dch * dcs,
                &dc_info,
                &dc_cfg,
                seeds,
                &nested,
            )
        })?;
    // Features the nested heads use (a 32-bit DC image's offset maps): the
    // file declares them (SPEC 2.8).
    let mut nested_features = crate::v1::head_features(&dc_info, &dc_head);
    if let Some(e) = &dc_explicit {
        nested_features = nested_features.union(crate::v1::head_features(&dc_info, &e.0));
    }

    // Alpha: a nested lossless gray image with the same sample type, one
    // stripe per lossy stripe.
    let alpha = if info.channels.has_alpha() {
        let mut a = Vec::with_capacity(g.w * g.h * bps);
        for y in 0..g.h {
            pulse.check().map_err(|_| Error::Cancelled)?;
            let (pixels, stride) = pixels.expect("alpha channel requires pixel input");
            let row = &pixels[y * stride..][..rb];
            for x in 0..g.w {
                let o = (x * nci + nci - 1) * bps;
                a.extend_from_slice(&row[o..o + bps]);
            }
        }
        let ai = ImageInfo::new(info.width, info.height, Channels::Gray, info.sample)
            .with_significant_bits(&[info.channel_bits(info.channels.count() - 1).unwrap()])?;
        let acfg = crate::EncodeConfig {
            effort: cfg.effort,
            stripe_rows: Some(sb * 8),
            threads: cfg.threads,
            column_groups: Some(gcols as u32),
            // Nested images stay within Lossless (and Extended) 0.1.
            float_shift: false,
            ..Default::default()
        };
        let coded = crate::seeds::with_role(crate::seeds::Role::Alpha, || {
            let seeds = seed_bank(cfg).map(|b| (b, seed_exact(cfg)));
            crate::v1::encode_parts_seeded_with_progress(&a, g.w * bps, &ai, &acfg, seeds, &nested)
        })?;
        nested_features = nested_features.union(crate::v1::head_features(&ai, &coded.0));
        if let Some(e) = &coded.2 {
            nested_features = nested_features.union(crate::v1::head_features(&ai, &e.0));
        }
        Some(coded)
    } else {
        None
    };

    pulse.check().map_err(|_| Error::Cancelled)?;
    // Pass 2: histograms -> tables.
    // Entropy mode: the tier's choice from the run-level token density, or
    // the configured mode. Small-tier images fold the token count into the
    // run-level coding below (both modes are coded anyway); larger tiers
    // need the counts before choosing, and lab profiling always wants them.
    let small = g.w * g.h <= SMALL_IMAGE;
    let want_stats =
        (cfg.decode_speed.is_some() && !small) || cfg!(all(feature = "lab", feature = "std"));
    let mut stats = want_stats.then(|| side_stats(&stripes, g.bw, cfg.threads));
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
                let st = *stats.get_or_insert_with(|| side_stats(&stripes, g.bw, cfg.threads));
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
    // A small automatic-tier encode can compare two different reconstructions.
    // For a lab request with a different bank, preserve the mode the normal
    // complete-file comparison would select, including its explicit fallback.
    #[cfg(feature = "lab")]
    let reference_entropy =
        if separate_tier_coefficients && cfg.seed_bank != crate::seeds::DEFAULT_BANK {
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
                trellis.map(|t| (t, false)),
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
            &steps,
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
    let pass1 = separate_tier_coefficients.then(|| stripes.clone());
    // The AC trellis re-decides the AC coefficients from each stripe's
    // recomputed transform.
    // Exact masks use 0 = lossy, 1 = exact; trellis block codes use
    // 64 = ordinary quantization, 0 = keep. Translate only for the search,
    // then restore the wire codes used by the already encoded DC image.
    let mut stripes = stripes;
    if trellis.is_some() && exact.is_some() {
        exact_aq_to_selector(&mut stripes);
    }
    let mut stripes = match &trellis {
        Some(t) => trellis::optimize(
            stripes,
            rows.expect("the coefficient path runs no pixel stages"),
            pw,
            g.bw,
            gcols / 8,
            &steps,
            cfg,
            effort,
            t,
            trellis_aq,
            open_lambda_map,
            trellis_bias.as_ref(),
            tier_huf
                || (cfg.decode_speed.is_none()
                    && cfg.entropy == LossyEntropy::SplitHuf
                    && trellis::huf_rate(cfg)),
            pulse,
        )?,
        None => stripes,
    };
    // Small tier images' split-Huffman candidate: the split-Huffman rate
    // model's trellis of the pass-1 coefficients.
    let pass1 = match (pass1, &trellis) {
        (Some(mut p), Some(t)) if trellis::huf_rate(cfg) => {
            if exact.is_some() {
                exact_aq_to_selector(&mut p);
            }
            let mut p = trellis::optimize(
                p,
                rows.expect("the coefficient path runs no pixel stages"),
                pw,
                g.bw,
                gcols / 8,
                &steps,
                cfg,
                effort,
                t,
                trellis_aq,
                open_lambda_map,
                trellis_bias.as_ref(),
                true,
                pulse,
            )?;
            if exact.is_some() {
                exact_aq_to_wire(&mut p);
            }
            Some(p)
        }
        (p, _) => p,
    };
    if trellis.is_some() && exact.is_some() {
        exact_aq_to_wire(&mut stripes);
    }
    // Lab counters of the coded coefficients (`lab::lossy_side_stats`).
    #[cfg(all(feature = "lab", feature = "std"))]
    if trellis.is_some() {
        prof::side_stats(&trellis::unit_stats(&stripes, cfg.threads));
    } else if let Some(st) = &stats {
        prof::side_stats(st);
    }
    pulse.check().map_err(|_| Error::Cancelled)?;
    // Table headers and independently coded stripe records in the selected
    // mode; with seed tables, also the same blob with explicit tables.
    struct AcCoding {
        entropy: LossyEntropy,
        tb: Vec<u8>,
        records: Vec<Vec<u8>>,
        explicit: Option<Coded>,
    }
    impl AcCoding {
        /// The coding without seed tables.
        fn explicit(&self) -> (&[u8], &[Vec<u8>]) {
            match &self.explicit {
                Some((t, r)) => (t, r),
                None => (&self.tb, &self.records),
            }
        }
    }
    let code_units = |units: &[StripeCoefs], entropy| -> Result<AcCoding, Error> {
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
        })
    };
    let code = |entropy| code_units(&stripes, entropy);
    // The tier's choice: split-Huffman when the cost model predicts it
    // decodes within the tier's ratio of run-level's time; small images
    // try both and also take split-Huffman when it is smaller (its tables
    // cost less than the rANS tables). With seed tables the size compares
    // each mode's smaller coding; the mode the encoder picks without seeds
    // is kept too, for the final comparison below.
    let (codings, pick, pick_explicit) = match cfg.decode_speed {
        None => (alloc::vec![code(cfg.entropy)?], 0, 0),
        Some(speed) => {
            let size = |(t, r): (&[u8], &[Vec<u8>])| {
                t.len() + r.iter().map(|s| s.len() + 3).sum::<usize>()
            };
            let smaller = |c: &AcCoding| size((&c.tb, &c.records)).min(size(c.explicit()));
            if small {
                // The run-level coding's token count stands in for the
                // side-stats coefficient walk: splitting column groups
                // preserves per-block emits.
                let (rl, n) = {
                    let (tb, records, n, explicit) =
                        encode_ans_stripes(&stripes, cfg, effort, pulse)?;
                    let rl = AcCoding {
                        entropy: LossyEntropy::RunLevel,
                        tb,
                        records,
                        explicit,
                    };
                    (rl, n)
                };
                let st = stats.unwrap_or_else(|| side_stats_lanes(&stripes, n));
                let ratio = predicted_huf_ratio((g.w * g.h) as f64, &st);
                let fast = huf_max_ratio(speed).is_some_and(|m| ratio <= m);
                // Split-Huffman never carries the run-level trellis's
                // coefficients (with the effort's trellis: the split-Huffman
                // rate model's, from pass 1).
                let sh = code_units(
                    pass1.as_ref().map_or(&stripes[..], |p| &p[..]),
                    LossyEntropy::SplitHuf,
                )?;
                let pick = (fast || smaller(&sh) < smaller(&rl)) as usize;
                let pick_explicit = (fast || size(sh.explicit()) < size(rl.explicit())) as usize;
                #[cfg(feature = "lab")]
                let (pick, pick_explicit) =
                    reference_entropy.map_or((pick, pick_explicit), |entropy| {
                        let pick = usize::from(entropy == LossyEntropy::SplitHuf as u8);
                        (pick, pick)
                    });
                (alloc::vec![rl, sh], pick, pick_explicit)
            } else {
                let st = stats.as_ref().unwrap();
                let ratio = predicted_huf_ratio((g.w * g.h) as f64, st);
                let fast = huf_max_ratio(speed).is_some_and(|m| ratio <= m);
                let mode = if fast {
                    LossyEntropy::SplitHuf
                } else {
                    LossyEntropy::RunLevel
                };
                (alloc::vec![code(mode)?], 0, 0)
            }
        }
    };

    // Bytes of the AC records as written, and whether compact restart states
    // pay for themselves (they are chosen when smaller).
    // Superblocks and small transforms: each unit's partition map. A file
    // whose units split some plane takes the joint maps (head record 23),
    // else one that merges takes record 21's maps, else no map (the plain
    // lossy file).
    let any_split = stripes.iter().any(|sc| sc.splits.iter().any(|&v| v != 0));
    let any_merge = sb_on && stripes.iter().any(|sc| sc.modes.iter().any(|&v| v != 0));
    let map_kind = if any_split {
        MapKind::Joint
    } else if any_merge {
        MapKind::Merges
    } else {
        MapKind::None
    };
    let default_maps = if cfg.coding_defaults {
        if layouts != [LossyChannelLayout::default(); 3] {
            return Err(Error::InvalidInput("coding defaults with channel layouts"));
        }
        Some(coding_defaults::encode(&stripes)?)
    } else {
        None
    };
    let sb_maps: Option<Vec<Vec<u8>>> = if let Some((_, maps)) = &default_maps {
        Some(maps.clone())
    } else {
        match map_kind {
            MapKind::None => None,
            MapKind::Merges => Some(
                stripes
                    .iter()
                    .map(|sc| superblock::map::encode(&sc.modes, sc.nby, sc.bw))
                    .collect(),
            ),
            MapKind::Joint => Some(
                stripes
                    .iter()
                    .map(|sc| {
                        // Bypassed parents (exact blocks: wire code 1; kept:
                        // code 0) carry no flags.
                        let bypass = |b: usize| {
                            let c = sc.aq.get(b).copied();
                            (exact.is_some() && c == Some(1)) || (skip_on && c == Some(0))
                        };
                        let n = sc.nby * sc.bw;
                        let modes = if sc.modes.is_empty() {
                            alloc::vec![0u8; n]
                        } else {
                            sc.modes.clone()
                        };
                        // Flags only where the final coefficients have AC (a
                        // plane the trellis emptied decodes as the 8x8 block).
                        let ac = superblock::ac_bits(&sc.q, n);
                        let splits: Vec<u8> = (0..n)
                            .map(|b| sc.splits.get(b).copied().unwrap_or(0) & ac[b])
                            .collect();
                        superblock::map::encode_joint(
                            &modes,
                            &splits,
                            &ac,
                            (sc.nby, sc.bw),
                            &bypass,
                        )
                    })
                    .collect(),
            ),
        }
    };
    let ac_records_len =
        |entropy: LossyEntropy, stripe_bytes: &[Vec<u8>]| -> Result<(u128, bool), Error> {
            let record_len =
                |n: usize| n as u128 + (64 - (n as u64).leading_zeros()).max(1).div_ceil(7) as u128;
            let (mut raw, mut packed) = (0u128, 0u128);
            #[cfg(all(feature = "lab", test))]
            let catalog = cfg
                .catalog_lab
                .is_some_and(model_candidates::Settings::uses_envelope);
            #[cfg(not(all(feature = "lab", test)))]
            let catalog = false;
            let compactable = cfg.compact_streams
                && entropy == LossyEntropy::RunLevel
                && !cfg.local_ans_contexts
                && !cfg.model_references
                && !catalog;
            for (u, (bytes, sc)) in stripe_bytes.iter().zip(&stripes).enumerate() {
                let map_len = if let Some(maps) = &sb_maps {
                    crate::varint_len(maps[u].len() as u64) + maps[u].len()
                } else if adaptive_mask == 0 || sc.modes.is_empty() {
                    // No mode map is written (with superblocks on and nothing
                    // merged, `modes` holds the selector's all-zero roles).
                    0
                } else if sc.modes.iter().all(|m| *m == sc.modes[0]) {
                    2
                } else {
                    1 + (sc.modes.len() * 3).div_ceil(8)
                };
                raw += record_len(map_len + bytes.len());
                if compactable {
                    packed += record_len(map_len + compact::packed_len(bytes, sc.bw.min(NL))?);
                }
            }
            Ok(if compactable && packed < raw {
                (packed, true)
            } else {
                (raw, false)
            })
        };
    let assemble = |entropy: LossyEntropy,
                    tb: &[u8],
                    stripe_bytes: &[Vec<u8>],
                    dc_head: &[u8],
                    dc_stripes: &[Vec<u8>],
                    alpha: Option<(&[u8], &[Vec<u8>])>,
                    seed_selection: crate::seeds::Selection|
     -> Result<Vec<u8>, Error> {
        use crate::parse::records::{
            H_UNIT_ORDINALS, R_AC, R_ALPHA, R_DC, R_EXACT, main_framed_len, put_main_record,
            put_record,
        };
        let compact_states = ac_records_len(entropy, stripe_bytes)?.1;
        let mut model_writer = if cfg.model_references {
            Some(model_refs::Writer::new(
                tb,
                entropy,
                if seed_selection.scope & crate::seeds::SCOPE_AC != 0 {
                    seed_selection.bank
                } else {
                    0
                },
            )?)
        } else {
            None
        };
        let tb = model_writer.as_ref().map_or(tb, |w| w.root);
        // Unit records (SPEC 2.7), in decode order: per lossy stripe s, the
        // DC stripe s / k when s is its first lossy stripe (k = DC stripe
        // height / sb), the alpha stripe s, lossy stripe s, then its exact
        // record (exact blocks only); with column groups, per stripe and
        // group in order. The unit table hint lists their spans.
        let k = dc_rows / sb as usize;
        let ordinals = cfg.unit_ordinals;
        let mut body = Vec::new();
        let mut spans = Vec::new();
        let ordinal_counter = core::cell::Cell::new(0u64);
        let mut rec = |id: u64, b: &[u8]| {
            let prefixed;
            let b = if cfg.model_references && id == R_DC {
                let mut bytes = vec![0, 0];
                bytes.extend_from_slice(b);
                prefixed = bytes;
                &prefixed[..]
            } else {
                b
            };
            let ord = ordinals.then_some(ordinal_counter.get());
            ordinal_counter.set(ordinal_counter.get() + 1);
            spans.push(main_framed_len(id, ord, b.len()));
            put_main_record(&mut body, id, ord, b);
        };
        for (u, st) in stripe_bytes.iter().enumerate() {
            let (si, gi) = (u / ng, u % ng);
            if let Some(c) = &coverage {
                let span = c.span(si, gi);
                if span.start == si {
                    rec(R_DC, &dc_stripes[span.unit]);
                }
            } else if si % k == 0 {
                rec(R_DC, &dc_stripes[si / k * ng + gi]);
            }
            if let Some((_, a)) = &alpha {
                rec(R_ALPHA, &a[u]);
            }
            let native;
            let st = if let Some(writer) = &mut model_writer {
                native = writer.ac(st, ordinal_counter.get())?;
                &native[..]
            } else {
                st
            };
            if adaptive_mask != 0 || compact_states || sb_maps.is_some() {
                let mut record = Vec::new();
                if let Some(maps) = &sb_maps {
                    put_varint(&mut record, maps[u].len() as u64);
                    record.extend_from_slice(&maps[u]);
                } else if adaptive_mask != 0 {
                    layout::write_modes(&mut record, &stripes[u].modes);
                }
                if compact_states {
                    record.extend_from_slice(&compact::pack(st, stripes[u].bw.min(NL))?);
                } else {
                    record.extend_from_slice(st);
                }
                rec(R_AC, &record);
            } else {
                rec(R_AC, st);
            }
            if let Some((_, er)) = &exact {
                rec(R_EXACT, &er[u]);
            }
        }
        // Head records in increasing id order (SPEC 2.8). The unit table
        // can be a hint (10) or a reservation (12), on either side of DC32 (11).
        let mut records = Vec::new();
        let hint_form = matches!(crate::v1::unit_table_form(), crate::v1::UnitTableForm::Hint);
        for (on, id) in [(exact.is_some(), H_EXACT), (aq_on, H_AQ), (skip_on, H_KEEP)] {
            if on {
                put_record(&mut records, id, &[]);
            }
        }
        if hint_form {
            crate::v1::put_unit_table(&mut records, stripe_bytes.len(), &spans);
        }
        if !fits16 {
            put_record(&mut records, H_DC32, &[]);
        }
        if !hint_form {
            crate::v1::put_unit_table(&mut records, stripe_bytes.len(), &spans);
        }
        if qp.matrix == MATRIX_EXPLICIT {
            let mut rec = Vec::new();
            envelope::write_explicit(&mut rec, qp.explicit.as_ref().expect("explicit tables"));
            put_record(&mut records, H_EXPLICIT, &rec);
        }
        if let Some(b) = &qp.bias {
            let mut rec = Vec::new();
            envelope::write_bias(&mut rec, b);
            put_record(&mut records, H_BIAS, &rec);
        }
        if let Some(t) = qp.transform {
            let mut rec = Vec::new();
            envelope::write_transform(&mut rec, t);
            put_record(&mut records, H_TRANSFORM, &rec);
        }
        match if cfg.coding_defaults {
            MapKind::None
        } else {
            map_kind
        } {
            MapKind::None => {}
            MapKind::Merges => put_record(&mut records, superblock::H_SUPERBLOCKS, &[]),
            MapKind::Joint => {
                #[cfg(feature = "lab")]
                let payload: &[u8] = if cfg.lab.small.as_ref().is_some_and(|s| s.int) {
                    &[1]
                } else {
                    &[]
                };
                #[cfg(not(feature = "lab"))]
                let payload: &[u8] = &[];
                put_record(&mut records, small::H_PARTITIONS, payload)
            }
        }
        if layouts != [LossyChannelLayout::default(); 3] {
            put_record(&mut records, H_LAYOUTS, &layouts.map(|l| l.byte()));
        }
        if ordinals {
            put_record(&mut records, H_UNIT_ORDINALS, &[]);
        }
        if cfg.local_ans_contexts {
            put_record(&mut records, local_ans::HEAD, &[]);
        }
        if let Some((head, _)) = &default_maps {
            put_record(&mut records, coding_defaults::HEAD, head);
        }
        if cfg.model_references {
            put_record(&mut records, model_refs::HEAD, &[1, 0]);
        }
        if let Some(head) = &coverage_head {
            put_record(&mut records, dc_coverage::HEAD, head);
        }
        #[cfg(all(feature = "lab", test))]
        let tb = if !cfg.model_references
            && cfg
                .catalog_lab
                .is_some_and(model_candidates::Settings::uses_envelope)
        {
            let mut bundle = ByteReader::new(tb);
            let n = bundle.var_usize()?;
            let tables = bundle.bytes(n)?;
            put_record(
                &mut records,
                model_candidates::HEAD,
                bundle.bytes(bundle.remaining())?,
            );
            tables
        } else {
            tb
        };
        let mut data = Vec::new();
        data.push(sb as u8);
        put_varint(&mut data, gcols as u64);
        envelope::write(
            &mut data,
            color,
            kind,
            &envelope::Params {
                scale,
                qp,
                entropy,
                compact: compact_states,
                seed: seed_selection,
                records: !records.is_empty(),
            },
        );
        put_varint(&mut data, tb.len() as u64);
        data.extend_from_slice(tb);
        if !records.is_empty() {
            put_varint(&mut data, records.len() as u64);
            data.extend_from_slice(&records);
        }
        // The nested heads, positional (SPEC 2.8): DC, alpha (with alpha),
        // exact blocks (with head record 5).
        put_varint(&mut data, dc_head.len() as u64);
        data.extend_from_slice(dc_head);
        if let Some((ah, _)) = &alpha {
            put_varint(&mut data, ah.len() as u64);
            data.extend_from_slice(ah);
        }
        if let Some((eh, _)) = &exact {
            put_varint(&mut data, eh.len() as u64);
            data.extend_from_slice(eh);
        }
        data.extend_from_slice(&body);
        let mut out = Vec::with_capacity(data.len() + 64);
        let mut c = cfg.clone();
        c.color = color;
        // New seed IDs declare `seed_bank_ids`, exact blocks `exact_blocks`,
        // an i32 DC image `samples_32` (implied by 32-bit sample types: the
        // header writer drops it there), channel layouts `channel_layouts`,
        // with the nested heads' features, in the header's feature mask
        // (the comparisons below count its bytes).
        use crate::parse::header::Feature;
        let mut features = nested_features;
        if seed_selection.is_some() {
            features = features.with(Feature::SeedBankIds);
        }
        if exact.is_some() {
            features = features.with(Feature::ExactBlocks);
        }
        if !fits16 {
            features = features.with(Feature::Samples32);
        }
        if layouts != [LossyChannelLayout::default(); 3] {
            features = features.with(Feature::ChannelLayouts);
        }
        if (3..=MATRIX_LAST_PRESET).contains(&qp.matrix) {
            features = features.with(Feature::DequantPresets);
        }
        if qp.matrix == MATRIX_EXPLICIT {
            features = features.with(Feature::ExplicitTables);
        }
        if qp.bias.is_some() {
            features = features.with(Feature::ReconBias);
        }
        if qp.transform.is_some() {
            features = features.with(Feature::ColorTransform);
        }
        if any_merge {
            features = features.with(Feature::Superblocks);
        }
        if cfg.model_references {
            features = features.with(Feature::ModelReferences);
        }
        if cfg.dc_coverage.is_some() {
            features = features.with(Feature::DcCoverage);
        }
        if cfg.coding_defaults {
            features = features.with(Feature::CodingDefaults);
        }
        if map_kind == MapKind::Joint {
            features = features.with(Feature::SmallTransforms);
        }
        if ordinals {
            features = features.with(Feature::UnitOrdinals);
        }
        if cfg.local_ans_contexts {
            features = features.with(Feature::LocalAnsContexts);
        }
        crate::container::write_head_features(
            &mut out,
            info,
            meta,
            crate::Coding::Lossy,
            Some(color),
            c.decode_speed(),
            features,
        )?;
        crate::container::put_data(&mut out, &data);
        Ok(out)
    };
    // Each table blob takes its smaller coding (the parts' bytes add up),
    // then the result is compared once with the all-explicit file: the file
    // this encoder writes without seed tables, byte for byte, which wins
    // ties. Both are complete files, so the seeded file's feature mask (the
    // mask field, or nothing when exact blocks already wrote it) counts. So
    // seed tables never make a file larger. Both candidates carry the same
    // exact blocks.
    type Part<'p> = (&'p [u8], &'p [Vec<u8>]);
    fn varint_len(n: usize) -> u128 {
        (64 - (n as u64).leading_zeros()).max(1).div_ceil(7) as u128
    }
    fn nested_len((head, recs): Part<'_>) -> u128 {
        varint_len(head.len())
            + head.len() as u128
            + recs
                .iter()
                .map(|r| varint_len(r.len()) + r.len() as u128)
                .sum::<u128>()
    }
    /// The smaller of a nested head's seeded coding `main` and its explicit
    /// one, and whether it is the seeded one.
    fn choose<'p>(main: Part<'p>, explicit: &'p Option<Coded>) -> (Part<'p>, bool) {
        match explicit {
            Some(e) if nested_len(main) < nested_len((&e.0, &e.1)) => (main, true),
            Some(e) => ((&e.0[..], &e.1[..]), false),
            None => (main, false),
        }
    }
    let ac_len = |c: &AcCoding, (tb, recs): Part<'_>| -> Result<u128, Error> {
        Ok(varint_len(tb.len()) + tb.len() as u128 + ac_records_len(c.entropy, recs)?.0)
    };
    let ac = &codings[pick];
    let (ac_part, ac_seeded) = match &ac.explicit {
        Some(e) if ac_len(ac, (&ac.tb, &ac.records))? < ac_len(ac, (&e.0, &e.1))? => {
            ((&ac.tb[..], &ac.records[..]), true)
        }
        _ => (ac.explicit(), false),
    };
    let (dc_part, dc_seeded) = choose((&dc_head, &dc_stripes), &dc_explicit);
    let al = alpha.as_ref().map(|(h, r, e)| choose((h, r), e));
    let scope = ((ac_seeded as u8) * crate::seeds::SCOPE_AC)
        | ((dc_seeded as u8) * crate::seeds::SCOPE_DC)
        | ((al.is_some_and(|a| a.1) as u8) * crate::seeds::SCOPE_ALPHA);
    let explicit_file = |c: &AcCoding| {
        let (dh, dr) = dc_explicit
            .as_ref()
            .map_or((&dc_head[..], &dc_stripes[..]), |e| (&e.0[..], &e.1[..]));
        let al = alpha.as_ref().map(|(h, r, e)| {
            e.as_ref()
                .map_or((&h[..], &r[..]), |e| (&e.0[..], &e.1[..]))
        });
        let (t, r) = c.explicit();
        assemble(c.entropy, t, r, dh, dr, al, crate::seeds::Selection::NONE)
    };
    if scope == 0 && pick == pick_explicit {
        #[cfg(all(feature = "lab", feature = "std"))]
        prof::ac_tables(
            codings[pick_explicit].entropy as u8,
            codings[pick_explicit].explicit().0.len() as u64,
        );
        let out = explicit_file(&codings[pick_explicit])?;
        pulse.check().map_err(|_| Error::Cancelled)?;
        return Ok(out);
    }
    let bank_id = seed_bank(cfg).map_or(0, |b| b.id);
    let seed_selection = if scope == 0 {
        crate::seeds::Selection::NONE
    } else {
        crate::seeds::Selection::new(bank_id, scope)
    };
    let file = assemble(
        ac.entropy,
        ac_part.0,
        ac_part.1,
        dc_part.0,
        dc_part.1,
        al.map(|a| a.0),
        seed_selection,
    )?;
    let without = explicit_file(&codings[pick_explicit])?;
    #[cfg(all(feature = "lab", feature = "std"))]
    {
        let (mode, tb) = if file.len() < without.len() {
            (ac.entropy, ac_part.0)
        } else {
            (
                codings[pick_explicit].entropy,
                codings[pick_explicit].explicit().0,
            )
        };
        prof::ac_tables(mode as u8, tb.len() as u64);
    }
    pulse.check().map_err(|_| Error::Cancelled)?;
    Ok(if file.len() < without.len() {
        file
    } else {
        without
    })
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
#[cfg(feature = "encode")]
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
#[cfg(all(feature = "encode", any(feature = "lab", feature = "native-jpeg")))]
fn finish_quantized_coefficients(
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
