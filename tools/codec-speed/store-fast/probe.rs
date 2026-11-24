//! Experimental payloads, NOT IMI wire syntax or a public codec API.
#![forbid(unsafe_code)]
use std::{fs, hint::black_box, time::Instant};
type Result<T> = std::result::Result<T, &'static str>;
#[derive(Clone, Copy)]
struct Image {
    width: usize,
    height: usize,
    channels: usize,
    bytes: usize,
}
impl Image {
    fn row(self) -> Result<usize> {
        if self.width == 0
            || self.height == 0
            || !(1..=4).contains(&self.channels)
            || ![1, 2, 4, 8].contains(&self.bytes)
        {
            return Err("geometry");
        }
        self.width
            .checked_mul(self.channels)
            .and_then(|n| n.checked_mul(self.bytes))
            .ok_or("row overflow")
    }
    fn check(self, data: &[u8], stride: usize) -> Result<usize> {
        let row = self.row()?;
        let end = (self.height - 1)
            .checked_mul(stride)
            .and_then(|n| n.checked_add(row))
            .ok_or("raster overflow")?;
        if stride < row || data.len() < end {
            return Err("raster bounds");
        }
        Ok(row)
    }
}
fn word(p: &[u8]) -> u64 {
    let mut a = [0; 8];
    a[..p.len()].copy_from_slice(p);
    u64::from_le_bytes(a)
}
fn putword(v: u64, p: &mut [u8]) {
    p.copy_from_slice(&v.to_le_bytes()[..p.len()]);
}
fn transform(row: &mut [u8], im: Image, mode: u8, inverse: bool) {
    if mode == 4 {
        for p in row.as_chunks_mut::<3>().0 {
            if inverse {
                p[0] = p[0].wrapping_add(p[1]);
                p[2] = p[2].wrapping_add(p[1]);
            } else {
                p[0] = p[0].wrapping_sub(p[1]);
                p[2] = p[2].wrapping_sub(p[1]);
            }
        }
    } else if mode == 5 {
        let bpp = im.channels * im.bytes;
        if inverse {
            for i in bpp..row.len() {
                row[i] ^= row[i - bpp];
            }
        } else {
            for i in (bpp..row.len()).rev() {
                row[i] ^= row[i - bpp];
            }
        }
    }
}
// LSB-first packing; raw sample patterns, including float NaNs, are integers here.
fn pack(row: &[u8], im: Image, out: &mut Vec<u8>) {
    out.clear();
    let bpp = im.channels * im.bytes;
    for c in 0..im.channels {
        let mut lo = u64::MAX;
        let mut hi = 0;
        let mut union = 0;
        for p in row.chunks_exact(bpp) {
            let v = word(&p[c * im.bytes..(c + 1) * im.bytes]);
            lo = lo.min(v);
            hi = hi.max(v);
            union |= v;
        }
        let shift = union.trailing_zeros().min(63);
        let bits = 64 - ((hi - lo) >> shift).leading_zeros();
        out.extend_from_slice(&lo.to_le_bytes()[..im.bytes]);
        out.push(shift as u8);
        out.push(bits as u8);
        let mut acc = 0u128;
        let mut used = 0;
        for p in row.chunks_exact(bpp) {
            let v = (word(&p[c * im.bytes..(c + 1) * im.bytes]) - lo) >> shift;
            acc |= (v as u128) << used;
            used += bits;
            while used >= 8 {
                out.push(acc as u8);
                acc >>= 8;
                used -= 8;
            }
        }
        if used > 0 {
            out.push(acc as u8);
        }
    }
}
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or("length overflow")?;
        let p = self.data.get(self.pos..end).ok_or("truncated")?;
        self.pos = end;
        Ok(p)
    }
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn done(&self) -> Result<()> {
        if self.pos == self.data.len() {
            Ok(())
        } else {
            Err("trailing data")
        }
    }
}
fn unpack(data: &[u8], row: &mut [u8], im: Image) -> Result<()> {
    let mut r = Reader { data, pos: 0 };
    let bpp = im.channels * im.bytes;
    for c in 0..im.channels {
        let base = word(r.take(im.bytes)?);
        let shift = r.byte()? as u32;
        let bits = r.byte()? as u32;
        if shift > 63 || bits > 64 || (bits > 0 && bits + shift > im.bytes as u32 * 8) {
            return Err("packed width");
        }
        let mask = if bits == 64 {
            u64::MAX
        } else {
            (1u64 << bits) - 1
        };
        let mut acc = 0u128;
        let mut used = 0;
        for p in row.chunks_exact_mut(bpp) {
            while used < bits {
                acc |= (r.byte()? as u128) << used;
                used += 8;
            }
            let code = acc as u64 & mask;
            acc >>= bits;
            used -= bits;
            let value = (code as u128)
                .checked_shl(shift)
                .and_then(|n| n.checked_add(base as u128))
                .ok_or("packed value")?;
            let max = if im.bytes == 8 {
                u64::MAX as u128
            } else {
                (1u128 << (im.bytes * 8)) - 1
            };
            if value > max {
                return Err("packed overflow");
            }
            putword(value as u64, &mut p[c * im.bytes..(c + 1) * im.bytes]);
        }
        if acc != 0 {
            return Err("packed padding");
        }
    }
    r.done()
}
// QOI-inspired RGB byte differences, green-relative differences and repeats.
// Other pixel widths use only exact whole-pixel runs and literal packets.
fn packets(row: &[u8], bpp: usize, out: &mut Vec<u8>) {
    out.clear();
    let mut previous = [0u8; 32];
    let mut x = 0;
    let mut literal = None;
    while x < row.len() / bpp {
        let p = &row[x * bpp..(x + 1) * bpp];
        let mut token = [0u8; 2];
        let mut size = 0;
        let mut advance = 1;
        if p == &previous[..bpp] {
            while advance < 64
                && (x + advance) * bpp < row.len()
                && &row[(x + advance) * bpp..(x + advance + 1) * bpp] == p
            {
                advance += 1;
            }
            token[0] = 0x80 | (advance - 1) as u8;
            size = 1;
        } else if bpp == 3 {
            let dr = p[0].wrapping_sub(previous[0]) as i8 as i16;
            let dg = p[1].wrapping_sub(previous[1]) as i8 as i16;
            let db = p[2].wrapping_sub(previous[2]) as i8 as i16;
            if (-2..=1).contains(&dr) && (-2..=1).contains(&dg) && (-2..=1).contains(&db) {
                token[0] = ((dr + 2) << 4 | (dg + 2) << 2 | (db + 2)) as u8;
                size = 1;
            } else if (-32..=31).contains(&dg)
                && (-8..=7).contains(&(dr - dg))
                && (-8..=7).contains(&(db - dg))
            {
                token[0] = 0x40 | (dg + 32) as u8;
                token[1] = ((dr - dg + 8) << 4 | (db - dg + 8)) as u8;
                size = 2;
            }
        }
        if size > 0 {
            literal = None;
            out.extend_from_slice(&token[..size]);
        } else {
            let (at, count) = literal.unwrap_or_else(|| {
                let at = out.len();
                out.push(0xc0);
                (at, 0)
            });
            out[at] = 0xc0 | count;
            out.extend_from_slice(p);
            literal = if count == 63 {
                None
            } else {
                Some((at, count + 1))
            };
        }
        previous[..bpp].copy_from_slice(p);
        x += advance;
    }
}
fn depacket(data: &[u8], row: &mut [u8], bpp: usize) -> Result<()> {
    let mut r = Reader { data, pos: 0 };
    let mut previous = [0u8; 32];
    let mut x = 0;
    while x < row.len() {
        let tag = r.byte()?;
        match tag >> 6 {
            0 | 1 => {
                if bpp != 3 {
                    return Err("RGB packet on other format");
                }
                let (dr, dg, db) = if tag >> 6 == 0 {
                    (
                        ((tag >> 4) & 3) as i16 - 2,
                        ((tag >> 2) & 3) as i16 - 2,
                        (tag & 3) as i16 - 2,
                    )
                } else {
                    let t = r.byte()?;
                    let g = (tag & 63) as i16 - 32;
                    (g + (t >> 4) as i16 - 8, g, g + (t & 15) as i16 - 8)
                };
                previous[0] = previous[0].wrapping_add(dr as u8);
                previous[1] = previous[1].wrapping_add(dg as u8);
                previous[2] = previous[2].wrapping_add(db as u8);
                row[x..x + 3].copy_from_slice(&previous[..3]);
                x += 3;
            }
            2 | 3 => {
                let n = ((tag & 63) as usize + 1) * bpp;
                let target = row.get_mut(x..x + n).ok_or("packet exceeds row")?;
                if tag >> 6 == 2 {
                    for p in target.chunks_exact_mut(bpp) {
                        p.copy_from_slice(&previous[..bpp]);
                    }
                } else {
                    target.copy_from_slice(r.take(n)?);
                    previous[..bpp].copy_from_slice(&target[n - bpp..]);
                }
                x += n;
            }
            _ => unreachable!(),
        }
    }
    r.done()
}
fn encode(src: &[u8], stride: usize, im: Image, mode: u8) -> Result<Vec<u8>> {
    let row = im.check(src, stride)?;
    if mode > 5 || (mode == 4 && (im.channels != 3 || im.bytes != 1)) {
        return Err("mode");
    }
    let width = u32::try_from(im.width).map_err(|_| "width")?;
    let height = u32::try_from(im.height).map_err(|_| "height")?;
    let capacity = row
        .checked_add(4)
        .and_then(|n| n.checked_mul(im.height))
        .and_then(|n| n.checked_add(16))
        .ok_or("output overflow")?;
    let mut out = Vec::new();
    out.try_reserve_exact(capacity).map_err(|_| "allocation")?;
    out.extend_from_slice(b"SPX1");
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&[im.channels as u8, im.bytes as u8, mode, 0]);
    let mut packed = Vec::new();
    let mut scratch = vec![0; row];
    for y in 0..im.height {
        let p = &src[y * stride..y * stride + row];
        if mode == 0 {
            out.extend_from_slice(p);
            continue;
        }
        if y > 0 && p == &src[(y - 1) * stride..(y - 1) * stride + row] {
            out.extend_from_slice(&0u32.to_le_bytes());
            continue;
        }
        if mode == 2 {
            packets(p, im.channels * im.bytes, &mut packed);
        } else if mode >= 3 {
            scratch.copy_from_slice(p);
            transform(&mut scratch, im, mode, false);
            pack(&scratch, im, &mut packed);
        }
        let compressed = mode >= 2 && packed.len() < row;
        let data = if compressed { &packed[..] } else { p };
        let len = u32::try_from(data.len())
            .map_err(|_| "row length")?
            .checked_mul(2)
            .ok_or("row length")?
            | u32::from(compressed);
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(data);
    }
    Ok(out)
}
fn header(data: &[u8]) -> Result<(Image, u8, Reader<'_>)> {
    let mut r = Reader { data, pos: 0 };
    if r.take(4)? != b"SPX1" {
        return Err("magic");
    }
    let width = r.u32()? as usize;
    let height = r.u32()? as usize;
    let im = Image {
        width,
        height,
        channels: r.byte()? as usize,
        bytes: r.byte()? as usize,
    };
    let mode = r.byte()?;
    if r.byte()? != 0 || mode > 5 || (mode == 4 && (im.channels != 3 || im.bytes != 1)) {
        return Err("mode");
    }
    im.row()?;
    Ok((im, mode, r))
}
fn decode_into(data: &[u8], out: &mut [u8], stride: usize) -> Result<()> {
    let (im, mode, mut r) = header(data)?;
    let row = im.check(out, stride)?;
    for y in 0..im.height {
        if mode == 0 {
            out[y * stride..y * stride + row].copy_from_slice(r.take(row)?);
            continue;
        }
        let record = r.u32()?;
        let n = (record >> 1) as usize;
        if record == 0 {
            if y == 0 {
                return Err("first row repeats");
            }
            out.copy_within((y - 1) * stride..(y - 1) * stride + row, y * stride);
            continue;
        }
        let src = r.take(n)?;
        let target = &mut out[y * stride..y * stride + row];
        if record & 1 == 0 {
            if n != row {
                return Err("raw row size");
            }
            target.copy_from_slice(src);
        } else if mode == 2 {
            depacket(src, target, im.channels * im.bytes)?;
        } else if mode >= 3 {
            unpack(src, target, im)?;
            transform(target, im, mode, true);
        } else {
            return Err("compressed row mode");
        }
    }
    r.done()
}
fn decode(data: &[u8]) -> Result<Vec<u8>> {
    let (im, _, _) = header(data)?;
    let row = im.row()?;
    let n = row.checked_mul(im.height).ok_or("output size")?;
    // This is a bounded experiment driver, not a production admission API.
    if n > 256 * 1024 * 1024 {
        return Err("experiment output cap");
    }
    let mut out = vec![0; n];
    decode_into(data, &mut out, row)?;
    Ok(out)
}
fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut a: Vec<_> = std::env::args().collect();
    if a.len() == 5 {
        let fields: Vec<String> = fs::read_to_string(&a[2])?
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        if fields.len() != 4 {
            return Err("geometry file requires width height channels sample_bytes".into());
        }
        let mut expanded = vec![a[0].clone(), a[1].clone()];
        expanded.extend(fields);
        expanded.extend_from_slice(&a[3..]);
        a = expanded;
    }
    if a.len() != 8 {
        return Err("probe RAW WIDTH HEIGHT CHANNELS SAMPLE_BYTES MODE OUTPUT".into());
    }
    let im = Image {
        width: a[2].parse()?,
        height: a[3].parse()?,
        channels: a[4].parse()?,
        bytes: a[5].parse()?,
    };
    let mode = a[6].parse()?;
    let raw = fs::read(&a[1])?;
    let row = im.row()?;
    let encoded = encode(&raw, row, im, mode)?;
    if decode(&encoded)? != raw {
        return Err("warmup pixels".into());
    }
    for _ in 0..3 {
        black_box(decode(&encoded)?);
    }
    let start = Instant::now();
    let output = black_box(encode(black_box(&raw), row, im, mode)?);
    let encode_ns = start.elapsed().as_nanos();
    if output != encoded {
        return Err("encode determinism".into());
    }
    let start = Instant::now();
    let decoded = black_box(decode(black_box(&encoded))?);
    let decode_ns = start.elapsed().as_nanos();
    if decoded != raw {
        return Err("timed pixels".into());
    }
    if a[7] != "-" {
        fs::write(&a[7], &encoded)?;
    }
    println!(
        "encode_ns={encode_ns}\ndecode_ns={decode_ns}\ncheck={{\"raw_bytes\":{}}}",
        raw.len()
    );
    println!(
        "{{\"encode_ns\":{encode_ns},\"decode_ns\":{decode_ns},\"encoded_bytes\":{},\"raw_bytes\":{}}}",
        encoded.len(),
        raw.len()
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_storage_width_channel_stride_and_mode_preserves_bits() {
        for bytes in [1, 2, 4, 8] {
            for channels in 1..=4 {
                for width in [1, 3, 17, 129] {
                    let im = Image {
                        width,
                        height: 7,
                        channels,
                        bytes,
                    };
                    let row = im.row().unwrap();
                    let stride = row + 13;
                    let mut data = vec![0xa5; stride * im.height];
                    let mut seed = 0x7138u32;
                    for y in 0..im.height {
                        for x in 0..row {
                            seed ^= seed << 13;
                            seed ^= seed >> 17;
                            seed ^= seed << 5;
                            data[y * stride + x] = if y == 0 {
                                0
                            } else if y == 1 {
                                255
                            } else {
                                seed as u8
                            };
                        }
                    }
                    data.copy_within(2 * stride..2 * stride + row, 3 * stride);
                    for mode in 0..=5 {
                        if mode == 4 && (bytes != 1 || channels != 3) {
                            continue;
                        }
                        let encoded = encode(&data, stride, im, mode).unwrap();
                        let mut out = vec![0xcc; stride * im.height];
                        decode_into(&encoded, &mut out, stride).unwrap();
                        for y in 0..im.height {
                            assert_eq!(
                                &out[y * stride..y * stride + row],
                                &data[y * stride..y * stride + row]
                            );
                            assert!(
                                out[y * stride + row..(y + 1) * stride]
                                    .iter()
                                    .all(|&v| v == 0xcc)
                            );
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn arbitrary_depths_offsets_and_zero_low_bits_are_exact() {
        for bytes in [1, 2, 4, 8] {
            for bits in 1..=bytes * 8 {
                for shift in [0, (bytes * 8 - bits) / 2] {
                    let im = Image {
                        width: 257,
                        height: 2,
                        channels: 1,
                        bytes,
                    };
                    let mut row = vec![0; im.row().unwrap() * 2];
                    let mask = if bits == 64 {
                        u64::MAX
                    } else {
                        (1u64 << bits) - 1
                    };
                    for (i, p) in row.chunks_exact_mut(bytes).enumerate() {
                        putword(((i as u64).wrapping_mul(743) & mask) << shift, p);
                    }
                    for mode in [3, 5] {
                        let enc = encode(&row, im.row().unwrap(), im, mode).unwrap();
                        assert_eq!(decode(&enc).unwrap(), row);
                    }
                }
            }
        }
    }
    #[test]
    fn float_special_values_are_opaque_bits() {
        let bits = [
            0u64,
            1 << 63,
            0x7ff0000000000000,
            0xfff0000000000000,
            0x7ff0000000000001,
            0x7ff80000deadbeef,
            1,
            u64::MAX,
        ];
        let raw: Vec<_> = bits.iter().flat_map(|v| v.to_le_bytes()).collect();
        let im = Image {
            width: 8,
            height: 1,
            channels: 1,
            bytes: 8,
        };
        for mode in [0, 1, 2, 3, 5] {
            assert_eq!(decode(&encode(&raw, 64, im, mode).unwrap()).unwrap(), raw);
        }
    }
    #[test]
    fn packet_boundaries_and_wrapping_deltas() {
        let mut row = vec![0; 3 * 193];
        for (i, p) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let v = (i / 65) as u8;
            p.copy_from_slice(&[v.wrapping_sub(1), v, v.wrapping_add(1)]);
        }
        let mut packed = Vec::new();
        packets(&row, 3, &mut packed);
        let mut out = vec![0; row.len()];
        depacket(&packed, &mut out, 3).unwrap();
        assert_eq!(out, row);
    }
    #[test]
    fn truncation_trailing_repeat_and_invalid_width_fail() {
        let im = Image {
            width: 5,
            height: 2,
            channels: 3,
            bytes: 1,
        };
        let raw = vec![0; 30];
        for mode in 0..=5 {
            let mut enc = encode(&raw, 15, im, mode).unwrap();
            for end in 0..enc.len() {
                assert!(decode(&enc[..end]).is_err());
            }
            enc.push(0);
            assert!(decode(&enc).is_err());
        }
        let mut enc = encode(&raw, 15, im, 1).unwrap();
        enc[16..20].copy_from_slice(&0u32.to_le_bytes());
        assert!(decode(&enc).is_err());
        let mut out = [0; 5];
        assert!(
            unpack(
                &[0, 0, 65],
                &mut out,
                Image {
                    width: 5,
                    height: 1,
                    channels: 1,
                    bytes: 1
                }
            )
            .is_err()
        );
        assert!(
            unpack(
                &[255, 0, 1, 1],
                &mut out,
                Image {
                    width: 5,
                    height: 1,
                    channels: 1,
                    bytes: 1
                }
            )
            .is_err()
        );
    }
}
