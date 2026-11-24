//! Shared decoder capacity arithmetic and encoder-side geometry planning.
//! Encoder estimates are independent of host pointer width, SIMD tier and
//! thread implementation; admission also accounts for actual parsed ownership.
//! See docs/DECODER_MEMORY.md for the accounting contract and its limitations.

use crate::Error;
#[cfg(feature = "encode")]
use crate::{DecoderMemoryLimit, EncodeConfig, ImageInfo, LossyConfig};

#[cfg(all(test, feature = "encode"))]
mod tests;

const KIB: u128 = 1024;

fn product(values: &[u128]) -> Result<u128, Error> {
    values
        .iter()
        .try_fold(1u128, |n, &v| n.checked_mul(v).ok_or(Error::TooLarge))
}

fn sum(values: &[u128]) -> Result<u128, Error> {
    values
        .iter()
        .try_fold(0u128, |n, &v| n.checked_add(v).ok_or(Error::TooLarge))
}

/// One prediction state. Callers choose actual slots/maps or encoder maxima,
/// and whether palette expansion uses actual output bytes or a maximum.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lossless_state_bytes(
    physical_width: u128,
    slots: u128,
    sample_bytes: u128,
    stripe_rows: u128,
    palette: bool,
    bytes_per_pixel: u128,
    group_width: u128,
) -> Result<u128, Error> {
    let per_pixel = sum(&[
        product(&[sum(&[product(&[4, slots])?, 1])?, sample_bytes])?,
        product(&[12, slots])?,
    ])?;
    let palette_bytes = if palette {
        sum(&[
            product(&[slots, 256, 16, sample_bytes])?,
            product(&[256, bytes_per_pixel])?,
            group_width,
        ])?
    } else {
        0
    };
    sum(&[
        product(&[physical_width, per_pixel])?,
        product(&[4, stripe_rows])?,
        8 * KIB,
        palette_bytes,
    ])
}

/// What a template palette stripe state (SPEC 2.14) adds to
/// [`lossless_state_bytes`]: the palette entries, output table and index row
/// when the payload's charge had no palette expansion (entries are at most
/// 16 bits, two bytes per sample). Its three index rows, typical-row flags
/// and halo plan take less than the ordinary rows that charge covers and a
/// template state does not allocate.
pub(crate) fn template_state_bytes(
    slots: u128,
    palette: bool,
    bytes_per_pixel: u128,
    group_width: u128,
) -> Result<u128, Error> {
    if palette {
        return Ok(0);
    }
    sum(&[
        product(&[slots, 256, 16, 2])?,
        product(&[256, bytes_per_pixel])?,
        group_width,
    ])
}

/// Paired coefficient/EOB vectors, vector growth, and split-Huffman temporaries.
pub(crate) fn lossy_stripe_bytes(group_width: u128, block_rows: u128) -> Result<u128, Error> {
    let blocks = product(&[group_width.div_ceil(8), block_rows])?;
    product(&[
        2,
        sum(&[
            product(&[2, sum(&[product(&[blocks, 3 * 64 * 4 + 3])?, 260])?])?,
            product(&[blocks, 160])?,
            64,
        ])?,
    ])
}

/// Additional live row-stream staging over serial slice allowances. Nested
/// prediction states and actual nested DC stripe conversions are charged by
/// their own lossless/DC hooks. Caller output is excluded.
pub(crate) fn row_stream_bytes(
    width: u128,
    height: u128,
    group_width: u128,
    block_rows: u128,
    sample_bytes: u128,
) -> Result<u128, Error> {
    let rows = product(&[block_rows, 8])?.min(height);
    let bw = width.div_ceil(8);
    let gbw = group_width.div_ceil(8);
    sum(&[
        product(&[2, width, rows, 3, sample_bytes])?, // full-width RGB stripe
        product(&[2, bw, block_rows, 16])?,           // retained full-width DC and AQ
        product(&[2, bw, 16])?,                       // raw DC row and dc_values conversion Vec
        product(&[2, width, sample_bytes])?,          // alpha row
        product(&[2, gbw, block_rows, 16])?,          // group DC/AQ extraction
        4 * KIB, // stream descriptors and short-lived conversion ownership
    ])
}

/// Use u128 throughout planning: even u32 dimensions times row descriptors and
/// workers can overflow u64. No allocations or image-size loops occur here.
#[derive(Clone, Copy)]
#[cfg(feature = "encode")]
struct Shape {
    w: u128,
    h: u128,
    group: u128,
    rows: u128,
}

#[cfg(feature = "encode")]
impl Shape {
    #[cfg(feature = "encode")]
    fn new(info: &ImageInfo, group: u32, rows: u32) -> Self {
        let w = u128::from(info.width);
        Self {
            w,
            h: u128::from(info.height),
            group: if group == 0 {
                w
            } else {
                u128::from(group).min(w)
            },
            rows: u128::from(rows).max(1).min(u128::from(info.height)),
        }
    }

    #[cfg(feature = "encode")]
    fn groups(self) -> u128 {
        self.w.div_ceil(self.group)
    }

    #[cfg(feature = "encode")]
    fn units(self) -> u128 {
        self.h.div_ceil(self.rows) * self.groups()
    }

    #[cfg(feature = "encode")]
    fn workers(self, threads: usize) -> u128 {
        (threads as u128).min(self.units())
    }

    /// 64-bit slice descriptors with Vec growth, outer work-item vectors,
    /// mutex slots, result slots, and simultaneous scheduler staging vectors.
    #[cfg(feature = "encode")]
    fn scheduling(self) -> u128 {
        64 * self.h * self.groups() + 512 * self.units()
    }
}

/// Parsed lossless models + worst-case sample-expansion LUTs. Actual maps are
/// data dependent, so reserve their maximum instead of guessing from bit depth.
/// The encoder's value tables (up to `encode::tables::CAP_WIDE` entries, on
/// words wider than 8 bits) and their expansion tables fit the same
/// allowance.
pub(crate) fn lossless_tables(channels: u128, depth: u32, palette: bool) -> u128 {
    let lut_bits = if depth <= 16 {
        depth.saturating_sub(1)
    } else {
        16
    };
    let models = channels + u128::from(palette);
    // Each DecModel has a 16 KiB table and <=256 token triples. Include model
    // construction temporaries, layout/OutMap vectors and palette's model.
    32 * KIB + models * 24 * KIB + channels * (1u128 << lut_bits) * 4
}

/// One live lossless StripeState, including the extra slot-zero row, palette
/// expansion and (conservatively, even for still images) inter reference rows.
#[cfg(feature = "encode")]
fn lossless_state(s: Shape, channels: u128, depth: u32, lanes: u32, palette: bool) -> u128 {
    let padded = s.group.div_ceil(u128::from(lanes.clamp(1, 16))) * 16;
    let sample = if depth <= 16 { 2 } else { 4 };
    lossless_state_bytes(
        padded,
        channels,
        sample,
        s.rows,
        palette,
        channels * 4,
        s.group,
    )
    .unwrap_or(u128::MAX)
}

#[cfg(feature = "encode")]
fn lossless_bytes(info: &ImageInfo, cfg: &EncodeConfig, threads: usize) -> u128 {
    let group = crate::v1::group_cols(cfg, info.width as usize) as u32;
    let rows = cfg
        .stripe_rows
        .unwrap_or_else(|| crate::v1::default_stripe_rows(info.width));
    let s = Shape::new(info, group, rows);
    let lanes = cfg
        .lanes
        .unwrap_or_else(|| crate::encode::default_lanes(info.width));
    // Coded channels: two 32-bit words per channel for 64-bit samples.
    let ch = (info.channels.count() * info.sample.words()) as u128;
    let depth = info.sample.word_depth();
    let state = lossless_state(s, ch, depth, lanes, cfg.palette);
    // Palette configurations may write template palette stripes (SPEC
    // 2.14), whose state the palette state covers, and their tables.
    let template = if cfg.palette {
        crate::template::TABLE_BYTES as u128
    } else {
        0
    };
    let shared = lossless_tables(ch, depth, cfg.palette) + template + 32 * s.units();
    // StreamDecoder retains every column group's row state concurrently.
    let stream = s.groups() * state;
    let frame = s.workers(threads) * state + s.scheduling();
    shared + stream.max(frame)
}

#[cfg(feature = "encode")]
fn lossy_bytes(info: &ImageInfo, cfg: &LossyConfig, threads: usize) -> u128 {
    let group = crate::lossy::lossy_group_cols(cfg, info.width as usize) as u32;
    let sb = cfg.stripe_rows_for(info.width);
    let s = Shape::new(info, group, sb * 8);
    let bw = s.w.div_ceil(8);
    let gbw = s.group.div_ceil(8);
    let block_rows = u128::from(sb).min(s.h.div_ceil(8));
    let dc_rows = (crate::lossy::dc_stripe_rows(cfg, sb, info.width, info.height) as u128)
        .min(s.h.div_ceil(8));
    let dc = Shape {
        w: bw,
        h: s.h.div_ceil(8),
        group: gbw,
        rows: dc_rows,
    };
    // DC can use I32 + an AQ/keep channel, even when the input is RGB8. It has
    // its own lossless maps/models/palette; do not count it as just four bytes
    // per block. The nested encoder chooses lanes from the full DC width.
    let dc_lanes = crate::encode::default_lanes(info.width.div_ceil(8));
    let dc_state = lossless_state(dc, 4, 32, dc_lanes, true);
    let dc_tables = lossless_tables(4, 32, true) + 32 * dc.units();
    let alpha = info.channels.has_alpha();
    let alpha_state = if alpha {
        lossless_state(
            s,
            1,
            info.sample.depth(),
            crate::encode::default_lanes(info.width),
            true,
        )
    } else {
        0
    };
    let alpha_tables = if alpha {
        lossless_tables(1, info.sample.depth(), true) + 32 * s.units()
    } else {
        0
    };

    // The current rANS and Huffman decoders allocate fixed maximum table
    // arrays. Lowering max_ac_tables does not shrink those allocations.
    let tables = crate::lossy::decoder_table_bytes() + dc_tables + alpha_tables;
    // One transfer table is retained by the parsed image and shared by all
    // workers; custom U8 code grids need it as well as full/custom U16 XYB.
    let shared =
        tables + crate::lossy::decoder_transfer_table_bytes(info, cfg.color) + 32 * s.units();
    let bps = info.sample.bytes() as u128;
    // Two coefficient/eob buffers (paired stripes); reserve Vec growth and
    // transient split/Huffman widths, table ids and unpacked side information.
    let coefficients = lossy_stripe_bytes(s.group, block_rows).unwrap_or(u128::MAX);
    // Both retained DC/aq buffers, raw DC pixels, and dc_values' temporary.
    let dc_buffers = 2 * gbw * dc_rows * (2 * 16 + 16 + 16);
    // Gray/RGBA reconstruction stages RGB and alpha, even for narrow output.
    let staged = 2 * s.group * s.rows * (3 * bps + if alpha { bps } else { 0 });
    // New transform kernels use bounded stack arrays, including rectangular
    // inverse and resampling temporaries. Reserve explicitly in the aggregate
    // plan; compact state/map readers add no heap buffer.
    let subblock_stack = if cfg.channel_layouts != [crate::LossyChannelLayout::default(); 3] {
        16 * KIB
    } else {
        0
    };
    // Exact blocks (when the encoder may use them): the nested block-list
    // image's tables and row state, plus decoded nested stripes: the one each
    // worker is reading and the next one, shared by the workers.
    let (exact_shared, exact_worker) = match crate::lossy::exact::planned(cfg, info) {
        Some(pol) => {
            let pw = 8 * bw.min(crate::lossy::exact::ROW_BLOCKS as u128);
            let rows = (pol.stripe_rows as u128).min(s.h.div_ceil(8) * 8);
            let nested = Shape {
                w: pw,
                h: s.h,
                group: pw,
                rows,
            };
            let stripe = pw * rows * 3;
            (
                lossless_tables(3, 8, true) + 2 * stripe,
                lossless_state(nested, 3, 8, crate::encode::default_lanes(pw as u32), true)
                    + stripe
                    + 8 * gbw * block_rows,
            )
        }
        None => (0, 0),
    };
    let local_tables = if cfg.local_ans_contexts {
        crate::lossy::decoder_table_bytes()
    } else {
        0
    };
    let worker = coefficients
        + local_tables
        + subblock_stack
        + dc_buffers
        + staged
        + dc_state
        + alpha_state
        + exact_worker
        + 64 * s.rows
        + 32 * gbw
        + crate::lossy::decoder_recon_bytes()
        + 16 * KIB;
    let frame = s.workers(threads) * worker + s.scheduling();
    // Streaming stages full-width RGB; grouping only shrinks AC scratch. DC
    // and alpha streams retain all groups' row states, with gathered DC/aq.
    let stream = worker
        + s.groups() * (dc_state + alpha_state)
        + 2 * s.w * s.rows * 3 * bps
        + 2 * bw * block_rows * 16
        + 2 * (bw * 16 + s.w * bps);
    shared + exact_shared + frame.max(stream)
}

#[cfg(feature = "encode")]
fn validate(info: &ImageInfo, limit: Option<DecoderMemoryLimit>) -> Result<usize, Error> {
    if info.width == 0 || info.height == 0 {
        return Err(Error::InvalidInput("empty image"));
    }
    let threads = limit.map_or(1, |l| l.threads);
    if threads == 0 {
        return Err(Error::InvalidInput(
            "decoder memory limit needs at least one thread",
        ));
    }
    Ok(threads)
}

#[cfg(feature = "encode")]
fn fits(bytes: u128, limit: Option<DecoderMemoryLimit>) -> bool {
    bytes <= limit.map_or(u128::from(u64::MAX), |l| u128::from(l.max_bytes))
}

/// Geometric group-width candidates, bounded by 33 iterations even for u32::MAX.
/// Start with the caller's layout; a budget may make its groups narrower.
#[cfg(feature = "encode")]
fn narrower(group: u32, width: u32, block: u32) -> Option<u32> {
    let current = if group == 0 { width } else { group.min(width) };
    if current <= block {
        return None;
    }
    Some((current / 2 / block).max(1) * block)
}

#[cfg(feature = "encode")]
pub(crate) fn plan_lossless(
    info: &ImageInfo,
    cfg: &EncodeConfig,
) -> Result<(EncodeConfig, u64), Error> {
    let threads = validate(info, cfg.decoder_memory_limit)?;
    let bytes = lossless_bytes(info, cfg, threads);
    if fits(bytes, cfg.decoder_memory_limit) {
        return Ok((cfg.clone(), bytes as u64));
    }
    if cfg.decoder_memory_limit.is_none() {
        return Err(Error::TooLarge);
    }
    let mut out = cfg.clone();
    // A smaller lane count pads row arrays to 16*ceil(width/lanes). Restoring
    // 16 lanes reduces that cost without changing any reconstructed sample.
    out.lanes = Some(16);
    for palette in [cfg.palette, false] {
        out.palette = palette;
        let mut group = crate::v1::group_cols(cfg, info.width as usize) as u32;
        loop {
            out.column_groups = Some(group);
            let bytes = lossless_bytes(info, &out, threads);
            if fits(bytes, cfg.decoder_memory_limit) {
                return Ok((out, bytes as u64));
            }
            let Some(next) = narrower(group, info.width, 1) else {
                break;
            };
            group = next;
        }
    }
    Err(Error::InvalidInput(
        "decoder memory limit is too small for this image",
    ))
}

#[cfg(feature = "encode")]
pub(crate) fn plan_lossy(info: &ImageInfo, cfg: &LossyConfig) -> Result<(LossyConfig, u64), Error> {
    let threads = validate(info, cfg.decoder_memory_limit)?;
    let bytes = lossy_bytes(info, cfg, threads);
    if fits(bytes, cfg.decoder_memory_limit) {
        return Ok((cfg.clone(), bytes as u64));
    }
    if cfg.decoder_memory_limit.is_none() {
        return Err(Error::TooLarge);
    }
    let mut out = cfg.clone();
    // Prefer retaining the requested columns/lanes. Reduce stripe height first,
    // then try narrower groups. Entropy, quantization and color stay unchanged.
    let mut group = crate::lossy::lossy_group_cols(cfg, info.width as usize) as u32;
    loop {
        out.column_groups = Some(group);
        out.stripe_block_rows = cfg.stripe_rows_for(info.width);
        loop {
            let bytes = lossy_bytes(info, &out, threads);
            if fits(bytes, cfg.decoder_memory_limit) {
                return Ok((out, bytes as u64));
            }
            if out.stripe_block_rows == 1 {
                break;
            }
            out.stripe_block_rows = out.stripe_block_rows.div_ceil(2);
        }
        let Some(next) = narrower(group, info.width, 8) else {
            break;
        };
        group = next;
    }
    Err(Error::InvalidInput(
        "decoder memory limit is too small for this image",
    ))
}
