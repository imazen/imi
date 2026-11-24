//! The lossy head's parameters (SPEC 2.8): a presence mask whose clear bits
//! take the frozen defaults, the quantizer scale, then the fields that
//! differ from their defaults in presence-bit order. Bit 4 says a head
//! record list follows the entropy tables (the head parser reads it).
#[cfg(all(feature = "encode", not(feature = "std")))]
use num_traits::Float as _;

use super::*;

/// Presence bits (SPEC 2.8).
const P_SEED: u64 = 1;
const P_ENTROPY: u64 = 1 << 1;
const P_MATRIX: u64 = 1 << 2;
const P_DC: u64 = 1 << 3;
/// A head record list follows the entropy tables.
const P_RECORDS: u64 = 1 << 4;
/// Ordinary rANS restart states (compact states are the default).
const P_FULL_STATES: u64 = 1 << 5;
const P_FLOOR: u64 = 1 << 6;
const P_SCALE: u64 = 1 << 7;
const P_MUL: u64 = 1 << 8;
/// Full-width bank identifier and independent scope varint.
const P_SEED_ID: u64 = 1 << 9;
const P_ALL: u64 = (1 << 10) - 1;

/// Base matrix ids of the Lossy 0.4 dequantization (feature
/// `dequant_presets`: 3, 4 and 5; `explicit_tables`: 63). Lab builds add
/// the tablemath study's probes 6-9, presets that did not qualify and are
/// never written unless an encoder asks for them; lab readers keep the
/// study's files reproducible.
#[cfg(feature = "lab")]
pub(crate) const MATRIX_PRESETS: core::ops::RangeInclusive<u8> = 3..=9;
/// Base matrix ids of the Lossy 0.4 dequantization (feature
/// `dequant_presets`: 3, 4 and 5; `explicit_tables`: 63).
#[cfg(not(feature = "lab"))]
pub(crate) const MATRIX_PRESETS: core::ops::RangeInclusive<u8> = 3..=5;
pub(crate) const MATRIX_EXPLICIT: u8 = 63;
/// Highest preset id ([`MATRIX_PRESETS`]' end).
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) const MATRIX_LAST_PRESET: u8 = 9;
/// Highest preset id ([`MATRIX_PRESETS`]' end).
#[cfg(not(feature = "lab"))]
#[cfg(feature = "encode")]
pub(crate) const MATRIX_LAST_PRESET: u8 = 5;

#[cfg(feature = "encode")]
fn floats(out: &mut Vec<u8>, values: &[f32]) {
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
}
#[cfg(feature = "encode")]
fn same(a: &[f32], b: &[f32]) -> bool {
    a.iter().zip(b).all(|(a, b)| a.to_bits() == b.to_bits())
}
pub(super) fn defaults(color: LossyColor, kind: InputKind) -> [f32; 3] {
    if kind == InputKind::U16 && color == LossyColor::YCoCg {
        [1.0 / 257.0; 3]
    } else {
        color.default_scales()
    }
}
pub(super) fn default_matrix(color: LossyColor) -> u8 {
    if color == LossyColor::Xyb { 2 } else { 0 }
}
fn read_f32(r: &mut impl crate::parse::payload::HeadInput) -> Result<f32, Error> {
    Ok(f32::from_le_bytes(r.bytes(4)?.try_into().unwrap()))
}

async fn read_f32_async(r: &mut impl crate::parse::payload::AsyncHeadInput) -> Result<f32, Error> {
    Ok(f32::from_le_bytes(r.bytes(4).await?.try_into().unwrap()))
}

/// The head's parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Params {
    pub scale: [f32; 3],
    pub qp: QParams,
    pub entropy: LossyEntropy,
    /// Compact rANS restart states.
    pub compact: bool,
    pub seed: crate::seeds::Selection,
    /// A head record list follows the entropy tables.
    pub records: bool,
}

/// Writes the presence mask, the quantizer scale and the fields that differ
/// from their defaults.
#[cfg(feature = "encode")]
pub(super) fn write(out: &mut Vec<u8>, color: LossyColor, kind: InputKind, p: &Params) {
    let qp = &p.qp;
    let base = QParams::from_quality(90.0, color, qp.matrix);
    let present = [
        (p.seed.is_some() && p.seed.legacy, P_SEED),
        (p.seed.is_some() && !p.seed.legacy, P_SEED_ID),
        (p.entropy != LossyEntropy::RunLevel, P_ENTROPY),
        (qp.matrix != default_matrix(color), P_MATRIX),
        (qp.dc != default_dc_step(color), P_DC),
        (p.records, P_RECORDS),
        (!p.compact, P_FULL_STATES),
        (!same(&p.scale, &defaults(color, kind)), P_SCALE),
        (qp.floor.to_bits() != 1f32.to_bits(), P_FLOOR),
        (!same(&qp.mul, &base.mul), P_MUL),
    ]
    .into_iter()
    .filter(|f| f.0)
    .fold(0, |m, f| m | f.1);
    put_varint(out, present);
    floats(out, &[qp.scale]);
    if present & P_SEED != 0 {
        out.push(p.seed.scope | u8::try_from(p.seed.bank).expect("legacy bank id"));
    }
    if present & P_ENTROPY != 0 {
        out.push(p.entropy as u8);
    }
    if present & P_MATRIX != 0 {
        out.push(qp.matrix);
    }
    if present & P_DC != 0 {
        out.extend_from_slice(&qp.dc);
    }
    if present & P_FLOOR != 0 {
        floats(out, &[qp.floor]);
    }
    if present & P_SCALE != 0 {
        floats(out, &p.scale);
    }
    if present & P_MUL != 0 {
        floats(out, &qp.mul);
    }
    if present & P_SEED_ID != 0 {
        put_varint(out, p.seed.bank);
        put_varint(out, u64::from(p.seed.scope >> 4));
    }
}

/// Reads the parameters of a file whose header declares `declared`: the
/// seed byte needs `seed_bank_1`; matrix ids 3, 4 and 5 `dequant_presets`,
/// 63 `explicit_tables` (gated syntax without its feature is `Corrupt`).
/// A decoding parse (`decodes`) decodes the Lossy 0.4 values (the four
/// features are implemented); a structure parse (writers finding where the
/// head ends) accepts the declared syntax and keeps a placeholder (the
/// values are not used).
pub(super) fn read(
    r: &mut impl crate::parse::payload::HeadInput,
    color: LossyColor,
    kind: InputKind,
    declared: crate::parse::header::Features,
    decodes: bool,
) -> Result<Params, Error> {
    crate::parse::payload::ready(read_async(
        &mut crate::parse::payload::ReadyInput(r),
        color,
        kind,
        declared,
        decodes,
    ))
}

pub(super) async fn read_async(
    r: &mut impl crate::parse::payload::AsyncHeadInput,
    color: LossyColor,
    kind: InputKind,
    declared: crate::parse::header::Features,
    decodes: bool,
) -> Result<Params, Error> {
    use crate::parse::header::Feature;
    let present = r.varint().await?;
    // A field this decoder does not know has no known length.
    if present & !P_ALL != 0 {
        return Err(Error::Unsupported("unknown lossy head field"));
    }
    let qscale = read_f32_async(r).await?;
    let seed = if present & P_SEED != 0 {
        if present & P_SEED_ID != 0 {
            return Err(Error::Corrupt("conflicting seed bank fields"));
        }
        if !declared.allows(Feature::SeedBank1) {
            return Err(Error::Corrupt(
                "seed tables without the feature seed_bank_1",
            ));
        }
        let s = r.u8().await?;
        // Bank ids are 1..=15; at least one blob carries selectors.
        if s & 0x80 != 0 || s & 0x70 == 0 || s & 15 == 0 {
            return Err(Error::Corrupt("bad seed byte"));
        }
        // Bank 1 is the feature declared above. Another bank comes with
        // its own feature id, which a decoder that lacks it refuses at the
        // header, so here it is gated syntax without its feature (lab
        // builds also read their experimental banks).
        if crate::seeds::bank(u64::from(s & 15)).is_none() {
            return Err(Error::Corrupt("seed bank without its feature"));
        }
        crate::seeds::Selection::legacy(s)
    } else {
        crate::seeds::Selection::NONE
    };
    let entropy = if present & P_ENTROPY != 0 {
        match r.u8().await? {
            // Packed (1) and Split (2) are retired (SPEC 2.8): reserved.
            1 | 2 => return Err(Error::Unsupported("retired entropy mode")),
            // An entropy mode a later version may define.
            v => LossyEntropy::from_u8(v).ok_or(Error::Unsupported("unknown entropy mode"))?,
        }
    } else {
        LossyEntropy::RunLevel
    };
    let matrix = if present & P_MATRIX != 0 {
        let m = r.u8().await?;
        let gate = match m {
            0..=2 => None,
            m if MATRIX_PRESETS.contains(&m) => Some(Feature::DequantPresets),
            MATRIX_EXPLICIT => Some(Feature::ExplicitTables),
            _ => return Err(Error::Unsupported("unknown quantization matrix")),
        };
        match gate {
            Some(f) if !declared.allows(f) => {
                return Err(Error::Corrupt("quantization matrix without its feature"));
            }
            // A structure parse (writers finding where the head ends):
            // the matrix is not decoded.
            Some(_) if !decodes => default_matrix(color),
            Some(_) => m,
            None => m,
        }
    } else {
        default_matrix(color)
    };
    let dc = if present & P_DC != 0 {
        let d = [r.u8().await?, r.u8().await?, r.u8().await?];
        if d.contains(&0) {
            return Err(Error::Corrupt("zero DC step multiplier"));
        }
        d
    } else {
        default_dc_step(color)
    };
    let floor = if present & P_FLOOR != 0 {
        read_f32_async(r).await?
    } else {
        1.0
    };
    let mut scale = defaults(color, kind);
    if present & P_SCALE != 0 {
        for s in &mut scale {
            *s = read_f32_async(r).await?;
        }
    }
    let mul = if present & P_MUL != 0 {
        [
            read_f32_async(r).await?,
            read_f32_async(r).await?,
            read_f32_async(r).await?,
        ]
    } else {
        QParams::from_quality(90.0, color, matrix).mul
    };
    let seed = if present & P_SEED_ID != 0 {
        if !declared.allows(Feature::SeedBankIds) {
            return Err(Error::Corrupt("seed bank id without its feature"));
        }
        let id = r.varint().await?;
        let scope = r.varint().await?;
        if scope & !7 != 0 {
            return Err(Error::Unsupported("unknown seed table scope"));
        }
        if id == 0 || scope == 0 {
            return Err(Error::Corrupt("bad seed bank selection"));
        }
        if crate::seeds::bank(id).is_none() {
            return Err(Error::Unsupported("unknown seed bank"));
        }
        crate::seeds::Selection::new(id, (scope as u8) << 4)
    } else {
        seed
    };
    let compact = present & P_FULL_STATES == 0;
    if compact && entropy != LossyEntropy::RunLevel {
        return Err(Error::Corrupt("compact states with incompatible entropy"));
    }
    Ok(Params {
        scale,
        qp: QParams {
            scale: qscale,
            floor,
            mul,
            matrix,
            dc,
            explicit: None,
            bias: None,
            transform: None,
        },
        entropy,
        compact,
        seed,
        records: present & P_RECORDS != 0,
    })
}

/// Explicit base tables (Lossy 0.4, matrix [`MATRIX_EXPLICIT`]): a header
/// byte (bits 0-1: 0 three tables, 1 channel 2 shares channel 1's, 2 one
/// table; bits 2-4: precision p, each value in units of 2^-p; bits 5-7 zero),
/// then each table's 64 values in zigzag order, each in 1..=65535 units: the
/// first as a varint, the others as zigzag-signed varint deltas from the
/// previous value. The encoder writes each table at the finest precision
/// whose values all fit.
#[cfg(feature = "encode")]
pub(super) fn write_explicit(out: &mut Vec<u8>, t: &[[f32; 64]; 3]) {
    let same =
        |a: &[f32; 64], b: &[f32; 64]| a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits());
    let share = if same(&t[0], &t[1]) && same(&t[1], &t[2]) {
        2
    } else if same(&t[1], &t[2]) {
        1
    } else {
        0
    };
    let n = [3, 2, 1][share];
    let max = t[..n].iter().flatten().fold(0.0f32, |m, &v| m.max(v));
    let p = (0..=6u8)
        .rev()
        .find(|&p| max * (1u32 << p) as f32 <= 65535.0)
        .unwrap_or(0);
    out.push(share as u8 | p << 2);
    for tc in &t[..n] {
        let mut prev = 0i64;
        for (z, &i) in coef::ZIGZAG.iter().enumerate() {
            let v = ((tc[i as usize] * (1u32 << p) as f32).round() as i64).clamp(1, 65535);
            if z == 0 {
                put_varint(out, v as u64);
            } else {
                let d = v - prev;
                put_varint(out, ((d << 1) ^ (d >> 63)) as u64);
            }
            prev = v;
        }
    }
}

/// Rounds tables through the explicit-table coding ([`write_explicit`] then
/// [`read_explicit`]) so an encoder quantizes with exactly the steps a
/// decoder reads. Values the coding cannot represent are `InvalidInput`.
#[cfg(feature = "lab")]
pub(super) fn round_trip_explicit(t: [[f32; 64]; 3]) -> Result<[[f32; 64]; 3], Error> {
    if !t
        .iter()
        .flatten()
        .all(|v| v.is_finite() && *v > 0.0 && *v <= 65535.0)
    {
        return Err(Error::InvalidInput("explicit quantization table value"));
    }
    let mut buf = Vec::new();
    write_explicit(&mut buf, &t);
    read_explicit(&mut ByteReader::new(&buf))
        .map_err(|_| Error::InvalidInput("explicit quantization table value"))
}

pub(super) fn read_explicit(p: &mut ByteReader<'_>) -> Result<[[f32; 64]; 3], Error> {
    let h = p.u8()?;
    let (share, prec) = (h & 3, (h >> 2) & 7);
    if share == 3 || prec > 6 || h >> 5 != 0 {
        return Err(Error::Corrupt("explicit quantization table header"));
    }
    let n = [3, 2, 1][share as usize];
    let mut t = [[0.0f32; 64]; 3];
    let unit = 1.0 / (1u32 << prec) as f32;
    for tc in t.iter_mut().take(n) {
        let mut v = 0i64;
        for (z, &i) in coef::ZIGZAG.iter().enumerate() {
            let x = p.varint()?;
            if z == 0 {
                v = x.min(1 << 20) as i64;
            } else {
                if x >= 1 << 20 {
                    return Err(Error::Corrupt("explicit quantization table value"));
                }
                let d = (x >> 1) as i64 ^ -((x & 1) as i64);
                v += d;
            }
            if !(1..=65535).contains(&v) {
                return Err(Error::Corrupt("explicit quantization table value"));
            }
            tc[i as usize] = v as f32 * unit;
        }
    }
    for c in n..3 {
        t[c] = t[c - 1];
    }
    Ok(t)
}

/// groups, 0 one for all channels, 1 luma and chroma, 2 one per channel;
/// bit 2: two frequency bands, then a byte k in 2..=63, the high band's
/// first zigzag index; bits 3-7 zero), then per group and band the i8
/// biases for |q| = 1 and |q| >= 2, in 1/256 of a step.
#[cfg(feature = "encode")]
pub(super) fn write_bias(out: &mut Vec<u8>, b: &ReconBias) {
    let two = b.k < 64 && b.b.iter().any(|c| c[0] != c[1]);
    let groups = if b.b[0] == b.b[1] && b.b[1] == b.b[2] {
        0
    } else if b.b[1] == b.b[2] {
        1
    } else {
        2
    };
    out.push(groups | (two as u8) << 2);
    if two {
        out.push(b.k);
    }
    let chans: &[usize] = [&[0][..], &[0, 1], &[0, 1, 2]][groups as usize];
    for &c in chans {
        for band in 0..1 + two as usize {
            out.extend(b.b[c][band].map(|v| v as u8));
        }
    }
}

pub(super) fn read_bias(p: &mut ByteReader<'_>) -> Result<ReconBias, Error> {
    let h = p.u8()?;
    let groups = h & 3;
    if groups == 3 || h >> 3 != 0 {
        return Err(Error::Corrupt("reconstruction bias header"));
    }
    let two = h & 4 != 0;
    let k = if two {
        let k = p.u8()?;
        if !(2..=63).contains(&k) {
            return Err(Error::Corrupt("reconstruction bias band"));
        }
        k
    } else {
        64
    };
    let mut b = [[[0i8; 2]; 2]; 3];
    let chans: &[usize] = [&[0][..], &[0, 1], &[0, 1, 2]][groups as usize];
    for &c in chans {
        for band in 0..1 + two as usize {
            b[c][band] = [p.u8()? as i8, p.u8()? as i8];
        }
        if !two {
            b[c][1] = b[c][0];
        }
    }
    for c in chans.len()..3 {
        b[c] = b[c - 1];
    }
    Ok(ReconBias { k, b })
}

/// Signalled color transform (Lossy 0.4, head record 17): the transform id,
/// then an explicit 3x4 f32 decode matrix for id 63.
#[cfg(feature = "encode")]
pub(super) fn write_transform(out: &mut Vec<u8>, t: ColorTransform) {
    out.push(t.id());
    if let ColorTransform::Explicit(m) = t {
        floats(out, &m.iter().flatten().copied().collect::<Vec<_>>());
    }
}

pub(super) fn read_transform(r: &mut ByteReader<'_>) -> Result<ColorTransform, Error> {
    Ok(match r.u8()? {
        1 => ColorTransform::Identity,
        2 => ColorTransform::Bt601,
        3 => ColorTransform::Bt709,
        4 => ColorTransform::YCoCg,
        63 => {
            let mut m = [[0.0f32; 4]; 3];
            for row in &mut m {
                for v in row {
                    *v = read_f32(r)?;
                }
            }
            if !ColorTransform::explicit_valid(&m) {
                return Err(Error::Corrupt("color transform value"));
            }
            ColorTransform::Explicit(m)
        }
        _ => return Err(Error::Unsupported("unknown color transform")),
    })
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::parse::header::{Feature, Features};

    fn params(color: LossyColor, qp: QParams) -> Params {
        Params {
            scale: defaults(color, InputKind::U8),
            qp,
            entropy: LossyEntropy::RunLevel,
            compact: true,
            seed: crate::seeds::Selection::NONE,
            records: false,
        }
    }

    fn roundtrip(color: LossyColor, p: &Params) -> (Vec<u8>, Params) {
        let mut out = Vec::new();
        write(&mut out, color, InputKind::U8, p);
        let mut r = ByteReader::new(&out);
        let back = read(
            &mut r,
            color,
            InputKind::U8,
            Features::EMPTY.with(Feature::SeedBank1),
            true,
        )
        .unwrap();
        assert_eq!(r.remaining(), 0);
        (out, back)
    }

    #[test]
    fn defaults_cost_nothing_and_overrides_round_trip() {
        for color in [LossyColor::Xyb, LossyColor::YCoCg] {
            let qp = QParams::from_quality(40.0, color, default_matrix(color));
            assert_eq!(qp.dc, default_dc_step(color));
            let p = params(color, qp);
            let (plain, back) = roundtrip(color, &p);
            // Every field at its default: `present` 0 and the scale.
            assert_eq!(plain.len(), 5);
            assert_eq!(plain[0], 0);
            assert_eq!(back, p);
            let mut custom = p;
            custom.qp.dc = [64, 7, 255];
            let (bytes, back) = roundtrip(color, &custom);
            assert_eq!(bytes[0] as u64, P_DC);
            assert_eq!(bytes.len(), plain.len() + 3);
            assert_eq!(back, custom);
            // The DC step follows the multiplier; AC steps do not.
            let (a, b) = (qp.steps(), custom.qp.steps());
            assert_eq!(a[0][1..], b[0][1..]);
            assert_eq!(b[0][0], (qp.base_steps()[0][0] * 1.0).max(1.0 / 64.0));
            // Every field written, in presence-bit order.
            let mut all = custom;
            all.seed = crate::seeds::Selection::legacy(0x11);
            all.entropy = LossyEntropy::SplitHuf;
            all.compact = false;
            all.records = true;
            all.qp.matrix = 1 - (default_matrix(color) & 1);
            all.qp.floor = 2.5;
            all.qp.mul = [0.5, 0.25, 2.0];
            all.scale = [3.0, 4.0, 5.0];
            let (bytes, back) = roundtrip(color, &all);
            assert_eq!(&bytes[..2], &[0xff, 0x03]);
            assert_eq!(back, all);
            assert_eq!(bytes.len(), 2 + 4 + 1 + 1 + 1 + 3 + 12 + 4 + 12);
        }
    }

    #[test]
    fn bad_values_and_unknown_fields_are_rejected() {
        let read_as = |b: &[u8], d| {
            read(
                &mut ByteReader::new(b),
                LossyColor::Xyb,
                InputKind::U8,
                d,
                true,
            )
        };
        let any = Features::EMPTY.with(Feature::SeedBank1);
        let qp = QParams::from_quality(40.0, LossyColor::Xyb, 2);
        let mut custom = params(LossyColor::Xyb, qp);
        custom.qp.dc = [64, 64, 64];
        let (mut bytes, _) = roundtrip(LossyColor::Xyb, &custom);
        let n = bytes.len();
        bytes[n - 1] = 0;
        assert_eq!(
            read_as(&bytes, any),
            Err(Error::Corrupt("zero DC step multiplier"))
        );
        // A field cut short.
        assert!(read_as(&bytes[..n - 1], any).is_err());
        // A presence bit this version does not define.
        let mut unknown = Vec::new();
        put_varint(&mut unknown, 1 << 10);
        unknown.extend_from_slice(&1f32.to_le_bytes());
        assert_eq!(
            read_as(&unknown, any),
            Err(Error::Unsupported("unknown lossy head field"))
        );
        // Compact states need run-level entropy.
        let mut split = params(LossyColor::Xyb, qp);
        split.entropy = LossyEntropy::SplitHuf;
        let mut out = Vec::new();
        write(&mut out, LossyColor::Xyb, InputKind::U8, &split);
        assert_eq!(
            read_as(&out, any),
            Err(Error::Corrupt("compact states with incompatible entropy"))
        );
        // Packed (1) and Split (2) are retired (SPEC 2.8): reserved values;
        // other values are unknown.
        let mut e = params(LossyColor::Xyb, qp);
        e.entropy = LossyEntropy::SplitHuf;
        let mut out = Vec::new();
        write(&mut out, LossyColor::Xyb, InputKind::U8, &e);
        assert_eq!(out[5], LossyEntropy::SplitHuf as u8);
        for (v, want) in [
            (1, Error::Unsupported("retired entropy mode")),
            (2, Error::Unsupported("retired entropy mode")),
            (4, Error::Unsupported("unknown entropy mode")),
        ] {
            let mut bad = out.clone();
            bad[5] = v;
            assert_eq!(read_as(&bad, any), Err(want), "entropy {v}");
        }
        // Matrix ids: the Lossy 0.4 ones are gated by their features (their
        // payloads are head records, parsed by the caller), others unknown.
        let gated = || Error::Corrupt("quantization matrix without its feature");
        for (m, d, want) in [
            (3u8, Features::EMPTY, Err(gated())),
            (4, Features::EMPTY.with(Feature::DequantPresets), Ok(4u8)),
            (63, Features::EMPTY, Err(gated())),
            (63, Features::EMPTY.with(Feature::ExplicitTables), Ok(63u8)),
            (5, Features::EMPTY, Err(gated())),
            (5, Features::EMPTY.with(Feature::DequantPresets), Ok(5u8)),
            // Lab builds read the tablemath study's probes 6-9 under the
            // same dequant_presets feature; default builds reject them.
            #[cfg(feature = "lab")]
            (6, Features::EMPTY, Err(gated())),
            #[cfg(feature = "lab")]
            (6, Features::EMPTY.with(Feature::DequantPresets), Ok(6u8)),
            #[cfg(not(feature = "lab"))]
            (
                6,
                Features::ALL,
                Err(Error::Unsupported("unknown quantization matrix")),
            ),
        ] {
            let mut p = params(LossyColor::Xyb, qp);
            p.qp.matrix = m;
            let mut out = Vec::new();
            write(&mut out, LossyColor::Xyb, InputKind::U8, &p);
            assert_eq!(read_as(&out, d).map(|p| p.qp.matrix), want, "matrix {m}");
        }
    }

    /// Explicit tables round-trip through the coding: the encoder quantizes
    /// with exactly the steps a decoder reads; unrepresentable values are
    /// `InvalidInput`.
    #[cfg(feature = "lab")]
    #[test]
    fn explicit_tables_round_trip_through_the_coding() {
        let mut t = [[0.0f32; 64]; 3];
        for (c, tc) in t.iter_mut().enumerate() {
            for (i, v) in tc.iter_mut().enumerate() {
                *v = 1.0 + (i + c) as f32 * 0.37;
            }
        }
        let back = round_trip_explicit(t).unwrap();
        assert_ne!(back, t, "values needing rounding must change");
        assert_eq!(round_trip_explicit(back).unwrap(), back, "idempotent");
        assert!(round_trip_explicit([[0.0; 64]; 3]).is_err());
        assert!(round_trip_explicit([[f32::NAN; 64]; 3]).is_err());
        assert!(round_trip_explicit([[70000.0; 64]; 3]).is_err());
    }

    /// The bias and transform record payloads round-trip, and unknown ids
    /// are `Unsupported`.
    #[test]
    fn bias_and_transform_records_round_trip() {
        let bias = ReconBias {
            k: 9,
            b: [
                [[12, 20], [12, 20]],
                [[-5, 7], [30, -128]],
                [[1, 2], [3, 4]],
            ],
        };
        let mut out = Vec::new();
        write_bias(&mut out, &bias);
        let back = read_bias(&mut ByteReader::new(&out)).unwrap();
        assert_eq!(back, bias);
        for t in [
            ColorTransform::Identity,
            ColorTransform::Bt601,
            ColorTransform::Bt709,
            ColorTransform::YCoCg,
            ColorTransform::Explicit(color::TRANSFORM_BT601),
        ] {
            let mut out = Vec::new();
            write_transform(&mut out, t);
            let back = read_transform(&mut ByteReader::new(&out)).unwrap();
            assert_eq!(back, t);
        }
        assert_eq!(
            read_transform(&mut ByteReader::new(&[5u8])),
            Err(Error::Unsupported("unknown color transform"))
        );
        let mut nan = Vec::new();
        nan.push(63);
        nan.extend_from_slice(&[f32::NAN.to_le_bytes(); 12].concat());
        assert!(read_transform(&mut ByteReader::new(&nan)).is_err());
    }

    /// The seed byte needs a file that declares `seed_bank_1`.
    #[test]
    fn seed_byte_needs_its_feature() {
        let qp = QParams::from_quality(40.0, LossyColor::Xyb, 2);
        let mut p = params(LossyColor::Xyb, qp);
        p.seed = crate::seeds::Selection::legacy(0x11);
        let mut out = Vec::new();
        write(&mut out, LossyColor::Xyb, InputKind::U8, &p);
        assert_eq!(out[0] as u64, P_SEED);
        let read_as = |d| {
            read(
                &mut ByteReader::new(&out),
                LossyColor::Xyb,
                InputKind::U8,
                d,
                true,
            )
        };
        assert_eq!(
            read_as(Features::EMPTY.with(Feature::SeedBank1))
                .unwrap()
                .seed,
            crate::seeds::Selection::legacy(0x11)
        );
        assert_eq!(
            read_as(Features::EMPTY).unwrap_err(),
            Error::Corrupt("seed tables without the feature seed_bank_1")
        );
        // Another bank has no feature in this version: gated syntax
        // without its feature (lab builds read their experimental banks).
        #[cfg(not(feature = "lab"))]
        {
            let mut two = out.clone();
            two[5] = 0x12;
            assert_eq!(
                read(
                    &mut ByteReader::new(&two),
                    LossyColor::Xyb,
                    InputKind::U8,
                    Features::ALL,
                    true
                )
                .unwrap_err(),
                Error::Corrupt("seed bank without its feature")
            );
        }
    }

    #[test]
    fn varint_bank_ids_and_scope_masks_are_independent_and_gated() {
        let qp = QParams::from_quality(40.0, LossyColor::Xyb, 2);
        for id in [
            1,
            127,
            128,
            16_383,
            16_384,
            u64::from(u32::MAX) + 1,
            u64::MAX,
        ] {
            for scope in 1..=7u8 {
                let mut p = params(LossyColor::Xyb, qp);
                p.seed = crate::seeds::Selection::new(id, scope << 4);
                let mut out = Vec::new();
                write(&mut out, LossyColor::Xyb, InputKind::U8, &p);
                let mut wire = ByteReader::new(&out);
                assert_eq!(wire.varint().unwrap(), P_SEED_ID);
                wire.bytes(4).unwrap();
                assert_eq!(wire.varint().unwrap(), id);
                assert_eq!(wire.varint().unwrap(), u64::from(scope));
                assert_eq!(wire.remaining(), 0);
                let parse = |features| {
                    read(
                        &mut ByteReader::new(&out),
                        LossyColor::Xyb,
                        InputKind::U8,
                        features,
                        true,
                    )
                };
                assert_eq!(
                    parse(Features::EMPTY).unwrap_err(),
                    Error::Corrupt("seed bank id without its feature")
                );
                if id == 1 {
                    assert_eq!(
                        parse(Features::EMPTY.with(Feature::SeedBankIds)).unwrap(),
                        p
                    );
                } else {
                    assert_eq!(
                        parse(Features::EMPTY.with(Feature::SeedBankIds)).unwrap_err(),
                        Error::Unsupported("unknown seed bank")
                    );
                }
            }
        }
        let parse = |tail: &[u8]| {
            let mut data = Vec::new();
            put_varint(&mut data, P_SEED_ID);
            data.extend_from_slice(&1f32.to_le_bytes());
            data.extend_from_slice(tail);
            read(
                &mut ByteReader::new(&data),
                LossyColor::Xyb,
                InputKind::U8,
                Features::ALL,
                true,
            )
        };
        for tail in [
            &[0, 1][..],
            &[1, 0],
            &[0x80],
            &[0x81, 0, 1],
            &[1, 0x81, 0],
            &[0xff; 11],
        ] {
            assert!(matches!(parse(tail), Err(Error::Corrupt(_))));
        }
        assert_eq!(
            parse(&[1, 8]).unwrap_err(),
            Error::Unsupported("unknown seed table scope")
        );
    }

    /// Writers parse heads for structure only (SPEC 2.8): declared Lossy
    /// 0.4 matrix ids decode in a decoding parse and are accepted (as a
    /// placeholder) by a structure parse; an unknown entropy mode is one
    /// a later version may define.
    #[test]
    fn structure_parses_accept_declared_lossy_0_4_syntax() {
        let qp = QParams::from_quality(40.0, LossyColor::Xyb, 2);
        for m in [3u8, 4, 5, 63] {
            let mut p = params(LossyColor::Xyb, qp);
            p.qp.matrix = m;
            let mut out = Vec::new();
            write(&mut out, LossyColor::Xyb, InputKind::U8, &p);
            let rd = |decodes| {
                read(
                    &mut ByteReader::new(&out),
                    LossyColor::Xyb,
                    InputKind::U8,
                    Features::ALL,
                    decodes,
                )
            };
            assert!(rd(true).is_ok(), "matrix {m}");
            assert!(rd(false).is_ok(), "matrix {m}");
        }
        let mut p = params(LossyColor::Xyb, qp);
        p.entropy = LossyEntropy::SplitHuf;
        p.compact = false;
        let mut out = Vec::new();
        write(&mut out, LossyColor::Xyb, InputKind::U8, &p);
        // present, quant scale, then the entropy byte.
        let at = 1 + 4;
        assert_eq!(out[at], LossyEntropy::SplitHuf as u8);
        out[at] = 4;
        assert_eq!(
            read(
                &mut ByteReader::new(&out),
                LossyColor::Xyb,
                InputKind::U8,
                Features::ALL,
                true
            )
            .unwrap_err(),
            Error::Unsupported("unknown entropy mode")
        );
    }
}
