//! Two-pass lossy encoding with working storage bounded by one work band of
//! about 8 block rows, full width, plus the current DC stripe's DC image
//! (issue #14). The caller supplies the source rows of each band twice,
//! top to bottom: pass 1 measures what the head must state before the first
//! unit record (DC range, the tier's side statistics, entropy histograms);
//! pass 2 recomputes each band's coefficients and codes its records.
//!
//! The quantized coefficients are computed exactly as the whole-image
//! encoder's pass 1 computes them, so a file decodes to the same pixels as
//! [`super::encode_lossy`] with the same configuration. The coding choices
//! the whole-image encoder makes by comparing complete candidates are made
//! explicitly here and are lossless: explicit AC tables (no seed bank),
//! no compact restart states, the tier rule for the entropy mode at every
//! image size, and a nested DC image coded in the bounded lossless group
//! grammar with tables measured in pass 1, each group searching its own
//! predictors and decorrelation within the effort's masks.
//!
//! Supported: Gray, GrayAlpha, RGB and RGBA of u8, u16, u32, f16 or f32
//! samples (16-bit as XYB with a known transfer, else YCoCg; float as XYB;
//! declared significant bits and u32 through the whole-image encoder's
//! precision conversion, each band range-checked as it is read), channel
//! layouts (row-local), small transforms and superblock merges (chosen by
//! the selector under pass 4's prices), a decoder memory budget (planned
//! from the geometry), no AQ, no inter reference.
//! Efforts 9 and 10 add one source pass per AC
//! trellis rate-model pass (XYB images above 64 Ki pixels; smaller ones are
//! refused, as the whole-image encoder compares candidate codings there).
//! Exact blocks add two: pass 0 measures their prices over the whole image
//! (`exact::estimate_bits`' run-level tables and `exact::select_mask`'s
//! residual costs), and after the trellis passes a pass codes the nested
//! exact image's stripes, which the caller stores and pass 2 places in the
//! records of the units holding their first blocks. Every pass chooses the
//! same blocks, each lane's EOB context carried across bands. Superblock
//! merges (the Compact tier) add one: pass 4 measures the selector's prices
//! from the units' token counts before any merge, and passes 1 and 2 run the
//! selection stripe by stripe (`superblock::select::Selector`). Anything
//! else is refused with `Error::Unsupported`; nothing requested is dropped.
//!
//! The public entry point is `stream_encode::encode_lossy_bounded` (`std`);
//! the in-memory form below is the tests' oracle path.
#![cfg_attr(not(feature = "std"), allow(dead_code))]

use super::*;
use crate::stream_encode::{GroupEncodeConfig, NativeGroupEncodeSession as GroupEncodeSession};

#[cfg(all(feature = "std", feature = "lab"))]
pub(crate) mod one_pass;

#[cfg(all(test, feature = "std", feature = "lab"))]
mod coding_context_experiment;

/// Everything both passes share: the head's parameters and the geometry.
struct Plan {
    #[cfg(all(feature = "std", feature = "lab", feature = "rayon"))]
    one_pass_threads: usize,
    local_ans_contexts: bool,
    coding_defaults: bool,
    g: Geom,
    sb: u32,
    nci: usize,
    ci: [usize; 3],
    rb: usize,
    pw: usize,
    xyb: bool,
    color: LossyColor,
    scale: [f32; 3],
    qp: QParams,
    steps: [[f32; 64]; 3],
    inv_steps: [[f32; 64]; 3],
    /// The AC trellis this effort runs (XYB only, as `finish_lossy_frame`).
    trellis: Option<trellis::LossyTrellis>,
    /// Centralized lossy effort plan (`effort.rs`).
    plan: crate::effort::LossyEffortPlan,
    cm: [[f32; 8]; 8],
    round: f32,
    dc_fix: bool,
    dering_on: bool,
    dering_cap: f32,
    white_code: Option<[u32; 3]>,
    u8_lin: [f32; 256],
    /// Block rows per DC stripe (a whole number of lossy stripes).
    dc_rows: usize,
    /// Block rows per work band: whole lossy stripes, about 8 block rows,
    /// dividing `dc_rows`. Tiers with taller DC stripes (Balanced, Compact,
    /// none) gather each DC stripe's DC image over several bands.
    work: usize,
    gcols: usize,
    ng: usize,
    /// Alpha: a nested lossless gray image, one stripe per lossy stripe.
    alpha: Option<ImageInfo>,
    kind: InputKind,
    /// Sample bytes.
    bps: usize,
    /// Code value -> linear light for 16-bit XYB (empty otherwise).
    lut16: Vec<f32>,
    /// Declared significant bits or u32 samples: the whole-image encoder's
    /// conversion (`precision::Input`), and every band is range-checked.
    input: Option<precision::Input>,
    info: ImageInfo,
    /// The exact-block policy (8-bit RGB(A)): blocks are chosen band by band
    /// under prices a statistics pass measures over the whole image.
    exact: Option<exact::Policy>,
    /// Superblock merges (Compact): parents chosen stripe by stripe under
    /// prices a statistics pass measures over the whole image.
    merges: Option<superblock::select::Opts>,
    /// Channel layouts (row-local transforms and sampling per plane), their
    /// quantizer steps (8x8 and split), and the planes whose transform is
    /// chosen per parent.
    layouts: [LossyChannelLayout; 3],
    layout_steps: [[[f32; 64]; 2]; 3],
    adaptive_mask: u8,
}

// A validated plan without sample conversion tables. Only materialize() makes
// it available to the coding passes; admission can retain this exact plan.
struct NativePreparation {
    plan: Plan,
    lut16_transfer: Option<transfer::Transfer>,
    precision_input: bool,
}

impl NativePreparation {
    fn materialize(mut self) -> Result<Plan, Error> {
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

fn plan(info: &ImageInfo, cfg: &LossyConfig) -> Result<Plan, Error> {
    prepare_plan(info, cfg)?.materialize()
}

fn prepare_plan(info: &ImageInfo, cfg: &LossyConfig) -> Result<NativePreparation, Error> {
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
    fn bands(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        (0..self.g.bh)
            .step_by(self.work)
            .map(|by0| (by0, (by0 + self.work).min(self.g.bh)))
    }

    /// DC stripes span several work bands: their DC records are coded in the
    /// nested pass, once each stripe's DC image is complete.
    fn split_dc(&self) -> bool {
        self.work < self.dc_rows
    }

    /// Whether this plan runs the streaming open-loop trellis-lambda AQ model.
    fn needs_open_lambda(&self) -> bool {
        self.plan.lambda_aq_open != 0.0 && (self.merges.is_some() || self.trellis.is_some())
    }

    /// Whether this plan runs closed-loop seam residual feedback between
    /// trellis pass 0 and pass 1.
    fn needs_closed_lambda(&self) -> bool {
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
    fn source_passes(&self, cfg: &LossyConfig) -> (u32, u32) {
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
    fn ends_dc_stripe(&self, by1: usize) -> bool {
        by1.is_multiple_of(self.dc_rows) || by1 == self.g.bh
    }

    /// Source pixel rows a band of block rows `[by0, by1)` reads.
    fn band_rows(&self, by0: usize, by1: usize) -> (usize, usize) {
        (by0 * 8, (by1 * 8).min(self.g.h))
    }

    /// Block row `by`'s color-converted (and deringed) planes for every
    /// column, `NCH * 8 * pw` samples, plane-major, from `band` (source rows
    /// `y0..`), and its white/black blocks: `fill_block_row` of the
    /// whole-image encoder, which the trellis recomputes rows with.
    fn fill_block_row(
        &self,
        band: &[u8],
        y0: usize,
        by: usize,
        planes: &mut [f32],
        extremes: &mut Vec<(u64, bool)>,
    ) {
        self.fill_block_row_columns(band, y0, by, 0..self.g.bw, planes, extremes);
    }

    fn fill_block_row_columns(
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
    fn stripe(&self, band: &[u8], y0: usize, s: usize) -> StripeCoefs {
        self.stripe_with_aq(band, y0, s, None)
    }

    fn stripe_with_aq(&self, band: &[u8], y0: usize, s: usize, aq_code: Option<u8>) -> StripeCoefs {
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
    fn rewrite(
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
    fn merge_internal(
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
    fn merge(
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
    fn band_stripes(&self, band: &[u8], y0: usize, by0: usize, by1: usize) -> Vec<StripeCoefs> {
        (by0 / self.sb as usize..by1.div_ceil(self.sb as usize))
            .map(|s| self.stripe(band, y0, s))
            .collect()
    }

    /// The coding units of stripe `sc`, in column-group order.
    fn units(&self, sc: StripeCoefs) -> Vec<StripeCoefs> {
        if self.ng == 1 {
            alloc::vec![sc]
        } else {
            split_units(sc, self.gcols / 8)
        }
    }
}

#[cfg(all(feature = "std", feature = "lab"))]
pub(crate) fn component_one_pass_records(
    info: &ImageInfo,
    cfg: &LossyConfig,
) -> Result<usize, Error> {
    Ok(one_pass::configure(plan(info, cfg)?, cfg)?.record_count(false))
}

/// Exact-block selection across bands (`exact::estimate_bits` and
/// `exact::select_mask` of the whole image): the run-level token and
/// residual prices a statistics pass measured, and each lane's luma EOB
/// context, carried from band to band.
/// What the bounded passes need from outside: source rows, the records the
/// nested pass spools for pass 2, and pass 2's output. Async, so one core
/// serves blocking callers (every call completes at once, driven by
/// `parse::payload::ready`) and a sans-IO session (a call waits until the
/// caller answers).
trait PassIo {
    fn stop(&self) -> &dyn how_far::Stop {
        &how_far::NoPulse
    }
    /// Source rows of block rows `[by0, by1)` into `band` (resized) for
    /// `pass`; returns their first row.
    async fn rows(
        &mut self,
        pass: u8,
        by0: usize,
        by1: usize,
        band: &mut Vec<u8>,
    ) -> Result<usize, Error>;
    /// Keeps a record the nested pass coded until pass 2 asks for it.
    async fn store<'a>(&'a mut self, kind: Spooled, rec: &'a [u8]) -> Result<(), Error>;
    /// Record `t` of `kind` (in store order) into `buf`.
    async fn load(&mut self, kind: Spooled, t: usize, buf: &mut Vec<u8>) -> Result<(), Error>;
    /// One main record of pass 2, in file order.
    async fn emit(&mut self, id: u64, payload: &[u8]) -> Result<(), Error>;
    /// The completed native choices are available before this band is released.
    /// Legacy writers need only the byte payload; V4 serialization also uses
    /// this view to avoid decoding its just-encoded transform map.
    async fn emit_ac(&mut self, payload: &[u8], _stripe: &StripeCoefs) -> Result<(), Error> {
        self.emit(crate::parse::records::R_AC, payload).await
    }
}

/// [`PassIo`] over blocking callbacks.
#[cfg(all(test, feature = "encode"))]
struct Callbacks<'a, F, S, L, E> {
    rows: &'a mut F,
    store: S,
    load: L,
    emit: E,
}

#[cfg(all(test, feature = "encode"))]
impl<F, S, L, E> PassIo for Callbacks<'_, F, S, L, E>
where
    F: FnMut(u8, usize, usize, &mut Vec<u8>) -> Result<usize, Error>,
    S: FnMut(Spooled, &[u8]) -> Result<(), Error>,
    L: FnMut(Spooled, usize, &mut Vec<u8>) -> Result<(), Error>,
    E: FnMut(u64, &[u8]) -> Result<(), Error>,
{
    async fn rows(
        &mut self,
        pass: u8,
        by0: usize,
        by1: usize,
        band: &mut Vec<u8>,
    ) -> Result<usize, Error> {
        (self.rows)(pass, by0, by1, band)
    }
    async fn store<'a>(&'a mut self, kind: Spooled, rec: &'a [u8]) -> Result<(), Error> {
        (self.store)(kind, rec)
    }
    async fn load(&mut self, kind: Spooled, t: usize, buf: &mut Vec<u8>) -> Result<(), Error> {
        (self.load)(kind, t, buf)
    }
    async fn emit(&mut self, id: u64, payload: &[u8]) -> Result<(), Error> {
        (self.emit)(id, payload)
    }
}

/// A [`Callbacks`] hook a pass does not use.
#[cfg(all(test, feature = "encode"))]
fn no_store(_: Spooled, _: &[u8]) -> Result<(), Error> {
    Err(Error::Corrupt(
        "bounded lossy: store outside the nested pass",
    ))
}
#[cfg(all(test, feature = "encode"))]
fn no_load(_: Spooled, _: usize, _: &mut Vec<u8>) -> Result<(), Error> {
    Err(Error::Corrupt("bounded lossy: load outside pass 2"))
}
#[cfg(all(test, feature = "encode"))]
fn no_emit(_: u64, _: &[u8]) -> Result<(), Error> {
    Err(Error::Corrupt("bounded lossy: record outside pass 2"))
}

struct Select {
    pol: exact::Policy,
    tokens: Vec<[f32; coef::ALPHA]>,
    proxy: Vec<[f32; 1024]>,
    lanes: Vec<u32>,
}

impl Select {
    /// Measures the prices: one source pass (pass 0).
    async fn measure<I: PassIo>(
        p: &Plan,
        pol: exact::Policy,
        threads: usize,
        io: &mut I,
    ) -> Result<Self, Error> {
        let mode = LossyEntropy::RunLevel;
        let mut lanes = vec![0u32; NL.min(p.g.bw)];
        let mut hist = vec![0u64; mode.nraw() * coef::ALPHA];
        let mut proxy = vec![[0u64; 1024]; 3];
        let mut band = Vec::new();
        for (by0, by1) in p.bands() {
            let y0 = io.rows(0, by0, by1, &mut band).await?;
            let stripes = p.band_stripes(&band, y0, by0, by1);
            for (l, prev) in lanes.iter_mut().enumerate() {
                exact::lane_hist(&stripes, l, prev, &mut hist);
            }
            let (_, y1) = p.band_rows(by0, by1);
            exact::proxy_hist(&band, p.rb, p.nci, p.g.w, y1 - y0, threads, &mut proxy);
        }
        let hist: Vec<Vec<u64>> = hist.chunks(coef::ALPHA).map(|h| h.to_vec()).collect();
        lanes.fill(0);
        Ok(Self {
            pol,
            tokens: exact::token_costs(&hist),
            proxy: exact::proxy_costs_of(&proxy),
            lanes,
        })
    }

    /// Restarts the lanes for another pass over the image.
    fn begin(&mut self) {
        self.lanes.fill(0);
    }

    /// The exact blocks of band `[by0, by1)` (`stripes`, from `band`, rows
    /// `y0..`), band-local raster order; marks them in `stripes`.
    #[allow(clippy::too_many_arguments)]
    fn band(
        &mut self,
        p: &Plan,
        stripes: &mut [StripeCoefs],
        band: &[u8],
        y0: usize,
        by0: usize,
        by1: usize,
        threads: usize,
    ) -> Vec<bool> {
        let bw = p.g.bw;
        let mut bits = vec![0.0f32; (by1 - by0) * bw];
        for (l, prev) in self.lanes.iter_mut().enumerate() {
            for (b, v) in exact::lane_bits(stripes, l, prev, &self.tokens) {
                bits[b as usize - by0 * bw] = v;
            }
        }
        let (_, y1) = p.band_rows(by0, by1);
        let mask = exact::select_with(
            band,
            p.rb,
            p.nci,
            p.g.w,
            y1 - y0,
            &bits,
            &self.proxy,
            &self.pol,
            threads,
        );
        exact::mark_from(stripes, &mask, by0);
        mask
    }
}

/// The exact blocks' source pixels packed into the nested image's stripes in
/// list order (unit by unit, raster order inside a unit), as `exact::encode`
/// packs them: B blocks per packed row, partial edge blocks repeating the
/// image edge, the last packed row zero padded.
struct Packer {
    b: usize,
    /// Blocks per nested stripe.
    per: usize,
    buf: Vec<u8>,
    n: usize,
    /// Exact blocks so far.
    total: usize,
}

impl Packer {
    fn new(p: &Plan, pol: &exact::Policy) -> Self {
        let b = p.g.bw.min(exact::ROW_BLOCKS);
        Self {
            b,
            per: pol.stripe_rows / 8 * b,
            buf: vec![0u8; 8 * b * pol.stripe_rows * 3],
            n: 0,
            total: 0,
        }
    }

    /// Packed-image row bytes.
    fn stride(&self) -> usize {
        8 * self.b * 3
    }

    /// Adds the `mask`ed blocks of band `[by0, by1)` (`band`, rows `y0..`)
    /// unit by unit, pushing each unit's count to `counts` and handing every
    /// complete stripe (pixels, rows) to `full`.
    #[allow(clippy::too_many_arguments)]
    fn band(
        &mut self,
        p: &Plan,
        band: &[u8],
        y0: usize,
        by0: usize,
        by1: usize,
        mask: &[bool],
        counts: &mut Vec<usize>,
        full: &mut dyn FnMut(&[u8], usize) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let (w, h, bw) = (p.g.w, p.g.h, p.g.bw);
        let gb = if p.gcols == 0 { bw } else { p.gcols / 8 };
        let sb = p.sb as usize;
        for s in by0 / sb..by1.div_ceil(sb) {
            for gi in 0..p.ng {
                let before = self.total;
                for by in s * sb..((s + 1) * sb).min(p.g.bh) {
                    for bx in gi * gb..((gi + 1) * gb).min(bw) {
                        if !mask[(by - by0) * bw + bx] {
                            continue;
                        }
                        let (ox, oy) = ((self.n % self.b) * 8, (self.n / self.b) * 8);
                        let stride = self.stride();
                        for dy in 0..8 {
                            let y = (by * 8 + dy).min(h - 1) - y0;
                            for dx in 0..8 {
                                let x = (bx * 8 + dx).min(w - 1);
                                let o = (oy + dy) * stride + (ox + dx) * 3;
                                self.buf[o..o + 3]
                                    .copy_from_slice(&band[y * p.rb + x * p.nci..][..3]);
                            }
                        }
                        self.n += 1;
                        self.total += 1;
                        if self.n == self.per {
                            full(&self.buf, self.per / self.b * 8)?;
                            self.buf.fill(0);
                            self.n = 0;
                        }
                    }
                }
                counts.push(self.total - before);
            }
        }
        Ok(())
    }

    /// Hands the last, partial stripe (its packed rows) to `full`.
    fn finish(
        &mut self,
        full: &mut dyn FnMut(&[u8], usize) -> Result<(), Error>,
    ) -> Result<(), Error> {
        if self.n != 0 {
            full(&self.buf, self.n.div_ceil(self.b) * 8)?;
            self.buf.fill(0);
            self.n = 0;
        }
        Ok(())
    }
}

/// Exact blocks as pass 1 settles them.
struct ExactPrep {
    sel: Select,
    /// Exact blocks per unit, in unit order.
    counts: Vec<usize>,
    /// Blocks per nested stripe.
    per: usize,
    /// The exact head (SPEC: B, every unit's count, the nested head, every
    /// nested stripe's size).
    head: Vec<u8>,
    /// Prepared numeric head inside the exact metadata; no decoder expansion.
    #[cfg(feature = "std")]
    model: core::ops::Range<usize>,
    /// Nested stripes.
    stripes: usize,
}

/// The AC tables pass 1 settles.
enum Ac {
    Rl(Tables),
    Huf(huf::HufTables),
}

/// What pass 1 settles before the first byte of output.
struct Prepared {
    #[cfg(feature = "lab")]
    first_band: Option<Vec<u8>>,
    entropy: LossyEntropy,
    ac: Ac,
    /// The AC table blob as the head carries it.
    tb: Vec<u8>,
    fits16: bool,
    dc_info: ImageInfo,
    dc_session: GroupEncodeSession,
    dc_head: Vec<u8>,
    /// The nested alpha image's session and head, with alpha.
    alpha: Option<(GroupEncodeSession, Vec<u8>)>,
    /// The trellis passes every coded stripe takes (empty without trellis).
    passes: Vec<trellis::Pass>,
    /// Exact blocks, when any block qualifies.
    exact: Option<ExactPrep>,
    /// Superblock merges or small transforms: the selector, and what it
    /// chose anywhere (then every AC record carries its unit's map).
    merges: Option<(superblock::select::Selector, Maps)>,
    /// Compact streaming open-loop trellis-lambda AQ model (`n_bands * bw * 4 B`).
    open_lambda: Option<adaptive_rdo::OpenLoopLambdaModel>,
    /// Compact streaming closed-loop seam residual feedback model
    /// (`<= 2 MiB` exact `f64` up to 16 MPx, `1 B/block` quantized `u8` above).
    closed_lambda: Option<adaptive_rdo::ClosedLoopLambdaModel>,
}

/// What the selector chose anywhere in the image.
#[derive(Clone, Copy, Debug, Default)]
struct Maps {
    /// A parent merged (16x16, 16x8 or 8x16).
    merge: bool,
    /// A plane split into four 4x4 transforms.
    split: bool,
}

impl Maps {
    /// The AC records lead with joint maps (head record 23): some plane
    /// splits. As `encode_lossy_frame`'s `MapKind::Joint`.
    fn joint(self) -> bool {
        self.split
    }
    /// The AC records lead with merge maps (head record 21).
    fn merges_only(self) -> bool {
        self.merge && !self.split
    }
}

#[cfg(all(test, feature = "encode"))]
fn fetcher<'a>(
    p: &'a Plan,
    rows: &'a mut dyn FnMut(u8, usize, &mut [u8]) -> Result<(), Error>,
) -> impl FnMut(u8, usize, usize, &mut Vec<u8>) -> Result<usize, Error> + 'a {
    move |pass, by0, by1, band| {
        let (y0, y1) = p.band_rows(by0, by1);
        let n = (y1 - y0).checked_mul(p.rb).ok_or(Error::TooLarge)?;
        band.clear();
        band.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
        band.resize(n, 0);
        rows(pass, y0, band)?;
        if p.input.is_some() {
            let mut info = p.info.clone();
            info.height = (y1 - y0) as u32;
            precision::validate_pixels(&info, band, p.rb)?;
        }
        Ok(y0)
    }
}

/// The payload of the one `R_STRIPE` record a group session just produced,
/// with its output consumed.
fn take_stripe_record(session: &mut GroupEncodeSession) -> Result<Vec<u8>, Error> {
    let framed = session.output();
    let mut r = ByteReader::new(framed);
    if r.varint()? != crate::v1::R_STRIPE {
        return Err(Error::Corrupt("bounded nested record"));
    }
    let len = r.varint()? as usize;
    let payload = framed[r.pos..r.pos + len].to_vec();
    let n = framed.len();
    session.consume_output(n)?;
    Ok(payload)
}

/// Fills `dc` with the DC image rows of block rows `[by0, by1)` from that
/// band's stripes, as the whole-image encoder lays them out: U16 (value +
/// 32768) or native-endian I32, three channels, or four with block `codes`
/// (exact blocks).
fn dc_band(
    p: &Plan,
    stripes: &[StripeCoefs],
    by0: usize,
    by1: usize,
    fits16: bool,
    codes: bool,
    dc: &mut Vec<u8>,
) {
    let dcs = if fits16 { 2 } else { 4 };
    let dch = if codes { 4 } else { NCH };
    let dc_stride = p.g.bw * dch * dcs;
    dc.clear();
    dc.resize((by1 - by0) * dc_stride, 0);
    for sc in stripes {
        for r in 0..sc.nby {
            for bx in 0..p.g.bw {
                let b = r * p.g.bw + bx;
                let o0 = (sc.by0 + r - by0) * dc_stride + bx * dch * dcs;
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
                    put(dc, c, if fits16 { v + 32768 } else { v });
                }
                if codes {
                    put(dc, 3, sc.aq[b] as i32);
                }
            }
        }
    }
}

impl Plan {
    /// The nested DC image and its group geometry (groups as wide as the
    /// lossy column groups, one DC stripe band per group row).
    fn dc_layout(&self, fits16: bool, codes: bool) -> (ImageInfo, GroupEncodeConfig) {
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
    fn alpha_cfg(&self) -> GroupEncodeConfig {
        let cols = if self.gcols == 0 {
            self.g.w
        } else {
            self.gcols
        };
        GroupEncodeConfig::new(cols as u32, self.sb * 8, usize::MAX)
    }

    /// Lossy stripe `s`'s alpha samples (rows of the image width) from `band`
    /// (source rows `y0..`); returns the row count.
    fn alpha_plane(&self, band: &[u8], y0: usize, s: usize, out: &mut Vec<u8>) -> usize {
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
fn dc_masks(cfg: &LossyConfig) -> Result<(u8, u8), Error> {
    let effort = crate::effort::Effort::new(cfg.effort)?;
    let d = crate::EncodeConfig::default();
    Ok((
        effort.decorrelations(d.decor_mask & 0x0f),
        effort.predictors(d.predictor_mask),
    ))
}

/// Pass 1: the DC range, the tier's side statistics and both coders'
/// histograms; then the entropy mode, its tables and the nested DC head.
/// Records the nested pass codes ahead of pass 2, which places them by index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spooled {
    /// A nested exact-image stripe.
    Exact,
    /// A DC group record of a DC stripe spanning several work bands.
    Dc,
}

/// Codes one nested exact stripe (`px`, rows `stride` bytes apart) and
/// queues its record in `coded` for storing.
fn code_exact(
    session: &mut GroupEncodeSession,
    px: &[u8],
    stride: usize,
    sizes: &mut Vec<usize>,
    coded: &mut Vec<Vec<u8>>,
) -> Result<(), Error> {
    session.push_group(px, stride)?;
    let rec = take_stripe_record(session)?;
    sizes.push(rec.len());
    coded.push(rec);
    Ok(())
}

/// Helper: fills `out` with the current band's open-loop lambda multipliers
/// from `model`/`cursor` (no-op when `model` is `None`).
#[allow(clippy::too_many_arguments)]
fn fill_band_open_lambda(
    p: &Plan,
    model: Option<&adaptive_rdo::OpenLoopLambdaModel>,
    cursor: &mut Option<adaptive_rdo::OpenLoopBandCursor>,
    band: &[u8],
    y0: usize,
    by0: usize,
    by1: usize,
    out: &mut Vec<f32>,
) -> Result<(), Error> {
    out.clear();
    if let (Some(m), Some(c)) = (model, cursor.as_mut()) {
        let rows = |by: usize, planes: &mut [f32], extremes: &mut Vec<(u64, bool)>| {
            p.fill_block_row(band, y0, by, planes, extremes)
        };
        m.fill_band(c, by0, by1, &rows, p.pw, &p.steps, out)?;
    }
    Ok(())
}

/// Pass 5 (when `p.merges.is_none()` and `p.needs_open_lambda()`): streams
/// the 3-row sliding-window open-loop trellis-lambda AQ model in `O(width)` RAM.
async fn measure_open_lambda<I: PassIo>(
    p: &Plan,
    io: &mut I,
) -> Result<adaptive_rdo::OpenLoopLambdaModel, Error> {
    let mut stream = adaptive_rdo::OpenLoopLambdaStream::new(
        p.g.bw,
        p.g.bh,
        p.work,
        &p.steps,
        p.plan.lambda_aq_open,
    )?;
    let plane_len = NCH
        .checked_mul(8)
        .and_then(|v| v.checked_mul(p.pw))
        .ok_or(Error::TooLarge)?;
    let mut planes = adaptive_rdo::try_vec(plane_len, 0.0f32)?;
    let mut extremes = adaptive_rdo::try_vec_cap(p.g.bw)?;
    let mut band = Vec::new();
    for (by0, by1) in p.bands() {
        let y0 = io.rows(5, by0, by1, &mut band).await?;
        for by in by0..by1 {
            io.stop().check().map_err(|_| Error::Cancelled)?;
            extremes.clear();
            p.fill_block_row(&band, y0, by, &mut planes, &mut extremes);
            stream.push_row(by, &planes, p.pw, &p.steps);
        }
    }
    stream.finish()
}

/// Pass 4: the superblock selector's prices from the units' token counts
/// before any merge (exact blocks marked), as `superblock::select::apply`
/// derives them from the whole image's units. When open-loop lambda AQ is
/// planned, also streams `OpenLoopLambdaStream` in the same pass (zero extra
/// source pass). When `p.plan.two_pass_pilot` is active, runs streaming pilot
/// passes 5 (and 6 when trellis runs) to learn empirical partition-map, 4x4
/// flag, and post-trellis token tables in fixed ~25 KiB RAM.
async fn merge_prices<I: PassIo>(
    p: &Plan,
    cfg: &LossyConfig,
    opts: superblock::select::Opts,
    sel: &mut Option<Select>,
    io: &mut I,
) -> Result<
    (
        superblock::select::Selector,
        Option<adaptive_rdo::OpenLoopLambdaModel>,
    ),
    Error,
> {
    let effort = crate::effort::Effort::new(cfg.effort)?;
    let mut base_hist = TokenHistogram::new(opts.split_huf);
    let mut band = Vec::new();
    let mut open_stream = if p.needs_open_lambda() {
        Some(adaptive_rdo::OpenLoopLambdaStream::new(
            p.g.bw,
            p.g.bh,
            p.work,
            &p.steps,
            p.plan.lambda_aq_open,
        )?)
    } else {
        None
    };
    let mut open_planes = if open_stream.is_some() {
        let plane_len = NCH
            .checked_mul(8)
            .and_then(|v| v.checked_mul(p.pw))
            .ok_or(Error::TooLarge)?;
        adaptive_rdo::try_vec(plane_len, 0.0f32)?
    } else {
        Vec::new()
    };
    let mut open_extremes = if open_stream.is_some() {
        adaptive_rdo::try_vec_cap(p.g.bw)?
    } else {
        Vec::new()
    };
    if let Some(sel) = sel {
        sel.begin();
    }
    for (by0, by1) in p.bands() {
        let y0 = io.rows(4, by0, by1, &mut band).await?;
        if let Some(stream) = &mut open_stream {
            for by in by0..by1 {
                io.stop().check().map_err(|_| Error::Cancelled)?;
                open_extremes.clear();
                p.fill_block_row(&band, y0, by, &mut open_planes, &mut open_extremes);
                stream.push_row(by, &open_planes, p.pw, &p.steps);
            }
        }
        let mut stripes = p.band_stripes(&band, y0, by0, by1);
        if let Some(sel) = sel {
            sel.band(p, &mut stripes, &band, y0, by0, by1, cfg.threads);
        }
        for sc in stripes {
            for u in p.units(sc) {
                base_hist.add_unit(&u);
            }
        }
    }
    let open_model = match open_stream {
        Some(s) => Some(s.finish()?),
        None => None,
    };
    if !p.plan.two_pass_pilot {
        return Ok((
            superblock::select::Selector::from_counts(
                &base_hist.rl,
                &base_hist.freqs,
                p.pw,
                p.gcols / 8,
                &p.steps,
                p.round,
                cfg,
                effort,
                opts,
            ),
            open_model,
        ));
    }

    // Streaming two-pass pilot (M3): Pass 5 gathers empirical map/flag tables
    // and post-selection histograms; Pass 6 (when trellis runs) gathers
    // post-1-pass-trellis token histograms; both hold only ~25 KiB retained state.
    let mut pilot_opts = opts;
    pilot_opts.rdo.trellis_select = false;
    let pilot_sel = superblock::select::Selector::from_counts(
        &base_hist.rl,
        &base_hist.freqs,
        p.pw,
        p.gcols / 8,
        &p.steps,
        p.round,
        cfg,
        effort,
        pilot_opts,
    );
    let pilot_huf_model = opts.split_huf && trellis::huf_rate(cfg);
    let want_pilot_huf = if p.trellis.is_some() {
        pilot_huf_model
    } else {
        opts.split_huf
    };
    let mut map_stats = adaptive_rdo::MapBitStats::default();
    let mut flag_stats = adaptive_rdo::FlagStats::default();
    let mut pilot_hist = TokenHistogram::new(want_pilot_huf);
    let mut open_cursor = open_model.as_ref().map(|m| m.cursor(p.pw)).transpose()?;
    let mut band_open_lambda = Vec::new();
    if let Some(sel) = sel {
        sel.begin();
    }
    for (by0, by1) in p.bands() {
        let y0 = io.rows(5, by0, by1, &mut band).await?;
        let selected = prepare_pilot_band(
            p,
            cfg,
            &pilot_sel,
            sel,
            open_model.as_ref(),
            &mut open_cursor,
            &band,
            y0,
            by0,
            by1,
            &mut band_open_lambda,
            io.stop(),
        )?;
        for sc in selected {
            map_stats.add_stripe(&sc, p.gcols / 8)?;
            if opts.small.is_some() {
                flag_stats.add_stripe(&sc);
            }
            for u in p.units(sc) {
                pilot_hist.add_unit(&u);
            }
        }
    }
    let learned_map = map_stats.into_table();
    let learned_flags = opts.small.is_some().then(|| flag_stats.into_flags());

    let final_hist = if let Some(mut t_set) = p.trellis {
        t_set.passes = 1;
        let pilot_pass = pilot_hist.trellis_pass(&t_set, cfg, effort, pilot_huf_model);
        let mut learned_hist = TokenHistogram::new(opts.split_huf);
        if let Some(sel) = sel {
            sel.begin();
        }
        for (by0, by1) in p.bands() {
            let y0 = io.rows(6, by0, by1, &mut band).await?;
            let selected = prepare_pilot_band(
                p,
                cfg,
                &pilot_sel,
                sel,
                open_model.as_ref(),
                &mut open_cursor,
                &band,
                y0,
                by0,
                by1,
                &mut band_open_lambda,
                io.stop(),
            )?;
            let rows = |by: usize, planes: &mut [f32], extremes: &mut Vec<(u64, bool)>| {
                p.fill_block_row(&band, y0, by, planes, extremes)
            };
            for sc in selected {
                let mut units = p.units(sc);
                let rewrite_args = trellis::StripeRewrite {
                    rows: &rows,
                    pw: p.pw,
                    image_bw: p.g.bw,
                    group_bw: p.gcols / 8,
                    steps: &p.steps,
                    cfg,
                    set: &t_set,
                    aq: &[],
                    lambda_map: &band_open_lambda,
                    lambda_by0: by0,
                    merged_trellis: p.plan.merged_trellis,
                    bias: None,
                    pass_index: 0,
                };
                trellis::rewrite_stripe_lambda(
                    &pilot_pass,
                    &mut units,
                    &rewrite_args,
                    None,
                    io.stop(),
                )?;
                for u in &units {
                    learned_hist.add_unit(u);
                }
            }
        }
        learned_hist
    } else {
        pilot_hist
    };

    Ok((
        superblock::select::Selector::from_counts_learned(
            &final_hist.rl,
            &final_hist.freqs,
            p.pw,
            p.gcols / 8,
            &p.steps,
            p.round,
            cfg,
            effort,
            opts,
            Some(learned_map),
            learned_flags,
        ),
        open_model,
    ))
}

/// Computes open-loop lambdas, pass-1 stripes, exact-block markings, and pilot
/// partition merges for one band (shared by streaming pilot passes 5 and 6).
#[allow(clippy::too_many_arguments)]
fn prepare_pilot_band(
    p: &Plan,
    cfg: &LossyConfig,
    pilot_sel: &superblock::select::Selector,
    sel: &mut Option<Select>,
    open_model: Option<&adaptive_rdo::OpenLoopLambdaModel>,
    open_cursor: &mut Option<adaptive_rdo::OpenLoopBandCursor>,
    band: &[u8],
    y0: usize,
    by0: usize,
    by1: usize,
    band_open_lambda: &mut Vec<f32>,
    stop: &dyn how_far::Stop,
) -> Result<Vec<StripeCoefs>, Error> {
    fill_band_open_lambda(
        p,
        open_model,
        open_cursor,
        band,
        y0,
        by0,
        by1,
        band_open_lambda,
    )?;
    let mut stripes = p.band_stripes(band, y0, by0, by1);
    if let Some(sel) = sel {
        sel.band(p, &mut stripes, band, y0, by0, by1, cfg.threads);
    }
    p.merge_internal(
        pilot_sel,
        stripes,
        band,
        y0,
        band_open_lambda,
        by0,
        cfg.threads,
        stop,
    )
}

/// With exact blocks, pass 0 measures their prices first, and after the
/// trellis passes a pass codes the nested exact stripes, handing each to
/// `store_exact` in order (they are needed again, by index, in pass 2).
/// With superblock merges, pass 4 measures the selector's prices before
/// pass 1, and passes 1 and 2 merge each band's stripes.
async fn pass1<I: PassIo>(p: &Plan, cfg: &LossyConfig, io: &mut I) -> Result<Prepared, Error> {
    let effort = crate::effort::Effort::new(cfg.effort)?;
    let mut sel = match p.exact {
        Some(pol) => Some(Select::measure(p, pol, cfg.threads, io).await?),
        None => None,
    };
    let (merge_sel, open_lambda) = match p.merges {
        Some(opts) => {
            let (ms, ol) = merge_prices(p, cfg, opts, &mut sel, io).await?;
            (Some(ms), ol)
        }
        None if p.needs_open_lambda() => (None, Some(measure_open_lambda(p, io).await?)),
        None => (None, None),
    };
    let mut open_cursor = open_lambda.as_ref().map(|m| m.cursor(p.pw)).transpose()?;
    let mut band_open_lambda = Vec::new();
    let mut maps = Maps::default();
    if let Some(sel) = &mut sel {
        sel.begin();
    }
    let mut band = Vec::new();
    let mut fits16 = true;
    let mut side = [0u64; 3];
    let mut ac_hist = TokenHistogram::both();
    // The nested DC groups' token counts, for measured DC tables; collected
    // as U16 and discarded if any DC needs I32.
    let (dc16_info, dc_cfg) = p.dc_layout(true, false);
    let masks = dc_masks(cfg)?;
    let (dc_measure, _) = GroupEncodeSession::nested(dc16_info, dc_cfg)?;
    let dc_measure = dc_measure.with_analysis(masks.0, masks.1);
    let mut dc_hists = dc_measure.empty_histograms();
    let mut dc = Vec::new();
    // The current DC stripe's DC images (three channels; four with block
    // codes) and whether its DCs fit U16.
    let (mut dc3_acc, mut dc4_acc, mut stripe_fits) = (Vec::new(), Vec::new(), true);
    // With exact blocks the DC image carries block codes, unless no block
    // qualifies: both layouts' counts are collected.
    let dc4_measure = match &sel {
        Some(_) => Some(
            GroupEncodeSession::nested(p.dc_layout(true, true).0, dc_cfg)?
                .0
                .with_analysis(masks.0, masks.1),
        ),
        None => None,
    };
    let mut dc4_hists = dc4_measure.as_ref().map(|m| m.empty_histograms());
    // The packed exact image's token counts, for its measured tables.
    let mut packer = sel.as_ref().map(|s| Packer::new(p, &s.pol));
    let ex_measure = match (&sel, &packer) {
        (Some(s), Some(pk)) => {
            let w = (8 * pk.b) as u32;
            let rows = s.pol.stripe_rows as u32;
            Some(
                GroupEncodeSession::nested(
                    ImageInfo::new(w, rows, Channels::Rgb, SampleType::U8),
                    GroupEncodeConfig::new(w, rows, usize::MAX),
                )?
                .0
                .with_analysis(masks.0, masks.1),
            )
        }
        _ => None,
    };
    let mut ex_hists = ex_measure.as_ref().map(|m| m.empty_histograms());
    let mut counts = Vec::new();
    let alpha_measure = match &p.alpha {
        Some(ai) => Some(
            GroupEncodeSession::nested(ai.clone(), p.alpha_cfg())?
                .0
                .with_analysis(masks.0, masks.1),
        ),
        None => None,
    };
    let mut alpha_hists = alpha_measure.as_ref().map(|m| m.empty_histograms());
    let mut plane = Vec::new();
    for (by0, by1) in p.bands() {
        let y0 = io.rows(1, by0, by1, &mut band).await?;
        let mut stripes = p.band_stripes(&band, y0, by0, by1);
        if let (Some(sel), Some(pk), Some(m), Some(h)) =
            (&mut sel, &mut packer, &ex_measure, &mut ex_hists)
        {
            let mask = sel.band(p, &mut stripes, &band, y0, by0, by1, cfg.threads);
            let stride = pk.stride();
            let w = 8 * pk.b;
            pk.band(
                p,
                &band,
                y0,
                by0,
                by1,
                &mask,
                &mut counts,
                &mut |px, rows| {
                    m.group_histograms(w, rows, px, stride, h);
                    Ok(())
                },
            )?;
        }
        if let Some(ms) = &merge_sel {
            fill_band_open_lambda(
                p,
                open_lambda.as_ref(),
                &mut open_cursor,
                &band,
                y0,
                by0,
                by1,
                &mut band_open_lambda,
            )?;
            stripes = p.merge(
                ms,
                stripes,
                &band,
                y0,
                &band_open_lambda,
                by0,
                cfg.threads,
                io.stop(),
            )?;
            maps.merge |= p.merges.is_some_and(|o| o.merges)
                && stripes.iter().any(|sc| sc.modes.iter().any(|&m| m != 0));
            maps.split |= stripes.iter().any(|sc| sc.splits.iter().any(|&v| v != 0));
        }
        let band_fits = stripes.iter().all(|sc| {
            sc.q.as_chunks::<64>()
                .0
                .iter()
                .all(|b| (-32768..=32767).contains(&b[0]))
        });
        stripe_fits &= band_fits;
        if fits16 && stripe_fits {
            dc_band(p, &stripes, by0, by1, true, false, &mut dc);
            dc3_acc.extend_from_slice(&dc);
            if dc4_measure.is_some() {
                dc_band(p, &stripes, by0, by1, true, true, &mut dc);
                dc4_acc.extend_from_slice(&dc);
            }
        }
        if p.ends_dc_stripe(by1) {
            if fits16 && stripe_fits {
                let cols = dc_cfg.columns as usize;
                for (acc, dch, m, h) in [
                    (&dc3_acc, NCH, Some(&dc_measure), Some(&mut dc_hists)),
                    (&dc4_acc, 4, dc4_measure.as_ref(), dc4_hists.as_mut()),
                ] {
                    let (Some(m), Some(h)) = (m, h) else { continue };
                    let stride = p.g.bw * dch * 2;
                    let rows = acc.len() / stride;
                    for x0 in (0..p.g.bw).step_by(cols) {
                        let w = cols.min(p.g.bw - x0);
                        m.group_histograms(w, rows, &acc[x0 * dch * 2..], stride, h);
                    }
                }
            }
            fits16 &= stripe_fits;
            stripe_fits = true;
            dc3_acc.clear();
            dc4_acc.clear();
        }
        if let (Some(m), Some(h)) = (&alpha_measure, &mut alpha_hists) {
            let cols = p.alpha_cfg().columns as usize;
            for sc in &stripes {
                let rows = p.alpha_plane(&band, y0, sc.by0 / p.sb as usize, &mut plane);
                for x0 in (0..p.g.w).step_by(cols) {
                    m.group_histograms(
                        cols.min(p.g.w - x0),
                        rows,
                        &plane[x0 * p.bps..],
                        p.g.w * p.bps,
                        h,
                    );
                }
            }
        }
        for sc in stripes {
            let st = side_stats(core::slice::from_ref(&sc), p.g.bw, 1);
            for (a, b) in side.iter_mut().zip(st) {
                *a += b;
            }
            for u in p.units(sc) {
                ac_hist.add_unit(&u);
            }
        }
    }
    // Exact blocks only when some block qualifies (as `encode_lossy`).
    let exact_total = match (&mut packer, &ex_measure, &mut ex_hists) {
        (Some(pk), Some(m), Some(h)) => {
            let stride = pk.stride();
            let w = 8 * pk.b;
            pk.finish(&mut |px, rows| {
                m.group_histograms(w, rows, px, stride, h);
                Ok(())
            })?;
            pk.total
        }
        _ => 0,
    };
    if exact_total == 0 {
        sel = None;
    }
    // The tier rule (the whole-image encoder's rule for images above
    // `SMALL_IMAGE`; smaller ones there also compare both codings).
    let entropy = match cfg.decode_speed {
        None => cfg.entropy,
        Some(speed) => {
            let ratio = predicted_huf_ratio((p.g.w * p.g.h) as f64, &side);
            if huf_max_ratio(speed).is_some_and(|max| ratio <= max) {
                LossyEntropy::SplitHuf
            } else {
                LossyEntropy::RunLevel
            }
        }
    };
    // The AC trellis (efforts 9, 9.5, 10, XYB): one source pass per rate-model
    // pass, each recomputing the pass-1 (and merged) units of every band and
    // applying the passes so far, as `trellis::optimize` prices them; the last
    // one also gathers the final tables' counts. Pass 0 also feeds the
    // streaming closed-loop seam feedback collector when enabled.
    let mut passes: Vec<trellis::Pass> = Vec::new();
    let mut closed_lambda: Option<adaptive_rdo::ClosedLoopLambdaModel> = None;
    let huf_model = entropy == LossyEntropy::SplitHuf;
    if let Some(set) = p.trellis.filter(|_| !huf_model || trellis::huf_rate(cfg)) {
        let n = set.passes as usize;
        let mut freqs: trellis::Freqs = ac_hist.freqs.clone();
        let mut band_closed_lambda = Vec::new();
        for k in 0..n {
            passes.push(if huf_model {
                trellis::Pass::split_huf(&freqs, &set, cfg, effort)
            } else {
                trellis::Pass::run_level(&ac_hist.rl, &set, cfg, effort)
            });
            let last = k + 1 == n;
            let mut next = trellis::freqs_new();
            let mut closed_stream = if k == 0 && !last && p.plan.lambda_aq_closed_loop != 0.0 {
                Some(adaptive_rdo::ClosedLoopLambdaStream::new(
                    p.g.bw,
                    p.g.bh,
                    &p.steps,
                    p.plan.lambda_aq_closed_loop,
                )?)
            } else {
                None
            };
            ac_hist.clear_rl();
            if last {
                ac_hist.clear_freqs();
            }
            if let Some(sel) = &mut sel {
                sel.begin();
            }
            for (by0, by1) in p.bands() {
                let y0 = io.rows(10 + k as u8, by0, by1, &mut band).await?;
                fill_band_open_lambda(
                    p,
                    open_lambda.as_ref(),
                    &mut open_cursor,
                    &band,
                    y0,
                    by0,
                    by1,
                    &mut band_open_lambda,
                )?;
                band_closed_lambda.clear();
                if k > 0
                    && let Some(cm) = &closed_lambda
                {
                    cm.fill_band(&band_open_lambda, by0, by1, &mut band_closed_lambda)?;
                }
                let mut stripes = p.band_stripes(&band, y0, by0, by1);
                if let Some(sel) = &mut sel {
                    sel.band(p, &mut stripes, &band, y0, by0, by1, cfg.threads);
                }
                if let Some(ms) = &merge_sel {
                    stripes = p.merge(
                        ms,
                        stripes,
                        &band,
                        y0,
                        &band_open_lambda,
                        by0,
                        cfg.threads,
                        io.stop(),
                    )?;
                }
                for sc in stripes {
                    let mut units = p.units(sc);
                    let emit = (huf_model && !last).then_some(&mut next);
                    p.rewrite(
                        cfg,
                        &passes,
                        &mut units,
                        &band,
                        y0,
                        &band_open_lambda,
                        &band_closed_lambda,
                        by0,
                        emit,
                        closed_stream.as_mut(),
                        io.stop(),
                    )?;
                    for u in &units {
                        ac_hist.add_rl(u);
                        if last {
                            ac_hist.add_huf(u);
                        }
                    }
                }
            }
            if let Some(stream) = closed_stream {
                closed_lambda = Some(stream.finish());
            }
            freqs = next;
        }
    }
    let ac = match entropy {
        LossyEntropy::RunLevel => {
            let hist = ac_hist.rl_rows();
            Ac::Rl(
                Tables::build_seeded(
                    &hist,
                    4,
                    cfg.max_ac_tables,
                    LossyEntropy::RunLevel,
                    effort,
                    None,
                    table_k_mode(cfg),
                )
                .0,
            )
        }
        LossyEntropy::SplitHuf => {
            let [luma, chroma, wid] = &ac_hist.freqs;
            Ac::Huf(huf::HufTables::build(luma, chroma, wid, effort, None, table_k_mode(cfg)).0)
        }
    };
    let tb = {
        let mut header = BitWriter::new();
        match &ac {
            Ac::Rl(t) => t.write(&mut header),
            Ac::Huf(t) => t.write(&mut header),
        }
        header.finish()
    };
    // The nested DC image: lossless groups with tables measured in this pass
    // (fixed tables when a DC needs I32, whose counts were not collected).
    let codes = sel.is_some();
    let (dc_info, dc_cfg) = p.dc_layout(fits16, codes);
    let (dc_session, dc_head) = if fits16 {
        let hists = match &dc4_hists {
            Some(h) if codes => h,
            _ => &dc_hists,
        };
        let (session, head) = GroupEncodeSession::nested_measured(dc_info.clone(), dc_cfg, hists)?;
        (session.with_analysis(masks.0, masks.1), head)
    } else {
        GroupEncodeSession::nested(dc_info.clone(), dc_cfg)?
    };
    let alpha = match (&p.alpha, &alpha_hists) {
        (Some(ai), Some(h)) => {
            let (session, head) =
                GroupEncodeSession::nested_measured(ai.clone(), p.alpha_cfg(), h)?;
            Some((session.with_analysis(masks.0, masks.1), head))
        }
        _ => None,
    };
    // The nested pass (pass 3), with exact blocks or DC stripes taller than
    // a work band: it codes the nested exact image (lossless groups, one per
    // nested stripe, with the tables measured above), whose sizes the exact
    // head lists before the first unit record, and the DC records of DC
    // stripes spanning several bands, each once its DC image is complete.
    // Both are stored for pass 2, which places them by index.
    let mut dc_session = dc_session;
    let split = p.split_dc();
    let mut ex = match (sel, ex_hists) {
        (Some(sel), Some(hists)) => {
            let pk = Packer::new(p, &sel.pol);
            let w = (8 * pk.b) as u32;
            let info = ImageInfo::new(
                w,
                u32::try_from(exact_total.div_ceil(pk.b) * 8).map_err(|_| Error::TooLarge)?,
                Channels::Rgb,
                SampleType::U8,
            );
            let (session, nested_head) = GroupEncodeSession::nested_measured(
                info,
                GroupEncodeConfig::new(w, sel.pol.stripe_rows as u32, usize::MAX),
                &hists,
            )?;
            Some((
                sel,
                pk,
                session.with_analysis(masks.0, masks.1),
                nested_head,
            ))
        }
        _ => None,
    };
    let mut sizes = Vec::new();
    if ex.is_some() || split {
        let dch = if codes { 4 } else { NCH };
        let dc_stride = p.g.bw * dch * if fits16 { 2 } else { 4 };
        let mut again = Vec::new();
        let mut coded: Vec<Vec<u8>> = Vec::new();
        let (mut acc, mut dcb) = (Vec::new(), Vec::new());
        if let Some((sel, ..)) = &mut ex {
            sel.begin();
        }
        for (by0, by1) in p.bands() {
            let y0 = io.rows(3, by0, by1, &mut band).await?;
            let mut stripes = p.band_stripes(&band, y0, by0, by1);
            if let Some((sel, pk, session, _)) = &mut ex {
                let mask = sel.band(p, &mut stripes, &band, y0, by0, by1, cfg.threads);
                let stride = pk.stride();
                pk.band(p, &band, y0, by0, by1, &mask, &mut again, &mut |px, _| {
                    code_exact(session, px, stride, &mut sizes, &mut coded)
                })?;
                for rec in coded.drain(..) {
                    io.store(Spooled::Exact, &rec).await?;
                }
            }
            if !split {
                continue;
            }
            if let Some(ms) = &merge_sel {
                fill_band_open_lambda(
                    p,
                    open_lambda.as_ref(),
                    &mut open_cursor,
                    &band,
                    y0,
                    by0,
                    by1,
                    &mut band_open_lambda,
                )?;
                stripes = p.merge(
                    ms,
                    stripes,
                    &band,
                    y0,
                    &band_open_lambda,
                    by0,
                    cfg.threads,
                    io.stop(),
                )?;
            }
            dc_band(p, &stripes, by0, by1, fits16, codes, &mut dcb);
            acc.extend_from_slice(&dcb);
            if p.ends_dc_stripe(by1) {
                let first = by0 / p.dc_rows * p.dc_rows;
                for _ in 0..p.ng {
                    let rect = dc_session
                        .next_group()
                        .ok_or(Error::Corrupt("bounded DC group order"))?;
                    if rect.y as usize != first {
                        return Err(Error::Corrupt("bounded DC group order"));
                    }
                    let x = rect.x as usize * dc_stride / p.g.bw;
                    dc_session.push_group(&acc[x..], dc_stride)?;
                    let rec = take_stripe_record(&mut dc_session)?;
                    io.store(Spooled::Dc, &rec).await?;
                }
                acc.clear();
            }
        }
        if let Some((_, pk, session, _)) = &mut ex {
            let stride = pk.stride();
            pk.finish(&mut |px, _| code_exact(session, px, stride, &mut sizes, &mut coded))?;
            for rec in coded.drain(..) {
                io.store(Spooled::Exact, &rec).await?;
            }
            if again != counts || !session.is_finished() {
                return Err(Error::InvalidInput(
                    "source rows changed between bounded lossy passes",
                ));
            }
        }
        if split && !dc_session.is_finished() {
            return Err(Error::Corrupt("bounded nested image incomplete"));
        }
    }
    let exact = match ex {
        Some((sel, pk, _, nested_head)) => {
            let mut head = Vec::new();
            put_varint(&mut head, pk.b as u64);
            for &n in &counts {
                put_varint(&mut head, n as u64);
            }
            #[cfg(feature = "std")]
            let model_start = head.len();
            head.extend_from_slice(&nested_head);
            #[cfg(feature = "std")]
            let model = model_start..head.len();
            for &n in &sizes {
                put_varint(&mut head, n as u64);
            }
            Some(ExactPrep {
                sel,
                counts,
                per: pk.per,
                head,
                #[cfg(feature = "std")]
                model,
                stripes: sizes.len(),
            })
        }
        None => None,
    };
    Ok(Prepared {
        #[cfg(feature = "lab")]
        first_band: None,
        merges: merge_sel.map(|s| (s, maps)),
        open_lambda,
        closed_lambda,
        exact,
        passes,
        alpha,
        entropy,
        ac,
        tb,
        fits16,
        dc_info,
        dc_session,
        dc_head,
    })
}

impl Plan {
    /// Main records of the payload: every unit's AC record plus each column
    /// group's DC record per DC stripe (SPEC 2.7).
    fn record_count(&self, exact: bool) -> usize {
        let units = self.g.stripes() * self.ng;
        let alpha = if self.alpha.is_some() { units } else { 0 };
        let exact = if exact { units } else { 0 };
        units + alpha + exact + self.g.bh.div_ceil(self.dc_rows) * self.ng
    }
}

/// Pass 2: recomputes each band and hands every main record, in decode
/// order (SPEC 2.7), to `io.emit(id, payload)`.
async fn pass2<I: PassIo>(
    p: &Plan,
    cfg: &LossyConfig,
    prep: &mut Prepared,
    io: &mut I,
) -> Result<(), Error> {
    use crate::parse::records::{R_DC, R_EXACT};
    let split = p.split_dc();
    let k = p.dc_rows / p.sb as usize;
    let dcs = if prep.fits16 { 2 } else { 4 };
    let codes = prep.exact.is_some();
    let dch = if codes { 4 } else { NCH };
    if let Some(e) = &mut prep.exact {
        e.sel.begin();
    }
    // Exact records: unit `unit` carries the nested stripes whose first
    // block is among its own (list entries from `list`).
    let (mut unit, mut list, mut next_t) = (0usize, 0usize, 0usize);
    let mut stripe_bytes = Vec::new();
    let mut band = Vec::new();
    let mut dc = Vec::new();
    let mut plane = Vec::new();
    let encoder_rl = match &prep.ac {
        Ac::Rl(t) => Some(EncodeTables::new(&t.freq, &t.ctx_map)),
        Ac::Huf(_) => None,
    };
    // Canonical Huffman codes are immutable across every unit in this image,
    // as in the whole-image encoder. Compile them once, not per unit.
    let encoder_huf = match &prep.ac {
        Ac::Huf(t) => Some(t.encoder()),
        Ac::Rl(_) => None,
    };
    let mut open_cursor = prep
        .open_lambda
        .as_ref()
        .map(|m| m.cursor(p.pw))
        .transpose()?;
    let mut band_open_lambda = Vec::new();
    let mut band_closed_lambda = Vec::new();
    for (by0, by1) in p.bands() {
        #[cfg(feature = "lab")]
        let prefetched = prep.first_band.take();
        #[cfg(not(feature = "lab"))]
        let prefetched: Option<Vec<u8>> = None;
        let y0 = if let Some(first) = prefetched {
            debug_assert_eq!(by0, 0);
            band = first;
            0
        } else {
            io.rows(2, by0, by1, &mut band).await?
        };
        fill_band_open_lambda(
            p,
            prep.open_lambda.as_ref(),
            &mut open_cursor,
            &band,
            y0,
            by0,
            by1,
            &mut band_open_lambda,
        )?;
        band_closed_lambda.clear();
        if let Some(cm) = &prep.closed_lambda {
            cm.fill_band(&band_open_lambda, by0, by1, &mut band_closed_lambda)?;
        }
        let mut stripes = p.band_stripes(&band, y0, by0, by1);
        if let Some(e) = &mut prep.exact {
            e.sel
                .band(p, &mut stripes, &band, y0, by0, by1, cfg.threads);
        }
        if let Some((ms, _)) = &prep.merges {
            stripes = p.merge(
                ms,
                stripes,
                &band,
                y0,
                &band_open_lambda,
                by0,
                cfg.threads,
                io.stop(),
            )?;
        }
        // DC records: coded here when the band is the DC stripe, else stored
        // by the nested pass.
        let mut dc_records = Vec::with_capacity(p.ng);
        let dc_stride = p.g.bw * dch * dcs;
        if !split {
            dc_band(p, &stripes, by0, by1, prep.fits16, codes, &mut dc);
        }
        for _ in 0..if split { 0 } else { p.ng } {
            let rect = prep
                .dc_session
                .next_group()
                .ok_or(Error::Corrupt("bounded DC group order"))?;
            if rect.y as usize != by0 {
                return Err(Error::Corrupt("bounded DC group order"));
            }
            let x = rect.x as usize * dch * dcs;
            prep.dc_session.push_group(&dc[x..], dc_stride)?;
            dc_records.push(take_stripe_record(&mut prep.dc_session)?);
        }
        for sc in stripes {
            let si = sc.by0 / p.sb as usize;
            if split && si.is_multiple_of(k) {
                dc_records.clear();
                for gi in 0..p.ng {
                    let mut rec = Vec::new();
                    io.load(Spooled::Dc, si / k * p.ng + gi, &mut rec).await?;
                    dc_records.push(rec);
                }
            }
            let mut alpha_records = Vec::new();
            if let Some((session, _)) = &mut prep.alpha {
                let rows = p.alpha_plane(&band, y0, si, &mut plane);
                for _ in 0..p.ng {
                    let rect = session
                        .next_group()
                        .ok_or(Error::Corrupt("bounded alpha group order"))?;
                    if rect.y as usize != si * p.sb as usize * 8 || rect.height as usize != rows {
                        return Err(Error::Corrupt("bounded alpha group order"));
                    }
                    session.push_group(&plane[rect.x as usize * p.bps..], p.g.w * p.bps)?;
                    alpha_records.push(take_stripe_record(session)?);
                }
            }
            let mut units = p.units(sc);
            p.rewrite(
                cfg,
                &prep.passes,
                &mut units,
                &band,
                y0,
                &band_open_lambda,
                &band_closed_lambda,
                by0,
                None,
                None,
                io.stop(),
            )?;
            for (gi, u) in units.into_iter().enumerate() {
                if si.is_multiple_of(k) {
                    if cfg.model_references {
                        let mut record = vec![0, 0];
                        record.extend_from_slice(&dc_records[gi]);
                        io.emit(R_DC, &record).await?;
                    } else {
                        io.emit(R_DC, &dc_records[gi]).await?;
                    }
                }
                if let Some(a) = alpha_records.get(gi) {
                    io.emit(crate::parse::records::R_ALPHA, a).await?;
                }
                let mut out = Vec::new();
                // With any merge or split, every unit's record leads with its
                // map, as `encode_lossy_frame` writes them.
                let maps = prep.merges.as_ref().map(|m| m.1).unwrap_or_default();
                if cfg.coding_defaults {
                    let settings = coding_defaults::encode_inline(&u)?;
                    put_varint(&mut out, settings.len() as u64);
                    out.extend_from_slice(&settings);
                } else if maps.merges_only() {
                    let map = superblock::map::encode(&u.modes, u.nby, u.bw);
                    put_varint(&mut out, map.len() as u64);
                    out.extend_from_slice(&map);
                } else if maps.joint() {
                    // Exact parents (wire code 1) carry no flags.
                    let bypass = |b: usize| codes && u.aq.get(b).copied() == Some(1);
                    let n = u.nby * u.bw;
                    let modes = if u.modes.is_empty() {
                        vec![0u8; n]
                    } else {
                        u.modes.clone()
                    };
                    // Flags only where the final coefficients have AC.
                    let ac = superblock::ac_bits(&u.q, n);
                    let splits: Vec<u8> = (0..n)
                        .map(|b| u.splits.get(b).copied().unwrap_or(0) & ac[b])
                        .collect();
                    let map =
                        superblock::map::encode_joint(&modes, &splits, &ac, (u.nby, u.bw), &bypass);
                    put_varint(&mut out, map.len() as u64);
                    out.extend_from_slice(&map);
                } else if p.adaptive_mask != 0 {
                    // Adaptive layouts: the unit's transform choices.
                    layout::write_modes(&mut out, &u.modes);
                }
                match (&prep.ac, &encoder_rl) {
                    (Ac::Rl(_), Some(encoder)) => {
                        let mut lanes: [Vec<Sym>; NL] = core::array::from_fn(|_| Vec::new());
                        stripe_symbols(&u, u.bw, &mut lanes);
                        let map_prefix = out.len();
                        encode_lanes_tables(encoder, &lanes, u.bw.min(NL), &mut out);
                        if cfg.model_references {
                            let body = model_refs::encode_causal_ans(
                                &lanes,
                                u.bw.min(NL),
                                cfg,
                                crate::effort::Effort::new(cfg.effort)?,
                                &out[map_prefix..],
                            )?;
                            out.truncate(map_prefix);
                            out.extend_from_slice(&body);
                        } else if cfg.local_ans_contexts {
                            let body = local_ans::encode(
                                &lanes,
                                u.bw.min(NL),
                                cfg,
                                crate::effort::Effort::new(cfg.effort)?,
                                &out[map_prefix..],
                            )?;
                            out.truncate(map_prefix);
                            out.extend_from_slice(&body);
                        }
                    }
                    (Ac::Huf(t), _) => {
                        if cfg.model_references {
                            put_varint(&mut out, 0);
                        }
                        let encoder = encoder_huf.as_ref().unwrap();
                        let (counts, widths) = stripe_counts_widths(&u, u.bw);
                        let symbols = huf::stripe_syms(&counts, &widths, u.nby * u.bw);
                        let (magnitudes, signs) = stripe_raw_split(&u, u.bw, &counts, &widths);
                        for (symbols, map) in [
                            (&symbols.luma, &t.luma_map[..]),
                            (&symbols.chroma, &t.chroma_map[..]),
                            (&symbols.wid, &t.wid_map[..]),
                        ] {
                            let mut section = Vec::new();
                            encoder.encode_section(symbols, map, &mut section);
                            put_varint(&mut out, section.len() as u64);
                            out.extend_from_slice(&section);
                        }
                        put_varint(&mut out, magnitudes.len() as u64);
                        out.extend_from_slice(&magnitudes);
                        out.extend_from_slice(&signs);
                    }
                    _ => unreachable!(),
                }
                io.emit_ac(&out, &u).await?;
                if let Some(e) = &prep.exact {
                    let n = e.counts[unit];
                    let mut rec = Vec::new();
                    while next_t < e.stripes && next_t * e.per < list + n {
                        io.load(Spooled::Exact, next_t, &mut stripe_bytes).await?;
                        rec.extend_from_slice(&stripe_bytes);
                        next_t += 1;
                    }
                    list += n;
                    io.emit(R_EXACT, &rec).await?;
                }
                unit += 1;
            }
        }
    }
    if !prep.dc_session.is_finished() || prep.alpha.as_ref().is_some_and(|(s, _)| !s.is_finished())
    {
        return Err(Error::Corrupt("bounded nested image incomplete"));
    }
    Ok(())
}

/// The `DATA` payload head (everything before the first main record), with
/// `records` as its head record list, and where that list starts in it.
fn data_head(p: &Plan, prep: &Prepared, records: &[u8]) -> (Vec<u8>, usize) {
    let mut data = Vec::new();
    data.push(p.sb as u8);
    put_varint(&mut data, p.gcols as u64);
    envelope::write(
        &mut data,
        p.color,
        p.kind,
        &envelope::Params {
            scale: p.scale,
            qp: p.qp,
            entropy: prep.entropy,
            compact: false,
            seed: crate::seeds::Selection::NONE,
            records: !records.is_empty(),
        },
    );
    put_varint(&mut data, prep.tb.len() as u64);
    data.extend_from_slice(&prep.tb);
    let mut at = data.len();
    if !records.is_empty() {
        put_varint(&mut data, records.len() as u64);
        at = data.len();
        data.extend_from_slice(records);
    }
    put_varint(&mut data, prep.dc_head.len() as u64);
    data.extend_from_slice(&prep.dc_head);
    if let Some((_, head)) = &prep.alpha {
        put_varint(&mut data, head.len() as u64);
        data.extend_from_slice(head);
    }
    if let Some(e) = &prep.exact {
        put_varint(&mut data, e.head.len() as u64);
        data.extend_from_slice(&e.head);
    }
    (data, at)
}

/// The file header for this payload (everything before `DATA`).
fn file_header(
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    p: &Plan,
    prep: &Prepared,
) -> Result<Vec<u8>, Error> {
    use crate::parse::header::Feature;
    let mut features = crate::v1::head_features(&prep.dc_info, &prep.dc_head);
    if let (Some(ai), Some((_, head))) = (&p.alpha, &prep.alpha) {
        features = features.union(crate::v1::head_features(ai, head));
    }
    if !prep.fits16 {
        features = features.with(Feature::Samples32);
    }
    if cfg.local_ans_contexts {
        features = features.with(Feature::LocalAnsContexts);
    }
    if cfg.coding_defaults {
        features = features.with(Feature::CodingDefaults);
    }
    if cfg.model_references {
        features = features.with(Feature::ModelReferences);
    }
    if prep.exact.is_some() {
        features = features.with(Feature::ExactBlocks);
    }
    let maps = prep.merges.as_ref().map(|m| m.1).unwrap_or_default();
    if maps.merge {
        features = features.with(Feature::Superblocks);
    }
    if maps.joint() {
        features = features.with(Feature::SmallTransforms);
    }
    if p.layouts != [LossyChannelLayout::default(); 3] {
        features = features.with(Feature::ChannelLayouts);
    }
    if (3..=MATRIX_LAST_PRESET).contains(&p.qp.matrix) {
        features = features.with(Feature::DequantPresets);
    }
    let mut c = cfg.clone();
    c.color = p.color;
    let mut out = Vec::new();
    crate::container::write_head_features(
        &mut out,
        info,
        meta,
        crate::Coding::Lossy,
        Some(p.color),
        c.decode_speed(),
        features,
    )?;
    Ok(out)
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

fn plan_working_bytes(p: &Plan) -> Result<u64, Error> {
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
    u64::try_from(
        2 * raster
            + 3 * coefs
            + dc
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

#[cfg(all(test, feature = "encode"))]
pub(crate) fn encode_lossy_bounded(
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    rows: &mut dyn FnMut(u8, usize, &mut [u8]) -> Result<(), Error>,
) -> Result<Vec<u8>, Error> {
    use crate::parse::records::{framed_len, put_record};
    let p = plan(info, cfg)?;
    let mut fetch = fetcher(&p, rows);
    let mut exact_stripes: Vec<Vec<u8>> = Vec::new();
    let mut dc_records: Vec<Vec<u8>> = Vec::new();
    let mut prep = crate::parse::payload::ready(pass1(
        &p,
        cfg,
        &mut Callbacks {
            rows: &mut fetch,
            store: |kind: Spooled, rec: &[u8]| {
                match kind {
                    Spooled::Exact => exact_stripes.push(rec.to_vec()),
                    Spooled::Dc => dc_records.push(rec.to_vec()),
                }
                Ok(())
            },
            load: no_load,
            emit: no_emit,
        },
    ))?;
    let mut body = Vec::new();
    let mut spans = Vec::new();
    crate::parse::payload::ready(pass2(
        &p,
        cfg,
        &mut prep,
        &mut Callbacks {
            rows: &mut fetch,
            store: no_store,
            load: |kind: Spooled, t: usize, out: &mut Vec<u8>| {
                out.clear();
                out.extend_from_slice(match kind {
                    Spooled::Exact => &exact_stripes[t][..],
                    Spooled::Dc => &dc_records[t][..],
                });
                Ok(())
            },
            emit: |id: u64, payload: &[u8]| {
                spans.push(framed_len(id, payload.len()));
                put_record(&mut body, id, payload);
                Ok(())
            },
        },
    ))?;
    let nunits = p.g.stripes() * p.ng;
    // Head records in increasing id order (SPEC 2.8), as `finish_lossy_frame`
    // writes them for this configuration.
    let mut records = Vec::new();
    if prep.exact.is_some() {
        put_record(&mut records, H_EXACT, &[]);
    }
    let hint_form = matches!(crate::v1::unit_table_form(), crate::v1::UnitTableForm::Hint);
    if hint_form {
        crate::v1::put_unit_table(&mut records, nunits, &spans);
    }
    if !prep.fits16 {
        put_record(&mut records, H_DC32, &[]);
    }
    if !hint_form {
        crate::v1::put_unit_table(&mut records, nunits, &spans);
    }
    match if cfg.coding_defaults {
        None
    } else {
        prep.merges.as_ref().map(|m| m.1)
    } {
        Some(m) if m.joint() => put_record(&mut records, small::H_PARTITIONS, &[]),
        Some(m) if m.merges_only() => put_record(&mut records, superblock::H_SUPERBLOCKS, &[]),
        _ => {}
    }
    if p.layouts != [LossyChannelLayout::default(); 3] {
        put_record(&mut records, H_LAYOUTS, &p.layouts.map(|l| l.byte()));
    }
    if cfg.local_ans_contexts {
        put_record(&mut records, local_ans::HEAD, &[]);
    }
    if cfg.coding_defaults {
        put_record(&mut records, coding_defaults::HEAD, &[1, 0, 0]);
    }
    if cfg.model_references {
        put_record(&mut records, model_refs::HEAD, &[1, 0]);
    }
    let (mut data, _) = data_head(&p, &prep, &records);
    data.extend_from_slice(&body);
    let mut out = file_header(info, meta, cfg, &p, &prep)?;
    crate::container::put_data(&mut out, &data);
    let out = crate::container::finish_unit_table(out)?;
    let out = crate::container::locate(out)?;
    if cfg.checksum {
        crate::checksum::finish_file(out, Some(cfg.hash))
    } else {
        Ok(out)
    }
}

/// Where the streamed writer puts the unit table (SPEC 2.7).
#[cfg(feature = "std")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StreamIndex {
    /// Head record 12 reserved at this many bytes per span and filled in place
    /// after each band (the index stays at the head); a span too wide for it
    /// turns the reservation into check tail plus a trailing `uIDX`.
    Reserved(u8),
    /// Head record 12 says check tail; the spans follow in a trailing `uIDX`.
    Trailing,
    V4 {
        directory: crate::v4::DirectoryPlacement,
        capacity: u64,
    },
}

/// A stream a bounded encode writes or reads by offset.
#[cfg(feature = "std")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stream {
    /// The encoded file; offset 0 is its first byte. Written in order, except
    /// the unit-table reservation, which is filled after the last band.
    Output,
    /// Index scratch: the spooled nested records, then one varint per main
    /// record. Appended, and read back in pass 2 and at the end.
    Index,
    /// Checksum scratch: eight bytes per checksum piece. Appended, and read
    /// back once at the end.
    Checksum,
}

/// The I/O of one bounded encode: source rows, and positioned writes and
/// reads of the three [`Stream`]s. Async, so the same driver runs blocking
/// (every call completes at once) or as a sans-IO session (a call waits for
/// the caller).
#[cfg(feature = "std")]
pub(crate) trait StreamIo {
    fn stop(&self) -> &dyn how_far::Stop {
        &how_far::NoPulse
    }
    /// Source rows from `y0` into `out` (its whole length, rows of
    /// `row_bytes`), for `pass` (0 exact prices, 4 superblock prices,
    /// 1 statistics, 10 + k trellis rate model k, 3 nested images,
    /// 2 coding). `out` may be replaced by another buffer of its length.
    async fn source(
        &mut self,
        pass: u8,
        y0: usize,
        out: &mut Vec<u8>,
    ) -> Result<(), crate::stream_encode::StreamEncodeError>;
    async fn write(
        &mut self,
        stream: Stream,
        at: u64,
        bytes: &[u8],
    ) -> Result<(), crate::stream_encode::StreamEncodeError>;
    async fn read(
        &mut self,
        stream: Stream,
        at: u64,
        buf: &mut [u8],
    ) -> Result<(), crate::stream_encode::StreamEncodeError>;
}

/// An append-only range of a scratch stream from `start`, with a running
/// hash so a replay can tell whether the scratch changed underneath
/// (`stream_encode::spool::Spool` by offset).
#[cfg(feature = "std")]
struct Range {
    stream: Stream,
    start: u64,
    len: u64,
    limit: u64,
    hash: zenhash::Xxh3,
    changed: &'static str,
}

#[cfg(feature = "std")]
impl Range {
    fn new(stream: Stream, start: u64, limit: u64, changed: &'static str) -> Result<Self, Error> {
        start.checked_add(limit).ok_or(Error::TooLarge)?;
        Ok(Self {
            stream,
            start,
            len: 0,
            limit,
            hash: zenhash::Xxh3::new(),
            changed,
        })
    }

    async fn append<'a, X: StreamIo>(
        &'a mut self,
        x: &'a mut X,
        bytes: &'a [u8],
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        let len = self
            .len
            .checked_add(bytes.len() as u64)
            .filter(|&n| n <= self.limit)
            .ok_or(Error::TooLarge)?;
        x.write(self.stream, self.start + self.len, bytes).await?;
        self.hash.update(bytes);
        self.len = len;
        Ok(())
    }

    /// Reads bytes the stream held before this range began.
    async fn read_before<X: StreamIo>(
        &self,
        x: &mut X,
        at: u64,
        buf: &mut [u8],
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        if at
            .checked_add(buf.len() as u64)
            .is_none_or(|end| end > self.start)
        {
            return Err(Error::InvalidInput(self.changed).into());
        }
        x.read(self.stream, at, buf).await
    }

    /// A checked replay, consumed to `None` to verify the entire range. Callers
    /// process each chunk before asking for the next, and propagate any error.
    fn replay(&self) -> CheckedReplay<'_> {
        CheckedReplay {
            range: self,
            buffer: [0u8; crate::stream_encode::spool::REPLAY_BYTES],
            done: 0,
            hash: zenhash::Xxh3::new(),
            failed: false,
        }
    }
}

/// Keeps the original replay buffer and hash, without an async callback whose
/// higher-ranked borrowed future prevents proving the native session is Send.
/// Each caller scopes this cursor to the same interval as the former replay.
#[cfg(feature = "std")]
struct CheckedReplay<'a> {
    range: &'a Range,
    buffer: [u8; crate::stream_encode::spool::REPLAY_BYTES],
    done: u64,
    hash: zenhash::Xxh3,
    failed: bool,
}

#[cfg(feature = "std")]
impl CheckedReplay<'_> {
    /// Replay is provisional until a call returns verified `None`.
    /// Verification follows consumption of the last chunk, preserving the old
    /// partial-write semantics. A read error or interrupted read cannot retry.
    async fn next<X: StreamIo>(
        &mut self,
        x: &mut X,
    ) -> Result<Option<&[u8]>, crate::stream_encode::StreamEncodeError> {
        if self.failed {
            return Err(Error::Corrupt("bounded replay failed").into());
        }
        self.failed = true;
        if self.done == self.range.len {
            if self.hash.digest() != self.range.hash.digest() {
                return Err(Error::InvalidInput(self.range.changed).into());
            }
            self.failed = false;
            return Ok(None);
        }
        let count = (self.range.len - self.done).min(self.buffer.len() as u64) as usize;
        x.read(
            self.range.stream,
            self.range.start + self.done,
            &mut self.buffer[..count],
        )
        .await?;
        self.hash.update(&self.buffer[..count]);
        self.done += count as u64;
        self.failed = false;
        Ok(Some(&self.buffer[..count]))
    }
}

/// Piece size of streamed file checksums: 1 MiB, the whole-image writers'
/// piece for files over 16 MiB (`checksum::piece_log2_for`); the length is
/// not known when the header is written.
#[cfg(feature = "std")]
pub(crate) const STREAM_PIECE_LOG2: u8 = 20;

/// A `cSUM` table (SPEC 1.4) of the bytes fed in order, XXH3-128 in `2^log2`
/// pieces, the digests spooled to the checksum stream. The bytes before
/// `hold` (rounded up to a piece) are kept and hashed last, so a writer may
/// patch them ([`Self::patch`]) after writing.
#[cfg(feature = "std")]
struct StreamSum {
    log2: u8,
    algo: crate::HashAlgorithm,
    hold: u64,
    held: Vec<u8>,
    hash: crate::Hasher,
    piece: u64,
    covered: u64,
    digests: Range,
}

#[cfg(feature = "std")]
impl StreamSum {
    fn new(
        start: u64,
        log2: u8,
        algo: crate::HashAlgorithm,
        hold: u64,
        limit: u64,
    ) -> Result<Self, Error> {
        Ok(Self {
            log2,
            algo,
            hold: hold.div_ceil(1 << log2) << log2,
            held: Vec::new(),
            hash: crate::Hasher::new(algo),
            piece: 0,
            covered: 0,
            digests: Range::new(
                Stream::Checksum,
                start,
                limit,
                "bounded lossy checksum scratch changed",
            )?,
        })
    }

    async fn feed<X: StreamIo>(
        &mut self,
        x: &mut X,
        mut bytes: &[u8],
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        self.covered = self
            .covered
            .checked_add(bytes.len() as u64)
            .ok_or(Error::TooLarge)?;
        let room = self.hold - self.held.len() as u64;
        if room != 0 {
            let n = room.min(bytes.len() as u64) as usize;
            self.held.try_reserve(n).map_err(|_| Error::TooLarge)?;
            self.held.extend_from_slice(&bytes[..n]);
            bytes = &bytes[n..];
        }
        let size = 1u64 << self.log2;
        while !bytes.is_empty() {
            let n = (size - self.piece).min(bytes.len() as u64) as usize;
            self.hash.update(&bytes[..n]);
            self.piece += n as u64;
            bytes = &bytes[n..];
            if self.piece == size {
                self.end_piece(x).await?;
            }
        }
        Ok(())
    }

    async fn end_piece<X: StreamIo>(
        &mut self,
        x: &mut X,
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        if self.piece != 0 {
            let d = self.hash.finish();
            self.digests.append(x, d.as_bytes()).await?;
            self.hash.reset();
            self.piece = 0;
        }
        Ok(())
    }

    /// Replaces held bytes at `at` (from the first byte fed).
    fn patch(&mut self, at: u64, bytes: &[u8]) {
        self.held[at as usize..][..bytes.len()].copy_from_slice(bytes);
    }

    /// Writes the whole `cSUM` chunk at `out[at..]`; returns its length.
    async fn write_chunk<X: StreamIo>(
        mut self,
        x: &mut X,
        at: u64,
    ) -> Result<u64, crate::stream_encode::StreamEncodeError> {
        self.end_piece(x).await?;
        let mut first = Vec::new();
        for piece in self.held.chunks(1 << self.log2) {
            first.extend_from_slice(crate::checksum::digest_of(self.algo, [piece]).as_bytes());
        }
        let payload = (first.len() as u64)
            .checked_add(self.digests.len)
            .and_then(|n| n.checked_add(4 + crate::varint_len(self.covered) as u64))
            .ok_or(Error::TooLarge)?;
        let mut head = crate::checksum::CSUM.to_vec();
        put_varint(&mut head, payload);
        head.extend_from_slice(&[self.algo.wire(), self.algo.width() as u8, self.log2]);
        put_varint(&mut head, self.covered);
        head.extend_from_slice(&first);
        x.write(Stream::Output, at, &head).await?;
        let mut pos = at + head.len() as u64;
        {
            let mut replay = self.digests.replay();
            while let Some(bytes) = replay.next(x).await? {
                x.write(Stream::Output, pos, bytes).await?;
                pos += bytes.len() as u64;
            }
        }
        Ok(pos - at)
    }
}

/// [`PassIo`] for the streamed driver: the passes' rows from the source, the
/// nested records spooled to the index stream, pass 2's records written to
/// the output (and fed to the checksum) with their spans spooled after the
/// nested records.
#[cfg(feature = "std")]
struct Drive<'a, X> {
    x: &'a mut X,
    p: &'a Plan,
    /// The first stream error; the passes see `Error::Cancelled`.
    failure: Option<crate::stream_encode::StreamEncodeError>,
    spool: Range,
    exact_at: Vec<(u64, usize, crate::Digest)>,
    dc_at: Vec<(u64, usize, crate::Digest)>,
    /// Pass 2: the index entries, the checksum, the output position, and
    /// the `DATA` length so far.
    index: Option<Range>,
    sum: Option<StreamSum>,
    end: u64,
    data_len: u64,
    v4: Option<super::v4::encode::Encoder>,
    v4_directory: crate::v4::DirectoryPlacement,
}

#[cfg(feature = "std")]
impl<X: StreamIo> Drive<'_, X> {
    fn fail(&mut self, e: crate::stream_encode::StreamEncodeError) -> Error {
        // A codec error from the source (a cancellation, an out-of-range
        // sample) is the passes' own error; the rest is reported after.
        match e {
            crate::stream_encode::StreamEncodeError::Codec(e) => e,
            e => {
                self.failure = Some(e);
                Error::Cancelled
            }
        }
    }

    async fn drain_v4(&mut self) -> Result<(), crate::stream_encode::StreamEncodeError> {
        let (data, entries) = self
            .v4
            .as_mut()
            .ok_or(Error::Corrupt("missing bounded V4 writer"))?
            .take_output();
        if self.v4_directory != crate::v4::DirectoryPlacement::None {
            self.index
                .as_mut()
                .ok_or(Error::Corrupt("bounded V4 index order"))?
                .append(self.x, &entries)
                .await?;
        }
        self.put(&data).await?;
        self.data_len = self
            .data_len
            .checked_add(data.len() as u64)
            .ok_or(Error::TooLarge)?;
        self.v4.as_mut().unwrap().restore_output(data, entries);
        Ok(())
    }

    async fn start_v4(
        &mut self,
        mut writer: super::v4::encode::Encoder,
        capacity: u64,
    ) -> Result<Option<u64>, crate::stream_encode::StreamEncodeError> {
        use crate::v4::DirectoryPlacement;
        let immutable = writer.prefix_len();
        let (data, entries) = writer.take_output();
        if data.get(immutable) != Some(&1) {
            return Err(Error::Corrupt("bounded V4 prefix").into());
        }
        let start = self.end;
        self.put(&data[..immutable]).await?;
        let mut prefix = Vec::new();
        put_varint(&mut prefix, self.v4_directory.wire_value());
        if self.v4_directory == DirectoryPlacement::Front {
            put_varint(&mut prefix, capacity);
        }
        self.put(&prefix).await?;
        let reservation = if self.v4_directory == DirectoryPlacement::Front {
            let at = self.end;
            let zeros = [0; 8192];
            let mut left = capacity;
            while left != 0 {
                let n = left.min(zeros.len() as u64) as usize;
                self.put(&zeros[..n]).await?;
                left -= n as u64;
            }
            Some(at)
        } else {
            None
        };
        self.put(&data[immutable + 1..]).await?;
        if self.v4_directory != DirectoryPlacement::None {
            self.index
                .as_mut()
                .ok_or(Error::Corrupt("bounded V4 index order"))?
                .append(self.x, &entries)
                .await?;
        }
        self.data_len = self.end.checked_sub(start).ok_or(Error::TooLarge)?;
        writer.restore_output(data, entries);
        self.v4 = Some(writer);
        Ok(reservation)
    }

    /// Writes `bytes` at the output position, fed to the checksum first.
    async fn put(&mut self, bytes: &[u8]) -> Result<(), crate::stream_encode::StreamEncodeError> {
        if let Some(s) = &mut self.sum {
            s.feed(self.x, bytes).await?;
        }
        let next = self
            .end
            .checked_add(bytes.len() as u64)
            .ok_or(Error::TooLarge)?;
        self.x.write(Stream::Output, self.end, bytes).await?;
        self.end = next;
        Ok(())
    }
}

#[cfg(feature = "std")]
impl<X: StreamIo> PassIo for Drive<'_, X> {
    fn stop(&self) -> &dyn how_far::Stop {
        self.x.stop()
    }
    async fn rows(
        &mut self,
        pass: u8,
        by0: usize,
        by1: usize,
        band: &mut Vec<u8>,
    ) -> Result<usize, Error> {
        let p = self.p;
        let (y0, y1) = p.band_rows(by0, by1);
        let n = (y1 - y0).checked_mul(p.rb).ok_or(Error::TooLarge)?;
        band.clear();
        band.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
        band.resize(n, 0);
        if let Err(e) = self.x.source(pass, y0, band).await {
            return Err(self.fail(e));
        }
        if p.input.is_some() {
            let mut info = p.info.clone();
            info.height = (y1 - y0) as u32;
            precision::validate_pixels(&info, band, p.rb)?;
        }
        Ok(y0)
    }

    async fn store<'a>(&'a mut self, kind: Spooled, rec: &'a [u8]) -> Result<(), Error> {
        let entry = (
            self.spool.len,
            rec.len(),
            crate::checksum::digest_of(crate::HashAlgorithm::Xxh3_64, [rec]),
        );
        match kind {
            Spooled::Exact => self.exact_at.push(entry),
            Spooled::Dc => self.dc_at.push(entry),
        }
        if let Err(e) = self.spool.append(self.x, rec).await {
            return Err(self.fail(e));
        }
        Ok(())
    }

    async fn load(&mut self, kind: Spooled, t: usize, buf: &mut Vec<u8>) -> Result<(), Error> {
        let (at, len, digest) = match kind {
            Spooled::Exact => self.exact_at[t],
            Spooled::Dc => self.dc_at[t],
        };
        buf.clear();
        buf.resize(len, 0);
        let index = self
            .index
            .as_ref()
            .ok_or(Error::Corrupt("bounded load order"))?;
        if let Err(e) = index.read_before(self.x, self.spool.start + at, buf).await {
            return Err(self.fail(e));
        }
        if crate::checksum::digest_of(crate::HashAlgorithm::Xxh3_64, [&buf[..]]) != digest {
            return Err(Error::InvalidInput("bounded lossy exact scratch changed"));
        }
        Ok(())
    }

    async fn emit(&mut self, id: u64, payload: &[u8]) -> Result<(), Error> {
        if let Some(writer) = &mut self.v4 {
            writer.push(id, payload, None)?;
            return self.drain_v4().await.map_err(|e| self.fail(e));
        }

        use crate::parse::records::{framed_len, put_record};
        let mut framed = Vec::new();
        put_record(&mut framed, id, payload);
        let mut entry = Vec::new();
        put_varint(&mut entry, framed_len(id, payload.len()));
        let r = async {
            self.put(&framed).await?;
            let index = self
                .index
                .as_mut()
                .ok_or(Error::Corrupt("bounded emit order"))?;
            index.append(self.x, &entry).await
        }
        .await;
        match r {
            Ok(()) => {
                self.data_len += framed.len() as u64;
                Ok(())
            }
            Err(e) => Err(self.fail(e)),
        }
    }
    async fn emit_ac(&mut self, payload: &[u8], stripe: &StripeCoefs) -> Result<(), Error> {
        if let Some(writer) = &mut self.v4 {
            writer.push(crate::parse::records::R_AC, payload, Some(stripe))?;
            return self.drain_v4().await.map_err(|e| self.fail(e));
        }
        self.emit(crate::parse::records::R_AC, payload).await
    }
}

/// [`encode_lossy_bounded`] written as it is coded, over [`StreamIo`]: the
/// header and `DATA` head first (an open `DATA` chunk), then each band's
/// records. Spans go to the index stream (one varint per record, at most
/// `max_index_bytes` together with the spooled nested records, from
/// `index_start`); with [`StreamIndex::Reserved`] they are filled into the
/// head reservation after the last band. No `DATA` locator is written. A
/// configured file checksum is a streamed `cSUM` chunk in
/// [`STREAM_PIECE_LOG2`] pieces after the records (before a trailing
/// `uIDX`, SPEC 1.4): its digests spool to the checksum stream from
/// `checksum_start` (8 bytes per piece, at most `max_index_bytes`), and the
/// bytes up to the end of the reservation's piece stay in memory until the
/// reservation is filled. Returns the file length.
#[cfg(feature = "std")]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn encode_lossy_bounded_io<X: StreamIo>(
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    x: &mut X,
    index: StreamIndex,
    index_start: u64,
    checksum_start: Option<u64>,
    max_index_bytes: u64,
) -> Result<u64, crate::stream_encode::StreamEncodeError> {
    encode_lossy_stream_io(
        info,
        meta,
        cfg,
        x,
        index,
        index_start,
        checksum_start,
        max_index_bytes,
        Preparation::Measured,
    )
    .await
}

#[cfg(feature = "std")]
#[derive(Clone, Copy)]
enum Preparation {
    Measured,
    #[cfg(all(test, feature = "lab"))]
    StudyFirstBandDc,
    #[cfg(feature = "lab")]
    OnePass,
    #[cfg(feature = "lab")]
    Budgeted(one_pass::rate::Limits),
}

#[cfg(feature = "std")]
fn validate_stream_plan(
    p: &Plan,
    cfg: &LossyConfig,
    index: StreamIndex,
    checksum_start: Option<u64>,
    max_index_bytes: u64,
) -> Result<Option<usize>, crate::stream_encode::StreamEncodeError> {
    if cfg.checksum != checksum_start.is_some() {
        return Err(Error::InvalidInput(
            "checksum scratch is required exactly when the file checksum is on",
        )
        .into());
    }
    let nrec = p.record_count(false);
    let width = match index {
        StreamIndex::Reserved(w) if (1..=8).contains(&w) => Some(w as usize),
        StreamIndex::Reserved(_) => {
            return Err(Error::InvalidInput("unit table reservation width").into());
        }
        StreamIndex::Trailing | StreamIndex::V4 { .. } => None,
    };
    if (nrec as u64)
        .checked_mul(10)
        .is_none_or(|n| n > max_index_bytes)
    {
        return Err(Error::TooLarge.into());
    }
    Ok(width)
}

#[cfg(feature = "std")]
#[allow(clippy::too_many_arguments)]
async fn encode_lossy_stream_io<X: StreamIo>(
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    x: &mut X,
    index: StreamIndex,
    index_start: u64,
    checksum_start: Option<u64>,
    max_index_bytes: u64,
    preparation: Preparation,
) -> Result<u64, crate::stream_encode::StreamEncodeError> {
    let p = plan(info, cfg)?;
    let p = match preparation {
        Preparation::Measured => p,
        #[cfg(all(test, feature = "lab"))]
        Preparation::StudyFirstBandDc => one_pass::configure(p, cfg)?,
        #[cfg(feature = "lab")]
        Preparation::OnePass | Preparation::Budgeted(_) => one_pass::configure(p, cfg)?,
    };
    encode_planned_stream_io(
        p,
        info,
        meta,
        cfg,
        x,
        index,
        index_start,
        checksum_start,
        max_index_bytes,
        preparation,
    )
    .await
}

#[cfg(feature = "std")]
#[allow(clippy::too_many_arguments)]
async fn encode_planned_stream_io<X: StreamIo>(
    p: Plan,
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    x: &mut X,
    index: StreamIndex,
    index_start: u64,
    checksum_start: Option<u64>,
    max_index_bytes: u64,
    preparation: Preparation,
) -> Result<u64, crate::stream_encode::StreamEncodeError> {
    if let StreamIndex::V4 {
        directory,
        capacity,
    } = index
    {
        return encode_v4_stream_io(
            p,
            info,
            meta,
            cfg,
            x,
            index_start,
            checksum_start,
            max_index_bytes,
            directory,
            capacity,
        )
        .await;
    }
    use crate::parse::records::{
        H_UNIT_RESERVE, INDEX_FOOTER, RESERVE_FILLED, RESERVE_TAIL, UIDX, chunk_len_of, put_record,
    };
    let width = validate_stream_plan(&p, cfg, index, checksum_start, max_index_bytes)?;
    // Nested exact stripes and split DC records spool to the index stream
    // first (pass 1 codes them, pass 2 places them by index); the index
    // entries follow them. Both share `max_index_bytes`.
    let mut d = Drive {
        x,
        p: &p,
        failure: None,
        spool: Range::new(
            Stream::Index,
            index_start,
            max_index_bytes,
            "bounded lossy exact scratch changed",
        )?,
        exact_at: Vec::new(),
        dc_at: Vec::new(),
        index: None,
        sum: None,
        end: 0,
        data_len: 0,
        v4: None,
        v4_directory: crate::v4::DirectoryPlacement::Tail,
    };
    let r = match preparation {
        Preparation::Measured => pass1(&p, cfg, &mut d).await,
        #[cfg(all(test, feature = "lab"))]
        Preparation::StudyFirstBandDc => {
            let mut prepared = one_pass::prepare(&p, cfg, &mut d, false).await?;
            one_pass::train_first_band_dc(&p, &mut prepared)?;
            Ok(prepared)
        }
        #[cfg(feature = "lab")]
        Preparation::OnePass => one_pass::prepare(&p, cfg, &mut d, false).await,
        #[cfg(feature = "lab")]
        Preparation::Budgeted(_) => one_pass::prepare(&p, cfg, &mut d, true).await,
    };
    if let Some(e) = d.failure.take() {
        return Err(e);
    }
    let mut prep = r?;
    let nrec = p.record_count(prep.exact.is_some());
    let left = max_index_bytes
        .checked_sub(d.spool.len)
        .ok_or(Error::TooLarge)?;
    if (nrec as u64).checked_mul(10).is_none_or(|n| n > left) {
        return Err(Error::TooLarge.into());
    }
    d.index = Some(Range::new(
        Stream::Index,
        d.spool.start + d.spool.len,
        left,
        "bounded lossy index scratch changed",
    )?);

    // Head records in increasing id order: exact blocks, DC32, then the unit
    // table (record 12), whose payload offset the reservation fill needs.
    let mut records = Vec::new();
    if prep.exact.is_some() {
        put_record(&mut records, H_EXACT, &[]);
    }
    #[cfg(feature = "lab")]
    if matches!(preparation, Preparation::Budgeted(_)) {
        put_record(&mut records, H_AQ, &[]);
    }
    if !prep.fits16 {
        put_record(&mut records, H_DC32, &[]);
    }
    let reserve_len = match width {
        Some(w) => nrec
            .checked_mul(w)
            .and_then(|n| n.checked_add(2))
            .ok_or(Error::TooLarge)?,
        None => 1,
    };
    let mut reserve = Vec::new();
    reserve
        .try_reserve_exact(reserve_len)
        .map_err(|_| Error::TooLarge)?;
    reserve.resize(reserve_len, 0);
    reserve[0] = RESERVE_TAIL;
    put_record(&mut records, H_UNIT_RESERVE, &reserve);
    drop(reserve);
    let reserve_in_records = records.len() - reserve_len;
    match if cfg.coding_defaults {
        None
    } else {
        prep.merges.as_ref().map(|m| m.1)
    } {
        Some(m) if m.joint() => put_record(&mut records, small::H_PARTITIONS, &[]),
        Some(m) if m.merges_only() => put_record(&mut records, superblock::H_SUPERBLOCKS, &[]),
        _ => {}
    }
    if p.layouts != [LossyChannelLayout::default(); 3] {
        put_record(&mut records, H_LAYOUTS, &p.layouts.map(|l| l.byte()));
    }
    if cfg.local_ans_contexts {
        put_record(&mut records, local_ans::HEAD, &[]);
    }
    if cfg.coding_defaults {
        put_record(&mut records, coding_defaults::HEAD, &[1, 0, 0]);
    }
    if cfg.model_references {
        put_record(&mut records, model_refs::HEAD, &[1, 0]);
    }
    let (head, records_at) = data_head(&p, &prep, &records);
    drop(records);
    let mut header = file_header(info, meta, cfg, &p, &prep)?;
    if cfg.checksum {
        header = crate::container::with_extension(
            header,
            crate::checksum::EXT_CHECKSUM,
            Some(&[STREAM_PIECE_LOG2, cfg.hash.wire()]),
        )?;
    }
    let data_rel = header.len() as u64 + 5;
    let reserve_at = data_rel + (records_at + reserve_in_records) as u64;
    if let Some(start) = checksum_start {
        d.sum = Some(StreamSum::new(
            start,
            STREAM_PIECE_LOG2,
            cfg.hash,
            reserve_at + reserve_len as u64,
            max_index_bytes,
        )?);
    }
    d.put(&header).await?;
    d.put(b"DATA\0").await?;
    d.put(&head).await?;
    d.data_len = head.len() as u64;
    drop(head);

    // Pass 2, written through; each record's span spooled.
    let r = match preparation {
        #[cfg(feature = "lab")]
        Preparation::Budgeted(limits) => {
            one_pass::rate::pass2(&p, cfg, &mut prep, &mut d, limits).await
        }
        _ => pass2(&p, cfg, &mut prep, &mut d).await,
    };
    if let Some(e) = d.failure.take() {
        return Err(e);
    }
    r?;
    let Drive {
        x,
        index,
        mut sum,
        end,
        data_len,
        ..
    } = d;
    let index = index.ok_or(Error::Corrupt("bounded index"))?;

    // The delayed update: fill the head reservation from the spooled spans,
    // in 64 KiB writes, unless a span is too wide for it.
    if let Some(w) = width {
        let bits = 8 * w as u32;
        let mut overflow = false;
        let mut entries: Vec<u8> = Vec::new();
        let mut pending: Vec<u8> = Vec::new();
        let mut written = 0u64;
        {
            let mut replay = index.replay();
            while let Some(chunk) = replay.next(x).await? {
                pending.extend_from_slice(chunk);
                let mut r = ByteReader::new(&pending);
                let mut used = 0;
                // A varint cut by the chunk boundary stays pending.
                while let Ok(e) = r.varint() {
                    used = r.pos;
                    overflow |= bits < 64 && e >> bits != 0;
                    if !overflow {
                        entries.extend_from_slice(&e.to_le_bytes()[..w]);
                    }
                }
                pending.drain(..used);
                if !overflow && entries.len() >= 1 << 16 {
                    if let Some(s) = &mut sum {
                        s.patch(reserve_at + 2 + written, &entries);
                    }
                    x.write(Stream::Output, reserve_at + 2 + written, &entries)
                        .await?;
                    written += entries.len() as u64;
                    entries.clear();
                }
            }
        }
        if !pending.is_empty() {
            return Err(Error::Corrupt("bounded lossy index scratch").into());
        }
        if !overflow {
            if let Some(s) = &mut sum {
                s.patch(reserve_at + 2 + written, &entries);
                s.patch(reserve_at, &[RESERVE_FILLED, w as u8]);
            }
            x.write(Stream::Output, reserve_at + 2 + written, &entries)
                .await?;
            written += entries.len() as u64;
            if written != (nrec * w) as u64 {
                return Err(Error::Corrupt("bounded lossy unit table count").into());
            }
            x.write(Stream::Output, reserve_at, &[RESERVE_FILLED, w as u8])
                .await?;
            let n = match sum {
                Some(s) => s.write_chunk(x, end).await?,
                None => 0,
            };
            return Ok(end + n);
        }
        // Too wide: any entries already written are inside a reservation that
        // still says check tail (its first byte); the trailing index decides.
    }
    // Trailing `uIDX` (SPEC 2.7): the spooled entries, then the footer. A
    // checksum covers it (one replay) and goes just before it.
    let n = index.len + INDEX_FOOTER as u64;
    let mut prefix = UIDX.to_vec();
    put_varint(&mut prefix, n + 1);
    let mut footer = data_len.to_le_bytes().to_vec();
    footer.extend_from_slice(&chunk_len_of(n).to_le_bytes());
    footer.extend_from_slice(&UIDX);
    let mut at = end;
    if let Some(mut s) = sum {
        s.feed(x, &prefix).await?;
        {
            let mut replay = index.replay();
            while let Some(chunk) = replay.next(x).await? {
                s.feed(x, chunk).await?;
            }
        }
        s.feed(x, &footer).await?;
        at += s.write_chunk(x, at).await?;
    }
    x.write(Stream::Output, at, &prefix).await?;
    at += prefix.len() as u64;
    {
        let mut replay = index.replay();
        while let Some(chunk) = replay.next(x).await? {
            x.write(Stream::Output, at, chunk).await?;
            at += chunk.len() as u64;
        }
    }
    x.write(Stream::Output, at, &footer).await?;
    at += footer.len() as u64;
    Ok(at)
}

/// [`StreamIo`] over blocking streams: the source rows callback, a
/// `Write + Seek` output and `Read + Write + Seek` scratch, each offset
/// relative to where the stream stood when the encode began. A stream is
/// seeked only when an access does not continue the previous one, so a
/// buffered output is not flushed for every record.
#[cfg(feature = "std")]
struct Blocking<'a, W, S, C> {
    stop: &'a dyn how_far::Stop,
    rows: &'a mut dyn FnMut(u8, usize, &mut [u8]) -> Result<(), Error>,
    out: &'a mut W,
    out_base: u64,
    out_at: u64,
    scratch: S,
    scratch_at: Option<u64>,
    sums: Option<C>,
    sums_at: Option<u64>,
}

#[cfg(feature = "std")]
fn seek_to<T: std::io::Seek>(
    s: &mut T,
    at: &mut Option<u64>,
    to: u64,
) -> Result<(), crate::stream_encode::StreamEncodeError> {
    if *at != Some(to) {
        s.seek(std::io::SeekFrom::Start(to))?;
    }
    Ok(())
}

#[cfg(feature = "std")]
impl<W, S, C> StreamIo for Blocking<'_, W, S, C>
where
    W: std::io::Write + std::io::Seek,
    S: std::io::Read + std::io::Write + std::io::Seek,
    C: std::io::Read + std::io::Write + std::io::Seek,
{
    fn stop(&self) -> &dyn how_far::Stop {
        self.stop
    }
    async fn source(
        &mut self,
        pass: u8,
        y0: usize,
        out: &mut Vec<u8>,
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        self.stop.check().map_err(|_| Error::Cancelled)?;
        (self.rows)(pass, y0, out)?;
        self.stop.check().map_err(|_| Error::Cancelled)?;
        Ok(())
    }

    async fn write(
        &mut self,
        stream: Stream,
        at: u64,
        bytes: &[u8],
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        self.stop.check().map_err(|_| Error::Cancelled)?;
        match stream {
            Stream::Output => {
                let to = self.out_base + at;
                if self.out_at != to {
                    self.out.seek(std::io::SeekFrom::Start(to))?;
                }
                self.out.write_all(bytes)?;
                self.out_at = to + bytes.len() as u64;
            }
            Stream::Index => {
                seek_to(&mut self.scratch, &mut self.scratch_at, at)?;
                self.scratch.write_all(bytes)?;
                self.scratch_at = Some(at + bytes.len() as u64);
            }
            Stream::Checksum => {
                let c = self
                    .sums
                    .as_mut()
                    .ok_or(Error::Corrupt("bounded checksum stream"))?;
                seek_to(c, &mut self.sums_at, at)?;
                c.write_all(bytes)?;
                self.sums_at = Some(at + bytes.len() as u64);
            }
        }
        self.stop.check().map_err(|_| Error::Cancelled)?;
        Ok(())
    }

    async fn read(
        &mut self,
        stream: Stream,
        at: u64,
        buf: &mut [u8],
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        self.stop.check().map_err(|_| Error::Cancelled)?;
        use crate::stream_encode::StreamEncodeError;
        let (s, pos): (&mut dyn ReadSeekWrite, &mut Option<u64>) = match stream {
            Stream::Output => return Err(Error::Corrupt("bounded output read").into()),
            Stream::Index => (&mut self.scratch, &mut self.scratch_at),
            Stream::Checksum => (
                self.sums
                    .as_mut()
                    .ok_or(Error::Corrupt("bounded checksum stream"))?,
                &mut self.sums_at,
            ),
        };
        s.flush()?;
        if *pos != Some(at) {
            s.seek(std::io::SeekFrom::Start(at))?;
        }
        s.read_exact(buf).map_err(StreamEncodeError::Read)?;
        *pos = Some(at + buf.len() as u64);
        self.stop.check().map_err(|_| Error::Cancelled)?;
        Ok(())
    }
}

#[cfg(feature = "std")]
trait ReadSeekWrite: std::io::Read + std::io::Write + std::io::Seek {}
#[cfg(feature = "std")]
impl<T: std::io::Read + std::io::Write + std::io::Seek> ReadSeekWrite for T {}

/// The existing measured band pipeline with V4 serialization and caller scratch.
#[cfg(feature = "std")]
async fn encode_v4_stream_io<X: StreamIo>(
    p: Plan,
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    x: &mut X,
    index_start: u64,
    checksum_start: Option<u64>,
    max_index_bytes: u64,
    directory: crate::v4::DirectoryPlacement,
    capacity: u64,
) -> Result<u64, crate::stream_encode::StreamEncodeError> {
    use crate::v4::DirectoryPlacement;
    if directory == DirectoryPlacement::Front {
        if capacity == 0 {
            return Err(Error::InvalidInput("zero V4 front reservation").into());
        }
        if capacity > max_index_bytes {
            return Err(Error::TooLarge.into());
        }
    } else if capacity != 0 {
        return Err(Error::InvalidInput("V4 reservation without front placement").into());
    }
    validate_stream_plan(
        &p,
        cfg,
        StreamIndex::Trailing,
        checksum_start,
        max_index_bytes,
    )?;
    let mut d = Drive {
        x,
        p: &p,
        failure: None,
        spool: Range::new(
            Stream::Index,
            index_start,
            max_index_bytes,
            "bounded lossy exact scratch changed",
        )?,
        exact_at: Vec::new(),
        dc_at: Vec::new(),
        index: None,
        sum: None,
        end: 0,
        data_len: 0,
        v4: None,
        v4_directory: crate::v4::DirectoryPlacement::Tail,
    };
    let measured = pass1(&p, cfg, &mut d).await;
    if let Some(error) = d.failure.take() {
        return Err(error);
    }
    let mut prep = measured?;
    let left = max_index_bytes
        .checked_sub(d.spool.len)
        .ok_or(Error::TooLarge)?;
    d.index = Some(Range::new(
        Stream::Index,
        d.spool.start + d.spool.len,
        left,
        "bounded V4 directory scratch changed",
    )?);
    d.v4_directory = directory;
    let exact = if let Some(exact) = &prep.exact {
        let mut r = ByteReader::new(&exact.head);
        let blocks_per_row = r.var_usize()?;
        Some(super::v4::encode::ExactInput {
            blocks_per_row,
            counts: &exact.counts,
            model: &exact.head[exact.model.clone()],
            lengths: &exact.head[exact.model.end..],
            stripes: exact.stripes,
        })
    } else {
        None
    };
    let maps = prep.merges.as_ref().map(|m| m.1).unwrap_or_default();
    let writer = super::v4::encode::Encoder::new(super::v4::encode::Input {
        info,
        color: p.color,
        rows: p.sb as usize,
        columns: p.gcols,
        params: envelope::Params {
            scale: p.scale,
            qp: p.qp,
            entropy: prep.entropy,
            compact: false,
            seed: crate::seeds::Selection::NONE,
            records: false,
        },
        layouts: p.layouts,
        dc_rows: p.dc_rows,
        ac_head: &prep.tb,
        dc_head: &prep.dc_head,
        alpha_head: prep.alpha.as_ref().map(|(_, head)| head.as_slice()),
        exact,
        control: if prep.exact.is_some() { 2 } else { 0 },
        dc32: !prep.fits16,
        scoped: cfg.coding_defaults,
        local_ans: cfg.local_ans_contexts,
        references: cfg.model_references,
        partitioned: maps.merge || maps.split,
        retained: &[],
        max_bytes: usize::MAX,
    })?;
    let legacy_header = file_header(info, meta, cfg, &p, &prep)?;
    let hp = crate::parse::header::read_header_sized(&mut ByteReader::new(&legacy_header))?;
    let features = crate::Features::from_bits(hp.declared.bits() & !(0x3f << 26))
        .ok_or(Error::Corrupt("bounded V4 features"))?;
    let mut header = Vec::new();
    crate::container::write_group_head_features(
        &mut header,
        info,
        meta,
        crate::Coding::Lossy,
        Some(p.color),
        cfg.decode_speed(),
        features,
        4,
    )?;
    if cfg.checksum {
        header = crate::container::with_extension(
            header,
            crate::checksum::EXT_CHECKSUM,
            Some(&[STREAM_PIECE_LOG2, cfg.hash.wire()]),
        )?;
        let hold = if directory == DirectoryPlacement::Front {
            (header.len() as u64)
                .checked_add(5)
                .and_then(|n| n.checked_add(writer.prefix_len() as u64))
                .and_then(|n| n.checked_add(1 + crate::varint_len(capacity) as u64))
                .and_then(|n| n.checked_add(capacity))
                .ok_or(Error::TooLarge)?
        } else {
            0
        };
        d.sum = Some(StreamSum::new(
            checksum_start.ok_or(Error::InvalidInput("missing checksum scratch"))?,
            STREAM_PIECE_LOG2,
            cfg.hash,
            hold,
            max_index_bytes,
        )?);
    }
    d.put(&header).await?;
    d.put(b"DATA\0").await?;
    let reservation = d.start_v4(writer, capacity).await?;
    let encoded = pass2(&p, cfg, &mut prep, &mut d).await;
    if let Some(error) = d.failure.take() {
        return Err(error);
    }
    encoded?;
    let (count, objects_len) = d.v4.as_mut().unwrap().finish()?;
    d.drain_v4().await?;
    let Drive {
        x,
        index,
        mut sum,
        mut end,
        ..
    } = d;
    let index = index.ok_or(Error::Corrupt("missing bounded V4 directory"))?;
    if directory != DirectoryPlacement::Tail {
        if directory == DirectoryPlacement::Front {
            let body_len = (crate::varint_len(count) as u64)
                .checked_add(index.len)
                .and_then(|n| n.checked_add(8))
                .ok_or(Error::TooLarge)?;
            if body_len
                .checked_add(crate::varint_len(body_len) as u64)
                .is_none_or(|n| n > capacity)
            {
                return Err(Error::TooLarge.into());
            }
            let mut at = reservation.ok_or(Error::Corrupt("missing V4 front reservation"))?;
            let mut prefix = Vec::new();
            put_varint(&mut prefix, body_len);
            put_varint(&mut prefix, count);
            if let Some(checksum) = &mut sum {
                checksum.patch(at, &prefix);
            }
            x.write(Stream::Output, at, &prefix).await?;
            at = at.checked_add(prefix.len() as u64).ok_or(Error::TooLarge)?;
            let mut replay = index.replay();
            while let Some(bytes) = replay.next(x).await? {
                if let Some(checksum) = &mut sum {
                    checksum.patch(at, bytes);
                }
                x.write(Stream::Output, at, bytes).await?;
                at = at.checked_add(bytes.len() as u64).ok_or(Error::TooLarge)?;
            }
            let suffix = objects_len.to_le_bytes();
            if let Some(checksum) = &mut sum {
                checksum.patch(at, &suffix);
            }
            x.write(Stream::Output, at, &suffix).await?;
        }
        if let Some(checksum) = sum {
            end = end
                .checked_add(checksum.write_chunk(x, end).await?)
                .ok_or(Error::TooLarge)?;
        }
        return Ok(end);
    }
    let payload_len = (crate::varint_len(count) as u64)
        .checked_add(index.len)
        .and_then(|n| n.checked_add(20))
        .ok_or(Error::TooLarge)?;
    let length = payload_len.checked_add(1).ok_or(Error::TooLarge)?;
    let chunk_len = (4 + crate::varint_len(length) as u64)
        .checked_add(payload_len)
        .ok_or(Error::TooLarge)?;
    let mut head = b"uIDX".to_vec();
    put_varint(&mut head, length);
    put_varint(&mut head, count);
    let mut suffix = objects_len.to_le_bytes().to_vec();
    suffix.extend_from_slice(&chunk_len.to_le_bytes());
    suffix.extend_from_slice(b"uIDX");
    if let Some(checksum) = &mut sum {
        checksum.feed(x, &head).await?;
        let mut replay = index.replay();
        while let Some(bytes) = replay.next(x).await? {
            checksum.feed(x, bytes).await?;
        }
        checksum.feed(x, &suffix).await?;
    }
    if let Some(checksum) = sum {
        end += checksum.write_chunk(x, end).await?;
    }
    x.write(Stream::Output, end, &head).await?;
    end += head.len() as u64;
    let mut replay = index.replay();
    while let Some(bytes) = replay.next(x).await? {
        x.write(Stream::Output, end, bytes).await?;
        end = end.checked_add(bytes.len() as u64).ok_or(Error::TooLarge)?;
    }
    x.write(Stream::Output, end, &suffix).await?;
    end.checked_add(suffix.len() as u64)
        .ok_or(Error::TooLarge.into())
}

/// [`encode_lossy_bounded_io`] over blocking streams: rows from `rows`,
/// the file to `out` from its current position, the index to `scratch` and
/// checksum digests to `checksum_scratch`, each appended after what they
/// hold. Returns both scratch objects.
#[cfg(feature = "std")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn encode_lossy_bounded_to<W, S, C>(
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    rows: &mut dyn FnMut(u8, usize, &mut [u8]) -> Result<(), Error>,
    out: &mut W,
    index: StreamIndex,
    mut scratch: S,
    mut checksum_scratch: Option<C>,
    max_index_bytes: u64,
    stop: &dyn how_far::Stop,
) -> Result<(S, Option<C>), crate::stream_encode::StreamEncodeError>
where
    W: std::io::Write + std::io::Seek,
    S: std::io::Read + std::io::Write + std::io::Seek,
    C: std::io::Read + std::io::Write + std::io::Seek,
{
    use std::io::SeekFrom;
    stop.check().map_err(|_| Error::Cancelled)?;
    let index_start = scratch.seek(SeekFrom::End(0))?;
    let checksum_start = match &mut checksum_scratch {
        Some(c) => Some(c.seek(SeekFrom::End(0))?),
        None => None,
    };
    let out_base = out.stream_position()?;
    let mut b = Blocking {
        stop,
        rows,
        out,
        out_base,
        out_at: out_base,
        scratch,
        scratch_at: Some(index_start),
        sums_at: checksum_start,
        sums: checksum_scratch,
    };
    let end = crate::parse::payload::ready(encode_lossy_bounded_io(
        info,
        meta,
        cfg,
        &mut b,
        index,
        index_start,
        checksum_start,
        max_index_bytes,
    ))?;
    let to = b.out_base + end;
    if b.out_at != to {
        b.out.seek(SeekFrom::Start(to))?;
    }
    b.out.flush()?;
    Ok((b.scratch, b.sums))
}

/// One pending request of a [`Session`]: what the encode waits for.
#[cfg(feature = "std")]
enum Request {
    Idle,
    Rows {
        pass: u8,
        y0: usize,
        buf: Vec<u8>,
    },
    Write {
        stream: Stream,
        at: u64,
        buf: Vec<u8>,
    },
    Read {
        stream: Stream,
        at: u64,
        buf: Vec<u8>,
    },
}

/// Shared between a [`Session`] and the [`Port`] its encode runs on.
#[cfg(feature = "std")]
struct Mailbox {
    request: Request,
    /// The caller has handled `request` (its buffer holds read results).
    answered: bool,
}

// Guards never span an await, a native poll, caller work or buffer destruction.
// A poisoned mailbox cannot provide a trustworthy answer; fail the session.
#[cfg(feature = "std")]
fn lock_mailbox(
    mail: &std::sync::Mutex<Mailbox>,
) -> Result<std::sync::MutexGuard<'_, Mailbox>, crate::stream_encode::StreamEncodeError> {
    mail.lock()
        .map_err(|_| Error::Corrupt("bounded session mailbox poisoned").into())
}

/// [`StreamIo`] that posts each access to the mailbox and waits until the
/// session's caller has handled it.
#[cfg(feature = "std")]
struct Port {
    mail: std::sync::Arc<std::sync::Mutex<Mailbox>>,
    /// Reused request buffer.
    spare: Vec<u8>,
    #[cfg(feature = "lab")]
    output_limit: Option<u64>,
}

#[cfg(feature = "std")]
impl Port {
    /// Posts `request`, waits for the answer, and returns its buffer.
    async fn ask(
        &mut self,
        request: Request,
    ) -> Result<Vec<u8>, crate::stream_encode::StreamEncodeError> {
        {
            let previous = {
                let mut m = lock_mailbox(&self.mail)?;
                m.answered = false;
                core::mem::replace(&mut m.request, request)
            };
            drop(previous);
        }
        core::future::poll_fn(|_| {
            let mut m = match lock_mailbox(&self.mail) {
                Ok(m) => m,
                Err(error) => return core::task::Poll::Ready(Err(error)),
            };
            if !m.answered {
                return core::task::Poll::Pending;
            }
            match core::mem::replace(&mut m.request, Request::Idle) {
                Request::Rows { buf, .. }
                | Request::Write { buf, .. }
                | Request::Read { buf, .. } => core::task::Poll::Ready(Ok(buf)),
                Request::Idle => core::task::Poll::Ready(Err(Error::Corrupt(
                    "bounded session answered nothing",
                )
                .into())),
            }
        })
        .await
    }

    fn buffer(&mut self, len: usize) -> Result<Vec<u8>, Error> {
        let mut buf = core::mem::take(&mut self.spare);
        buf.clear();
        buf.try_reserve(len).map_err(|_| Error::TooLarge)?;
        Ok(buf)
    }
}

#[cfg(feature = "std")]
impl StreamIo for Port {
    async fn source(
        &mut self,
        pass: u8,
        y0: usize,
        out: &mut Vec<u8>,
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        // The band itself goes to the caller and comes back filled.
        let buf = core::mem::take(out);
        let len = buf.len();
        *out = self.ask(Request::Rows { pass, y0, buf }).await?;
        if out.len() != len {
            return Err(Error::InvalidInput("bounded session row buffer length").into());
        }
        Ok(())
    }

    async fn write(
        &mut self,
        stream: Stream,
        at: u64,
        bytes: &[u8],
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        #[cfg(feature = "lab")]
        if stream == Stream::Output
            && self.output_limit.is_some_and(|limit| {
                at.checked_add(bytes.len() as u64)
                    .is_none_or(|end| end > limit)
            })
        {
            return Err(Error::TooLarge.into());
        }
        let mut buf = self.buffer(bytes.len())?;
        buf.extend_from_slice(bytes);
        self.spare = self.ask(Request::Write { stream, at, buf }).await?;
        Ok(())
    }

    async fn read(
        &mut self,
        stream: Stream,
        at: u64,
        out: &mut [u8],
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        let mut buf = self.buffer(out.len())?;
        buf.resize(out.len(), 0);
        let buf = self.ask(Request::Read { stream, at, buf }).await?;
        out.copy_from_slice(&buf);
        self.spare = buf;
        Ok(())
    }
}

/// What a [`Session`] needs next.
#[cfg(feature = "std")]
pub(crate) enum Step<'a> {
    /// Fill `rows` with source rows from `y0` (whole rows, tightly packed),
    /// for `pass`.
    Rows {
        pass: u8,
        y0: usize,
        rows: &'a mut [u8],
    },
    /// Write `bytes` at offset `at` of `stream`.
    Write {
        stream: Stream,
        at: u64,
        bytes: &'a [u8],
    },
    /// Read `buf.len()` bytes at offset `at` of `stream` into `buf`.
    Read {
        stream: Stream,
        at: u64,
        buf: &'a mut [u8],
    },
    /// The encode is complete: the output is `len` bytes long.
    Done { len: u64 },
}

/// The exact validated native preparation, with sample tables and session state
/// deferred until start. Metadata is moved in, never copied during preparation.
#[cfg(feature = "std")]
pub(crate) struct SessionPlan {
    preparation: NativePreparation,
    info: ImageInfo,
    meta: Metadata,
    cfg: LossyConfig,
    index: StreamIndex,
    max_index_bytes: u64,
    passes: (u32, u32, u32),
    mode: Preparation,
}

#[cfg(feature = "std")]
impl SessionPlan {
    pub(crate) fn new(
        info: ImageInfo,
        meta: Metadata,
        cfg: LossyConfig,
        index: StreamIndex,
        max_index_bytes: u64,
    ) -> Result<Self, crate::stream_encode::StreamEncodeError> {
        Self::prepared(
            info,
            meta,
            cfg,
            index,
            max_index_bytes,
            Preparation::Measured,
        )
    }

    #[cfg(feature = "lab")]
    pub(crate) fn one_pass(
        info: ImageInfo,
        meta: Metadata,
        cfg: LossyConfig,
        max_index_bytes: u64,
        max_working_bytes: u64,
    ) -> Result<Self, crate::stream_encode::StreamEncodeError> {
        one_pass::admit(&info, &meta, &cfg, max_index_bytes, max_working_bytes)?;
        Self::prepared(
            info,
            meta,
            cfg,
            StreamIndex::Trailing,
            max_index_bytes,
            Preparation::OnePass,
        )
    }

    #[cfg(feature = "lab")]
    pub(crate) fn budgeted(
        info: ImageInfo,
        meta: Metadata,
        cfg: LossyConfig,
        max_index_bytes: u64,
        max_working_bytes: u64,
        limits: one_pass::rate::Limits,
    ) -> Result<Self, crate::stream_encode::StreamEncodeError> {
        one_pass::rate::admit(
            &info,
            &meta,
            &cfg,
            max_index_bytes,
            max_working_bytes,
            limits,
        )?;
        Self::prepared(
            info,
            meta,
            cfg,
            StreamIndex::Trailing,
            max_index_bytes,
            Preparation::Budgeted(limits),
        )
    }

    fn prepared(
        info: ImageInfo,
        meta: Metadata,
        cfg: LossyConfig,
        index: StreamIndex,
        max_index_bytes: u64,
        mode: Preparation,
    ) -> Result<Self, crate::stream_encode::StreamEncodeError> {
        let preparation = prepare_plan(&info, &cfg)?;
        let (preparation, min, max) = match mode {
            #[cfg(all(test, feature = "lab"))]
            Preparation::StudyFirstBandDc => {
                unreachable!("study uses the private stream driver")
            }
            Preparation::Measured => {
                let (min, max) = preparation.plan.source_passes(&cfg);
                (preparation, min, max)
            }
            #[cfg(feature = "lab")]
            Preparation::OnePass | Preparation::Budgeted(_) => {
                let NativePreparation {
                    plan,
                    lut16_transfer,
                    precision_input,
                } = preparation;
                (
                    NativePreparation {
                        plan: one_pass::configure(plan, &cfg)?,
                        lut16_transfer,
                        precision_input,
                    },
                    1,
                    1,
                )
            }
        };
        validate_stream_plan(
            &preparation.plan,
            &cfg,
            index,
            cfg.checksum.then_some(0),
            max_index_bytes,
        )?;
        let band_rows = (preparation.plan.work * 8).min(preparation.plan.g.h) as u32;
        Ok(Self {
            preparation,
            info,
            meta,
            cfg,
            index,
            max_index_bytes,
            passes: (min, max, band_rows),
            mode,
        })
    }

    pub(crate) fn passes(&self) -> (u32, u32, u32) {
        self.passes
    }
    pub(crate) fn info(&self) -> &ImageInfo {
        &self.info
    }
    pub(crate) fn metadata(&self) -> &Metadata {
        &self.meta
    }

    pub(crate) fn start(self) -> Result<Session, crate::stream_encode::StreamEncodeError> {
        Session::start_planned(
            self.info,
            self.meta,
            self.cfg,
            self.index,
            self.max_index_bytes,
            self.mode,
            self.preparation.materialize()?,
            self.passes,
        )
    }
}

/// A bounded lossy encode as a state machine. No lock spans native work or a
/// returned request. The suspended future and its owned request buffers are Send.
#[cfg(feature = "std")]
pub(crate) struct Session {
    run: Option<
        core::pin::Pin<
            alloc::boxed::Box<
                dyn core::future::Future<
                        Output = Result<u64, crate::stream_encode::StreamEncodeError>,
                    > + Send,
            >,
        >,
    >,
    mail: std::sync::Arc<std::sync::Mutex<Mailbox>>,
    held: Option<Request>,
    done: Option<u64>,
    failed: bool,
    // The first explicit caller failure is returned unchanged on the next step.
    failure: Option<crate::stream_encode::StreamEncodeError>,
    passes: (u32, u32, u32),
}

#[cfg(feature = "std")]
impl Session {
    pub(crate) fn new(
        info: ImageInfo,
        meta: Metadata,
        cfg: LossyConfig,
        index: StreamIndex,
        max_index_bytes: u64,
    ) -> Result<Self, crate::stream_encode::StreamEncodeError> {
        Self::prepared(
            info,
            meta,
            cfg,
            index,
            max_index_bytes,
            Preparation::Measured,
        )
    }

    #[cfg(feature = "lab")]
    pub(crate) fn one_pass(
        info: ImageInfo,
        meta: Metadata,
        cfg: LossyConfig,
        max_index_bytes: u64,
        max_working_bytes: u64,
    ) -> Result<Self, crate::stream_encode::StreamEncodeError> {
        SessionPlan::one_pass(info, meta, cfg, max_index_bytes, max_working_bytes)?.start()
    }

    #[cfg(feature = "lab")]
    pub(crate) fn budgeted(
        info: ImageInfo,
        meta: Metadata,
        cfg: LossyConfig,
        max_index_bytes: u64,
        max_working_bytes: u64,
        limits: one_pass::rate::Limits,
    ) -> Result<Self, crate::stream_encode::StreamEncodeError> {
        SessionPlan::budgeted(info, meta, cfg, max_index_bytes, max_working_bytes, limits)?.start()
    }

    fn prepared(
        info: ImageInfo,
        meta: Metadata,
        cfg: LossyConfig,
        index: StreamIndex,
        max_index_bytes: u64,
        preparation: Preparation,
    ) -> Result<Self, crate::stream_encode::StreamEncodeError> {
        // Refuse before the first request what the encode would refuse.
        let p = plan(&info, &cfg)?;
        let (p, min, max) = match preparation {
            #[cfg(all(test, feature = "lab"))]
            Preparation::StudyFirstBandDc => {
                unreachable!("study uses the private stream driver")
            }
            Preparation::Measured => {
                let (min, max) = p.source_passes(&cfg);
                (p, min, max)
            }
            #[cfg(feature = "lab")]
            Preparation::OnePass | Preparation::Budgeted(_) => {
                // Configure first: one-pass source bands follow the stripe
                // height, not the measured encoder's smaller work bands.
                (one_pass::configure(p, &cfg)?, 1, 1)
            }
        };
        let passes = (min, max, (p.work * 8).min(p.g.h) as u32);
        Self::start_planned(
            info,
            meta,
            cfg,
            index,
            max_index_bytes,
            preparation,
            p,
            passes,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn start_planned(
        info: ImageInfo,
        meta: Metadata,
        cfg: LossyConfig,
        index: StreamIndex,
        max_index_bytes: u64,
        preparation: Preparation,
        p: Plan,
        passes: (u32, u32, u32),
    ) -> Result<Self, crate::stream_encode::StreamEncodeError> {
        let mail = std::sync::Arc::new(std::sync::Mutex::new(Mailbox {
            request: Request::Idle,
            answered: false,
        }));
        let mut port = Port {
            mail: mail.clone(),
            spare: Vec::new(),
            #[cfg(feature = "lab")]
            output_limit: match preparation {
                Preparation::Budgeted(limits) => Some(limits.frame_bytes),
                _ => None,
            },
        };
        let checksum = cfg.checksum.then_some(0);
        let run = alloc::boxed::Box::pin(async move {
            encode_planned_stream_io(
                p,
                &info,
                &meta,
                &cfg,
                &mut port,
                index,
                0,
                checksum,
                max_index_bytes,
                preparation,
            )
            .await
        });
        Ok(Self {
            run: Some(run),
            mail,
            held: None,
            done: None,
            failed: false,
            failure: None,
            passes,
        })
    }

    pub(crate) fn passes(&self) -> (u32, u32, u32) {
        self.passes
    }

    fn answer(&mut self) -> Result<(), crate::stream_encode::StreamEncodeError> {
        if let Some(request) = self.held.take() {
            let previous = {
                let mut mail = lock_mailbox(&self.mail)?;
                mail.answered = true;
                core::mem::replace(&mut mail.request, request)
            };
            drop(previous);
        }
        Ok(())
    }

    // Native polling happens here, outside the mutex. All errors flow through
    // step(), which drops the future/buffers and prevents any subsequent poll.
    fn advance(&mut self) -> Result<(), crate::stream_encode::StreamEncodeError> {
        use core::task::{Context, Poll, Waker};
        self.answer()?;
        match self
            .run
            .as_mut()
            .expect("active session")
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(result) => {
                self.done = Some(result?);
                self.run = None;
                return Ok(());
            }
            Poll::Pending => {}
        }
        let request = {
            let mut mail = lock_mailbox(&self.mail)?;
            core::mem::replace(&mut mail.request, Request::Idle)
        };
        if matches!(request, Request::Idle) {
            return Err(Error::Corrupt("bounded session suspended without a request").into());
        }
        self.held = Some(request);
        Ok(())
    }

    fn terminate(&mut self) {
        self.failed = true;
        self.run = None;
        self.held = None;
        // Once the future is dropped this is the only Arc. Recover poisoned
        // storage solely to release its payload, never to resume the protocol.
        let request = {
            let mut mail = self.mail.lock().unwrap_or_else(|e| e.into_inner());
            core::mem::replace(&mut mail.request, Request::Idle)
        };
        drop(request);
    }

    /// Borrow the current unanswered row request without completing it. The
    /// common adapter uses this after publishing the preceding completed band.
    pub(crate) fn pending_rows(&mut self) -> Option<(u8, usize, &mut [u8])> {
        if self.failed || self.done.is_some() {
            return None;
        }
        match self.held.as_mut()? {
            Request::Rows { pass, y0, buf } => Some((*pass, *y0, buf)),
            _ => None,
        }
    }

    /// Completes the previous step. Done is fused; after the first typed error,
    /// later calls return a state error without polling the completed future.
    pub(crate) fn step(&mut self) -> Result<Step<'_>, crate::stream_encode::StreamEncodeError> {
        if let Some(error) = self.failure.take() {
            return Err(error);
        }
        if self.failed {
            return Err(Error::Corrupt("bounded session failed").into());
        }
        if let Some(len) = self.done {
            return Ok(Step::Done { len });
        }
        if let Err(error) = self.advance() {
            self.terminate();
            return Err(error);
        }
        if let Some(len) = self.done {
            return Ok(Step::Done { len });
        }
        Ok(match self.held.as_mut() {
            Some(Request::Rows { pass, y0, buf }) => Step::Rows {
                pass: *pass,
                y0: *y0,
                rows: buf,
            },
            Some(Request::Write { stream, at, buf }) => Step::Write {
                stream: *stream,
                at: *at,
                bytes: buf,
            },
            Some(Request::Read { stream, at, buf }) => Step::Read {
                stream: *stream,
                at: *at,
                buf,
            },
            _ => unreachable!("advance validated the request"),
        })
    }

    /// Latch the first caller failure and release suspended native buffers now.
    /// Even a future that swallowed an I/O error cannot be resumed after this.
    pub(crate) fn fail(&mut self, error: crate::stream_encode::StreamEncodeError) {
        if self.failed || self.done.is_some() {
            return;
        }
        self.failure = Some(error);
        self.terminate();
    }
}

#[cfg(test)]
mod tests;
