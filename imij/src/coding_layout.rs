//! Lossless coded-channel layout shared by all encode/decode drivers.
use crate::Channels;
#[cfg(feature = "encode")]
use crate::mapping::{self};
use crate::mapping::{ChannelMap, MapMode};
use alloc::{vec, vec::Vec};

pub(crate) const CTX_CLASSES: usize = 18;
#[cfg_attr(not(feature = "lab"), allow(dead_code))]
pub(crate) const MAX_CTX: usize = 8;
pub(crate) const NUM_DECOR: u8 = 4;

/// Most coded slots of an image: four channels of two words (64-bit).
pub(crate) const MAX_SLOTS: usize = 8;

/// Coding layout shared by encoder and decoder.
pub(crate) struct Layout {
    /// One map per coded channel: the container channels, or for 64-bit
    /// samples the hi words of every channel, then the lo words.
    pub maps: Vec<ChannelMap>,
    /// Coded channel index of each coded slot.
    pub slot_channel: Vec<usize>,
    pub slot_bits: Vec<u32>,
    /// Number of color slots (1, 3 or 4).
    pub color_slots: usize,
    pub decor_ok: bool,
    /// Near-lossless bound of each slot in coded units (all 0: lossless).
    pub near: Vec<u32>,
    /// The value table of each coded channel with a [`MapMode::Table`] map
    /// (ascending ordered values; SPEC 2.1), empty for every other map; no
    /// entries at all when no channel has a table (read through
    /// [`Layout::table`]; boxed to keep `Layout` small).
    pub tables: Option<alloc::boxed::Box<[Vec<u32>]>>,
}

impl Layout {
    /// `maps` holds one map per coded channel: `channels.count()` maps, or
    /// twice that for 64-bit samples (hi words, then lo words). Slots are
    /// the color channels (G, R, B or Y), then alpha, then for 64-bit
    /// samples the lo words in the same order; a Const coded channel is
    /// not a slot (SPEC 2.2).
    pub(crate) fn new(channels: Channels, maps: Vec<ChannelMap>) -> Self {
        let nch = channels.count();
        let color: &[usize] = match channels {
            Channels::Gray | Channels::GrayAlpha | Channels::Scalar => &[0],
            Channels::Rgb | Channels::Rgba => &[1, 0, 2],
            Channels::Cmyk => &[0, 1, 2, 3],
            Channels::Oklab | Channels::OklabAlpha => &[0, 1, 2],
        };
        let color_slots = color.len();
        let alpha = channels.has_alpha().then_some(nch - 1);
        let mut slot_channel = Vec::with_capacity(maps.len());
        for word in 0..maps.len() / nch {
            for &c in color.iter().chain(alpha.iter()) {
                let cc = word * nch + c;
                if maps[cc].mode != MapMode::Const {
                    slot_channel.push(cc);
                }
            }
        }
        let slot_bits: Vec<u32> = slot_channel.iter().map(|&c| maps[c].bits as u32).collect();
        let decor_ok = matches!(channels, Channels::Rgb | Channels::Rgba)
            && slot_bits[0] == slot_bits[1]
            && slot_bits[0] == slot_bits[2];
        let near = vec![0; slot_channel.len()];
        // No table list unless a channel has one: files without tables
        // allocate nothing for them.
        let tables = None;
        Self {
            maps,
            slot_channel,
            slot_bits,
            color_slots,
            decor_ok,
            near,
            tables,
        }
    }

    /// Sets the value tables, one per coded channel (empty for every map
    /// but [`MapMode::Table`]).
    pub(crate) fn with_tables(mut self, tables: Vec<Vec<u32>>) -> Self {
        debug_assert_eq!(tables.len(), self.maps.len());
        debug_assert!(
            self.maps
                .iter()
                .zip(&tables)
                .all(|(m, t)| (m.mode == MapMode::Table) != t.is_empty())
        );
        if tables.iter().any(|t| !t.is_empty()) {
            self.tables = Some(tables.into_boxed_slice());
        }
        self
    }

    /// Coded channel `c`'s value table (empty unless its map is
    /// [`MapMode::Table`]).
    #[inline]
    pub(crate) fn table(&self, c: usize) -> &[u32] {
        self.tables
            .as_deref()
            .and_then(|t| t.get(c))
            .map_or(&[], Vec::as_slice)
    }

    /// Whether any coded channel has a value table.
    #[inline]
    pub(crate) fn has_tables(&self) -> bool {
        self.tables.is_some()
    }

    /// The coded value of ordered container value `x` in coded channel `c`
    /// (through its table for a [`MapMode::Table`] map).
    #[inline]
    #[cfg(feature = "encode")]
    pub(crate) fn reduce(&self, c: usize, x: u32, depth: u32) -> u32 {
        let m = &self.maps[c];
        if m.mode == MapMode::Table {
            mapping::table_reduce(self.table(c), x)
        } else {
            m.reduce(x, depth)
        }
    }

    /// Sets the near-lossless bounds from per-container-channel values.
    #[cfg(feature = "near-lossless")]
    pub(crate) fn with_near(mut self, per_channel: &[u32]) -> Self {
        self.near = self.slot_channel.iter().map(|&c| per_channel[c]).collect();
        self
    }

    /// True when any slot is coded near-lossless.
    #[inline]
    pub(crate) fn is_near(&self) -> bool {
        cfg!(feature = "near-lossless") && self.near.iter().any(|&d| d != 0)
    }

    /// True when per-lane adaptive pixel color transform (`decor = 4`, SPEC 2.4.1)
    /// is valid for this layout: exact RGB/RGBA whose three color slots are 8-bit.
    #[inline]
    pub(crate) fn adapt_pix_ok(&self) -> bool {
        self.decor_ok && self.slot_bits[0] == 8 && !self.is_near()
    }

    /// Per-container-channel near-lossless bounds (0 for uncoded channels).
    #[cfg(feature = "encode")]
    pub(crate) fn channel_near(&self) -> Vec<u32> {
        let mut out = vec![0; self.maps.len()];
        for (&c, &d) in self.slot_channel.iter().zip(&self.near) {
            out[c] = d;
        }
        out
    }
}

/// Lane width for `lanes` column segments: `ceil(w / lanes)`.
#[cfg(feature = "encode")]
pub(crate) fn seg_width(w: usize, lanes: u32) -> usize {
    w.div_ceil(lanes.clamp(1, 16) as usize).max(1)
}

#[cfg_attr(not(feature = "lab"), allow(dead_code))]
#[inline(always)]
pub(crate) fn ctx_class(s: u32) -> usize {
    ((32 - s.leading_zeros()) as usize).min(CTX_CLASSES - 1)
}
