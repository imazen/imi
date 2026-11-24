//! Channel geometry and borrowed per-parent transform selectors.
#[cfg(feature = "encode")]
use super::ZIGZAG;
use super::{ByteReader, Error, NCH, subblock::Layout};
#[cfg(all(test, feature = "encode"))]
use alloc::vec;
#[cfg(feature = "encode")]
use alloc::vec::Vec;

/// Largest transform within an 8×8 parent, after optional subsampling.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum LossyTransform {
    #[default]
    Dct8,
    Dct4,
    Dct2,
    /// Encoder chooses 8×8 or 4×4 per parent/channel using a bounded
    /// quantization-error and coefficient-bit estimate. No subsampling search.
    Adaptive4,
}

/// Sampling of a transformed color plane; alpha is always stored losslessly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChannelSampling {
    #[default]
    Full,
    HalfHorizontal,
    HalfVertical,
    HalfBoth,
}

/// Geometry of one coded color plane. XYB order is Y, X, B−Y; YCoCg order
/// is Y, Co, Cg. Subsampling uses centered box samples and parent-local
/// triangle reconstruction. It can introduce seams; it is explicitly opt-in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LossyChannelLayout {
    transform: LossyTransform,
    sampling: ChannelSampling,
}
impl LossyChannelLayout {
    pub const fn new(transform: LossyTransform, sampling: ChannelSampling) -> Self {
        Self {
            transform,
            sampling,
        }
    }
    pub const fn transform(self) -> LossyTransform {
        self.transform
    }
    pub const fn sampling(self) -> ChannelSampling {
        self.sampling
    }
    pub(super) fn adaptive(self) -> bool {
        self.transform == LossyTransform::Adaptive4
    }
    pub(super) fn layout(self, split: bool) -> Layout {
        let (sx, sy) = match self.sampling {
            ChannelSampling::Full => (0, 0),
            ChannelSampling::HalfHorizontal => (1, 0),
            ChannelSampling::HalfVertical => (0, 1),
            ChannelSampling::HalfBoth => (1, 1),
        };
        let log_block = match self.transform {
            LossyTransform::Dct8 => 3,
            LossyTransform::Dct4 => 2,
            LossyTransform::Dct2 => 1,
            LossyTransform::Adaptive4 => {
                if split {
                    2
                } else {
                    3
                }
            }
        };
        Layout { sx, sy, log_block }
    }
    #[cfg(feature = "encode")]
    pub(super) fn byte(self) -> u8 {
        self.layout(false).byte() | ((self.adaptive() as u8) << 4)
    }
    pub(super) fn from_byte(b: u8) -> Result<Self, Error> {
        let l = Layout::from_byte(b & !16)?;
        if b & 16 != 0 && l.log_block != 3 {
            return Err(Error::Corrupt("adaptive transform base"));
        }
        Ok(Self {
            transform: if b & 16 != 0 {
                LossyTransform::Adaptive4
            } else {
                match l.log_block {
                    1 => LossyTransform::Dct2,
                    2 => LossyTransform::Dct4,
                    _ => LossyTransform::Dct8,
                }
            },
            sampling: match (l.sx, l.sy) {
                (0, 0) => ChannelSampling::Full,
                (1, 0) => ChannelSampling::HalfHorizontal,
                (0, 1) => ChannelSampling::HalfVertical,
                _ => ChannelSampling::HalfBoth,
            },
        })
    }
}

pub(super) fn adaptive_mask(layouts: &[LossyChannelLayout; NCH]) -> u8 {
    layouts
        .iter()
        .enumerate()
        .fold(0, |m, (c, l)| m | ((l.adaptive() as u8) << c))
}

/// A selector map borrows the record. No image-sized map or decode allocation.
#[derive(Clone, Copy)]
pub(super) enum ModeMap<'a> {
    Constant(u8),
    Packed(&'a [u8]),
}
impl ModeMap<'_> {
    pub(super) fn get(self, b: usize) -> u8 {
        match self {
            Self::Constant(v) => v,
            Self::Packed(p) => {
                let bit = b * 3;
                let word =
                    p[bit / 8] as u16 | ((p.get(bit / 8 + 1).copied().unwrap_or(0) as u16) << 8);
                ((word >> (bit % 8)) & 7) as u8
            }
        }
    }
}

#[cfg(feature = "encode")]
pub(super) fn write_modes(out: &mut Vec<u8>, modes: &[u8]) {
    let first = modes[0];
    if modes.iter().all(|&m| m == first) {
        out.extend_from_slice(&[0, first]);
        return;
    }
    out.push(1);
    let n = (modes.len() * 3).div_ceil(8);
    let start = out.len();
    out.resize(start + n, 0);
    for (i, &m) in modes.iter().enumerate() {
        let bit = i * 3;
        out[start + bit / 8] |= m << (bit % 8);
        if bit % 8 > 5 {
            out[start + bit / 8 + 1] |= m >> (8 - bit % 8);
        }
    }
}

pub(super) fn read_modes<'a>(
    bytes: &'a [u8],
    blocks: usize,
    mask: u8,
) -> Result<(ModeMap<'a>, &'a [u8]), Error> {
    if mask == 0 {
        return Ok((ModeMap::Constant(0), bytes));
    }
    let mut r = ByteReader::new(bytes);
    let modes = match r.u8()? {
        0 => {
            let m = r.u8()?;
            if m & !mask != 0 {
                return Err(Error::Corrupt("unused transform selector"));
            }
            ModeMap::Constant(m)
        }
        1 => {
            let bits = blocks.checked_mul(3).ok_or(Error::TooLarge)?;
            let p = r.bytes(bits.div_ceil(8))?;
            if bits % 8 != 0 && p.last().is_some_and(|&v| v >> (bits % 8) != 0) {
                return Err(Error::Corrupt("transform map padding"));
            }
            let m = ModeMap::Packed(p);
            for b in 0..blocks {
                if m.get(b) & !mask != 0 {
                    return Err(Error::Corrupt("unused transform selector"));
                }
            }
            m
        }
        _ => return Err(Error::Corrupt("transform map kind")),
    };
    // LAB trace: the per-block transform selectors, one digit per block in
    // raster order of the record's region (bit c = plane c uses 4x4).
    #[cfg(all(feature = "lab", feature = "std"))]
    if crate::trace::on() {
        let v = match &modes {
            ModeMap::Constant(m) => {
                alloc::format!("constant {m}: planes {m:03b} (bit c = plane c 4x4)")
            }
            ModeMap::Packed(_) => {
                let d: alloc::string::String = (0..blocks)
                    .map(|b| char::from(b'0' + modes.get(b)))
                    .collect();
                alloc::format!("per block {blocks}: {d}")
            }
        };
        crate::trace::field_addr(
            "transform map",
            v,
            crate::trace::Mark::Addr(bytes.as_ptr() as usize),
            r.mark(),
        );
    }
    Ok((modes, &bytes[r.pos..]))
}

/// Transform/quantize one nondefault channel of a block row. Existing full8
/// coefficients are supplied only when needed by the base/adaptive candidate.
/// The source is eight planar rows with explicit float-sample stride.
#[cfg(feature = "encode")]
pub(super) fn encode_row(
    source: &[f32],
    stride: usize,
    bw: usize,
    channel: usize,
    config: LossyChannelLayout,
    steps: &[[f32; 64]; 2],
    normalizer: f32,
    full8: &[f32],
    aq_row: &[f32],
    round: f32,
    dc_round: f32,
    q: &mut [i32],
    nz: &mut [u64],
    modes: &mut [u8],
) {
    let base = config.layout(false);
    for bx in 0..bw {
        let pixels = core::array::from_fn(|i| source[(i / 8) * stride + bx * 8 + i % 8]);
        let c0 = if base == Layout::FULL8 {
            full8[bx * 64..bx * 64 + 64].try_into().unwrap()
        } else {
            base.forward(&pixels)
        };
        let aq = aq_row.get(bx).copied().unwrap_or(1.0);
        let q0 = quantize(&c0, &steps[0], aq, round, dc_round);
        let mut chosen = q0;
        if config.adaptive() && base != config.layout(true) {
            let c1 = config.layout(true).forward(&pixels);
            let q1 = quantize(&c1, &steps[1], aq, round, dc_round);
            if cost(&c1, &q1, &steps[1], aq, normalizer * aq)
                < cost(&c0, &q0, &steps[0], aq, normalizer * aq)
            {
                chosen = q1;
                modes[bx] |= 1 << channel;
            }
        }
        q[(bx * NCH + channel) * 64..][..64].copy_from_slice(&chosen);
        nz[bx * NCH + channel] = ZIGZAG
            .iter()
            .enumerate()
            .fold(0, |m, (k, &i)| m | ((chosen[i as usize] != 0) as u64) << k);
    }
}

/// `dc_round`: the DC's rounding offset (0.5, nearest, unless a lab encode
/// turns nearest DC rounding off).
#[cfg(feature = "encode")]
pub(super) fn quantize(
    c: &[f32; 64],
    steps: &[f32; 64],
    aq: f32,
    round: f32,
    dc_round: f32,
) -> [i32; 64] {
    core::array::from_fn(|i| {
        let s = steps[i] * if i == 0 { 1.0 } else { aq };
        // The block mean rounds to nearest (no dead zone; see `dc_row`).
        let r = if i == 0 { dc_round } else { round };
        let v = (c[i].abs() / s + r).min(4_194_303.0) as i32;
        if c[i] < 0.0 { -v } else { v }
    })
}

/// Orthonormal coefficient SSE with the SAME channel normalization for both
/// candidates; equal to sampled-plane SSE up to transform rounding.
/// Encoder-only rate proxy. Include a selector bit, DC, signs, magnitude and
/// runs; actual global entropy-table coupling is evaluated by corpus RD tests.
#[cfg(feature = "encode")]
pub(super) fn cost(
    c: &[f32; 64],
    q: &[i32; 64],
    steps: &[f32; 64],
    aq: f32,
    normalizer: f32,
) -> f64 {
    let mut error = 0.0f64;
    let mut bits = 1u32;
    let mut run = 0u32;
    for &i in &ZIGZAG {
        let i = i as usize;
        let step = steps[i] * if i == 0 { 1.0 } else { aq };
        let residual = ((c[i] - q[i] as f32 * step) / normalizer) as f64;
        error += residual * residual;
        if q[i] == 0 {
            run += 1;
        } else {
            bits +=
                2 + (32 - q[i].unsigned_abs().leading_zeros()) + (32 - (run + 1).leading_zeros());
            run = 0;
        }
    }
    error + 0.08 * bits as f64
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    #[test]
    fn selector_maps_roundtrip_and_reject_reserved_bits() {
        for mask in 1..8 {
            for n in 1..130 {
                let modes: Vec<_> = (0..n).map(|i| (i as u8 & 7) & mask).collect();
                let mut bytes = vec![];
                write_modes(&mut bytes, &modes);
                bytes.push(91);
                let (m, rest) = read_modes(&bytes, n, mask).unwrap();
                assert_eq!(rest, [91]);
                for (i, &v) in modes.iter().enumerate() {
                    assert_eq!(m.get(i), v);
                }
                for cut in 0..bytes.len() - 1 {
                    assert!(read_modes(&bytes[..cut], n, mask).is_err());
                }
            }
        }
        assert!(read_modes(&[0, 8], 1, 7).is_err());
        assert!(read_modes(&[1, 0x80], 1, 7).is_err());
        assert!(read_modes(&[2, 0], 1, 7).is_err());
    }
}
