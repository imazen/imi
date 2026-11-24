//! Scalar reconstruction of SPEC 2.13, independent of the production packet core.
use crate::{
    bits::Reader,
    error::{Error, Result},
    header::Header,
    image::Image,
};
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode(
    data: &[u8],
    header: &Header,
    bit_smarts: bool,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    image: &mut Image,
) -> Result<()> {
    let mode = *data
        .first()
        .ok_or_else(|| Error::corrupt("truncated quick mode"))?;
    if mode == 2 {
        if !bit_smarts {
            return Err(Error::corrupt("bit planes without declaration"));
        }
        return decode_planes(&data[1..], header, x, y, width, height, image);
    }
    if mode > 1 {
        return Err(Error::unsupported("quick mode"));
    }
    values(
        &data[1..],
        mode,
        u64::from(width) * u64::from(height),
        header.sample.bytes(),
        header.channels as usize,
        |ordinal, c, v| {
            image.put_sample(
                x + (ordinal % u64::from(width)) as u32,
                y + (ordinal / u64::from(width)) as u32,
                c,
                v,
            );
            Ok(())
        },
    )
}

fn values(
    data: &[u8],
    mode: u8,
    count: u64,
    size: usize,
    channels: usize,
    mut emit: impl FnMut(u64, usize, u64) -> Result<()>,
) -> Result<()> {
    let mut r = Reader::new(data);
    let mask = if size == 8 {
        u64::MAX
    } else {
        (1u64 << (8 * size)) - 1
    };
    let mut previous = [0, 0, 0, mask];
    let mut cache = [[0u64; 4]; 64];
    let mut produced = 0;
    let word = |r: &mut Reader<'_>| -> Result<u64> {
        let mut bytes = [0; 8];
        bytes[..size].copy_from_slice(r.take(size, "quick word")?);
        Ok(u64::from_le_bytes(bytes))
    };
    while produced < count {
        let mut run = 1;
        if mode == 0 {
            for c in 0..channels {
                emit(produced, c, word(&mut r)?)?;
            }
            produced += 1;
            continue;
        }
        let tag = r.u8("quick packet")?;
        if tag >= 254 {
            previous[0] = word(&mut r)?;
            if channels < 3 {
                previous[1] = previous[0];
                previous[2] = previous[0];
            } else {
                previous[1] = word(&mut r)?;
                previous[2] = word(&mut r)?;
            }
            if tag == 255 {
                previous[3] = word(&mut r)?;
            }
        } else {
            match tag >> 6 {
                0 => previous = cache[tag as usize],
                1 => {
                    for (c, shift) in [4, 2, 0].into_iter().enumerate() {
                        previous[c] = previous[c]
                            .wrapping_add(u64::from((tag >> shift) & 3))
                            .wrapping_sub(2)
                            & mask;
                    }
                }
                2 => {
                    let other = r.u8("quick luma")?;
                    let green = i64::from(tag & 63) - 32;
                    for (c, delta) in [
                        green + i64::from(other >> 4) - 8,
                        green,
                        green + i64::from(other & 15) - 8,
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        previous[c] = previous[c].wrapping_add(delta as u64) & mask;
                    }
                }
                _ => run = u64::from(tag & 63) + 1,
            }
        }
        if run > count - produced {
            return Err(Error::corrupt("quick run exceeds unit"));
        }
        if channels < 3 && (previous[0] != previous[1] || previous[0] != previous[2]) {
            return Err(Error::corrupt("quick gray tuple"));
        }
        let index: u64 = previous
            .iter()
            .zip([3, 5, 7, 11])
            .map(|(v, weight)| {
                v.to_le_bytes()[..size]
                    .iter()
                    .map(|&b| u64::from(b))
                    .sum::<u64>()
                    * weight
            })
            .sum();
        cache[index as usize % 64] = previous;
        for ordinal in produced..produced + run {
            for c in 0..channels {
                let component = if channels == 2 && c == 1 { 3 } else { c };
                emit(ordinal, c, previous[component])?;
            }
        }
        produced += run;
    }
    if r.pos != data.len() {
        return Err(Error::corrupt("quick unit trailing bytes"));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn decode_planes(
    data: &[u8],
    header: &Header,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    image: &mut Image,
) -> Result<()> {
    let n = header.sample.bytes() * header.channels as usize;
    let count = usize::try_from(u64::from(width) * u64::from(height))
        .map_err(|_| Error::corrupt("plane count"))?;
    let mut r = Reader::new(data);
    let mut lengths = [0usize; 32];
    for length in &mut lengths[..n] {
        *length = usize::try_from(r.varint("plane length")?)
            .map_err(|_| Error::corrupt("plane length"))?;
    }
    let mut planes: Vec<Vec<u8>> = Vec::new();
    planes
        .try_reserve_exact(n)
        .map_err(|_| Error::corrupt("plane allocation"))?;
    for length in &lengths[..n] {
        let data = r.take(*length, "plane")?;
        let (&kind, body) = data
            .split_first()
            .ok_or_else(|| Error::corrupt("empty plane"))?;
        let mut out = Vec::new();
        out.try_reserve_exact(count)
            .map_err(|_| Error::corrupt("plane allocation"))?;
        out.resize(count, 0u8);
        let expand = |code: u8, base: u8, mask: u8| {
            let mut value = base;
            let mut src = 0;
            for bit in 0..8 {
                if mask & (1 << bit) != 0 {
                    value |= ((code >> src) & 1) << bit;
                    src += 1;
                }
            }
            value
        };
        match kind {
            0 => {
                if body.len() != count {
                    return Err(Error::corrupt("raw plane extent"));
                }
                out.copy_from_slice(body);
            }
            1 => values(body, 1, count as u64, 1, 1, |i, _, v| {
                out[i as usize] = v as u8;
                Ok(())
            })?,
            2 => {
                if body.len() != 1 {
                    return Err(Error::corrupt("constant plane extent"));
                }
                out.fill(body[0]);
            }
            3 | 4 => {
                let base = *body.first().ok_or_else(|| Error::corrupt("plane mask"))?;
                let mask = *body.get(1).ok_or_else(|| Error::corrupt("plane mask"))?;
                let bits = mask.count_ones() as usize;
                if !(1..=7).contains(&bits) || base & mask != 0 {
                    return Err(Error::corrupt("plane mask"));
                }
                let codes = &body[2..];
                if kind == 3 {
                    let total = count
                        .checked_mul(bits)
                        .ok_or_else(|| Error::corrupt("masked plane size"))?;
                    if codes.len() != total.div_ceil(8) {
                        return Err(Error::corrupt("masked plane extent"));
                    }
                    if total % 8 != 0 && codes[codes.len() - 1] >> (total % 8) != 0 {
                        return Err(Error::corrupt("masked plane padding"));
                    }
                    for (i, v) in out.iter_mut().enumerate() {
                        let mut code = 0;
                        for j in 0..bits {
                            let bit = i * bits + j;
                            code |= ((codes[bit / 8] >> (bit % 8)) & 1) << j;
                        }
                        *v = expand(code, base, mask);
                    }
                } else {
                    values(codes, 1, count as u64, 1, 1, |i, _, v| {
                        if v >= (1 << bits) {
                            return Err(Error::corrupt("mapped packet value"));
                        }
                        out[i as usize] = expand(v as u8, base, mask);
                        Ok(())
                    })?;
                }
            }
            5 | 6 => {
                let entries = usize::from(
                    *body
                        .first()
                        .ok_or_else(|| Error::corrupt("plane dictionary"))?,
                ) + 1;
                if entries < 2 {
                    return Err(Error::corrupt("plane dictionary"));
                }
                let table = body
                    .get(1..1 + entries)
                    .ok_or_else(|| Error::corrupt("plane dictionary"))?;
                if table.windows(2).any(|v| v[0] >= v[1]) {
                    return Err(Error::corrupt("plane dictionary order"));
                }
                let bits = (usize::BITS - (entries - 1).leading_zeros()) as usize;
                let codes = &body[1 + entries..];
                if kind == 5 {
                    let total = count
                        .checked_mul(bits)
                        .ok_or_else(|| Error::corrupt("dictionary plane size"))?;
                    if codes.len() != total.div_ceil(8) {
                        return Err(Error::corrupt("dictionary plane extent"));
                    }
                    if total % 8 != 0 && codes[codes.len() - 1] >> (total % 8) != 0 {
                        return Err(Error::corrupt("dictionary plane padding"));
                    }
                    for (i, v) in out.iter_mut().enumerate() {
                        let mut code = 0usize;
                        for j in 0..bits {
                            let bit = i * bits + j;
                            code |= usize::from((codes[bit / 8] >> (bit % 8)) & 1) << j;
                        }
                        *v = *table
                            .get(code)
                            .ok_or_else(|| Error::corrupt("mapped plane value"))?;
                    }
                } else {
                    values(codes, 1, count as u64, 1, 1, |i, _, v| {
                        out[i as usize] = *table
                            .get(v as usize)
                            .ok_or_else(|| Error::corrupt("mapped packet value"))?;
                        Ok(())
                    })?;
                }
            }
            7 => {
                if body.len() != 1 || usize::from(body[0]) >= planes.len() {
                    return Err(Error::corrupt("plane reference"));
                }
                out.copy_from_slice(&planes[usize::from(body[0])]);
            }
            _ => return Err(Error::unsupported("quick plane mode")),
        }
        planes.push(out);
    }
    if r.remaining() != 0 {
        return Err(Error::corrupt("trailing plane bytes"));
    }
    // The first plane supplies the exact admitted pixel ordinal domain.
    for (i, _) in planes[0].iter().enumerate() {
        for c in 0..header.channels as usize {
            let mut value = 0u64;
            for byte in 0..header.sample.bytes() {
                value |= u64::from(planes[c * header.sample.bytes() + byte][i]) << (8 * byte);
            }
            image.put_sample(
                x + (i % width as usize) as u32,
                y + (i / width as usize) as u32,
                c,
                value,
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod plane_tests {
    use super::*;
    fn header(channels: u8) -> Header {
        Header {
            version: 3,
            features: (1 << 22) | (1 << 23),
            set: (1 << 22) | (1 << 23),
            sample: crate::header::SampleType::U8,
            sample_code: 0,
            channels,
            channel_layout: 0,
            width: 2,
            height: 3,
        }
    }
    fn file(bodies: &[&[u8]]) -> Vec<u8> {
        let mut out = vec![2];
        for b in bodies {
            assert!(b.len() < 128);
            out.push(b.len() as u8);
        }
        for b in bodies {
            out.extend_from_slice(b);
        }
        out
    }
    #[test]
    fn independent_all_eight_plane_bodies_and_exact_extents() {
        let cases: &[(&[u8], &[u8])] = &[
            (&[0, 10, 20, 20, 20, 20, 20], &[10, 20, 20, 20, 20, 20]),
            (&[1, 254, 10, 254, 20, 195], &[10, 20, 20, 20, 20, 20]),
            (&[2, 20], &[20; 6]),
            (&[3, 2, 0xa0, 0xa9, 0x0a], &[34, 130, 130, 130, 130, 130]),
            (
                &[4, 2, 0xa0, 254, 1, 127, 195],
                &[34, 130, 130, 130, 130, 130],
            ),
            (&[5, 2, 10, 20, 99, 0xa4, 0x0a], &[10, 20, 99, 99, 99, 99]),
            (
                &[6, 2, 10, 20, 99, 254, 0, 127, 195],
                &[10, 20, 20, 20, 20, 20],
            ),
        ];
        for &(body, expected) in cases {
            let h = header(1);
            let mut image = Image::new(2, 3, 1, 0, 1).unwrap();
            decode(&file(&[body]), &h, true, 0, 0, 2, 3, &mut image).unwrap();
            assert_eq!(image.buf, expected);
        }
        let mut full = vec![5, 255];
        full.extend(0u8..=255);
        full.extend([255, 1, 2, 3, 4, 5]);
        for kind in [5, 6] {
            if kind == 6 {
                full[0] = 6;
                full.truncate(258);
                full.extend([254, 255, 254, 1, 127, 127, 127, 127]);
            }
            // Multi-byte minimal length encoding is independent of production helpers.
            let mut encoded = vec![2];
            let mut len = full.len();
            while len >= 128 {
                encoded.push(len as u8 | 128);
                len >>= 7;
            }
            encoded.push(len as u8);
            encoded.extend(&full);
            let mut image = Image::new(2, 3, 1, 0, 1).unwrap();
            decode(&encoded, &header(1), true, 0, 0, 2, 3, &mut image).unwrap();
            assert_eq!(image.buf, [255, 1, 2, 3, 4, 5]);
        }
        let h = header(2);
        let mut image = Image::new(2, 3, 2, 0, 1).unwrap();
        decode(
            &file(&[cases[0].0, &[7, 0]]),
            &h,
            true,
            0,
            0,
            2,
            3,
            &mut image,
        )
        .unwrap();
        assert_eq!(
            image.buf,
            cases[0].1.iter().flat_map(|v| [*v, *v]).collect::<Vec<_>>()
        );
        for bad in [
            &[7, 0][..],
            &[5, 2, 10, 20, 99, 0xff, 0x0a],
            &[5, 1, 20, 10, 0],
            &[3, 2, 3, 0, 0],
            &[8],
        ] {
            let mut image = Image::new(2, 3, 1, 0, 1).unwrap();
            assert!(decode(&file(&[bad]), &header(1), true, 0, 0, 2, 3, &mut image).is_err());
        }
        let mut image = Image::new(2, 3, 1, 0, 1).unwrap();
        assert!(
            decode(
                &file(&[cases[0].0]),
                &header(1),
                false,
                0,
                0,
                2,
                3,
                &mut image
            )
            .is_err()
        );
    }
}
