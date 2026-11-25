//! Encoder.

use crate::coding_layout::*;
use alloc::vec;
use alloc::vec::Vec;
use how_far::{ProgressExt, Pulse};

use crate::mapping::{self, ChannelMap, MapMode};
use crate::predict::{self, NUM_PREDICTORS};
use crate::{Error, ImageInfo, SampleType};

#[path = "encode_precision.rs"]
mod precision;
#[path = "encode_tables.rs"]
pub(crate) mod tables;

/// Front-end knobs (prediction search, stripes, lanes) plus lab-only
/// entropy settings.
#[derive(Clone, Debug)]
#[cfg_attr(not(feature = "lab"), allow(dead_code))]
pub struct AnalysisConfig {
    /// Rows per stripe; `None` picks from the width.
    pub stripe_rows: Option<u32>,
    /// Shortest zero run coded as a run token.
    pub run_min: u32,
    /// Maximum contexts per slot (1..=8).
    pub max_ctx: u32,
    /// Add the first color slot's residual to chroma contexts.
    pub ctx_cross: bool,
    /// Bitmask of allowed decorrelation modes.
    pub decor_mask: u8,
    /// Bitmask of allowed predictors.
    pub predictor_mask: u8,
    /// Lab: full-resolution guide planes (coded domain, per container
    /// channel) enabling guided predictors 7..=10. Output is not decodable.
    #[cfg(feature = "lab")]
    pub guide: Option<alloc::sync::Arc<Vec<Vec<u16>>>>,
    /// Direct-token bits for coded depths above 8.
    pub k_high: u32,
    /// Maximum codeword length (<= 12).
    pub max_code_len: u32,
    /// Column segments (lanes) per stripe.
    pub lanes: u32,
    /// Direct-token bits for coded depths <= 8.
    pub k_low: u32,
    /// Bits below the MSB carried in a value token.
    pub msb_bits: u32,
    /// Run token direct bits / msb bits.
    pub run_k: u32,
    pub run_msb_bits: u32,
    /// Encoder threads (stripes are analyzed in parallel).
    pub threads: usize,
    /// Channel maps and coded planes only: no stripe analysis (empty
    /// residuals; the caller analyses its own units, e.g. column groups).
    #[doc(hidden)]
    pub maps_only: bool,
    /// Near-lossless bound per memory channel in sample units (0 = exact).
    pub max_error: [u32; 4],
    /// Allow the Scientific profile's float map ([`MapMode::FloatShift`]).
    pub float_shift: bool,
    /// Allow value-table maps ([`MapMode::Table`]) where estimated smaller:
    /// a file's own payload only, not a lossy file's nested images.
    pub value_tables: bool,
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            stripe_rows: None,
            run_min: 4,
            max_ctx: 4,
            ctx_cross: true,
            decor_mask: 0b1111,
            predictor_mask: 0x7F,
            #[cfg(feature = "lab")]
            guide: None,
            k_high: 6,
            max_code_len: 12,
            lanes: 1,
            k_low: 8,
            msb_bits: 2,
            run_k: 4,
            run_msb_bits: 1,
            threads: 1,
            maps_only: false,
            max_error: [0; 4],
            float_shift: false,
            value_tables: false,
        }
    }
}

/// Default lane count: 16 lanes of at least 32 columns where possible.
pub fn default_lanes(w: u32) -> u32 {
    (w / 32).clamp(1, 16)
}

pub(crate) fn default_stripe_rows(width: u32) -> u32 {
    (65536 / width.max(1)).clamp(8, 256)
}

/// Runs the pixel front-end (mapping, prediction, decorrelation).
#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
pub fn analyze(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    cfg: &AnalysisConfig,
) -> Result<Analysis, Error> {
    analyze_with_progress(pixels, stride, info, cfg, &how_far::NoPulse)
}

pub(crate) fn analyze_with_progress(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    cfg: &AnalysisConfig,
    pulse: &dyn Pulse,
) -> Result<Analysis, Error> {
    let pulse = pulse.live();
    use how_far::Stop;
    pulse.check().map_err(|_| Error::Cancelled)?;
    let w = info.width as usize;
    let h = info.height as usize;
    if w == 0 || h == 0 {
        return Err(Error::InvalidInput("empty image"));
    }
    let extent = info
        .checked_stride_len(stride)
        .ok_or(Error::InvalidInput("invalid image layout"))?;
    if pixels.len() < extent {
        return Err(Error::InvalidInput("pixel buffer too small"));
    }
    let row_bytes = info.row_bytes();
    let plane_len = w.checked_mul(h).ok_or(Error::TooLarge)?;
    let nch = info.channels.count();
    let depth = info.sample.word_depth();

    // 1. Coded channels as order-preserving unsigned ints: the container
    // samples, or for 64-bit samples their hi words (ordered like a 32-bit
    // sample of the same kind), then their lo words (SPEC 2.1).
    let ncc = nch * info.sample.words();
    let mut planes = reserved(ncc)?;
    for _ in 0..ncc {
        planes.push(reserved::<u32>(plane_len)?);
    }
    let bps = info.sample.bytes();
    for y in 0..h {
        pulse.check().map_err(|_| Error::Cancelled)?;
        let row = &pixels[y * stride..y * stride + row_bytes];
        let block_bytes = (mapping::SCAN_WORDS / ncc).max(1) * nch * bps;
        for block in row.chunks(block_bytes) {
            pulse.check().map_err(|_| Error::Cancelled)?;
            for px in block.chunks_exact(nch * bps) {
                for c in 0..nch {
                    let b = &px[c * bps..(c + 1) * bps];
                    let v = match bps {
                        1 => b[0] as u32,
                        2 => u16::from_ne_bytes([b[0], b[1]]) as u32,
                        4 => u32::from_ne_bytes([b[0], b[1], b[2], b[3]]),
                        _ => {
                            let v = u64::from_ne_bytes(b.try_into().unwrap());
                            planes[nch + c].push(v as u32);
                            (v >> 32) as u32
                        }
                    };
                    planes[c].push(info.sample.order(v));
                }
            }
        }
        pulse.step(1).map_err(|_| Error::Cancelled)?;
    }

    // 2. Channel mappings.
    let is_f32 = info.sample == SampleType::F32;
    let mut maps: Vec<ChannelMap> = (0..ncc)
        .map(|c| -> Result<_, Error> {
            pulse.check().map_err(|_| Error::Cancelled)?;
            // Alpha, and every lo word of a 64-bit sample, may be Const.
            let is_alpha = info.channels.has_alpha() && c % nch == nch - 1;
            let allow_const = is_alpha || c >= nch;
            if depth > 16 {
                return mapping::detect32_checked(&planes[c], is_f32, allow_const, || {
                    pulse.check().map_err(|_| Error::Cancelled)
                });
            }
            let mut p = mapping::presence(depth);
            for chunk in planes[c].chunks(mapping::SCAN_WORDS) {
                pulse.check().map_err(|_| Error::Cancelled)?;
                for &v in chunk {
                    p[v as usize] = true;
                }
            }
            Ok(mapping::detect(&p, depth, is_alpha))
        })
        .collect::<Result<_, _>>()?;
    pulse.check().map_err(|_| Error::Cancelled)?;
    precision::choose_maps(info, cfg, &planes, &mut maps);
    // Scientific float map: floats whose low raw bits are zero in every
    // sample (BitRound) code their real precision whatever their signs.
    if cfg.float_shift && matches!(info.sample, SampleType::F16 | SampleType::F32) {
        for (c, m) in maps.iter_mut().enumerate() {
            if m.mode == MapMode::Const {
                continue;
            }
            let mut raw_or = 0u32;
            for chunk in planes[c].chunks(mapping::SCAN_WORDS) {
                pulse.check().map_err(|_| Error::Cancelled)?;
                raw_or = chunk
                    .iter()
                    .fold(raw_or, |a, &v| a | mapping::float_order(v, depth));
            }
            if let Some(f) = mapping::float_shift_map(raw_or, depth, m) {
                *m = f;
            }
        }
    }
    pulse.check().map_err(|_| Error::Cancelled)?;
    let near = crate::near::fit_maps(info, &planes, &mut maps, &cfg.max_error, || {
        pulse.check().map_err(|_| Error::Cancelled)
    })?;
    #[cfg(feature = "lab")]
    if cfg.guide.is_some() && near.iter().any(|&d| d != 0) {
        return Err(Error::InvalidInput(
            "guide planes with near-lossless coding",
        ));
    }
    // Value tables: channels using few of their values code each sample's
    // index among them (SPEC 2.1 map mode 9).
    #[cfg(feature = "lab")]
    let value_tables = cfg.value_tables && cfg.guide.is_none();
    #[cfg(not(feature = "lab"))]
    let value_tables = cfg.value_tables;
    let tables = if value_tables {
        let check = || pulse.check().map_err(|_| Error::Cancelled);
        tables::choose(info, cfg, &planes, &mut maps, &near, &check)?
    } else {
        vec![Vec::new(); maps.len()]
    };
    for (c, m) in maps.iter().enumerate() {
        pulse.check().map_err(|_| Error::Cancelled)?;
        if m.mode == MapMode::Table {
            tables::reduce_plane(&mut planes[c], &tables[c], depth, || {
                pulse.check().map_err(|_| Error::Cancelled)
            })?;
        } else if m.mode != MapMode::Const && !m.is_identity(depth) {
            for chunk in planes[c].chunks_mut(mapping::SCAN_WORDS) {
                pulse.check().map_err(|_| Error::Cancelled)?;
                for v in chunk {
                    *v = m.reduce(*v, depth);
                }
            }
        }
    }
    let layout = Layout::new(info.channels, maps)
        .with_near(&near)
        .with_tables(tables);
    let ns = layout.slot_channel.len();
    let slot_planes: Vec<&[u32]> = layout
        .slot_channel
        .iter()
        .map(|&c| planes[c].as_slice())
        .collect();

    // 3. Stripes: predictor + decorrelation choice, residual planes.
    let hs = cfg
        .stripe_rows
        .unwrap_or_else(|| default_stripe_rows(info.width))
        .max(1) as usize;
    let sw = seg_width(w, cfg.lanes);
    if cfg.maps_only {
        return Ok(Analysis {
            layout,
            planes,
            res: Vec::new(),
            w,
            h,
            hs,
            sw,
            stripe_decor: Vec::new(),
            stripe_adapt: Vec::new(),
            row_pred: Vec::new(),
        });
    }
    let nstripes = h.div_ceil(hs);
    let mut res = reserved(ns)?;
    for _ in 0..ns {
        res.push(filled(plane_len, 0u32)?);
    }
    let mut stripe_decor = filled(nstripes, 0u8)?;
    let mut stripe_adapt: Vec<Option<crate::adapt_color::AdaptWeights>> = filled(nstripes, None)?;
    let mut row_pred = filled(h, 0u8)?;
    {
        // Disjoint per-stripe views of every residual plane.
        let mut per = reserved(nstripes)?;
        for _ in 0..nstripes {
            per.push(reserved(ns)?);
        }
        for plane in res.iter_mut() {
            // A caller may request more rows than the image contains. Clamp
            // before multiplying so a valid short image cannot overflow the
            // host index type on a 32-bit target.
            for (si, chunk) in plane.chunks_mut(hs.min(h) * w).enumerate() {
                per[si].push(chunk);
            }
        }
        let lay = &layout;
        let sp = &slot_planes;
        let out = crate::par::par_map(per, cfg.threads, |si, mut views| -> Result<_, Error> {
            pulse.check().map_err(|_| Error::Cancelled)?;
            let y0 = si * hs;
            let y1 = (y0 + hs).min(h);
            let result = if lay.is_near() {
                let (d, pr) =
                    crate::near::analyze_stripe_near(lay, sp, w, sw, y0, y1, cfg, &mut views);
                (d, None, pr)
            } else {
                analyze_stripe_with_adapt(lay, sp, w, sw, y0, y1, cfg, &mut views, None)?
            };
            pulse.check().map_err(|_| Error::Cancelled)?;
            Ok(result)
        });
        for (si, result) in out.into_iter().enumerate() {
            let (decor, adapt, preds) = result?;
            let y0 = si * hs;
            stripe_decor[si] = decor;
            stripe_adapt[si] = adapt;
            row_pred[y0..y0 + preds.len()].copy_from_slice(&preds);
        }
    }
    pulse.check().map_err(|_| Error::Cancelled)?;
    Ok(Analysis {
        layout,
        planes,
        res,
        w,
        h,
        hs,
        sw,
        stripe_decor,
        stripe_adapt,
        row_pred,
    })
}

// Full-raster analysis storage must report allocation failure through the
// encoder's Result. Reserving first also prevents resize/push from growing
// these buffers infallibly. This is not aggregate working-memory admission:
// predictor scratch, scheduler state and subsequent coding stages are separate.
fn reserved<T>(len: usize) -> Result<Vec<T>, Error> {
    let mut out = Vec::new();
    out.try_reserve_exact(len).map_err(|_| Error::TooLarge)?;
    Ok(out)
}

fn filled<T: Clone>(len: usize, value: T) -> Result<Vec<T>, Error> {
    let mut out = reserved(len)?;
    out.resize(len, value);
    Ok(out)
}

/// Result of the pixel front-end: mappings, predictor/decorrelation choices
/// and zigzag residual planes (one per slot, `w * h` each).
#[cfg_attr(not(feature = "lab"), allow(dead_code))]
pub struct Analysis {
    pub(crate) layout: Layout,
    pub(crate) planes: Vec<Vec<u32>>,
    /// Zigzag residuals per slot (`w * h` each).
    pub res: Vec<Vec<u32>>,
    pub w: usize,
    pub h: usize,
    pub hs: usize,
    /// Lane (column segment) width.
    pub sw: usize,
    pub stripe_decor: Vec<u8>,
    pub(crate) stripe_adapt: Vec<Option<crate::adapt_color::AdaptWeights>>,
    pub row_pred: Vec<u8>,
}

#[cfg_attr(not(feature = "lab"), allow(dead_code))]
impl Analysis {
    pub fn slot_bits(&self) -> &[u32] {
        &self.layout.slot_bits
    }
    pub fn maps(&self) -> &[ChannelMap] {
        &self.layout.maps
    }
    /// Coded (mapped) container-channel planes.
    pub fn coded_planes(&self) -> &[Vec<u32>] {
        &self.planes
    }
}

fn group_cost(f: &[u64], _max_len: u32) -> f64 {
    let used = f.iter().filter(|&&x| x > 0).count();
    if used <= 1 {
        return 16.0;
    }
    let total: u64 = f.iter().sum();
    let lt = crate::mathx::log2(total as f64);
    let data: f64 = f
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| c as f64 * (lt - crate::mathx::log2(c as f64)))
        .sum();
    // ~7 bits per transmitted weight plus table header.
    data + 7.0 * used as f64 + 11.0
}

/// Groups the fine context classes into at most `max_ctx` contiguous
/// contexts. Returns the class->context map and per-context histograms.
pub(crate) fn choose_ctx_map(
    hist: &[Vec<u64>],
    max_ctx: usize,
    max_len: u32,
) -> ([u8; CTX_CLASSES], Vec<Vec<u64>>) {
    let asz = hist[0].len();
    // Start with non-empty classes as groups; empty classes join the previous group.
    let mut groups: Vec<(usize, usize, Vec<u64>)> = Vec::new(); // (first class, last class, hist)
    for c in 0..CTX_CLASSES {
        let empty = hist[c].iter().all(|&x| x == 0);
        if empty && !groups.is_empty() {
            groups.last_mut().unwrap().1 = c;
        } else if empty {
            groups.push((c, c, vec![0u64; asz]));
        } else if groups.len() == 1 && groups[0].2.iter().all(|&x| x == 0) {
            groups[0].1 = c;
            groups[0].2 = hist[c].clone();
        } else {
            groups.push((c, c, hist[c].clone()));
        }
    }
    let mut costs: Vec<f64> = groups.iter().map(|g| group_cost(&g.2, max_len)).collect();
    loop {
        if groups.len() <= 1 {
            break;
        }
        let mut best: Option<(f64, usize, Vec<u64>, f64)> = None;
        for i in 0..groups.len() - 1 {
            let merged: Vec<u64> = groups[i]
                .2
                .iter()
                .zip(&groups[i + 1].2)
                .map(|(a, b)| a + b)
                .collect();
            let mc = group_cost(&merged, max_len);
            let delta = mc - costs[i] - costs[i + 1];
            if best.as_ref().is_none_or(|b| delta < b.0) {
                best = Some((delta, i, merged, mc));
            }
        }
        let (delta, i, merged, mc) = best.unwrap();
        if delta > 0.0 && groups.len() <= max_ctx {
            break;
        }
        groups[i].1 = groups[i + 1].1;
        groups[i].2 = merged;
        costs[i] = mc;
        groups.remove(i + 1);
        costs.remove(i + 1);
    }
    let mut map = [0u8; CTX_CLASSES];
    for (gi, g) in groups.iter().enumerate() {
        for c in g.0..=g.1 {
            map[c] = gi as u8;
        }
    }
    (map, groups.into_iter().map(|g| g.2).collect())
}

// ---- fast row-array analysis ---------------------------------------------------

/// Per-row scratch: lane-aware neighbor arrays for one slot.
struct RowNb {
    cur: Vec<u32>,
    w: Vec<u32>,
    n: Vec<u32>,
    nw: Vec<u32>,
    ne: Vec<u32>,
    /// Guide values at x, x-1 (lane rule), and the row above.
    g: Vec<u32>,
    gw: Vec<u32>,
    gn: Vec<u32>,
}

impl RowNb {
    fn new(w: usize) -> Self {
        Self {
            cur: vec![0; w],
            w: vec![0; w],
            n: vec![0; w],
            nw: vec![0; w],
            ne: vec![0; w],
            g: vec![0; w],
            gw: vec![0; w],
            gn: vec![0; w],
        }
    }

    fn fill_guide(&mut self, g: &[u32], width: usize, sw: usize, y: usize, y0: usize) {
        let row = &g[y * width..(y + 1) * width];
        let mut xs = 0;
        while xs < width {
            let xe = (xs + sw).min(width);
            for x in xs..xe {
                let gv = row[x];
                let gn = if y > y0 { g[(y - 1) * width + x] } else { gv };
                self.g[x] = gv;
                self.gn[x] = gn;
                self.gw[x] = if x > xs { row[x - 1] } else { gn };
            }
            xs = xe;
        }
    }

    /// Fills neighbor arrays per the format's lane edge rules.
    fn fill(&mut self, p: &[u32], width: usize, sw: usize, y: usize, y0: usize) {
        let row = &p[y * width..(y + 1) * width];
        for (c, &v) in self.cur.iter_mut().zip(row) {
            *c = v;
        }
        if y == y0 {
            let mut xs = 0;
            while xs < width {
                let xe = (xs + sw).min(width);
                for x in xs..xe {
                    let wv = if x > xs { row[x - 1] } else { 0 };
                    self.w[x] = wv;
                    self.n[x] = wv;
                    self.nw[x] = wv;
                    self.ne[x] = wv;
                }
                xs = xe;
            }
            return;
        }
        let up = &p[(y - 1) * width..y * width];
        let mut xs = 0;
        while xs < width {
            let xe = (xs + sw).min(width);
            for x in xs..xe {
                let n = up[x];
                self.n[x] = n;
                let first = x == xs;
                self.w[x] = if first { n } else { row[x - 1] };
                self.nw[x] = if first { n } else { up[x - 1] };
                self.ne[x] = if x + 1 == xe { n } else { up[x + 1] };
            }
            xs = xe;
        }
    }
}

#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn pred_elem(p: u8, w: u32, n: u32, nw: u32, ne: u32, mask: u32, g: u32, gw: u32, gn: u32) -> u32 {
    let _ = (g, gw, gn);
    match p {
        7 => g,
        8 => w.wrapping_add(g).wrapping_sub(gw) & mask,
        9 => n.wrapping_add(g).wrapping_sub(gn) & mask,
        10 => {
            let lo = w.min(n);
            let hi = w.max(n);
            let m = if nw >= hi {
                lo
            } else if nw <= lo {
                hi
            } else {
                w.wrapping_add(n).wrapping_sub(nw)
            };
            predict::avg(g, m)
        }
        predict::P_W => w,
        predict::P_N => n,
        predict::P_AVG_WN => predict::avg(w, n),
        predict::P_MED => {
            let lo = w.min(n);
            let hi = w.max(n);
            let g = w.wrapping_add(n).wrapping_sub(nw);
            if nw >= hi {
                lo
            } else if nw <= lo {
                hi
            } else {
                g
            }
        }
        predict::P_GRAD => w.wrapping_add(n).wrapping_sub(nw) & mask,
        predict::P_PAETH => predict::paeth(w, n, nw),
        _ => predict::avg(w, ne),
    }
}

/// Residuals of `nb` under predictor `p` into `d` (modular, `bits` wide).
fn residuals(nb: &RowNb, p: u8, mask: u32, d: &mut [u32]) {
    macro_rules! go {
        ($p:expr) => {
            for x in 0..d.len() {
                d[x] = nb.cur[x].wrapping_sub(pred_elem(
                    $p, nb.w[x], nb.n[x], nb.nw[x], nb.ne[x], mask, nb.g[x], nb.gw[x], nb.gn[x],
                )) & mask;
            }
        };
    }
    match p {
        0 => go!(0),
        1 => go!(1),
        2 => go!(2),
        3 => go!(3),
        4 => go!(4),
        5 => go!(5),
        6 => go!(6),
        7 => go!(7),
        8 => go!(8),
        9 => go!(9),
        _ => go!(10),
    }
}

/// Bit length of the zigzag of each modular residual, summed.
#[inline(always)]
pub(crate) fn zz(d: u32, bits: u32, mask: u32) -> u32 {
    let sign = (d >> (bits.max(1) - 1)) & 1;
    ((d << 1) ^ 0u32.wrapping_sub(sign)) & mask
}

#[inline(always)]
fn bitlen_f(z: u32) -> u32 {
    // Exact for z < 2^24.
    (((z as f32).to_bits() >> 23) as i32 - 126).max(0) as u32
}

pub(crate) fn cost_sum(d: &[u32], bits: u32, mask: u32) -> u64 {
    if bits == 0 {
        return 0;
    }
    d.iter().map(|&v| bitlen_f(zz(v, bits, mask)) as u64).sum()
}

#[inline(always)]
fn sext(v: u32, bits: u32) -> i32 {
    if bits == 0 {
        return 0;
    }
    ((v << (32 - bits)) as i32) >> (32 - bits)
}

/// Applies decorrelation `m` to (G, R, B) residual rows into (r_out, b_out).
#[allow(clippy::too_many_arguments)]
pub(crate) fn decor_rows(
    m: u8,
    g: &[u32],
    r: &[u32],
    b: &[u32],
    bits: u32,
    mask: u32,
    ro: &mut [u32],
    bo: &mut [u32],
) {
    match m {
        1 => {
            for x in 0..g.len() {
                ro[x] = r[x].wrapping_sub(g[x]) & mask;
                bo[x] = b[x].wrapping_sub(g[x]) & mask;
            }
        }
        2 => {
            for x in 0..g.len() {
                ro[x] = r[x].wrapping_sub(g[x]) & mask;
                bo[x] = b[x].wrapping_sub(r[x]) & mask;
            }
        }
        _ => {
            for x in 0..g.len() {
                ro[x] = r[x].wrapping_sub(g[x]) & mask;
                let avg = (sext(r[x], bits) as i64 + sext(g[x], bits) as i64) >> 1;
                bo[x] = b[x].wrapping_sub(avg as u32) & mask;
            }
        }
    }
}

/// Fast equivalent of `choose_stripe` + `residual_row` for one stripe.
#[allow(clippy::too_many_arguments)]
pub(crate) fn analyze_stripe_fast(
    layout: &Layout,
    planes: &[&[u32]],
    w: usize,
    sw: usize,
    y0: usize,
    y1: usize,
    cfg: &AnalysisConfig,
    res: &mut [&mut [u32]],
    guide: Option<&[&[u32]]>,
) -> (u8, Vec<u8>) {
    analyze_stripe_impl::<true>(layout, planes, w, sw, y0, y1, cfg, res, guide)
}

/// Stripe analysis that also evaluates per-lane adaptive pixel color transform
/// (`DECOR_ADAPT_PIX = 4`, bit 4 of `cfg.decor_mask`) on eligible exact 8-bit
/// RGB/RGBA layouts when no inter/lab guide plane is present.
#[allow(clippy::too_many_arguments)]
pub(crate) fn analyze_stripe_with_adapt(
    layout: &Layout,
    planes: &[&[u32]],
    w: usize,
    sw: usize,
    y0: usize,
    y1: usize,
    cfg: &AnalysisConfig,
    res: &mut [&mut [u32]],
    guide: Option<&[&[u32]]>,
) -> Result<(u8, Option<crate::adapt_color::AdaptWeights>, Vec<u8>), Error> {
    let adapt_bit = 1u8 << crate::adapt_color::DECOR_ADAPT_PIX;
    #[cfg(feature = "lab")]
    let has_lab_guide = cfg.guide.is_some();
    #[cfg(not(feature = "lab"))]
    let has_lab_guide = false;
    let try_adapt = (cfg.decor_mask & adapt_bit) != 0
        && layout.adapt_pix_ok()
        && guide.is_none()
        && !has_lab_guide;
    if !try_adapt {
        let (decor, preds) = analyze_stripe_fast(layout, planes, w, sw, y0, y1, cfg, res, guide);
        return Ok((decor, None, preds));
    }

    let fixed_allowed = (cfg.decor_mask & 0x0f) != 0;
    let ns = planes.len();
    let rows = y1 - y0;
    let stripe_px = rows.checked_mul(w).ok_or(Error::TooLarge)?;
    let zz_cost = |slices: &[&[u32]]| -> u64 {
        slices
            .iter()
            .map(|s| s.iter().map(|&z| bitlen_f(z) as u64).sum::<u64>())
            .sum()
    };

    let (fixed_decor, fixed_preds, fixed_cost) = if fixed_allowed {
        let (d, p) = analyze_stripe_fast(layout, planes, w, sw, y0, y1, cfg, res, None);
        let refs: Vec<&[u32]> = res.iter().map(|v| &**v).collect();
        let c = zz_cost(&refs);
        (d, p, c)
    } else {
        (0, Vec::new(), u64::MAX)
    };

    let (aw, [r_tx, b_tx]) =
        crate::adapt_color::fit_and_transform_stripe_pixel(planes, w, sw, y0, y1)?;
    let row_span = y0 * w..y1 * w;
    let mut tx_refs: Vec<&[u32]> = Vec::with_capacity(ns);
    for (s, plane) in planes.iter().enumerate() {
        tx_refs.push(match s {
            1 => r_tx.as_slice(),
            2 => b_tx.as_slice(),
            _ => &plane[row_span.clone()],
        });
    }
    let mut tx_cfg = cfg.clone();
    tx_cfg.decor_mask = 1; // Mode 0 only: pixel color transform is already applied.
    if !fixed_allowed {
        // Write directly into caller's `res` without allocating a second candidate buffer.
        let (_, tx_preds) =
            analyze_stripe_fast(layout, &tx_refs, w, sw, 0, rows, &tx_cfg, res, None);
        return Ok((crate::adapt_color::DECOR_ADAPT_PIX, Some(aw), tx_preds));
    }

    let mut tx_res: Vec<Vec<u32>> = Vec::with_capacity(ns);
    for _ in 0..ns {
        let mut v = Vec::new();
        v.try_reserve_exact(stripe_px)
            .map_err(|_| Error::TooLarge)?;
        v.resize(stripe_px, 0);
        tx_res.push(v);
    }
    let mut tx_views: Vec<&mut [u32]> = tx_res.iter_mut().map(Vec::as_mut_slice).collect();
    let (_, tx_preds) = analyze_stripe_fast(
        layout,
        &tx_refs,
        w,
        sw,
        0,
        rows,
        &tx_cfg,
        &mut tx_views,
        None,
    );
    let tx_res_refs: Vec<&[u32]> = tx_res.iter().map(Vec::as_slice).collect();
    let widths = crate::v1::lane_widths(w, sw);
    let active = crate::v1::active_lanes(&widths);
    let hdr_bits = (aw.byte_len(active) as u64) * 8;
    let adapt_cost = zz_cost(&tx_res_refs).saturating_add(hdr_bits);

    if adapt_cost < fixed_cost {
        for s in 0..ns {
            res[s].copy_from_slice(&tx_res[s]);
        }
        Ok((crate::adapt_color::DECOR_ADAPT_PIX, Some(aw), tx_preds))
    } else {
        Ok((fixed_decor, None, fixed_preds))
    }
}

fn analyze_stripe_impl<const FIXED_W: bool>(
    layout: &Layout,
    planes: &[&[u32]],
    w: usize,
    sw: usize,
    y0: usize,
    y1: usize,
    cfg: &AnalysisConfig,
    res: &mut [&mut [u32]],
    guide: Option<&[&[u32]]>,
) -> (u8, Vec<u8>) {
    let ns = planes.len();
    let cs = layout.color_slots;
    let decors: Vec<u8> = (0..NUM_DECOR)
        .filter(|&m| (cfg.decor_mask >> m) & 1 == 1 && (m == 0 || layout.decor_ok))
        .collect();
    let decors = if decors.is_empty() { vec![0] } else { decors };
    #[allow(unused_mut)]
    let mut preds: Vec<u8> = (0..NUM_PREDICTORS)
        .filter(|&p| (cfg.predictor_mask >> p) & 1 == 1)
        .collect();
    // Reference planes (inter frames): predictors 7 (reference), 8 (W +
    // reference gradient), 9 (N + reference gradient); 7 also on a stripe's
    // first row.
    #[allow(unused_mut)]
    let mut guide: Option<Vec<&[u32]>> = guide.map(|g| g.to_vec());
    if guide.is_some() {
        preds.extend([predict::P_REF, predict::P_REF_W, predict::P_REF_N]);
    }
    // Lab: guide planes of the two-layer experiment (also predictor 10).
    #[cfg(feature = "lab")]
    let lab_guide: Vec<Vec<u32>>;
    #[cfg(feature = "lab")]
    if guide.is_none()
        && let Some(g) = &cfg.guide
    {
        preds.extend([7u8, 8, 9, 10]);
        lab_guide = layout
            .slot_channel
            .iter()
            .map(|&c| g[c].iter().map(|&v| v as u32).collect())
            .collect();
        guide = Some(lab_guide.iter().map(|v| v.as_slice()).collect());
    }
    let preds = if preds.is_empty() { vec![0] } else { preds };
    let rows = y1 - y0;
    let masks: Vec<u32> = layout
        .slot_bits
        .iter()
        .map(|&b| if b == 0 { 0 } else { ((1u64 << b) - 1) as u32 })
        .collect();
    let mut d: Vec<Vec<u32>> = (0..ns).map(|_| vec![0u32; w]).collect();
    let (mut ro, mut bo) = (vec![0u32; w], vec![0u32; w]);
    // With no prediction/decorrelation choice, scoring cannot affect the
    // result. Emit the same modular W residuals directly, retaining the
    // north-at-lane-start rule on rows after the stripe's first row.
    if FIXED_W && guide.is_none() && preds == [predict::P_W] && decors.len() == 1 {
        let decor = decors[0];
        for y in y0..y1 {
            for s in 0..ns {
                let row = &planes[s][y * w..(y + 1) * w];
                for xs in (0..w).step_by(sw) {
                    let xe = (xs + sw).min(w);
                    let mut west = if y == y0 {
                        0
                    } else {
                        planes[s][(y - 1) * w + xs]
                    };
                    for x in xs..xe {
                        d[s][x] = row[x].wrapping_sub(west) & masks[s];
                        west = row[x];
                    }
                }
            }
            if cs == 3 && layout.decor_ok && decor != 0 {
                decor_rows(
                    decor,
                    &d[0],
                    &d[1],
                    &d[2],
                    layout.slot_bits[0],
                    masks[0],
                    &mut ro,
                    &mut bo,
                );
                d[1].copy_from_slice(&ro);
                d[2].copy_from_slice(&bo);
            }
            for s in 0..ns {
                let row = &mut res[s][(y - y0) * w..(y - y0 + 1) * w];
                for (out, &delta) in row.iter_mut().zip(&d[s]) {
                    *out = zz(delta, layout.slot_bits[s], masks[s]);
                }
            }
        }
        return (decor, vec![predict::P_W; rows]);
    }
    let mut nbs: Vec<RowNb> = (0..ns).map(|_| RowNb::new(w)).collect();
    // cost[decor][row][pred]
    let mut cost = vec![vec![vec![u64::MAX / 4; preds.len()]; rows]; decors.len()];
    let do_decor = cs == 3 && layout.decor_ok;
    for y in y0..y1 {
        for s in 0..ns {
            nbs[s].fill(planes[s], w, sw, y, y0);
            if let Some(g) = &guide {
                nbs[s].fill_guide(g[s], w, sw, y, y0);
            }
        }
        for (pi, &p) in preds.iter().enumerate() {
            if y == y0 && p != predict::P_W && p != predict::P_REF {
                continue;
            }
            let pp = p;
            for s in 0..ns {
                residuals(&nbs[s], pp, masks[s], &mut d[s]);
            }
            let base: u64 = (0..ns)
                .filter(|&s| !(do_decor && (s == 1 || s == 2)))
                .map(|s| cost_sum(&d[s], layout.slot_bits[s], masks[s]))
                .sum();
            for (di, &m) in decors.iter().enumerate() {
                let c = if do_decor {
                    let bits = layout.slot_bits[0];
                    if m == 0 {
                        base + cost_sum(&d[1], bits, masks[1]) + cost_sum(&d[2], bits, masks[2])
                    } else {
                        decor_rows(m, &d[0], &d[1], &d[2], bits, masks[0], &mut ro, &mut bo);
                        base + cost_sum(&ro, bits, masks[0]) + cost_sum(&bo, bits, masks[0])
                    }
                } else {
                    base
                };
                cost[di][y - y0][pi] = c;
            }
        }
    }
    let mut best = (u64::MAX, 0usize);
    for di in 0..decors.len() {
        let total: u64 = cost[di].iter().map(|r| *r.iter().min().unwrap()).sum();
        if total < best.0 {
            best = (total, di);
        }
    }
    let di = best.1;
    let decor = decors[di];
    let mut pr: Vec<u8> = cost[di]
        .iter()
        .map(|r| {
            let (pi, _) = r.iter().enumerate().min_by_key(|&(_, &c)| c).unwrap();
            preds[pi]
        })
        .collect();
    if pr[0] != predict::P_REF {
        pr[0] = predict::P_W;
    }
    // Residual planes for the chosen predictors.
    for y in y0..y1 {
        for s in 0..ns {
            nbs[s].fill(planes[s], w, sw, y, y0);
            if let Some(g) = &guide {
                nbs[s].fill_guide(g[s], w, sw, y, y0);
            }
            residuals(&nbs[s], pr[y - y0], masks[s], &mut d[s]);
        }
        if do_decor && decor != 0 {
            let bits = layout.slot_bits[0];
            decor_rows(decor, &d[0], &d[1], &d[2], bits, masks[0], &mut ro, &mut bo);
            d[1].copy_from_slice(&ro);
            d[2].copy_from_slice(&bo);
        }
        for s in 0..ns {
            let bits = layout.slot_bits[s];
            let row = &mut res[s][(y - y0) * w..(y - y0 + 1) * w];
            for x in 0..w {
                row[x] = if bits == 0 {
                    0
                } else {
                    zz(d[s][x], bits, masks[s])
                };
            }
        }
    }
    (decor, pr)
}

#[cfg(all(test, feature = "encode"))]
#[path = "encode_fixed_tests.rs"]
mod fixed_tests;

#[cfg(all(test, feature = "encode"))]
mod allocation_tests {
    use super::*;

    #[test]
    fn unrepresentable_analysis_capacity_is_an_error() {
        assert!(matches!(reserved::<u32>(usize::MAX), Err(Error::TooLarge)));
        assert!(matches!(filled(usize::MAX, 0u32), Err(Error::TooLarge)));
    }
}
