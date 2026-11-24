//! Channel maps (SPEC section 2.1) and the order maps (section 4 step 4).

use crate::error::{Error, Result};
use crate::header::MapKind;

/// One coded channel's map: mode, significant bits, the Const value, and
/// for mode 9 its value table.
#[derive(Debug, Clone)]
pub(crate) struct Map {
    pub(crate) mode: u8,
    pub(crate) bits: u8,
    pub(crate) value: u32,
    /// Word depth of the coded channel: 8/16/32.
    pub(crate) depth: u32,
    pub(crate) kind: MapKind,
    /// Mode 9 (SPEC 2.1): the ascending ordered values; empty otherwise.
    pub(crate) table: Vec<u32>,
}

impl Map {
    pub(crate) fn new(
        mode: u8,
        bits: u8,
        value: u32,
        depth: u32,
        kind: MapKind,
        features: u32,
    ) -> Result<Map> {
        if mode > 9 {
            // The lossless map mode is a closed field (SPEC 0.3); modes 7, 8
            // and 9 are known but gated by their features below.
            return Err(Error::corrupt(format!("unknown channel map mode {mode}")));
        }
        if u32::from(bits) > depth {
            return Err(Error::corrupt("map bits exceed container depth"));
        }
        let map = Map {
            mode,
            bits,
            value,
            depth,
            kind,
            table: Vec::new(),
        };
        match mode {
            4 => {
                if depth < 32 && value >> depth != 0 {
                    return Err(Error::corrupt("const value out of range"));
                }
            }
            5 => {
                if depth != 32 || bits != 16 {
                    return Err(Error::corrupt("bad F16InF32 map"));
                }
            }
            6 => {
                if depth != 32 || !(1..=16).contains(&bits) {
                    return Err(Error::corrupt("bad UnormF32 map"));
                }
            }
            7 => {
                if depth < 32 {
                    return Err(Error::corrupt("offset map on a narrow sample"));
                }
                if features >> crate::registry::FEATURE_OFFSET_MAP & 1 == 0 {
                    return Err(Error::corrupt("offset map without its feature"));
                }
            }
            8 => {
                if (depth != 16 && depth != 32) || bits < 1 || u32::from(bits) >= depth {
                    return Err(Error::corrupt("bad FloatShift map"));
                }
                if features >> crate::registry::FEATURE_FLOAT_SHIFT & 1 == 0 {
                    return Err(Error::corrupt("FloatShift map without its feature"));
                }
            }
            9 => {
                // `value + 1` entries, at most 65536 and at most 2^depth,
                // indexed by exactly ceil(log2(entries)) bits.
                let entries = u64::from(value) + 1;
                let mut need = 0u32;
                while (1u64 << need) < entries {
                    need += 1;
                }
                if entries > 65536 || entries > 1u64 << depth || u32::from(bits) != need {
                    return Err(Error::corrupt("bad value table map"));
                }
                if features >> crate::registry::FEATURE_VALUE_TABLES & 1 == 0 {
                    return Err(Error::corrupt("value table map without its feature"));
                }
            }
            _ => {}
        }
        Ok(map)
    }

    pub(crate) fn is_const(&self) -> bool {
        self.mode == 4
    }

    /// Mode 9: reads the table after the map's value field (SPEC 2.1), each
    /// entry a varint distance past its predecessor plus one.
    pub(crate) fn read_table(&mut self, mut varint: impl FnMut() -> Result<u64>) -> Result<()> {
        let entries = self.value as usize + 1;
        let limit = 1u64 << self.depth;
        let mut table = Vec::with_capacity(entries);
        let mut floor = 0u64;
        for _ in 0..entries {
            let entry = floor
                .checked_add(varint()?)
                .filter(|e| *e < limit)
                .ok_or_else(|| Error::corrupt("value table entry out of range"))?;
            table.push(entry as u32);
            floor = entry + 1;
        }
        self.table = table;
        Ok(())
    }

    /// Expand a coded value to the final sample word (maps 5 and 6 produce the
    /// float sample directly; every other mode produces the order-domain
    /// integer, which the order map inverse then turns into the sample).
    pub(crate) fn expand_sample(&self, coded: u32) -> u32 {
        let n = u32::from(self.bits);
        let d = self.depth;
        let order_domain = match self.mode {
            5 => return widen_half_to_f32(coded),
            6 => return f32_div_to_bits(coded, n),
            0 => coded,
            1 => coded.wrapping_shl(d - n) & mask(d),
            2 => {
                if n == 0 {
                    0
                } else {
                    // Bit replication: bit i = s[(i - phase) mod n], phase = (d - n) mod n.
                    let phase = (d - n) % n;
                    let mut out = 0u32;
                    for i in 0..d {
                        let src = (i + n - phase % n) % n;
                        out |= ((coded >> src) & 1) << i;
                    }
                    out
                }
            }
            3 => {
                // x = round((s * (2^d - 1)) / (2^n - 1)) with half-up rounding,
                // computed exactly in 64-bit integers.
                if n == 0 {
                    0
                } else {
                    let num = 2 * u64::from(coded) * mask64(d) + mask64(n);
                    let den = 2 * mask64(n);
                    (num / den) as u32
                }
            }
            4 => self.value,
            7 => self.value.wrapping_add(coded),
            // Indices past the last entry give the last entry.
            9 => self.table[(coded as usize).min(self.table.len() - 1)],
            8 => {
                let on = order_map(coded, n);
                let shifted = on.wrapping_shl(d - n) & mask(d);
                order_map(shifted, d)
            }
            _ => coded,
        };
        order_inverse(self.kind, order_domain, d)
    }

    /// Reduce a reference sample word to a coded value, for inter predictors
    /// (SPEC section 4): `reduce(order(sample))`.
    pub(crate) fn reduce(&self, sample_word: u32) -> Result<u32> {
        let n = u32::from(self.bits);
        let d = self.depth;
        let o = order_forward(self.kind, sample_word, d);
        Ok(match self.mode {
            0 => o & mask(n),
            1 | 2 => (o >> (d - n)) & mask(n),
            3 => {
                // Inverse of the round-half-up scale, clamped into the coded range.
                let num = u64::from(o) * mask64(n) + mask64(d) / 2;
                let den = mask64(d);
                ((num / den) as u32) & mask(n)
            }
            7 => o.wrapping_sub(self.value),
            8 => o >> (d - n),
            // The greatest entry at most `o`, or entry 0.
            9 => self
                .table
                .iter()
                .take_while(|&&e| e <= o)
                .count()
                .saturating_sub(1) as u32,
            other => {
                return Err(Error::corrupt(format!(
                    "map mode {other} cannot be an inter reference"
                )));
            }
        })
    }
}

pub(crate) fn mask(bits: u32) -> u32 {
    if bits >= 32 {
        u32::MAX
    } else {
        (1u32 << bits) - 1
    }
}

fn mask64(bits: u32) -> u64 {
    if bits >= 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    }
}

/// The float order map on an `n`-bit pattern (an involution).
pub(crate) fn order_map(value: u32, n: u32) -> u32 {
    value ^ ((value >> (n - 1)) * mask(n - 1))
}

/// Forward order map per kind, on a `d`-bit word.
pub(crate) fn order_forward(kind: MapKind, value: u32, d: u32) -> u32 {
    match kind {
        MapKind::Unsigned => value,
        MapKind::Signed => value ^ (1u32 << (d - 1)),
        MapKind::Float => order_map(value, d),
    }
}

/// Inverse order map per kind.
pub(crate) fn order_inverse(kind: MapKind, value: u32, d: u32) -> u32 {
    // Every order map is an involution.
    order_forward(kind, value, d)
}

/// f16 bits (ordered) -> f32 bits, NaN payload shifted up 13 bits.
fn widen_half_to_f32(ordered_half: u32) -> u32 {
    let raw = order_map(ordered_half, 16);
    let sign = (raw >> 15) & 1;
    let exponent = (raw >> 10) & 0x1F;
    let mantissa = raw & 0x3FF;
    if exponent == 0 {
        if mantissa == 0 {
            return sign << 31;
        }
        // Subnormal half: normalize into f32.
        let mut m = mantissa;
        let mut e = -1i32;
        while m & 0x400 == 0 {
            m <<= 1;
            e += 1;
        }
        return (sign << 31) | (((127 - 15 - e) as u32) << 23) | ((m & 0x3FF) << 13);
    }
    if exponent == 31 {
        return (sign << 31) | 0x7F80_0000 | (mantissa << 13);
    }
    (sign << 31) | ((exponent - 15 + 127) << 23) | (mantissa << 13)
}

/// UnormF32: IEEE single division of the coded value by the full range.
fn f32_div_to_bits(coded: u32, bits: u32) -> u32 {
    let value = coded as f32 / mask(bits) as f32;
    value.to_bits()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_maps_are_involutions() {
        for v in [0u32, 1, 0x7FFF, 0x8000, 0xFFFF] {
            assert_eq!(
                order_inverse(MapKind::Signed, order_forward(MapKind::Signed, v, 16), 16),
                v
            );
            assert_eq!(
                order_inverse(MapKind::Float, order_forward(MapKind::Float, v, 16), 16),
                v
            );
        }
    }

    #[test]
    fn replicate_eight_in_sixteen() {
        let map = Map::new(2, 8, 0, 16, MapKind::Unsigned, 0).unwrap();
        assert_eq!(map.expand_sample(0xAB), 0xABAB);
    }

    #[test]
    fn shift_and_scale_examples() {
        let shift = Map::new(1, 8, 0, 16, MapKind::Unsigned, 0).unwrap();
        assert_eq!(shift.expand_sample(0xAB), 0xAB00);
        let scale = Map::new(3, 8, 0, 16, MapKind::Unsigned, 0).unwrap();
        assert_eq!(scale.expand_sample(0xFF), 0xFFFF);
        assert_eq!(scale.expand_sample(0x00), 0);
    }

    #[test]
    fn f16_const_is_order_inverted() {
        // A Const f16 value of 0x8000 is ordered; its raw bit pattern is
        // 0xFFFF (this is the geometry-0-c1-s2-k1 wire case).
        let map = Map::new(4, 0, 0x8000, 16, MapKind::Float, 0).unwrap();
        let raw = order_inverse(MapKind::Float, map.value, 16);
        assert_eq!(raw, 0xFFFF);
        // Ordered 0xFFFE is the negative subnormal whose raw bits are 0x8001;
        // ordered 0x0001 stays the smallest positive subnormal.
        let map = Map::new(4, 0, 0xFFFE, 16, MapKind::Float, 0).unwrap();
        assert_eq!(order_inverse(MapKind::Float, map.value, 16), 0x8001);
        assert_eq!(order_inverse(MapKind::Float, 0x0001, 16), 0x0001);
    }

    #[test]
    fn unknown_map_mode_is_corrupt() {
        // The lossless map mode is a closed field (SPEC 0.3).
        let err = Map::new(9, 8, 0, 8, MapKind::Unsigned, 0).unwrap_err();
        assert!(matches!(err, Error::Corrupt(_)));
    }

    #[test]
    fn sample_type_order_kinds() {
        use crate::header::{SampleType, coded_channel_kind};
        assert_eq!(coded_channel_kind(SampleType::F16, 0, 1), MapKind::Float);
        assert_eq!(coded_channel_kind(SampleType::I32, 0, 1), MapKind::Signed);
        assert_eq!(coded_channel_kind(SampleType::F64, 0, 1), MapKind::Float);
        assert_eq!(coded_channel_kind(SampleType::F64, 1, 1), MapKind::Unsigned);
    }
}
