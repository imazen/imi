// Exact byte-plane units: constant bits, dense masks and optional pixel packets.
use super::pixels::{ByteDecoder, Shape};
use alloc::vec::Vec;
use core::ops::Range;
type Result<T> = core::result::Result<T, &'static str>;

fn reserve<T>(v: &mut Vec<T>, n: usize) -> Result<()> {
    v.try_reserve_exact(n).map_err(|_| "allocation")
}
#[cfg(feature = "encode")]
fn var(out: &mut Vec<u8>, mut n: usize) {
    while n >= 128 {
        out.push(n as u8 | 128);
        n >>= 7;
    }
    out.push(n as u8);
}
fn read_var(data: &[u8], pos: &mut usize) -> Result<usize> {
    let mut n = 0usize;
    let mut shift = 0;
    loop {
        let b = *data.get(*pos).ok_or("plane lengths")?;
        *pos += 1;
        if shift >= usize::BITS || usize::from(b & 127) > usize::MAX >> shift {
            return Err("plane length overflow");
        }
        n |= usize::from(b & 127) << shift;
        if b < 128 {
            if shift > 0 && b == 0 {
                return Err("non-minimal plane length");
            }
            return Ok(n);
        }
        shift += 7;
    }
}
#[cfg(feature = "encode")]
fn compact(v: u8, mask: u8) -> u8 {
    let mut code = 0;
    let mut to = 0;
    for from in 0..8 {
        if mask & (1 << from) != 0 {
            code |= ((v >> from) & 1) << to;
            to += 1;
        }
    }
    code
}
fn expand(code: u8, base: u8, mask: u8) -> u8 {
    let mut value = base;
    let mut from = 0;
    for to in 0..8 {
        if mask & (1 << to) != 0 {
            value |= ((code >> from) & 1) << to;
            from += 1;
        }
    }
    value
}

/// Body starts with mode 2, then N lengths, then N complete plane bodies.
/// N is channels * sample bytes. Plane bodies: raw 0, packets 1, constant 2,
/// fixed-mask packed/packet 3/4, dictionary packed/packet 5/6,
/// or earlier-plane reuse 7. Dense codes are LSB-first with zero final padding.
#[cfg(feature = "encode")]
pub(crate) fn encode(
    input: &[u8],
    stride: usize,
    shape: Shape,
    check: impl FnMut() -> Result<()>,
) -> Result<Vec<u8>> {
    encode_variant(input, stride, shape, true, check)
}

#[cfg(feature = "encode")]
pub(crate) fn encode_variant(
    input: &[u8],
    stride: usize,
    shape: Shape,
    packets_enabled: bool,
    mut check: impl FnMut() -> Result<()>,
) -> Result<Vec<u8>> {
    shape.check(input.len(), stride)?;
    let n = shape.channels * shape.bytes;
    let count = shape.width.checked_mul(shape.height).ok_or("plane size")?;
    let mut bodies = Vec::new();
    reserve(&mut bodies, n)?;
    let mut originals: Vec<Vec<u8>> = Vec::new();
    reserve(&mut originals, n.saturating_sub(1))?;
    // Reusable scratch across planes: avoids allocating separate `saved`,
    // `remapped` and three `count * 2` packet buffers per plane.
    let mut remapped = Vec::new();
    let mut trial = Vec::new();
    let packet_cap = if packets_enabled {
        count
            .checked_mul(2)
            .and_then(|v| v.checked_add(258))
            .ok_or("plane size")?
    } else {
        0
    };
    let plane_shape = Shape {
        channels: 1,
        bytes: 1,
        ..shape
    };
    for plane in 0..n {
        let mut data = Vec::new();
        reserve(&mut data, count)?;
        let (mut and, mut or) = (255u8, 0u8);
        let mut seen = [false; 256];
        for y in 0..shape.height {
            check()?;
            let row = &input[y * stride..y * stride + shape.width * n];
            for (x, px) in row.chunks_exact(n).enumerate() {
                if x & 16383 == 0 {
                    check()?;
                }
                let v = px[plane];
                data.push(v);
                and &= v;
                or |= v;
                seen[v as usize] = true;
            }
        }
        let mask = and ^ or;
        let bits = mask.count_ones() as u8;
        if bits == 0 {
            bodies.push(alloc::vec![2, and]);
            if plane + 1 < n {
                originals.push(Vec::new());
            }
            continue;
        }
        if let Some(previous) = originals
            .iter()
            .position(|old| !old.is_empty() && old == &data)
        {
            bodies.push(alloc::vec![7, previous as u8]);
            if plane + 1 < n {
                originals.push(Vec::new());
            }
            continue;
        }
        let mut body = Vec::new();
        let packed_len = if bits < 8 {
            3 + count
                .checked_mul(bits as usize)
                .ok_or("plane size")?
                .div_ceil(8)
        } else {
            count.checked_add(1).ok_or("plane size")?
        };
        reserve(&mut body, packed_len)?;
        if bits < 8 {
            body.extend_from_slice(&[3, and, mask]);
            let codes: [u8; 256] = core::array::from_fn(|v| compact(v as u8, mask));
            let mut acc = 0u16;
            let mut used = 0;
            for row in data.chunks_exact(shape.width) {
                check()?;
                for (i, &v) in row.iter().enumerate() {
                    if i & 16383 == 0 {
                        check()?;
                    }
                    acc |= u16::from(codes[v as usize]) << used;
                    used += bits;
                    if used >= 8 {
                        body.push(acc as u8);
                        acc >>= 8;
                        used -= 8;
                    }
                }
            }
            if used != 0 {
                body.push(acc as u8);
            }
        } else {
            body.push(0);
            body.extend_from_slice(&data);
        }
        if packets_enabled {
            trial.clear();
            if trial.capacity() < packet_cap {
                reserve(&mut trial, packet_cap)?;
            }
            trial.push(1);
            super::pixels::encode_into(&data, shape.width, plane_shape, &mut trial, &mut check)?;
            if trial.len() < body.len() {
                body.clear();
                body.extend_from_slice(&trial);
            }
            if bits < 8 {
                remapped.clear();
                if remapped.capacity() < count {
                    reserve(&mut remapped, count)?;
                }
                let codes: [u8; 256] = core::array::from_fn(|v| compact(v as u8, mask));
                for (i, &v) in data.iter().enumerate() {
                    if i & 16383 == 0 {
                        check()?;
                    }
                    remapped.push(codes[v as usize]);
                }
                trial.clear();
                trial.extend_from_slice(&[4, and, mask]);
                super::pixels::encode_into(
                    &remapped,
                    shape.width,
                    plane_shape,
                    &mut trial,
                    &mut check,
                )?;
                if trial.len() < body.len() {
                    body.clear();
                    body.extend_from_slice(&trial);
                }
            }
        }
        // Exact value remapping handles sparse/full-range byte values even
        // when no individual bit is constant (e.g. bit-replicated samples).
        let mut values = [0u8; 256];
        let mut nvalues = 0usize;
        for (v, &present) in seen.iter().enumerate() {
            if present {
                values[nvalues] = v as u8;
                nvalues += 1;
            }
        }
        if nvalues < 256 {
            let dict_bits = (usize::BITS - (nvalues - 1).leading_zeros()) as u8;
            let dict_packed_len = 2 + nvalues
                + count
                    .checked_mul(dict_bits as usize)
                    .ok_or("plane size")?
                    .div_ceil(8);
            let try_dict_packed = dict_packed_len < body.len();
            if try_dict_packed || packets_enabled {
                let mut indices = [0u8; 256];
                for (i, &v) in values[..nvalues].iter().enumerate() {
                    indices[v as usize] = i as u8;
                }
                if try_dict_packed {
                    let mut dictionary = Vec::new();
                    reserve(&mut dictionary, dict_packed_len)?;
                    dictionary.extend_from_slice(&[5, (nvalues - 1) as u8]);
                    dictionary.extend_from_slice(&values[..nvalues]);
                    let mut acc = 0u16;
                    let mut used = 0;
                    for (i, &v) in data.iter().enumerate() {
                        if i & 16383 == 0 {
                            check()?;
                        }
                        acc |= u16::from(indices[v as usize]) << used;
                        used += dict_bits;
                        if used >= 8 {
                            dictionary.push(acc as u8);
                            acc >>= 8;
                            used -= 8;
                        }
                    }
                    if used != 0 {
                        dictionary.push(acc as u8);
                    }
                    if dictionary.len() < body.len() {
                        body = dictionary;
                    }
                }
                if packets_enabled {
                    remapped.clear();
                    if remapped.capacity() < count {
                        reserve(&mut remapped, count)?;
                    }
                    for (i, &v) in data.iter().enumerate() {
                        if i & 16383 == 0 {
                            check()?;
                        }
                        remapped.push(indices[v as usize]);
                    }
                    trial.clear();
                    trial.extend_from_slice(&[6, (nvalues - 1) as u8]);
                    trial.extend_from_slice(&values[..nvalues]);
                    super::pixels::encode_into(
                        &remapped,
                        shape.width,
                        plane_shape,
                        &mut trial,
                        &mut check,
                    )?;
                    if trial.len() < body.len() {
                        body.clear();
                        body.extend_from_slice(&trial);
                    }
                }
            }
        }
        if plane + 1 < n {
            originals.push(data);
        }
        bodies.push(body);
    }
    let size = bodies
        .iter()
        .try_fold(1 + 10 * n, |s, b| s.checked_add(b.len()))
        .ok_or("plane size")?;
    let mut out = Vec::new();
    reserve(&mut out, size)?;
    out.push(2);
    for body in &bodies {
        var(&mut out, body.len());
    }
    for body in bodies {
        out.extend_from_slice(&body);
    }
    check()?;
    Ok(out)
}

fn unpack8<const B: usize>(
    data: &[u8],
    out: &mut [u8],
    lut: &[u8; 256],
    limit: u16,
) -> Result<usize> {
    let count = (data.len() / B).min(out.len() / 8);
    for (src, dst) in data
        .as_chunks::<B>()
        .0
        .iter()
        .zip(out.as_chunks_mut::<8>().0)
        .take(count)
    {
        let mut word = [0; 8];
        word[..B].copy_from_slice(src);
        let word = u64::from_le_bytes(word);
        for (i, v) in dst.iter_mut().enumerate() {
            let code = ((word >> (i * B)) & ((1 << B) - 1)) as usize;
            if code >= limit as usize {
                return Err("mapped plane value");
            }
            *v = lut[code];
        }
    }
    Ok(count * 8)
}

// Inline byte packet state avoids a separate allocation per plane. Admission
// charges the full largest variant for every entry in the bounded table.
#[allow(clippy::large_enum_variant)]
enum Coding {
    Raw,
    Constant(u8),
    Copy(usize),
    Packets(ByteDecoder),
    MaskPackets {
        decoder: ByteDecoder,
        lut: [u8; 256],
        limit: u16,
    },
    Mask {
        bits: u8,
        lut: [u8; 256],
        limit: u16,
        offset: usize,
        acc: u16,
        pending: u8,
    },
}
struct Plane {
    range: Range<usize>,
    coding: Coding,
}
pub(crate) struct State {
    shape: Shape,
    planes: Vec<Plane>,
    scratch: Vec<u8>,
    row: usize,
    failed: Option<&'static str>,
}
impl State {
    pub(crate) fn new(shape: Shape, payload: &[u8]) -> Result<Self> {
        shape.row()?;
        if payload.first() != Some(&2) {
            return Err("plane unit mode");
        }
        let count = shape.width.checked_mul(shape.height).ok_or("plane size")?;
        let n = shape.bytes * shape.channels;
        let mut pos = 1;
        let mut lengths = [0usize; 32];
        for len in &mut lengths[..n] {
            *len = read_var(payload, &mut pos)?;
        }
        let mut planes = Vec::new();
        reserve(&mut planes, n)?;
        for &len in &lengths[..n] {
            let end = pos.checked_add(len).ok_or("plane size")?;
            let data = payload.get(pos..end).ok_or("plane extent")?;
            let mode = *data.first().ok_or("empty plane")?;
            let coding = match mode {
                0 if len == count.checked_add(1).ok_or("plane size")? => Coding::Raw,
                0 => return Err("raw plane extent"),
                1 => Coding::Packets(ByteDecoder::new(Shape {
                    channels: 1,
                    bytes: 1,
                    ..shape
                })?),
                2 if len == 2 => Coding::Constant(data[1]),
                2 => return Err("constant plane extent"),
                3 => {
                    let (&base, &mask) = data.get(1).zip(data.get(2)).ok_or("plane mask")?;
                    let bits = mask.count_ones() as u8;
                    if !(1..=7).contains(&bits) || base & mask != 0 {
                        return Err("plane mask");
                    }
                    let total = count.checked_mul(bits as usize).ok_or("plane size")?;
                    if len != 3 + total.div_ceil(8) {
                        return Err("masked plane extent");
                    }
                    if total % 8 != 0 && data[len - 1] >> (total % 8) != 0 {
                        return Err("masked plane padding");
                    }
                    Coding::Mask {
                        bits,
                        lut: core::array::from_fn(|v| expand(v as u8, base, mask)),
                        limit: 1u16 << bits,
                        offset: 3,
                        acc: 0,
                        pending: 0,
                    }
                }
                4 => {
                    let (&base, &mask) = data.get(1).zip(data.get(2)).ok_or("plane mask")?;
                    let bits = mask.count_ones() as u8;
                    if !(1..=7).contains(&bits) || base & mask != 0 {
                        return Err("plane mask");
                    }
                    Coding::MaskPackets {
                        decoder: ByteDecoder::new(Shape {
                            channels: 1,
                            bytes: 1,
                            ..shape
                        })?,
                        lut: core::array::from_fn(|v| expand(v as u8, base, mask)),
                        limit: 1u16 << bits,
                    }
                }
                5 | 6 => {
                    let entries = usize::from(*data.get(1).ok_or("plane dictionary")?) + 1;
                    if entries < 2 {
                        return Err("plane dictionary");
                    }
                    let values = data.get(2..2 + entries).ok_or("plane dictionary")?;
                    if values.windows(2).any(|v| v[0] >= v[1]) {
                        return Err("plane dictionary order");
                    }
                    let bits = (usize::BITS - (entries - 1).leading_zeros()) as u8;
                    let mut lut = [0; 256];
                    lut[..entries].copy_from_slice(values);
                    if data[0] == 5 {
                        let total = count.checked_mul(bits as usize).ok_or("plane size")?;
                        if len != 2 + entries + total.div_ceil(8) {
                            return Err("dictionary plane extent");
                        }
                        if total % 8 != 0 && data[len - 1] >> (total % 8) != 0 {
                            return Err("masked plane padding");
                        }
                        Coding::Mask {
                            bits,
                            lut,
                            limit: entries as u16,
                            offset: 2 + entries,
                            acc: 0,
                            pending: 0,
                        }
                    } else {
                        Coding::MaskPackets {
                            decoder: ByteDecoder::new(Shape {
                                channels: 1,
                                bytes: 1,
                                ..shape
                            })?,
                            lut,
                            limit: entries as u16,
                        }
                    }
                }
                7 => {
                    let source = usize::from(*data.get(1).ok_or("plane reference")?);
                    if len != 2 || source >= planes.len() {
                        return Err("plane reference");
                    }
                    Coding::Copy(source)
                }
                _ => return Err("unsupported quick plane mode"),
            };
            planes.push(Plane {
                range: pos..end,
                coding,
            });
            pos = end;
        }
        if pos != payload.len() {
            return Err("trailing plane bytes");
        }
        let mut scratch = Vec::new();
        let row = shape.row()?;
        reserve(&mut scratch, row)?;
        scratch.resize(row, 0);
        Ok(Self {
            shape,
            planes,
            scratch,
            row: 0,
            failed: None,
        })
    }
    #[cfg(all(test, feature = "encode"))]
    pub(crate) fn heap_bytes(&self) -> usize {
        self.planes.capacity() * core::mem::size_of::<Plane>() + self.scratch.capacity()
    }
    pub(crate) fn next(&mut self, payload: &[u8], out: &mut [u8]) -> Result<()> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        let result = self.row(payload, out);
        if let Err(error) = result {
            self.failed = Some(error);
        }
        result
    }
    fn row(&mut self, payload: &[u8], out: &mut [u8]) -> Result<()> {
        if self.row >= self.shape.height || out.len() < self.shape.row()? {
            return Err("plane output row");
        }
        let width = self.shape.width;
        for (ordinal, plane) in self.planes.iter_mut().enumerate() {
            let (previous, remaining) = self.scratch.split_at_mut(ordinal * width);
            let dest = &mut remaining[..width];
            let data = payload.get(plane.range.clone()).ok_or("plane extent")?;
            match &mut plane.coding {
                Coding::Raw => dest.copy_from_slice(&data[1 + self.row * width..][..width]),
                Coding::Constant(v) => dest.fill(*v),
                Coding::Copy(source) => dest.copy_from_slice(&previous[*source * width..][..width]),
                Coding::Packets(d) => d.next(&data[1..], dest)?,
                Coding::MaskPackets {
                    decoder,
                    lut,
                    limit,
                } => {
                    let start = if data[0] == 4 {
                        3
                    } else {
                        3 + usize::from(data[1])
                    };
                    decoder.next(&data[start..], dest)?;
                    for v in dest {
                        if u16::from(*v) >= *limit {
                            return Err("mapped packet value");
                        }
                        *v = lut[*v as usize];
                    }
                }
                Coding::Mask {
                    bits,
                    lut,
                    limit,
                    offset,
                    acc,
                    pending,
                } => {
                    let done = if *pending == 0 {
                        let bytes = data.get(*offset..).ok_or("masked plane extent")?;
                        match *bits {
                            1 => unpack8::<1>(bytes, dest, lut, *limit)?,
                            2 => unpack8::<2>(bytes, dest, lut, *limit)?,
                            3 => unpack8::<3>(bytes, dest, lut, *limit)?,
                            4 => unpack8::<4>(bytes, dest, lut, *limit)?,
                            5 => unpack8::<5>(bytes, dest, lut, *limit)?,
                            6 => unpack8::<6>(bytes, dest, lut, *limit)?,
                            7 => unpack8::<7>(bytes, dest, lut, *limit)?,
                            8 => unpack8::<8>(bytes, dest, lut, *limit)?,
                            _ => return Err("plane mask"),
                        }
                    } else {
                        0
                    };
                    *offset += done / 8 * usize::from(*bits);
                    for v in &mut dest[done..] {
                        if *pending < *bits {
                            *acc |= u16::from(*data.get(*offset).ok_or("masked plane extent")?)
                                << *pending;
                            *offset += 1;
                            *pending += 8;
                        }
                        let code = *acc & ((1 << *bits) - 1);
                        if code >= *limit {
                            return Err("mapped plane value");
                        }
                        *v = lut[code as usize];
                        *acc >>= *bits;
                        *pending -= *bits;
                    }
                }
            }
        }
        let n = self.planes.len();
        if n == 4 {
            let (a, rest) = self.scratch.split_at(width);
            let (b, rest) = rest.split_at(width);
            let (c, d) = rest.split_at(width);
            for (pixel, (((a, b), c), d)) in out[..self.shape.row()?]
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(a.iter().zip(b).zip(c).zip(d))
            {
                *pixel = [*a, *b, *c, *d];
            }
        } else {
            for (x, pixel) in out[..self.shape.row()?].chunks_exact_mut(n).enumerate() {
                for (plane, v) in pixel.iter_mut().enumerate() {
                    *v = self.scratch[plane * width + x];
                }
            }
        }
        self.row += 1;
        Ok(())
    }
}

/// Heap capacities retained by a live unit; the outer quick state is charged separately.
pub(crate) fn working_bytes(width: u128, bytes_per_pixel: u128) -> Option<u128> {
    bytes_per_pixel
        .checked_mul(core::mem::size_of::<Plane>() as u128)?
        .checked_add(width.checked_mul(bytes_per_pixel)?)
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    fn file(bodies: &[&[u8]]) -> Vec<u8> {
        let mut out = alloc::vec![2];
        for b in bodies {
            var(&mut out, b.len());
        }
        for b in bodies {
            out.extend_from_slice(b);
        }
        out
    }
    fn decode(body: &[u8], shape: Shape) -> Result<Vec<u8>> {
        let mut state = State::new(shape, body)?;
        let row = shape.row()?;
        let mut out = alloc::vec![0xa5;(row+3)*shape.height];
        for y in 0..shape.height {
            state.next(body, &mut out[y * (row + 3)..y * (row + 3) + row])?;
            assert!(
                out[y * (row + 3) + row..(y + 1) * (row + 3)]
                    .iter()
                    .all(|v| *v == 0xa5)
            );
        }
        Ok(out
            .chunks_exact(row + 3)
            .flat_map(|r| r[..row].iter().copied())
            .collect())
    }
    #[test]
    fn all_plane_kinds_and_reused_rows_are_exact() {
        let s = Shape {
            width: 2,
            height: 3,
            channels: 1,
            bytes: 1,
        };
        let raw = [0, 10, 20, 20, 20, 20, 20];
        let packets = [1, 254, 10, 254, 20, 195];
        let constant = [2, 20];
        let masked = [3, 2, 0xa0, 0xa9, 0x0a];
        let mapped = [4, 2, 0xa0, 254, 1, 127, 195];
        let dict = [5, 2, 10, 20, 99, 0xa4, 0x0a];
        let dict_packets = [6, 2, 10, 20, 99, 254, 0, 127, 195];
        assert_eq!(decode(&file(&[&raw]), s).unwrap(), [10, 20, 20, 20, 20, 20]);
        assert_eq!(
            decode(&file(&[&packets]), s).unwrap(),
            [10, 20, 20, 20, 20, 20]
        );
        assert_eq!(decode(&file(&[&constant]), s).unwrap(), [20; 6]);
        assert_eq!(
            decode(&file(&[&masked]), s).unwrap(),
            [34, 130, 130, 130, 130, 130]
        );
        assert_eq!(
            decode(&file(&[&mapped]), s).unwrap(),
            [34, 130, 130, 130, 130, 130]
        );
        assert_eq!(
            decode(&file(&[&dict]), s).unwrap(),
            [10, 20, 99, 99, 99, 99]
        );
        assert_eq!(
            decode(&file(&[&dict_packets]), s).unwrap(),
            [10, 20, 20, 20, 20, 20]
        );
        // Legal full dictionaries use 8-bit indices, including index 255.
        let mut full = alloc::vec![5, 255];
        full.extend(0u8..=255);
        full.extend([255, 1, 2, 3, 4, 5]);
        assert_eq!(decode(&file(&[&full]), s).unwrap(), [255, 1, 2, 3, 4, 5]);
        full[0] = 6;
        full.truncate(258);
        full.extend([254, 255, 254, 1, 127, 127, 127, 127]);
        assert_eq!(decode(&file(&[&full]), s).unwrap(), [255, 1, 2, 3, 4, 5]);
        let pair = Shape { channels: 2, ..s };
        assert_eq!(
            decode(&file(&[&raw, &[7, 0]]), pair).unwrap(),
            [10, 10, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20]
        );
        let state = State::new(pair, &file(&[&raw, &[7, 0]])).unwrap();
        assert!((state.heap_bytes() as u128) <= working_bytes(pair.width as u128, 2).unwrap());
    }
    #[test]
    fn dictionary_and_mask_errors_are_sticky_and_bounded() {
        let s = Shape {
            width: 9,
            height: 1,
            channels: 1,
            bytes: 1,
        };
        for codes in [alloc::vec![0xff, 0xff, 3], alloc::vec![0, 0, 3]] {
            let mut b = alloc::vec![5, 2, 10, 20, 99];
            b.extend(codes);
            let f = file(&[&b]);
            let mut state = State::new(s, &f).unwrap();
            let mut out = [0xa5; 9];
            assert_eq!(state.next(&f, &mut out), Err("mapped plane value"));
            assert_eq!(state.next(&f, &mut out), Err("mapped plane value"));
        }
        let s = Shape { width: 1, ..s };
        for b in [&[4, 0, 3, 254, 4][..], &[6, 2, 10, 20, 99, 254, 3][..]] {
            let f = file(&[b]);
            let mut state = State::new(s, &f).unwrap();
            let mut out = [0];
            assert_eq!(state.next(&f, &mut out), Err("mapped packet value"));
            assert_eq!(state.next(&f, &mut out), Err("mapped packet value"));
        }
        for b in [
            &[5, 2, 20, 10, 99, 0][..],
            &[5, 0, 0, 0][..],
            &[7, 0][..],
            &[3, 1, 1, 0][..],
            &[3, 0, 1, 0x80][..],
        ] {
            assert!(State::new(s, &file(&[b])).is_err());
        }
    }
    #[test]
    fn sparse_values_and_replicated_words_choose_exact_compact_units() {
        for packets in [false, true] {
            for bits in 1..=7 {
                let s = Shape {
                    width: 127,
                    height: 17,
                    channels: 1,
                    bytes: 2,
                };
                let max = (1u16 << bits) - 1;
                let raw: Vec<u8> = (0..s.width * s.height)
                    .flat_map(|i| {
                        let code = (i as u16) % (max + 1);
                        ((u32::from(code) * 65535 / u32::from(max)) as u16).to_le_bytes()
                    })
                    .collect();
                let f = encode_variant(&raw, s.row().unwrap(), s, packets, || Ok(())).unwrap();
                assert_eq!(decode(&f, s).unwrap(), raw);
                assert!(f.len() < raw.len());
            }
        }
    }
}
