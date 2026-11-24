//! SPEC 4's inter-frame reference `R = reduce(o) mod 2^n`, implemented here
//! from the SPEC text alone (the order maps of 2.1, the F16InF32 widening,
//! the reduce table), checked against the decoder's [`coded_ref`] for every
//! map a slot may have (2.1's validity rules) on every sample type of at
//! most 32 bits: exhaustively for 8- and 16-bit samples, and on edge and
//! pseudo-random words (NaN payloads, infinities, signed zeros, subnormals,
//! values each map cannot represent) for 32-bit samples.
use super::*;
use crate::mapping::{ChannelMap, MapMode};
use std::collections::HashMap;

/// SPEC 2.1's order maps.
fn order(sample: SampleType, v: u32) -> u32 {
    match sample {
        SampleType::F16 => v ^ ((v >> 15) * 0x7FFF),
        SampleType::F32 => v ^ ((v >> 31) * 0x7FFF_FFFF),
        SampleType::I8 => v ^ 0x80,
        SampleType::I16 => v ^ 0x8000,
        SampleType::I32 => v ^ 0x8000_0000,
        _ => v,
    }
}

/// `order_n` of SPEC 2.1 (FloatShift).
fn order_n(t: u32, n: u32) -> u32 {
    t ^ ((t >> (n - 1)) * ((1u32 << (n - 1)) - 1))
}

/// The F16InF32 widening of half `h` (SPEC 2.1: the half value widened
/// exactly to f32, NaN payload shifted up 13 bits).
fn widen(h: u16) -> u32 {
    let sign = u32::from(h >> 15) << 31;
    let exp = u32::from(h >> 10 & 31);
    let man = u32::from(h & 0x3FF);
    match (exp, man) {
        (0, 0) => sign,
        // man * 2^-24, normalized for f32.
        (0, _) => {
            let msb = 31 - man.leading_zeros(); // 0..=9
            let e = msb + 127 - 24;
            sign | e << 23 | (man << (23 - msb)) & 0x7F_FFFF
        }
        (31, _) => sign | 0xFF << 23 | man << 13,
        _ => sign | (exp + 127 - 15) << 23 | man << 13,
    }
}

fn mask(n: u32) -> u32 {
    if n == 32 { u32::MAX } else { (1 << n) - 1 }
}

/// SPEC 4's table.
fn spec_reduce(m: &ChannelMap, sample: SampleType, v: u32, halves: &HashMap<u32, u16>) -> u32 {
    let (d, n) = (sample.depth(), u32::from(m.bits));
    let o = order(sample, v);
    let r = match m.mode {
        MapMode::Lsb => o,
        MapMode::Shift | MapMode::Replicate => o >> (d - n),
        MapMode::Scale => {
            let (cn, cd) = ((1u64 << n) - 1, (1u64 << d) - 1);
            ((u64::from(o) * cn + cd / 2) / cd) as u32
        }
        MapMode::F16InF32 => halves
            .get(&v)
            .map_or(0, |&h| order(SampleType::F16, h.into())),
        MapMode::UnormF32 => {
            let f = f32::from_bits(v);
            let c = ((1u32 << n) - 1) as f32;
            let t = f * c + 0.5;
            if t.is_nan() || t < 0.0 {
                0
            } else if f64::from(t) >= 4_294_967_296.0 {
                u32::MAX
            } else {
                f64::from(t).trunc() as u64 as u32
            }
        }
        MapMode::Offset => o.wrapping_sub(m.value),
        MapMode::FloatShift => order_n(v >> (d - n), n),
        MapMode::Const | MapMode::Table => unreachable!("not exercised here"),
    };
    r & mask(n)
}

/// Every map SPEC 2.1 allows a coded slot of `sample` (Const excluded).
fn maps(sample: SampleType) -> Vec<ChannelMap> {
    let d = sample.depth();
    let wide = d == 32;
    let float = matches!(sample, SampleType::F16 | SampleType::F32);
    let map = |mode, bits: u32, value| ChannelMap {
        mode,
        bits: bits as u8,
        value,
    };
    let mut out = Vec::new();
    for bits in 0..=d {
        out.push(map(MapMode::Lsb, bits, 0));
        if bits >= 1 {
            out.push(map(MapMode::Shift, bits, 0));
            if !wide || bits <= 16 {
                out.push(map(MapMode::Replicate, bits, 0));
                out.push(map(MapMode::Scale, bits, 0));
            }
        }
        if float && (1..d).contains(&bits) {
            out.push(map(MapMode::FloatShift, bits, 0));
        }
        if wide {
            for value in [0, 1, 0x7FFF_FFFF, 0x8000_0000, 0xDEAD_BEEF, u32::MAX] {
                out.push(map(MapMode::Offset, bits, value));
            }
        }
        if sample == SampleType::F32 && (1..=16).contains(&bits) {
            out.push(map(MapMode::UnormF32, bits, 0));
        }
    }
    if sample == SampleType::F32 {
        out.push(map(MapMode::F16InF32, 16, 0));
    }
    out
}

/// Words to reduce: every value of an 8- or 16-bit sample; for 32-bit
/// samples edge words and pseudo-random ones.
fn words(sample: SampleType) -> Vec<u32> {
    if sample.depth() <= 16 {
        return (0..1u32 << sample.depth()).collect();
    }
    let mut v = vec![
        0,
        1,
        2,
        0x7F,
        0xFF,
        0x100,
        0xFFFF,
        0x1_0000,
        0x7FFF_FFFF,
        0x8000_0000,
        0x8000_0001,
        u32::MAX,
        u32::MAX - 1,
        // f32: +-0, +-inf, NaNs (quiet, signalling, payloads with and
        // without low 13 bits), subnormals, 1.0, the largest half, the
        // smallest half subnormal, values just off a half, the largest f32.
        0x3F80_0000,
        0x3F80_0001,
        0xBF80_0000,
        0x477F_E000,
        0x477F_E001,
        0x4780_0000,
        0x3380_0000,
        0x3300_0000,
        0x3380_2000,
        0x3880_0000,
        0x387F_C000,
        0x7F80_0000,
        0xFF80_0000,
        0x7FC0_0000,
        0xFFC0_0000,
        0x7F80_0001,
        0x7F80_2000,
        0xFFC0_2000,
        0x7FC0_0001,
        0x0000_0001,
        0x007F_FFFF,
        0x7F7F_FFFF,
        0xFF7F_FFFF,
        0x4F80_0000,
        0x4F7F_FFFF,
        0x5F80_0000,
        0xBF00_0000,
        0xBE80_0000,
        0x3EFF_FFFF,
    ];
    let mut s = 0x9E37_79B9u32;
    for _ in 0..20_000 {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        v.push(s);
    }
    v
}

#[test]
fn reduce_matches_spec_4() {
    let halves: HashMap<u32, u16> = (0..=u16::MAX).map(|h| (widen(h), h)).collect();
    // The widening is one to one, and the decoder's agrees with the text's.
    assert_eq!(halves.len(), 1 << 16);
    for h in 0..=u16::MAX {
        assert_eq!(
            crate::mapping::f16_to_f32_bits(h),
            widen(h),
            "half {h:#06x}"
        );
    }
    let mut checked = 0u64;
    for sample in [
        SampleType::U8,
        SampleType::I8,
        SampleType::U16,
        SampleType::I16,
        SampleType::F16,
        SampleType::U32,
        SampleType::I32,
        SampleType::F32,
    ] {
        let ws = words(sample);
        for m in maps(sample) {
            for &v in &ws {
                let want = spec_reduce(&m, sample, v, &halves);
                let got = coded_ref(&m, &[], sample, v);
                assert_eq!(got, want, "{sample:?} {m:?} sample {v:#x}");
                checked += 1;
            }
        }
    }
    assert!(checked > 10_000_000, "{checked}");
}
