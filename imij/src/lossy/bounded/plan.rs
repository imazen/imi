//! Bounded-memory lossy encoding plan, geometry, and working-memory admission.

use super::*;

/// Everything both passes share: the head's parameters and the geometry.
pub(super) struct Plan {
    #[cfg(all(feature = "std", feature = "lab", feature = "rayon"))]
    pub(super) one_pass_threads: usize,
    pub(super) local_ans_contexts: bool,
    pub(super) coding_defaults: bool,
    pub(super) g: Geom,
    pub(super) sb: u32,
    pub(super) nci: usize,
    pub(super) ci: [usize; 3],
    pub(super) rb: usize,
    pub(super) pw: usize,
    pub(super) xyb: bool,
    pub(super) color: LossyColor,
    pub(super) scale: [f32; 3],
    pub(super) qp: QParams,
    pub(super) steps: [[f32; 64]; 3],
    pub(super) inv_steps: [[f32; 64]; 3],
    /// The AC trellis this effort runs (XYB only, as `finish_lossy_frame`).
    pub(super) trellis: Option<trellis::LossyTrellis>,
    /// Centralized lossy effort plan (`effort.rs`).
    pub(super) plan: crate::effort::LossyEffortPlan,
    pub(super) cm: [[f32; 8]; 8],
    pub(super) round: f32,
    pub(super) dc_fix: bool,
    pub(super) dering_on: bool,
    pub(super) dering_cap: f32,
    pub(super) white_code: Option<[u32; 3]>,
    pub(super) u8_lin: [f32; 256],
    /// Block rows per DC stripe (a whole number of lossy stripes).
    pub(super) dc_rows: usize,
    /// Block rows per work band: whole lossy stripes, about 8 block rows,
    /// dividing `dc_rows`. Tiers with taller DC stripes (Balanced, Compact,
    /// none) gather each DC stripe's DC image over several bands.
    pub(super) work: usize,
    pub(super) gcols: usize,
    pub(super) ng: usize,
    /// Alpha: a nested lossless gray image, one stripe per lossy stripe.
    pub(super) alpha: Option<ImageInfo>,
    pub(super) kind: InputKind,
    /// Sample bytes.
    pub(super) bps: usize,
    /// Code value -> linear light for 16-bit XYB (empty otherwise).
    pub(super) lut16: Vec<f32>,
    /// Declared significant bits or u32 samples: the whole-image encoder's
    /// conversion (`precision::Input`), and every band is range-checked.
    pub(super) input: Option<precision::Input>,
    pub(super) info: ImageInfo,
    /// The exact-block policy (8-bit RGB(A)): blocks are chosen band by band
    /// under prices a statistics pass measures over the whole image.
    pub(super) exact: Option<exact::Policy>,
    /// Superblock merges (Compact): parents chosen stripe by stripe under
    /// prices a statistics pass measures over the whole image.
    pub(super) merges: Option<superblock::select::Opts>,
    /// Channel layouts (row-local transforms and sampling per plane), their
    /// quantizer steps (8x8 and split), and the planes whose transform is
    /// chosen per parent.
    pub(super) layouts: [LossyChannelLayout; 3],
    pub(super) layout_steps: [[[f32; 64]; 2]; 3],
    pub(super) adaptive_mask: u8,
}

// A validated plan without sample conversion tables. Only materialize() makes
// it available to the coding passes; admission can retain this exact plan.
pub(super) struct NativePreparation {
    pub(super) plan: Plan,
    pub(super) lut16_transfer: Option<transfer::Transfer>,
    pub(super) precision_input: bool,
}

impl NativePreparation {
    pub(super) fn materialize(mut self) -> Result<Plan, Error> {
        self.plan.lut16 = match self.lut16_transfer {
            Some(t) => (0..65536)
                .map(|c| t.decode(c as f64 / 65535.0) as f32)
                .collect(),
            None => Vec::new(),
        };
        if self.precision_input {
            self.plan.input = Some(precision::Input::new(&self.plan.info, self.plan.color)?);
        }
        Ok(self.plan)
    }
}

pub(super) fn plan(info: &ImageInfo, cfg: &LossyConfig) -> Result<Plan, Error> {
    prepare_plan(info, cfg)?.materialize()
}

pub(super) fn prepare_plan(
    info: &ImageInfo,
    cfg: &LossyConfig,
) -> Result<NativePreparation, Error> {
    local_ans::validate(cfg)?;
    if cfg.dc_coverage.is_some() {
        return Err(Error::Unsupported(
            "bounded nonuniform DC writer is not integrated",
        ));
    }
    if cfg.coding_defaults && cfg.channel_layouts != [LossyChannelLayout::default(); 3] {
        return Err(Error::Unsupported("scoped defaults with channel layouts"));
    }
    #[cfg(feature = "lab")]
    crate::seeds::validate_id(cfg.seed_bank)?;
    let unsupported = Error::Unsupported;
    if info.channels.is_native_model() {
        return Err(unsupported("pixel model requires lossless coding"));
    }
    info.validate_precision()?;
    if !matches!(
        info.channels,
        Channels::Gray | Channels::GrayAlpha | Channels::Rgb | Channels::Rgba
    ) || !matches!(
        info.sample,
        SampleType::U8 | SampleType::U16 | SampleType::U32 | SampleType::F16 | SampleType::F32
    ) {
        return Err(unsupported(
            "bounded lossy encoding supports Gray, GrayAlpha, RGB and RGBA of u8, u16, u32, f16 or f32 samples",
        ));
    }
    if info.sample == SampleType::U32 && !cfg!(feature = "extended") {
        return Err(unsupported("32-bit samples (extended tier)"));
    }
    if info.width == 0 || info.height == 0 {
        return Err(Error::InvalidInput("empty image"));
    }
    let effort = crate::effort::Effort::new(cfg.effort)?;
    if !cfg.quality.is_finite() || !cfg.round.is_finite() {
        return Err(Error::InvalidInput("invalid lossy numeric parameter"));
    }
    // A decoder memory budget narrows the column groups and stripes as the
    // whole-image encoder does (`decoder_memory::plan_lossy`, from the
    // geometry alone); only this function reads them.
    let planned;
    let cfg = if cfg.decoder_memory_limit.is_some() {
        planned = crate::decoder_memory::plan_lossy(info, cfg)?.0;
        &planned
    } else {
        cfg
    };
    if aq_on_cfg(cfg) {
        return Err(unsupported("bounded lossy encoding excludes AQ"));
    }
    let layouts = cfg.channel_layouts;
    let full_layouts = layouts == [LossyChannelLayout::default(); 3];
    // As `encode_lossy_frame`.
    if cfg.small_transforms
        && (cfg.decode_speed != Some(crate::DecodeSpeed::Compact)
            || info.sample != SampleType::U8
            || !full_layouts
            || cfg.effort >= 9.0)
    {
        return Err(Error::Unsupported(
            "small transforms require Compact U8 stills with default layouts, effort below 9 and no trellis",
        ));
    }
    #[cfg(feature = "lab")]
    if cfg.rdoq_lambda != 0.0
        || cfg.lf_pred != 0
        || cfg.trellis.is_some()
        || cfg.lab.explicit_matrix.is_some()
        || cfg.lab.recon_bias.is_some()
        || cfg.lab.recon_bias_centroid.is_some()
        || cfg.lab.color_transform.is_some()
        || lab_dc_step(cfg) != [0, 0]
    {
        return Err(unsupported(
            "bounded lossy encoding excludes lab coefficient switches",
        ));
    }
    #[cfg(feature = "lab")]
    if cfg.lab.small.is_some() || cfg.lab.superblocks.is_some() {
        return Err(unsupported(
            "bounded lossy encoding excludes lab superblock and small-transform switches",
        ));
    }
    let kind = InputKind::of(info).ok_or(unsupported("lossy pixel format"))?;
    // As `encode_lossy_frame`: 16-bit input is XYB with a known transfer,
    // else YCoCg on code values in 1/257 units; float input is XYB.
    let xfer16 = if kind == InputKind::U16 && cfg.color == LossyColor::Xyb {
        transfer::Transfer::from_cicp(info.cicp.transfer_characteristics)
    } else {
        None
    };
    let custom_color = precision::custom_color(info);
    if custom_color
        && cfg.color == LossyColor::Xyb
        && transfer::Transfer::from_cicp(info.cicp.transfer_characteristics).is_none()
    {
        return Err(unsupported("unknown transfer for significant-bit XYB"));
    }
    let color = match kind {
        InputKind::U16 if xfer16.is_some() => LossyColor::Xyb,
        InputKind::U16 => LossyColor::YCoCg,
        InputKind::F16 | InputKind::F32 => LossyColor::Xyb,
        _ => cfg.color,
    };
    let wide = kind == InputKind::U16 && color == LossyColor::YCoCg;
    let lut16_transfer = xfer16.filter(|_| !custom_color);
    // Storage and range rules of declared precision (the pixels are checked
    // band by band as they are read).
    let empty = {
        let mut i = info.clone();
        i.height = 0;
        i
    };
    precision::validate_pixels(&empty, &[], 0)?;
    let precision_input = custom_color || kind == InputKind::U32;
    if precision_input {
        precision::Input::validate(info, color)?;
    }
    let quantizer_scale = cfg.scale_for(color, info.width, info.height);
    if !quantizer_scale.is_finite() {
        return Err(Error::InvalidInput("invalid lossy numeric parameter"));
    }
    // Centralized lossy effort policy: superblocks and 4x4 adaptive partitions
    // stay eligible alongside the AC trellis for `DecodeSpeed::Compact` and
    // `adaptive_partitions`, matching `encode_lossy_frame`.
    // Channel layouts turn the trellis, merges, small transforms and exact
    // blocks off, as there.
    let effort_plan = effort.lossy_plan(adaptive_mode(cfg));
    let effort_trellis = trellis::for_effort_plan(effort_plan)
        .filter(|t| quantizer_scale >= t.min_quality)
        .filter(|_| full_layouts);
    let sb_eligible = (effort_trellis.is_none() || adaptive_enabled(cfg))
        && full_layouts
        && (cfg.decode_speed.is_some()
            || matches!(cfg.entropy, LossyEntropy::RunLevel | LossyEntropy::SplitHuf));
    // Small transforms: chosen by the same selector, as in `encode_lossy_frame`.
    let sb_on = superblocks_on(cfg, info, sb_eligible);
    let small = small_on(
        cfg,
        info,
        effort,
        full_layouts
            && (cfg.decode_speed.is_some()
                || matches!(cfg.entropy, LossyEntropy::RunLevel | LossyEntropy::SplitHuf)),
    );
    let trellis = effort_trellis.filter(|_| color == LossyColor::Xyb);
    let merges = (sb_on || small)
        .then(|| partition::build_selector_opts(cfg, effort_plan, sb_on, small, trellis.is_some()));
    if trellis.is_some()
        && (info.width as usize).saturating_mul(info.height as usize) <= SMALL_IMAGE
    {
        return Err(unsupported(
            "small images at trellis efforts compare whole-image codings; use encode_lossy",
        ));
    }
    let matrix = match cfg.matrix {
        m @ 0..=MATRIX_LAST_PRESET => m,
        MATRIX_EXPLICIT => {
            return Err(unsupported(
                "bounded lossy encoding does not write explicit tables",
            ));
        }
        _ if color == LossyColor::Xyb => 2,
        _ => {
            if QParams::from_quality(quantizer_scale, color, 5).valid() {
                5
            } else {
                0
            }
        }
    };
    let mut qp = QParams::from_quality(quantizer_scale, color, matrix);
    if let Some(m) = cfg.channel_mul {
        qp.mul = m;
    }
    qp.apply_ac_scale(cfg.ac_scale)?;
    if !qp.valid() {
        return Err(Error::InvalidInput("invalid quantization parameters"));
    }
    let steps = qp.steps();
    let sb = cfg.stripe_rows_for(info.width);
    let g = Geom::new(info.width, info.height, sb);
    let ci = color_index(info.channels);
    let dc_fix = dc_nearest(cfg);
    // Deringing and white/black DC blocks need full 8x8 luma.
    let full8_luma = layouts[0] == LossyChannelLayout::default();
    let dering_on = dering::enabled(cfg) && full8_luma;
    let dc_rows = dc_stripe_rows(cfg, sb, info.width, info.height);
    let gcols = lossy_group_cols(cfg, g.w);
    let plan = Plan {
        local_ans_contexts: cfg.local_ans_contexts || cfg.model_references,
        coding_defaults: cfg.coding_defaults,
        #[cfg(all(feature = "std", feature = "lab", feature = "rayon"))]
        one_pass_threads: 1,
        sb,
        nci: info.channels.count(),
        ci,
        rb: info.row_bytes(),
        pw: g.bw * 8,
        xyb: color == LossyColor::Xyb,
        color,
        scale: if wide {
            [1.0 / 257.0; 3]
        } else {
            color.default_scales()
        },
        qp,
        steps,
        inv_steps: steps.map(|st| st.map(|v| 1.0 / v)),
        trellis,
        plan: effort_plan,
        cm: dct::dct_matrix(),
        round: cfg.round.clamp(0.0, 0.5),
        dc_fix,
        dering_on,
        dering_cap: 2.0 * steps[0][0],
        white_code: (matches!(kind, InputKind::U8 | InputKind::U16)
            && full8_luma
            && (dering_on || dc_fix))
            .then(|| core::array::from_fn(|c| (1u32 << info.channel_bits(ci[c]).unwrap()) - 1)),
        u8_lin: core::array::from_fn(|i| color::SRGB8_TO_LINEAR[i] as f32),
        dc_rows,
        work: {
            let sb = sb as usize;
            let base = sb * (8 / sb).max(1);
            if dc_rows.is_multiple_of(base) {
                base
            } else {
                sb
            }
        },
        gcols,
        ng: if gcols == 0 { 1 } else { g.w.div_ceil(gcols) },
        kind,
        bps: info.sample.bytes(),
        lut16: Vec::new(),
        input: None,
        info: info.clone(),
        exact: exact::policy(cfg, info, full_layouts),
        layouts,
        layout_steps: core::array::from_fn(|c| {
            [false, true].map(|split| {
                let l = layouts[c].layout(split);
                if l == subblock::Layout::FULL8 {
                    steps[c]
                } else {
                    l.steps(&steps[c])
                }
            })
        }),
        adaptive_mask: layout::adaptive_mask(&layouts),
        merges,
        alpha: if info.channels.has_alpha() {
            let bits = info.channel_bits(info.channels.count() - 1).unwrap();
            Some(
                ImageInfo::new(info.width, info.height, Channels::Gray, info.sample)
                    .with_significant_bits(&[bits])?,
            )
        } else {
            None
        },
        g,
    };
    Ok(NativePreparation {
        plan,
        lut16_transfer,
        precision_input,
    })
}

impl Plan {
    /// Work bands: (first block row, end block row).
    pub(super) fn bands(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        (0..self.g.bh)
            .step_by(self.work)
            .map(|by0| (by0, (by0 + self.work).min(self.g.bh)))
    }

    /// DC stripes span several work bands: their DC records are coded in the
    /// nested pass, once each stripe's DC image is complete.
    pub(super) fn split_dc(&self) -> bool {
        self.work < self.dc_rows
    }

    /// Whether this plan runs the streaming open-loop trellis-lambda AQ model.
    pub(super) fn needs_open_lambda(&self) -> bool {
        self.plan.lambda_aq_open != 0.0 && (self.merges.is_some() || self.trellis.is_some())
    }

    /// Whether this plan runs closed-loop seam residual feedback between
    /// trellis pass 0 and pass 1.
    pub(super) fn needs_closed_lambda(&self) -> bool {
        self.plan.lambda_aq_closed_loop != 0.0 && self.trellis.is_some_and(|t| t.passes >= 2)
    }

    /// Source passes an encode of this plan reads, `(min, max)`, as `pass1`
    /// and `pass2` run them; content decides between the two. Always:
    /// statistics (1) and coding (2), plus exact-block prices (0),
    /// superblock/open-loop prices (4), and optional two-pass pilot passes
    /// (5, 6) when planned. The nested pass (3) always runs
    /// when DC stripes span several work bands; otherwise it runs only when
    /// pass 0's prices select exact blocks. The trellis rate-model passes
    /// (10 + k) depend on pass 1's statistics only when a decode speed whose
    /// tier compares entropy codings leaves split-Huffman possible without
    /// its rate model. `cfg` is the configuration `plan` was given.
    pub(super) fn source_passes(&self, cfg: &LossyConfig) -> (u32, u32) {
        let mut always = 2;
        let mut content = 0;
        if self.exact.is_some() {
            always += 1;
        }
        if self.merges.is_some() {
            always += 1;
            if self.plan.two_pass_pilot {
                always += 1;
                if self.trellis.is_some() {
                    always += 1;
                }
            }
        } else if self.needs_open_lambda() {
            always += 1;
        }
        if self.split_dc() {
            always += 1;
        } else if self.exact.is_some() {
            content += 1;
        }
        if let Some(set) = self.trellis {
            let n = u32::from(set.passes);
            // `pass1` skips the trellis on split-Huffman files without the
            // split-Huffman rate model; `None`: pass 1 measures the coding.
            let split_huf = match cfg.decode_speed {
                None => Some(cfg.entropy == LossyEntropy::SplitHuf),
                Some(speed) => huf_max_ratio(speed).is_none().then_some(false),
            };
            match split_huf {
                _ if trellis::huf_rate(cfg) => always += n,
                Some(false) => always += n,
                Some(true) => {}
                None => content += n,
            }
        }
        (always, always + content)
    }

    /// Whether band `[_, by1)` completes a DC stripe.
    pub(super) fn ends_dc_stripe(&self, by1: usize) -> bool {
        by1.is_multiple_of(self.dc_rows) || by1 == self.g.bh
    }

    /// Source pixel rows a band of block rows `[by0, by1)` reads.
    pub(super) fn band_rows(&self, by0: usize, by1: usize) -> (usize, usize) {
        (by0 * 8, (by1 * 8).min(self.g.h))
    }

    /// Block row `by`'s color-converted (and deringed) planes for every
    /// column, `NCH * 8 * pw` samples, plane-major, from `band` (source rows
    /// `y0..`), and its white/black blocks: `fill_block_row` of the
    /// whole-image encoder, which the trellis recomputes rows with.
    pub(super) fn fill_block_row(
        &self,
        band: &[u8],
        y0: usize,
        by: usize,
        planes: &mut [f32],
        extremes: &mut Vec<(u64, bool)>,
    ) {
        self.fill_block_row_columns(band, y0, by, 0..self.g.bw, planes, extremes);
    }

    pub(super) fn fill_block_row_columns(
        &self,
        band: &[u8],
        y0: usize,
        by: usize,
        columns: core::ops::Range<usize>,
        planes: &mut [f32],
        extremes: &mut Vec<(u64, bool)>,
    ) {
        let g = &self.g;
        let x0 = columns.start * 8;
        let w = (columns.end * 8).min(g.w) - x0;
        let pw = (columns.end - columns.start) * 8;
        for y in 0..8 {
            let sy = (by * 8 + y).min(g.h - 1) - y0;
            let row = &band[sy * self.rb + x0 * self.nci * self.bps..][..w * self.nci * self.bps];
            let (n, ci) = (self.nci, self.ci);
            match self.kind {
                InputKind::U8 if self.input.is_some() => {
                    let input = self.input.as_ref().unwrap();
                    fill_color_row::<1>(row, planes, pw, y, w, n, ci, |c, b| {
                        input.read(c, b[0] as u32)
                    })
                }
                InputKind::U16 if self.input.is_some() => {
                    let input = self.input.as_ref().unwrap();
                    fill_color_row::<2>(row, planes, pw, y, w, n, ci, |c, b| {
                        input.read(c, u16::from_ne_bytes(*b) as u32)
                    })
                }
                InputKind::U8 if self.xyb => {
                    let lin = &self.u8_lin;
                    fill_color_row::<1>(row, planes, pw, y, w, n, ci, |_, b| lin[b[0] as usize])
                }
                InputKind::U8 => {
                    fill_color_row::<1>(row, planes, pw, y, w, n, ci, |_, b| b[0] as f32)
                }
                InputKind::U16 if !self.lut16.is_empty() => {
                    let lut = &self.lut16;
                    fill_color_row::<2>(row, planes, pw, y, w, n, ci, |_, b| {
                        lut[u16::from_ne_bytes(*b) as usize]
                    })
                }
                InputKind::U16 => fill_color_row::<2>(row, planes, pw, y, w, n, ci, |_, b| {
                    u16::from_ne_bytes(*b) as f32
                }),
                InputKind::F16 => fill_color_row::<2>(row, planes, pw, y, w, n, ci, |_, b| {
                    finite(f32::from_bits(crate::mapping::f16_to_f32_bits(
                        u16::from_ne_bytes(*b),
                    )))
                }),
                InputKind::F32 => fill_color_row::<4>(row, planes, pw, y, w, n, ci, |_, b| {
                    finite(f32::from_ne_bytes(*b))
                }),
                InputKind::U32 => {
                    let input = self.input.as_ref().unwrap();
                    fill_color_row::<4>(row, planes, pw, y, w, n, ci, |c, b| {
                        input.read(c, u32::from_ne_bytes(*b))
                    })
                }
            }
        }
        let (p0, rest) = planes.split_at_mut(8 * pw);
        let (p1, p2) = rest.split_at_mut(8 * pw);
        enc_simd::color_rows(
            self.xyb,
            self.scale,
            &color::OPSIN_F32,
            color::OPSIN_BIAS,
            color::CBRT_BIAS,
            p0,
            p1,
            p2,
        );
        if let Some(white_code) = &self.white_code {
            // `block_extremes` clamps rows at its geometry's height: a
            // band-local geometry clamps at the band end, which is the image
            // end for the last band and never reached by any other.
            let band_g = Geom::new(w as u32, (band.len() / self.rb) as u32, self.sb);
            block_extremes(
                &band_g,
                &band[x0 * self.nci * self.bps..],
                self.rb,
                by * 8 - y0,
                self.nci,
                self.ci,
                self.kind,
                white_code,
                extremes,
            );
            if self.dering_on {
                dering_row(p0, pw, extremes, self.dering_cap);
            }
        }
    }

    /// Lossy stripe `s` quantized from `band` (rows `y0..` of the source,
    /// `rb` bytes each), as the whole-image encoder's pass 1 does.
    pub(super) fn stripe(&self, band: &[u8], y0: usize, s: usize) -> StripeCoefs {
        self.stripe_with_aq(band, y0, s, None)
    }

    pub(super) fn stripe_with_aq(
        &self,
        band: &[u8],
        y0: usize,
        s: usize,
        aq_code: Option<u8>,
    ) -> StripeCoefs {
        #[cfg(all(feature = "std", feature = "lab", feature = "rayon"))]
        if self.one_pass_threads > 1 && self.g.bw > 32 {
            return one_pass::parallel_stripe(self, band, y0, s, self.one_pass_threads, aq_code);
        }
        let g = &self.g;
        let pw = self.pw;
        let (by0, by1) = g.rows(s);
        let nby = by1 - by0;
        let mut q = vec![0i32; nby * g.bw * NCH * 64];
        let mut nz = vec![0u64; nby * g.bw * NCH];
        let mut planes = vec![0.0f32; NCH * 8 * pw];
        let mut extremes: Vec<(u64, bool)> = Vec::with_capacity(g.bw);
        let mut coefs = vec![0.0f32; g.bw * 64];
        // Per parent, the planes coded split (adaptive layouts only).
        let mut modes = vec![
            0u8;
            if self.adaptive_mask != 0 {
                nby * g.bw
            } else {
                0
            }
        ];
        let aq: Vec<f32> = aq_code.map_or_else(Vec::new, |a| vec![a as f32 / 64.0; g.bw]);
        for r in 0..nby {
            self.fill_block_row(band, y0, by0 + r, &mut planes, &mut extremes);
            let qr = &mut q[r * g.bw * NCH * 64..][..g.bw * NCH * 64];
            let nr = &mut nz[r * g.bw * NCH..][..g.bw * NCH];
            for ch in 0..NCH {
                let layout = self.layouts[ch];
                if layout.layout(false) == subblock::Layout::FULL8 {
                    enc_simd::fdct_row(
                        &planes[ch * 8 * pw..][..8 * pw],
                        pw,
                        g.bw,
                        &self.cm,
                        &mut coefs,
                    );
                }
                if layout == LossyChannelLayout::default() {
                    enc_simd::quant_row(
                        &coefs,
                        g.bw,
                        ch,
                        &self.inv_steps[ch],
                        &aq,
                        self.round,
                        qr,
                        nr,
                    );
                    if self.dc_fix {
                        let ext = (ch == 0 && self.white_code.is_some()).then_some(&extremes[..]);
                        dc_row(&coefs, g.bw, ch, self.inv_steps[ch][0], ext, qr, nr);
                    }
                } else {
                    // As `encode_lossy_frame`: the row's transforms (and per
                    // parent choices) from the plane rows.
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
                        layout,
                        &self.layout_steps[ch],
                        self.steps[ch][1],
                        &coefs,
                        &aq,
                        self.round,
                        if self.dc_fix { 0.5 } else { self.round },
                        qr,
                        nr,
                        mr,
                    );
                }
            }
        }
        StripeCoefs {
            by0,
            nby,
            bw: g.bw,
            q,
            nz,
            aq: aq_code.map_or_else(Vec::new, |a| vec![a; nby * g.bw]),
            modes,
            splits: Vec::new(),
        }
    }

    /// Applies trellis `passes` in order to one stripe's units (its column
    /// groups), recomputing rows from `band`; the last pass emits its
    /// split-Huffman frequencies into `freqs`, and pass 0 optionally feeds
    /// `closed_stream` before restoring exact-block wire codes.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn rewrite(
        &self,
        cfg: &LossyConfig,
        passes: &[trellis::Pass],
        units: &mut [StripeCoefs],
        band: &[u8],
        y0: usize,
        open_lambda: &[f32],
        closed_lambda: &[f32],
        lambda_by0: usize,
        mut freqs: Option<&mut trellis::Freqs>,
        closed_stream: Option<&mut adaptive_rdo::ClosedLoopLambdaStream>,
        stop: &dyn how_far::Stop,
    ) -> Result<(), Error> {
        let Some(set) = &self.trellis else {
            return Ok(());
        };
        let rows = |by: usize, planes: &mut [f32], extremes: &mut Vec<(u64, bool)>| {
            self.fill_block_row(band, y0, by, planes, extremes)
        };
        // Exact masks use 0 = lossy, 1 = exact; trellis block codes use 64 =
        // ordinary quantization, 0 = keep (as `finish_lossy_frame`).
        exact_aq_to_selector(units);
        let last = passes.len().saturating_sub(1);
        for (k, pass) in passes.iter().enumerate() {
            let lmap = if k == 0 || closed_lambda.is_empty() {
                open_lambda
            } else {
                closed_lambda
            };
            let rewrite_args = trellis::StripeRewrite {
                rows: &rows,
                pw: self.pw,
                image_bw: self.g.bw,
                group_bw: self.gcols / 8,
                steps: &self.steps,
                cfg,
                set,
                aq: &[],
                lambda_map: lmap,
                lambda_by0,
                merged_trellis: self.plan.merged_trellis,
                bias: None,
                pass_index: k,
            };
            trellis::rewrite_stripe_lambda(
                pass,
                units,
                &rewrite_args,
                if k == last {
                    freqs.as_deref_mut()
                } else {
                    None
                },
                stop,
            )?;
        }
        if let Some(stream) = closed_stream {
            let st = adaptive_rdo::closed_loop_stripe_errors(
                stream.ctx(),
                units,
                &rows,
                self.pw,
                self.g.bw,
                self.gcols / 8,
                stop,
            )?;
            stream.push_stripe(st)?;
        }
        exact_aq_to_wire(units);
        Ok(())
    }

    /// Superblock merges of `stripes` leaving `sc.aq` in internal selector /
    /// trellis form (`64 = ordinary, 0 = exact/keep`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn merge_internal(
        &self,
        sel: &superblock::select::Selector,
        mut stripes: Vec<StripeCoefs>,
        band: &[u8],
        y0: usize,
        open_lambda: &[f32],
        lambda_by0: usize,
        threads: usize,
        stop: &dyn how_far::Stop,
    ) -> Result<Vec<StripeCoefs>, Error> {
        let rows = |by: usize, planes: &mut [f32], extremes: &mut Vec<(u64, bool)>| {
            self.fill_block_row(band, y0, by, planes, extremes)
        };
        exact_aq_to_selector(&mut stripes);
        crate::par::par_map(stripes, threads, |_, sc| {
            sel.stripe_with_lambda(sc, &rows, open_lambda, lambda_by0, stop)
        })
        .into_iter()
        .collect()
    }

    /// Superblock merges of `stripes` (from `band`, rows `y0..`) under
    /// `sel`'s prices, as `encode_lossy_frame` applies them after marking
    /// exact blocks: exact parents take block code 0 (never merged) for the
    /// choice and get their codes back after it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn merge(
        &self,
        sel: &superblock::select::Selector,
        stripes: Vec<StripeCoefs>,
        band: &[u8],
        y0: usize,
        open_lambda: &[f32],
        lambda_by0: usize,
        threads: usize,
        stop: &dyn how_far::Stop,
    ) -> Result<Vec<StripeCoefs>, Error> {
        let mut stripes = self.merge_internal(
            sel,
            stripes,
            band,
            y0,
            open_lambda,
            lambda_by0,
            threads,
            stop,
        )?;
        exact_aq_to_wire(&mut stripes);
        Ok(stripes)
    }

    /// The full-width stripes of band `[by0, by1)` from `band` (rows `y0..`).
    pub(super) fn band_stripes(
        &self,
        band: &[u8],
        y0: usize,
        by0: usize,
        by1: usize,
    ) -> Vec<StripeCoefs> {
        (by0 / self.sb as usize..by1.div_ceil(self.sb as usize))
            .map(|s| self.stripe(band, y0, s))
            .collect()
    }

    /// The coding units of stripe `sc`, in column-group order.
    pub(super) fn units(&self, sc: StripeCoefs) -> Vec<StripeCoefs> {
        if self.ng == 1 {
            alloc::vec![sc]
        } else {
            split_units(sc, self.gcols / 8)
        }
    }
}

impl Plan {
    /// The nested DC image and its group geometry (groups as wide as the
    /// lossy column groups, one DC stripe band per group row).
    pub(super) fn dc_layout(&self, fits16: bool, codes: bool) -> (ImageInfo, GroupEncodeConfig) {
        let info = ImageInfo::new(
            self.g.bw as u32,
            self.g.bh as u32,
            if codes { Channels::Rgba } else { Channels::Rgb },
            if fits16 {
                SampleType::U16
            } else {
                SampleType::I32
            },
        );
        let cols = if self.gcols == 0 {
            self.g.bw as u32
        } else {
            (self.gcols / 8) as u32
        };
        (
            info,
            GroupEncodeConfig::new(cols, self.dc_rows as u32, usize::MAX),
        )
    }

    /// The alpha image's group geometry: one group row per lossy stripe,
    /// groups as wide as the lossy column groups.
    pub(super) fn alpha_cfg(&self) -> GroupEncodeConfig {
        let cols = if self.gcols == 0 {
            self.g.w
        } else {
            self.gcols
        };
        GroupEncodeConfig::new(cols as u32, self.sb * 8, usize::MAX)
    }

    /// Lossy stripe `s`'s alpha samples (rows of the image width) from `band`
    /// (source rows `y0..`); returns the row count.
    pub(super) fn alpha_plane(&self, band: &[u8], y0: usize, s: usize, out: &mut Vec<u8>) -> usize {
        let (by0, by1) = self.g.rows(s);
        let (r0, r1) = (by0 * 8, (by1 * 8).min(self.g.h));
        out.clear();
        for y in r0..r1 {
            let row = &band[(y - y0) * self.rb..][..self.rb];
            for px in row.chunks_exact(self.nci * self.bps) {
                out.extend_from_slice(&px[(self.nci - 1) * self.bps..]);
            }
        }
        r1 - r0
    }
}

/// The nested DC groups' decorrelation and predictor masks: the effort's,
/// as the whole-image lossless encoder narrows its defaults.
pub(super) fn dc_masks(cfg: &LossyConfig) -> Result<(u8, u8), Error> {
    let effort = crate::effort::Effort::new(cfg.effort)?;
    let d = crate::EncodeConfig::default();
    Ok((
        effort.decorrelations(d.decor_mask & 0x0f),
        effort.predictors(d.predictor_mask),
    ))
}

/// Encodes a lossy imij from rows the caller supplies band by band.
/// `rows(pass, y0, out)` fills `out` with source rows `y0..y0 + out.len() /
/// row_bytes` (tightly packed); each pass requests every band once, top to
/// bottom. Codec working storage holds one work band's rows and
/// coefficients at a time; the encoded file is returned whole, with the
/// unit table in this thread's form (the head hint by default), the `DATA`
/// locator and, when configured, the file checksum.
/// An upper bound on the heap the bounded encoder holds for `info` and
/// `cfg`, which the common-job adapter admits against `max_memory_bytes`;
/// also refuses what [`plan`] refuses. Terms: the work band's source rows,
/// its coefficients (`i32` per coefficient plus a nonzero mask per block and
/// channel) held for the band, its units and the coded records, the DC
/// image of a DC stripe that spans several bands, a further copy of the
/// coefficients for exact-block and superblock selection, and a fixed 8 MiB
/// for tables, histograms and record buffers. Checked against heap peaks in
/// `bounded_working_bytes_covers_the_heap_peak` (benchmarks
/// `bounded_lossy_2026-10-17.md`).
pub(crate) fn bounded_working_bytes(info: &ImageInfo, cfg: &LossyConfig) -> Result<u64, Error> {
    plan_working_bytes(&prepare_plan(info, cfg)?.plan)
}

pub(super) fn plan_working_bytes(p: &Plan) -> Result<u64, Error> {
    // Widen before products: admission must not wrap on 32-bit hosts.
    let rows = p.work as u128 * 8;
    let raster = rows * p.rb as u128;
    let blocks = p.work as u128 * p.g.bw as u128;
    let coefs = blocks * NCH as u128 * (64 * 4 + 8);
    let dc = if p.split_dc() {
        p.dc_rows as u128 * p.g.bw as u128 * 16
    } else {
        0
    };
    let select = if p.exact.is_some() || p.merges.is_some() {
        2 * coefs
    } else {
        0
    };
    let trellis = if p.trellis.is_some() { coefs } else { 0 };
    let local = if p.local_ans_contexts {
        8 * coefs + (8 << 20)
    } else {
        0
    };
    let defaults = if p.coding_defaults { blocks * 32 } else { 0 };
    let open_lambda = if p.needs_open_lambda() {
        let n_bands = p.g.bh.div_ceil(p.work.max(1)) as u128;
        // Boundary first-row cache + 3-row stream buffers + band cursor (`min_q`,
        // `mean_q`, `sat_smooth`, `open_band`, `closed_band`, row planes).
        n_bands * p.g.bw as u128 * 4 + blocks * 20 + p.g.bw as u128 * (32 + NCH as u128 * 32 * 8)
    } else {
        0
    };
    let closed_lambda = if p.needs_closed_lambda() {
        let nb = p.g.bw as u128 * p.g.bh as u128;
        let store = if nb <= adaptive_rdo::CLOSED_LOOP_EXACT_MAX_BLOCKS as u128 {
            nb * 8
        } else {
            nb
        };
        store + blocks * 24 + p.g.bw as u128 * (NCH as u128 * 32 * 12)
    } else {
        0
    };
    let dc_rdo = if p.plan.dc_rdo && p.trellis.is_some() {
        // Exact single-DC-stripe accumulator (`q`/`raw_c`/`ac_norm` 36 B +
        // `base_lock`/`ext_lock`/`lambda`/`aq_code` 10 B = 46 B/block) plus
        // `optimize_dc_stripe` live `orig` buffer (4 B/block) = 50 B/block
        // (budgeted at 52 B/block), plus row-width scratch (`planes` 768 B/col
        // + `extremes` 16 B/col + Viterbi/rate-model row buffers 19 B/col =
        // 803 B/col, budgeted at 832 B/col).
        let stripe_blocks = p.dc_rows.min(p.g.bh) as u128 * p.g.bw as u128;
        stripe_blocks * 52 + p.g.bw as u128 * 832
    } else {
        0
    };
    u64::try_from(
        2 * raster
            + 3 * coefs
            + dc
            + dc_rdo
            + select
            + trellis
            + local
            + defaults
            + open_lambda
            + closed_lambda
            + (8 << 20),
    )
    .map_err(|_| Error::TooLarge)
}
