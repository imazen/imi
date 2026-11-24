//! Per-channel sample mappings: code `n`-bit values and expand them to the
//! container depth on decode. This is what makes 10/12-bit content stored in
//! 16-bit containers (MSB-aligned, bit-replicated, scaled or LSB-aligned)
//! and 0/max alpha cost no more than their real precision.

use alloc::vec;
use alloc::vec::Vec;

/// A lossless channel map mode (SPEC 2.1). New modes come with a feature
/// (modes 7 and 8 did), so the enum is `#[non_exhaustive]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
#[non_exhaustive]
pub enum MapMode {
    /// `x = s` (also used for n == container depth).
    Lsb = 0,
    /// `x = s << (D - n)`.
    Shift = 1,
    /// `x` = `s` bit-replicated to D bits.
    Replicate = 2,
    /// `x = round(s * (2^D - 1) / (2^n - 1))`.
    Scale = 3,
    /// Every sample equals `value`; the channel is not coded (alpha only).
    Const = 4,
    /// F32: `s` is an ordered f16; the sample is that half value
    /// widened exactly to f32.
    F16InF32 = 5,
    /// F32: the sample is `s / (2^n - 1)` computed in f32.
    UnormF32 = 6,
    /// 32-bit word coding: `x = value + s` (wrapping).
    Offset = 7,
    /// Float samples (f16, f32), Scientific profile: `s` is the top `bits`
    /// bits of the raw float pattern, ordered as a `bits`-wide float (sign
    /// then magnitude); the low `D - bits` raw bits are zero. Codes floats
    /// whose low mantissa bits were zeroed (BitRound, DISCARD_LSB) at their
    /// real precision whatever their signs.
    FloatShift = 8,
    /// Any sample type, feature `value_tables`: `s` indexes a per-channel
    /// table of the ordered sample values the channel uses (ascending, at
    /// most 65,536 entries); `bits` is the table's index width and
    /// `value` its entry count minus one. Indices past the last entry
    /// expand to the last entry. [`ChannelMap::expand`] and
    /// [`ChannelMap::reduce`] cannot see the table: the codec expands and
    /// reduces through it.
    Table = 9,
}

impl MapMode {
    pub(crate) fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => Self::Lsb,
            1 => Self::Shift,
            2 => Self::Replicate,
            3 => Self::Scale,
            4 => Self::Const,
            5 => Self::F16InF32,
            6 => Self::UnormF32,
            7 => Self::Offset,
            8 => Self::FloatShift,
            9 => Self::Table,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelMap {
    pub mode: MapMode,
    /// Coded bit depth (0..=32).
    pub bits: u8,
    /// Constant for [`MapMode::Const`], base for [`MapMode::Offset`].
    pub value: u32,
}

impl ChannelMap {
    pub fn identity(depth: u32) -> Self {
        Self {
            mode: MapMode::Lsb,
            bits: depth as u8,
            value: 0,
        }
    }

    pub fn is_identity(&self, depth: u32) -> bool {
        match self.mode {
            MapMode::Lsb => true,
            MapMode::Const
            | MapMode::F16InF32
            | MapMode::UnormF32
            | MapMode::FloatShift
            | MapMode::Table => false,
            MapMode::Offset => self.value == 0,
            _ => self.bits as u32 == depth,
        }
    }

    /// Expands a coded value to the container depth.
    #[inline]
    pub fn expand(&self, s: u32, depth: u32) -> u32 {
        let n = self.bits as u32;
        match self.mode {
            MapMode::Lsb => s,
            MapMode::Shift => s << (depth - n),
            MapMode::Replicate => replicate(s, n, depth),
            MapMode::Scale => {
                let max_d = (1u64 << depth) - 1;
                let max_n = (1u64 << n) - 1;
                ((s as u64 * max_d + max_n / 2) / max_n) as u32
            }
            MapMode::Const => self.value,
            MapMode::F16InF32 => f32_order(f16_to_f32_bits(f16_order(s as u16))),
            MapMode::UnormF32 => f32_order(unorm_f32(s, n).to_bits()),
            MapMode::Offset => self.value.wrapping_add(s),
            MapMode::FloatShift => float_order(order_bits(s, n) << (depth - n), depth),
            MapMode::Table => {
                debug_assert!(false, "table maps expand through their table");
                s
            }
        }
    }

    /// Maps a container sample to its coded value (encoder side).
    #[inline]
    pub fn reduce(&self, x: u32, depth: u32) -> u32 {
        let n = self.bits as u32;
        match self.mode {
            MapMode::Lsb => x,
            MapMode::Shift | MapMode::Replicate => x >> (depth - n),
            MapMode::Scale => {
                let max_d = (1u64 << depth) - 1;
                let max_n = (1u64 << n) - 1;
                ((x as u64 * max_n + max_d / 2) / max_d) as u32
            }
            MapMode::Const => 0,
            MapMode::F16InF32 => {
                f32_to_f16_bits_exact(f32_order(x)).map_or(0, |h| f16_order(h) as u32)
            }
            MapMode::UnormF32 => {
                let f = f32::from_bits(f32_order(x));
                let max = ((1u64 << n) - 1) as f32;
                let v = f * max + 0.5;
                if v >= 0.0 { v as u32 } else { 0 }
            }
            MapMode::Offset => x.wrapping_sub(self.value),
            MapMode::FloatShift => order_bits(float_order(x, depth) >> (depth - n), n),
            MapMode::Table => {
                debug_assert!(false, "table maps reduce through their table");
                x
            }
        }
    }

    /// Expansion lookup table (2^bits entries; bits <= 16).
    pub fn lut(&self, depth: u32) -> Vec<u32> {
        (0..1u32 << self.bits)
            .map(|s| self.expand(s, depth))
            .collect()
    }
}

/// Most entries of a value table (map mode 9, SPEC 2.1): 16-bit indices.
pub(crate) const MAX_TABLE: usize = 1 << 16;

/// The index width of a value table of `count` entries (1..=[`MAX_TABLE`]):
/// the fewest bits that index every entry.
pub(crate) fn table_bits(count: usize) -> u8 {
    debug_assert!((1..=MAX_TABLE).contains(&count));
    (usize::BITS - (count - 1).leading_zeros()) as u8
}

/// The expansion of `s` through a value table: its entry, or the last entry
/// for indices past the end (SPEC 2.1).
#[inline(always)]
pub(crate) fn table_expand(table: &[u32], s: u32) -> u32 {
    table[(s as usize).min(table.len() - 1)]
}

/// The coded value of an ordered sample `o` under a value table: the index of
/// the greatest entry at most `o`, or 0 below the first entry. Exact for every
/// value in the table; the rule for other values (inter references, SPEC 4)
/// keeps reduction defined for any sample.
#[inline]
pub(crate) fn table_reduce(table: &[u32], o: u32) -> u32 {
    table.partition_point(|&e| e <= o).saturating_sub(1) as u32
}

fn replicate(s: u32, n: u32, depth: u32) -> u32 {
    if n == 0 {
        return 0;
    }
    let mut out = 0u32;
    let mut filled = 0;
    while filled < depth {
        let shift = depth as i32 - filled as i32 - n as i32;
        if shift >= 0 {
            out |= s << shift;
        } else {
            out |= s >> (-shift);
        }
        filled += n;
    }
    out
}

/// Chooses the cheapest exact mapping for a channel given which container
/// values occur (`present[x]`). `allow_const` permits [`MapMode::Const`].
pub fn detect(present: &[bool], depth: u32, allow_const: bool) -> ChannelMap {
    // Wide channels (including lossy DC) often use only a handful of the
    // 65536 possible values. Skip empty cache-line-sized chunks with a byte
    // comparison instead of visiting each absent value individually.
    // Keep ascending order: it also makes the first failing map value cheap.
    let mut values = Vec::new();
    for (chunk, flags) in present.chunks(64).enumerate() {
        if flags == [false; 64] {
            continue;
        }
        for (i, &used) in flags.iter().enumerate() {
            if used {
                values.push((chunk * 64 + i) as u32);
            }
        }
    }
    if values.is_empty() {
        return ChannelMap {
            mode: MapMode::Lsb,
            bits: 0,
            value: 0,
        };
    }
    if allow_const && values.len() == 1 {
        return ChannelMap {
            mode: MapMode::Const,
            bits: 0,
            value: values[0],
        };
    }
    for n in 0..depth {
        for mode in [
            MapMode::Lsb,
            MapMode::Shift,
            MapMode::Replicate,
            MapMode::Scale,
        ] {
            if n == 0 && mode != MapMode::Lsb {
                continue;
            }
            let m = ChannelMap {
                mode,
                bits: n as u8,
                value: 0,
            };
            let ok = values.iter().all(|&x| {
                let s = m.reduce(x, depth);
                s < (1 << n) && m.expand(s, depth) == x
            });
            if ok {
                return m;
            }
        }
    }
    ChannelMap::identity(depth)
}

/// Order map of an `n`-bit sign-magnitude pattern (an involution): the
/// magnitude bits of a pattern with its top bit set are inverted.
#[inline(always)]
pub(crate) fn order_bits(t: u32, n: u32) -> u32 {
    if n == 0 {
        return 0;
    }
    let half = (1u32 << (n - 1)) - 1;
    t ^ (((t >> (n - 1)) & 1).wrapping_neg() & half)
}

/// The order map of float samples of container depth `depth` (16: f16,
/// 32: f32).
#[inline(always)]
pub(crate) fn float_order(v: u32, depth: u32) -> u32 {
    if depth == 16 {
        f16_order(v as u16) as u32
    } else {
        f32_order(v)
    }
}

/// [`MapMode::FloatShift`] for float samples whose raw patterns OR to
/// `raw_or`, when it beats the map `current`: the coded width is the raw
/// width less the low bits that are zero in every sample. It replaces a map
/// of more bits, or an `F16InF32` map of as many (whose f16 mantissa then
/// carries dead low bits, ones for negative values).
#[cfg(feature = "encode")]
pub(crate) fn float_shift_map(raw_or: u32, depth: u32, current: &ChannelMap) -> Option<ChannelMap> {
    let n = depth.saturating_sub(raw_or.trailing_zeros());
    let bits = current.bits as u32;
    let better = n < bits || (n == bits && current.mode == MapMode::F16InF32);
    (n >= 1 && better).then_some(ChannelMap {
        mode: MapMode::FloatShift,
        bits: n as u8,
        value: 0,
    })
}

/// `s / (2^n - 1)` in f32 (IEEE division: identical on every platform).
#[inline]
pub(crate) fn unorm_f32(s: u32, n: u32) -> f32 {
    s as f32 / ((1u64 << n) - 1) as f32
}

/// Exact widening of f16 bits to f32 bits (NaN payloads shift up).
pub(crate) fn f16_to_f32_bits(h: u16) -> u32 {
    let sign = ((h >> 15) as u32) << 31;
    let exp = ((h >> 10) & 0x1F) as u32;
    let man = (h & 0x3FF) as u32;
    match exp {
        0 if man == 0 => sign,
        0 => {
            // Subnormal: normalize.
            let mut e: i32 = -14;
            let mut m = man;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            sign | (((e + 127) as u32) << 23) | ((m & 0x3FF) << 13)
        }
        31 => sign | (0xFF << 23) | (man << 13),
        _ => sign | ((exp + 127 - 15) << 23) | (man << 13),
    }
}

/// f32 bits -> f16 bits if the value (and payload) is exactly representable.
pub(crate) fn f32_to_f16_bits_exact(b: u32) -> Option<u16> {
    let sign = ((b >> 31) as u16) << 15;
    let exp = ((b >> 23) & 0xFF) as i32;
    let man = b & 0x7F_FFFF;
    let h = if exp == 0xFF {
        sign | 0x7C00 | (man >> 13) as u16
    } else if exp == 0 {
        if man != 0 {
            return None; // f32 subnormals are below f16 range
        }
        sign
    } else {
        let e = exp - 127 + 15;
        if e >= 31 {
            return None;
        }
        if e >= 1 {
            sign | ((e as u16) << 10) | (man >> 13) as u16
        } else {
            // f16 subnormal: value = m * 2^-24.
            let shift = 14 - e; // 14..=24 for representable values
            if shift > 24 {
                return None;
            }
            let full = man | 0x80_0000;
            sign | (full >> shift) as u16
        }
    };
    (f16_to_f32_bits(h) == b).then_some(h)
}

/// f32 bit pattern <-> order-preserving integer (an involution).
#[inline(always)]
pub fn f32_order(v: u32) -> u32 {
    v ^ (((v as i32) >> 31) as u32 & 0x7FFF_FFFF)
}

/// Chooses the cheapest exact mapping for a 32-bit channel from its
/// (ordered) values. `is_f32` enables the float maps.
pub fn detect32(vals: &[u32], is_f32: bool, allow_const: bool) -> ChannelMap {
    match detect32_checked(vals, is_f32, allow_const, || {
        Ok::<_, core::convert::Infallible>(())
    }) {
        Ok(map) => map,
        Err(never) => match never {},
    }
}

/// Bound cooperative checks by scanned words, independently of image width.
/// Candidate ordering, early rejection and mapping tie breaks are unchanged.
pub(crate) fn detect32_checked<E>(
    vals: &[u32],
    is_f32: bool,
    allow_const: bool,
    mut check: impl FnMut() -> Result<(), E>,
) -> Result<ChannelMap, E> {
    let Some(&first) = vals.first() else {
        return Ok(ChannelMap {
            mode: MapMode::Lsb,
            bits: 0,
            value: 0,
        });
    };
    let (mut lo, mut hi, mut or_all) = (u32::MAX, 0u32, 0u32);
    for chunk in vals.chunks(SCAN_WORDS) {
        check()?;
        for &v in chunk {
            lo = lo.min(v);
            hi = hi.max(v);
            or_all |= v;
        }
    }
    if lo == hi {
        if allow_const {
            return Ok(ChannelMap {
                mode: MapMode::Const,
                bits: 0,
                value: first,
            });
        }
        return Ok(ChannelMap {
            mode: MapMode::Offset,
            bits: 0,
            value: first,
        });
    }
    let bitlen = |x: u32| 32 - x.leading_zeros();
    let mut best = ChannelMap {
        mode: MapMode::Offset,
        bits: bitlen(hi - lo) as u8,
        value: lo,
    };
    if best.bits == 32 {
        best = ChannelMap::identity(32);
    }
    let tz = or_all.trailing_zeros();
    if 32 - tz < best.bits as u32 {
        best = ChannelMap {
            mode: MapMode::Shift,
            bits: (32 - tz) as u8,
            value: 0,
        };
    }
    // Every precision supported by the extended Replicate/Scale wire modes.
    // Most rejected candidates stop at the first nonmatching sample; do not
    // assume that sensor or auxiliary channels use only common bit depths.
    for n in 1..=16u32 {
        if n >= best.bits as u32 {
            break;
        }
        for mode in [MapMode::Replicate, MapMode::Scale] {
            let m = ChannelMap {
                mode,
                bits: n as u8,
                value: 0,
            };
            if checked_all(vals, &mut check, |x| {
                let s = m.reduce(x, 32);
                s < (1 << n) && m.expand(s, 32) == x
            })? {
                best = m;
                break;
            }
        }
    }
    if is_f32 {
        for n in 1..=16u32 {
            if n >= best.bits as u32 {
                break;
            }
            let m = ChannelMap {
                mode: MapMode::UnormF32,
                bits: n as u8,
                value: 0,
            };
            if checked_all(vals, &mut check, |x| {
                let s = m.reduce(x, 32);
                s < (1 << n) && m.expand(s, 32) == x
            })? {
                best = m;
                break;
            }
        }
        if (best.bits as u32) > 16 {
            let m = ChannelMap {
                mode: MapMode::F16InF32,
                bits: 16,
                value: 0,
            };
            if checked_all(vals, &mut check, |x| {
                f32_to_f16_bits_exact(f32_order(x)).is_some()
            })? {
                best = m;
            }
        }
    }
    Ok(best)
}

// 64 KiB of mapped u32 words per checkpoint, including partial final chunks.
pub(crate) const SCAN_WORDS: usize = 16_384;

fn checked_all<E>(
    values: &[u32],
    check: &mut impl FnMut() -> Result<(), E>,
    mut predicate: impl FnMut(u32) -> bool,
) -> Result<bool, E> {
    for chunk in values.chunks(SCAN_WORDS) {
        check()?;
        if !chunk.iter().all(|&v| predicate(v)) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Presence bitmap helper.
pub fn presence(depth: u32) -> Vec<bool> {
    vec![false; 1 << depth]
}

/// f16 bit pattern <-> order-preserving integer (an involution).
#[inline(always)]
pub fn f16_order(v: u16) -> u16 {
    v ^ (((v as i16) >> 15) as u16 & 0x7FFF)
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    #[test]
    fn checked_scan_stops_before_visiting_the_next_block() {
        let values = vec![17; SCAN_WORDS * 3 + 1];
        let mut checks = 0;
        let mut visited = 0;
        let result = checked_all(
            &values,
            &mut || {
                checks += 1;
                if checks == 2 {
                    Err(crate::Error::Cancelled)
                } else {
                    Ok(())
                }
            },
            |_| {
                visited += 1;
                true
            },
        );
        assert_eq!(result, Err(crate::Error::Cancelled));
        assert_eq!(visited, 16_384);
        assert_eq!(checks, 2);
    }

    #[test]
    fn checked_mapping_cancels_statistics_and_matching_candidates() {
        let values: Vec<u32> = (0..SCAN_WORDS * 2 + 1)
            .map(|i| if i % 2 == 0 { 0 } else { u32::MAX })
            .collect();
        let stats_checks = values.len().div_ceil(SCAN_WORDS);
        // The initial range scan and the first exact Replicate candidate
        // both traverse this plane. Cancellation must work inside either.
        for stop_at in [2, stats_checks + 2] {
            let mut checks = 0;
            let result = detect32_checked(&values, false, false, || {
                checks += 1;
                if checks == stop_at {
                    Err(crate::Error::Cancelled)
                } else {
                    Ok(())
                }
            });
            assert_eq!(result, Err(crate::Error::Cancelled));
            assert_eq!(checks, stop_at);
        }
        let expected = ChannelMap {
            mode: MapMode::Replicate,
            bits: 1,
            value: 0,
        };
        assert_eq!(detect32(&values, false, false), expected);
        let checked =
            detect32_checked(&values, false, false, || Ok::<_, crate::Error>(())).unwrap();
        assert_eq!(checked, expected);
        for value in values {
            assert_eq!(checked.expand(checked.reduce(value, 32), 32), value);
        }
    }

    fn detect_vals(vals: &[u32], depth: u32) -> ChannelMap {
        let mut p = presence(depth);
        for &v in vals {
            p[v as usize] = true;
        }
        detect(&p, depth, true)
    }

    #[test]
    fn detects_common_layouts() {
        let tenbit: Vec<u32> = (0..1024).collect();
        let m = detect_vals(&tenbit.iter().map(|v| v << 6).collect::<Vec<_>>(), 16);
        assert_eq!((m.mode, m.bits), (MapMode::Shift, 10));
        let m = detect_vals(
            &tenbit
                .iter()
                .map(|&v| replicate(v, 10, 16))
                .collect::<Vec<_>>(),
            16,
        );
        assert_eq!(m.bits, 10);
        let m = detect_vals(&tenbit, 16);
        assert_eq!((m.mode, m.bits), (MapMode::Lsb, 10));
        let m = detect_vals(&(0..256).map(|v| v * 257).collect::<Vec<_>>(), 16);
        assert_eq!(m.bits, 8);
        let m = detect_vals(&[0, 65535], 16);
        assert_eq!(m.bits, 1);
        let m = detect_vals(&[0, 255], 8);
        assert_eq!(m.bits, 1);
        let m = detect_vals(&[65535], 16);
        assert_eq!(m.mode, MapMode::Const);
        let twelve: Vec<u32> = (0..4096)
            .map(|s| ((s as u64 * 65535 + 2047) / 4095) as u32)
            .collect();
        let m = detect_vals(&twelve, 16);
        assert_eq!(m.bits, 12);
        for (i, &x) in twelve.iter().enumerate() {
            assert_eq!(m.expand(m.reduce(x, 16), 16), x, "i={i}");
        }
    }

    #[test]
    fn detects_presence_at_chunk_boundaries() {
        // Include short tails as well as the full 16-bit domain. No used
        // value may be skipped at either side of a scan-chunk boundary.
        for len in [1, 63, 64, 65, 127, 128, 129, 256, 65536] {
            for x in [0, len / 2, len - 1] {
                let mut p = vec![false; len];
                p[x] = true;
                let m = detect(&p, 16, true);
                assert_eq!((m.mode, m.value), (MapMode::Const, x as u32));
                let m = detect(&p, 16, false);
                assert_eq!(m.expand(m.reduce(x as u32, 16), 16), x as u32);
                p[x] = false;
                let m = detect(&p, 16, true);
                assert_eq!((m.mode, m.bits), (MapMode::Lsb, 0));
            }
        }
    }

    #[test]
    fn f16_f32_widening_exact() {
        for h in 0..=u16::MAX {
            let b = f16_to_f32_bits(h);
            assert_eq!(f32_to_f16_bits_exact(b), Some(h), "{h:#x}");
            // Matches the hardware conversion for non-NaN values.
            let f = f32::from_bits(b);
            if !f.is_nan() {
                let e = (h >> 10) & 0x1F;
                let m = (h & 0x3FF) as f32;
                let s = if h >> 15 == 1 { -1.0f32 } else { 1.0 };
                let v = if e == 0 {
                    s * m * 2f32.powi(-24)
                } else if e == 31 {
                    s * f32::INFINITY
                } else {
                    s * (1.0 + m / 1024.0) * 2f32.powi(e as i32 - 15)
                };
                assert_eq!(f, v, "{h:#x}");
            }
        }
        assert_eq!(f32_to_f16_bits_exact(0.1f32.to_bits()), None);
        assert_eq!(f32_to_f16_bits_exact(1e-30f32.to_bits()), None);
    }

    #[test]
    fn detect32_maps() {
        let ord = |v: &[f32]| v.iter().map(|x| f32_order(x.to_bits())).collect::<Vec<_>>();
        let unorm8: Vec<f32> = (0..256).map(|s| s as f32 / 255.0).collect();
        let m = detect32(&ord(&unorm8), true, false);
        assert_eq!((m.mode, m.bits), (MapMode::UnormF32, 8));
        let halfs: Vec<f32> = (0..3000)
            .map(|i| f32::from_bits(f16_to_f32_bits(i as u16 * 7)))
            .collect();
        let m = detect32(&ord(&halfs), true, false);
        assert_eq!((m.mode, m.bits), (MapMode::F16InF32, 16));
        let small: Vec<u32> = (0..100u32)
            .map(|v| (v as i32 - 50) as u32 ^ 0x8000_0000)
            .collect();
        let m = detect32(&small, false, false);
        assert_eq!((m.mode, m.bits), (MapMode::Offset, 7));
        // Low 20 bits always zero: Shift codes the top 32 - 20 = 12 bits.
        let rep8: Vec<u32> = (0..256u32).map(|v| v * 0x0101_0101).collect();
        let m = detect32(&rep8, false, false);
        assert_eq!(m.bits, 8);
        let shifted: Vec<u32> = (0..1000u32).map(|v| v << 20).collect();
        let m = detect32(&shifted, false, false);
        assert_eq!((m.mode, m.bits), (MapMode::Shift, 12));
        for (vals, f) in [
            (ord(&unorm8), true),
            (ord(&halfs), true),
            (small.clone(), false),
            (shifted.clone(), false),
        ] {
            let m = detect32(&vals, f, false);
            for &x in &vals {
                assert_eq!(m.expand(m.reduce(x, 32), 32), x);
            }
        }
    }

    #[test]
    fn every_integer_mapping_precision_is_exact() {
        for depth in [8u32, 16, 32] {
            for bits in 1..=depth.min(16) {
                for mode in [
                    MapMode::Lsb,
                    MapMode::Shift,
                    MapMode::Replicate,
                    MapMode::Scale,
                ] {
                    let map = ChannelMap {
                        mode,
                        bits: bits as u8,
                        value: 0,
                    };
                    // Exhaust the entire meaningful domain, so a detector
                    // cannot accidentally infer fewer bits from a sparse set.
                    let values: Vec<u32> = (0..1u32 << bits)
                        .map(|code| map.expand(code, depth))
                        .collect();
                    let found = if depth == 32 {
                        detect32(&values, false, false)
                    } else {
                        detect_vals(&values, depth)
                    };
                    assert_eq!(found.bits, bits as u8, "{depth}/{bits}/{mode:?}");
                    for &value in &values {
                        assert_eq!(
                            found.expand(found.reduce(value, depth), depth),
                            value,
                            "{depth}/{bits}/{mode:?}/{value}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn every_unorm_float_mapping_precision_is_exact() {
        for bits in 1..=16 {
            let values: Vec<u32> = (0..1u32 << bits)
                .map(|v| f32_order((v as f32 / ((1u32 << bits) - 1) as f32).to_bits()))
                .collect();
            let map = detect32(&values, true, false);
            assert!(map.bits <= bits as u8, "unorm precision {bits}");
            for value in values {
                assert_eq!(map.expand(map.reduce(value, 32), 32), value);
            }
        }
    }

    #[test]
    fn float_shift_is_exact_for_every_pattern_with_zero_low_bits() {
        // f16: every pattern, every coded width.
        for n in 1..16u32 {
            let m = ChannelMap {
                mode: MapMode::FloatShift,
                bits: n as u8,
                value: 0,
            };
            for raw in 0..=u16::MAX as u32 {
                let x = f16_order(raw as u16) as u32;
                let s = m.reduce(x, 16);
                assert!(s < 1 << n);
                let exact = raw & crate::mask_of(16 - n) == 0;
                assert_eq!(m.expand(s, 16) == x, exact, "n={n} raw={raw:#x}");
            }
        }
        // f32: specials, boundaries and pseudo-random patterns per width.
        let mut st = 0x2545_f491u32;
        let mut pats = vec![
            0,
            0x8000_0000,
            0x7F80_0000,
            0xFF80_0000,
            0x7FC0_0000,
            0xFFFF_FFFF,
            0x7FFF_FFFF,
            0x0000_0001,
            0x8000_0001,
            0x007F_FFFF,
        ];
        for _ in 0..4000 {
            st ^= st << 13;
            st ^= st >> 17;
            st ^= st << 5;
            pats.push(st);
        }
        for n in 1..32u32 {
            let m = ChannelMap {
                mode: MapMode::FloatShift,
                bits: n as u8,
                value: 0,
            };
            for &p in &pats {
                let raw = p & !crate::mask_of(32 - n);
                let x = f32_order(raw);
                let s = m.reduce(x, 32);
                assert!((s as u64) < (1u64 << n));
                assert_eq!(m.expand(s, 32), x, "n={n} raw={raw:#x}");
            }
        }
    }

    #[test]
    fn float_shift_order_is_monotonic() {
        // Coded values follow the float order (mod 2^n, -0 next to +0), so
        // smooth fields crossing zero predict well.
        let n = 10u32;
        let mut prev: Option<u32> = None;
        let neg = (0x8000u16..=0xFC00).rev().filter(|v| v & 0x3F == 0);
        let pos = (0u16..=0x7C00).filter(|v| v & 0x3F == 0);
        let m = ChannelMap {
            mode: MapMode::FloatShift,
            bits: n as u8,
            value: 0,
        };
        for v in neg.chain(pos) {
            let s = m.reduce(f16_order(v) as u32, 16);
            let o = s.wrapping_add(1 << (n - 1)) & crate::mask_of(n);
            if let Some(p) = prev {
                assert!(o > p, "{v:#x}");
            }
            prev = Some(o);
        }
    }

    #[test]
    fn f16_order_is_monotonic() {
        let mut prev = None;
        // Negative values from -inf toward -0, then +0 toward +inf.
        let neg = (0x8000u16..=0xFC00).rev();
        let pos = 0u16..=0x7C00;
        for v in neg.chain(pos) {
            let o = f16_order(v).wrapping_add(0x8000);
            if let Some(p) = prev {
                assert!(o > p, "{v:#x}");
            }
            prev = Some(o);
            assert_eq!(f16_order(f16_order(v)), v);
        }
    }
}
