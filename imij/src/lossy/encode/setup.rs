//! Frame validation, quantization/color setup, lab AQ preparation, and
//! reusable row-plane filling for the whole-image lossy encoder.

use alloc::borrow::Cow;
#[cfg(feature = "lab")]
use alloc::vec;
use alloc::vec::Vec;

use crate::effort::{Effort, LossyEffortPlan};
use crate::lossy::{
    ColorTransform, Geom, InputKind, LossyChannelLayout, LossyColor, LossyConfig, MATRIX_EXPLICIT,
    MATRIX_LAST_PRESET, QParams, adaptive_mode, aq_map, block_extremes, color, color_index,
    dc_nearest, dct, dering, dering_row, enc_simd, fill_color_row, finite, layout, local_ans,
    precision, subblock, transfer, trellis,
};
#[cfg(feature = "lab")]
use crate::lossy::{NCH, aq_field_lab, aq_on_cfg, envelope, lab_dc_step};
use crate::{Channels, Error, ImageInfo, SampleType};

/// Validated, precomputed parameters for encoding one lossy frame.
pub(super) struct FrameSetup {
    pub effort: Effort,
    pub plan: LossyEffortPlan,
    pub trellis: Option<(trellis::LossyTrellis, bool)>,
    pub layouts: [LossyChannelLayout; 3],
    pub adaptive_mask: u8,
    pub skip_on: bool,
    pub kind: InputKind,
    pub nci: usize,
    pub bps: usize,
    pub ci: [usize; 3],
    pub color: LossyColor,
    pub xyb: bool,
    pub rb: usize,
    pub sb: u32,
    pub g: Geom,
    pub pw: usize,
    pub qp: QParams,
    pub steps: [[f32; 64]; 3],
    pub inv_steps: [[f32; 64]; 3],
    pub layout_steps: [[[f32; 64]; 2]; 3],
    pub transform_forward: Option<[[f32; 4]; 3]>,
    pub scale: [f32; 3],
    pub cm: [[f32; 8]; 8],
    pub round: f32,
    pub dering_on: bool,
    pub dc_fix: bool,
    pub white_code: Option<[u32; 3]>,
    pub dering_cap: f32,
    pub u8_lin: [f32; 256],
    pub lut16: Vec<f32>,
    pub input_precision: Option<precision::Input>,
    pub centroid_k: Option<u8>,
}

/// Validates input geometry, sample precision, and lossy config, resolving
/// `QParams`, effort plan, trellis eligibility, color transform, and LUTs.
pub(super) fn prepare_frame<'a>(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    cfg: &'a LossyConfig,
    previous: Option<(&'a [u8], usize)>,
) -> Result<(Cow<'a, LossyConfig>, FrameSetup, Option<(&'a [u8], usize)>), Error> {
    if info.channels.is_native_model() {
        return Err(Error::Unsupported("pixel model requires lossless coding"));
    }
    // LAB: this encode's switches, fixed now for every thread it uses.
    #[cfg(feature = "lab")]
    let cfg: Cow<'a, LossyConfig> = {
        let mut c = cfg.clone();
        c.lab = c.lab.pinned();
        Cow::Owned(c)
    };
    #[cfg(not(feature = "lab"))]
    let cfg: Cow<'a, LossyConfig> = Cow::Borrowed(cfg);

    local_ans::validate(&cfg)?;
    #[cfg(feature = "lab")]
    if cfg.coding_defaults && cfg.lab.small.as_ref().is_some_and(|s| s.int) {
        return Err(Error::InvalidInput(
            "coding defaults use native float transforms",
        ));
    }
    #[cfg(feature = "lab")]
    crate::seeds::validate_id(cfg.seed_bank)?;
    info.validate_precision()?;
    let effort = Effort::new(cfg.effort)?;
    let cfg: Cow<'a, LossyConfig> = if cfg.decoder_memory_limit.is_some() {
        Cow::Owned(crate::decoder_memory::plan_lossy(info, &cfg)?.0)
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
    validate_lab_settings(info, &cfg, previous)?;

    // Centralized lossy effort policy (effort.rs).
    let plan = effort.lossy_plan(adaptive_mode(&cfg));
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
    let map = aq_map(&cfg);
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
    for (i, v) in lab_dc_step(&cfg).into_iter().enumerate() {
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
    let dering_on = dering::enabled(&cfg) && full8_luma;
    let dc_fix = dc_nearest(&cfg);
    // Maximum code per color channel, for integer inputs (the decoder
    // clamps those): white and black blocks for deringing and DC rounding.
    let white_code: Option<[u32; 3]> =
        (matches!(kind, InputKind::U8 | InputKind::U16) && full8_luma && (dering_on || dc_fix))
            .then(|| core::array::from_fn(|c| (1u32 << info.channel_bits(ci[c]).unwrap()) - 1));
    let dering_cap = 2.0 * steps[0][0];

    // LAB: per-image centroid reconstruction bias (Lossy 0.4). The stats
    // ride along the pass-1 quantization, one accumulator per stripe.
    #[cfg(feature = "lab")]
    let centroid_k = cfg.lab.recon_bias_centroid;
    #[cfg(not(feature = "lab"))]
    let centroid_k: Option<u8> = None;

    let setup = FrameSetup {
        effort,
        plan,
        trellis,
        layouts,
        adaptive_mask,
        skip_on,
        kind,
        nci,
        bps,
        ci,
        color,
        xyb,
        rb,
        sb,
        g,
        pw,
        qp,
        steps,
        inv_steps,
        layout_steps,
        transform_forward,
        scale,
        cm,
        round,
        dering_on,
        dc_fix,
        white_code,
        dering_cap,
        u8_lin,
        lut16,
        input_precision,
        centroid_k,
    };
    Ok((cfg, setup, previous))
}

#[cfg(feature = "lab")]
fn validate_lab_settings(
    info: &ImageInfo,
    cfg: &LossyConfig,
    previous: Option<(&[u8], usize)>,
) -> Result<(), Error> {
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
        && (cfg.entropy != crate::lossy::LossyEntropy::RunLevel
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
    Ok(())
}

/// Reusable color-plane builder for block rows, shared across pass 1,
/// partition selection, trellis recomputation, and lab AQ fields.
pub(super) struct RowFiller<'a> {
    pixels: &'a [u8],
    stride: usize,
    setup: &'a FrameSetup,
}

impl<'a> RowFiller<'a> {
    pub(super) fn new(pixels: &'a [u8], stride: usize, setup: &'a FrameSetup) -> Self {
        Self {
            pixels,
            stride,
            setup,
        }
    }

    /// Planar color for the 8 pixel rows of block row `by` (linear light for
    /// XYB, code values for YCoCg; rows past the bottom repeat the last one),
    /// color converted, with overshoot deringing on the luma plane;
    /// `extremes` gets the row's white/black blocks (integer inputs).
    pub(super) fn fill_block_row(
        &self,
        by: usize,
        planes: &mut [f32],
        extremes: &mut Vec<(u64, bool)>,
    ) {
        let s = self.setup;
        for y in 0..8 {
            let sy = (by * 8 + y).min(s.g.h - 1);
            let row = &self.pixels[sy * self.stride..][..s.rb];
            match s.kind {
                InputKind::U8 if s.input_precision.is_some() => {
                    let input = s.input_precision.as_ref().unwrap();
                    fill_color_row::<1>(row, planes, s.pw, y, s.g.w, s.nci, s.ci, |c, b| {
                        input.read(c, b[0] as u32)
                    });
                }
                InputKind::U16 if s.input_precision.is_some() => {
                    let input = s.input_precision.as_ref().unwrap();
                    fill_color_row::<2>(row, planes, s.pw, y, s.g.w, s.nci, s.ci, |c, b| {
                        input.read(c, u16::from_ne_bytes(*b) as u32)
                    });
                }
                InputKind::U8 if s.xyb => {
                    fill_color_row::<1>(row, planes, s.pw, y, s.g.w, s.nci, s.ci, |_, b| {
                        s.u8_lin[b[0] as usize]
                    })
                }
                InputKind::U8 => {
                    fill_color_row::<1>(row, planes, s.pw, y, s.g.w, s.nci, s.ci, |_, b| {
                        b[0] as f32
                    })
                }
                InputKind::U16 if !s.lut16.is_empty() => {
                    fill_color_row::<2>(row, planes, s.pw, y, s.g.w, s.nci, s.ci, |_, b| {
                        s.lut16[u16::from_ne_bytes(*b) as usize]
                    })
                }
                InputKind::U16 => {
                    fill_color_row::<2>(row, planes, s.pw, y, s.g.w, s.nci, s.ci, |_, b| {
                        u16::from_ne_bytes(*b) as f32
                    })
                }
                InputKind::U32 => {
                    let input = s.input_precision.as_ref().unwrap();
                    fill_color_row::<4>(row, planes, s.pw, y, s.g.w, s.nci, s.ci, |c, b| {
                        input.read(c, u32::from_ne_bytes(*b))
                    })
                }
                InputKind::F16 => {
                    fill_color_row::<2>(row, planes, s.pw, y, s.g.w, s.nci, s.ci, |_, b| {
                        finite(f32::from_bits(crate::mapping::f16_to_f32_bits(
                            u16::from_ne_bytes(*b),
                        )))
                    })
                }
                InputKind::F32 => {
                    fill_color_row::<4>(row, planes, s.pw, y, s.g.w, s.nci, s.ci, |_, b| {
                        finite(f32::from_ne_bytes(*b))
                    })
                }
            }
        }
        let (p0, rest) = planes.split_at_mut(8 * s.pw);
        let (p1, p2) = rest.split_at_mut(8 * s.pw);
        match s.transform_forward.as_ref() {
            Some(t) => enc_simd::color_rows_transform(
                s.xyb,
                s.scale,
                &color::OPSIN_F32,
                color::OPSIN_BIAS,
                color::CBRT_BIAS,
                Some(t),
                p0,
                p1,
                p2,
            ),
            None => enc_simd::color_rows(
                s.xyb,
                s.scale,
                &color::OPSIN_F32,
                color::OPSIN_BIAS,
                color::CBRT_BIAS,
                p0,
                p1,
                p2,
            ),
        }
        if let Some(white_code) = s.white_code {
            block_extremes(
                &s.g,
                self.pixels,
                self.stride,
                by * 8,
                s.nci,
                s.ci,
                s.kind,
                &white_code,
                extremes,
            );
            if s.dering_on {
                dering_row(p0, s.pw, extremes, s.dering_cap);
            }
        }
    }

    /// LAB: luma plane for block row `by` (U8 input), for jpegli's AQ field
    /// and trellis AQ coupling.
    #[cfg(feature = "lab")]
    pub(super) fn fill_luma_row(&self, by: usize, out: &mut [f32]) {
        let s = self.setup;
        let mut planes = vec![0.0f32; NCH * 8 * s.pw];
        for y in 0..8 {
            let row = &self.pixels[(by * 8 + y).min(s.g.h - 1) * self.stride..][..s.rb];
            if s.xyb {
                fill_color_row::<1>(row, &mut planes, s.pw, y, s.g.w, s.nci, s.ci, |_, b| {
                    s.u8_lin[b[0] as usize]
                });
            } else {
                fill_color_row::<1>(row, &mut planes, s.pw, y, s.g.w, s.nci, s.ci, |_, b| {
                    b[0] as f32
                });
            }
        }
        let (p0, rest) = planes.split_at_mut(8 * s.pw);
        let (p1, p2) = rest.split_at_mut(8 * s.pw);
        match s.transform_forward.as_ref() {
            Some(t) => enc_simd::color_rows_transform(
                s.xyb,
                s.scale,
                &color::OPSIN_F32,
                color::OPSIN_BIAS,
                color::CBRT_BIAS,
                Some(t),
                p0,
                p1,
                p2,
            ),
            None => enc_simd::color_rows(
                s.xyb,
                s.scale,
                &color::OPSIN_F32,
                color::OPSIN_BIAS,
                color::CBRT_BIAS,
                p0,
                p1,
                p2,
            ),
        }
        out.copy_from_slice(&planes[..8 * s.pw]);
    }
}

/// Computes optional lab trellis AQ coupling strengths and lab AQ field codes.
pub(super) fn prepare_lab_aq(
    cfg: &LossyConfig,
    setup: &FrameSetup,
    filler: &RowFiller<'_>,
) -> Result<(Vec<f32>, Vec<u8>), Error> {
    #[cfg(feature = "lab")]
    {
        let coupling = matches!(&setup.trellis, Some((t, _)) if t.aq_coupling != 0.0);
        if (coupling || cfg.aq_field.is_some())
            && (setup.kind != InputKind::U8 || setup.input_precision.is_some())
        {
            return Err(Error::Unsupported(
                "lab trellis AQ coupling and AQ field need U8 input",
            ));
        }
        let trellis_aq = if coupling {
            trellis::aq_strengths(
                setup.g.bw,
                setup.g.bh,
                setup.xyb,
                setup.steps[0][1],
                cfg.threads,
                |by, out| filler.fill_luma_row(by, out),
            )
        } else {
            Vec::new()
        };
        let field_codes = match cfg.aq_field {
            Some(f) => {
                let units = aq_field_lab::luma_units(setup.xyb);
                let qf = aq_field_lab::field(
                    setup.g.bw,
                    setup.g.bh,
                    setup.steps[0][1] * units,
                    f.fade,
                    cfg.threads,
                    |by, out| {
                        filler.fill_luma_row(by, out);
                        for v in out.iter_mut() {
                            *v *= units;
                        }
                    },
                );
                aq_field_lab::step_codes(
                    &qf,
                    setup.g.bw,
                    setup.g.bh,
                    f.strength,
                    f.levels,
                    f.cell as usize,
                    f.relative,
                )
            }
            None => Vec::new(),
        };
        Ok((trellis_aq, field_codes))
    }
    #[cfg(not(feature = "lab"))]
    {
        let _ = (cfg, setup, filler);
        Ok((Vec::new(), Vec::new()))
    }
}
