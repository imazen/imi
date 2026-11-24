//! Closed-loop spatial steering for IMI lossy encodes (lab): encode,
//! decode, a metric's per-pixel error map, per-block AC step multipliers in
//! the DC image's AQ channel (`ImiEncoderConfig::aq_map`, a/64), repeat until the
//! target holds. After zenjpeg's `zq` loop: global quality first (bracketed
//! secant), then per-block redistribution with a quality correction each
//! pass, keeping the smallest file that meets the target.
//!
//! ```text
//! imij-cmp steer-batch JOBS_JSON
//! ```
//!
//! Each job: `{"source", "width", "height", "output", "spec"}`; `spec` holds
//! `metric` (zensim | ssim2 | ba-max | ba-p3 | plain) and its knobs (see
//! [`Spec`]). `plain` encodes `qualities` without steering. Every final file
//! is cross-scored with zensim, SSIMULACRA2, Butteraugli and CVVDP.
use crate::rd::{Result, hash, save};
use serde_json::{Value, json};
use std::{fs, path::Path, sync::Arc, time::Instant};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Metric {
    Zensim,
    Ssim2,
    BaMax,
    BaP3,
}

impl Metric {
    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "zensim" => Self::Zensim,
            "ssim2" => Self::Ssim2,
            "ba-max" => Self::BaMax,
            "ba-p3" => Self::BaP3,
            _ => return Err(format!("unknown steering metric {s}").into()),
        })
    }
    fn higher_better(self) -> bool {
        matches!(self, Self::Zensim | Self::Ssim2)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Alloc {
    Equal,
    Rate,
    Lambda,
}

/// Steering knobs. Multipliers live in log2 units ("octaves"): the AQ code
/// of a block is round(64 * 2^l).
#[derive(Clone, Debug)]
pub(crate) struct Spec {
    metric: Metric,
    target: f64,
    /// Acceptance band above the target (metric units): a pass with margin
    /// in [0, tol] ends the global search.
    tol: f64,
    /// Encode passes for the global quality search (uniform map).
    global_passes: usize,
    /// Redistribution passes after it (0 = global search only).
    steer_passes: usize,
    /// Map update gain: l += -gain * log2(block error / reference).
    gain: f64,
    /// Largest change of l per pass (octaves).
    step: f64,
    /// Updates smaller than this (octaves) are dropped, so cells without a
    /// clear signal stay at x1 and the map stays sparse (cheap to code).
    deadzone: f64,
    /// Range of l (octaves): finer to coarser.
    lo: f64,
    hi: f64,
    /// Multiplier levels per octave (codes snap to 2^(k / levels)).
    levels: f64,
    /// Map cells are grain x grain blocks (one value each).
    grain: usize,
    /// Allocation rule. `equal`: iterate toward equal cell errors (the
    /// target for Butteraugli max, else the mean). `rate`: the mean times
    /// the cell's share of nonzero AC coefficients in the uniform pass
    /// (under D ~ m^a and dR/dlog2(m) ~ -nonzeros the RD optimum has D
    /// proportional to the nonzero count). `lambda` (additive metrics):
    /// one map from the cells' marginal returns between two uniform passes,
    /// lambda = error drop / estimated bit increase; cells with high lambda
    /// get finer steps (the equal-slope condition); later passes only
    /// correct the quality.
    alloc: Alloc,
    /// `lambda` allocation: l = -strength * log2(lambda / median lambda).
    strength: f64,
    /// Starting quality (the bracket grows by x1.3 +- 1 from it).
    q0: f64,
    color: imij::LossyColor,
    stripe_block_rows: u32,
    effort: f32,
    threads: usize,
}

impl Spec {
    fn parse(v: &Value) -> Result<Self> {
        let f = |k: &str, d: f64| -> Result<f64> {
            match v.get(k) {
                None | Some(Value::Null) => Ok(d),
                Some(x) => x
                    .as_f64()
                    .ok_or_else(|| format!("{k} must be a number").into()),
            }
        };
        let metric = Metric::parse(v["metric"].as_str().ok_or("metric missing")?)?;
        let target = v["target"].as_f64().ok_or("target missing")?;
        let tol = f(
            "tol",
            match metric {
                Metric::Zensim | Metric::Ssim2 => 0.5,
                Metric::BaMax | Metric::BaP3 => 0.02 * target,
            },
        )?;
        let spec = Self {
            metric,
            target,
            tol,
            global_passes: f("global_passes", 8.0)? as usize,
            steer_passes: f("steer_passes", 6.0)? as usize,
            gain: f("gain", 0.5)?,
            step: f("step", 0.5)?,
            deadzone: f("deadzone", 0.0)?,
            lo: f("lo", -1.5)?,
            hi: f("hi", 1.5)?,
            levels: f("levels", 4.0)?,
            grain: f("grain", 1.0)? as usize,
            alloc: match v["alloc"].as_str().unwrap_or("equal") {
                "equal" => Alloc::Equal,
                "rate" => Alloc::Rate,
                "lambda" => Alloc::Lambda,
                _ => return Err("alloc must be equal, rate or lambda".into()),
            },
            strength: f("strength", 0.5)?,
            q0: f("q0", 50.0)?,
            color: match v["color"].as_str().unwrap_or("xyb") {
                "xyb" => imij::LossyColor::Xyb,
                "ycocg" => imij::LossyColor::YCoCg,
                _ => return Err("invalid color".into()),
            },
            stripe_block_rows: f("stripe_block_rows", 8.0)? as u32,
            effort: f("effort", 10.0)? as f32,
            threads: f("threads", 1.0)? as usize,
        };
        if !(spec.target.is_finite()
            && spec.tol > 0.0
            && spec.global_passes >= 1
            && spec.gain >= 0.0
            && spec.step > 0.0
            && spec.deadzone >= 0.0
            && spec.lo <= 0.0
            && spec.hi >= 0.0
            && spec.lo >= -6.0
            && spec.hi <= 1.99
            && spec.levels >= 1.0
            && spec.grain >= 1
            && (1.0..=100.0).contains(&spec.q0))
        {
            return Err("steering spec out of range".into());
        }
        Ok(spec)
    }

    fn cfg(&self, q: f64, map: Option<Arc<[u8]>>) -> imij::ImiEncoderConfig {
        let mut c = imij::ImiEncoderConfig::lossy(q as f32)
            .tune_lossy(|t| t.color = self.color)
            .tune_lossy(|t| t.entropy = imij::LossyEntropy::RunLevel)
            .with_effort(self.effort)
            .with_threads(self.threads);
        c = c.tune_lossy(|t| {
            t.stripe_block_rows = self.stripe_block_rows;
        });
        c = c.tune_lossy(|t| {
            t.lab.aq_map = map;
        });
        c
    }
}

/// Warm metric state for one source image.
pub(crate) struct Metrics {
    w: usize,
    h: usize,
    src: Vec<u8>,
    zensim: zensim::Zensim,
    zensim_ref: zensim::PrecomputedReference,
    ba: butteraugli::ButteraugliReference,
}

/// One pass's view of the steered metric: the score and a per-block error
/// (higher = worse) on the 8x8 block grid.
struct Measure {
    score: f64,
    blocks: Vec<f64>,
}

fn px3(b: &[u8]) -> &[[u8; 3]] {
    b.as_chunks::<3>().0
}

impl Metrics {
    pub(crate) fn new(src: Vec<u8>, w: usize, h: usize) -> Result<Self> {
        let zensim = zensim::Zensim::new(zensim::ZensimProfile::PreviewV0_2).with_parallel(false);
        let zensim_ref = zensim.precompute_reference(&zensim::RgbSlice::new(px3(&src), w, h))?;
        let ba = butteraugli::ButteraugliReference::new(
            &src,
            w,
            h,
            butteraugli::ButteraugliParams::default().with_compute_diffmap(true),
        )?;
        Ok(Self {
            w,
            h,
            src,
            zensim,
            zensim_ref,
            ba,
        })
    }

    /// Per-pixel map -> per-block value: mean or max over each 8x8 block.
    fn blocks(&self, map: &[f32], max: bool) -> Vec<f64> {
        let (bw, bh) = (self.w.div_ceil(8), self.h.div_ceil(8));
        let mut out = vec![0f64; bw * bh];
        for y in 0..self.h {
            for x in 0..self.w {
                let (b, v) = ((y / 8) * bw + x / 8, map[y * self.w + x] as f64);
                out[b] = if max { out[b].max(v) } else { out[b] + v };
            }
        }
        out
    }

    fn measure(&self, metric: Metric, dec: &[u8]) -> Result<Measure> {
        Ok(match metric {
            Metric::Zensim => {
                let r = self.zensim.compute_with_ref_and_diffmap(
                    &self.zensim_ref,
                    &zensim::RgbSlice::new(px3(dec), self.w, self.h),
                    zensim::DiffmapWeighting::Trained,
                )?;
                Measure {
                    score: r.score(),
                    blocks: self.blocks(r.diffmap(), false),
                }
            }
            Metric::Ssim2 => {
                let m = crate::ssim2map::ssim2_map(&self.src, dec, self.w, self.h);
                Measure {
                    score: m.score,
                    blocks: self.blocks(&m.map, false),
                }
            }
            Metric::BaMax | Metric::BaP3 => {
                let r = self.ba.compare(dec)?;
                let map = r.diffmap.as_ref().ok_or("butteraugli diffmap missing")?;
                let map = map.buf();
                if metric == Metric::BaMax {
                    Measure {
                        score: r.score,
                        blocks: self.blocks(map, true),
                    }
                } else {
                    // pnorm_3 = mean of the 3-, 6- and 12-norms; a pixel's
                    // share of norm p is n_p * d^p / sum(d^p).
                    let mut sums = [0f64; 3];
                    for &d in map {
                        let d = d as f64;
                        let d3 = d * d * d;
                        sums[0] += d3;
                        sums[1] += d3 * d3;
                        sums[2] += d3 * d3 * d3 * d3;
                    }
                    let n = map.len() as f64;
                    let norms = [
                        (sums[0] / n).powf(1.0 / 3.0),
                        (sums[1] / n).powf(1.0 / 6.0),
                        (sums[2] / n).powf(1.0 / 12.0),
                    ];
                    let k: Vec<f64> = (0..3)
                        .map(|i| {
                            if sums[i] > 0.0 {
                                norms[i] / (3.0 * sums[i])
                            } else {
                                0.0
                            }
                        })
                        .collect();
                    let share: Vec<f32> = map
                        .iter()
                        .map(|&d| {
                            let d3 = (d as f64).powi(3);
                            (k[0] * d3 + k[1] * d3 * d3 + k[2] * d3.powi(4)) as f32
                        })
                        .collect();
                    Measure {
                        score: r.pnorm_3,
                        blocks: self.blocks(&share, false),
                    }
                }
            }
        })
    }

    /// Every metric of a final file (CVVDP is scored separately).
    fn cross(&self, dec: &[u8]) -> Result<Value> {
        let z = self.zensim.compute_with_ref(
            &self.zensim_ref,
            &zensim::RgbSlice::new(px3(dec), self.w, self.h),
        )?;
        fn slice(px: &[u8], w: usize, h: usize) -> Result<fast_ssim2::PixelSlice<'_>> {
            let d = fast_ssim2::PixelDescriptor::RGB8_SRGB;
            Ok(fast_ssim2::PixelSlice::new(
                px,
                w as u32,
                h as u32,
                w * 3,
                d,
            )?)
        }
        let ssim2 = fast_ssim2::Ssimulacra2Reference::new(&slice(&self.src, self.w, self.h)?)?
            .compare(&slice(dec, self.w, self.h)?)?;
        let ba = self.ba.compare(dec)?;
        Ok(json!({
            "zensim": z.score(), "ssimulacra2": ssim2, "butteraugli_max": ba.score,
            "butteraugli_p3": ba.pnorm_3,
        }))
    }
}

/// One encode + decode + measurement.
struct Pass {
    phase: &'static str,
    q: f64,
    data: Vec<u8>,
    dec: Vec<u8>,
    score: f64,
    blocks: Vec<f64>,
    map: Option<Arc<[u8]>>,
    enc_s: f64,
    metric_s: f64,
}

impl Pass {
    fn margin(&self, spec: &Spec) -> f64 {
        if spec.metric.higher_better() {
            self.score - spec.target
        } else {
            spec.target - self.score
        }
    }
}

fn run_pass(
    spec: &Spec,
    m: &Metrics,
    phase: &'static str,
    q: f64,
    map: Option<Arc<[u8]>>,
) -> Result<Pass> {
    let info = imij::ImageInfo::new(
        m.w as u32,
        m.h as u32,
        imij::Channels::Rgb,
        imij::SampleType::U8,
    );
    let t = Instant::now();
    let data = spec.cfg(q, map.clone()).encode_samples(
        imij::ImageRef::new(&m.src, m.w * 3, &info),
        &imij::Metadata::default(),
    )?;
    let enc_s = t.elapsed().as_secs_f64();
    let (_, dec) = imij::decode(&data)?;
    let t = Instant::now();
    let meas = m.measure(spec.metric, &dec)?;
    Ok(Pass {
        phase,
        q,
        data,
        dec,
        score: meas.score,
        blocks: meas.blocks,
        map,
        enc_s,
        metric_s: t.elapsed().as_secs_f64(),
    })
}

/// Map state: log2 multiplier per cell (grain x grain blocks).
struct MapState {
    bw: usize,
    bh: usize,
    cw: usize,
    ch: usize,
    grain: usize,
    l: Vec<f64>,
}

impl MapState {
    fn new(w: usize, h: usize, grain: usize) -> Self {
        let (bw, bh) = (w.div_ceil(8), h.div_ceil(8));
        let (cw, ch) = (bw.div_ceil(grain), bh.div_ceil(grain));
        Self {
            bw,
            bh,
            cw,
            ch,
            grain,
            l: vec![0.0; cw * ch],
        }
    }

    /// Block errors -> cell errors (max for a max-norm target, else sum).
    fn cells(&self, blocks: &[f64], max: bool) -> Vec<f64> {
        let mut out = vec![0f64; self.cw * self.ch];
        for by in 0..self.bh {
            for bx in 0..self.bw {
                let c = (by / self.grain) * self.cw + bx / self.grain;
                let v = blocks[by * self.bw + bx];
                out[c] = if max { out[c].max(v) } else { out[c] + v };
            }
        }
        out
    }

    /// Redistribution step: cells above the reference error get finer,
    /// cells below coarser, by -gain * log2(e / ref), at most `step`
    /// octaves per pass; the mean log multiplier is then removed (the
    /// quality correction sets the level).
    fn update(&mut self, spec: &Spec, blocks: &[f64], weights: Option<&[f64]>) {
        let max = spec.metric == Metric::BaMax;
        let e = self.cells(blocks, max);
        let reference = if max {
            spec.target
        } else {
            e.iter().sum::<f64>() / e.len() as f64
        };
        if reference <= 0.0 {
            return;
        }
        // Errors are floored at 1/16 of the reference: an error-free cell
        // moves by the step, not to infinity.
        for (i, (l, &ei)) in self.l.iter_mut().zip(&e).enumerate() {
            let r = reference * weights.map_or(1.0, |w| w[i]);
            let u = -spec.gain * (ei.max(r / 16.0) / r).log2();
            let u = if u.abs() < spec.deadzone { 0.0 } else { u };
            *l = (*l + u.clamp(-spec.step, spec.step)).clamp(spec.lo, spec.hi);
        }
        let mean = self.l.iter().sum::<f64>() / self.l.len() as f64;
        for l in &mut self.l {
            *l = (*l - mean).clamp(spec.lo, spec.hi);
        }
    }

    /// One map from marginal returns between uniform passes `lo` (lower
    /// quality) and `hi`: per cell, lambda = error drop / proxy-bit rise;
    /// l = -strength * log2(lambda / median), clamped, mean removed.
    fn lambda_map(&mut self, spec: &Spec, lo: &Pass, hi: &Pass) -> Result<()> {
        let (rlo, rhi) = (block_bits(&lo.data)?, block_bits(&hi.data)?);
        let dd = self.cells(
            &lo.blocks
                .iter()
                .zip(&hi.blocks)
                .map(|(a, b)| a - b)
                .collect::<Vec<_>>(),
            false,
        );
        let dr = self.cells(
            &rhi.iter().zip(&rlo).map(|(a, b)| a - b).collect::<Vec<_>>(),
            false,
        );
        let blocks_per_cell = (self.grain * self.grain) as f64;
        let lam: Vec<f64> = dd
            .iter()
            .zip(&dr)
            .map(|(&d, &r)| d.max(0.0) / r.max(0.5 * blocks_per_cell))
            .collect();
        let mut sorted: Vec<f64> = lam.iter().copied().filter(|&v| v > 0.0).collect();
        sorted.sort_by(f64::total_cmp);
        let Some(&med) = sorted.get(sorted.len() / 2) else {
            return Ok(());
        };
        for (l, &v) in self.l.iter_mut().zip(&lam) {
            *l = (-spec.strength * (v.max(med / 64.0) / med).log2()).clamp(spec.lo, spec.hi);
        }
        let mean = self.l.iter().sum::<f64>() / self.l.len() as f64;
        for l in &mut self.l {
            *l = (*l - mean).clamp(spec.lo, spec.hi);
        }
        Ok(())
    }

    /// AQ codes per block (None when every code is 64).
    fn codes(&self, spec: &Spec) -> Option<Arc<[u8]>> {
        let mut out = vec![64u8; self.bw * self.bh];
        for by in 0..self.bh {
            for bx in 0..self.bw {
                let l = self.l[(by / self.grain) * self.cw + bx / self.grain];
                let lq = (l * spec.levels).round() / spec.levels;
                out[by * self.bw + bx] = (64.0 * lq.exp2()).round().clamp(1.0, 255.0) as u8;
            }
        }
        out.iter().any(|&a| a != 64).then(|| out.into())
    }
}

/// Proxy bits per block of a uniform-map file: 2 + log2|v| per nonzero AC
/// coefficient (token, sign, magnitude), all planes.
fn block_bits(data: &[u8]) -> Result<Vec<f64>> {
    Ok(imij::chroma_lab::unpack(data)?
        .as_chunks::<192>()
        .0
        .iter()
        .map(|b| {
            (0..3)
                .flat_map(|c| &b[c * 64 + 1..c * 64 + 64])
                .filter(|&&v| v != 0)
                .map(|&v| 2.0 + (v.unsigned_abs() as f64).log2())
                .sum()
        })
        .collect())
}

/// Per-cell nonzero AC coefficients (all planes, +1 per block) of a
/// uniform-map file, relative to their mean.
fn rate_weights(state: &MapState, data: &[u8]) -> Result<Vec<f64>> {
    let q = imij::chroma_lab::unpack(data)?;
    let blocks: Vec<f64> = q
        .as_chunks::<192>()
        .0
        .iter()
        .map(|b| {
            1.0 + (0..3)
                .map(|c| {
                    b[c * 64 + 1..c * 64 + 64]
                        .iter()
                        .filter(|&&v| v != 0)
                        .count()
                })
                .sum::<usize>() as f64
        })
        .collect();
    let cells = state.cells(&blocks, false);
    let mean = cells.iter().sum::<f64>() / cells.len() as f64;
    Ok(cells.iter().map(|c| c / mean).collect())
}

/// Bytes the AQ channel adds to the nested DC image: the DC image
/// re-encoded with and without its 4th channel (lossless, effort 10, the
/// file's DC stripe height).
pub(crate) fn aq_channel_bytes(data: &[u8]) -> Result<Value> {
    let (info, px, hs) = imij::lab::lossy_dc_image(data)?;
    if info.channels != imij::Channels::Rgba {
        return Ok(json!({"dc4": Value::Null, "dc3": Value::Null, "aq": 0}));
    }
    let enc = |info: &imij::ImageInfo, px: &[u8]| -> Result<usize> {
        let cfg = {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.with_effort(10.0);
            cfg = cfg.tune_lossless(|t| {
                t.stripe_rows = Some(hs);
            });
            cfg = cfg.with_threads(1);
            cfg = cfg.with_column_groups(Some(0));
            cfg
        };
        Ok(cfg
            .encode_samples(
                imij::ImageRef::new(px, info.row_bytes(), info),
                &imij::Metadata::default(),
            )?
            .len())
    };
    let bps = info.sample.bytes();
    let rgb: Vec<u8> = px
        .chunks_exact(4 * bps)
        .flat_map(|p| p[..3 * bps].to_vec())
        .collect();
    let info3 = imij::ImageInfo::new(info.width, info.height, imij::Channels::Rgb, info.sample);
    let (dc4, dc3) = (enc(&info, &px)?, enc(&info3, &rgb)?);
    Ok(json!({"dc4": dc4, "dc3": dc3, "aq": dc4 as i64 - dc3 as i64}))
}

fn pass_row(p: &Pass, spec: &Spec) -> Value {
    let (nz, levels) = match &p.map {
        None => (0, 0),
        Some(m) => {
            let mut seen = [false; 256];
            m.iter().for_each(|&a| seen[a as usize] = true);
            (
                m.iter().filter(|&&a| a != 64).count(),
                seen.iter().filter(|&&s| s).count(),
            )
        }
    };
    let aq = aq_channel_bytes(&p.data).map_or(Value::Null, |v| v["aq"].clone());
    json!({
        "phase": p.phase, "q": p.q, "bytes": p.data.len(), "aq_bytes": aq, "score": p.score,
        "margin": p.margin(spec), "feasible": p.margin(spec) >= 0.0,
        "map_blocks_not_64": nz, "map_codes": levels,
        "encode_s": p.enc_s, "metric_s": p.metric_s,
    })
}

/// The loop. Returns (all passes, index of the best global-search pass,
/// index of the best pass overall).
fn steer(spec: &Spec, m: &Metrics) -> Result<(Vec<Pass>, usize, usize)> {
    let mut passes: Vec<Pass> = Vec::new();
    // Best = feasible with the fewest bytes, else the largest margin.
    let better = |a: &Pass, b: &Pass| -> bool {
        match (a.margin(spec) >= 0.0, b.margin(spec) >= 0.0) {
            (true, false) => true,
            (false, true) => false,
            (true, true) => a.data.len() < b.data.len(),
            (false, false) => a.margin(spec) > b.margin(spec),
        }
    };
    let best_of = |passes: &[Pass]| -> usize {
        (1..passes.len()).fold(0, |b, i| if better(&passes[i], &passes[b]) { i } else { b })
    };
    // Global search: margin rises with q. Bracket [lo infeasible, hi
    // feasible]; secant inside the bracket (Illinois), else expand.
    // (quality, margin) of the closest infeasible / feasible pass so far.
    type Side = Option<(f64, f64)>;
    let (mut lo, mut hi): (Side, Side) = (None, None);
    let mut q = spec.q0;
    for _ in 0..spec.global_passes {
        let p = run_pass(spec, m, "global", q, None)?;
        let mg = p.margin(spec);
        passes.push(p);
        if (0.0..=spec.tol).contains(&mg) {
            break;
        }
        if mg < 0.0 {
            lo = Some((q, mg));
        } else {
            hi = Some((q, mg));
        }
        // Aim at the middle of the band.
        let aim = spec.tol / 2.0;
        q = match (lo, hi) {
            (Some((ql, ml)), Some((qh, mh))) => {
                let t = ((aim - ml) / (mh - ml)).clamp(0.1, 0.9);
                ql + t * (qh - ql)
            }
            (Some((ql, _)), None) => (ql * 1.3 + 1.0).min(100.0),
            (None, Some((qh, _))) => ((qh - 1.0) / 1.3).max(1.0),
            (None, None) => unreachable!(),
        };
        if passes.iter().any(|p| (p.q - q).abs() < 0.05) {
            break;
        }
    }
    let gbest = best_of(&passes);
    if spec.steer_passes == 0 || passes[gbest].margin(spec) < 0.0 {
        return Ok((passes, gbest, gbest));
    }
    // Slope of the margin in q around the target, from the bracket (or the
    // two closest global passes).
    let slope = {
        let mut g: Vec<&Pass> = passes.iter().collect();
        g.sort_by(|a, b| a.margin(spec).abs().total_cmp(&b.margin(spec).abs()));
        let (a, b) = (g[0], g.get(1).copied().unwrap_or(g[0]));
        let s = (a.margin(spec) - b.margin(spec)) / (a.q - b.q);
        if s.is_finite() && s > 0.0 {
            s
        } else {
            spec.tol / 2.0
        }
    };
    let mut state = MapState::new(m.w, m.h, spec.grain);
    let weights = if spec.alloc == Alloc::Rate && spec.metric != Metric::BaMax {
        Some(rate_weights(&state, &passes[gbest].data)?)
    } else {
        None
    };
    let lambda = spec.alloc == Alloc::Lambda && spec.metric != Metric::BaMax;
    if lambda {
        // Partner uniform pass: the global pass nearest q* whose quality
        // differs by 10-50%, else a probe at q* * 1.25 (or / 1.25 near 100).
        let qs = passes[gbest].q;
        let partner = (0..passes.len())
            .filter(|&i| {
                let r = passes[i].q / qs;
                (1.1..=1.5).contains(&r) || (1.0 / 1.5..=1.0 / 1.1).contains(&r)
            })
            .min_by(|&a, &b| {
                (passes[a].q - qs)
                    .abs()
                    .total_cmp(&(passes[b].q - qs).abs())
            });
        let partner = match partner {
            Some(i) => i,
            None => {
                let q = if qs * 1.25 <= 100.0 {
                    qs * 1.25
                } else {
                    qs / 1.25
                };
                passes.push(run_pass(spec, m, "probe", q, None)?);
                passes.len() - 1
            }
        };
        let (lo, hi) = if passes[partner].q < qs {
            (partner, gbest)
        } else {
            (gbest, partner)
        };
        state.lambda_map(spec, &passes[lo], &passes[hi])?;
    }
    let mut cur = gbest;
    for _ in 0..spec.steer_passes {
        if !lambda {
            state.update(spec, &passes[cur].blocks, weights.as_deref());
        }
        let map = state.codes(spec);
        let pc = &passes[cur];
        let qn = (pc.q + (spec.tol / 2.0 - pc.margin(spec)) / slope).clamp(1.0, 100.0);
        let p = run_pass(spec, m, "steer", qn, map)?;
        passes.push(p);
        cur = passes.len() - 1;
    }
    let best = best_of(&passes);
    Ok((passes, gbest, best))
}

fn final_row(
    p: &Pass,
    m: &Metrics,
    scorer: &mut cvvdp::Cvvdp,
    out: &Path,
    name: &str,
) -> Result<Value> {
    let path = out.join(format!("{name}.imij"));
    fs::write(&path, &p.data)?;
    let mut scores = m.cross(&p.dec)?;
    scores["cvvdp_fhd_jod"] = json!(scorer.score_with_warm_ref(&p.dec)?);
    Ok(json!({
        "q": p.q, "bytes": p.data.len(), "encoded": path, "encoded_sha256": hash(&p.data),
        "decoded_sha256": hash(&p.dec), "scores": scores,
        "aq_map": p.map.is_some(), "aq_channel": aq_channel_bytes(&p.data)?,
    }))
}

/// Raw RGB8, or an 8-bit PNG (RGB; gray expanded; RGBA only when opaque).
fn read_rgb8(path: &Path) -> Result<Vec<u8>> {
    let data = fs::read(path)?;
    if path.extension().is_none_or(|e| e != "png") {
        return Ok(data);
    }
    let img = zenpng::decode(
        &data,
        &zenpng::PngDecodeConfig::default(),
        &enough::Unstoppable,
    )?;
    if img.info.bit_depth != 8 {
        return Err("PNG source must be 8-bit".into());
    }
    let px = img.pixels.copy_to_contiguous_bytes();
    let n = img.info.width as usize * img.info.height as usize;
    Ok(match px.len() / n {
        3 => px,
        1 => px.iter().flat_map(|&v| [v, v, v]).collect(),
        4 if px.as_chunks::<4>().0.iter().all(|p| p[3] == 255) => px
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect(),
        _ => return Err("PNG source must be RGB8, gray8 or opaque RGBA8".into()),
    })
}

fn job(j: &Value) -> Result<()> {
    let source = Path::new(j["source"].as_str().ok_or("source missing")?);
    let w = j["width"].as_u64().ok_or("width missing")? as usize;
    let h = j["height"].as_u64().ok_or("height missing")? as usize;
    let out = Path::new(j["output"].as_str().ok_or("output missing")?);
    if out.join("result.json").exists() {
        return Ok(());
    }
    let src = read_rgb8(source)?;
    crate::rd::check_rgb(&src, w as u32, h as u32)?;
    fs::create_dir_all(out)?;
    let t0 = Instant::now();
    let m = Metrics::new(src, w, h)?;
    let mut cv = crate::quality::perceptual::cvvdp_fhd(w as u32, h as u32)?;
    cv.warm_reference(&m.src)?;
    let spec_v = &j["spec"];
    let mut row = json!({
        "source": source, "source_sha256": hash(&m.src), "width": w, "height": h,
        "spec": spec_v, "image": j["image"], "image_class": j["image_class"],
    });
    if spec_v["metric"] == "plain" {
        // Plain encoder: a quality grid, scored like the steered files.
        let spec = Spec::parse(
            &json!({"metric": "zensim", "target": 0, "color": spec_v["color"],
            "stripe_block_rows": spec_v["stripe_block_rows"], "effort": spec_v["effort"]}),
        )?;
        let mut rows = Vec::new();
        for q in spec_v["qualities"].as_array().ok_or("qualities missing")? {
            let q = q.as_f64().ok_or("quality must be a number")?;
            let p = run_pass(&spec, &m, "plain", q, None)?;
            rows.push(final_row(&p, &m, &mut cv, out, &format!("q{q}"))?);
        }
        row["plain"] = json!(rows);
    } else {
        let spec = Spec::parse(spec_v)?;
        let (passes, gbest, best) = steer(&spec, &m)?;
        row["passes"] = json!(
            passes
                .iter()
                .map(|p| pass_row(p, &spec))
                .collect::<Vec<_>>()
        );
        row["global_best"] = json!(gbest);
        row["best"] = json!(best);
        row["global"] = final_row(&passes[gbest], &m, &mut cv, out, "global")?;
        row["steered"] = if best == gbest {
            row["global"].clone()
        } else {
            final_row(&passes[best], &m, &mut cv, out, "steered")?
        };
        row["met"] = json!(passes[best].margin(&spec) >= 0.0);
    }
    row["wall_s"] = json!(t0.elapsed().as_secs_f64());
    save(&out.join("result.json"), &row)
}

/// `aq-cost FILE...`: what the AQ channel of each lossy file costs as
/// coded, and under alternative codings (bytes of losslessly re-encoded
/// nested images, effort 10, the file's DC stripe height):
/// - `as_coded`: the DC image with vs without its 4th channel;
/// - `index`: the 4th channel holding each code's rank among the file's
///   distinct codes (a format with an AQ code table in the header);
/// - `plane_codes` / `plane_index`: the codes or ranks as their own gray
///   image (a format with a separate AQ plane);
/// - `plane_index_gN`: ranks on an N x N block grid (N = 2, 4), from the
///   top-left block of each cell; only equal to the map when it is
///   constant on those cells (`gN_exact`).
pub(crate) fn aq_cost(a: &[String]) -> Result<()> {
    for f in &a[2..] {
        let data = fs::read(f)?;
        match aq_cost_row(&data)? {
            Some(mut row) => {
                row["file"] = json!(f);
                println!("{row}");
            }
            None => println!("{}", json!({"file": f, "bytes": data.len(), "aq": false})),
        }
    }
    Ok(())
}

/// [`aq_cost`]'s measurements of one lossy file (None without an AQ
/// channel); `codes` lists the distinct codes (a code table's contents).
pub(crate) fn aq_cost_row(data: &[u8]) -> Result<Option<Value>> {
    {
        let (info, px, hs) = imij::lab::lossy_dc_image(data)?;
        if info.channels != imij::Channels::Rgba {
            return Ok(None);
        }
        let enc = |info: &imij::ImageInfo, px: &[u8]| -> Result<usize> {
            let cfg = {
                let mut cfg = imij::ImiEncoderConfig::default();
                cfg = cfg.with_effort(10.0);
                cfg = cfg.tune_lossless(|t| {
                    t.stripe_rows = Some(hs);
                });
                cfg = cfg.with_threads(1);
                cfg = cfg.with_column_groups(Some(0));
                cfg
            };
            Ok(cfg
                .encode_samples(
                    imij::ImageRef::new(px, info.row_bytes(), info),
                    &imij::Metadata::default(),
                )?
                .len())
        };
        let (bw, bh) = (info.width as usize, info.height as usize);
        let bps = info.sample.bytes();
        let codes: Vec<u8> = px
            .chunks_exact(4 * bps)
            .map(|p| {
                if bps == 2 {
                    u16::from_ne_bytes([p[6], p[7]]) as u8
                } else {
                    p[12]
                }
            })
            .collect();
        let mut distinct: Vec<u8> = codes.clone();
        distinct.sort_unstable();
        distinct.dedup();
        let rank: Vec<u8> = codes
            .iter()
            .map(|c| distinct.binary_search(c).unwrap() as u8)
            .collect();
        let with4 = |vals: &[u8]| -> Vec<u8> {
            let mut out = px.clone();
            for (p, &v) in out.chunks_exact_mut(4 * bps).zip(vals) {
                if bps == 2 {
                    p[6..8].copy_from_slice(&(v as u16).to_ne_bytes());
                } else {
                    p[12..16].copy_from_slice(&(v as i32).to_ne_bytes());
                }
            }
            out
        };
        let rgb: Vec<u8> = px
            .chunks_exact(4 * bps)
            .flat_map(|p| p[..3 * bps].to_vec())
            .collect();
        let info3 = imij::ImageInfo::new(info.width, info.height, imij::Channels::Rgb, info.sample);
        let gray = |w: usize, h: usize| {
            imij::ImageInfo::new(
                w as u32,
                h as u32,
                imij::Channels::Gray,
                imij::SampleType::U8,
            )
        };
        let dc3 = enc(&info3, &rgb)?;
        let mut row = json!({
            "bytes": data.len(), "blocks": bw * bh, "codes": distinct.len(),
            "code_values": distinct,
            "not_64": codes.iter().filter(|&&c| c != 64).count(), "dc3": dc3,
            "as_coded": enc(&info, &px)? as i64 - dc3 as i64,
            "index": enc(&info, &with4(&rank))? as i64 - dc3 as i64,
            "plane_codes": enc(&gray(bw, bh), &codes)?,
            "plane_index": enc(&gray(bw, bh), &rank)?,
        });
        for g in [2usize, 4] {
            let (cw, ch) = (bw.div_ceil(g), bh.div_ceil(g));
            let coarse: Vec<u8> = (0..cw * ch)
                .map(|i| rank[(i / cw) * g * bw + (i % cw) * g])
                .collect();
            let exact = (0..bw * bh).all(|i| rank[i] == coarse[(i / bw / g) * cw + (i % bw) / g]);
            row[format!("plane_index_g{g}")] = json!(enc(&gray(cw, ch), &coarse)?);
            row[format!("g{g}_exact")] = json!(exact);
        }
        Ok(Some(row))
    }
}

pub(crate) fn command(a: &[String]) -> Result<()> {
    if a.len() != 3 {
        return Err("steer-batch JOBS_JSON".into());
    }
    let jobs: Vec<Value> = serde_json::from_slice(&fs::read(&a[2])?)?;
    let mut failures = 0;
    for j in &jobs {
        match job(j) {
            Ok(()) => eprintln!("complete {}", j["output"]),
            Err(e) => {
                eprintln!("failed {}: {e}", j["output"]);
                failures += 1;
            }
        }
    }
    if failures == 0 {
        Ok(())
    } else {
        Err(format!("{failures} steering jobs failed").into())
    }
}
