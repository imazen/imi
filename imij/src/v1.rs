//! Format v1: independent stripes, 16 column lanes, interleaved rANS.

#[cfg(all(test, feature = "lab", feature = "std"))]
pub(crate) mod model_study;
mod owned_rows;
pub(crate) use owned_rows::{OwnedLossless, owned_extra_bytes};

use alloc::vec;
use alloc::vec::Vec;
use how_far::Pulse;
#[cfg(feature = "encode")]
use how_far::{Complete, PhaseSpec, ProgressExt, Stages, Total};

#[cfg(all(test, feature = "encode"))]
#[path = "inter_parity.rs"]
mod inter_parity;

use crate::bits::BitReader;
#[cfg(feature = "encode")]
use crate::bits::BitWriter;
#[cfg(feature = "encode")]
use crate::coding_layout::CTX_CLASSES;
use crate::coding_layout::{Layout, MAX_SLOTS};
#[cfg(all(test, feature = "encode"))]
use crate::encode::analyze;
#[cfg(feature = "encode")]
use crate::encode::{self, AnalysisConfig, choose_ctx_map, default_lanes};
use crate::mapping::{ChannelMap, MapMode};
use crate::predict::{self, unzigzag};
#[cfg(feature = "encode")]
use crate::put_varint;
#[cfg(feature = "encode")]
use crate::rans::NCTX;
use crate::rans::{self, DecModel, LaneState, Model, NL, Words};
#[cfg(feature = "encode")]
use crate::rans::{CLASSES, Sym};
#[cfg(feature = "encode")]
use crate::tokens::TokenCfg;
use crate::{ByteReader, Error, ImageInfo, Metadata, SampleType};

/// Lossless head presence bits (SPEC 2).
pub(crate) const H_PALETTE: u64 = 1;
pub(crate) const H_QUICK: u64 = 1 << 3;
pub(crate) const H_QUICK_BITS: u64 = 1 << 4;
pub(crate) const H_RECORDS: u64 = 1 << 1;
/// Lab builds only: the head names its standalone-seeds bank (a bank byte
/// follows `present`). Conforming decoders find the bit undefined
/// (`Unsupported`).
pub(crate) const H_LAB_BANK: u64 = 1 << 2;
#[cfg(feature = "encode")]
use crate::parse::records::put_record;
/// Lossless head record ids (SPEC 2): the unit table hint and the
/// near-lossless bounds.
pub(crate) use crate::parse::records::{H_UNIT_ORDINALS, H_UNIT_RESERVE, H_UNIT_TABLE, R_STRIPE};
#[cfg(feature = "encode")]
use crate::parse::records::{main_framed_len, put_main_record};
pub(crate) const H_NEAR: u64 = crate::registry::HEAD_NEAR;

/// The channel maps of a lossless head written for `info`: their count (one
/// per coded word, two per 64-bit channel) and bytes each (mode, bits, and a
/// u16 or, for 32- and 64-bit samples, u32 value), before a value table.
#[cfg(feature = "encode")]
fn map_shape(info: &ImageInfo) -> (usize, usize) {
    let entry = 2 + if info.sample.bytes() >= 4 { 4 } else { 2 };
    (info.channels.count() * info.sample.words(), entry)
}

/// Features a lossless payload head written for `info` uses (SPEC 0.1):
/// `near_lossless` for a bounds record, `float_shift`, `offset_map` and
/// `value_tables` for those channel maps. Reads the encoder's own head layout.
#[cfg(feature = "encode")]
pub(crate) fn head_features(info: &ImageInfo, head: &[u8]) -> crate::parse::header::Features {
    use crate::parse::header::{Feature, Features};
    let walk = || -> Result<Features, Error> {
        let mut r = ByteReader::new(head);
        let present = r.varint()?;
        if present & H_QUICK != 0 {
            return Ok(crate::quick::features(if present & H_QUICK_BITS == 0 {
                crate::QuickBitSmarts::Off
            } else {
                crate::QuickBitSmarts::Packed
            }));
        }
        if present & H_LAB_BANK != 0 {
            r.varint()?;
        }
        r.u8()?;
        r.varint()?;
        r.varint()?;
        let (maps, entry) = map_shape(info);
        let mut f = Features::EMPTY;
        for _ in 0..maps {
            let m = r.bytes(entry)?;
            if m[0] == MapMode::FloatShift as u8 {
                f = f.with(Feature::FloatShift);
            }
            if m[0] == MapMode::Offset as u8 {
                f = f.with(Feature::OffsetMap);
            }
            if m[0] == MapMode::Table as u8 {
                f = f.with(Feature::ValueTables);
                // The entry count minus one: the value field's low bytes.
                let count = u16::from_le_bytes([m[2], m[3]]) as usize + 1;
                for _ in 0..count {
                    r.varint()?;
                }
            }
        }
        let n = r.var_usize()?;
        r.bytes(n)?;
        if present & H_RECORDS != 0 {
            let n = r.var_usize()?;
            let mut rr = ByteReader::new(r.bytes(n)?);
            while rr.remaining() != 0 {
                let id = rr.varint()?;
                let len = rr.var_usize()?;
                rr.bytes(len)?;
                if id == H_NEAR {
                    f = f.with(Feature::NearLossless);
                }
                if id == crate::bilevel::RECORD {
                    f = f.with(Feature::Bilevel);
                }
                if id == crate::template::RECORD {
                    f = f.with(Feature::TemplatePalettes);
                }
                if id == H_UNIT_ORDINALS {
                    f = f.with(Feature::UnitOrdinals);
                }
            }
        }
        Ok(f)
    };
    walk().expect("the encoder's own head")
}

/// Encoder settings.
///
/// New fields can arrive in any release: start from [`EncodeConfig::default`]
/// and assign the fields to change.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
#[cfg(feature = "encode")]
pub(crate) struct EncodeConfig {
    pub stripe_rows: Option<u32>,
    pub lanes: Option<u32>,
    pub k_low: u32,
    pub k_high: u32,
    pub m: u32,
    pub run_min: u32,
    pub max_ctx: usize,
    pub ctx_cross: bool,
    pub decor_mask: u8,
    pub predictor_mask: u8,
    /// Encoder threads (stripes are independent).
    pub threads: usize,
    /// Lab: guide planes for the two-layer experiment (output not decodable).
    #[cfg(feature = "lab")]
    #[doc(hidden)]
    pub guide: Option<alloc::sync::Arc<Vec<Vec<u16>>>>,
    /// Encoder search effort, finite 0..=10, including fractional values.
    /// Lower values try fewer allowed predictors and decorrelation modes.
    /// Default 7.5 balances measured encode cost and size. Use 10 for the
    /// full search. Reconstruction stays lossless at every effort.
    pub effort: f32,
    /// Code stripes with at most 256 distinct colors as palette indices when
    /// that is estimated smaller, and try template palette stripes (SPEC
    /// 2.14, also for one-channel images) where they make the file smaller.
    pub palette: bool,
    /// Column groups: stripes split into independent column ranges of this
    /// width (each with its own lanes and streams). `None` = automatic,
    /// `Some(0)` = never.
    pub column_groups: Option<u32>,
    /// Optional decoder working-memory target; see [`crate::DecoderMemoryLimit`].
    pub decoder_memory_limit: Option<crate::DecoderMemoryLimit>,
    /// Error-bounded near-lossless coding (Scientific profile): the largest
    /// absolute difference between a decoded and a source sample, per memory
    /// channel (R, G, B, A or Gray, A), in sample units. 0 (the default)
    /// keeps the channel exact. Integer samples only. See
    /// [`Self::with_max_error`].
    pub max_error: [u32; 4],
    /// Allow the Scientific profile's float map for f16/f32 samples whose
    /// low raw bits are zero in every sample (BitRound-ed data); it is used
    /// only when it codes fewer bits than every other map. Default true.
    pub float_shift: bool,
    /// Allow value-table channel maps (SPEC 2.1 map mode 9, Scientific
    /// profile 0.2) on words wider than 8 bits: a channel that uses few of
    /// its container's values codes each sample's index among them, where
    /// that is estimated smaller. Default true. Half floats from tone-mapping
    /// inverses and floats from Radiance files code 6-30% smaller
    /// (docs/VALUE_TABLES.md in the repository); on 8- and 16-bit output the
    /// decoder then looks every sample up in the table.
    pub value_tables: bool,
    /// Write a file checksum (`cSUM` chunk before `DATA`, SPEC 1.4): XXH3-128
    /// of every 64 KiB to 1 MiB piece of the file. Off by default. Nested
    /// images (previews, frames, plane tiles) never get one: the outer
    /// file's covers them.
    pub checksum: bool,
    /// The checksum's digest algorithm.
    pub hash: crate::HashAlgorithm,
    /// Prefix every main unit record in `DATA` with its 0-based ordinal
    /// (head record 37, feature `unit_ordinals`, SPEC 2.7), so indexed
    /// region readers verify records locally without reading every record
    /// head at open. Off by default.
    pub unit_ordinals: bool,
    /// LAB: full-width bank ID for experimental standalone seed tables.
    /// Zero disables seeds; `seed_exact` controls explicit comparison.
    /// This remains a lab-only format, not a normative profile feature.
    #[cfg(feature = "lab")]
    #[doc(hidden)]
    pub seed_bank: u64,
    #[cfg(feature = "lab")]
    #[doc(hidden)]
    pub seed_exact: bool,
}

#[cfg(feature = "encode")]
impl Default for EncodeConfig {
    fn default() -> Self {
        Self {
            stripe_rows: None,
            lanes: None,
            // k = 5 for <= 8-bit: +12% photo decode speed for +0.45% size vs 4.
            k_low: 5,
            k_high: 4,
            m: 2,
            run_min: 4,
            max_ctx: NCTX,
            ctx_cross: true,
            decor_mask: 0b1111,
            predictor_mask: 0x7F,
            threads: 1,
            effort: 7.5,
            palette: true,
            column_groups: None,
            decoder_memory_limit: None,
            max_error: [0; 4],
            float_shift: true,
            value_tables: true,
            checksum: false,
            hash: crate::HashAlgorithm::DEFAULT,
            unit_ordinals: false,
            #[cfg(feature = "lab")]
            guide: None,
            #[cfg(feature = "lab")]
            seed_bank: 0,
            #[cfg(feature = "lab")]
            seed_exact: true,
        }
    }
}

#[cfg(feature = "encode")]
impl EncodeConfig {
    /// Constrains the decoder's estimated buffers. Encoding may restore 16
    /// lanes, narrow column groups or disable palettes to fit. An impossible
    /// budget returns `Error::InvalidInput` before image analysis.
    #[cfg(all(test, feature = "encode"))]
    pub fn with_decoder_memory_limit(mut self, limit: crate::DecoderMemoryLimit) -> Self {
        self.decoder_memory_limit = Some(limit);
        self
    }

    /// Estimated bytes after applying the memory policy for this image. Without
    /// a budget, estimates the current layout for a single decoder worker.
    /// Includes conservative allowances for data-dependent channel maps.
    pub fn estimated_decoder_memory(&self, info: &ImageInfo) -> Result<u64, Error> {
        crate::decoder_memory::plan_lossless(info, self).map(|(_, bytes)| bytes)
    }

    /// Sets search effort (validated when encoding). See [`Self::effort`].
    #[cfg(all(test, feature = "encode"))]
    pub fn with_effort(mut self, effort: f32) -> Self {
        self.effort = effort;
        self
    }

    /// Error-bounded near-lossless coding of every channel: each decoded
    /// sample differs from its source by at most `max_error` sample units
    /// (JPEG-LS NEAR style; the file requires the Scientific profile). 0
    /// keeps lossless coding. Integer samples only: encoding f16/f32 with a
    /// nonzero bound fails with [`Error::InvalidInput`].
    #[cfg(all(test, feature = "encode"))]
    pub fn with_max_error(mut self, max_error: u32) -> Self {
        self.max_error = [max_error; 4];
        self
    }
}

/// Images at least this wide get column groups by default.
#[cfg(feature = "encode")]
const AUTO_GROUP_MIN_WIDTH: usize = 8192;
/// Default column group width: lanes of 128 columns, like a 2048-wide
/// image. Zen 4: 32768x2048 photos 1 thread +21%, 12 threads +11%;
/// 43534x900 screenshots +41% / +27%; 64-row strips on 12 threads
/// 2.6-3.2x; bytes -2.3..+0.1%. Corpus images 2-6K wide: neutral speed,
/// +0.25% bytes, hence the width threshold
/// (benchmarks/lossless_column_groups_zen4_2026-10-03.md).
#[cfg(feature = "encode")]
const AUTO_GROUP_COLS: usize = 2048;

/// Column group width for an image `w` wide (0 = no groups).
#[cfg(feature = "encode")]
pub(crate) fn group_cols(cfg: &EncodeConfig, w: usize) -> usize {
    let g = match cfg.column_groups {
        Some(g) => g as usize,
        None if w >= AUTO_GROUP_MIN_WIDTH => AUTO_GROUP_COLS,
        None => 0,
    };
    if g == 0 || g >= w { 0 } else { g }
}

#[cfg(feature = "encode")]
pub(crate) fn default_stripe_rows(w: u32) -> u32 {
    // ~256K pixels per stripe, 16..=64 rows. The 64-row cap gives 1-2 MP
    // graphics enough stripes to use threads: 12-thread decode +64% on
    // graphics and +10% on photos for +1.1% / +0.02% bytes, single-thread
    // unchanged (benchmarks/lossless_stripe_rows_zen4_2026-10-03.md).
    (262_144 / w.max(1)).clamp(16, 64)
}

/// Step-major residual plane of one stripe slot.
#[cfg(feature = "encode")]
pub(crate) fn step_major(res: &[u32], w: usize, lw: usize, y0: usize, y1: usize) -> Vec<u32> {
    let rows = y1 - y0;
    let mut v = vec![0u32; rows * lw * NL];
    for (l, xs) in (0..w).step_by(lw).enumerate() {
        let xe = (xs + lw).min(w);
        for r in 0..rows {
            let row = &res[(y0 + r) * w..];
            for x in xs..xe {
                v[(r * lw + x - xs) * NL + l] = row[x];
            }
        }
    }
    v
}

/// Lanes with at least one column (always a prefix).
pub(crate) fn active_lanes(widths: &[u32; NL]) -> usize {
    widths.iter().take_while(|&&w| w > 0).count()
}

pub(crate) fn lane_widths(w: usize, lw: usize) -> [u32; NL] {
    core::array::from_fn(|l| {
        let xs = l * lw;
        if xs >= w {
            0
        } else {
            ((xs + lw).min(w) - xs) as u32
        }
    })
}

/// Test helper: the lossless coder without progress.
#[cfg(all(test, feature = "encode"))]
#[cfg(feature = "encode")]
pub(crate) fn encode(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &EncodeConfig,
) -> Result<Vec<u8>, Error> {
    encode_with_progress(pixels, stride, info, meta, cfg, &how_far::NoPulse)
}

#[cfg(feature = "encode")]
pub(crate) fn encode_with_progress(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &EncodeConfig,
    pulse: &dyn Pulse,
) -> Result<Vec<u8>, Error> {
    pulse.check().map_err(|_| Error::Cancelled)?;
    Stages::new(
        pulse,
        &[
            PhaseSpec::new("lossless payload", 1, Total::Unknown),
            PhaseSpec::new("unit index", 1, Total::Unknown),
            PhaseSpec::new("locator", 1, Total::Unknown),
            PhaseSpec::new("file checksum", 1, Total::Unknown),
        ],
    )
    .complete_with(|stages| {
        let out =
            stages.run(|p| encode_frame_with_progress(pixels, stride, info, meta, cfg, None, p))?;
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
            let out = crate::checksum::finish_file(out, cfg.checksum.then_some(cfg.hash))?;
            p.check().map_err(|_| Error::Cancelled)?;
            Ok(out)
        })
    })
}

/// [`encode`] of an inter frame (or any nested image: never a checksum): `reference` (pixels, stride; the image's
/// size and format) is what the decoder's buffer holds (the previous
/// frame's pixels under the frame's rectangle).
#[cfg(feature = "encode")]
pub(crate) fn encode_frame(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &EncodeConfig,
    reference: Option<(&[u8], usize)>,
) -> Result<Vec<u8>, Error> {
    encode_frame_with_progress(
        pixels,
        stride,
        info,
        meta,
        cfg,
        reference,
        &how_far::NoPulse,
    )
}

#[cfg(feature = "encode")]
pub(crate) fn encode_frame_with_progress(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &EncodeConfig,
    reference: Option<(&[u8], usize)>,
    pulse: &dyn Pulse,
) -> Result<Vec<u8>, Error> {
    pulse.check().map_err(|_| Error::Cancelled)?;
    #[cfg(feature = "lab")]
    crate::seeds::validate_id(cfg.seed_bank)?;
    // Seed tables are signalled by a lossy file's envelope; standalone
    // images use them only in the lab's experimental standalone-seeds format
    // (still images; not a profile version).
    #[cfg(feature = "lab")]
    let seeds = (reference.is_none())
        .then(|| crate::seeds::bank(cfg.seed_bank))
        .flatten()
        .map(|b| (b, cfg.seed_exact));
    #[cfg(not(feature = "lab"))]
    let seeds = None;
    let (head, stripes, explicit) = encode_parts_ref_with_progress(
        pixels,
        stride,
        info,
        cfg,
        reference,
        seeds,
        true,
        pulse,
        None,
        #[cfg(all(test, feature = "lab", feature = "std"))]
        None,
    )?;
    let file = |head: Vec<u8>, stripes: Vec<Vec<u8>>| -> Result<Vec<u8>, Error> {
        pulse.check().map_err(|_| Error::Cancelled)?;
        let capacity = stripes
            .iter()
            .try_fold(head.len(), |n, s| n.checked_add(s.len())?.checked_add(4))
            .and_then(|n| n.checked_add(256))
            .ok_or(Error::TooLarge)?;
        let mut out = Vec::new();
        out.try_reserve(capacity).map_err(|_| Error::TooLarge)?;
        crate::container::write_head_features(
            &mut out,
            info,
            meta,
            crate::Coding::Lossless,
            None,
            crate::DecodeSpeed::Balanced,
            head_features(info, &head),
        )?;
        // The head (with the unit table hint for two or more units, SPEC
        // 2.7), then one framed record per unit.
        let mut data = head;
        let ordinals = cfg.unit_ordinals;
        for (i, sb) in stripes.iter().enumerate() {
            pulse.check().map_err(|_| Error::Cancelled)?;
            put_main_record(&mut data, R_STRIPE, ordinals.then_some(i as u64), sb);
        }
        crate::container::put_data(&mut out, &data);
        pulse.check().map_err(|_| Error::Cancelled)?;
        Ok(out)
    };
    let out = file(head, stripes)?;
    // With seed tables, the explicit coding wins ties: never larger.
    match explicit {
        Some((h, s)) => {
            let alt = file(h, s)?;
            Ok(if out.len() < alt.len() { out } else { alt })
        }
        None => Ok(out),
    }
}

/// Bilevel and template palette stripes (SPEC 2.10, 2.14): which tables to
/// write and which units take them. The tables (none, bilevel, template,
/// both) giving the fewest bytes win, each unit taking its smallest allowed
/// coding and each table counted with 8 bytes of framing. Ties keep fewer
/// tables, and a table no unit would use is left out. `units` holds the
/// ordinary stripes and receives the chosen ones.
#[allow(clippy::type_complexity)]
#[cfg(feature = "encode")]
fn choose_tables(
    units: &mut [Vec<u8>],
    bilevel: Option<&(Vec<u8>, Vec<Vec<u8>>)>,
    template: Option<&(Vec<u8>, Vec<Option<Vec<u8>>>)>,
) -> (bool, bool) {
    let pick = |use_bl: bool, use_tp: bool, u: usize| -> (usize, u8) {
        let mut best = (units[u].len(), 0);
        if let Some((_, b)) = bilevel.filter(|_| use_bl)
            && b[u].len() < best.0
        {
            best = (b[u].len(), 1);
        }
        if let Some((_, t)) = template.filter(|_| use_tp)
            && let Some(t) = &t[u]
            && t.len() < best.0
        {
            best = (t.len(), 2);
        }
        best
    };
    let mut best = (usize::MAX, false, false);
    for (use_bl, use_tp) in [(false, false), (true, false), (false, true), (true, true)] {
        if (use_bl && bilevel.is_none()) || (use_tp && template.is_none()) {
            continue;
        }
        let mut total = 0;
        let mut used = [0usize; 3];
        for u in 0..units.len() {
            let (len, kind) = pick(use_bl, use_tp, u);
            total += len;
            used[kind as usize] += 1;
        }
        if (use_bl && used[1] == 0) || (use_tp && used[2] == 0) {
            continue;
        }
        total += bilevel.filter(|_| use_bl).map_or(0, |b| b.0.len() + 8);
        total += template.filter(|_| use_tp).map_or(0, |t| t.0.len() + 8);
        if total < best.0 {
            best = (total, use_bl, use_tp);
        }
    }
    let (_, use_bl, use_tp) = best;
    let kinds: Vec<u8> = (0..units.len())
        .map(|u| pick(use_bl, use_tp, u).1)
        .collect();
    for (u, kind) in kinds.into_iter().enumerate() {
        match (kind, bilevel, template) {
            (1, Some((_, b)), _) => units[u].clone_from(&b[u]),
            (2, _, Some((_, t))) => units[u].clone_from(t[u].as_ref().unwrap()),
            _ => {}
        }
    }
    (use_bl, use_tp)
}

/// A lossless payload in parts: the head (coding parameters and models,
/// [`parse_head`]) and the coded stripes.
/// A palette stripe: entries (slot values packed 16 bits each, slot 0
/// highest), row predictors and the index residual plane (step-major).
#[cfg(feature = "encode")]
struct PalStripe {
    entries: Vec<u64>,
    preds: Vec<u8>,
    plane: Vec<u32>,
}

/// Rough coded size in bits of a zigzag residual run: order-0 entropy of
/// the bit lengths plus the bits below the leading one.
#[cfg(feature = "encode")]
pub(crate) fn est_bits(res: &[u32]) -> f64 {
    let mut hist = [0u64; 33];
    let mut extra = 0u64;
    for &v in res {
        let b = 32 - v.leading_zeros();
        hist[b as usize] += 1;
        extra += b.saturating_sub(1) as u64;
    }
    let n = res.len() as f64;
    let mut h = 0.0;
    for &c in &hist {
        if c > 0 {
            let c = c as f64;
            h += c * (flog2(n) - flog2(c));
        }
    }
    h + extra as f64
}

/// log2 for x >= 1 (no_std; within 0.005).
#[cfg(feature = "encode")]
fn flog2(x: f64) -> f64 {
    let b = 63 - (x as u64).leading_zeros();
    let m = x / (1u64 << b) as f64;
    b as f64 + (m - 1.0) * (1.0 + 0.346 * (2.0 - m))
}

/// Palette of slot values `y0..y1` when it has at most 256 colors, ordered
/// by the sum of the slot values, and the stripe's index plane.
#[cfg(feature = "encode")]
fn stripe_palette(
    slots: &[&[u32]],
    w: usize,
    y0: usize,
    y1: usize,
) -> Option<(Vec<u64>, Vec<u32>)> {
    const T: usize = 1024;
    let mut keys = vec![0u64; T];
    let mut used = vec![false; T];
    let key = |i: usize| slots.iter().fold(0u64, |k, p| (k << 16) | p[i] as u64);
    let mut n = 0usize;
    let mut last = None;
    for i in y0 * w..y1 * w {
        let k = key(i);
        if last == Some(k) {
            continue;
        }
        last = Some(k);
        let mut h = (k.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 54) as usize;
        loop {
            if !used[h] {
                n += 1;
                if n > 256 {
                    return None;
                }
                used[h] = true;
                keys[h] = k;
                break;
            }
            if keys[h] == k {
                break;
            }
            h = (h + 1) & (T - 1);
        }
    }
    let ns = slots.len();
    let sum = |k: u64| (0..ns).map(|s| (k >> (16 * s)) & 0xFFFF).sum::<u64>();
    let mut entries: Vec<u64> = (0..T).filter(|&h| used[h]).map(|h| keys[h]).collect();
    entries.sort_unstable_by_key(|&k| (sum(k), k));
    let mut idx_of = vec![0u32; T];
    for (i, &k) in entries.iter().enumerate() {
        let mut h = (k.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 54) as usize;
        while keys[h] != k {
            h = (h + 1) & (T - 1);
        }
        idx_of[h] = i as u32;
    }
    let mut plane = Vec::with_capacity((y1 - y0) * w);
    // No sentinel key: four 16-bit slots of 0xFFFF pack to u64::MAX.
    let mut last: Option<(u64, u32)> = None;
    for i in y0 * w..y1 * w {
        let k = key(i);
        let idx = match last {
            Some((lk, li)) if lk == k => li,
            _ => {
                let mut h = (k.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 54) as usize;
                while keys[h] != k {
                    h = (h + 1) & (T - 1);
                }
                last = Some((k, idx_of[h]));
                idx_of[h]
            }
        };
        plane.push(idx);
    }
    Some((entries, plane))
}

#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
#[cfg(feature = "encode")]
pub(crate) fn encode_parts(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    cfg: &EncodeConfig,
) -> Result<Parts, Error> {
    encode_parts_ref(pixels, stride, info, cfg, None, None, false)
}

/// [`encode_parts`] of a lossy file's nested image, its tables seeded from
/// `seeds` (bank, and whether to also code the explicit alternative).
#[cfg(feature = "encode")]
pub(crate) fn encode_parts_seeded(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    cfg: &EncodeConfig,
    seeds: Option<(&'static crate::seeds::Bank, bool)>,
) -> Result<Parts, Error> {
    encode_parts_ref(pixels, stride, info, cfg, None, seeds, false)
}

#[cfg(feature = "encode")]
pub(crate) fn encode_parts_seeded_with_progress(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    cfg: &EncodeConfig,
    seeds: Option<(&'static crate::seeds::Bank, bool)>,
    pulse: &dyn Pulse,
) -> Result<Parts, Error> {
    encode_parts_ref_with_progress(
        pixels,
        stride,
        info,
        cfg,
        None,
        seeds,
        false,
        pulse,
        None,
        #[cfg(all(test, feature = "lab", feature = "std"))]
        None,
    )
}

/// Numeric coding with explicit DC rectangles, already checked against geometry.
#[cfg(feature = "encode")]
pub(crate) fn encode_dc_rectangles(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    cfg: &EncodeConfig,
    seeds: Option<(&'static crate::seeds::Bank, bool)>,
    rectangles: &[(usize, usize, usize)],
    pulse: &dyn Pulse,
) -> Result<Parts, Error> {
    encode_parts_ref_with_progress(
        pixels,
        stride,
        info,
        cfg,
        None,
        seeds,
        false,
        pulse,
        Some(rectangles),
        #[cfg(all(test, feature = "lab", feature = "std"))]
        None,
    )
}

/// A coded lossless payload: head, stripe records, and with seed tables the
/// same payload coded with explicit tables (head, records).
#[cfg(feature = "encode")]
pub(crate) type Parts = (Vec<u8>, Vec<Vec<u8>>, Option<(Vec<u8>, Vec<Vec<u8>>)>);

/// A container sample of `row` (native endian) at sample index `i`.
#[inline(always)]
pub(crate) fn read_sample(row: &[u8], i: usize, bps: usize) -> u32 {
    match bps {
        1 => row[i] as u32,
        2 => u16::from_ne_bytes([row[2 * i], row[2 * i + 1]]) as u32,
        _ => u32::from_ne_bytes(row[4 * i..4 * i + 4].try_into().unwrap()),
    }
}

/// The coded value (in channel map `m`'s domain, masked to its bits) of a
/// reference container sample: what the inter predictors predict from.
/// `table`: the channel's value table (map mode 9), else empty.
#[inline(always)]
pub(crate) fn coded_ref(m: &ChannelMap, table: &[u32], sample: SampleType, raw: u32) -> u32 {
    let o = sample.order(raw);
    let s = if m.mode == MapMode::Table {
        crate::mapping::table_reduce(table, o)
    } else {
        m.reduce(o, sample.depth())
    };
    s & crate::mask_of(m.bits as u32)
}

/// [`encode_parts`] for an inter frame: `reference` (pixels and stride,
/// the image's size and format) is what the decoder's buffer holds when it
/// decodes this image (the previous frame); rows may predict from it
/// (predictors 7..=9). Samples up to 32 bits only (ignored for wider samples).
/// `standalone`: the head of a file's own payload, not a lossy payload's
/// nested image: it carries the unit table hint (two or more units, SPEC
/// 2.7), and a seeded one names its bank itself (presence bit 2 and a bank
/// byte; the lab's experimental standalone-seeds format) instead of a lossy
/// envelope doing it.
#[cfg(feature = "encode")]
pub(crate) fn encode_parts_ref(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    cfg: &EncodeConfig,
    reference: Option<(&[u8], usize)>,
    seeds: Option<(&'static crate::seeds::Bank, bool)>,
    standalone: bool,
) -> Result<Parts, Error> {
    encode_parts_ref_with_progress(
        pixels,
        stride,
        info,
        cfg,
        reference,
        seeds,
        standalone,
        &how_far::NoPulse,
        None,
        #[cfg(all(test, feature = "lab", feature = "std"))]
        None,
    )
}

#[allow(clippy::too_many_arguments)]
#[cfg(feature = "encode")]
fn encode_parts_ref_with_progress(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    cfg: &EncodeConfig,
    reference: Option<(&[u8], usize)>,
    seeds: Option<(&'static crate::seeds::Bank, bool)>,
    standalone: bool,
    pulse: &dyn Pulse,
    rectangles: Option<&[(usize, usize, usize)]>,
    #[cfg(all(test, feature = "lab", feature = "std"))] model_study: Option<
        &mut model_study::NativeStudy,
    >,
) -> Result<Parts, Error> {
    pulse.check().map_err(|_| Error::Cancelled)?;
    info.validate_precision()?;
    let effort = crate::effort::Effort::new(cfg.effort)?;
    let planned;
    let cfg = if cfg.decoder_memory_limit.is_some() {
        planned = crate::decoder_memory::plan_lossless(info, cfg)?.0;
        &planned
    } else {
        cfg
    };
    let reference = reference.filter(|_| info.sample.bytes() <= 4);
    let near_requested = cfg.max_error[..info.channels.count()]
        .iter()
        .any(|&d| d != 0);
    if near_requested && reference.is_some() {
        return Err(Error::InvalidInput(
            "near-lossless images cannot predict from a reference frame",
        ));
    }
    let lanes = cfg
        .lanes
        .unwrap_or_else(|| default_lanes(info.width))
        .clamp(1, 16);
    let hs = cfg
        .stripe_rows
        .unwrap_or_else(|| default_stripe_rows(info.width))
        .max(1);
    let ecfg = AnalysisConfig {
        stripe_rows: Some(hs),
        lanes,
        decor_mask: effort.decorrelations(cfg.decor_mask),
        predictor_mask: effort.predictors(cfg.predictor_mask),
        threads: cfg.threads,
        #[cfg(feature = "lab")]
        guide: cfg.guide.clone(),
        // Column groups and inter frames analyse their own units; the
        // extended tier still needs the full residuals for its context
        // shifts.
        maps_only: (group_cols(cfg, info.width as usize) != 0 || reference.is_some())
            && info.sample.bytes() < 4,
        max_error: cfg.max_error,
        float_shift: cfg.float_shift,
        value_tables: standalone && cfg.value_tables && value_tables_allowed(),
        ..Default::default()
    };
    Stages::new(
        pulse,
        &[
            PhaseSpec::new("pixel analysis", 1, Total::Unknown).units("mapped rows"),
            PhaseSpec::new("unit coding", 1, Total::Unknown).units("encoded units"),
        ],
    )
    .complete_with(|stages| {
        let a = stages.run(|p| encode::analyze_with_progress(pixels, stride, info, &ecfg, p))?;
        stages.run(|pulse| {
            let pulse = pulse.live();
            use how_far::Stop;
            pulse.check().map_err(|_| Error::Cancelled)?;
            let (w, h, lw) = (a.w, a.h, a.sw);
            let hs = hs as usize;
            let nstripes = h.div_ceil(hs);
            let layout = &a.layout;
            let ns = layout.slot_channel.len();
            // Inter frames: the reference's coded values per slot (w x h).
            let guide: Option<Vec<Vec<u32>>> = reference
                .map(|(rp, rs)| -> Result<Vec<Vec<u32>>, Error> {
                    let nch = info.channels.count();
                    let bps = info.sample.bytes();
                    layout
                        .slot_channel
                        .iter()
                        .map(|&c| {
                            let m = &layout.maps[c];
                            let mut v = Vec::with_capacity(w * h);
                            for y in 0..h {
                                pulse.check().map_err(|_| Error::Cancelled)?;
                                let row = &rp[y * rs..];
                                for x in 0..w {
                                    v.push(coded_ref(
                                        m,
                                        layout.table(c),
                                        info.sample,
                                        read_sample(row, x * nch + c, bps),
                                    ));
                                }
                            }
                            Ok(v)
                        })
                        .collect()
                })
                .transpose()?;
            // Column groups: units (stripe, group) in stripe-major order; without
            // groups one unit per stripe over the whole width.
            let gcols = group_cols(cfg, w);
            #[cfg(feature = "lab")]
            if gcols != 0 && cfg.guide.is_some() {
                return Err(Error::InvalidInput("guide planes with column groups"));
            }
            let ng = if gcols == 0 { 1 } else { w.div_ceil(gcols) };
            let units = rectangles.map_or(nstripes * ng, <[_]>::len);
            let bounds = |u: usize| {
                rectangles.map_or_else(
                    || (u / ng * hs, ((u / ng + 1) * hs).min(h), u % ng),
                    |v| v[u],
                )
            };
            // (first column, width, lane width) of group gi.
            let geo = |gi: usize| -> (usize, usize, usize) {
                if ng == 1 {
                    (0, w, lw)
                } else {
                    let x0 = gi * gcols;
                    let gw = gcols.min(w - x0);
                    (x0, gw, gw.div_ceil(lanes as usize))
                }
            };

            // Token schemes per slot.
            let mut tcs = Vec::with_capacity(ns);
            for s in 0..ns {
                let depth = layout.slot_bits[s];
                let mut k = if depth <= 8 { cfg.k_low } else { cfg.k_high };
                let m = cfg.m.min(4);
                while k > 0 && TokenCfg::v1(k, m, depth).alphabet() > 256 {
                    k -= 1;
                }
                let tc = TokenCfg::v1(k, m, depth);
                if tc.alphabet() > 256 {
                    return Err(Error::InvalidInput("token alphabet too large"));
                }
                tcs.push(tc);
            }
            let cross_of = |s: usize| cfg.ctx_cross && s >= 1 && s < layout.color_slots;
            // Extended tier: shift context sums so 32-bit residual magnitudes land
            // in the 16 context classes (median sum ~ class 8). Baseline: 0.
            let shifts: Vec<u32> = (0..ns)
                .map(|s| {
                    if layout.slot_bits[s] <= 16 {
                        return 0;
                    }
                    let mut hist = [0u64; 34];
                    for &u in a.res[s].iter().step_by(7) {
                        // Bit length of a typical context sum (~2u), at most 33.
                        hist[(64 - (2 * u as u64).leading_zeros()) as usize] += 1;
                    }
                    let total: u64 = hist.iter().sum();
                    let mut acc = 0;
                    let mut med = 0;
                    for (b, &c) in hist.iter().enumerate() {
                        acc += c;
                        if 2 * acc >= total {
                            med = b as u32;
                            break;
                        }
                    }
                    med.saturating_sub(8).min(31)
                })
                .collect();

            // Palette stripes: at most 256 colors, slot depths <= 16, and at most
            // four slots (the encoder packs a color into 64 bits).
            let pal_ok =
                cfg.palette && (2..=4).contains(&ns) && layout.slot_bits.iter().all(|&b| b <= 16);
            let pal_tc = {
                let m = cfg.m.min(4);
                let mut k = cfg.k_low;
                while k > 0 && TokenCfg::v1(k, m, 8).alphabet() > 256 {
                    k -= 1;
                }
                TokenCfg::v1(k, m, 8)
            };
            let pal_lay = pal_layout();
            let slot_planes: Vec<&[u32]> = layout
                .slot_channel
                .iter()
                .map(|&c| a.planes[c].as_slice())
                .collect();
            let entry_bytes: usize = layout
                .slot_bits
                .iter()
                .map(|&b| if b <= 8 { 1 } else { 2 })
                .sum();
            let pal_hist = |plane: &[u32], lw: usize, widths: &[u32; NL], rows: usize| {
                let mut hist = vec![vec![0u64; pal_tc.alphabet() as usize]; CTX_CLASSES];
                for l in 0..NL {
                    rans::lane_hist(
                        plane,
                        None,
                        lw,
                        rows,
                        widths[l] as usize,
                        l,
                        &pal_tc,
                        cfg.run_min,
                        0,
                        &mut hist,
                    );
                }
                hist
            };

            // Pass 1 (parallel over units): step-major planes + class histograms
            // (slot histograms empty and a palette histogram for palette stripes),
            // decorrelation mode and row predictors.
            let unit_ids: Vec<usize> = (0..units).collect();
            #[allow(clippy::type_complexity)]
            let pass1: Vec<(
                Vec<Vec<u32>>,
                Vec<Vec<Vec<u64>>>,
                Option<(PalStripe, Vec<Vec<u64>>)>,
                u8,
                Vec<u8>,
            )> = crate::par::par_map(unit_ids, cfg.threads, |_, u| -> Result<_, Error> {
                pulse.check().map_err(|_| Error::Cancelled)?;
                let si = u / ng;
                let (y0, y1, gi) = bounds(u);
                let rows = y1 - y0;
                let (x0, gw, lw) = geo(gi);
                let widths = lane_widths(gw, lw);
                // Slot planes and residuals of the unit: the full-image analysis
                // (rows y0..y1 of planes `pw` wide), or for column groups the
                // group's columns cropped and analysed here (rows 0..rows).
                let (local, decor, preds) = if rectangles.is_none() && ng == 1 && guide.is_none() {
                    (None, a.stripe_decor[si], a.row_pred[y0..y1].to_vec())
                } else {
                    let lp: Vec<Vec<u32>> = slot_planes
                        .iter()
                        .map(|p| {
                            (y0..y1)
                                .flat_map(|y| p[y * w + x0..y * w + x0 + gw].iter().copied())
                                .collect()
                        })
                        .collect();
                    let mut lr = vec![vec![0u32; gw * rows]; ns];
                    let lpr: Vec<&[u32]> = lp.iter().map(|v| v.as_slice()).collect();
                    let mut views: Vec<&mut [u32]> =
                        lr.iter_mut().map(|v| v.as_mut_slice()).collect();
                    let crop = |p: &[u32]| -> Vec<u32> {
                        (y0..y1)
                            .flat_map(|y| p[y * w + x0..y * w + x0 + gw].iter().copied())
                            .collect()
                    };
                    let gl: Option<Vec<Vec<u32>>> =
                        guide.as_ref().map(|g| g.iter().map(|p| crop(p)).collect());
                    let glr: Option<Vec<&[u32]>> = gl
                        .as_ref()
                        .map(|g| g.iter().map(|v| v.as_slice()).collect());
                    let (d, pr) = if layout.is_near() {
                        crate::near::analyze_stripe_near(
                            layout, &lpr, gw, lw, 0, rows, &ecfg, &mut views,
                        )
                    } else {
                        encode::analyze_stripe_fast(
                            layout,
                            &lpr,
                            gw,
                            lw,
                            0,
                            rows,
                            &ecfg,
                            &mut views,
                            glr.as_deref(),
                        )
                    };
                    (Some((lp, lr)), d, pr)
                };
                let (pw, py0) = if local.is_none() { (w, y0) } else { (gw, 0) };
                let planes_u: Vec<&[u32]> = match &local {
                    Some((lp, _)) => lp.iter().map(|v| v.as_slice()).collect(),
                    None => slot_planes.clone(),
                };
                let res_u: Vec<&[u32]> = match &local {
                    Some((_, lr)) => lr.iter().map(|v| v.as_slice()).collect(),
                    None => a.res.iter().map(|v| v.as_slice()).collect(),
                };
                let pal = if pal_ok {
                    stripe_palette(&planes_u, pw, py0, py0 + rows).and_then(|(entries, idx)| {
                        let mut res = vec![0u32; rows * gw];
                        let (_, preds) = encode::analyze_stripe_fast(
                            &pal_lay,
                            &[&idx],
                            gw,
                            lw,
                            0,
                            rows,
                            &ecfg,
                            &mut [&mut res],
                            None,
                        );
                        let normal: f64 = (0..ns)
                            .map(|s| est_bits(&res_u[s][py0 * pw..(py0 + rows) * pw]))
                            .sum();
                        let pal = est_bits(&res) + 8.0 * (entries.len() * entry_bytes + 1) as f64;
                        (pal < normal).then(|| PalStripe {
                            entries,
                            preds,
                            plane: step_major(&res, gw, lw, 0, rows),
                        })
                    })
                } else {
                    None
                };
                if let Some(ps) = pal {
                    let hist = pal_hist(&ps.plane, lw, &widths, rows);
                    pulse.check().map_err(|_| Error::Cancelled)?;
                    return Ok((Vec::new(), Vec::new(), Some((ps, hist)), decor, preds));
                }
                let pl: Vec<Vec<u32>> = (0..ns)
                    .map(|s| step_major(res_u[s], pw, lw, py0, py0 + rows))
                    .collect();
                let mut hists = Vec::with_capacity(ns);
                for s in 0..ns {
                    let asz = tcs[s].alphabet() as usize;
                    let mut hist = vec![vec![0u64; asz]; CTX_CLASSES];
                    let cr = if cross_of(s) {
                        Some(pl[0].as_slice())
                    } else {
                        None
                    };
                    for l in 0..NL {
                        rans::lane_hist(
                            &pl[s],
                            cr,
                            lw,
                            rows,
                            widths[l] as usize,
                            l,
                            &tcs[s],
                            cfg.run_min,
                            shifts[s],
                            &mut hist,
                        );
                    }
                    hists.push(hist);
                }
                pulse.check().map_err(|_| Error::Cancelled)?;
                Ok((pl, hists, None, decor, preds))
            })
            .into_iter()
            .collect::<Result<_, _>>()?;

            #[cfg(all(test, feature = "lab", feature = "std"))]
            let study_counts = model_study.as_ref().map(|_| {
                pass1
                    .iter()
                    .map(|p| model_study::UnitCounts {
                        slots: p.1.clone(),
                        palette: p.2.as_ref().map(|p| p.1.clone()),
                    })
                    .collect::<Vec<_>>()
            });
            // Models. With a seed bank, each also returns its seed candidates:
            // (context, seed, estimated bits saved).
            let bank = seeds.map(|s| s.0);
            let mut sets: Vec<([u8; 3], crate::seeds::KindSet)> = Vec::new();
            if let Some(bank) = bank {
                for tc in tcs.iter().chain([&pal_tc]) {
                    let key = [tc.k as u8, tc.m as u8, tc.depth as u8];
                    if sets.iter().all(|s| s.0 != key) {
                        let alpha = tc.alphabet() as usize;
                        let kind = crate::seeds::Kind::Lossless;
                        if let Some(set) = crate::seeds::kind_set(bank, kind, key, alpha) {
                            sets.push((key, set));
                        }
                    }
                }
            }
            type Cand = (usize, u8, f64, crate::seeds::Expanded);
            let build = |tc: TokenCfg,
                         hist: &[Vec<u64>],
                         cross: bool,
                         ctx_shift: u32|
             -> (Model, Vec<Cand>) {
                let asz = tc.alphabet() as usize;
                let (map18, groups) = choose_ctx_map(hist, cfg.max_ctx.clamp(1, NCTX), 12);
                let mut ctx_map = [0u8; CLASSES];
                ctx_map.copy_from_slice(&map18[..CLASSES]);
                let key = [tc.k as u8, tc.m as u8, tc.depth as u8];
                let set = sets.iter().find(|s| s.0 == key).map(|s| &s.1);
                let mut cand = Vec::new();
                let mut freq = vec![vec![0u32; asz]; NCTX];
                let mut weights = vec![vec![0u32; asz]; NCTX];
                let mut prec = vec![1u32; NCTX];
                for c in 0..NCTX {
                    let g = groups.get(c);
                    if g.is_none_or(|g| g.iter().all(|&v| v == 0)) {
                        freq[c][0] = rans::M;
                        weights[c][0] = 1;
                        continue;
                    }
                    let (mut wt, mut f, p) = rans::choose_table(g.unwrap());
                    rans::enforce_run_cap(&tc, &mut wt);
                    if let Some(f2) = rans::weights_to_freq(&wt) {
                        f = f2;
                    }
                    if c <= ctx_map.iter().copied().max().unwrap_or(0) as usize {
                        crate::seeds::record_table(
                            crate::seeds::Kind::Lossless,
                            key,
                            g.unwrap(),
                            || {
                                (
                                    rans::weights_cost(&wt, p),
                                    crate::seeds::rans_data_bits(g.unwrap(), &f).unwrap_or(0.0),
                                )
                            },
                        );
                        if let Some(set) = set {
                            let explicit = rans::weights_cost(&wt, p) as f64
                                + crate::seeds::rans_data_bits(g.unwrap(), &f).unwrap_or(f64::MAX);
                            if let Some((i, bits)) = set.best(g.unwrap()) {
                                let seeded = bits + set.seeds.index_bits() as f64;
                                if seeded < explicit {
                                    cand.push((c, i as u8, explicit - seeded, set.table(i)));
                                }
                            }
                        }
                    }
                    freq[c] = f;
                    weights[c] = wt;
                    prec[c] = p;
                }
                let model = Model {
                    tc,
                    ctx_map,
                    freq,
                    weights,
                    prec,
                    cross,
                    ctx_shift,
                    seeds: [None; NCTX],
                };
                (model, cand)
            };
            let sum_hists = |asz: usize, it: &mut dyn Iterator<Item = &Vec<Vec<u64>>>| {
                let mut hist = vec![vec![0u64; asz]; CTX_CLASSES];
                for hs_ in it {
                    for (c, row) in hs_.iter().enumerate() {
                        for (t, &v) in row.iter().enumerate() {
                            hist[c][t] += v;
                        }
                    }
                }
                hist
            };
            let mut models: Vec<(Model, Vec<Cand>)> = Vec::with_capacity(ns);
            for s in 0..ns {
                pulse.check().map_err(|_| Error::Cancelled)?;
                let tc = tcs[s];
                let hist = sum_hists(
                    tc.alphabet() as usize,
                    &mut pass1.iter().filter(|p| p.2.is_none()).map(|p| &p.1[s]),
                );
                models.push(build(tc, &hist, cross_of(s), shifts[s]));
            }
            if pass1.iter().any(|p| p.2.is_some()) {
                let hist = sum_hists(
                    pal_tc.alphabet() as usize,
                    &mut pass1.iter().filter_map(|p| p.2.as_ref().map(|x| &x.1)),
                );
                models.push(build(pal_tc, &hist, false, 0));
            }
            // Seeds replace tables only when the head saves enough overall
            // (selector bits included).
            let tables: usize = models
                .iter()
                .map(|(m, _)| m.ctx_map.iter().copied().max().unwrap_or(0) as usize + 1)
                .sum();
            let gain: f64 = models
                .iter()
                .flat_map(|(_, c)| c.iter().map(|x| x.2))
                .sum::<f64>()
                - tables as f64;
            let seeded = bank.filter(|_| gain >= crate::seeds::MIN_GAIN_BITS);
            // Explicit models, and the seeded ones when seeds are used.
            let explicit: Vec<Model> = models.iter().map(|(m, _)| m.clone()).collect();
            let seeded_models: Option<Vec<Model>> = seeded.map(|b| {
                models
                    .into_iter()
                    .map(|(mut m, cand)| {
                        // Candidates exist only for token schemes the bank has.
                        let _ = b;
                        for (c, i, g, e) in cand {
                            crate::seeds::record_seed(
                                crate::seeds::Kind::Lossless,
                                m.seed_key(),
                                i,
                                g,
                            );
                            m.freq[c] = e.freq;
                            m.weights[c] = e.weights;
                            m.prec[c] = e.prec;
                            m.seeds[c] = Some(i);
                        }
                        m
                    })
                    .collect()
            });

            // Near-lossless bounds only matter when a unit codes slots: palette
            // stripes are exact, so an image of palette stripes is written as plain
            // lossless (version 1, no Scientific profile).
            let near_used = layout.is_near() && pass1.iter().any(|p| p.2.is_none());

            // Pass 2 (parallel over stripes): code streams.
            #[allow(clippy::type_complexity)]
            let planes: Vec<(Vec<Vec<u32>>, Option<PalStripe>, u8, Vec<u8>)> = pass1
                .into_iter()
                .map(|p| (p.0, p.2.map(|x| x.0), p.3, p.4))
                .collect();
            // Stripes and payload head coded with `all` (slot models, then the
            // palette model when present), tables behind selectors with `bank`.
            // Bilevel stripes (SPEC 2.10) for a lone 1-bit slot, and bilevel palette
            // stripes for two colors: `code` keeps each unit that is smaller when the
            // units' gain pays for the table.
            let key = |i: usize| slot_planes.iter().fold(0u64, |k, p| k << 16 | p[i] as u64);
            let two = || {
                let (mut c, mut n) = ([key(0), 0], [0usize; 2]);
                for i in 0..w * h {
                    let k = key(i);
                    let j = (k != c[0]) as usize;
                    if j == 1 && n[1] > 0 && k != c[1] {
                        return None;
                    }
                    (c[j], n[j]) = (k, n[j] + 1);
                }
                if n[1] > n[0] {
                    c.swap(0, 1);
                }
                // Stripe head: layout byte, two entries (SPEC 2.5), then the stream.
                let mut head = vec![crate::bilevel::PAL, 1];
                for (e, s) in c.iter().flat_map(|&e| (0..ns).map(move |s| (e, s))) {
                    let v = e >> (16 * (ns - 1 - s)) & 0xFFFF;
                    match layout.slot_bits[s] <= 8 {
                        true => head.push(v as u8),
                        false => head.extend_from_slice(&(v as u16).to_le_bytes()),
                    }
                }
                let plane = (0..w * h).map(|i| (key(i) != c[0]) as u32).collect();
                (n[1] > 0).then_some((alloc::borrow::Cow::Owned(plane), head))
            };
            pulse.check().map_err(|_| Error::Cancelled)?;
            let unit = |u: usize| {
                let (y0, y1, gi) = bounds(u);
                let (x0, gw, lw) = geo(gi);
                (y0, y1 - y0, x0, gw, lw)
            };
            let bilevel = standalone
                .then(|| match layout.slot_bits[..] == [1] {
                    true => Some((alloc::borrow::Cow::Borrowed(slot_planes[0]), Vec::new())),
                    false => pal_ok.then(two).flatten(),
                })
                .flatten()
                .map(|(plane, head)| {
                    let units = (0..units).map(unit).collect();
                    let (rec, mut st) = crate::bilevel::encode(&plane, w, units, cfg.threads);
                    for s in st.iter_mut().filter(|_| !head.is_empty()) {
                        s.splice(..1, head.iter().copied());
                    }
                    (rec, st)
                });
            pulse.check().map_err(|_| Error::Cancelled)?;
            // Template palette stripes (SPEC 2.14) for units of at most 256
            // slot tuples, with tables shared by those units: `code` keeps
            // each that is smaller when the units' gain pays for the tables.
            let template = (standalone
                && cfg.palette
                && !layout.is_near()
                && ns <= 4
                && layout.slot_bits.iter().all(|&b| b <= 16))
            .then(|| {
                // The image's tuples when there are at most 256 (the shared
                // entries), and each unit's indices: into the shared entries,
                // or its own when it has at most two of a larger shared set
                // (the two-entry rule) or there are no shared entries.
                let global = crate::template::unit_palette(&slot_planes, w, &(0, h, 0, w, 0));
                type Indices = (Option<Vec<u64>>, Vec<u8>, usize);
                let pals: Vec<Option<Indices>> =
                    crate::par::par_map((0..units).collect(), cfg.threads, |_, u: usize| {
                        let un = unit(u);
                        let Some((ge, gi)) = &global else {
                            let (e, idx) = crate::template::unit_palette(&slot_planes, w, &un)?;
                            let n = e.len();
                            return Some((Some(e), idx, n));
                        };
                        let (y0, rows, x0, gw, _) = un;
                        let mut idx: Vec<u8> = (y0..y0 + rows)
                            .flat_map(|y| gi[y * w + x0..y * w + x0 + gw].iter().copied())
                            .collect();
                        let own = crate::template::local_entries(ge, &mut idx);
                        Some(match own {
                            Some(e) => {
                                let n = e.len();
                                (Some(e), idx, n)
                            }
                            None => (None, idx, ge.len()),
                        })
                    });
                let inputs: Vec<crate::template::UnitIndices<'_>> = pals
                    .iter()
                    .enumerate()
                    .filter_map(|(u, p)| {
                        p.as_ref().map(|(_, idx, n)| crate::template::UnitIndices {
                            unit: unit(u),
                            idx,
                            n: *n,
                        })
                    })
                    .collect();
                if inputs.is_empty() {
                    return None;
                }
                let shared = global
                    .as_ref()
                    .filter(|_| pals.iter().flatten().any(|p| p.0.is_none()))
                    .map(|(e, _)| (e.as_slice(), layout.slot_bits.as_slice()));
                let (rec, streams) = crate::template::encode(&inputs, shared, cfg.threads);
                let mut streams = streams.into_iter();
                let units: Vec<Option<Vec<u8>>> = pals
                    .iter()
                    .map(|p| {
                        let (own, _, _) = p.as_ref()?;
                        // Layout byte, own entries (SPEC 2.5), rows and stream.
                        let mut sb = vec![crate::template::SHARED];
                        if let Some(entries) = own {
                            sb = vec![crate::template::STRIPE, (entries.len() - 1) as u8];
                            for &k in entries {
                                for s in 0..ns {
                                    let v = (k >> (16 * (ns - 1 - s))) & 0xFFFF;
                                    if layout.slot_bits[s] <= 8 {
                                        sb.push(v as u8);
                                    } else {
                                        sb.extend_from_slice(&(v as u16).to_le_bytes());
                                    }
                                }
                            }
                        }
                        sb.extend_from_slice(&streams.next().unwrap());
                        Some(sb)
                    })
                    .collect();
                Some((rec, units))
            })
            .flatten();
            pulse.check().map_err(|_| Error::Cancelled)?;
            let code =
                |all: &[Model],
                 bank: Option<&'static crate::seeds::Bank>,
                 #[cfg(all(test, feature = "lab", feature = "std"))] only_unit: Option<usize>|
                 -> Result<_, Error> {
                    pulse.check().map_err(|_| Error::Cancelled)?;
                    let (models, pal_model) = all.split_at(ns.min(all.len()));
                    let pal_model = pal_model.first();
                    let encode_tables: Vec<_> = models
                        .iter()
                        .map(|m| rans::EncodeTables::new(&m.freq, &m.ctx_map))
                        .collect();
                    let palette_tables = pal_model
                        .as_ref()
                        .map(|m| rans::EncodeTables::new(&m.freq, &m.ctx_map));

                    let mref = models;
                    let pmref = &pal_model;
                    let mut stripe_bytes: Vec<Vec<u8>> = crate::par::par_map(
                        planes.iter().collect(),
                        cfg.threads,
                        |u, p| -> Result<_, Error> {
                            pulse.check().map_err(|_| Error::Cancelled)?;
                            #[cfg(all(test, feature = "lab", feature = "std"))]
                            if only_unit.is_some_and(|selected| selected != u) {
                                return Ok(Vec::new());
                            }
                            let (pl, pal, decor, preds) = p;
                            let (y0, y1, gi) = bounds(u);
                            let rows = y1 - y0;
                            let (_, gw, lw) = geo(gi);
                            let widths = lane_widths(gw, lw);
                            let mut sb = Vec::new();
                            if let Some(ps) = pal {
                                let md = pmref.as_ref().unwrap();
                                sb.push(PAL_STRIPE);
                                sb.push((ps.entries.len() - 1) as u8);
                                for &k in &ps.entries {
                                    for s in 0..ns {
                                        let v = (k >> (16 * (ns - 1 - s))) & 0xFFFF;
                                        if layout.slot_bits[s] <= 8 {
                                            sb.push(v as u8);
                                        } else {
                                            sb.extend_from_slice(&(v as u16).to_le_bytes());
                                        }
                                    }
                                }
                                for pair in ps.preds.chunks(2) {
                                    sb.push(pair[0] | (pair.get(1).copied().unwrap_or(0) << 4));
                                }
                                let lanes_syms: [Vec<Sym>; NL] = core::array::from_fn(|l| {
                                    let mut v = Vec::new();
                                    rans::lane_symbols(
                                        &ps.plane,
                                        None,
                                        lw,
                                        rows,
                                        widths[l] as usize,
                                        l,
                                        &md.tc,
                                        cfg.run_min,
                                        0,
                                        &mut v,
                                    );
                                    v
                                });
                                let mut st = Vec::new();
                                rans::encode_lanes_tables(
                                    palette_tables.as_ref().unwrap(),
                                    &lanes_syms,
                                    active_lanes(&widths),
                                    &mut st,
                                );
                                put_varint(&mut sb, st.len() as u64);
                                sb.extend_from_slice(&st);
                                pulse.step(1).map_err(|_| Error::Cancelled)?;
                                return Ok(sb);
                            }
                            sb.push(*decor);
                            for pair in preds.chunks(2) {
                                sb.push(pair[0] | (pair.get(1).copied().unwrap_or(0) << 4));
                            }
                            let mut streams = Vec::with_capacity(ns);
                            for s in 0..ns {
                                pulse.check().map_err(|_| Error::Cancelled)?;
                                let md = &mref[s];
                                let cr = if md.cross {
                                    Some(pl[0].as_slice())
                                } else {
                                    None
                                };
                                let lanes_syms: [Vec<Sym>; NL] = core::array::from_fn(|l| {
                                    let mut v = Vec::new();
                                    rans::lane_symbols(
                                        &pl[s],
                                        cr,
                                        lw,
                                        rows,
                                        widths[l] as usize,
                                        l,
                                        &md.tc,
                                        cfg.run_min,
                                        md.ctx_shift,
                                        &mut v,
                                    );
                                    v
                                });
                                let mut st = Vec::new();
                                rans::encode_lanes_tables(
                                    &encode_tables[s],
                                    &lanes_syms,
                                    active_lanes(&widths),
                                    &mut st,
                                );
                                streams.push(st);
                            }
                            for st in &streams {
                                put_varint(&mut sb, st.len() as u64);
                            }
                            for st in &streams {
                                sb.extend_from_slice(st);
                            }
                            pulse.step(1).map_err(|_| Error::Cancelled)?;
                            Ok(sb)
                        },
                    )
                    .into_iter()
                    .collect::<Result<_, _>>()?;

                    let (use_bl, use_tp) =
                        choose_tables(&mut stripe_bytes, bilevel.as_ref(), template.as_ref());
                    let bl = bilevel.as_ref().filter(|_| use_bl);
                    let tp = template.as_ref().filter(|_| use_tp);
                    // Payload head (SPEC 2): presence, geometry, channel maps,
                    // models, then the head records: the unit table hint (a file's own
                    // payload of two or more units), the near-lossless bounds, the
                    // bilevel table and the template tables.
                    let head_bank = bank.filter(|_| standalone);
                    let ordinals = standalone && cfg.unit_ordinals;
                    let mut records = Vec::new();
                    if standalone {
                        let spans: Vec<u64> = stripe_bytes
                            .iter()
                            .enumerate()
                            .map(|(i, sb)| {
                                main_framed_len(R_STRIPE, ordinals.then_some(i as u64), sb.len())
                            })
                            .collect();
                        put_unit_table(&mut records, stripe_bytes.len(), &spans);
                    }
                    if near_used {
                        let mut bounds = Vec::new();
                        for d in layout.channel_near() {
                            put_varint(&mut bounds, d as u64);
                        }
                        put_record(&mut records, H_NEAR, &bounds);
                    }
                    if let Some((rec, _)) = bl {
                        put_record(&mut records, crate::bilevel::RECORD, rec);
                    }
                    if let Some((rec, _)) = tp {
                        put_record(&mut records, crate::template::RECORD, rec);
                    }
                    if ordinals {
                        put_record(&mut records, H_UNIT_ORDINALS, &[]);
                    }
                    let mut data = Vec::new();
                    let present = [
                        (pal_model.is_some(), H_PALETTE),
                        (!records.is_empty(), H_RECORDS),
                        (head_bank.is_some(), H_LAB_BANK),
                    ]
                    .into_iter()
                    .filter(|f| f.0)
                    .fold(0, |m, f| m | f.1);
                    put_varint(&mut data, present);
                    if let Some(b) = head_bank {
                        put_varint(&mut data, b.id);
                    }
                    data.push(lanes as u8);
                    put_varint(&mut data, hs as u64);
                    put_varint(&mut data, gcols as u64);
                    let extended = info.sample.bytes() >= 4;
                    for (c, m) in layout.maps.iter().enumerate() {
                        let table = layout.table(c);
                        data.push(m.mode as u8);
                        data.push(m.bits);
                        if extended {
                            data.extend_from_slice(&m.value.to_le_bytes());
                        } else {
                            data.extend_from_slice(&(m.value as u16).to_le_bytes());
                        }
                        put_value_table(&mut data, table);
                    }
                    let mut bw = BitWriter::new();
                    for md in models.iter().chain(pal_model) {
                        md.write(&mut bw, bank);
                    }
                    crate::seeds::record_group(255, || {
                        let (mut table_bits, mut tables) = (0, 0);
                        for md in models.iter().chain(pal_model) {
                            let used = md.ctx_map.iter().copied().max().unwrap_or(0) as usize + 1;
                            for c in 0..used {
                                table_bits += rans::weights_cost(&md.weights[c], md.prec[c]);
                                tables += 1;
                            }
                        }
                        (bw.bit_len(), table_bits, tables)
                    });
                    let mb = bw.finish();
                    put_varint(&mut data, mb.len() as u64);
                    data.extend_from_slice(&mb);
                    if !records.is_empty() {
                        put_varint(&mut data, records.len() as u64);
                        data.extend_from_slice(&records);
                    }
                    pulse.check().map_err(|_| Error::Cancelled)?;
                    Ok((data, stripe_bytes))
                };
            #[cfg(all(test, feature = "lab", feature = "std"))]
            if let Some(study) = model_study {
                study.collect(
                    study_counts.as_ref().expect("requested counts"),
                    &explicit,
                    ns,
                    bank,
                    |slot, hist| {
                        let tc = if slot < ns { tcs[slot] } else { pal_tc };
                        build(
                            tc,
                            hist,
                            slot < ns && cross_of(slot),
                            if slot < ns { shifts[slot] } else { 0 },
                        )
                    },
                    code,
                )?;
                if let Some(models) = seeded_models.as_ref() {
                    study.definitions.push(model_study::Definition::new(
                        model_study::Training::Global,
                        seeded.expect("seeded models").id,
                        code(models, seeded, None)?,
                    ));
                }
            }
            Ok(match seeded_models {
                Some(m) => {
                    let (head, stripes) = code(
                        &m,
                        seeded,
                        #[cfg(all(test, feature = "lab", feature = "std"))]
                        None,
                    )?;
                    let exact = seeds.is_none_or(|s| s.1);
                    (
                        head,
                        stripes,
                        exact
                            .then(|| {
                                code(
                                    &explicit,
                                    None,
                                    #[cfg(all(test, feature = "lab", feature = "std"))]
                                    None,
                                )
                            })
                            .transpose()?,
                    )
                }
                None => {
                    let (head, stripes) = code(
                        &explicit,
                        None,
                        #[cfg(all(test, feature = "lab", feature = "std"))]
                        None,
                    )?;
                    (head, stripes, None)
                }
            })
        })
    })
}

/// Encodes like [`encode`], plus preview layers (e.g. a 50% Mitchell
/// downscale made by the caller). Each preview is a complete imij stream in a
/// `PRVW` chunk placed before `DATA`, so it can be read from a prefix of the
/// file and decoded without touching the full image. Decoders that do not
/// care skip it.
#[cfg(feature = "encode")]
pub fn encode_with_previews(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &EncodeConfig,
    previews: &[crate::ImageRef<'_>],
) -> Result<Vec<u8>, Error> {
    let main = encode_frame(pixels, stride, info, meta, cfg, None)?;
    let mut nested = Vec::with_capacity(previews.len());
    // Nested files carry no DATA locator (their readers have the bytes).
    for p in previews {
        nested.push(encode_frame(
            p.pixels,
            p.stride,
            p.info,
            &Metadata::default(),
            cfg,
            None,
        )?);
    }
    // pRVW chunks go in front of DATA; the checksum, if any, covers them.
    let chunks: Vec<([u8; 4], &[u8])> = nested
        .iter()
        .map(|n| (crate::parse::chunks::PRVW, &n[..]))
        .collect();
    let out = crate::container::insert_before_data(&main, &chunks)?;
    crate::checksum::finish_file(out, cfg.checksum.then_some(cfg.hash))
}

/// Embedded preview streams, in file order. Each is a complete IMI file:
/// decode with [`decode`] / [`StreamDecoder`]. Only the bytes before the
/// main `DATA` chunk are read, and `data` may be just that prefix.
pub fn previews(data: &[u8]) -> Result<Vec<&[u8]>, Error> {
    Ok(crate::container::parse(data, true, &payload_len)?.previews)
}

// ---- decoding -------------------------------------------------------------------

pub(crate) struct Parsed<'a> {
    pub quick: bool,
    pub unit_rectangles: Option<alloc::sync::Arc<Vec<(usize, usize, usize)>>>,
    pub quick_bits: bool,
    pub info: ImageInfo,
    pub layout: Layout,
    pub lw: usize,
    pub hs: usize,
    /// Lanes per stripe (per column group).
    pub lanes: usize,
    /// Column group width (0 = no groups).
    pub gcols: usize,
    /// The unit table hint (head record 10, or a filled reservation, SPEC
    /// 2.7): the bytes of each unit's record span, when the head carries
    /// one. A reader that walks the records checks them against it.
    pub hint: Option<Vec<u64>>,
    /// The unit table reservation (head record 12), if any.
    pub reserve: Option<crate::parse::records::Reserve>,
    /// Whether main unit records carry 0-based ordinals (head record 37, SPEC 2.7).
    pub unit_ordinals: bool,
    /// Inter frame: the output buffer holds the previous frame, and rows may
    /// predict from it (predictors 7..=9). Set by the caller, not the file.
    pub inter: bool,
    pub models: Vec<DecModel>,
    /// Palette stripes' index channel (when the file has any).
    pub pal: Option<Pal>,
    /// Bilevel stripes' table (head record 25, SPEC 2.10).
    pub bilevel: Option<alloc::boxed::Box<crate::bilevel::Table>>,
    /// Template palette stripes' tables (head record 35, SPEC 2.14).
    pub template: Option<alloc::boxed::Box<crate::template::Table>>,
    /// Records of the units (stripe, column group), stripe-major.
    pub stripes: Vec<&'a [u8]>,
    /// Length field and body of head records, relative to this head's start.
    /// Rewriters use the actual parser boundary rather than another grammar walk.
    pub head_records_span: Option<core::ops::Range<usize>>,
    /// LAB: display names of the slots, resolved while the head is parsed
    /// (nested lossy images name their planes only then).
    #[cfg(all(feature = "lab", feature = "std"))]
    pub slot_names: Vec<alloc::string::String>,
}

/// The index channel of palette stripes: its model, a one-slot 8-bit
/// layout, and an output map for that layout (scalar reconstruction).
pub(crate) struct Pal {
    pub model: DecModel,
    pub layout: Layout,
    pub om: OutMap,
}

/// The one-slot layout of a palette index channel.
pub(crate) fn pal_layout() -> Layout {
    Layout::new(
        crate::Channels::Gray,
        vec![ChannelMap {
            mode: MapMode::Lsb,
            bits: 8,
            value: 0,
        }],
    )
}

/// Decorrelation byte of a palette stripe.
pub(crate) const PAL_STRIPE: u8 = 0x80;

/// How a writer stores a payload's unit table (SPEC 2.7). The reference
/// writers write the hint; tests and the lab choose another form per thread
/// (`lab::set_unit_table_form`), as the writers that cannot hold a payload
/// whole would write it. Widths are bytes per reserved entry, 1 to 8.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(not(all(test, feature = "encode")), allow(dead_code))]
pub enum UnitTableForm {
    /// Head record 10 for payloads of two or more units.
    #[default]
    Hint,
    /// No unit table: readers walk the records.
    None,
    /// A writer that cannot seek: head record 12 says check tail (3 bytes)
    /// and the spans follow in the trailing `uIDX` chunk.
    Trailing,
    /// A seekable writer: head record 12 reserved at this width and filled
    /// in place after the records (check tail and a trailing index when a
    /// span does not fit).
    Reserved(u8),
    /// Head record 12 reserved at this width and left empty.
    ReservedEmpty(u8),
    /// Head record 12 reserved at this width, then marked check tail, with
    /// the spans in a trailing `uIDX` chunk.
    ReservedTail(u8),
}

#[cfg(any(
    all(all(test, feature = "encode"), feature = "std"),
    all(feature = "lab", feature = "std")
))]
std::thread_local! {
    /// Tests and LAB: this thread's unit table form.
    pub(crate) static UNIT_TABLE: core::cell::Cell<UnitTableForm> =
        const { core::cell::Cell::new(UnitTableForm::Hint) };
}

/// Tests: runs `f` with this thread writing payloads without the unit table
/// hint (readers then walk the records).
#[cfg(all(test, feature = "std", feature = "encode"))]
pub(crate) fn without_hint<T>(f: impl FnOnce() -> T) -> T {
    with_unit_table(UnitTableForm::None, f)
}

/// Tests: runs `f` with this thread writing the unit table as `form`.
#[cfg(all(test, feature = "std", feature = "encode"))]
pub(crate) fn with_unit_table<T>(form: UnitTableForm, f: impl FnOnce() -> T) -> T {
    let old = UNIT_TABLE.with(|c| c.replace(form));
    let r = f();
    UNIT_TABLE.with(|c| c.set(old));
    r
}

/// This thread's unit table form: the hint, unless a test or the lab chose
/// another.
#[cfg(feature = "encode")]
pub(crate) fn unit_table_form() -> UnitTableForm {
    #[cfg(any(
        all(all(test, feature = "encode"), feature = "std"),
        all(feature = "lab", feature = "std")
    ))]
    return UNIT_TABLE.with(|c| c.get());
    #[allow(unreachable_code)]
    UnitTableForm::Hint
}

/// The unit table head record of a payload whose main record spans are
/// `spans` (SPEC 2.7), in this thread's form: head record 10 for two or more
/// units (`units`), or head record 12 (check tail, a reservation that
/// starts as check tail and [`crate::container::finish_unit_table`] fills,
/// or an empty one).
#[cfg(feature = "encode")]
pub(crate) fn put_unit_table(records: &mut Vec<u8>, units: usize, spans: &[u64]) {
    use crate::parse::records::{H_UNIT_RESERVE, RESERVE_TAIL};
    match unit_table_form() {
        UnitTableForm::Hint if units >= 2 => {
            let mut hint = Vec::with_capacity(3 * spans.len());
            for &e in spans {
                put_varint(&mut hint, e);
            }
            put_record(records, H_UNIT_TABLE, &hint);
        }
        UnitTableForm::Hint | UnitTableForm::None => {}
        UnitTableForm::Trailing => put_record(records, H_UNIT_RESERVE, &[RESERVE_TAIL]),
        UnitTableForm::Reserved(w)
        | UnitTableForm::ReservedEmpty(w)
        | UnitTableForm::ReservedTail(w) => {
            let mut rec = vec![0; 2 + spans.len() * w as usize];
            if !matches!(unit_table_form(), UnitTableForm::ReservedEmpty(_)) {
                rec[0] = RESERVE_TAIL;
            }
            put_record(records, H_UNIT_RESERVE, &rec);
        }
    }
}

/// Length of an open `DATA` payload (it delimits itself) in a file whose
/// header declares the features `d`.
pub(crate) fn payload_len(
    h: &crate::Header,
    d: crate::parse::header::Features,
    body: &[u8],
) -> Result<usize, Error> {
    payload_len_checked(h, d, body, &crate::parse::limits::Unrestricted)
}

/// [`payload_len`] under an admission policy.
pub(crate) fn payload_len_checked(
    h: &crate::Header,
    d: crate::parse::header::Features,
    body: &[u8],
    admission: &impl crate::parse::limits::Admission,
) -> Result<usize, Error> {
    if crate::parse::payload::native_selected(h.lossy_color, d, body)? {
        #[cfg(not(feature = "native-jpeg"))]
        {
            return Err(Error::Unsupported("feature native_components"));
        }
        #[cfg(feature = "native-jpeg")]
        {
            return Ok(
                crate::jpeg::wire::parse(h.info.clone(), h.version, body, admission)?.payload_len,
            );
        }
    }
    match h.coding {
        crate::Coding::Lossless => {
            Ok(parse_body_checked(h.info.clone(), h.version, d, body, admission)?.1)
        }
        crate::Coding::Lossy => {
            let color = h
                .lossy_color
                .ok_or(Error::Corrupt("lossy coding without a color"))?;
            crate::lossy::payload_len_checked(&h.info, h.version, color, d, body, admission)
        }
    }
}

/// Bytes from the start of a `DATA` payload to the end of its head (its
/// first unit record: what the DATA locator's `prefix` counts, SPEC 1.0)
/// when the head carries the unit table hint or a unit table reservation
/// (head records 10 and 12, SPEC 2.7), else `None`.
pub(crate) fn head_end_with_hint(
    h: &crate::Header,
    d: crate::parse::header::Features,
    body: &[u8],
) -> Result<Option<usize>, Error> {
    if h.version == 4 {
        return Ok(None);
    }
    let mut r = ByteReader::new(body);
    let mut head =
        crate::parse::payload::read_head(h, d, &mut r, &crate::parse::limits::Structure)?;
    let table = head.reserve().is_some() || head.take_hint().is_some();
    Ok(table.then_some(r.pos))
}

/// Header, metadata and `DATA` payload of a file.
pub(crate) fn parse_header(data: &[u8]) -> Result<(ImageInfo, Metadata, &[u8]), Error> {
    let (info, meta, body, _) = parse_header_lossy_checked(
        data,
        &crate::parse::limits::Unrestricted,
        crate::checksum::Verify::Now(1),
    )?;
    Ok((info, meta, body))
}

/// [`parse_header`] and the header's declared features, which the payload
/// parsers check gated syntax against.
#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
pub(crate) fn parse_header_lossy(
    data: &[u8],
) -> Result<(ImageInfo, Metadata, &[u8], crate::parse::header::Features), Error> {
    parse_header_lossy_checked(
        data,
        &crate::parse::limits::Unrestricted,
        crate::checksum::Verify::Now(1),
    )
}

pub(crate) fn parse_header_lossy_checked<'a>(
    data: &'a [u8],
    admission: &impl crate::parse::limits::Admission,
    verify: crate::checksum::Verify,
) -> Result<
    (
        ImageInfo,
        Metadata,
        &'a [u8],
        crate::parse::header::Features,
    ),
    Error,
> {
    // Inspection can report the metadata of files requiring a wider decoder.
    let version = crate::parse::header::header_version(data)?;
    let f = if !cfg!(feature = "v3") || version == 4 {
        crate::v4::admitted::parse_structure(
            data,
            admission,
            verify != crate::checksum::Verify::Off,
        )?
    } else {
        parse_container(data, admission, verify)?
    };
    let meta = f.metadata()?;
    Ok((f.header.info, meta, f.data, f.declared))
}

/// The container of a whole file to decode: [`parse_container`] after
/// [`crate::parse::header::check_decodable`] on its header.
pub(crate) fn parse_file<'a>(
    data: &'a [u8],
    admission: &impl crate::parse::limits::Admission,
    verify: crate::checksum::Verify,
) -> Result<crate::container::File<'a>, Error> {
    let hp = crate::parse::header::read_header_sized(&mut ByteReader::new(data))?;
    crate::parse::header::check_decodable(&hp.header, hp.declared)?;
    if hp.header.version == 4 {
        return Err(Error::Unsupported(
            "V4 requires its declared payload driver",
        ));
    }
    parse_container(data, admission, verify)
}

/// The container of a whole file, checked for a `DATA` coding that matches
/// the header; `verify` as in [`crate::container::parse_checked`].
fn parse_container<'a>(
    data: &'a [u8],
    admission: &impl crate::parse::limits::Admission,
    verify: crate::checksum::Verify,
) -> Result<crate::container::File<'a>, Error> {
    let open_length = |h: &crate::Header, d: crate::parse::header::Features, body: &'a [u8]| {
        payload_len_checked(h, d, body, admission)
    };
    crate::container::parse_checked(data, false, &open_length, admission, verify)
}

/// The file header (no metadata copies). Reads a prefix and checks no file
/// checksum.
pub fn header(data: &[u8]) -> Result<crate::Header, Error> {
    Ok(crate::container::parse_checked(
        data,
        true,
        &payload_len,
        &crate::parse::limits::Unrestricted,
        crate::checksum::Verify::Off,
    )?
    .header)
}

impl Parsed<'_> {
    /// Move the owned head after the caller has retained its record spans.
    /// Borrowed unit records are intentionally discarded.
    pub(crate) fn without_records(self) -> Parsed<'static> {
        Parsed {
            quick: self.quick,
            unit_rectangles: self.unit_rectangles,
            quick_bits: self.quick_bits,
            info: self.info,
            layout: self.layout,
            lw: self.lw,
            hs: self.hs,
            lanes: self.lanes,
            gcols: self.gcols,
            hint: self.hint,
            reserve: self.reserve,
            unit_ordinals: self.unit_ordinals,
            inter: self.inter,
            models: self.models,
            pal: self.pal,
            bilevel: self.bilevel,
            template: self.template,
            stripes: Vec::new(),
            head_records_span: self.head_records_span,
            #[cfg(all(feature = "lab", feature = "std"))]
            slot_names: self.slot_names,
        }
    }

    /// Stripes of the image.
    pub(crate) fn stripe_count(&self) -> usize {
        (self.info.height as usize).div_ceil(self.hs)
    }

    /// Column groups per stripe.
    pub(crate) fn ngroups(&self) -> usize {
        if self.gcols == 0 {
            1
        } else {
            (self.info.width as usize).div_ceil(self.gcols)
        }
    }

    /// First column, width and lane width of column group `gi`.
    pub(crate) fn group(&self, gi: usize) -> (usize, usize, usize) {
        let w = self.info.width as usize;
        if self.gcols == 0 {
            (0, w, self.lw)
        } else {
            let x0 = gi * self.gcols;
            let gw = self.gcols.min(w - x0);
            (x0, gw, gw.div_ceil(self.lanes))
        }
    }

    /// Output bytes per pixel.
    pub(crate) fn bpp(&self) -> usize {
        self.info.row_bytes() / self.info.width as usize
    }

    /// True when every coded channel fits 16 bits: decode with u16 storage
    /// and the SIMD kernels, whatever the container width.
    pub(crate) fn narrow(&self) -> bool {
        // Decoder-only builds without extended reject wide models before
        // reconstruction. Producers can still parse wide models for remux.
        !cfg!(any(feature = "extended", feature = "encode"))
            || self.layout.slot_bits.iter().all(|&b| b <= 16)
    }
}

/// Header and lossless payload of a file (lab tools and tests; decoders split
/// the body once with [`split_body`]).
#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
pub(crate) fn parse(data: &[u8]) -> Result<Parsed<'_>, Error> {
    let f = parse_file(
        data,
        &crate::parse::limits::Unrestricted,
        crate::checksum::Verify::Now(1),
    )?;
    parse_complete_body(f.header.info, f.header.version, f.declared, f.data)
}

fn parse_complete_body(
    info: ImageInfo,
    version: u8,
    d: crate::parse::header::Features,
    body: &[u8],
) -> Result<Parsed<'_>, Error> {
    parse_complete_body_checked(info, version, d, body, &crate::parse::limits::Unrestricted)
}

pub(crate) fn parse_complete_body_checked<'a>(
    info: ImageInfo,
    version: u8,
    d: crate::parse::header::Features,
    body: &'a [u8],
    admission: &impl crate::parse::limits::Admission,
) -> Result<Parsed<'a>, Error> {
    let (parsed, consumed) = parse_body_checked(info, version, d, body, admission)?;
    if consumed != body.len() {
        return Err(Error::Corrupt("trailing DATA bytes"));
    }
    Ok(parsed)
}

fn parse_body_checked<'a>(
    info: ImageInfo,
    version: u8,
    d: crate::parse::header::Features,
    body: &'a [u8],
    admission: &impl crate::parse::limits::Admission,
) -> Result<(Parsed<'a>, usize), Error> {
    if version == 4 {
        return Err(Error::Unsupported(
            "V4 requires its declared payload driver",
        ));
    }
    let mut r = ByteReader::new(body);
    let mut p: Parsed<'_> = parse_head_checked(info, &mut r, admission, d)?;
    // One stripe record per unit (SPEC 2.7), checked against the hint.
    let nunits = p
        .stripe_count()
        .checked_mul(p.ngroups())
        .ok_or(Error::TooLarge)?;
    if nunits > r.remaining() / 2 {
        return Err(Error::Corrupt("stripe count exceeds data"));
    }
    let mut records = crate::parse::records::Split::with_ordinals(
        &mut r,
        version,
        p.hint.take(),
        nunits,
        p.unit_ordinals,
    )?;
    for _ in 0..nunits {
        admission.check()?;
        p.stripes.push(records.next(R_STRIPE)?);
    }
    records.finish()?;
    Ok((p, r.pos))
}

/// The head of a lossless payload (everything before the stripes): the
/// image's coding parameters and models, no stripes yet. Nested images in
/// lossy files store this head and interleave their stripes. `admission`
/// charges the parsed geometry and models (`Unrestricted` compiles away).
/// `d`: the file's declared features: the near-lossless bounds record needs
/// `near_lossless`, the float map `float_shift`, the offset map
/// `offset_map` (SPEC 0.1).
pub(crate) fn parse_head_checked(
    info: ImageInfo,
    r: &mut impl crate::parse::payload::HeadInput,
    admission: &impl crate::parse::limits::Admission,
    d: crate::parse::header::Features,
) -> Result<Parsed<'static>, Error> {
    crate::parse::payload::ready(parse_head_async(
        info,
        &mut crate::parse::payload::ReadyInput(r),
        admission,
        d,
    ))
}

pub(crate) async fn parse_head_async(
    info: ImageInfo,
    r: &mut impl crate::parse::payload::AsyncHeadInput,
    admission: &impl crate::parse::limits::Admission,
    declared: crate::parse::header::Features,
) -> Result<Parsed<'static>, Error> {
    parse_head_inner(info, r, admission, None, declared, false).await
}

/// The head of a nested image of a lossy payload (DC, alpha, exact
/// blocks), whose tables carry selectors for `bank` (lossy seed byte scope
/// bits 5 and 6), under the file's declared features `d` (SPEC 2.8): no
/// near-lossless bounds, no float map, no value table and no unit table of
/// its own.
pub(crate) fn parse_head_nested(
    info: ImageInfo,
    r: &mut impl crate::parse::payload::HeadInput,
    admission: &impl crate::parse::limits::Admission,
    bank: Option<&'static crate::seeds::Bank>,
    d: crate::parse::header::Features,
) -> Result<Parsed<'static>, Error> {
    use crate::parse::header::Feature;
    let d = d
        .without(Feature::NearLossless)
        .without(Feature::FloatShift)
        .without(Feature::ValueTables);
    crate::parse::payload::ready(parse_head_inner(
        info,
        &mut crate::parse::payload::ReadyInput(r),
        admission,
        bank,
        d,
        true,
    ))
}

/// The lossless head parser under the declared features `declared`;
/// `nested`: the head of a lossy payload's nested image.
async fn parse_head_inner(
    info: ImageInfo,
    r: &mut impl crate::parse::payload::AsyncHeadInput,
    admission: &impl crate::parse::limits::Admission,
    bank: Option<&'static crate::seeds::Bank>,
    declared: crate::parse::header::Features,
    nested: bool,
) -> Result<Parsed<'static>, Error> {
    use crate::parse::header::Feature;
    #[cfg(all(feature = "lab", feature = "std"))]
    use crate::trace::describe as d;
    use crate::trace::{tf, tfd};
    admission.model()?;
    let head_start = r.pos();
    let present = tfd!("head fields", r, r.varint().await?, |v: &u64| {
        d::lossless_present(*v)
    });
    // A field this decoder does not know has no known length.
    let known = H_PALETTE
        | H_RECORDS
        | H_QUICK
        | H_QUICK_BITS
        | if cfg!(feature = "lab") { H_LAB_BANK } else { 0 };
    if present & !known != 0 {
        return Err(Error::Unsupported("unknown lossless head field"));
    }
    let quick = present & H_QUICK != 0;
    let quick_bits = present & H_QUICK_BITS != 0;
    if quick_bits && !quick {
        return Err(Error::Corrupt("quick bit smarts without quick pixels"));
    }
    if quick_bits && !declared.allows(Feature::QuickBitSmarts) {
        return Err(Error::Corrupt("quick bit smarts without feature"));
    }
    if quick && !declared.allows(Feature::QuickPixels) {
        return Err(Error::Corrupt(
            "quick pixels without the feature quick_pixels",
        ));
    }
    if quick && (present & (H_PALETTE | H_LAB_BANK) != 0 || bank.is_some()) {
        return Err(Error::Corrupt("quick pixels with palette or seed bank"));
    }
    let has_pal = present & H_PALETTE != 0;
    // Lab: the experimental standalone-seeds format names its bank (a
    // nested head takes it from the lossy envelope instead).
    #[cfg(feature = "lab")]
    let bank = if present & H_LAB_BANK != 0 {
        if bank.is_some() {
            return Err(Error::Corrupt("seed bank named twice"));
        }
        let id = tf!("seed bank (lab)", r, r.varint().await?);
        Some(crate::seeds::bank(id).ok_or(Error::Unsupported("unknown seed bank"))?)
    } else {
        bank
    };
    let lanes = tfd!("lanes", r, r.u8().await?, |v: &u8| {
        alloc::format!(
            "{v} interleaved rANS lanes: each group splits into {v} column bands decoded in lockstep"
        )
    }) as usize;
    if !(1..=NL).contains(&lanes) || (quick && lanes != 1) {
        return Err(Error::Corrupt("bad lane count"));
    }
    let hs = tfd!("stripe height", r, r.var_usize().await?, |v: &usize| {
        alloc::format!("{v} pixel rows per stripe (stripes decode independently)")
    });
    if hs == 0 {
        return Err(Error::Corrupt("bad stripe height"));
    }
    let gcols = tfd!("column group width", r, r.varint().await?, |v: &u64| {
        if *v == 0 {
            "0 = none: one group spans the width".into()
        } else {
            alloc::format!("{v} pixels per column group")
        }
    });
    if gcols >= info.width as u64 {
        return Err(Error::Corrupt("bad column group width"));
    }
    let gcols = gcols as usize;
    let nch = info.channels.count();
    // Coded channels: the container channels, or for 64-bit samples their
    // hi words, then their lo words (SPEC 2.1), each `depth` bits wide.
    let words = info.sample.words();
    let depth = info.sample.word_depth();
    let extended = info.sample.bytes() >= 4;
    // Wide nested DC models need the same capability as wide public samples.
    // Narrow nested codec planes keep their existing sample admission.
    if (!nested || extended) && admission.decodes() {
        info.sample.check_decode_support()?;
    }
    let mut maps = Vec::with_capacity(nch * words);
    // Value tables by coded channel; allocated only when a map has one.
    let mut tables: Vec<Vec<u32>> = Vec::new();
    for cc in 0..nch * words {
        if quick {
            maps.push(ChannelMap::identity(depth));
            continue;
        }
        let (c, word) = (cc % nch, cc / nch);
        let mode = MapMode::from_u8(tfd!("channel map mode", r, r.u8().await?, |v: &u8| {
            alloc::format!("{}: {v} = {}", coded_label(&info, cc), d::map_mode(*v))
        }))
        .ok_or(Error::Corrupt("bad map mode"))?;
        if mode == MapMode::FloatShift && !declared.allows(Feature::FloatShift) {
            return Err(Error::Corrupt("float map without the feature float_shift"));
        }
        if mode == MapMode::Offset && !declared.allows(Feature::OffsetMap) {
            return Err(Error::Corrupt("offset map without the feature offset_map"));
        }
        if mode == MapMode::Table && !declared.allows(Feature::ValueTables) {
            return Err(Error::Corrupt(
                "value table without the feature value_tables",
            ));
        }
        let bits = tfd!("channel map bits", r, r.u8().await?, |v: &u8| {
            alloc::format!(
                "{}: {v} significant bits in a {depth}-bit container",
                coded_label(&info, cc)
            )
        });
        let value = if extended {
            tf!("channel map value", r, r.u32().await?)
        } else {
            tf!("channel map value", r, r.u16().await?) as u32
        };
        let is_alpha = info.channels.has_alpha() && c == nch - 1;
        let ok = bits as u32 <= depth
            && match mode {
                MapMode::Lsb => true,
                // Alpha, and the lo word of any 64-bit channel.
                MapMode::Const => is_alpha || word == 1,
                MapMode::Shift => bits > 0,
                MapMode::Replicate | MapMode::Scale => bits > 0 && (!extended || bits <= 16),
                MapMode::F16InF32 => info.sample == SampleType::F32 && bits == 16,
                MapMode::UnormF32 => info.sample == SampleType::F32 && (1..=16).contains(&bits),
                MapMode::Offset => extended,
                MapMode::FloatShift => {
                    matches!(info.sample, SampleType::F16 | SampleType::F32)
                        && bits >= 1
                        && (bits as u32) < depth
                }
                // `value` is the entry count minus one; `bits` indexes them.
                MapMode::Table => {
                    let count = value as u64 + 1;
                    count <= crate::mapping::MAX_TABLE as u64
                        && count <= 1u64 << depth
                        && bits == crate::mapping::table_bits(count as usize)
                }
            };
        if !ok {
            return Err(Error::Corrupt("bad channel map"));
        }
        if mode == MapMode::Table {
            // Boxed: the head parser's own state does not grow by the
            // table reader's.
            let table = alloc::boxed::Box::pin(read_value_table(
                r,
                value as usize + 1,
                depth,
                admission,
                &info,
                cc,
            ))
            .await?;
            tables.resize_with(cc, Vec::new);
            tables.push(table);
        }
        maps.push(ChannelMap { mode, bits, value });
    }
    if !tables.is_empty() {
        tables.resize_with(nch * words, Vec::new);
    }
    // The near-lossless bounds come in a head record after the models; they
    // change neither the slots' bits nor what admission charges.
    let mut layout = Layout::new(info.channels, maps);
    if !tables.is_empty() {
        layout = layout.with_tables(tables);
    }
    let ns = layout.slot_channel.len();
    if quick {
        admission.quick(&info, hs, gcols, quick_bits)?;
    } else {
        admission.lossless(&info, &layout, lanes, hs, gcols, has_pal)?;
    }
    let mlen = if quick {
        0
    } else {
        tf!("model bytes", r, r.var_usize().await?)
    };
    let mb = crate::trace::tfba!("entropy models (bit-packed)", r, mlen);
    let mut br = BitReader::new(mb);
    let mut models = Vec::with_capacity(ns);
    for s in 0..if quick { 0 } else { ns } {
        admission.check()?;
        let md = Model::read(&mut br, layout.slot_bits[s], bank)?;
        if md.cross && !(s >= 1 && s < layout.color_slots) {
            return Err(Error::Corrupt("bad cross flag"));
        }
        models.push(md.dec_model());
    }
    let pal = if has_pal {
        let md = Model::read(&mut br, 8, bank)?;
        if md.cross {
            return Err(Error::Corrupt("bad cross flag"));
        }
        let layout = pal_layout();
        let om = OutMap::new(
            &ImageInfo::new(
                info.width,
                info.height,
                crate::Channels::Gray,
                SampleType::U8,
            ),
            &layout,
        );
        Some(Pal {
            model: md.dec_model(),
            layout,
            om,
        })
    } else {
        None
    };
    if br.overrun() {
        return Err(Error::Corrupt("truncated models"));
    }
    // Head records (SPEC 2): the unit table hint (one entry per unit, its
    // count known from the geometry before the record is decoded) and the
    // near-lossless bounds.
    let units = {
        let groups = if gcols == 0 {
            1
        } else {
            (info.width as usize).div_ceil(gcols)
        };
        (info.height as usize).div_ceil(hs).checked_mul(groups)
    };
    let mut hint = None;
    let mut reserve = None;
    let mut unit_ordinals = false;
    #[cfg(feature = "near-lossless")]
    let mut near = None;
    let mut bilevel = None;
    let mut template = None;
    let mut head_records_span = None;
    if present & H_RECORDS != 0 {
        let records_start = usize::try_from(r.pos() - head_start).map_err(|_| Error::TooLarge)?;
        let n = tf!("head record bytes", r, r.var_usize().await?);
        let list_at = r.pos();
        let list_end = usize::try_from(r.pos() - head_start)
            .map_err(|_| Error::TooLarge)?
            .checked_add(n)
            .ok_or(Error::TooLarge)?;
        head_records_span = Some(records_start..list_end);
        let list = r.bytes(n).await?;
        #[cfg(all(feature = "lab", feature = "std"))]
        crate::trace::head_records(list, false);
        crate::parse::records::head_records_at(list, |id, rec, at| match id {
            H_UNIT_TABLE | H_UNIT_RESERVE if nested => {
                Err(Error::Corrupt("unit table in a nested image head"))
            }
            H_UNIT_TABLE => {
                let n = units.ok_or(Error::Corrupt("unit table length"))?;
                hint = Some(crate::parse::records::hint_entries(rec, n)?);
                Ok(())
            }
            H_UNIT_RESERVE => {
                if hint.is_some() {
                    return Err(Error::Corrupt("unit table twice"));
                }
                let n = units.ok_or(Error::Corrupt("unit table length"))?;
                let (state, filled) = crate::parse::records::reserve_entries(rec, n)?;
                hint = filled;
                reserve = Some(crate::parse::records::Reserve {
                    state,
                    at: list_at + at as u64,
                    len: rec.len(),
                });
                Ok(())
            }
            H_NEAR => {
                if !declared.allows(Feature::NearLossless) {
                    return Err(Error::Corrupt(
                        "near-lossless payload without the feature near_lossless",
                    ));
                }
                #[cfg(feature = "near-lossless")]
                {
                    near = Some(near_bounds(&info, &layout, rec)?);
                    Ok(())
                }
                #[cfg(not(feature = "near-lossless"))]
                Err(Error::Unsupported("feature near_lossless"))
            }
            crate::bilevel::RECORD => {
                if !declared.allows(Feature::Bilevel) {
                    return Err(Error::Corrupt("bilevel table without the feature bilevel"));
                }
                if nested {
                    return Err(Error::Corrupt("bilevel table in a nested image head"));
                }
                bilevel = Some(alloc::boxed::Box::new(crate::bilevel::Table::read(rec)?));
                Ok(())
            }
            crate::template::RECORD => {
                if !declared.allows(Feature::TemplatePalettes) {
                    return Err(Error::Corrupt(
                        "template tables without the feature template_palettes",
                    ));
                }
                if nested {
                    return Err(Error::Corrupt("template tables in a nested image head"));
                }
                if quick {
                    return Err(Error::Corrupt(
                        "quick pixels with transformed sample coding",
                    ));
                }
                admission.template(&info, &layout, lanes, hs, gcols, has_pal)?;
                template = Some(crate::template::Table::read(rec, &layout.slot_bits)?);
                Ok(())
            }
            H_UNIT_ORDINALS => {
                if !declared.allows(Feature::UnitOrdinals) {
                    return Err(Error::Corrupt(
                        "unit ordinals without the feature unit_ordinals",
                    ));
                }
                if nested {
                    return Err(Error::Corrupt("unit ordinals in a nested image head"));
                }
                if !rec.is_empty() {
                    return Err(Error::Corrupt("unit ordinals record length"));
                }
                unit_ordinals = true;
                Ok(())
            }
            _ if id & 1 == 0 => Ok(()),
            _ => Err(Error::Unsupported("unknown lossless head record")),
        })?;
    }
    if quick && bilevel.is_some() {
        return Err(Error::Corrupt(
            "quick pixels with transformed sample coding",
        ));
    }
    #[cfg(feature = "near-lossless")]
    if let Some(near) = near {
        if quick {
            return Err(Error::Corrupt(
                "quick pixels with transformed sample coding",
            ));
        }
        layout = layout.with_near(&near);
    }
    let w = info.width as usize;
    let lw = w.div_ceil(lanes);
    #[cfg(all(feature = "lab", feature = "std"))]
    if crate::trace::on() {
        let slots: alloc::vec::Vec<alloc::string::String> = layout
            .slot_channel
            .iter()
            .zip(&layout.slot_bits)
            .map(|(&c, b)| alloc::format!("{} {b}-bit", coded_label(&info, c)))
            .collect();
        crate::trace::note(
            "coded slots",
            alloc::format!(
                "{} (slot order G, R, B, then A{}; residuals are coded per slot at the channel's \
                 significant bits, then expanded to the {:?} container by the channel maps)",
                slots.join(", "),
                if info.sample.words() == 2 {
                    ", hi words then lo words"
                } else {
                    ""
                },
                info.sample
            ),
        );
        crate::trace::note(
            "color decorrelation",
            alloc::string::String::from(if layout.decor_ok {
                "allowed: each stripe picks a reversible integer mode (R - G, B - G etc.), no YCoCg-R"
            } else {
                "not allowed (color slots differ in depth, or gray)"
            }),
        );
    }
    #[cfg(all(feature = "lab", feature = "std"))]
    let slot_names = layout
        .slot_channel
        .iter()
        .map(|&c| coded_label(&info, c))
        .collect();
    Ok(Parsed {
        unit_rectangles: None,
        quick,
        quick_bits,
        info,
        layout,
        lw,
        hs,
        lanes,
        gcols,
        hint,
        reserve,
        unit_ordinals,
        inter: false,
        models,
        #[cfg(all(feature = "lab", feature = "std"))]
        slot_names,
        pal,
        bilevel,
        template,
        stripes: Vec::new(),
        head_records_span,
    })
}

/// The near-lossless bounds record (SPEC 2, head record 19): one bound
/// per coded channel in coded units, at least one nonzero, filling the
/// record; only sample types that define bounds (SPEC 2.6).
#[cfg(feature = "near-lossless")]
fn near_bounds(info: &ImageInfo, layout: &Layout, rec: &[u8]) -> Result<Vec<u32>, Error> {
    if !crate::near::defined_for(info.sample) {
        return Err(Error::Corrupt(
            "near-lossless payload for a sample type without bounds",
        ));
    }
    let mut r = ByteReader::new(rec);
    let mut near = vec![0u32; layout.maps.len()];
    for (c, m) in layout.maps.iter().enumerate() {
        let dv = r.varint()?;
        // Bounds are in coded units of a regular map; a table's indices
        // have no uniform step, so its channel stays exact.
        let ok = dv == 0
            || (!matches!(m.mode, MapMode::Const | MapMode::Table)
                && dv <= crate::near::delta_cap(m.bits as u32) as u64);
        if !ok {
            return Err(Error::Corrupt("bad near-lossless bound"));
        }
        near[c] = dv as u32;
    }
    if r.remaining() != 0 {
        return Err(Error::Corrupt("near-lossless bounds record length"));
    }
    if near.iter().all(|&d| d == 0) {
        return Err(Error::Corrupt("near-lossless head without a bound"));
    }
    Ok(near)
}

/// A value table (map mode 9, SPEC 2.1) of `count` ascending ordered
/// values below `2^depth`: each a varint, the distance from the smallest
/// value it may take (0 for the first entry, its predecessor plus one after).
async fn read_value_table(
    r: &mut impl crate::parse::payload::AsyncHeadInput,
    count: usize,
    depth: u32,
    admission: &impl crate::parse::limits::Admission,
    info: &ImageInfo,
    cc: usize,
) -> Result<Vec<u32>, Error> {
    admission.value_table(count)?;
    let mut table = Vec::new();
    table
        .try_reserve_exact(count)
        .map_err(|_| Error::TooLarge)?;
    #[cfg(all(feature = "lab", feature = "std"))]
    let start = r.mark();
    let limit = 1u64 << depth;
    let mut next = 0u64;
    for i in 0..count {
        let e = next
            .checked_add(r.varint().await?)
            .filter(|&e| e < limit)
            .ok_or(Error::Corrupt("value table entry out of range"))?;
        table.push(e as u32);
        next = e + 1;
        if i % 4096 == 4095 {
            admission.check()?;
        }
    }
    #[cfg(all(feature = "lab", feature = "std"))]
    if crate::trace::on() {
        crate::trace::field_addr(
            "value table",
            crate::trace::shown(alloc::format!(
                "{}: {count} ordered values {} ..= {}",
                coded_label(info, cc),
                table[0],
                table[count - 1]
            )),
            start,
            r.mark(),
        );
    }
    #[cfg(not(all(feature = "lab", feature = "std")))]
    let _ = (info, cc);
    Ok(table)
}

/// Writes a value table (see [`read_value_table`]).
#[cfg(feature = "encode")]
pub(crate) fn put_value_table(out: &mut Vec<u8>, table: &[u32]) {
    let mut next = 0u64;
    for &e in table {
        put_varint(out, e as u64 - next);
        next = e as u64 + 1;
    }
}

/// LAB trace: the name of coded channel `cc` (a container channel, or for
/// 64-bit samples its hi or lo word).
#[cfg(all(feature = "lab", feature = "std"))]
fn coded_label(info: &ImageInfo, cc: usize) -> alloc::string::String {
    let nch = info.channels.count();
    let name = crate::trace::channel_label(info.channels, cc % nch);
    match (info.sample.words(), cc / nch) {
        (1, _) => name,
        (_, 0) => alloc::format!("{name} hi"),
        _ => alloc::format!("{name} lo"),
    }
}

/// Empty u16 value tables for images without a table channel (no
/// allocation), one per possible coded channel.
static NO_TABLES16: [Vec<u16>; 4] = [const { Vec::new() }; 4];

/// Value tables of the 8- and 16-bit writers, per coded channel.
#[derive(Default)]
struct Tables16 {
    /// u16 entries ([`crate::recon::OutKind::Lut`]); empty for other maps.
    l16: [Vec<u16>; 4],
    /// `l16` pair-packed for the AVX-512 16-bit writer's u32 gathers.
    #[cfg(all(feature = "avx512", target_arch = "x86_64"))]
    packed: Vec<Vec<u32>>,
}

/// Output writer for one row: coded values (step-major per channel) to pixels.
pub(crate) struct OutMap {
    /// SIMD writer transforms, when every channel has one (read only by
    /// the x86 writers), and whether i8 samples flip their sign bit.
    #[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
    simd8: Option<(Vec<crate::recon::OutKind>, bool)>,
    #[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
    simd16: Option<(Vec<crate::recon::OutKind>, crate::recon::Ord16)>,
    /// Per coded channel transforms of the 32-bit writer, and the output
    /// words of a pixel in memory order: (coded channel, order map inverse).
    #[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
    simd32: Option<(Vec<crate::wide::Out32>, Vec<(usize, crate::wide::Ord32)>)>,
    /// Per coded channel: Some(lut) when expansion is needed and the
    /// coded depth is <= 16 bits.
    luts: Vec<Option<Vec<u32>>>,
    /// Value tables of the 8- and 16-bit writers ([`crate::recon::OutKind::Lut`]),
    /// per coded channel; empty for every other map.
    #[cfg_attr(
        not(any(
            target_arch = "x86_64",
            target_arch = "aarch64",
            all(target_arch = "wasm32", target_feature = "simd128")
        )),
        allow(dead_code)
    )]
    tables16: Option<alloc::boxed::Box<Tables16>>,
    maps: Vec<ChannelMap>,
    /// Per coded channel: coded slot, or None for const.
    slot_of: Vec<Option<usize>>,
    consts: Vec<u32>,
    sample: SampleType,
    nch: usize,
}

impl OutMap {
    /// Expansion payload in u32-sized capacity units, shared with admission.
    /// Include the word writers' u16 and packed copies; the primary LUT still
    /// has `1 << bits` entries. Algebraic maps need no expansion payload.
    pub(crate) fn lut_entries(map: &ChannelMap, depth: u32) -> usize {
        if matches!(map.mode, MapMode::Const | MapMode::Shift | MapMode::Offset)
            || map.is_identity(depth)
            || map.bits > 16
        {
            0
        } else {
            let entries = 1usize << map.bits;
            if map.mode == MapMode::Table && depth <= 16 {
                let packed =
                    cfg!(all(feature = "avx512", target_arch = "x86_64")) && entries >= 32768;
                let bytes_per_entry =
                    size_of::<u32>() + size_of::<u16>() + if packed { size_of::<u16>() } else { 0 };
                (entries * bytes_per_entry).div_ceil(size_of::<u32>())
            } else {
                entries
            }
        }
    }

    pub(crate) fn new(info: &ImageInfo, layout: &Layout) -> Self {
        let depth = info.sample.word_depth();
        let nch = info.channels.count();
        let mut slot_of = vec![None; layout.maps.len()];
        for (s, &c) in layout.slot_channel.iter().enumerate() {
            slot_of[c] = Some(s);
        }
        let luts: Vec<Option<Vec<u32>>> = layout
            .maps
            .iter()
            .enumerate()
            .map(|(c, m)| {
                // Shift/Offset expansion is exact scalar/SIMD arithmetic.
                // A table here also forces the wide SIMD writer to gather
                // instead of using its existing shift/add operations.
                if Self::lut_entries(m, depth) == 0 {
                    None
                } else if m.mode == MapMode::Table {
                    // Every coded value of the slot, past the last entry too.
                    let table = layout.table(c);
                    Some(
                        (0..1u32 << m.bits)
                            .map(|s| crate::mapping::table_expand(table, s))
                            .collect(),
                    )
                } else {
                    Some(m.lut(depth))
                }
            })
            .collect();
        let consts: Vec<u32> = layout.maps.iter().map(|m| m.value).collect();
        // The 8- and 16-bit writers look value tables up in u16 (at most
        // four coded channels at those depths). Boxed and only with a table
        // channel: other images allocate nothing and keep `OutMap` small.
        let tables16: Option<alloc::boxed::Box<Tables16>> = (layout.has_tables() && depth <= 16)
            .then(|| {
                let mut t16: alloc::boxed::Box<Tables16> = alloc::boxed::Box::default();
                for ((m, lut), t) in layout.maps.iter().zip(&luts).zip(t16.l16.iter_mut()) {
                    if let Some(lut) = lut
                        && m.mode == MapMode::Table
                    {
                        *t = lut.iter().map(|&v| v as u16).collect();
                    }
                }
                // The AVX-512 16-bit writer gathers u16 values two at a
                // time: `packed[c][j] = l16[c][2j] | l16[c][2j + 1] << 16`
                // keeps the byte count of `l16` with u32 gather semantics
                // (table lengths are even powers of two). Per channel,
                // empty below 32768 entries.
                #[cfg(all(feature = "avx512", target_arch = "x86_64"))]
                {
                    t16.packed = t16
                        .l16
                        .iter()
                        .map(|l| {
                            if l.len() < 32768 {
                                return Vec::new();
                            }
                            l.as_chunks::<2>()
                                .0
                                .iter()
                                .map(|w| w[0] as u32 | (w[1] as u32) << 16)
                                .collect()
                        })
                        .collect();
                }
                t16
            });
        let simd8 = if matches!(info.sample, SampleType::U8 | SampleType::I8) {
            use crate::recon::OutKind;
            layout
                .maps
                .iter()
                .map(|m| match m.mode {
                    MapMode::Table => Some(OutKind::Lut),
                    MapMode::Const => Some(OutKind::Const(m.value as u16)),
                    _ if m.is_identity(8) => Some(OutKind::Ident),
                    MapMode::Shift => Some(OutKind::Shl(8 - m.bits as u32)),
                    MapMode::Replicate | MapMode::Scale if m.bits == 1 => Some(OutKind::Bit(255)),
                    MapMode::Replicate | MapMode::Scale if (2..8).contains(&m.bits) => {
                        OutKind::short8(m.bits as u32, m.mode == MapMode::Replicate, |v| {
                            m.expand(v, 8)
                        })
                    }
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
                .map(|k| (k, info.sample == SampleType::I8))
        } else {
            None
        };
        let simd16 = if matches!(
            info.sample,
            SampleType::U16 | SampleType::F16 | SampleType::I16
        ) {
            use crate::recon::{Ord16, OutKind};
            let ord = match info.sample {
                SampleType::F16 => Ord16::Float,
                SampleType::I16 => Ord16::Sign,
                _ => Ord16::Plain,
            };
            layout
                .maps
                .iter()
                .map(|m| match m.mode {
                    MapMode::Table => Some(OutKind::Lut),
                    MapMode::Const => Some(OutKind::Const(m.value as u16)),
                    _ if m.is_identity(16) => Some(OutKind::Ident),
                    MapMode::Shift => Some(OutKind::Shl(16 - m.bits as u32)),
                    MapMode::Replicate if m.bits >= 8 => Some(OutKind::Rep16(m.bits as u32)),
                    MapMode::Replicate | MapMode::Scale if m.bits == 1 => Some(OutKind::Bit(65535)),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
                .map(|k| (k, ord))
        } else {
            None
        };
        let simd32 = if depth == 32 {
            use crate::wide::{Ord32, Out32};
            let kinds = layout
                .maps
                .iter()
                .zip(&luts)
                .map(|(m, lut): (&ChannelMap, &Option<Vec<u32>>)| match m.mode {
                    MapMode::Const => Out32::Const(m.value),
                    _ if lut.is_some() => Out32::Lut,
                    MapMode::FloatShift => Out32::FloatShift(m.bits as u32),
                    MapMode::Offset => Out32::Add(m.value),
                    MapMode::Shift => Out32::Shl(32 - m.bits as u32),
                    _ => Out32::Ident,
                })
                .collect();
            let ord = match info.sample {
                SampleType::I32 | SampleType::I64 => Ord32::Sign,
                SampleType::F32 | SampleType::F64 => Ord32::Float,
                _ => Ord32::Plain,
            };
            // A 64-bit sample is its lo word, then its hi word (little-endian
            // memory order; the x86 writer is the only user).
            let words = (0..nch)
                .flat_map(|c| {
                    if info.sample.words() == 2 {
                        [Some((nch + c, Ord32::Plain)), Some((c, ord))]
                    } else {
                        [Some((c, ord)), None]
                    }
                })
                .flatten()
                .collect();
            Some((kinds, words))
        } else {
            None
        };
        Self {
            simd8,
            simd16,
            simd32,
            luts,
            tables16,
            maps: layout.maps.clone(),
            slot_of,
            consts,
            sample: info.sample,
            nch,
        }
    }

    /// Per-channel u16 value tables for the 8- and 16-bit writers: empty
    /// slices for every channel when the image has no table channel.
    #[cfg_attr(
        not(any(
            target_arch = "x86_64",
            target_arch = "aarch64",
            all(target_arch = "wasm32", target_feature = "simd128")
        )),
        allow(dead_code)
    )]
    #[inline(always)]
    fn luts16(&self) -> &[Vec<u16>; 4] {
        self.tables16.as_deref().map_or(&NO_TABLES16, |t| &t.l16)
    }

    /// The expanded value of coded channel `c` at step position `i`.
    #[inline(always)]
    fn value<S: crate::px::Px>(&self, cur: &[Vec<S>], c: usize, i: usize, depth: u32) -> u32 {
        match self.slot_of[c] {
            Some(s) => {
                let cv = cur[s][i].get();
                match &self.luts[c] {
                    Some(t) => t[cv as usize],
                    None => self.maps[c].expand(cv, depth),
                }
            }
            None => self.consts[c],
        }
    }
}

/// Reconstructs one row (scalar reference) and writes output pixels.
#[allow(clippy::too_many_arguments)]
pub(crate) fn recon_row_scalar<S: crate::px::Px>(
    layout: &Layout,
    res: &[Vec<S>],
    prev: &[Vec<S>],
    cur: &mut [Vec<S>],
    widths: &[u32; NL],
    lw: usize,
    row: usize,
    pred: u8,
    decor: u8,
    om: &OutMap,
    out_row: &mut [u8],
) {
    if layout.is_near() {
        recon_row_scalar_impl::<S, true>(
            layout, res, prev, cur, widths, lw, row, pred, decor, om, out_row,
        )
    } else {
        recon_row_scalar_impl::<S, false>(
            layout, res, prev, cur, widths, lw, row, pred, decor, om, out_row,
        )
    }
}

/// [`recon_row_scalar`]; `NEAR`: the layout has near-lossless slots.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn recon_row_scalar_impl<S: crate::px::Px, const NEAR: bool>(
    layout: &Layout,
    res: &[Vec<S>],
    prev: &[Vec<S>],
    cur: &mut [Vec<S>],
    widths: &[u32; NL],
    lw: usize,
    row: usize,
    pred: u8,
    decor: u8,
    om: &OutMap,
    out_row: &mut [u8],
) {
    let ns = res.len();
    let cs = layout.color_slots;
    for l in 0..NL {
        let wl = widths[l] as usize;
        for x in 0..wl {
            let i = x * NL + l;
            let mut d = [0u32; MAX_SLOTS];
            for s in 0..ns {
                d[s] = unzigzag(res[s][i].get(), crate::mask_of(layout.slot_bits[s]));
            }
            if cs == 3 && decor != 0 {
                let bits = layout.slot_bits[0];
                decor_inverse(decor, &mut d, bits, crate::mask_of(bits));
            }
            for s in 0..ns {
                let mask = crate::mask_of(layout.slot_bits[s]);
                let (wv, n, nw, ne) = if row == 0 {
                    let wv = if x > 0 { cur[s][i - NL].get() } else { 0 };
                    (wv, wv, wv, wv)
                } else {
                    let n = prev[s][i].get();
                    let (wv, nw) = if x > 0 {
                        (cur[s][i - NL].get(), prev[s][i - NL].get())
                    } else {
                        (n, n)
                    };
                    let ne = if x + 1 < wl { prev[s][i + NL].get() } else { n };
                    (wv, n, nw, ne)
                };
                let p = predict::predict(pred, wv, n, nw, ne, mask);
                cur[s][i] = S::put(if NEAR && layout.near[s] != 0 {
                    crate::near::reconstruct(p, d[s], layout.slot_bits[s], layout.near[s])
                } else {
                    p.wrapping_add(d[s]) & mask
                });
            }
        }
    }
    write_output_row(om, cur, widths, lw, out_row);
}

/// Writes coded values (step-major per slot) as output pixels.
pub(crate) fn write_output_row<S: crate::px::Px>(
    om: &OutMap,
    cur: &[Vec<S>],
    widths: &[u32; NL],
    lw: usize,
    out_row: &mut [u8],
) {
    #[cfg(target_arch = "x86_64")]
    if let Some(cur16) = S::rows16(cur) {
        use archmage::SimdToken;
        if !force_scalar()
            && let Some(t) = archmage::X64V3Token::summon()
        {
            if let Some((kinds, sign)) = &om.simd8 {
                let cs: Vec<&[u16]> = cur16.iter().map(|v| v.as_slice()).collect();
                #[cfg(feature = "avx512")]
                if !force_avx2()
                    && let Some(t4) = archmage::X64V4xToken::summon()
                    && crate::recon::out_v4x::write_row8(
                        t4,
                        &cs,
                        &om.slot_of,
                        kinds,
                        om.luts16(),
                        *sign,
                        widths,
                        lw,
                        out_row,
                    )
                {
                    return;
                }
                // The table-free instance keeps main's loads.
                if om.tables16.is_some() {
                    crate::recon::out_avx2::write_row8::<true>(
                        t,
                        &cs,
                        &om.slot_of,
                        kinds,
                        om.luts16(),
                        *sign,
                        widths,
                        lw,
                        out_row,
                    );
                } else {
                    crate::recon::out_avx2::write_row8::<false>(
                        t,
                        &cs,
                        &om.slot_of,
                        kinds,
                        &NO_TABLES16,
                        *sign,
                        widths,
                        lw,
                        out_row,
                    );
                }
                return;
            }
            if let Some((kinds, ord)) = &om.simd16 {
                let cs: Vec<&[u16]> = cur16.iter().map(|v| v.as_slice()).collect();
                #[cfg(feature = "avx512")]
                if let Some(t16) = om.tables16.as_deref()
                    && !force_avx2()
                    && let Some(t4) = archmage::X64V4xToken::summon()
                    && crate::recon::out_v4x::write_row16(
                        t4,
                        &cs,
                        &om.slot_of,
                        kinds,
                        &t16.l16,
                        &om.luts,
                        &t16.packed,
                        *ord,
                        widths,
                        lw,
                        out_row,
                    )
                {
                    return;
                }
                if om.tables16.is_some() {
                    crate::recon::out_avx2::write_row16::<true>(
                        t,
                        &cs,
                        &om.slot_of,
                        kinds,
                        om.luts16(),
                        *ord,
                        widths,
                        lw,
                        out_row,
                    );
                } else {
                    crate::recon::out_avx2::write_row16::<false>(
                        t,
                        &cs,
                        &om.slot_of,
                        kinds,
                        &NO_TABLES16,
                        *ord,
                        widths,
                        lw,
                        out_row,
                    );
                }
                return;
            }
        }
    }
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    if let Some((kinds, words)) = &om.simd32 {
        use archmage::SimdToken;
        if !force_scalar()
            && lw < (1 << 28)
            && let Some(t) = archmage::Wasm128Token::summon()
        {
            let c16: Option<Vec<&[u16]>> =
                S::rows16(cur).map(|r| r.iter().map(|v| v.as_slice()).collect());
            let c32: Option<Vec<&[u32]>> =
                S::rows32(cur).map(|r| r.iter().map(|v| v.as_slice()).collect());
            crate::wide::out_wasm128::write_row32(
                t,
                c16.as_deref(),
                c32.as_deref(),
                &om.slot_of,
                kinds,
                &om.luts,
                words,
                widths,
                lw,
                out_row,
            );
            return;
        }
    }
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    if let Some(cur16) = S::rows16(cur) {
        use archmage::SimdToken;
        if !force_scalar()
            && let Some(t) = archmage::Wasm128Token::summon()
        {
            // At most MAX_SLOTS coded planes. Borrow their rows on the stack
            // instead of allocating a Vec of references for every output row.
            let refs: [&[u16]; MAX_SLOTS] =
                core::array::from_fn(|s| cur16.get(s).map_or(&[][..], |row| row.as_slice()));
            let cs = &refs[..cur16.len()];
            if let Some((kinds, sign)) = &om.simd8 {
                crate::recon::out_wasm128::write_row8(
                    t,
                    cs,
                    &om.slot_of,
                    kinds,
                    om.luts16(),
                    *sign,
                    widths,
                    lw,
                    out_row,
                );
                return;
            }
            if let Some((kinds, ord)) = &om.simd16 {
                crate::recon::out_wasm128::write_row16(
                    t,
                    cs,
                    &om.slot_of,
                    kinds,
                    om.luts16(),
                    *ord,
                    widths,
                    lw,
                    out_row,
                );
                return;
            }
        }
    }
    #[cfg(target_arch = "x86_64")]
    if let Some((kinds, words)) = &om.simd32 {
        use archmage::SimdToken;
        if !force_scalar()
            && lw < (1 << 28)
            && let Some(t) = archmage::X64V3Token::summon()
        {
            let c16: Option<Vec<&[u16]>> =
                S::rows16(cur).map(|r| r.iter().map(|v| v.as_slice()).collect());
            let c32: Option<Vec<&[u32]>> =
                S::rows32(cur).map(|r| r.iter().map(|v| v.as_slice()).collect());
            crate::wide::out_avx2::write_row32(
                t,
                c16.as_deref(),
                c32.as_deref(),
                &om.slot_of,
                kinds,
                &om.luts,
                words,
                widths,
                lw,
                out_row,
            );
            return;
        }
    }
    #[cfg(target_arch = "aarch64")]
    if let Some((kinds, words)) = &om.simd32 {
        use archmage::SimdToken;
        if !force_scalar()
            && lw < (1 << 28)
            && let Some(t) = archmage::NeonToken::summon()
        {
            let c16: Option<Vec<&[u16]>> =
                S::rows16(cur).map(|r| r.iter().map(|v| v.as_slice()).collect());
            let c32: Option<Vec<&[u32]>> =
                S::rows32(cur).map(|r| r.iter().map(|v| v.as_slice()).collect());
            crate::wide::out_neon::write_row32(
                t,
                c16.as_deref(),
                c32.as_deref(),
                &om.slot_of,
                kinds,
                &om.luts,
                words,
                widths,
                lw,
                out_row,
            );
            return;
        }
    }
    #[cfg(target_arch = "aarch64")]
    if let Some(cur16) = S::rows16(cur) {
        use archmage::SimdToken;
        if !force_scalar()
            && let Some(t) = archmage::NeonToken::summon()
        {
            let cs: Vec<&[u16]> = cur16.iter().map(|v| v.as_slice()).collect();
            if let Some((kinds, sign)) = &om.simd8 {
                crate::recon::out_neon::write_row8(
                    t,
                    &cs,
                    &om.slot_of,
                    kinds,
                    om.luts16(),
                    *sign,
                    widths,
                    lw,
                    out_row,
                );
                return;
            }
            if let Some((kinds, ord)) = &om.simd16 {
                crate::recon::out_neon::write_row16(
                    t,
                    &cs,
                    &om.slot_of,
                    kinds,
                    om.luts16(),
                    *ord,
                    widths,
                    lw,
                    out_row,
                );
                return;
            }
        }
    }
    let nch = om.nch;
    let depth = om.sample.word_depth();
    let bps = om.sample.bytes();
    if om.sample.words() == 2 {
        // 64-bit samples: hi word (order map inverse) and lo word.
        for l in 0..NL {
            for x in 0..widths[l] as usize {
                let (px, i) = (l * lw + x, x * NL + l);
                for c in 0..nch {
                    let hi = om.sample.order(om.value(cur, c, i, depth));
                    let lo = om.value(cur, nch + c, i, depth);
                    let v = ((hi as u64) << 32) | lo as u64;
                    let o = (px * nch + c) * 8;
                    out_row[o..o + 8].copy_from_slice(&v.to_ne_bytes());
                }
            }
        }
        return;
    }
    for l in 0..NL {
        let wl = widths[l] as usize;
        for x in 0..wl {
            let px = l * lw + x;
            for c in 0..nch {
                let v = om.sample.order(om.value(cur, c, x * NL + l, depth));
                let o = (px * nch + c) * bps;
                match bps {
                    1 => out_row[o] = v as u8,
                    2 => out_row[o..o + 2].copy_from_slice(&(v as u16).to_ne_bytes()),
                    _ => out_row[o..o + 4].copy_from_slice(&v.to_ne_bytes()),
                }
            }
        }
    }
}

#[inline(always)]
fn sgn(v: u32, bits: u32) -> i64 {
    if bits == 0 {
        return 0;
    }
    let half = 1u64 << (bits - 1);
    if v as u64 >= half {
        v as i64 - (1i64 << bits)
    } else {
        v as i64
    }
}

/// Inverse of [`encode::decor_forward`] on (G, R, B) residuals.
#[inline(always)]
pub(crate) fn decor_inverse(mode: u8, d: &mut [u32], bits: u32, mask: u32) {
    let g = d[0];
    match mode {
        1 => {
            d[1] = d[1].wrapping_add(g) & mask;
            d[2] = d[2].wrapping_add(g) & mask;
        }
        2 => {
            d[1] = d[1].wrapping_add(g) & mask;
            d[2] = d[2].wrapping_add(d[1]) & mask;
        }
        3 => {
            d[1] = d[1].wrapping_add(g) & mask;
            let avg = (sgn(d[1], bits) + sgn(g, bits)) >> 1;
            d[2] = d[2].wrapping_add(avg as u32) & mask;
        }
        _ => {}
    }
}

/// Paired lossless rows (lab switch for A/B timing).
#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
pub(crate) static ROW_PAIRS: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(2);

/// LAB: value-table maps: 0 (default) where estimated smaller, 1 never,
/// 2 on every candidate channel (`lab::set_value_tables`).
#[cfg(feature = "lab")]
pub(crate) static VALUE_TABLES: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// Whether the encoder considers value tables (always, outside the lab).
#[cfg(feature = "encode")]
fn value_tables_allowed() -> bool {
    #[cfg(feature = "lab")]
    return VALUE_TABLES.load(core::sync::atomic::Ordering::Relaxed) != 1;
    #[cfg(not(feature = "lab"))]
    true
}

/// LAB: value tables on every candidate channel, without the estimate.
#[cfg(feature = "encode")]
pub(crate) fn value_tables_forced() -> bool {
    #[cfg(feature = "lab")]
    return VALUE_TABLES.load(core::sync::atomic::Ordering::Relaxed) == 2;
    #[cfg(not(feature = "lab"))]
    false
}

/// Decodes two slots' rows together with the AVX-512 paired kernel; false
/// when it does not apply (other tiers, 32-bit storage), and the caller
/// decodes them one at a time.
#[allow(clippy::too_many_arguments)]
fn decode_row_pair<S: crate::px::Px>(
    md: [&DecModel; 2],
    words: [&Words<'_>; 2],
    st: [&mut LaneState; 2],
    widths: &[u32; NL],
    lw: usize,
    row: usize,
    prev: [&[S]; 2],
    cross: [Option<&[S]>; 2],
    out: [&mut [S]; 2],
) -> bool {
    #[cfg(any(all(test, feature = "encode"), feature = "lab"))]
    if ROW_PAIRS.load(core::sync::atomic::Ordering::Relaxed) < 1 {
        return false;
    }
    #[cfg(target_arch = "x86_64")]
    {
        use archmage::SimdToken;
        if force_scalar() {
            return false;
        }
        let [oa, ob] = out;
        let (Some(pa), Some(pb), Some(oa), Some(ob)) = (
            S::slice16(prev[0]),
            S::slice16(prev[1]),
            S::slice16_mut(oa),
            S::slice16_mut(ob),
        ) else {
            return false;
        };
        let c = cross.map(|c| c.map(|c| S::slice16(c).unwrap()));
        if !force_avx2()
            && let Some(t) = archmage::X64V4Token::summon()
        {
            rans::avx512::decode_row2(t, md, words, st, widths, lw, row, [pa, pb], c, [oa, ob]);
            return true;
        }
        let Some(t) = archmage::X64V3Token::summon() else {
            return false;
        };
        rans::avx2::decode_row2(
            t,
            md,
            words,
            st,
            widths,
            lw,
            [row, row],
            [pa, pb],
            c,
            [oa, ob],
        );
        true
    }
    #[cfg(target_arch = "aarch64")]
    {
        use archmage::SimdToken;
        if force_scalar() {
            return false;
        }
        let [oa, ob] = out;
        let (Some(pa), Some(pb), Some(oa), Some(ob)) = (
            S::slice16(prev[0]),
            S::slice16(prev[1]),
            S::slice16_mut(oa),
            S::slice16_mut(ob),
        ) else {
            return false;
        };
        let c = cross.map(|c| c.map(|c| S::slice16(c).unwrap()));
        let Some(t) = archmage::NeonToken::summon() else {
            return false;
        };
        crate::rans_neon::decode_row2(
            t,
            md,
            words,
            st,
            widths,
            lw,
            [row, row],
            [pa, pb],
            c,
            [oa, ob],
        );
        true
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        let _ = (md, words, st, widths, lw, row, prev, cross, out);
        false
    }
}

/// Decodes three slots' rows together (AVX-512); false when the kernel
/// does not apply.
#[allow(clippy::too_many_arguments)]
fn decode_row_triple<S: crate::px::Px>(
    md: [&DecModel; 3],
    words: [&Words<'_>; 3],
    st: [&mut LaneState; 3],
    widths: &[u32; NL],
    lw: usize,
    rows: [usize; 3],
    prev: [&[S]; 3],
    cross: [Option<&[S]>; 3],
    out: [&mut [S]; 3],
) -> bool {
    #[cfg(any(all(test, feature = "encode"), feature = "lab"))]
    if ROW_PAIRS.load(core::sync::atomic::Ordering::Relaxed) < 2 {
        return false;
    }
    #[cfg(target_arch = "x86_64")]
    {
        use archmage::SimdToken;
        if force_scalar() {
            return false;
        }
        let [oa, ob, oc] = out;
        let (Some(pa), Some(pb), Some(pc), Some(oa), Some(ob), Some(oc)) = (
            S::slice16(prev[0]),
            S::slice16(prev[1]),
            S::slice16(prev[2]),
            S::slice16_mut(oa),
            S::slice16_mut(ob),
            S::slice16_mut(oc),
        ) else {
            return false;
        };
        let c = cross.map(|c| c.map(|c| S::slice16(c).unwrap()));
        if !force_avx2()
            && let Some(t) = archmage::X64V4Token::summon()
        {
            rans::avx512::decode_row3(
                t,
                md,
                words,
                st,
                widths,
                lw,
                rows,
                [pa, pb, pc],
                c,
                [oa, ob, oc],
            );
            return true;
        }
        // AVX2 keeps pairs only: three instances outgrow its 16 registers
        // (-3% on Zen 3; benchmarks/lossless_row_pairs_zen4_2026-10-03.txt).
        false
    }
    #[cfg(target_arch = "aarch64")]
    {
        use archmage::SimdToken;
        if force_scalar() {
            return false;
        }
        let [oa, ob, oc] = out;
        let (Some(pa), Some(pb), Some(pc), Some(oa), Some(ob), Some(oc)) = (
            S::slice16(prev[0]),
            S::slice16(prev[1]),
            S::slice16(prev[2]),
            S::slice16_mut(oa),
            S::slice16_mut(ob),
            S::slice16_mut(oc),
        ) else {
            return false;
        };
        let c = cross.map(|c| c.map(|c| S::slice16(c).unwrap()));
        let Some(t) = archmage::NeonToken::summon() else {
            return false;
        };
        crate::rans_neon::decode_row3(
            t,
            md,
            words,
            st,
            widths,
            lw,
            rows,
            [pa, pb, pc],
            c,
            [oa, ob, oc],
        );
        true
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        let _ = (md, words, st, widths, lw, rows, prev, cross, out);
        false
    }
}

/// Kernel selection: `IMIJ_SCALAR` forces the scalar reference (testing).
#[allow(clippy::too_many_arguments)]
#[inline]
fn decode_row<S: crate::px::Px>(
    md: &DecModel,
    words: &Words<'_>,
    st: &mut LaneState,
    widths: &[u32; NL],
    lw: usize,
    row: usize,
    prev: &[S],
    cross: Option<&[S]>,
    out: &mut [S],
) {
    // SIMD kernels handle 16-bit storage (baseline tier) only.
    #[cfg(target_arch = "x86_64")]
    if let (Some(prev16), Some(out16)) = (S::slice16(prev), S::slice16_mut(out)) {
        use archmage::SimdToken;
        let cross16 = cross.map(|c| S::slice16(c).unwrap());
        let (prev, cross, out) = (prev16, cross16, out16);
        if !force_scalar() {
            if !force_avx2()
                && let Some(t) = archmage::X64V4Token::summon()
            {
                rans::avx512::decode_row(t, md, words, st, widths, lw, row, prev, cross, out);
                return;
            }
            if let Some(t) = archmage::X64V3Token::summon() {
                rans::avx2::decode_row(t, md, words, st, widths, lw, row, prev, cross, out);
                return;
            }
        }
        rans::decode_row_scalar(md, words, st, widths, lw, row, prev, cross, out);
        return;
    }
    // 32-bit storage (extended tier).
    #[cfg(target_arch = "x86_64")]
    if let (Some(prev32), Some(out32)) = (S::slice32(prev), S::slice32_mut(out)) {
        use archmage::SimdToken;
        if !force_scalar()
            && let Some(t) = archmage::X64V3Token::summon()
        {
            let cross32 = cross.map(|c| S::slice32(c).unwrap());
            crate::wide::avx2::decode_row(
                t, md, words, st, widths, lw, row, prev32, cross32, out32,
            );
            return;
        }
        rans::decode_row_scalar(
            md,
            words,
            st,
            widths,
            lw,
            row,
            prev32,
            cross.map(|c| S::slice32(c).unwrap()),
            out32,
        );
        return;
    }
    // aarch64: the register-resident NEON kernel handles 16-bit storage.
    #[cfg(target_arch = "aarch64")]
    if let (Some(prev16), Some(out16)) = (S::slice16(prev), S::slice16_mut(out)) {
        use archmage::SimdToken;
        if !force_scalar()
            && let Some(t) = archmage::NeonToken::summon()
        {
            let cross16 = cross.map(|c| S::slice16(c).unwrap());
            crate::rans_neon::decode_row(t, md, words, st, widths, lw, row, prev16, cross16, out16);
            return;
        }
    }
    // Generated portable tiers share one body across both storage widths.
    #[cfg(any(
        target_arch = "aarch64",
        all(target_arch = "wasm32", target_feature = "simd128")
    ))]
    if !force_scalar() {
        crate::rans_portable::decode_row(md, words, st, widths, lw, row, prev, cross, out);
        return;
    }
    rans::decode_row_scalar(md, words, st, widths, lw, row, prev, cross, out);
}

/// AVX2 reconstruction for 32-bit storage; false = use the scalar path.
#[allow(clippy::too_many_arguments)]
#[cfg_attr(not(target_arch = "x86_64"), allow(unused_variables))]
fn recon_row_simd32(
    layout: &Layout,
    res: &[Vec<u32>],
    prev: &[Vec<u32>],
    cur: &mut [Vec<u32>],
    widths: &[u32; NL],
    lw: usize,
    row: usize,
    pred: u8,
    decor: u8,
) -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        use archmage::SimdToken;
        if force_scalar() || lw >= (1 << 30) {
            return false;
        }
        let Some(t) = archmage::X64V3Token::summon() else {
            return false;
        };
        fn go<const NS: usize, const NEAR: bool>(
            t: archmage::X64V3Token,
            bits: &[u32],
            near: &[u32],
            color_slots: usize,
            res: &[Vec<u32>],
            prev: &[Vec<u32>],
            cur: &mut [Vec<u32>],
            (widths, lw, row, pred, decor): (&[u32; NL], usize, usize, u8, u8),
        ) {
            let r: [&[u32]; NS] = core::array::from_fn(|s| res[s].as_slice());
            let pv: [&[u32]; NS] = core::array::from_fn(|s| prev[s].as_slice());
            let mut it = cur.iter_mut();
            let mut c: [&mut [u32]; NS] =
                core::array::from_fn(|_| it.next().unwrap().as_mut_slice());
            use crate::wide::avx2::recon_row as rr;
            let cs = color_slots;
            match pred {
                0 => rr::<NS, 0, NEAR>(t, bits, near, cs, &r, &pv, &mut c, widths, lw, row, decor),
                1 => rr::<NS, 1, NEAR>(t, bits, near, cs, &r, &pv, &mut c, widths, lw, row, decor),
                2 => rr::<NS, 2, NEAR>(t, bits, near, cs, &r, &pv, &mut c, widths, lw, row, decor),
                3 => rr::<NS, 3, NEAR>(t, bits, near, cs, &r, &pv, &mut c, widths, lw, row, decor),
                4 => rr::<NS, 4, NEAR>(t, bits, near, cs, &r, &pv, &mut c, widths, lw, row, decor),
                5 => rr::<NS, 5, NEAR>(t, bits, near, cs, &r, &pv, &mut c, widths, lw, row, decor),
                _ => rr::<NS, 6, NEAR>(t, bits, near, cs, &r, &pv, &mut c, widths, lw, row, decor),
            }
        }
        // Slots in chunks of at most four (64-bit samples have up to
        // eight); decorrelation touches only the first chunk's color slots.
        let near_on = layout.is_near();
        let mut s0 = 0;
        while s0 < res.len() {
            let n = (res.len() - s0).min(4);
            let (bits, near, cs) = (
                &layout.slot_bits[s0..],
                &layout.near[s0..],
                if s0 == 0 { layout.color_slots } else { 0 },
            );
            let (r, pv, c) = (&res[s0..s0 + n], &prev[s0..s0 + n], &mut cur[s0..s0 + n]);
            let a = (widths, lw, row, pred, decor);
            match (n, near_on) {
                (1, false) => go::<1, false>(t, bits, near, cs, r, pv, c, a),
                (2, false) => go::<2, false>(t, bits, near, cs, r, pv, c, a),
                (3, false) => go::<3, false>(t, bits, near, cs, r, pv, c, a),
                (_, false) => go::<4, false>(t, bits, near, cs, r, pv, c, a),
                (1, true) => go::<1, true>(t, bits, near, cs, r, pv, c, a),
                (2, true) => go::<2, true>(t, bits, near, cs, r, pv, c, a),
                (3, true) => go::<3, true>(t, bits, near, cs, r, pv, c, a),
                (_, true) => go::<4, true>(t, bits, near, cs, r, pv, c, a),
            }
            s0 += n;
        }
        return true;
    }
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    {
        use archmage::SimdToken;
        if !force_scalar() && lw < (1 << 30) && archmage::Wasm128Token::summon().is_some() {
            crate::recon_portable::recon_row32(
                &layout.slot_bits,
                &layout.near,
                layout.color_slots,
                res,
                prev,
                cur,
                widths,
                lw,
                row,
                pred,
                decor,
            );
            return true;
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        use archmage::SimdToken;
        if !force_scalar() && lw < (1 << 30) && archmage::NeonToken::summon().is_some() {
            crate::recon_portable::recon_row32(
                &layout.slot_bits,
                &layout.near,
                layout.color_slots,
                res,
                prev,
                cur,
                widths,
                lw,
                row,
                pred,
                decor,
            );
            return true;
        }
    }
    #[allow(unreachable_code)]
    false
}

#[cfg(feature = "std")]
pub(crate) fn force_scalar() -> bool {
    static F: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *F.get_or_init(|| std::env::var_os("IMIJ_SCALAR").is_some())
}

#[cfg(not(feature = "std"))]
pub(crate) fn force_scalar() -> bool {
    false
}

#[cfg(feature = "std")]
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub(crate) fn force_avx2() -> bool {
    static F: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *F.get_or_init(|| std::env::var_os("IMIJ_NO_AVX512").is_some())
}

#[cfg(not(feature = "std"))]
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub(crate) fn force_avx2() -> bool {
    false
}

/// SIMD reconstruction when available; returns false to use the scalar path.
#[allow(clippy::too_many_arguments)]
#[cfg_attr(not(target_arch = "x86_64"), allow(unused_variables))]
fn recon_row_simd(
    layout: &Layout,
    res: &[Vec<u16>],
    prev: &[Vec<u16>],
    cur: &mut [Vec<u16>],
    widths: &[u32; NL],
    lw: usize,
    row: usize,
    pred: u8,
    decor: u8,
) -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        use archmage::SimdToken;
        if force_scalar() || lw >= 32000 {
            return false;
        }
        let max_bits = layout.slot_bits.iter().copied().max().unwrap_or(0);
        let pred = if pred == predict::P_PAETH && max_bits > 14 {
            crate::recon::avx2::PAETH32
        } else {
            pred
        };
        let Some(t) = archmage::X64V3Token::summon() else {
            return false;
        };
        fn go<const NS: usize, const NEAR: bool>(
            t: archmage::X64V3Token,
            bits: &[u32],
            near: &[u32],
            color_slots: usize,
            res: &[Vec<u16>],
            prev: &[Vec<u16>],
            cur: &mut [Vec<u16>],
            (widths, lw, row, pred, decor): (&[u32; NL], usize, usize, u8, u8),
        ) {
            let r: [&[u16]; NS] = core::array::from_fn(|s| res[s].as_slice());
            let pv: [&[u16]; NS] = core::array::from_fn(|s| prev[s].as_slice());
            let mut it = cur.iter_mut();
            let mut c: [&mut [u16]; NS] =
                core::array::from_fn(|_| it.next().unwrap().as_mut_slice());
            use crate::recon::avx2::recon_row as rr;
            let cs = color_slots;
            match pred {
                0 => rr::<NS, 0, NEAR>(t, bits, near, cs, &r, &pv, &mut c, widths, lw, row, decor),
                1 => rr::<NS, 1, NEAR>(t, bits, near, cs, &r, &pv, &mut c, widths, lw, row, decor),
                2 => rr::<NS, 2, NEAR>(t, bits, near, cs, &r, &pv, &mut c, widths, lw, row, decor),
                3 => rr::<NS, 3, NEAR>(t, bits, near, cs, &r, &pv, &mut c, widths, lw, row, decor),
                4 => rr::<NS, 4, NEAR>(t, bits, near, cs, &r, &pv, &mut c, widths, lw, row, decor),
                5 => rr::<NS, 5, NEAR>(t, bits, near, cs, &r, &pv, &mut c, widths, lw, row, decor),
                6 => rr::<NS, 6, NEAR>(t, bits, near, cs, &r, &pv, &mut c, widths, lw, row, decor),
                _ => rr::<NS, { crate::recon::avx2::PAETH32 }, NEAR>(
                    t, bits, near, cs, &r, &pv, &mut c, widths, lw, row, decor,
                ),
            }
        }
        // Slots in chunks of at most four, as in `recon_row_simd32`.
        let near_on = layout.is_near();
        let mut s0 = 0;
        while s0 < res.len() {
            let n = (res.len() - s0).min(4);
            let (bits, near, cs) = (
                &layout.slot_bits[s0..],
                &layout.near[s0..],
                if s0 == 0 { layout.color_slots } else { 0 },
            );
            let (r, pv, c) = (&res[s0..s0 + n], &prev[s0..s0 + n], &mut cur[s0..s0 + n]);
            let a = (widths, lw, row, pred, decor);
            match (n, near_on) {
                (1, false) => go::<1, false>(t, bits, near, cs, r, pv, c, a),
                (2, false) => go::<2, false>(t, bits, near, cs, r, pv, c, a),
                (3, false) => go::<3, false>(t, bits, near, cs, r, pv, c, a),
                (_, false) => go::<4, false>(t, bits, near, cs, r, pv, c, a),
                (1, true) => go::<1, true>(t, bits, near, cs, r, pv, c, a),
                (2, true) => go::<2, true>(t, bits, near, cs, r, pv, c, a),
                (3, true) => go::<3, true>(t, bits, near, cs, r, pv, c, a),
                (_, true) => go::<4, true>(t, bits, near, cs, r, pv, c, a),
            }
            s0 += n;
        }
        return true;
    }
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    {
        use archmage::SimdToken;
        if !force_scalar() && lw < 32000 && archmage::Wasm128Token::summon().is_some() {
            crate::recon_portable::recon_row16(
                &layout.slot_bits,
                &layout.near,
                layout.color_slots,
                res,
                prev,
                cur,
                widths,
                lw,
                row,
                pred,
                decor,
            );
            return true;
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        use archmage::SimdToken;
        if !force_scalar() && lw < 32000 && archmage::NeonToken::summon().is_some() {
            crate::recon_portable::recon_row16(
                &layout.slot_bits,
                &layout.near,
                layout.color_slots,
                res,
                prev,
                cur,
                widths,
                lw,
                row,
                pred,
                decor,
            );
            return true;
        }
    }
    #[allow(unreachable_code)]
    false
}

/// Inter-predictor row reconstruction with the AVX2 kernel; false when it
/// does not apply (other tiers, 32-bit storage, huge lanes).
#[allow(clippy::too_many_arguments)]
#[cfg_attr(not(target_arch = "x86_64"), allow(unused_variables))]
fn recon_row_inter_simd<S: crate::px::Px>(
    layout: &Layout,
    res: &[Vec<u16>],
    prev: &[Vec<u16>],
    cur: &mut [Vec<u16>],
    refs: &RefRows<S>,
    lw: usize,
    pred: u8,
    decor: u8,
) -> bool {
    let (Some(gcur), Some(gprev)) = (S::rows16(&refs.cur), S::rows16(&refs.prev)) else {
        return false;
    };
    #[cfg(target_arch = "x86_64")]
    {
        use archmage::SimdToken;
        if force_scalar() || lw >= 32000 {
            return false;
        }
        let Some(t) = archmage::X64V3Token::summon() else {
            return false;
        };
        #[allow(clippy::too_many_arguments)]
        fn go<const NS: usize>(
            t: archmage::X64V3Token,
            bits: &[u32],
            color_slots: usize,
            res: &[Vec<u16>],
            prev: &[Vec<u16>],
            cur: &mut [Vec<u16>],
            gcur: &[Vec<u16>],
            gprev: &[Vec<u16>],
            (lw, pred, decor): (usize, u8, u8),
        ) {
            let r: [&[u16]; NS] = core::array::from_fn(|s| res[s].as_slice());
            let pv: [&[u16]; NS] = core::array::from_fn(|s| prev[s].as_slice());
            let mut it = cur.iter_mut();
            let mut c: [&mut [u16]; NS] =
                core::array::from_fn(|_| it.next().unwrap().as_mut_slice());
            let gc: [&[u16]; NS] = core::array::from_fn(|s| gcur[s].as_slice());
            let gp: [&[u16]; NS] = core::array::from_fn(|s| gprev[s].as_slice());
            use crate::recon::avx2::recon_row_inter as rr;
            let cs = color_slots;
            match pred {
                predict::P_REF => {
                    rr::<NS, { predict::P_REF }>(t, bits, cs, &r, &pv, &mut c, &gc, &gp, lw, decor)
                }
                predict::P_REF_N => rr::<NS, { predict::P_REF_N }>(
                    t, bits, cs, &r, &pv, &mut c, &gc, &gp, lw, decor,
                ),
                _ => rr::<NS, { predict::P_REF_W }>(
                    t, bits, cs, &r, &pv, &mut c, &gc, &gp, lw, decor,
                ),
            }
        }
        // Slots in chunks of at most four, as in `recon_row_simd`.
        let mut s0 = 0;
        while s0 < res.len() {
            let n = (res.len() - s0).min(4);
            let (bits, cs) = (
                &layout.slot_bits[s0..],
                if s0 == 0 { layout.color_slots } else { 0 },
            );
            let (r, pv, c) = (&res[s0..s0 + n], &prev[s0..s0 + n], &mut cur[s0..s0 + n]);
            let (gc, gp) = (&gcur[s0..s0 + n], &gprev[s0..s0 + n]);
            let a = (lw, pred, decor);
            match n {
                1 => go::<1>(t, bits, cs, r, pv, c, gc, gp, a),
                2 => go::<2>(t, bits, cs, r, pv, c, gc, gp, a),
                3 => go::<3>(t, bits, cs, r, pv, c, gc, gp, a),
                _ => go::<4>(t, bits, cs, r, pv, c, gc, gp, a),
            }
            s0 += n;
        }
        return true;
    }
    #[allow(unreachable_code)]
    false
}

/// Resumable decoder state for one stripe: decodes one output row per call.
pub(crate) struct StripeState<'a, S: crate::px::Px> {
    core: StripeCore<S>,
    words: Vec<Words<'a>>,
}

/// Resumable arithmetic and row buffers, independent of compressed storage.
struct StripeCore<S: crate::px::Px> {
    rows: usize,
    row: usize,
    /// Lane width of the unit (its column group).
    lw: usize,
    decor: u8,
    preds: Vec<u8>,
    widths: [u32; NL],
    states: Vec<LaneState>,
    res_prev: Vec<Vec<S>>,
    res_cur: Vec<Vec<S>>,
    val_prev: Vec<Vec<S>>,
    val_cur: Vec<Vec<S>>,
    /// Slot 0 of the next row, decoded ahead together with slots 1 and 2.
    res_next0: Vec<S>,
    /// `res_cur[0]` already holds this row's slot 0.
    ahead0: bool,
    /// Palette stripe: one index channel, expanded through `lut`.
    pal: Option<PalState<S>>,
    /// Inter frames: reference rows (coded values) for predictors 7..=9.
    refs: Option<RefRows<S>>,
    /// Bilevel stripe (SPEC 2.10): its decoded rows.
    bl: Option<crate::bilevel::Rows>,
    /// Template palette stripe (SPEC 2.14): its decoded index rows.
    tp: Option<crate::template::Rows>,
}

/// The reference (previous frame) rows an inter stripe predicts from, in
/// coded values per slot, step-major like the decoded rows. Each is read
/// from the output row just before the row is overwritten.
pub(crate) struct RefRows<S> {
    cur: Vec<Vec<S>>,
    prev: Vec<Vec<S>>,
    /// Per stripe row: convert the reference row (the row predicts from it,
    /// or the next row's predictor 8 or 9 reads it as the row above).
    need: Vec<bool>,
}

/// Coded reference values of `out_row` (the previous frame's pixels) per
/// slot, step-major.
fn ref_row<S: crate::px::Px>(
    p: &Parsed<'_>,
    out_row: &[u8],
    widths: &[u32; NL],
    lw: usize,
    dst: &mut [Vec<S>],
) {
    let nch = p.info.channels.count();
    let bps = p.info.sample.bytes();
    for (s, d) in dst.iter_mut().enumerate() {
        let c = p.layout.slot_channel[s];
        let m = &p.layout.maps[c];
        // Integer samples through an Lsb map: the sample, masked.
        let plain =
            m.mode == MapMode::Lsb && matches!(p.info.sample, SampleType::U8 | SampleType::U16);
        let mask = crate::mask_of(m.bits as u32);
        // One-channel Lsb rows: the coded value is the sample masked, so the
        // conversion is a lanes-by-steps transpose of the row's samples.
        #[cfg(target_arch = "x86_64")]
        if plain
            && nch == 1
            && bps == 2
            && lw.is_multiple_of(NL)
            && widths.iter().all(|&w| w as usize == lw)
            && !force_scalar()
            && let Some(t) = <archmage::X64V3Token as archmage::SimdToken>::summon()
            && let Some(d16) = S::slice16_mut(d.as_mut_slice())
        {
            crate::recon::out_avx2::ref_row16(t, out_row, lw, mask as u16, d16);
            continue;
        }
        if plain && nch == 1 && bps == 2 {
            let (row, _) = out_row.as_chunks::<2>();
            for (l, &wl) in widths.iter().enumerate() {
                if wl == 0 {
                    continue;
                }
                let src = &row[l * lw..l * lw + wl as usize];
                for (x, &v) in src.iter().enumerate() {
                    d[x * NL + l] = S::put(u16::from_ne_bytes(v) as u32 & mask);
                }
            }
            continue;
        }
        if plain && nch == 1 && bps == 1 {
            for (l, &wl) in widths.iter().enumerate() {
                let wl = wl as usize;
                for x in 0..wl {
                    d[x * NL + l] = S::put(out_row[l * lw + x] as u32 & mask);
                }
            }
            continue;
        }
        if plain && nch == 3 && bps == 1 {
            let (dst, _) = d.as_chunks_mut::<NL>();
            for (l, &wl) in widths.iter().enumerate() {
                if wl == 0 {
                    continue;
                }
                let start = l * lw * 3;
                let end = start + wl as usize * 3;
                let (pixels, remainder) = out_row[start..end].as_chunks::<3>();
                debug_assert!(remainder.is_empty());
                for (step, pixel) in dst.iter_mut().zip(pixels) {
                    step[l] = S::put(pixel[c] as u32 & mask);
                }
            }
            continue;
        }
        for (l, &wl) in widths.iter().enumerate() {
            let x0 = l * lw;
            for x in 0..wl as usize {
                let raw = read_sample(out_row, (x0 + x) * nch + c, bps);
                d[x * NL + l] = S::put(if plain {
                    raw & mask
                } else {
                    coded_ref(m, p.layout.table(c), p.info.sample, raw)
                });
            }
        }
    }
}

/// Reconstruction of a row with an inter predictor (7..=9), in passes over
/// the step-major rows (16 lanes side by side, so the loops vectorize):
/// residuals, decorrelation, then the prediction. Positions past a lane's
/// width compute values nothing reads. Predictors 8 and 9 never occur on a
/// stripe's first row.
#[allow(clippy::too_many_arguments)]
fn recon_row_inter<S: crate::px::Px>(
    layout: &Layout,
    res: &[Vec<S>],
    prev: &[Vec<S>],
    cur: &mut [Vec<S>],
    refs: &RefRows<S>,
    widths: &[u32; NL],
    pred: u8,
    decor: u8,
) {
    let ns = res.len();
    let cs = layout.color_slots;
    for l in 0..NL {
        let wl = widths[l] as usize;
        for x in 0..wl {
            let i = x * NL + l;
            let mut d = [0u32; MAX_SLOTS];
            for s in 0..ns {
                d[s] = unzigzag(res[s][i].get(), crate::mask_of(layout.slot_bits[s]));
            }
            if cs == 3 && decor != 0 {
                let bits = layout.slot_bits[0];
                decor_inverse(decor, &mut d, bits, crate::mask_of(bits));
            }
            for s in 0..ns {
                let mask = crate::mask_of(layout.slot_bits[s]);
                let g = refs.cur[s][i].get();
                let p = match pred {
                    predict::P_REF => g,
                    predict::P_REF_N => prev[s][i]
                        .get()
                        .wrapping_add(g)
                        .wrapping_sub(refs.prev[s][i].get()),
                    _ => {
                        // W + the reference's horizontal step; at each
                        // lane's first column W = N and the reference's W
                        // is the reference above.
                        let (w, gw) = if x > 0 {
                            (cur[s][i - NL].get(), refs.cur[s][i - NL].get())
                        } else {
                            (prev[s][i].get(), refs.prev[s][i].get())
                        };
                        w.wrapping_add(g).wrapping_sub(gw)
                    }
                };
                cur[s][i] = S::put(p.wrapping_add(d[s]) & mask);
            }
        }
    }
}

/// Decoder state of a palette stripe.
pub(crate) struct PalState<S> {
    /// The entries as one row of `n` pixels in a single lane (step-major).
    entries: Vec<Vec<S>>,
    n: usize,
    /// Output pixel bytes of each index (256 entries; unused = entry 0),
    /// built on the first row.
    tbl: Vec<u8>,
    /// Output bytes per pixel.
    bpp: usize,
    /// Gray output of the scalar index reconstruction (discarded).
    scratch: Vec<u8>,
}

/// Writes one row of palette indices (step-major) as output pixels.
fn expand_row<S: crate::px::Px, const B: usize>(
    tbl: &[u8],
    idx: &[S],
    widths: &[u32; NL],
    lw: usize,
    out_row: &mut [u8],
) {
    let tbl: &[[u8; B]; 256] = tbl.as_chunks::<B>().0.try_into().unwrap();
    let out = out_row.as_chunks_mut::<B>().0;
    for (l, &wl) in widths.iter().enumerate() {
        let wl = wl as usize;
        if wl == 0 {
            break;
        }
        let o = &mut out[l * lw..l * lw + wl];
        for (xl, o) in o.iter_mut().enumerate() {
            *o = tbl[idx[xl * NL + l].get() as usize & 255];
        }
    }
}

/// Output pixel bytes of `n` coded slot tuples (`entries`: one row of `n`
/// pixels in lane 0, step-major), 256 entries; unused ones repeat entry 0.
fn lut<S: crate::px::Px>(om: &OutMap, entries: &[Vec<S>], n: usize, bpp: usize) -> Vec<u8> {
    let mut tbl = vec![0u8; 256 * bpp];
    let mut one = [0u32; NL];
    one[0] = n as u32;
    write_output_row(om, entries, &one, n, &mut tbl[..n * bpp]);
    for i in n..256 {
        tbl.copy_within(0..bpp, i * bpp);
    }
    tbl
}

/// Writes one row of step-major indices through a [`lut`] table.
pub(crate) fn expand<S: crate::px::Px>(
    t: &[u8],
    idx: &[S],
    wd: &[u32; NL],
    lw: usize,
    out_row: &mut [u8],
) {
    match t.len() / 256 {
        1 => expand_row::<S, 1>(t, idx, wd, lw, out_row),
        2 => expand_row::<S, 2>(t, idx, wd, lw, out_row),
        3 => expand_row::<S, 3>(t, idx, wd, lw, out_row),
        4 => expand_row::<S, 4>(t, idx, wd, lw, out_row),
        6 => expand_row::<S, 6>(t, idx, wd, lw, out_row),
        8 => expand_row::<S, 8>(t, idx, wd, lw, out_row),
        12 => expand_row::<S, 12>(t, idx, wd, lw, out_row),
        16 => expand_row::<S, 16>(t, idx, wd, lw, out_row),
        24 => expand_row::<S, 24>(t, idx, wd, lw, out_row),
        32 => expand_row::<S, 32>(t, idx, wd, lw, out_row),
        _ => unreachable!(),
    }
}

impl<'a, S: crate::px::Px> StripeState<'a, S> {
    /// State of unit `u` (stripe `u / ngroups`, column group `u % ngroups`)
    /// from its record bytes.
    pub(crate) fn new_with(p: &Parsed<'_>, u: usize, bytes: &'a [u8]) -> Result<Self, Error> {
        let ng = p.ngroups();
        let (y0, y1, gi) = p.unit_rectangles.as_ref().map_or_else(
            || {
                let y0 = u / ng * p.hs;
                (y0, (y0 + p.hs).min(p.info.height as usize), u % ng)
            },
            |v| v[u],
        );
        Self::from_shape(p, gi, y1 - y0, bytes)
    }

    /// Geometry supplied by a checked coverage description, without a
    /// positional unit array or uniform-height ordinal calculation.
    pub(crate) fn new_span(
        p: &Parsed<'_>,
        gi: usize,
        rows: usize,
        bytes: &'a [u8],
    ) -> Result<Self, Error> {
        if gi >= p.ngroups() || rows == 0 || rows > p.hs || rows > p.info.height as usize {
            return Err(Error::Corrupt("numeric span geometry"));
        }
        Self::from_shape(p, gi, rows, bytes)
    }

    fn from_shape(p: &Parsed<'_>, gi: usize, rows: usize, bytes: &'a [u8]) -> Result<Self, Error> {
        Self::from_shape_inter(p, gi, rows, bytes, p.inter)
    }

    fn from_shape_inter(
        p: &Parsed<'_>,
        gi: usize,
        rows: usize,
        bytes: &'a [u8],
        inter: bool,
    ) -> Result<Self, Error> {
        let (_, w, lw) = p.group(gi);
        let ns = p.layout.slot_channel.len();
        let widths = lane_widths(w, lw);
        let mut r = ByteReader::new(bytes);
        let decor = crate::trace::tfd!("color decorrelation", r, r.u8()?, |v: &u8| {
            crate::trace::describe::decor(*v)
        });
        let mut pal = None;
        let bl_pal = decor == crate::bilevel::PAL;
        let tp = decor == crate::template::STRIPE || decor == crate::template::SHARED;
        if decor == PAL_STRIPE || bl_pal || decor == crate::template::STRIPE {
            if decor == PAL_STRIPE && p.pal.is_none() {
                return Err(Error::Corrupt("palette stripe without a palette model"));
            }
            let n = crate::trace::tf!("palette entries - 1", r, r.u8()?) as usize + 1;
            if bl_pal && n > 2 {
                return Err(Error::Corrupt("bilevel palette with more than two entries"));
            }
            #[cfg(all(feature = "lab", feature = "std"))]
            let pal_at = r.mark();
            // The entries as one row of n pixels (a single lane), written
            // through the regular output writer.
            let mut lut = vec![vec![S::default(); n * NL]; ns];
            for i in 0..n {
                for (sl, l) in lut.iter_mut().enumerate() {
                    let bits = p.layout.slot_bits[sl];
                    let v = if bits <= 8 {
                        r.u8()? as u32
                    } else if bits <= 16 {
                        r.u16()? as u32
                    } else {
                        return Err(Error::Corrupt("palette on a 32-bit channel"));
                    };
                    if v >> bits != 0 {
                        return Err(Error::Corrupt("palette entry out of range"));
                    }
                    l[i * NL] = S::put(v);
                }
            }
            #[cfg(all(feature = "lab", feature = "std"))]
            if crate::trace::on() {
                crate::trace::field_addr(
                    "palette",
                    alloc::format!("{n} entries"),
                    pal_at,
                    r.mark(),
                );
            }
            pal = Some(PalState {
                entries: lut,
                n,
                tbl: Vec::new(),
                bpp: p.bpp(),
                scratch: vec![0u8; w],
            });
        } else if decor == crate::template::SHARED {
            let t = p.template.as_ref().ok_or(Error::Corrupt(
                "template palette stripe without template tables",
            ))?;
            let sh = t.shared.as_ref().ok_or(Error::Corrupt(
                "shared template palette stripe without shared entries",
            ))?;
            let n = sh[0].len();
            let entries = sh
                .iter()
                .map(|v| {
                    let mut l = vec![S::default(); n * NL];
                    for (i, &e) in v.iter().enumerate() {
                        l[i * NL] = S::put(e);
                    }
                    l
                })
                .collect();
            pal = Some(PalState {
                entries,
                n,
                tbl: Vec::new(),
                bpp: p.bpp(),
                scratch: Vec::new(),
            });
        } else if decor != crate::bilevel::STRIPE
            && (decor >= crate::coding_layout::NUM_DECOR || (decor != 0 && !p.layout.decor_ok))
        {
            return Err(Error::Corrupt("bad decorrelation mode"));
        }
        if tp {
            if p.template.is_none() {
                return Err(Error::Corrupt(
                    "template palette stripe without template tables",
                ));
            }
            let n = pal.as_ref().map_or(1, |ps: &PalState<S>| ps.n);
            let body = r.bytes(r.remaining())?;
            let (typical, rest) = crate::template::split_rows(body, rows)?;
            #[cfg(all(feature = "lab", feature = "std"))]
            if crate::trace::on() {
                let a = body.as_ptr() as usize;
                let repeated = typical.iter().filter(|&&t| t).count();
                crate::trace::enter_addr(
                    "payload",
                    "2.14",
                    alloc::format!("template palette body ({n} entries)"),
                    crate::trace::Mark::Addr(a),
                );
                crate::trace::field_addr(
                    "typical rows (bit r: row r repeats the row above)",
                    alloc::format!("{repeated} of {rows} rows repeat"),
                    crate::trace::Mark::Addr(a),
                    crate::trace::Mark::Addr(a + rows.div_ceil(8)),
                );
                crate::trace::lane_stream(
                    &alloc::format!("palette index plane ({n} entries, template contexts)"),
                    rest,
                    active_lanes(&widths),
                );
                crate::trace::exit_addr(crate::trace::Mark::Addr(a + body.len()));
            }
            let (wd, x) = Words::from_bytes(rest, active_lanes(&widths))?;
            return Ok(Self {
                words: vec![wd],
                core: StripeCore {
                    rows,
                    row: 0,
                    lw,
                    decor,
                    preds: Vec::new(),
                    widths,
                    states: vec![LaneState {
                        x,
                        run: [0; NL],
                        wp: 0,
                    }],
                    res_prev: Vec::new(),
                    res_cur: Vec::new(),
                    val_prev: Vec::new(),
                    val_cur: Vec::new(),
                    res_next0: Vec::new(),
                    ahead0: false,
                    pal,
                    refs: None,
                    bl: None,
                    tp: Some(crate::template::Rows::new(lw, w, n, typical)),
                },
            });
        }
        if decor == crate::bilevel::STRIPE || bl_pal {
            if p.bilevel.is_none() || (!bl_pal && p.layout.slot_bits[..] != [1]) {
                return Err(Error::Corrupt(
                    "bilevel stripe without a bilevel table or 1-bit slot",
                ));
            }
            let body = r.bytes(r.remaining())?;
            #[cfg(all(feature = "lab", feature = "std"))]
            if crate::trace::on() {
                let a = body.as_ptr() as usize;
                crate::trace::enter_addr(
                    "payload",
                    "2.10",
                    alloc::string::String::from("bilevel body"),
                    crate::trace::Mark::Addr(a),
                );
                crate::trace::lane_stream(
                    "bilevel values (template contexts)",
                    body,
                    active_lanes(&widths),
                );
                crate::trace::exit_addr(crate::trace::Mark::Addr(a + body.len()));
            }
            let (wd, x) = Words::from_bytes(body, active_lanes(&widths))?;
            return Ok(Self {
                words: vec![wd],
                core: StripeCore {
                    rows,
                    row: 0,
                    lw,
                    decor,
                    preds: Vec::new(),
                    widths,
                    states: vec![LaneState {
                        x,
                        run: [0; NL],
                        wp: 0,
                    }],
                    res_prev: Vec::new(),
                    res_cur: Vec::new(),
                    val_prev: Vec::new(),
                    val_cur: Vec::new(),
                    res_next0: Vec::new(),
                    ahead0: false,
                    pal,
                    refs: None,
                    bl: Some(crate::bilevel::Rows::new(lw, w)),
                    tp: None,
                },
            });
        }
        // A palette stripe codes one channel (the indices).
        let ns = if pal.is_some() { 1 } else { ns };
        let pb = crate::trace::tfd!(
            "row predictors",
            r,
            r.bytes(rows.div_ceil(2))?,
            |b: &&[u8]| { crate::trace::describe::predictors(b, rows) }
        );
        let preds: Vec<u8> = (0..rows)
            .map(|i| (pb[i / 2] >> (4 * (i % 2))) & 15)
            .collect();
        // Inter predictors read the previous frame as coded values of at
        // most 32 bits; 64-bit samples and near-lossless images never use
        // them (SPEC 4, 2.6).
        let inter = inter && pal.is_none() && p.info.sample.words() == 1 && !p.layout.is_near();
        let npred = if inter {
            predict::NUM_PREDICTORS_INTER
        } else {
            predict::NUM_PREDICTORS
        };
        let first_ok = preds[0] == predict::P_W || (inter && preds[0] == predict::P_REF);
        if preds.iter().any(|&q| q >= npred) || !first_ok {
            return Err(Error::Corrupt("bad predictor"));
        }
        let refs = (pal.is_none() && preds.iter().any(|&q| q >= predict::P_REF)).then(|| {
            let need = (0..rows)
                .map(|r| {
                    // Predictors 8 and 9 read the reference row above
                    // (8 at each lane's first column).
                    preds[r] >= predict::P_REF
                        || preds.get(r + 1).is_some_and(|&q| q >= predict::P_REF_W)
                })
                .collect();
            RefRows {
                cur: vec![vec![S::default(); lw * NL]; ns],
                prev: vec![vec![S::default(); lw * NL]; ns],
                need,
            }
        });
        let mut lens = Vec::with_capacity(ns);
        for _ in 0..ns {
            lens.push(crate::trace::tf!("slot stream length", r, r.var_usize()?));
        }
        let mut words = Vec::with_capacity(ns);
        let mut states = Vec::with_capacity(ns);
        for (slot, &len) in lens.iter().enumerate() {
            let b = r.bytes(len)?;
            #[cfg(all(feature = "lab", feature = "std"))]
            crate::trace::lane_stream(
                &if pal.is_some() {
                    alloc::format!("slot {slot} (palette index plane, 8-bit)")
                } else {
                    alloc::format!(
                        "slot {slot} ({}, {}-bit)",
                        p.slot_names[slot],
                        p.layout.slot_bits[slot]
                    )
                },
                b,
                active_lanes(&widths),
            );
            #[cfg(not(all(feature = "lab", feature = "std")))]
            let _ = slot;
            let (wd, x) = Words::from_bytes(b, active_lanes(&widths))?;
            words.push(wd);
            states.push(LaneState {
                x,
                run: [0; NL],
                wp: 0,
            });
        }
        let step_row = lw * NL;
        let buf = || -> Vec<Vec<S>> { vec![vec![S::default(); step_row]; ns] };
        Ok(Self {
            words,
            core: StripeCore {
                rows,
                row: 0,
                lw,
                decor,
                preds,
                widths,
                states,
                res_prev: buf(),
                res_cur: buf(),
                val_prev: buf(),
                val_cur: buf(),
                res_next0: vec![S::default(); step_row],
                ahead0: false,
                pal,
                refs,
                bl: None,
                tp: None,
            },
        })
    }

    pub(crate) fn next_row(
        &mut self,
        p: &Parsed<'_>,
        om: &OutMap,
        out: &mut [u8],
    ) -> Result<(), Error> {
        self.core.next_row(&self.words, p, om, out)
    }
}

impl<S: crate::px::Px> StripeCore<S> {
    /// Decodes the next row of the stripe into `out_row` (row_bytes long).
    pub(crate) fn next_row(
        &mut self,
        words: &[Words<'_>],
        p: &Parsed<'_>,
        om: &OutMap,
        out_row: &mut [u8],
    ) -> Result<(), Error> {
        if let (Some(bl), Some(t)) = (&mut self.bl, &p.bilevel) {
            if bl.tbl.is_empty() {
                // Output pixels of values 0 and 1: the stripe's palette
                // entries or the coded values; `inv` swaps them.
                let mut v = vec![S::default(); 2 * NL];
                v[NL] = S::put(1);
                bl.tbl = match &self.pal {
                    Some(ps) => lut(om, &ps.entries, ps.n, ps.bpp),
                    None => lut(om, &[v], 2, p.bpp()),
                };
                if t.inv {
                    let b = p.bpp();
                    let (e0, e1) = bl.tbl.split_at_mut(b);
                    e0.swap_with_slice(&mut e1[..b]);
                }
            }
            bl.decode(
                t,
                &words[0],
                &mut self.states[0],
                &self.widths,
                self.lw,
                out_row,
            );
            self.row += 1;
            if self.row == self.rows {
                if self.states[0].wp > words[0].len {
                    return Err(Error::Corrupt("lane stream overrun"));
                }
                check_runs_ended(&self.states[0])?;
            }
            return Ok(());
        }
        if let (Some(tp), Some(t)) = (&mut self.tp, &p.template) {
            if tp.tbl.is_empty() {
                let ps = self.pal.as_ref().unwrap();
                tp.tbl = lut(om, &ps.entries, ps.n, ps.bpp);
            }
            tp.decode(
                t,
                &words[0],
                &mut self.states[0],
                &self.widths,
                self.lw,
                out_row,
            );
            self.row += 1;
            if self.row == self.rows {
                if self.states[0].wp > words[0].len {
                    return Err(Error::Corrupt("lane stream overrun"));
                }
                check_runs_ended(&self.states[0])?;
            }
            return Ok(());
        }
        if self.pal.is_some() {
            return self.next_row_pal(words, p, om, out_row);
        }
        let row = self.row;
        debug_assert!(row < self.rows);
        let lw = self.lw;
        let ns = words.len();
        // Slot 0 of this row was decoded ahead by the previous row.
        let mut s = self.ahead0 as usize;
        let mut ahead_next = false;
        while s < ns {
            // Slots 1 and 2 of this row with slot 0 of the next row (which
            // reads only this row's slot 0).
            if s == 1 && ns >= 3 && row + 1 < self.rows {
                let (head, rest) = self.res_cur.split_at_mut(1);
                let (r1, r2) = rest.split_at_mut(1);
                let c = |k: usize| p.models[k].cross.then(|| head[0].as_slice());
                let (s0, s12) = self.states.split_at_mut(1);
                let (s1, s2) = s12.split_at_mut(1);
                if decode_row_triple(
                    [&p.models[1], &p.models[2], &p.models[0]],
                    [&words[1], &words[2], &words[0]],
                    [&mut s1[0], &mut s2[0], &mut s0[0]],
                    &self.widths,
                    lw,
                    [row, row, row + 1],
                    [&self.res_prev[1], &self.res_prev[2], &head[0]],
                    [c(1), c(2), None],
                    [&mut r1[0], &mut r2[0], &mut self.res_next0],
                ) {
                    ahead_next = true;
                    s = 3;
                    continue;
                }
            }
            // Slots after the first read only slot 0 (cross channel), never
            // each other: decode two of them together when a paired kernel
            // applies.
            if s >= 1 && s + 1 < ns {
                let (head, rest) = self.res_cur.split_at_mut(s);
                let (ra, rb) = rest.split_at_mut(1);
                let c = |k: usize| p.models[k].cross.then(|| head[0].as_slice());
                let (sa, sb) = self.states.split_at_mut(s + 1);
                if decode_row_pair(
                    [&p.models[s], &p.models[s + 1]],
                    [&words[s], &words[s + 1]],
                    [&mut sa[s], &mut sb[0]],
                    &self.widths,
                    lw,
                    row,
                    [&self.res_prev[s], &self.res_prev[s + 1]],
                    [c(s), c(s + 1)],
                    [&mut ra[0], &mut rb[0]],
                ) {
                    s += 2;
                    continue;
                }
            }
            let (before, rest) = self.res_cur.split_at_mut(s);
            let cross = if p.models[s].cross {
                Some(before[0].as_slice())
            } else {
                None
            };
            decode_row(
                &p.models[s],
                &words[s],
                &mut self.states[s],
                &self.widths,
                lw,
                row,
                &self.res_prev[s],
                cross,
                &mut rest[0],
            );
            s += 1;
        }
        let (pred, decor) = (self.preds[row], self.decor);
        if let Some(rf) = &mut self.refs
            && rf.need[row]
        {
            // The output row still holds the previous frame: read it before
            // overwriting it (the next row's predictor 9 needs it too).
            core::mem::swap(&mut rf.cur, &mut rf.prev);
            ref_row(p, out_row, &self.widths, lw, &mut rf.cur);
        }
        if pred >= predict::P_REF {
            let rf = self.refs.as_ref().unwrap();
            let simd = match (S::rows16(&self.res_cur), S::rows16(&self.val_prev)) {
                (Some(r), Some(vp)) => {
                    let vc = S::rows16_mut(&mut self.val_cur).unwrap();
                    recon_row_inter_simd(&p.layout, r, vp, vc, rf, lw, pred, decor)
                }
                _ => false,
            };
            if !simd {
                recon_row_inter(
                    &p.layout,
                    &self.res_cur,
                    &self.val_prev,
                    &mut self.val_cur,
                    rf,
                    &self.widths,
                    pred,
                    decor,
                );
            }
            write_output_row(om, &self.val_cur, &self.widths, lw, out_row);
            return self.finish_row(words, ahead_next);
        }
        let simd = match (S::rows16(&self.res_cur), S::rows16(&self.val_prev)) {
            (Some(r), Some(vp)) => {
                let vc = S::rows16_mut(&mut self.val_cur).unwrap();
                recon_row_simd(&p.layout, r, vp, vc, &self.widths, lw, row, pred, decor)
            }
            _ => match (S::rows32(&self.res_cur), S::rows32(&self.val_prev)) {
                (Some(r), Some(vp)) => {
                    let vc = S::rows32_mut(&mut self.val_cur).unwrap();
                    recon_row_simd32(&p.layout, r, vp, vc, &self.widths, lw, row, pred, decor)
                }
                _ => false,
            },
        };
        if !simd {
            recon_row_scalar(
                &p.layout,
                &self.res_cur,
                &self.val_prev,
                &mut self.val_cur,
                &self.widths,
                lw,
                row,
                pred,
                decor,
                om,
                out_row,
            );
        } else {
            write_output_row(om, &self.val_cur, &self.widths, lw, out_row);
        }
        self.finish_row(words, ahead_next)
    }

    /// End of a row: rotate the row buffers; overrun check after the last.
    fn finish_row(&mut self, words: &[Words<'_>], ahead_next: bool) -> Result<(), Error> {
        core::mem::swap(&mut self.res_prev, &mut self.res_cur);
        core::mem::swap(&mut self.val_prev, &mut self.val_cur);
        if ahead_next {
            core::mem::swap(&mut self.res_cur[0], &mut self.res_next0);
        }
        self.ahead0 = ahead_next;
        self.row += 1;
        if self.row == self.rows {
            for (s, st) in self.states.iter().enumerate() {
                if st.wp > words[s].len {
                    return Err(Error::Corrupt("lane stream overrun"));
                }
                check_runs_ended(st)?;
            }
        }
        Ok(())
    }
}

impl<S: crate::px::Px> StripeCore<S> {
    /// [`StripeState::next_row`] of a palette stripe: the index channel
    /// through the palette model and the row predictors, expanded to the
    /// slots through the stripe's palette, then the regular output writer.
    fn next_row_pal(
        &mut self,
        words: &[Words<'_>],
        p: &Parsed<'_>,
        om: &OutMap,
        out_row: &mut [u8],
    ) -> Result<(), Error> {
        let row = self.row;
        let lw = self.lw;
        let pm = p.pal.as_ref().unwrap();
        decode_row(
            &pm.model,
            &words[0],
            &mut self.states[0],
            &self.widths,
            lw,
            row,
            &self.res_prev[0],
            None,
            &mut self.res_cur[0],
        );
        let pred = self.preds[row];
        let ps = self.pal.as_mut().unwrap();
        let simd = match (S::rows16(&self.res_cur), S::rows16(&self.val_prev)) {
            (Some(r), Some(vp)) => {
                let vc = S::rows16_mut(&mut self.val_cur).unwrap();
                recon_row_simd(&pm.layout, r, vp, vc, &self.widths, lw, row, pred, 0)
            }
            _ => false,
        };
        if !simd {
            recon_row_scalar(
                &pm.layout,
                &self.res_cur,
                &self.val_prev,
                &mut self.val_cur,
                &self.widths,
                lw,
                row,
                pred,
                0,
                &pm.om,
                &mut ps.scratch,
            );
        }
        if ps.tbl.is_empty() {
            ps.tbl = lut(om, &ps.entries, ps.n, ps.bpp);
        }
        expand(&ps.tbl, &self.val_cur[0], &self.widths, lw, out_row);
        core::mem::swap(&mut self.res_prev, &mut self.res_cur);
        core::mem::swap(&mut self.val_prev, &mut self.val_cur);
        self.row += 1;
        if self.row == self.rows {
            if self.states[0].wp > words[0].len {
                return Err(Error::Corrupt("lane stream overrun"));
            }
            check_runs_ended(&self.states[0])?;
        }
        Ok(())
    }
}

/// After a unit's last row no lane may still be inside a zero run: a run
/// token covers only positions of its own lane in its unit (SPEC 3.1).
fn check_runs_ended(st: &LaneState) -> Result<(), Error> {
    if st.run.iter().any(|&r| r != 0) {
        return Err(Error::Corrupt("run past the end of its lane"));
    }
    Ok(())
}

/// Where a unit's decoded rows go ([`decode_unit_rows`]): the buffer for
/// row `i` of the unit (its column group's width), then a call once the row
/// is written. One generic driver serves every lossless route; the sinks
/// compile to plain slice arithmetic.
pub(crate) trait UnitSink {
    /// The rows to decode of a unit of `rows` rows (all, by default).
    #[inline]
    fn rows(&self, rows: usize) -> usize {
        rows
    }
    /// The output for row `i`.
    fn row(&mut self, i: usize) -> &mut [u8];
    /// Row `i` is written.
    #[inline]
    fn done(&mut self, _i: usize) -> Result<(), Error> {
        Ok(())
    }
}

/// Rows `stride` bytes apart in `out`, `n` bytes each (the first is the
/// unit's first row, starting at its column group's first column).
pub(crate) struct Strided<'o> {
    pub out: &'o mut [u8],
    pub stride: usize,
    pub n: usize,
}

impl UnitSink for Strided<'_> {
    #[inline]
    fn row(&mut self, i: usize) -> &mut [u8] {
        &mut self.out[i * self.stride..][..self.n]
    }
}

/// The unit's rows as slices.
#[cfg(feature = "std")]
struct Slices<'s, 'o>(&'s mut [&'o mut [u8]]);

#[cfg(feature = "std")]
impl UnitSink for Slices<'_, '_> {
    #[inline]
    fn row(&mut self, i: usize) -> &mut [u8] {
        self.0[i]
    }
}

/// One row buffer and a call after every row (streamed decoding).
pub(crate) struct Each<'r, F> {
    pub row: &'r mut [u8],
    pub each: F,
}

impl<F: FnMut(&[u8]) -> Result<(), Error>> UnitSink for Each<'_, F> {
    #[inline]
    fn row(&mut self, _: usize) -> &mut [u8] {
        self.row
    }
    #[inline]
    fn done(&mut self, _: usize) -> Result<(), Error> {
        (self.each)(self.row)
    }
}

/// A rectangle of a unit (region decoding): rows `first..last` of it,
/// `col` bytes into each, copied into `rows`; every row up to `last`
/// decodes into `buf` (a row of the unit).
#[cfg(feature = "v3")]
struct Crop<'s, 'o> {
    buf: Vec<u8>,
    first: usize,
    last: usize,
    col: usize,
    rows: &'s mut [&'o mut [u8]],
}

#[cfg(feature = "v3")]
impl UnitSink for Crop<'_, '_> {
    #[inline]
    fn rows(&self, rows: usize) -> usize {
        self.last.min(rows)
    }
    #[inline]
    fn row(&mut self, _: usize) -> &mut [u8] {
        &mut self.buf
    }
    #[inline]
    fn done(&mut self, i: usize) -> Result<(), Error> {
        if i >= self.first {
            let dst = &mut self.rows[i - self.first];
            let n = dst.len();
            dst.copy_from_slice(&self.buf[self.col..self.col + n]);
        }
        Ok(())
    }
}

/// Decodes unit `u` (stripe, column group) from its record `bytes` into
/// `sink`, choosing the sample width once (u16 when every coded channel
/// fits 16 bits, else u32); `check` runs before every row (cancellation).
pub(crate) fn decode_unit_rows<K: UnitSink>(
    p: &Parsed<'_>,
    om: &OutMap,
    u: usize,
    bytes: &[u8],
    sink: &mut K,
    check: &impl Fn() -> Result<(), Error>,
) -> Result<(), Error> {
    if p.quick {
        let mut state = crate::quick::State::new(p, u, bytes)?;
        for row in 0..sink.rows(state.rows()) {
            check()?;
            state.next(bytes, sink.row(row))?;
            sink.done(row)?;
        }
        return Ok(());
    }
    fn go<S: crate::px::Px, K: UnitSink>(
        p: &Parsed<'_>,
        om: &OutMap,
        u: usize,
        bytes: &[u8],
        sink: &mut K,
        check: &impl Fn() -> Result<(), Error>,
    ) -> Result<(), Error> {
        let mut st = StripeState::<S>::new_with(p, u, bytes)?;
        for i in 0..sink.rows(st.core.rows) {
            check()?;
            st.next_row(p, om, sink.row(i))?;
            sink.done(i)?;
        }
        Ok(())
    }
    #[cfg(any(feature = "extended", feature = "encode"))]
    if !p.narrow() {
        return go::<u32, K>(p, om, u, bytes, sink, check);
    }
    go::<u16, K>(p, om, u, bytes, sink, check)
}

/// Decodes stripe `si` into `out`, whose first row is the stripe's first
/// row: its column groups one after the other (each unit's state stays
/// hot); `check` runs before every unit and row (cancellation).
pub(crate) fn decode_stripe(
    p: &Parsed<'_>,
    si: usize,
    om: &OutMap,
    out: &mut [u8],
    stride: usize,
    check: &impl Fn() -> Result<(), Error>,
) -> Result<(), Error> {
    let ng = p.ngroups();
    for gi in 0..ng {
        let x = p.group(gi).0 * p.bpp();
        decode_unit_into_checked(p, si * ng + gi, om, &mut out[x..], stride, check)?;
    }
    Ok(())
}

/// Every unit's geometry, in unit order, for [`crate::progress::UnitProgress`].
pub(crate) fn unit_shapes<'p>(
    p: &'p Parsed<'_>,
) -> impl ExactSizeIterator<Item = crate::progress::UnitShape> + 'p {
    let (ns, ng, h) = (p.stripe_count(), p.ngroups(), p.info.height as usize);
    (0..ns * ng).map(move |u| {
        let (si, gi) = (u / ng, u % ng);
        let (x, cols, _) = p.group(gi);
        let y = si * p.hs;
        crate::progress::UnitShape {
            stripe: si,
            group: gi,
            groups: ng,
            y,
            rows: p.hs.min(h - y),
            x,
            cols,
        }
    })
}

/// [`decode_stripe`] (or the quick stripe copy) unit by unit, reporting each
/// unit to `units`.
#[cfg(feature = "std")]
fn decode_stripe_observed(
    p: &Parsed<'_>,
    si: usize,
    om: &OutMap,
    out: &mut [u8],
    stride: usize,
    units: &crate::progress::UnitProgress<'_>,
) -> Result<(), Error> {
    let ng = p.ngroups();
    if p.quick && ng == 1 {
        let r = units
            .begin(si)
            .and_then(|()| quick_stripe_into(p, si, om, out, stride));
        units.end(si, &r);
        return r;
    }
    for gi in 0..ng {
        let u = si * ng + gi;
        let x = p.group(gi).0 * p.bpp();
        let r = units
            .begin(u)
            .and_then(|()| decode_unit_into_checked(p, u, om, &mut out[x..], stride, &|| Ok(())));
        units.end(u, &r);
        r?;
    }
    Ok(())
}

/// [`decode_parsed_into_checked`] reporting each unit to a live `pulse`.
pub(crate) fn decode_parsed_into_observed(
    p: Parsed<'_>,
    out: &mut [u8],
    stride: usize,
    check: &impl Fn() -> Result<(), Error>,
    pulse: &dyn Pulse,
) -> Result<ImageInfo, Error> {
    let Some(units) = crate::progress::UnitProgress::plan(pulse, unit_shapes(&p), 1) else {
        return decode_parsed_into_checked(p, out, stride, check);
    };
    let extent = p
        .info
        .checked_stride_len(stride)
        .ok_or(Error::InvalidInput("invalid output layout"))?;
    if out.len() < extent {
        return Err(Error::InvalidInput("output buffer too small"));
    }
    let om = OutMap::new(&p.info, &p.layout);
    let ng = p.ngroups();
    for si in 0..p.stripe_count() {
        let base = si * p.hs * stride;
        for gi in 0..ng {
            let u = si * ng + gi;
            let x = p.group(gi).0 * p.bpp();
            let r = units.begin(u).and_then(|()| {
                decode_unit_into_checked(&p, u, &om, &mut out[base + x..], stride, check)
            });
            units.end(u, &r);
            r?;
        }
    }
    Ok(p.info)
}

/// Decodes unit `u` (stripe, column group) into `out` (rows of `stride`
/// bytes; the first row is the unit's first row, the first column its
/// group's first column).
pub(crate) fn decode_unit_into_checked(
    p: &Parsed<'_>,
    u: usize,
    om: &OutMap,
    out: &mut [u8],
    stride: usize,
    check: &impl Fn() -> Result<(), Error>,
) -> Result<(), Error> {
    check()?;
    let (_, gw, _) = p.group(u % p.ngroups());
    let mut sink = Strided {
        out,
        stride,
        n: gw * p.bpp(),
    };
    decode_unit_rows(p, om, u, p.stripes[u], &mut sink, check)
}

/// Decodes unit `u` from `bytes`, one row of its group's width at a time
/// into `row`, calling `each` after every row; `check` runs before every
/// row (cancellation).
pub(crate) fn unit_rows(
    p: &Parsed<'_>,
    om: &OutMap,
    u: usize,
    bytes: &[u8],
    row: &mut [u8],
    each: impl FnMut(&[u8]) -> Result<(), Error>,
    check: &impl Fn() -> Result<(), Error>,
) -> Result<(), Error> {
    let gi = p
        .unit_rectangles
        .as_ref()
        .map_or(u % p.ngroups(), |v| v[u].2);
    let (_, gw, _) = p.group(gi);
    let mut sink = Each {
        row: &mut row[..gw * p.bpp()],
        each,
    };
    decode_unit_rows(p, om, u, bytes, &mut sink, check)
}

/// Reconstruct a frame rectangle from its caller-owned previous pixels.
/// Reference rows are captured before the corresponding destination changes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn rectangle_into_reference(
    p: &Parsed<'_>,
    om: &OutMap,
    group: usize,
    rows: usize,
    bytes: &[u8],
    row: &mut [u8],
    output: &mut [u8],
    stride: usize,
    check: &impl Fn() -> Result<(), Error>,
) -> Result<(), Error> {
    if group >= p.ngroups() || rows == 0 || rows > p.hs || rows > p.info.height as usize {
        return Err(Error::Corrupt("numeric rectangle geometry"));
    }
    let width = p.group(group).1;
    let n = width.checked_mul(p.bpp()).ok_or(Error::TooLarge)?;
    let extent = (rows - 1)
        .checked_mul(stride)
        .and_then(|v| v.checked_add(n))
        .ok_or(Error::TooLarge)?;
    if row.len() < n || stride < n || output.len() < extent {
        return Err(Error::InvalidInput("numeric reference row extent"));
    }
    if p.quick {
        let mut state = crate::quick::State::new_rectangle(p, width, rows, bytes)?;
        for y in 0..rows {
            check()?;
            state.next(bytes, &mut row[..n])?;
            output[y * stride..y * stride + n].copy_from_slice(&row[..n]);
        }
        return Ok(());
    }
    #[allow(clippy::too_many_arguments)]
    fn go<S: crate::px::Px>(
        p: &Parsed<'_>,
        om: &OutMap,
        group: usize,
        rows: usize,
        bytes: &[u8],
        row: &mut [u8],
        output: &mut [u8],
        stride: usize,
        check: &impl Fn() -> Result<(), Error>,
    ) -> Result<(), Error> {
        let n = p
            .group(group)
            .1
            .checked_mul(p.bpp())
            .ok_or(Error::TooLarge)?;
        let mut state = StripeState::<S>::from_shape_inter(p, group, rows, bytes, true)?;
        for y in 0..rows {
            check()?;
            row[..n].copy_from_slice(&output[y * stride..y * stride + n]);
            state.next_row(p, om, &mut row[..n])?;
            output[y * stride..y * stride + n].copy_from_slice(&row[..n]);
        }
        Ok(())
    }
    #[cfg(any(feature = "extended", feature = "encode"))]
    if !p.narrow() {
        return go::<u32>(p, om, group, rows, bytes, row, output, stride, check);
    }
    go::<u16>(p, om, group, rows, bytes, row, output, stride, check)
}

/// Decode an explicitly shaped numeric rectangle. The caller validates its
/// origin/coverage; every row uses the ordinary independent-unit arithmetic.
pub(crate) fn rectangle_rows(
    p: &Parsed<'_>,
    om: &OutMap,
    group: usize,
    rows: usize,
    bytes: &[u8],
    row: &mut [u8],
    mut each: impl FnMut(&[u8]) -> Result<(), Error>,
    check: &impl Fn() -> Result<(), Error>,
) -> Result<(), Error> {
    if group >= p.ngroups() || rows == 0 || rows > p.hs || rows > p.info.height as usize {
        return Err(Error::Corrupt("numeric rectangle geometry"));
    }
    let width = p.group(group).1;
    let row_bytes = width.checked_mul(p.bpp()).ok_or(Error::TooLarge)?;
    if row.len() < row_bytes {
        return Err(Error::InvalidInput("numeric rectangle row extent"));
    }
    let row = &mut row[..row_bytes];
    if p.quick {
        let mut state = crate::quick::State::new_rectangle(p, width, rows, bytes)?;
        for _ in 0..rows {
            check()?;
            state.next(bytes, row)?;
            each(row)?;
        }
        return Ok(());
    }
    fn go<S: crate::px::Px>(
        p: &Parsed<'_>,
        om: &OutMap,
        group: usize,
        rows: usize,
        bytes: &[u8],
        row: &mut [u8],
        mut each: impl FnMut(&[u8]) -> Result<(), Error>,
        check: &impl Fn() -> Result<(), Error>,
    ) -> Result<(), Error> {
        let mut state = StripeState::<S>::new_span(p, group, rows, bytes)?;
        for _ in 0..rows {
            check()?;
            state.next_row(p, om, row)?;
            each(row)?;
        }
        Ok(())
    }
    #[cfg(any(feature = "extended", feature = "encode"))]
    if !p.narrow() {
        return go::<u32>(p, om, group, rows, bytes, row, each, check);
    }
    go::<u16>(p, om, group, rows, bytes, row, each, check)
}

/// Decodes unit `u` (stripe, column group) into `rows`, its row slices.
#[cfg(feature = "std")]
fn decode_unit(p: &Parsed<'_>, u: usize, om: &OutMap, rows: &mut [&mut [u8]]) -> Result<(), Error> {
    decode_unit_rows(p, om, u, p.stripes[u], &mut Slices(rows), &|| Ok(()))
}

/// Stripe state in the storage width of the image.
#[cfg_attr(
    not(any(feature = "extended", feature = "encode")),
    allow(
        clippy::large_enum_variant,
        reason = "keep stripe state inline after wide variant removal"
    )
)]
enum AnyStripe<'a> {
    Quick(alloc::boxed::Box<crate::quick::State>, &'a [u8]),
    S16(StripeState<'a, u16>),
    // Baseline-only readers admit at most 16 coded bits. Omit the variant so
    // row dispatch cannot retain the wide kernel in those builds.
    #[cfg(any(feature = "extended", feature = "encode"))]
    S32(StripeState<'a, u32>),
}

/// Row-at-a-time decoder of one unit (stripe, column group). Lossy decoding
/// uses it for a nested DC stripe, decoding each lossy stripe's DC rows when
/// that stripe needs them.
pub(crate) struct UnitRows<'a>(AnyStripe<'a>);

impl<'a> UnitRows<'a> {
    pub(crate) fn new(p: &Parsed<'a>, u: usize) -> Result<Self, Error> {
        Self::new_with(p, u, p.stripes[u])
    }

    /// [`Self::new`] with the unit's record bytes given (region decoding:
    /// `p` holds no records).
    pub(crate) fn new_with(p: &Parsed<'_>, u: usize, bytes: &'a [u8]) -> Result<Self, Error> {
        if p.quick {
            return Ok(Self(AnyStripe::Quick(
                alloc::boxed::Box::new(crate::quick::State::new(p, u, bytes)?),
                bytes,
            )));
        }
        #[cfg(any(feature = "extended", feature = "encode"))]
        if !p.narrow() {
            return Ok(Self(AnyStripe::S32(StripeState::new_with(p, u, bytes)?)));
        }
        Ok(Self(AnyStripe::S16(StripeState::new_with(p, u, bytes)?)))
    }

    /// Decodes the next row into `out` (the unit's group width of samples).
    pub(crate) fn next_row(
        &mut self,
        p: &Parsed<'_>,
        om: &OutMap,
        out: &mut [u8],
    ) -> Result<(), Error> {
        match &mut self.0 {
            AnyStripe::Quick(st, bytes) => st.next(bytes, out),
            AnyStripe::S16(st) => st.next_row(p, om, out),
            #[cfg(any(feature = "extended", feature = "encode"))]
            AnyStripe::S32(st) => st.next_row(p, om, out),
        }
    }

    /// Whether every row of the unit is decoded.
    fn done(&self) -> bool {
        match &self.0 {
            AnyStripe::Quick(st, _) => st.done(),
            AnyStripe::S16(st) => st.core.row == st.core.rows,
            #[cfg(any(feature = "extended", feature = "encode"))]
            AnyStripe::S32(st) => st.core.row == st.core.rows,
        }
    }
}

/// Row-at-a-time decoder of one stripe: its column groups' units side by
/// side, each row decoded across all of them (streamed decoding, which
/// delivers whole image rows).
pub(crate) struct StripeRows<'a> {
    units: Vec<UnitRows<'a>>,
}

impl<'a> StripeRows<'a> {
    /// The stripe whose column groups' units are `unit(0..ngroups)`. The
    /// first unit parses before the table is reserved (exactly one entry
    /// per group), so a malformed stripe fails before any table exists.
    fn new(
        p: &Parsed<'_>,
        mut unit: impl FnMut(usize) -> Result<UnitRows<'a>, Error>,
    ) -> Result<Self, Error> {
        let ng = p.ngroups();
        let first = unit(0)?;
        let mut units = Vec::new();
        units.try_reserve_exact(ng).map_err(|_| Error::TooLarge)?;
        units.push(first);
        for gi in 1..ng {
            units.push(unit(gi)?);
        }
        Ok(Self { units })
    }

    /// Decodes the stripe's next row into `row` (the image's row bytes);
    /// `check` runs before every unit (cancellation).
    fn next_row(
        &mut self,
        p: &Parsed<'_>,
        om: &OutMap,
        row: &mut [u8],
        check: &impl Fn() -> Result<(), Error>,
    ) -> Result<(), Error> {
        let bpp = p.bpp();
        for (gi, st) in self.units.iter_mut().enumerate() {
            check()?;
            let (x0, gw, _) = p.group(gi);
            st.next_row(p, om, &mut row[x0 * bpp..][..gw * bpp])?;
        }
        Ok(())
    }

    /// Whether every row of the stripe is decoded.
    fn done(&self) -> bool {
        self.units.first().is_none_or(UnitRows::done)
    }
}

/// Capacity of the simultaneously retained streaming group descriptors.
pub(crate) fn stream_group_descriptor_bytes() -> usize {
    core::mem::size_of::<AnyStripe<'static>>()
}

/// The reader's retained stripe owner and temporary borrowed-to-offset table.
pub(crate) fn stream_record_extra_bytes() -> usize {
    #[cfg(feature = "std")]
    {
        core::mem::size_of::<owned_rows::RecordStripe>()
            + MAX_SLOTS * core::mem::size_of::<core::ops::Range<usize>>()
    }
    #[cfg(not(feature = "std"))]
    {
        0
    }
}

/// Complete-slice row decoder. Legacy files retain stripe state. V4 retains
/// checked models/group descriptors and one full-width decoded band; descriptor
/// storage grows with the encoded group count, without a complete decoded raster.
pub struct StreamDecoder<'a> {
    inner: StreamInner<'a>,
    #[cfg(feature = "v3")]
    check: Option<alloc::boxed::Box<StreamCheck<'a>>>,
    pixel_check: Option<crate::pixelsum::RowCheck>,
    pixel_status: crate::pixelsum::PixelChecksumStatus,
}

/// File checksum pieces a row-at-a-time decode checks just ahead of each
/// stripe: before a stripe's first row, every piece up to its last record.
#[cfg(feature = "v3")]
struct StreamCheck<'a> {
    pieces: crate::checksum::Pieces<'a>,
    /// Pieces before this one are checked.
    next: u64,
    /// (first row of a stripe, file offset after its last record), by stripe.
    marks: Vec<(u32, usize)>,
    mark: usize,
}

#[cfg(feature = "v3")]
impl StreamCheck<'_> {
    /// Checks what row `y` of `height` needs; the last row checks the rest.
    fn before_row(&mut self, y: u32, height: u32) -> Result<(), Error> {
        let mut upto = self.next;
        while let Some(&(row, end)) = self.marks.get(self.mark) {
            if row > y {
                break;
            }
            let end = self.pieces.covered.offset(end);
            upto = upto.max(self.pieces.overlapping(0..end).end);
            self.mark += 1;
        }
        if y + 1 >= height {
            upto = self.pieces.table.pieces();
        }
        if upto > self.next {
            self.pieces.verify(self.next..upto)?;
            self.next = upto;
        }
        Ok(())
    }
}

enum StreamInner<'a> {
    V4(alloc::boxed::Box<crate::v4::plan::Rows<'a>>),
    #[cfg(feature = "native-jpeg")]
    Native(alloc::boxed::Box<crate::jpeg::wire::NativeStream<'a>>),
    #[cfg(feature = "v3")]
    Lossless(alloc::boxed::Box<LosslessStream<'a>>),
    #[cfg(feature = "v3")]
    Lossy(alloc::boxed::Box<crate::lossy::LossyStream<'a>>),
}

impl<'a> StreamDecoder<'a> {
    /// Parses headers; pixel buffers are allocated on the first row.
    /// A `cSUM` file checksum is verified piece by piece just ahead of the
    /// stripes: before a stripe's first row, every piece up to the stripe's
    /// last record; the last row checks the rest of the file. No row is
    /// returned from bytes that were not verified.
    pub fn new(data: &'a [u8]) -> Result<Self, Error> {
        Self::open(data, true, &crate::parse::limits::Unrestricted)
    }

    /// [`Self::new`] under `limits`, which also set whether a file checksum
    /// is verified ([`crate::Limits::verify_checksum`]).
    pub fn with_limits(data: &'a [u8], limits: &crate::Limits) -> Result<Self, Error> {
        Self::open(
            data,
            limits.verify_checksum,
            &crate::parse::limits::Limited(limits, crate::parse::limits::Unrestricted),
        )
    }

    fn open(
        data: &'a [u8],
        verify: bool,
        admission: &impl crate::parse::limits::Admission,
    ) -> Result<Self, Error> {
        if crate::parse::header::header_version(data)? == 4 {
            let hp = crate::parse::header::read_header_sized(&mut ByteReader::new(data))?;
            admission.header(&hp.header.info, hp.len)?;
            let mut resume = admission.v4_resume();
            resume.nested_policy(verify, None);
            let session = resume.session(admission.v4_cancel());
            let (rows, pixel_check, pixel_status) =
                crate::v4::plan::Rows::new(alloc::borrow::Cow::Borrowed(data), &session)?;
            admission.v4_commit(session.v4_resume())?;
            return Ok(Self {
                inner: StreamInner::V4(alloc::boxed::Box::new(rows)),
                #[cfg(feature = "v3")]
                check: None,
                pixel_check,
                pixel_status,
            });
        }
        #[cfg(feature = "v3")]
        return Self::open_legacy(data, verify, admission);
        #[cfg(not(feature = "v3"))]
        Err(Error::Unsupported("enable the v3 feature for V2/V3 images"))
    }

    #[cfg(feature = "v3")]
    fn open_legacy(
        data: &'a [u8],
        verify: bool,
        admission: &impl crate::parse::limits::Admission,
    ) -> Result<Self, Error> {
        let mode = if verify {
            crate::checksum::Verify::Defer
        } else {
            crate::checksum::Verify::Off
        };
        let f = parse_file(data, admission, mode)?;
        if matches!(f.pixelsum, crate::pixelsum::Table::Known(..)) {
            admission.hint(core::mem::size_of::<crate::pixelsum::Raster>())?;
        }
        let pixel_check = crate::pixelsum::RowCheck::new(f.pixelsum, &f.header.info)?;
        let pixel_status = f.pixelsum.status();
        let color = f.header.lossy_color;
        let declared = f.declared;
        let version = f.header.version;
        let (info, body) = (f.header.info, f.data);
        // The payload is parsed before its pieces are checked: a mismatch
        // explains a parse error, so it is reported first (SPEC 1.4).
        let explain = |e: Error| match f.checksum.and_then(|c| c.pieces) {
            Some(p) => p.verify_all(1).err().unwrap_or(e),
            None => e,
        };
        let (inner, ends) = if crate::parse::payload::native_selected(color, declared, body)? {
            #[cfg(not(feature = "native-jpeg"))]
            {
                return Err(Error::Unsupported("feature native_components"));
            }
            #[cfg(feature = "native-jpeg")]
            {
                if let Some(pc) = f.checksum.and_then(|c| c.pieces) {
                    pc.verify_all(1)?;
                }
                let p =
                    crate::jpeg::wire::parse(info, version, body, admission).map_err(explain)?;
                if p.payload_len != body.len() {
                    return Err(Error::Corrupt("trailing DATA bytes"));
                }
                (
                    StreamInner::Native(alloc::boxed::Box::new(
                        crate::jpeg::wire::NativeStream::new(p)?,
                    )),
                    None,
                )
            }
        } else if let Some(color) = color {
            let s = crate::lossy::LossyStream::new(info, version, color, declared, body)
                .map_err(explain)?;
            let ends = f.checksum.map(|_| s.stripe_ends());
            (StreamInner::Lossy(alloc::boxed::Box::new(s)), ends)
        } else {
            let p = parse_complete_body(info, version, declared, body).map_err(explain)?;
            let ng = p.ngroups();
            let ends = f.checksum.map(|_| {
                (0..p.stripe_count())
                    .map(|s| ((s * p.hs) as u32, p.stripes[s * ng + ng - 1]))
                    .collect()
            });
            let s = LosslessStream::from_parsed(p);
            (StreamInner::Lossless(alloc::boxed::Box::new(s)), ends)
        };
        let base = data.as_ptr() as usize;
        let check = match (f.checksum.and_then(|c| c.pieces), ends) {
            (Some(pieces), Some(ends)) => Some(alloc::boxed::Box::new(StreamCheck {
                pieces,
                next: 0,
                marks: ends
                    .into_iter()
                    .map(|(row, rec): (u32, &[u8])| (row, rec.as_ptr() as usize - base + rec.len()))
                    .collect(),
                mark: 0,
            })),
            _ => None,
        };
        Ok(Self {
            inner,
            check,
            pixel_check,
            pixel_status,
        })
    }

    pub fn info(&self) -> &ImageInfo {
        match &self.inner {
            StreamInner::V4(s) => s.info(),
            #[cfg(feature = "native-jpeg")]
            StreamInner::Native(s) => &s.parsed.head.info,
            #[cfg(feature = "v3")]
            StreamInner::Lossless(s) => &s.p.info,
            #[cfg(feature = "v3")]
            StreamInner::Lossy(s) => s.info(),
        }
    }

    /// Decoded bitmap coverage of the primary image; stopping early is Pending.
    pub fn pixel_checksum_status(&self) -> crate::pixelsum::PixelChecksumStatus {
        self.pixel_check
            .as_ref()
            .map_or(self.pixel_status, |c| c.status())
    }

    /// Rows decoded so far.
    pub fn rows_decoded(&self) -> u32 {
        match &self.inner {
            StreamInner::V4(s) => s.rows_decoded(),
            #[cfg(feature = "native-jpeg")]
            StreamInner::Native(s) => s.y as u32,
            #[cfg(feature = "v3")]
            StreamInner::Lossless(s) => s.y as u32,
            #[cfg(feature = "v3")]
            StreamInner::Lossy(s) => s.rows_decoded(),
        }
    }

    /// Decodes the next row into `out` (at least `row_bytes()` long).
    /// Returns `Ok(false)` once all rows were produced.
    pub fn next_row(&mut self, out: &mut [u8]) -> Result<bool, Error> {
        if let Some(c) = &self.pixel_check {
            c.before()?;
        }
        #[cfg(feature = "v3")]
        {
            let (y, h) = (self.rows_decoded(), self.info().height);
            if let Some(c) = &mut self.check
                && y < h
            {
                c.before_row(y, h)?;
            }
        }
        let produced = match &mut self.inner {
            StreamInner::V4(s) => s.next_row_checked(out, &|| Ok(())),
            #[cfg(feature = "native-jpeg")]
            StreamInner::Native(s) => s.next_row(out),
            #[cfg(feature = "v3")]
            StreamInner::Lossless(s) => s.next_row(out),
            #[cfg(feature = "v3")]
            StreamInner::Lossy(s) => s.next_row(out),
        }?;
        if produced && let Some(c) = &mut self.pixel_check {
            c.row(out)?;
        }
        Ok(produced)
    }
}

pub(crate) struct LosslessStream<'a> {
    pub(crate) p: Parsed<'a>,
    om: OutMap,
    stripe: usize,
    /// The current stripe's column groups (none between stripes).
    cur: Option<StripeRows<'a>>,
    y: usize,
}

impl<'a> LosslessStream<'a> {
    /// Streams an already parsed image (a nested image of a lossy file).
    pub(crate) fn from_parsed(p: Parsed<'a>) -> Self {
        let om = OutMap::new(&p.info, &p.layout);
        Self {
            p,
            om,
            stripe: 0,
            cur: None,
            y: 0,
        }
    }

    /// Test-only observation of actual simultaneously owned Vec capacities.
    #[cfg(all(test, feature = "encode"))]
    pub(crate) fn retained_group_capacity(&self) -> (usize, usize) {
        fn planes<S>(planes: &Vec<Vec<S>>) -> usize {
            planes.capacity() * size_of::<Vec<S>>()
                + planes
                    .iter()
                    .map(|p| p.capacity() * size_of::<S>())
                    .sum::<usize>()
        }
        fn state<S: crate::px::Px>(s: &StripeState<'_, S>) -> usize {
            let words = s.words.capacity() * size_of::<Words<'_>>();
            let s = &s.core;
            s.preds.capacity()
                + words
                + s.states.capacity() * size_of::<LaneState>()
                + planes(&s.res_prev)
                + planes(&s.res_cur)
                + planes(&s.val_prev)
                + planes(&s.val_cur)
                + s.res_next0.capacity() * size_of::<S>()
                + s.pal.as_ref().map_or(0, |p| {
                    planes(&p.entries) + p.tbl.capacity() + p.scratch.capacity()
                })
                + s.refs
                    .as_ref()
                    .map_or(0, |r| planes(&r.cur) + planes(&r.prev) + r.need.capacity())
        }
        let Some(cur) = &self.cur else {
            return (0, 0);
        };
        (
            cur.units.len(),
            cur.units.capacity() * size_of::<UnitRows<'_>>()
                + cur
                    .units
                    .iter()
                    .map(|s| match &s.0 {
                        AnyStripe::Quick(s, _) => s.working_bytes(),
                        AnyStripe::S16(s) => state(s),
                        #[cfg(any(feature = "extended", feature = "encode"))]
                        AnyStripe::S32(s) => state(s),
                    })
                    .fold(0usize, usize::saturating_add),
        )
    }

    #[cfg(any(feature = "v3", test))]
    pub(crate) fn next_row(&mut self, out: &mut [u8]) -> Result<bool, Error> {
        self.next_row_checked(out, &|| Ok(()))
    }

    pub(crate) fn next_row_checked(
        &mut self,
        out: &mut [u8],
        check: &(impl Fn() -> Result<(), Error> + Sync),
    ) -> Result<bool, Error> {
        check()?;
        if self.y >= self.p.info.height as usize {
            return Ok(false);
        }
        let rb = self.p.info.row_bytes();
        if out.len() < rb {
            return Err(Error::InvalidInput("row buffer too small"));
        }
        let cur = match &mut self.cur {
            Some(cur) => cur,
            None => {
                let (p, first) = (&self.p, self.stripe * self.p.ngroups());
                self.cur.insert(StripeRows::new(p, |gi| {
                    check()?;
                    UnitRows::new(p, first + gi)
                })?)
            }
        };
        cur.next_row(&self.p, &self.om, out, check)?;
        if cur.done() {
            self.cur = None;
            self.stripe += 1;
        }
        self.y += 1;
        check()?;
        Ok(true)
    }
}

/// Reader decoding of a lossless `DATA` payload after its head `p` (read by
/// `parse::payload::read_head`): one stripe's records at a time, each span
/// checked against the unit table hint when the head has one.
#[cfg(feature = "std")]
pub(crate) async fn read_lossless<
    R: crate::read::input::Input,
    A: crate::parse::limits::Admission,
>(
    src: &mut crate::read::Src<R>,
    p: alloc::boxed::Box<Parsed<'static>>,
    admission: &A,
    on_row: &mut (impl crate::read::input::Rows + ?Sized),
) -> Result<(), Error> {
    let mut p = p;
    p.inter = on_row.has_reference();
    let info = &p.info.clone();
    let units = p.stripe_count() * p.ngroups();
    if let Some(room) = src.remaining() {
        crate::parse::records::check_count(units, room)?;
    }
    src.begin_records_with(p.hint.take(), p.unit_ordinals)?;
    crate::trace::exit(src.pos);
    let om = OutMap::new(&p.info, &p.layout);
    let mut row = Vec::new();
    let mut recs = Vec::new();
    let mut y = 0u32;
    for si in 0..p.stripe_count() {
        // Record buffers grow only as records arrive; a head alone cannot
        // reserve the group table or the row buffer.
        admission.check()?;
        #[cfg(all(feature = "lab", feature = "std"))]
        trace_lossless_stripe(&p, si, src.pos);
        src.records_async(&mut recs, p.ngroups(), R_STRIPE).await?;
        crate::trace::exit(src.pos);
        if row.is_empty() {
            row.try_reserve_exact(info.row_bytes())
                .map_err(|_| Error::TooLarge)?;
            row.resize(info.row_bytes(), 0);
        }
        let mut state = owned_rows::RecordStripe::new(&p, si, recs)?;
        let end = ((si + 1) * p.hs).min(info.height as usize) as u32;
        while y < end {
            if p.inter {
                on_row.reference(info, y, &mut row).await?;
            }
            if !state.next_row(&p, &om, &mut row, &|| admission.check())? {
                return Err(Error::Corrupt("short lossless stripe"));
            }
            on_row.row(info, y, &row).await?;
            y += 1;
        }
        if state.next_row(&p, &om, &mut row, &|| admission.check())? {
            return Err(Error::Corrupt("long lossless stripe"));
        }
        recs = state.into_records();
    }
    Ok(())
}

/// LAB trace: opens stripe `si` and labels its column-group records.
#[cfg(all(feature = "lab", feature = "std"))]
fn trace_lossless_stripe(p: &Parsed<'_>, si: usize, at: u64) {
    if !crate::trace::on() {
        return;
    }
    let y0 = si * p.hs;
    let rows = p.hs.min(p.info.height as usize - y0);
    let slots = p.slot_names.join(", ");
    crate::trace::tspan!(
        "stripe",
        "2.3",
        alloc::format!("stripe {si} (rows {}-{}; slots {slots})", y0, y0 + rows - 1),
        at
    );
    crate::trace::region(0, y0 as u64, p.info.width as u64, rows as u64);
    for gi in 0..p.ngroups() {
        let (x0, gw, _) = p.group(gi);
        crate::trace::tnext!(
            alloc::format!(
                "stripe {si} group {gi} (columns {}-{}; slots {slots})",
                x0,
                x0 + gw - 1
            ),
            Some([x0 as u64, y0 as u64, gw as u64, rows as u64])
        );
    }
}

/// The header, internal color, `DATA` body and declared features of a lossy
/// file, or None for lossless.
#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
pub(crate) fn lossy_body(
    data: &[u8],
) -> Result<
    Option<(
        ImageInfo,
        crate::LossyColor,
        &[u8],
        crate::parse::header::Features,
    )>,
    Error,
> {
    let color = crate::parse::header::read_header(&mut ByteReader::new(data))?.lossy_color;
    let (info, _, body, d) = parse_header_lossy(data)?;
    Ok(color.map(|c| (info, c, body, d)))
}

/// Decodes into `out` (rows of `stride` bytes, native-endian samples).
pub fn decode_into(data: &[u8], out: &mut [u8], stride: usize) -> Result<ImageInfo, Error> {
    decode_into_inter(data, out, stride, false, None, true)
}

/// [`decode_into`] with `inter` (see `decode_into_frame`).
pub(crate) fn decode_into_inter(
    data: &[u8],
    out: &mut [u8],
    stride: usize,
    inter: bool,
    inherited: Option<&ImageInfo>,
    verify: bool,
) -> Result<ImageInfo, Error> {
    if crate::parse::header::header_version(data)? == 4 {
        let mut limits = crate::Limits::NONE;
        limits.verify_checksum = verify;
        let admission = crate::parse::limits::Unrestricted;
        let file = crate::v4::admitted::parse(data, &admission, verify, Some(limits))?;
        return crate::v4::admitted::decode_frame(
            &file,
            out,
            stride,
            1,
            inter,
            inherited,
            &admission,
            &|| Ok(()),
        );
    }
    let f = parse_file(
        data,
        &crate::parse::limits::Unrestricted,
        crate::checksum::Verify::from_flag(verify, 1),
    )?;
    let info = decode_file_serial(&f, out, stride, inter, inherited)?;
    f.pixelsum.verify(0, &info, out, stride)?;
    Ok(info)
}

/// Decodes the image of a parsed file (its checksum checked or skipped) as
/// a frame (`frames.rs`): `inter`: `out` holds the previous frame;
/// `inherited`: the file whose color a lossy frame inherits.
pub(crate) fn decode_parsed_frame(
    f: &crate::container::File<'_>,
    out: &mut [u8],
    stride: usize,
    threads: usize,
    inter: bool,
    inherited: Option<&ImageInfo>,
) -> Result<ImageInfo, Error> {
    if !cfg!(feature = "v3") || f.header.version == 4 {
        return crate::v4::admitted::decode_frame(
            f,
            out,
            stride,
            threads,
            inter,
            inherited,
            &crate::parse::limits::Unrestricted,
            &|| Ok(()),
        );
    }
    #[cfg(feature = "std")]
    let decoded = {
        let info = f.header.info.clone();
        let color = f.header.lossy_color;
        decode_body_into_frame(
            info,
            f.header.version,
            f.data,
            color,
            f.declared,
            out,
            stride,
            threads,
            inter,
            inherited,
            None,
            None,
        )
    };
    #[cfg(not(feature = "std"))]
    let decoded = {
        let _ = threads;
        decode_file_serial(f, out, stride, inter, inherited)
    };
    let info = decoded?;
    f.pixelsum.verify(0, &info, out, stride)?;
    Ok(info)
}

/// [`decode_parsed_frame`] on one thread.
pub(crate) fn decode_parsed_frame_admitted(
    f: &crate::container::File<'_>,
    out: &mut [u8],
    stride: usize,
    inter: bool,
    inherited: Option<&ImageInfo>,
    admission: &impl crate::parse::limits::Admission,
    check: &(impl Fn() -> Result<(), Error> + Sync),
) -> Result<ImageInfo, Error> {
    if !cfg!(feature = "v3") || f.header.version == 4 {
        return crate::v4::admitted::decode_frame(
            f, out, stride, 1, inter, inherited, admission, check,
        );
    }
    check()?;
    if matches!(f.pixelsum, crate::pixelsum::Table::Known(..)) {
        admission.hint(core::mem::size_of::<crate::pixelsum::Raster>())?;
        admission.pixel_checksum(&f.header.info)?;
    }
    let mut info = f.header.info.clone();
    let decoded =
        if crate::parse::payload::native_selected(f.header.lossy_color, f.declared, f.data)? {
            #[cfg(not(feature = "native-jpeg"))]
            {
                return Err(Error::Unsupported("feature native_components"));
            }
            #[cfg(feature = "native-jpeg")]
            {
                let parsed = crate::jpeg::wire::parse(info, f.header.version, f.data, admission)?;
                if parsed.payload_len != f.data.len() {
                    return Err(Error::Corrupt("trailing DATA bytes"));
                }
                crate::jpeg::wire::decode(parsed, out, stride, check)?
            }
        } else if let Some(color) = f.header.lossy_color {
            if let Some(parent) = inherited {
                info.cicp = parent.cicp;
                info.alpha_mode = parent.alpha_mode;
            }
            let parsed = crate::lossy::parse_lossy_checked(
                info,
                f.header.version,
                color,
                f.declared,
                f.data,
                admission,
            )?;
            crate::lossy::decode_lossy_parsed_checked::<true>(
                parsed,
                out,
                stride,
                1,
                &crate::Limits::NONE,
                true,
                inter,
                check,
            )?
        } else {
            let mut parsed =
                parse_complete_body_checked(info, f.header.version, f.declared, f.data, admission)?;
            parsed.inter = inter;
            decode_parsed_into_checked(parsed, out, stride, check)?
        };
    f.pixelsum.verify_checked(0, &decoded, out, stride, check)?;
    check()?;
    Ok(decoded)
}

/// Unrestricted serial frame reconstruction.
fn decode_file_serial(
    f: &crate::container::File<'_>,
    out: &mut [u8],
    stride: usize,
    inter: bool,
    inherited: Option<&ImageInfo>,
) -> Result<ImageInfo, Error> {
    let declared = f.declared;
    if crate::parse::payload::native_selected(f.header.lossy_color, declared, f.data)? {
        #[cfg(not(feature = "native-jpeg"))]
        {
            return Err(Error::Unsupported("feature native_components"));
        }
        #[cfg(feature = "native-jpeg")]
        {
            return crate::jpeg::wire::decode(
                crate::jpeg::wire::parse(
                    f.header.info.clone(),
                    f.header.version,
                    f.data,
                    &crate::parse::limits::Unrestricted,
                )?,
                out,
                stride,
                &|| Ok(()),
            );
        }
    }
    let (mut info, body) = (f.header.info.clone(), f.data);
    if let Some(color) = f.header.lossy_color {
        if let Some(parent) = inherited {
            info.cicp = parent.cicp;
            info.alpha_mode = parent.alpha_mode;
        }
        return crate::lossy::decode_lossy_frame(
            info,
            f.header.version,
            color,
            declared,
            body,
            out,
            stride,
            1,
            inter,
        );
    }
    let mut p = parse_complete_body(info, f.header.version, declared, body)?;
    p.inter = inter;
    decode_parsed_into(p, out, stride)
}

/// Reuse validated structure for both caller-owned and allocating output.
fn decode_parsed_into(p: Parsed<'_>, out: &mut [u8], stride: usize) -> Result<ImageInfo, Error> {
    decode_parsed_into_checked(p, out, stride, &|| Ok(()))
}

/// Serial reconstruction shared by ordinary and admitted slice decoding.
pub(crate) fn decode_parsed_into_checked(
    p: Parsed<'_>,
    out: &mut [u8],
    stride: usize,
    check: &impl Fn() -> Result<(), Error>,
) -> Result<ImageInfo, Error> {
    let extent = p
        .info
        .checked_stride_len(stride)
        .ok_or(Error::InvalidInput("invalid output layout"))?;
    if out.len() < extent {
        return Err(Error::InvalidInput("output buffer too small"));
    }
    let om = OutMap::new(&p.info, &p.layout);
    for si in 0..p.stripe_count() {
        decode_stripe(&p, si, &om, &mut out[si * p.hs * stride..], stride, check)?;
    }
    Ok(p.info)
}

/// Decodes into a new tightly packed buffer with [`crate::Limits::DEFAULT`].
pub fn decode(data: &[u8]) -> Result<(ImageInfo, Vec<u8>), Error> {
    decode_with_limits(data, &crate::Limits::DEFAULT)
}

/// Decodes into a new tightly packed buffer, rejecting images over `limits`.
pub fn decode_with_limits(
    data: &[u8],
    limits: &crate::Limits,
) -> Result<(ImageInfo, Vec<u8>), Error> {
    if crate::parse::header::header_version(data)? == 4 {
        return crate::decode_resources::DecodeSession::new(
            crate::decode_resources::DecodePolicy::default().with_output_limits(*limits),
        )
        .decode(data);
    }
    let f = parse_file(
        data,
        &crate::parse::limits::Limited(limits, crate::parse::limits::Unrestricted),
        crate::checksum::Verify::from_flag(limits.verify_checksum, 1),
    )?;
    let (info, out) = decode_body_with_limits(
        f.header.info.clone(),
        f.header.version,
        f.header.lossy_color,
        f.declared,
        f.data,
        limits,
    )?;
    f.pixelsum.verify(0, &info, &out, info.row_bytes())?;
    Ok((info, out))
}

/// [`decode_with_limits`] of a `DATA` payload whose header was read (and
/// checked against `limits`) by the caller: plane tiles, whose header and
/// framing their tile profile checks.
pub(crate) fn decode_body_with_limits(
    info: ImageInfo,
    version: u8,
    color: Option<crate::LossyColor>,
    declared: crate::parse::header::Features,
    body: &[u8],
    limits: &crate::Limits,
) -> Result<(ImageInfo, Vec<u8>), Error> {
    let total = info.checked_buffer_len().ok_or(Error::TooLarge)?;
    let rb = info.row_bytes();
    if crate::parse::payload::native_selected(color, declared, body)? {
        #[cfg(not(feature = "native-jpeg"))]
        {
            return Err(Error::Unsupported("feature native_components"));
        }
        #[cfg(feature = "native-jpeg")]
        {
            let p =
                crate::jpeg::wire::parse(info, version, body, &crate::parse::limits::Unrestricted)?;
            if p.payload_len != body.len() {
                return Err(Error::Corrupt("trailing DATA bytes"));
            }
            let mut out = Vec::new();
            out.try_reserve_exact(total).map_err(|_| Error::TooLarge)?;
            out.resize(total, 0);
            let info = crate::jpeg::wire::decode(p, &mut out, rb, &|| Ok(()))?;
            return Ok((info, out));
        }
    }
    if let Some(color) = color {
        let parsed = crate::lossy::parse_lossy_as(info, version, color, declared, body)?;
        let mut out = vec![0u8; total];
        let info = crate::lossy::decode_lossy_parsed(parsed, &mut out, rb, 1, limits, true, false)?;
        return Ok((info, out));
    }
    // Validate the structure before allocating the output.
    let parsed = parse_complete_body(info, version, declared, body)?;
    let mut out = vec![0u8; total];
    let info = decode_parsed_into(parsed, &mut out, rb)?;
    Ok((info, out))
}

/// Decodes using up to `threads` threads (stripes are independent).
///
/// A `cSUM` file checksum is verified on the same workers. Lossless: each
/// worker checks the pieces under a unit's bytes before decoding it, so a
/// mismatch can be reported after `out` was written; it wins over any decode
/// error. Lossy: the pieces are checked before decoding.
#[cfg(feature = "std")]
pub fn decode_into_threads(
    data: &[u8],
    out: &mut [u8],
    stride: usize,
    threads: usize,
) -> Result<ImageInfo, Error> {
    if crate::parse::header::header_version(data)? == 4 {
        return crate::v4::decode::threaded(data, out, stride, threads, &crate::Limits::NONE, None);
    }
    decode_into_frame(
        data,
        out,
        stride,
        threads,
        false,
        None,
        true,
        &crate::parse::limits::Unrestricted,
    )
}

/// [`decode_into_threads`] under `limits`, which also set whether a file
/// checksum is verified ([`crate::Limits::verify_checksum`]).
#[cfg(feature = "std")]
pub fn decode_into_threads_with_limits(
    data: &[u8],
    out: &mut [u8],
    stride: usize,
    threads: usize,
    limits: &crate::Limits,
) -> Result<ImageInfo, Error> {
    if crate::parse::header::header_version(data)? == 4 {
        return crate::v4::decode::threaded(data, out, stride, threads, limits, None);
    }
    decode_into_frame(
        data,
        out,
        stride,
        threads,
        false,
        None,
        limits.verify_checksum,
        &crate::parse::limits::Limited(limits, crate::parse::limits::Unrestricted),
    )
}

/// How a multi-threaded decode checks a file checksum's pieces. The default
/// is `Jit`; the others stay for LAB comparisons (`IMIJ_CSUM_SCHED`,
/// benchmarks/csum_pieces_2026-10-07).
#[cfg(feature = "std")]
#[derive(Clone, Copy, PartialEq, Eq)]
// Only LAB builds construct the other variants.
#[cfg_attr(not(feature = "lab"), allow(dead_code))]
pub(crate) enum Schedule {
    /// Every worker checks pieces until none are left, then decodes.
    Prepass,
    /// Piece checks are the first tasks of the decode's own queue (lossless)
    /// or run beside it on the rayon pool (lossy): no barrier.
    Queue,
    /// Lossless: before a unit, its worker checks the unclaimed pieces that
    /// start before the unit's last byte (the decode then reads them from
    /// cache); workers check what is left after the last unit. Lossy as
    /// Prepass: beside small decodes the join cost more than the hash.
    Jit,
}

#[cfg(feature = "std")]
fn schedule() -> Schedule {
    #[cfg(feature = "lab")]
    if let Some(s) = crate::lab::csum_schedule() {
        return s;
    }
    Schedule::Jit
}

/// [`decode_into_threads`] with `inter`: the image is an inter frame and
/// `out` holds the previous frame (rows may predict from it, lossy blocks
/// may keep it).
#[cfg(feature = "std")]
pub(crate) fn decode_into_frame(
    data: &[u8],
    out: &mut [u8],
    stride: usize,
    threads: usize,
    inter: bool,
    inherited: Option<&ImageInfo>,
    verify: bool,
    admission: &impl crate::parse::limits::Admission,
) -> Result<ImageInfo, Error> {
    let mode = if verify {
        crate::checksum::Verify::Defer
    } else {
        crate::checksum::Verify::Off
    };
    let f = parse_file(data, admission, mode)?;
    let color = f.header.lossy_color;
    let declared = f.declared;
    let pieces = f.checksum.and_then(|c| c.pieces);
    let version = f.header.version;
    let (info, body) = (f.header.info, f.data);
    let decoded = decode_body_into_frame(
        info,
        version,
        body,
        color,
        declared,
        out,
        stride,
        threads,
        inter,
        inherited,
        pieces.as_ref(),
        None,
    )?;
    f.pixelsum.verify(0, &decoded, out, stride)?;
    Ok(decoded)
}

/// [`decode_into_threads`] reporting each lossless or lossy unit to a live
/// `pulse` ([`crate::progress`]). Native JPEG payloads decode as usual and
/// report no units.
#[cfg(all(feature = "std", feature = "lab"))]
pub(crate) fn decode_into_threads_observed(
    data: &[u8],
    out: &mut [u8],
    stride: usize,
    threads: usize,
    pulse: &dyn Pulse,
) -> Result<ImageInfo, Error> {
    if crate::parse::header::header_version(data)? == 4 {
        return crate::v4::decode::threaded(
            data,
            out,
            stride,
            threads,
            &crate::Limits::NONE,
            Some(pulse),
        );
    }
    let f = parse_file(
        data,
        &crate::parse::limits::Unrestricted,
        crate::checksum::Verify::Defer,
    )?;
    let pieces = f.checksum.and_then(|c| c.pieces);
    let decoded = decode_body_into_frame(
        f.header.info,
        f.header.version,
        f.data,
        f.header.lossy_color,
        f.declared,
        out,
        stride,
        threads,
        false,
        None,
        pieces.as_ref(),
        Some(pulse),
    )?;
    f.pixelsum.verify(0, &decoded, out, stride)?;
    Ok(decoded)
}

/// The checks of `pieces` beside a decode: a mismatch wins over the decode's
/// own result, which it explains.
#[cfg(feature = "std")]
fn with_pieces(
    checked: Result<(), Error>,
    decoded: Result<ImageInfo, Error>,
) -> Result<ImageInfo, Error> {
    checked?;
    decoded
}

/// Full-width quick stripe for the non-cancellable threaded entry point.
/// A raw unit is one bulk copy on tight output; padded rows keep their padding.
#[cfg(feature = "std")]
fn quick_stripe_into(
    p: &Parsed<'_>,
    si: usize,
    om: &OutMap,
    out: &mut [u8],
    stride: usize,
) -> Result<(), Error> {
    if let Some(raw) = crate::quick::raw(p, si, p.stripes[si])? {
        let row = p.info.row_bytes();
        if stride == row {
            out[..raw.len()].copy_from_slice(raw);
        } else {
            for (src, dst) in raw.chunks_exact(row).zip(out.chunks_mut(stride)) {
                dst[..row].copy_from_slice(src);
            }
        }
        #[cfg(target_endian = "big")]
        for row in out.chunks_mut(stride).take(raw.len() / row) {
            for sample in row[..p.info.row_bytes()].chunks_exact_mut(p.info.sample.bytes()) {
                sample.reverse();
            }
        }
        return Ok(());
    }
    decode_stripe(p, si, om, out, stride, &|| Ok(()))
}

#[cfg(feature = "std")]
#[allow(clippy::too_many_arguments)]
fn decode_body_into_frame(
    mut info: ImageInfo,
    version: u8,
    body: &[u8],
    color: Option<crate::LossyColor>,
    declared: crate::parse::header::Features,
    out: &mut [u8],
    stride: usize,
    threads: usize,
    inter: bool,
    inherited: Option<&ImageInfo>,
    pieces: Option<&crate::checksum::Pieces<'_>>,
    pulse: Option<&dyn Pulse>,
) -> Result<ImageInfo, Error> {
    let parallel = crate::par::worker_count(threads, 2) > 1;
    let sched = schedule();
    let first =
        !parallel || sched == Schedule::Prepass || (color.is_some() && sched == Schedule::Jit);
    if let Some(pc) = pieces
        && first
    {
        pc.verify_all(threads)?;
    }
    if crate::parse::payload::native_selected(color, declared, body)? {
        #[cfg(not(feature = "native-jpeg"))]
        {
            return Err(Error::Unsupported("feature native_components"));
        }
        #[cfg(feature = "native-jpeg")]
        {
            if let Some(pc) = pieces {
                pc.verify_all(threads)?;
            }
            let p =
                crate::jpeg::wire::parse(info, version, body, &crate::parse::limits::Unrestricted)?;
            if p.payload_len != body.len() {
                return Err(Error::Corrupt("trailing DATA bytes"));
            }
            return crate::jpeg::wire::decode_threads(p, out, stride, threads);
        }
    }
    let queued = pieces.filter(|_| !first);
    let jit = queued.filter(|_| sched == Schedule::Jit);
    if let Some(color) = color {
        if let Some(parent) = inherited {
            info.cicp = parent.cicp;
            info.alpha_mode = parent.alpha_mode;
        }
        let decode = || {
            #[cfg(feature = "lab")]
            if let Some(pulse) = pulse {
                return crate::lossy::decode_lossy_frame_observed(
                    info, version, color, declared, body, out, stride, threads, inter, pulse,
                );
            }
            crate::lossy::decode_lossy_frame(
                info, version, color, declared, body, out, stride, threads, inter,
            )
        };
        let Some(pc) = queued else {
            return decode();
        };
        #[cfg(feature = "rayon")]
        {
            let (checked, decoded) = rayon::join(|| pc.verify_all(threads), decode);
            return with_pieces(checked, decoded);
        }
        #[cfg(not(feature = "rayon"))]
        {
            pc.verify_all(threads)?;
            return decode();
        }
    }
    let mut p = match parse_complete_body(info, version, declared, body) {
        Ok(p) => p,
        // A mismatch explains a parse error, so it is reported first.
        Err(e) => {
            if let Some(pc) = queued {
                pc.verify_all(threads)?;
            }
            return Err(e);
        }
    };
    p.inter = inter;
    let extent = p
        .info
        .checked_stride_len(stride)
        .ok_or(Error::InvalidInput("invalid output layout"))?;
    if out.len() < extent {
        return Err(Error::InvalidInput("output buffer too small"));
    }
    let rb = p.info.row_bytes();
    let om = OutMap::new(&p.info, &p.layout);
    let ns = p.stripe_count();
    let ng = p.ngroups();
    let checksum_threads = threads;
    let raw = p.quick && ng == 1 && p.stripes.iter().all(|unit| unit.first() == Some(&0));
    // Small raw rasters do only bounded copies. Keep them on the caller rather
    // than distributing bounded copy traffic and waiting for worker wakeups.
    // Large rasters retain parallel first-touch/copy bandwidth. This conservative
    // bound is qualified separately from packet work and caller pool entry;
    // it is a scheduling budget, not an assertion about every CPU's LLC size.
    let threads = crate::par::worker_count(
        if raw && extent <= 32 * 1024 * 1024 {
            1
        } else {
            threads
        },
        ns * ng,
    );
    // Reports each unit to a live pulse; otherwise one branch per unit.
    let observed = pulse
        .and_then(|pulse| crate::progress::UnitProgress::plan(pulse, unit_shapes(&p), threads));
    let observed = observed.as_ref();
    if threads == 1 {
        if let Some(pc) = queued {
            pc.verify_all(if raw { checksum_threads } else { 1 })?;
        }
        for si in 0..ns {
            if let Some(observed) = observed {
                decode_stripe_observed(
                    &p,
                    si,
                    &om,
                    &mut out[si * p.hs * stride..],
                    stride,
                    observed,
                )?;
                continue;
            }
            if p.quick && ng == 1 {
                quick_stripe_into(&p, si, &om, &mut out[si * p.hs * stride..], stride)?;
                continue;
            }
            decode_stripe(&p, si, &om, &mut out[si * p.hs * stride..], stride, &|| {
                Ok(())
            })?;
        }
        return Ok(p.info);
    }
    // Queued piece checks: queue checks them before a worker's first unit;
    // jit checks, after a worker's last unit, what `claim_through` left.
    let npieces = queued.map_or(0, |pc| pc.table.pieces());
    let next_piece = std::sync::atomic::AtomicU64::new(0);
    let piece_err: std::sync::Mutex<Option<Error>> = std::sync::Mutex::new(None);
    let check_pieces = || {
        let Some(pc) = queued else { return };
        loop {
            let i = next_piece.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if i >= npieces {
                break;
            }
            if let Err(e) = pc.verify(i..i + 1) {
                *piece_err.lock().unwrap() = Some(e);
            }
        }
    };
    // Jit: claims, in order, the pieces that start before covered offset
    // `end`, checking each.
    let claim_through = |rec: &[u8]| {
        let Some(pc) = jit else { return };
        // Records are slices of the file; anything else claims every piece.
        let file = pc.covered.data;
        let pos = (rec.as_ptr_range().end as usize)
            .checked_sub(file.as_ptr() as usize)
            .filter(|&p| p <= file.len())
            .unwrap_or(file.len());
        let end = pc.covered.offset(pos);
        let mut i = next_piece.load(std::sync::atomic::Ordering::Relaxed);
        while i < npieces && i << pc.table.log2 < end {
            match next_piece.compare_exchange_weak(
                i,
                i + 1,
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
            ) {
                Ok(_) => {
                    if let Err(e) = pc.verify(i..i + 1) {
                        *piece_err.lock().unwrap() = Some(e);
                    }
                    i += 1;
                }
                Err(now) => i = now,
            }
        }
    };
    let finish = |err: Option<Error>| {
        let decoded = match err {
            Some(e) => Err(e),
            None => Ok(p.info.clone()),
        };
        with_pieces(
            piece_err.lock().unwrap().take().map_or(Ok(()), Err),
            decoded,
        )
    };
    if ng > 1 {
        // Work items are units (stripe, column group): each gets its row
        // slices.
        let bpp = p.bpp();
        let mut units: Vec<Vec<&mut [u8]>> = (0..ns * ng).map(|_| Vec::new()).collect();
        for (y, row) in out[..extent].chunks_mut(stride).enumerate() {
            let si = y / p.hs;
            let mut rest = &mut row[..rb];
            for gi in 0..ng {
                let (_, gw, _) = p.group(gi);
                let (a, b) = rest.split_at_mut(gw * bpp);
                units[si * ng + gi].push(a);
                rest = b;
            }
        }
        let next = std::sync::atomic::AtomicUsize::new(0);
        let slots: Vec<std::sync::Mutex<Vec<&mut [u8]>>> =
            units.into_iter().map(std::sync::Mutex::new).collect();
        let err: std::sync::Mutex<Option<Error>> = std::sync::Mutex::new(None);
        crate::par::scoped_reuse(threads, p.quick, || {
            if jit.is_none() {
                check_pieces();
            }
            loop {
                let u = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if u >= ns * ng {
                    break;
                }
                claim_through(p.stripes[u]);
                let mut rows = core::mem::take(&mut *slots[u].lock().unwrap());
                let r = match observed {
                    Some(observed) => observed
                        .begin(u)
                        .and_then(|()| decode_unit(&p, u, &om, &mut rows)),
                    None => decode_unit(&p, u, &om, &mut rows),
                };
                if let Some(observed) = observed {
                    observed.end(u, &r);
                }
                if let Err(e) = r {
                    *err.lock().unwrap() = Some(e);
                }
            }
            check_pieces();
        });
        return finish(err.into_inner().unwrap());
    }
    // Disjoint row chunks per stripe; each stripe decodes into a zero-based view.
    let chunk_len = stride
        .checked_mul(p.hs)
        .ok_or(Error::InvalidInput("stripe extent overflow"))?;
    if raw {
        // Adjacent quick units share one output band per worker. Raw copies
        // keep sequential addresses; inexpensive units avoid one mutex and
        // one atomic claim per stripe. Packet units retain dynamic scheduling
        // because their spatial reconstruction cost can be very uneven.
        let per_band = ns.div_ceil(threads);
        let band_len = chunk_len
            .checked_mul(per_band)
            .ok_or(Error::InvalidInput("stripe band extent overflow"))?;
        let bands = out[..extent].chunks_mut(band_len).enumerate().collect();
        let err: std::sync::Mutex<Option<Error>> = std::sync::Mutex::new(None);
        crate::par::bands(bands, |(band, rows)| {
            if jit.is_none() {
                check_pieces();
            }
            for (index, stripe) in rows.chunks_mut(chunk_len).enumerate() {
                let si = band * per_band + index;
                claim_through(p.stripes[si]);
                let r = match observed {
                    Some(observed) => observed
                        .begin(si)
                        .and_then(|()| quick_stripe_into(&p, si, &om, stripe, stride)),
                    None => quick_stripe_into(&p, si, &om, stripe, stride),
                };
                if let Some(observed) = observed {
                    observed.end(si, &r);
                }
                if let Err(e) = r {
                    *err.lock().unwrap() = Some(e);
                }
            }
            check_pieces();
        });
        return finish(err.into_inner().unwrap());
    }
    let mut chunks: Vec<(usize, &mut [u8])> =
        out[..extent].chunks_mut(chunk_len).enumerate().collect();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let slots: Vec<std::sync::Mutex<Option<(usize, &mut [u8])>>> = chunks
        .drain(..)
        .map(|c| std::sync::Mutex::new(Some(c)))
        .collect();
    let err: std::sync::Mutex<Option<Error>> = std::sync::Mutex::new(None);
    crate::par::scoped_reuse(threads, p.quick, || {
        if jit.is_none() {
            check_pieces();
        }
        loop {
            let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if i >= ns {
                break;
            }
            claim_through(p.stripes[i]);
            let (si, buf) = slots[i].lock().unwrap().take().unwrap();
            let r = match observed {
                Some(observed) => observed
                    .begin(si)
                    .and_then(|()| decode_stripe(&p, si, &om, buf, stride, &|| Ok(()))),
                None => decode_stripe(&p, si, &om, buf, stride, &|| Ok(())),
            };
            if let Some(observed) = observed {
                observed.end(si, &r);
            }
            if let Err(e) = r {
                *err.lock().unwrap() = Some(e);
            }
        }
        check_pieces();
    });
    finish(err.into_inner().unwrap())
}

/// Region decoding (`crate::region`): the working memory one unit of column
/// groups `g0..g1` needs while it decodes: its prediction state
/// ([`crate::decoder_memory::lossless_state_bytes`], the estimate the decode
/// planner charges) and one row of the group. The widest group counts.
#[cfg(feature = "v3")]
pub(crate) fn region_unit_bytes(p: &Parsed<'_>, g0: usize, g1: usize) -> Result<u64, Error> {
    let slots = p.layout.slot_channel.len() as u128;
    let sample = if p.narrow() { 2 } else { 4 };
    let bpp = p.bpp() as u128;
    let mut most = 0u128;
    for g in g0..g1 {
        let (_, gw, lw) = p.group(g);
        let state = if p.quick {
            crate::quick::state_bytes(&p.info, gw as u128, p.quick_bits)?
        } else {
            crate::decoder_memory::lossless_state_bytes(
                (lw * NL) as u128,
                slots,
                sample,
                p.hs as u128,
                p.pal.is_some(),
                bpp,
                gw as u128,
            )?
        };
        let unit = state
            .checked_add((gw as u128).checked_mul(bpp).ok_or(Error::TooLarge)?)
            .ok_or(Error::TooLarge)?;
        most = most.max(unit);
    }
    u64::try_from(most).map_err(|_| Error::TooLarge)
}

/// Region decoding (`crate::region`): decodes the units of stripes
/// `s0..s1` and column groups `g0..g1` from `units` (their record payloads,
/// stripe-major, then group) and writes the rectangle `(x, y, w, h)` of the
/// image into `out` (rows of `stride` bytes). Rows of a unit below the
/// rectangle are not decoded. Units are decoded on up to `threads` threads.
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "v3")]
pub(crate) fn decode_region_units(
    p: &Parsed<'_>,
    (x, y, w, h): (usize, usize, usize, usize),
    (s0, s1, g0, g1): (usize, usize, usize, usize),
    units: &[&[u8]],
    out: &mut [u8],
    stride: usize,
    threads: usize,
    obs: &crate::region::RegionObs<'_>,
) -> Result<(), Error> {
    let om = OutMap::new(&p.info, &p.layout);
    let bpp = p.bpp();
    let ng = p.ngroups();
    let gn = g1 - g0;
    debug_assert_eq!(units.len(), (s1 - s0) * gn);
    // Row pieces of `out` per unit: the rectangle's columns inside its group.
    let mut pieces: Vec<Vec<&mut [u8]>> = (0..units.len()).map(|_| Vec::new()).collect();
    for (i, row) in out.chunks_mut(stride).take(h).enumerate() {
        let si = (y + i) / p.hs;
        let mut rest = &mut row[..w * bpp];
        for gi in g0..g1 {
            let (gx, gw, _) = p.group(gi);
            let n = (gx + gw).min(x + w) - gx.max(x);
            let (a, b) = rest.split_at_mut(n * bpp);
            pieces[(si - s0) * gn + gi - g0].push(a);
            rest = b;
        }
    }
    let items: Vec<(usize, Vec<&mut [u8]>)> = pieces.into_iter().enumerate().collect();
    let decode = |_, (i, mut rows): (usize, Vec<&mut [u8]>)| -> Result<(), Error> {
        let (si, gi) = (s0 + i / gn, g0 + i % gn);
        let (gx, gw, _) = p.group(gi);
        let col = (gx.max(x) - gx) * bpp;
        let first = y.saturating_sub(si * p.hs);
        let last = (y + h).min((si + 1) * p.hs) - si * p.hs;
        let mut buf = Vec::new();
        buf.try_reserve_exact(gw * bpp)
            .map_err(|_| Error::TooLarge)?;
        buf.resize(gw * bpp, 0);
        let mut sink = Crop {
            buf,
            first,
            last,
            col,
            rows: &mut rows,
        };
        obs.begin(si, gi)?;
        let r = decode_unit_rows(p, &om, si * ng + gi, units[i], &mut sink, &|| obs.check());
        obs.end(si, gi, &r);
        r
    };
    for r in crate::par::par_map(items, threads, decode) {
        r?;
    }
    Ok(())
}

/// Highest available lossless entropy tier, accounting for testing overrides.
/// Individual sample layouts and lossy stages may use different kernels.
#[doc(hidden)]
pub fn simd_tier() -> &'static str {
    if force_scalar() {
        return "scalar";
    }
    #[cfg(target_arch = "x86_64")]
    {
        use archmage::SimdToken;
        if !force_avx2() && archmage::X64V4Token::summon().is_some() {
            return "avx512";
        }
        if archmage::X64V3Token::summon().is_some() {
            return "avx2";
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        use archmage::SimdToken;
        if archmage::NeonToken::summon().is_some() {
            return "neon";
        }
    }
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    {
        use archmage::SimdToken;
        if archmage::Wasm128Token::summon().is_some() {
            return "wasm128";
        }
    }
    "scalar"
}

/// Reads the header and metadata without decoding pixels.
pub fn probe(data: &[u8]) -> Result<(ImageInfo, Metadata), Error> {
    let (info, meta, _) = parse_header(data)?;
    Ok((info, meta))
}

/// Lab: per-slot SIMD utilization of the default encoding:
/// (steps, steps with any coded symbol, coded symbols).
#[cfg(feature = "lab")]
#[doc(hidden)]
pub fn lane_utilization(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
) -> Result<Vec<(u64, u64, u64)>, Error> {
    let cfg = EncodeConfig::default();
    let lanes = default_lanes(info.width);
    let hs = default_stripe_rows(info.width) as usize;
    let ecfg = AnalysisConfig {
        stripe_rows: Some(hs as u32),
        lanes,
        ..Default::default()
    };
    let a = encode::analyze(pixels, stride, info, &ecfg)?;
    let (w, h, lw) = (a.w, a.h, a.sw);
    let widths = lane_widths(w, lw);
    let layout = &a.layout;
    let ns = layout.slot_channel.len();
    let mut out = vec![(0u64, 0u64, 0u64); ns];
    for si in 0..h.div_ceil(hs) {
        let y0 = si * hs;
        let y1 = (y0 + hs).min(h);
        let rows = y1 - y0;
        let pl: Vec<Vec<u32>> = (0..ns)
            .map(|s| step_major(&a.res[s], w, lw, y0, y1))
            .collect();
        for s in 0..ns {
            let depth = layout.slot_bits[s];
            let tc = TokenCfg::v1(
                if depth <= 8 { cfg.k_low } else { cfg.k_high },
                cfg.m,
                depth,
            );
            let cr = if cfg.ctx_cross && s >= 1 && s < layout.color_slots {
                Some(pl[0].as_slice())
            } else {
                None
            };
            let mut busy = vec![false; rows * lw];
            let mut n = 0u64;
            for l in 0..NL {
                let mut v = Vec::new();
                rans::lane_symbols(
                    &pl[s],
                    cr,
                    lw,
                    rows,
                    widths[l] as usize,
                    l,
                    &tc,
                    cfg.run_min,
                    0,
                    &mut v,
                );
                n += v.len() as u64;
                for y in &v {
                    busy[y.step as usize] = true;
                }
            }
            out[s].0 += (rows * lw) as u64;
            out[s].1 += busy.iter().filter(|&&b| b).count() as u64;
            out[s].2 += n;
        }
    }
    Ok(out)
}

#[cfg(all(test, feature = "encode"))]
#[path = "outmap_precision_tests.rs"]
mod outmap_precision_tests;

#[cfg(all(test, feature = "encode"))]
#[path = "near_parity_tests.rs"]
mod near_parity_tests;

#[cfg(all(test, feature = "encode"))]
#[path = "samples_writer_tests.rs"]
mod samples_writer_tests;

#[cfg(all(test, feature = "encode"))]
#[path = "unit_driver_tests.rs"]
mod unit_driver_tests;

#[cfg(all(test, feature = "std", feature = "encode"))]
#[path = "unit_table_tests.rs"]
mod unit_table_tests;

#[cfg(all(test, feature = "encode"))]
#[path = "reduce_spec_tests.rs"]
mod reduce_spec_tests;

#[cfg(all(test, feature = "encode"))]
mod head_features_tests;
