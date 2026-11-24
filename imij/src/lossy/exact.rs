//! Exact blocks: 8x8 blocks of an 8-bit RGB or RGBA image coded losslessly
//! next to the lossy stream (lossy head record 5; docs/LOSSY.md "Exact
//! blocks").
//!
//! The DC image gets a fourth channel of block codes, 1 = exact and 0 =
//! lossy. An exact block's AC are coded as EOBs and its DC normally; the
//! decoder does not reconstruct it. The exact blocks' source pixels form a
//! list in decode order (unit by unit, raster order inside a unit), packed
//! left to right into rows of B blocks of a nested lossless RGB8 image. The
//! exact head locates everything: every unit's exact block count (so its
//! first list entry) and every nested stripe's size. A nested stripe sits in
//! the exact record of the unit holding its first block, so a streaming
//! decoder has it before it needs it, and a region reader finds the stripes
//! a unit needs, in whichever unit's record, from the head alone.
//!
//! Measurements: benchmarks/lossless_over_lossy_2026-10-07 (the feature),
//! benchmarks/exact_blocks_per_unit_2026-10-11 (the locator).

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ops::Range;

use super::Geom;
#[cfg(feature = "encode")]
use super::coef::Tables;
#[cfg(feature = "encode")]
use super::coef::{self, ZIGZAG, ac_ctx, eob_ctx, rl_token};
#[cfg(feature = "encode")]
use super::{Group, NCH, StripeCoefs};
#[cfg(feature = "encode")]
use crate::rans::M;
#[cfg(feature = "encode")]
use crate::rans::NL;
use crate::{ByteReader, Error};

/// Most blocks per packed row of the nested image.
pub(crate) const ROW_BLOCKS: usize = 64;

/// Where a nested stripe sits: the unit whose exact record carries it, its
/// offset in that record and its size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StripeAt {
    pub(crate) unit: usize,
    pub(crate) offset: usize,
    pub(crate) len: usize,
}

/// Exact blocks of a parsed lossy file.
pub(crate) struct Exact<'a> {
    /// The nested lossless RGB8 block-list image; its stripes are collected
    /// from the unit records.
    pub(crate) img: crate::v1::Parsed<'a>,
    /// Per unit, in decode order: first list entry and exact block count
    /// (from the head).
    pub(crate) ranges: Vec<(usize, usize)>,
    /// Per nested stripe: where it sits (from the head).
    pub(crate) stripes_at: Vec<StripeAt>,
    /// Per unit, in decode order: its exact record (in-memory files).
    pub(crate) records: Vec<&'a [u8]>,
}

impl Exact<'static> {
    /// Parses the exact head of a payload of `units` units, unit `u` having
    /// `unit_blocks(u)` blocks: varint B (blocks per packed row), one varint
    /// per unit (its exact blocks), the nested image's lossless head, one
    /// varint per nested stripe (its bytes); nothing after.
    pub(crate) fn parse_head(
        r: &mut ByteReader<'_>,
        bw: usize,
        units: usize,
        unit_blocks: impl Fn(usize) -> usize,
        admission: &impl crate::parse::limits::Admission,
    ) -> Result<Self, Error> {
        let b = r.var_usize()?;
        if b == 0 || b > bw.min(ROW_BLOCKS) {
            return Err(Error::Corrupt("exact blocks geometry"));
        }
        // Every count takes at least a byte: bound the list before reserving.
        if units > r.remaining() {
            return Err(Error::Corrupt("exact block counts exceed the head"));
        }
        let mut ranges = Vec::new();
        ranges
            .try_reserve_exact(units)
            .map_err(|_| Error::TooLarge)?;
        let mut blocks = 0usize;
        for u in 0..units {
            let n = r.var_usize()?;
            if n > unit_blocks(u) {
                return Err(Error::Corrupt("exact block count"));
            }
            ranges.push((blocks, n));
            blocks += n;
        }
        if blocks == 0 {
            return Err(Error::Corrupt("exact blocks geometry"));
        }
        let info = crate::ImageInfo::new(
            (b * 8) as u32,
            u32::try_from(blocks.div_ceil(b) * 8)
                .map_err(|_| Error::Corrupt("exact blocks geometry"))?,
            crate::Channels::Rgb,
            crate::SampleType::U8,
        );
        // A nested image of a lossy payload (SPEC 2.8): never near-lossless
        // or the float map, never a unit table of its own; an RGB8 image
        // uses no feature.
        let img = crate::v1::parse_head_nested(
            info,
            r,
            admission,
            None,
            crate::parse::header::Features::EMPTY,
        )?;
        if img.gcols != 0 || img.hs % 8 != 0 {
            return Err(Error::Corrupt("exact blocks head"));
        }
        admission.exact(&img)?;
        let mut e = Self {
            img,
            ranges,
            stripes_at: Vec::new(),
            records: Vec::new(),
        };
        let n = e.nested_stripes();
        if n > r.remaining() {
            return Err(Error::Corrupt("exact stripe sizes exceed the head"));
        }
        e.stripes_at
            .try_reserve_exact(n)
            .map_err(|_| Error::TooLarge)?;
        let per = e.per_stripe();
        // Stripe t starts at list entry t * per, in the unit holding it, after
        // the stripes that start earlier in that unit.
        let mut u = 0;
        let mut offset = 0usize;
        for t in 0..n {
            while e.ranges[u].0 + e.ranges[u].1 <= t * per {
                u += 1;
                offset = 0;
            }
            let len = r.var_usize()?;
            e.stripes_at.push(StripeAt {
                unit: u,
                offset,
                len,
            });
            offset = offset
                .checked_add(len)
                .ok_or(Error::Corrupt("exact stripe sizes"))?;
        }
        if r.remaining() != 0 {
            return Err(Error::Corrupt("exact blocks head"));
        }
        Ok(e)
    }
}

impl<'a> Exact<'a> {
    /// Move the owned head after the caller has retained its record spans.
    /// Borrowed unit records are intentionally discarded.
    pub(crate) fn without_records(self) -> Exact<'static> {
        Exact {
            img: self.img.without_records(),
            ranges: self.ranges,
            stripes_at: self.stripes_at,
            records: Vec::new(),
        }
    }

    /// Blocks per packed row.
    pub(crate) fn row_blocks(&self) -> usize {
        self.img.info.width as usize / 8
    }

    /// Blocks per nested stripe.
    pub(crate) fn per_stripe(&self) -> usize {
        self.img.hs / 8 * self.row_blocks()
    }

    /// Nested stripes of the image.
    pub(crate) fn nested_stripes(&self) -> usize {
        (self.img.info.height as usize).div_ceil(self.img.hs)
    }

    /// Nested stripes holding list entries `start .. start + n`.
    pub(crate) fn stripes_of(&self, (start, n): (usize, usize)) -> Range<usize> {
        if n == 0 {
            return 0..0;
        }
        let per = self.per_stripe();
        start / per..(start + n - 1) / per + 1
    }

    /// Nested stripes whose first block is one of unit `u`'s: the ones its
    /// exact record carries, in order.
    pub(crate) fn carried_by(&self, u: usize) -> Range<usize> {
        let (start, n) = self.ranges[u];
        let per = self.per_stripe();
        start.div_ceil(per)..(start + n).div_ceil(per)
    }

    /// Bytes of one decoded nested stripe.
    pub(crate) fn stripe_bytes(&self) -> usize {
        self.img.hs * self.row_blocks() * 24
    }

    /// Region decoding: the working memory a work item holds for the exact
    /// blocks of a unit of `unit_blocks` blocks: one decoded nested stripe,
    /// the unit's block positions and the nested decoder's state.
    #[cfg(feature = "v3")]
    pub(crate) fn item_bytes(&self, unit_blocks: u128) -> Result<u128, Error> {
        let state = u128::from(crate::v1::region_unit_bytes(&self.img, 0, 1)?);
        Ok(self.stripe_bytes() as u128 + 8 * unit_blocks + state)
    }

    /// Splits unit `u`'s exact record into the nested stripes it carries
    /// (pushed to `out` with their indexes); the record must be exactly
    /// those stripes' bytes, as the head sizes them.
    pub(crate) fn split_record<'r>(
        &self,
        rec: &'r [u8],
        u: usize,
        out: &mut Vec<(usize, &'r [u8])>,
    ) -> Result<(), Error> {
        let mut at = 0usize;
        for t in self.carried_by(u) {
            let s = self.stripes_at[t];
            let bytes = at
                .checked_add(s.len)
                .and_then(|end| rec.get(at..end))
                .ok_or(Error::Corrupt("exact record"))?;
            out.try_reserve(1).map_err(|_| Error::TooLarge)?;
            out.push((t, bytes));
            at += s.len;
        }
        if at != rec.len() {
            return Err(Error::Corrupt("exact record"));
        }
        Ok(())
    }

    /// Reads unit `records.len()`'s record (in-memory files).
    pub(crate) fn push_record(&mut self, rec: &'a [u8]) -> Result<(), Error> {
        let u = self.records.len();
        if u >= self.ranges.len() {
            return Err(Error::Corrupt("exact record"));
        }
        let mut parts = Vec::new();
        self.split_record(rec, u, &mut parts)?;
        self.img
            .stripes
            .try_reserve(parts.len())
            .map_err(|_| Error::TooLarge)?;
        self.img.stripes.extend(parts.into_iter().map(|(_, b)| b));
        self.records.try_reserve(1).map_err(|_| Error::TooLarge)?;
        self.records.push(rec);
        Ok(())
    }

    /// After the last record: every unit's record and nested stripe arrived.
    pub(crate) fn finish(&self) -> Result<(), Error> {
        if self.records.len() != self.ranges.len()
            || self.img.stripes.len() != self.nested_stripes()
        {
            return Err(Error::Corrupt("exact blocks missing"));
        }
        Ok(())
    }

    /// Decodes nested stripe `t` (its bytes in `img.stripes`).
    pub(crate) fn decode_stripe(
        &self,
        om: &crate::v1::OutMap,
        t: usize,
        check: &impl Fn() -> Result<(), Error>,
    ) -> Result<Vec<u8>, Error> {
        if t >= self.img.stripes.len() {
            return Err(Error::Corrupt("exact blocks missing"));
        }
        let mut px = alloc_zeroed(self.stripe_bytes())?;
        crate::v1::decode_unit_into_checked(
            &self.img,
            t,
            om,
            &mut px,
            self.row_blocks() * 24,
            check,
        )?;
        Ok(px)
    }

    /// Decodes nested stripe `t` from its record bytes (forward reading:
    /// stripes decode as their records arrive).
    pub(crate) fn decode_stripe_bytes(
        &self,
        om: &crate::v1::OutMap,
        t: usize,
        bytes: &[u8],
        check: &impl Fn() -> Result<(), Error>,
    ) -> Result<Vec<u8>, Error> {
        let mut px = alloc_zeroed(self.stripe_bytes())?;
        let mut sink = crate::v1::Strided {
            out: &mut px,
            stride: self.row_blocks() * 24,
            n: self.row_blocks() * 24,
        };
        crate::v1::decode_unit_rows(&self.img, om, t, bytes, &mut sink, check)?;
        Ok(px)
    }

    /// Unit `u`'s list entries: first entry and count.
    pub(crate) fn range(&self, u: usize) -> Result<(usize, usize), Error> {
        self.ranges
            .get(u)
            .copied()
            .ok_or(Error::Corrupt("exact record"))
    }

    /// Writes the list entries held by nested stripe `t` (decoded pixels
    /// `px`) that belong to a unit (first entry `start`, block positions
    /// `pos`) into its RGB8 rows, `gw` pixels wide.
    pub(crate) fn write<R: RowsMut + ?Sized>(
        &self,
        t: usize,
        px: &[u8],
        start: usize,
        pos: &[(u32, u32)],
        gw: usize,
        rows: &mut R,
    ) {
        let (b, per) = (self.row_blocks(), self.per_stripe());
        let rb = b * 24;
        let lo = start.max(t * per);
        let hi = (start + pos.len()).min((t + 1) * per);
        for i in lo..hi {
            let (r, bx) = pos[i - start];
            let local = i - t * per;
            let (ox, oy) = ((local % b) * 8, (local / b) * 8);
            let x0 = bx as usize * 8;
            let n = ((x0 + 8).min(gw) - x0) * 3;
            for dy in 0..8 {
                let Some(row) = rows.row(r as usize * 8 + dy) else {
                    break;
                };
                row[x0 * 3..][..n].copy_from_slice(&px[(oy + dy) * rb + ox * 3..][..n]);
            }
        }
    }
}

fn alloc_zeroed(n: usize) -> Result<Vec<u8>, Error> {
    let mut v = Vec::new();
    v.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
    v.resize(n, 0);
    Ok(v)
}

/// Mutable RGB8 rows of one unit (its column group), top to bottom.
pub(crate) trait RowsMut {
    /// Row `y` of the unit from its first pixel, or None past its last row.
    fn row(&mut self, y: usize) -> Option<&mut [u8]>;
}

impl RowsMut for [&mut [u8]] {
    fn row(&mut self, y: usize) -> Option<&mut [u8]> {
        self.get_mut(y).map(|r| &mut **r)
    }
}

/// Rows in one buffer: row y at `y * stride + off`.
pub(crate) struct Strided<'b> {
    pub(crate) buf: &'b mut [u8],
    pub(crate) stride: usize,
    pub(crate) off: usize,
    pub(crate) rows: usize,
}

impl RowsMut for Strided<'_> {
    fn row(&mut self, y: usize) -> Option<&mut [u8]> {
        if y >= self.rows {
            return None;
        }
        self.buf.get_mut(y * self.stride + self.off..)
    }
}

/// The exact blocks of a unit of `nby` block rows of `gbw` blocks, in list
/// order, as (block row, block column), from its block scales (`aq`, 0 =
/// exact). The record's count `n` must match.
pub(crate) fn positions(
    aq: &[f32],
    nby: usize,
    gbw: usize,
    n: usize,
    out: &mut Vec<(u32, u32)>,
) -> Result<(), Error> {
    out.clear();
    let aq = aq
        .get(..nby * gbw)
        .ok_or(Error::Corrupt("exact block codes"))?;
    out.try_reserve(n).map_err(|_| Error::TooLarge)?;
    for (b, &s) in aq.iter().enumerate() {
        if s == 0.0 {
            if out.len() == n {
                return Err(Error::Corrupt("exact blocks and records disagree"));
            }
            out.push(((b / gbw) as u32, (b % gbw) as u32));
        }
    }
    if out.len() != n {
        return Err(Error::Corrupt("exact blocks and records disagree"));
    }
    Ok(())
}

/// The nested stripes of one whole-image decode, shared by its work items:
/// each is decoded once, by the first item that needs it, and dropped after
/// the last unit that reads it.
#[cfg(feature = "std")]
pub(crate) struct Shared {
    slots: Vec<std::sync::Mutex<(usize, Option<Arc<Vec<u8>>>)>>,
}

#[cfg(feature = "std")]
impl Shared {
    pub(crate) fn new(e: &Exact<'_>) -> Result<Self, Error> {
        let n = e.nested_stripes();
        let mut users = Vec::new();
        users.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
        users.resize(n, 0usize);
        for &r in &e.ranges {
            for t in e.stripes_of(r) {
                *users
                    .get_mut(t)
                    .ok_or(Error::Corrupt("exact blocks missing"))? += 1;
            }
        }
        let mut slots = Vec::new();
        slots.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
        slots.extend(users.into_iter().map(|u| std::sync::Mutex::new((u, None))));
        Ok(Self { slots })
    }

    fn get(
        &self,
        e: &Exact<'_>,
        om: &crate::v1::OutMap,
        t: usize,
        check: &impl Fn() -> Result<(), Error>,
    ) -> Result<Arc<Vec<u8>>, Error> {
        let slot = self
            .slots
            .get(t)
            .ok_or(Error::Corrupt("exact blocks missing"))?;
        // Held while decoding: other items needing the stripe wait for it.
        let mut g = slot.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(px) = &g.1 {
            return Ok(px.clone());
        }
        let px = Arc::new(e.decode_stripe(om, t, check)?);
        g.1 = Some(px.clone());
        Ok(px)
    }

    fn release(&self, t: usize) {
        if let Some(slot) = self.slots.get(t) {
            let mut g = slot.lock().unwrap_or_else(|p| p.into_inner());
            g.0 = g.0.saturating_sub(1);
            if g.0 == 0 {
                g.1 = None;
            }
        }
    }
}

/// Without threads every work item decodes the nested stripes it needs.
#[cfg(not(feature = "std"))]
pub(crate) struct Shared;

/// Per work item: exact block positions and the nested stripe used last.
#[derive(Default)]
pub(crate) struct ItemState {
    pos: Vec<(u32, u32)>,
    cur: Option<(usize, Arc<Vec<u8>>)>,
}

/// Where [`apply_unit`] finds a unit's decoded nested stripes.
#[derive(Clone, Copy)]
pub(crate) enum Nested<'s> {
    /// Decoded from the parsed image's stripes on first use: once per
    /// decode with `shared`, else once per work item.
    Decode {
        om: &'s crate::v1::OutMap,
        shared: Option<&'s Shared>,
    },
    /// Decoded as their records arrived (forward reading), by index.
    Held(&'s [(usize, Vec<u8>)]),
    /// Their bytes by index (a region read, which fetches only the stripes
    /// its units need), decoded on first use once per work item.
    #[cfg(feature = "v3")]
    Bytes {
        om: &'s crate::v1::OutMap,
        stripes: &'s [(usize, &'s [u8])],
    },
}

/// Writes the exact blocks of a unit (list entries `range`: first entry and
/// count; stripe `s` of a column group with geometry `gg`, block scales
/// `aq`) into its reconstructed RGB8 rows.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_unit<R: RowsMut + ?Sized>(
    e: &Exact<'_>,
    nested: Nested<'_>,
    st: &mut ItemState,
    range: (usize, usize),
    s: usize,
    gg: &Geom,
    aq: &[f32],
    rows: &mut R,
    check: &impl Fn() -> Result<(), Error>,
) -> Result<(), Error> {
    let (by0, by1) = gg.rows(s);
    positions(aq, by1 - by0, gg.bw, range.1, &mut st.pos)?;
    for t in e.stripes_of(range) {
        let (om, shared) = match nested {
            Nested::Held(held) => {
                let (_, px) = held
                    .iter()
                    .find(|(c, _)| *c == t)
                    .ok_or(Error::Corrupt("exact blocks missing"))?;
                e.write(t, px, range.0, &st.pos, gg.w, rows);
                continue;
            }
            #[cfg(feature = "v3")]
            Nested::Bytes { om, stripes } => {
                if !matches!(&st.cur, Some((c, _)) if *c == t) {
                    let (_, bytes) = stripes
                        .iter()
                        .find(|(c, _)| *c == t)
                        .ok_or(Error::Corrupt("exact blocks missing"))?;
                    st.cur = Some((t, Arc::new(e.decode_stripe_bytes(om, t, bytes, check)?)));
                }
                let px = st.cur.as_ref().map(|c| c.1.clone()).unwrap();
                e.write(t, &px, range.0, &st.pos, gg.w, rows);
                continue;
            }
            Nested::Decode { om, shared } => (om, shared),
        };
        let px = match &st.cur {
            Some((c, px)) if *c == t => px.clone(),
            _ => {
                #[cfg(feature = "std")]
                let px = match shared {
                    Some(sh) => sh.get(e, om, t, check)?,
                    None => Arc::new(e.decode_stripe(om, t, check)?),
                };
                #[cfg(not(feature = "std"))]
                let px = {
                    let _ = shared;
                    Arc::new(e.decode_stripe(om, t, check)?)
                };
                st.cur = Some((t, px.clone()));
                px
            }
        };
        e.write(t, &px, range.0, &st.pos, gg.w, rows);
        #[cfg(feature = "std")]
        if let Some(sh) = shared {
            sh.release(t);
        }
    }
    Ok(())
}

// ---- encoder ------------------------------------------------------------------

/// Encoder choice of exact blocks.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg(feature = "encode")]
pub(crate) struct Policy {
    /// Blocks whose estimated lossless bits are at most `ratio` times their
    /// run-level AC bits qualify.
    pub(crate) ratio: f32,
    /// Nested stripe height in pixel rows (a multiple of 8).
    pub(crate) stripe_rows: usize,
}

/// Validated default ratio (VALIDATE tuning, benchmarks/lossless_over_lossy_2026-10-07).
#[cfg(feature = "encode")]
pub(crate) const RATIO: f32 = 1.5;

/// Exact blocks for this encode, or None. Eligible images: 8-bit RGB or
/// RGBA, full-range code values, no AQ, keep blocks or per-channel layouts.
/// `cfg.exact_blocks`: Some(k > 0) = ratio k on any tier, Some(<= 0) = off,
/// None = the tier's default: Compact on (ratio [`RATIO`]); every other tier
/// and no tier off, since their decode budgets do not hold on ARM
/// (benchmarks/lossless_over_lossy_2026-10-07). Nested stripes: 64 rows for
/// Compact, 16 otherwise.
#[cfg(feature = "encode")]
pub(crate) fn policy(
    cfg: &super::LossyConfig,
    info: &crate::ImageInfo,
    eligible: bool,
) -> Option<Policy> {
    use crate::DecodeSpeed;
    if !eligible
        || !matches!(info.channels, crate::Channels::Rgb | crate::Channels::Rgba)
        || info.sample != crate::SampleType::U8
        || info.has_custom_precision()
    {
        return None;
    }
    let compact = cfg.decode_speed == Some(DecodeSpeed::Compact);
    let stripe_rows = if compact { 64 } else { 16 };
    #[cfg(all(feature = "lab", feature = "std"))]
    if let Some(k) = super::block_lab::exact_ratio() {
        return Some(Policy {
            ratio: k,
            stripe_rows: super::block_lab::exact_stripe_rows().unwrap_or(stripe_rows),
        });
    }
    let ratio = match cfg.exact_blocks {
        Some(k) if k > 0.0 && k.is_finite() => k,
        Some(_) => return None,
        None if compact => RATIO,
        None => return None,
    };
    Some(Policy { ratio, stripe_rows })
}

/// The policy an encode with `cfg` may use on a still image like `info`
/// (exact blocks still depend on its content), for memory planning. Declared
/// precision does not matter here: the plan stays conservative.
#[cfg(feature = "encode")]
pub(crate) fn planned(cfg: &super::LossyConfig, info: &crate::ImageInfo) -> Option<Policy> {
    let eligible =
        !super::aq_on_cfg(cfg) && cfg.channel_layouts == [super::LossyChannelLayout::default(); 3];
    let mut full = crate::ImageInfo::new(info.width, info.height, info.channels, info.sample);
    full.cicp = info.cicp;
    policy(cfg, &full, eligible)
}

/// The exact-block mask (raster order) of 8-bit RGB(A) pixels (`bpp` bytes
/// each): blocks whose estimated lossless bits ([`proxy_bits`]) are at most
/// `p.ratio` times their estimated run-level AC bits `bits`. A block stops
/// summing once it passes that limit, so the mask is the one the full sums
/// give at a fraction of their cost.
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "encode")]
pub(crate) fn select_mask(
    px: &[u8],
    stride: usize,
    bpp: usize,
    w: usize,
    h: usize,
    bits: &[f32],
    p: &Policy,
    threads: usize,
) -> Vec<bool> {
    let cost = proxy_costs(px, stride, bpp, w, h, threads);
    select_with(px, stride, bpp, w, h, bits, &cost, p, threads)
}

/// [`select_mask`] under given residual costs ([`proxy_costs_of`]): rows
/// `px` of `h` pixel rows, whose last block row is clipped at `h`.
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "encode")]
pub(super) fn select_with(
    px: &[u8],
    stride: usize,
    bpp: usize,
    w: usize,
    h: usize,
    bits: &[f32],
    cost: &[[f32; 1024]],
    p: &Policy,
    threads: usize,
) -> Vec<bool> {
    let bw = w.div_ceil(8);
    let rows = crate::par::par_map((0..h.div_ceil(8)).collect(), threads, |_, by| {
        let mut mask = alloc::vec![false; bw];
        let mut acc = [0.0f32; 4];
        block_residuals(px, stride, bpp, w, h, by, |b, c, res| {
            if c == 0 {
                acc = [0.0; 4];
            }
            for (i, &v) in res.iter().enumerate() {
                acc[i & 3] += cost[c][v as usize];
            }
            let sum = (acc[0] + acc[1]) + (acc[2] + acc[3]);
            let limit = p.ratio * bits[b];
            if c == 2 {
                mask[b % bw] = sum <= limit;
            }
            sum <= limit
        });
        mask
    });
    rows.concat()
}

/// Estimated lossless bits per 8x8 block (raster order) of 8-bit RGB(A)
/// pixels (`bpp` bytes each): G, R - G, B - G, MED prediction inside the
/// block (as in the packed nested image, where neighbours are unrelated),
/// -log2 p under the image's residual histograms (add-one smoothing).
#[cfg(all(test, feature = "encode"))]
pub(crate) fn proxy_bits(px: &[u8], stride: usize, bpp: usize, w: usize, h: usize) -> Vec<f32> {
    let cost = proxy_costs(px, stride, bpp, w, h, 1);
    let mut out = alloc::vec![0.0f32; w.div_ceil(8) * h.div_ceil(8)];
    let mut acc = [0.0f32; 4];
    for by in 0..h.div_ceil(8) {
        block_residuals(px, stride, bpp, w, h, by, |b, c, res| {
            if c == 0 {
                acc = [0.0; 4];
            }
            for (i, &v) in res.iter().enumerate() {
                acc[i & 3] += cost[c][v as usize];
            }
            out[b] = (acc[0] + acc[1]) + (acc[2] + acc[3]);
            true
        });
    }
    out
}

/// -log2 p of every residual + 512 per channel under the image's residual
/// histograms (add-one smoothing).
#[cfg(feature = "encode")]
fn proxy_costs(
    px: &[u8],
    stride: usize,
    bpp: usize,
    w: usize,
    h: usize,
    threads: usize,
) -> Vec<[f32; 1024]> {
    let mut hist = alloc::vec![[0u64; 1024]; 3];
    proxy_hist(px, stride, bpp, w, h, threads, &mut hist);
    proxy_costs_of(&hist)
}

/// Adds the in-block residual counts of rows `px` (`h` pixel rows, the last
/// block row clipped at `h`) to `hist` (one histogram per channel).
#[cfg(feature = "encode")]
pub(super) fn proxy_hist(
    px: &[u8],
    stride: usize,
    bpp: usize,
    w: usize,
    h: usize,
    threads: usize,
    hist: &mut [[u64; 1024]],
) {
    // Four interleaved histogram copies per channel: flat images repeat
    // one residual, and one counter would serialize the increments.
    let rows: Vec<usize> = (0..h.div_ceil(8)).collect();
    let parts = crate::par::par_fold(
        &rows,
        threads,
        || alloc::vec![[0u32; 1024]; 12],
        |hist, &by| {
            block_residuals(px, stride, bpp, w, h, by, |_, c, res| {
                for (i, &v) in res.iter().enumerate() {
                    hist[c * 4 + (i & 3)][v as usize] += 1;
                }
                true
            })
        },
    );
    for part in parts {
        for (i, p) in part.iter().enumerate() {
            for (a, &b) in hist[i / 4].iter_mut().zip(p) {
                *a += b as u64;
            }
        }
    }
}

/// -log2 p of every residual + 512 per channel under residual histograms
/// `hist` ([`proxy_hist`]; add-one smoothing).
#[cfg(feature = "encode")]
pub(super) fn proxy_costs_of(hist: &[[u64; 1024]]) -> Vec<[f32; 1024]> {
    (0..3)
        .map(|c| {
            let n: [u64; 1024] = core::array::from_fn(|v| 1 + hist[c][v]);
            let tot = n.iter().sum::<u64>() as f64;
            core::array::from_fn(|v| crate::mathx::log2(tot / n[v] as f64) as f32)
        })
        .collect()
}

/// Calls `f(block, channel, residuals)` for every 8x8 block of block row
/// `by` in order and channel G, R - G, B - G in turn, with the channel's in-block MED
/// residuals + 512 (raster order inside the block, clipped at the image
/// edge; left and upper neighbours only inside the block; G starts from
/// 128, the differences from 0). `f` returning false skips the block's
/// remaining channels.
#[cfg(feature = "encode")]
fn block_residuals(
    px: &[u8],
    stride: usize,
    bpp: usize,
    w: usize,
    h: usize,
    by: usize,
    mut f: impl FnMut(usize, usize, &[u16]) -> bool,
) {
    let bw = w.div_ceil(8);
    let mut ch = [[[0i32; 8]; 8]; 3];
    let mut res = [0u16; 64];
    {
        let (y0, bh) = (by * 8, (h - by * 8).min(8));
        for bx in 0..bw {
            let (x0, bwd) = (bx * 8, (w - bx * 8).min(8));
            for dy in 0..bh {
                let row = &px[(y0 + dy) * stride + x0 * bpp..][..bwd * bpp];
                for (dx, p) in row.chunks_exact(bpp).enumerate() {
                    let g = p[1] as i32;
                    ch[0][dy][dx] = g;
                    ch[1][dy][dx] = p[0] as i32 - g;
                    ch[2][dy][dx] = p[2] as i32 - g;
                }
            }
            for (c, v) in ch.iter().enumerate() {
                let start = if c == 0 { 128 } else { 0 };
                let mut k = 0;
                for dy in 0..bh {
                    for dx in 0..bwd {
                        // MED: a + b - d clamped to [min(a, b), max(a, b)],
                        // branch-free (photos make its branches random).
                        let pred = match (dx > 0, dy > 0) {
                            (true, true) => {
                                let (a, b, d) = (v[dy][dx - 1], v[dy - 1][dx], v[dy - 1][dx - 1]);
                                (a + b - d).clamp(a.min(b), a.max(b))
                            }
                            (true, false) => v[dy][dx - 1],
                            (false, true) => v[dy - 1][dx],
                            (false, false) => start,
                        };
                        res[k] = (v[dy][dx] - pred + 512) as u16;
                        k += 1;
                    }
                }
                if !f(by * bw + bx, c, &res[..k]) {
                    break;
                }
            }
        }
    }
}

/// Each block's estimated run-level AC bits (raster order) for choosing
/// exact blocks: the image coded as one stripe (each lane's luma EOB context
/// carried down the whole image) under run-level tables built from that
/// coding at effort 0 with the default 16 AC tables. It depends only on the
/// quantized coefficients, so stripe height, column groups, effort and table
/// limits never change which blocks are exact (they never change decoded
/// pixels). Full-width stripes.
#[cfg(feature = "encode")]
pub(super) fn estimate_bits(stripes: &[StripeCoefs], threads: usize) -> Vec<f32> {
    let bw = stripes.first().map_or(0, |s| s.bw);
    let bh = stripes.iter().map(|s| s.by0 + s.nby).max().unwrap_or(0);
    let mode = super::LossyEntropy::RunLevel;
    let lanes: Vec<usize> = (0..NL.min(bw)).collect();
    // Lanes are independent context chains: histogram and bits per lane.
    let hists = crate::par::par_fold(
        &lanes,
        threads,
        || alloc::vec![0u64; mode.nraw() * coef::ALPHA],
        |hist, &l| {
            walk(stripes, l, |_, ctx, tok, _| {
                hist[ctx * coef::ALPHA + tok as usize] += 1
            })
        },
    );
    let mut hist = alloc::vec![alloc::vec![0u64; coef::ALPHA]; mode.nraw()];
    for h in hists {
        for (i, n) in h.into_iter().enumerate() {
            hist[i / coef::ALPHA][i % coef::ALPHA] += n;
        }
    }
    let cost = token_costs(&hist);
    let per_lane = crate::par::par_map(lanes, threads, |_, l| lane_bits(stripes, l, &mut 0, &cost));
    let mut bits = alloc::vec![0.0f32; bw * bh];
    for v in per_lane {
        for (b, sum) in v {
            bits[b as usize] = sum;
        }
    }
    bits
}

/// Token costs of the run-level histograms `hist` of the one-stripe coding
/// ([`walk`]): `log2(M / freq)` under the tables effort 0 builds with the
/// default 16 AC tables.
#[cfg(feature = "encode")]
pub(super) fn token_costs(hist: &[Vec<u64>]) -> Vec<[f32; coef::ALPHA]> {
    let mode = super::LossyEntropy::RunLevel;
    let effort = crate::effort::Effort::new(0.0).expect("effort 0 is valid");
    let tables = Tables::build_with_effort(hist, 4, 16, mode, effort);
    (0..mode.nraw())
        .map(|ctx| {
            let t = tables.ctx_map[ctx] as usize;
            core::array::from_fn(|tok| {
                let f = tables.freq[t][tok].max(1);
                crate::mathx::log2(M as f64 / f as f64) as f32
            })
        })
        .collect()
}

/// Every block's bits of lane `l` of full-width `stripes` under `cost`, as
/// (image block, bits) in coding order, the lane's luma EOB context starting
/// from `prev_luma` and left there.
#[cfg(feature = "encode")]
pub(super) fn lane_bits(
    stripes: &[StripeCoefs],
    l: usize,
    prev_luma: &mut u32,
    cost: &[[f32; coef::ALPHA]],
) -> Vec<(u32, f32)> {
    let mut v: Vec<(u32, f32)> = Vec::new();
    walk_from(stripes, l, prev_luma, |b, ctx, tok, raw| {
        let c = cost[ctx][tok as usize] + raw as f32;
        match v.last_mut() {
            Some((lb, sum)) if *lb as usize == b => *sum += c,
            _ => v.push((b as u32, c)),
        }
    });
    v
}

/// Adds lane `l`'s run-level token counts of full-width `stripes` to `hist`
/// (`nraw * ALPHA`, context-major), its luma EOB context starting from
/// `prev_luma` and left there.
#[cfg(feature = "encode")]
pub(super) fn lane_hist(stripes: &[StripeCoefs], l: usize, prev_luma: &mut u32, hist: &mut [u64]) {
    walk_from(stripes, l, prev_luma, |_, ctx, tok, _| {
        hist[ctx * coef::ALPHA + tok as usize] += 1
    });
}

/// Calls `f(block, ctx, token, raw bits)` for every run-level EOB and AC
/// token of lane `l` (block columns bx = l mod 16) of full-width stripes,
/// coded as one stripe: block rows top to bottom, the lane's luma EOB
/// context continuing across stripes.
#[cfg(feature = "encode")]
fn walk(stripes: &[StripeCoefs], l: usize, f: impl FnMut(usize, usize, u32, u32)) {
    walk_from(stripes, l, &mut 0, f)
}

/// [`walk`] with the lane's luma EOB context starting from `prev_luma` and
/// left there, for stripes coded band by band.
#[cfg(feature = "encode")]
fn walk_from(
    stripes: &[StripeCoefs],
    l: usize,
    prev_luma: &mut u32,
    mut f: impl FnMut(usize, usize, u32, u32),
) {
    let grp = Group::ALL;
    let bw = stripes.first().map_or(0, |s| s.bw);
    for sc in stripes {
        for r in 0..sc.nby {
            for bx in (l..bw).step_by(NL) {
                let b = r * bw + bx;
                let gb = (sc.by0 + r) * bw + bx;
                let mut luma = 0u32;
                for c in 0..grp.nch {
                    let ch = grp.ch0 + c;
                    let q: &[i32; 64] = sc.q[(b * NCH + c) * 64..][..64].try_into().unwrap();
                    let ac = sc.nz[b * NCH + c] & !1;
                    let eob = if ac == 0 { 0 } else { 63 - ac.leading_zeros() };
                    f(
                        gb,
                        eob_ctx(ch, if c == 0 { *prev_luma } else { luma }),
                        eob,
                        0,
                    );
                    if c == 0 {
                        luma = eob;
                        *prev_luma = eob;
                    }
                    let mut start = 1u32;
                    let mut nzb = ac;
                    while nzb != 0 {
                        let k = nzb.trailing_zeros();
                        nzb &= nzb - 1;
                        let v = q[ZIGZAG[k as usize] as usize];
                        let mut run = k - start;
                        while run > coef::MAX_RUN {
                            f(gb, ac_ctx(ch, start, eob), coef::ZRL, 0);
                            start += 4;
                            run -= 4;
                        }
                        let (t, _, n) = rl_token(run, v);
                        f(gb, ac_ctx(ch, start, eob), t, n);
                        start = k + 1;
                    }
                }
            }
        }
    }
}

/// Each block's run-level bits under `tables` (raster order): every EOB and
/// AC token's `log2(M / freq)` plus its raw bits, three planes. The DC lives
/// in the nested DC image and is not counted. Full-width stripes only.
#[cfg(all(feature = "lab", feature = "std"))]
pub(super) fn block_bits(stripes: &[StripeCoefs], tables: &Tables) -> Vec<f32> {
    let bw = stripes.first().map_or(0, |s| s.bw);
    let bh = stripes.iter().map(|s| s.by0 + s.nby).max().unwrap_or(0);
    let mut out = alloc::vec![0.0f32; bw * bh];
    let cost = |ctx: usize, tok: u32| -> f64 {
        let t = tables.ctx_map[ctx] as usize;
        let f = tables.freq[t][tok as usize].max(1);
        crate::mathx::log2(M as f64 / f as f64)
    };
    let grp = Group::ALL;
    for sc in stripes {
        debug_assert_eq!(sc.bw, bw);
        for l in 0..NL {
            let mut prev_luma = 0u32;
            for r in 0..sc.nby {
                for bx in (l..bw).step_by(NL) {
                    let b = r * bw + bx;
                    let mut bits = 0f64;
                    let mut luma = 0u32;
                    for c in 0..grp.nch {
                        let ch = grp.ch0 + c;
                        let q: &[i32; 64] = sc.q[(b * NCH + c) * 64..][..64].try_into().unwrap();
                        let ac = sc.nz[b * NCH + c] & !1;
                        let eob = if ac == 0 { 0 } else { 63 - ac.leading_zeros() };
                        bits += cost(eob_ctx(ch, if c == 0 { prev_luma } else { luma }), eob);
                        if c == 0 {
                            luma = eob;
                            prev_luma = eob;
                        }
                        let mut start = 1u32;
                        let mut nzb = ac;
                        while nzb != 0 {
                            let k = nzb.trailing_zeros();
                            nzb &= nzb - 1;
                            let v = q[ZIGZAG[k as usize] as usize];
                            let mut run = k - start;
                            while run > coef::MAX_RUN {
                                bits += cost(ac_ctx(ch, start, eob), coef::ZRL);
                                start += 4;
                                run -= 4;
                            }
                            let (t, _, n) = rl_token(run, v);
                            bits += cost(ac_ctx(ch, start, eob), t) + n as f64;
                            start = k + 1;
                        }
                    }
                    out[(sc.by0 + r) * bw + bx] = bits as f32;
                }
            }
        }
    }
    out
}

/// Drops the AC coefficients of `mask`ed blocks (full-width stripes, raster
/// block order), keeping their DC, and sets the stripes' block codes (1 =
/// exact, 0 = lossy).
#[cfg(feature = "encode")]
pub(super) fn mark(stripes: &mut [StripeCoefs], mask: &[bool]) {
    mark_from(stripes, mask, 0)
}

/// [`mark`] with `mask` starting at block row `by0`.
#[cfg(feature = "encode")]
pub(super) fn mark_from(stripes: &mut [StripeCoefs], mask: &[bool], by0: usize) {
    for sc in stripes.iter_mut() {
        let b0 = (sc.by0 - by0) * sc.bw;
        for (b, &m) in mask[b0..b0 + sc.nby * sc.bw].iter().enumerate() {
            if m {
                for c in 0..NCH {
                    let o = (b * NCH + c) * 64;
                    sc.q[o + 1..o + 64].fill(0);
                    sc.nz[b * NCH + c] &= 1;
                }
            }
        }
        sc.aq = mask[b0..b0 + sc.nby * sc.bw]
            .iter()
            .map(|&m| m as u8)
            .collect();
    }
}

/// The nested block-list image of the `mask`ed blocks of 8-bit RGB(A)
/// pixels (`bpp` bytes each), in unit order (`sb` block rows per stripe,
/// column groups of `gcols` pixels, 0 = none): returns the exact head (with
/// every unit's count and every nested stripe's size) and one record per
/// unit (see the module docs). Partial edge blocks repeat
/// the image edge; the last packed row is zero padded.
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "encode")]
pub(super) fn encode(
    px: &[u8],
    stride: usize,
    bpp: usize,
    geom: &Geom,
    gcols: usize,
    mask: &[bool],
    stripe_rows: usize,
    effort: f32,
    threads: usize,
) -> Result<(Vec<u8>, Vec<Vec<u8>>), Error> {
    let (w, h, bw, bh, sb) = (geom.w, geom.h, geom.bw, geom.bh, geom.sb);
    let gb = if gcols == 0 { bw } else { gcols / 8 };
    let ng = bw.div_ceil(gb);
    let nst = bh.div_ceil(sb);
    // List order: unit by unit, raster order inside a unit.
    let mut list = Vec::new();
    let mut counts = Vec::with_capacity(nst * ng);
    for s in 0..nst {
        for gi in 0..ng {
            let before = list.len();
            for by in s * sb..((s + 1) * sb).min(bh) {
                for bx in gi * gb..((gi + 1) * gb).min(bw) {
                    if mask[by * bw + bx] {
                        list.push((bx, by));
                    }
                }
            }
            counts.push(list.len() - before);
        }
    }
    let mut recs: Vec<Vec<u8>> = alloc::vec![Vec::new(); nst * ng];
    if list.is_empty() {
        return Ok((Vec::new(), recs));
    }
    let b = bw.min(ROW_BLOCKS);
    let (pw, ph) = (8 * b, 8 * list.len().div_ceil(b));
    let mut img = alloc::vec![0u8; pw * ph * 3];
    for (i, &(bx, by)) in list.iter().enumerate() {
        let (ox, oy) = ((i % b) * 8, (i / b) * 8);
        for dy in 0..8 {
            let y = (by * 8 + dy).min(h - 1);
            for dx in 0..8 {
                let x = (bx * 8 + dx).min(w - 1);
                let o = ((oy + dy) * pw + ox + dx) * 3;
                img[o..o + 3].copy_from_slice(&px[y * stride + x * bpp..][..3]);
            }
        }
    }
    let info = crate::ImageInfo::new(
        pw as u32,
        ph as u32,
        crate::Channels::Rgb,
        crate::SampleType::U8,
    );
    let cfg = crate::EncodeConfig {
        effort,
        stripe_rows: Some(stripe_rows as u32),
        threads,
        column_groups: Some(0),
        ..Default::default()
    };
    // Explicit tables: the seed byte has no selector bit for this image.
    let (head, parts, _) = crate::seeds::with_role(crate::seeds::Role::Exact, || {
        crate::v1::encode_parts_seeded(&img, pw * 3, &info, &cfg, None)
    })?;
    // The head: B, every unit's count, the nested head, every stripe's
    // size. A unit's record: the stripes whose first block is one of its own.
    let per = stripe_rows / 8 * b;
    let mut hd = Vec::with_capacity(head.len() + 2 * counts.len() + 3 * parts.len() + 2);
    crate::put_varint(&mut hd, b as u64);
    for &n in &counts {
        crate::put_varint(&mut hd, n as u64);
    }
    hd.extend_from_slice(&head);
    for part in &parts {
        crate::put_varint(&mut hd, part.len() as u64);
    }
    let mut parts = parts.into_iter().enumerate().peekable();
    let mut start = 0;
    for (rec, &n) in recs.iter_mut().zip(&counts) {
        while let Some((_, part)) = parts.next_if(|(t, _)| t * per < start + n) {
            rec.extend_from_slice(&part);
        }
        start += n;
    }
    debug_assert!(parts.next().is_none());
    Ok((hd, recs))
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::parse::limits::Unrestricted;

    /// An exact head over units with `counts` exact blocks (each of at most
    /// 64 blocks), `b` per packed row, 16-row nested stripes of the given
    /// `sizes`.
    fn head(counts: &[usize], b: usize, sizes: &[usize]) -> Vec<u8> {
        // The nested head of at least one packed row (a head with no exact
        // block is refused before the nested head matters).
        let blocks: usize = counts.iter().sum::<usize>().max(1);
        let rows = blocks.div_ceil(b) * 8;
        let px = alloc::vec![7u8; b * 8 * rows * 3];
        let info = crate::ImageInfo::new(
            (b * 8) as u32,
            rows as u32,
            crate::Channels::Rgb,
            crate::SampleType::U8,
        );
        let cfg = crate::EncodeConfig {
            stripe_rows: Some(16),
            column_groups: Some(0),
            ..Default::default()
        };
        let (nested, ..) = crate::v1::encode_parts(&px, b * 24, &info, &cfg).unwrap();
        let mut h = Vec::new();
        crate::put_varint(&mut h, b as u64);
        for &n in counts {
            crate::put_varint(&mut h, n as u64);
        }
        h.extend_from_slice(&nested);
        for &l in sizes {
            crate::put_varint(&mut h, l as u64);
        }
        h
    }

    fn parse(h: &[u8], units: usize) -> Result<Exact<'static>, Error> {
        Exact::parse_head(&mut ByteReader::new(h), 64, units, |_| 64, &Unrestricted)
    }

    #[test]
    fn the_head_places_every_stripe() {
        // Units of 0, 10, 2 and 8 exact blocks, 4 per packed row, 16-row
        // nested stripes: 8 entries per stripe, stripes start at entries 0,
        // 8 and 16, in units 1, 1 and 3.
        let e = parse(&head(&[0, 10, 2, 8], 4, &[5, 6, 7]), 4).unwrap();
        assert_eq!((e.per_stripe(), e.nested_stripes()), (8, 3));
        assert_eq!(e.ranges, [(0, 0), (0, 10), (10, 2), (12, 8)]);
        let at = |unit, offset, len| StripeAt { unit, offset, len };
        assert_eq!(e.stripes_at, [at(1, 0, 5), at(1, 5, 6), at(3, 0, 7)]);
        assert_eq!(
            (0..4).map(|u| e.carried_by(u)).collect::<Vec<_>>(),
            [0..0, 0..2, 2..2, 2..3]
        );
        // Unit 3 reads stripes 1 and 2, the first carried by unit 1.
        assert_eq!(e.stripes_of(e.ranges[3]), 1..3);
        // Records hold exactly their stripes.
        let mut out = Vec::new();
        e.split_record(b"aaaaabbbbbb", 1, &mut out).unwrap();
        assert_eq!(out, [(0, &b"aaaaa"[..]), (1, b"bbbbbb")]);
        for (rec, u) in [
            (&b"aaaaabbbbb"[..], 1),
            (b"aaaaabbbbbbb", 1),
            (b"x", 0),
            (b"x", 2),
        ] {
            assert!(matches!(
                e.split_record(rec, u, &mut Vec::new()),
                Err(Error::Corrupt("exact record"))
            ));
        }
        e.split_record(&[], 2, &mut Vec::new()).unwrap();
    }

    #[test]
    fn heads_out_of_bounds_are_corrupt() {
        let good = head(&[3, 5], 4, &[9]);
        parse(&good, 2).unwrap();
        // A count above the unit's blocks, no exact block at all, a missing
        // or extra stripe size, a unit count the head cannot hold.
        let over = Exact::parse_head(&mut ByteReader::new(&good), 64, 2, |_| 4, &Unrestricted);
        assert!(matches!(over, Err(Error::Corrupt(_))));
        for h in [
            head(&[0, 0], 4, &[]),
            head(&[3, 5], 4, &[]),
            head(&[3, 5], 4, &[9, 9]),
        ] {
            assert!(matches!(parse(&h, 2), Err(Error::Corrupt(_))), "{h:?}");
        }
        assert!(matches!(parse(&good, 1000), Err(Error::Corrupt(_))));
    }

    #[test]
    fn missing_records_or_stripes_are_corrupt() {
        let mut e = parse(&head(&[0, 10, 2, 8], 4, &[2, 2, 2]), 4).unwrap();
        e.push_record(&[]).unwrap();
        e.push_record(b"xxyy").unwrap();
        // 2 of 4 records and 2 of 3 stripes arrived.
        assert!(matches!(e.finish(), Err(Error::Corrupt(_))));
        e.push_record(&[]).unwrap();
        e.push_record(b"zz").unwrap();
        assert!(e.finish().is_ok());
        assert!(e.push_record(&[]).is_err());
        // Decoding a nested stripe that is not lossless data fails cleanly.
        let om = crate::v1::OutMap::new(&e.img.info, &e.img.layout);
        assert!(e.decode_stripe(&om, 0, &|| Ok(())).is_err());
        assert!(matches!(
            e.decode_stripe(&om, 3, &|| Ok(())),
            Err(Error::Corrupt(_))
        ));
    }

    /// The per-pixel reference of [`proxy_bits`].
    fn proxy_reference(px: &[u8], stride: usize, bpp: usize, w: usize, h: usize) -> Vec<f64> {
        let bw = w.div_ceil(8);
        let chan = |x: usize, y: usize, c: usize| -> i32 {
            let p = &px[y * stride + x * bpp..][..3];
            let g = p[1] as i32;
            [g, p[0] as i32 - g, p[2] as i32 - g][c]
        };
        let mut res = Vec::new();
        let mut hist = alloc::vec![[1u64; 1024]; 3];
        for y in 0..h {
            for x in 0..w {
                for c in 0..3 {
                    let (inl, inu) = (x % 8 != 0, y % 8 != 0);
                    let a = if inl { chan(x - 1, y, c) } else { 0 };
                    let b = if inu { chan(x, y - 1, c) } else { 0 };
                    let d = if inl && inu { chan(x - 1, y - 1, c) } else { 0 };
                    let pred = match (inl, inu) {
                        (true, true) if d >= a.max(b) => a.min(b),
                        (true, true) if d <= a.min(b) => a.max(b),
                        (true, true) => a + b - d,
                        (true, false) => a,
                        (false, true) => b,
                        _ if c == 0 => 128,
                        _ => 0,
                    };
                    let e = (chan(x, y, c) - pred + 512) as usize;
                    hist[c][e] += 1;
                    res.push(((y / 8) * bw + x / 8, c, e));
                }
            }
        }
        let tot: Vec<f64> = hist.iter().map(|h| h.iter().sum::<u64>() as f64).collect();
        let mut out = alloc::vec![0.0; bw * h.div_ceil(8)];
        for (b, c, e) in res {
            out[b] += crate::mathx::log2(tot[c] / hist[c][e] as f64);
        }
        out
    }

    #[test]
    fn proxy_matches_the_per_pixel_reference() {
        let mut s = 0x1234_5678u32;
        for (w, h, bpp) in [
            (1usize, 1usize, 3usize),
            (13, 9, 3),
            (40, 21, 4),
            (64, 64, 3),
        ] {
            let stride = w * bpp + 5;
            let px: Vec<u8> = (0..stride * h)
                .map(|i| {
                    s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    // Flat runs and noise.
                    if (i / 17) % 3 == 0 {
                        200
                    } else {
                        (s >> 24) as u8
                    }
                })
                .collect();
            let fast = proxy_bits(&px, stride, bpp, w, h);
            let reference = proxy_reference(&px, stride, bpp, w, h);
            assert_eq!(fast.len(), reference.len());
            for (a, b) in fast.iter().zip(&reference) {
                assert!(
                    (*a as f64 - b).abs() <= 1e-3 * b.max(1.0),
                    "{w}x{h}: {a} vs {b}"
                );
            }
        }
    }

    #[test]
    fn early_exit_mask_equals_the_full_sums() {
        let mut s = 0x9E37_79B9u32;
        let mut rnd = || {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            s
        };
        for (w, h, bpp) in [(13usize, 9usize, 3usize), (64, 40, 4), (100, 64, 3)] {
            let stride = w * bpp + 3;
            let px: Vec<u8> = (0..stride * h)
                .map(|i| if (i / 24) % 4 == 0 { 30 } else { rnd() as u8 })
                .collect();
            let full = proxy_bits(&px, stride, bpp, w, h);
            let bits: Vec<f32> = full
                .iter()
                .map(|&p| p * (rnd() % 300) as f32 / 200.0)
                .collect();
            for ratio in [0.5f32, 1.0, 1.5, 3.0] {
                let p = Policy {
                    ratio,
                    stripe_rows: 16,
                };
                let want: Vec<bool> = full
                    .iter()
                    .zip(&bits)
                    .map(|(&x, &a)| x <= ratio * a)
                    .collect();
                for threads in [1, 3, 8] {
                    assert_eq!(
                        select_mask(&px, stride, bpp, w, h, &bits, &p, threads),
                        want,
                        "{w}x{h} k{ratio} {threads} threads"
                    );
                }
            }
        }
    }

    #[test]
    fn heads_out_of_range_are_corrupt() {
        // B of 0, above 64, above the image's blocks per row.
        for b in [0u64, 65, 9] {
            let mut h = Vec::new();
            crate::put_varint(&mut h, b);
            crate::put_varint(&mut h, 5);
            assert!(
                matches!(
                    Exact::parse_head(&mut ByteReader::new(&h), 8, 1, |_| 64, &Unrestricted),
                    Err(Error::Corrupt(_))
                ),
                "B {b}"
            );
        }
    }

    #[test]
    fn positions_check_codes_against_counts() {
        let mut pos = Vec::new();
        let aq = [1.0, 0.0, 1.0, 0.0, 0.0, 1.0];
        positions(&aq, 2, 3, 3, &mut pos).unwrap();
        assert_eq!(pos, [(0, 1), (1, 0), (1, 1)]);
        for n in [2, 4] {
            assert!(matches!(
                positions(&aq, 2, 3, n, &mut pos),
                Err(Error::Corrupt(_))
            ));
        }
        // Fewer codes than blocks.
        assert!(positions(&aq[..5], 2, 3, 2, &mut pos).is_err());
    }

    #[test]
    fn exact_codes_other_than_zero_and_one_are_corrupt() {
        // One block of an RGBA U16 DC image: DCs + code.
        for (code, ok) in [(0u16, Some(1.0f32)), (1, Some(0.0)), (2, None), (64, None)] {
            let px: Vec<u8> = [32768u16, 32768, 32768, code]
                .iter()
                .flat_map(|v| v.to_ne_bytes())
                .collect();
            let (mut dc, mut sc) = (Vec::new(), Vec::new());
            let r =
                super::super::dc_values(crate::SampleType::U16, true, true, &px, &mut dc, &mut sc);
            match ok {
                Some(s) => {
                    r.unwrap();
                    assert_eq!(sc, [s]);
                }
                None => assert!(matches!(r, Err(Error::Corrupt(_)))),
            }
        }
    }
}
