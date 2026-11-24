//! Value-table channel maps for the encoder (SPEC 2.1, map mode 9): which
//! coded channels code each sample as its index in the ascending set of
//! values the channel uses.
//!
//! A channel is a candidate when its words are wider than 8 bits, it is
//! coded (not Const), exact (no near-lossless bound), its values leave gaps
//! in its regular map's coded range, and it uses at most [`cap`] distinct
//! values. The choice compares the
//! existing predictor and entropy estimates on sampled windows, scaled to the
//! image, plus each table's exact bytes: no trial encoding, no change to
//! stored sample values.
use super::{AnalysisConfig, Layout, analyze_stripe_fast, seg_width};
use crate::mapping::{self, ChannelMap, MapMode};
use crate::{Error, ImageInfo};
use alloc::{vec, vec::Vec};

/// Most entries the encoder writes in the table of a 16- or 32-bit channel.
/// On the 2026-10-15 study (experiments/value_table_2026-10-15) tables of up
/// to 2^14 entries keep nearly all of the gain of unlimited tables (SI-HDR
/// 0.713 against 0.709 of plain size, ITM 0.926 against 0.925); 2^12 keeps
/// less (0.748, 0.936). A table and its expansion table then fit the
/// decoder memory estimate's existing per-channel allowance.
pub(crate) const CAP_WIDE: usize = 1 << 14;

/// LAB: the largest table above 8 bits (0: [`CAP_WIDE`]), for measuring
/// decode speed against table size (`lab::set_value_table_cap`).
#[cfg(feature = "lab")]
pub(crate) static LAB_CAP: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

/// Most table entries the encoder writes for a coded channel `depth` bits deep.
pub(crate) fn cap(depth: u32) -> usize {
    if depth <= 8 {
        return 1 << depth;
    }
    #[cfg(feature = "lab")]
    {
        let lab = LAB_CAP.load(core::sync::atomic::Ordering::Relaxed);
        if lab != 0 {
            return lab.min(mapping::MAX_TABLE);
        }
    }
    CAP_WIDE
}

/// Chooses value tables for `maps` (ordered, unreduced `planes`; `near`: each
/// coded channel's near-lossless bound). Rewrites the chosen maps to
/// [`MapMode::Table`] and returns one table per coded channel (empty where
/// none was chosen).
pub(super) fn choose(
    info: &ImageInfo,
    cfg: &AnalysisConfig,
    planes: &[Vec<u32>],
    maps: &mut [ChannelMap],
    near: &[u32],
    check: &dyn Fn() -> Result<(), Error>,
) -> Result<Vec<Vec<u32>>, Error> {
    let depth = info.sample.word_depth();
    // 8-bit words keep their regular maps (palette stripes and the
    // algebraic maps cover their sparse values): on the 772-image low-bit
    // corpus tables gained nothing in aggregate (0.9998 of the bytes) and
    // made 181 images larger, up to 1.35x on 16x16 icons; KADID-10k was the
    // only 8-bit set to gain (1.3%). The lab's forced mode still writes them.
    let forced = crate::v1::value_tables_forced();
    if depth <= 8 && !forced {
        return Ok(vec![Vec::new(); maps.len()]);
    }
    let mut tables: Vec<Option<Vec<u32>>> = Vec::with_capacity(maps.len());
    for (c, m) in maps.iter().enumerate() {
        check()?;
        let candidate = m.mode != MapMode::Const && near[c] == 0;
        tables.push(if candidate {
            distinct(&planes[c], depth, cap(depth), check)?
                .filter(|t| m.bits >= 32 || t.len() < 1usize << m.bits)
        } else {
            None
        });
    }
    let none = || vec![Vec::new(); maps.len()];
    if tables.iter().all(Option::is_none) {
        return Ok(none());
    }
    let width = info.width as usize;
    let height = info.height as usize;
    let windows = windows(width, height);
    let sampled: usize = windows.iter().map(|w| w.width * w.height).sum();
    let scale = (width * height) as f64 / sampled as f64;
    // Estimated bytes of the image with the tables in `chosen`.
    let cost = |chosen: &[bool]| -> Result<f64, Error> {
        check()?;
        let mut m = maps.to_vec();
        let mut t = none();
        let mut bytes = 0.0;
        for (c, table) in tables.iter().enumerate() {
            if let (true, Some(table)) = (chosen[c], table) {
                m[c] = table_map(table.len());
                bytes += table_bytes(table) as f64;
                t[c] = table.clone();
            }
        }
        let layout = Layout::new(info.channels, m).with_tables(t);
        let bits = sample_bits(&layout, planes, width, depth, cfg, &windows);
        Ok(bytes + bits * scale / 8.0)
    };
    let candidates: Vec<usize> = (0..maps.len()).filter(|&c| tables[c].is_some()).collect();
    let mut chosen: Vec<bool> = tables.iter().map(Option::is_some).collect();
    if forced {
        return Ok(apply(tables, &chosen, maps));
    }
    let plain = cost(&vec![false; maps.len()])?;
    let mut best = cost(&chosen)?;
    if best >= plain {
        chosen.fill(false);
        best = plain;
    }
    // One greedy pass: tables interact through color decorrelation, which
    // needs equal color depths.
    if candidates.len() > 1 {
        for &c in &candidates {
            chosen[c] = !chosen[c];
            let toggled = cost(&chosen)?;
            if toggled < best {
                best = toggled;
            } else {
                chosen[c] = !chosen[c];
            }
        }
    }
    // An estimate, not a proof of the final size: keep the regular maps for
    // ties and small estimated gains.
    if best >= plain * 0.99 {
        return Ok(none());
    }
    Ok(apply(tables, &chosen, maps))
}

/// The `chosen` tables, one per coded channel, with their maps rewritten.
fn apply(tables: Vec<Option<Vec<u32>>>, chosen: &[bool], maps: &mut [ChannelMap]) -> Vec<Vec<u32>> {
    tables
        .into_iter()
        .enumerate()
        .map(|(c, table)| match table {
            Some(table) if chosen[c] => {
                maps[c] = table_map(table.len());
                table
            }
            _ => Vec::new(),
        })
        .collect()
}

/// The map of a value table of `count` entries.
fn table_map(count: usize) -> ChannelMap {
    ChannelMap {
        mode: MapMode::Table,
        bits: mapping::table_bits(count),
        value: count as u32 - 1,
    }
}

/// The bytes of a table's entries in the head (SPEC 2.1).
fn table_bytes(table: &[u32]) -> usize {
    let mut out = Vec::new();
    crate::v1::put_value_table(&mut out, table);
    out.len()
}

/// The ascending distinct values of `vals` (`depth`-bit), or `None` past
/// `cap` of them.
fn distinct(
    vals: &[u32],
    depth: u32,
    cap: usize,
    check: &dyn Fn() -> Result<(), Error>,
) -> Result<Option<Vec<u32>>, Error> {
    if depth <= 16 {
        let mut present = reserved::<bool>(1 << depth)?;
        present.resize(1 << depth, false);
        for chunk in vals.chunks(mapping::SCAN_WORDS) {
            check()?;
            for &v in chunk {
                present[v as usize] = true;
            }
        }
        if present.iter().filter(|&&p| p).count() > cap {
            return Ok(None);
        }
        return Ok(Some(
            (0..1u32 << depth)
                .filter(|&v| present[v as usize])
                .collect(),
        ));
    }
    let mut set = ValueSet::new(cap)?;
    // A strided pass rejects most dense channels before the full one.
    for chunk in vals.chunks(mapping::SCAN_WORDS * 61) {
        check()?;
        for &v in chunk.iter().step_by(61) {
            if !set.insert(v) {
                return Ok(None);
            }
        }
    }
    for chunk in vals.chunks(mapping::SCAN_WORDS) {
        check()?;
        for &v in chunk {
            if !set.insert(v) {
                return Ok(None);
            }
        }
    }
    Ok(Some(set.sorted()))
}

fn reserved<T>(len: usize) -> Result<Vec<T>, Error> {
    let mut out = Vec::new();
    out.try_reserve_exact(len).map_err(|_| Error::TooLarge)?;
    Ok(out)
}

/// An open-addressing set of at most `cap` u32 values.
struct ValueSet {
    keys: Vec<u32>,
    used: Vec<bool>,
    shift: u32,
    len: usize,
    cap: usize,
}

impl ValueSet {
    fn new(cap: usize) -> Result<Self, Error> {
        // At most half full.
        let slots = (2 * cap).next_power_of_two().max(16);
        let mut keys = reserved(slots)?;
        keys.resize(slots, 0);
        let mut used = reserved(slots)?;
        used.resize(slots, false);
        Ok(Self {
            keys,
            used,
            shift: 32 - slots.trailing_zeros(),
            len: 0,
            cap,
        })
    }

    #[inline]
    fn slot(&self, v: u32) -> usize {
        (v.wrapping_mul(0x9E37_79B1) >> self.shift) as usize
    }

    /// Inserts `v`; false when that would exceed the cap.
    #[inline]
    fn insert(&mut self, v: u32) -> bool {
        let mask = self.keys.len() - 1;
        let mut i = self.slot(v);
        while self.used[i] {
            if self.keys[i] == v {
                return true;
            }
            i = (i + 1) & mask;
        }
        if self.len == self.cap {
            return false;
        }
        self.used[i] = true;
        self.keys[i] = v;
        self.len += 1;
        true
    }

    fn sorted(self) -> Vec<u32> {
        let mut out: Vec<u32> = self
            .keys
            .iter()
            .zip(&self.used)
            .filter(|(_, u)| **u)
            .map(|(&k, _)| k)
            .collect();
        out.sort_unstable();
        out
    }
}

/// Reduces ordered values `vals` to their indices in `table` (which holds
/// every one of them), calling `check` before each block of
/// [`mapping::SCAN_WORDS`] values.
pub(super) fn reduce_plane(
    vals: &mut [u32],
    table: &[u32],
    depth: u32,
    mut check: impl FnMut() -> Result<(), Error>,
) -> Result<(), Error> {
    if depth <= 16 {
        let mut index = reserved::<u32>(1 << depth)?;
        index.resize(1 << depth, 0);
        for (i, &e) in table.iter().enumerate() {
            index[e as usize] = i as u32;
        }
        for chunk in vals.chunks_mut(mapping::SCAN_WORDS) {
            check()?;
            for v in chunk {
                *v = index[*v as usize];
            }
        }
    } else {
        for chunk in vals.chunks_mut(mapping::SCAN_WORDS) {
            check()?;
            for v in chunk {
                *v = mapping::table_reduce(table, *v);
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Window {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

/// Up to 16 windows of up to 256x16 on a 4x4 grid.
fn windows(width: usize, height: usize) -> Vec<Window> {
    let (w, h) = (width.min(256), height.min(16));
    // Avoid span*i overflowing on large dimensions or 32-bit targets.
    let position = |span: usize, i: usize| (span / 3) * i + (span % 3) * i / 3;
    let mut result = Vec::with_capacity(16);
    for i in 0..4 {
        for j in 0..4 {
            let window = Window {
                x: position(width - w, j),
                y: position(height - h, i),
                width: w,
                height: h,
            };
            if !result.contains(&window) {
                result.push(window);
            }
        }
    }
    result
}

/// Estimated bits of the sampled windows under `layout`.
fn sample_bits(
    layout: &Layout,
    planes: &[Vec<u32>],
    width: usize,
    depth: u32,
    cfg: &AnalysisConfig,
    windows: &[Window],
) -> f64 {
    let mut bits = 0.0;
    for window in windows {
        let cropped: Vec<Vec<u32>> = layout
            .slot_channel
            .iter()
            .map(|&c| {
                (window.y..window.y + window.height)
                    .flat_map(|row| {
                        let start = row * width + window.x;
                        planes[c][start..start + window.width]
                            .iter()
                            .map(|&v| layout.reduce(c, v, depth))
                    })
                    .collect()
            })
            .collect();
        let sp: Vec<&[u32]> = cropped.iter().map(|p| p.as_slice()).collect();
        let mut residuals = vec![vec![0u32; window.width * window.height]; sp.len()];
        let mut views: Vec<&mut [u32]> = residuals.iter_mut().map(|r| r.as_mut_slice()).collect();
        analyze_stripe_fast(
            layout,
            &sp,
            window.width,
            seg_width(window.width, cfg.lanes),
            0,
            window.height,
            cfg,
            &mut views,
            None,
        );
        bits += residuals
            .iter()
            .map(|r| crate::v1::est_bits(r))
            .sum::<f64>();
    }
    bits
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    #[test]
    fn value_set_collects_sorted_distinct_values_up_to_its_cap() {
        // 300 values in 1..u32::MAX, scattered over the hash slots.
        let vals: Vec<u32> = (0..10_000u32).map(|i| (i % 300 + 1) * 14_000_029).collect();
        assert_eq!(distinct(&vals, 32, 299, &|| Ok(())).unwrap(), None);
        let t = distinct(&vals, 32, 300, &|| Ok(())).unwrap().unwrap();
        assert_eq!(t.len(), 300);
        assert!(t.windows(2).all(|p| p[0] < p[1]));
        let mut with_extremes = vals.clone();
        with_extremes.extend([0, u32::MAX]);
        assert_eq!(distinct(&with_extremes, 32, 301, &|| Ok(())).unwrap(), None);
        let t = distinct(&with_extremes, 32, 302, &|| Ok(()))
            .unwrap()
            .unwrap();
        assert_eq!((t[0], t[t.len() - 1], t.len()), (0, u32::MAX, 302));
    }

    #[test]
    fn narrow_distinct_values_and_their_indices_round_trip() {
        for depth in [8, 16] {
            let vals: Vec<u32> = (0..5000u32)
                .map(|i| ((i * 37) % (1 << depth)) & !3)
                .collect();
            let t = distinct(&vals, depth, cap(depth), &|| Ok(()))
                .unwrap()
                .unwrap();
            let mut coded = vals.clone();
            reduce_plane(&mut coded, &t, depth, || Ok(())).unwrap();
            for (&v, &s) in vals.iter().zip(&coded) {
                assert_eq!(mapping::table_expand(&t, s), v);
            }
        }
    }

    /// Value-table scans check for cancellation before each block of
    /// `SCAN_WORDS` values and stop before the next block.
    #[test]
    fn value_table_scans_stop_between_blocks() {
        let n = mapping::SCAN_WORDS * 3 + 1;
        for depth in [16, 32] {
            let vals: Vec<u32> = (0..n as u32).map(|i| (i % 7) * 3).collect();
            let stop_at = |k: usize| {
                let calls = core::cell::Cell::new(0usize);
                move || {
                    calls.set(calls.get() + 1);
                    if calls.get() == k {
                        Err(Error::Cancelled)
                    } else {
                        Ok(())
                    }
                }
            };
            let check = stop_at(2);
            assert!(matches!(
                distinct(&vals, depth, cap(depth), &check),
                Err(Error::Cancelled)
            ));
            let t = distinct(&vals, depth, cap(depth), &|| Ok(()))
                .unwrap()
                .unwrap();
            let mut coded = vals.clone();
            assert!(matches!(
                reduce_plane(&mut coded, &t, depth, stop_at(2)),
                Err(Error::Cancelled)
            ));
            // The first block was reduced, the rest untouched.
            let b = mapping::SCAN_WORDS;
            assert!(
                coded[..b]
                    .iter()
                    .zip(&vals)
                    .all(|(&s, &v)| mapping::table_expand(&t, s) == v)
            );
            assert_eq!(coded[b..], vals[b..]);
        }
    }

    #[test]
    fn grid_windows_stay_inside_every_shape() {
        for w in [1, 3, 255, 256, 257, 4097, usize::MAX / 2] {
            for h in [1, 15, 16, 17, 1000, usize::MAX / 2] {
                let ws = windows(w, h);
                assert!((1..=16).contains(&ws.len()));
                for r in ws {
                    assert!(r.x <= w - r.width && r.y <= h - r.height);
                }
            }
        }
    }
}
